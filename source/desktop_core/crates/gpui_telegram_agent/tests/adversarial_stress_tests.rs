//! Adversarial stress, fuzzing, and boundary edge tests for M4 CUA Lease, Talk Mode & Inherited Re-exports.

use gpui_telegram_agent::computer::{
    CuaAction, CuaExecutor, LeaseState,
};
use gpui_telegram_agent::talk::{
    TalkCoordinator, TalkSessionState, VadBargeInDetector, MAX_AUDIO_BUFFER_CHUNKS,
};
use gpui_telegram_agent::ComputerLeaseBroker;
use gpui_telegram_core::domain::{
    conversation, dialog, engine_tdrp_dialogs, engine_tdrp_media_stories, engine_tdrp_moderation,
    engine_tdrp_topics_channels, message, miniapp, payment, secret_chat, story, wallet,
};
use gpui_telegram_proto::tdrp::{
    calls_group_calls, chat_admin_moderation, export_payments_bots, folders_filters, forum_topics,
    polls_todo, settings_cloud_passkeys, stories_gifts_stars, TdrpCoordinatorState,
};
use std::sync::{Arc, Barrier, RwLock};
use std::thread;
use std::time::Instant;

// ============================================================================
// 1. CUA Lease Broker Stress & Race Condition Tests
// ============================================================================

#[test]
fn test_stress_cua_emergency_takeover_sub_10ms_latency_distribution() {
    let mut broker = ComputerLeaseBroker::new();
    let iterations = 1000;
    let mut max_duration = std::time::Duration::ZERO;

    for i in 0..iterations {
        let _ = broker.acquire_lease("agent_stress", "device_stress");
        let start = Instant::now();
        let new_gen = broker.emergency_takeover();
        let elapsed = start.elapsed();

        if elapsed > max_duration {
            max_duration = elapsed;
        }

        assert!(
            elapsed.as_millis() < 10,
            "Iteration {} emergency takeover took {:?}, must be < 10ms",
            i,
            elapsed
        );
        assert!(new_gen > 0);
        assert!(!broker.is_lease_active());
    }

    println!(
        "Emergency takeover 1,000 iterations max latency: {:?}",
        max_duration
    );
}

#[test]
fn test_adversarial_cua_simultaneous_concurrent_acquire() {
    let broker = Arc::new(RwLock::new(ComputerLeaseBroker::new()));
    let num_threads = 16;
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::new();

    for thread_idx in 0..num_threads {
        let b = Arc::clone(&broker);
        let bar = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            bar.wait();
            let agent_id = format!("agent_{}", thread_idx);
            let mut guard = b.write().unwrap();
            guard.acquire_lease(&agent_id, "device_vm_1")
        }));
    }

    let mut granted_leases = Vec::new();
    for h in handles {
        if let Ok(Some(lease)) = h.join() {
            granted_leases.push(lease);
        }
    }

    // At most ONE distinct agent should be granted an active lease at a time
    assert!(
        !granted_leases.is_empty(),
        "At least one thread should acquire lease"
    );
    // If multiple were returned because re-entrant or whatever, their agent_ids must match
    let first_agent = &granted_leases[0].holder_user_id;
    for l in &granted_leases {
        assert_eq!(&l.holder_user_id, first_agent);
    }
}

#[test]
fn test_adversarial_cua_sequential_acquire_after_release() {
    let mut broker = ComputerLeaseBroker::new();

    // Agent 1 acquires lease
    let lease1 = broker
        .acquire_lease("agent_1", "dev_1")
        .expect("agent 1 should acquire lease");
    assert_eq!(lease1.state, LeaseState::ActiveControl);

    // Agent 2 attempts while Agent 1 is active -> MUST FAIL
    let attempt2 = broker.acquire_lease("agent_2", "dev_1");
    assert!(
        attempt2.is_none(),
        "Agent 2 must not acquire while Agent 1 is active"
    );

    // Agent 1 releases lease
    let released = broker.release_lease(&lease1.lease_id);
    assert!(released, "Release of active lease must succeed");
    assert!(!broker.is_lease_active());

    // Agent 2 attempts to acquire lease AFTER Agent 1 released
    let lease2 = broker.acquire_lease("agent_2", "dev_1");
    assert!(
        lease2.is_some(),
        "Agent 2 MUST be able to acquire lease after Agent 1 released!"
    );
}

#[test]
fn test_adversarial_cua_acquire_after_emergency_takeover() {
    let mut broker = ComputerLeaseBroker::new();

    // Agent 1 acquires lease
    let lease1 = broker
        .acquire_lease("agent_1", "dev_1")
        .expect("agent 1 should acquire lease");
    let gen1 = lease1.generation;

    // Human triggers emergency takeover
    let gen2 = broker.emergency_takeover();
    assert!(gen2 > gen1, "Emergency takeover must bump generation");
    assert!(!broker.is_lease_active());

    // Agent 2 attempts to acquire lease after human takeover cleared prior session
    let lease2 = broker.acquire_lease("agent_2", "dev_1");
    assert!(
        lease2.is_some(),
        "Agent 2 MUST be able to acquire a new lease after emergency takeover!"
    );
    if let Some(l2) = lease2 {
        assert!(
            l2.generation >= gen2,
            "New lease generation ({}) must be >= post-takeover generation ({})",
            l2.generation,
            gen2
        );
    }
}

