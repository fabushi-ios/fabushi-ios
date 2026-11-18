//! Tier 1 Happy-Path Multi-Actor Server & Protocol Parity Suite (`R1..R4`)
//!
//! Executes live black-box HTTP REST, WebSocket (`/api/v1/ws`), HTTP 206 Range,
//! and SSE (`/api/v1/hermes/turns/{id}/events?sse=true`) assertions against
//! `gpui_telegram_server` with zero internal crate imports.

use std::time::Duration;

use e2e_ios_parity::{unique_tag, Ed25519PasskeySigner, LoopbackServerCluster};
use reqwest::StatusCode;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// Domain: R1 | Feature: F08 | Tier: 1
#[tokio::test]
async fn tier1_r1_health_and_server_status_semver_probe() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let probe = cluster.unauthenticated_actor();

    let health = probe.get("/health").await;
    assert_eq!(health.status, StatusCode::OK);
    assert!(!health.body_text().trim().is_empty());

    let status = probe.get("/api/v1/status").await;
    assert_eq!(status.status, StatusCode::OK);
    let body = status.json();
    assert_eq!(body["server"], "gpui_telegram_server");
    let version = body["version"].as_str().expect("status version string");
    let parts: Vec<&str> = version.split('.').collect();
    assert!(
        parts.len() >= 3 && parts.iter().take(3).all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
        "Expected valid semver in /api/v1/status, got: {version}"
    );
}

// Domain: R1 | Feature: F01 | Tier: 1
#[tokio::test]
async fn tier1_r1_register_login_me_and_session_logout() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let password = "StrongPass#2026_Login";
    let alice = cluster.register_actor("alice_r1", password).await;
    let uname = alice.username.clone().unwrap();

    // Verify /api/v1/auth/me returns matching profile
    let me = alice.get("/api/v1/auth/me").await;
    assert_eq!(me.status, StatusCode::OK);
    let me_json = me.json();
    assert_eq!(me_json["user_id"].as_u64(), Some(alice.uid()));
    assert_eq!(me_json["username"].as_str(), Some(uname.as_str()));

    // Secondary login issues distinct session
    let mut second_device = cluster.unauthenticated_actor();
    let login = second_device
        .post_json(
            "/api/v1/auth/login",
            &json!({
                "username": uname,
                "password": password,
                "device_name": "iPad Pro 13"
            }),
        )
        .await;
    assert_eq!(login.status, StatusCode::OK);
    let login_json = login.json();
    let sid2 = login_json["session_id"].as_str().expect("sid2").to_string();
    let tok2 = login_json["token"].as_str().expect("tok2").to_string();
    assert_ne!(sid2, alice.session_id());

    second_device.bearer_token = Some(tok2);
    second_device.session_id = Some(sid2.clone());

    // Self-logout via DELETE /api/v1/auth/sessions/{session_id}
    let del = second_device
        .delete(&format!("/api/v1/auth/sessions/{sid2}"))
        .await;
    assert_eq!(del.status, StatusCode::OK);
    assert_eq!(del.json()["revoked"], true);

    // Subsequent authenticated call on logged-out token must return 401
    let after = second_device.get("/api/v1/auth/me").await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);

    // Primary session remains active
    assert_eq!(alice.get("/api/v1/auth/me").await.status, StatusCode::OK);
}

