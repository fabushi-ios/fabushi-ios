use axum::{body::{to_bytes, Body}, http::{header, Method, Request, StatusCode}, Router};
use gpui_telegram_server::{build_router_with_state, server::AppState, ServerConfig};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
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
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn open_durable(dir: &TempDirGuard) -> (Arc<AppState>, Router) {
    let cfg = ServerConfig {
        database_url: dir.0.join("feat.db").to_string_lossy().into_owned(),
        media_storage_path: dir.0.join("cas").to_string_lossy().into_owned(),
        ..Default::default()
    };
    let state = Arc::new(AppState::try_new(cfg).unwrap());
    let app = build_router_with_state(state.clone());
    (state, app)
}

async fn api(app: &Router, method: Method, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token { b = b.header(header::AUTHORIZATION, format!("Bearer {t}")); }
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let st = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn reg(app: &Router, uname: &str) -> (u64, String) {
    let (st, v) = api(app, Method::POST, "/api/v1/auth/register", None, Some(json!({
        "username": uname, "password": "Password!1", "first_name": uname
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    (v["user_id"].as_u64().unwrap(), v["token"].as_str().unwrap().to_string())
}

fn fake_png_320x240() -> Vec<u8> {
    let mut b = Vec::from(&b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"[..]);
    b.extend_from_slice(&320u32.to_be_bytes());
    b.extend_from_slice(&240u32.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    b.extend_from_slice(&[0u8; 64]);
    b
}

fn wav_pcm_bytes(samples: usize) -> Vec<u8> {
    let data_len = (samples * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&8000u32.to_le_bytes());
    b.extend_from_slice(&16000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..samples { b.extend_from_slice((((i % 64) as i16) * 400).to_le_bytes().as_slice()); }
    b
}

#[tokio::test]
async fn test_messaging_edit_revoke_reactions_polls_read_cursors_and_sync() {
    let dir = TempDirGuard::new("tg_parity_msg");
    let (_, app) = open_durable(&dir);
    let (_aid, a_tok) = reg(&app, "alice_msg").await;
    let (bid, b_tok) = reg(&app, "bob_msg").await;

    let (st, chat) = api(&app, Method::POST, "/api/v1/chats", Some(&a_tok), Some(json!({
        "title": "Parity Group", "chat_type": "group", "member_ids": [bid]
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let cid = chat["chat_id"].as_i64().unwrap();

    let (st, s1) = api(&app, Method::POST, &format!("/api/v1/chats/{cid}/messages"), Some(&a_tok), Some(json!({
        "text": "Hello Rust GPUI!", "client_nonce": 9901,
        "entities": [{ "kind": "bold", "offset": 6, "length": 4 }]
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let mid = s1["message_id"].as_i64().unwrap();
    let (st_dup, s_dup) = api(&app, Method::POST, &format!("/api/v1/chats/{cid}/messages"), Some(&a_tok), Some(json!({
        "text": "Hello Rust GPUI!", "client_nonce": 9901
    }))).await;
    assert_eq!(st_dup, StatusCode::CREATED);
    assert_eq!(s_dup["message_id"], mid);

    let (st_forbid, _) = api(&app, Method::PUT, &format!("/api/v1/chats/{cid}/messages/{mid}"), Some(&b_tok), Some(json!({ "text": "Hacked" }))).await;
    assert_eq!(st_forbid, StatusCode::FORBIDDEN);
    let (st_edit, ed) = api(&app, Method::PUT, &format!("/api/v1/chats/{cid}/messages/{mid}"), Some(&a_tok), Some(json!({ "text": "Edited Rust GPUI!" }))).await;
    assert_eq!(st_edit, StatusCode::OK);
    assert_eq!(ed["text"], "Edited Rust GPUI!");

    let (st, rx) = api(&app, Method::POST, &format!("/api/v1/chats/{cid}/messages/{mid}/reactions"), Some(&b_tok), Some(json!({ "emoji": "🔥", "action": "toggle" }))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        rx["reactions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["emoji"] == "🔥" && r["count"] == 1),
        "expected 🔥 reaction with count 1 in {:?}",
        rx
    );

    let (st, poll_res) = api(&app, Method::POST, &format!("/api/v1/chats/{cid}/polls"), Some(&a_tok), Some(json!({
        "question": "Best systems language?", "options": ["Go", "Rust", "C++"],
        "is_anonymous": false, "is_quiz": true, "correct_option_id": 1, "solution": "Zero-cost abstractions!"
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let pid = poll_res["poll"]["poll_id"].as_i64().unwrap();
    let (st, voted) = api(&app, Method::POST, &format!("/api/v1/polls/{pid}/vote"), Some(&b_tok), Some(json!({ "option_ids": [1] }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(voted["poll"]["total_voters"], 1);
    assert_eq!(voted["poll"]["solution"], "Zero-cost abstractions!");
    let (st, closed) = api(&app, Method::POST, &format!("/api/v1/polls/{pid}/close"), Some(&a_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(closed["poll"]["is_closed"], true);

    let (st, b_chat) = api(&app, Method::GET, &format!("/api/v1/chats/{cid}"), Some(&b_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(b_chat["unread_count"].as_u64().unwrap() >= 2);
    let poll_msg_id = poll_res["message_id"].as_i64().unwrap();
    let (st, rd) = api(&app, Method::POST, &format!("/api/v1/chats/{cid}/read"), Some(&b_tok), Some(json!({ "max_id": poll_msg_id }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rd["unread_count"], 0);
    let (st, diff) = api(&app, Method::POST, "/api/v1/sync/get_difference", Some(&b_tok), Some(json!({ "chat_id": cid, "pts": 0 }))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!diff["updates"].as_array().unwrap().is_empty());

    let (st, del) = api(&app, Method::DELETE, &format!("/api/v1/chats/{cid}/messages/{mid}?revoke=true"), Some(&a_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(del["revoke"], true);
}

#[tokio::test]
async fn test_media_206_social_calls_search_stats_export_wallet_and_acp_reopen() {
    let dir = TempDirGuard::new("tg_parity_feat");
    let (state, app) = open_durable(&dir);
    let (aid, a_tok) = reg(&app, "alice_feat").await;
    let (bid, b_tok) = reg(&app, "bob_feat").await;
    let (_cid_u, c_tok) = reg(&app, "charlie_outsider").await;

    let (_, chat) = api(&app, Method::POST, "/api/v1/chats", Some(&a_tok), Some(json!({
        "title": "Media Channel", "chat_type": "group", "member_ids": [bid]
    }))).await;
    let cid = chat["chat_id"].as_i64().unwrap();

    let png = fake_png_320x240();
    let (st, init) = api(&app, Method::POST, "/api/v1/media/upload/init", Some(&a_tok), Some(json!({
        "filename": "pic.png", "size_bytes": png.len(), "chunk_size": 1024
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let uid = init["upload_id"].as_str().unwrap();
    let part_req = Request::builder().method(Method::PUT).uri(format!("/api/v1/media/upload/{uid}/part/0"))
        .header(header::AUTHORIZATION, format!("Bearer {a_tok}")).body(Body::from(png.clone())).unwrap();
    assert_eq!(app.clone().oneshot(part_req).await.unwrap().status(), StatusCode::OK);
    let (st, comp) = api(&app, Method::POST, &format!("/api/v1/media/upload/{uid}/complete"), Some(&a_tok), Some(json!({
        "filename": "pic.png", "mime_type": "image/png", "chat_id": cid
    }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!((comp["width"].as_u64(), comp["height"].as_u64()), (Some(320), Some(240)));
    let file_id = comp["file_id"].as_str().unwrap().to_string();

    let range_req = Request::builder().method(Method::GET).uri(format!("/api/v1/media/{file_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {b_tok}")).header(header::RANGE, "bytes=0-15").body(Body::empty()).unwrap();
    let r206 = app.clone().oneshot(range_req).await.unwrap();
    assert_eq!(r206.status(), StatusCode::PARTIAL_CONTENT);
    let deny_req = Request::builder().method(Method::GET).uri(format!("/api/v1/media/{file_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {c_tok}")).body(Body::empty()).unwrap();
    assert_eq!(app.clone().oneshot(deny_req).await.unwrap().status(), StatusCode::FORBIDDEN);

    let wav = wav_pcm_bytes(8000);
    let wav_req = Request::builder().method(Method::POST).uri("/api/v1/media/upload?filename=voice.wav&mime_type=audio/wav&is_voice=true")
        .header(header::AUTHORIZATION, format!("Bearer {a_tok}")).body(Body::from(wav)).unwrap();
    let wav_res = app.clone().oneshot(wav_req).await.unwrap();
    assert_eq!(wav_res.status(), StatusCode::CREATED);
    let wjson: Value = serde_json::from_slice(&to_bytes(wav_res.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(wjson["waveform"].as_array().unwrap().len(), 100);

    let (st, sets) = api(&app, Method::GET, "/api/v1/stickers/sets", Some(&a_tok), None).await;
    assert_eq!(st, StatusCode::OK);
    let set_id = sets["sets"][0]["set_id"].as_str().unwrap().to_string();
    assert_eq!(api(&app, Method::POST, &format!("/api/v1/stickers/sets/{set_id}/install"), Some(&a_tok), None).await.0, StatusCode::OK);
    assert!(!api(&app, Method::GET, "/api/v1/gifs/search?q=rocket", Some(&a_tok), None).await.1["gifs"].as_array().unwrap().is_empty());

    let (st, story) = api(&app, Method::POST, "/api/v1/stories", Some(&a_tok), Some(json!({ "media_id": file_id, "caption": "24h" }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let sid = story["story_id"].as_str().unwrap().to_string();
    assert_eq!(api(&app, Method::POST, &format!("/api/v1/stories/{sid}/view"), Some(&b_tok), None).await.0, StatusCode::OK);
    assert_eq!(api(&app, Method::DELETE, &format!("/api/v1/stories/{sid}"), Some(&a_tok), None).await.0, StatusCode::OK);

    let (st, call) = api(&app, Method::POST, "/api/v1/calls/invite", Some(&a_tok), Some(json!({ "callee_id": bid, "is_video": true, "chat_id": cid }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let call_id = call["call_id"].as_str().unwrap().to_string();
    assert_eq!(api(&app, Method::POST, "/api/v1/calls/signal", Some(&b_tok), Some(json!({ "call_id": call_id, "signal_type": "answer", "sdp": "v=0" }))).await.0, StatusCode::OK);
    assert_eq!(api(&app, Method::POST, &format!("/api/v1/calls/{call_id}/rate"), Some(&a_tok), Some(json!({ "rating": 5 }))).await.0, StatusCode::OK);
    assert!(api(&app, Method::GET, "/api/v1/webrtc/turn_credentials", Some(&a_tok), None).await.1["credential"].as_str().is_some());

    let _ = api(&app, Method::POST, &format!("/api/v1/chats/{cid}/messages"), Some(&a_tok), Some(json!({ "text": "UniqueTantivyKeyword42" }))).await;
    assert_eq!(api(&app, Method::GET, "/api/v1/search?q=UniqueTantivyKeyword42", Some(&b_tok), None).await.1["total"], 1);
    assert_eq!(api(&app, Method::GET, "/api/v1/search?q=UniqueTantivyKeyword42", Some(&c_tok), None).await.1["total"], 0);
    assert!(api(&app, Method::GET, &format!("/api/v1/chats/{cid}/statistics"), Some(&a_tok), None).await.1["member_count"].as_u64().unwrap() >= 2);
    assert_eq!(api(&app, Method::GET, &format!("/api/v1/chats/{cid}/export"), Some(&a_tok), None).await.0, StatusCode::OK);

    assert_eq!(api(&app, Method::POST, "/api/v1/wallet/topup", Some(&a_tok), Some(json!({ "amount_stars": 500 }))).await.0, StatusCode::OK);
    let (st, gift) = api(&app, Method::POST, "/api/v1/wallet/send_stars", Some(&a_tok), Some(json!({ "to_user_id": bid, "amount_stars": 120, "message": "Thanks" }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(gift["balance"]["stars_balance"], 380);
    let _ = aid;

    assert_eq!(api(&app, Method::POST, "/api/v1/tasks", Some(&a_tok), Some(json!({ "task_id": "durable_task_1" }))).await.0, StatusCode::CREATED);
    assert_eq!(api(&app, Method::POST, "/api/v1/mcp/servers", Some(&a_tok), Some(json!({ "server_id": "mcp_1", "name": "Local", "endpoint": "stdio://mcp" }))).await.0, StatusCode::CREATED);
    drop(app);
    drop(state);
    let (_s2, app2) = open_durable(&dir);
    assert_eq!(api(&app2, Method::GET, "/api/v1/tasks", Some(&a_tok), None).await.1["tasks"][0]["task_id"], "durable_task_1");
    assert_eq!(api(&app2, Method::GET, "/api/v1/mcp/servers", Some(&a_tok), None).await.1["servers"][0]["server_id"], "mcp_1");
}
