use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, TopicId, UserId};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use gpui_telegram_proto::sync::{SyncGetDifferenceRequest, UpdateEvent};
use gpui_telegram_server::db::{
    ChatMemberRecord, ChatRecord, ForumTopicRecord, ReadStateRecord, ServerDatabase, UserRecord,
};
use gpui_telegram_server::messaging::{MessageNonceDedup, MessagePipeline};
use gpui_telegram_server::sync::{PtsEngine, SyncDifferenceService};
use std::collections::HashSet;
use std::sync::Arc;

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

#[tokio::test]
async fn test_schema_creation_and_migrations() {
    let db = ServerDatabase::open_in_memory().expect("Failed to open in-memory database");

    // Verify all 10 core tables exist in sqlite_master
    let table_names = [
        "users",
        "sessions",
        "chats",
        "chat_members",
        "forum_topics",
        "messages",
        "pts_sequences",
        "pts_updates",
        "message_dedup",
        "read_states",
    ];

    for table in table_names {
        let exists: bool = db
            .pool()
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get(0),
                )
                .map_err(|e| TelegramError::Database(e.to_string()))
            })
            .expect("Failed to query sqlite_master");

        assert!(exists, "Expected table {} to exist in database", table);
    }

    // Verify migrations can be re-run safely (idempotent)
    db.pool()
        .with_conn(|conn| gpui_telegram_server::db::run_migrations(conn))
        .expect("Re-running migrations should succeed without errors");
}

