use gpui_telegram_client::{
    ClientConfig, ClientDomainEvent, LocalStore, TelegramClient,
};
use gpui_telegram_core::types::{ChatId, MessageId, PeerId, UserId};
use gpui_telegram_proto::message::{MessageContent, MessageItem, ReactionCount};
use gpui_telegram_proto::sync::{PtsUpdate, ReadHistoryEvent, ReactionEvent, UpdateEvent};
use gpui_telegram_proto::ws::WsServerFrame;
use gpui_telegram_proto::{DeleteTombstone, PollCard, PollOption};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> (String, String, Vec<u8>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut header_end = None;
    while header_end.is_none() {
        let n = stream.read(&mut tmp).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = Some(pos + 4);
        }
    }
    let hend = header_end.unwrap_or(buf.len());
    let headers_str = String::from_utf8_lossy(&buf[..hend]).to_string();
    let content_len = headers_str
        .lines()
        .find_map(|l| {
            let lower = l.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while buf.len() < hend + content_len {
        let n = stream.read(&mut tmp).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let first_line = headers_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    (method, path, buf[hend..].to_vec())
}

#[test]
fn test_chunked_put_upload_and_cas_media_cache_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = ClientConfig::default();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let put_parts = Arc::new(AtomicUsize::new(0));
    let downloads = Arc::new(AtomicUsize::new(0));
    let payload: Vec<u8> = (0..(300 * 1024)).map(|i| (i % 251) as u8).collect();
    let expected = payload.clone();

    let addr = rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let put_parts_srv = put_parts.clone();
        let downloads_srv = downloads.clone();
        let bytes_srv = payload.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { break };
                let (method, path, _) = read_http_request(&mut stream).await;
                let (status, ctype, body) = if method == "POST" && path == "/api/v1/media/upload/init" {
                    ("200 OK", "application/json", br#"{"upload_id":"up-1","chunk_size":1048576}"#.to_vec())
                } else if method == "PUT" && path.starts_with("/api/v1/media/upload/up-1/part/") {
                    put_parts_srv.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", "application/json", br#"{"ok":true}"#.to_vec())
                } else if method == "POST" && path == "/api/v1/media/upload/up-1/complete" {
                    ("200 OK", "application/json", br#"{"file_id":"cas-file-1","sha256":"abc","size_bytes":307200,"mime_type":"application/octet-stream"}"#.to_vec())
                } else if method == "GET" && path == "/api/v1/media/stream/remote-file-2" {
                    downloads_srv.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", "application/octet-stream", bytes_srv.clone())
                } else {
                    ("404 Not Found", "application/json", br#"{"error":"not_found"}"#.to_vec())
                };
                let head = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    status,
                    ctype,
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            }
        });
        addr
    });

    let mut cfg = config;
    cfg.server_url = format!("http://{}", addr);
    let client = TelegramClient::open_with_dir(cfg.clone(), dir.path()).expect("open client");
    client.set_token(Some("tok-123".to_string()));

    let uploaded = rt.block_on(client.upload_media("blob.bin", "application/octet-stream", expected.clone())).unwrap();
    assert_eq!(uploaded.file_id, "cas-file-1");
    assert_eq!(put_parts.load(Ordering::SeqCst), 1);
    assert!(client.local_media_path("cas-file-1").expect("cas path").exists());
    assert_eq!(rt.block_on(client.get_media_bytes("cas-file-1")).unwrap(), expected);
    assert_eq!(downloads.load(Ordering::SeqCst), 0);

    let first_fetch = rt.block_on(client.get_media_bytes("remote-file-2")).unwrap();
    assert_eq!(first_fetch, expected);
    assert_eq!(downloads.load(Ordering::SeqCst), 1);

    let reopened = TelegramClient::open_with_dir(cfg, dir.path()).expect("reopen client");
    reopened.set_token(Some("tok-123".to_string()));
    assert_eq!(rt.block_on(reopened.get_media_bytes("remote-file-2")).unwrap(), expected);
    assert_eq!(downloads.load(Ordering::SeqCst), 1);
}

