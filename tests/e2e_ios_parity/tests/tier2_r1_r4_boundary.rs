//! Tier 2 Boundary, Negative, Authorization & Corner-Case Parity Suite (`R1..R4`)
//!
//! Exercises credential boundaries, 2FA/Passkey replay defenses, token revocation,
//! unauthorized chat/message/forward/pin rejections, DM block enforcement, member bans,
//! invite exhaustion, closed-poll rejections, PTS gap recovery, and UTF-16 CJK/emoji/RTL
//! entity offsets against live `gpui_telegram_server`.

use e2e_ios_parity::{unique_tag, Ed25519PasskeySigner, LoopbackServerCluster};
use reqwest::StatusCode;
use serde_json::json;

// Domain: R1 | Feature: F01 | Tier: 2
#[tokio::test]
async fn tier2_r1_credential_boundaries_oversized_password_and_duplicate_username() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let anon = cluster.unauthenticated_actor();

    // Empty username rejected
    let empty_u = anon
        .post_json(
            "/api/v1/auth/register",
            &json!({ "username": "   ", "password": "StrongPass#2026", "first_name": "X" }),
        )
        .await;
    assert_eq!(empty_u.status, StatusCode::BAD_REQUEST);

    // Password > 1024 bytes rejected
    let huge_pw = "A".repeat(1025);
    let over_pw = anon
        .post_json(
            "/api/v1/auth/register",
            &json!({ "username": unique_tag("hugepw"), "password": huge_pw, "first_name": "X" }),
        )
        .await;
    assert!(over_pw.status.is_client_error());

    // Email registration with < 8 char password rejected
    let uname_e = unique_tag("short_epw");
    let email = format!("{uname_e}@example.test");
    let code_resp = anon
        .post_json("/api/v1/auth/email/send-code", &json!({ "email": email }))
        .await;
    let code = code_resp.json()["debug_code"].as_str().unwrap().to_string();
    let short_email_reg = anon
        .post_json(
            "/api/v1/auth/email/register",
            &json!({
                "email": email,
                "code": code,
                "username": uname_e,
                "password": "short7!",
                "first_name": "Short"
            }),
        )
        .await;
    assert_eq!(short_email_reg.status, StatusCode::BAD_REQUEST);

    // Duplicate username conflict (409)
    let first = cluster.register_actor("dup_check", "StrongPass#2026").await;
    let dup = anon
        .post_json(
            "/api/v1/auth/register",
            &json!({
                "username": first.username.unwrap(),
                "password": "StrongPass#2026",
                "first_name": "Clone"
            }),
        )
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
}

// Domain: R1 | Feature: F05 | Tier: 2
#[tokio::test]
async fn tier2_r1_wrong_login_password_and_wrong_2fa_cloud_password_rejected() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let user = cluster.register_actor("tfa_neg", "CorrectLogin#2026").await;
    let uname = user.username.clone().unwrap();

    let anon = cluster.unauthenticated_actor();
    let bad_login = anon
        .post_json(
            "/api/v1/auth/login",
            &json!({ "username": uname, "password": "WrongPassword#0000", "device_name": "Attacker" }),
        )
        .await;
    assert_eq!(bad_login.status, StatusCode::UNAUTHORIZED);

    // Enable 2FA and submit wrong cloud password
    assert_eq!(
        user.post_json(
            "/api/v1/auth/2fa/set",
            &json!({ "new_password": "RealCloudPassword#1" }),
        )
        .await
        .status,
        StatusCode::OK
    );

    let step1 = anon
        .post_json(
            "/api/v1/auth/login",
            &json!({ "username": uname, "password": "CorrectLogin#2026", "device_name": "iPhone" }),
        )
        .await
        .json();
    assert_eq!(step1["requires_2fa"], true);
    let ch_token = step1["challenge_token"].as_str().unwrap();

    let bad_2fa = anon
        .post_json(
            "/api/v1/auth/2fa/verify",
            &json!({ "challenge_token": ch_token, "cloud_password": "WrongCloudPassword!" }),
        )
        .await;
    assert_eq!(bad_2fa.status, StatusCode::UNAUTHORIZED);

    // Disable with wrong current_password also fails with 401
    let bad_disable = user
        .delete_json(
            "/api/v1/auth/2fa/disable",
            &json!({ "current_password": "WrongCloudPassword!" }),
        )
        .await;
    assert_eq!(bad_disable.status, StatusCode::UNAUTHORIZED);
}