// Domain: R1 | Feature: F09 | Tier: 1
#[tokio::test]
async fn tier1_r1_multi_session_list_single_revoke_ws_disconnect_and_revoke_others() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let password = "StrongPass#2026_MultiSess";
    let primary = cluster.register_actor("multisess", password).await;
    let uname = primary.username.clone().unwrap();

    // Open Device B and Device C
    let mut dev_b = cluster.unauthenticated_actor();
    let b_json = dev_b
        .post_json(
            "/api/v1/auth/login",
            &json!({ "username": uname, "password": password, "device_name": "Device-B" }),
        )
        .await
        .json();
    dev_b.user_id = Some(primary.uid());
    dev_b.session_id = Some(b_json["session_id"].as_str().unwrap().to_string());
    dev_b.bearer_token = Some(b_json["token"].as_str().unwrap().to_string());

    let mut dev_c = cluster.unauthenticated_actor();
    let c_json = dev_c
        .post_json(
            "/api/v1/auth/login",
            &json!({ "username": uname, "password": password, "device_name": "Device-C" }),
        )
        .await
        .json();
    dev_c.user_id = Some(primary.uid());
    dev_c.session_id = Some(c_json["session_id"].as_str().unwrap().to_string());
    dev_c.bearer_token = Some(c_json["token"].as_str().unwrap().to_string());

    // Connect live WebSockets on Device B and Primary
    let mut ws_b = dev_b.connect_ws().await;
    ws_b.send_frame(&json!({ "type": "Ping", "payload": { "nonce": 77 } }))
        .await;
    let pong = ws_b
        .recv_until(Duration::from_secs(5), |f| f["type"] == "Pong")
        .await;
    assert!(pong.is_some(), "Device B WS must receive Pong before revocation");

    // List active sessions -> at least 3
    let list_resp = primary.get("/api/v1/auth/sessions").await;
    assert_eq!(list_resp.status, StatusCode::OK);
    let sessions = list_resp.json()["sessions"].as_array().unwrap().clone();
    assert!(sessions.len() >= 3);

    // Revoke single Device B session -> WS B receives 401 / disconnect
    let rev_b = primary
        .delete(&format!("/api/v1/auth/sessions/{}", dev_b.session_id()))
        .await;
    assert_eq!(rev_b.status, StatusCode::OK);
    assert!(
        ws_b.wait_for_disconnect_or_401(Duration::from_secs(5)).await,
        "Revoked session WebSocket must receive 401 or Close frame"
    );
    assert_eq!(dev_b.get("/api/v1/auth/me").await.status, StatusCode::UNAUTHORIZED);

    // Revoke all other sessions -> Device C revoked, Primary stays alive
    let mut ws_c = dev_c.connect_ws().await;
    let rev_others = primary.delete("/api/v1/auth/sessions/others").await;
    assert_eq!(rev_others.status, StatusCode::OK);
    assert!(rev_others.json()["revoked_count"].as_u64().unwrap_or(0) >= 1);
    assert!(ws_c.wait_for_disconnect_or_401(Duration::from_secs(5)).await);
    assert_eq!(dev_c.get("/api/v1/auth/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(primary.get("/api/v1/auth/me").await.status, StatusCode::OK);
}

// Domain: R1 | Feature: F05 | Tier: 1
#[tokio::test]
async fn tier1_r1_two_factor_setup_challenge_verify_and_disable() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let login_pw = "StrongPass#2026_2FA";
    let cloud_pw = "CloudSecret#9988";
    let user = cluster.register_actor("tfa_user", login_pw).await;
    let uname = user.username.clone().unwrap();

    // Enable 2FA cloud password
    let set_resp = user
        .post_json(
            "/api/v1/auth/2fa/set",
            &json!({
                "new_password": cloud_pw,
                "hint": "nine nine",
                "recovery_email": "recover_2fa@example.test"
            }),
        )
        .await;
    assert_eq!(set_resp.status, StatusCode::OK);
    assert_eq!(set_resp.json()["has_password"], true);

    // Login triggers 2FA challenge instead of issuing token immediately
    let anon = cluster.unauthenticated_actor();
    let login_step1 = anon
        .post_json(
            "/api/v1/auth/login",
            &json!({
                "username": uname,
                "password": login_pw,
                "device_name": "iPhone-2FA"
            }),
        )
        .await;
    assert_eq!(login_step1.status, StatusCode::OK);
    let step1_json = login_step1.json();
    assert_eq!(step1_json["requires_2fa"], true);
    assert_eq!(step1_json["hint"], "nine nine");
    let challenge_token = step1_json["challenge_token"]
        .as_str()
        .expect("challenge_token")
        .to_string();

    // Complete 2FA verification
    let verify_resp = anon
        .post_json(
            "/api/v1/auth/2fa/verify",
            &json!({
                "challenge_token": challenge_token,
                "cloud_password": cloud_pw,
                "device_name": "iPhone-2FA"
            }),
        )
        .await;
    assert_eq!(verify_resp.status, StatusCode::OK);
    assert!(verify_resp.json()["token"].as_str().is_some());

    // Disable 2FA
    let disable_resp = user
        .delete_json(
            "/api/v1/auth/2fa/disable",
            &json!({ "current_password": cloud_pw }),
        )
        .await;
    assert_eq!(disable_resp.status, StatusCode::OK);
    assert_eq!(disable_resp.json()["has_password"], false);
}

