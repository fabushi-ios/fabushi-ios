use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::gateway::{
    dispatch_production_session_gateway_call, SessionGatewayError,
};
use mahayana_host_runtime::extensions::session::legacy_telegram_retirement::{
    LegacyTelegramRetirementError, RetiredTelegramMarker, APP_HOST_DIRNAME, FEATURE_HOST_DIRNAME,
    TELEGRAM_DIRNAME,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use serde_json::json;

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "fabushi-legacy-retirement-{label}-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn file_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut map = BTreeMap::new();
    if !root.exists() {
        return map;
    }
    fn visit(base: &Path, current: &Path, map: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if let Ok(entries) = fs::read_dir(current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(base, &path, map);
                } else if path.is_file() {
                    if let Ok(rel) = path.strip_prefix(base) {
                        if let Ok(content) = fs::read(&path) {
                            map.insert(rel.to_path_buf(), content);
                        }
                    }
                }
            }
        }
    }
    visit(root, root, &mut map);
    map
}

fn setup_legacy_fixture(root: &Path) -> PathBuf {
    let feature_host = root.join(APP_HOST_DIRNAME).join(FEATURE_HOST_DIRNAME);
    let telegram_dir = feature_host.join(TELEGRAM_DIRNAME);
    fs::create_dir_all(&telegram_dir).expect("create telegram dir");

    // Write legacy sqlite database and sidecars
    fs::write(
        telegram_dir.join("account-123.sqlite3"),
        b"legacy-sqlite3-binary-content-123\0\xff",
    )
    .expect("write sqlite3");
    fs::write(
        telegram_dir.join("account-123.sqlite3-wal"),
        b"legacy-sqlite3-wal-log",
    )
    .expect("write wal");
    fs::write(
        telegram_dir.join("account-123.sqlite3-shm"),
        b"legacy-sqlite3-shm-index",
    )
    .expect("write shm");
    fs::write(
        telegram_dir.join("preserved-user-data.bin"),
        b"legacy-custom-user-cache",
    )
    .expect("write preserved");

    // Write sibling files in feature-host that must NEVER be touched
    fs::write(
        feature_host.join("product-settings.json"),
        b"{\"network\":{\"proxyEnabled\":false}}",
    )
    .expect("write product-settings");
    let runtime_ws = feature_host.join("runtime/workspace");
    fs::create_dir_all(&runtime_ws).expect("create runtime workspace");
    fs::write(runtime_ws.join("workspace-file.txt"), b"workspace content")
        .expect("write workspace file");

    // Write external file outside feature-host
    let outside = root.join("app-host/account-session.json");
    fs::write(&outside, b"{\"activeAccount\":\"acc-1\"}").expect("write outside");

    feature_host
}

/// 1) 无有效 Native 凭证时绝不执行；若无有效 Native 凭证，立即拒绝且绝不触碰任何文件。
#[test]
fn requirement_1_refuses_when_no_valid_native_credential() {
    let fixture = TestDir::new("req1-no-credential");
    let feature_host = setup_legacy_fixture(fixture.path());
    let app_host_dir = fixture.path().join(APP_HOST_DIRNAME);
    let before_snapshot = file_snapshot(&app_host_dir);

    let session = Arc::new(ProductionSessionWorkers::with_agents_root(
        fixture.path().join("agents"),
        500,
    ));
    session.set_account_data_root(fixture.path());
    session.set_has_valid_native_credential(false);

    let key_cleared = Arc::new(AtomicBool::new(false));
    let key_cleared_clone = Arc::clone(&key_cleared);
    session.set_telegram_key_clear_callback(move || {
        key_cleared_clone.store(true, Ordering::SeqCst);
        Ok(())
    });

    // Calling Phase 1 directly must be rejected
    let err_phase1 = session
        .retire_legacy_telegram_data(false)
        .expect_err("must be rejected without credential");
    assert!(matches!(
        err_phase1,
        LegacyTelegramRetirementError::NoValidNativeCredential
    ));

    // Calling Phase 2 directly must also be rejected
    let err_phase2 = session
        .retire_legacy_telegram_data(true)
        .expect_err("must be rejected without credential");
    assert!(matches!(
        err_phase2,
        LegacyTelegramRetirementError::NoValidNativeCredential
    ));

    // Calling via Gateway must return BadRequest
    let gw_phase1 = dispatch_production_session_gateway_call(
        &session,
        "retireLegacyTelegramData",
        &json!({"removeImmediately": false}),
    )
    .expect("handled");
    assert!(matches!(gw_phase1, Err(SessionGatewayError::BadRequest(_))));

    let gw_phase2 = dispatch_production_session_gateway_call(
        &session,
        "retireLegacyTelegramData",
        &json!({"removeImmediately": true}),
    )
    .expect("handled");
    assert!(matches!(gw_phase2, Err(SessionGatewayError::BadRequest(_))));

    // Verification: Absolutely NO files touched, no marker written, callback not triggered
    assert_eq!(
        file_snapshot(&app_host_dir),
        before_snapshot,
        "app-host filesystem must remain completely untouched when native credential is invalid"
    );
    assert!(!key_cleared.load(Ordering::SeqCst));
    assert!(feature_host.join("telegram").exists());
    assert!(!feature_host.join(".retired-telegram-marker.json").exists());
}

