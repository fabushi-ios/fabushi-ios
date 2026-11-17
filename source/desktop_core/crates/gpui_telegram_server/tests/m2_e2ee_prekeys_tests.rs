use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ed25519_dalek::{Signer, SigningKey};
use gpui_telegram_proto::e2ee::{
    EncryptedEnvelope, EncryptedPayloadKind, OneTimePreKeyDto, PerDeviceCiphertext,
    PublishPreKeysRequest, RatchetHeaderDto, SignedPreKeyDto, UserPreKeyBundlesResponse,
};
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
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open_app(dir: &TempDirGuard) -> (Arc<AppState>, Router) {
    let cfg = ServerConfig {
        database_url: dir.0.join("e2ee_srv.db").to_string_lossy().into_owned(),
        media_storage_path: dir.0.join("cas").to_string_lossy().into_owned(),
        ..Default::default()
    };
    let state = Arc::new(AppState::try_new(cfg).unwrap());
    let app = build_router_with_state(state.clone());
    (state, app)
}

async fn api(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
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
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn reg(app: &Router, uname: &str) -> (u64, String) {
    let (st, v) = api(
        app,
        Method::POST,
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": uname, "password": "Password!1", "first_name": uname
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    (
        v["user_id"].as_u64().unwrap(),
        v["token"].as_str().unwrap().to_string(),
    )
}

fn build_valid_prekey_req(device_id: &str, spk_id: u32, otpk_ids: &[u32]) -> PublishPreKeysRequest {
    let mut ed_seed = [0u8; 32];
    ed_seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    ed_seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    let signing_key = SigningKey::from_bytes(&ed_seed);
    let verifying_key = signing_key.verifying_key();

    let mut ik_pub = [0u8; 32];
    ik_pub[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    ik_pub[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    let mut spk_pub = [0u8; 32];
    spk_pub[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    spk_pub[16..].copy_from_slice(Uuid::new_v4().as_bytes());

    let mut msg = Vec::with_capacity(16 + 32);
    msg.extend_from_slice(b"gpui-e2ee-spk-v1");
    msg.extend_from_slice(&spk_pub);
    let sig = signing_key.sign(&msg);

    let otpks = otpk_ids
        .iter()
        .map(|&id| {
            let mut pk = [0u8; 32];
            pk[..16].copy_from_slice(Uuid::new_v4().as_bytes());
            pk[16..].copy_from_slice(Uuid::new_v4().as_bytes());
            OneTimePreKeyDto {
                key_id: id,
                public_key: B64.encode(pk),
            }
        })
        .collect();

    PublishPreKeysRequest {
        device_id: device_id.to_string(),
        device_name: Some("Test Desktop".to_string()),
        identity_key: B64.encode(ik_pub),
        identity_signing_key: B64.encode(verifying_key.to_bytes()),
        signed_prekey: SignedPreKeyDto {
            key_id: spk_id,
            public_key: B64.encode(spk_pub),
            signature: B64.encode(sig.to_bytes()),
            created_at_ms: 1_700_000_000_000,
        },
        one_time_prekeys: otpks,
    }
}

#[tokio::test]
async fn test_server_e2ee_prekeys_atomic_otpk_and_zero_knowledge_guards() {
    let dir = TempDirGuard::new("m2_e2ee_srv");
    let (state, app) = open_app(&dir);

    let (alice_id, alice_tok) = reg(&app, "e2ee_alice").await;
    let (bob_id, bob_tok) = reg(&app, "e2ee_bob").await;

    // 1. Invalid Ed25519 SPK signature must be rejected with 400
    let mut bad_req = build_valid_prekey_req("bob_dev_1", 1, &[10]);
    bad_req.signed_prekey.signature = B64.encode([0xAAu8; 64]);
    let (st_bad, _) = api(
        &app,
        Method::POST,
        "/api/v1/e2ee/prekeys",
        Some(&bob_tok),
        Some(serde_json::to_value(&bad_req).unwrap()),
    )
    .await;
    assert_eq!(st_bad, StatusCode::BAD_REQUEST);

    // 2. Valid prekey upload with 2 OTPKs succeeds
    let good_req = build_valid_prekey_req("bob_dev_1", 1, &[10, 11]);
    let (st_ok, pub_resp) = api(
        &app,
        Method::POST,
        "/api/v1/e2ee/prekeys",
        Some(&bob_tok),
        Some(serde_json::to_value(&good_req).unwrap()),
    )
    .await;
    assert_eq!(st_ok, StatusCode::OK);
    assert_eq!(pub_resp["remaining_one_time_prekeys"].as_u64(), Some(2));

    // Count confirms 2 OTPKs
    let (st_cnt, cnt_resp) = api(
        &app,
        Method::GET,
        "/api/v1/e2ee/prekeys/count?device_id=bob_dev_1",
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(st_cnt, StatusCode::OK);
    assert_eq!(cnt_resp["remaining_one_time_prekeys"].as_u64(), Some(2));

    // 3. Atomic OTPK consumption across both route aliases (`/devices/{uid}/prekeys` and `/prekeys/{uid}`)
    let (st_f1, v1) = api(
        &app,
        Method::GET,
        &format!("/api/v1/e2ee/devices/{bob_id}/prekeys"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_f1, StatusCode::OK);
    let b1: UserPreKeyBundlesResponse = serde_json::from_value(v1).unwrap();
    let otpk1 = b1.devices[0].one_time_prekey.as_ref().unwrap().key_id;

    let (st_f2, v2) = api(
        &app,
        Method::GET,
        &format!("/api/v1/e2ee/prekeys/{bob_id}"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_f2, StatusCode::OK);
    let b2: UserPreKeyBundlesResponse = serde_json::from_value(v2).unwrap();
    let otpk2 = b2.devices[0].one_time_prekey.as_ref().unwrap().key_id;
    assert_ne!(otpk1, otpk2, "single-use OTPK must never be double-spent");

    // Third fetch falls back cleanly to SPK-only (`one_time_prekey: null`)
    let (st_f3, v3) = api(
        &app,
        Method::GET,
        &format!("/api/v1/e2ee/devices/{bob_id}/prekeys"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_f3, StatusCode::OK);
    let b3: UserPreKeyBundlesResponse = serde_json::from_value(v3).unwrap();
    assert!(b3.devices[0].one_time_prekey.is_none());

    // 4. Create DM chat + verify Zero-Knowledge Tantivy guard + send_as + ttl_seconds
    let (st_chat, chat_v) = api(
        &app,
        Method::POST,
        "/api/v1/chats",
        Some(&alice_tok),
        Some(json!({
            "title": "Secret E2EE",
            "chat_type": 0,
            "member_ids": [bob_id]
        })),
    )
    .await;
    assert_eq!(st_chat, StatusCode::CREATED);
    let chat_id = chat_v["chat_id"].as_i64().unwrap();

    // Check send_as endpoint returns caller identity
    let (st_sa, sa_v) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{chat_id}/send_as"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_sa, StatusCode::OK);
    let peers = sa_v["peers"].as_array().unwrap();
    assert!(!peers.is_empty());
    assert_eq!(peers[0]["peer_id"].as_u64(), Some(alice_id));

    // Send MessageContent::Encrypted -> server must accept, store, echo send_as_peer_id, and NOT Tantivy-index
    let env = EncryptedEnvelope {
        version: 1,
        sender_user_id: alice_id,
        sender_device_id: "alice_dev_1".to_string(),
        sender_identity_key: B64.encode([1u8; 32]),
        sender_signing_key: B64.encode([4u8; 32]),
        kind: EncryptedPayloadKind::PairwiseFanout {
            recipients: vec![PerDeviceCiphertext {
                recipient_user_id: bob_id,
                recipient_device_id: "bob_dev_1".to_string(),
                used_signed_prekey_id: Some(1),
                used_one_time_prekey_id: Some(otpk1),
                ephemeral_key: Some(B64.encode([2u8; 32])),
                header: RatchetHeaderDto {
                    dh_pub: B64.encode([3u8; 32]),
                    prev_chain_len: 0,
                    msg_num: 0,
                },
                ciphertext: B64.encode(b"UNIQUE_ZERO_KNOWLEDGE_BLOB_TOKEN"),
            }],
        },
    };

    // Configure chat TTL so server computes ttl_seconds & expires_at_ms
    let (st_ttl, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_id}/ttl"),
        Some(&alice_tok),
        Some(json!({ "ttl_seconds": 30 })),
    )
    .await;
    assert_eq!(st_ttl, StatusCode::OK);

    let content_val = serde_json::to_value(
        gpui_telegram_proto::message::MessageContent::Encrypted(env),
    )
    .unwrap();
    let (st_msg, msg_v) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&alice_tok),
        Some(json!({
            "text": "UNIQUE_ZERO_KNOWLEDGE_BLOB_TOKEN",
            "content": content_val,
            "send_as_peer_id": alice_id
        })),
    )
    .await;
    assert_eq!(st_msg, StatusCode::CREATED);
    assert!(msg_v["message_id"].as_i64().unwrap() > 0);
    assert_eq!(msg_v["send_as_peer_id"].as_u64(), Some(alice_id));
    assert_eq!(msg_v["ttl_seconds"].as_u64(), Some(30));
    assert!(msg_v["expires_at_ms"].as_i64().is_some());

    // Search for `UNIQUE_ZERO_KNOWLEDGE_BLOB_TOKEN` -> zero hits (Tantivy skipped!)
    let (st_search, search_v) = api(
        &app,
        Method::GET,
        "/api/v1/search?q=UNIQUE_ZERO_KNOWLEDGE_BLOB_TOKEN",
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_search, StatusCode::OK);
    let hits = search_v["hits"].as_array().map(|a| a.len()).unwrap_or(0);
    assert_eq!(hits, 0, "Encrypted message must never be indexed in Tantivy");

    // Fetch history and verify ttl_seconds + expires_at_ms echoed
    let (st_hist, hist_v) = api(
        &app,
        Method::GET,
        &format!("/api/v1/chats/{chat_id}/messages?limit=10"),
        Some(&bob_tok),
        None,
    )
    .await;
    assert_eq!(st_hist, StatusCode::OK);
    let msgs = hist_v["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["ttl_seconds"].as_u64(), Some(30));
    assert!(msgs[0]["expires_at_ms"].as_i64().is_some());

    // 5. Reject EncryptedEnvelope when sender_user_id != authenticated user (403 Forbidden)
    let spoofed_env = EncryptedEnvelope {
        version: 1,
        sender_user_id: alice_id,
        sender_device_id: "bob_dev_spoof".to_string(),
        sender_identity_key: B64.encode([1u8; 32]),
        sender_signing_key: B64.encode([4u8; 32]),
        kind: EncryptedPayloadKind::PairwiseFanout {
            recipients: vec![PerDeviceCiphertext {
                recipient_user_id: bob_id,
                recipient_device_id: "bob_dev_1".to_string(),
                used_signed_prekey_id: Some(1),
                used_one_time_prekey_id: None,
                ephemeral_key: Some(B64.encode([2u8; 32])),
                header: RatchetHeaderDto {
                    dh_pub: B64.encode([3u8; 32]),
                    prev_chain_len: 0,
                    msg_num: 0,
                },
                ciphertext: B64.encode(b"SPOOFED_SENDER"),
            }],
        },
    };
    let spoof_content = serde_json::to_value(
        gpui_telegram_proto::message::MessageContent::Encrypted(spoofed_env.clone()),
    )
    .unwrap();
    let (st_spoof, _) = api(
        &app,
        Method::POST,
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&bob_tok),
        Some(json!({
            "content": spoof_content,
        })),
    )
    .await;
    assert_eq!(
        st_spoof,
        StatusCode::FORBIDDEN,
        "Server must reject EncryptedEnvelope whose sender_user_id != auth_user"
    );

    // 6. WebSocket SendMessage must also reject EncryptedEnvelope when sender_user_id != auth_user
    let (ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel();
    let ws_req = gpui_telegram_proto::ws::SendMessageRequest::new(
        gpui_telegram_core::types::ChatId(chat_id),
        999_888,
        gpui_telegram_proto::message::MessageContent::Encrypted(spoofed_env),
    );
    state.gateway_hub.handle_client_frame(
        gpui_telegram_core::types::UserId(bob_id),
        gpui_telegram_proto::ws::WsClientFrame::SendMessage(ws_req),
        &ws_tx,
    );
    let ws_reply = ws_rx.try_recv().expect("WS hub must reply with 403 error frame");
    assert!(
        matches!(ws_reply, gpui_telegram_proto::ws::WsServerFrame::Error { code: 403, .. }),
        "Expected WS Error 403 on sender_user_id mismatch, got {:?}",
        ws_reply
    );
}
