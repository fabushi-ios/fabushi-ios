use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use gpui_telegram_server::{
    auth::{hash_password, verify_password},
    build_router,
    server::AppState,
    ws::GatewayHub,
    ServerConfig,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::mpsc;
use tower::ServiceExt;

#[tokio::test]
async fn test_health_and_status() {
    let app = build_router();

    // 1. GET /health
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"OK");

    // 2. GET /api/v1/status
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["server"], "gpui_telegram_server");
}

#[tokio::test]
async fn test_argon2id_password_hashing() {
    let password = "SuperSecretPassword123!";
    let hashed = hash_password(password).expect("Hashing should succeed");

    assert!(hashed.starts_with("$argon2id$"));
    assert!(verify_password(password, &hashed));
    assert!(!verify_password("WrongPassword!", &hashed));
    assert!(!verify_password("", &hashed));
}

#[tokio::test]
async fn test_auth_registration_and_login() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state);

    // 1. Register a new user
    let register_body = json!({
        "username": "alice_test",
        "password": "PasswordAlice456!",
        "first_name": "Alice",
        "last_name": "Smith",
        "device_name": "Alice Desktop"
    });

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&register_body).unwrap()))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let reg_json: Value = serde_json::from_slice(&body).unwrap();
    let token = reg_json["token"].as_str().unwrap().to_string();
    let user_id = reg_json["user_id"].as_u64().unwrap();
    assert_eq!(reg_json["username"], "alice_test");

    // 2. Reject duplicate registration
    let req_dup = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&register_body).unwrap()))
        .unwrap();

    let res_dup = app.clone().oneshot(req_dup).await.unwrap();
    assert_eq!(res_dup.status(), StatusCode::CONFLICT);

    // 3. Login with correct credentials
    let login_body = json!({
        "username": "alice_test",
        "password": "PasswordAlice456!",
        "device_name": "Alice Laptop"
    });

    let req_login = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
        .unwrap();

    let res_login = app.clone().oneshot(req_login).await.unwrap();
    assert_eq!(res_login.status(), StatusCode::OK);
    let body = to_bytes(res_login.into_body(), usize::MAX).await.unwrap();
    let login_json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(login_json["user_id"], user_id);

    // 4. Login with wrong password
    let bad_login_body = json!({
        "username": "alice_test",
        "password": "WrongPassword!",
        "device_name": "Alice Laptop"
    });

    let req_bad = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&bad_login_body).unwrap()))
        .unwrap();

    let res_bad = app.clone().oneshot(req_bad).await.unwrap();
    assert_eq!(res_bad.status(), StatusCode::UNAUTHORIZED);

    // 5. Access protected /api/v1/auth/me with Bearer token
    let req_me = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let res_me = app.clone().oneshot(req_me).await.unwrap();
    assert_eq!(res_me.status(), StatusCode::OK);
    let body = to_bytes(res_me.into_body(), usize::MAX).await.unwrap();
    let me_json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(me_json["user_id"], user_id);
    assert_eq!(me_json["username"], "alice_test");
}