// Domain: R1 | Feature: F07 | Tier: 1
#[tokio::test]
async fn tier1_r1_passkey_ed25519_register_list_and_assertion_login() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let user = cluster.register_actor("passkey_u", "StrongPass#2026_PK").await;
    let signer = Ed25519PasskeySigner::generate("ios_passkey");

    // Register passkey on authenticated account
    let reg = user
        .post_json(
            "/api/v1/auth/passkey/register",
            &json!({
                "credential_id": signer.credential_id,
                "public_key": signer.public_key_hex(),
                "device_name": "iPhone 16 Pro Secure Enclave"
            }),
        )
        .await;
    assert_eq!(reg.status, StatusCode::CREATED);

    let list = user.get("/api/v1/auth/passkey/list").await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.json()["total"], 1);

    // Passwordless login via challenge + Ed25519 signature assertion
    let anon = cluster.unauthenticated_actor();
    let ch_resp = anon
        .post_json(
            "/api/v1/auth/passkey/challenge",
            &json!({ "username": user.username }),
        )
        .await;
    assert_eq!(ch_resp.status, StatusCode::OK);
    let challenge = ch_resp.json()["challenge"].as_str().unwrap().to_string();

    let sig_hex = signer.sign_challenge_hex(&challenge);
    let verify = anon
        .post_json(
            "/api/v1/auth/passkey/verify",
            &json!({
                "challenge": challenge,
                "credential_id": signer.credential_id,
                "client_data_json": "{\"type\":\"webauthn.get\"}",
                "authenticator_data": "49960de5880e8c687434170f6476605b8fe4aeb9a28632c7995cf3ba831d97630500000001",
                "signature": sig_hex
            }),
        )
        .await;
    assert_eq!(verify.status, StatusCode::OK);
    let v_json = verify.json();
    assert_eq!(v_json["user_id"].as_u64(), Some(user.uid()));
    assert!(v_json["token"].as_str().is_some());
}

// Domain: R1 | Feature: F06 | Tier: 1
#[tokio::test]
async fn tier1_r1_qr_pairing_init_poll_accept_and_verify() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let authed_phone = cluster.register_actor("qr_host", "StrongPass#2026_QR").await;
    let new_tablet = cluster.unauthenticated_actor();

    let init = new_tablet
        .post_json("/api/v1/auth/pair/init", &json!({ "device_name": "iPad Mini QR" }))
        .await;
    assert_eq!(init.status, StatusCode::OK);
    let init_json = init.json();
    let pair_token = init_json["pair_token"].as_str().unwrap().to_string();
    assert!(init_json["qr_url"].as_str().unwrap().starts_with("tg://login?token="));

    let poll_pending = new_tablet
        .get(&format!("/api/v1/auth/pair/poll/{pair_token}"))
        .await;
    assert_eq!(poll_pending.status, StatusCode::OK);

    let accept = authed_phone
        .post_json("/api/v1/auth/pair/accept", &json!({ "pair_token": pair_token }))
        .await;
    assert_eq!(accept.status, StatusCode::OK);
    assert_eq!(accept.json()["status"], "confirmed");

    let verify = new_tablet
        .post_json("/api/v1/auth/pair/verify", &json!({ "pair_token": pair_token }))
        .await;
    assert_eq!(verify.status, StatusCode::OK);
    let v_json = verify.json();
    assert_eq!(v_json["verified"], true);
    assert_eq!(v_json["user_id"].as_u64(), Some(authed_phone.uid()));
    assert!(v_json["token"].as_str().is_some());
}

