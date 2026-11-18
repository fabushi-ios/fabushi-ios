use crate::server::{AppState, AuthBearer};
use axum::{
    body::Bytes,
    extract::{FromRequestParts, Path, State},
    http::{request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use gpui_telegram_agent::tasks::{ApprovalDecision, ApprovalGate};
use gpui_telegram_core::action_bus::{ActionContext, ActionError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub struct OptionalAuth(pub Option<AuthBearer>);

impl FromRequestParts<Arc<AppState>> for OptionalAuth {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let auth = AuthBearer::from_request_parts(parts, state).await.ok();
        Ok(OptionalAuth(auth))
    }
}

fn canonicalize_json(val: &Value) -> Value {
    match val {
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), canonicalize_json(v));
            }
            let mut out = serde_json::Map::new();
            for (k, v) in sorted {
                out.insert(k, v);
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canonicalize_json).collect()),
        other => other.clone(),
    }
}

fn canonical_params_hash(user_id: u64, action_name: &str, input: &Value) -> String {
    let canonical = canonicalize_json(input);
    let canonical_str = serde_json::to_string(&canonical).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(format!("{user_id}:{action_name}:{canonical_str}").as_bytes());
    format!("{:x}", hasher.finalize())
}

fn hmac_sha256_hex(secret: &[u8; 32], message: &[u8]) -> String {
    const BLOCK_SIZE: usize = 64;
    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..32 {
        ipad[i] ^= secret[i];
        opad[i] ^= secret[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    format!("{:x}", outer.finalize())
}

fn sign_approval_token(
    secret: &[u8; 32],
    gate_id: &str,
    user_id: u64,
    action_name: &str,
    params_hash: &str,
) -> String {
    let msg = format!("{gate_id}:{user_id}:{action_name}:{params_hash}");
    let sig = hmac_sha256_hex(secret, msg.as_bytes());
    format!("{gate_id}.{sig}")
}

fn verify_approval_signature(
    secret: &[u8; 32],
    token: &str,
    user_id: u64,
    action_name: &str,
    params_hash: &str,
) -> Option<String> {
    let (gate_id, sig_hex) = token.trim().split_once('.')?;
    if gate_id.is_empty() || sig_hex.len() != 64 {
        return None;
    }
    let msg = format!("{gate_id}:{user_id}:{action_name}:{params_hash}");
    let expected = hmac_sha256_hex(secret, msg.as_bytes());
    let valid: bool = sig_hex.as_bytes().ct_eq(expected.as_bytes()).into();
    if valid {
        Some(gate_id.to_string())
    } else {
        None
    }
}

async fn load_gate_cached(
    state: &AppState,
    gate_id: &str,
    refresh_if_undecided: bool,
) -> Option<ApprovalGate> {
    let cached = {
        let gates = state.approval_gates.read().await;
        gates.get(gate_id).cloned()
    };

    if let Some(ref gate) = cached {
        if !(refresh_if_undecided && gate.decision.is_none()) {
            return Some(gate.clone());
        }
    }

    let db = state.db.clone();
    let gid = gate_id.to_string();
    let loaded = tokio::task::spawn_blocking(move || db.get_approval_gate(&gid))
        .await
        .ok()
        .and_then(|res| res.ok())
        .flatten();

    if let Some(loaded_gate) = loaded {
        let mut gates = state.approval_gates.write().await;
        match gates.get_mut(gate_id) {
            Some(existing) => {
                if existing.decision.is_none() && loaded_gate.decision.is_some() {
                    *existing = loaded_gate.clone();
                    return Some(loaded_gate);
                }
                return Some(existing.clone());
            }
            None => {
                gates.insert(gate_id.to_string(), loaded_gate.clone());
                return Some(loaded_gate);
            }
        }
    }

    cached
}

pub async fn handle_action_post(
    OptionalAuth(auth): OptionalAuth,
    State(state): State<Arc<AppState>>,
    Path(action_name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let action = match state.action_registry.get(&action_name) {
        Some(a) => a,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "success": false,
                    "error": "action_not_found",
                    "message": format!("Action '{action_name}' not found")
                })),
            )
                .into_response();
        }
    };

    // Require authenticated session for all non-ReadOnly actions
    if !action.risk_level().is_read_only() && auth.is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "success": false,
                "error": "unauthorized",
                "message": format!("Action '{action_name}' requires an authenticated session")
            })),
        )
            .into_response();
    }

    let input_val = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "success": false,
                        "error": "invalid_json",
                        "message": format!("Malformed JSON payload: {e}")
                    })),
                )
                    .into_response();
            }
        }
    };

    let mut ctx = ActionContext::new();
    if let Some(ref a) = auth {
        ctx = ctx.with_user(a.user_id.0);
        ctx.session_id = Some(a.session.session_id.clone());
    }

    if action.risk_level().requires_explicit_confirmation() {
        let Some(ref caller) = auth else {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "success": false,
                    "error": "unauthorized",
                    "message": format!("Action '{action_name}' requires an authenticated session")
                })),
            )
                .into_response();
        };

        let user_id = caller.user_id.0;
        let expected_task_id = format!("action:{user_id}:{action_name}");
        let params_hash = canonical_params_hash(user_id, &action_name, &input_val);
        let now_ms = Utc::now().timestamp_millis().max(0) as u64;

        let mut approved_gate_valid = false;
        if let Some(raw_token) = headers.get("X-Approval-Token").and_then(|v| v.to_str().ok()) {
            if let Some(gate_id) = verify_approval_signature(
                &state.action_approval_secret,
                raw_token,
                user_id,
                &action_name,
                &params_hash,
            ) {
                if let Some(mut gate) = load_gate_cached(&state, &gate_id, true).await {
                    if matches!(
                        &gate.decision,
                        Some(ApprovalDecision::Deny { reason: Some(r) }) if r == "consumed"
                    ) && gate.tool_name == action_name
                        && gate.task_id == expected_task_id
                        && gate.params_summary == params_hash
                    {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({
                                "success": false,
                                "error": "approval_consumed",
                                "gate_id": gate_id,
                                "message": "Approval gate has already been consumed"
                            })),
                        )
                            .into_response();
                    }

                    let valid = gate.is_approved()
                        && !gate.is_denied()
                        && !gate.is_expired()
                        && now_ms < gate.expires_at_ms
                        && gate.tool_name == action_name
                        && gate.task_id == expected_task_id
                        && gate.params_summary == params_hash;
                    if valid {
                        if matches!(gate.decision, Some(ApprovalDecision::ApproveOnce)) {
                            // Atomically consume single-use gate via SQLite conditional UPDATE
                            // outside the global approval_gates write lock.
                            gate.decision = Some(ApprovalDecision::Deny {
                                reason: Some("consumed".into()),
                            });
                            let gate_val = serde_json::to_value(&gate).unwrap_or(Value::Null);
                            let db = state.db.clone();
                            let gid = gate.gate_id.clone();
                            let consume_res = tokio::task::spawn_blocking(move || {
                                db.try_consume_approval_gate(&gid, &gate_val)
                            })
                            .await;

                            match consume_res {
                                Ok(Ok(true)) => {
                                    state
                                        .approval_gates
                                        .write()
                                        .await
                                        .insert(gate_id.clone(), gate);
                                    approved_gate_valid = true;
                                }
                                Ok(Ok(false)) => {
                                    state
                                        .approval_gates
                                        .write()
                                        .await
                                        .insert(gate_id.clone(), gate);
                                    return (
                                        StatusCode::FORBIDDEN,
                                        Json(json!({
                                            "success": false,
                                            "error": "approval_consumed",
                                            "gate_id": gate_id,
                                            "message": "Approval gate has already been consumed"
                                        })),
                                    )
                                        .into_response();
                                }
                                Ok(Err(_)) | Err(_) => {
                                    return (
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        Json(json!({
                                            "success": false,
                                            "error": "approval_store_error",
                                            "message": "Failed to verify and consume approval gate"
                                        })),
                                    )
                                        .into_response();
                                }
                            }
                        } else {
                            approved_gate_valid = true;
                        }
                    }
                }
            }
        }

        if !approved_gate_valid {
            let gate_id = format!("actgate_{}", Uuid::new_v4().simple());
            let gate = ApprovalGate::new(
                gate_id.clone(),
                expected_task_id.clone(),
                action_name.clone(),
                params_hash,
            );
            let expires_at_ms = gate.expires_at_ms;
            let gate_val = serde_json::to_value(&gate).unwrap_or(Value::Null);
            let db = state.db.clone();
            let gid = gate_id.clone();
            let tid = expected_task_id.clone();
            let persist_res = tokio::task::spawn_blocking(move || {
                db.upsert_approval_gate(&gid, &tid, None, &gate_val)
            })
            .await;
            if !matches!(persist_res, Ok(Ok(()))) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "success": false,
                        "error": "approval_store_error",
                        "message": "Failed to persist approval gate"
                    })),
                )
                    .into_response();
            }
            state.approval_gates.write().await.insert(gate_id.clone(), gate);

            return (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "success": false,
                    "error": "Approval required",
                    "action": action_name,
                    "risk_level": action.risk_level(),
                    "gate_id": gate_id,
                    "expires_at_ms": expires_at_ms,
                    "message": format!("Action '{action_name}' requires explicit approval confirmation")
                })),
            )
                .into_response();
        }

        ctx = ctx.with_approval(true);
    }

    match action.execute(ctx, input_val).await {
        Ok(result) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "result": result
            })),
        )
            .into_response(),
        Err(err) => {
            let status = match err {
                ActionError::NotFound(_) => StatusCode::NOT_FOUND,
                ActionError::InvalidInput(_) => StatusCode::UNPROCESSABLE_ENTITY,
                ActionError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
                ActionError::Forbidden(_) => StatusCode::FORBIDDEN,
                ActionError::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
                ActionError::ExecutionFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
                ActionError::Serialization(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };

            (
                status,
                Json(json!({
                    "success": false,
                    "error": err.code(),
                    "message": err.to_string(),
                    "details": err.details()
                })),
            )
                .into_response()
        }
    }
}