#[test]
fn test_ws_pts_push_dispatch_and_sqlite_local_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = LocalStore::open_with_dir(dir.path()).expect("store");
    let client = TelegramClient::open_with_dir(ClientConfig::default(), dir.path()).expect("client");
    let chat_id = ChatId(77);

    let mut msg1 = MessageItem::new(MessageId(10), chat_id, UserId(2), PeerId::Chat(chat_id), MessageContent::plain_text("hello"))
        .with_client_nonce(9001);
    msg1.id = MessageId(10);
    let ev1 = client.process_ws_frame(WsServerFrame::PtsPush(PtsUpdate::new(chat_id, 1, 1, UpdateEvent::NewMessage(msg1.clone()), 1000)));
    assert!(matches!(ev1.as_slice(), [ClientDomainEvent::NewMessage(_)]));

    let mut edited = msg1.clone();
    edited.content = MessageContent::plain_text("hello edited");
    let _ = client.process_ws_frame(WsServerFrame::PtsPush(PtsUpdate::new(chat_id, 2, 1, UpdateEvent::EditMessage(edited), 1001)));

    let react_ev = ReactionEvent::new(chat_id, MessageId(10), UserId(3), vec![ReactionCount::new("🔥", 1, false)]);
    let _ = client.process_ws_frame(WsServerFrame::PtsPush(PtsUpdate::new(chat_id, 3, 1, UpdateEvent::ReactionUpdated(react_ev), 1002)));

    let gap_msg = MessageItem::new(MessageId(12), chat_id, UserId(2), PeerId::Chat(chat_id), MessageContent::plain_text("after gap"));
    let gap_evs = client.process_ws_frame(WsServerFrame::PtsPush(PtsUpdate::new(chat_id, 5, 1, UpdateEvent::NewMessage(gap_msg), 1005)));
    assert!(matches!(gap_evs.as_slice(), [ClientDomainEvent::SyncGapDetected { chat_id: ChatId(77), from_pts: 3 }]));
    assert_eq!(client.pending_gap_chats(), vec![ChatId(77)]);

    let miss_msg = MessageItem::new(MessageId(11), chat_id, UserId(2), PeerId::Chat(chat_id), MessageContent::plain_text("filled gap"));
    let poll = PollCard::new("501", "Vote?", vec![PollOption::new(0, "Yes"), PollOption::new(1, "No")], true);
    let _ = client.process_ws_frame(WsServerFrame::PtsPush(PtsUpdate::new(chat_id, 4, 1, UpdateEvent::NewMessage(miss_msg), 1004)));
    let _ = client.process_ws_frame(WsServerFrame::PtsPush(PtsUpdate::new(chat_id, 5, 0, UpdateEvent::PollUpdated { chat_id, message_id: MessageId(12), poll }, 1006)));
    let _ = client.process_ws_frame(WsServerFrame::PtsPush(PtsUpdate::new(chat_id, 5, 0, UpdateEvent::ReadHistory(ReadHistoryEvent::new(chat_id, MessageId(12), false, 0)), 1007)));

    let msgs = client.get_messages(chat_id).unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].reactions[0].emoji, "🔥");
    assert_eq!(client.local_store.read_inbox_max_id(chat_id), Some(MessageId(12)));

    let _ = client.process_ws_frame(WsServerFrame::PtsPush(PtsUpdate::new(
        chat_id,
        6,
        1,
        UpdateEvent::DeleteMessages(DeleteTombstone::new(PeerId::Chat(chat_id), vec![MessageId(11)], true)),
        1008,
    )));
    assert_eq!(client.get_messages(chat_id).unwrap().len(), 2);
    drop(store);
    let reloaded = LocalStore::open_with_dir(dir.path()).unwrap();
    assert_eq!(reloaded.get_messages(chat_id).unwrap().len(), 2);
}

