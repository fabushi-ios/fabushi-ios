use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::UserId;
use gpui_telegram_server::{
    media::{stream_blob, CasStore},
    search::{SearchDocument, SearchQueryParams, TantivyIndexer},
    server::AppState,
    webrtc::{can_transition, generate_turn_token, validate_transition, SignalingHub},
    ServerConfig,
};
use gpui_telegram_webrtc::signaling::{CallState, SignalingFrame};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tower::ServiceExt;

// ============================================================================
// 1. ADVERSARIAL AUTH & TOKEN ATTACK HARNESS
// ============================================================================

#[tokio::test]
async fn test_adversarial_auth_fuzzing_and_tampered_tokens() {
    let state = Arc::new(AppState::new(ServerConfig::default()));
    let app = gpui_telegram_server::server::build_router_with_state(state.clone());

    // Register a legitimate user
    let (user, session, valid_token) = state
        .session_manager
        .register(
            "adversary_target",
            "TargetStrongPass#2026",
            "Target",
            None,
            "Target Laptop",
        )
        .expect("Registration must succeed");

    let tampered_secret = format!("{}.tampered_secret_xyz", session.session_id);
    let tampered_prefix = format!("forged_prefix.{}", valid_token.split('.').nth(1).unwrap_or(""));
    let too_many_parts = format!("{}.extra.part.secret", valid_token);

    // Matrix of adversarial / forged / malformed tokens
    let adversarial_tokens = vec![
        "",                                               // Empty string
        "   ",                                            // Whitespace
        "invalid_random_string",                          // Random garbage
        "sess_00000000000000000000000000000000.secret",  // Non-existent session
        tampered_secret.as_str(),                         // Valid session ID with forged secret
        tampered_prefix.as_str(),                         // Forged session ID with valid secret
        too_many_parts.as_str(),                          // Too many segments
        "sess_!@#$%^&*()",                                // Injection characters
    ];

    for bad_token in adversarial_tokens {
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/auth/me")
            .header(header::AUTHORIZATION, format!("Bearer {}", bad_token))
            .body(Body::empty())
            .unwrap();

        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "Adversarial token '{}' must be rejected with 401 Unauthorized",
            bad_token
        );
    }

    // Verify legitimate token still works perfectly
    let req_valid = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", valid_token))
        .body(Body::empty())
        .unwrap();

    let res_valid = app.clone().oneshot(req_valid).await.unwrap();
    assert_eq!(res_valid.status(), StatusCode::OK);
    let body = to_bytes(res_valid.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["user_id"], user.user_id.raw());
    assert_eq!(json["account_generation"], 1);

    // Multi-device session revocation: Revoke target session
    let revoked = state.session_manager.revoke_session(&session.session_id);
    assert!(revoked.is_some());

    // Account generation fence must have monotonically bumped
    let fence = state.session_manager.get_or_create_fence(user.user_id);
    assert_eq!(fence.current(), 2);

    // Now previously valid token MUST be rejected immediately due to revoked session and fence bump
    let req_post_revoke = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", valid_token))
        .body(Body::empty())
        .unwrap();

    let res_post_revoke = app.clone().oneshot(req_post_revoke).await.unwrap();
    assert_eq!(res_post_revoke.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================================
// 2. ADVERSARIAL TANTIVY SEARCH CONCURRENCY & ISOLATION HARNESS
// ============================================================================

#[tokio::test]
async fn test_adversarial_tantivy_high_concurrency_stress() {
    let indexer = TantivyIndexer::in_ram().expect("In-ram Tantivy indexer must initialize");

    // Spawn 20 concurrent worker tasks each indexing 10 distinct messages
    let mut handles = Vec::new();
    for task_id in 0..20 {
        let indexer_clone = indexer.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..10 {
                let doc = SearchDocument::new(
                    format!("concurrent_msg_{task_id}_{i}"),
                    format!("chat_{}", task_id % 4),
                    format!("user_{task_id}"),
                    1_700_000_000 + (task_id * 10 + i) as i64,
                    format!("Stress test payload word_alpha_{task_id} message_item_{i} keyword_target"),
                    if i % 2 == 0 { Some("photo".into()) } else { None },
                );
                indexer_clone.index_message(doc).await.unwrap();
            }
        }));
    }

    // Await all 20 concurrent indexing tasks without any DirectoryLockBusy panic or error
    for h in handles {
        h.await.unwrap();
    }

    // Force commit and flush
    indexer.commit().await.expect("Commit after concurrent write must succeed");

    // Verify search hits: Tantivy query with max limit returns 100 hits
    let query_all = SearchQueryParams::new("keyword_target").with_limit(100);
    let all_hits = indexer.search_hits(&query_all).expect("Search must succeed");
    assert_eq!(all_hits.len(), 100, "Search results must be capped at 100 per Tantivy query contract");

    // Verify in-chat isolation under concurrency:
    // 20 tasks % 4 = exactly 5 tasks per chat (5 tasks * 10 docs = 50 docs per chat)
    let mut total_indexed_count = 0;
    for chat_num in 0..4 {
        let chat_name = format!("chat_{chat_num}");
        let query_chat = SearchQueryParams::new("keyword_target")
            .with_chat_id(&chat_name)
            .with_limit(100);
        let chat_hits = indexer.search_hits(&query_chat).expect("Search per-chat must succeed");
        assert_eq!(chat_hits.len(), 50, "Chat isolation must contain exactly 50 documents for {}", chat_name);
        for hit in &chat_hits {
            assert_eq!(hit.chat_id, chat_name);
        }
        total_indexed_count += chat_hits.len();
    }
    assert_eq!(total_indexed_count, 200, "Total documents indexed across all 4 chats must equal 200");

    // Deletion tombstone verification:
    // Delete 5 messages from chat_0
    for i in 0..5 {
        indexer.delete_message(format!("concurrent_msg_0_{i}")).await.unwrap();
    }
    indexer.commit().await.unwrap();

    let query_chat0 = SearchQueryParams::new("keyword_target")
        .with_chat_id("chat_0")
        .with_limit(100);
    let post_delete_hits = indexer.search_hits(&query_chat0).unwrap();
    assert_eq!(post_delete_hits.len(), 45, "5 deleted messages must be masked by tombstones");
}

