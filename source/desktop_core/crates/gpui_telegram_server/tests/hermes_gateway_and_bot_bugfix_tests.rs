use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use gpui_telegram_server::{build_router_with_state, server::AppState, ServerConfig};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tower::ServiceExt;
use uuid::Uuid;

struct TempDirGuard(PathBuf);

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let p = std::env::temp_dir().join(format!("{prefix}_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open_durable(dir: &TempDirGuard) -> (Arc<AppState>, Router) {
    let cfg = ServerConfig {
        database_url: dir.0.join("hermes_server.db").to_string_lossy().into_owned(),
        media_storage_path: dir.0.join("cas").to_string_lossy().into_owned(),
        ..Default::default()
    };
    let state = Arc::new(AppState::try_new(cfg).unwrap());
    let app = build_router_with_state(state.clone());
    (state, app)
}

async fn api_raw(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    accept: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(a) = accept {
        b = b.header(header::ACCEPT, a);
    }
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let st = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (st, String::from_utf8_lossy(&bytes).into_owned())
}

async fn api(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (st, raw) = api_raw(app, method, uri, token, None, body).await;
    (st, serde_json::from_str(&raw).unwrap_or(Value::Null))
}

async fn reg(app: &Router, uname: &str) -> (u64, String) {
    let (st, v) = api(
        app,
        Method::POST,
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": uname,
            "password": "Password!1",
            "first_name": uname
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    (
        v["user_id"].as_u64().unwrap(),
        v["token"].as_str().unwrap().to_string(),
    )
}

async fn poll_chat_messages_until(
    app: &Router,
    token: &str,
    chat_id: i64,
    min_count: usize,
) -> Vec<Value> {
    let uri = format!("/api/v1/chats/{chat_id}/messages");
    for _ in 0..80 {
        let (st, v) = api(app, Method::GET, &uri, Some(token), None).await;
        assert_eq!(st, StatusCode::OK);
        let msgs = v["messages"].as_array().cloned().unwrap_or_default();
        if msgs.len() >= min_count {
            return msgs;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let (_, v) = api(app, Method::GET, &uri, Some(token), None).await;
    v["messages"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn test_custom_ai_bot_creation_single_reply_slash_commands_and_call_invite() {
    let dir = TempDirGuard::new("hermes_bot_bugfix");
    let (_state, app) = open_durable(&dir);
    let (_uid, tok) = reg(&app, "alice_bot_user").await;

    // 1. Create custom AI bot via /api/v1/contacts/add
    let (st, created_bot) = api(
        &app,
        Method::POST,
        "/api/v1/contacts/add",
        Some(&tok),
        Some(json!({
            "first_name": "Kestrel Quant",
            "username": "kestrel_quant_bot",
            "is_bot": true,
            "persona": "Quantitative Rust architecture strategist",
            "system_prompt": "You are Kestrel Quant, focused exclusively on Rust architecture.",
            "model": "hermes-local-v1",
            "welcome_message": "Kestrel Quant online and ready."
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(created_bot["chat_type"], 4);
    assert_eq!(created_bot["is_bot"], true);
    assert_eq!(created_bot["username"], "kestrel_quant_bot");
    assert_eq!(
        created_bot["persona"],
        "Quantitative Rust architecture strategist"
    );

    let bot_uid = created_bot["user_id"].as_u64().unwrap();
    let bot_chat_id = created_bot["chat_id"].as_i64().unwrap();

    // Chat list & detail report chat_type=4, is_bot=true, custom persona (NOT "大乘 AI 助手")
    let (st, chats_list) = api(&app, Method::GET, "/api/v1/chats", Some(&tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let bot_chat = chats_list["chats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["chat_id"].as_i64() == Some(bot_chat_id))
        .expect("custom bot chat present in list");
    assert_eq!(bot_chat["chat_type"], 4);
    assert_eq!(bot_chat["is_bot"], true);
    assert_eq!(bot_chat["peer_user_id"], bot_uid);
    let bio = bot_chat["peer_bio"].as_str().unwrap_or("");
    assert!(bio.contains("Quantitative Rust architecture strategist"));
    assert!(!bio.contains("大乘 AI 助手"));

    let (st, chat_detail) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{bot_chat_id}"),
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(chat_detail["chat_type"], 4);
    assert_eq!(chat_detail["is_bot"], true);
    assert_eq!(chat_detail["peer_username"], "kestrel_quant_bot");

    // Resolve peer details by @username
    let (st, resolved) = api(
        &app,
        Method::GET,
        "/api/v1/peers/resolve?username=kestrel_quant_bot",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(resolved["kind"], "user");
    assert_eq!(resolved["user"]["is_bot"], true);
    assert_eq!(resolved["user"]["username"], "kestrel_quant_bot");

    // Per-bot SOUL KV returns custom persona
    let (st, bot_soul) = api(
        &app,
        Method::GET,
        &format!("/api/v1/hermes/soul?bot_id={bot_uid}"),
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!bot_soul["persona_name"]
        .as_str()
        .unwrap_or("")
        .contains("大乘 AI 助手"));

    // 2. Sending "你好" triggers single authoritative bot reply (welcome + "你好" + 1 reply = 3 msgs)
    let (st, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{bot_chat_id}/messages"),
        Some(&tok),
        Some(json!({ "text": "你好", "client_nonce": 101 })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let msgs = poll_chat_messages_until(&app, &tok, bot_chat_id, 3).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let (_, recheck) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{bot_chat_id}/messages"),
        Some(&tok),
        None,
    )
    .await;
    let recheck_msgs = recheck["messages"].as_array().unwrap();
    assert_eq!(
        recheck_msgs.len(),
        3,
        "Expected welcome + user '你好' + single authoritative reply, got {:?}",
        recheck_msgs
    );
    let reply = &msgs[2];
    assert_eq!(reply["is_bot_sender"], true);
    assert_eq!(reply["sender_id"], bot_uid);
    assert_eq!(reply["sender_name"], "Kestrel Quant");
    assert!(!reply["text"].as_str().unwrap_or("").is_empty());
    assert!(!reply["text"].as_str().unwrap_or("").contains("大乘 AI 助手"));

    // Follow-up message with voice hint triggers inline Talk Mode keyboard & preserves history
    let (st, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{bot_chat_id}/messages"),
        Some(&tok),
        Some(json!({ "text": "Let's test voice talk mode", "client_nonce": 102 })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let msgs2 = poll_chat_messages_until(&app, &tok, bot_chat_id, 5).await;
    assert_eq!(msgs2.len(), 5);
    assert!(
        msgs2[4]["inline_keyboard"].is_array() || msgs2[4]["bot_keyboard"].is_array(),
        "Expected inline keyboard buttons on bot reply: {:?}",
        msgs2[4]
    );

    // Verify slash command execution in bot chat persists reply + buttons
    let (st, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{bot_chat_id}/messages"),
        Some(&tok),
        Some(json!({ "text": "/help", "client_nonce": 103 })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let msgs3 = poll_chat_messages_until(&app, &tok, bot_chat_id, 7).await;
    assert_eq!(msgs3.len(), 7);
    assert_eq!(msgs3[6]["is_bot_sender"], true);

    // Persistent conversation history inside Hermes SQLite store
    let (st, sessions_val) = api(
        &app,
        Method::GET,
        &format!("/api/v1/hermes/sessions?chat_id={bot_chat_id}"),
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let sessions = sessions_val["sessions"].as_array().unwrap();
    assert!(!sessions.is_empty());
    let sid = sessions[0]["session_id"].as_str().unwrap().to_string();
    let (st, session_detail) = api(
        &app,
        Method::GET,
        &format!("/api/v1/hermes/sessions/{sid}"),
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(session_detail["messages"].as_array().unwrap().len() >= 2);

    // Idempotent /api/v1/chats/{chat_id}/agent-runs
    let (st, run1) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{bot_chat_id}/agent-runs"),
        Some(&tok),
        Some(json!({
            "prompt": "/soul",
            "bot_id": bot_uid,
            "idempotency_key": "idem_run_1"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, run2) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{bot_chat_id}/agent-runs"),
        Some(&tok),
        Some(json!({
            "prompt": "/soul",
            "bot_id": bot_uid,
            "idempotency_key": "idem_run_1"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(run1["turn_id"], run2["turn_id"]);

    // 3. POST /api/v1/calls/invite targeting AI bot by peer user_id AND by chat_id succeeds 201 mode="ai_talk"
    let (st, call_by_uid) = api(
        &app,
        Method::POST,
        "/api/v1/calls/invite",
        Some(&tok),
        Some(json!({
            "callee_id": bot_uid,
            "is_video": false,
            "chat_id": bot_chat_id
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(call_by_uid["mode"], "ai_talk");
    assert_eq!(call_by_uid["is_bot"], true);
    assert_eq!(call_by_uid["state"], "active");
    assert!(!call_by_uid["talk_session_id"]
        .as_str()
        .unwrap_or("")
        .is_empty());

    let (st, call_by_chat_id) = api(
        &app,
        Method::POST,
        "/api/v1/calls/invite",
        Some(&tok),
        Some(json!({
            "callee_id": bot_chat_id as u64,
            "is_video": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(call_by_chat_id["mode"], "ai_talk");
    assert_eq!(call_by_chat_id["is_bot"], true);
    assert_eq!(call_by_chat_id["callee_id"], bot_uid);

    let (st, missing_call) = api(
        &app,
        Method::POST,
        "/api/v1/calls/invite",
        Some(&tok),
        Some(json!({ "callee_id": 9_888_777_666u64, "is_video": false })),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(missing_call["error"], "user_not_found");
}

#[tokio::test]
async fn test_hermes_gateway_http_routes_openai_compat_and_acp_rpc() {
    let dir = TempDirGuard::new("hermes_gateway_routes");
    let (state, app) = open_durable(&dir);
    let (_uid, tok) = reg(&app, "alice_gateway").await;

    // Status & Commands
    let (st, status_json) = api(&app, Method::GET, "/api/v1/hermes/status", Some(&tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(status_json["healthy"], true);
    assert_eq!(status_json["engine"], "hermes-rust");
    assert!(status_json["toolsets_count"].as_u64().unwrap() >= 28);
    assert!(status_json["tools_count"].as_u64().unwrap() >= 40);

    let (st, cmds_json) = api(&app, Method::GET, "/api/v1/hermes/commands", Some(&tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(cmds_json["commands"].as_array().unwrap().len() >= 50);

    // Direct slash command runner (/help, /memory, /soul, /kanban, /cron)
    for cmd in [
        "/help",
        "/memory add Always keep debug=0 in Cargo.toml",
        "/soul",
        "/kanban list",
        "/cron list",
    ] {
        let (st, out) = api(
            &app,
            Method::POST,
            "/api/v1/hermes/command",
            Some(&tok),
            Some(json!({ "command": cmd })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(out["handled"], true);
        assert!(!out["output"].as_str().unwrap_or("").is_empty());
    }

    // Turns + Events (JSON and SSE)
    let (st, turn) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/turns",
        Some(&tok),
        Some(json!({
            "prompt": "Summarize workspace status",
            "session_id": "gw_sess_1"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let turn_id = turn["turn_id"].as_str().unwrap();
    let sess_id = turn["session_id"].as_str().unwrap();
    assert!(!turn["response"].as_str().unwrap_or("").is_empty());

    let (st, evs) = api(
        &app,
        Method::GET,
        &format!("/api/v1/hermes/turns/{turn_id}/events"),
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!evs["events"].as_array().unwrap().is_empty());

    let (st, sse_body) = api_raw(
        &app,
        Method::GET,
        &format!("/api/v1/hermes/turns/{turn_id}/events?stream=true"),
        Some(&tok),
        Some("text/event-stream"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(sse_body.contains("data: [DONE]"));

    // Sessions: list, search, branch, compress, undo, export
    let (st, search) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/sessions/search",
        Some(&tok),
        Some(json!({ "query": "workspace" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!search["hits"].as_array().unwrap().is_empty());

    let (st, branch) = api(
        &app,
        Method::POST,
        &format!("/api/v1/hermes/sessions/{sess_id}/branch"),
        Some(&tok),
        Some(json!({ "branch_name": "alt-branch" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(branch["parent_session_id"], sess_id);

    let (st, comp) = api(
        &app,
        Method::POST,
        &format!("/api/v1/hermes/sessions/{sess_id}/compress"),
        Some(&tok),
        Some(json!({ "focus": "decisions" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(comp["compressed"], true);

    let (st, exp) = api(
        &app,
        Method::POST,
        &format!("/api/v1/hermes/sessions/{sess_id}/export"),
        Some(&tok),
        Some(json!({ "format": "sharegpt", "redact": true })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!exp["content"].as_str().unwrap_or("").is_empty());
    assert_eq!(exp["format"], "sharegpt");

    let (st, undo_res) = api(
        &app,
        Method::POST,
        &format!("/api/v1/hermes/sessions/{sess_id}/undo"),
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(undo_res.get("removed_messages").is_some());

    // Memory GET/PUT/action + Soul GET/PUT + Honcho GET/POST
    let (st, _) = api(
        &app,
        Method::PUT,
        "/api/v1/hermes/memory",
        Some(&tok),
        Some(json!({
            "memory_markdown": "Lean dev builds",
            "user_markdown": "Concise Rust style"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, mem_after) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/memory/action",
        Some(&tok),
        Some(json!({ "action": "add", "scope": "memory", "content": "Zero stale target artifacts" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(mem_after["memory_markdown"]
        .as_str()
        .unwrap()
        .contains("Zero stale target artifacts"));

    let (st, soul_put) = api(
        &app,
        Method::PUT,
        "/api/v1/hermes/soul",
        Some(&tok),
        Some(json!({
            "persona_name": "Hermes Architect",
            "content": "# Hermes Architect\nPure Rust multi-agent engineer."
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(soul_put["persona_name"], "Hermes Architect");

    let (st, honcho) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/honcho",
        Some(&tok),
        Some(json!({
            "peer_name": "alice",
            "note": "Values fast local SQLite WAL indexing",
            "query": "What storage does alice value?"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!honcho["profile_summary"].as_str().unwrap_or("").is_empty());

    // Skills CRUD + Approve
    let (st, skill_created) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/skills",
        Some(&tok),
        Some(json!({
            "name": "wal-hygiene",
            "description": "Checks SQLite WAL checkpoints",
            "content": "Run PRAGMA wal_checkpoint(TRUNCATE).",
            "trusted": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(skill_created["name"], "wal-hygiene");

    let (st, _) = api(
        &app,
        Method::PATCH,
        "/api/v1/hermes/skills/wal-hygiene",
        Some(&tok),
        Some(json!({ "old_text": "TRUNCATE", "new_text": "PASSIVE" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, skill_got) = api(
        &app,
        Method::GET,
        "/api/v1/hermes/skills/wal-hygiene",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(skill_got["content"].as_str().unwrap().contains("PASSIVE"));

    let (st, _) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/skills/wal-hygiene/approve",
        Some(&tok),
        Some(json!({ "approved": true })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = api(
        &app,
        Method::DELETE,
        "/api/v1/hermes/skills/wal-hygiene",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Toolsets + Approvals + Cron + Heartbeat + Kanban + Providers + Usage + Rollback + Terminal + Talk
    let (st, ts) = api(&app, Method::GET, "/api/v1/hermes/toolsets", Some(&tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ts["toolsets"].as_array().unwrap().len() >= 28);
    let (st, _) = api(
        &app,
        Method::PUT,
        "/api/v1/hermes/toolsets",
        Some(&tok),
        Some(json!({ "toolset": "web", "enabled": false })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let apr = state
        .hermes
        .store()
        .enqueue_approval(
            gpui_telegram_agent::hermes::state_db::CreateApprovalRequest {
                id: None,
                session_id: sess_id.to_string(),
                command: "rm -rf /tmp/staging".into(),
                risk_level: "dangerous".into(),
                pattern_name: "recursive_rm".into(),
            },
        )
        .unwrap();
    let (st, aprs) = api(&app, Method::GET, "/api/v1/hermes/approvals", Some(&tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!aprs["approvals"].as_array().unwrap().is_empty());
    let (st, dec) = api(
        &app,
        Method::POST,
        &format!("/api/v1/hermes/approvals/{}/decide", apr.id),
        Some(&tok),
        Some(json!({ "decision": "deny", "reason": "unsafe" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(dec["status"], "denied");

    let (st, cron) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/cron",
        Some(&tok),
        Some(json!({
            "name": "Daily Digest",
            "schedule": "0 9 * * *",
            "prompt": "Report open cards",
            "enabled": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let job_id = cron["job_id"].as_str().unwrap();
    let (st, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/hermes/cron/{job_id}/run"),
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = api(
        &app,
        Method::DELETE,
        &format!("/api/v1/hermes/cron/{job_id}"),
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, hb) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/heartbeat",
        Some(&tok),
        Some(json!({ "enabled": true, "interval_secs": 300 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(hb["enabled"], true);

    let (st, board) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/kanban/boards",
        Some(&tok),
        Some(json!({ "name": "Gateway Board", "columns": ["Backlog", "Done"] })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let board_id = board["board_id"].as_str().unwrap();
    let (st, card) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/kanban/cards",
        Some(&tok),
        Some(json!({ "board_id": board_id, "title": "Verify HTTP", "column": "Backlog" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let card_id = card["card_id"].as_str().unwrap();
    let (st, moved) = api(
        &app,
        Method::PATCH,
        &format!("/api/v1/hermes/kanban/cards/{card_id}"),
        Some(&tok),
        Some(json!({ "column": "Done" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(moved["column"], "Done");
    let (st, _) = api(
        &app,
        Method::DELETE,
        &format!("/api/v1/hermes/kanban/cards/{card_id}"),
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = api(&app, Method::GET, "/api/v1/hermes/providers", Some(&tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = api(&app, Method::GET, "/api/v1/hermes/usage", Some(&tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/rollback",
        Some(&tok),
        Some(json!({ "session_id": sess_id })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = api(
        &app,
        Method::GET,
        "/api/v1/hermes/terminal/processes",
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, talk) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/talk/turn",
        Some(&tok),
        Some(json!({ "transcript": "Hello Hermes Voice" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!talk["reply_text"].as_str().unwrap_or("").is_empty());

    // OpenAI-compatible /v1/models & /v1/chat/completions (JSON + SSE)
    let (st, models) = api(&app, Method::GET, "/v1/models", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(models["object"], "list");
    assert!(!models["data"].as_array().unwrap().is_empty());

    let (st, oai) = api(
        &app,
        Method::POST,
        "/v1/chat/completions",
        Some(&tok),
        Some(json!({
            "model": "hermes-local-v1",
            "messages": [{ "role": "user", "content": "Say hello via OpenAI gateway" }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(oai["object"], "chat.completion");
    assert!(!oai["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .is_empty());

    let (st, oai_sse) = api_raw(
        &app,
        Method::POST,
        "/v1/chat/completions",
        Some(&tok),
        Some("text/event-stream"),
        Some(json!({
            "model": "hermes-local-v1",
            "stream": true,
            "messages": [{ "role": "user", "content": "Stream greeting" }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(oai_sse.contains("chat.completion.chunk"));
    assert!(oai_sse.contains("data: [DONE]"));

    // ACP JSON-RPC 2.0 endpoint
    let (st, acp_init) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/acp",
        Some(&tok),
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(acp_init["jsonrpc"], "2.0");
    assert_eq!(acp_init["result"]["serverInfo"]["name"], "hermes-rust-agent");

    let (st, acp_tools) = api(
        &app,
        Method::POST,
        "/api/v1/hermes/acp",
        Some(&tok),
        Some(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(acp_tools["result"]["tools"].as_array().unwrap().len() >= 40);
}

#[tokio::test]
async fn test_numeric_named_bot_reply_uses_display_name_without_debug_banner() {
    let dir = TempDirGuard::new("hermes_bot_name_clean");
    let (_state, app) = open_durable(&dir);
    let (_uid, tok) = reg(&app, "alice_clean_bot").await;

    let (st, created_bot) = api(
        &app,
        Method::POST,
        "/api/v1/contacts/add",
        Some(&tok),
        Some(json!({
            "first_name": "1",
            "username": "1_bot",
            "is_bot": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let bot_uid = created_bot["user_id"].as_u64().unwrap();
    let bot_chat_id = created_bot["chat_id"].as_i64().unwrap();
    let bot_uid_str = bot_uid.to_string();

    for (idx, prompt) in ["你好", "你可以做什么", "1"].iter().enumerate() {
        let (st, _) = api(
            &app,
            Method::POST,
            &format!("/api/v1/chats/{bot_chat_id}/messages"),
            Some(&tok),
            Some(json!({ "text": prompt, "client_nonce": 500 + idx })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
    }

    let msgs = poll_chat_messages_until(&app, &tok, bot_chat_id, 7).await;
    let bot_replies: Vec<&str> = msgs
        .iter()
        .filter(|m| m["is_bot_sender"] == true && m["sender_id"].as_u64() == Some(bot_uid))
        .filter_map(|m| m["text"].as_str())
        .skip(1) // skip initial welcome message
        .collect();
    assert_eq!(bot_replies.len(), 3, "expected 3 bot replies, got {bot_replies:?}");
    for reply in bot_replies {
        assert!(!reply.contains(&bot_uid_str), "reply leaked numeric user id: {reply}");
        assert!(!reply.contains("You are 1 (@1_bot)"), "reply leaked English system prompt: {reply}");
        assert!(!reply.contains("Received:"), "reply leaked debug echo: {reply}");
        assert!(!reply.contains("Configure an upstream API key"), "reply leaked API key tip: {reply}");
    }
}
