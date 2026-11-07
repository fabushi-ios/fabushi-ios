use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::extension::{
    TRANSCRIPT_EXTENSION_DEPENDENCIES, TRANSCRIPT_EXTENSION_ID, TranscriptExtensionEventBridge,
    bind_runner_outline_stream_events, start_transcript_extension,
};
use mahayana_host_runtime::host_event_bus::SandHostEventBus;

fn temp_root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-transcript-extension-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn extension_composes_manager_roster_and_profile_watch_lifecycle() {
    let root = temp_root();
    let agents = root.join("agents");
    fs::create_dir_all(&agents).expect("agents root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let extension = start_transcript_extension(
        &root,
        Arc::clone(&sessions),
        Arc::new(move |event| {
            sink_events.lock().expect("events").push(event);
        }),
    );

    assert_eq!(TRANSCRIPT_EXTENSION_ID, "transcript");
    assert_eq!(
        TRANSCRIPT_EXTENSION_DEPENDENCIES,
        &[
            "attachments",
            "content-search",
            "memory",
            "session",
            "telemetry",
            "trays",
            "turn-execution",
        ]
    );
    let manager = extension.manager();
    assert!(Arc::ptr_eq(&manager.session_workers(), &sessions));
    assert_eq!(extension.roster_emit().outline_stream_coalescing_ms(), 250);
    extension.roster_emit().emit_agents().expect("emit empty roster");
    assert_eq!(
        events
            .lock()
            .expect("events")
            .last()
            .and_then(|event| event.get("channel"))
            .and_then(serde_json::Value::as_str),
        Some("agents")
    );
    // Watch setup is fail-open in production. A platform watcher can be
    // unavailable, but the failure must remain inspectable instead of silently
    // replacing the authoritative roster path.
    assert!(
        extension.profile_watch_active() || extension.profile_watch_error().is_some()
    );
    assert!(!manager.is_disposed());
    drop(extension);
    assert!(manager.is_disposed());

    let _ = fs::remove_dir_all(root);
}


#[test]
fn extension_event_bridge_emits_frozen_host_topics_with_identity_payloads() {
    let bus = SandHostEventBus::default();
    let observed = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
    let mut subscriptions = Vec::new();
    for topic in [
        "transcript.automation-config-changed",
        "transcript.listener-connect-card",
        "transcript.run-started",
        "transcript.run-ended",
    ] {
        let observed = Arc::clone(&observed);
        let topic_owned = topic.to_string();
        subscriptions.push(bus.on(topic, move |payload| {
            observed
                .lock()
                .expect("observed events")
                .push((topic_owned.clone(), payload.clone()));
            Ok(())
        }));
    }
    let bridge = TranscriptExtensionEventBridge::new(bus);

    bridge.automation_config_changed();
    bridge.listener_connect_card("agent-a", "slack");
    bridge.run_started("request-a");
    bridge.run_ended("request-a");

    let observed = observed.lock().expect("observed events");
    assert_eq!(observed.len(), 4);
    assert_eq!(
        observed[0],
        (
            "transcript.automation-config-changed".to_string(),
            serde_json::json!({})
        )
    );
    assert_eq!(
        observed[1],
        (
            "transcript.listener-connect-card".to_string(),
            serde_json::json!({"agentId":"agent-a","platform":"slack"})
        )
    );
    assert_eq!(
        observed[2],
        (
            "transcript.run-started".to_string(),
            serde_json::json!({"requestId":"request-a"})
        )
    );
    assert_eq!(
        observed[3],
        (
            "transcript.run-ended".to_string(),
            serde_json::json!({"requestId":"request-a"})
        )
    );
    drop(subscriptions);
}


