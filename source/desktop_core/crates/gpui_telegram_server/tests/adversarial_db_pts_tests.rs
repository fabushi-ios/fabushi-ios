use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, TopicId, UserId};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use gpui_telegram_proto::sync::{SyncGetDifferenceRequest, UpdateEvent};
use gpui_telegram_server::db::{
    ChatMemberRecord, ChatRecord, ForumTopicRecord, MessageRecord, ServerDatabase,
    UserRecord,
};
use gpui_telegram_server::messaging::{MessageNonceDedup, MessagePipeline};
use gpui_telegram_server::sync::{PtsEngine, SyncDifferenceService};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Barrier;

fn create_test_user(db: &ServerDatabase, user_id: u64, username: &str) -> UserRecord {
    let now = chrono::Utc::now().timestamp_millis();
    let user = UserRecord {
        user_id: UserId(user_id),
        username: Some(username.to_string()),
        phone: Some(format!("+1000{}", user_id)),
        first_name: username.to_string(),
        last_name: None,
        bio: None,
        password_hash: None,
        is_bot: false,
        is_verified: true,
        is_support: false,
        is_scam: false,
        is_fake: false,
        profile_photo_file_id: None,
        account_generation: 1,
        created_at_ms: now,
        updated_at_ms: now,
    };
    db.create_user(&user).expect("Failed to create test user");
    user
}

fn create_test_chat(db: &ServerDatabase, chat_id: i64, creator_id: u64, title: &str) -> ChatRecord {
    let now = chrono::Utc::now().timestamp_millis();
    let chat = ChatRecord {
        chat_id: ChatId(chat_id),
        chat_type: 1,
        peer_type: 1,
        title: title.to_string(),
        username: None,
        about: None,
        creator_id: UserId(creator_id),
        is_forum: false,
        flags: 0,
        photo_file_id: None,
        slowmode_seconds: 0,
        linked_chat_id: None,
        pinned_message_id: None,
        pts: 0,
        last_message_id: MsgId(0),
        last_message_date_ms: 0,
        unread_count: 0,
        account_generation: 1,
        created_at_ms: now,
        updated_at_ms: now,
    };
    db.create_chat(&chat).expect("Failed to create test chat");
    chat
}

// -----------------------------------------------------------------------------
// TASK 1: High-concurrency PTS sequence contention (Single chat & Multi-chat)
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_high_concurrency_pts_single_chat_100_tasks() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open db"));
    create_test_user(&db, 1, "alice");
    create_test_chat(&db, 1001, 1, "PTS Contention Single");

    let engine = Arc::new(PtsEngine::new(db.clone()));
    let chat_id = ChatId(1001);
    let concurrency = 100;
    let barrier = Arc::new(Barrier::new(concurrency));

    let mut handles = Vec::new();
    for i in 0..concurrency {
        let engine_clone = engine.clone();
        let barrier_clone = barrier.clone();
        let handle = tokio::spawn(async move {
            barrier_clone.wait().await;
            let msg = MessageItem::new(
                MsgId(i as i64 + 1),
                chat_id,
                UserId(1),
                PeerId::Chat(chat_id),
                MessageContent::Text {
                    text: format!("Contention update {}", i),
                    entities: Vec::new(),
                },
            );
            engine_clone
                .advance_pts(chat_id, 1, UpdateEvent::NewMessage(msg))
                .expect("advance_pts must succeed")
        });
        handles.push(handle);
    }

    let mut pts_results = Vec::new();
    for handle in handles {
        let (pts, update) = handle.await.expect("task panicked");
        assert_eq!(pts, update.pts);
        pts_results.push(pts);
    }

    // Exact count
    assert_eq!(pts_results.len(), concurrency);

    // Contiguity: exactly 1..=100
    pts_results.sort_unstable();
    let expected: Vec<u64> = (1..=(concurrency as u64)).collect();
    assert_eq!(pts_results, expected, "Contiguity failure in PTS allocation");

    // Zero duplicates
    let unique: HashSet<u64> = pts_results.into_iter().collect();
    assert_eq!(unique.len(), concurrency);

    // Final database state check
    let current_pts = engine.current_pts(chat_id).expect("current_pts query");
    assert_eq!(current_pts, concurrency as u64);
}