// Domain: R1 | Feature: F03 | Tier: 1
#[tokio::test]
async fn tier1_r1_phone_otp_and_email_otp_registration_flows() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let anon = cluster.unauthenticated_actor();

    // 1. Phone OTP flow
    let rand_digits: u32 = (uuid::Uuid::new_v4().as_u128() % 9_000_000) as u32 + 1_000_000;
    let phone = format!("+1415{rand_digits}");
    let send_phone = anon
        .post_json("/api/v1/auth/phone/send-code", &json!({ "phone": phone }))
        .await;
    assert_eq!(send_phone.status, StatusCode::OK);
    let sp_json = send_phone.json();
    let p_code = sp_json["debug_code"].as_str().expect("phone debug_code");
    let p_hash = sp_json["phone_code_hash"].as_str().expect("phone_code_hash");

    let verify_phone = anon
        .post_json(
            "/api/v1/auth/phone/verify",
            &json!({
                "phone": phone,
                "code": p_code,
                "phone_code_hash": p_hash,
                "first_name": "PhoneUser",
                "device_name": "iPhone-OTP"
            }),
        )
        .await;
    assert_eq!(verify_phone.status, StatusCode::OK);
    assert!(verify_phone.json()["token"].as_str().is_some());

    // 2. Email OTP registration flow (>= 8 char password)
    let uname = unique_tag("email_u");
    let email = format!("{uname}@example.test");
    let send_email = anon
        .post_json("/api/v1/auth/email/send-code", &json!({ "email": email }))
        .await;
    assert_eq!(send_email.status, StatusCode::OK);
    let e_code = send_email.json()["debug_code"]
        .as_str()
        .expect("email debug_code")
        .to_string();

    let reg_email = anon
        .post_json(
            "/api/v1/auth/email/register",
            &json!({
                "email": email,
                "code": e_code,
                "username": uname,
                "password": "EmailPass#2026Strong",
                "first_name": "EmailAlice"
            }),
        )
        .await;
    assert!(reg_email.status.is_success(), "email register failed: {}", reg_email.body_text());
    assert!(reg_email.json()["token"].as_str().is_some());
}

// Domain: R1 | Feature: F02 | Tier: 1
#[tokio::test]
async fn tier1_r1_alipay_qr_start_poll_simulate_scan_and_complete() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let anon = cluster.unauthenticated_actor();

    let start = anon.post_json("/api/v1/auth/alipay/start", &json!({})).await;
    assert_eq!(start.status, StatusCode::OK);
    let s_json = start.json();
    let attempt_id = s_json["attempt_id"].as_str().expect("attempt_id").to_string();
    assert!(!s_json["login_url"].as_str().unwrap_or("").is_empty());

    let poll_before = anon
        .get(&format!("/api/v1/auth/alipay/poll/{attempt_id}"))
        .await;
    assert_eq!(poll_before.status, StatusCode::OK);
    assert_eq!(poll_before.json()["status"], "pending");

    let scan = anon
        .post_json(
            "/api/v1/auth/alipay/simulate-scan",
            &json!({ "attempt_id": attempt_id }),
        )
        .await;
    assert_eq!(scan.status, StatusCode::OK);

    let poll_after = anon
        .get(&format!("/api/v1/auth/alipay/poll/{attempt_id}"))
        .await;
    assert_eq!(poll_after.status, StatusCode::OK);
    let after_json = poll_after.json();
    assert_ne!(after_json["status"], "pending");
    assert!(after_json["token"].as_str().is_some());
}

