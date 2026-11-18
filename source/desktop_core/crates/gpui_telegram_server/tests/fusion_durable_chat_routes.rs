use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_server::{
    server::{build_router_with_state, AppState},
    ServerConfig,
};
use serde_json::{json, Value};
use std::{collections::HashSet, path::PathBuf, sync::Arc};
use tokio::sync::Barrier;
use tower::ServiceExt;

/// RAII cleanup guard for the explicit database reopen test.
/// Removes the temporary directory and all SQLite/WAL/SHM files on drop,
/// including on assertion panic.
struct TempDbDirGuard {
    dir: PathBuf,
    db_path: PathBuf,
}

impl TempDbDirGuard {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "gpui_fus_s001_chat_routes_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("Failed to create temp dir for SQLite reopen test");
        let db_path = dir.join("fusion_chats.db");
        Self { dir, db_path }
    }

    fn database_url(&self) -> String {
        format!("sqlite://{}", self.db_path.display())
    }
}

impl Drop for TempDbDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn register_user(app: &Router, username: &str) -> (u64, String) {
    let body = json!({
        "username": username,
        "password": format!("Pass_{}_98765!", username),
        "first_name": username,
        "device_name": "Test WorkStation"
    });

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    let user_id = json["user_id"].as_u64().expect("user_id must be u64");
    let token = json["token"]
        .as_str()
        .expect("token must be string")
        .to_string();
    (user_id, token)
}

#[tokio::test]
async fn test_auth_bearer_required_for_chat_routes() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state);

    // 1. POST /api/v1/chats without token -> 401
    let req_create = Request::builder()
        .method("POST")
        .uri("/api/v1/chats")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"title": "Unauthenticated Group"}"#))
        .unwrap();
    let res_create = app.clone().oneshot(req_create).await.unwrap();
    assert_eq!(res_create.status(), StatusCode::UNAUTHORIZED);

    // 2. GET /api/v1/chats with invalid token -> 401
    let req_list = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, "Bearer invalid.token.value")
        .body(Body::empty())
        .unwrap();
    let res_list = app.clone().oneshot(req_list).await.unwrap();
    assert_eq!(res_list.status(), StatusCode::UNAUTHORIZED);

    // 3. GET /api/v1/chats/1 without token -> 401
    let req_get = Request::builder()
        .method("GET")
        .uri("/api/v1/chats/1")
        .body(Body::empty())
        .unwrap();
    let res_get = app.oneshot(req_get).await.unwrap();
    assert_eq!(res_get.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_empty_account_returns_empty_array_and_zero_total() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state);

    let (_user_id, token) = register_user(&app, "empty_user").await;
    let bearer = format!("Bearer {}", token);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bearer)
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["chats"], json!([]));
    assert_eq!(json["total"], 0);
}