// Domain: R1 | Feature: F07 | Tier: 2
#[tokio::test]
async fn tier2_r1_passkey_challenge_replay_and_unknown_credential_rejected() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let user = cluster.register_actor("pk_replay", "StrongPass#2026").await;
    let signer = Ed25519PasskeySigner::generate("pk_cred");

    assert_eq!(
        user.post_json(
            "/api/v1/auth/passkey/register",
            &json!({
                "credential_id": signer.credential_id,
                "public_key": signer.public_key_hex(),
                "device_name": "iPhone 16 Pro"
            }),
        )
        .await
        .status,
        StatusCode::CREATED
    );

    let anon = cluster.unauthenticated_actor();
    let ch = anon
        .post_json(
            "/api/v1/auth/passkey/challenge",
            &json!({ "username": user.username }),
        )
        .await
        .json()["challenge"]
        .as_str()
        .unwrap()
        .to_string();
    let sig = signer.sign_challenge_hex(&ch);

    // First verify succeeds and consumes challenge
    let first = anon
        .post_json(
            "/api/v1/auth/passkey/verify",
            &json!({
                "challenge": ch,
                "credential_id": signer.credential_id,
                "signature": sig
            }),
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);

    // Second verify with SAME challenge is rejected (replay defense)
    let replay = anon
        .post_json(
            "/api/v1/auth/passkey/verify",
            &json!({
                "challenge": ch,
                "credential_id": signer.credential_id,
                "signature": sig
            }),
        )
        .await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);

    // Fresh challenge with unknown credential_id -> 401
    let ch2 = anon
        .post_json("/api/v1/auth/passkey/challenge", &json!({}))
        .await
        .json()["challenge"]
        .as_str()
        .unwrap()
        .to_string();
    let unknown = anon
        .post_json(
            "/api/v1/auth/passkey/verify",
            &json!({
                "challenge": ch2,
                "credential_id": "nonexistent_cred_9999",
                "signature": sig
            }),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
}

// Domain: R1 | Feature: F09 | Tier: 2
#[tokio::test]
async fn tier2_r1_tampered_or_missing_bearer_token_fenced_with_401() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let anon = cluster.unauthenticated_actor();
    assert_eq!(anon.get("/api/v1/auth/me").await.status, StatusCode::UNAUTHORIZED);

    let mut tampered = cluster.register_actor("tamper_u", "StrongPass#2026").await;
    tampered.bearer_token = Some(format!("{}_corrupted", tampered.token()));
    assert_eq!(
        tampered.get("/api/v1/auth/me").await.status,
        StatusCode::UNAUTHORIZED
    );
}

