use futures::{SinkExt, StreamExt};
use gpui_telegram_client::e2ee::keys::{
    b64_encode, random_32, x25519_pub_from_secret, DeviceIdentityKeys,
};
use gpui_telegram_client::e2ee::x3dh::{x3dh_initiator, x3dh_responder};
use gpui_telegram_client::media_transfer::{
    index_file_chunks, read_chunk_at, FileTransferRecord, TransferStatus,
};
use gpui_telegram_client::net::ws_channel::WsChannel;
use gpui_telegram_client::net::ws_transport::WsTransport;
use gpui_telegram_client::{
    compute_base_backoff_ms, compute_jittered_backoff_ms, ClientConfig, ClientProxyConfig,
    DoubleRatchetSession, HttpClient, LocalStore, NetworkState, OutboxStatus,
    RichSendMessageRequest, SenderKeyState, TelegramClient,
};
use gpui_telegram_core::types::{ChatId, MessageId, PeerId, UserId};
use gpui_telegram_proto::contracts::chats::ChatFolderDto;
use gpui_telegram_proto::e2ee::{
    DevicePreKeyBundle, EncryptedEnvelope, EncryptedPayloadKind, OneTimePreKeyDto,
    PublishPreKeysRequest, SignedPreKeyDto, UserPreKeyBundlesResponse,
};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use parking_lot::Mutex;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message as WsMessage;

fn sha256_hex_local(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let mut out = String::with_capacity(64);
    for b in h.finalize() {
        let _ = write!(&mut out, "{:02x}", b);
    }
    out
}

async fn read_http_request(stream: &mut TcpStream) -> (String, String, String, Vec<u8>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut header_end = None;
    while header_end.is_none() {
        let n = stream.read(&mut tmp).await.unwrap_or(0);
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
        let n = stream.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let first_line = headers_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let body = if buf.len() >= hend {
        buf[hend..].to_vec()
    } else {
        Vec::new()
    };
    (method, path, headers_str, body)
}

async fn write_json_response(stream: &mut TcpStream, status: u16, reason: &str, json_body: &str) {
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_body}",
        json_body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

fn make_rich_req(chat_id: i64, text: &str, nonce: Option<u64>) -> RichSendMessageRequest {
    RichSendMessageRequest {
        chat_id,
        text: Some(text.to_string()),
        content: Some(MessageContent::plain_text(text)),
        entities: Vec::new(),
        reply_to: None,
        quote_text: None,
        topic_id: None,
        client_nonce: nonce,
        forward_from_chat_id: None,
        forward_from_msg_id: None,
        silent: false,
        scheduled_at_ms: None,
        inline_keyboard: None,
        send_as_peer_id: None,
    }
}

fn make_test_auth(user_id: u64, username: &str, token: &str) -> gpui_telegram_client::AuthResponse {
    gpui_telegram_client::AuthResponse {
        user_id,
        username: username.to_string(),
        first_name: Some(username.to_string()),
        token: token.to_string(),
        session_id: format!("sess_{user_id}_{username}"),
        created_at: None,
        expires_at: None,
    }
}

#[test]
fn test_f6_outbox_persistence_crash_recovery_backoff_and_reconnect_flush() {
    // 1. Verify exponential backoff & jitter bounds
    assert_eq!(compute_base_backoff_ms(0), 500);
    assert_eq!(compute_base_backoff_ms(1), 1000);
    assert_eq!(compute_base_backoff_ms(6), 30_000);
    assert_eq!(compute_base_backoff_ms(12), 30_000);
    for r in 0..8 {
        let j = compute_jittered_backoff_ms(r);
        assert!((250..=30_000).contains(&j), "jitter out of bounds: {j}");
    }

    // 2. Verify valid/invalid status transitions
    assert!(OutboxStatus::Pending.can_transition_to(OutboxStatus::InFlight));
    assert!(OutboxStatus::InFlight.can_transition_to(OutboxStatus::Delivered));
    assert!(OutboxStatus::InFlight.can_transition_to(OutboxStatus::Pending));
    assert!(OutboxStatus::InFlight.can_transition_to(OutboxStatus::Failed));
    assert!(!OutboxStatus::Delivered.can_transition_to(OutboxStatus::Pending));

    let dir = tempfile::tempdir().expect("tempdir");
    let rt = tokio::runtime::Runtime::new().expect("rt");

    // 3. Spin up mock server that rejects first attempt with 503 then accepts with 200 OK
    let fail_mode = Arc::new(AtomicBool::new(true));
    let delivered_nonces = Arc::new(Mutex::new(Vec::<u64>::new()));
    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();

    let fail_srv = Arc::clone(&fail_mode);
    let nonces_srv = Arc::clone(&delivered_nonces);
    rt.spawn(async move {
        let mut next_msg_id = 5000i64;
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let fail = Arc::clone(&fail_srv);
            let nonces = Arc::clone(&nonces_srv);
            tokio::spawn(async move {
                let (method, path, _, body) = read_http_request(&mut stream).await;
                if method == "POST"
                    && (path == "/api/v1/messages/send" || path == "/api/v1/chats/42/messages")
                {
                    let parsed: serde_json::Value =
                        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                    let nonce = parsed["client_nonce"].as_u64().unwrap_or(0);
                    let text_val = parsed["text"].as_str().unwrap_or("").to_string();
                    if text_val == "bad_permanent" {
                        write_json_response(
                            &mut stream,
                            400,
                            "Bad Request",
                            r#"{"error":"invalid message"}"#,
                        )
                        .await;
                        return;
                    }
                    if fail.load(Ordering::SeqCst) {
                        write_json_response(
                            &mut stream,
                            503,
                            "Service Unavailable",
                            r#"{"error":"transient outage"}"#,
                        )
                        .await;
                        return;
                    }
                    next_msg_id += 1;
                    nonces.lock().push(nonce);
                    let resp = json!({
                        "message_id": next_msg_id,
                        "id": next_msg_id,
                        "pts": 11,
                        "pts_count": 1,
                        "duplicate": false,
                        "date_ms": 1700000000000i64,
                        "date": "2026-11-17T00:00:00Z",
                        "sender_id": 100,
                        "chat_id": 42,
                        "text": text_val,
                        "client_nonce": nonce,
                        "status": "sent"
                    });
                    write_json_response(&mut stream, 200, "OK", &resp.to_string()).await;
                } else {
                    write_json_response(&mut stream, 200, "OK", "{}").await;
                }
            });
        }
    });

    let server_url = format!("http://{addr}");
    let cfg = ClientConfig {
        server_url: server_url.clone(),
        ..Default::default()
    };

    // 4. First session: create in-flight + pending records and verify persistence & nonce seeding
    let now_ms = chrono::Utc::now().timestamp_millis();
    let high_nonce = (now_ms as u64) + 50_000;
    {
        let client1 = TelegramClient::open_with_dir(cfg.clone(), dir.path()).unwrap();
        client1.set_auth_session(make_test_auth(100, "alice", "tok_a"));

        // Enqueue a high-nonce message directly into InFlight (simulating crash mid-send)
        let req1 = make_rich_req(42, "crashed mid flight", Some(high_nonce));
        let rec1 = client1
            .outbox
            .store()
            .enqueue_in_flight(&req1, high_nonce, now_ms)
            .unwrap();
        assert_eq!(rec1.status, OutboxStatus::InFlight);

        // Permanent 400 failure -> Failed immediately
        let perm_req = make_rich_req(42, "bad_permanent", Some(high_nonce + 1));
        let perm_res = rt.block_on(client1.send_message_durable(perm_req));
        assert!(perm_res.is_err());
        let perm_rec = client1
            .outbox
            .store()
            .get(high_nonce + 1)
            .unwrap()
            .expect("perm rec");
        assert_eq!(perm_rec.status, OutboxStatus::Failed);

        // Transient 503 failure while online -> stays Pending with retry_count=1
        let trans_req = make_rich_req(42, "retry me later", Some(high_nonce + 2));
        let trans_res = rt.block_on(client1.send_message_durable(trans_req));
        assert!(trans_res.is_err());
        let trans_rec = client1
            .outbox
            .store()
            .get(high_nonce + 2)
            .unwrap()
            .expect("trans rec");
        assert_eq!(trans_rec.status, OutboxStatus::Pending);
        assert_eq!(trans_rec.retry_count, 1);
    }

    // 5. Reopen client on same directory -> crash recovery resets InFlight -> Pending & seeds nonce
    let client2 = TelegramClient::open_with_dir(cfg, dir.path()).unwrap();
    let recovered = client2
        .outbox
        .store()
        .recover_in_flight(chrono::Utc::now().timestamp_millis())
        .unwrap();
    assert!(recovered >= 1, "crashed InFlight record must recover to Pending");
    let rec1_after = client2.outbox.store().get(high_nonce).unwrap().unwrap();
    assert_eq!(rec1_after.status, OutboxStatus::Pending);
    assert!(
        client2.next_client_nonce() > high_nonce + 2,
        "next_nonce must be seeded above persisted max outbox nonce"
    );
    client2.set_auth_session(make_test_auth(100, "alice", "tok_a"));

    // 6. Server recovers & NetworkStatusListener notifies Online -> immediate flush!
    fail_mode.store(false, Ordering::SeqCst);
    client2.network_listener.set_online(false);
    assert_eq!(client2.network_listener.state(), NetworkState::Offline);
    client2.network_listener.set_online(true);
    let _ = rt.block_on(client2.outbox.notify_reconnected_and_flush());
    let _ = rt.block_on(client2.outbox.flush_due());

    let r1 = client2.outbox.store().get(high_nonce).unwrap().unwrap();
    let r2 = client2.outbox.store().get(high_nonce + 2).unwrap().unwrap();
    assert_eq!(r1.status, OutboxStatus::Delivered);
    assert!(r1.server_message_id.is_some());
    assert_eq!(r2.status, OutboxStatus::Delivered);
    assert!(r2.server_message_id.is_some());

    // Calling flush_due again does NOT duplicate already-Delivered messages
    let count_before = delivered_nonces.lock().len();
    let _ = rt.block_on(client2.outbox.flush_due());
    assert_eq!(delivered_nonces.lock().len(), count_before);

    // Clear on account scope change wipes outbox
    client2.local_store.clear_all();
    assert!(client2.outbox.store().get(high_nonce).unwrap().is_none());
}

