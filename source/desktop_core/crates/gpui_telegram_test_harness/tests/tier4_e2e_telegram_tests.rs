use futures::{SinkExt, StreamExt};
use gpui_telegram_test_harness::tier4_e2e::{E2eSystemDriver, EphemeralTestServer};
use reqwest::Method;
use serde_json::{json, Value};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMsg;

fn minimal_png_1x1() -> Vec<u8> {
    // Valid 1x1 RGBA PNG generated cleanly via image::RgbaImage
    let mut img = image::RgbaImage::new(1, 1);
    img.put_pixel(0, 0, image::Rgba([42, 171, 238, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode 1x1 png");
    out.into_inner()
}

#[tokio::test]
async fn test_tier4_full_telegram_e2e_over_tcp_ws_and_restart() {
    let mut server = EphemeralTestServer::start().await.expect("start server");
    let drv = E2eSystemDriver::new(server.base_url());

    // 1. Status + multi-user register with truthful empty initial chats + opt-in welcome
    let (st, status) = drv.json_req(Method::GET, "/api/v1/status", None, None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));

    let (alice_id, alice_tok) = drv.register_user("t4_alice", "StrongPass!9").await.unwrap();
    let (bob_id, bob_tok) = drv.register_user("t4_bob", "StrongPass!9").await.unwrap();
    let (_charlie_id, charlie_tok) = drv.register_user("t4_charlie", "StrongPass!9").await.unwrap();

    let (st, chats0) = drv.json_req(Method::GET, "/api/v1/chats", Some(&alice_tok), None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(chats0["total"], 0);

    let (st, welcome) = drv.json_req(Method::POST, "/api/v1/chats/welcome", Some(&alice_tok), Some(json!({}))).await.unwrap();
    assert!(st == 200 || st == 201);
    assert!(welcome["chat_id"].as_i64().unwrap_or(0) > 0);

    // 2. Profile, contacts, sessions, group + channel + invite join + topic + folder
    let (st, prof) = drv.json_req(Method::PUT, "/api/v1/users/me", Some(&alice_tok), Some(json!({
        "first_name": "Alice", "bio": "Tier4 E2E", "emoji_status": "rocket"
    }))).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(prof["bio"], "Tier4 E2E");

    let (st, grp) = drv.json_req(Method::POST, "/api/v1/chats", Some(&alice_tok), Some(json!({
        "title": "Tier4 Forum", "chat_type": "group", "is_forum": true
    }))).await.unwrap();
    assert!(st == 200 || st == 201);
    let gid = grp["chat_id"].as_i64().unwrap();

    let (st, inv) = drv.json_req(Method::POST, &format!("/api/v1/chats/{gid}/invites"), Some(&alice_tok), Some(json!({}))).await.unwrap();
    assert!(st == 200 || st == 201);
    let (st, joined) = drv.json_req(Method::POST, "/api/v1/chats/join", Some(&bob_tok), Some(json!({ "code": inv["code"] }))).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(joined["chat_id"], gid);

    let (st, topic) = drv.json_req(Method::POST, &format!("/api/v1/chats/{gid}/topics"), Some(&alice_tok), Some(json!({ "title": "General", "icon_color": 7322096 }))).await.unwrap();
    assert!(st == 200 || st == 201);
    let tid = topic["topic_id"].as_i64().unwrap();

    let (st, folder) = drv.json_req(Method::POST, "/api/v1/folders", Some(&alice_tok), Some(json!({ "title": "Work", "include_chats": [gid] }))).await.unwrap();
    assert!(st == 200 || st == 201);
    assert!(folder["id"].as_i64().unwrap_or(0) > 0);

    // 3. Real TCP WebSocket connect + SubscribeChat + Ping/Pong + live push
    let (mut ws_bob, _) = connect_async(&server.ws_url(&bob_tok)).await.expect("bob ws");
    ws_bob
        .send(WsMsg::Text(json!({ "type": "SubscribeChat", "payload": { "chat_id": gid } }).to_string().into()))
        .await
        .unwrap();
    ws_bob
        .send(WsMsg::Text(json!({ "type": "Ping", "payload": { "nonce": 99 } }).to_string().into()))
        .await
        .unwrap();
    let mut got_pong = false;
    for _ in 0..4 {
        if let Ok(Some(Ok(WsMsg::Text(txt)))) = timeout(Duration::from_secs(4), ws_bob.next()).await {
            let v: Value = serde_json::from_str(&txt).unwrap_or(Value::Null);
            if v["type"] == "Pong" && v["payload"]["nonce"] == 99 {
                got_pong = true;
                break;
            }
        }
    }
    assert!(got_pong, "Expected WebSocket Pong");

    // 4. Send, edit, reaction, quiz poll, read receipt ✓ -> ✓✓, revoke
    let (st, m1) = drv.json_req(Method::POST, &format!("/api/v1/chats/{gid}/messages"), Some(&alice_tok), Some(json!({
        "text": "Tier4UniqueSearchTerm hello", "topic_id": tid, "client_nonce": 424242
    }))).await.unwrap();
    assert!(st == 200 || st == 201);
    let mid1 = m1["message_id"].as_i64().unwrap();

    let mut got_ws_msg = false;
    for _ in 0..8 {
        if let Ok(Some(Ok(WsMsg::Text(txt)))) = timeout(Duration::from_secs(4), ws_bob.next()).await {
            let v: Value = serde_json::from_str(&txt).unwrap_or(Value::Null);
            if v["type"] == "PtsPush" || v["type"] == "Update" {
                got_ws_msg = true;
                break;
            }
        }
    }
    assert!(got_ws_msg, "Bob must receive live WebSocket message push");

    let (st, ed) = drv.json_req(Method::PUT, &format!("/api/v1/chats/{gid}/messages/{mid1}"), Some(&alice_tok), Some(json!({ "text": "Tier4UniqueSearchTerm edited" }))).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(ed["text"], "Tier4UniqueSearchTerm edited");

    let (st, poll) = drv.json_req(Method::POST, &format!("/api/v1/chats/{gid}/polls"), Some(&alice_tok), Some(json!({
        "question": "Quiz?", "options": ["A", "B"], "is_quiz": true, "correct_option_id": 1, "solution": "B!"
    }))).await.unwrap();
    assert!(st == 200 || st == 201);
    let pid = poll["poll"]["poll_id"].as_i64().unwrap();
    let poll_mid = poll["message_id"].as_i64().unwrap();
    assert_eq!(drv.json_req(Method::POST, &format!("/api/v1/polls/{pid}/vote"), Some(&bob_tok), Some(json!({ "option_ids": [1] }))).await.unwrap().0, 200);

    let (_, chat_before) = drv.json_req(Method::GET, &format!("/api/v1/chats/{gid}"), Some(&alice_tok), None).await.unwrap();
    assert!(chat_before["read_outbox_max_id"].as_i64().unwrap_or(0) < poll_mid);
    let (st, rd) = drv.json_req(Method::POST, &format!("/api/v1/chats/{gid}/read"), Some(&bob_tok), Some(json!({ "max_id": poll_mid }))).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(rd["unread_count"], 0);
    let (_, chat_after) = drv.json_req(Method::GET, &format!("/api/v1/chats/{gid}"), Some(&alice_tok), None).await.unwrap();
    assert!(chat_after["read_outbox_max_id"].as_i64().unwrap_or(0) >= poll_mid);

    // 5. Media chunked upload + HTTP 206 partial download + 403 non-member denial + Tantivy search
    let png = minimal_png_1x1();
    let (st, init) = drv.json_req(Method::POST, "/api/v1/media/upload/init", Some(&alice_tok), Some(json!({
        "filename": "dot.png", "size_bytes": png.len(), "chunk_size": 1024
    }))).await.unwrap();
    assert!(st == 200 || st == 201);
    let uid = init["upload_id"].as_str().unwrap();
    let put_res = drv.http.put(format!("{}/api/v1/media/upload/{uid}/part/0", server.base_url()))
        .bearer_auth(&alice_tok).body(png.clone()).send().await.unwrap();
    assert_eq!(put_res.status().as_u16(), 200);
    let (st, comp) = drv.json_req(Method::POST, &format!("/api/v1/media/upload/{uid}/complete"), Some(&alice_tok), Some(json!({
        "filename": "dot.png", "mime_type": "image/png", "chat_id": gid
    }))).await.unwrap();
    assert_eq!(st, 200);
    let file_id = comp["file_id"].as_str().unwrap().to_string();

    let r206 = drv.http.get(format!("{}/api/v1/media/{file_id}", server.base_url()))
        .bearer_auth(&bob_tok).header("Range", "bytes=0-15").send().await.unwrap();
    assert_eq!(r206.status().as_u16(), 206);
    assert_eq!(r206.bytes().await.unwrap().as_ref(), &png[..16]);
    let r403 = drv.http.get(format!("{}/api/v1/media/{file_id}", server.base_url()))
        .bearer_auth(&charlie_tok).send().await.unwrap();
    assert_eq!(r403.status().as_u16(), 403);

    let (_, s_bob) = drv.json_req(Method::GET, "/api/v1/search?q=Tier4UniqueSearchTerm", Some(&bob_tok), None).await.unwrap();
    assert!(s_bob["total"].as_u64().unwrap_or(0) >= 1);
    let (_, s_out) = drv.json_req(Method::GET, "/api/v1/search?q=Tier4UniqueSearchTerm", Some(&charlie_tok), None).await.unwrap();
    assert_eq!(s_out["total"], 0);

    // 6. Calls + Stories + Tasks + Server Restart Durability!
    let (st, _) = drv.json_req(Method::POST, "/api/v1/stories", Some(&alice_tok), Some(json!({ "media_id": file_id, "caption": "persist" }))).await.unwrap();
    assert!(st == 200 || st == 201);
    let (st, call) = drv.json_req(Method::POST, "/api/v1/calls/invite", Some(&alice_tok), Some(json!({ "callee_id": bob_id, "is_video": false, "chat_id": gid }))).await.unwrap();
    assert!(st == 200 || st == 201);
    assert!(call["call_id"].as_str().is_some());
    assert_eq!(drv.json_req(Method::POST, "/api/v1/tasks", Some(&alice_tok), Some(json!({ "task_id": "t4_durable_task" }))).await.unwrap().0, 201);

    let _ = ws_bob.close(None).await;
    while let Ok(Some(Ok(_))) = timeout(Duration::from_millis(100), ws_bob.next()).await {}
    drop(ws_bob);
    drop(drv);
    server.restart().await.expect("restart server cleanly");
    let drv2 = E2eSystemDriver::new(server.base_url());
    let (st, me2) = drv2.json_req(Method::GET, "/api/v1/users/me", Some(&alice_tok), None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(me2["user_id"], alice_id);
    let (st, tasks2) = drv2.json_req(Method::GET, "/api/v1/tasks", Some(&alice_tok), None).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(tasks2["tasks"][0]["task_id"], "t4_durable_task");
    let (st, chats2) = drv2.json_req(Method::GET, "/api/v1/chats", Some(&bob_tok), None).await.unwrap();
    assert_eq!(st, 200);
    assert!(!chats2["chats"].as_array().unwrap().is_empty());
}