async fn sync_gate_and_conflict_response(state: &AppState, gate_id: &str) -> Response {
    let db = state.db.clone();
    let gid = gate_id.to_string();
    let fetch_res = tokio::task::spawn_blocking(move || db.get_approval_gate(&gid)).await;
    match fetch_res {
        Ok(Ok(Some(fresh_gate))) => {
            let expired = fresh_gate.is_expired();
            let is_consumed = |g: &ApprovalGate| {
                matches!(
                    &g.decision,
                    Some(ApprovalDecision::Deny { reason: Some(r) }) if r == "consumed"
                )
            };
            {
                let mut gates = state.approval_gates.write().await;
                let keep_existing =
                    gates.get(gate_id).is_some_and(is_consumed) && !is_consumed(&fresh_gate);
                if !keep_existing {
                    gates.insert(gate_id.to_string(), fresh_gate);
                }
            }
            if expired {
                (
                    StatusCode::GONE,
                    Json(json!({
                        "success": false,
                        "error": "gate_expired",
                        "message": "Approval gate has expired"
                    })),
                )
                    .into_response()
            } else {
                (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "success": false,
                        "error": "already_decided",
                        "message": "Approval gate has already been decided or consumed"
                    })),
                )
                    .into_response()
            }
        }
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": "gate_not_found",
                "message": format!("Approval gate '{gate_id}' not found")
            })),
        )
            .into_response(),
        Ok(Err(_)) | Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "approval_store_error",
                "message": "Failed to refresh approval gate state"
            })),
        )
            .into_response(),
    }
}

