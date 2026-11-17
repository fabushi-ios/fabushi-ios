//! FUS-S003: Route-level integration tests for authenticated, member-scoped
//! durable HTTP messaging (`POST /api/v1/messages/send` and `GET /api/v1/messages/history`).

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use gpui_telegram_core::types::{ChatId, MsgId, TopicId, UserId};
use gpui_telegram_server::{
    config::ServerConfig,
    db::{ChatMemberRecord, ForumTopicRecord, ServerDatabase},
    server::{build_router_with_state, AppState},
};
use rusqlite::params;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

struct TempDbDirGuard {
    dir: PathBuf,
}

impl TempDbDirGuard {
    fn new(prefix: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("{}_{}", prefix, Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        Self { dir }
    }

    fn db_path(&self) -> PathBuf {
        self.dir.join("fusion_s003.sqlite")
    }
}

impl Drop for TempDbDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn new_in_memory_state() -> Arc<AppState> {
    let mut cfg = ServerConfig::default();
    cfg.database_url = ":memory:".to_string();
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open in-memory db"));
    Arc::new(AppState::with_db(cfg, db))
}

async fn send_json_request(
    state: &Arc<AppState>,
    method: Method,
    uri: &str,
    bearer_token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let raw_bytes = body.map(|v| serde_json::to_vec(&v).expect("serialize json body"));
    send_raw_request(state, method, uri, bearer_token, raw_bytes).await
}

async fn send_raw_request(
    state: &Arc<AppState>,
    method: Method,
    uri: &str,
    bearer_token: Option<&str>,
    raw_body: Option<Vec<u8>>,
) -> (StatusCode, Value) {
    let app = build_router_with_state(state.clone());
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer_token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", token));
    }
    let req_body = match raw_body {
        Some(bytes) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(bytes)
        }
        None => Body::empty(),
    };

    let req = builder.body(req_body).expect("build request");
    let resp = app.oneshot(req).await.expect("execute oneshot request");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .expect("read response body");
    let json_val = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            json!({ "_raw_body": String::from_utf8_lossy(&bytes).to_string() })
        })
    };
    (status, json_val)
}

async fn register_test_user(state: &Arc<AppState>, phone: &str, first_name: &str) -> (u64, String) {
    let username = format!("test_{}", phone.chars().filter(|c| c.is_ascii_digit()).collect::<String>());
    let (status_verify, verify_body) = send_json_request(
        state,
        Method::POST,
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": username,
            "password": format!("Pass_{}_98765!", username),
            "first_name": first_name,
            "device_name": "Fusion Test",
        })),
    )
    .await;
    assert_eq!(status_verify, StatusCode::CREATED, "registration failed: {}", verify_body);

    let user_id = verify_body["user_id"].as_u64().expect("user_id in verify_code");
    let token = verify_body["token"]
        .as_str()
        .expect("token in verify_code")
        .to_string();
    (user_id, token)
}

