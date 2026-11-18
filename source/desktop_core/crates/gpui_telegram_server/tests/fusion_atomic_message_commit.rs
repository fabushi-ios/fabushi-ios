use chrono::{TimeZone, Utc};
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::{ChatId, MessageFlags, MsgId, PeerId, TopicId, UserId};
use gpui_telegram_proto::message::{
    ForwardInfo, MessageContent, MessageItem, ReactionCount, ReplyHeader,
};
use gpui_telegram_proto::sync::{SyncGetDifferenceRequest, UpdateEvent};
use gpui_telegram_server::db::{ChatRecord, ForumTopicRecord, ServerDatabase, UserRecord};
use gpui_telegram_server::messaging::{MessageNonceDedup, MessagePipeline};
use gpui_telegram_server::sync::{PtsEngine, SyncDifferenceService};
use rusqlite::params;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Barrier;

/// RAII temporary directory guard for the reopen durability test.
/// Strictly cleans up the directory and all SQLite/WAL/SHM files on drop.
struct TempDbDirGuard {
    dir: PathBuf,
    db_path: PathBuf,
}

impl TempDbDirGuard {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "gpui_fus_s002_atomic_msg_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir for reopen test");
        let db_path = dir.join("fusion_atomic_messages.db");
        Self { dir, db_path }
    }
}

