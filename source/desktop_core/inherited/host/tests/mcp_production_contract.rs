use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mahayana_host_runtime::extensions::mcp::extension::{PollingHandle, PollingPort};
use mahayana_host_runtime::extensions::mcp::production::{
    DASHBOARD_GET_TEAMS_PATH, DASHBOARD_PUBLISH_PLUGIN_PATH, DASHBOARD_UNPUBLISH_PLUGIN_PATH,
    LegacyMcpAuthCleanupOutcome, RealPluginSkillsPolling,
    cleanup_legacy_mcp_auth_credentials, is_legacy_mcp_auth_file,
};

#[test]
fn production_rpc_paths_match_frozen_dashboard_contract() {
    assert_eq!(
        DASHBOARD_GET_TEAMS_PATH,
        "/aiserver.v1.DashboardService/GetTeams"
    );
    assert_eq!(
        DASHBOARD_PUBLISH_PLUGIN_PATH,
        "/aiserver.v1.DashboardService/PublishPlugin"
    );
    assert_eq!(
        DASHBOARD_UNPUBLISH_PLUGIN_PATH,
        "/aiserver.v1.DashboardService/UnpublishPlugin"
    );
}

#[test]
fn production_polling_runs_immediately_and_stops_after_dispose() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let calls_for_callback = Arc::clone(&calls);
    let seen_for_callback = Arc::clone(&seen);

    let polling = RealPluginSkillsPolling::with_interval(Duration::from_millis(10));
    let mut handle = polling.start(Arc::new(move || {
        let n = calls_for_callback.fetch_add(1, Ordering::SeqCst) + 1;
        seen_for_callback.lock().expect("seen poisoned").push(n);
    }));

    let deadline = Instant::now() + Duration::from_secs(1);
    while calls.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(calls.load(Ordering::SeqCst) >= 2, "polling never advanced");

    handle.dispose();
    let after_dispose = calls.load(Ordering::SeqCst);
    thread::sleep(Duration::from_millis(40));
    assert_eq!(calls.load(Ordering::SeqCst), after_dispose);
    assert!(!seen.lock().expect("seen poisoned").is_empty());
}

#[test]
fn production_polling_dispose_is_idempotent() {
    let polling = RealPluginSkillsPolling::with_interval(Duration::from_secs(60));
    let mut handle = polling.start(Arc::new(|| {}));
    handle.dispose();
    handle.dispose();
}


#[test]
fn legacy_mcp_auth_cleanup_matches_frozen_file_selection() {
    assert!(is_legacy_mcp_auth_file("mcp-auth.json"));
    assert!(is_legacy_mcp_auth_file("mcp-auth.json.backup"));
    assert!(!is_legacy_mcp_auth_file("mcp-auth.jsonx"));

    let root = std::env::temp_dir().join(format!(
        "fabushi-mcp-auth-cleanup-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("mcp-auth.json"), b"legacy").unwrap();
    fs::write(root.join("mcp-auth.json.old"), b"legacy").unwrap();
    fs::write(root.join("keep.json"), b"keep").unwrap();

    let (outcome, removed) = cleanup_legacy_mcp_auth_credentials(&root);
    assert_eq!(outcome, LegacyMcpAuthCleanupOutcome::Deleted);
    assert_eq!(removed, 2);
    assert!(!root.join("mcp-auth.json").exists());
    assert!(!root.join("mcp-auth.json.old").exists());
    assert!(root.join("keep.json").exists());

    let (outcome, removed) = cleanup_legacy_mcp_auth_credentials(&root);
    assert_eq!(outcome, LegacyMcpAuthCleanupOutcome::NotFound);
    assert_eq!(removed, 0);
    let _ = fs::remove_dir_all(root);
}