#[tokio::test]
async fn test_auth_bearer_rejection_on_invalid_or_missing_tokens() {
    let app = build_router();

    // 1. Missing token
    let req_missing = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .body(Body::empty())
        .unwrap();

    let res_missing = app.clone().oneshot(req_missing).await.unwrap();
    assert_eq!(res_missing.status(), StatusCode::UNAUTHORIZED);

    // 2. Invalid token
    let req_invalid = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, "Bearer invalid_nonexistent_token_123")
        .body(Body::empty())
        .unwrap();

    let res_invalid = app.clone().oneshot(req_invalid).await.unwrap();
    assert_eq!(res_invalid.status(), StatusCode::UNAUTHORIZED);

    // 3. Query param token works for protected route
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app_with_state = gpui_telegram_server::server::build_router_with_state(state.clone());

    let (_user, _session, raw_token) = state
        .session_manager
        .register(
            "bob_test",
            "ValidPassword123!",
            "Bob",
            None,
            "Bob Phone",
        )
        .unwrap();

    let req_query = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/me?token={}", raw_token))
        .body(Body::empty())
        .unwrap();

    let res_query = app_with_state.oneshot(req_query).await.unwrap();
    assert_eq!(res_query.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_multi_device_session_revocation() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    // Register user
    let (user, session1, token1) = state
        .session_manager
        .register(
            "charlie_test",
            "CharliePassword123!",
            "Charlie",
            None,
            "Device 1",
        )
        .unwrap();

    // Add a second session for same user
    let (_session2, token2) = state.session_manager.create_session(
        user.user_id,
        "Device 2",
        chrono::Duration::days(30),
    );

    // Verify both tokens work
    let req1 = Request::builder()
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", token1))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req1).await.unwrap().status(), StatusCode::OK);

    let req2 = Request::builder()
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", token2))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req2).await.unwrap().status(), StatusCode::OK);

    // Check sessions endpoint
    let req_sessions = Request::builder()
        .uri("/api/v1/auth/sessions")
        .header(header::AUTHORIZATION, format!("Bearer {}", token1))
        .body(Body::empty())
        .unwrap();
    let res_sessions = app.clone().oneshot(req_sessions).await.unwrap();
    assert_eq!(res_sessions.status(), StatusCode::OK);
    let body = to_bytes(res_sessions.into_body(), usize::MAX).await.unwrap();
    let sess_json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(sess_json["sessions"].as_array().unwrap().len(), 2);

    // Revoke session 1
    let req_revoke = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/auth/sessions/{}", session1.session_id))
        .header(header::AUTHORIZATION, format!("Bearer {}", token1))
        .body(Body::empty())
        .unwrap();
    let res_revoke = app.clone().oneshot(req_revoke).await.unwrap();
    assert_eq!(res_revoke.status(), StatusCode::OK);

    // Now token1 MUST be rejected as revoked
    let req_revoked_check = Request::builder()
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", token1))
        .body(Body::empty())
        .unwrap();
    let res_revoked = app.clone().oneshot(req_revoked_check).await.unwrap();
    assert_eq!(res_revoked.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_gateway_hub_frame_dispatch_and_broadcast() {
    let hub = Arc::new(GatewayHub::new());
    let user_id = UserId(200_001);
    let (tx1, mut rx1) = mpsc::unbounded_channel::<WsServerFrame>();
    let (tx2, mut rx2) = mpsc::unbounded_channel::<WsServerFrame>();

    // Register two devices for same user
    hub.register(user_id, tx1.clone());
    hub.register(user_id, tx2);

    assert_eq!(hub.user_connection_count(user_id), 2);
    assert_eq!(hub.total_connections(), 2);

    // 1. Test Ping/Pong frame
    hub.handle_client_frame(user_id, WsClientFrame::Ping { nonce: 98765 }, &tx1);
    let frame = rx1.recv().await.unwrap();
    match frame {
        WsServerFrame::Pong { nonce } => assert_eq!(nonce, 98765),
        other => panic!("Expected Pong, got {:?}", other),
    }

    // 2. Test typing event broadcast without advancing PTS
    let initial_pts = hub.current_pts();
    hub.subscribe_chat(100, user_id);
    let typing = gpui_telegram_proto::sync::TypingEvent::new(
        gpui_telegram_core::types::ChatId(100),
        user_id,
        gpui_telegram_proto::sync::TypingAction::Typing,
    );
    hub.handle_client_frame(user_id, WsClientFrame::SendTyping(typing), &tx1);

    // PTS must NOT have advanced for typing event
    assert_eq!(hub.current_pts(), initial_pts);

    // Active sockets should receive the typing event
    let frame1_typing = rx1.recv().await.unwrap();
    match frame1_typing {
        WsServerFrame::Update(gpui_telegram_proto::sync::UpdateEvent::UserTyping(t)) => {
            assert_eq!(t.chat_id.0, 100);
            assert_eq!(t.user_id, user_id);
        }
        other => panic!("Expected UserTyping update on rx1, got {:?}", other),
    }

    let frame2 = rx2.recv().await.unwrap();
    match frame2 {
        WsServerFrame::Update(gpui_telegram_proto::sync::UpdateEvent::UserTyping(t)) => {
            assert_eq!(t.chat_id.0, 100);
            assert_eq!(t.user_id, user_id);
        }
        other => panic!("Expected UserTyping update on rx2, got {:?}", other),
    }

    // 3. Test multi-device revocation disconnect signal
    hub.disconnect_user(user_id);
    let err_frame = rx1.recv().await.unwrap();
    match err_frame {
        WsServerFrame::Error { code, .. } => assert_eq!(code, 401),
        other => panic!("Expected Error 401, got {:?}", other),
    }
}

#[tokio::test]
async fn test_protected_routes_matrix() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    let (_user, _session, token) = state
        .session_manager
        .register(
            "dan_test",
            "DanPassword789!",
            "Dan",
            None,
            "Dan Workstation",
        )
        .unwrap();

    let bearer = format!("Bearer {}", token);

    // 1. POST /api/v1/chats/create (create durable group chat before sending message)
    let req_chat = Request::builder()
        .method("POST")
        .uri("/api/v1/chats/create")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"title": "Rust Developers"}"#))
        .unwrap();
    let res = app.clone().oneshot(req_chat).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let chat_bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let chat_json: serde_json::Value = serde_json::from_slice(&chat_bytes).unwrap();
    let created_chat_id = chat_json["chat_id"].as_i64().unwrap();

    // 2. POST /api/v1/messages/send
    let req_send = Request::builder()
        .method("POST")
        .uri("/api/v1/messages/send")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({"chat_id": created_chat_id, "text": "Hello world"}).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req_send).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 3. GET /api/v1/sync/difference
    let req_sync = Request::builder()
        .method("GET")
        .uri("/api/v1/sync/difference")
        .header(header::AUTHORIZATION, &bearer)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req_sync).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 4. POST /api/v1/media/upload/init
    let req_media = Request::builder()
        .method("POST")
        .uri("/api/v1/media/upload/init")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"filename": "doc.pdf", "size_bytes": 1024}"#))
        .unwrap();
    let res = app.clone().oneshot(req_media).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // 5. GET /api/v1/webrtc/turn_credentials
    let req_turn = Request::builder()
        .method("GET")
        .uri("/api/v1/webrtc/turn_credentials")
        .header(header::AUTHORIZATION, &bearer)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req_turn).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 6. GET /api/v1/search/messages
    let req_search = Request::builder()
        .method("GET")
        .uri("/api/v1/search/messages?q=rust")
        .header(header::AUTHORIZATION, &bearer)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req_search).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 7. POST /api/v1/auth/passkey/challenge (Public)
    let req_pk = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/passkey/challenge")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"username": "dan_test"}"#))
        .unwrap();
    let res = app.clone().oneshot(req_pk).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 8. POST /api/v1/auth/pair/init (Public)
    let req_pair = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/pair/init")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"device_name": "Companion"}"#))
        .unwrap();
    let res = app.clone().oneshot(req_pair).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_ws_upgrade_route_auth_rejection_and_acceptance() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    // 1. WS upgrade without token rejected with 401
    let req_no_token = Request::builder()
        .method("GET")
        .uri("/api/v1/ws")
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req_no_token).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // 2. WS upgrade with invalid token rejected with 401
    let req_bad_token = Request::builder()
        .method("GET")
        .uri("/api/v1/ws?token=non_existent_token")
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req_bad_token).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // 3. WS upgrade with valid query token accepts and upgrades (101 Switching Protocols)
    let (_user, _session, token) = state
        .session_manager
        .register(
            "eve_test",
            "EvePassword123!",
            "Eve",
            None,
            "Eve Terminal",
        )
        .unwrap();

    let req_valid = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/ws?token={}", token))
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req_valid).await.unwrap();
    // In in-memory oneshot tests without a real TCP socket, passing auth reaches WebSocketUpgrade
    // which responds with 426 Upgrade Required (confirming auth passed!)
    assert_ne!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(res.status(), StatusCode::UPGRADE_REQUIRED);
}