#[tokio::test]
async fn test_high_concurrency_pts_multi_chat_500_tasks() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open db"));
    create_test_user(&db, 1, "alice");

    let num_chats: usize = 10;
    let tasks_per_chat: usize = 50;
    let total_tasks = num_chats * tasks_per_chat;

    for c in 0..num_chats {
        create_test_chat(&db, 2000 + c as i64, 1, &format!("Chat {}", c));
    }

    let engine = Arc::new(PtsEngine::new(db.clone()));
    let barrier = Arc::new(Barrier::new(total_tasks));

    let mut handles = Vec::new();
    for c in 0..num_chats {
        let chat_id = ChatId(2000 + c as i64);
        for i in 0..tasks_per_chat {
            let engine_clone = engine.clone();
            let barrier_clone = barrier.clone();
            let handle = tokio::spawn(async move {
                barrier_clone.wait().await;
                let msg = MessageItem::new(
                    MsgId(i as i64 + 1),
                    chat_id,
                    UserId(1),
                    PeerId::Chat(chat_id),
                    MessageContent::Text {
                        text: format!("Chat {} update {}", chat_id.0, i),
                        entities: Vec::new(),
                    },
                );
                let (pts, _) = engine_clone
                    .advance_pts(chat_id, 1, UpdateEvent::NewMessage(msg))
                    .expect("advance_pts must succeed");
                (chat_id, pts)
            });
            handles.push(handle);
        }
    }

    let mut chat_pts: std::collections::HashMap<ChatId, Vec<u64>> = std::collections::HashMap::new();
    for handle in handles {
        let (chat_id, pts) = handle.await.expect("task join failed");
        chat_pts.entry(chat_id).or_default().push(pts);
    }

    assert_eq!(chat_pts.len(), num_chats);
    let expected: Vec<u64> = (1..=(tasks_per_chat as u64)).collect();

    for c in 0..num_chats {
        let chat_id = ChatId(2000 + c as i64);
        let mut list = chat_pts.remove(&chat_id).expect("chat results exist");
        list.sort_unstable();
        assert_eq!(list, expected, "PTS for chat {} must be contiguous 1..=50", chat_id.0);

        let final_pts = engine.current_pts(chat_id).expect("current_pts");
        assert_eq!(final_pts, tasks_per_chat as u64);
    }
}

// -----------------------------------------------------------------------------
// TASK 2: Adversarial Duplicate Nonce Attacks
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_adversarial_duplicate_nonce_burst_barrier() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open db"));
    create_test_user(&db, 42, "charlie");
    create_test_chat(&db, 3001, 42, "Dedup Adversarial Chat");

    let engine = PtsEngine::new(db.clone());
    let dedup = MessageNonceDedup::new(1000, 60_000);
    let pipeline = Arc::new(MessagePipeline::new(dedup, engine.clone()));

    let nonce = 777_888_999_111u64;
    let concurrency = 100;
    let barrier = Arc::new(Barrier::new(concurrency));

    let mut handles = Vec::new();
    for i in 0..concurrency {
        let pipeline_clone = pipeline.clone();
        let barrier_clone = barrier.clone();
        let msg = MessageItem::new(
            MsgId(0),
            ChatId(3001),
            UserId(42),
            PeerId::Chat(ChatId(3001)),
            MessageContent::Text {
                text: format!("Duplicate nonce attempt {}", i),
                entities: Vec::new(),
            },
        )
        .with_client_nonce(nonce);

        handles.push(tokio::spawn(async move {
            barrier_clone.wait().await;
            pipeline_clone.process_incoming_message(msg)
        }));
    }

    let mut results = Vec::new();
    for h in handles {
        let res = h.await.expect("task join failed");
        results.push(res);
    }

    // Verify all tasks returned Ok
    for res in &results {
        assert!(res.is_ok(), "Expected Ok for idempotent send, got: {:?}", res);
    }

    // Verify exactly 1 row in messages table
    let message_rows: i64 = db
        .pool()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE chat_id = 3001",
                [],
                |row| row.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("query messages table");
    assert_eq!(message_rows, 1, "There must be exactly 1 message row in the database");

    // Verify exactly 1 row in message_dedup table
    let dedup_rows: i64 = db
        .pool()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM message_dedup WHERE sender_id = 42 AND client_nonce = ?1",
                rusqlite::params![nonce as i64],
                |row| row.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("query dedup table");
    assert_eq!(dedup_rows, 1, "There must be exactly 1 entry in message_dedup");

    // Verify all returned updates have pts = 1 and same message id
    for res in results {
        let update = res.unwrap();
        assert_eq!(update.pts, 1, "Returned update pts must be 1");
        if let UpdateEvent::NewMessage(m) = update.event {
            assert_eq!(m.id, MsgId(1));
            assert_eq!(m.client_nonce, Some(nonce));
        } else {
            panic!("Expected NewMessage update event");
        }
    }

    // Verify current PTS of chat is 1
    let chat_pts = engine.current_pts(ChatId(3001)).expect("current_pts");
    assert_eq!(chat_pts, 1, "Chat PTS must not have advanced beyond 1");
}

