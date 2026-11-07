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

    // 1. POST /api/v1/messages/send
    let req_send = Request::builder()
        .method("POST")
        .uri("/api/v1/messages/send")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"chat_id": 42, "text": "Hello world"}"#))
        .unwrap();
    let res = app.clone().oneshot(req_send).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 2. POST /api/v1/chats/create
    let req_chat = Request::builder()
        .method("POST")
        .uri("/api/v1/chats/create")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"title": "Rust Developers"}"#))
        .unwrap();
    let res = app.clone().oneshot(req_chat).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

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

    // guard drops here (or on panic unwind), cleanly deleting the isolated temporary directory
    // and all SQLite artifacts (test_persist.db, -wal, -shm).
}