async fn create_test_chat(state: &Arc<AppState>, token: &str, title: &str) -> i64 {
    let (status, body) = send_json_request(
        state,
        Method::POST,
        "/api/v1/chats",
        Some(token),
        Some(json!({ "title": title })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create chat failed: {}", body);
    body["chat_id"].as_i64().expect("chat_id in create chat")
}

#[tokio::test]
async fn test_auth_bearer_required_for_messaging_routes() {
    let state = new_in_memory_state();

    // Missing bearer token -> 401
    let (status_send_no_auth, _) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        None,
        Some(json!({ "chat_id": 1, "text": "hello" })),
    )
    .await;
    assert_eq!(status_send_no_auth, StatusCode::UNAUTHORIZED);

    let (status_hist_no_auth, _) =
        send_json_request(&state, Method::GET, "/api/v1/messages/history?chat_id=1", None, None)
            .await;
    assert_eq!(status_hist_no_auth, StatusCode::UNAUTHORIZED);

    // Invalid bearer token -> 401
    let (status_send_bad_auth, _) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some("invalid-session-token"),
        Some(json!({ "chat_id": 1, "text": "hello" })),
    )
    .await;
    assert_eq!(status_send_bad_auth, StatusCode::UNAUTHORIZED);

    let (status_hist_bad_auth, _) = send_json_request(
        &state,
        Method::GET,
        "/api/v1/messages/history?chat_id=1",
        Some("invalid-session-token"),
        None,
    )
    .await;
    assert_eq!(status_hist_bad_auth, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_member_send_and_history_roundtrip_with_chat_metadata_update() {
    let state = new_in_memory_state();
    let (alice_id, alice_token) = register_test_user(&state, "+15550003001", "Alice").await;
    let (bob_id, bob_token) = register_test_user(&state, "+15550003002", "Bob").await;

    let chat_id = create_test_chat(&state, &alice_token, "Engineering Core").await;

    // Add Bob as an active member of the chat
    state
        .db
        .add_chat_member(&ChatMemberRecord {
            chat_id: ChatId(chat_id),
            user_id: UserId(bob_id),
            role: "member".to_string(),
            permissions_bitmask: 0xFFFF_FFFF,
            admin_title: None,
            admin_rights: 0,
            banned_rights: 0,
            until_date_ms: None,
            joined_at_ms: 1_700_000_000_000,
            invited_by: Some(UserId(alice_id)),
            updated_at_ms: 1_700_000_000_000,
        })
        .expect("add Bob to chat");

    // Initially history is empty with current_pts == 0
    let (status_empty, empty_hist) = send_json_request(
        &state,
        Method::GET,
        &format!("/api/v1/messages/history?chat_id={}", chat_id),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(status_empty, StatusCode::OK);
    assert_eq!(empty_hist["messages"].as_array().unwrap().len(), 0);
    assert_eq!(empty_hist["has_more"], false);
    assert_eq!(empty_hist["current_pts"], 0);

    // Alice sends message #1 (including a spoofed sender_id which must be ignored)
    let (status_m1, m1) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id,
            "sender_id": 999_999_999u64,
            "text": "First durable message from Alice",
            "client_nonce": 1001u64,
        })),
    )
    .await;
    assert_eq!(status_m1, StatusCode::OK, "m1 send failed: {}", m1);
    assert_eq!(m1["message_id"], 1);
    assert_eq!(m1["pts"], 1);
    assert_eq!(m1["pts_count"], 1);
    assert_eq!(m1["duplicate"], false);
    assert_eq!(m1["sender_id"], alice_id);
    assert_eq!(m1["chat_id"], chat_id);
    assert_eq!(m1["text"], "First durable message from Alice");
    assert_eq!(m1["client_nonce"], 1001);
    assert_eq!(m1["status"], "sent");

    // Bob replies with message #2
    let (status_m2, m2) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&bob_token),
        Some(json!({
            "chat_id": chat_id,
            "text": "Second durable message from Bob",
            "reply_to": 1,
            "client_nonce": 2002u64,
        })),
    )
    .await;
    assert_eq!(status_m2, StatusCode::OK, "m2 send failed: {}", m2);
    assert_eq!(m2["message_id"], 2);
    assert_eq!(m2["pts"], 2);
    assert_eq!(m2["pts_count"], 1);
    assert_eq!(m2["duplicate"], false);
    assert_eq!(m2["sender_id"], bob_id);
    assert_eq!(m2["reply_to"], 1);

    // Alice fetches history: message 1 is outgoing, message 2 is incoming
    let (status_hist_alice, hist_alice) = send_json_request(
        &state,
        Method::GET,
        &format!("/api/v1/messages/history?chat_id={}", chat_id),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(status_hist_alice, StatusCode::OK);
    assert_eq!(hist_alice["current_pts"], 2);
    assert_eq!(hist_alice["has_more"], false);
    let alice_msgs = hist_alice["messages"].as_array().expect("messages array");
    assert_eq!(alice_msgs.len(), 2);
    assert_eq!(alice_msgs[0]["message_id"], 1);
    assert_eq!(alice_msgs[0]["sender_id"], alice_id);
    assert_eq!(alice_msgs[0]["text"], "First durable message from Alice");
    assert_eq!(alice_msgs[0]["pts"], 1);
    assert_eq!(alice_msgs[0]["is_outgoing"], true);
    assert_eq!(alice_msgs[1]["message_id"], 2);
    assert_eq!(alice_msgs[1]["sender_id"], bob_id);
    assert_eq!(alice_msgs[1]["text"], "Second durable message from Bob");
    assert_eq!(alice_msgs[1]["reply_to"], 1);
    assert_eq!(alice_msgs[1]["pts"], 2);
    assert_eq!(alice_msgs[1]["is_outgoing"], false);

    // Bob fetches history: message 1 is incoming, message 2 is outgoing
    let (status_hist_bob, hist_bob) = send_json_request(
        &state,
        Method::GET,
        &format!("/api/v1/messages/history?chat_id={}", chat_id),
        Some(&bob_token),
        None,
    )
    .await;
    assert_eq!(status_hist_bob, StatusCode::OK);
    let bob_msgs = hist_bob["messages"].as_array().expect("messages array");
    assert_eq!(bob_msgs.len(), 2);
    assert_eq!(bob_msgs[0]["is_outgoing"], false);
    assert_eq!(bob_msgs[1]["is_outgoing"], true);

    // Verify S001 chat route reflects updated pts and last_message_id from MessagePipeline commit
    let (status_chat, chat_body) = send_json_request(
        &state,
        Method::GET,
        &format!("/api/v1/chats/{}", chat_id),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(status_chat, StatusCode::OK);
    assert_eq!(chat_body["pts"], 2);
    assert_eq!(chat_body["last_message_id"], 2);
    assert!(chat_body["last_message_date_ms"].as_i64().unwrap_or(0) > 0);
}

#[tokio::test]
async fn test_nonmember_and_nonexistent_chat_rejected_without_writes() {
    let state = new_in_memory_state();
    let (alice_id, alice_token) = register_test_user(&state, "+15550003011", "Alice").await;
    let (_mallory_id, mallory_token) =
        register_test_user(&state, "+15550003012", "Mallory").await;
    let (kicked_id, kicked_token) = register_test_user(&state, "+15550003013", "KickedUser").await;
    let (muted_id, muted_token) = register_test_user(&state, "+15550003014", "MutedUser").await;

    let chat_id = create_test_chat(&state, &alice_token, "Private Ops").await;

    // Add KickedUser with role = "kicked" and MutedUser with banned_rights bit 0 set
    state
        .db
        .add_chat_member(&ChatMemberRecord {
            chat_id: ChatId(chat_id),
            user_id: UserId(kicked_id),
            role: "kicked".to_string(),
            permissions_bitmask: 0,
            admin_title: None,
            admin_rights: 0,
            banned_rights: 0,
            until_date_ms: None,
            joined_at_ms: 1_700_000_000_000,
            invited_by: Some(UserId(alice_id)),
            updated_at_ms: 1_700_000_000_000,
        })
        .expect("add kicked member");

    state
        .db
        .add_chat_member(&ChatMemberRecord {
            chat_id: ChatId(chat_id),
            user_id: UserId(muted_id),
            role: "restricted".to_string(),
            permissions_bitmask: 0,
            admin_title: None,
            admin_rights: 0,
            banned_rights: 1, // send_messages banned
            until_date_ms: None,
            joined_at_ms: 1_700_000_000_000,
            invited_by: Some(UserId(alice_id)),
            updated_at_ms: 1_700_000_000_000,
        })
        .expect("add muted member");

    // Non-member Mallory attempts to send -> 403 Forbidden
    let (status_mallory_send, _) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&mallory_token),
        Some(json!({
            "chat_id": chat_id,
            "text": "Unauthorized injection",
            "client_nonce": 777,
        })),
    )
    .await;
    assert_eq!(status_mallory_send, StatusCode::FORBIDDEN);

    // Non-member Mallory attempts to read history -> 403 Forbidden
    let (status_mallory_hist, _) = send_json_request(
        &state,
        Method::GET,
        &format!("/api/v1/messages/history?chat_id={}", chat_id),
        Some(&mallory_token),
        None,
    )
    .await;
    assert_eq!(status_mallory_hist, StatusCode::FORBIDDEN);

    // Kicked user attempts to send and read history -> 403 Forbidden
    let (status_kicked_send, _) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&kicked_token),
        Some(json!({ "chat_id": chat_id, "text": "Kicked send" })),
    )
    .await;
    assert_eq!(status_kicked_send, StatusCode::FORBIDDEN);

    let (status_kicked_hist, _) = send_json_request(
        &state,
        Method::GET,
        &format!("/api/v1/messages/history?chat_id={}", chat_id),
        Some(&kicked_token),
        None,
    )
    .await;
    assert_eq!(status_kicked_hist, StatusCode::FORBIDDEN);

    // Muted user attempts to send -> 403 Forbidden (can still read history -> 200 OK)
    let (status_muted_send, _) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&muted_token),
        Some(json!({ "chat_id": chat_id, "text": "Muted send" })),
    )
    .await;
    assert_eq!(status_muted_send, StatusCode::FORBIDDEN);

    let (status_muted_hist, _) = send_json_request(
        &state,
        Method::GET,
        &format!("/api/v1/messages/history?chat_id={}", chat_id),
        Some(&muted_token),
        None,
    )
    .await;
    assert_eq!(status_muted_hist, StatusCode::OK);

    // Non-existent chat -> 404 Not Found on both send and history
    let (status_missing_send, _) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({ "chat_id": 999_999, "text": "Ghost chat" })),
    )
    .await;
    assert_eq!(status_missing_send, StatusCode::NOT_FOUND);

    let (status_missing_hist, _) = send_json_request(
        &state,
        Method::GET,
        "/api/v1/messages/history?chat_id=999999",
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(status_missing_hist, StatusCode::NOT_FOUND);

    // Verify zero messages, PTS updates, or dedup records were persisted
    assert_eq!(state.db.get_current_pts(ChatId(chat_id)).unwrap(), 0);
    assert!(state.db.find_dedup_message(_mallory_id, 777).unwrap().is_none());
}