#[tokio::test]
async fn test_websocket_tcp_wire_ping_pong_roundtrip() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::protocol::Message as TungsteniteMessage;

    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    let (_user, _session, token) = state
        .session_manager
        .register(
            "frank_ws_test",
            "FrankPassword123!",
            "Frank",
            None,
            "Frank Terminal",
        )
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let ws_url = format!("ws://{}/api/v1/ws?token={}", addr, token);
    let (mut ws_stream, _) = connect_async(&ws_url)
        .await
        .expect("WebSocket connection should succeed");

    // Send Ping frame
    let ping_frame = WsClientFrame::Ping { nonce: 1234567 };
    let ping_json = serde_json::to_string(&ping_frame).unwrap();
    ws_stream
        .send(TungsteniteMessage::Text(ping_json.into()))
        .await
        .unwrap();

    // Receive Pong frame
    let resp_msg = ws_stream.next().await.unwrap().unwrap();
    match resp_msg {
        TungsteniteMessage::Text(text) => {
            let server_frame: WsServerFrame = serde_json::from_str(&text).unwrap();
            match server_frame {
                WsServerFrame::Pong { nonce } => assert_eq!(nonce, 1234567),
                other => panic!("Expected Pong frame, got {:?}", other),
            }
        }
        other => panic!("Expected text message, got {:?}", other),
    }

    server_handle.abort();
}

/// RAII cleanup guard that creates an isolated temporary directory for SQLite persistence tests
/// and automatically unlinks the directory and all database, WAL, and SHM files on drop (even on panic).
struct TempDbGuard {
    dir: std::path::PathBuf,
    db_path: std::path::PathBuf,
}

impl TempDbGuard {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("gpui_test_persist_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("Failed to create isolated temp directory for test DB");
        let db_path = dir.join("test_persist.db");
        Self { dir, db_path }
    }

    fn db_url(&self) -> String {
        format!("sqlite://{}", self.db_path.display())
    }
}

impl Drop for TempDbGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn test_sqlite_persistence_across_restart() {
    let guard = TempDbGuard::new();
    let db_url = guard.db_url();

    let mut config = ServerConfig::default();
    config.database_url = db_url;

    // 1. Initial server instance: register user
    {
        let state1 = Arc::new(AppState::new(config.clone()));
        let app1 = gpui_telegram_server::server::build_router_with_state(state1);

        let register_body = json!({
            "username": "restart_persisted_user",
            "password": "CorrectPassword123!",
            "first_name": "Persist",
            "last_name": "Tester",
            "device_name": "Instance 1"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&register_body).unwrap()))
            .unwrap();

