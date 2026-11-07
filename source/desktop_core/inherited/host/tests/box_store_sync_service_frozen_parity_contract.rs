use mahayana_host_runtime::extensions::box_store_sync::box_store_sync_service::{
    BOX_HOME_REGENERABLE_JUNK_IGNORE_PATTERNS, BOX_STORE_CHROME_INTERVAL_MS,
    build_box_home_ignore, plan_periodic_cycle,
};

#[test]
fn frozen_periodic_plan_preserves_handoff_idle_and_pack_semantics() {
    let busy = plan_periodic_cycle(false, true, BOX_STORE_CHROME_INTERVAL_MS);
    assert!(!busy.seal_hydration_handoff);
    assert!(!busy.include_idle_only);
    assert!(!busy.include_store_dbs);
    assert!(busy.skip_live_handle_store_dbs);
    assert!(!busy.include_packs);

    let seal = plan_periodic_cycle(true, true, 0);
    assert!(seal.seal_hydration_handoff);
    assert!(seal.include_idle_only);
    assert!(seal.include_store_dbs);
    assert!(!seal.skip_live_handle_store_dbs);
    assert!(seal.include_packs);

    let before_due = plan_periodic_cycle(
        true,
        false,
        BOX_STORE_CHROME_INTERVAL_MS.saturating_sub(1),
    );
    assert!(!before_due.include_idle_only);
    assert!(before_due.include_store_dbs);
    assert!(before_due.skip_live_handle_store_dbs);
    assert!(!before_due.include_packs);

    let due = plan_periodic_cycle(true, false, BOX_STORE_CHROME_INTERVAL_MS);
    assert!(due.include_idle_only);
    assert!(due.include_store_dbs);
    assert!(due.skip_live_handle_store_dbs);
    assert!(due.include_packs);
}

#[test]
fn frozen_better_cli_home_ignore_set_is_complete() {
    assert_eq!(BOX_HOME_REGENERABLE_JUNK_IGNORE_PATTERNS.len(), 23);
    let ignore = build_box_home_ignore();
    for path in [
        ".gradle/caches/modules.bin",
        ".gradle/daemon/daemon.log",
        ".gradle/wrapper/dists/x",
        ".m2/repository/a/b.jar",
        ".vnc/session.log",
        ".dbus/session-bus/x",
        ".Xauthority",
        "tmp/runtime.sock",
    ] {
        assert!(ignore.ignores(path), "expected frozen home ignore for {path}");
    }
    assert!(!ignore.ignores("projects/app/src/main.rs"));
}

#[test]
fn shipping_service_has_one_owner_and_consumes_frozen_lifecycle_plan() {
    let service = include_str!("../src/extensions/box_store_sync/box_store_sync_service.rs");
    let production = include_str!("../src/extensions/box_store_sync/production.rs");
    let extension = include_str!("../src/extensions/box_store_sync/extension.rs");
    let host_extensions = include_str!("../src/host_production_extensions.rs");

    assert_eq!(
        production.matches("pub struct ProductionBoxStoreSyncService").count(),
        1,
        "shipping Host must have one BoxStoreSync service owner"
    );
    assert!(extension.contains("service.start();"));
    assert!(extension.contains("self.service.dispose();"));
    assert!(host_extensions.contains("ProductionBoxStoreSyncFactory"));

    for marker in [
        "started.swap(true, Ordering::AcqRel)",
        "plan_periodic_cycle(",
        "plan.skip_live_handle_store_dbs",
        "plan.include_idle_only",
        "plan.include_packs",
        "TryLockError::WouldBlock",
        "accept_matching_canonical_on_conflict",
        "sync_chrome_auth_state_category(",
        "sync_chrome_profile_category(",
        "sync_box_home_category(",
        "ManifestHydrationUpdate::PromoteComplete",
        "remove_hydration_handoff_marker(&hydration_marker_path)",
        "run_local_agent_db_snapshot_unqueued",
        "poll_inner.transfer.sweep_leaked_temps(&roots)",
        "pub fn get_store_id(&self)",
        "pub fn get_box_store_status(&self)",
        "pub fn clear_box_store_now(&self)",
        "pub fn forget_agent(&self",
        "legacy_agent_store_for_source_id",
        "prepare_canonical_manifest_reset",
        "ManifestHydrationUpdate::ResetComplete",
        "store_db_complete",
        "sand_data_excludes()",
    ] {
        assert!(
            production.contains(marker),
            "shipping service is missing frozen lifecycle marker {marker}"
        );
    }

    for marker in [
        "BOX_HOME_REGENERABLE_JUNK_IGNORE_PATTERNS",
        "build_box_home_ignore",
        "plan_periodic_cycle",
    ] {
        assert!(
            service.contains(marker),
            "canonical service owner is missing frozen behavior marker {marker}"
        );
    }
}

#[test]
fn shipping_service_recovery_uses_canonical_manifest_and_object_store() {
    let production = include_str!("../src/extensions/box_store_sync/production.rs");

    let clear = production
        .split("fn clear_store_now(&self)")
        .nth(1)
        .expect("clear_store_now production implementation");
    assert!(clear.contains("self.stopped.store(true, Ordering::Release)"));
    assert!(clear.contains("reset_canonical_store"));
    assert!(clear.contains("legacy_agent_store_for_source_id"));

    let reset = production
        .split("fn reset_canonical_store(")
        .nth(1)
        .expect("canonical reset implementation");
    assert!(reset.contains("prepare_canonical_manifest_reset(store)?"));
    assert!(reset.contains("write_manifest_with_retry_and_conflict_reporter("));
    assert!(reset.contains("self.report_manifest_write_conflict(info)"));
    assert!(reset.contains("ManifestHydrationUpdate::ResetComplete"));

    let forget = production
        .split("fn forget_agent(&self")
        .nth(1)
        .expect("forget_agent production implementation");
    assert!(forget.contains("load_manifest_for_write("));
    assert!(forget.contains("write_manifest_with_retry_and_conflict_reporter("));
    assert!(forget.contains("self.report_manifest_write_conflict(info)"));

    let status = production
        .split("fn read_store_status(&self)")
        .nth(1)
        .expect("read_store_status production implementation");
    assert!(status.contains("read_manifest_strict(store.as_ref())?"));
    assert!(status.contains("count_store_db_manifest_entries"));
    assert!(status.contains("count_agent_dir_manifest_entries"));
}

#[test]
fn shipping_periodic_hydration_requires_complete_store_db_after_categories() {
    let production = include_str!("../src/extensions/box_store_sync/production.rs");
    let cycle = production
        .split("fn run_local_cycle(")
        .nth(1)
        .expect("shipping cycle");
    let category_pos = cycle
        .find("sync_box_home_category(")
        .expect("last regular category");
    let store_db_pos = cycle
        .find("let capture = sync_store_db_snapshots_with_trace(")
        .expect("store db sweep");
    assert!(
        category_pos < store_db_pos,
        "frozen behavior syncs normal categories before store.db sweep"
    );
    assert!(cycle.contains("&& store_db_complete"));
    assert!(cycle.contains("BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES"));
}