// ============================================================================
// 3. ADVERSARIAL CAS MEDIA & RFC 7233 RANGE HARNESS
// ============================================================================

#[tokio::test]
async fn test_adversarial_cas_corrupted_chunks_and_rfc7233_edge_cases() {
    let cas = CasStore::temp("adversarial_cas").unwrap();
    let upload_id = "adv_upload_session_99";

    // 1. Chunk boundary attacks:
    // Init session for 300 bytes, part_size 100 -> 3 parts total
    cas.init_chunked_upload(upload_id, 300, 100, None)
        .await
        .unwrap();

    // Attack: write chunk with wrong size (80 bytes instead of 100)
    let bad_size = vec![0xAA; 80];
    let err_size = cas.write_chunk(upload_id, 0, &bad_size).await;
    assert!(matches!(err_size, Err(TelegramError::Validation(_))));

    // Attack: out-of-bounds part index (part 3 when parts are 0..=2)
    let err_oob = cas.write_chunk(upload_id, 3, &[0xBB; 100]).await;
    assert!(matches!(err_oob, Err(TelegramError::Validation(_))));

    // Store a reference file of 1,000 bytes for RFC 7233 Range testing
    let sample_payload: Vec<u8> = (0..1000).map(|i| (i % 255) as u8).collect();
    let hash = cas.store_blob(&sample_payload).await.unwrap();

    // Test Content Deduplication
    let hash_dup = cas.store_blob(&sample_payload).await.unwrap();
    assert_eq!(hash, hash_dup, "CAS must deduplicate identical payloads");

    // RFC 7233 Edge Cases:
    // Edge case 1: Single-byte closed range: bytes=0-0
    let resp = stream_blob(&cas, &hash, Some("bytes=0-0"), None).await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get("content-range").unwrap().to_str().unwrap(),
        "bytes 0-0/1000"
    );
    assert_eq!(resp.headers().get("content-length").unwrap().to_str().unwrap(), "1");
    let b = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(b.as_ref(), &[sample_payload[0]]);

    // Edge case 2: Single-byte closed range at end: bytes=999-999
    let resp = stream_blob(&cas, &hash, Some("bytes=999-999"), None).await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get("content-range").unwrap().to_str().unwrap(),
        "bytes 999-999/1000"
    );
    assert_eq!(resp.headers().get("content-length").unwrap().to_str().unwrap(), "1");
    let b = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(b.as_ref(), &[sample_payload[999]]);

    // Edge case 3: Unsatisfiable range beyond EOF: bytes=1000- -> 416
    let resp = stream_blob(&cas, &hash, Some("bytes=1000-"), None).await;
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        resp.headers().get("content-range").unwrap().to_str().unwrap(),
        "bytes */1000"
    );

    // Edge case 4: Reversed range where start > end: bytes=500-200 -> 416 per RFC 7233 Section 4.4
    let resp_rev = stream_blob(&cas, &hash, Some("bytes=500-200"), None).await;
    assert_eq!(resp_rev.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        resp_rev.headers().get("content-range").unwrap().to_str().unwrap(),
        "bytes */1000"
    );

    // Edge case 5: Malformed syntax: bytes=invalid-non-numeric -> 400 Bad Request
    let resp_bad = stream_blob(&cas, &hash, Some("bytes=foo-bar"), None).await;
    assert_eq!(resp_bad.status(), StatusCode::BAD_REQUEST);

    // Edge case 6: Non-existent CAS blob -> 404
    let resp_missing = stream_blob(&cas, "0000000000000000000000000000000000000000000000000000000000000000", None, None).await;
    assert_eq!(resp_missing.status(), StatusCode::NOT_FOUND);
}

