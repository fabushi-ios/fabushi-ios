use std::collections::HashMap;
use std::fs;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::forever_box::{
    DISK_PRESSURE_HEARTBEAT_MS, DiskPressureGuard, DiskPressureGuardOptions,
    DiskPressureLevel, DiskPressureReminderEpisodes, DiskPressureTrigger,
    DiskVolumeSample, DiskVolumeSnapshot, GIB, classify_disk_pressure,
};
use mahayana_host_runtime::extensions::forever_box::disk_pressure_guard::{
    aggregate_disk_pressure_level,
};

fn snapshot(available_bytes: u64) -> DiskVolumeSnapshot {
    DiskVolumeSnapshot {
        volume: "workspace".into(),
        device_id: "device-a".into(),
        total_bytes: 100 * GIB,
        available_bytes,
    }
}

#[test]
fn disk_pressure_classifier_preserves_frozen_thresholds_and_hysteresis() {
    assert_eq!(
        classify_disk_pressure(100 * GIB, 20 * GIB, DiskPressureLevel::Healthy),
        DiskPressureLevel::Healthy
    );
    assert_eq!(
        classify_disk_pressure(100 * GIB, 8 * GIB, DiskPressureLevel::Healthy),
        DiskPressureLevel::Soft
    );
    assert_eq!(
        classify_disk_pressure(100 * GIB, 2 * GIB, DiskPressureLevel::Healthy),
        DiskPressureLevel::Hard
    );
    assert_eq!(
        classify_disk_pressure(100 * GIB, 3 * GIB, DiskPressureLevel::Hard),
        DiskPressureLevel::Hard,
        "hard pressure must not clear until both hard recovery thresholds clear"
    );
    assert_eq!(
        classify_disk_pressure(100 * GIB, 9 * GIB, DiskPressureLevel::Soft),
        DiskPressureLevel::Soft,
        "soft pressure must preserve its recovery hysteresis"
    );
    assert_eq!(
        classify_disk_pressure(100 * GIB, 21 * GIB, DiskPressureLevel::Soft),
        DiskPressureLevel::Healthy
    );

    let levels = HashMap::from([
        ("one".to_string(), DiskPressureLevel::Soft),
        ("two".to_string(), DiskPressureLevel::Hard),
    ]);
    assert_eq!(
        aggregate_disk_pressure_level(&levels),
        DiskPressureLevel::Hard
    );
}

#[test]
fn disk_pressure_guard_reports_transitions_heartbeats_and_aggregate_changes() {
    let sample = Arc::new(Mutex::new(DiskVolumeSample {
        snapshots: vec![snapshot(8 * GIB)],
        complete: true,
    }));
    let now = Arc::new(AtomicU64::new(0));
    let reports = Arc::new(Mutex::new(Vec::new()));
    let changes = Arc::new(Mutex::new(Vec::new()));
    let successes = Arc::new(Mutex::new(Vec::new()));

    let guard = DiskPressureGuard::new(DiskPressureGuardOptions {
        read_volumes: {
            let sample = Arc::clone(&sample);
            Arc::new(move || Ok(sample.lock().expect("sample").clone()))
        },
        report: {
            let reports = Arc::clone(&reports);
            Arc::new(move |report| reports.lock().expect("reports").push(report.clone()))
        },
        on_pressure_change: {
            let changes = Arc::clone(&changes);
            Arc::new(move |level| changes.lock().expect("changes").push(level))
        },
        on_successful_sample: Some({
            let successes = Arc::clone(&successes);
            Arc::new(move |level, complete| {
                successes.lock().expect("successes").push((level, complete))
            })
        }),
        log: Arc::new(|_| {}),
        clock: Some({
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::Acquire))
        }),
    });

    guard.on_tick();
    assert_eq!(
        reports.lock().expect("reports")[0].trigger,
        DiskPressureTrigger::Transition
    );
    assert_eq!(
        changes.lock().expect("changes").as_slice(),
        &[Some(DiskPressureLevel::Soft)]
    );

    now.store(DISK_PRESSURE_HEARTBEAT_MS - 1, Ordering::Release);
    guard.on_tick();
    assert_eq!(reports.lock().expect("reports").len(), 1);

    now.store(DISK_PRESSURE_HEARTBEAT_MS, Ordering::Release);
    guard.on_tick();
    assert_eq!(reports.lock().expect("reports").len(), 2);
    assert_eq!(
        reports.lock().expect("reports")[1].trigger,
        DiskPressureTrigger::Heartbeat
    );

    sample.lock().expect("sample").snapshots = vec![snapshot(30 * GIB)];
    guard.on_tick();
    assert_eq!(
        changes.lock().expect("changes").last().copied(),
        Some(None)
    );
    assert!(
        successes
            .lock()
            .expect("successes")
            .iter()
            .all(|(_, complete)| *complete)
    );
}

#[test]
fn reminder_episode_claim_commit_and_restart_match_frozen_ledger_semantics() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-disk-pressure-ledger-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("ledger root");

    let episodes = DiskPressureReminderEpisodes::new(
        Some(&root),
        Some(Arc::new(|| "episode-fixed".into())),
        None,
    );
    episodes.observe_pressure(DiskPressureLevel::Soft, true);
    episodes.enroll(["agent-a", "agent-b"]);
    assert_eq!(
        episodes.claim("agent-a", "claim-a").as_deref(),
        Some("episode-fixed")
    );
    episodes.release("agent-a", "claim-a");
    assert_eq!(
        episodes.claim("agent-a", "claim-a2").as_deref(),
        Some("episode-fixed")
    );
    assert!(episodes.commit("agent-a", "claim-a2"));
    assert_eq!(episodes.claim("agent-a", "claim-a3"), None);
    episodes.forget_agent("agent-b");
    let durable = fs::read_to_string(root.join("host-disk-pressure-reminders.json"))
        .expect("durable reminder ledger");
    assert!(
        !durable.contains("agent-b"),
        "forgetAgent must remove the Agent from persisted pending/handled reminder state"
    );

    let restored = DiskPressureReminderEpisodes::new(Some(&root), None, None);
    assert_eq!(
        restored.claim("agent-b", "claim-b").as_deref(),
        Some("episode-fixed"),
        "pending reminders must survive Host restart"
    );
    assert!(restored.commit("agent-b", "claim-b"));

    restored.observe_pressure(DiskPressureLevel::Healthy, true);
    restored.observe_pressure(DiskPressureLevel::Hard, true);
    let new_episode = restored
        .claim("agent-c", "claim-c")
        .expect("new pressure episode");
    assert_ne!(new_episode, "episode-fixed");

    let _ = fs::remove_dir_all(root);
}
