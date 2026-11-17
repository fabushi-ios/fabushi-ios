use crate::auth::passkey_service::{PasskeyChallenge, PasskeyService, PasskeyVerifyRequest};
use crate::auth::{verify_password, AuthBearer};
use crate::db::auth_store::PasskeyCredentialRecord;
use crate::routes::{err_resp, issue_persistent_session, map_telegram_err, user_record_to_profile};
use crate::server::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use chrono::Utc;
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::contracts::{QrPairAcceptRequest, QrPairInitResponse};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Deserialize, Default)]
pub struct PasskeyChallengeReq {
    pub user_id: Option<UserId>,
    pub username: Option<String>,
}

pub async fn passkey_challenge(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasskeyChallengeReq>,
) -> Json<PasskeyChallenge> {
    let uid = req.user_id.or_else(|| {
        req.username
            .as_deref()
            .and_then(|u| state.db.get_user_by_username(u.trim()).ok().flatten().map(|r| r.user_id))
    });
    let challenge = PasskeyService::generate_challenge(uid);
    let now_ms = Utc::now().timestamp_millis();
    let _ = state.db.save_passkey_challenge(&challenge.challenge, uid, "auth", now_ms + 300_000);
    state.passkey_challenges.write().insert(challenge.challenge.clone(), now_ms);
    Json(challenge)
}

#[derive(Debug, Deserialize)]
pub struct PasskeyRegisterReq {
    pub credential_id: String,
    pub public_key: String,
    pub device_name: Option<String>,
}

pub async fn passkey_register(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasskeyRegisterReq>,
) -> Response {
    if req.credential_id.trim().is_empty() || req.public_key.trim().is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_passkey", "credential_id and public_key required");
    }
    let now_ms = Utc::now().timestamp_millis();
    let rec = PasskeyCredentialRecord {
        credential_id: req.credential_id.trim().to_string(),
        user_id: auth.user_id,
        public_key: req.public_key.trim().to_string(),
        sign_count: 0,
        device_name: req.device_name,
        created_at_ms: now_ms,
        last_used_at_ms: now_ms,
    };
    if let Err(e) = state.db.save_passkey_credential(&rec) {
        return map_telegram_err(e);
    }
    (StatusCode::CREATED, Json(rec)).into_response()
}