pub async fn handle_action_gate_approve(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(gate_id): Path<String>,
    body: Bytes,
) -> Response {
    let payload: Value = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "success": false,
                        "error": "invalid_json",
                        "message": format!("Malformed JSON payload: {e}")
                    })),
                )
                    .into_response();
            }
        }
    };

    let Some(mut gate) = load_gate_cached(&state, &gate_id, true).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": "gate_not_found",
                "message": format!("Approval gate '{gate_id}' not found")
            })),
        )
            .into_response();
    };

    let approved_opt = payload.get("approved").and_then(|v| v.as_bool());
    let decision_raw = match payload.get("decision") {
        None | Some(Value::Null) => "",
        Some(Value::String(s)) => s.as_str(),
        Some(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": "invalid_decision",
                    "message": "Invalid decision. Supported decisions are 'approve_once' or 'deny'"
                })),
            )
                .into_response();
        }
    };
    let decision_str = decision_raw.trim().to_ascii_lowercase();
    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .map(String::from);

    let (decision, db_decision_label) = if decision_str == "always_allow" {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "success": false,
                "error": "always_allow_not_permitted",
                "message": "HTTP action approval tokens are strictly single-use; 'always_allow' is not permitted"
            })),
        )
            .into_response();
    } else if !decision_str.is_empty()
        && decision_str != "deny"
        && decision_str != "approve"
        && decision_str != "approve_once"
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "invalid_decision",
                "message": "Invalid decision. Supported decisions are 'approve_once' or 'deny'"
            })),
        )
            .into_response();
    } else if decision_str == "deny" || approved_opt == Some(false) {
        (ApprovalDecision::Deny { reason }, "deny")
    } else if decision_str == "approve"
        || decision_str == "approve_once"
        || (decision_str.is_empty() && approved_opt == Some(true))
    {
        (ApprovalDecision::ApproveOnce, "approve_once")
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "invalid_decision",
                "message": "Invalid decision. Supported decisions are 'approve_once' or 'deny'"
            })),
        )
            .into_response();
    };

    let now_ms = Utc::now().timestamp_millis().max(0) as u64;
    let expected_prefix = format!("action:{}:", auth.user_id.0);

    if !gate.task_id.starts_with(&expected_prefix) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "success": false,
                "error": "forbidden",
                "message": "Caller does not own this approval gate"
            })),
        )
            .into_response();
    }

    if gate.is_expired() {
        return (
            StatusCode::GONE,
            Json(json!({
                "success": false,
                "error": "gate_expired",
                "message": "Approval gate has expired"
            })),
        )
            .into_response();
    }

    if gate.check_expiration(now_ms) {
        let gate_val = serde_json::to_value(&gate).unwrap_or(Value::Null);
        let db = state.db.clone();
        let gid = gate.gate_id.clone();
        let timeout_res = tokio::task::spawn_blocking(move || {
            db.try_decide_approval_gate(&gid, "timeout", &gate_val)
        })
        .await;
        return match timeout_res {
            Ok(Ok(true)) => {
                let mut gates = state.approval_gates.write().await;
                if gates.get(&gate_id).is_none_or(|g| g.decision.is_none()) {
                    gates.insert(gate_id.clone(), gate);
                }
                (
                    StatusCode::GONE,
                    Json(json!({
                        "success": false,
                        "error": "gate_expired",
                        "message": "Approval gate has expired"
                    })),
                )
                    .into_response()
            }
            Ok(Ok(false)) => sync_gate_and_conflict_response(&state, &gate_id).await,
            Ok(Err(_)) | Err(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "success": false,
                    "error": "approval_store_error",
                    "message": "Failed to persist approval gate expiration"
                })),
            )
                .into_response(),
        };
    }

    if let Err(_) = gate.decide(decision) {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "success": false,
                "error": "already_decided",
                "message": "Approval gate has already been decided or consumed"
            })),
        )
            .into_response();
    }

    let gate_val = serde_json::to_value(&gate).unwrap_or(Value::Null);
    let db = state.db.clone();
    let gid = gate.gate_id.clone();
    let label = db_decision_label.to_string();
    let decide_res = tokio::task::spawn_blocking(move || {
        db.try_decide_approval_gate(&gid, &label, &gate_val)
    })
    .await;

    match decide_res {
        Ok(Ok(true)) => {
            let mut gates = state.approval_gates.write().await;
            if gates.get(&gate_id).is_none_or(|g| g.decision.is_none()) {
                gates.insert(gate_id.clone(), gate.clone());
            }
        }
        Ok(Ok(false)) => {
            return sync_gate_and_conflict_response(&state, &gate_id).await;
        }
        Ok(Err(_)) | Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "success": false,
                    "error": "approval_store_error",
                    "message": "Failed to persist approval gate decision"
                })),
            )
                .into_response();
        }
    }

    if gate.is_approved() {
        let token = sign_approval_token(
            &state.action_approval_secret,
            &gate.gate_id,
            auth.user_id.0,
            &gate.tool_name,
            &gate.params_summary,
        );
        (
            StatusCode::OK,
            Json(json!({
                "approved": true,
                "gate_id": gate.gate_id,
                "action": gate.tool_name,
                "expires_at_ms": gate.expires_at_ms,
                "approval_token": token
            })),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            Json(json!({
                "approved": false,
                "denied": true,
                "gate_id": gate.gate_id,
                "action": gate.tool_name
            })),
        )
            .into_response()
    }
}