        let res = app1.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        // state1 and app1 are dropped here
    }

    // 2. Simulated daemon restart: instantiate a completely new AppState pointing to same DB
    {
        let state2 = Arc::new(AppState::new(config.clone()));
        let app2 = gpui_telegram_server::server::build_router_with_state(state2);

        // Login with previously registered credentials
        let login_body = json!({
            "username": "restart_persisted_user",
            "password": "CorrectPassword123!",
            "device_name": "Instance 2 Post Restart"
        });

        let req_login = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
            .unwrap();

        let res_login = app2.clone().oneshot(req_login).await.unwrap();
        assert_eq!(res_login.status(), StatusCode::OK);

        let body = to_bytes(res_login.into_body(), usize::MAX).await.unwrap();
        let login_json: Value = serde_json::from_slice(&body).unwrap();
        let token = login_json["token"].as_str().unwrap().to_string();
        assert_eq!(login_json["username"], "restart_persisted_user");

        // Verify GET /api/v1/auth/me works with restored session
        let req_me = Request::builder()
            .method("GET")
            .uri("/api/v1/auth/me")
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();

        let res_me = app2.oneshot(req_me).await.unwrap();
        assert_eq!(res_me.status(), StatusCode::OK);
        let me_body = to_bytes(res_me.into_body(), usize::MAX).await.unwrap();
        let me_json: Value = serde_json::from_slice(&me_body).unwrap();
        assert_eq!(me_json["username"], "restart_persisted_user");
        assert_eq!(me_json["first_name"], "Persist");
    }
}

#[tokio::test]
async fn test_email_challenge_flow() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    // 1. Send code to valid email
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"email": "test_user@example.com"}"#))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["ok"], true);
    assert!(json["expires_at_ms"].is_number());
    assert!(json["resend_after_ms"].is_number());

    // 2. Immediate resend must be rejected with 429 Too Many Requests
    let req_rate = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"email": "test_user@example.com"}"#))
        .unwrap();

    let res_rate = app.clone().oneshot(req_rate).await.unwrap();
    assert_eq!(res_rate.status(), StatusCode::TOO_MANY_REQUESTS);

    // 3. Invalid email format must be rejected with 400 Bad Request
    let req_bad = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"email": "not-an-email"}"#))
        .unwrap();

    let res_bad = app.clone().oneshot(req_bad).await.unwrap();
    assert_eq!(res_bad.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_email_registration_flow() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    // 1. Request verification code
    let req_send = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/send-code")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"email": "alice_register@example.com"}"#))
        .unwrap();

    let res_send = app.clone().oneshot(req_send).await.unwrap();
    assert_eq!(res_send.status(), StatusCode::OK);
    let body_send = to_bytes(res_send.into_body(), usize::MAX).await.unwrap();
    let json_send: Value = serde_json::from_slice(&body_send).unwrap();
    let debug_code = json_send["debug_code"].as_str().expect("debug_code available in tests").to_string();

    // 2. Reject registration with wrong verification code
    let bad_code_body = json!({
        "email": "alice_register@example.com",
        "code": "000000",
        "username": "alice_registered",
        "password": "PasswordAlice123!",
        "first_name": "Alice"
    });
    let req_bad_code = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&bad_code_body).unwrap()))
        .unwrap();
    let res_bad_code = app.clone().oneshot(req_bad_code).await.unwrap();
    assert_eq!(res_bad_code.status(), StatusCode::BAD_REQUEST);

    // 3. Successful registration with valid code
    let valid_body = json!({
        "email": "alice_register@example.com",
        "code": debug_code,
        "username": "alice_registered",
        "password": "PasswordAlice123!",
        "first_name": "Alice",
        "device_name": "Alice Laptop"
    });
    let req_reg = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&valid_body).unwrap()))
        .unwrap();
    let res_reg = app.clone().oneshot(req_reg).await.unwrap();
    assert_eq!(res_reg.status(), StatusCode::CREATED);
    let body_reg = to_bytes(res_reg.into_body(), usize::MAX).await.unwrap();
    let json_reg: Value = serde_json::from_slice(&body_reg).unwrap();
    let token = json_reg["token"].as_str().unwrap();
    assert_eq!(json_reg["username"], "alice_registered");

    // 4. Access /api/v1/auth/me with Bearer token
    let req_me = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let res_me = app.clone().oneshot(req_me).await.unwrap();
    assert_eq!(res_me.status(), StatusCode::OK);

    // 5. Duplicate registration with same email or username rejected
    let req_dup = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/email/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&valid_body).unwrap()))
        .unwrap();
    let res_dup = app.clone().oneshot(req_dup).await.unwrap();
    assert_eq!(res_dup.status(), StatusCode::BAD_REQUEST); // Challenge already consumed or conflict
}