impl Drop for TempDbDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn seed_user(db: &ServerDatabase, user_id: u64, username: &str) -> UserRecord {
    let now = Utc::now().timestamp_millis();
    let user = UserRecord {
        user_id: UserId(user_id),
        username: Some(username.to_string()),
        phone: Some(format!("+1555{:07}", user_id)),
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
    db.create_user(&user).expect("seed user");
    user
}

fn seed_chat(db: &ServerDatabase, chat_id: i64, creator_id: u64, title: &str) -> ChatRecord {
    let now = Utc::now().timestamp_millis();
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
    db.create_chat(&chat).expect("seed chat");
    chat
}

fn count_rows(db: &ServerDatabase, sql: &str, chat_id: i64) -> i64 {
    db.pool()
        .with_conn(|conn| {
            conn.query_row(sql, params![chat_id], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("count_rows query")
}

#[tokio::test]
async fn test_forced_insert_failure_rolls_back_all_tables_and_recovers_on_retry() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open in-memory db"));
    seed_user(&db, 101, "alice");
    let initial_chat = seed_chat(&db, 500, 101, "Atomic Rollback Chat");

    let engine = PtsEngine::new(db.clone());
    let dedup = MessageNonceDedup::new(256, 60_000);
    let pipeline = MessagePipeline::new(dedup.clone(), engine.clone());

    // Phase 1: Install a BEFORE INSERT trigger on `message_dedup`.
    // At this point in the transaction, `pts_sequences`, `pts_updates`, `messages`, and `chats`
    // metadata have all been written inside the transaction and must all roll back together.
    db.pool()
        .with_conn(|conn| {
            conn.execute_batch(
                r#"
                CREATE TRIGGER abort_on_message_dedup_insert
                BEFORE INSERT ON message_dedup
                BEGIN
                    SELECT RAISE(ABORT, 'simulated_dedup_insert_failure');
                END;
                "#,
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("install message_dedup trigger");

    let nonce = 900_001u64;
    let msg_date = Utc.timestamp_millis_opt(1_762_000_123_000).unwrap();
    let mut msg = MessageItem::new(
        MsgId::generate_client(1),
        ChatId(500),
        UserId(101),
        PeerId::Chat(ChatId(500)),
        MessageContent::plain_text("Must roll back atomically"),
    )
    .with_client_nonce(nonce);
    msg.date = msg_date;

    let err = pipeline
        .process_incoming_message(msg.clone())
        .expect_err("send must fail when message_dedup trigger aborts");
    assert!(
        matches!(err, TelegramError::Database(_)),
        "Expected Database error from trigger abort, got: {:?}",
        err
    );

    // Verify zero rows in messages, pts_updates, message_dedup, pts_sequences, and untouched chat metadata
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM messages WHERE chat_id = ?1", 500),
        0,
        "messages table must roll back"
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM pts_updates WHERE chat_id = ?1",
            500
        ),
        0,
        "pts_updates journal must roll back"
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM message_dedup WHERE chat_id = ?1",
            500
        ),
        0,
        "message_dedup table must roll back"
    );
    assert_eq!(
        engine.current_pts(ChatId(500)).expect("current_pts"),
        0,
        "pts_sequences must remain at 0 after rollback"
    );
    let chat_after_fail_1 = db
        .get_chat(ChatId(500))
        .expect("get_chat")
        .expect("chat exists");
    assert_eq!(chat_after_fail_1.pts, 0);
    assert_eq!(chat_after_fail_1.last_message_id, MsgId(0));
    assert_eq!(chat_after_fail_1.last_message_date_ms, 0);
    assert_eq!(
        chat_after_fail_1.updated_at_ms,
        initial_chat.updated_at_ms,
        "chat updated_at_ms must not advance on rolled back message"
    );
    assert!(
        dedup.is_empty(),
        "in-memory dedup cache must not publish an entry on transaction failure"
    );

    // Phase 2: Replace with a BEFORE INSERT trigger on `messages` (after `pts_sequences` and
    // `pts_updates` have been written in the transaction) and verify full rollback again.
    db.pool()
        .with_conn(|conn| {
            conn.execute_batch(
                r#"
                DROP TRIGGER abort_on_message_dedup_insert;
                CREATE TRIGGER abort_on_messages_insert
                BEFORE INSERT ON messages
                BEGIN
                    SELECT RAISE(ABORT, 'simulated_messages_insert_failure');
                END;
                "#,
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("swap trigger to messages");

    let err_2 = pipeline
        .process_incoming_message(msg.clone())
        .expect_err("send must fail when messages trigger aborts");
    assert!(matches!(err_2, TelegramError::Database(_)));

    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM messages WHERE chat_id = ?1", 500),
        0
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM pts_updates WHERE chat_id = ?1",
            500
        ),
        0
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM message_dedup WHERE chat_id = ?1",
            500
        ),
        0
    );
    assert_eq!(engine.current_pts(ChatId(500)).expect("current_pts"), 0);
    assert!(dedup.is_empty());

    // Phase 3: Drop trigger and retry the exact same message; it must commit cleanly with ID=1, PTS=1.
    db.pool()
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER abort_on_messages_insert;")
                .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("drop messages trigger");

    let committed = pipeline
        .process_incoming_message(msg.clone())
        .expect("retry after clearing trigger must succeed");
    assert_eq!(committed.chat_id, ChatId(500));
    assert_eq!(committed.pts, 1);
    assert_eq!(committed.pts_count, 1);
    let committed_id = match &committed.event {
        UpdateEvent::NewMessage(item) => item.id,
        other => panic!("expected NewMessage, got {:?}", other),
    };
    assert_eq!(committed_id, MsgId(1));

    // Verify all tables and chat metadata committed atomically
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM messages WHERE chat_id = ?1", 500),
        1
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM pts_updates WHERE chat_id = ?1",
            500
        ),
        1
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM message_dedup WHERE chat_id = ?1",
            500
        ),
        1
    );
    let chat_after_commit = db
        .get_chat(ChatId(500))
        .expect("get_chat")
        .expect("chat exists");
    assert_eq!(chat_after_commit.pts, 1);
    assert_eq!(chat_after_commit.last_message_id, MsgId(1));
    assert_eq!(
        chat_after_commit.last_message_date_ms,
        msg_date.timestamp_millis()
    );
    assert!(dedup.contains(101, nonce));
}