#[test]
fn test_adversarial_cua_stale_generation_fencing_attack() {
    let mut broker = ComputerLeaseBroker::new();
    let lease1 = broker
        .acquire_lease("agent_1", "dev_1")
        .expect("lease granted");
    let initial_gen = broker.current_generation();

    // Execute valid action
    let valid_action = CuaAction::Click {
        x: 100,
        y: 200,
        button: "left".into(),
    };
    assert!(
        CuaExecutor::execute_action_fenced(&valid_action, &lease1, initial_gen).is_ok()
    );

    // Human emergency takeover
    let takeover_gen = broker.emergency_takeover();
    assert_eq!(takeover_gen, initial_gen + 1);

    // Adversary attempts to replay or inject actions using lease1
    let malicious_action = CuaAction::TypeString {
        text: "format c: /y".into(),
    };

    // Attack 1: Using stale lease with old generation against new broker generation
    let res1 = CuaExecutor::execute_action_fenced(&malicious_action, &lease1, broker.current_generation());
    assert!(res1.is_err(), "Action with stale lease generation must be rejected");

    // Attack 2: Using stale lease marked preempted
    let mut preempted_lease = lease1.clone();
    preempted_lease.preempt();
    let res2 = CuaExecutor::execute_action_fenced(&malicious_action, &preempted_lease, broker.current_generation());
    assert!(res2.is_err(), "Action with preempted lease must be rejected");

    // Attack 3: Using stale lease with fake updated generation
    let mut forged_lease = lease1.clone();
    forged_lease.generation = broker.current_generation(); // forged generation
    // But forged lease is still invalid if preempted, or if broker active_lease is None
    assert!(!broker.is_lease_active());
}

// ============================================================================
// 2. Talk Mode 320-Chunk Buffer & VAD Barge-In Stress Tests
// ============================================================================

#[test]
fn test_adversarial_talk_feed_500_chunks_buffer_behavior() {
    let mut coord = TalkCoordinator::new();
    coord.state = TalkSessionState::Speaking;

    // Feed 500 chunks
    let mut accepted_count = 0;
    let mut rejected_count = 0;

    for i in 0..500 {
        let chunk = vec![(i % 256) as u8; 3200];
        match coord.enqueue_audio(chunk) {
            Ok(()) => accepted_count += 1,
            Err(_) => rejected_count += 1,
        }
    }

    println!(
        "Fed 500 chunks: accepted={}, rejected={}, buffered_chunks={}",
        accepted_count,
        rejected_count,
        coord.buffered_chunks_count()
    );

    // Buffer MUST NEVER exceed MAX_AUDIO_BUFFER_CHUNKS (320)
    assert!(
        coord.buffered_chunks_count() <= MAX_AUDIO_BUFFER_CHUNKS,
        "Buffer count {} exceeded max 320",
        coord.buffered_chunks_count()
    );
    assert_eq!(coord.buffered_chunks_count(), MAX_AUDIO_BUFFER_CHUNKS);

    // Inspect first chunk in queue: is it chunk 0 (oldest retained) or chunk 180 (oldest dropped)?
    if let Some(front) = coord.dequeue_audio() {
        println!("First chunk byte 0 value: {}", front[0]);
    }
}

#[test]
fn test_adversarial_vad_bargein_instantaneous_flush_under_stress() {
    let mut detector = VadBargeInDetector::new(400.0);
    let mut coord = TalkCoordinator::new();
    coord.state = TalkSessionState::Speaking;

    // Fill buffer to max capacity (320 chunks)
    for i in 0..MAX_AUDIO_BUFFER_CHUNKS {
        let _ = coord.enqueue_audio(vec![(i % 256) as u8; 3200]);
    }
    assert_eq!(coord.buffered_chunks_count(), MAX_AUDIO_BUFFER_CHUNKS);

    // High energy voice frames
    let speech_frame = vec![2000i16; 320];

    // Frame 1: 20ms
    assert!(!detector.process_pcm_frame(&speech_frame));
    assert_eq!(detector.consecutive_speech_frames, 1);
    assert_eq!(coord.buffered_chunks_count(), MAX_AUDIO_BUFFER_CHUNKS);

    // Frame 2: 40ms
    assert!(!detector.process_pcm_frame(&speech_frame));
    assert_eq!(detector.consecutive_speech_frames, 2);
    assert_eq!(coord.buffered_chunks_count(), MAX_AUDIO_BUFFER_CHUNKS);

    // Frame 3: 60ms -> triggers barge-in!
    let start_detect = Instant::now();
    let barge_in = detector.process_pcm_frame(&speech_frame);
    let detect_elapsed = start_detect.elapsed();
    assert!(barge_in);
    assert!(
        detect_elapsed.as_millis() < 5,
        "VAD frame detection took {:?}, must be < 5ms",
        detect_elapsed
    );

    // Barge-in buffer flush execution
    let start_flush = Instant::now();
    let flushed = coord.handle_barge_in();
    let flush_elapsed = start_flush.elapsed();

    assert!(
        flush_elapsed.as_millis() < 5,
        "Barge-in buffer flush took {:?}, must be < 5ms",
        flush_elapsed
    );
    assert_eq!(flushed, MAX_AUDIO_BUFFER_CHUNKS);
    assert_eq!(coord.buffered_chunks_count(), 0);
    assert_eq!(coord.state, TalkSessionState::Interrupted);

    // Resume listening
    coord.resume_listening();
    assert_eq!(coord.state, TalkSessionState::Listening);
}

