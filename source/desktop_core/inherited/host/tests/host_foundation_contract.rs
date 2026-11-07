use std::sync::{Arc, Mutex};

use mahayana_host_runtime::{
    attachment_paths::{get_agent_assets_dir, get_agent_attachments_dir, get_agent_media_store_roots},
    automations::automation_id::stable_automation_id,
    r#box::box_monitor_layout::{display_space_sentence, SAND_MONITOR_HEIGHT, SAND_MONITOR_WIDTH},
    durable_file_policy::{BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES, SAND_UPGRADE_RESUME_FILE_NAME},
    host_diagnostics::{pin_host_diagnostics_reporter, report_host_diagnostic, HostDiagnostic},
    ports::{transport::{SandTransport, SandUpdate}, user_computer::{SingleUserComputer, DEFAULT_SAND_COMPUTER_ID}},
    sand_quiet_work_origin::SAND_QUIET_WORK_ORIGIN_KEY,
    sand_user_identity::{normalize_sand_user_full_name, render_user_identity_system_prompt, MAX_FULL_NAME_LENGTH},
    sha256::sha256_hex,
    storage::folder_id::is_safe_folder_id,
};
use serde_json::Map;

#[test]
fn host_foundation_constants_and_hashing_match_reference_contracts() {
    assert_eq!(SAND_QUIET_WORK_ORIGIN_KEY, "sand.quiet-work-origin");
    assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert!(is_safe_folder_id("agent-42"));
    for unsafe_id in ["", ".", "..", "a/b", "a\\b", "a\0b"] { assert!(!is_safe_folder_id(unsafe_id)); }
    assert_eq!(SAND_MONITOR_WIDTH, 1280);
    assert_eq!(SAND_MONITOR_HEIGHT, 800);
    assert!(display_space_sentence(None, None).contains("0..1279 × 0..799"));
    assert_eq!(BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES[0], SAND_UPGRADE_RESUME_FILE_NAME);
}

#[test]
fn automation_id_is_stable_uuid_shaped_and_versioned() {
    let a = stable_automation_id("agent", "local");
    let b = stable_automation_id("agent", "local");
    assert_eq!(a, b);
    assert_eq!(a.len(), 36);
    assert_eq!(&a[14..15], "5");
    assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
}

#[test]
fn attachment_paths_and_single_computer_follow_grok_defaults() {
    assert_eq!(get_agent_attachments_dir("/tmp/a").to_string_lossy(), "/tmp/a/attachments");
    assert_eq!(get_agent_assets_dir("/tmp/a").to_string_lossy(), "/tmp/a/assets");
    assert_eq!(get_agent_media_store_roots("/tmp/a").len(), 2);
    let computer = SingleUserComputer::always_connected(7_u32, None);
    assert_eq!(computer.list()[0].id, DEFAULT_SAND_COMPUTER_ID);
    assert_eq!(computer.resolve(None).unwrap().computer, &7);
    assert!(computer.resolve(Some("other")).is_none());
}

#[test]
fn transport_tracks_only_send_and_reaction_outcomes() {
    let mut next = 0_u32;
    let mut transport = SandTransport::new(|update: &SandUpdate| { next += 1; Some(format!("{}-{next}", update.kind)) });
    transport.on_update(&SandUpdate { kind: "send-message".into(), fields: Map::new() });
    assert_eq!(transport.last_sent_message_id(), Some("send-message-1"));
    transport.on_update(&SandUpdate { kind: "react-to-message".into(), fields: Map::new() });
    assert!(transport.last_reaction_applied());
}

#[test]
fn user_identity_clamps_and_diagnostics_reporter_is_replaceable() {
    assert_eq!(normalize_sand_user_full_name(Some("  Ada   Lovelace ")).as_deref(), Some("Ada Lovelace"));
    assert_eq!(normalize_sand_user_full_name(Some("   ")), None);
    assert_eq!(normalize_sand_user_full_name(Some(&"x".repeat(300))).unwrap().len(), MAX_FULL_NAME_LENGTH);
    assert!(render_user_identity_system_prompt(Some("Ada")).contains("Your user is Ada"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    pin_host_diagnostics_reporter(Some(Arc::new(move |d| sink.lock().unwrap().push(d.kind.clone()))));
    report_host_diagnostic(&HostDiagnostic { kind: "ready".into(), fields: Map::new() });
    pin_host_diagnostics_reporter(None);
    assert_eq!(&*seen.lock().unwrap(), &["ready".to_string()]);
}