#[test]
fn test_f7_chunked_transfer_seek_sha256_resume_and_404_reinit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rt = tokio::runtime::Runtime::new().expect("rt");

    // Create 150 KB file on disk (with part_size 32 KB -> 5 parts)
    let file_path = dir.path().join("large_archive.bin");
    let raw_bytes: Vec<u8> = (0..(150 * 1024)).map(|i| ((i * 31 + 7) % 256) as u8).collect();
    std::fs::write(&file_path, &raw_bytes).unwrap();

    // Verify disk seek & per-chunk SHA-256 indexing without loading entire file
    let (total_size, part_size, total_parts, full_sha256, chunk_hashes) =
        index_file_chunks(&file_path, Some(32 * 1024)).unwrap();
    assert_eq!(total_size, raw_bytes.len() as u64);
    assert_eq!(part_size, 32 * 1024);
    assert_eq!(total_parts, 5);
    assert_eq!(full_sha256, sha256_hex_local(&raw_bytes));
    let p2_bytes = read_chunk_at(&file_path, 2, part_size, total_size).unwrap();
    assert_eq!(p2_bytes, &raw_bytes[(64 * 1024)..(96 * 1024)]);
    assert_eq!(sha256_hex_local(&p2_bytes), chunk_hashes[2]);

    // Mock server tracks uploaded parts per upload_id; simulates interruption on first run
    struct UploadSrvState {
        init_count: usize,
        uploaded_parts: HashMap<String, HashSet<u32>>,
        part_upload_calls: Vec<(String, u32)>,
        fail_part2_remaining: usize,
    }
    let srv_state = Arc::new(Mutex::new(UploadSrvState {
        init_count: 0,
        uploaded_parts: HashMap::new(),
        part_upload_calls: Vec::new(),
        fail_part2_remaining: 3,
    }));

    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let srv_ref = Arc::clone(&srv_state);
    let total_size_srv = total_size;
    let full_sha_srv = full_sha256.clone();

    rt.spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let st = Arc::clone(&srv_ref);
            let fsha = full_sha_srv.clone();
            tokio::spawn(async move {
                let (method, path, _, body) = read_http_request(&mut stream).await;
                if method == "POST" && path == "/api/v1/media/upload/init" {
                    let resp_str = {
                        let mut guard = st.lock();
                        guard.init_count += 1;
                        let uid = format!("up_{}", guard.init_count);
                        guard.uploaded_parts.insert(uid.clone(), HashSet::new());
                        json!({
                            "upload_id": uid,
                            "chunk_size": 32 * 1024,
                            "expires_at_ms": 1800000000000i64
                        })
                        .to_string()
                    };
                    write_json_response(&mut stream, 200, "OK", &resp_str).await;
                } else if method == "GET"
                    && path.starts_with("/api/v1/media/upload/")
                    && path.ends_with("/status")
                {
                    let uid = path
                        .trim_start_matches("/api/v1/media/upload/")
                        .trim_end_matches("/status")
                        .to_string();
                    let status_resp = {
                        let guard = st.lock();
                        guard.uploaded_parts.get(&uid).map(|parts| {
                            let uploaded: Vec<bool> =
                                (0..5u32).map(|p| parts.contains(&p)).collect();
                            let missing: Vec<u32> = (0..5u32).filter(|p| !parts.contains(p)).collect();
                            json!({
                                "upload_id": uid,
                                "total_size": total_size_srv,
                                "part_size": 32 * 1024,
                                "total_parts": 5,
                                "uploaded_parts": uploaded,
                                "missing_parts": missing,
                                "is_complete": missing.is_empty()
                            })
                            .to_string()
                        })
                    };
                    if let Some(resp_str) = status_resp {
                        write_json_response(&mut stream, 200, "OK", &resp_str).await;
                    } else {
                        write_json_response(&mut stream, 404, "Not Found", r#"{"error":"expired"}"#)
                            .await;
                    }
                } else if method == "PUT" && path.starts_with("/api/v1/media/upload/") {
                    let rest = path.trim_start_matches("/api/v1/media/upload/");
                    let segs: Vec<&str> = rest.split('/').collect();
                    let uid = segs[0].to_string();
                    let part_idx: u32 = segs.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let should_fail = {
                        let mut guard = st.lock();
                        guard.part_upload_calls.push((uid.clone(), part_idx));
                        if part_idx == 2 && guard.fail_part2_remaining > 0 {
                            guard.fail_part2_remaining -= 1;
                            true
                        } else {
                            guard.uploaded_parts.entry(uid).or_default().insert(part_idx);
                            false
                        }
                    };
                    if should_fail {
                        write_json_response(
                            &mut stream,
                            500,
                            "Internal Error",
                            r#"{"error":"dropped"}"#,
                        )
                        .await;
                        return;
                    }
                    let _ = body;
                    write_json_response(&mut stream, 200, "OK", r#"{"ok":true}"#).await;
                } else if method == "POST" && path.ends_with("/complete") {
                    let uid = path
                        .trim_start_matches("/api/v1/media/upload/")
                        .trim_end_matches("/complete");
                    let resp = json!({
                        "file_id": format!("file_{uid}"),
                        "sha256": fsha,
                        "size_bytes": 150 * 1024,
                        "mime_type": "application/octet-stream"
                    });
                    write_json_response(&mut stream, 200, "OK", &resp.to_string()).await;
                } else {
                    write_json_response(&mut stream, 404, "Not Found", "{}").await;
                }
            });
        }
    });

    let cfg = ClientConfig {
        server_url: format!("http://{addr}"),
        ..Default::default()
    };
    let client = TelegramClient::open_with_dir(cfg.clone(), dir.path()).unwrap();
    client.set_auth_session(make_test_auth(101, "bob", "tok_b"));

    // First attempt fails on part 2 -> parts 0 and 1 are persisted in `file_transfers`
    let first = rt.block_on(client.upload_file_resumable(
        &file_path,
        "application/octet-stream",
        Some(10),
        Some("xfer_1"),
    ));
    assert!(first.is_err(), "first run should fail on injected part 2 error");

    let rec_mid = client.chunked_transfers.store().get("xfer_1").unwrap().unwrap();
    assert_eq!(rec_mid.status, TransferStatus::Paused);
    assert_eq!(rec_mid.uploaded_parts, vec![true, true, false, false, false]);
    assert_eq!(rec_mid.chunk_hashes, chunk_hashes);

    // Reopen client & resume same transfer -> queries `/status`, skips parts 0 & 1, uploads only 2,3,4
    let calls_before_resume = srv_state.lock().part_upload_calls.len();
    let client2 = TelegramClient::open_with_dir(cfg.clone(), dir.path()).unwrap();
    client2.set_auth_session(make_test_auth(101, "bob", "tok_b"));
    let second = rt
        .block_on(client2.upload_file_resumable(
            &file_path,
            "application/octet-stream",
            Some(10),
            Some("xfer_1"),
        ))
        .expect("resume must complete");
    assert_eq!(second.file_id, "file_up_1");

    let resumed_parts: Vec<u32> = srv_state.lock().part_upload_calls[calls_before_resume..]
        .iter()
        .map(|(_, p)| *p)
        .collect();
    assert_eq!(
        resumed_parts,
        vec![2, 3, 4],
        "must only upload missing parts 2,3,4"
    );

    // Simulate expired server session (404 on /status) -> auto re-init and complete
    let rec_expired = FileTransferRecord {
        transfer_id: "xfer_404".to_string(),
        upload_id: Some("unknown_expired_id".to_string()),
        status: TransferStatus::Paused,
        ..rec_mid
    };
    client2
        .chunked_transfers
        .store()
        .insert(&rec_expired)
        .unwrap();
    let reinit_res = rt
        .block_on(client2.upload_file_resumable(
            &file_path,
            "application/octet-stream",
            Some(10),
            Some("xfer_404"),
        ))
        .expect("404 on status must re-init upload");
    assert_eq!(reinit_res.file_id, "file_up_2");
}