#[tokio::test]
async fn test_alipay_oauth_start_and_poll_pending() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    // 1. Start OAuth attempt
    let req_start = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"device_name": "Test Client"}"#))
        .unwrap();

    let res_start = app.clone().oneshot(req_start).await.unwrap();
    assert_eq!(res_start.status(), StatusCode::OK);
    let body_start = to_bytes(res_start.into_body(), usize::MAX).await.unwrap();
    let json_start: Value = serde_json::from_slice(&body_start).unwrap();
    let attempt_id = json_start["attempt_id"].as_str().unwrap();
    assert!(json_start["auth_url"].as_str().unwrap().contains("openauth.alipay.com"));

    // 2. Poll initial pending status
    let req_poll = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();
    let res_poll = app.clone().oneshot(req_poll).await.unwrap();
    assert_eq!(res_poll.status(), StatusCode::OK);
    let body_poll = to_bytes(res_poll.into_body(), usize::MAX).await.unwrap();
    let json_poll: Value = serde_json::from_slice(&body_poll).unwrap();
    assert_eq!(json_poll["status"], "pending");

    // 3. Poll non-existent attempt returns 404
    let req_missing = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/alipay/poll/non-existent-attempt-id")
        .body(Body::empty())
        .unwrap();
    let res_missing = app.clone().oneshot(req_missing).await.unwrap();
    assert_eq!(res_missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_alipay_oauth_simulate_scan_and_atomic_destruction() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    // 1. Start OAuth
    let req_start = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{}"#))
        .unwrap();
    let res_start = app.clone().oneshot(req_start).await.unwrap();
    let body_start = to_bytes(res_start.into_body(), usize::MAX).await.unwrap();
    let json_start: Value = serde_json::from_slice(&body_start).unwrap();
    let attempt_id = json_start["attempt_id"].as_str().unwrap();

    // 2. Simulate scan
    let sim_body = json!({
        "attempt_id": attempt_id,
        "mock_alipay_user_id": "alipay_scan_user_1",
        "mock_nickname": "支付宝扫码用户"
    });
    let req_sim = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/simulate-scan")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&sim_body).unwrap()))
        .unwrap();
    let res_sim = app.clone().oneshot(req_sim).await.unwrap();
    assert_eq!(res_sim.status(), StatusCode::OK);

    // 3. First poll: completed with session delivery
    let req_poll1 = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();
    let res_poll1 = app.clone().oneshot(req_poll1).await.unwrap();
    assert_eq!(res_poll1.status(), StatusCode::OK);
    let body_poll1 = to_bytes(res_poll1.into_body(), usize::MAX).await.unwrap();
    let json_poll1: Value = serde_json::from_slice(&body_poll1).unwrap();
    assert_eq!(json_poll1["status"], "completed");
    let token = json_poll1["session"]["token"].as_str().unwrap();

    // 4. Verify token works against /api/v1/auth/me
    let req_me = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req_me).await.unwrap().status(), StatusCode::OK);

    // 5. Second poll: MUST be rejected with 410 Gone (One-time destruction)
    let req_poll2 = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();
    let res_poll2 = app.clone().oneshot(req_poll2).await.unwrap();
    assert_eq!(res_poll2.status(), StatusCode::GONE);
}

#[tokio::test]
async fn test_alipay_oauth_callback_flow() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    // 1. Start OAuth
    let req_start = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{}"#))
        .unwrap();
    let res_start = app.clone().oneshot(req_start).await.unwrap();
    let body_start = to_bytes(res_start.into_body(), usize::MAX).await.unwrap();
    let json_start: Value = serde_json::from_slice(&body_start).unwrap();
    let attempt_id = json_start["attempt_id"].as_str().unwrap();
    let auth_url = json_start["auth_url"].as_str().unwrap();

    // Extract state parameter from auth_url
    let state_param = auth_url.split("state=").nth(1).unwrap().to_string();

    // 2. Trigger callback with auth_code and state
    let cb_body = json!({
        "auth_code": "auth_code_99999",
        "state": state_param
    });
    let req_cb = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/alipay/callback")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&cb_body).unwrap()))
        .unwrap();
    let res_cb = app.clone().oneshot(req_cb).await.unwrap();
    assert_eq!(res_cb.status(), StatusCode::OK);

    // 3. Poll completed session
    let req_poll = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/alipay/poll/{}", attempt_id))
        .body(Body::empty())
        .unwrap();
    let res_poll = app.clone().oneshot(req_poll).await.unwrap();
    assert_eq!(res_poll.status(), StatusCode::OK);
    let body_poll = to_bytes(res_poll.into_body(), usize::MAX).await.unwrap();
    let json_poll: Value = serde_json::from_slice(&body_poll).unwrap();
    assert_eq!(json_poll["status"], "completed");
    assert!(json_poll["session"]["token"].is_string());
    assert!(json_poll["token"].is_string());
    assert!(json_poll["user"]["user_id"].is_u64());
}