#[tokio::test]
async fn test_client_nonce_idempotent_retry_and_conflicting_payload_rejection() {
    let state = new_in_memory_state();
    let (_alice_id, alice_token) = register_test_user(&state, "+15550003021", "Alice").await;
    let chat_id_1 = create_test_chat(&state, &alice_token, "Chat One").await;
    let chat_id_2 = create_test_chat(&state, &alice_token, "Chat Two").await;

    // Initial send with client_nonce = 424242
    let (status_first, first_body) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id_1,
            "text": "Idempotent transfer payload",
            "client_nonce": 424242u64,
        })),
    )
    .await;
    assert_eq!(status_first, StatusCode::OK);
    assert_eq!(first_body["message_id"], 1);
    assert_eq!(first_body["pts"], 1);
    assert_eq!(first_body["pts_count"], 1);
    assert_eq!(first_body["duplicate"], false);

    // Exact retry with same client_nonce and identical intent -> 200 OK, duplicate = true, pts_count = 0
    let (status_retry, retry_body) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id_1,
            "text": "Idempotent transfer payload",
            "client_nonce": 424242u64,
        })),
    )
    .await;
    assert_eq!(status_retry, StatusCode::OK);
    assert_eq!(retry_body["message_id"], 1);
    assert_eq!(retry_body["pts"], 1);
    assert_eq!(retry_body["pts_count"], 0);
    assert_eq!(retry_body["duplicate"], true);
    assert_eq!(retry_body["date"], first_body["date"]);

    // Conflicting retry #1: same client_nonce, different text -> 409 Conflict
    let (status_conflict_text, conflict_text_body) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id_1,
            "text": "Mutated text with same nonce",
            "client_nonce": 424242u64,
        })),
    )
    .await;
    assert_eq!(status_conflict_text, StatusCode::CONFLICT);
    assert_eq!(conflict_text_body["error"], "nonce_conflict");

    // Conflicting retry #2: same client_nonce, different chat_id -> 409 Conflict
    let (status_conflict_chat, _) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id_2,
            "text": "Idempotent transfer payload",
            "client_nonce": 424242u64,
        })),
    )
    .await;
    assert_eq!(status_conflict_chat, StatusCode::CONFLICT);

    // Conflicting retry #3: same client_nonce, added reply_to -> 409 Conflict
    let (status_conflict_reply, _) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id_1,
            "text": "Idempotent transfer payload",
            "reply_to": 1,
            "client_nonce": 424242u64,
        })),
    )
    .await;
    assert_eq!(status_conflict_reply, StatusCode::CONFLICT);

    // History in chat_id_1 still has exactly 1 message and current_pts == 1
    let (status_hist, hist_body) = send_json_request(
        &state,
        Method::GET,
        &format!("/api/v1/messages/history?chat_id={}", chat_id_1),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(status_hist, StatusCode::OK);
    assert_eq!(hist_body["current_pts"], 1);
    assert_eq!(hist_body["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_malformed_requests_and_pagination_bounds_rejected() {
    let state = new_in_memory_state();
    let (_alice_id, alice_token) = register_test_user(&state, "+15550003031", "Alice").await;
    let chat_id = create_test_chat(&state, &alice_token, "Validation Lab").await;

    // 1. Empty raw body & malformed JSON on send
    let (status_empty_raw, _) = send_raw_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(Vec::new()),
    )
    .await;
    assert_eq!(status_empty_raw, StatusCode::BAD_REQUEST);

    let (status_bad_json, _) = send_raw_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(b"{not-valid-json".to_vec()),
    )
    .await;
    assert_eq!(status_bad_json, StatusCode::BAD_REQUEST);

    // 2. Invalid chat_id, empty/whitespace text, oversized text (> 4096 chars), invalid client_nonce, invalid reply_to
    let long_text = "x".repeat(4097);
    let bad_send_payloads = vec![
        json!({ "text": "missing chat_id" }),
        json!({ "chat_id": 0, "text": "zero chat_id" }),
        json!({ "chat_id": -5, "text": "negative chat_id" }),
        json!({ "chat_id": "1", "text": "string chat_id" }),
        json!({ "chat_id": chat_id }),
        json!({ "chat_id": chat_id, "text": "" }),
        json!({ "chat_id": chat_id, "text": "   \n\t  " }),
        json!({ "chat_id": chat_id, "text": long_text }),
        json!({ "chat_id": chat_id, "text": "hello", "client_nonce": 0 }),
        json!({ "chat_id": chat_id, "text": "hello", "client_nonce": -10 }),
        json!({ "chat_id": chat_id, "text": "hello", "client_nonce": u64::MAX }),
        json!({ "chat_id": chat_id, "text": "hello", "client_nonce": "abc" }),
        json!({ "chat_id": chat_id, "text": "hello", "reply_to": 0 }),
        json!({ "chat_id": chat_id, "text": "hello", "reply_to": -1 }),
        json!({ "chat_id": chat_id, "text": "hello", "reply_to": 1, "reply_to_id": 2 }),
        json!({ "chat_id": chat_id, "text": "hello", "topic_id": 0 }),
        json!({
            "chat_id": chat_id,
            "text": "hi",
            "entities": [{ "offset": 0, "length": 10, "entity_type": { "type": "Bold" } }]
        }),
        json!({
            "chat_id": chat_id,
            "content": { "type": "Sticker", "data": { "emoji": "😀" } }
        }),
        json!({
            "chat_id": chat_id,
            "content": {
                "type": "Media",
                "data": { "file_id": "", "mime_type": "image/png", "size_bytes": 10 }
            }
        }),
        json!({
            "chat_id": chat_id,
            "content": {
                "type": "Poll",
                "data": {
                    "id": "p1",
                    "question": "   ",
                    "options": [],
                    "total_voters": 0,
                    "is_closed": false,
                    "is_anonymous": true
                }
            }
        }),
    ];

    for payload in bad_send_payloads {
        let (status, body) = send_json_request(
            &state,
            Method::POST,
            "/api/v1/messages/send",
            Some(&alice_token),
            Some(payload.clone()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "Expected 400 for payload {}, got {} ({})",
            payload,
            status,
            body
        );
    }

    // 3. Invalid query parameters on GET /api/v1/messages/history
    let bad_history_uris = vec![
        "/api/v1/messages/history".to_string(),
        "/api/v1/messages/history?chat_id=0".to_string(),
        "/api/v1/messages/history?chat_id=-1".to_string(),
        "/api/v1/messages/history?chat_id=abc".to_string(),
        format!("/api/v1/messages/history?chat_id={}&limit=0", chat_id),
        format!("/api/v1/messages/history?chat_id={}&limit=101", chat_id),
        format!("/api/v1/messages/history?chat_id={}&limit=abc", chat_id),
        format!("/api/v1/messages/history?chat_id={}&before_id=0", chat_id),
        format!("/api/v1/messages/history?chat_id={}&before_id=-3", chat_id),
        format!(
            "/api/v1/messages/history?chat_id={}&before_id=5&offset_id=4",
            chat_id
        ),
        format!("/api/v1/messages/history?chat_id={}&after_id=-1", chat_id),
        format!(
            "/api/v1/messages/history?chat_id={}&before_id=3&after_id=3",
            chat_id
        ),
        format!("/api/v1/messages/history?chat_id={}&topic_id=0", chat_id),
    ];

    for uri in bad_history_uris {
        let (status, body) =
            send_json_request(&state, Method::GET, &uri, Some(&alice_token), None).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "Expected 400 for URI {}, got {} ({})",
            uri,
            status,
            body
        );
    }

    // Verify no side effects occurred in DB
    assert_eq!(state.db.get_current_pts(ChatId(chat_id)).unwrap(), 0);
}