#[test]
fn extension_installs_executable_outline_stream_coalescing_on_shipping_roster_owner() {
    let root = temp_root();
    let agents = root.join("agents");
    fs::create_dir_all(&agents).expect("agents root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let extension = start_transcript_extension(
        &root,
        Arc::clone(&sessions),
        Arc::new(move |event| {
            sink_events.lock().expect("events").push(event);
        }),
    );
    let roster = extension.roster_emit();

    assert_eq!(roster.outline_stream_coalescing_ms(), 250);
    roster.set_outline_stream_coalescing_ms(60_000);

    roster.queue_outline_stream_update(
        "agent-a",
        "item-a",
        serde_json::json!({"id":"item-a","text":"first"}),
    );
    roster.queue_outline_stream_update(
        "agent-a",
        "item-a",
        serde_json::json!({"id":"item-a","text":"second"}),
    );
    assert!(roster.has_pending_outline_stream_update());

    roster.queue_outline_stream_update(
        "agent-a",
        "item-b",
        serde_json::json!({"id":"item-b","text":"other"}),
    );
    let first_outline = events
        .lock()
        .expect("events")
        .iter()
        .find(|event| event["channel"] == "outline")
        .cloned()
        .expect("different item must pre-flush pending outline");
    assert_eq!(first_outline["payload"]["item"]["id"], "item-a");
    assert_eq!(first_outline["payload"]["item"]["text"], "second");

    roster.flush_outline_stream_update();
    let outline_events = events
        .lock()
        .expect("events")
        .iter()
        .filter(|event| event["channel"] == "outline")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(outline_events.len(), 2);
    assert_eq!(outline_events[1]["payload"]["item"]["id"], "item-b");

    roster.queue_outline_stream_update(
        "agent-a",
        "item-c",
        serde_json::json!({"id":"item-c","text":"pending"}),
    );
    assert!(roster.has_pending_outline_stream_update());
    roster.stop_outline_stream_coalescing();
    assert!(!roster.has_pending_outline_stream_update());

    drop(extension);
    let _ = fs::remove_dir_all(root);
}


#[test]
fn shipping_runner_event_bridge_feeds_outline_coalescing_and_flushes_on_terminal() {
    let root = temp_root();
    let agents = root.join("agents");
    fs::create_dir_all(&agents).expect("agents root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let bus = SandHostEventBus::default();
    let sink_bus = bus.clone();
    let extension = start_transcript_extension(
        &root,
        Arc::clone(&sessions),
        Arc::new(move |event| sink_bus.publish(event)),
    );
    extension.roster_emit().set_outline_stream_coalescing_ms(60_000);
    let _subscription =
        bind_runner_outline_stream_events(&bus, extension.roster_emit());

    let outline_events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&outline_events);
    let _outline_listener = bus.subscribe_listener(move |event| {
        if event.get("channel").and_then(serde_json::Value::as_str) == Some("outline") {
            observed.lock().expect("outline events").push(event.clone());
        }
    });

    bus.publish(serde_json::json!({
        "channel": "runner-inference",
        "payload": {
            "streamId": "stream-a",
            "agentId": "agent-a",
            "type": "delta",
            "content": "hello"
        }
    }));
    bus.publish(serde_json::json!({
        "channel": "runner-inference",
        "payload": {
            "streamId": "stream-a",
            "agentId": "agent-a",
            "type": "delta",
            "content": "hello world"
        }
    }));

    {
        let events = outline_events.lock().expect("outline events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["payload"]["type"], "appended");
        assert_eq!(events[0]["payload"]["item"]["text"], "hello");
    }
    assert!(extension.roster_emit().has_pending_outline_stream_update());

    bus.publish(serde_json::json!({
        "channel": "runner-inference",
        "payload": {
            "streamId": "stream-a",
            "type": "completed",
            "content": "hello world"
        }
    }));

    let events = outline_events.lock().expect("outline events");
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["payload"]["type"], "updated");
    assert_eq!(events[1]["payload"]["item"]["text"], "hello world");
    assert_eq!(
        events[0]["payload"]["item"]["id"],
        events[1]["payload"]["item"]["id"]
    );
    assert!(!extension.roster_emit().has_pending_outline_stream_update());

    drop(events);
    drop(extension);
    let _ = fs::remove_dir_all(root);
}