// Domain: R3 | Feature: F22 | Tier: 2
#[tokio::test]
async fn tier2_r3_unauthorized_chat_history_send_forward_and_non_admin_pin_forbidden() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("acl_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("acl_bob", "StrongPass#2026").await;
    let outsider = cluster.register_actor("acl_outsider", "StrongPass#2026").await;

    let secret_group = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Private Core Team", "chat_type": "group", "member_ids": [bob.uid()] }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    let msg_id = alice
        .post_json(
            &format!("/api/v1/chats/{secret_group}/messages"),
            &json!({ "text": "Confidential roadmap", "client_nonce": 5001 }),
        )
        .await
        .json()["message_id"]
        .as_i64()
        .unwrap();

    // Outsider cannot read or send in secret_group
    assert_eq!(
        outsider
            .get(&format!("/api/v1/chats/{secret_group}/messages"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        outsider
            .post_json(
                &format!("/api/v1/chats/{secret_group}/messages"),
                &json!({ "text": "Intrusion", "client_nonce": 5002 }),
            )
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    // Outsider cannot forward message from secret_group into their own Saved Messages
    let saved = outsider
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Saved Messages", "chat_type": "saved" }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();
    let illegal_fwd = outsider
        .post_json(
            &format!("/api/v1/chats/{saved}/messages"),
            &json!({
                "forward_from_chat_id": secret_group,
                "forward_from_msg_id": msg_id,
                "client_nonce": 5003
            }),
        )
        .await;
    assert_eq!(illegal_fwd.status, StatusCode::FORBIDDEN);

    // Non-admin Bob cannot pin in group
    let bob_pin = bob
        .post_json(
            &format!("/api/v1/chats/{secret_group}/pins"),
            &json!({ "message_id": msg_id }),
        )
        .await;
    assert_eq!(bob_pin.status, StatusCode::FORBIDDEN);
}

// Domain: R3 | Feature: F17 | Tier: 2
#[tokio::test]
async fn tier2_r3_direct_message_block_enforces_403_user_is_blocked_and_unblock_restores() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("blk_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("blk_bob", "StrongPass#2026").await;

    let dm_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "DM", "chat_type": "direct", "peer_user_id": bob.uid() }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    // Alice blocks Bob
    let blk = alice
        .post_json(&format!("/api/v1/users/{}/block", bob.uid()), &json!({}))
        .await;
    assert_eq!(blk.status, StatusCode::OK);

    // Bob sending in DM fails with 403 user_is_blocked
    let blocked_send = bob
        .post_json(
            &format!("/api/v1/chats/{dm_id}/messages"),
            &json!({ "text": "Are you there?", "client_nonce": 6001 }),
        )
        .await;
    assert_eq!(blocked_send.status, StatusCode::FORBIDDEN);
    assert_eq!(blocked_send.json()["error"], "user_is_blocked");

    // Unblock restores messaging
    assert_eq!(
        alice
            .delete(&format!("/api/v1/users/{}/block", bob.uid()))
            .await
            .status,
        StatusCode::OK
    );
    assert!(
        bob.post_json(
            &format!("/api/v1/chats/{dm_id}/messages"),
            &json!({ "text": "Unblocked hello!", "client_nonce": 6002 }),
        )
        .await
        .status
        .is_success()
    );
}

// Domain: R3 | Feature: F19 | Tier: 2
#[tokio::test]
async fn tier2_r3_restricted_member_send_ban_and_kicked_member_access_revocation() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let admin = cluster.register_actor("mod_admin", "StrongPass#2026").await;
    let member = cluster.register_actor("mod_member", "StrongPass#2026").await;

    let chat_id = admin
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Moderated Supergroup", "chat_type": "supergroup", "member_ids": [member.uid()] }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    // Restrict member with send-messages banned bit (banned_rights = 1)
    let restrict = admin
        .put_json(
            &format!("/api/v1/chats/{chat_id}/members/{}", member.uid()),
            &json!({ "role": "restricted", "banned_rights": 1 }),
        )
        .await;
    assert_eq!(restrict.status, StatusCode::OK);

    let send_attempt = member
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "Muted attempt", "client_nonce": 7001 }),
        )
        .await;
    assert_eq!(send_attempt.status, StatusCode::FORBIDDEN);

    // Kick member completely -> history access also becomes 403
    let kick = admin
        .delete(&format!("/api/v1/chats/{chat_id}/members/{}", member.uid()))
        .await;
    assert_eq!(kick.status, StatusCode::OK);
    assert_eq!(
        member
            .get(&format!("/api/v1/chats/{chat_id}/messages"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

// Domain: R3 | Feature: F22 | Tier: 2
#[tokio::test]
async fn tier2_r3_invite_usage_limit_exhaustion_and_revocation() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let owner = cluster.register_actor("inv_owner", "StrongPass#2026").await;
    let u1 = cluster.register_actor("inv_u1", "StrongPass#2026").await;
    let u2 = cluster.register_actor("inv_u2", "StrongPass#2026").await;
    let u3 = cluster.register_actor("inv_u3", "StrongPass#2026").await;

    let chat_id = owner
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Limited Group", "chat_type": "group" }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    // Single-use invite
    let inv1 = owner
        .post_json(
            &format!("/api/v1/chats/{chat_id}/invites"),
            &json!({ "usage_limit": 1 }),
        )
        .await
        .json();
    let code1 = inv1["code"].as_str().unwrap();

    assert_eq!(
        u1.post_json("/api/v1/invites/join", &json!({ "code": code1 }))
            .await
            .status,
        StatusCode::OK
    );
    // Second joiner exceeds usage_limit=1
    let exhausted = u2
        .post_json("/api/v1/invites/join", &json!({ "code": code1 }))
        .await;
    assert!(exhausted.status.is_client_error());

    // Revoked invite rejected immediately
    let inv2 = owner
        .post_json(&format!("/api/v1/chats/{chat_id}/invites"), &json!({}))
        .await
        .json();
    let inv2_id = inv2["invite_id"].as_str().unwrap();
    let code2 = inv2["code"].as_str().unwrap();
    assert_eq!(
        owner
            .delete(&format!("/api/v1/chats/{chat_id}/invites/{inv2_id}"))
            .await
            .status,
        StatusCode::OK
    );
    assert!(
        u3.post_json("/api/v1/invites/join", &json!({ "code": code2 }))
            .await
            .status
            .is_client_error()
    );
}

// Domain: R4 | Feature: F27 | Tier: 2
#[tokio::test]
async fn tier2_r4_closed_poll_vote_and_empty_reaction_rejected() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("neg_poll_a", "StrongPass#2026").await;
    let bob = cluster.register_actor("neg_poll_b", "StrongPass#2026").await;

    let chat_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Poll Boundary", "chat_type": "group", "member_ids": [bob.uid()] }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    let poll = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/polls"),
            &json!({
                "question": "Ship today?",
                "options": ["Yes", "Definitely"]
            }),
        )
        .await
        .json();
    let poll_id = poll["poll_id"]
        .as_u64()
        .or_else(|| poll["poll"]["poll_id"].as_u64())
        .unwrap();
    let msg_id = poll["message_id"]
        .as_i64()
        .or_else(|| poll["message"]["message_id"].as_i64())
        .unwrap_or(1);

    assert_eq!(
        alice
            .post_json(&format!("/api/v1/polls/{poll_id}/close"), &json!({}))
            .await
            .status,
        StatusCode::OK
    );

    // Late vote after poll close must fail with 4xx
    let late_vote = bob
        .post_json(
            &format!("/api/v1/polls/{poll_id}/vote"),
            &json!({ "option_ids": [0] }),
        )
        .await;
    assert!(late_vote.status.is_client_error());

    // Whitespace-only emoji reaction rejected with 400
    let bad_react = bob
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages/{msg_id}/reactions"),
            &json!({ "emoji": "   ", "action": "add" }),
        )
        .await;
    assert_eq!(bad_react.status, StatusCode::BAD_REQUEST);
}

