use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repository root")
        .to_path_buf()
}

fn read(path: &str) -> String {
    let direct = repo_root().join(path);
    if direct.exists() {
        return fs::read_to_string(direct).expect("read source contract");
    }
    let rewired = if let Some(stripped) = path.strip_prefix("source/mahayana/mahayana-rs/") {
        repo_root().join("inherited/mahayana-rs").join(stripped)
    } else if let Some(stripped) = path.strip_prefix("source/host/") {
        repo_root().join("inherited/host").join(stripped)
    } else {
        direct
    };
    fs::read_to_string(rewired).expect("read source contract")
}

#[test]
fn typed_g10_protocol_contract_is_owned_by_mahayana_host_protocol() {
    let source = read("source/mahayana/mahayana-rs/mahayana-host-protocol/src/lib.rs");
    for needle in [
        "pub struct ComputerWindowRequest",
        "pub struct ComputerWindowResult",
        "pub enum ComputerWindowAction",
        "pub struct ComputerAccessibilityTreeRequest",
        "pub struct ComputerAccessibilityNode",
        "pub struct ComputerAccessibilityTree",
        "pub struct ComputerSemanticActionRequest",
        "pub struct ComputerSemanticActionResult",
        "pub enum ComputerSemanticAction",
        "#[serde(rename_all = \"camelCase\", deny_unknown_fields)]",
    ] {
        assert!(source.contains(needle), "missing typed G10 protocol owner: {needle}");
    }
}

#[test]
fn remote_computer_native_path_remains_lease_and_snapshot_fenced() {
    let source = read("source/host/src/remote_computer_target.rs");
    for needle in [
        "ComputerControlLeaseRequest::new(",
        "mahayana_computer::execute_with_lease_fenced(",
        "parsed.snapshot_generation",
        "ComputerError::SnapshotStale",
    ] {
        assert!(source.contains(needle), "remote Computer fence regressed: {needle}");
    }
    assert!(
        !source.contains("mahayana_computer::execute_with_lease(\n            &actions"),
        "remote production owner must not fall back to the unfenced API"
    );
}

#[test]
fn computer_owner_keeps_lock_preemption_and_act_then_observe() {
    let source = read("source/mahayana/mahayana-rs/mahayana-computer/src/lib.rs");
    for needle in [
        "static EXECUTION_LOCK",
        "static USER_OVERRIDE_EPOCH",
        "static SNAPSHOT_GENERATION",
        "require_snapshot_generation(expected_snapshot_generation)?;",
        "ensure_ai_not_preempted(ai_epoch)?;",
        "let snapshot = macos::capture_screen()?;",
        "let snapshot = portable::capture_screen()?;",
        "let snapshot_generation = advance_snapshot_generation()?;",
    ] {
        assert!(source.contains(needle), "Computer owner invariant regressed: {needle}");
    }
}


#[test]
fn g10_native_execution_stays_inside_fenced_computer_owner() {
    let source = read("source/mahayana/mahayana-rs/mahayana-computer/src/lib.rs");
    for needle in [
        "pub fn window_control_with_lease_fenced(",
        "pub fn accessibility_tree_with_lease_fenced(",
        "pub fn semantic_action_with_lease_fenced(",
        "with_fenced_native_control(",
        "acquire_control_lease(lease)?;",
        "require_snapshot_generation(expected_snapshot_generation)?;",
        "let _execution = EXECUTION_LOCK",
        "ensure_interactive_session()?",
        "AXUIElementCopyAttributeValue(",
        "AXUIElementPerformAction(",
        "AXUIElementSetAttributeValue(",
        "ComputerError::SnapshotStale",
        "macos-ax-window:",
        "macos-ax:{generation}:",
        "windows_native::window_control(request)",
        "windows_native::accessibility_tree(request, observation_generation)",
        "windows_native::semantic_action(request, current_generation)",
        "linux_native::window_control(request)",
        "linux_native::accessibility_tree(request, observation_generation)",
        "linux_native::semantic_action(request, current_generation)",
    ] {
        assert!(source.contains(needle), "native G10 execution owner missing: {needle}");
    }
}

#[test]
fn windows_uia_and_linux_capability_gates_are_explicit_production_owners() {
    let windows = read("source/mahayana/mahayana-rs/mahayana-computer/src/windows_native.rs");
    for needle in [
        "UIAutomation::new()",
        "UIWindowPattern",
        "UIInvokePattern",
        "UIValuePattern",
        "OpenInputDesktop",
        "SwitchDesktop",
        "windows-uia-window:",
        "windows-uia:{generation}:",
    ] {
        assert!(windows.contains(needle), "Windows native owner missing: {needle}");
    }

    let linux = read("source/mahayana/mahayana-rs/mahayana-computer/src/linux_native.rs");
    for needle in ["WAYLAND_DISPLAY", "DISPLAY", "AT_SPI_BUS_ADDRESS", "fail closed"] {
        assert!(linux.contains(needle), "Linux capability gate missing: {needle}");
    }
}
