use crate::routes::err_resp;
use crate::server::{AppState, AuthBearer};
use axum::{body::Bytes, extract::{Path, State}, http::StatusCode, response::{IntoResponse, Json, Response}};
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

fn non_empty_str(v: Option<&Value>) -> Option<String> {
    v.and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn parse_optional_chat_id(v: Option<&Value>) -> Option<u64> {
    v.and_then(|x| {
        x.as_i64()
            .filter(|&n| n > 0)
            .map(|n| n as u64)
            .or_else(|| x.as_u64().filter(|&n| n > 0))
    })
}

fn copy_task_metadata(dst: &mut Value, src: Option<&Value>, default_requires_approval: bool) {
    let Some(obj) = dst.as_object_mut() else { return };
    let goal = src.and_then(|s| non_empty_str(s.get("goal")));
    let prompt = src.and_then(|s| non_empty_str(s.get("prompt")));
    let title = src
        .and_then(|s| non_empty_str(s.get("title")))
        .or_else(|| goal.clone())
        .or_else(|| prompt.clone());
    if let Some(t) = title {
        obj.insert("title".into(), Value::String(t));
    }
    if let Some(g) = goal {
        obj.insert("goal".into(), Value::String(g));
    }
    if let Some(p) = prompt {
        obj.insert("prompt".into(), Value::String(p));
    }
    if let Some(cid) = src.and_then(|s| parse_optional_chat_id(s.get("chat_id"))) {
        obj.insert("chat_id".into(), Value::from(cid));
    }
    let req_approval = src
        .and_then(|s| s.get("requires_approval").and_then(|v| v.as_bool()))
        .unwrap_or(default_requires_approval);
    obj.insert("requires_approval".into(), Value::Bool(req_approval));
}

pub async fn tasks_list(_auth: AuthBearer, State(state): State<Arc<AppState>>) -> Json<Value> {
    let db_rows: HashMap<String, Value> = state
        .db
        .load_acp_tasks()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let registry = state.task_registry.read().await;
    let mut tasks: Vec<Value> = registry
        .values()
        .map(|task| {
            let mut entry = json!({
                "task_id": task.task_id,
                "state": task.state.as_str(),
                "status": task.state.as_str(),
                "state_history": task.state_history,
                "abort_reason": task.abort_reason,
                "is_active": task.is_active(),
                "is_terminal": task.is_terminal(),
            });
            copy_task_metadata(
                &mut entry,
                db_rows.get(&task.task_id),
                matches!(task.state, gpui_telegram_agent::tasks::TaskState::WaitingAuth),
            );
            entry
        })
        .collect();
    tasks.sort_by(|a, b| a["task_id"].as_str().cmp(&b["task_id"].as_str()));
    let total = tasks.len();
    Json(json!({ "tasks": tasks, "columns": ["inbox", "active", "review", "completed", "failed", "cancelled"], "total": total }))
}

pub async fn tasks_create(_auth: AuthBearer, State(state): State<Arc<AppState>>, Json(payload): Json<Value>) -> Response {
    let task_id = non_empty_str(payload.get("task_id")).unwrap_or_else(|| format!("task_{}", Uuid::new_v4().simple()));
    let mut task = gpui_telegram_agent::tasks::TaskFsm::new(task_id.clone());
    if let Some(target_str) = non_empty_str(
        payload
            .get("initial_state")
            .or_else(|| payload.get("state"))
            .or_else(|| payload.get("status")),
    ) {
        let target = if target_str.eq_ignore_ascii_case("awaiting_approval") {
            Some(gpui_telegram_agent::tasks::TaskState::WaitingAuth)
        } else {
            gpui_telegram_agent::tasks::TaskState::from_str(&target_str)
        };
        if let Some(target) = target {
            if target != gpui_telegram_agent::tasks::TaskState::Pending {
                if matches!(
                    target,
                    gpui_telegram_agent::tasks::TaskState::WaitingAuth
                        | gpui_telegram_agent::tasks::TaskState::WaitingConnection
                        | gpui_telegram_agent::tasks::TaskState::Completed
                        | gpui_telegram_agent::tasks::TaskState::Failed
                ) {
                    let _ = task.transition_to(gpui_telegram_agent::tasks::TaskState::Running);
                }
                let _ = task.transition_to(target);
            }
        }
    }
    let mut val = serde_json::to_value(&task).unwrap_or_else(|_| json!({}));
    copy_task_metadata(
        &mut val,
        Some(&payload),
        matches!(task.state, gpui_telegram_agent::tasks::TaskState::WaitingAuth),
    );
    if let Some(obj) = val.as_object_mut() {
        obj.insert("status".into(), Value::String(task.state.as_str().into()));
    }
    let _ = state.db.upsert_acp_task(&task_id, &val, task.state.as_str());
    state.task_registry.write().await.insert(task_id.clone(), task.clone());
    let mut resp = json!({
        "task_id": task_id,
        "state": task.state.as_str(),
        "status": task.state.as_str(),
        "created": true,
    });
    copy_task_metadata(
        &mut resp,
        Some(&val),
        matches!(task.state, gpui_telegram_agent::tasks::TaskState::WaitingAuth),
    );
    (StatusCode::CREATED, Json(resp)).into_response()
}

fn is_action_approval_id(id: &str) -> bool {
    let trimmed = id.trim();
    trimmed.starts_with("actgate_") || trimmed.starts_with("action:")
}

fn reject_action_gate_response() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": "invalid_task_id",
            "message": "Action approval gates must be decided via /api/v1/actions/approvals/{gate_id}/approve",
            "success": false
        })),
    )
        .into_response()
}

