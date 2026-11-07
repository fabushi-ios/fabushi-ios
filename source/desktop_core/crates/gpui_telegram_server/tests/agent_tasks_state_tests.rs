use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use gpui_telegram_agent::tasks::{ApprovalGate, TaskFsm, TaskState};
use gpui_telegram_server::{
    build_router_with_state,
    server::AppState,
    ServerConfig,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn setup_test_server() -> (Arc<AppState>, axum::Router, String, u64) {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let (user, _session, token) = state
        .session_manager
        .register(
            "test_agent_user",
            "Password123!",
            "AgentUser",
            None,
            "GPUI Test Device",
        )
        .expect("Registration must succeed");

    let app = build_router_with_state(state.clone());
    (state, app, token, user.user_id.raw())
}

#[tokio::test]
async fn test_agent_turn_live_state_dispatch() {
    let (_state, app, token, user_id) = setup_test_server().await;

    let payload = json!({
        "prompt": "Analyze the Telegram agent architecture",
        "session_id": "test_session_turn_01",
        "thinking_budget_tokens": 64000
    });

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/agent/turn")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["turn_id"].as_str().unwrap().starts_with("turn_"));
    assert_eq!(json["caller_id"].as_u64().unwrap(), user_id);
    assert_eq!(json["status"], "executed");
    assert_eq!(json["prompt"], "Analyze the Telegram agent architecture");
    assert!(json["response"]
        .as_str()
        .unwrap()
        .contains("Analyze the Telegram agent architecture"));

    let outputs = json["outputs"].as_array().expect("outputs must be array");
    assert!(
        outputs.len() >= 2,
        "Outputs should contain thinking and final response"
    );
}

#[tokio::test]
async fn test_agent_runs_live_state_dispatch() {
    let (_state, app, token, user_id) = setup_test_server().await;

    let payload = json!({
        "prompt": "Coordinate multi-agent task execution",
        "session_id": "test_session_runs_02"
    });

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/agent/runs")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["run_id"].as_str().unwrap().starts_with("run_"));
    assert_eq!(json["user_id"].as_u64().unwrap(), user_id);
    assert_eq!(json["status"], "completed");
    assert_eq!(json["prompt"], "Coordinate multi-agent task execution");
    assert!(json["response"]
        .as_str()
        .unwrap()
        .contains("Coordinate multi-agent task execution"));
}

#[tokio::test]
async fn test_tasks_list_empty_and_populated_live_state() {
    let (state, app, token, _user_id) = setup_test_server().await;

    // 1. Initial empty task registry check
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/tasks")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 0);
    assert_eq!(json["tasks"].as_array().unwrap().len(), 0);
    assert!(json["columns"].as_array().unwrap().contains(&json!("active")));

    // 2. Populate task registry with live TaskFsm instances
    {
        let mut registry = state.task_registry.write().await;
        registry.insert(
            "task_01".into(),
            TaskFsm::new("task_01".into()),
        );

        let mut t2 = TaskFsm::new("task_02".into());
        t2.transition_to(TaskState::Running).unwrap();
        registry.insert("task_02".into(), t2);
    }

    // 3. Create a third task via POST /api/v1/tasks endpoint
    let create_payload = json!({
        "task_id": "task_03",
        "initial_state": "pending"
    });
    let create_req = Request::builder()
        .method("POST")
        .uri("/api/v1/tasks")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&create_payload).unwrap()))
        .unwrap();

    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::CREATED);

    // 4. Query /api/v1/tasks again and verify all 3 live tasks are returned
    let req2 = Request::builder()
        .method("GET")
        .uri("/api/v1/tasks")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let res2 = app.oneshot(req2).await.unwrap();
    assert_eq!(res2.status(), StatusCode::OK);

    let body2 = to_bytes(res2.into_body(), usize::MAX).await.unwrap();
    let json2: Value = serde_json::from_slice(&body2).unwrap();
    assert_eq!(json2["total"], 3);

    let tasks = json2["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 3);
    assert_eq!(tasks[0]["task_id"], "task_01");
    assert_eq!(tasks[0]["state"], "pending");
    assert_eq!(tasks[1]["task_id"], "task_02");
    assert_eq!(tasks[1]["state"], "running");
    assert_eq!(tasks[2]["task_id"], "task_03");
    assert_eq!(tasks[2]["state"], "pending");
}

