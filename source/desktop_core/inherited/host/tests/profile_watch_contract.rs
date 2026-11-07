use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::{
    SandAgentProfile, get_sand_profile_path, write_sand_profile_file,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::extension::TranscriptExtensionEventBridge;
use mahayana_host_runtime::extensions::transcript::production_runtime::ProductionTranscriptRuntime;
use mahayana_host_runtime::extensions::transcript::profile_watch::{
    PROFILE_WATCH_DEBOUNCE_MS, ProfileWatchCoalescer, ProfileWatchEvent,
    SAND_DEFAULT_AGENT_NAME, get_agent_display_profile, is_profile_watch_filename,
    profile_watch_events, read_profile_watch_name, resolve_agent_profile,
    watched_profile_path_agent_id,
};
use mahayana_host_runtime::extensions::transcript::roster_emit::ProductionRosterEmit;
use mahayana_host_runtime::host_event_bus::SandHostEventBus;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-profile-watch-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn profile_watch_filters_exact_frozen_file_family() {
    assert!(is_profile_watch_filename("profile.json"));
    assert!(is_profile_watch_filename("settings.json"));
    assert!(is_profile_watch_filename("avatar.png"));
    assert!(is_profile_watch_filename("avatar.webp"));
    assert!(!is_profile_watch_filename("notes.txt"));

    let root = temp_root("paths");
    assert_eq!(
        watched_profile_path_agent_id(
            &root,
            &root.join("agent-a").join("profile.json")
        )
        .as_deref(),
        Some("agent-a")
    );
    assert!(watched_profile_path_agent_id(
        &root,
        &root.join("agent-a").join("nested").join("profile.json")
    )
    .is_none());
}

#[test]
fn display_and_resolved_profile_match_frozen_defaulting_contract() {
    let root = temp_root("profile");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let record = sessions
        .materialize_new_session(None, "user", None)
        .expect("materialize");
    let profile_path = get_sand_profile_path(root.join(&record.id));
    write_sand_profile_file(
        &profile_path,
        &SandAgentProfile {
            name: "   ".into(),
            description: "description".into(),
            title: String::new(),
            avatar_shape: String::new(),
            avatar_color: String::new(),
        },
    )
    .expect("profile");

    let display = get_agent_display_profile(&sessions, &record.id).expect("display");
    assert_eq!(display.name, SAND_DEFAULT_AGENT_NAME);
    assert_eq!(display.description, "description");
    assert_eq!(read_profile_watch_name(&sessions, &record.id), None);

    let resolved = resolve_agent_profile(&root.join(&record.id).join("store.db"));
    assert_eq!(resolved.name, SAND_DEFAULT_AGENT_NAME);
    assert_eq!(resolved.description, "description");
    assert_eq!(resolved.file_path, profile_path);
    assert_eq!(
        resolved.settings_file_path,
        root.join(&record.id).join("settings.json")
    );

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn profile_watch_coalescing_window_is_fixed_and_session_switch_cancels_pending_emit() {
    let started = Instant::now();
    let mut coalescer = ProfileWatchCoalescer::default();

    coalescer.schedule("agent-a", started);
    coalescer.schedule(
        "agent-a",
        started + Duration::from_millis(PROFILE_WATCH_DEBOUNCE_MS - 10),
    );
    assert_eq!(
        coalescer.recv_timeout(
            started + Duration::from_millis(PROFILE_WATCH_DEBOUNCE_MS - 10)
        ),
        Duration::from_millis(10)
    );
    assert!(coalescer
        .take_due(started + Duration::from_millis(PROFILE_WATCH_DEBOUNCE_MS - 1))
        .is_none());
    assert_eq!(
        coalescer.take_due(started + Duration::from_millis(PROFILE_WATCH_DEBOUNCE_MS)),
        Some("agent-a".to_string())
    );

    let mut watched = Some("agent-a".to_string());
    coalescer.schedule("agent-a", started);
    assert!(!coalescer.switch_agent(
        &mut watched,
        Some("agent-a".to_string())
    ));
    assert_eq!(
        coalescer.take_due(started + Duration::from_millis(PROFILE_WATCH_DEBOUNCE_MS)),
        Some("agent-a".to_string())
    );

    coalescer.schedule("agent-a", started);
    assert!(coalescer.switch_agent(
        &mut watched,
        Some("agent-b".to_string())
    ));
    assert_eq!(watched.as_deref(), Some("agent-b"));
    assert!(coalescer
        .take_due(started + Duration::from_millis(PROFILE_WATCH_DEBOUNCE_MS * 2))
        .is_none());
}

#[test]
fn profile_watch_routes_timeline_before_profile_changed_through_transcript_owner() {
    let root = temp_root("event-bridge");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let transcript = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let projected = Arc::new(Mutex::new(Vec::new()));
    let projected_sink = Arc::clone(&projected);
    let roster = Arc::new(ProductionRosterEmit::new(
        Arc::clone(&sessions),
        transcript,
        Arc::new(move |event| projected_sink.lock().expect("projected").push(event)),
    ));

    let bridge = TranscriptExtensionEventBridge::new(SandHostEventBus::default());
    let _projection_subscriptions = bridge.bind_profile_watch_projection(Arc::clone(&roster));

    let subscribed = Arc::new(Mutex::new(Vec::<String>::new()));
    let subscribed_sink = Arc::clone(&subscribed);
    let profile_subscription = bridge.subscribe_profile_changed(move |agent_id| {
        subscribed_sink
            .lock()
            .expect("subscribed")
            .push(agent_id.to_string());
    });

    let sink = bridge.profile_watch_event_sink();
    let events = profile_watch_events("agent-a", Some(("Old", "New")));
    assert!(matches!(events[0], ProfileWatchEvent::Timeline { .. }));
    assert!(matches!(events[1], ProfileWatchEvent::ProfileChanged { .. }));
    for event in events {
        (sink)(event);
    }

    {
        let projected_guard = projected.lock().expect("projected");
        assert_eq!(projected_guard.len(), 2);
        assert_eq!(projected_guard[0]["channel"], "timeline");
        assert_eq!(projected_guard[0]["payload"]["agentId"], "agent-a");
        assert_eq!(projected_guard[0]["payload"]["event"]["type"], "name-changed");
        assert_eq!(projected_guard[0]["payload"]["event"]["from"], "Old");
        assert_eq!(projected_guard[0]["payload"]["event"]["to"], "New");
        assert_eq!(projected_guard[1]["channel"], "profile-changed");
        assert_eq!(projected_guard[1]["payload"]["agentId"], "agent-a");
    }

    assert_eq!(
        subscribed.lock().expect("subscribed").as_slice(),
        &["agent-a".to_string()]
    );

    drop(profile_subscription);
    for event in profile_watch_events("agent-b", None) {
        (sink)(event);
    }
    assert_eq!(
        subscribed.lock().expect("subscribed").as_slice(),
        &["agent-a".to_string()]
    );
    {
        let projected_guard = projected.lock().expect("projected");
        assert_eq!(projected_guard.len(), 3);
        assert_eq!(projected_guard[2]["channel"], "profile-changed");
        assert_eq!(projected_guard[2]["payload"]["agentId"], "agent-b");
    }

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}