#[tokio::test]
async fn test_foreign_key_violation_rolls_back_pts_and_publishes_no_cache() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open db"));
    seed_user(&db, 10, "valid_sender");
    seed_chat(&db, 600, 10, "Existing Chat");

    let engine = PtsEngine::new(db.clone());
    let dedup = MessageNonceDedup::new(128, 60_000);
    let pipeline = MessagePipeline::new(dedup.clone(), engine.clone());

    // 1. Nonexistent chat_id must fail and leave pts_sequences/pts_updates/dedup untouched
    let bad_chat_msg = MessageItem::new(
        MsgId(0),
        ChatId(999_999),
        UserId(10),
        PeerId::Chat(ChatId(999_999)),
        MessageContent::plain_text("Missing chat"),
    )
    .with_client_nonce(1111);
    assert!(pipeline.process_incoming_message(bad_chat_msg).is_err());
    assert_eq!(engine.current_pts(ChatId(999_999)).unwrap(), 0);
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM pts_updates WHERE chat_id = ?1",
            999_999
        ),
        0
    );
    assert!(dedup.is_empty());

    // 2. Nonexistent sender_id in existing chat 600 must fail FK and roll back chat 600 PTS
    let bad_sender_msg = MessageItem::new(
        MsgId(0),
        ChatId(600),
        UserId(888_888),
        PeerId::Chat(ChatId(600)),
        MessageContent::plain_text("Missing sender"),
    )
    .with_client_nonce(2222);
    assert!(pipeline.process_incoming_message(bad_sender_msg).is_err());
    assert_eq!(engine.current_pts(ChatId(600)).unwrap(), 0);
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM pts_updates WHERE chat_id = ?1",
            600
        ),
        0
    );
    assert!(dedup.is_empty());
}