#[test]
fn test_f8_e2ee_x3dh_double_ratchet_multi_device_and_empty_plaintext() {
    // 1. Direct X3DH + Double Ratchet edge cases (empty plaintext, out-of-order, replay reject)
    let alice_id = DeviceIdentityKeys::generate().unwrap();
    let bob_id = DeviceIdentityKeys::generate().unwrap();
    let bob_spk_sec = random_32().unwrap();
    let bob_spk_pub = x25519_pub_from_secret(&bob_spk_sec);
    let spk_sig = bob_id.sign_signed_prekey(&bob_spk_pub);
    assert!(gpui_telegram_client::verify_signed_prekey(
        &bob_id.ed25519_pub,
        &bob_spk_pub,
        &spk_sig
    )
    .is_ok());
    let bob_otpk_sec = random_32().unwrap();
    let bob_otpk_pub = x25519_pub_from_secret(&bob_otpk_sec);

    let bundle = DevicePreKeyBundle {
        user_id: 2,
        device_id: "bob_1".into(),
        device_name: Some("Bob".into()),
        identity_key: b64_encode(&bob_id.x25519_pub),
        identity_signing_key: b64_encode(&bob_id.ed25519_pub),
        signed_prekey: SignedPreKeyDto {
            key_id: 1,
            public_key: b64_encode(&bob_spk_pub),
            signature: b64_encode(&spk_sig),
            created_at_ms: 1_700_000_000_000,
        },
        one_time_prekey: Some(OneTimePreKeyDto {
            key_id: 10,
            public_key: b64_encode(&bob_otpk_pub),
        }),
    };

    let ek_a_sec = random_32().unwrap();
    let ek_a_pub = x25519_pub_from_secret(&ek_a_sec);
    let init = x3dh_initiator(&alice_id.x25519_sec, &ek_a_sec, &bundle).unwrap();
    let resp = x3dh_responder(
        &bob_id.x25519_sec,
        &bob_spk_sec,
        Some(&bob_otpk_sec),
        &alice_id.x25519_pub,
        &ek_a_pub,
    )
    .unwrap();
    assert_eq!(*init.shared_secret, *resp.shared_secret);
    assert_eq!(init.associated_data, resp.associated_data);

    let mut alice_dr = DoubleRatchetSession::init_alice(
        *init.shared_secret,
        bob_spk_pub,
        init.associated_data.clone(),
    )
    .unwrap();
    let mut bob_dr = DoubleRatchetSession::init_bob(
        *resp.shared_secret,
        bob_spk_sec,
        resp.associated_data,
    );

    // Empty plaintext & out-of-order decryption
    let (h0, c0) = alice_dr.encrypt(b"").unwrap();
    let (h1, c1) = alice_dr.encrypt(b"msg-1").unwrap();
    let (h2, c2) = alice_dr.encrypt(b"msg-2").unwrap();
    assert_eq!(bob_dr.decrypt(&h2, &c2).unwrap(), b"msg-2");
    assert_eq!(bob_dr.decrypt(&h0, &c0).unwrap(), b"");
    assert_eq!(bob_dr.decrypt(&h1, &c1).unwrap(), b"msg-1");
    assert!(bob_dr.decrypt(&h1, &c1).is_err(), "replay must fail");

    // 2. Multi-device E2eeManager flow across 2 Alice devices (A1, A2) and 2 Bob devices (B1, B2)
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let prekey_store: Arc<Mutex<HashMap<(u64, String), PublishPreKeysRequest>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let srv_store = Arc::clone(&prekey_store);

    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    rt.spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let store = Arc::clone(&srv_store);
            tokio::spawn(async move {
                let (method, path, headers, body) = read_http_request(&mut stream).await;
                let uid: u64 = headers
                    .lines()
                    .find_map(|l| {
                        let lower = l.to_ascii_lowercase();
                        lower.strip_prefix("authorization:").and_then(|v| {
                            v.trim().strip_prefix("bearer tok_").and_then(|s| s.parse().ok())
                        })
                    })
                    .unwrap_or(0);
                if method == "POST" && path == "/api/v1/e2ee/prekeys" {
                    let req: PublishPreKeysRequest = serde_json::from_slice(&body).unwrap();
                    let count = req.one_time_prekeys.len();
                    let dev = req.device_id.clone();
                    store.lock().insert((uid, dev.clone()), req);
                    let resp = json!({
                        "success": true,
                        "user_id": uid,
                        "device_id": dev,
                        "remaining_one_time_prekeys": count,
                        "updated_at_ms": 1700000000000i64
                    });
                    write_json_response(&mut stream, 200, "OK", &resp.to_string()).await;
                } else if method == "GET" && path.starts_with("/api/v1/e2ee/devices/") {
                    let target_uid: u64 = path
                        .trim_start_matches("/api/v1/e2ee/devices/")
                        .trim_end_matches("/prekeys")
                        .parse()
                        .unwrap_or(0);
                    let resp_str = {
                        let mut guard = store.lock();
                        let mut devices = Vec::new();
                        for ((u, dev), req) in guard.iter_mut() {
                            if *u == target_uid {
                                let otpk = if req.one_time_prekeys.is_empty() {
                                    None
                                } else {
                                    Some(req.one_time_prekeys.remove(0))
                                };
                                devices.push(DevicePreKeyBundle {
                                    user_id: target_uid,
                                    device_id: dev.clone(),
                                    device_name: req.device_name.clone(),
                                    identity_key: req.identity_key.clone(),
                                    identity_signing_key: req.identity_signing_key.clone(),
                                    signed_prekey: req.signed_prekey.clone(),
                                    one_time_prekey: otpk,
                                });
                            }
                        }
                        let resp = UserPreKeyBundlesResponse {
                            user_id: target_uid,
                            devices,
                        };
                        serde_json::to_string(&resp).unwrap()
                    };
                    write_json_response(&mut stream, 200, "OK", &resp_str).await;
                } else {
                    write_json_response(&mut stream, 404, "Not Found", "{}").await;
                }
            });
        }
    });

    let cfg = ClientConfig {
        server_url: format!("http://{addr}"),
        ..Default::default()
    };
    let d_a1 = tempfile::tempdir().unwrap();
    let d_a2 = tempfile::tempdir().unwrap();
    let d_b1 = tempfile::tempdir().unwrap();
    let d_b2 = tempfile::tempdir().unwrap();

    let c_a1 = TelegramClient::open_with_dir(cfg.clone(), d_a1.path()).unwrap();
    c_a1.set_device_id("dev_a1");
    c_a1.set_auth_session(make_test_auth(1, "alice", "tok_1"));

    let c_a2 = TelegramClient::open_with_dir(cfg.clone(), d_a2.path()).unwrap();
    c_a2.set_device_id("dev_a2");
    c_a2.set_auth_session(make_test_auth(1, "alice", "tok_1"));

    let c_b1 = TelegramClient::open_with_dir(cfg.clone(), d_b1.path()).unwrap();
    c_b1.set_device_id("dev_b1");
    c_b1.set_auth_session(make_test_auth(2, "bob", "tok_2"));

    let c_b2 = TelegramClient::open_with_dir(cfg.clone(), d_b2.path()).unwrap();
    c_b2.set_device_id("dev_b2");
    c_b2.set_auth_session(make_test_auth(2, "bob", "tok_2"));

    for c in [&c_a1, &c_a2, &c_b1, &c_b2] {
        rt.block_on(c.e2ee_init_and_publish_prekeys(5)).unwrap();
    }

    // A1 encrypts DM to Bob -> fans out to B1, B2 AND sender's own other device A2
    let env = rt.block_on(c_a1.e2ee_encrypt_dm(2, b"multi-device secret")).unwrap();
    assert_eq!(c_b1.e2ee_decrypt_envelope(&env).unwrap(), b"multi-device secret");
    assert_eq!(c_b2.e2ee_decrypt_envelope(&env).unwrap(), b"multi-device secret");
    assert_eq!(c_a2.e2ee_decrypt_envelope(&env).unwrap(), b"multi-device secret");

    // Group Sender Keys across A1, B1, B2
    let (dist_opt, grp_env) = rt
        .block_on(c_a1.e2ee_encrypt_group(777, &[1, 2], b"group hello"))
        .unwrap();
    let dist_env = dist_opt.expect("first group send distributes sender key");
    assert_eq!(c_b1.e2ee_decrypt_envelope(&dist_env).unwrap(), b"");
    assert_eq!(c_b2.e2ee_decrypt_envelope(&dist_env).unwrap(), b"");
    assert_eq!(c_b1.e2ee_decrypt_envelope(&grp_env).unwrap(), b"group hello");
    assert_eq!(c_b2.e2ee_decrypt_envelope(&grp_env).unwrap(), b"group hello");

    // Persistence across restart for B1
    drop(c_b1);
    let c_b1_reloaded = TelegramClient::open_with_dir(cfg, d_b1.path()).unwrap();
    c_b1_reloaded.set_device_id("dev_b1");
    c_b1_reloaded.set_auth_session(make_test_auth(2, "bob", "tok_2"));
    let (_, grp_env2) = rt
        .block_on(c_a1.e2ee_encrypt_group(777, &[1, 2], b"after restart"))
        .unwrap();
    assert_eq!(
        c_b1_reloaded.e2ee_decrypt_envelope(&grp_env2).unwrap(),
        b"after restart"
    );

    // Direct SenderKeyState unit sanity
    let (sk, dist_msg) = SenderKeyState::create_own(99, 1, "d1".into()).unwrap();
    assert_eq!(sk.chat_id, 99);
    assert_eq!(dist_msg.chat_id, 99);
}