/// 2) 保留窗口期内（未触发退役时）旧文件原封不动，支持回滚；
#[test]
fn requirement_2_retention_window_preserves_old_files_for_rollback() {
    let fixture = TestDir::new("req2-retention-window");
    let feature_host = setup_legacy_fixture(fixture.path());
    let app_host_dir = fixture.path().join(APP_HOST_DIRNAME);
    let before_snapshot = file_snapshot(&app_host_dir);

    let session = Arc::new(ProductionSessionWorkers::with_agents_root(
        fixture.path().join("agents"),
        500,
    ));
    session.set_account_data_root(fixture.path());
    session.set_has_valid_native_credential(true);

    // Normal host operations (listing agents, counting, checking paths) during retention window
    let count = session.count_owned_agents().expect("count agents");
    assert_eq!(count, 0);

    let list = dispatch_production_session_gateway_call(&session, "listAgents", &json!({}))
        .expect("handled")
        .expect("success");
    assert!(list.is_array());

    // Without triggering retirement, all legacy Telegram files remain completely untouched
    let after_snapshot = file_snapshot(&app_host_dir);
    assert_eq!(
        after_snapshot, before_snapshot,
        "retention window must preserve all legacy telegram files byte-for-byte"
    );

    // Rollback check: verify legacy sqlite file can still be opened / read
    let sqlite_bytes =
        fs::read(feature_host.join("telegram/account-123.sqlite3")).expect("read sqlite3");
    assert_eq!(sqlite_bytes, b"legacy-sqlite3-binary-content-123\0\xff");
}