#[tokio::test]
async fn test_conflicting_retries_rejected_across_memory_and_sqlite_while_benign_fields_succeed() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open db"));
    seed_user(&db, 42, "sender_42");
    seed_chat(&db, 700, 42, "Chat 700");
    seed_chat(&db, 701, 42, "Chat 701");

    let topic = ForumTopicRecord {
        chat_id: ChatId(700),
        topic_id: TopicId(10),
        title: "Architecture".to_string(),
        icon_color: 1,
        icon_emoji_id: None,
        creator_id: UserId(42),
        is_pinned: false,
        is_closed: false,
        is_hidden: false,
        last_message_id: None,
        unread_count: 0,
        created_at_ms: Utc::now().timestamp_millis(),
    };
    db.create_forum_topic(&topic).expect("create topic");

    let engine = PtsEngine::new(db.clone());
    let dedup = MessageNonceDedup::new(256, 60_000);
    let pipeline = MessagePipeline::new(dedup, engine.clone());

    let nonce = 777_000_111u64;
    let mut base_msg = MessageItem::new(
        MsgId::generate_client(5),
        ChatId(700),
        UserId(42),
        PeerId::Chat(ChatId(700)),
        MessageContent::plain_text("Canonical client intent"),
    )
    .with_reply_to(ReplyHeader::new(MsgId(1)).with_quote("quoted"))
    .with_client_nonce(nonce);
    base_msg.topic_id = Some(TopicId(10));
    base_msg.forward_info = Some(ForwardInfo {
        from_id: PeerId::User(UserId(42)),
        from_name: Some("Origin".to_string()),
        date: 1_700_000_000,
        original_message_id: Some(MsgId(9)),
    });

    let first = pipeline
        .process_incoming_message(base_msg.clone())
        .expect("initial commit");
    assert_eq!(first.pts, 1);
    assert_eq!(first.pts_count, 1);

    // Verify forum topic last_message_id updated atomically as well
    let topics = db.list_forum_topics(ChatId(700)).expect("list topics");
    assert_eq!(topics[0].last_message_id, Some(MsgId(1)));

    // Benign retry variations (different client/server ID, different date, different flags, reactions)
    // must be treated as the SAME client semantic intent and return pts=1, pts_count=0.
    let mut benign_retry = base_msg.clone();
    benign_retry.id = MsgId(0);
    benign_retry.date = Utc::now() + chrono::Duration::seconds(300);
    benign_retry.edit_date = Some(Utc::now());
    benign_retry.flags = MessageFlags {
        is_out: true,
        is_silent: true,
        has_reactions: true,
        ..Default::default()
    };
    benign_retry.reactions = vec![ReactionCount::new("👍", 3, true)];

    // Check against Tier 1 memory dedup
    let replay_mem = pipeline
        .process_incoming_message(benign_retry.clone())
        .expect("benign retry must succeed on Tier 1 cache");
    assert_eq!(replay_mem.pts, 1);
    assert_eq!(replay_mem.pts_count, 0);

    // Check against a separately constructed pipeline instance (Tier 2 SQLite lookup)
    let pipeline_separate =
        MessagePipeline::new(MessageNonceDedup::new(256, 60_000), engine.clone());
    let replay_db = pipeline_separate
        .process_incoming_message(benign_retry)
        .expect("benign retry must succeed on Tier 2 SQLite");
    assert_eq!(replay_db.pts, 1);
    assert_eq!(replay_db.pts_count, 0);

    // Conflicting retries with same (sender, nonce) must fail on BOTH Tier 1 (memory) and Tier 2 (fresh pipeline)
    let fresh_pipeline_for_tier2 = || {
        MessagePipeline::new(MessageNonceDedup::new(256, 60_000), PtsEngine::new(db.clone()))
    };

    // 1. Different content
    let mut diff_content = base_msg.clone();
    diff_content.content = MessageContent::plain_text("Tampered content with same nonce");
    assert!(matches!(
        pipeline.process_incoming_message(diff_content.clone()),
        Err(TelegramError::Validation(_))
    ));
    assert!(matches!(
        fresh_pipeline_for_tier2().process_incoming_message(diff_content),
        Err(TelegramError::Validation(_))
    ));

    // 2. Different chat_id
    let mut diff_chat = base_msg.clone();
    diff_chat.chat_id = ChatId(701);
    diff_chat.to_peer = PeerId::Chat(ChatId(701));
    assert!(matches!(
        pipeline.process_incoming_message(diff_chat.clone()),
        Err(TelegramError::Validation(_))
    ));
    assert!(matches!(
        fresh_pipeline_for_tier2().process_incoming_message(diff_chat),
        Err(TelegramError::Validation(_))
    ));

    // 3. Different reply_to
    let mut diff_reply = base_msg.clone();
    diff_reply.reply_to = Some(ReplyHeader::new(MsgId(99)));
    assert!(matches!(
        pipeline.process_incoming_message(diff_reply.clone()),
        Err(TelegramError::Validation(_))
    ));
    assert!(matches!(
        fresh_pipeline_for_tier2().process_incoming_message(diff_reply),
        Err(TelegramError::Validation(_))
    ));

    // 4. Different topic_id
    let mut diff_topic = base_msg.clone();
    diff_topic.topic_id = None;
    assert!(matches!(
        pipeline.process_incoming_message(diff_topic.clone()),
        Err(TelegramError::Validation(_))
    ));
    assert!(matches!(
        fresh_pipeline_for_tier2().process_incoming_message(diff_topic),
        Err(TelegramError::Validation(_))
    ));

    // 5. Different forward_info
    let mut diff_forward = base_msg.clone();
    diff_forward.forward_info = None;
    assert!(matches!(
        pipeline.process_incoming_message(diff_forward.clone()),
        Err(TelegramError::Validation(_))
    ));
    assert!(matches!(
        fresh_pipeline_for_tier2().process_incoming_message(diff_forward),
        Err(TelegramError::Validation(_))
    ));

    // Verify no extra rows or PTS increments occurred in either chat
    assert_eq!(engine.current_pts(ChatId(700)).unwrap(), 1);
    assert_eq!(engine.current_pts(ChatId(701)).unwrap(), 0);
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM messages WHERE chat_id = ?1", 700),
        1
    );
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM messages WHERE chat_id = ?1", 701),
        0
    );
}

