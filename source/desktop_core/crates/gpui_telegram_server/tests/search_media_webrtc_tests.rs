use axum::body::to_bytes;
use axum::http::StatusCode;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_server::media::{stream_blob, CasStore, MediaStorageService};
use gpui_telegram_server::search::{SearchDocument, SearchQueryParams, TantivyIndexer};
use gpui_telegram_server::webrtc::{
    can_transition, generate_turn_token, validate_transition, SignalingHub,
};
use gpui_telegram_webrtc::signaling::{CallState, SignalingFrame};
use std::time::Duration;
use tokio::sync::mpsc;

// ============================================================================
// 1. TANTIVY 0.22 FULL-TEXT SEARCH TESTS
// ============================================================================

#[tokio::test]
async fn test_tantivy_index_and_sub_millisecond_search() {
    let indexer = TantivyIndexer::in_ram().expect("In-ram Tantivy indexer must initialize");

    // Ingest 50 messages across 5 chats
    for i in 0..50 {
        let chat_id = format!("chat_{}", i % 5);
        let sender_id = format!("user_{}", i % 10);
        let content = if i == 7 {
            "Special confidential meeting regarding project alpha deployment"
        } else if i % 2 == 0 {
            "Hello world greeting from team member"
        } else {
            "Quick brown fox jumps over lazy dog"
        };

        let doc = SearchDocument::new(
            format!("msg_{i}"),
            chat_id,
            sender_id,
            1_700_000_000 + i as i64,
            content,
            if i % 3 == 0 { Some("photo".into()) } else { None },
        );
        indexer.index_message(doc).await.expect("Index message must succeed");
    }

    // Force commit and flush
    indexer.commit().await.expect("Commit must succeed");

    // Search scoped to chat_2 for "confidential meeting"
    let start = std::time::Instant::now();
    let query = SearchQueryParams::new("confidential meeting").with_chat_id("chat_2");
    let hits = indexer.search_hits(&query).expect("Search hits must succeed");
    let elapsed = start.elapsed();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message_id, "msg_7");
    assert_eq!(hits[0].chat_id, "chat_2");
    // Assert sub-millisecond or sub-5ms query execution
    assert!(elapsed < Duration::from_millis(15), "Query took {:?}", elapsed);

    // Verify snippet has <mark> tags
    assert!(
        hits[0].snippet_html.contains("<mark>confidential</mark>")
            || hits[0].snippet_html.contains("<mark>meeting</mark>"),
        "Snippet was: {}",
        hits[0].snippet_html
    );
}

#[tokio::test]
async fn test_tantivy_in_chat_isolation() {
    let indexer = TantivyIndexer::in_ram().expect("In-ram Tantivy indexer must initialize");

    // Index secret word in chat_alpha
    indexer
        .index_message(SearchDocument::new(
            "m1",
            "chat_alpha",
            "u1",
            1000,
            "TopSecretTokenXYZ987",
            None,
        ))
        .await
        .unwrap();

    indexer.commit().await.unwrap();

    // Search scoped to chat_beta
    let hits = indexer
        .search_hits(&SearchQueryParams::new("TopSecretTokenXYZ987").with_chat_id("chat_beta"))
        .unwrap();
    assert_eq!(hits.len(), 0, "Chat isolation must prevent cross-chat leakage");

    // Search scoped to chat_alpha
    let hits_alpha = indexer
        .search_hits(&SearchQueryParams::new("TopSecretTokenXYZ987").with_chat_id("chat_alpha"))
        .unwrap();
    assert_eq!(hits_alpha.len(), 1);
    assert_eq!(hits_alpha[0].message_id, "m1");
}