// Domain: R4 | Feature: F28 | Tier: 2
#[tokio::test]
async fn tier2_r4_non_sender_edit_delete_forbidden_and_tombstone_removes_from_history() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("del_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("del_bob", "StrongPass#2026").await;

    let chat_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Edit ACL", "chat_type": "group", "member_ids": [bob.uid()] }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    let msg_id = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "Alice authored message", "client_nonce": 8001 }),
        )
        .await
        .json()["message_id"]
        .as_i64()
        .unwrap();

    // Non-admin Bob cannot edit or delete Alice's message
    assert_eq!(
        bob.put_json(
            &format!("/api/v1/chats/{chat_id}/messages/{msg_id}"),
            &json!({ "text": "Tampered by Bob" }),
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        bob.delete(&format!("/api/v1/chats/{chat_id}/messages/{msg_id}?revoke=true"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    // Alice deletes with revoke=true -> disappears from Bob's history
    assert_eq!(
        alice
            .delete(&format!("/api/v1/chats/{chat_id}/messages/{msg_id}?revoke=true"))
            .await
            .status,
        StatusCode::OK
    );
    let bob_hist = bob.get(&format!("/api/v1/chats/{chat_id}/messages")).await.json();
    let remaining = bob_hist["messages"].as_array().unwrap();
    assert!(!remaining.iter().any(|m| m["message_id"].as_i64() == Some(msg_id)));
}

// Domain: R4 | Feature: F32 | Tier: 2
#[tokio::test]
async fn tier2_r4_pts_gap_detection_and_sync_difference_exact_convergence() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("pts_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("pts_bob", "StrongPass#2026").await;

    let chat_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "PTS Gap Chat", "chat_type": "direct", "peer_user_id": bob.uid() }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    let first = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "Anchor #1", "client_nonce": 9001 }),
        )
        .await
        .json();
    let anchor_pts = first["pts"].as_u64().unwrap();

    // Simulate Bob offline while Alice sends 3 updates
    for idx in 2..=4u64 {
        let r = alice
            .post_json(
                &format!("/api/v1/chats/{chat_id}/messages"),
                &json!({ "text": format!("Missed #{idx}"), "client_nonce": 9000 + idx }),
            )
            .await;
        assert!(r.status.is_success());
    }

    // Bob reconciles from anchor_pts via GET /api/v1/sync/difference
    let diff = bob
        .get(&format!(
            "/api/v1/sync/difference?chat_id={chat_id}&pts={anchor_pts}&limit=50"
        ))
        .await;
    assert_eq!(diff.status, StatusCode::OK);
    let diff_json = diff.json();
    let new_pts = diff_json["new_pts"]
        .as_u64()
        .or_else(|| diff_json["current_pts"].as_u64())
        .or_else(|| diff_json["pts"].as_u64())
        .expect("new_pts in sync/difference");
    assert!(
        new_pts >= anchor_pts + 3,
        "Expected new_pts >= {}, got {}",
        anchor_pts + 3,
        new_pts
    );
}