#[tokio::test]
async fn test_concurrent_same_and_different_nonces_contiguity_and_single_journal_entry() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open db"));
    seed_user(&db, 1, "user_one");
    seed_user(&db, 2, "user_two");
    seed_chat(&db, 800, 1, "Concurrency Chat");

    let engine = PtsEngine::new(db.clone());
    let pipeline = Arc::new(MessagePipeline::new(
        MessageNonceDedup::new(2048, 60_000),
        engine.clone(),
    ));

    // 10 distinct nonces across 2 senders, each retried 5 times concurrently = 50 concurrent tasks.
    // Every retry for a given (sender, nonce) has IDENTICAL semantic content.
    let senders = [1u64, 2u64];
    let nonces_per_sender = 5usize;
    let retries_per_nonce = 5usize;
    let total_tasks = senders.len() * nonces_per_sender * retries_per_nonce;
    let distinct_messages = senders.len() * nonces_per_sender;
    let barrier = Arc::new(Barrier::new(total_tasks));

    let mut handles = Vec::with_capacity(total_tasks);
    for &sender_id in &senders {
        for n_idx in 0..nonces_per_sender {
            let nonce = (sender_id * 10_000) + n_idx as u64;
            let text = format!("Canonical message from sender {} nonce {}", sender_id, nonce);
            for retry_idx in 0..retries_per_nonce {
                let pipeline_clone = pipeline.clone();
                let barrier_clone = barrier.clone();
                let text_clone = text.clone();
                handles.push(tokio::spawn(async move {
                    let mut msg = MessageItem::new(
                        MsgId::generate_client(retry_idx as u32),
                        ChatId(800),
                        UserId(sender_id),
                        PeerId::Chat(ChatId(800)),
                        MessageContent::plain_text(text_clone),
                    )
                    .with_client_nonce(nonce);
                    msg.flags.is_out = retry_idx % 2 == 0;
                    barrier_clone.wait().await;
                    let update = pipeline_clone
                        .process_incoming_message(msg)
                        .expect("concurrent identical retry must succeed");
                    ((sender_id, nonce), update)
                }));
            }
        }
    }

    let mut by_key: std::collections::HashMap<(u64, u64), Vec<(MsgId, u64, u32)>> =
        std::collections::HashMap::new();
    for h in handles {
        let (key, update) = h.await.expect("join task");
        let msg_id = match update.event {
            UpdateEvent::NewMessage(m) => m.id,
            other => panic!("expected NewMessage, got {:?}", other),
        };
        by_key
            .entry(key)
            .or_default()
            .push((msg_id, update.pts, update.pts_count));
    }

    assert_eq!(by_key.len(), distinct_messages);

    let mut allocated_msg_ids = HashSet::new();
    let mut allocated_pts = HashSet::new();

    for ((sender_id, nonce), outcomes) in &by_key {
        assert_eq!(outcomes.len(), retries_per_nonce);
        let (first_msg_id, first_pts, _) = outcomes[0];
        let mut creator_count = 0usize;
        for &(m_id, pts, pts_count) in outcomes {
            assert_eq!(
                m_id, first_msg_id,
                "All concurrent retries for ({}, {}) must receive same MsgId",
                sender_id, nonce
            );
            assert_eq!(
                pts, first_pts,
                "All concurrent retries for ({}, {}) must receive same PTS",
                sender_id, nonce
            );
            if pts_count == 1 {
                creator_count += 1;
            } else {
                assert_eq!(pts_count, 0);
            }
        }
        assert_eq!(
            creator_count, 1,
            "Exactly one concurrent task for ({}, {}) may advance pts_count=1",
            sender_id, nonce
        );
        assert!(allocated_msg_ids.insert(first_msg_id.0));
        assert!(allocated_pts.insert(first_pts));
    }

    let mut sorted_ids: Vec<i64> = allocated_msg_ids.into_iter().collect();
    sorted_ids.sort_unstable();
    let expected_ids: Vec<i64> = (1..=(distinct_messages as i64)).collect();
    assert_eq!(sorted_ids, expected_ids, "Server message IDs must be contiguous 1..=10");

    let mut sorted_pts: Vec<u64> = allocated_pts.into_iter().collect();
    sorted_pts.sort_unstable();
    let expected_pts: Vec<u64> = (1..=(distinct_messages as u64)).collect();
    assert_eq!(sorted_pts, expected_pts, "PTS sequence must be contiguous 1..=10");

    // Verify SQLite tables and SyncGetDifference journal have exactly 10 entries
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM messages WHERE chat_id = ?1", 800),
        distinct_messages as i64
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM pts_updates WHERE chat_id = ?1",
            800
        ),
        distinct_messages as i64
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM message_dedup WHERE chat_id = ?1",
            800
        ),
        distinct_messages as i64
    );

    let diff = SyncDifferenceService::new(db.clone())
        .get_difference(&SyncGetDifferenceRequest::new(ChatId(800), 0, 100))
        .expect("get_difference");
    assert_eq!(diff.new_messages.len(), distinct_messages);
    assert_eq!(diff.pts, distinct_messages as u64);

    let chat_rec = db
        .get_chat(ChatId(800))
        .expect("get_chat")
        .expect("chat exists");
    assert_eq!(chat_rec.pts, distinct_messages as u64);
    assert_eq!(chat_rec.last_message_id, MsgId(distinct_messages as i64));
}

