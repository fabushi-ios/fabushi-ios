use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use gpui_telegram_core::types::UserId;
use gpui_telegram_server::push::PushNotification;
use gpui_telegram_server::server::AppState;
use gpui_telegram_server::{build_router, build_router_with_state};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn register_and_login(app: &axum::Router, username: &str) -> String {
    let reg_body = json!({
        "username": username,
        "password": "Password123!",
        "first_name": "Test",
        "last_name": "User"
    });

    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&reg_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let login_body = json!({
        "username": username,
        "password": "Password123!",
        "device_name": "Test Device"
    });

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let val: Value = serde_json::from_slice(&bytes).unwrap();
    val["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_chunked_upload_lifecycle_and_status_inspection() {
    let app = build_router();
    let token = register_and_login(&app, "chunk_tester").await;

    // 1. Inspect non-existent upload returns 404
    let req_nonexistent = Request::builder()
        .method("GET")
        .uri("/api/v1/media/upload/upl_nonexistent_123/status")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let res_nonexistent = app.clone().oneshot(req_nonexistent).await.unwrap();
    assert_eq!(res_nonexistent.status(), StatusCode::NOT_FOUND);

    // 2. Initialize chunked upload with 4 parts (total 4096 bytes, 1024 per part)
    let init_body = json!({
        "size_bytes": 4096,
        "chunk_size": 1024,
        "filename": "test_document.bin"
    });

    let req_init = Request::builder()
        .method("POST")
        .uri("/api/v1/media/upload/init")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&init_body).unwrap()))
        .unwrap();

    let res_init = app.clone().oneshot(req_init).await.unwrap();
    assert_eq!(res_init.status(), StatusCode::CREATED);
    let init_val: Value = serde_json::from_slice(&to_bytes(res_init.into_body(), usize::MAX).await.unwrap()).unwrap();
    let upload_id = init_val["upload_id"].as_str().unwrap().to_string();
    assert_eq!(init_val["total_parts"], 4);

    // 3. Inspect status immediately after initialization
    let req_status1 = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/media/upload/{upload_id}/status"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let res_status1 = app.clone().oneshot(req_status1).await.unwrap();
    assert_eq!(res_status1.status(), StatusCode::OK);
    let status1_val: Value = serde_json::from_slice(&to_bytes(res_status1.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(status1_val["upload_id"], upload_id);
    assert_eq!(status1_val["total_parts"], 4);
    assert_eq!(status1_val["is_complete"], false);
    let missing1 = status1_val["missing_parts"].as_array().unwrap();
    assert_eq!(missing1, &[json!(0), json!(1), json!(2), json!(3)]);

    // 4. Ingest parts out-of-order: part 0 and part 2
    let chunk_data = vec![0x42u8; 1024];

    let req_part0 = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/media/upload/{upload_id}/part/0"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(chunk_data.clone()))
        .unwrap();
    let res_part0 = app.clone().oneshot(req_part0).await.unwrap();
    assert_eq!(res_part0.status(), StatusCode::OK);

    let req_part2 = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/media/upload/{upload_id}/part/2"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(chunk_data.clone()))
        .unwrap();
    let res_part2 = app.clone().oneshot(req_part2).await.unwrap();
    assert_eq!(res_part2.status(), StatusCode::OK);

    // 5. Inspect status after sparse upload: missing parts must be [1, 3]
    let req_status2 = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/media/upload/{upload_id}/status"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let res_status2 = app.clone().oneshot(req_status2).await.unwrap();
    assert_eq!(res_status2.status(), StatusCode::OK);
    let status2_val: Value = serde_json::from_slice(&to_bytes(res_status2.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(status2_val["is_complete"], false);
    let missing2 = status2_val["missing_parts"].as_array().unwrap();
    assert_eq!(missing2, &[json!(1), json!(3)]);
    let uploaded2 = status2_val["uploaded_parts"].as_array().unwrap();
    assert_eq!(uploaded2, &[json!(true), json!(false), json!(true), json!(false)]);

    // 6. Ingest remaining parts 1 and 3
    let req_part1 = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/media/upload/{upload_id}/part/1"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(chunk_data.clone()))
        .unwrap();
    assert_eq!(app.clone().oneshot(req_part1).await.unwrap().status(), StatusCode::OK);

    let req_part3 = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/media/upload/{upload_id}/part/3"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(chunk_data.clone()))
        .unwrap();
    assert_eq!(app.clone().oneshot(req_part3).await.unwrap().status(), StatusCode::OK);

    // 7. Inspect status: all parts received, is_complete is true
    let req_status3 = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/media/upload/{upload_id}/status"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let res_status3 = app.clone().oneshot(req_status3).await.unwrap();
    assert_eq!(res_status3.status(), StatusCode::OK);
    let status3_val: Value = serde_json::from_slice(&to_bytes(res_status3.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(status3_val["is_complete"], true);
    let missing3 = status3_val["missing_parts"].as_array().unwrap();
    assert!(missing3.is_empty());

    // 8. Finalize chunked upload
    let req_complete = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/media/upload/{upload_id}/complete"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let res_complete = app.clone().oneshot(req_complete).await.unwrap();
    assert_eq!(res_complete.status(), StatusCode::OK);
    let complete_val: Value = serde_json::from_slice(&to_bytes(res_complete.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(complete_val["file_id"].is_string() || complete_val["sha256"].is_string());
}

#[tokio::test]
async fn test_push_device_registration_routes() {
    let app = build_router();
    let token = register_and_login(&app, "push_user").await;

    // 1. Register push token
    let reg_body = json!({
        "platform": "mock",
        "token": "device_token_abc_123",
        "device_id": "macbook_pro_m3",
        "app_version": "1.2.0"
    });

    let req_reg = Request::builder()
        .method("POST")
        .uri("/api/v1/devices/register_push")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&reg_body).unwrap()))
        .unwrap();

    let res_reg = app.clone().oneshot(req_reg).await.unwrap();
    assert_eq!(res_reg.status(), StatusCode::OK);
    let reg_val: Value = serde_json::from_slice(&to_bytes(res_reg.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(reg_val["success"], true);
    assert_eq!(reg_val["registered"], true);
    assert_eq!(reg_val["platform"], "mock");

    // 2. Unregister push token
    let unreg_body = json!({
        "token": "device_token_abc_123"
    });

    let req_unreg = Request::builder()
        .method("DELETE")
        .uri("/api/v1/devices/push")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&unreg_body).unwrap()))
        .unwrap();

    let res_unreg = app.clone().oneshot(req_unreg).await.unwrap();
    assert_eq!(res_unreg.status(), StatusCode::OK);
    let unreg_val: Value = serde_json::from_slice(&to_bytes(res_unreg.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(unreg_val["success"], true);
    assert_eq!(unreg_val["deleted"], true);
}

#[tokio::test]
async fn test_action_bus_http_routes() {
    let app = build_router();
    let token = register_and_login(&app, "action_user").await;

    // 1. Catalog list: GET /api/v1/actions
    let req_list = Request::builder()
        .method("GET")
        .uri("/api/v1/actions")
        .body(Body::empty())
        .unwrap();

    let res_list = app.clone().oneshot(req_list).await.unwrap();
    assert_eq!(res_list.status(), StatusCode::OK);
    let list_val: Value = serde_json::from_slice(&to_bytes(res_list.into_body(), usize::MAX).await.unwrap()).unwrap();
    let actions = list_val["actions"].as_array().unwrap();
    assert!(actions.iter().any(|a| a["name"] == "send_message"));
    assert!(actions.iter().any(|a| a["name"] == "delete_message"));

    // 2. Action schema: GET /api/v1/actions/send_message/schema
    let req_schema = Request::builder()
        .method("GET")
        .uri("/api/v1/actions/send_message/schema")
        .body(Body::empty())
        .unwrap();

    let res_schema = app.clone().oneshot(req_schema).await.unwrap();
    assert_eq!(res_schema.status(), StatusCode::OK);
    let schema_val: Value = serde_json::from_slice(&to_bytes(res_schema.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(schema_val["name"], "send_message");
    assert!(schema_val["input_schema"].is_object());

    // 3. Create a real chat in SQLite for authentic action execution
    let res_chat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "title": "Action Bus Real Chat",
                    "chat_type": "group"
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_chat.status(), StatusCode::CREATED);
    let chat_val: Value = serde_json::from_slice(&to_bytes(res_chat.into_body(), usize::MAX).await.unwrap()).unwrap();
    let real_chat_id = chat_val["chat_id"].as_i64().unwrap();

    // 4. Unauthenticated mutating action (send_message) must return 401 Unauthorized
    let action_input = json!({
        "chat_id": real_chat_id,
        "text": "Hello via Action Bus HTTP API!"
    });
    let res_unauth_send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/send_message")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&action_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_unauth_send.status(), StatusCode::UNAUTHORIZED);

    // 5. Extra unknown field must be rejected with 422 Unprocessable Entity
    let res_extra = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/send_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "chat_id": real_chat_id,
                    "text": "Should fail",
                    "unexpected_prop": 123
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_extra.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // 6. Real send_message execution persists to SQLite and advances PTS
    let res_exec = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/send_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&action_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_exec.status(), StatusCode::OK);
    let exec_val: Value = serde_json::from_slice(&to_bytes(res_exec.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(exec_val["success"], true);
    assert_eq!(exec_val["result"]["chat_id"], real_chat_id);
    let sent_msg_id = exec_val["result"]["message_id"].as_i64().unwrap();
    assert!(sent_msg_id >= 1);
    assert!(exec_val["result"]["pts"].as_u64().unwrap() >= 1);

    // 7. get_chat_history and list_dialogs return real persisted DB data
    let res_hist = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/get_chat_history")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "chat_id": real_chat_id,
                    "limit": 10
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_hist.status(), StatusCode::OK);
    let hist_val: Value = serde_json::from_slice(&to_bytes(res_hist.into_body(), usize::MAX).await.unwrap()).unwrap();
    let msgs = hist_val["result"]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["message_id"], sent_msg_id);
    assert_eq!(msgs[0]["text"], "Hello via Action Bus HTTP API!");

    let res_dialogs = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/list_dialogs")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({ "limit": 20 })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_dialogs.status(), StatusCode::OK);
    let dialogs_val: Value = serde_json::from_slice(&to_bytes(res_dialogs.into_body(), usize::MAX).await.unwrap()).unwrap();
    let dialogs = dialogs_val["result"]["dialogs"].as_array().unwrap();
    assert!(dialogs.iter().any(|d| d["chat_id"] == real_chat_id && d["last_message_text"] == "Hello via Action Bus HTTP API!"));

    // 8. High-risk action without auth returns 401 Unauthorized
    let del_input = json!({
        "chat_id": real_chat_id,
        "message_ids": [sent_msg_id]
    });

    let res_del_noauth = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_del_noauth.status(), StatusCode::UNAUTHORIZED);

    // 9. High-risk action with auth but NO X-Approval-Token returns 403 Forbidden with gate_id
    let res_del_unapproved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_del_unapproved.status(), StatusCode::FORBIDDEN);
    let gate_err: Value = serde_json::from_slice(&to_bytes(res_del_unapproved.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(gate_err["error"], "Approval required");
    let gate_id = gate_err["gate_id"].as_str().unwrap().to_string();

    // 10. Static "X-Approval-Token: APPROVED" bypass must be rejected with 403 Forbidden
    let res_static_bypass = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("X-Approval-Token", "APPROVED")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_static_bypass.status(), StatusCode::FORBIDDEN);

    // 11. Approve gate and receive HMAC-signed approval_token
    let res_approve = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({ "approved": true })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_approve.status(), StatusCode::OK);
    let approve_val: Value = serde_json::from_slice(&to_bytes(res_approve.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(approve_val["approved"], true);
    let approval_token = approve_val["approval_token"].as_str().unwrap().to_string();

    // 12. Execute delete_message with signed X-Approval-Token -> 200 OK & real soft-deletion
    let res_del_auth = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("X-Approval-Token", &approval_token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_del_auth.status(), StatusCode::OK);
    let del_val: Value = serde_json::from_slice(&to_bytes(res_del_auth.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(del_val["success"], true);
    assert_eq!(del_val["result"]["deleted_count"], 1);

    // 13. Single-use approval token cannot be replayed
    let res_replay = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("X-Approval-Token", &approval_token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_replay.status(), StatusCode::FORBIDDEN);

    // 14. Confirm message is soft-deleted in get_chat_history
    let res_hist_after = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/get_chat_history")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "chat_id": real_chat_id,
                    "limit": 10
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_hist_after.status(), StatusCode::OK);
    let hist_after_val: Value = serde_json::from_slice(&to_bytes(res_hist_after.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(hist_after_val["result"]["messages"].as_array().unwrap().is_empty());

    // 15. Non-existent action should return 404
    let req_404 = Request::builder()
        .method("POST")
        .uri("/api/v1/actions/some_unknown_action")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&json!({})).unwrap()))
        .unwrap();

    let res_404 = app.clone().oneshot(req_404).await.unwrap();
    assert_eq!(res_404.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_chunked_upload_concurrent_idor_and_path_traversal() {
    let app = build_router();
    let alice_token = register_and_login(&app, "alice_cas_sec").await;
    let bob_token = register_and_login(&app, "bob_cas_sec").await;

    // 1. Alice initializes an 8192-byte, 8-chunk upload
    let res_init = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/media/upload/init")
                .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "size_bytes": 8192,
                    "chunk_size": 1024,
                    "filename": "parallel.bin"
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_init.status(), StatusCode::CREATED);
    let init_val: Value = serde_json::from_slice(&to_bytes(res_init.into_body(), usize::MAX).await.unwrap()).unwrap();
    let upload_id = init_val["upload_id"].as_str().unwrap().to_string();

    // 2. Upload all 8 chunks concurrently
    let mut set = tokio::task::JoinSet::new();
    for idx in 0..8u32 {
        let app_c = app.clone();
        let tok_c = alice_token.clone();
        let uid_c = upload_id.clone();
        set.spawn(async move {
            let chunk = vec![(idx as u8).wrapping_add(1); 1024];
            let res = app_c
                .oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri(format!("/api/v1/media/upload/{uid_c}/part/{idx}"))
                        .header(header::AUTHORIZATION, format!("Bearer {tok_c}"))
                        .body(Body::from(chunk))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
        });
    }
    while let Some(res) = set.join_next().await {
        res.unwrap();
    }

    // 3. Status must report 100% completion with zero lost bits
    let res_status = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/media/upload/{upload_id}/status"))
                .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_status.status(), StatusCode::OK);
    let status_val: Value = serde_json::from_slice(&to_bytes(res_status.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(status_val["is_complete"], true);
    assert!(status_val["missing_parts"].as_array().unwrap().is_empty());

    // 4. Cross-user IDOR: Bob must receive 403 Forbidden on status, part, and complete
    let res_bob_status = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/media/upload/{upload_id}/status"))
                .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_bob_status.status(), StatusCode::FORBIDDEN);

    let res_bob_part = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/media/upload/{upload_id}/part/0"))
                .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                .body(Body::from(vec![0u8; 1024]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_bob_part.status(), StatusCode::FORBIDDEN);

    let res_bob_complete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/media/upload/{upload_id}/complete"))
                .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_bob_complete.status(), StatusCode::FORBIDDEN);

    // 5. Path traversal upload_id must return 400 Bad Request
    let res_traversal = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/media/upload/..%2F..%2Ffoo/status")
                .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_traversal.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_push_fcm_dispatcher_and_non_ascii_token_safety() {
    let state = Arc::new(AppState::default());
    let app = build_router_with_state(state.clone());

    let reg_user = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "username": "fcm_tester",
                    "password": "Password123!",
                    "first_name": "FCM",
                    "last_name": "User"
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg_user.status(), StatusCode::CREATED);
    let reg_val: Value = serde_json::from_slice(&to_bytes(reg_user.into_body(), usize::MAX).await.unwrap()).unwrap();
    let token = reg_val["token"].as_str().unwrap().to_string();
    let user_id = UserId(reg_val["user_id"].as_u64().unwrap());

    // 1. Register FCM push token
    let res_fcm = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/register_push")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "platform": "fcm",
                    "token": "fcm_token_valid_123456789"
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_fcm.status(), StatusCode::OK);

    // 2. Dispatch via PushGatewayHub -> succeeds with FcmDispatcher
    let notif = PushNotification {
        title: "Test".into(),
        body: "Hello FCM".into(),
        chat_id: None,
        message_id: None,
        badge: None,
        sound: None,
        custom_data: Default::default(),
    };
    let receipts = state
        .push_hub
        .send_to_user(user_id, &notif)
        .await;
    assert_eq!(receipts.len(), 1);
    let receipt = receipts[0].as_ref().expect("FCM dispatch should succeed");
    assert!(receipt.delivered);

    // 3. Multi-byte UTF-8 token preview slicing must not panic
    let res_utf8 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/register_push")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "platform": "mock",
                    "token": "a🎉🎉🎉"
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_utf8.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_single_use_approval_gate_toctou_prevention() {
    let app = build_router();
    let token = register_and_login(&app, "toctou_tester").await;

    // 1. Create a real chat
    let res_chat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "title": "TOCTOU Regression Chat",
                    "chat_type": "group"
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_chat.status(), StatusCode::CREATED);
    let chat_val: Value =
        serde_json::from_slice(&to_bytes(res_chat.into_body(), usize::MAX).await.unwrap()).unwrap();
    let chat_id = chat_val["chat_id"].as_i64().unwrap();

    // 2. Insert a test message via send_message action
    let res_send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/send_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "chat_id": chat_id,
                    "text": "Message guarded by single-use gate"
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_send.status(), StatusCode::OK);
    let send_val: Value =
        serde_json::from_slice(&to_bytes(res_send.into_body(), usize::MAX).await.unwrap()).unwrap();
    let message_id = send_val["result"]["message_id"].as_i64().unwrap();

    // 3. Call delete_message without approval -> 403 Forbidden with Approval required & gate_id
    let del_input = json!({
        "chat_id": chat_id,
        "message_ids": [message_id]
    });
    let res_unapproved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_unapproved.status(), StatusCode::FORBIDDEN);
    let gate_err: Value = serde_json::from_slice(
        &to_bytes(res_unapproved.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(gate_err["error"], "Approval required");
    let gate_id = gate_err["gate_id"].as_str().unwrap().to_string();

    // 4. Approve gate -> 200 OK with single-use approval_token
    let res_approve = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({ "approved": true })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_approve.status(), StatusCode::OK);
    let approve_val: Value = serde_json::from_slice(
        &to_bytes(res_approve.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(approve_val["approved"], true);
    let approval_token = approve_val["approval_token"].as_str().unwrap().to_string();

    // 5. Spawn 16 concurrent requests synchronized via tokio::sync::Barrier
    let concurrency = 16usize;
    let barrier = Arc::new(tokio::sync::Barrier::new(concurrency));
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..concurrency {
        let app_c = app.clone();
        let tok_c = token.clone();
        let approval_c = approval_token.clone();
        let body_bytes = serde_json::to_vec(&del_input).unwrap();
        let bar = barrier.clone();
        set.spawn(async move {
            bar.wait().await;
            let res = app_c
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/actions/delete_message")
                        .header(header::AUTHORIZATION, format!("Bearer {tok_c}"))
                        .header("X-Approval-Token", &approval_c)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body_bytes))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = res.status();
            let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
            let val: Value = serde_json::from_slice(&body).unwrap();
            (status, val)
        });
    }

    let mut ok_count = 0usize;
    let mut forbidden_count = 0usize;
    while let Some(joined) = set.join_next().await {
        let (status, val) = joined.unwrap();
        if status == StatusCode::OK {
            ok_count += 1;
            assert_eq!(val["success"], true);
        } else if status == StatusCode::FORBIDDEN {
            forbidden_count += 1;
            assert_eq!(val["message"], "Approval gate has already been consumed");
        } else {
            panic!("Unexpected status code from concurrent delete_message: {status} ({val})");
        }
    }

    assert_eq!(ok_count, 1, "Exactly 1 concurrent request must succeed with 200 OK");
    assert_eq!(
        forbidden_count, 15,
        "Exactly 15 concurrent requests must fail with 403 Forbidden"
    );

    // 6. Subsequent sequential request with the same token must also return 403 Forbidden
    let res_seq = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("X-Approval-Token", &approval_token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_seq.status(), StatusCode::FORBIDDEN);
    let seq_val: Value =
        serde_json::from_slice(&to_bytes(res_seq.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(seq_val["message"], "Approval gate has already been consumed");
}

#[tokio::test]
async fn test_action_schema_requires_approval_and_always_allow_rejected() {
    let app = build_router();
    let token = register_and_login(&app, "schema_policy_tester").await;

    // 1. GET /api/v1/actions/{action_name}/schema includes requires_approval matching GET /api/v1/actions
    let res_del_schema = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/actions/delete_message/schema")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_del_schema.status(), StatusCode::OK);
    let del_schema: Value = serde_json::from_slice(
        &to_bytes(res_del_schema.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(del_schema["requires_approval"], true);

    let res_send_schema = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/actions/send_message/schema")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_send_schema.status(), StatusCode::OK);
    let send_schema: Value = serde_json::from_slice(
        &to_bytes(res_send_schema.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(send_schema["requires_approval"], false);

    // 2. Create chat & message, trigger approval gate, and verify decision="always_allow" is rejected
    let res_chat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "title": "Policy Chat",
                    "chat_type": "group"
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_chat.status(), StatusCode::CREATED);
    let chat_val: Value =
        serde_json::from_slice(&to_bytes(res_chat.into_body(), usize::MAX).await.unwrap()).unwrap();
    let chat_id = chat_val["chat_id"].as_i64().unwrap();

    let res_send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/send_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "chat_id": chat_id,
                    "text": "Policy check"
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_send.status(), StatusCode::OK);
    let send_val: Value =
        serde_json::from_slice(&to_bytes(res_send.into_body(), usize::MAX).await.unwrap()).unwrap();
    let message_id = send_val["result"]["message_id"].as_i64().unwrap();

    let res_unapproved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "chat_id": chat_id,
                    "message_ids": [message_id]
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_unapproved.status(), StatusCode::FORBIDDEN);
    let gate_err: Value = serde_json::from_slice(
        &to_bytes(res_unapproved.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let gate_id = gate_err["gate_id"].as_str().unwrap().to_string();

    let res_always_allow = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json!({
                    "approved": true,
                    "decision": "always_allow"
                })).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_always_allow.status(), StatusCode::FORBIDDEN);
    let err_val: Value = serde_json::from_slice(
        &to_bytes(res_always_allow.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert!(err_val.get("approval_token").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_multi_node_single_use_approval_gate_replay_prevention() {
    use gpui_telegram_server::db::ServerDatabase;
    use gpui_telegram_server::ServerConfig;

    let tmp_dir = tempfile::tempdir().unwrap();
    let temp_db_path = tmp_dir.path().join("multi_node_approval.db");
    let db_a = Arc::new(ServerDatabase::open(&temp_db_path).unwrap());
    let db_b = Arc::new(ServerDatabase::open(&temp_db_path).unwrap());

    let shared_secret = [0x5Au8; 32];
    let mut s_a = AppState::with_db(ServerConfig::default(), db_a);
    s_a.action_approval_secret = shared_secret;
    let state_a = Arc::new(s_a);
    let app_a = build_router_with_state(state_a.clone());

    let mut s_b = AppState::with_db(ServerConfig::default(), db_b.clone());
    s_b.action_approval_secret = shared_secret;
    let state_b = Arc::new(s_b);
    let app_b = build_router_with_state(state_b.clone());

    // 1. Register test user and create a chat on Node A
    let token = register_and_login(&app_a, "multinode_replay_tester").await;

    let res_chat = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "title": "Multi-Node Replay Chat",
                        "chat_type": "group"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_chat.status(), StatusCode::CREATED);
    let chat_val: Value =
        serde_json::from_slice(&to_bytes(res_chat.into_body(), usize::MAX).await.unwrap()).unwrap();
    let chat_id = chat_val["chat_id"].as_i64().unwrap();

    // 2. Send a message on Node A
    let res_send = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/send_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "chat_id": chat_id,
                        "text": "Message guarded across nodes"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_send.status(), StatusCode::OK);
    let send_val: Value =
        serde_json::from_slice(&to_bytes(res_send.into_body(), usize::MAX).await.unwrap()).unwrap();
    let message_id = send_val["result"]["message_id"].as_i64().unwrap();

    // 3. Trigger 403 on delete_message via Node A to get gate_id
    let del_input = json!({
        "chat_id": chat_id,
        "message_ids": [message_id]
    });
    let res_unapproved = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_unapproved.status(), StatusCode::FORBIDDEN);
    let gate_err: Value = serde_json::from_slice(
        &to_bytes(res_unapproved.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let gate_id = gate_err["gate_id"].as_str().unwrap().to_string();

    // 4. Approve via Node A to obtain single-use approval_token
    let res_approve = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "approved": true })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_approve.status(), StatusCode::OK);
    let approve_val: Value = serde_json::from_slice(
        &to_bytes(res_approve.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(approve_val["approved"], true);
    let approval_token = approve_val["approval_token"].as_str().unwrap().to_string();

    // 5. Pre-populate / load the ApproveOnce gate into Node B's local memory BEFORE Node A executes
    let loaded_gate_b = db_b
        .get_approval_gate(&gate_id)
        .unwrap()
        .expect("Gate must exist in shared SQLite database");
    assert!(loaded_gate_b.is_approved());
    state_b
        .approval_gates
        .write()
        .await
        .insert(gate_id.clone(), loaded_gate_b);

    // 6. Execute delete_message with X-Approval-Token on Node A -> strictly 200 OK
    let res_exec_a = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("X-Approval-Token", &approval_token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_exec_a.status(), StatusCode::OK);
    let exec_a_val: Value = serde_json::from_slice(
        &to_bytes(res_exec_a.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(exec_a_val["success"], true);

    // 7. Replay exact same X-Approval-Token on Node B (which cached ApproveOnce) -> strictly 403 Forbidden
    let res_replay_b = app_b
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("X-Approval-Token", &approval_token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_replay_b.status(), StatusCode::FORBIDDEN);
    let replay_b_val: Value = serde_json::from_slice(
        &to_bytes(res_replay_b.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(replay_b_val["error"], "approval_consumed");
    assert_eq!(
        replay_b_val["message"],
        "Approval gate has already been consumed"
    );

    // Confirm Node B updated its stale local cache to Deny { reason: Some("consumed") }
    let cached_b_after = state_b.approval_gates.read().await.get(&gate_id).cloned().unwrap();
    assert!(cached_b_after.is_denied());
}

#[tokio::test]
async fn test_approval_gate_decision_normalization_and_validation() {
    let app = build_router();
    let token = register_and_login(&app, "decision_norm_tester").await;

    let res_chat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "title": "Decision Normalization Chat",
                        "chat_type": "group"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_chat.status(), StatusCode::CREATED);
    let chat_val: Value =
        serde_json::from_slice(&to_bytes(res_chat.into_body(), usize::MAX).await.unwrap()).unwrap();
    let chat_id = chat_val["chat_id"].as_i64().unwrap();

    let trigger_gate = |app_c: axum::Router, tok: String| async move {
        let res = app_c
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/actions/delete_message")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "chat_id": chat_id,
                            "message_ids": [999999]
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let val: Value =
            serde_json::from_slice(&to_bytes(res.into_body(), usize::MAX).await.unwrap()).unwrap();
        val["gate_id"].as_str().unwrap().to_string()
    };

    let gate_id_1 = trigger_gate(app.clone(), token.clone()).await;

    // 1. Mixed-case "ALWAYS_ALLOW" -> 403 Forbidden (always_allow_not_permitted), gate remains undecided
    let res_upper_always = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id_1}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "approved": true,
                        "decision": " ALWAYS_ALLOW "
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_upper_always.status(), StatusCode::FORBIDDEN);
    let upper_always_val: Value = serde_json::from_slice(
        &to_bytes(res_upper_always.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(upper_always_val["error"], "always_allow_not_permitted");

    // 2. Unrecognized decision "foo" -> 400 Bad Request (invalid_decision), gate remains undecided
    let res_invalid = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id_1}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "approved": true,
                        "decision": "foo"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_invalid.status(), StatusCode::BAD_REQUEST);
    let invalid_val: Value = serde_json::from_slice(
        &to_bytes(res_invalid.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(invalid_val["error"], "invalid_decision");
    assert_eq!(
        invalid_val["message"],
        "Invalid decision. Supported decisions are 'approve_once' or 'deny'"
    );

    // 3. Mixed-case " DENY " -> 200 OK with denied: true
    let res_deny = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id_1}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "decision": " DENY ",
                        "reason": "explicitly denied"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_deny.status(), StatusCode::OK);
    let deny_val: Value =
        serde_json::from_slice(&to_bytes(res_deny.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(deny_val["approved"], false);
    assert_eq!(deny_val["denied"], true);

    // 4. Canonical "APPROVE_ONCE" on a fresh gate -> 200 OK with approval_token
    let gate_id_2 = trigger_gate(app.clone(), token.clone()).await;
    let res_approve_once = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id_2}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "decision": " APPROVE_ONCE "
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_approve_once.status(), StatusCode::OK);
    let approve_once_val: Value = serde_json::from_slice(
        &to_bytes(res_approve_once.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(approve_once_val["approved"], true);
    assert!(approve_once_val["approval_token"].as_str().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_approval_routes_do_not_stall_unrelated_users_during_sqlite_writer_lock() {
    use gpui_telegram_server::db::ServerDatabase;
    use gpui_telegram_server::ServerConfig;
    use std::time::{Duration, Instant};

    let tmp_dir = tempfile::tempdir().unwrap();
    let temp_db_path = tmp_dir.path().join("writer_contention.db");
    let db = Arc::new(ServerDatabase::open(&temp_db_path).unwrap());
    let state = Arc::new(AppState::with_db(ServerConfig::default(), db.clone()));
    let app = build_router_with_state(state.clone());

    let token_u1 = register_and_login(&app, "contention_u1").await;
    let token_u2 = register_and_login(&app, "contention_u2").await;

    let res_chat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats")
                .header(header::AUTHORIZATION, format!("Bearer {token_u1}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "title": "Contention Chat",
                        "chat_type": "group"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_chat.status(), StatusCode::CREATED);
    let chat_val: Value =
        serde_json::from_slice(&to_bytes(res_chat.into_body(), usize::MAX).await.unwrap()).unwrap();
    let chat_id = chat_val["chat_id"].as_i64().unwrap();

    let del_input = json!({
        "chat_id": chat_id,
        "message_ids": [12345]
    });
    let res_unapproved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token_u1}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_unapproved.status(), StatusCode::FORBIDDEN);
    let gate_err: Value = serde_json::from_slice(
        &to_bytes(res_unapproved.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let gate_id = gate_err["gate_id"].as_str().unwrap().to_string();

    let res_approve = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token_u1}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "approved": true })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_approve.status(), StatusCode::OK);
    let approve_val: Value = serde_json::from_slice(
        &to_bytes(res_approve.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let approval_token = approve_val["approval_token"].as_str().unwrap().to_string();

    // Hold SQLite writer lock for 250ms while u1 attempts delete_message
    let held = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let held_flag = held.clone();
    let db_w = db.clone();
    let writer_thread = std::thread::spawn(move || {
        db_w.pool()
            .with_transaction(|_tx| {
                held_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(250));
                Ok(())
            })
            .unwrap();
    });
    while !held.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let app_u1 = app.clone();
    let u1_task = tokio::spawn(async move {
        app_u1
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/actions/delete_message")
                    .header(header::AUTHORIZATION, format!("Bearer {token_u1}"))
                    .header("X-Approval-Token", &approval_token)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    });

    // Give u1 a moment to reach try_consume_approval_gate (blocked on SQLite writer)
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Unrelated u2 /approve(404) must NOT stall behind u1 on approval_gates.write().await
    let start_u2 = Instant::now();
    let res_u2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/approvals/nonexistent_gate_id/approve")
                .header(header::AUTHORIZATION, format!("Bearer {token_u2}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "approved": true })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let elapsed_u2 = start_u2.elapsed();
    assert_eq!(res_u2.status(), StatusCode::NOT_FOUND);
    assert!(
        elapsed_u2 < Duration::from_millis(100),
        "Unrelated user's /approve stalled behind SQLite writer lock: {elapsed_u2:?}"
    );

    let res_u1 = u1_task.await.unwrap();
    assert_eq!(res_u1.status(), StatusCode::OK);
    writer_thread.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_multi_node_approval_gate_decision_race_and_overwrite_prevention() {
    use gpui_telegram_server::db::ServerDatabase;
    use gpui_telegram_server::ServerConfig;

    let tmp_dir = tempfile::tempdir().unwrap();
    let temp_db_path = tmp_dir.path().join("multi_node_decision_race.db");
    let db_a = Arc::new(ServerDatabase::open(&temp_db_path).unwrap());
    let db_b = Arc::new(ServerDatabase::open(&temp_db_path).unwrap());

    let shared_secret = [0x6Bu8; 32];
    let mut s_a = AppState::with_db(ServerConfig::default(), db_a);
    s_a.action_approval_secret = shared_secret;
    let state_a = Arc::new(s_a);
    let app_a = build_router_with_state(state_a.clone());

    let mut s_b = AppState::with_db(ServerConfig::default(), db_b.clone());
    s_b.action_approval_secret = shared_secret;
    let state_b = Arc::new(s_b);
    let app_b = build_router_with_state(state_b.clone());

    let query_db_decision = |db: &ServerDatabase, gate_id: &str| -> Option<String> {
        db.pool()
            .with_reader(|conn| {
                conn.query_row(
                    "SELECT decision FROM acp_approval_gates WHERE gate_id = ?1",
                    [gate_id],
                    |r| r.get(0),
                )
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
            })
            .unwrap()
    };

    // 1. Register test user, create chat, and send a message on Node A
    let token = register_and_login(&app_a, "multinode_race_tester").await;

    let res_chat = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "title": "Decision Race Chat",
                        "chat_type": "group"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_chat.status(), StatusCode::CREATED);
    let chat_val: Value =
        serde_json::from_slice(&to_bytes(res_chat.into_body(), usize::MAX).await.unwrap()).unwrap();
    let chat_id = chat_val["chat_id"].as_i64().unwrap();

    let res_send = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/send_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "chat_id": chat_id,
                        "text": "Race guard message"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_send.status(), StatusCode::OK);
    let send_val: Value =
        serde_json::from_slice(&to_bytes(res_send.into_body(), usize::MAX).await.unwrap()).unwrap();
    let message_id = send_val["result"]["message_id"].as_i64().unwrap();

    // 2. Create unapproved gate G on Node A
    let del_input = json!({
        "chat_id": chat_id,
        "message_ids": [message_id]
    });
    let res_unapproved = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_unapproved.status(), StatusCode::FORBIDDEN);
    let gate_err: Value = serde_json::from_slice(
        &to_bytes(res_unapproved.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let gate_id = gate_err["gate_id"].as_str().unwrap().to_string();

    // 3. Ensure Node B caches gate G with decision: None (simulating startup hydration or prior probe)
    let undecided_gate_b = db_b
        .get_approval_gate(&gate_id)
        .unwrap()
        .expect("Gate must exist in shared SQLite");
    assert!(undecided_gate_b.decision.is_none());
    state_b
        .approval_gates
        .write()
        .await
        .insert(gate_id.clone(), undecided_gate_b);

    // 4. On Node A, approve G and execute delete_message -> 200 OK (SQLite decision = 'consumed')
    let res_approve_a = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "approved": true })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_approve_a.status(), StatusCode::OK);
    let approve_a_val: Value = serde_json::from_slice(
        &to_bytes(res_approve_a.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let approval_token = approve_a_val["approval_token"]
        .as_str()
        .unwrap()
        .to_string();

    let res_exec_a = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("X-Approval-Token", &approval_token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_exec_a.status(), StatusCode::OK);
    assert_eq!(
        query_db_decision(&db_b, &gate_id).as_deref(),
        Some("consumed")
    );

    // 5. On Node B (holding stale decision: None in memory), attempt /approve -> strictly 409 Conflict
    let res_approve_b = app_b
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "approved": true })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_approve_b.status(), StatusCode::CONFLICT);
    let approve_b_val: Value = serde_json::from_slice(
        &to_bytes(res_approve_b.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(approve_b_val["error"], "already_decided");
    assert!(approve_b_val.get("approval_token").is_none());

    // SQLite decision MUST remain 'consumed' (NOT overwritten to 'approve_once')
    assert_eq!(
        query_db_decision(&db_b, &gate_id).as_deref(),
        Some("consumed")
    );

    // 6. Subsequent replay of delete_message on both Node A and Node B returns 403 Forbidden (approval_consumed)
    for (label, router) in [("Node A", app_a.clone()), ("Node B", app_b.clone())] {
        let res_replay = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/actions/delete_message")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("X-Approval-Token", &approval_token)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res_replay.status(),
            StatusCode::FORBIDDEN,
            "{label} replay must return 403 Forbidden"
        );
        let replay_val: Value = serde_json::from_slice(
            &to_bytes(res_replay.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(replay_val["error"], "approval_consumed");
    }

    // 7. Denial override prevention: Create gate G2 on Node A, cache undecided on Node B, deny on Node A, attempt approve on Node B
    let res_unapproved_2 = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_unapproved_2.status(), StatusCode::FORBIDDEN);
    let gate_err_2: Value = serde_json::from_slice(
        &to_bytes(res_unapproved_2.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let gate_id_2 = gate_err_2["gate_id"].as_str().unwrap().to_string();

    // Populate Node B's cache with undecided G2
    let undecided_g2_b = db_b.get_approval_gate(&gate_id_2).unwrap().unwrap();
    assert!(undecided_g2_b.decision.is_none());
    state_b
        .approval_gates
        .write()
        .await
        .insert(gate_id_2.clone(), undecided_g2_b);

    // Also verify try_consume_approval_gate refuses to consume an undecided (NULL) gate
    assert!(
        !db_b
            .try_consume_approval_gate(&gate_id_2, &json!({}))
            .unwrap(),
        "try_consume_approval_gate must not consume NULL decision rows"
    );
    assert_eq!(query_db_decision(&db_b, &gate_id_2), None);

    // Deny G2 on Node A
    let res_deny_a = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id_2}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "decision": "deny" })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_deny_a.status(), StatusCode::OK);
    assert_eq!(
        query_db_decision(&db_b, &gate_id_2).as_deref(),
        Some("deny")
    );

    // Verify try_consume_approval_gate refuses to consume a 'deny' row
    assert!(
        !db_b
            .try_consume_approval_gate(&gate_id_2, &json!({}))
            .unwrap(),
        "try_consume_approval_gate must not consume 'deny' decision rows"
    );
    assert_eq!(
        query_db_decision(&db_b, &gate_id_2).as_deref(),
        Some("deny")
    );

    // Attempt to approve G2 on Node B -> strictly 409 Conflict, SQLite remains 'deny'
    let res_approve_g2_b = app_b
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id_2}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "approved": true })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_approve_g2_b.status(), StatusCode::CONFLICT);
    let approve_g2_b_val: Value = serde_json::from_slice(
        &to_bytes(res_approve_g2_b.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(approve_g2_b_val["error"], "already_decided");
    assert!(approve_g2_b_val.get("approval_token").is_none());
    assert_eq!(
        query_db_decision(&db_b, &gate_id_2).as_deref(),
        Some("deny")
    );

    // 8. Concurrent multi-node /approve race on fresh gate G3 (Node A: deny vs Node B: approve_once)
    let res_unapproved_3 = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_unapproved_3.status(), StatusCode::FORBIDDEN);
    let gate_err_3: Value = serde_json::from_slice(
        &to_bytes(res_unapproved_3.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let gate_id_3 = gate_err_3["gate_id"].as_str().unwrap().to_string();

    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let bar_a = barrier.clone();
    let bar_b = barrier.clone();
    let app_a_race = app_a.clone();
    let app_b_race = app_b.clone();
    let tok_a = token.clone();
    let tok_b = token.clone();
    let gid_a = gate_id_3.clone();
    let gid_b = gate_id_3.clone();

    let task_a = tokio::spawn(async move {
        bar_a.wait().await;
        app_a_race
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/actions/approvals/{gid_a}/approve"))
                    .header(header::AUTHORIZATION, format!("Bearer {tok_a}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({ "decision": "deny" })).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    });

    let task_b = tokio::spawn(async move {
        bar_b.wait().await;
        app_b_race
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/actions/approvals/{gid_b}/approve"))
                    .header(header::AUTHORIZATION, format!("Bearer {tok_b}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({ "decision": "approve_once" })).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    });

    let (status_a, status_b) = tokio::join!(task_a, task_b);
    let status_a = status_a.unwrap();
    let status_b = status_b.unwrap();
    assert!(
        (status_a == StatusCode::OK && status_b == StatusCode::CONFLICT)
            || (status_a == StatusCode::CONFLICT && status_b == StatusCode::OK),
        "Exactly one node must win concurrent /approve and the other must get 409 Conflict; got A={status_a}, B={status_b}"
    );
    let final_g3_decision = query_db_decision(&db_b, &gate_id_3);
    if status_a == StatusCode::OK {
        assert_eq!(final_g3_decision.as_deref(), Some("deny"));
    } else {
        assert_eq!(final_g3_decision.as_deref(), Some("approve_once"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_cross_route_task_approve_cannot_resurrect_consumed_action_gate() {
    use gpui_telegram_server::db::ServerDatabase;
    use gpui_telegram_server::ServerConfig;

    let tmp_dir = tempfile::tempdir().unwrap();
    let temp_db_path = tmp_dir.path().join("cross_route_task_approve.db");
    let db_a = Arc::new(ServerDatabase::open(&temp_db_path).unwrap());
    let db_b = Arc::new(ServerDatabase::open(&temp_db_path).unwrap());

    let shared_secret = [0x7Cu8; 32];
    let mut s_a = AppState::with_db(ServerConfig::default(), db_a);
    s_a.action_approval_secret = shared_secret;
    let state_a = Arc::new(s_a);
    let app_a = build_router_with_state(state_a.clone());

    let mut s_b = AppState::with_db(ServerConfig::default(), db_b.clone());
    s_b.action_approval_secret = shared_secret;
    let state_b = Arc::new(s_b);
    let app_b = build_router_with_state(state_b.clone());

    let query_db_decision = |db: &ServerDatabase, gate_id: &str| -> Option<String> {
        db.pool()
            .with_reader(|conn| {
                conn.query_row(
                    "SELECT decision FROM acp_approval_gates WHERE gate_id = ?1",
                    [gate_id],
                    |r| r.get(0),
                )
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
            })
            .unwrap()
    };

    // 1. Register user, create chat, and send a message on Node A
    let token = register_and_login(&app_a, "cross_route_tester").await;

    let res_chat = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "title": "Cross-Route Guard Chat",
                        "chat_type": "group"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_chat.status(), StatusCode::CREATED);
    let chat_val: Value =
        serde_json::from_slice(&to_bytes(res_chat.into_body(), usize::MAX).await.unwrap()).unwrap();
    let chat_id = chat_val["chat_id"].as_i64().unwrap();

    let res_send = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/send_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "chat_id": chat_id,
                        "text": "Protected message for cross-route test"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_send.status(), StatusCode::OK);
    let send_val: Value =
        serde_json::from_slice(&to_bytes(res_send.into_body(), usize::MAX).await.unwrap()).unwrap();
    let message_id = send_val["result"]["message_id"].as_i64().unwrap();

    // 2. Trigger delete_message on Node A -> 403 Forbidden with actgate_* gate
    let del_input = json!({
        "chat_id": chat_id,
        "message_ids": [message_id]
    });
    let res_unapproved = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_unapproved.status(), StatusCode::FORBIDDEN);
    let gate_err: Value = serde_json::from_slice(
        &to_bytes(res_unapproved.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let gate_id = gate_err["gate_id"].as_str().unwrap().to_string();
    assert!(gate_id.starts_with("actgate_"));

    // 3. Hydrate/cache the undecided gate on Node B (and verify startup hydration on Node C)
    let undecided_gate_b = db_b
        .get_approval_gate(&gate_id)
        .unwrap()
        .expect("Gate must exist in shared SQLite");
    assert!(undecided_gate_b.decision.is_none());
    let action_task_id = undecided_gate_b.task_id.clone();
    assert!(action_task_id.starts_with("action:"));
    state_b
        .approval_gates
        .write()
        .await
        .insert(gate_id.clone(), undecided_gate_b);

    let mut s_c = AppState::with_db(ServerConfig::default(), db_b.clone());
    s_c.action_approval_secret = shared_secret;
    let state_c = Arc::new(s_c);
    assert!(state_c.approval_gates.read().await.contains_key(&gate_id));
    let app_c = build_router_with_state(state_c);

    // 4. Approve on Node A via Action Bus approval endpoint and execute delete_message -> 200 OK & consumed
    let res_approve_a = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/actions/approvals/{gate_id}/approve"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "decision": "approve_once" })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_approve_a.status(), StatusCode::OK);
    let approve_a_val: Value = serde_json::from_slice(
        &to_bytes(res_approve_a.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let approval_token = approve_a_val["approval_token"]
        .as_str()
        .unwrap()
        .to_string();

    let res_exec_a = app_a
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/actions/delete_message")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("X-Approval-Token", &approval_token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_exec_a.status(), StatusCode::OK);
    assert_eq!(
        query_db_decision(&db_b, &gate_id).as_deref(),
        Some("consumed")
    );

    // 5. Call legacy POST /api/v1/tasks/{task_id}/approve on Node B with gate_id and action task_id
    for target_id in [&gate_id, &action_task_id] {
        let res_legacy = app_b
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/tasks/{target_id}/approve"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({ "approved": true })).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res_legacy.status(),
            StatusCode::BAD_REQUEST,
            "Legacy /tasks/{target_id}/approve must reject action gate requests with 400"
        );
        let legacy_val: Value = serde_json::from_slice(
            &to_bytes(res_legacy.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(legacy_val["success"], false);
        assert_eq!(legacy_val["error"], "invalid_task_id");
        assert_eq!(
            query_db_decision(&db_b, &gate_id).as_deref(),
            Some("consumed"),
            "SQLite decision must remain 'consumed'"
        );
    }

    // 6. Verify post-lookup guard: even if an action gate were indexed under a non-prefixed key in memory,
    //    tasks_approve must reject it without mutating the gate or SQLite.
    let mut aliased_gate = db_b.get_approval_gate(&gate_id).unwrap().unwrap();
    aliased_gate.decision = None;
    state_b
        .approval_gates
        .write()
        .await
        .insert("alias_lookup_key".to_string(), aliased_gate);
    let res_alias = app_b
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tasks/alias_lookup_key/approve")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "approved": true })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_alias.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        query_db_decision(&db_b, &gate_id).as_deref(),
        Some("consumed")
    );
    assert!(
        state_b
            .approval_gates
            .read()
            .await
            .get("alias_lookup_key")
            .unwrap()
            .decision
            .is_none(),
        "Post-lookup rejection must not call gate.decide"
    );

    // 7. Replay delete_message with consumed token on Node B and Node C -> 403 Forbidden (approval_consumed)
    for (label, router) in [("Node B", app_b.clone()), ("Node C", app_c.clone())] {
        let res_replay = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/actions/delete_message")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("X-Approval-Token", &approval_token)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&del_input).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res_replay.status(),
            StatusCode::FORBIDDEN,
            "{label} replay must be rejected with 403 Forbidden"
        );
        let replay_val: Value = serde_json::from_slice(
            &to_bytes(res_replay.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(replay_val["error"], "approval_consumed");
    }
}