#[tokio::test]
async fn test_migrated_google_cloud_alipay_routes_and_email_code_contract() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    // 1. Verify /api/auth/alipay/login-url -> /api/auth/alipay/callback -> /api/auth/alipay/cli-session
    let req_login_url = Request::builder()
        .method("GET")
        .uri("/api/auth/alipay/login-url?platform=cli")
        .body(Body::empty())
        .unwrap();
    let res_login_url = app.clone().oneshot(req_login_url).await.unwrap();
    assert_eq!(res_login_url.status(), StatusCode::OK);
    let body_login_url = to_bytes(res_login_url.into_body(), usize::MAX).await.unwrap();
    let json_login_url: Value = serde_json::from_slice(&body_login_url).unwrap();
    assert_eq!(json_login_url["success"], true);
    let state_token = json_login_url["state"].as_str().unwrap().to_string();
    assert!(state_token.starts_with("fbs_"));

    // Poll cli-session while pending
    let req_cli_pending = Request::builder()
        .method("GET")
        .uri(format!("/api/auth/alipay/cli-session?state={}", state_token))
        .body(Body::empty())
        .unwrap();
    let res_cli_pending = app.clone().oneshot(req_cli_pending).await.unwrap();
    assert_eq!(res_cli_pending.status(), StatusCode::OK);
    let json_cli_pending: Value = serde_json::from_slice(
        &to_bytes(res_cli_pending.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(json_cli_pending["status"], "pending");

    // Complete via GET /api/auth/alipay/callback with HTML Accept header
    let req_cb_html = Request::builder()
        .method("GET")
        .uri(format!(
            "/api/auth/alipay/callback?auth_code=mock_code_777&state={}",
            state_token
        ))
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .unwrap();
    let res_cb_html = app.clone().oneshot(req_cb_html).await.unwrap();
    assert_eq!(res_cb_html.status(), StatusCode::OK);
    let html_bytes = to_bytes(res_cb_html.into_body(), usize::MAX).await.unwrap();
    let html_str = String::from_utf8_lossy(&html_bytes);
    assert!(html_str.contains("支付宝登录成功"));
    assert!(!html_str.contains("flutter.ombhrum.com"));

    // Claim completed cli-session
    let req_cli_done = Request::builder()
        .method("GET")
        .uri(format!("/api/auth/alipay/cli-session?state={}", state_token))
        .body(Body::empty())
        .unwrap();
    let res_cli_done = app.clone().oneshot(req_cli_done).await.unwrap();
    assert_eq!(res_cli_done.status(), StatusCode::OK);
    let json_cli_done: Value = serde_json::from_slice(
        &to_bytes(res_cli_done.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(json_cli_done["status"], "complete");
    assert!(json_cli_done["accessToken"].as_str().unwrap().len() > 10);

    // 2. Verify /api/auth/alipay/sdk-login & /api/auth/alipay/register & /api/auth/alipay/auth-string
    let req_sdk = Request::builder()
        .method("POST")
        .uri("/api/auth/alipay/sdk-login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"authCode":"mock_sdk_code_123"}"#))
        .unwrap();
    let res_sdk = app.clone().oneshot(req_sdk).await.unwrap();
    assert_eq!(res_sdk.status(), StatusCode::OK);

    let req_reg = Request::builder()
        .method("POST")
        .uri("/api/auth/alipay/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"alipayUserId":"2088102188887777","nickname":"迁移测试用户"}"#,
        ))
        .unwrap();
    let res_reg = app.clone().oneshot(req_reg).await.unwrap();
    assert_eq!(res_reg.status(), StatusCode::CREATED);

    let req_auth_str = Request::builder()
        .method("GET")
        .uri("/api/auth/alipay/auth-string")
        .body(Body::empty())
        .unwrap();
    let res_auth_str = app.clone().oneshot(req_auth_str).await.unwrap();
    assert_eq!(res_auth_str.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_welcome_bot_provisioning_contacts_groups_and_categorized_search() {
    let app = build_router();

    // 1. Register a brand new user
    let reg_body = json!({
        "username": "founder_tg",
        "password": "StrongPassword!1",
        "first_name": "Pavel",
        "last_name": "Durov",
        "device_name": "Telegram GPUI Mac"
    });
    let req_reg = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&reg_body).unwrap()))
        .unwrap();
    let res_reg = app.clone().oneshot(req_reg).await.unwrap();
    assert_eq!(res_reg.status(), StatusCode::CREATED);
    let reg_json: Value =
        serde_json::from_slice(&to_bytes(res_reg.into_body(), usize::MAX).await.unwrap()).unwrap();
    let token = reg_json["token"].as_str().unwrap().to_string();

    // 2. Fetch /api/v1/chats -> newly registered account starts clean (empty chats), then opt-in via POST /api/v1/chats/welcome
    let req_chats_init = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let res_chats_init = app.clone().oneshot(req_chats_init).await.unwrap();
    assert_eq!(res_chats_init.status(), StatusCode::OK);
    let chats_init_json: Value =
        serde_json::from_slice(&to_bytes(res_chats_init.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(chats_init_json["chats"].as_array().unwrap().len(), 0);
    assert_eq!(chats_init_json["total"], 0);

    let req_welcome = Request::builder()
        .method("POST")
        .uri("/api/v1/chats/welcome")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let res_welcome = app.clone().oneshot(req_welcome).await.unwrap();
    assert_eq!(res_welcome.status(), StatusCode::CREATED);

    let req_chats = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let res_chats = app.clone().oneshot(req_chats).await.unwrap();
    assert_eq!(res_chats.status(), StatusCode::OK);
    let chats_json: Value =
        serde_json::from_slice(&to_bytes(res_chats.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    let chats_arr = chats_json["chats"].as_array().unwrap();
    assert!(!chats_arr.is_empty(), "Opt-in /chats/welcome must provision Official Welcome Bot chat");
    let welcome_chat = chats_arr
        .iter()
        .find(|c| c["title"] == "Telegram Official Bot")
        .expect("Telegram Official Bot chat must be provisioned");
    assert_eq!(welcome_chat["is_bot"], true);
    let welcome_chat_id = welcome_chat["chat_id"].as_i64().unwrap();

    // Verify welcome messages exist in /api/v1/messages/history
    let req_hist = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/messages/history?chat_id={}", welcome_chat_id))
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let res_hist = app.clone().oneshot(req_hist).await.unwrap();
    assert_eq!(res_hist.status(), StatusCode::OK);
    let hist_json: Value =
        serde_json::from_slice(&to_bytes(res_hist.into_body(), usize::MAX).await.unwrap()).unwrap();
    let msgs = hist_json["messages"].as_array().unwrap();
    assert!(msgs.len() >= 2, "Official Welcome Bot must send initial welcome messages");

    // 3. Add AI Bot contact via POST /api/v1/contacts/add
    let add_bot_body = json!({
        "first_name": "Rust Code Assistant",
        "username": "rust_code_bot",
        "is_bot": true,
        "bot_system_prompt": "Expert Rust GPUI engineer"
    });
    let req_add_bot = Request::builder()
        .method("POST")
        .uri("/api/v1/contacts/add")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&add_bot_body).unwrap()))
        .unwrap();
    let res_add_bot = app.clone().oneshot(req_add_bot).await.unwrap();
    assert_eq!(res_add_bot.status(), StatusCode::CREATED);
    let bot_json: Value =
        serde_json::from_slice(&to_bytes(res_add_bot.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(bot_json["is_bot"], true);
    assert_eq!(bot_json["title"], "Rust Code Assistant");

    // 4. Add human friend contact via POST /api/v1/contacts/add
    let add_friend_body = json!({
        "first_name": "Nikolai",
        "last_name": "Durov",
        "username": "nikolai_d",
        "phone": "+15550199",
        "is_bot": false
    });
    let req_add_friend = Request::builder()
        .method("POST")
        .uri("/api/v1/contacts/add")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&add_friend_body).unwrap()))
        .unwrap();
    let res_add_friend = app.clone().oneshot(req_add_friend).await.unwrap();
    assert_eq!(res_add_friend.status(), StatusCode::CREATED);

    // 5. Create group with member_usernames and about via POST /api/v1/chats/create
    let create_group_body = json!({
        "title": "Telegram Core Rust Port",
        "member_usernames": ["@rust_code_bot", "nikolai_d"],
        "about": "Full SourceFiles 1:1 migration"
    });
    let req_group = Request::builder()
        .method("POST")
        .uri("/api/v1/chats/create")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&create_group_body).unwrap()))
        .unwrap();
    let res_group = app.clone().oneshot(req_group).await.unwrap();
    assert_eq!(res_group.status(), StatusCode::CREATED);

    // 6. Categorized search via GET /api/v1/search/messages
    let req_search_apps = Request::builder()
        .method("GET")
        .uri("/api/v1/search/messages?q=&category=Apps")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let res_search_apps = app.clone().oneshot(req_search_apps).await.unwrap();
    assert_eq!(res_search_apps.status(), StatusCode::OK);
    let search_apps_json: Value =
        serde_json::from_slice(&to_bytes(res_search_apps.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    let app_results = search_apps_json["results"].as_array().unwrap();
    assert!(
        app_results.iter().any(|r| r["title"] == "Telegram Official Bot"),
        "Apps category search must return Official Bot"
    );
    assert!(
        app_results.iter().any(|r| r["title"] == "Rust Code Assistant"),
        "Apps category search must return created AI Bot"
    );
    assert!(
        !app_results.iter().any(|r| r["title"] == "Nikolai Durov"),
        "Human contact (750_000_000+) must NOT be misclassified as an AI Bot in Apps category search"
    );

    // 7. Register a second user in the same database and verify multi-user Welcome Bot & contact creation
    let reg2_body = json!({
        "username": "second_user_tg",
        "password": "StrongPassword!2",
        "first_name": "AAA",
        "device_name": "Telegram GPUI Mac 2",
        "auto_welcome": true
    });
    let req_reg2 = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&reg2_body).unwrap()))
        .unwrap();
    let res_reg2 = app.clone().oneshot(req_reg2).await.unwrap();
    assert_eq!(res_reg2.status(), StatusCode::CREATED);
    let reg2_json: Value =
        serde_json::from_slice(&to_bytes(res_reg2.into_body(), usize::MAX).await.unwrap()).unwrap();
    let token2 = reg2_json["token"].as_str().unwrap().to_string();

    let req_chats2 = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, format!("Bearer {}", token2))
        .body(Body::empty())
        .unwrap();
    let res_chats2 = app.clone().oneshot(req_chats2).await.unwrap();
    assert_eq!(res_chats2.status(), StatusCode::OK);
    let chats2_json: Value =
        serde_json::from_slice(&to_bytes(res_chats2.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    let chats2_arr = chats2_json["chats"].as_array().unwrap();
    let welcome_chat2 = chats2_arr
        .iter()
        .find(|c| c["title"] == "Telegram Official Bot")
        .expect("Second user must also get Telegram Official Bot without UNIQUE constraint collision");
    assert_eq!(welcome_chat2["username"], "telegram_official_bot");
    assert_ne!(welcome_chat2["chat_id"], welcome_chat["chat_id"]);
}

#[tokio::test]
async fn test_auth_registration_and_login_extended_bounds() {
    use std::path::PathBuf;
    use uuid::Uuid;

    struct AuthBoundsTempDir(PathBuf);
    impl Drop for AuthBoundsTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let root = std::env::temp_dir().join(format!("gpui_auth_bounds_{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&root).expect("create temp test dir");
    let _dir = AuthBoundsTempDir(root.clone());

    let cfg = ServerConfig {
        database_url: root.join("auth_bounds.db").to_string_lossy().into_owned(),
        media_storage_path: root.join("cas").to_string_lossy().into_owned(),
        ..Default::default()
    };
    let state = Arc::new(AppState::try_new(cfg.clone()).expect("init AppState"));
    let app = gpui_telegram_server::server::build_router_with_state(state);

    // 1. 70-char username and 512-byte Argon2id password register cleanly and can log in
    let user_70 = "u".repeat(70);
    let pass_512 = "P".repeat(512);
    let res_70 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "username": user_70,
                        "password": pass_512,
                        "first_name": "Extended"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_70.status(), StatusCode::CREATED);
    let body_70: Value =
        serde_json::from_slice(&to_bytes(res_70.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body_70["username"], user_70);

    let login_70 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "username": user_70,
                        "password": pass_512,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_70.status(), StatusCode::OK);

    // 2. Exact upper bounds: 128-char username and 1024-byte password succeed
    let user_128 = "a".repeat(128);
    let pass_1024 = "B".repeat(1024);
    let res_128 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "username": user_128,
                        "password": pass_1024,
                        "first_name": "Boundary"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_128.status(), StatusCode::CREATED);

    let hash_1024 = hash_password(&pass_1024).expect("1024-byte password must hash");
    assert!(verify_password(&pass_1024, &hash_1024));

    // Reopen durable SQLite to ensure login works on SQLite fallback path for extended bounds too
    drop(app);
    let reopened = Arc::new(AppState::try_new(cfg).expect("reopen AppState"));
    let reopened_app = gpui_telegram_server::server::build_router_with_state(reopened);
    let login_128 = reopened_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "username": user_128,
                        "password": pass_1024,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_128.status(), StatusCode::OK);

    // 3. Reject 129-char username and 1025-byte password
    let user_129 = "x".repeat(129);
    let res_129 = reopened_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "username": user_129,
                        "password": "ValidPassword123!",
                        "first_name": "TooLong"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_129.status(), StatusCode::BAD_REQUEST);
    let err_129: Value =
        serde_json::from_slice(&to_bytes(res_129.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(err_129["message"], "username must be 1..=128 characters");

    let pass_1025 = "Z".repeat(1025);
    let res_1025 = reopened_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "username": "valid_user_1025",
                        "password": pass_1025,
                        "first_name": "TooLongPass"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_1025.status(), StatusCode::BAD_REQUEST);
    let err_1025: Value =
        serde_json::from_slice(&to_bytes(res_1025.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(err_1025["message"], "password must not exceed 1024 bytes");

    assert!(hash_password(&pass_1025).is_err());
    assert!(!verify_password(&pass_1025, &hash_1024));
    drop(reopened_app);
}