/// 3) 阶段 1 仅重命名精确的 telegram 目录而不触碰外部文件，且多次调用完全幂等；
#[test]
fn requirement_3_phase1_renames_only_exact_directory_and_is_idempotent() {
    let fixture = TestDir::new("req3-phase1-idempotent");
    let feature_host = setup_legacy_fixture(fixture.path());

    // Sibling snapshots before retirement
    let product_settings_before = fs::read(feature_host.join("product-settings.json")).unwrap();
    let runtime_ws_before =
        fs::read(feature_host.join("runtime/workspace/workspace-file.txt")).unwrap();
    let outside_before = fs::read(fixture.path().join("app-host/account-session.json")).unwrap();

    let session = Arc::new(ProductionSessionWorkers::with_agents_root(
        fixture.path().join("agents"),
        500,
    ));
    session.set_account_data_root(fixture.path());
    session.set_has_valid_native_credential(true);

    // First call: Phase 1 (Retire / Rename, Release N+2)
    let report1 = session
        .retire_legacy_telegram_data(false)
        .expect("phase 1 retirement succeeds");

    assert!(report1.success);
    assert_eq!(report1.phase, "N+2");
    assert!(!report1.purged);
    assert!(!report1.telegram_key_cleared);
    assert!(!report1.marker_recovered);

    let retired_name = report1.retired_dir_name.expect("retiredDirName present");
    assert!(retired_name.starts_with(".retired-telegram-"));
    assert!(report1.retired_at_ms.is_some());

    // Exact `telegram` directory was renamed to `.retired-telegram-<timestamp>`
    assert!(!feature_host.join("telegram").exists());
    let retired_dir = feature_host.join(&retired_name);
    assert!(retired_dir.is_dir());
    assert!(retired_dir.join("account-123.sqlite3").exists());
    assert!(retired_dir.join("account-123.sqlite3-wal").exists());
    assert!(retired_dir.join("account-123.sqlite3-shm").exists());
    assert!(retired_dir.join("preserved-user-data.bin").exists());

    // Marker file is created
    let marker_file = feature_host.join(".retired-telegram-marker.json");
    assert!(marker_file.is_file());
    let marker_raw = fs::read_to_string(&marker_file).unwrap();
    let marker: RetiredTelegramMarker = serde_json::from_str(&marker_raw).unwrap();
    assert_eq!(marker.retired_dir_name, Some(retired_name.clone()));
    assert_eq!(marker.retired_at_ms, report1.retired_at_ms.unwrap());
    assert_eq!(marker.release_phase, "N+2");
    assert!(!marker.purged);

    // Sibling files and external files are 100% untouched
    assert_eq!(
        fs::read(feature_host.join("product-settings.json")).unwrap(),
        product_settings_before
    );
    assert_eq!(
        fs::read(feature_host.join("runtime/workspace/workspace-file.txt")).unwrap(),
        runtime_ws_before
    );
    assert_eq!(
        fs::read(fixture.path().join("app-host/account-session.json")).unwrap(),
        outside_before
    );

    // Snapshot after first retirement
    let snapshot_after_call1 = file_snapshot(fixture.path());

    // Second call: direct method call (idempotent)
    let report2 = session
        .retire_legacy_telegram_data(false)
        .expect("phase 1 call 2 succeeds");
    assert_eq!(report2.phase, "N+2");
    assert_eq!(report2.retired_dir_name, Some(retired_name.clone()));
    assert_eq!(report2.retired_at_ms, report1.retired_at_ms);
    assert!(!report2.purged);

    // Third call: via Gateway route (idempotent)
    let gw_res =
        dispatch_production_session_gateway_call(&session, "retireLegacyTelegramData", &json!({}))
            .expect("handled")
            .expect("success");
    assert_eq!(gw_res["phase"], "N+2");
    assert_eq!(gw_res["retiredDirName"], retired_name);
    assert_eq!(gw_res["purged"], false);

    // Filesystem must remain completely unchanged across repeated idempotent calls
    assert_eq!(
        file_snapshot(fixture.path()),
        snapshot_after_call1,
        "subsequent Phase 1 calls must not create extra directories or mutate marker"
    );
}