#[tokio::test]
async fn test_foreign_key_enforcement() {
    let db = ServerDatabase::open_in_memory().expect("Failed to open database");

    // Attempting to insert a chat with a nonexistent creator_id must fail due to FOREIGN KEY constraint
    let now = chrono::Utc::now().timestamp_millis();
    let invalid_chat = ChatRecord {
        chat_id: ChatId(999),
        chat_type: 1,
        peer_type: 1,
        title: "Invalid Group".to_string(),
        username: None,
        about: None,
        creator_id: UserId(88888), // Does not exist
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

    let result = db.create_chat(&invalid_chat);
    assert!(result.is_err(), "Inserting chat with invalid creator_id must violate foreign key");

    // Now insert the creator user first
    create_test_user(&db, 88888, "valid_creator");

    // Inserting the chat now succeeds
    let valid_result = db.create_chat(&invalid_chat);
    assert!(valid_result.is_ok(), "Inserting chat with existing creator must succeed");
}

#[tokio::test]
async fn test_atomic_monotonic_pts_concurrency_50_tasks() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("Failed to open database"));
    create_test_user(&db, 1, "alice");
    create_test_chat(&db, 100, 1, "Concurrency Chat");

    let engine = Arc::new(PtsEngine::new(db.clone()));
    let chat_id = ChatId(100);

    let mut handles = Vec::new();
    let task_count = 50;

    for i in 0..task_count {
        let engine_clone = engine.clone();
        let handle = tokio::spawn(async move {
            let msg = MessageItem::new(
                MsgId(i as i64 + 1),
                chat_id,
                UserId(1),
                PeerId::Chat(chat_id),
                MessageContent::Text {
                    text: format!("Message concurrent #{}", i),
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
        let (pts, update) = handle.await.expect("Task panicked");
        assert_eq!(pts, update.pts);
        pts_results.push(pts);
    }

    // Invariant 1: Exactly 50 updates generated
    assert_eq!(pts_results.len(), task_count);

    // Invariant 2: Monotonic contiguity - zero gaps, zero duplicates
    pts_results.sort_unstable();
    let expected: Vec<u64> = (1..=(task_count as u64)).collect();
    assert_eq!(pts_results, expected, "PTS sequences must be strictly contiguous from 1 to 50");

    let unique_pts: HashSet<u64> = pts_results.into_iter().collect();
    assert_eq!(unique_pts.len(), task_count, "There must be zero duplicate PTS allocations");

    // Invariant 3: Final database PTS matches expected count
    let current_pts = engine.current_pts(chat_id).expect("current_pts query failed");
    assert_eq!(current_pts, task_count as u64);
}

#[tokio::test]
async fn test_sync_get_difference_catchup() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("Failed to open database"));
    create_test_user(&db, 1, "bob");
    create_test_chat(&db, 200, 1, "Sync Diff Chat");

    let engine = PtsEngine::new(db.clone());
    let diff_service = SyncDifferenceService::new(db.clone());
    let chat_id = ChatId(200);

    // Generate 20 events
    for i in 1..=20 {
        let msg = MessageItem::new(
            MsgId(i),
            chat_id,
            UserId(1),
            PeerId::Chat(chat_id),
            MessageContent::Text {
                text: format!("Item #{}", i),
                entities: Vec::new(),
            },
        );
        engine
            .advance_pts(chat_id, 1, UpdateEvent::NewMessage(msg))
            .expect("advance_pts failed");
    }

    // 1. Catch-up query across range pts=5, limit=10
    let req1 = SyncGetDifferenceRequest::new(chat_id, 5, 10);
    let resp1 = diff_service.get_difference(&req1).expect("get_difference failed");

    assert_eq!(resp1.new_messages.len(), 10);
    assert_eq!(resp1.pts, 15);
    assert!(resp1.has_more, "has_more should be true when more updates remain");
    assert!(!resp1.snapshot_required);

    // 2. Second page from pts=15, limit=10
    let req2 = SyncGetDifferenceRequest::new(chat_id, 15, 10);
    let resp2 = diff_service.get_difference(&req2).expect("get_difference failed");

    assert_eq!(resp2.new_messages.len(), 5);
    assert_eq!(resp2.pts, 20);
    assert!(!resp2.has_more, "has_more should be false when caught up to current_pts");
    assert!(!resp2.snapshot_required);

    // 3. Client already caught up at pts=20
    let req3 = SyncGetDifferenceRequest::new(chat_id, 20, 10);
    let resp3 = diff_service.get_difference(&req3).expect("get_difference failed");
    assert_eq!(resp3.new_messages.len(), 0);
    assert!(!resp3.has_more);
    assert!(!resp3.snapshot_required);

    // 4. Test journal floor fallback: manually prune updates below pts=15
    engine.prune_journal(chat_id, 5, 0).expect("prune_journal failed");
    let floor = engine.journal_floor_pts(chat_id).expect("journal_floor_pts failed");
    assert!(floor > 5, "Floor should advance after pruning");

    // Client requests from pts=2 (which is below floor)
    let req4 = SyncGetDifferenceRequest::new(chat_id, 2, 10);
    let resp4 = diff_service.get_difference(&req4).expect("get_difference failed");
    assert!(resp4.snapshot_required, "Snapshot should be required when client pts < floor");
    assert!(resp4.has_more);
}

#[tokio::test]
async fn test_idempotent_resend_duplicate_client_nonce() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("Failed to open database"));
    create_test_user(&db, 42, "charlie");
    create_test_chat(&db, 300, 42, "Dedup Chat");

    let engine = PtsEngine::new(db.clone());
    let dedup = MessageNonceDedup::new(100, 60_000);
    let pipeline = MessagePipeline::new(dedup.clone(), engine.clone());

    let nonce = 123456789012345u64;
    let original_msg = MessageItem::new(
        MsgId(0), // client ID 0 -> will receive server ID 1
        ChatId(300),
        UserId(42),
        PeerId::Chat(ChatId(300)),
        MessageContent::Text {
            text: "Hello, durable idempotency!".to_string(),
            entities: Vec::new(),
        },
    )
    .with_client_nonce(nonce);

    // 1. Initial send
    let first_update = pipeline
        .process_incoming_message(original_msg.clone())
        .expect("First send must succeed");

    let first_pts = first_update.pts;
    assert_eq!(first_pts, 1);
    assert_eq!(first_update.pts_count, 1);

    let committed_msg_id = match &first_update.event {
        UpdateEvent::NewMessage(m) => m.id,
        _ => panic!("Expected NewMessage update"),
    };
    assert_eq!(committed_msg_id, MsgId(1));

    // 2. Duplicate resend with identical nonce (Tier 1 memory hit)
    let second_update = pipeline
        .process_incoming_message(original_msg.clone())
        .expect("Duplicate resend must succeed idempotently without error");

    assert_eq!(second_update.pts, first_pts, "Replayed message must retain original PTS");
    assert_eq!(second_update.pts_count, 0, "Idempotent replay must not advance pts_count");
    if let UpdateEvent::NewMessage(m) = &second_update.event {
        assert_eq!(m.id, committed_msg_id, "Replayed message must retain original message_id");
    } else {
        panic!("Expected NewMessage event on replay");
    }

    // 3. Test Tier 2 durable database lookup (clear memory cache)
    let fresh_dedup = MessageNonceDedup::new(100, 60_000);
    let pipeline2 = MessagePipeline::new(fresh_dedup, engine.clone());

    let third_update = pipeline2
        .process_incoming_message(original_msg.clone())
        .expect("Tier 2 DB dedup must succeed idempotently");

    assert_eq!(third_update.pts, first_pts, "Tier 2 replayed message must retain original PTS");
    assert_eq!(third_update.pts_count, 0);

    // 4. Verify message table only has 1 committed row
    let row_count: i64 = db
        .pool()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE chat_id = 300",
                [],
                |row| row.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("query failed");
    assert_eq!(row_count, 1, "There must be exactly one message row in database");

    // 5. Verify current PTS is still 1
    let current_pts = engine.current_pts(ChatId(300)).expect("query current_pts failed");
    assert_eq!(current_pts, 1, "PTS counter must not have advanced for duplicate sends");
}