#[tokio::test]
async fn test_create_then_list_and_get_durable_chat() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state.clone());

    let (alice_id, alice_token) = register_user(&app, "alice_chat_owner").await;
    let bearer = format!("Bearer {}", alice_token);

    // Create first chat via POST /api/v1/chats with leading/trailing whitespace,
    // an attempted creator_id spoof, and an unsupported optional field.
    let create_payload_1 = json!({
        "title": "  Fabushi Architecture WG  ",
        "creator_id": 999_999_999u64,
        "unsupported_participants": [123, 456]
    });
    let req_create_1 = Request::builder()
        .method("POST")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&create_payload_1).unwrap()))
        .unwrap();

    let res_create_1 = app.clone().oneshot(req_create_1).await.unwrap();
    assert_eq!(res_create_1.status(), StatusCode::CREATED);
    let body_1 = to_bytes(res_create_1.into_body(), usize::MAX).await.unwrap();
    let created_1: Value = serde_json::from_slice(&body_1).unwrap();

    let chat_id_1 = created_1["chat_id"].as_i64().expect("chat_id must be i64");
    assert_eq!(chat_id_1, 1);
    assert_eq!(created_1["title"], "Fabushi Architecture WG");
    assert_eq!(created_1["creator_id"], alice_id);
    assert!(created_1.get("unsupported_participants").is_none());

    // Create second chat via legacy alias POST /api/v1/chats/create
    let create_payload_2 = json!({
        "title": "Rust Core & GPUI 🚀"
    });
    let req_create_2 = Request::builder()
        .method("POST")
        .uri("/api/v1/chats/create")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&create_payload_2).unwrap()))
        .unwrap();

    let res_create_2 = app.clone().oneshot(req_create_2).await.unwrap();
    assert_eq!(res_create_2.status(), StatusCode::CREATED);
    let body_2 = to_bytes(res_create_2.into_body(), usize::MAX).await.unwrap();
    let created_2: Value = serde_json::from_slice(&body_2).unwrap();
    let chat_id_2 = created_2["chat_id"].as_i64().expect("chat_id must be i64");
    assert_eq!(chat_id_2, 2);
    assert_eq!(created_2["title"], "Rust Core & GPUI 🚀");
    assert_eq!(created_2["creator_id"], alice_id);

    // Verify underlying SQLite records in both `chats` and `chat_members`
    let db_chat_1 = state
        .db
        .get_chat(ChatId(chat_id_1))
        .expect("get_chat should succeed")
        .expect("chat 1 must exist in SQLite");
    assert_eq!(db_chat_1.title, "Fabushi Architecture WG");
    assert_eq!(db_chat_1.creator_id, UserId(alice_id));

    let db_member_1 = state
        .db
        .get_chat_member(ChatId(chat_id_1), UserId(alice_id))
        .expect("get_chat_member should succeed")
        .expect("creator membership must exist in SQLite");
    assert_eq!(db_member_1.role, "owner");

    // GET /api/v1/chats returns both chats in deterministic order
    let req_list = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bearer)
        .body(Body::empty())
        .unwrap();
    let res_list = app.clone().oneshot(req_list).await.unwrap();
    assert_eq!(res_list.status(), StatusCode::OK);
    let list_bytes = to_bytes(res_list.into_body(), usize::MAX).await.unwrap();
    let list_json: Value = serde_json::from_slice(&list_bytes).unwrap();

    assert_eq!(list_json["total"], 2);
    let chats = list_json["chats"].as_array().expect("chats must be array");
    assert_eq!(chats.len(), 2);
    assert_eq!(chats[0]["chat_id"], 1);
    assert_eq!(chats[0]["title"], "Fabushi Architecture WG");
    assert_eq!(chats[0]["unread_count"], 0);
    assert_eq!(chats[0]["is_group"], true);
    assert!(chats[0].get("last_message").is_none());

    assert_eq!(chats[1]["chat_id"], 2);
    assert_eq!(chats[1]["title"], "Rust Core & GPUI 🚀");
    assert_eq!(chats[1]["unread_count"], 0);
    assert_eq!(chats[1]["is_group"], true);

    // GET /api/v1/chats/{id} returns persisted chat data
    let req_get_1 = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/chats/{}", chat_id_1))
        .header(header::AUTHORIZATION, &bearer)
        .body(Body::empty())
        .unwrap();
    let res_get_1 = app.oneshot(req_get_1).await.unwrap();
    assert_eq!(res_get_1.status(), StatusCode::OK);
    let get_1_bytes = to_bytes(res_get_1.into_body(), usize::MAX).await.unwrap();
    let get_1_json: Value = serde_json::from_slice(&get_1_bytes).unwrap();
    assert_eq!(get_1_json["chat_id"], 1);
    assert_eq!(get_1_json["title"], "Fabushi Architecture WG");
    assert_eq!(get_1_json["creator_id"], alice_id);
    assert_eq!(get_1_json["is_group"], true);
    assert_eq!(get_1_json["unread_count"], 0);
}