#[test]
fn test_react_to_message_primary_and_legacy_fallback_updates_local_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let legacy_hits = Arc::new(AtomicUsize::new(0));

    let addr = rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let p_hits = primary_hits.clone();
        let l_hits = legacy_hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { break };
                let (method, path, body_bytes) = read_http_request(&mut stream).await;
                let req_json: serde_json::Value =
                    serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null);

                let (status, body) = if method == "POST"
                    && path == "/api/v1/chats/88/messages/501/reactions"
                {
                    p_hits.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(req_json["emoji"], "❤️");
                    assert_eq!(req_json["action"], "add");
                    (
                        "200 OK",
                        r#"{"success":true,"chat_id":88,"message_id":501,"pts":12,"reactions":[{"emoji":"❤️","count":1,"is_chosen_by_me":true}],"details":[]}"#
                            .as_bytes()
                            .to_vec(),
                    )
                } else if method == "POST"
                    && path == "/api/v1/chats/99/messages/602/reactions"
                {
                    p_hits.fetch_add(1, Ordering::SeqCst);
                    ("404 Not Found", br#"{"error":"route_not_mounted"}"#.to_vec())
                } else if method == "POST" && path == "/api/v1/messages/reaction" {
                    l_hits.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(req_json["chat_id"], 99);
                    assert_eq!(req_json["message_id"], 602);
                    assert_eq!(req_json["emoji"], "🔥");
                    assert_eq!(req_json["action"], "add");
                    (
                        "200 OK",
                        r#"{"success":true,"chat_id":99,"message_id":602,"pts":21,"reactions":[{"emoji":"🔥","count":2,"is_chosen_by_me":true}],"details":[]}"#
                            .as_bytes()
                            .to_vec(),
                    )
                } else {
                    ("404 Not Found", br#"{"error":"not_found"}"#.to_vec())
                };
                let head = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    status,
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            }
        });
        addr
    });

    let mut cfg = ClientConfig::default();
    cfg.server_url = format!("http://{}", addr);
    let client = TelegramClient::open_with_dir(cfg, dir.path()).expect("client");
    client.set_token(Some("tok-rx".to_string()));

    // Insert local messages so `LocalStore::update_reactions` persists into SQLite
    client.local_store.insert_message(MessageItem::new(
        MessageId(501),
        ChatId(88),
        UserId(2),
        PeerId::Chat(ChatId(88)),
        MessageContent::plain_text("primary msg"),
    ));
    client.local_store.insert_message(MessageItem::new(
        MessageId(602),
        ChatId(99),
        UserId(2),
        PeerId::Chat(ChatId(99)),
        MessageContent::plain_text("legacy fallback msg"),
    ));

    // 1. Primary REST route
    let r1 = rt
        .block_on(client.react_to_message(88, 501, "❤️", Some("add")))
        .expect("primary reaction");
    assert_eq!(r1.reactions.len(), 1);
    assert_eq!(r1.reactions[0].emoji, "❤️");
    assert_eq!(r1.reactions[0].count, 1);
    assert!(r1.reactions[0].is_chosen_by_me);
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(legacy_hits.load(Ordering::SeqCst), 0);

    let stored_88 = client.get_messages(ChatId(88)).unwrap();
    assert_eq!(stored_88[0].reactions.len(), 1);
    assert_eq!(stored_88[0].reactions[0].emoji, "❤️");
    assert!(stored_88[0].reactions[0].is_chosen_by_me);

    // 2. Fallback to legacy `/api/v1/messages/reaction` on 404
    let r2 = rt
        .block_on(client.react_to_message(99, 602, "🔥", Some("add")))
        .expect("legacy fallback reaction");
    assert_eq!(r2.reactions.len(), 1);
    assert_eq!(r2.reactions[0].emoji, "🔥");
    assert_eq!(r2.reactions[0].count, 2);
    assert!(r2.reactions[0].is_chosen_by_me);
    assert_eq!(primary_hits.load(Ordering::SeqCst), 2);
    assert_eq!(legacy_hits.load(Ordering::SeqCst), 1);

    let stored_99 = client.get_messages(ChatId(99)).unwrap();
    assert_eq!(stored_99[0].reactions.len(), 1);
    assert_eq!(stored_99[0].reactions[0].emoji, "🔥");
    assert_eq!(stored_99[0].reactions[0].count, 2);
}