#[tokio::test]
async fn test_concurrent_duplicate_nonce_race() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("Failed to open database"));
    create_test_user(&db, 10, "david");
    create_test_chat(&db, 400, 10, "Race Chat");

    let engine = PtsEngine::new(db.clone());
    let dedup = MessageNonceDedup::new(1000, 60_000);
    let pipeline = Arc::new(MessagePipeline::new(dedup, engine.clone()));

    let nonce = 9876543210123u64;
    let mut handles = Vec::new();

    for _ in 0..10 {
        let pipeline_clone = pipeline.clone();
        let msg = MessageItem::new(
            MsgId(0),
            ChatId(400),
            UserId(10),
            PeerId::Chat(ChatId(400)),
            MessageContent::Text {
                text: "Racing duplicate nonce".to_string(),
                entities: Vec::new(),
            },
        )
        .with_client_nonce(nonce);

        handles.push(tokio::spawn(async move {
            pipeline_clone
                .process_incoming_message(msg)
                .expect("Every task must return Ok")
        }));
    }

    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.expect("task join failed"));
    }

    // All tasks must agree on PTS = 1
    for r in &results {
        assert_eq!(r.pts, 1);
    }

    // Database must contain exactly 1 message
    let row_count: i64 = db
        .pool()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE chat_id = 400",
                [],
                |row| row.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("query failed");
    assert_eq!(row_count, 1);
}

