use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::{
    SandAgentProfile, get_sand_profile_path, write_sand_profile_file,
};
use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::production_runtime::ProductionTranscriptRuntime;
use mahayana_host_runtime::extensions::transcript::roster_emit::ProductionRosterEmit;
use serde_json::Value;

const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-roster-emit-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn profile(name: &str) -> SandAgentProfile {
    SandAgentProfile {
        name: name.into(),
        description: "description".into(),
        title: String::new(),
        avatar_shape: String::new(),
        avatar_color: String::new(),
    }
}

#[test]
fn production_roster_emit_publishes_full_and_incremental_ordered_events() {
    let root = temp_root("events");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let record = sessions
        .materialize_new_session(Some(&profile("First")), "user", None)
        .expect("materialize agent");
    SandAgentSessionStore::new(Arc::clone(&sessions))
        .write_active_agent_id(&record.id)
        .expect("active pointer");

    let transcript = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink_events = Arc::clone(&events);
    let emitter = ProductionRosterEmit::new(
        Arc::clone(&sessions),
        transcript,
        Arc::new(move |event| sink_events.lock().expect("events").push(event)),
    );

    emitter.emit_agents().expect("full roster");
    assert!(emitter.cache_seeded());
    let first = events.lock().expect("events")[0].clone();
    assert_eq!(first["channel"], "agents");
    assert_eq!(first["payload"]["activeAgentId"], record.id);
    assert_eq!(first["payload"]["coverage"]["kind"], "complete-roster");
    assert_eq!(first["payload"]["ordered"]["replicaKey"], "roster");
    assert_eq!(first["payload"]["ordered"]["sequence"], 1);
    assert_eq!(first["payload"]["agents"][0]["name"], "First");
    assert!(first["payload"]["agents"][0]["snapshotEpoch"].is_string());
    assert_eq!(first["payload"]["agents"][0]["snapshotSeq"], 1);

    write_sand_profile_file(
        get_sand_profile_path(root.join(&record.id)),
        &profile("Renamed"),
    )
    .expect("rename profile");
    emitter
        .emit_agent_update(&record.id)
        .expect("incremental update");
    let events = events.lock().expect("events");
    let second = events.last().expect("second event");
    assert_eq!(second["channel"], "agent-upserted");
    assert_eq!(second["payload"]["agent"]["id"], record.id);
    assert_eq!(second["payload"]["agent"]["name"], "Renamed");
    assert_eq!(second["payload"]["ordered"]["sequence"], 2);
    assert_eq!(
        second["payload"]["agent"]["snapshotEpoch"],
        first["payload"]["agents"][0]["snapshotEpoch"]
    );
    assert_eq!(second["payload"]["agent"]["snapshotSeq"], 2);

    drop(events);
    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn name_change_projects_timeline_event_to_the_bound_background_wake_sink() {
    let root = temp_root("timeline-wake");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let transcript = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let emitted = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink_emitted = Arc::clone(&emitted);
    let emitter = ProductionRosterEmit::new(
        Arc::clone(&sessions),
        transcript,
        Arc::new(move |event| sink_emitted.lock().expect("events").push(event)),
    );
    let wakes = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let sink_wakes = Arc::clone(&wakes);
    emitter.bind_timeline_wake_sink(Some(Arc::new(move |agent_id, event| {
        sink_wakes
            .lock()
            .expect("wakes")
            .push((agent_id.to_string(), event));
    })));

    emitter.publish_name_changed("agent-a", "Old", "New");
    emitter.publish_timeline_event(
        "agent-a",
        serde_json::json!({
            "type":"channel-connected",
            "label":"Slack"
        }),
    );
    emitter.publish_timeline_event(
        "agent-a",
        serde_json::json!({
            "type":"automation-changed",
            "action":"updated",
            "automationId":"auto-1",
            "automationName":"Daily"
        }),
    );

    let emitted = emitted.lock().expect("events");
    assert_eq!(emitted[0]["channel"], "timeline");
    assert_eq!(emitted[0]["payload"]["event"]["type"], "name-changed");
    drop(emitted);
    let wakes = wakes.lock().expect("wakes");
    assert_eq!(wakes.len(), 3);
    assert_eq!(wakes[0].0, "agent-a");
    assert_eq!(wakes[0].1["from"], "Old");
    assert_eq!(wakes[0].1["to"], "New");
    assert_eq!(wakes[1].1["type"], "channel-connected");
    assert_eq!(wakes[1].1["label"], "Slack");
    assert_eq!(wakes[2].1["type"], "automation-changed");
    assert_eq!(wakes[2].1["automationId"], "auto-1");

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_profile_timeline_event_uses_background_lane_and_silent_hidden_wake() {
    let bind = SHIPPING_HOST
        .find("roster_emit.bind_timeline_wake_sink")
        .expect("shipping timeline wake binding");
    let persist = SHIPPING_HOST[bind..]
        .find("append_agent_transcript_entries(&agent_id, &[entry])")
        .map(|offset| bind + offset)
        .expect("timeline entry persistence");
    let running_fence = SHIPPING_HOST[bind..]
        .find("is_agent_running(&agent_id)")
        .map(|offset| bind + offset)
        .expect("active-run fence");
    let dispatch = SHIPPING_HOST[bind..]
        .find("run_local_background_revival_turn(")
        .map(|offset| bind + offset)
        .expect("timeline background dispatch");
    assert!(persist < running_fence && running_fence < dispatch);
    let body = &SHIPPING_HOST[dispatch
        ..SHIPPING_HOST[dispatch..]
            .find("let cross_user_settings_path")
            .map(|offset| dispatch + offset)
            .expect("timeline binding boundary")];
    assert!(body.contains("\"event\""));
    assert!(body.contains(
        "&prompt,
                        true,"
    ));
    assert!(body.contains("report_agent_error(&report)"));
    assert!(body.contains("Timeline event follow-up failed"));
}

#[test]
fn incremental_emit_falls_back_to_full_roster_before_cache_seed() {
    let root = temp_root("fallback");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let record = sessions
        .materialize_new_session(Some(&profile("Agent")), "user", None)
        .expect("materialize agent");
    let transcript = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink_events = Arc::clone(&events);
    let emitter = ProductionRosterEmit::new(
        Arc::clone(&sessions),
        transcript,
        Arc::new(move |event| sink_events.lock().expect("events").push(event)),
    );

    emitter
        .emit_agent_update(&record.id)
        .expect("fallback full");
    assert_eq!(events.lock().expect("events")[0]["channel"], "agents");

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_roster_projection_reuses_stream_identity_and_flushes_on_finish() {
    let root = temp_root("outline-stream");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let transcript = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink_events = Arc::clone(&events);
    let emitter = ProductionRosterEmit::new(
        Arc::clone(&sessions),
        transcript,
        Arc::new(move |event| sink_events.lock().expect("events").push(event)),
    );
    emitter.set_outline_stream_coalescing_ms(250);

    emitter.apply_runner_text_delta("agent-a", "stream-a", "hello");
    let first = events.lock().expect("events")[0].clone();
    assert_eq!(first["channel"], "outline");
    assert_eq!(first["payload"]["type"], "appended");
    let item_id = first["payload"]["item"]["id"]
        .as_str()
        .expect("item id")
        .to_string();
    drop(first);

    emitter.apply_runner_text_delta("agent-a", "stream-a", "hello world");
    assert!(emitter.has_pending_outline_stream_update());
    emitter.finish_runner_outline_stream("stream-a");

    let emitted = events.lock().expect("events");
    let updated = emitted.last().expect("updated event");
    assert_eq!(updated["payload"]["type"], "updated");
    assert_eq!(updated["payload"]["item"]["id"], item_id);
    assert_eq!(updated["payload"]["item"]["text"], "hello world");
    drop(emitted);

    emitter.apply_runner_text_delta("agent-a", "stream-a", "new turn");
    let emitted = events.lock().expect("events");
    let appended = emitted.last().expect("new appended event");
    assert_eq!(appended["payload"]["type"], "appended");
    assert_ne!(appended["payload"]["item"]["id"], item_id);
    drop(emitted);

    emitter.stop_outline_stream_coalescing();
    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}