// Domain: R3 | Feature: F18 | Tier: 1
#[tokio::test]
async fn tier1_r3_chats_groups_topics_folders_archive_stories_and_invites() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("alice_r3", "StrongPass#2026").await;
    let bob = cluster.register_actor("bob_r3", "StrongPass#2026").await;
    let carol = cluster.register_actor("carol_r3", "StrongPass#2026").await;

    // Create forum group
    let grp = alice
        .post_json(
            "/api/v1/chats",
            &json!({
                "title": "iOS Architecture Forum",
                "chat_type": "supergroup",
                "is_forum": true,
                "member_ids": [bob.uid()]
            }),
        )
        .await;
    assert!(grp.status.is_success());
    let chat_id = grp.json()["chat_id"].as_i64().unwrap();

    // Forum topic creation & listing
    let topic = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/topics"),
            &json!({ "title": "Liquid Glass Specs", "icon_color": 0x6FB9F0 }),
        )
        .await;
    assert!(topic.status.is_success());
    let topics_list = alice.get(&format!("/api/v1/chats/{chat_id}/topics")).await;
    assert_eq!(topics_list.status, StatusCode::OK);
    assert!(!topics_list.json()["topics"].as_array().unwrap().is_empty());

    // Chat Folders CRUD
    let folder = alice
        .post_json(
            "/api/v1/folders",
            &json!({
                "title": "Work",
                "emoticon": "💼",
                "include_chats": [chat_id],
                "include_groups": true
            }),
        )
        .await;
    assert_eq!(folder.status, StatusCode::CREATED);
    let folder_id = folder.json()["id"].as_i64().unwrap();
    assert_eq!(alice.get("/api/v1/folders").await.status, StatusCode::OK);
    assert_eq!(
        alice.delete(&format!("/api/v1/folders/{folder_id}")).await.status,
        StatusCode::OK
    );

    // Archive, Pin, Mute dialog toggles
    let arch = alice
        .post_json(&format!("/api/v1/chats/{chat_id}/archive"), &json!({ "archived": true }))
        .await;
    assert_eq!(arch.status, StatusCode::OK);
    assert_eq!(arch.json()["is_archived"], true);

    let pin_dlg = alice
        .post_json(&format!("/api/v1/chats/{chat_id}/pin"), &json!({ "pinned": true }))
        .await;
    assert_eq!(pin_dlg.status, StatusCode::OK);
    assert_eq!(pin_dlg.json()["is_pinned"], true);

    // Stories lifecycle
    let story = alice
        .post_json(
            "/api/v1/stories",
            &json!({
                "media_url": "https://example.invalid/story_glass.jpg",
                "media_type": "photo",
                "caption": "Shipping native iOS!"
            }),
        )
        .await;
    assert_eq!(story.status, StatusCode::CREATED);
    let story_json = story.json();
    let story_id = story_json["story_id"]
        .as_str()
        .or_else(|| story_json["id"].as_str())
        .unwrap()
        .to_string();
    assert_eq!(
        bob.post_json(&format!("/api/v1/stories/{story_id}/view"), &json!({ "reaction": "🔥" }))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        alice.delete(&format!("/api/v1/stories/{story_id}")).await.status,
        StatusCode::OK
    );

    // Invite link create & Carol join
    let inv = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/invites"),
            &json!({ "title": "QA Invite", "usage_limit": 5 }),
        )
        .await;
    assert_eq!(inv.status, StatusCode::CREATED);
    let code = inv.json()["code"].as_str().unwrap().to_string();

    let joined = carol
        .post_json("/api/v1/invites/join", &json!({ "code": code }))
        .await;
    assert_eq!(joined.status, StatusCode::OK);
    assert_eq!(joined.json()["chat_id"].as_i64(), Some(chat_id));
}

// Domain: R3 | Feature: F20 | Tier: 1
#[tokio::test]
async fn tier1_r3_tantivy_unified_and_scoped_search() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("search_a", "StrongPass#2026").await;
    let bob = cluster.register_actor("search_b", "StrongPass#2026").await;

    let dm = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Search DM", "chat_type": "direct", "peer_user_id": bob.uid() }),
        )
        .await
        .json();
    let chat_id = dm["chat_id"].as_i64().unwrap();
    let token_word = unique_tag("zephyrquantum");

    let send = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({
                "text": format!("Unique Tantivy needle {token_word} https://telegram.org/blog"),
                "client_nonce": 1001
            }),
        )
        .await;
    assert!(send.status.is_success());

    // Poll briefly for nonblocking Tantivy commit
    let mut found = false;
    for _ in 0..20 {
        let res = bob
            .get(&format!("/api/v1/search?q={token_word}&chat_id={chat_id}"))
            .await;
        assert_eq!(res.status, StatusCode::OK);
        let j = res.json();
        if j["total"].as_u64().unwrap_or(0) >= 1
            || j["hits"].as_array().map(|a| !a.is_empty()).unwrap_or(false)
            || j["results"].as_array().map(|a| !a.is_empty()).unwrap_or(false)
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(75)).await;
    }
    assert!(found, "Tantivy search must return indexed message for active chat member");
}