/// 4) 在 rename 与写 marker 之间模拟崩溃后，重启能自动恢复 marker 并保持幂等；
#[test]
fn requirement_4_crash_between_rename_and_marker_auto_recovers_and_is_idempotent() {
    let fixture = TestDir::new("req4-crash-recovery");
    let feature_host = fixture
        .path()
        .join(APP_HOST_DIRNAME)
        .join(FEATURE_HOST_DIRNAME);

    // Simulate crash after rename:
    // .retired-telegram-1730000000000 exists, but NO .retired-telegram-marker.json exists!
    let simulated_retired_name = ".retired-telegram-1730000000000";
    let simulated_retired_dir = feature_host.join(simulated_retired_name);
    fs::create_dir_all(&simulated_retired_dir).expect("create simulated retired dir");
    fs::write(
        simulated_retired_dir.join("crashed.sqlite3"),
        b"crashed-store",
    )
    .unwrap();

    // Sibling file
    fs::write(
        feature_host.join("product-settings.json"),
        b"{\"intact\":true}",
    )
    .unwrap();

    let marker_file = feature_host.join(".retired-telegram-marker.json");
    assert!(
        !marker_file.exists(),
        "marker must not exist before recovery"
    );
    assert!(!feature_host.join("telegram").exists());

    // Host restarts and runs retirement
    let session = Arc::new(ProductionSessionWorkers::with_agents_root(
        fixture.path().join("agents"),
        500,
    ));
    session.set_account_data_root(fixture.path());
    session.set_has_valid_native_credential(true);

    let report = session
        .retire_legacy_telegram_data(false)
        .expect("recovery call succeeds");

    assert!(report.success);
    assert!(
        report.marker_recovered,
        "marker must be reported as recovered"
    );
    assert_eq!(
        report.retired_dir_name,
        Some(simulated_retired_name.to_string())
    );
    assert_eq!(report.retired_at_ms, Some(1730000000000));
    assert_eq!(report.phase, "N+2");
    assert!(!report.purged);

    // Verify marker file was written automatically on disk
    assert!(marker_file.is_file());
    let marker_raw = fs::read_to_string(&marker_file).unwrap();
    let marker: RetiredTelegramMarker = serde_json::from_str(&marker_raw).unwrap();
    assert_eq!(
        marker.retired_dir_name,
        Some(simulated_retired_name.to_string())
    );
    assert_eq!(marker.retired_at_ms, 1730000000000);
    assert_eq!(marker.release_phase, "N+2");
    assert!(!marker.purged);

    // Second call after recovery is idempotent
    let report_again = session
        .retire_legacy_telegram_data(false)
        .expect("subsequent call succeeds");
    assert_eq!(
        report_again.retired_dir_name,
        Some(simulated_retired_name.to_string())
    );
    assert_eq!(report_again.retired_at_ms, Some(1730000000000));
    assert!(
        !report_again.marker_recovered,
        "marker is now already present"
    );
    assert!(!report_again.purged);
}