#[tokio::test]
async fn test_adversarial_mixed_nonces_multi_sender_burst() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open db"));
    let chat_id = ChatId(3002);
    create_test_user(&db, 100, "user100");
    create_test_user(&db, 200, "user200");
    create_test_chat(&db, 3002, 100, "Multi-Sender Burst Chat");

    let engine = PtsEngine::new(db.clone());
    let dedup = MessageNonceDedup::new(1000, 60_000);
    let pipeline = Arc::new(MessagePipeline::new(dedup, engine.clone()));

    // 2 senders, each generates 5 distinct nonces, each nonce is sent 4 times concurrently (40 tasks)
    let senders = [100u64, 200u64];
    let nonces_per_sender = 5;
    let burst_per_nonce = 4;
    let total_tasks = senders.len() * nonces_per_sender * burst_per_nonce;
    let barrier = Arc::new(Barrier::new(total_tasks));

    let mut handles = Vec::new();
    for &sender_id in &senders {
        for n_idx in 0..nonces_per_sender {
            let nonce = (sender_id * 1000) + n_idx as u64;
            for burst_idx in 0..burst_per_nonce {
                let pipeline_clone = pipeline.clone();
                let barrier_clone = barrier.clone();
                let msg = MessageItem::new(
                    MsgId(0),
                    chat_id,
                    UserId(sender_id),
                    PeerId::Chat(chat_id),
                    MessageContent::Text {
                        text: format!("Sender {} Nonce {} Try {}", sender_id, nonce, burst_idx),
                        entities: Vec::new(),
                    },
                )
                .with_client_nonce(nonce);

                handles.push(tokio::spawn(async move {
                    barrier_clone.wait().await;
                    pipeline_clone.process_incoming_message(msg)
                }));
            }
        }
    }

    for h in handles {
        let res = h.await.expect("task join failed");
        assert!(res.is_ok(), "Every concurrent burst send must succeed");
    }

    // Exactly 10 distinct messages should be persisted (2 senders * 5 nonces)
    let msg_count: i64 = db
        .pool()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE chat_id = 3002",
                [],
                |row| row.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("count messages");
    assert_eq!(msg_count, 10);

    // Final chat PTS should be exactly 10
    let final_pts = engine.current_pts(chat_id).expect("current_pts");
    assert_eq!(final_pts, 10);
}