pub async fn passkey_list(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    match state.db.list_user_passkeys(auth.user_id) {
        Ok(list) => (StatusCode::OK, Json(json!({ "passkeys": list, "total": list.len() }))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn passkey_verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasskeyVerifyRequest>,
) -> Response {
    let mem_hit = state.passkey_challenges.write().remove(&req.challenge).is_some();
    let db_hit = match state.db.consume_passkey_challenge(&req.challenge) {
        Ok(Some((_uid, _kind, exp))) => Utc::now().timestamp_millis() <= exp,
        Ok(None) => mem_hit,
        Err(_) => mem_hit,
    };
    if !db_hit {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_challenge", "Unknown or expired challenge");
    }
    let cred = match state.db.get_passkey_credential(&req.credential_id) {
        Ok(Some(c)) => c,
        _ => return err_resp(StatusCode::UNAUTHORIZED, "unknown_credential", "Passkey credential is not registered"),
    };
    if !PasskeyService::verify_assertion(&req).unwrap_or(false) {
        return err_resp(StatusCode::UNAUTHORIZED, "verification_failed", "Passkey assertion failed");
    }
    let Some(user) = state.db.get_user(cred.user_id).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "user_not_found", "Passkey owner account not found");
    };
    let dev = cred.device_name.as_deref().unwrap_or("Passkey Authenticator");
    (StatusCode::OK, Json(issue_persistent_session(&state, &user, dev))).into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct PairInitReq {
    pub device_name: Option<String>,
}

pub async fn pair_init(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PairInitReq>,
) -> Response {
    let pair_token = format!("pair_{}", Uuid::new_v4().simple());
    let expires_at_ms = Utc::now().timestamp_millis() + 300_000;
    if let Err(e) = state.db.create_qr_session(&pair_token, expires_at_ms) {
        return map_telegram_err(e);
    }
    let qr_url = format!("tg://login?token={}", pair_token);
    let _dto = QrPairInitResponse { pair_token: pair_token.clone(), qr_url: qr_url.clone(), expires_at_ms };
    Json(json!({
        "pair_token": pair_token,
        "qr_url": qr_url,
        "qr_payload": qr_url,
        "device_name": req.device_name.unwrap_or_else(|| "GPUI Companion".into()),
        "expires_at_ms": expires_at_ms,
    }))
    .into_response()
}

pub async fn pair_accept(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<QrPairAcceptRequest>,
) -> Response {
    let Some(user) = state.db.get_user(auth.user_id).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "user_not_found", "User not found");
    };
    let auth_resp = issue_persistent_session(&state, &user, "QR Paired Device");
    let sess_json = serde_json::to_string(&auth_resp).unwrap_or_else(|_| "{}".into());
    match state.db.accept_qr_session(&req.pair_token, auth.user_id, &sess_json) {
        Ok(true) => (StatusCode::OK, Json(json!({ "ok": true, "status": "confirmed", "pair_token": req.pair_token }))).into_response(),
        Ok(false) => err_resp(StatusCode::BAD_REQUEST, "invalid_or_expired_pair_token", "QR token invalid, expired, or already used"),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn pair_poll(
    State(state): State<Arc<AppState>>,
    Path(pair_token): Path<String>,
) -> Response {
    match state.db.get_qr_session(&pair_token) {
        Ok(Some(mut s)) => {
            if let Some(ref sess) = s.session {
                s.user = state.db.get_user(UserId(sess.user_id)).ok().flatten().map(|u| user_record_to_profile(&state, &u));
            }
            (StatusCode::OK, Json(s)).into_response()
        }
        Ok(None) => err_resp(StatusCode::NOT_FOUND, "pair_token_not_found", "QR login session not found"),
        Err(e) => map_telegram_err(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct PairVerifyReq {
    pub pair_token: String,
    pub cloud_password: Option<String>,
}

pub async fn pair_verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PairVerifyReq>,
) -> Response {
    let status = match state.db.get_qr_session(&req.pair_token) {
        Ok(Some(s)) => s,
        _ => return err_resp(StatusCode::UNAUTHORIZED, "invalid_pair_token", "Pair token not found or expired"),
    };
    let Some(sess) = status.session else {
        return err_resp(StatusCode::UNAUTHORIZED, "pair_not_confirmed", "QR token has not been confirmed by an authenticated device");
    };
    if let Ok(Some(hash)) = state.db.get_two_factor_hash(UserId(sess.user_id)) {
        let pw = req.cloud_password.as_deref().unwrap_or("");
        if !verify_password(pw, &hash) {
            return err_resp(StatusCode::UNAUTHORIZED, "two_factor_required", "Valid 2FA cloud password required");
        }
    }
    (StatusCode::OK, Json(json!({
        "verified": true, "user_id": sess.user_id, "session_id": sess.session_id,
        "token": sess.token, "expires_at": sess.expires_at, "session": sess
    }))).into_response()
}

pub async fn auth_sessions_list(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    match state
        .session_store
        .list_active_sessions_for_user(auth.user_id.raw())
    {
        Ok(records) => {
            let mut sessions: Vec<gpui_telegram_proto::contracts::SessionDto> = records
                .into_iter()
                .map(|r| gpui_telegram_proto::contracts::SessionDto {
                    is_current: r.session_id == auth.session.session_id,
                    session_id: r.session_id,
                    device_name: r.device_id,
                    platform: Some(r.client_type),
                    ip_address: r.ip_address,
                    created_at_ms: r.created_at_ms,
                    last_active_ms: r.last_active_ms,
                })
                .collect();
            let mut seen: std::collections::HashSet<String> =
                sessions.iter().map(|s| s.session_id.clone()).collect();
            for s in state.session_manager.list_user_sessions(auth.user_id) {
                if seen.insert(s.session_id.clone()) {
                    let ts_ms = s.created_at.timestamp_millis();
                    sessions.push(gpui_telegram_proto::contracts::SessionDto {
                        is_current: s.session_id == auth.session.session_id,
                        session_id: s.session_id,
                        device_name: s.device_name,
                        platform: None,
                        ip_address: None,
                        created_at_ms: ts_ms,
                        last_active_ms: ts_ms,
                    });
                }
            }
            (StatusCode::OK, Json(json!({ "user_id": auth.user_id.raw(), "sessions": sessions }))).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn auth_revoke_other_sessions(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    match state
        .session_store
        .revoke_other_sessions_async(
            auth.user_id.raw(),
            &auth.session.session_id,
            "Other sessions revoked by user",
        )
        .await
    {
        Ok(revoked_ids) => {
            let db_count = revoked_ids.len() as u64;
            for sid in &revoked_ids {
                state.session_manager.revoke_session(sid);
                state.gateway_hub.disconnect_session(auth.user_id, sid);
            }
            let mut mem_count: u64 = 0;
            for s in state.session_manager.list_user_sessions(auth.user_id) {
                if s.session_id != auth.session.session_id
                    && state.session_manager.revoke_session(&s.session_id).is_some()
                {
                    state.gateway_hub.disconnect_session(auth.user_id, &s.session_id);
                    mem_count += 1;
                }
            }
            (StatusCode::OK, Json(json!({ "revoked_count": db_count.max(mem_count) }))).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn auth_delete_session(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Response {
    match state
        .session_store
        .revoke_user_session_async(auth.user_id.raw(), &session_id, "Session revoked by user")
        .await
    {
        Ok(db_revoked) => {
            let mem_revoked = match state.session_manager.get(&session_id) {
                Some(tok) if tok.user_id == auth.user_id => {
                    state.session_manager.revoke_session(&session_id).is_some()
                }
                _ => false,
            };
            if db_revoked || mem_revoked {
                state
                    .gateway_hub
                    .disconnect_session(auth.user_id, &session_id);
                let fence = state.session_manager.get_or_create_fence(auth.user_id);
                (StatusCode::OK, Json(json!({ "revoked": true, "session_id": session_id, "account_generation": fence.current() }))).into_response()
            } else {
                err_resp(StatusCode::NOT_FOUND, "session_not_found", "Session not found for current user")
            }
        }
        Err(e) => map_telegram_err(e),
    }
}