#[tokio::test]
async fn test_tasks_approve_resolves_gate_and_transitions_fsm() {
    let (state, app, token, _user_id) = setup_test_server().await;

    let task_id = "task_security_gate_01";
    let gate_id = "gate_security_01";

    // 1. Prepare task in WaitingAuth state
    {
        let mut task = TaskFsm::new(task_id.into());
        task.transition_to(TaskState::Running).unwrap();
        task.transition_to(TaskState::WaitingAuth).unwrap();
        assert_eq!(task.state, TaskState::WaitingAuth);
        state.task_registry.write().await.insert(task_id.into(), task);
    }

    // 2. Prepare approval gate for task
    {
        let gate = ApprovalGate::new(
            gate_id.into(),
            task_id.into(),
            "terminal_exec".into(),
            "ls -la /".into(),
        );
        state.approval_gates.write().await.insert(gate_id.into(), gate);
    }

    // 3. Call approve endpoint
    let approve_payload = json!({
        "approved": true,
        "decision": "approve"
    });

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/tasks/{}/approve", task_id))
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&approve_payload).unwrap()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["task_id"], task_id);
    assert_eq!(json["approved"], true);
    assert_eq!(json["current_state"], "running");
    assert_eq!(json["transition_success"], true);
    assert_eq!(json["gate_id"], gate_id);

    // 4. Verify live state in task_registry transitioned to Running
    {
        let registry = state.task_registry.read().await;
        let task = registry.get(task_id).expect("Task must exist in registry");
        assert_eq!(task.state, TaskState::Running);
        assert_eq!(task.state_history.len(), 4); // Pending -> Running -> WaitingAuth -> Running
    }

    // 5. Verify live state in approval_gates resolved to Approved
    {
        let gates = state.approval_gates.read().await;
        let gate = gates.get(gate_id).expect("Gate must exist in approval_gates");
        assert!(gate.is_approved());
    }
}

#[tokio::test]
async fn test_tasks_deny_cancels_task_and_records_denial() {
    let (state, app, token, _user_id) = setup_test_server().await;

    let task_id = "task_denied_02";
    let gate_id = "gate_denied_02";

    // 1. Prepare task in WaitingAuth state
    {
        let mut task = TaskFsm::new(task_id.into());
        task.transition_to(TaskState::Running).unwrap();
        task.transition_to(TaskState::WaitingAuth).unwrap();
        state.task_registry.write().await.insert(task_id.into(), task);
    }

    // 2. Prepare approval gate
    {
        let gate = ApprovalGate::new(
            gate_id.into(),
            task_id.into(),
            "rm_tool".into(),
            "rm -rf /tmp/data".into(),
        );
        state.approval_gates.write().await.insert(gate_id.into(), gate);
    }

    // 3. Deny task approval
    let deny_payload = json!({
        "approved": false,
        "decision": "deny",
        "reason": "Forbidden command"
    });

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/tasks/{}/approve", task_id))
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&deny_payload).unwrap()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["task_id"], task_id);
    assert_eq!(json["approved"], false);
    assert_eq!(json["current_state"], "cancelled");

    // 4. Verify live task_registry state transitioned to Cancelled
    {
        let registry = state.task_registry.read().await;
        let task = registry.get(task_id).expect("Task must exist");
        assert_eq!(task.state, TaskState::Cancelled);
    }

    // 5. Verify live approval_gates state recorded Deny
    {
        let gates = state.approval_gates.read().await;
        let gate = gates.get(gate_id).expect("Gate must exist");
        assert!(gate.is_denied());
    }
}