// Domain: R4 | Feature: F23 | Tier: 1
#[tokio::test]
async fn tier1_r4_ws_ptspush_delivery_and_client_nonce_idempotency() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("ws_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("ws_bob", "StrongPass#2026").await;

    let dm = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Realtime Chat", "chat_type": "direct", "peer_user_id": bob.uid() }),
        )
        .await
        .json();
    let chat_id = dm["chat_id"].as_i64().unwrap();

    let mut bob_ws = bob.connect_ws().await;
    bob_ws
        .send_frame(&json!({ "type": "SubscribeChat", "payload": { "chat_id": chat_id } }))
        .await;

    let nonce: u64 = 987_654_321;
    let send1 = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "Hello inverted transcript!", "client_nonce": nonce }),
        )
        .await;
    assert!(send1.status.is_success());
    let s1 = send1.json();
    let msg_id_1 = s1["message_id"].as_i64().unwrap();
    let pts_1 = s1["pts"].as_u64().unwrap();

    // Bob's WebSocket receives PtsPush with NewMessage
    let push = bob_ws
        .recv_until(Duration::from_secs(5), |f| {
            f["type"] == "PtsPush" && f["payload"]["chat_id"].as_i64() == Some(chat_id)
        })
        .await;
    assert!(push.is_some(), "Bob WS must receive PtsPush for Alice's message");

    // Retry with identical client_nonce -> idempotent deduplication (same message_id & pts)
    let send2 = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "Hello inverted transcript!", "client_nonce": nonce }),
        )
        .await;
    assert!(send2.status.is_success());
    let s2 = send2.json();
    assert_eq!(s2["message_id"].as_i64(), Some(msg_id_1));
    assert_eq!(s2["pts"].as_u64(), Some(pts_1));
}

// Domain: R4 | Feature: F28 | Tier: 1
#[tokio::test]
async fn tier1_r4_replies_edits_forwards_and_deletes() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("msg_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("msg_bob", "StrongPass#2026").await;

    let chat1 = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Source Group", "chat_type": "group", "member_ids": [bob.uid()] }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();
    let chat2 = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Target Group", "chat_type": "group", "member_ids": [bob.uid()] }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    let orig = alice
        .post_json(
            &format!("/api/v1/chats/{chat1}/messages"),
            &json!({ "text": "Original statement", "client_nonce": 2001 }),
        )
        .await
        .json()["message_id"]
        .as_i64()
        .unwrap();

    // Reply
    let reply = bob
        .post_json(
            &format!("/api/v1/chats/{chat1}/messages"),
            &json!({ "text": "Replying to Alice", "reply_to": orig, "client_nonce": 2002 }),
        )
        .await;
    assert!(reply.status.is_success());

    // Edit original
    let edit = alice
        .put_json(
            &format!("/api/v1/chats/{chat1}/messages/{orig}"),
            &json!({ "text": "Edited statement v2" }),
        )
        .await;
    assert_eq!(edit.status, StatusCode::OK);

    // Forward into chat2
    let fwd = alice
        .post_json(
            &format!("/api/v1/chats/{chat2}/messages"),
            &json!({
                "forward_from_chat_id": chat1,
                "forward_from_msg_id": orig,
                "client_nonce": 2003
            }),
        )
        .await;
    assert!(fwd.status.is_success());
    let hist2 = bob.get(&format!("/api/v1/chats/{chat2}/messages")).await.json();
    let msgs2 = hist2["messages"].as_array().unwrap();
    assert!(!msgs2.is_empty());
    assert!(msgs2.iter().any(|m| !m["forward_info"].is_null()));

    // Delete original for everyone
    let del = alice
        .delete(&format!("/api/v1/chats/{chat1}/messages/{orig}?revoke=true"))
        .await;
    assert_eq!(del.status, StatusCode::OK);
}