pub async fn handle_actions_list(State(state): State<Arc<AppState>>) -> Response {
    let actions = state
        .action_registry
        .list()
        .into_iter()
        .map(|a| {
            let risk = a.risk_level();
            json!({
                "name": a.name(),
                "category": a.category().as_str(),
                "description": a.description(),
                "risk_level": risk,
                "requires_approval": risk.requires_explicit_confirmation(),
                "read_only": risk.is_read_only(),
            })
        })
        .collect::<Vec<_>>();

    (StatusCode::OK, Json(json!({ "actions": actions }))).into_response()
}

pub async fn handle_action_schema(
    State(state): State<Arc<AppState>>,
    Path(action_name): Path<String>,
) -> Response {
    let action = match state.action_registry.get(&action_name) {
        Some(a) => a,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "success": false,
                    "error": "action_not_found",
                    "message": format!("Action '{action_name}' not found")
                })),
            )
                .into_response();
        }
    };

    let risk = action.risk_level();
    (
        StatusCode::OK,
        Json(json!({
            "name": action.name(),
            "category": action.category().as_str(),
            "description": action.description(),
            "risk_level": risk,
            "requires_approval": risk.requires_explicit_confirmation(),
            "input_schema": action.input_schema(),
            "output_schema": action.output_schema(),
        })),
    )
        .into_response()
}