// -----------------------------------------------------------------------------
// TASK 3: Catch-up difference edge cases (pts >= current, window, behind floor)
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_catchup_diff_edge_cases() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open db"));
    create_test_user(&db, 1, "sync_user");
    create_test_chat(&db, 4001, 1, "Catchup Edge Cases Chat");

    let engine = PtsEngine::new(db.clone());
    let diff_service = SyncDifferenceService::new(db.clone());
    let chat_id = ChatId(4001);

    // Advance 50 updates
    for i in 1..=50 {
        let msg = MessageItem::new(
            MsgId(i),
            chat_id,
            UserId(1),
            PeerId::Chat(chat_id),
            MessageContent::Text {
                text: format!("Update {}", i),
                entities: Vec::new(),
            },
        );
        engine
            .advance_pts(chat_id, 1, UpdateEvent::NewMessage(msg))
            .expect("advance");
    }

    assert_eq!(engine.current_pts(chat_id).unwrap(), 50);

    // Case 1: pts == current_pts (50)
    let req_eq = SyncGetDifferenceRequest::new(chat_id, 50, 10);
    let resp_eq = diff_service.get_difference(&req_eq).expect("get_difference");
    assert_eq!(resp_eq.pts, 50);
    assert_eq!(resp_eq.current_pts, 50);
    assert!(resp_eq.new_messages.is_empty());
    assert!(!resp_eq.has_more);
    assert!(!resp_eq.snapshot_required);

    // Case 2: pts > current_pts (client is ahead, e.g. 55)
    let req_ahead = SyncGetDifferenceRequest::new(chat_id, 55, 10);
    let resp_ahead = diff_service.get_difference(&req_ahead).expect("get_difference");
    assert_eq!(resp_ahead.pts, 50);
    assert_eq!(resp_ahead.current_pts, 50);
    assert!(resp_ahead.new_messages.is_empty());
    assert!(!resp_ahead.has_more);
    assert!(!resp_ahead.snapshot_required);

    // Case 3: Limit boundary clamping
    // 3a. When limit=0 and pts_total_limit=Some(0), .clamp(1, 100) clamps to 1
    let req_zero_limit = SyncGetDifferenceRequest::new(chat_id, 0, 0);
    let resp_zero_limit = diff_service.get_difference(&req_zero_limit).expect("limit 0");
    assert_eq!(resp_zero_limit.new_messages.len(), 1, "Limit 0 must clamp to 1 message minimum");
    assert_eq!(resp_zero_limit.pts, 1);
    assert!(resp_zero_limit.has_more);

    // 3b. When pts_total_limit is None and limit is 0, defaults to 100 (fetches all 50)
    let req_default_limit = SyncGetDifferenceRequest {
        chat_id,
        pts: 0,
        pts_total_limit: None,
        limit: 0,
        date: 0,
        filter: None,
    };
    let resp_default_limit = diff_service.get_difference(&req_default_limit).expect("default limit");
    assert_eq!(resp_default_limit.new_messages.len(), 50, "Unspecified limit must default to 100");
    assert_eq!(resp_default_limit.pts, 50);
    assert!(!resp_default_limit.has_more);

    // 3c. When limit exceeds 100 (e.g. 500), clamped to max 100
    let req_high_limit = SyncGetDifferenceRequest::new(chat_id, 0, 500);
    let resp_high_limit = diff_service.get_difference(&req_high_limit).expect("limit 500");
    assert_eq!(resp_high_limit.new_messages.len(), 50); // fetches all 50 (within 100 ceiling)

    // Case 4: Prune journal to simulate floor retention
    // Keep 15 updates with 30-day age cutoff: floor should become 36 (50 - 15 + 1)
    let thirty_days_ms = 30 * 24 * 3600 * 1000;
    engine.prune_journal(chat_id, 15, thirty_days_ms).expect("prune");
    let floor = engine.journal_floor_pts(chat_id).expect("floor query");
    assert_eq!(floor, 36);

    // Client requests behind floor: pts = 30 (< 36 - 1)
    let req_behind = SyncGetDifferenceRequest::new(chat_id, 30, 10);
    let resp_behind = diff_service.get_difference(&req_behind).expect("req behind floor");
    assert!(resp_behind.snapshot_required, "Behind floor MUST require snapshot");
    assert!(resp_behind.has_more);
    assert!(resp_behind.new_messages.is_empty());

    // Boundary test: Client requests exactly at floor boundary (pts = 35, where floor is 36)
    // Note: client has pts 35, wants updates starting from 36. Since 36 IS in journal, no snapshot required!
    let req_boundary = SyncGetDifferenceRequest::new(chat_id, 35, 10);
    let resp_boundary = diff_service.get_difference(&req_boundary).expect("req boundary");
    assert!(!resp_boundary.snapshot_required, "pts = floor - 1 must NOT require snapshot");
    assert_eq!(resp_boundary.new_messages.len(), 10);
    assert_eq!(resp_boundary.pts, 45);
    assert!(resp_boundary.has_more);

    // Client requests one below boundary (pts = 34, wants from 35, but 35 was pruned): snapshot_required = true
    let req_below_boundary = SyncGetDifferenceRequest::new(chat_id, 34, 10);
    let resp_below_boundary = diff_service.get_difference(&req_below_boundary).expect("req below boundary");
    assert!(resp_below_boundary.snapshot_required, "pts = floor - 2 MUST require snapshot");
}