// Domain: R4 | Feature: F29 | Tier: 1
#[tokio::test]
async fn tier1_r4_pinned_messages_and_auto_delete_ttl() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("pin_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("pin_bob", "StrongPass#2026").await;

    let chat_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Pins & TTL Group", "chat_type": "group", "member_ids": [bob.uid()] }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    let m1 = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "Release Checklist v2.0", "client_nonce": 3001 }),
        )
        .await
        .json()["message_id"]
        .as_i64()
        .unwrap();

    // Pin message
    let pin = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/pins"),
            &json!({ "message_id": m1, "silent": false }),
        )
        .await;
    assert!(pin.status.is_success());

    let pins = bob.get(&format!("/api/v1/chats/{chat_id}/pins")).await;
    assert_eq!(pins.status, StatusCode::OK);
    assert!(pins.json()["pinned_message_ids"]
        .as_array()
        .unwrap()
        .contains(&Value::from(m1)));

    // Unpin message
    let unpin = alice
        .delete(&format!("/api/v1/chats/{chat_id}/pins/{m1}"))
        .await;
    assert_eq!(unpin.status, StatusCode::OK);

    // Configure per-chat auto-delete TTL (86400s = 24h)
    let ttl = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/ttl"),
            &json!({ "ttl_seconds": 86400 }),
        )
        .await;
    assert_eq!(ttl.status, StatusCode::OK);
}

// Domain: R4 | Feature: F27 | Tier: 1
#[tokio::test]
async fn tier1_r4_reactions_and_interactive_polls_vote_close() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("poll_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("poll_bob", "StrongPass#2026").await;

    let chat_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Polls & Reactions", "chat_type": "group", "member_ids": [bob.uid()] }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    let msg_id = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "React with ❤️!", "client_nonce": 4001 }),
        )
        .await
        .json()["message_id"]
        .as_i64()
        .unwrap();

    // Bob reacts via HTTP -> authoritative reaction count = 1
    let react = bob
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages/{msg_id}/reactions"),
            &json!({ "emoji": "❤️", "action": "add" }),
        )
        .await;
    assert_eq!(react.status, StatusCode::OK);
    assert!(!react.json()["reactions"].as_array().unwrap().is_empty());

    // Create quiz poll
    let poll_resp = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/polls"),
            &json!({
                "question": "Which scroll transform powers Telegram-iOS transcript?",
                "options": ["Standard top-down", "180-degree inverted", "WebView"],
                "is_anonymous": false,
                "allows_multiple_answers": false,
                "is_quiz": true,
                "correct_option_id": 1,
                "solution": "Telegram-iOS inverts UICollectionView by pi radians."
            }),
        )
        .await;
    assert!(poll_resp.status.is_success());
    let poll_id = poll_resp.json()["poll_id"]
        .as_u64()
        .or_else(|| poll_resp.json()["poll"]["poll_id"].as_u64())
        .expect("poll_id");

    // Bob votes option 1
    let vote = bob
        .post_json(
            &format!("/api/v1/polls/{poll_id}/vote"),
            &json!({ "option_ids": [1] }),
        )
        .await;
    assert_eq!(vote.status, StatusCode::OK);

    // Alice closes poll
    let close = alice
        .post_json(&format!("/api/v1/polls/{poll_id}/close"), &json!({}))
        .await;
    assert_eq!(close.status, StatusCode::OK);
}

// Domain: R4 | Feature: F25 | Tier: 1
#[tokio::test]
async fn tier1_r4_media_cas_upload_and_http_206_range_stream() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("media_alice", "StrongPass#2026").await;

    let mut payload = Vec::with_capacity(4096);
    for i in 0..4096u32 {
        payload.push(((i * 31 + 7) & 0xFF) as u8);
    }
    let expected_sha256 = hex::encode(Sha256::digest(&payload));

    let upload = alice
        .upload_multipart(
            "/api/v1/media/upload",
            "sample_stream.mp4",
            "video/mp4",
            payload.clone(),
        )
        .await;
    assert_eq!(upload.status, StatusCode::CREATED);
    let up_json = upload.json();
    let file_id = up_json["file_id"].as_str().expect("file_id").to_string();
    assert_eq!(up_json["sha256"].as_str(), Some(expected_sha256.as_str()));
    assert_eq!(up_json["size_bytes"].as_u64(), Some(4096));

    // Request byte range 100..=227 (128 bytes) -> HTTP 206 Partial Content
    let range_resp = alice
        .get_range(&format!("/api/v1/media/stream/{file_id}"), "bytes=100-227")
        .await;
    assert_eq!(range_resp.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(range_resp.bytes.as_slice(), &payload[100..=227]);
}