#[test]
fn test_f9_proxy_http_connect_and_socks5_tunneling_http_and_ws() {
    let rt = tokio::runtime::Runtime::new().expect("rt");

    // 1. Backend HTTP + WebSocket server
    let backend_listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    rt.spawn(async move {
        loop {
            let Ok((mut stream, _)) = backend_listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut peek_buf = [0u8; 2048];
                let n = stream.peek(&mut peek_buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&peek_buf[..n]);
                if head.to_ascii_lowercase().contains("upgrade: websocket") {
                    if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                        let ack = WsServerFrame::Ack {
                            client_nonce: 9001,
                            message_id: 55,
                            pts: 7,
                            correlation_id: 0,
                        };
                        let _ = ws
                            .send(WsMessage::Text(serde_json::to_string(&ack).unwrap().into()))
                            .await;
                        while let Some(Ok(msg)) = ws.next().await {
                            if let WsMessage::Text(t) = msg {
                                if let Ok(WsClientFrame::Ping { nonce }) = serde_json::from_str(&t) {
                                    let pong = WsServerFrame::Pong { nonce };
                                    let _ = ws
                                        .send(WsMessage::Text(
                                            serde_json::to_string(&pong).unwrap().into(),
                                        ))
                                        .await;
                                }
                            }
                        }
                    }
                    return;
                }
                let (_m, path, _, _) = read_http_request(&mut stream).await;
                if path == "/health" {
                    write_json_response(&mut stream, 200, "OK", r#"{"ok":true}"#).await;
                } else {
                    write_json_response(
                        &mut stream,
                        200,
                        "OK",
                        r#"{"status":"healthy","version":"m2"}"#,
                    )
                    .await;
                }
            });
        }
    });

    // 2. Local HTTP CONNECT Proxy
    let http_proxy_hits = Arc::new(AtomicUsize::new(0));
    let http_proxy_listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let http_proxy_addr = http_proxy_listener.local_addr().unwrap();
    let http_hits_srv = Arc::clone(&http_proxy_hits);
    rt.spawn(async move {
        loop {
            let Ok((mut client_stream, _)) = http_proxy_listener.accept().await else {
                break;
            };
            let hits = Arc::clone(&http_hits_srv);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                loop {
                    let n = client_stream.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let req_str = String::from_utf8_lossy(&buf);
                let first_line = req_str.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("");
                let target = parts.next().unwrap_or("");
                hits.fetch_add(1, Ordering::SeqCst);
                if method.eq_ignore_ascii_case("CONNECT") {
                    if let Ok(mut remote) = TcpStream::connect(target).await {
                        let _ = client_stream
                            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                            .await;
                        let _ = tokio::io::copy_bidirectional(&mut client_stream, &mut remote).await;
                    }
                } else if let Ok(url) = reqwest::Url::parse(target) {
                    let host_port = format!(
                        "{}:{}",
                        url.host_str().unwrap(),
                        url.port_or_known_default().unwrap()
                    );
                    if let Ok(mut remote) = TcpStream::connect(&host_port).await {
                        let path_q = if let Some(q) = url.query() {
                            format!("{}?{}", url.path(), q)
                        } else {
                            url.path().to_string()
                        };
                        let rebuilt = req_str.replacen(target, &path_q, 1);
                        let _ = remote.write_all(rebuilt.as_bytes()).await;
                        let _ = tokio::io::copy_bidirectional(&mut client_stream, &mut remote).await;
                    }
                }
            });
        }
    });

    // 3. Local SOCKS5 Proxy (supports username/password auth)
    let socks_proxy_hits = Arc::new(AtomicUsize::new(0));
    let socks_listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let socks_addr = socks_listener.local_addr().unwrap();
    let socks_hits_srv = Arc::clone(&socks_proxy_hits);
    rt.spawn(async move {
        loop {
            let Ok((mut stream, _)) = socks_listener.accept().await else {
                break;
            };
            let hits = Arc::clone(&socks_hits_srv);
            tokio::spawn(async move {
                let mut hdr = [0u8; 2];
                if stream.read_exact(&mut hdr).await.is_err() || hdr[0] != 0x05 {
                    return;
                }
                let mut methods = vec![0u8; hdr[1] as usize];
                if stream.read_exact(&mut methods).await.is_err() {
                    return;
                }
                if methods.contains(&0x02) {
                    let _ = stream.write_all(&[0x05, 0x02]).await;
                    let mut auth_ver = [0u8; 2];
                    if stream.read_exact(&mut auth_ver).await.is_err() {
                        return;
                    }
                    let mut u = vec![0u8; auth_ver[1] as usize];
                    let _ = stream.read_exact(&mut u).await;
                    let mut plen = [0u8; 1];
                    let _ = stream.read_exact(&mut plen).await;
                    let mut p = vec![0u8; plen[0] as usize];
                    let _ = stream.read_exact(&mut p).await;
                    if u == b"alice" && p == b"secret" {
                        let _ = stream.write_all(&[0x01, 0x00]).await;
                    } else {
                        let _ = stream.write_all(&[0x01, 0x01]).await;
                        return;
                    }
                } else {
                    let _ = stream.write_all(&[0x05, 0x00]).await;
                }

                let mut req_hdr = [0u8; 4];
                if stream.read_exact(&mut req_hdr).await.is_err() {
                    return;
                }
                let dest = match req_hdr[3] {
                    0x01 => {
                        let mut ip = [0u8; 4];
                        let _ = stream.read_exact(&mut ip).await;
                        let mut port = [0u8; 2];
                        let _ = stream.read_exact(&mut port).await;
                        format!(
                            "{}.{}.{}.{}:{}",
                            ip[0],
                            ip[1],
                            ip[2],
                            ip[3],
                            u16::from_be_bytes(port)
                        )
                    }
                    0x03 => {
                        let mut len = [0u8; 1];
                        let _ = stream.read_exact(&mut len).await;
                        let mut host = vec![0u8; len[0] as usize];
                        let _ = stream.read_exact(&mut host).await;
                        let mut port = [0u8; 2];
                        let _ = stream.read_exact(&mut port).await;
                        format!(
                            "{}:{}",
                            String::from_utf8_lossy(&host),
                            u16::from_be_bytes(port)
                        )
                    }
                    _ => return,
                };
                hits.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut remote) = TcpStream::connect(&dest).await {
                    let _ = stream
                        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                        .await;
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut remote).await;
                }
            });
        }
    });

    let base_url = format!("http://{backend_addr}");
    let http_client = HttpClient::new_with_proxy(
        base_url.clone(),
        Duration::from_secs(5),
        Some(rt.handle().clone()),
        None,
    );

    // Test HTTP CONNECT Proxy: HTTP + probe + WS
    let http_proxy = ClientProxyConfig::http("127.0.0.1", http_proxy_addr.port());
    http_client.set_proxy(Some(http_proxy.clone())).unwrap();
    let http_probe = rt.block_on(http_client.probe_proxy(&http_proxy)).unwrap();
    assert!(http_probe < 5000);
    assert!(rt.block_on(http_client.check_health()).unwrap());
    assert!(http_proxy_hits.load(Ordering::SeqCst) >= 2);

    let ws_chan = WsChannel::new();
    let mut rx_http = ws_chan.subscribe();
    let ws_http = WsTransport::new_with_runtime(base_url.clone(), ws_chan.clone(), Some(rt.handle().clone()))
        .with_proxy(Some(http_proxy));
    ws_http.set_token(Some("tok_ws".into()));
    ws_http.start();
    let frame1 = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), rx_http.recv())
            .await
            .unwrap()
            .unwrap()
    });
    assert!(matches!(frame1, WsServerFrame::Ack { client_nonce: 9001, .. }));
    ws_http.stop();

    // Test SOCKS5 Proxy with auth: HTTP + probe + WS
    let socks_proxy = ClientProxyConfig::socks5("127.0.0.1", socks_addr.port())
        .with_auth("alice", "secret");
    http_client.set_proxy(Some(socks_proxy.clone())).unwrap();
    let socks_probe = rt.block_on(http_client.probe_proxy(&socks_proxy)).unwrap();
    assert!(socks_probe < 5000);
    assert!(rt.block_on(http_client.check_health()).unwrap());
    assert!(socks_proxy_hits.load(Ordering::SeqCst) >= 2);

    let ws_chan2 = WsChannel::new();
    let mut rx_socks = ws_chan2.subscribe();
    let ws_socks = WsTransport::new_with_runtime(base_url, ws_chan2, Some(rt.handle().clone()))
        .with_proxy(Some(socks_proxy));
    ws_socks.set_token(Some("tok_ws2".into()));
    ws_socks.start();
    let frame2 = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), rx_socks.recv())
            .await
            .unwrap()
            .unwrap()
    });
    assert!(matches!(frame2, WsServerFrame::Ack { client_nonce: 9001, .. }));
    ws_socks.stop();
}