// ============================================================================
// 4. ADVERSARIAL WEBRTC CALL FSM & ANTI-SPOOFING HARNESS
// ============================================================================

#[tokio::test]
async fn test_adversarial_webrtc_fsm_spoofing_and_illegal_transitions() {
    // 1. Verify invalid FSM transitions are strictly rejected
    assert!(!can_transition(CallState::Ringing, CallState::Connected));
    assert!(!can_transition(CallState::Idle, CallState::Connecting));
    assert!(!can_transition(CallState::Ended, CallState::Dialing));
    assert!(!can_transition(CallState::Failed, CallState::Ringing));

    assert!(validate_transition(CallState::Ringing, CallState::Connected).is_err());
    assert!(validate_transition(CallState::Idle, CallState::Connecting).is_err());

    // 2. Anti-spoofing in SignalingHub:
    let hub = SignalingHub::new();
    let alice = UserId::new(5001);
    let bob = UserId::new(5002);
    let eve = UserId::new(5003);

    let (alice_tx, mut alice_rx) = mpsc::channel(16);
    let (bob_tx, mut bob_rx) = mpsc::channel(16);
    let (eve_tx, mut _eve_rx) = mpsc::channel(16);

    hub.register_client(alice, alice_tx);
    hub.register_client(bob, bob_tx);
    hub.register_client(eve, eve_tx);

    let call_id = "adv_call_session_777".to_string();

    // Alice invites Bob
    hub.dispatch_frame(
        alice,
        SignalingFrame::Invite {
            call_id: call_id.clone(),
            from_user: alice,
            to_user: bob,
            is_video: false,
        },
    )
    .await
    .unwrap();

    // Bob receives Invite
    let bob_invite = bob_rx.recv().await.unwrap();
    assert!(matches!(bob_invite, SignalingFrame::Invite { .. }));

    // Eve attempts to inject Ringing into Alice and Bob's call
    let eve_ring = hub
        .dispatch_frame(eve, SignalingFrame::Ringing { call_id: call_id.clone() })
        .await;
    assert!(matches!(eve_ring, Err(TelegramError::PermissionDenied(_))));

    // Eve attempts to inject Accept into Alice and Bob's call
    let eve_accept = hub
        .dispatch_frame(eve, SignalingFrame::Accept { call_id: call_id.clone() })
        .await;
    assert!(matches!(eve_accept, Err(TelegramError::PermissionDenied(_))));

    // Eve attempts to inject SDP Offer into Alice and Bob's call
    let eve_offer = hub
        .dispatch_frame(
            eve,
            SignalingFrame::Offer {
                call_id: call_id.clone(),
                sdp: "v=0\r\no=eve...".into(),
            },
        )
        .await;
    assert!(matches!(eve_offer, Err(TelegramError::PermissionDenied(_))));

    // Eve attempts to inject ICE candidate into Alice and Bob's call
    let eve_ice = hub
        .dispatch_frame(
            eve,
            SignalingFrame::IceCandidate {
                call_id: call_id.clone(),
                candidate: "candidate:eve...".into(),
                sdp_mid: None,
                sdp_mline_index: None,
            },
        )
        .await;
    assert!(matches!(eve_ice, Err(TelegramError::PermissionDenied(_))));

    // Bob properly responds with Ringing and Accept
    hub.dispatch_frame(bob, SignalingFrame::Ringing { call_id: call_id.clone() })
        .await
        .unwrap();
    let alice_ring = alice_rx.recv().await.unwrap();
    assert!(matches!(alice_ring, SignalingFrame::Ringing { .. }));

    hub.dispatch_frame(bob, SignalingFrame::Accept { call_id: call_id.clone() })
        .await
        .unwrap();
    let alice_accept = alice_rx.recv().await.unwrap();
    assert!(matches!(alice_accept, SignalingFrame::Accept { .. }));

    // Eve attempts to Hangup Alice and Bob's call
    let _ = hub
        .dispatch_frame(
            eve,
            SignalingFrame::Hangup {
                call_id: call_id.clone(),
                reason: "Malicious termination".into(),
            },
        )
        .await;
    // Alice's channel must NOT receive any hangup frame from Eve
    assert!(alice_rx.try_recv().is_err());

    // 3. RFC 5766 STUN/TURN Ephemeral Credentials with Long Secret (> 64 bytes)
    let long_secret = "a_very_long_secret_key_that_exceeds_sixty_four_bytes_to_stress_test_rfc2104_key_hashing_logic_properly_1234567890";
    let uris = vec!["turn:turn.domain.org:3478".to_string()];
    let turn_token = generate_turn_token(alice, long_secret, uris, 12);
    assert!(turn_token.username.ends_with(":5001"));
    assert!(!turn_token.credential.is_empty());
}