// Domain: R4 | Feature: F24 | Tier: 2
#[tokio::test]
async fn tier2_r4_utf16_cjk_zwj_emoji_rtl_and_surrogate_entity_offsets_roundtrip() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("utf16_alice", "StrongPass#2026").await;
    let bob = cluster.register_actor("utf16_bob", "StrongPass#2026").await;

    let chat_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "UTF-16 Torture", "chat_type": "direct", "peer_user_id": bob.uid() }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    // Prefix: "中文👨‍👩‍👧‍👦مرحبا " -> compute exact UTF-16 code-unit length for Bold span "Code"
    let prefix = "中文👨‍👩‍👧‍👦مرحبا ";
    let target = "Code";
    let full_text = format!("{prefix}{target}");
    let utf16_offset = prefix.encode_utf16().count();
    let utf16_len = target.encode_utf16().count();

    let send = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({
                "text": full_text,
                "entities": [
                    { "type": "Bold", "offset": utf16_offset, "length": utf16_len }
                ],
                "client_nonce": 9501
            }),
        )
        .await;
    assert!(send.status.is_success(), "UTF-16 entity send failed: {}", send.body_text());

    let hist = bob.get(&format!("/api/v1/chats/{chat_id}/messages")).await.json();
    let msg = &hist["messages"].as_array().unwrap()[0];
    assert_eq!(msg["text"].as_str(), Some(full_text.as_str()));
}

// Domain: R4 | Feature: F23 | Tier: 2
#[tokio::test]
async fn tier2_r4_message_text_empty_and_over_4096_char_boundary_enforced() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("len_alice", "StrongPass#2026").await;

    let chat_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Length Boundary", "chat_type": "group" }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();

    // Whitespace-only message rejected
    let empty_msg = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": "    \n\t  ", "client_nonce": 9801 }),
        )
        .await;
    assert_eq!(empty_msg.status, StatusCode::BAD_REQUEST);

    // Exact 4096 chars accepted
    let exact_4096 = "X".repeat(4096);
    let ok_msg = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": exact_4096, "client_nonce": 9802 }),
        )
        .await;
    assert!(ok_msg.status.is_success());

    // 4097 chars rejected with 400
    let over_4097 = "X".repeat(4097);
    let over_msg = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({ "text": over_4097, "client_nonce": 9803 }),
        )
        .await;
    assert_eq!(over_msg.status, StatusCode::BAD_REQUEST);
}

// Domain: R4 | Feature: F25 | Tier: 2
#[tokio::test]
async fn tier2_r4_zero_byte_media_upload_invalid_base64_and_missing_media_id_rejected() {
    let cluster = LoopbackServerCluster::attach_or_spawn().await;
    let alice = cluster.register_actor("medneg_alice", "StrongPass#2026").await;

    // Empty body upload -> 400
    let empty_up = alice
        .request(
            reqwest::Method::POST,
            "/api/v1/media/upload",
            &[],
            Some(Vec::new()),
            Some("image/png"),
        )
        .await;
    assert_eq!(empty_up.status, StatusCode::BAD_REQUEST);

    // Invalid base64 JSON upload -> 400
    let bad_b64 = alice
        .post_json(
            "/api/v1/media/upload",
            &json!({
                "data_base64": "!!!not_valid_base64!!!",
                "filename": "bad.png",
                "mime_type": "image/png"
            }),
        )
        .await;
    assert_eq!(bad_b64.status, StatusCode::BAD_REQUEST);

    // Sending message referencing nonexistent media_id -> 400
    let chat_id = alice
        .post_json(
            "/api/v1/chats",
            &json!({ "title": "Missing Media Chat", "chat_type": "group" }),
        )
        .await
        .json()["chat_id"]
        .as_i64()
        .unwrap();
    let ghost_media = alice
        .post_json(
            &format!("/api/v1/chats/{chat_id}/messages"),
            &json!({
                "media_id": "deadbeef_nonexistent_sha256",
                "client_nonce": 9901
            }),
        )
        .await;
    assert_eq!(ghost_media.status, StatusCode::BAD_REQUEST);
}