/// 5) 用户触发 remove_immediately: true（或 N+3 阶段）彻底清理 .retired-telegram-* 与 telegram-key。
#[test]
fn requirement_5_remove_immediately_purges_directory_and_clears_telegram_key() {
    // Scenario 5A: Purge after Phase 1 retirement
    {
        let fixture = TestDir::new("req5a-purge-after-phase1");
        let feature_host = setup_legacy_fixture(fixture.path());

        let session = Arc::new(ProductionSessionWorkers::with_agents_root(
            fixture.path().join("agents"),
            500,
        ));
        session.set_account_data_root(fixture.path());
        session.set_has_valid_native_credential(true);

        let key_cleared = Arc::new(AtomicBool::new(false));
        let key_cleared_clone = Arc::clone(&key_cleared);
        session.set_telegram_key_clear_callback(move || {
            key_cleared_clone.store(true, Ordering::SeqCst);
            Ok(())
        });

        // Step 1: Perform Phase 1 retirement
        let report1 = session.retire_legacy_telegram_data(false).unwrap();
        let retired_name = report1.retired_dir_name.unwrap();
        assert!(feature_host.join(&retired_name).is_dir());
        assert!(!key_cleared.load(Ordering::SeqCst));

        // Step 2: User clicks "Remove legacy data" (remove_immediately: true)
        let purge_report = session
            .retire_legacy_telegram_data(true)
            .expect("purge succeeds");

        assert!(purge_report.success);
        assert_eq!(purge_report.phase, "N+3");
        assert!(purge_report.purged);
        assert!(purge_report.telegram_key_cleared);
        assert!(
            key_cleared.load(Ordering::SeqCst),
            "telegram-key callback must be invoked"
        );

        // The .retired-telegram-* directory is thoroughly deleted
        assert!(!feature_host.join(&retired_name).exists());
        assert!(!feature_host.join("telegram").exists());

        // Marker file reflects purged: true
        let marker_file = feature_host.join(".retired-telegram-marker.json");
        let marker: RetiredTelegramMarker =
            serde_json::from_str(&fs::read_to_string(&marker_file).unwrap()).unwrap();
        assert!(marker.purged);
        assert_eq!(marker.release_phase, "N+3");
        assert!(marker.purged_at_ms.is_some());

        // Sibling files remain untouched
        assert!(feature_host.join("product-settings.json").is_file());
        assert!(feature_host
            .join("runtime/workspace/workspace-file.txt")
            .is_file());

        // Step 3: Repeated call with remove_immediately: true is idempotent
        let second_purge = session
            .retire_legacy_telegram_data(true)
            .expect("repeat purge succeeds");
        assert!(second_purge.purged);
        assert_eq!(second_purge.phase, "N+3");
    }

    // Scenario 5B: User triggers "Remove legacy data" directly before Phase 1
    {
        let fixture = TestDir::new("req5b-direct-purge-phase0");
        let feature_host = setup_legacy_fixture(fixture.path());

        let session = Arc::new(ProductionSessionWorkers::with_agents_root(
            fixture.path().join("agents"),
            500,
        ));
        session.set_account_data_root(fixture.path());
        session.set_has_valid_native_credential(true);

        let key_cleared = Arc::new(AtomicBool::new(false));
        let key_cleared_clone = Arc::clone(&key_cleared);
        session.set_telegram_key_clear_callback(move || {
            key_cleared_clone.store(true, Ordering::SeqCst);
            Ok(())
        });

        // Gateway call with removeImmediately: true
        let gw_res = dispatch_production_session_gateway_call(
            &session,
            "retireLegacyTelegramData",
            &json!({"removeImmediately": true}),
        )
        .expect("handled")
        .expect("success");

        assert_eq!(gw_res["phase"], "N+3");
        assert_eq!(gw_res["purged"], true);
        assert_eq!(gw_res["telegramKeyCleared"], true);
        assert!(key_cleared.load(Ordering::SeqCst));

        // telegram directory is thoroughly deleted
        assert!(!feature_host.join("telegram").exists());

        // Marker file written with purged: true
        let marker_file = feature_host.join(".retired-telegram-marker.json");
        let marker: RetiredTelegramMarker =
            serde_json::from_str(&fs::read_to_string(&marker_file).unwrap()).unwrap();
        assert!(marker.purged);
        assert_eq!(marker.release_phase, "N+3");

        // Siblings untouched
        assert!(feature_host.join("product-settings.json").is_file());
        assert!(feature_host
            .join("runtime/workspace/workspace-file.txt")
            .is_file());
        assert!(fixture
            .path()
            .join("app-host/account-session.json")
            .is_file());
    }
}

/// Boundary security: symlink pointing outside feature-host must be rejected
#[test]
fn requirement_boundary_security_rejects_symlink_escaping_feature_host() {
    let fixture = TestDir::new("req-symlink-security");
    let feature_host = fixture
        .path()
        .join(APP_HOST_DIRNAME)
        .join(FEATURE_HOST_DIRNAME);
    fs::create_dir_all(&feature_host).unwrap();

    // Create an external directory outside feature-host
    let outside_target = fixture.path().join("external-sensitive-data");
    fs::create_dir_all(&outside_target).unwrap();
    fs::write(outside_target.join("secret.txt"), b"sensitive-key-material").unwrap();

    // Create symlink: feature-host/telegram -> outside_target
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_target, feature_host.join("telegram")).unwrap();

    let session = Arc::new(ProductionSessionWorkers::with_agents_root(
        fixture.path().join("agents"),
        500,
    ));
    session.set_account_data_root(fixture.path());
    session.set_has_valid_native_credential(true);

    // Phase 1 should reject the symlink
    #[cfg(unix)]
    {
        let err = session
            .retire_legacy_telegram_data(false)
            .expect_err("symlink must be rejected");
        assert!(matches!(
            err,
            LegacyTelegramRetirementError::SecurityViolation(_)
        ));

        // External target must NEVER be touched
        assert!(outside_target.join("secret.txt").is_file());
        assert_eq!(
            fs::read(outside_target.join("secret.txt")).unwrap(),
            b"sensitive-key-material"
        );
    }
}