// -----------------------------------------------------------------------------
// TASK 4: Foreign Key Cascading & Constraints
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_foreign_key_constraints_comprehensive() {
    let db = ServerDatabase::open_in_memory().expect("open db");

    let now = chrono::Utc::now().timestamp_millis();

    // 1. Message with non-existent chat_id -> must fail
    create_test_user(&db, 1, "valid_user");
    let invalid_msg = MessageRecord {
        chat_id: ChatId(99999), // does not exist
        message_id: MsgId(1),
        sender_id: UserId(1),
        to_peer_raw: 0,
        topic_id: None,
        reply_to_id: None,
        pts: 1,
        date_ms: now,
        content_json: "{}".to_string(),
        media_type: None,
        flags_bitmask: 0,
        client_nonce: None,
        edit_date_ms: None,
        deleted_at_ms: None,
        reply_to_json: None,
        forward_info_json: None,
        reactions_json: None,
    };
    assert!(
        db.save_message(&invalid_msg).is_err(),
        "Message with invalid chat_id must fail foreign key check"
    );

    // 2. Message with non-existent sender_id -> must fail
    create_test_chat(&db, 5001, 1, "Valid Chat");
    let invalid_sender_msg = MessageRecord {
        chat_id: ChatId(5001),
        message_id: MsgId(1),
        sender_id: UserId(99999), // does not exist
        to_peer_raw: 0,
        topic_id: None,
        reply_to_id: None,
        pts: 1,
        date_ms: now,
        content_json: "{}".to_string(),
        media_type: None,
        flags_bitmask: 0,
        client_nonce: None,
        edit_date_ms: None,
        deleted_at_ms: None,
        reply_to_json: None,
        forward_info_json: None,
        reactions_json: None,
    };
    assert!(
        db.save_message(&invalid_sender_msg).is_err(),
        "Message with invalid sender_id must fail foreign key check"
    );

    // 3. Session with non-existent user_id -> must fail
    let invalid_session_result = db.pool().with_conn(|conn| {
        conn.execute(
            r#"
            INSERT INTO sessions (
                session_id, user_id, device_id, client_type, app_version,
                ip_address, location_desc, account_generation, token_hash,
                created_at_ms, last_active_ms, expires_at_ms, revoked_at_ms
            ) VALUES ('sess_invalid', 99999, 'dev1', 'desktop', '1.0', NULL, NULL, 1, 'hash', 0, 0, 0, NULL)
            "#,
            [],
        )
        .map_err(|e| TelegramError::Database(e.to_string()))
    });
    assert!(
        invalid_session_result.is_err(),
        "Session with invalid user_id must fail foreign key check"
    );

    // 4. Chat member with non-existent user_id -> must fail
    let invalid_member = ChatMemberRecord {
        chat_id: ChatId(5001),
        user_id: UserId(99999), // does not exist
        role: "member".to_string(),
        permissions_bitmask: 0,
        admin_title: None,
        admin_rights: 0,
        banned_rights: 0,
        until_date_ms: None,
        joined_at_ms: now,
        invited_by: None,
        updated_at_ms: now,
    };
    assert!(
        db.add_chat_member(&invalid_member).is_err(),
        "ChatMember with invalid user_id must fail foreign key check"
    );

    // 5. Forum topic with non-existent chat_id -> must fail
    let invalid_topic = ForumTopicRecord {
        chat_id: ChatId(88888), // does not exist
        topic_id: TopicId(1),
        title: "Ghost topic".to_string(),
        icon_color: 0,
        icon_emoji_id: None,
        creator_id: UserId(1),
        is_pinned: false,
        is_closed: false,
        is_hidden: false,
        last_message_id: None,
        unread_count: 0,
        created_at_ms: now,
    };
    assert!(
        db.create_forum_topic(&invalid_topic).is_err(),
        "ForumTopic with invalid chat_id must fail foreign key check"
    );

    // 6. Cascading Deletion Verification
    // Add valid member and message to chat 5001
    create_test_user(&db, 2, "member2");
    let valid_member = ChatMemberRecord {
        chat_id: ChatId(5001),
        user_id: UserId(2),
        role: "member".to_string(),
        permissions_bitmask: 0,
        admin_title: None,
        admin_rights: 0,
        banned_rights: 0,
        until_date_ms: None,
        joined_at_ms: now,
        invited_by: None,
        updated_at_ms: now,
    };
    db.add_chat_member(&valid_member).expect("add valid member");

    let valid_msg = MessageRecord {
        chat_id: ChatId(5001),
        message_id: MsgId(1),
        sender_id: UserId(1),
        to_peer_raw: 0,
        topic_id: None,
        reply_to_id: None,
        pts: 1,
        date_ms: now,
        content_json: "{}".to_string(),
        media_type: None,
        flags_bitmask: 0,
        client_nonce: None,
        edit_date_ms: None,
        deleted_at_ms: None,
        reply_to_json: None,
        forward_info_json: None,
        reactions_json: None,
    };
    db.save_message(&valid_msg).expect("save valid message");

    // Deleting chat 5001 should cascade delete all messages and chat_members
    db.pool()
        .with_conn(|conn| {
            conn.execute("DELETE FROM chats WHERE chat_id = 5001", [])
                .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("delete chat");

    let remaining_msgs: i64 = db
        .pool()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE chat_id = 5001",
                [],
                |row| row.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("query remaining messages");
    assert_eq!(remaining_msgs, 0, "Messages must be cascade deleted when chat is deleted");

    let remaining_members: i64 = db
        .pool()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM chat_members WHERE chat_id = 5001",
                [],
                |row| row.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("query remaining members");
    assert_eq!(remaining_members, 0, "Chat members must be cascade deleted when chat is deleted");
}