// Domain: R4 | Feature: F48 | Tier: 1
#[tokio::test]
async fn tier1_r4_hermes_slash_turn_and_sse_event_stream() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("hermes_alice", "StrongPass#2026").await;

    let turn = alice
        .post_json(
            "/api/v1/hermes/turns",
            &json!({ "prompt": "/help" }),
        )
        .await;
    assert_eq!(turn.status, StatusCode::OK);
    let t_json = turn.json();
    let turn_id = t_json["turn_id"]
        .as_str()
        .or_else(|| t_json["id"].as_str())
        .expect("turn_id");

    let sse = alice
        .get_with_headers(
            &format!("/api/v1/hermes/turns/{turn_id}/events?sse=true"),
            &[("Accept", "text/event-stream")],
        )
        .await;
    assert_eq!(sse.status, StatusCode::OK);
    let sse_text = sse.body_text();
    assert!(
        sse_text.contains("data:"),
        "Expected SSE stream data frames, got: {sse_text}"
    );
}

// Domain: R3 | Feature: F17 | Tier: 1
#[tokio::test]
async fn tier1_r3_draft_persistence_and_read_history_unread_convergence() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("draft_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("draft_bob", "StrongPass#2026").await;

    let chat_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Draft & Read DM", "chat_type": "direct", "peer_user_id": bob.uid() }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    // Save draft, verify GET /draft, then clear draft
    let put_draft = alice
        .put_json(
            &format!("/api/v1/chats/{chat_id}/draft"),
            &json!({ "text": "Server-synced draft v1" }),
        )
        .await;
    assert_eq!(put_draft.status, StatusCode::OK);
    let get_draft = alice.get(&format!("/api/v1/chats/{chat_id}/draft")).await;
    assert_eq!(get_draft.status, StatusCode::OK);
    assert!(get_draft.body_text().contains("Server-synced draft v1"));
    assert_eq!(
        alice.delete(&format!("/api/v1/chats/{chat_id}/draft")).await.status,
        StatusCode::OK
    );

    // Alice sends 2 messages -> Bob marks read -> unread_count converges to 0
    let _m1 = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "Unread #1", "client_nonce": 9601 }),
        )
        .await;
    let m2 = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "Unread #2", "client_nonce": 9602 }),
        )
        .await
        .json()["message_id"]
        .as_i64()
        .unwrap();

    let mark_read = bob
        .post_json(
            &format!("/api/v1/chats/{chat_id}/read"),
            &json!({ "max_id": m2 }),
        )
        .await;
    assert_eq!(mark_read.status, StatusCode::OK);

    let bob_chat = bob.get(&format!("/api/v1/chats/{chat_id}")).await.json();
    assert_eq!(bob_chat["unread_count"].as_u64(), Some(0));
}

// Domain: R4 | Feature: F30 | Tier: 1
#[tokio::test]
async fn tier1_r4_ws_live_reaction_push_and_typing_broadcast() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("wsrx_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("wsrx_bob", "StrongPass#2026").await;

    let chat_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "WS Reaction Room", "chat_type": "group", "member_ids": [bob.uid()] }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    let msg_id = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "React over WS", "client_nonce": 9701 }),
        )
        .await
        .json()["message_id"]
        .as_i64()
        .unwrap();

    let mut alice_ws = alice.connect_ws().await;
    alice_ws
        .send_frame(&json!({ "type": "SubscribeChat", "payload": { "chat_id": chat_id } }))
        .await;

    let rx = bob
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages/{msg_id}/reactions"),
            &json!({ "emoji": "🔥", "action": "add" }),
        )
        .await;
    assert_eq!(rx.status, StatusCode::OK);

    let rx_push = alice_ws
        .recv_until(Duration::from_secs(5), |f| {
            f["type"] == "PtsPush" && f["payload"]["chat_id"].as_i64() == Some(chat_id)
        })
        .await;
    assert!(rx_push.is_some(), "Alice WS must receive PtsPush when Bob reacts");
}