#[test]
fn test_f10_save_draft_unwrap_folder_cache_and_ttl_purge() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rt = tokio::runtime::Runtime::new().expect("rt");

    // 1. Verify HttpClient::save_draft unwraps `{"draft": {...}}` server envelope
    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    rt.spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let (method, path, _, body) = read_http_request(&mut stream).await;
                if method == "PUT" && path == "/api/v1/chats/77/draft" {
                    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let resp = json!({ "draft": parsed });
                    write_json_response(&mut stream, 200, "OK", &resp.to_string()).await;
                } else {
                    write_json_response(&mut stream, 404, "Not Found", "{}").await;
                }
            });
        }
    });

    let http = HttpClient::new_with_proxy(
        format!("http://{addr}"),
        Duration::from_secs(5),
        Some(rt.handle().clone()),
        None,
    );
    let draft = gpui_telegram_proto::contracts::DraftDto {
        chat_id: 77,
        text: "unsent draft text".into(),
        entities: Vec::new(),
        reply_to_msg_id: None,
        topic_id: None,
        date_ms: 1_700_000_000_000,
    };
    let saved = rt.block_on(http.save_draft("tok", 77, &draft)).unwrap();
    assert_eq!(saved.text, "unsent draft text");
    assert_eq!(saved.chat_id, 77);

    // 2. Verify LocalStore folder caching + delta upsert/remove + TTL purge
    let store = LocalStore::open_with_dir(dir.path()).unwrap();
    let folder = ChatFolderDto {
        id: 5,
        title: "Work".into(),
        emoticon: Some("💼".into()),
        include_chats: vec![77],
        exclude_chats: vec![],
        pinned_chats: vec![77],
        include_contacts: true,
        include_non_contacts: false,
        include_groups: true,
        include_channels: false,
        include_bots: false,
        exclude_muted: false,
        exclude_read: false,
        exclude_archived: false,
        sort_order: 0,
    };
    store.upsert_folder(folder);
    assert_eq!(store.get_folders().len(), 1);
    assert_eq!(store.get_folders()[0].title, "Work");
    store.remove_folder(5);
    assert!(store.get_folders().is_empty());

    // Insert 1 expired TTL message and 1 non-expired message
    let mut expired = MessageItem::new(
        MessageId(101),
        ChatId(77),
        UserId(1),
        PeerId::Chat(ChatId(77)),
        MessageContent::plain_text("burn after reading"),
    );
    expired.ttl_seconds = Some(5);
    expired.expires_at_ms = Some(1_000);

    let mut alive = MessageItem::new(
        MessageId(102),
        ChatId(77),
        UserId(1),
        PeerId::Chat(ChatId(77)),
        MessageContent::plain_text("stay alive"),
    );
    alive.ttl_seconds = Some(60);
    alive.expires_at_ms = Some(50_000);

    store.insert_message(expired);
    store.insert_message(alive);
    let purged = store.purge_expired_messages(10_000);
    assert_eq!(purged, vec![(ChatId(77), vec![MessageId(101)])]);
    let remaining = store.get_messages(ChatId(77)).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, MessageId(102));
}