#[test]
fn test_ws_reaction_update_from_other_peer_preserves_viewer_chosen_in_sqlite_local_store() {
    use gpui_telegram_client::net::AuthResponse;
    use gpui_telegram_client::LocalStore;
    use gpui_telegram_proto::message::ReactionCount;
    use gpui_telegram_proto::sync::{PtsUpdate, ReactionEvent, UpdateEvent};
    use gpui_telegram_proto::ws::WsServerFrame;

    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = ClientConfig::default();
    let client = TelegramClient::open_with_dir(cfg, dir.path()).expect("client");
    client.set_auth_session(AuthResponse {
        token: "tok-viewer-9001".to_string(),
        session_id: "sess-9001".to_string(),
        user_id: 9001,
        username: "viewer9001".to_string(),
        first_name: Some("Viewer".to_string()),
        created_at: None,
        expires_at: None,
    });

    let mut msg = MessageItem::new(
        MessageId(501),
        ChatId(88),
        UserId(9001),
        PeerId::Chat(ChatId(88)),
        MessageContent::plain_text("viewer reaction isolation"),
    );
    msg.reactions = vec![ReactionCount::new("❤️", 1, true)];
    client.local_store.insert_message(msg);

    // Peer 9999 adds ❤️ (count 2) and 🔥 (count 1); broadcast carries peer 9999's `is_chosen_by_me: true`
    let peer_push = WsServerFrame::PtsPush(PtsUpdate::new(
        ChatId(88),
        1,
        1,
        UpdateEvent::ReactionUpdated(ReactionEvent::new(
            ChatId(88),
            MessageId(501),
            UserId(9999),
            vec![
                ReactionCount::new("❤️", 2, true),
                ReactionCount::new("🔥", 1, true),
            ],
        )),
        1_700_000_000_000,
    ));
    let _ = client.process_ws_frame(peer_push);

    let mem_msgs = client.get_messages(ChatId(88)).unwrap();
    let heart = mem_msgs[0].reactions.iter().find(|r| r.emoji == "❤️").unwrap();
    let fire = mem_msgs[0].reactions.iter().find(|r| r.emoji == "🔥").unwrap();
    assert_eq!(heart.count, 2);
    assert!(heart.is_chosen_by_me, "Viewer 9001's own ❤️ choice must remain true");
    assert_eq!(fire.count, 1);
    assert!(
        !fire.is_chosen_by_me,
        "Peer 9999's 🔥 must not mark viewer 9001 as is_chosen_by_me in LocalStore"
    );

    // Reopen SQLite from disk to prove durable SQLite isolation as well
    let reopened = LocalStore::open_with_dir(dir.path()).expect("reopen sqlite");
    let disk_msgs = reopened.get_messages(ChatId(88)).unwrap();
    let disk_heart = disk_msgs[0].reactions.iter().find(|r| r.emoji == "❤️").unwrap();
    let disk_fire = disk_msgs[0].reactions.iter().find(|r| r.emoji == "🔥").unwrap();
    assert_eq!(disk_heart.count, 2);
    assert!(disk_heart.is_chosen_by_me);
    assert_eq!(disk_fire.count, 1);
    assert!(!disk_fire.is_chosen_by_me);
}