#[tokio::test]
async fn test_disjoint_accounts_and_guessed_ids_denied() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state);

    let (alice_id, alice_token) = register_user(&app, "alice_isolated").await;
    let (bob_id, bob_token) = register_user(&app, "bob_isolated").await;
    assert_ne!(alice_id, bob_id);

    let alice_bearer = format!("Bearer {}", alice_token);
    let bob_bearer = format!("Bearer {}", bob_token);

    // Alice creates chat 1
    let req_alice_create = Request::builder()
        .method("POST")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &alice_bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"title": "Alice Secret Group"}"#))
        .unwrap();
    let res_alice_create = app.clone().oneshot(req_alice_create).await.unwrap();
    assert_eq!(res_alice_create.status(), StatusCode::CREATED);
    let alice_chat: Value =
        serde_json::from_slice(&to_bytes(res_alice_create.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    let alice_chat_id = alice_chat["chat_id"].as_i64().unwrap();

    // Bob creates chat 2
    let req_bob_create = Request::builder()
        .method("POST")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bob_bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"title": "Bob Operations Group"}"#))
        .unwrap();
    let res_bob_create = app.clone().oneshot(req_bob_create).await.unwrap();
    assert_eq!(res_bob_create.status(), StatusCode::CREATED);
    let bob_chat: Value =
        serde_json::from_slice(&to_bytes(res_bob_create.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    let bob_chat_id = bob_chat["chat_id"].as_i64().unwrap();
    assert_ne!(alice_chat_id, bob_chat_id);

    // Alice list contains ONLY Alice's chat
    let req_alice_list = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &alice_bearer)
        .body(Body::empty())
        .unwrap();
    let res_alice_list = app.clone().oneshot(req_alice_list).await.unwrap();
    assert_eq!(res_alice_list.status(), StatusCode::OK);
    let alice_list: Value =
        serde_json::from_slice(&to_bytes(res_alice_list.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(alice_list["total"], 1);
    assert_eq!(alice_list["chats"][0]["chat_id"], alice_chat_id);
    assert_eq!(alice_list["chats"][0]["title"], "Alice Secret Group");

    // Bob list contains ONLY Bob's chat
    let req_bob_list = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bob_bearer)
        .body(Body::empty())
        .unwrap();
    let res_bob_list = app.clone().oneshot(req_bob_list).await.unwrap();
    assert_eq!(res_bob_list.status(), StatusCode::OK);
    let bob_list: Value =
        serde_json::from_slice(&to_bytes(res_bob_list.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(bob_list["total"], 1);
    assert_eq!(bob_list["chats"][0]["chat_id"], bob_chat_id);
    assert_eq!(bob_list["chats"][0]["title"], "Bob Operations Group");

    // Bob guessing Alice's chat_id gets 404 indistinguishable from a nonexistent chat_id
    let req_bob_guess_alice = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/chats/{}", alice_chat_id))
        .header(header::AUTHORIZATION, &bob_bearer)
        .body(Body::empty())
        .unwrap();
    let res_bob_guess_alice = app.clone().oneshot(req_bob_guess_alice).await.unwrap();
    assert_eq!(res_bob_guess_alice.status(), StatusCode::NOT_FOUND);
    let guess_existing_json: Value = serde_json::from_slice(
        &to_bytes(res_bob_guess_alice.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    let req_bob_guess_missing = Request::builder()
        .method("GET")
        .uri("/api/v1/chats/999999")
        .header(header::AUTHORIZATION, &bob_bearer)
        .body(Body::empty())
        .unwrap();
    let res_bob_guess_missing = app.clone().oneshot(req_bob_guess_missing).await.unwrap();
    assert_eq!(res_bob_guess_missing.status(), StatusCode::NOT_FOUND);
    let guess_missing_json: Value = serde_json::from_slice(
        &to_bytes(res_bob_guess_missing.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    // Must not disclose existence or title of Alice's chat
    assert_eq!(guess_existing_json, guess_missing_json);
    assert!(!guess_existing_json
        .to_string()
        .contains("Alice Secret Group"));

    // Invalid or non-positive chat IDs return 400 or 404
    for invalid_id in ["0", "-1", "-999", "not_an_id"] {
        let req_invalid = Request::builder()
            .method("GET")
            .uri(format!("/api/v1/chats/{}", invalid_id))
            .header(header::AUTHORIZATION, &bob_bearer)
            .body(Body::empty())
            .unwrap();
        let res_invalid = app.clone().oneshot(req_invalid).await.unwrap();
        assert!(
            res_invalid.status() == StatusCode::BAD_REQUEST
                || res_invalid.status() == StatusCode::NOT_FOUND,
            "Expected 400 or 404 for chat_id '{}', got {}",
            invalid_id,
            res_invalid.status()
        );
    }
}

#[tokio::test]
async fn test_invalid_titles_rejected_without_writes() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state);

    let (_user_id, token) = register_user(&app, "validator_user").await;
    let bearer = format!("Bearer {}", token);

    let oversized_ascii = "A".repeat(256);
    let oversized_unicode = "测".repeat(256);

    let invalid_bodies: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"{malformed json".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        br#"{"title": null}"#.to_vec(),
        br#"{"title": 12345}"#.to_vec(),
        br#"{"title": ""}"#.to_vec(),
        br#"{"title": "    \t\n   "}"#.to_vec(),
        serde_json::to_vec(&json!({ "title": oversized_ascii })).unwrap(),
        serde_json::to_vec(&json!({ "title": oversized_unicode })).unwrap(),
    ];

    for payload in invalid_bodies {
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/chats")
            .header(header::AUTHORIZATION, &bearer)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload.clone()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "Expected 400 BAD_REQUEST for invalid payload: {:?}",
            String::from_utf8_lossy(&payload)
        );
    }

    // Verify zero chats were created after all invalid attempts
    let req_list = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bearer)
        .body(Body::empty())
        .unwrap();
    let res_list = app.clone().oneshot(req_list).await.unwrap();
    let list_json: Value =
        serde_json::from_slice(&to_bytes(res_list.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(list_json["total"], 0);
    assert_eq!(list_json["chats"], json!([]));

    // Boundary check: 255 Unicode scalar characters is accepted
    let max_valid_unicode = "测".repeat(255);
    let req_valid = Request::builder()
        .method("POST")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({ "title": max_valid_unicode })).unwrap(),
        ))
        .unwrap();
    let res_valid = app.oneshot(req_valid).await.unwrap();
    assert_eq!(res_valid.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_concurrent_chat_creation_uniqueness() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state);

    let (user_id, token) = register_user(&app, "concurrent_creator").await;
    let bearer = format!("Bearer {}", token);

    let concurrency = 24usize;
    let barrier = Arc::new(Barrier::new(concurrency));
    let mut handles = Vec::with_capacity(concurrency);

    for idx in 0..concurrency {
        let app_clone = app.clone();
        let bearer_clone = bearer.clone();
        let barrier_clone = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier_clone.wait().await;
            let body = json!({
                "title": format!("Concurrent Chat #{:02}", idx)
            });
            let req = Request::builder()
                .method("POST")
                .uri("/api/v1/chats")
                .header(header::AUTHORIZATION, &bearer_clone)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap();

            let res = app_clone.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::CREATED);
            let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
            let json: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(json["creator_id"], user_id);
            json["chat_id"].as_i64().expect("chat_id must be i64")
        }));
    }

    let mut allocated_ids = HashSet::new();
    for handle in handles {
        let chat_id = handle.await.unwrap();
        assert!(chat_id > 0, "Allocated chat_id must be positive, got {}", chat_id);
        assert!(
            allocated_ids.insert(chat_id),
            "Duplicate chat_id allocated under concurrency: {}",
            chat_id
        );
    }
    assert_eq!(allocated_ids.len(), concurrency);

    // Listing must return all 24 chats in deterministic ascending chat_id order
    let req_list = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bearer)
        .body(Body::empty())
        .unwrap();
    let res_list = app.oneshot(req_list).await.unwrap();
    assert_eq!(res_list.status(), StatusCode::OK);
    let list_json: Value =
        serde_json::from_slice(&to_bytes(res_list.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(list_json["total"], concurrency);
    let chats = list_json["chats"].as_array().unwrap();
    let listed_ids: Vec<i64> = chats.iter().map(|c| c["chat_id"].as_i64().unwrap()).collect();
    let expected_ids: Vec<i64> = (1..=(concurrency as i64)).collect();
    assert_eq!(listed_ids, expected_ids);
}

#[tokio::test]
async fn test_persistence_across_database_reopen() {
    let guard = TempDbDirGuard::new();
    let mut config = ServerConfig::default();
    config.database_url = guard.database_url();

    let (alice_id, alice_token, alice_chat_id, bob_token);

    // Phase 1: Initial server instance creates users and chats
    {
        let state_1 = Arc::new(AppState::new(config.clone()));
        let app_1 = build_router_with_state(state_1);

        let (a_id, a_tok) = register_user(&app_1, "alice_reopen").await;
        let (_b_id, b_tok) = register_user(&app_1, "bob_reopen").await;
        alice_id = a_id;
        alice_token = a_tok;
        bob_token = b_tok;

        let req_create = Request::builder()
            .method("POST")
            .uri("/api/v1/chats")
            .header(header::AUTHORIZATION, format!("Bearer {}", alice_token))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"title": "Durable Across Restart"}"#))
            .unwrap();
        let res_create = app_1.oneshot(req_create).await.unwrap();
        assert_eq!(res_create.status(), StatusCode::CREATED);
        let created_json: Value =
            serde_json::from_slice(&to_bytes(res_create.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        alice_chat_id = created_json["chat_id"].as_i64().unwrap();
        assert!(alice_chat_id > 0);
    }

    // Phase 2: Reopen a fresh AppState & ServerDatabase against the same SQLite path
    {
        let state_2 = Arc::new(AppState::new(config));
        let app_2 = build_router_with_state(state_2);

        let alice_bearer = format!("Bearer {}", alice_token);
        let bob_bearer = format!("Bearer {}", bob_token);

        // Alice's chat list persists across reopen
        let req_list = Request::builder()
            .method("GET")
            .uri("/api/v1/chats")
            .header(header::AUTHORIZATION, &alice_bearer)
            .body(Body::empty())
            .unwrap();
        let res_list = app_2.clone().oneshot(req_list).await.unwrap();
        assert_eq!(res_list.status(), StatusCode::OK);
        let list_json: Value =
            serde_json::from_slice(&to_bytes(res_list.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(list_json["total"], 1);
        assert_eq!(list_json["chats"][0]["chat_id"], alice_chat_id);
        assert_eq!(list_json["chats"][0]["title"], "Durable Across Restart");

        // Alice's GET /api/v1/chats/{id} persists across reopen
        let req_get = Request::builder()
            .method("GET")
            .uri(format!("/api/v1/chats/{}", alice_chat_id))
            .header(header::AUTHORIZATION, &alice_bearer)
            .body(Body::empty())
            .unwrap();
        let res_get = app_2.clone().oneshot(req_get).await.unwrap();
        assert_eq!(res_get.status(), StatusCode::OK);
        let get_json: Value =
            serde_json::from_slice(&to_bytes(res_get.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(get_json["chat_id"], alice_chat_id);
        assert_eq!(get_json["title"], "Durable Across Restart");
        assert_eq!(get_json["creator_id"], alice_id);

        // Bob is still denied access to Alice's chat after reopen
        let req_bob_get = Request::builder()
            .method("GET")
            .uri(format!("/api/v1/chats/{}", alice_chat_id))
            .header(header::AUTHORIZATION, &bob_bearer)
            .body(Body::empty())
            .unwrap();
        let res_bob_get = app_2.clone().oneshot(req_bob_get).await.unwrap();
        assert_eq!(res_bob_get.status(), StatusCode::NOT_FOUND);

        // Creating another chat after reopen allocates a new monotonic ID without collision
        let req_create_2 = Request::builder()
            .method("POST")
            .uri("/api/v1/chats")
            .header(header::AUTHORIZATION, &alice_bearer)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"title": "Post-Reopen Chat"}"#))
            .unwrap();
        let res_create_2 = app_2.oneshot(req_create_2).await.unwrap();
        assert_eq!(res_create_2.status(), StatusCode::CREATED);
        let created_2_json: Value =
            serde_json::from_slice(&to_bytes(res_create_2.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let second_chat_id = created_2_json["chat_id"].as_i64().unwrap();
        assert!(second_chat_id > alice_chat_id);
    }
}

#[tokio::test]
async fn test_no_orphan_chat_on_failed_membership_transaction() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = build_router_with_state(state.clone());

    let (_user_id, token) = register_user(&app, "tx_rollback_user").await;
    let bearer = format!("Bearer {}", token);

    // Inject a BEFORE INSERT trigger on `chat_members` so `INSERT INTO chats` succeeds
    // inside the transaction, and `INSERT INTO chat_members` aborts.
    state
        .db
        .pool()
        .with_conn(|conn| {
            conn.execute_batch(
                r#"
                CREATE TRIGGER abort_chat_member_insert_for_test
                BEFORE INSERT ON chat_members
                BEGIN
                    SELECT RAISE(ABORT, 'sensitive_internal_sql_trigger_failure_detail');
                END;
                "#,
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("Failed to install test trigger");

    // Attempt to create a chat via the route
    let req_fail = Request::builder()
        .method("POST")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"title": "Should Rollback Atomically"}"#))
        .unwrap();
    let res_fail = app.clone().oneshot(req_fail).await.unwrap();
    assert_eq!(res_fail.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let fail_body = to_bytes(res_fail.into_body(), usize::MAX).await.unwrap();
    let fail_text = String::from_utf8_lossy(&fail_body);
    assert!(
        !fail_text.contains("sensitive_internal_sql_trigger_failure_detail")
            && !fail_text.contains("chat_members"),
        "500 response must not leak SQL/internal details: {}",
        fail_text
    );

    // Verify no orphan chat or member row exists in SQLite
    let (chats_count, members_count): (i64, i64) = state
        .db
        .pool()
        .with_conn(|conn| {
            let c: i64 = conn
                .query_row("SELECT COUNT(*) FROM chats", [], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let m: i64 = conn
                .query_row("SELECT COUNT(*) FROM chat_members", [], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok((c, m))
        })
        .expect("Failed to count rows after rollback");
    assert_eq!(chats_count, 0, "Orphan chat row must not remain after membership failure");
    assert_eq!(members_count, 0, "Member row must not remain after membership failure");

    // Remove the trigger and verify subsequent creation succeeds cleanly
    state
        .db
        .pool()
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER abort_chat_member_insert_for_test;")
                .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("Failed to drop test trigger");

    let req_ok = Request::builder()
        .method("POST")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"title": "Committed After Recovery"}"#))
        .unwrap();
    let res_ok = app.clone().oneshot(req_ok).await.unwrap();
    assert_eq!(res_ok.status(), StatusCode::CREATED);

    let req_list = Request::builder()
        .method("GET")
        .uri("/api/v1/chats")
        .header(header::AUTHORIZATION, &bearer)
        .body(Body::empty())
        .unwrap();
    let res_list = app.oneshot(req_list).await.unwrap();
    assert_eq!(res_list.status(), StatusCode::OK);
    let list_json: Value =
        serde_json::from_slice(&to_bytes(res_list.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(list_json["total"], 1);
    assert_eq!(list_json["chats"][0]["title"], "Committed After Recovery");
}