#[tokio::test]
async fn test_restart_reopen_dedup_and_conflict_enforcement() {
    let guard = TempDbDirGuard::new();
    let nonce = 424_242_424u64;
    let msg_timestamp_ms = 1_762_500_000_000i64;
    let msg_date = Utc.timestamp_millis_opt(msg_timestamp_ms).unwrap();

    // Phase 1: Open database on disk, seed user & chat, commit message with nonce
    {
        let db = Arc::new(ServerDatabase::open(&guard.db_path).expect("open file db phase 1"));
        seed_user(&db, 77, "persistent_sender");
        seed_chat(&db, 900, 77, "Persistent Chat");

        let pipeline = MessagePipeline::new(
            MessageNonceDedup::new(128, 60_000),
            PtsEngine::new(db.clone()),
        );

        let mut msg = MessageItem::new(
            MsgId(0),
            ChatId(900),
            UserId(77),
            PeerId::Chat(ChatId(900)),
            MessageContent::plain_text("Persisted across restart"),
        )
        .with_client_nonce(nonce);
        msg.date = msg_date;

        let update = pipeline
            .process_incoming_message(msg)
            .expect("commit message before restart");
        assert_eq!(update.pts, 1);
        assert_eq!(update.pts_count, 1);
    }

    // Phase 2: Reopen database from disk with a cold memory cache
    {
        let db_reopened =
            Arc::new(ServerDatabase::open(&guard.db_path).expect("reopen file db phase 2"));
        let engine_reopened = PtsEngine::new(db_reopened.clone());
        let pipeline_reopened = MessagePipeline::new(
            MessageNonceDedup::new(128, 60_000),
            engine_reopened.clone(),
        );

        // Exact semantic retry after reopen yields original ID=1, PTS=1, pts_count=0
        let retry_msg = MessageItem::new(
            MsgId::generate_client(99),
            ChatId(900),
            UserId(77),
            PeerId::Chat(ChatId(900)),
            MessageContent::plain_text("Persisted across restart"),
        )
        .with_client_nonce(nonce);

        let replay = pipeline_reopened
            .process_incoming_message(retry_msg)
            .expect("retry after reopen must succeed");
        assert_eq!(replay.pts, 1);
        assert_eq!(replay.pts_count, 0);
        match &replay.event {
            UpdateEvent::NewMessage(m) => {
                assert_eq!(m.id, MsgId(1));
                assert_eq!(m.date.timestamp_millis(), msg_timestamp_ms);
            }
            other => panic!("expected NewMessage, got {:?}", other),
        }

        // Conflicting retry after reopen must fail with Validation error
        let conflicting_retry = MessageItem::new(
            MsgId(0),
            ChatId(900),
            UserId(77),
            PeerId::Chat(ChatId(900)),
            MessageContent::plain_text("Different text after restart"),
        )
        .with_client_nonce(nonce);
        let err = pipeline_reopened
            .process_incoming_message(conflicting_retry)
            .expect_err("conflicting retry after reopen must fail");
        assert!(matches!(err, TelegramError::Validation(_)));

        // New message with a fresh nonce after reopen advances to MsgId(2), PTS=2
        let second_msg = MessageItem::new(
            MsgId(0),
            ChatId(900),
            UserId(77),
            PeerId::Chat(ChatId(900)),
            MessageContent::plain_text("Second message after reopen"),
        )
        .with_client_nonce(nonce + 1);
        let second_update = pipeline_reopened
            .process_incoming_message(second_msg)
            .expect("second message after reopen must commit");
        assert_eq!(second_update.pts, 2);
        assert_eq!(second_update.pts_count, 1);

        let chat_rec = db_reopened
            .get_chat(ChatId(900))
            .expect("get_chat")
            .expect("chat exists");
        assert_eq!(chat_rec.pts, 2);
        assert_eq!(chat_rec.last_message_id, MsgId(2));
    }
}