#[tokio::test]
async fn test_forum_topics_and_read_states() {
    let db = ServerDatabase::open_in_memory().expect("Failed to open database");
    create_test_user(&db, 1, "owner");
    create_test_chat(&db, 500, 1, "Forum Supergroup");

    // Add chat member
    let member = ChatMemberRecord {
        chat_id: ChatId(500),
        user_id: UserId(1),
        role: "owner".to_string(),
        permissions_bitmask: 0xFFFFFFFF,
        admin_title: Some("Founder".to_string()),
        admin_rights: 0xFF,
        banned_rights: 0,
        until_date_ms: None,
        joined_at_ms: chrono::Utc::now().timestamp_millis(),
        invited_by: None,
        updated_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    db.add_chat_member(&member).expect("add_chat_member failed");
    let fetched_member = db
        .get_chat_member(ChatId(500), UserId(1))
        .expect("get_chat_member failed")
        .expect("Member should exist");
    assert_eq!(fetched_member.role, "owner");

    // Create forum topics: General topic (ID = 1) and custom topic
    let general_topic = ForumTopicRecord {
        chat_id: ChatId(500),
        topic_id: TopicId(ForumTopicRecord::GENERAL_TOPIC_ID),
        title: "General".to_string(),
        icon_color: 0x00FF00,
        icon_emoji_id: None,
        creator_id: UserId(1),
        is_pinned: false,
        is_closed: false,
        is_hidden: false,
        last_message_id: None,
        unread_count: 0,
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    db.create_forum_topic(&general_topic)
        .expect("create general topic failed");
    assert!(general_topic.is_general());

    let custom_topic = ForumTopicRecord {
        chat_id: ChatId(500),
        topic_id: TopicId(2),
        title: "Announcements".to_string(),
        icon_color: 0xFF0000,
        icon_emoji_id: Some(12345),
        creator_id: UserId(1),
        is_pinned: true,
        is_closed: false,
        is_hidden: false,
        last_message_id: None,
        unread_count: 0,
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    db.create_forum_topic(&custom_topic)
        .expect("create custom topic failed");

    let topics = db.list_forum_topics(ChatId(500)).expect("list_forum_topics failed");
    assert_eq!(topics.len(), 2);
    // Pinned topic should be listed first
    assert_eq!(topics[0].topic_id, TopicId(2));
    assert!(topics[0].is_pinned);

    // Read State test
    let read_state = ReadStateRecord {
        chat_id: ChatId(500),
        user_id: UserId(1),
        topic_id: TopicId(2),
        inbox_max_id: MsgId(10),
        outbox_max_id: MsgId(5),
        unread_count: 3,
        updated_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    db.update_read_state(&read_state).expect("update_read_state failed");

    let fetched_read_state = db
        .get_read_state(ChatId(500), UserId(1), TopicId(2))
        .expect("get_read_state failed")
        .expect("Read state should exist");
    assert_eq!(fetched_read_state.inbox_max_id, MsgId(10));
    assert_eq!(fetched_read_state.unread_count, 3);
}

#[tokio::test]
async fn test_message_edit_and_soft_delete() {
    let pipeline = MessagePipeline::open_in_memory().expect("open_in_memory failed");
    create_test_user(pipeline.db(), 7, "eve");
    create_test_chat(pipeline.db(), 600, 7, "Edit Delete Chat");

    let original = MessageItem::new(
        MsgId(0),
        ChatId(600),
        UserId(7),
        PeerId::Chat(ChatId(600)),
        MessageContent::Text {
            text: "Original text".to_string(),
            entities: Vec::new(),
        },
    );

    let send_update = pipeline
        .process_incoming_message(original)
        .expect("process_incoming_message failed");
    let msg_id = match send_update.event {
        UpdateEvent::NewMessage(m) => m.id,
        _ => panic!("Expected NewMessage"),
    };
    assert_eq!(send_update.pts, 1);

    // Edit message
    let edit_update = pipeline
        .edit_message(
            ChatId(600),
            msg_id,
            MessageContent::Text {
                text: "Edited text!".to_string(),
                entities: Vec::new(),
            },
        )
        .expect("edit_message failed");
    assert_eq!(edit_update.pts, 2);
    if let UpdateEvent::EditMessage(m) = edit_update.event {
        assert_eq!(m.id, msg_id);
        assert!(m.flags.is_edited);
    } else {
        panic!("Expected EditMessage event");
    }

    // Delete message
    let delete_update = pipeline
        .delete_messages(ChatId(600), vec![msg_id], true)
        .expect("delete_messages failed");
    assert_eq!(delete_update.pts, 3);
    if let UpdateEvent::DeleteMessages(tombstone) = delete_update.event {
        assert!(tombstone.is_deleted(msg_id));
        assert!(tombstone.for_everyone);
    } else {
        panic!("Expected DeleteMessages event");
    }

    // Check soft-deleted message in DB
    let rec = pipeline
        .db()
        .get_message(ChatId(600), msg_id)
        .expect("get_message failed")
        .expect("Message record should exist");
    assert!(rec.deleted_at_ms.is_some(), "Message should be marked soft-deleted");
}