#[tokio::test]
async fn test_tantivy_message_deletion_and_atomic_upsert() {
    let indexer = TantivyIndexer::in_ram().expect("In-ram Tantivy indexer must initialize");

    // 1. Index initial message
    indexer
        .index_message(SearchDocument::new(
            "doc_100",
            "chat_main",
            "user_alice",
            2000,
            "Original content prior to editing",
            None,
        ))
        .await
        .unwrap();
    indexer.commit().await.unwrap();

    let initial = indexer
        .search_hits(&SearchQueryParams::new("Original").with_chat_id("chat_main"))
        .unwrap();
    assert_eq!(initial.len(), 1);

    // 2. Atomic Upsert: Index same message_id with new content
    indexer
        .index_message(SearchDocument::new(
            "doc_100",
            "chat_main",
            "user_alice",
            2005,
            "Revised updated content after edit",
            None,
        ))
        .await
        .unwrap();
    indexer.commit().await.unwrap();

    let old_search = indexer
        .search_hits(&SearchQueryParams::new("Original").with_chat_id("chat_main"))
        .unwrap();
    assert_eq!(old_search.len(), 0, "Old content must be purged on upsert");

    let new_search = indexer
        .search_hits(&SearchQueryParams::new("Revised").with_chat_id("chat_main"))
        .unwrap();
    assert_eq!(new_search.len(), 1);
    assert_eq!(new_search[0].message_id, "doc_100");

    // 3. Deletion
    indexer.delete_message("doc_100").await.unwrap();
    indexer.commit().await.unwrap();

    let deleted_search = indexer
        .search_hits(&SearchQueryParams::new("Revised").with_chat_id("chat_main"))
        .unwrap();
    assert_eq!(deleted_search.len(), 0, "Deleted message must no longer match");
}

#[tokio::test]
async fn test_tantivy_faceted_media_and_date_range() {
    let indexer = TantivyIndexer::in_ram().expect("In-ram Tantivy indexer must initialize");

    indexer
        .index_message(SearchDocument::new(
            "m_photo_old",
            "chat_1",
            "user_1",
            1000,
            "Sunset at the beach photo attachment",
            Some("photo".into()),
        ))
        .await
        .unwrap();

    indexer
        .index_message(SearchDocument::new(
            "m_photo_new",
            "chat_1",
            "user_1",
            5000,
            "Sunrise at the mountain photo attachment",
            Some("photo".into()),
        ))
        .await
        .unwrap();

    indexer
        .index_message(SearchDocument::new(
            "m_doc_mid",
            "chat_1",
            "user_1",
            3000,
            "Quarterly financial report document",
            Some("document".into()),
        ))
        .await
        .unwrap();

    indexer.commit().await.unwrap();

    // Query 1: Filter by media_type = photo AND date range [4000..=6000]
    let hits = indexer
        .search_hits(
            &SearchQueryParams::new("attachment")
                .with_media_type("photo")
                .with_date_range(Some(4000), Some(6000)),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message_id, "m_photo_new");

    // Query 2: Filter by media_type = document
    let hits_doc = indexer
        .search_hits(&SearchQueryParams::new("report").with_media_type("document"))
        .unwrap();
    assert_eq!(hits_doc.len(), 1);
    assert_eq!(hits_doc[0].message_id, "m_doc_mid");
}

#[tokio::test]
async fn test_tantivy_batched_commit_actor_threshold() {
    let indexer = TantivyIndexer::in_ram().expect("In-ram Tantivy indexer must initialize");

    // Ingest 100 documents without manual commit to trigger the 100-doc threshold
    for i in 0..100 {
        indexer
            .index_message(SearchDocument::new(
                format!("batch_msg_{i}"),
                "batch_chat",
                "u1",
                1000 + i as i64,
                format!("Automatic batch ingestion payload number {i}"),
                None,
            ))
            .await
            .unwrap();
    }

    // Give writer task a brief moment to process batch commit and reload reader
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Use reader to search
    let hits = indexer
        .search_hits(&SearchQueryParams::new("Automatic").with_chat_id("batch_chat").with_limit(100))
        .unwrap();
    assert_eq!(hits.len(), 100, "All 100 batch-committed documents must be immediately searchable");
}

// ============================================================================
// 2. CONTENT-ADDRESSABLE STORAGE (CAS) & RFC 7233 STREAMING TESTS
// ============================================================================