#[test]
fn test_regression_issue1_sender_key_distribution_spoofing_and_message_sender_mismatch_rejected() {
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let prekey_store: Arc<Mutex<HashMap<(u64, String), PublishPreKeysRequest>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let srv_store = Arc::clone(&prekey_store);

    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    rt.spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let store = Arc::clone(&srv_store);
            tokio::spawn(async move {
                let (method, path, headers, body) = read_http_request(&mut stream).await;
                let uid: u64 = headers
                    .lines()
                    .find_map(|l| {
                        let lower = l.to_ascii_lowercase();
                        lower.strip_prefix("authorization:").and_then(|v| {
                            v.trim().strip_prefix("bearer tok_").and_then(|s| s.parse().ok())
                        })
                    })
                    .unwrap_or(0);
                if method == "POST" && path == "/api/v1/e2ee/prekeys" {
                    let req: PublishPreKeysRequest = serde_json::from_slice(&body).unwrap();
                    let count = req.one_time_prekeys.len();
                    let dev = req.device_id.clone();
                    store.lock().insert((uid, dev.clone()), req);
                    let resp = json!({
                        "success": true,
                        "user_id": uid,
                        "device_id": dev,
                        "remaining_one_time_prekeys": count,
                        "updated_at_ms": 1700000000000i64
                    });
                    write_json_response(&mut stream, 200, "OK", &resp.to_string()).await;
                } else if method == "GET" && path.starts_with("/api/v1/e2ee/devices/") {
                    let target_uid: u64 = path
                        .trim_start_matches("/api/v1/e2ee/devices/")
                        .trim_end_matches("/prekeys")
                        .parse()
                        .unwrap_or(0);
                    let resp_str = {
                        let mut guard = store.lock();
                        let mut devices = Vec::new();
                        for ((u, dev), req) in guard.iter_mut() {
                            if *u == target_uid {
                                let otpk = if req.one_time_prekeys.is_empty() {
                                    None
                                } else {
                                    Some(req.one_time_prekeys.remove(0))
                                };
                                devices.push(DevicePreKeyBundle {
                                    user_id: target_uid,
                                    device_id: dev.clone(),
                                    device_name: req.device_name.clone(),
                                    identity_key: req.identity_key.clone(),
                                    identity_signing_key: req.identity_signing_key.clone(),
                                    signed_prekey: req.signed_prekey.clone(),
                                    one_time_prekey: otpk,
                                });
                            }
                        }
                        serde_json::to_string(&UserPreKeyBundlesResponse {
                            user_id: target_uid,
                            devices,
                        })
                        .unwrap()
                    };
                    write_json_response(&mut stream, 200, "OK", &resp_str).await;
                } else {
                    write_json_response(&mut stream, 404, "Not Found", "{}").await;
                }
            });
        }
    });

    let cfg = ClientConfig {
        server_url: format!("http://{addr}"),
        ..Default::default()
    };
    let d_a = tempfile::tempdir().unwrap();
    let d_b = tempfile::tempdir().unwrap();
    let d_c = tempfile::tempdir().unwrap();

    let c_a = TelegramClient::open_with_dir(cfg.clone(), d_a.path()).unwrap();
    c_a.set_device_id("dev_a1");
    c_a.set_auth_session(make_test_auth(1, "alice", "tok_1"));

    let c_b = TelegramClient::open_with_dir(cfg.clone(), d_b.path()).unwrap();
    c_b.set_device_id("dev_b1");
    c_b.set_auth_session(make_test_auth(2, "bob", "tok_2"));

    let c_c = TelegramClient::open_with_dir(cfg, d_c.path()).unwrap();
    c_c.set_device_id("dev_c1");
    c_c.set_auth_session(make_test_auth(3, "charlie", "tok_3"));

    for c in [&c_a, &c_b, &c_c] {
        rt.block_on(c.e2ee_init_and_publish_prekeys(5)).unwrap();
    }

    // 1. Alice distributes her legitimate group SenderKey to Bob & Charlie
    let (dist_opt, grp1) = rt
        .block_on(c_a.e2ee_encrypt_group(888, &[1, 2, 3], b"legit alice 1"))
        .unwrap();
    let dist_env = dist_opt.unwrap();
    assert_eq!(c_b.e2ee_decrypt_envelope(&dist_env).unwrap(), b"");
    assert_eq!(c_b.e2ee_decrypt_envelope(&grp1).unwrap(), b"legit alice 1");

    // 2. Charlie establishes a pairwise DR session to Bob, then crafts a spoofed SenderKeyDistributionMessage
    // claiming sender_user_id=1 (Alice) and sender_device_id="dev_a1".
    let _ = rt
        .block_on(c_c.e2ee_encrypt_dm(2, b"charlie setup"))
        .unwrap();
    let (mut fake_alice_sk, mut spoof_dist) =
        SenderKeyState::create_own(888, 1, "dev_a1".to_string()).unwrap();
    spoof_dist.sender_user_id = 1;
    spoof_dist.sender_device_id = "dev_a1".to_string();
    let spoof_bytes = serde_json::to_vec(&spoof_dist).unwrap();
    let spoof_env = c_c
        .e2ee_manager()
        .encrypt_pairwise_for_targets(&[(2, "dev_b1".to_string())], &spoof_bytes, true)
        .unwrap();

    // Bob MUST reject the spoofed SenderKeyDistribution because envelope sender is 3:dev_c1 != 1:dev_a1
    let spoof_res = c_b.e2ee_decrypt_envelope(&spoof_env);
    assert!(
        spoof_res.is_err(),
        "Spoofed SenderKeyDistribution must be rejected"
    );

    // Forged GroupSenderKey signed by fake_alice_sk must also fail, while Alice's authentic key remains intact
    let forged_ct = fake_alice_sk.encrypt_group_message(b"IMPERSONATED").unwrap();
    let c_id = c_a.e2ee_manager().identity().unwrap();
    let forged_env = EncryptedEnvelope {
        version: 1,
        sender_user_id: 1,
        sender_device_id: "dev_a1".to_string(),
        sender_identity_key: b64_encode(&c_id.x25519_pub),
        sender_signing_key: b64_encode(&c_id.ed25519_pub),
        kind: EncryptedPayloadKind::GroupSenderKey(forged_ct),
    };
    assert!(c_b.e2ee_decrypt_envelope(&forged_env).is_err());

    let (_, grp2) = rt
        .block_on(c_a.e2ee_encrypt_group(888, &[1, 2, 3], b"legit alice 2"))
        .unwrap();
    assert_eq!(c_b.e2ee_decrypt_envelope(&grp2).unwrap(), b"legit alice 2");

    // 3. try_decrypt_message_item must reject MessageItem whose sender_id != env.sender_user_id
    let dm_env = rt.block_on(c_a.e2ee_encrypt_dm(2, b"authentic dm")).unwrap();
    let mismatched_item = MessageItem::new(
        MessageId(500),
        ChatId(99),
        UserId(3),
        PeerId::User(UserId(2)),
        MessageContent::Encrypted(dm_env.clone()),
    );
    assert_eq!(
        c_b.e2ee_manager().try_decrypt_message_item(&mismatched_item),
        None
    );
    let authentic_item = MessageItem::new(
        MessageId(501),
        ChatId(99),
        UserId(1),
        PeerId::User(UserId(2)),
        MessageContent::Encrypted(dm_env),
    );
    assert_eq!(
        c_b.e2ee_manager()
            .try_decrypt_message_item(&authentic_item)
            .as_deref(),
        Some("authentic dm")
    );
}