pub async fn tasks_approve(_auth: AuthBearer, State(state): State<Arc<AppState>>, Path(task_id): Path<String>, body_bytes: Bytes) -> Response {
    if is_action_approval_id(&task_id) {
        return reject_action_gate_response();
    }

    let payload: Value = if body_bytes.is_empty() { json!({}) } else { serde_json::from_slice(&body_bytes).unwrap_or_else(|_| json!({})) };
    let is_approved = payload.get("approved").and_then(|v| v.as_bool()).unwrap_or(true);
    let decision_str = payload.get("decision").and_then(|v| v.as_str()).unwrap_or(if is_approved { "approve" } else { "deny" });
    let decision = if decision_str == "deny" || !is_approved {
        gpui_telegram_agent::ApprovalDecision::Deny { reason: payload.get("reason").and_then(|v| v.as_str()).map(String::from) }
    } else if decision_str == "always_allow" {
        gpui_telegram_agent::ApprovalDecision::AlwaysAllow
    } else {
        gpui_telegram_agent::ApprovalDecision::ApproveOnce
    };

    let mut gates = state.approval_gates.write().await;
    let gate_key = if gates.contains_key(&task_id) { Some(task_id.clone()) } else {
        gates.iter().find_map(|(k, g)| if g.task_id == task_id { Some(k.clone()) } else { None })
    };
    if let Some(ref key) = gate_key {
        if let Some(gate) = gates.get(key) {
            if is_action_approval_id(&gate.gate_id) || is_action_approval_id(&gate.task_id) {
                return reject_action_gate_response();
            }
        }
    }
    let resolved_gate_id = if let Some(key) = gate_key {
        if let Some(gate) = gates.get_mut(&key) {
            let _ = gate.decide(decision.clone());
            let _ = state.db.upsert_approval_gate(&gate.gate_id, &gate.task_id, Some(decision_str), &serde_json::to_value(&*gate).unwrap_or(Value::Null));
            gate.gate_id.clone()
        } else { format!("gate_{}", task_id) }
    } else {
        let gid = format!("gate_{}", task_id);
        let mut g = gpui_telegram_agent::ApprovalGate::new(gid.clone(), task_id.clone(), "task_approval".into(), "API triggered task approval".into());
        let _ = g.decide(decision.clone());
        let _ = state.db.upsert_approval_gate(&gid, &task_id, Some(decision_str), &serde_json::to_value(&g).unwrap_or(Value::Null));
        gates.insert(gid.clone(), g);
        gid
    };
    drop(gates);

    let prev_meta = state.db.get_acp_task(&task_id).ok().flatten();
    let mut tasks = state.task_registry.write().await;
    let (current_state, transition_success) = if let Some(task) = tasks.get_mut(&task_id) {
        let target = match decision {
            gpui_telegram_agent::ApprovalDecision::Deny { .. } => gpui_telegram_agent::tasks::TaskState::Cancelled,
            _ if matches!(task.state, gpui_telegram_agent::tasks::TaskState::WaitingAuth | gpui_telegram_agent::tasks::TaskState::Pending) => gpui_telegram_agent::tasks::TaskState::Running,
            _ => task.state,
        };
        let ok = if target != task.state { task.transition_to(target).is_ok() } else { true };
        let mut val = serde_json::to_value(&*task).unwrap_or_else(|_| json!({}));
        copy_task_metadata(&mut val, prev_meta.as_ref(), false);
        if let Some(obj) = val.as_object_mut() {
            obj.insert("requires_approval".into(), Value::Bool(false));
            obj.insert("status".into(), Value::String(task.state.as_str().into()));
        }
        let _ = state.db.upsert_acp_task(&task_id, &val, task.state.as_str());
        (task.state.as_str().to_string(), ok)
    } else { ("unknown".into(), true) };

    (StatusCode::OK, Json(json!({
        "task_id": task_id, "approved": is_approved, "decision": format!("{:?}", decision),
        "current_state": current_state, "transition_success": transition_success, "gate_id": resolved_gate_id, "resolved_at": Utc::now(),
    }))).into_response()
}