#[test]
fn test_legacy_and_modern_reaction_wire_deserialization_and_chosen_aliases() {
    use gpui_telegram_client::HistoricalMessage;
    use gpui_telegram_proto::contracts::messages::ReactionHttpResponse;
    use gpui_telegram_proto::message::ReactionCount;

    // 1. Live default server (`http://34.46.77.133:8080`) legacy ack omitting chat_id, message_id, and reactions
    let legacy_ack: ReactionHttpResponse = serde_json::from_str(
        r#"{"success": true, "pts": 42, "status": "reaction_updated"}"#,
    )
    .expect("legacy reaction_updated response must deserialize");
    assert_eq!(legacy_ack.pts, 42);
    assert_eq!(legacy_ack.status.as_deref(), Some("reaction_updated"));
    assert!(legacy_ack.reactions.is_empty());
    assert!(
        !legacy_ack.has_authoritative_reactions(),
        "Omitted reactions field on legacy ack must not be marked authoritative"
    );

    // 2. Modern server removing the last reaction explicitly returns `"reactions": []` (authoritative empty)
    let modern_empty: ReactionHttpResponse = serde_json::from_str(
        r#"{"chat_id": 23, "message_id": 101, "pts": 43, "reactions": []}"#,
    )
    .expect("modern empty reactions response must deserialize");
    assert_eq!(modern_empty.chat_id, 23);
    assert_eq!(modern_empty.message_id, 101);
    assert_eq!(modern_empty.pts, 43);
    assert!(modern_empty.reactions.is_empty());
    assert!(
        modern_empty.has_authoritative_reactions(),
        "Explicit empty reactions array must remain authoritative"
    );

    // 3. ReactionCount serde aliases (`chosen`, `user_reacted`, `is_chosen_by_me`)
    let r_chosen: ReactionCount =
        serde_json::from_str(r#"{"emoji": "👍", "count": 3, "chosen": true}"#).unwrap();
    let r_user_reacted: ReactionCount =
        serde_json::from_str(r#"{"emoji": "🔥", "count": 2, "user_reacted": true}"#).unwrap();
    let r_canonical: ReactionCount =
        serde_json::from_str(r#"{"emoji": "❤️", "count": 1, "is_chosen_by_me": true}"#).unwrap();
    assert!(r_chosen.is_chosen_by_me);
    assert!(r_user_reacted.is_chosen_by_me);
    assert!(r_canonical.is_chosen_by_me);

    // 4. HistoricalMessage distinguishes omitted `"reactions"` (legacy) vs explicit `"reactions": []` (modern)
    let hm_legacy: HistoricalMessage = serde_json::from_str(
        r#"{"message_id": 101, "sender_id": 2, "text": "Legacy history item"}"#,
    )
    .unwrap();
    assert!(!hm_legacy.has_authoritative_reactions());

    let hm_modern_empty: HistoricalMessage = serde_json::from_str(
        r#"{"message_id": 101, "sender_id": 2, "text": "Modern history item", "reactions": []}"#,
    )
    .unwrap();
    assert!(hm_modern_empty.has_authoritative_reactions());
}

#[tokio::test]
async fn test_legacy_server_reaction_fallback_persists_in_sqlite_without_insert_message_and_survives_reload_and_restart() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let modern_authoritative_empty = Arc::new(AtomicBool::new(false));
    let modern_flag = modern_authoritative_empty.clone();

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock server");
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let modern_empty = modern_flag.load(Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let mut total = 0usize;
                loop {
                    let n = match stream.read(&mut buf[total..]).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    total += n;
                    if let Some(hdr_end) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
                        let body_start = hdr_end + 4;
                        let hdr_str = String::from_utf8_lossy(&buf[..hdr_end]);
                        let content_len = hdr_str
                            .lines()
                            .find_map(|line| {
                                let (k, v) = line.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if total >= body_start + content_len {
                            break;
                        }
                    }
                }
                let req_str = String::from_utf8_lossy(&buf[..total]);
                let first_line = req_str.lines().next().unwrap_or("");
                let (status, body) = if first_line.starts_with("POST /api/v1/chats/")
                    && first_line.contains("/reactions ")
                {
                    // Live server returns 404 on modern reactions endpoint
                    ("404 Not Found", r#"{"error":"not found"}"#.to_string())
                } else if first_line.starts_with("POST /api/v1/messages/reaction ") {
                    // Legacy fallback omits chat_id, message_id, and reactions
                    (
                        "200 OK",
                        r#"{"success":true,"pts":7,"status":"reaction_updated"}"#.to_string(),
                    )
                } else if first_line.starts_with("GET /api/v1/chats/")
                    && first_line.contains("/messages")
                {
                    if modern_empty {
                        (
                            "200 OK",
                            r#"{"messages":[{"message_id":101,"sender_id":2,"text":"Multi-line AI reply\nLine 2\nLine 3","date":1700000000000,"is_outgoing":false,"reactions":[]}]}"#.to_string(),
                        )
                    } else {
                        ("404 Not Found", r#"{"error":"not found"}"#.to_string())
                    }
                } else if first_line.starts_with("GET /api/v1/messages/history") {
                    // Legacy history endpoint omits `"reactions"` completely
                    (
                        "200 OK",
                        r#"{"messages":[{"message_id":101,"sender_id":2,"text":"Multi-line AI reply\nLine 2\nLine 3","date":1700000000000,"is_outgoing":false}]}"#.to_string(),
                    )
                } else {
                    ("404 Not Found", r#"{"error":"unhandled"}"#.to_string())
                };
                let http_resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(http_resp.as_bytes()).await;
            });
        }
    });

    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = ClientConfig {
        server_url: format!("http://{addr}"),
        ..Default::default()
    };

    // 1. Open durable SQLite client and fetch legacy history (no `insert_message` called!)
    let client1 = TelegramClient::open_with_dir(cfg.clone(), dir.path()).expect("open client1");
    client1.set_token(Some("legacy-tok".to_string()));

    let initial_hist = client1
        .get_chat_messages(23, Some(50))
        .await
        .expect("fetch initial legacy history");
    assert_eq!(initial_hist.len(), 1);
    assert!(initial_hist[0].reactions.is_empty());

    // 2. React to message 101 (`404` -> `/api/v1/messages/reaction` legacy ack)
    let ack = client1
        .react_to_message(23, 101, "❤️", Some("add"))
        .await
        .expect("legacy reaction fallback must succeed");
    assert_eq!(ack.pts, 7);
    assert!(!ack.has_authoritative_reactions());

    let cached_rx = client1
        .local_store
        .get_reactions(ChatId(23), MessageId(101))
        .expect("reactions cached");
    assert_eq!(cached_rx.len(), 1);
    assert_eq!(cached_rx[0].emoji, "❤️");
    assert_eq!(cached_rx[0].count, 1);
    assert!(cached_rx[0].is_chosen_by_me);

    // 3. Switching chats and reloading legacy history (which omits `"reactions"`) hydrates ❤️!
    let reloaded_hist = client1
        .get_chat_messages(23, Some(50))
        .await
        .expect("reload legacy history");
    assert_eq!(reloaded_hist[0].reactions.len(), 1);
    assert_eq!(reloaded_hist[0].reactions[0].emoji, "❤️");
    assert_eq!(reloaded_hist[0].reactions[0].count, 1);
    assert!(reloaded_hist[0].reactions[0].chosen);

    tokio::task::spawn_blocking(move || drop(client1))
        .await
        .unwrap();

    // 4. Reopen a brand-new `TelegramClient::open_with_dir` on the same directory (app restart!)
    let client2 = TelegramClient::open_with_dir(cfg.clone(), dir.path()).expect("reopen client2");
    client2.set_token(Some("legacy-tok".to_string()));
    let restarted_hist = client2
        .get_chat_messages(23, Some(50))
        .await
        .expect("history after restart");
    assert_eq!(restarted_hist[0].reactions.len(), 1);
    assert_eq!(restarted_hist[0].reactions[0].emoji, "❤️");
    assert!(restarted_hist[0].reactions[0].chosen);

    // 5. Add a second reaction 🔥, then remove ❤️, and verify removal persists across restart too
    client2
        .react_to_message(23, 101, "🔥", Some("add"))
        .await
        .expect("add fire");
    client2
        .react_to_message(23, 101, "❤️", Some("remove"))
        .await
        .expect("remove heart");

    tokio::task::spawn_blocking(move || drop(client2))
        .await
        .unwrap();

    let client3 = TelegramClient::open_with_dir(cfg, dir.path()).expect("reopen client3");
    client3.set_token(Some("legacy-tok".to_string()));
    let after_rm_hist = client3
        .get_chat_messages(23, Some(50))
        .await
        .expect("history after removing heart");
    assert_eq!(after_rm_hist[0].reactions.len(), 1);
    assert_eq!(after_rm_hist[0].reactions[0].emoji, "🔥");
    assert!(after_rm_hist[0].reactions[0].chosen);

    // 6. When a modern endpoint explicitly returns `"reactions": []`, it authoritatively clears cached reactions
    modern_authoritative_empty.store(true, Ordering::SeqCst);
    let modern_cleared = client3
        .get_chat_messages(23, Some(50))
        .await
        .expect("modern empty reactions history");
    assert!(modern_cleared[0].reactions.is_empty());
    assert!(client3
        .local_store
        .get_reactions(ChatId(23), MessageId(101))
        .unwrap_or_default()
        .is_empty());

    server_task.abort();
    tokio::task::spawn_blocking(move || drop(client3))
        .await
        .unwrap();
}

#[test]
fn test_sqlite_cache_account_scope_isolation_and_ws_chat_zero_lookup() {
    use gpui_telegram_client::net::AuthResponse;
    use gpui_telegram_proto::sync::{ReactionEvent, UpdateEvent};
    use gpui_telegram_proto::ws::WsServerFrame;

    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = ClientConfig {
        server_url: "http://127.0.0.1:19999".to_string(),
        ..Default::default()
    };

    // 1. User A (user_id = 1001) authenticates and stores `is_chosen_by_me = true` reaction
    let client_a = TelegramClient::open_with_dir(cfg.clone(), dir.path()).expect("open client_a");
    client_a.set_auth_session(AuthResponse {
        user_id: 1001,
        username: "alice".to_string(),
        session_id: "sess_a".to_string(),
        token: "tok_a".to_string(),
        first_name: Some("Alice".to_string()),
        created_at: None,
        expires_at: None,
    });
    client_a.local_store.update_reactions(
        ChatId(77),
        MessageId(5001),
        vec![ReactionCount::new("👍", 1, true)],
    );

    // Un-PTS'd WS `Update` with `chat_id == 0` resolves `ChatId(77)` via `find_chat_id_for_message`
    let evs = client_a.process_ws_frame(WsServerFrame::Update(UpdateEvent::ReactionUpdated(
        ReactionEvent::new(
            ChatId(0),
            MessageId(5001),
            UserId(2002),
            vec![
                ReactionCount::new("👍", 1, false),
                ReactionCount::new("🔥", 1, true),
            ],
        ),
    )));
    assert_eq!(evs.len(), 1);
    let stored_a = client_a
        .local_store
        .get_reactions(ChatId(77), MessageId(5001))
        .expect("reactions resolved for chat 77");
    assert!(stored_a.iter().any(|r| r.emoji == "👍" && r.is_chosen_by_me));
    assert!(stored_a.iter().any(|r| r.emoji == "🔥" && !r.is_chosen_by_me));
    drop(client_a);

    // 2. Reopening as the SAME User A (user_id = 1001) preserves durable reactions
    let client_a2 = TelegramClient::open_with_dir(cfg.clone(), dir.path()).expect("reopen client_a2");
    client_a2.set_auth_session(AuthResponse {
        user_id: 1001,
        username: "alice".to_string(),
        session_id: "sess_a2".to_string(),
        token: "tok_a2".to_string(),
        first_name: Some("Alice".to_string()),
        created_at: None,
        expires_at: None,
    });
    assert!(client_a2
        .local_store
        .get_reactions(ChatId(77), MessageId(5001))
        .is_some());
    drop(client_a2);

    // 3. Switching to User B (user_id = 2002) WITHOUT explicit logout() automatically clears User A's cache!
    let client_b = TelegramClient::open_with_dir(cfg, dir.path()).expect("open client_b");
    client_b.set_auth_session(AuthResponse {
        user_id: 2002,
        username: "bob".to_string(),
        session_id: "sess_b".to_string(),
        token: "tok_b".to_string(),
        first_name: Some("Bob".to_string()),
        created_at: None,
        expires_at: None,
    });
    assert!(
        client_b
            .local_store
            .get_reactions(ChatId(77), MessageId(5001))
            .is_none(),
        "Account switch without explicit logout must isolate SQLite reactions per user_id"
    );
}
