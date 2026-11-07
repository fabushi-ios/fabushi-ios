use mahayana_host_runtime::extensions::plugins::runtime_final::{
    PluginRuntimeManager, PluginStatus,
};
use mahayana_host_runtime::extensions::process::bounded_framer::BoundedLineFramer;
use mahayana_host_runtime::extensions::process::command_custody::CommandProcessCustody;
use mahayana_host_runtime::extensions::process::tree_reaper::ChildProcessTreeTracker;
use std::time::{Duration, Instant};

#[test]
fn test_ocrm_process_bounded_line_framer() {
    let mut framer = BoundedLineFramer::new(100, "Line overflow error");

    // Push chunk containing complete line and incomplete line
    let lines = framer
        .push(b"hello world\npart")
        .expect("Framing succeeds");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], b"hello world");
    assert_eq!(framer.pending_byte_length(), 4); // "part" remains in buffer

    // Push completion of second line
    let lines2 = framer.push(b"ial line\n").expect("Framing succeeds");
    assert_eq!(lines2.len(), 1);
    assert_eq!(lines2[0], b"partial line");
    assert_eq!(framer.pending_byte_length(), 0);

    // Overflow enforcement
    let mut small_framer = BoundedLineFramer::new(10, "Buffer limit exceeded");
    let res = small_framer.push(b"this is a very long line without newline");
    assert!(res.is_err());
    assert_eq!(res.unwrap_err(), "Buffer limit exceeded");
}

#[test]
fn test_ocrm_process_custody_and_tree_reaper() {
    // 1. Process tree tracker
    let tracker = ChildProcessTreeTracker::new();
    tracker.register_child(12345);
    tracker.register_child(67890);
    assert_eq!(tracker.active_count(), 2);
    assert!(tracker.is_tracked(12345));

    assert!(tracker.unregister_child(12345));
    assert_eq!(tracker.active_count(), 1);
    assert!(!tracker.is_tracked(12345));

    // 2. Command custody
    let custody = CommandProcessCustody::new();
    custody.set_group_capacity("git-ops", 2);

    let slot1 = custody.acquire_slot("git-ops").expect("Acquire slot 1");
    let slot2 = custody.acquire_slot("git-ops").expect("Acquire slot 2");

    // Third slot fails due to limit 2
    let slot3_err = custody.acquire_slot("git-ops").unwrap_err();
    assert!(slot3_err.contains("exhausted"));

    // Drop slot1 releases slot
    drop(slot1);
    let _slot3 = custody.acquire_slot("git-ops").expect("Acquire slot 3 after drop");
    drop(slot2);

    // 3. Cancellation token
    let token = custody.create_cancellation_token("cmd-001");
    assert!(!custody.is_cancelled("cmd-001"));
    assert!(custody.cancel_command("cmd-001"));
    assert!(custody.is_cancelled("cmd-001"));
    assert!(token.load(std::sync::atomic::Ordering::SeqCst));

    // 4. Deadline enforcement
    let start = Instant::now() - Duration::from_millis(200);
    assert!(custody.enforce_deadline(start, Duration::from_millis(100)).is_err());
    let fresh_start = Instant::now();
    assert!(custody.enforce_deadline(fresh_start, Duration::from_secs(10)).is_ok());
}

#[test]
fn test_ocrm_plugins_runtime_final() {
    let manager = PluginRuntimeManager::new();

    // Status tracking
    assert_eq!(manager.get_status("my-plugin"), PluginStatus::Disabled);
    manager.set_status("my-plugin", PluginStatus::Active);
    assert_eq!(manager.get_status("my-plugin"), PluginStatus::Active);

    manager.set_status("broken-plugin", PluginStatus::Error("Crash on start".to_string()));
    assert_eq!(
        manager.get_status("broken-plugin"),
        PluginStatus::Error("Crash on start".to_string())
    );

    // Uninstallation
    let receipt = manager.uninstall_plugin("my-plugin", 14);
    assert_eq!(receipt.plugin_id, "my-plugin");
    assert_eq!(receipt.files_removed, 14);
    assert_eq!(manager.get_status("my-plugin"), PluginStatus::Uninstalled);

    let fetched = manager.get_uninstall_receipt("my-plugin").expect("Receipt exists");
    assert_eq!(fetched.files_removed, 14);
}