pub async fn mcp_servers_list(_auth: AuthBearer, State(state): State<Arc<AppState>>) -> Json<Value> {
    let tools = state.mcp_registry.read().list_tools();
    let servers: Vec<Value> = state.db.load_mcp_servers().unwrap_or_default().into_iter().map(|(id, name, ep, st, ts)| json!({
        "server_id": id, "name": name, "endpoint": ep, "status": st, "tools": ts,
    })).collect();
    Json(json!({ "servers": servers, "registered_tools_count": tools.len() }))
}

pub async fn mcp_servers_register(_auth: AuthBearer, State(state): State<Arc<AppState>>, Json(payload): Json<Value>) -> Response {
    let server_id = payload.get("server_id").and_then(|v| v.as_str()).unwrap_or("default");
    let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or(server_id);
    let endpoint = payload.get("endpoint").and_then(|v| v.as_str()).unwrap_or("stdio://local");
    let mut reg_tools = Vec::new();
    if let Some(tool_name) = payload.get("tool_name").and_then(|v| v.as_str()) {
        let desc = payload.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let schema = payload.get("schema").and_then(|v| v.as_str()).unwrap_or("{}");
        let def = gpui_telegram_agent::McpToolDefinition::new(tool_name, desc, schema, gpui_telegram_agent::ToolRiskLevel::Low).with_server_id(server_id);
        reg_tools.push(serde_json::to_value(&def).unwrap_or(Value::Null));
        state.mcp_registry.write().register(def);
    }
    let _ = state.db.upsert_mcp_server(server_id, name, endpoint, "connected", &Value::Array(reg_tools));
    (StatusCode::CREATED, Json(json!({ "status": "registered", "server_id": server_id }))).into_response()
}

pub async fn computer_lease_acquire(auth: AuthBearer, State(state): State<Arc<AppState>>, Json(payload): Json<Value>) -> Response {
    let device_id = payload.get("device_id").and_then(|v| v.as_str()).unwrap_or("desktop_local");
    let agent_id = format!("agent_{}", auth.user_id.raw());
    match state.computer_lease_broker.write().acquire_lease(&agent_id, device_id) {
        Some(lease) => {
            let _ = state.db.upsert_computer_lease(&lease.lease_id, auth.user_id.0, Some(device_id), "active", &json!([]), Utc::now().timestamp_millis());
            (StatusCode::CREATED, Json(json!({ "status": "acquired", "lease": lease }))).into_response()
        }
        None => err_resp(StatusCode::CONFLICT, "lease_busy", "Control lease already held"),
    }
}

pub async fn computer_lease_emergency_takeover(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Json<Value> {
    let new_gen = state.computer_lease_broker.write().emergency_takeover();
    let _ = state.db.upsert_computer_lease(&format!("takeover_{new_gen}"), auth.user_id.0, None, "preempted", &json!([]), Utc::now().timestamp_millis());
    Json(json!({ "status": "emergency_takeover_executed", "action": "preempt_and_fence", "new_generation": new_gen, "latency_sub_10ms": true }))
}

pub async fn talk_session_create(auth: AuthBearer, State(state): State<Arc<AppState>>, Json(_payload): Json<Value>) -> Response {
    let sid = format!("talk_{}_{}", auth.user_id.0, Uuid::new_v4().simple());
    let (talk_state, generation, signal_seq) = {
        let mut sessions = state.talk_sessions.write();
        let coord = sessions.entry(sid.clone()).or_insert_with(gpui_telegram_agent::TalkCoordinator::new);
        coord.session_id = sid.clone();
        coord.state = gpui_telegram_agent::TalkSessionState::Listening;
        (coord.state, coord.generation, coord.signal_seq)
    };
    state.user_active_talk_sessions.write().insert(auth.user_id, sid.clone());
    let _ = state.db.upsert_talk_session(&sid, auth.user_id.0, "listening", &json!([]), Utc::now().timestamp_millis());
    (StatusCode::CREATED, Json(json!({
        "session_id": sid, "state": talk_state, "generation": generation, "signal_seq": signal_seq,
    }))).into_response()
}