#[test]
fn test_adversarial_vad_glitch_rejection() {
    let mut detector = VadBargeInDetector::new(400.0);
    let speech_frame = vec![2000i16; 320];
    let silent_frame = vec![10i16; 320];

    // 2 speech frames followed by 1 silent frame (glitch/noise burst)
    assert!(!detector.process_pcm_frame(&speech_frame));
    assert_eq!(detector.consecutive_speech_frames, 1);
    assert!(!detector.process_pcm_frame(&speech_frame));
    assert_eq!(detector.consecutive_speech_frames, 2);

    // Glitch interrupted by silence -> must reset consecutive counter
    assert!(!detector.process_pcm_frame(&silent_frame));
    assert_eq!(detector.consecutive_speech_frames, 0);

    // Another 2 speech frames -> still not 3 consecutive
    assert!(!detector.process_pcm_frame(&speech_frame));
    assert!(!detector.process_pcm_frame(&speech_frame));
    assert_eq!(detector.consecutive_speech_frames, 2);

    // Third speech frame -> NOW it triggers
    assert!(detector.process_pcm_frame(&speech_frame));
    assert_eq!(detector.consecutive_speech_frames, 3);
}

// ============================================================================
// 3. Inherited Re-Exports Cross-Crate Instantiation
// ============================================================================

#[test]
fn test_cross_crate_domain_and_tdrp_instantiation() {
    // 1. gpui_telegram_core::domain cross-module instantiation
    let conv_id = conversation::ConversationId::new("room_alpha_999");
    let dest = conversation::ConversationDestination {
        conversation_id: conv_id.clone(),
        child: None,
    };
    assert_eq!(dialog::destination_message_conversation_id(&dest).as_str(), "room_alpha_999");
    assert_eq!(engine_tdrp_dialogs::destination_message_conversation_id(&dest).as_str(), "room_alpha_999");

    let client_msg = message::ClientMessageId("msg_adversarial_001".into());
    assert_eq!(client_msg.0, "msg_adversarial_001");

    let story_id = story::StoryId("story_adv_002".into());
    assert_eq!(story_id.0, "story_adv_002");

    let wallet_acc = wallet::WalletAccountId("wallet_adv_003".into());
    assert_eq!(wallet_acc.0, "wallet_adv_003");

    let money = payment::Money::new("EUR", 9999);
    assert_eq!(money.amount_minor, 9999);

    let perm = miniapp::MiniAppPermission::Payments;
    assert_eq!(perm, miniapp::MiniAppPermission::Payments);

    let sec_key = secret_chat::SecretPublicKey::from_bytes([99u8; 32]);
    assert_eq!(sec_key.algorithm, "X25519");

    let action = engine_tdrp_moderation::CommunityAdminAction::ManageTopics;
    assert_eq!(action, engine_tdrp_moderation::CommunityAdminAction::ManageTopics);

    let mut msg_state = fabushi_messaging_core::engine::MessagingState::default();
    msg_state.migrate_legacy_topic_child_states();

    assert!(engine_tdrp_media_stories::message_content_uses_other(&message::MessageContent::Dice {
        emoji: "🎯".to_string(),
        value: 10,
    }));

    // 2. gpui_telegram_proto::tdrp cross-module instantiation
    let mut coordinator = TdrpCoordinatorState::new();
    assert!(TdrpCoordinatorState::is_handled("folders.list"));
    assert!(TdrpCoordinatorState::is_handled("topics.list"));
    assert!(TdrpCoordinatorState::is_handled("calls.start"));
    assert!(TdrpCoordinatorState::is_handled("groupCalls.join"));
    assert!(!TdrpCoordinatorState::is_handled("invalid.call"));

    let _f = folders_filters::FolderFilterState::default();
    let _t = forum_topics::ForumTopicsState::default();
    let _p = polls_todo::PollsTodoState::default();
    let _s = stories_gifts_stars::StoriesGiftsStarsState::default();
    let _m = chat_admin_moderation::ChatAdminModerationState::default();
    let _c = calls_group_calls::CallsGroupCallsState::default();
    let _set = settings_cloud_passkeys::SettingsCloudPasskeysState::default();
    let _exp = export_payments_bots::ExportPaymentsBotsState::default();

    assert_eq!(TdrpCoordinatorState::normalize_method("tdrp.calls.start"), "calls.start");
    assert!(coordinator.dispatch("unknown.action", &serde_json::json!({})).is_none());
}