#[test]
fn test_regression_issue2_ws_transport_stop_start_proxy_hotswap_no_livelock() {
    let rt = tokio::runtime::Runtime::new().expect("rt");

    let backend_ws_count = Arc::new(AtomicUsize::new(0));
    let backend_listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    let ws_cnt_srv = Arc::clone(&backend_ws_count);
    rt.spawn(async move {
        loop {
            let Ok((stream, _)) = backend_listener.accept().await else {
                break;
            };
            let cnt = Arc::clone(&ws_cnt_srv);
            tokio::spawn(async move {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                    let conn_num = cnt.fetch_add(1, Ordering::SeqCst) + 1;
                    let ack = WsServerFrame::Ack {
                        correlation_id: 0,
                        client_nonce: conn_num as u64,
                        message_id: conn_num as i64,
                        pts: 1,
                    };
                    let _ = ws
                        .send(WsMessage::Text(serde_json::to_string(&ack).unwrap().into()))
                        .await;
                    while let Some(Ok(msg)) = ws.next().await {
                        if let WsMessage::Text(t) = msg {
                            if let Ok(WsClientFrame::Ping { nonce }) = serde_json::from_str(&t) {
                                let pong = WsServerFrame::Pong { nonce };
                                let _ = ws
                                    .send(WsMessage::Text(
                                        serde_json::to_string(&pong).unwrap().into(),
                                    ))
                                    .await;
                            }
                        }
                    }
                }
            });
        }
    });

    // SOCKS5 proxy
    let socks_hits = Arc::new(AtomicUsize::new(0));
    let socks_listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let socks_addr = socks_listener.local_addr().unwrap();
    let socks_hits_srv = Arc::clone(&socks_hits);
    rt.spawn(async move {
        loop {
            let Ok((mut stream, _)) = socks_listener.accept().await else {
                break;
            };
            let hits = Arc::clone(&socks_hits_srv);
            tokio::spawn(async move {
                let mut hdr = [0u8; 2];
                if stream.read_exact(&mut hdr).await.is_err() || hdr[0] != 0x05 {
                    return;
                }
                let mut methods = vec![0u8; hdr[1] as usize];
                if stream.read_exact(&mut methods).await.is_err() {
                    return;
                }
                let _ = stream.write_all(&[0x05, 0x00]).await;
                let mut req_hdr = [0u8; 4];
                if stream.read_exact(&mut req_hdr).await.is_err() {
                    return;
                }
                let dest = match req_hdr[3] {
                    0x01 => {
                        let mut ip = [0u8; 4];
                        let _ = stream.read_exact(&mut ip).await;
                        let mut port = [0u8; 2];
                        let _ = stream.read_exact(&mut port).await;
                        format!(
                            "{}.{}.{}.{}:{}",
                            ip[0],
                            ip[1],
                            ip[2],
                            ip[3],
                            u16::from_be_bytes(port)
                        )
                    }
                    0x03 => {
                        let mut len = [0u8; 1];
                        let _ = stream.read_exact(&mut len).await;
                        let mut host = vec![0u8; len[0] as usize];
                        let _ = stream.read_exact(&mut host).await;
                        let mut port = [0u8; 2];
                        let _ = stream.read_exact(&mut port).await;
                        format!(
                            "{}:{}",
                            String::from_utf8_lossy(&host),
                            u16::from_be_bytes(port)
                        )
                    }
                    _ => return,
                };
                hits.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut remote) = TcpStream::connect(&dest).await {
                    let _ = stream
                        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                        .await;
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut remote).await;
                }
            });
        }
    });

    let ws_chan = WsChannel::new();
    let mut rx = ws_chan.subscribe();
    let transport = WsTransport::new_with_runtime(
        format!("http://{backend_addr}"),
        ws_chan.clone(),
        Some(rt.handle().clone()),
    );
    transport.set_token(Some("tok_hotswap".into()));
    transport.start();

    let first = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap()
    });
    assert!(matches!(first, WsServerFrame::Ack { client_nonce: 1, .. }));
    assert!(transport.is_connected());

    // Immediately hot-swap proxy with stop()+start() back-to-back
    let socks_proxy = ClientProxyConfig::socks5("127.0.0.1", socks_addr.port());
    transport.set_proxy(Some(socks_proxy));
    transport.stop();
    transport.start();

    let second = rt.block_on(async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
            let f = tokio::time::timeout(rem, rx.recv())
                .await
                .expect("must connect via proxy after stop()+start()")
                .unwrap();
            if matches!(f, WsServerFrame::Ack { client_nonce: 2, .. }) {
                break f;
            }
        }
    });
    assert!(matches!(second, WsServerFrame::Ack { client_nonce: 2, .. }));

    // Settle for 2.2s (> 1s reconnect ping-pong cadence) and confirm zero churn
    rt.block_on(async {
        tokio::time::sleep(Duration::from_millis(2200)).await;
    });
    assert!(
        transport.is_connected(),
        "Transport must remain Connected without live-lock, got {:?}",
        transport.status()
    );
    assert_eq!(socks_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backend_ws_count.load(Ordering::SeqCst), 2);

    assert!(ws_chan.send(WsClientFrame::Ping { nonce: 777 }));
    let pong = rt.block_on(async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
            let f = tokio::time::timeout(rem, rx.recv()).await.unwrap().unwrap();
            if matches!(f, WsServerFrame::Pong { nonce: 777 }) {
                break f;
            }
        }
    });
    assert!(matches!(pong, WsServerFrame::Pong { nonce: 777 }));
    transport.stop();
}

#[test]
fn test_regression_issue3_outbox_reconnect_during_inflight_flush_wakes_backed_off_item() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rt = tokio::runtime::Runtime::new().expect("rt");

    let inflight_started = Arc::new(tokio::sync::Notify::new());
    let inflight_started_srv = Arc::clone(&inflight_started);
    let delivered_nonces = Arc::new(Mutex::new(Vec::<u64>::new()));
    let nonces_srv = Arc::clone(&delivered_nonces);

    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    rt.spawn(async move {
        let mut next_msg_id = 8000i64;
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let notify = Arc::clone(&inflight_started_srv);
            let nonces = Arc::clone(&nonces_srv);
            tokio::spawn(async move {
                let (method, path, _, body) = read_http_request(&mut stream).await;
                if method == "POST"
                    && (path == "/api/v1/messages/send" || path == "/api/v1/chats/42/messages")
                {
                    let parsed: serde_json::Value =
                        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                    let nonce = parsed["client_nonce"].as_u64().unwrap_or(0);
                    if nonce == 50_001 {
                        notify.notify_waiters();
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    next_msg_id += 1;
                    nonces.lock().push(nonce);
                    let resp = json!({
                        "message_id": next_msg_id,
                        "id": next_msg_id,
                        "pts": 20,
                        "pts_count": 1,
                        "duplicate": false,
                        "date_ms": 1700000000000i64,
                        "date": "2026-11-17T00:00:00Z",
                        "sender_id": 100,
                        "chat_id": 42,
                        "text": parsed["text"].as_str().unwrap_or(""),
                        "client_nonce": nonce,
                        "status": "sent"
                    });
                    write_json_response(&mut stream, 200, "OK", &resp.to_string()).await;
                } else {
                    write_json_response(&mut stream, 200, "OK", "{}").await;
                }
            });
        }
    });

    let cfg = ClientConfig {
        server_url: format!("http://{addr}"),
        ..Default::default()
    };
    let client = TelegramClient::open_with_dir(cfg, dir.path()).unwrap();
    client.set_auth_session(make_test_auth(100, "alice", "tok_race"));
    client.network_listener.set_online(true);

    let now_ms = chrono::Utc::now().timestamp_millis();
    client
        .outbox
        .store()
        .enqueue(&make_rich_req(42, "in flight", Some(50_001)), 50_001, now_ms)
        .unwrap();
    client
        .outbox
        .store()
        .enqueue(&make_rich_req(42, "backed off", Some(50_002)), 50_002, now_ms)
        .unwrap();
    // Put 50_002 into 25-second backoff
    let _ = client
        .outbox
        .store()
        .claim_next_due(now_ms)
        .unwrap();
    // Note: claim_next_due claimed 50_001 (lowest nonce); reset 50_001 back to Pending, and put 50_002 into 25s retry
    client.outbox.store().recover_in_flight(now_ms).unwrap();
    let conn_check = client.outbox.store().get(50_002).unwrap().unwrap();
    assert_eq!(conn_check.status, OutboxStatus::Pending);
    // Use claim + schedule_retry to place 50_002 in 25s backoff cleanly via public store API
    let c1 = client.outbox.store().claim_next_due(now_ms).unwrap().unwrap();
    assert_eq!(c1.client_nonce, 50_001);
    let c2 = client.outbox.store().claim_next_due(now_ms).unwrap().unwrap();
    assert_eq!(c2.client_nonce, 50_002);
    client
        .outbox
        .store()
        .schedule_retry(50_002, 4, now_ms + 25_000, "503 simulated", now_ms)
        .unwrap();
    client
        .outbox
        .store()
        .schedule_retry(50_001, 0, 0, "ready", now_ms)
        .unwrap();

    // Start background OutboxWorker and wait until 50_001 HTTP request is in-flight
    client.outbox.start(rt.handle());
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), inflight_started.notified())
            .await
            .expect("50_001 must enter in-flight HTTP call");
        // Flap Offline -> Online WHILE 50_001 is still awaiting the 300ms HTTP response
        client.network_listener.set_online(false);
        client.network_listener.set_online(true);

        let start = tokio::time::Instant::now();
        loop {
            let rec = client.outbox.store().get(50_002).unwrap().unwrap();
            if rec.status == OutboxStatus::Delivered {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_millis(1500),
                "50_002 stalled in {:?} past 1.5s despite reconnect during in-flight flush_due",
                rec.status
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    });
    client.outbox.stop();
}