#[tokio::test]
async fn test_integer_overflow_boundaries_rejected_without_negative_sqlite_wrapping() {
    let db = Arc::new(ServerDatabase::open_in_memory().expect("open db"));
    seed_user(&db, 50, "boundary_user");
    seed_chat(&db, 950, 50, "Boundary Chat");

    let engine = PtsEngine::new(db.clone());
    let dedup = MessageNonceDedup::new(128, 60_000);
    let pipeline = MessagePipeline::new(dedup.clone(), engine.clone());

    // 1. client_nonce > i64::MAX (would wrap to negative i64 if cast via `as i64`)
    for overflowing_nonce in [(i64::MAX as u64) + 1, u64::MAX] {
        let msg = MessageItem::new(
            MsgId(0),
            ChatId(950),
            UserId(50),
            PeerId::Chat(ChatId(950)),
            MessageContent::plain_text("Overflowing nonce"),
        )
        .with_client_nonce(overflowing_nonce);

        let err = pipeline
            .process_incoming_message(msg)
            .expect_err("client_nonce > i64::MAX must be rejected");
        assert!(matches!(err, TelegramError::Validation(_)));
    }

    // 2. sender_id == 0 or sender_id > i64::MAX must be rejected
    for invalid_sender in [0u64, (i64::MAX as u64) + 1, u64::MAX] {
        let msg = MessageItem::new(
            MsgId(0),
            ChatId(950),
            UserId(invalid_sender),
            PeerId::Chat(ChatId(950)),
            MessageContent::plain_text("Invalid sender"),
        )
        .with_client_nonce(100);

        let err = pipeline
            .process_incoming_message(msg)
            .expect_err("invalid sender_id must be rejected");
        assert!(matches!(err, TelegramError::Validation(_)));
    }

    // 3. Maximum valid i64::MAX nonce must succeed without error
    let max_valid_nonce = i64::MAX as u64;
    let valid_msg = MessageItem::new(
        MsgId(0),
        ChatId(950),
        UserId(50),
        PeerId::Chat(ChatId(950)),
        MessageContent::plain_text("Max valid i64 nonce"),
    )
    .with_client_nonce(max_valid_nonce);
    let ok_update = pipeline
        .process_incoming_message(valid_msg.clone())
        .expect("i64::MAX client_nonce must succeed");
    assert_eq!(ok_update.pts, 1);

    // Verify stored client_nonce in SQLite is positive i64::MAX, not negative
    let stored_nonce: i64 = db
        .pool()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT client_nonce FROM message_dedup WHERE sender_id = 50",
                [],
                |r| r.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .expect("query stored nonce");
    assert_eq!(stored_nonce, i64::MAX);
}