#[tokio::test]
async fn test_history_pagination_ordering_and_topic_filtering() {
    let state = new_in_memory_state();
    let (alice_id, alice_token) = register_test_user(&state, "+15550003041", "Alice").await;
    let chat_id = create_test_chat(&state, &alice_token, "Pagination & Topics").await;

    // Create forum topic #10 in chat
    state
        .db
        .create_forum_topic(&ForumTopicRecord {
            chat_id: ChatId(chat_id),
            topic_id: TopicId(10),
            title: "Release Track".to_string(),
            icon_color: 0x6FB9F0,
            icon_emoji_id: None,
            creator_id: UserId(alice_id),
            is_pinned: false,
            is_closed: false,
            is_hidden: false,
            last_message_id: None,
            unread_count: 0,
            created_at_ms: 1_700_000_000_000,
        })
        .expect("create forum topic");

    // Send 5 messages (messages 2 and 4 belong to topic_id = 10)
    for i in 1..=5 {
        let topic = if i % 2 == 0 { Some(10) } else { None };
        let (status, body) = send_json_request(
            &state,
            Method::POST,
            "/api/v1/messages/send",
            Some(&alice_token),
            Some(json!({
                "chat_id": chat_id,
                "text": format!("Message #{}", i),
                "topic_id": topic,
                "client_nonce": (5000 + i) as u64,
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "send #{} failed: {}", i, body);
        assert_eq!(body["message_id"], i);
        assert_eq!(body["pts"], i);
    }

    // Backward pagination: Page 1 (limit=2 -> messages [4, 5], has_more=true)
    let (s_p1, p1) = send_json_request(
        &state,
        Method::GET,
        &format!("/api/v1/messages/history?chat_id={}&limit=2", chat_id),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(s_p1, StatusCode::OK);
    assert_eq!(p1["has_more"], true);
    assert_eq!(p1["current_pts"], 5);
    let p1_msgs = p1["messages"].as_array().unwrap();
    assert_eq!(
        p1_msgs.iter().map(|m| m["message_id"].as_i64().unwrap()).collect::<Vec<_>>(),
        vec![4, 5]
    );

    // Page 2 (limit=2, before_id=4 -> messages [2, 3], has_more=true)
    let (s_p2, p2) = send_json_request(
        &state,
        Method::GET,
        &format!(
            "/api/v1/messages/history?chat_id={}&limit=2&before_id=4",
            chat_id
        ),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(s_p2, StatusCode::OK);
    assert_eq!(p2["has_more"], true);
    let p2_msgs = p2["messages"].as_array().unwrap();
    assert_eq!(
        p2_msgs.iter().map(|m| m["message_id"].as_i64().unwrap()).collect::<Vec<_>>(),
        vec![2, 3]
    );

    // Page 3 (limit=2, before_id=2 -> messages [1], has_more=false)
    let (s_p3, p3) = send_json_request(
        &state,
        Method::GET,
        &format!(
            "/api/v1/messages/history?chat_id={}&limit=2&before_id=2",
            chat_id
        ),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(s_p3, StatusCode::OK);
    assert_eq!(p3["has_more"], false);
    let p3_msgs = p3["messages"].as_array().unwrap();
    assert_eq!(
        p3_msgs.iter().map(|m| m["message_id"].as_i64().unwrap()).collect::<Vec<_>>(),
        vec![1]
    );

    // Forward pagination: after_id=2, limit=2 -> messages [3, 4], has_more=true
    let (s_fwd, fwd) = send_json_request(
        &state,
        Method::GET,
        &format!(
            "/api/v1/messages/history?chat_id={}&after_id=2&limit=2",
            chat_id
        ),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(s_fwd, StatusCode::OK);
    assert_eq!(fwd["has_more"], true);
    let fwd_msgs = fwd["messages"].as_array().unwrap();
    assert_eq!(
        fwd_msgs.iter().map(|m| m["message_id"].as_i64().unwrap()).collect::<Vec<_>>(),
        vec![3, 4]
    );

    // Topic filtering: topic_id=10 -> messages [2, 4], has_more=false
    let (s_topic, topic_hist) = send_json_request(
        &state,
        Method::GET,
        &format!("/api/v1/messages/history?chat_id={}&topic_id=10", chat_id),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(s_topic, StatusCode::OK);
    assert_eq!(topic_hist["has_more"], false);
    let topic_msgs = topic_hist["messages"].as_array().unwrap();
    assert_eq!(
        topic_msgs
            .iter()
            .map(|m| m["message_id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![2, 4]
    );
}

#[tokio::test]
async fn test_restart_durability_for_history_pts_and_nonce_dedup() {
    let guard = TempDbDirGuard::new("fusion_s003_restart");
    let db_path_str = guard.db_path().to_string_lossy().to_string();

    let (alice_id, alice_token, chat_id) = {
        let mut cfg = ServerConfig::default();
        cfg.database_url = db_path_str.clone();
        let state = Arc::new(AppState::try_new(cfg).expect("init state phase 1"));

        let (alice_id, alice_token) =
            register_test_user(&state, "+15550003051", "AliceRestart").await;
        let chat_id = create_test_chat(&state, &alice_token, "Durable Room").await;

        let (s1, m1) = send_json_request(
            &state,
            Method::POST,
            "/api/v1/messages/send",
            Some(&alice_token),
            Some(json!({
                "chat_id": chat_id,
                "text": "Persisted before restart #1",
                "client_nonce": 90001u64,
            })),
        )
        .await;
        assert_eq!(s1, StatusCode::OK);
        assert_eq!(m1["message_id"], 1);
        assert_eq!(m1["pts"], 1);

        let (s2, m2) = send_json_request(
            &state,
            Method::POST,
            "/api/v1/messages/send",
            Some(&alice_token),
            Some(json!({
                "chat_id": chat_id,
                "text": "Persisted before restart #2",
                "reply_to": 1,
                "client_nonce": 90002u64,
            })),
        )
        .await;
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(m2["message_id"], 2);
        assert_eq!(m2["pts"], 2);

        (alice_id, alice_token, chat_id)
    };

    // Phase 2: Reopen fresh AppState on the same SQLite database file (cold Tier-1 memory cache)
    let mut cfg2 = ServerConfig::default();
    cfg2.database_url = db_path_str;
    let state2 = Arc::new(AppState::try_new(cfg2).expect("init state phase 2"));

    // 1. History is intact after restart
    let (s_hist, hist) = send_json_request(
        &state2,
        Method::GET,
        &format!("/api/v1/messages/history?chat_id={}", chat_id),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(s_hist, StatusCode::OK);
    assert_eq!(hist["current_pts"], 2);
    let msgs = hist["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["message_id"], 1);
    assert_eq!(msgs[0]["text"], "Persisted before restart #1");
    assert_eq!(msgs[1]["message_id"], 2);
    assert_eq!(msgs[1]["text"], "Persisted before restart #2");
    assert_eq!(msgs[1]["reply_to"], 1);

    // 2. Retrying Phase 1 client_nonce = 90001 hits Tier-2 SQLite dedup -> 200 OK, message_id = 1, pts_count = 0
    let (s_retry, retry_body) = send_json_request(
        &state2,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id,
            "text": "Persisted before restart #1",
            "client_nonce": 90001u64,
        })),
    )
    .await;
    assert_eq!(s_retry, StatusCode::OK);
    assert_eq!(retry_body["message_id"], 1);
    assert_eq!(retry_body["pts"], 1);
    assert_eq!(retry_body["pts_count"], 0);
    assert_eq!(retry_body["duplicate"], true);
    assert_eq!(retry_body["sender_id"], alice_id);

    // 3. Conflicting retry with Phase 1 client_nonce = 90001 after restart -> 409 Conflict
    let (s_conflict, _) = send_json_request(
        &state2,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id,
            "text": "Tampered after restart",
            "client_nonce": 90001u64,
        })),
    )
    .await;
    assert_eq!(s_conflict, StatusCode::CONFLICT);

    // 4. Sending a new message after restart continues at message_id = 3 and pts = 3
    let (s_new, new_body) = send_json_request(
        &state2,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id,
            "text": "Sent after restart #3",
            "client_nonce": 90003u64,
        })),
    )
    .await;
    assert_eq!(s_new, StatusCode::OK);
    assert_eq!(new_body["message_id"], 3);
    assert_eq!(new_body["pts"], 3);
    assert_eq!(new_body["pts_count"], 1);
}

#[tokio::test]
async fn test_forced_db_failure_returns_500_without_leaking_sql_or_orphan_state() {
    let state = new_in_memory_state();
    let (alice_id, alice_token) = register_test_user(&state, "+15550003061", "Alice").await;
    let chat_id = create_test_chat(&state, &alice_token, "Rollback Verification").await;

    // Install a trigger that aborts INSERT INTO messages
    state
        .db
        .pool()
        .with_conn(|conn| {
            conn.execute_batch(
                r#"
                CREATE TRIGGER abort_messages_insert_s003
                BEFORE INSERT ON messages
                BEGIN
                    SELECT RAISE(ABORT, 'simulated_s003_messages_failure_secret_sql');
                END;
                "#,
            )
            .unwrap();
            Ok(())
        })
        .expect("create abort trigger");

    let (status_fail, fail_body) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id,
            "text": "Must roll back atomically",
            "client_nonce": 88888u64,
        })),
    )
    .await;
    assert_eq!(status_fail, StatusCode::INTERNAL_SERVER_ERROR);
    let serialized = fail_body.to_string();
    assert!(
        !serialized.contains("simulated_s003_messages_failure_secret_sql"),
        "500 response must not leak internal SQL/trigger details: {}",
        serialized
    );

    // Verify complete rollback: PTS is still 0, no pts_updates row, no message_dedup row
    assert_eq!(state.db.get_current_pts(ChatId(chat_id)).unwrap(), 0);
    assert!(state.db.find_dedup_message(alice_id, 88888).unwrap().is_none());

    // Drop the trigger and verify the same nonce now commits cleanly as message_id = 1, pts = 1
    state
        .db
        .pool()
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER abort_messages_insert_s003;")
                .unwrap();
            let updates_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pts_updates WHERE chat_id = ?1",
                    params![chat_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(updates_count, 0);
            Ok(())
        })
        .expect("drop trigger and check pts_updates");

    let (status_ok, ok_body) = send_json_request(
        &state,
        Method::POST,
        "/api/v1/messages/send",
        Some(&alice_token),
        Some(json!({
            "chat_id": chat_id,
            "text": "Must roll back atomically",
            "client_nonce": 88888u64,
        })),
    )
    .await;
    assert_eq!(status_ok, StatusCode::OK);
    assert_eq!(ok_body["message_id"], 1);
    assert_eq!(ok_body["pts"], 1);
}