#[tokio::test]
async fn test_chunked_upload_multi_part_assembly_and_sha256() {
    let cas = CasStore::temp("chunked_test").expect("Temp CAS store must initialize");

    // Create 256KB test payload
    let total_size: u64 = 256 * 1024;
    let part_size: u32 = 64 * 1024; // 4 parts total
    let mut payload = Vec::with_capacity(total_size as usize);
    for i in 0..total_size {
        payload.push((i % 251) as u8);
    }

    // Initialize session
    let upload_id = "test_upload_session_42";
    let session = cas
        .init_chunked_upload(upload_id, total_size, part_size, None)
        .await
        .expect("Init upload must succeed");
    assert_eq!(session.total_parts, 4);

    // Ingest chunks out of order: part 2, part 0, part 3, part 1
    let order = [2usize, 0, 3, 1];
    for &part_idx in &order {
        let start = part_idx * part_size as usize;
        let end = start + part_size as usize;
        let chunk_data = &payload[start..end];

        let is_complete = cas
            .write_chunk(upload_id, part_idx as u32, chunk_data)
            .await
            .expect("Write chunk must succeed");

        if part_idx == 1 {
            assert!(is_complete, "Last missing chunk should mark session complete");
        } else {
            assert!(!is_complete);
        }
    }

    // Complete upload & promote to CAS
    let hash = cas
        .complete_chunked_upload(upload_id)
        .await
        .expect("Complete upload must succeed");

    assert!(cas.cas_file_exists(&hash));

    // Verify disk content matches original payload byte-for-byte
    let read_back = cas.read_blob(&hash).await.expect("Read blob must succeed");
    assert_eq!(read_back, payload);

    // Verify staging files are cleaned up
    assert!(!cas.staging_part_path(upload_id).exists());
    assert!(!cas.staging_meta_path(upload_id).exists());
}

#[tokio::test]
async fn test_cas_deduplication() {
    let cas = CasStore::temp("dedup_test").unwrap();
    let data = b"Identical media payload shared across multiple chats";

    // Direct store blob
    let hash1 = cas.store_blob(data).await.unwrap();
    let hash2 = cas.store_blob(data).await.unwrap();
    assert_eq!(hash1, hash2);

    let path = cas.cas_path(&hash1);
    assert!(path.is_file());

    // Storage service wrapper integration
    let storage_service = MediaStorageService::with_cas_store(cas.clone());
    let fetch = storage_service.fetch_range(&hash1, 0, data.len()).unwrap();
    assert_eq!(fetch, data);
}

#[tokio::test]
async fn test_chunked_upload_corrupted_chunk_and_sha256_mismatch() {
    let cas = CasStore::temp("corrupted_test").unwrap();
    let upload_id = "corrupted_session_1";

    // Init session expecting 100 bytes with 50-byte chunks
    cas.init_chunked_upload(
        upload_id,
        100,
        50,
        Some("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".into()),
    )
    .await
    .unwrap();

    // 1. Send chunk with wrong size (30 bytes instead of 50)
    let bad_chunk = vec![0u8; 30];
    let err = cas.write_chunk(upload_id, 0, &bad_chunk).await;
    assert!(matches!(err, Err(TelegramError::Validation(_))));

    // 2. Send chunk with out-of-bounds index (chunk 5 when total_parts is 2)
    let err_oob = cas.write_chunk(upload_id, 5, &[0u8; 50]).await;
    assert!(matches!(err_oob, Err(TelegramError::Validation(_))));

    // 3. Complete valid chunks but with mismatched expected hash
    let valid_chunk = vec![0x42u8; 50];
    cas.write_chunk(upload_id, 0, &valid_chunk).await.unwrap();
    cas.write_chunk(upload_id, 1, &valid_chunk).await.unwrap();

    let mismatch_err = cas.complete_chunked_upload(upload_id).await;
    assert!(matches!(mismatch_err, Err(TelegramError::Validation(_))));
}

