use std::fs;

use mahayana_host_runtime::extensions::host_upgrade::host_bundle_upgrade::{
    HostBundleStagePhase, HostBundleUpdateState, StageHostBundleUpgradeArgs,
    build_sand_supervisor_command, fetch_stage_and_report_host_bundle,
    host_bundle_watch_initial_delay_ms, host_bundle_watch_next_delay_ms,
    is_host_version_swap_vetoed, is_sand_host_upgrade_available, note_failed_swap_marker_for_retry,
    stage_host_bundle_upgrade,
};
use mahayana_host_runtime::extensions::host_upgrade::host_upgrade_marker::HostUpgradeMarker;

#[test]
fn availability_and_watch_jitter_match_frozen_bounds() {
    assert!(is_sand_host_upgrade_available(Some("old"), Some("new")));
    assert!(!is_sand_host_upgrade_available(Some("same"), Some("same")));
    assert!(!is_sand_host_upgrade_available(None, None));
    assert_eq!(host_bundle_watch_initial_delay_ms(1000, 0.5, || 0.25), 250);
    assert_eq!(host_bundle_watch_next_delay_ms(1000, 0.5, || 0.0), 500);
    assert_eq!(host_bundle_watch_next_delay_ms(1000, 0.5, || 1.0), 1500);
}

#[test]
fn bundle_and_command_install_are_atomic_and_veto_ack_is_sanitized() {
    let root = std::env::temp_dir().join(format!("fabushi-host-upgrade-{}", uuid::Uuid::new_v4()));
    let dir = root.join("supervisor");
    let bundle = dir.join("incoming.tgz");
    let bundle_part = dir.join("incoming.tgz.part");
    let command = dir.join("command.json");
    let command_part = dir.join("command.json.part");
    let mut args = StageHostBundleUpgradeArgs::production("abc1234", b"bundle", 123);
    args.dir = dir.clone();
    args.staged_bundle_path = bundle.clone();
    args.staged_bundle_part_path = bundle_part.clone();
    args.command_path = command.clone();
    args.command_part_path = command_part.clone();
    args.reason = Some("manual");
    args.force_now = true;
    let installed = stage_host_bundle_upgrade(&args).unwrap();
    assert_eq!(installed.id, "upgrade-abc1234");
    assert_eq!(fs::read(bundle).unwrap(), b"bundle");
    assert!(!bundle_part.exists());
    assert!(!command_part.exists());
    let raw = fs::read_to_string(command).unwrap();
    assert!(raw.contains("\"forceNow\":true"));

    let acks = dir.join("acks");
    fs::create_dir_all(&acks).unwrap();
    fs::write(acks.join("upgrade-bad_name"), b"").unwrap();
    assert!(is_host_version_swap_vetoed("bad/name", Some("old"), &acks));
    assert!(!is_host_version_swap_vetoed(
        "bad/name",
        Some("bad/name"),
        &acks
    ));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_swap_restage_is_bounded_and_crash_loop_is_never_retried() {
    let mut state = HostBundleUpdateState {
        staged_version: Some("v2".into()),
        ..Default::default()
    };
    let marker = HostUpgradeMarker {
        outcome: Some("failed".into()),
        mode: Some("bundle".into()),
        to_version: Some("v2".into()),
        swap_error: Some("swap-failed".into()),
        ..Default::default()
    };
    assert!(note_failed_swap_marker_for_retry(&mut state, &marker, 2));
    assert_eq!(state.staged_version, None);
    state.staged_version = Some("v2".into());
    assert!(note_failed_swap_marker_for_retry(&mut state, &marker, 2));
    state.staged_version = Some("v2".into());
    assert!(!note_failed_swap_marker_for_retry(&mut state, &marker, 2));

    let crash = HostUpgradeMarker {
        swap_error: Some("post-swap-crash-loop".into()),
        ..marker
    };
    state.staged_version = Some("v2".into());
    assert!(!note_failed_swap_marker_for_retry(&mut state, &crash, 9));
}

#[test]
fn fetch_stage_reports_failure_once_and_commits_state_only_after_stage() {
    let mut state = HostBundleUpdateState::default();
    let mut reports = Vec::new();
    let result = fetch_stage_and_report_host_bundle(
        "v2",
        Some("v1"),
        "watch",
        &mut state,
        || Err("network".into()),
        |_| Ok(()),
        |event| reports.push(event),
        |_, _| {},
    );
    assert_eq!(result.unwrap_err().phase, HostBundleStagePhase::Fetch);
    assert_eq!(reports.len(), 1);

    let _ = fetch_stage_and_report_host_bundle(
        "v2",
        Some("v1"),
        "watch",
        &mut state,
        || Err("network-again".into()),
        |_| Ok(()),
        |event| reports.push(event),
        |_, _| {},
    );
    assert_eq!(reports.len(), 1);

    fetch_stage_and_report_host_bundle(
        "v2",
        Some("v1"),
        "manual",
        &mut state,
        || Ok(vec![1, 2, 3]),
        |bytes| {
            assert_eq!(bytes, &[1, 2, 3]);
            Ok(())
        },
        |_| {},
        |_, _| {},
    )
    .unwrap();
    assert_eq!(state.staged_version.as_deref(), Some("v2"));
    assert_eq!(state.last_failed_version, None);

    let cmd = build_sand_supervisor_command("id", 1, Some("v2"), None, None, false);
    assert_eq!(cmd.force_now, None);
}