#[test]
fn test_regression_issue4_chunked_mid_upload_404_reinit_and_complete_error_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rt = tokio::runtime::Runtime::new().expect("rt");

    let file_path = dir.path().join("mid404.bin");
    let raw_bytes: Vec<u8> = (0..(100 * 1024)).map(|i| ((i * 17 + 3) % 256) as u8).collect();
    std::fs::write(&file_path, &raw_bytes).unwrap();
    let full_sha = sha256_hex_local(&raw_bytes);

    #[derive(Clone, Copy)]
    enum Mode {
        ReinitOnceOnPart1,
        Always404OnPart1,
        Complete400,
        Complete503,
    }
    struct Srv {
        mode: Mode,
        init_count: usize,
        evicted_up1: bool,
    }
    let srv = Arc::new(Mutex::new(Srv {
        mode: Mode::ReinitOnceOnPart1,
        init_count: 0,
        evicted_up1: false,
    }));

    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let srv_ref = Arc::clone(&srv);
    let fsha = full_sha.clone();

    rt.spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let st = Arc::clone(&srv_ref);
            let sha = fsha.clone();
            tokio::spawn(async move {
                let (method, path, _, _body) = read_http_request(&mut stream).await;
                if method == "POST" && path == "/api/v1/media/upload/init" {
                    let resp_str = {
                        let mut g = st.lock();
                        g.init_count += 1;
                        let uid = format!("up_{}", g.init_count);
                        json!({
                            "upload_id": uid,
                            "chunk_size": 32 * 1024,
                            "expires_at_ms": 1800000000000i64
                        })
                        .to_string()
                    };
                    write_json_response(&mut stream, 200, "OK", &resp_str).await;
                } else if method == "PUT" && path.starts_with("/api/v1/media/upload/") {
                    let rest = path.trim_start_matches("/api/v1/media/upload/");
                    let segs: Vec<&str> = rest.split('/').collect();
                    let uid = segs[0];
                    let part_idx: u32 = segs.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let return_404 = {
                        let mut g = st.lock();
                        match g.mode {
                            Mode::ReinitOnceOnPart1 => {
                                if uid == "up_1" && part_idx == 1 {
                                    g.evicted_up1 = true;
                                    true
                                } else {
                                    false
                                }
                            }
                            Mode::Always404OnPart1 => part_idx == 1,
                            Mode::Complete400 | Mode::Complete503 => false,
                        }
                    };
                    if return_404 {
                        write_json_response(
                            &mut stream,
                            404,
                            "Not Found",
                            r#"{"error":"staging evicted mid-upload"}"#,
                        )
                        .await;
                    } else {
                        write_json_response(&mut stream, 200, "OK", r#"{"ok":true}"#).await;
                    }
                } else if method == "POST" && path.ends_with("/complete") {
                    let uid = path
                        .trim_start_matches("/api/v1/media/upload/")
                        .trim_end_matches("/complete");
                    let mode = st.lock().mode;
                    match mode {
                        Mode::Complete400 => {
                            write_json_response(
                                &mut stream,
                                400,
                                "Bad Request",
                                r#"{"error":"checksum mismatch"}"#,
                            )
                            .await;
                        }
                        Mode::Complete503 => {
                            write_json_response(
                                &mut stream,
                                503,
                                "Service Unavailable",
                                r#"{"error":"cas temporary outage"}"#,
                            )
                            .await;
                        }
                        _ => {
                            let resp = json!({
                                "file_id": format!("file_{uid}"),
                                "sha256": sha,
                                "size_bytes": 100 * 1024,
                                "mime_type": "application/octet-stream"
                            });
                            write_json_response(&mut stream, 200, "OK", &resp.to_string()).await;
                        }
                    }
                } else {
                    write_json_response(&mut stream, 404, "Not Found", "{}").await;
                }
            });
        }
    });

    let cfg = ClientConfig {
        server_url: format!("http://{addr}"),
        ..Default::default()
    };
    let client = TelegramClient::open_with_dir(cfg, dir.path()).unwrap();
    client.set_auth_session(make_test_auth(101, "bob", "tok_chunk"));

    // Case A: Mid-upload 404 on PUT /part/1 transparently re-initializes and completes on up_2
    let res_a = rt
        .block_on(client.upload_file_resumable(
            &file_path,
            "application/octet-stream",
            Some(10),
            Some("xfer_mid404"),
        ))
        .expect("mid-upload 404 must re-init and succeed");
    assert_eq!(res_a.file_id, "file_up_2");
    assert_eq!(srv.lock().init_count, 2);
    let rec_a = client.chunked_transfers.store().get("xfer_mid404").unwrap().unwrap();
    assert_eq!(rec_a.status, TransferStatus::Completed);

    // Case B: Persistent 404 on PUT /part/1 triggers bounded 1-retry guard and marks Failed
    srv.lock().mode = Mode::Always404OnPart1;
    let init_before_b = srv.lock().init_count;
    let res_b = rt.block_on(client.upload_file_resumable(
        &file_path,
        "application/octet-stream",
        Some(10),
        Some("xfer_loop404"),
    ));
    assert!(res_b.is_err());
    assert_eq!(
        srv.lock().init_count - init_before_b,
        2,
        "must re-init at most once on repeated mid-upload 404"
    );
    let rec_b = client.chunked_transfers.store().get("xfer_loop404").unwrap().unwrap();
    assert_eq!(rec_b.status, TransferStatus::Failed);

    // Case C: /complete 400 -> Failed (not stuck in InProgress)
    srv.lock().mode = Mode::Complete400;
    let res_c = rt.block_on(client.upload_file_resumable(
        &file_path,
        "application/octet-stream",
        Some(10),
        Some("xfer_comp400"),
    ));
    assert!(res_c.is_err());
    let rec_c = client.chunked_transfers.store().get("xfer_comp400").unwrap().unwrap();
    assert_eq!(rec_c.status, TransferStatus::Failed);

    // Case D: /complete 503 -> Paused (not stuck in InProgress)
    srv.lock().mode = Mode::Complete503;
    let res_d = rt.block_on(client.upload_file_resumable(
        &file_path,
        "application/octet-stream",
        Some(10),
        Some("xfer_comp503"),
    ));
    assert!(res_d.is_err());
    let rec_d = client.chunked_transfers.store().get("xfer_comp503").unwrap().unwrap();
    assert_eq!(rec_d.status, TransferStatus::Paused);
}

#[test]
fn test_regression_issue6_seed_nonce_near_i64_max_and_durable_send_without_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let cfg = ClientConfig {
        server_url: "http://127.0.0.1:1".to_string(),
        ..Default::default()
    };

    let near_max = (i64::MAX as u64) - 500;
    {
        let client1 = TelegramClient::open_with_dir(cfg.clone(), dir.path()).unwrap();
        let req = make_rich_req(42, "near max nonce", Some(near_max));
        client1
            .outbox
            .store()
            .enqueue(&req, near_max, 1_700_000_000_000)
            .unwrap();
    }

    // Reopen without auth token
    let client2 = TelegramClient::open_with_dir(cfg, dir.path()).unwrap();
    let next_n = client2.next_client_nonce();
    assert!(
        next_n > near_max && next_n <= i64::MAX as u64,
        "seeded nonce {next_n} must be strictly > persisted_max {near_max} and <= i64::MAX"
    );

    // send_message_durable without token must still enqueue in outbox
    client2.network_listener.set_online(true);
    let no_tok_nonce = 777_001u64;
    let res = rt.block_on(client2.send_message_durable(make_rich_req(
        42,
        "queued without token",
        Some(no_tok_nonce),
    )));
    assert!(res.is_err());
    let rec = client2
        .outbox
        .store()
        .get(no_tok_nonce)
        .unwrap()
        .expect("message must be persisted in outbox even when token is missing");
    assert_eq!(rec.status, OutboxStatus::Pending);
}