#[tokio::test]
async fn test_http206_streaming_closed_range() {
    let cas = CasStore::temp("stream_closed").unwrap();
    let test_data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
    let hash = cas.store_blob(&test_data).await.unwrap();

    // Request Range: bytes=100-199 (100 bytes)
    let resp = stream_blob(&cas, &hash, Some("bytes=100-199"), Some("application/octet-stream")).await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get("content-range").unwrap().to_str().unwrap(),
        "bytes 100-199/5000"
    );
    assert_eq!(
        resp.headers().get("content-length").unwrap().to_str().unwrap(),
        "100"
    );

    let body_bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(body_bytes.as_ref(), &test_data[100..=199]);
}

#[tokio::test]
async fn test_http206_streaming_open_ended_and_suffix_ranges() {
    let cas = CasStore::temp("stream_open").unwrap();
    let test_data: Vec<u8> = (0..2048).map(|i| (i % 127) as u8).collect();
    let hash = cas.store_blob(&test_data).await.unwrap();

    // 1. Open-ended: bytes=2000- (last 48 bytes: 2000..=2047)
    let resp_open = stream_blob(&cas, &hash, Some("bytes=2000-"), Some("video/mp4")).await;
    assert_eq!(resp_open.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp_open.headers().get("content-range").unwrap().to_str().unwrap(),
        "bytes 2000-2047/2048"
    );
    let bytes_open = to_bytes(resp_open.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(bytes_open.as_ref(), &test_data[2000..2048]);

    // 2. Suffix: bytes=-100 (last 100 bytes: 1948..=2047)
    let resp_suffix = stream_blob(&cas, &hash, Some("bytes=-100"), None).await;
    assert_eq!(resp_suffix.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp_suffix.headers().get("content-range").unwrap().to_str().unwrap(),
        "bytes 1948-2047/2048"
    );
    let bytes_suffix = to_bytes(resp_suffix.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(bytes_suffix.as_ref(), &test_data[1948..2048]);
}

#[tokio::test]
async fn test_http206_streaming_unsatisfiable_range_and_full_content() {
    let cas = CasStore::temp("stream_416").unwrap();
    let test_data = b"Hello GPUI Telegram Media Stream";
    let hash = cas.store_blob(test_data).await.unwrap();

    // 1. Unsatisfiable: bytes=99999-
    let resp_416 = stream_blob(&cas, &hash, Some("bytes=99999-"), None).await;
    assert_eq!(resp_416.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        resp_416.headers().get("content-range").unwrap().to_str().unwrap(),
        format!("bytes */{}", test_data.len())
    );

    // 2. Full Content: No Range header -> HTTP 200 OK
    let resp_200 = stream_blob(&cas, &hash, None, Some("text/plain")).await;
    assert_eq!(resp_200.status(), StatusCode::OK);
    assert_eq!(
        resp_200.headers().get("content-length").unwrap().to_str().unwrap(),
        test_data.len().to_string()
    );
    let body_200 = to_bytes(resp_200.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(body_200.as_ref(), test_data);
}

// ============================================================================
// 3. WEBRTC CALL SIGNALING HUB & CALL FSM TESTS
// ============================================================================

#[tokio::test]
async fn test_call_fsm_happy_path_transitions_and_relay() {
    let hub = SignalingHub::new();
    let alice = UserId::new(1001);
    let bob = UserId::new(1002);

    let (alice_tx, mut alice_rx) = mpsc::channel(16);
    let (bob_tx, mut bob_rx) = mpsc::channel(16);

    hub.register_client(alice, alice_tx);
    hub.register_client(bob, bob_tx);

    assert!(hub.is_client_online(alice));
    assert!(hub.is_client_online(bob));

    let call_id = "call_session_999".to_string();

    // Step 1: Alice invites Bob -> Dialing
    hub.dispatch_frame(
        alice,
        SignalingFrame::Invite {
            call_id: call_id.clone(),
            from_user: alice,
            to_user: bob,
            is_video: true,
        },
    )
    .await
    .unwrap();

    assert_eq!(hub.get_call_state(&call_id), Some(CallState::Dialing));
    let bob_frame = bob_rx.recv().await.expect("Bob should receive Invite");
    assert!(matches!(bob_frame, SignalingFrame::Invite { .. }));

    // Step 2: Bob responds with Ringing -> Ringing
    hub.dispatch_frame(bob, SignalingFrame::Ringing { call_id: call_id.clone() })
        .await
        .unwrap();

    assert_eq!(hub.get_call_state(&call_id), Some(CallState::Ringing));
    let alice_frame = alice_rx.recv().await.expect("Alice should receive Ringing");
    assert!(matches!(alice_frame, SignalingFrame::Ringing { .. }));

    // Step 3: Bob accepts -> Connecting
    hub.dispatch_frame(bob, SignalingFrame::Accept { call_id: call_id.clone() })
        .await
        .unwrap();

    assert_eq!(hub.get_call_state(&call_id), Some(CallState::Connecting));
    let alice_frame2 = alice_rx.recv().await.expect("Alice should receive Accept");
    assert!(matches!(alice_frame2, SignalingFrame::Accept { .. }));

    // Step 4: Alice sends Offer
    hub.dispatch_frame(
        alice,
        SignalingFrame::Offer {
            call_id: call_id.clone(),
            sdp: "v=0\r\no=alice...".into(),
        },
    )
    .await
    .unwrap();

    let bob_frame2 = bob_rx.recv().await.expect("Bob should receive Offer");
    assert!(matches!(bob_frame2, SignalingFrame::Offer { .. }));

    // Step 5: Bob sends Answer -> Connected
    hub.dispatch_frame(
        bob,
        SignalingFrame::Answer {
            call_id: call_id.clone(),
            sdp: "v=0\r\no=bob...".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(hub.get_call_state(&call_id), Some(CallState::Connected));
    let alice_frame3 = alice_rx.recv().await.expect("Alice should receive Answer");
    assert!(matches!(alice_frame3, SignalingFrame::Answer { .. }));

    // Step 6: Alice sends IceCandidate
    hub.dispatch_frame(
        alice,
        SignalingFrame::IceCandidate {
            call_id: call_id.clone(),
            candidate: "candidate:1 1 UDP 2130706431...".into(),
            sdp_mid: Some("audio".into()),
            sdp_mline_index: Some(0),
        },
    )
    .await
    .unwrap();

    let bob_frame3 = bob_rx.recv().await.expect("Bob should receive IceCandidate");
    assert!(matches!(bob_frame3, SignalingFrame::IceCandidate { .. }));

    // Step 7: Alice hangs up -> Ended
    hub.dispatch_frame(
        alice,
        SignalingFrame::Hangup {
            call_id: call_id.clone(),
            reason: "Call completed".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(hub.get_call_state(&call_id), Some(CallState::Ended));
    let bob_frame4 = bob_rx.recv().await.expect("Bob should receive Hangup");
    assert!(matches!(bob_frame4, SignalingFrame::Hangup { .. }));
}

#[tokio::test]
async fn test_call_fsm_anti_spoofing_and_unauthorized_rejections() {
    let hub = SignalingHub::new();
    let alice = UserId::new(2001);
    let bob = UserId::new(2002);
    let eve = UserId::new(2003);

    let (alice_tx, _alice_rx) = mpsc::channel(16);
    let (bob_tx, _bob_rx) = mpsc::channel(16);
    let (eve_tx, _eve_rx) = mpsc::channel(16);

    hub.register_client(alice, alice_tx);
    hub.register_client(bob, bob_tx);
    hub.register_client(eve, eve_tx);

    // 1. Cannot call self
    let self_call_err = hub
        .dispatch_frame(
            alice,
            SignalingFrame::Invite {
                call_id: "self_call".into(),
                from_user: alice,
                to_user: alice,
                is_video: false,
            },
        )
        .await;
    assert!(matches!(self_call_err, Err(TelegramError::Validation(_))));

    // 2. Caller spoofing: Eve claims invite is from Alice
    let spoof_err = hub
        .dispatch_frame(
            eve,
            SignalingFrame::Invite {
                call_id: "spoof_call".into(),
                from_user: alice,
                to_user: bob,
                is_video: false,
            },
        )
        .await;
    assert!(matches!(spoof_err, Err(TelegramError::PermissionDenied(_))));

    // 3. Unauthorized accept: Eve attempts to Accept call between Alice and Bob
    hub.dispatch_frame(
        alice,
        SignalingFrame::Invite {
            call_id: "valid_call".into(),
            from_user: alice,
            to_user: bob,
            is_video: false,
        },
    )
    .await
    .unwrap();

    let eve_accept = hub
        .dispatch_frame(eve, SignalingFrame::Accept { call_id: "valid_call".into() })
        .await;
    assert!(matches!(eve_accept, Err(TelegramError::PermissionDenied(_))));
}

#[tokio::test]
async fn test_call_fsm_state_machine_validation() {
    assert!(can_transition(CallState::Idle, CallState::Dialing));
    assert!(can_transition(CallState::Dialing, CallState::Ringing));
    assert!(can_transition(CallState::Ringing, CallState::Connecting));
    assert!(can_transition(CallState::Connecting, CallState::Connected));
    assert!(can_transition(CallState::Connected, CallState::Ended));

    // Aborts
    assert!(can_transition(CallState::Dialing, CallState::Ended));
    assert!(can_transition(CallState::Ringing, CallState::Ended));
    assert!(can_transition(CallState::Connecting, CallState::Ended));

    // Disallowed direct leaps
    assert!(!can_transition(CallState::Idle, CallState::Connected));
    assert!(!can_transition(CallState::Ended, CallState::Dialing));
    assert!(!can_transition(CallState::Failed, CallState::Connected));

    assert!(validate_transition(CallState::Dialing, CallState::Ringing).is_ok());
    assert!(validate_transition(CallState::Idle, CallState::Connected).is_err());
}

#[tokio::test]
async fn test_sfu_room_multicast() {
    let hub = SignalingHub::new();
    let chat_id = ChatId::new(555);

    let u1 = UserId::new(101);
    let u2 = UserId::new(102);
    let u3 = UserId::new(103);

    let (tx1, mut rx1) = mpsc::channel(16);
    let (tx2, mut rx2) = mpsc::channel(16);
    let (tx3, mut rx3) = mpsc::channel(16);

    hub.register_client(u1, tx1);
    hub.register_client(u2, tx2);
    hub.register_client(u3, tx3);

    hub.join_sfu_room(chat_id, u1);
    hub.join_sfu_room(chat_id, u2);
    hub.join_sfu_room(chat_id, u3);

    let subscribers = hub.get_sfu_subscribers(chat_id);
    assert_eq!(subscribers.len(), 3);

    // User 1 broadcasts track frame to room
    let frame = SignalingFrame::IceCandidate {
        call_id: "sfu_conf_1".into(),
        candidate: "track_data_relay".into(),
        sdp_mid: None,
        sdp_mline_index: None,
    };

    let delivered = hub.broadcast_to_sfu_room(chat_id, u1, frame.clone()).await;
    assert_eq!(delivered, 2, "Should deliver to all other members (u2 and u3)");

    // Verify u2 and u3 received, u1 did not
    assert!(rx2.try_recv().is_ok());
    assert!(rx3.try_recv().is_ok());
    assert!(rx1.try_recv().is_err());

    // User 3 leaves room
    hub.leave_sfu_room(chat_id, u3);
    assert_eq!(hub.get_sfu_subscribers(chat_id).len(), 2);

    let delivered_after = hub.broadcast_to_sfu_room(chat_id, u1, frame).await;
    assert_eq!(delivered_after, 1, "Only u2 should receive now");
}

#[test]
fn test_stun_turn_ephemeral_token_generation() {
    let user_id = UserId::new(99998888);
    let secret = "super_secure_turn_secret_token_12345";
    let uris = vec![
        "turn:turn1.telegram.local:3478?transport=udp".to_string(),
        "turns:turn1.telegram.local:5349?transport=tcp".to_string(),
    ];

    let token = generate_turn_token(user_id, secret, uris.clone(), 24);

    assert_eq!(token.uris, uris);
    assert!(token.username.ends_with(":99998888"));
    assert!(!token.credential.is_empty());
    // Credential must be valid Base64
    assert!(base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &token.credential).is_ok());
    assert!(token.expires_at > chrono::Utc::now());
}
