use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::host_diagnostics::{HostDiagnostic, pin_host_diagnostics_reporter};
use mahayana_host_runtime::extensions::settings::settings_service::{
    MCP_CUSTOM_INSTRUCTIONS_MAX_LENGTH, SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS,
    SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES, SandAutoReviewInstructions,
    SandLocalToolPermission, SettingsService, is_valid_iana_time_zone,
    get_json_value_at_path, set_json_value_at_path,
};

fn temp_path(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-settings-{label}-{}-{suffix}.json",
        std::process::id()
    ))
}

#[test]
fn host_settings_projects_secret_audit_findings_without_secret_values() {
    let path = temp_path("secret-audit");
    let service = SettingsService::new(&path);
    let observed = Arc::new(Mutex::new(Vec::<HostDiagnostic>::new()));
    let sink = Arc::clone(&observed);
    pin_host_diagnostics_reporter(Some(Arc::new(move |diagnostic| {
        if diagnostic.kind == "security.secrets.finding" {
            sink.lock().expect("secret diagnostics").push(diagnostic.clone());
        }
    })));

    service
        .set_host_settings(&serde_json::json!({
            "agents": {
                "entries": {
                    "main": { "apiKey": "sk-settings-plaintext-secret" }
                }
            }
        }))
        .expect("host settings");

    pin_host_diagnostics_reporter(None);
    let findings = observed.lock().expect("secret diagnostics");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].fields["findingKind"], "plaintext_residue");
    assert!(!serde_json::to_string(&findings[0].fields)
        .expect("serialize findings")
        .contains("sk-settings-plaintext-secret"));
    let _ = fs::remove_file(path);
}

#[test]
fn host_settings_updates_skill_entries_without_overwriting_redacted_secrets() {
    let path = temp_path("skill-mutation");
    fs::write(
        &path,
        r#"{"version":1,"skills":{"entries":{"demo":{"enabled":true,"apiKey":"existing","env":{"KEEP":"old","REMOVE":"gone"}}}}}"#,
    )
    .expect("seed skill settings");
    let service = SettingsService::new(&path);

    let host = service
        .set_host_settings(&serde_json::json!({
            "skills": {
                "entries": {
                    "demo": {
                        "enabled": false,
                        "apiKey": " [REDACTED] ",
                        "env": {
                            "KEEP": " [REDACTED] ",
                            "REMOVE": "",
                            "NEW": " value "
                        }
                    }
                }
            }
        }))
        .expect("skill settings");

    assert_eq!(host["skills"]["entries"]["demo"]["enabled"], false);
    assert_eq!(host["skills"]["entries"]["demo"]["apiKey"], "existing");
    assert_eq!(host["skills"]["entries"]["demo"]["env"]["KEEP"], "old");
    assert_eq!(host["skills"]["entries"]["demo"]["env"]["NEW"], "value");
    assert!(host["skills"]["entries"]["demo"]["env"]
        .get("REMOVE")
        .is_none());
    let _ = fs::remove_file(path);
}

#[test]
fn timezone_settings_validate_persist_preserve_unknown_fields_and_notify_effective_changes() {
    let path = temp_path("timezone");
    fs::write(
        &path,
        r#"{"version":1,"unrelated":{"keep":true},"userTimeZone":"UTC"}"#,
    )
    .expect("seed settings");
    let service = SettingsService::new(&path);
    assert!(is_valid_iana_time_zone("America/Los_Angeles"));
    assert!(!is_valid_iana_time_zone("Not/AZone"));
    assert_eq!(service.get_user_time_zone().as_deref(), Some("UTC"));

    let observed = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let observed_for_listener = Arc::clone(&observed);
    let _subscription = service.subscribe_to_user_time_zone(Arc::new(move |zone| {
        observed_for_listener.lock().expect("observed lock").push(zone);
    }));

    assert!(!service
        .set_user_time_zone_override(Some("Not/AZone"))
        .expect("invalid zone ignored"));
    assert!(service
        .set_user_time_zone_override(Some(" America/Los_Angeles "))
        .expect("set override"));
    assert_eq!(
        service.get_user_time_zone().as_deref(),
        Some("America/Los_Angeles")
    );

    assert!(service
        .set_user_time_zone(Some("Europe/Berlin"))
        .expect("set detected zone"));
    assert_eq!(
        service.get_user_time_zone().as_deref(),
        Some("America/Los_Angeles")
    );

    assert!(service
        .set_user_time_zone_override(Some(""))
        .expect("clear override"));
    assert_eq!(service.get_user_time_zone().as_deref(), Some("Europe/Berlin"));

    let persisted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("settings text"))
            .expect("settings json");
    assert_eq!(persisted["unrelated"]["keep"], true);
    assert_eq!(
        observed.lock().expect("observed lock").as_slice(),
        &[
            Some("America/Los_Angeles".to_string()),
            Some("Europe/Berlin".to_string())
        ]
    );
    let _ = fs::remove_file(path);
}

#[test]
fn auto_review_instructions_default_normalize_persist_and_notify() {
    let path = temp_path("auto-review");
    let service = SettingsService::new(&path);
    assert_eq!(
        service.get_auto_review_instructions(),
        SandAutoReviewInstructions::default()
    );

    let changed = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let changed_for_listener = Arc::clone(&changed);
    let _subscription = service.subscribe_to_changes(Arc::new(move |keys| {
        changed_for_listener.lock().expect("changed lock").push(keys);
    }));

    let oversized = "x".repeat(SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS + 50);
    let mut allow = vec!["  allow shell  ".to_string(), "allow shell".to_string(), oversized];
    for index in 0..(SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES + 10) {
        allow.push(format!("allow-{index}"));
    }
    let instructions = SandAutoReviewInstructions {
        is_enabled: false,
        allow_instructions: allow,
        block_instructions: vec!["  block delete  ".into(), "".into()],
    };
    assert!(service
        .set_auto_review_instructions(&instructions)
        .expect("persist auto-review instructions"));

    let normalized = service.get_auto_review_instructions();
    assert!(!normalized.is_enabled);
    assert_eq!(normalized.allow_instructions[0], "allow shell");
    assert_eq!(normalized.allow_instructions.len(), SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES);
    assert_eq!(
        normalized.allow_instructions[1].chars().count(),
        SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS
    );
    assert_eq!(normalized.block_instructions, vec!["block delete".to_string()]);
    assert_eq!(
        changed.lock().expect("changed lock").as_slice(),
        &[vec!["autoReviewInstructions".to_string()]]
    );
    assert!(!service
        .set_auto_review_instructions(&normalized)
        .expect("identical normalized settings are stable"));

    let host = service.get_host_settings();
    assert_eq!(host["autoReviewInstructions"]["isEnabled"], false);
    let _ = fs::remove_file(path);
}


#[test]
fn frozen_mcp_settings_normalize_migrate_delete_and_account_scope() {
    let path = temp_path("mcp");
    let service = SettingsService::new(&path);
    let oversized = "x".repeat(MCP_CUSTOM_INSTRUCTIONS_MAX_LENGTH + 32);

    service
        .set_mcp_custom_instructions(BTreeMap::from([
            ("hex".into(), "".into()),
            ("plain".into(), "".into()),
            ("alpha".into(), oversized),
        ]))
        .expect("legacy instructions");
    let legacy = service.get_mcp_custom_instructions();
    assert_eq!(legacy.get("hex").map(String::as_str), Some(""));
    assert!(!legacy.contains_key("plain"));
    assert_eq!(
        legacy.get("alpha").map(|value| value.chars().count()),
        Some(MCP_CUSTOM_INSTRUCTIONS_MAX_LENGTH)
    );

    service
        .set_mcp_custom_instructions(BTreeMap::from([(
            "Legacy".into(),
            "instruction".into(),
        )]))
        .expect("legacy seed");
    assert!(service
        .migrate_mcp_custom_instruction_to_server_id("11", "Legacy")
        .expect("migrate"));
    assert_eq!(
        service
            .get_raw_mcp_custom_instruction_by_server_id("11")
            .as_deref(),
        Some("instruction")
    );
    assert!(service
        .delete_mcp_custom_instruction_by_server_id("11", "Legacy", true)
        .expect("delete"));
    assert!(service.get_raw_mcp_custom_instruction("Legacy").is_none());

    service
        .set_mcp_disabled_tools_by_server_id(BTreeMap::from([
            (
                "12".into(),
                vec!["A".into(), "A".into(), "".into(), "B".into()],
            ),
            ("bad".into(), vec!["X".into()]),
        ]))
        .expect("disabled tools");
    assert_eq!(
        service.get_mcp_disabled_tools_by_server_id(),
        BTreeMap::from([("12".into(), vec!["A".into(), "B".into()])])
    );

    service.scope_to_account("account-a").expect("initial scope");
    service
        .set_mcp_custom_instructions(BTreeMap::from([("x".into(), "y".into())]))
        .expect("scoped mcp");
    service
        .set_local_tool_permission(SandLocalToolPermission::Always)
        .expect("permission");
    assert!(service.scope_to_account("account-b").expect("scope change"));
    assert!(service.get_mcp_custom_instructions().is_empty());
    assert_eq!(
        service.get_local_tool_permission(),
        SandLocalToolPermission::Ask
    );
    let _ = fs::remove_file(path);
}

#[test]
fn permission_ceiling_feature_override_and_inference_usage_match_frozen_contract() {
    let path = temp_path("runtime");
    let service = SettingsService::new(&path);
    service
        .set_local_tool_permission(SandLocalToolPermission::Always)
        .expect("choice");
    service
        .set_local_tool_permission_ceiling(Some(SandLocalToolPermission::Ask))
        .expect("ceiling");
    assert_eq!(
        service.get_local_tool_permission(),
        SandLocalToolPermission::Ask
    );

    let observed = Arc::new(Mutex::new(Vec::<BTreeMap<String, bool>>::new()));
    let sink = Arc::clone(&observed);
    let _subscription =
        service.subscribe_to_feature_flag_overrides(Arc::new(move |overrides| {
            sink.lock().expect("feature overrides").push(overrides);
        }));
    service
        .set_host_settings(&serde_json::json!({
            "featureFlagOverrides": {
                "sand_multitask": false,
                "sand_spotlight": true
            }
        }))
        .expect("host settings");
    assert_eq!(
        observed.lock().expect("feature overrides").as_slice(),
        &[BTreeMap::from([
            ("sand_multitask".into(), false),
            ("sand_spotlight".into(), true),
        ])]
    );

    assert_eq!(service.get_inference_provider(), "cursor");
    assert!(service
        .set_inference_provider("codex")
        .expect("set provider"));
    assert!(!service
        .set_inference_provider("invalid")
        .expect("reject provider"));
    service
        .record_inference_usage(
            "codex",
            Some(10.4),
            Some(4.6),
            Some(-1.0),
            Some(f64::NAN),
        )
        .expect("record usage");
    let usage = service.get_inference_router_usage();
    assert_eq!(usage["providers"]["codex"]["requests"], 1);
    assert_eq!(usage["providers"]["codex"]["inputTokens"], 10);
    assert_eq!(usage["providers"]["codex"]["outputTokens"], 5);
    assert_eq!(usage["providers"]["codex"]["cacheReadTokens"], 0);
    assert_eq!(usage["providers"]["codex"]["cacheWriteTokens"], 0);
    assert!(usage["providers"]["codex"]["lastUsedAt"].is_string());

    let host = service.get_host_settings();
    assert_eq!(host["notifications"]["isEnabled"], false);
    assert_eq!(host["notifications"]["minIntervalMs"], 5000);
    let _ = fs::remove_file(path);
}


#[test]
fn host_settings_switch_scope_before_applying_same_update_and_normalizes_invalid_permission() {
    let path = temp_path("host-update-order");
    let service = SettingsService::new(&path);
    service.scope_to_account("account-a").expect("first scope");
    service
        .set_mcp_custom_instructions(BTreeMap::from([("old".into(), "old-value".into())]))
        .expect("old instructions");
    service
        .set_has_seen_onboarding(true)
        .expect("onboarding owner");

    let host = service
        .set_host_settings(&serde_json::json!({
            "mcpCustomInstructionsAccountScope": "account-b",
            "mcpCustomInstructions": { "new": "new-value" },
            "localToolPermission": { "not": "a permission" }
        }))
        .expect("scoped update");

    assert_eq!(
        host["mcpCustomInstructions"],
        serde_json::json!({ "new": "new-value" })
    );
    assert_eq!(host["localToolPermission"], "ask");
    assert!(host.get("hasSeenOnboarding").is_none());
    let _ = fs::remove_file(path);
}


#[test]
fn frozen_notification_and_default_auto_review_persistence_are_normalized() {
    let path = temp_path("normalization");
    fs::write(
        &path,
        r#"{"version":1,"notifications":{"isEnabled":true,"extra":"drop"},"autoReviewInstructions":{"isEnabled":false,"allowInstructions":["x"],"blockInstructions":[]}}"#,
    )
    .expect("seed settings");
    let service = SettingsService::new(&path);

    let notifications = service.get_notification_config();
    assert_eq!(notifications["isEnabled"], false);
    let persisted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("settings text"))
            .expect("settings json");
    assert_eq!(
        persisted["notifications"],
        serde_json::json!({"isEnabled": false})
    );

    assert!(service
        .set_auto_review_instructions(&SandAutoReviewInstructions::default())
        .expect("clear default auto review override"));
    let persisted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("settings text"))
            .expect("settings json");
    assert!(persisted.get("autoReviewInstructions").is_none());
    let _ = fs::remove_file(path);
}

// ---------------------------------------------------------------------------
// OCRM-CONFIG-O01: Config Path Mutation & Prototype Pollution Defense
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_config_o01_path_mutation() {
    let mut config = serde_json::json!({
        "gateway": {
            "port": 8080
        }
    });

    // Valid path read and update
    assert_eq!(get_json_value_at_path(&config, "gateway.port"), Some(&serde_json::json!(8080)));
    assert!(set_json_value_at_path(&mut config, "gateway.auth.mode", serde_json::json!("token")).is_ok());
    assert_eq!(get_json_value_at_path(&config, "gateway.auth.mode"), Some(&serde_json::json!("token")));

    // Falsifiable: Prototype pollution prevention
    assert!(set_json_value_at_path(&mut config, "__proto__.polluted", serde_json::json!(true)).is_err());
    assert!(set_json_value_at_path(&mut config, "gateway.prototype.danger", serde_json::json!(true)).is_err());
    assert!(set_json_value_at_path(&mut config, "constructor.name", serde_json::json!("bad")).is_err());
    assert_eq!(get_json_value_at_path(&config, "__proto__.polluted"), None);
}

// ---------------------------------------------------------------------------
// OCRM-CONFIG-O02: Immutable Config Mode and Read-Only Guard
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_config_o02_immutable_mode_guard() {
    let path = temp_path("immutable-guard");
    fs::write(&path, r#"{"version":1,"readOnly":true}"#).expect("seed");
    let service = SettingsService::new(&path);

    // Reading settings works normally
    let host = service.get_host_settings();
    assert_eq!(host.get("version"), Some(&serde_json::json!(1)));
    assert_eq!(host.get("readOnly"), Some(&serde_json::json!(true)));
    assert!(service.is_read_only());

    // Mutations must fail closed and leave disk untouched
    assert!(service.set_has_seen_onboarding(true).is_err());
    assert!(service.set_host_settings(&serde_json::json!({"userTimeZone": "UTC"})).is_err());

    let on_disk = fs::read_to_string(&path).expect("read");
    assert!(!on_disk.contains("hasSeenOnboarding"), "Disk must remain untouched in readOnly mode");

    let _ = fs::remove_file(path);
}

// ---------------------------------------------------------------------------
// OCRM-CONFIG-O03: Single Authoritative Writer Mutex
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_config_o03_single_writer_lock() {
    let path = temp_path("writer-lock");
    fs::write(&path, r#"{"version":1,"counter":0}"#).expect("seed");
    let service = Arc::new(SettingsService::new(&path));

    // Parallel mutation attempts through single service must serialize safely
    let mut handles = Vec::new();
    for i in 0..5 {
        let svc = Arc::clone(&service);
        handles.push(std::thread::spawn(move || {
            let _ = svc.set_host_settings(&serde_json::json!({
                "mcpCustomInstructionsAccountScope": format!("account-{}", i)
            }));
        }));
    }
    for h in handles {
        h.join().expect("thread join");
    }

    let final_settings = service.get_host_settings();
    assert!(final_settings.get("mcpCustomInstructionsAccountScope").is_some());
    let _ = fs::remove_file(path);
}

// ---------------------------------------------------------------------------
// OCRM-CONFIG-O04: Atomic Settings IO Snapshot and Staging Rename
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_config_o04_io_atomic_snapshot() {
    let path = temp_path("atomic-io");
    fs::write(&path, r#"{"version":1,"key":"original"}"#).expect("seed");
    let service = SettingsService::new(&path);

    assert!(service.set_has_seen_onboarding(true).is_ok());

    // Verify file content on disk is valid JSON
    let text = fs::read_to_string(&path).expect("read");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed.get("key"), Some(&serde_json::json!("original")));
    assert_eq!(parsed.get("hasSeenOnboarding"), Some(&serde_json::json!(true)));
    let _ = fs::remove_file(path);
}

// ---------------------------------------------------------------------------
// OCRM-CONFIG-O05: Schema Normalization & Unknown Field Preservation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_config_o05_schema_normalization() {
    let path = temp_path("schema-normalization");
    fs::write(
        &path,
        r#"{"version":1,"customUserField":"keep-me","userTimeZone":"UTC"}"#,
    )
    .expect("seed");
    let service = SettingsService::new(&path);

    // Update known field
    assert!(service.set_user_time_zone(Some("Asia/Tokyo")).is_ok());

    // Verify unknown field is preserved
    let on_disk = fs::read_to_string(&path).expect("read");
    assert!(on_disk.contains("customUserField"), "Unknown field must not be dropped");
    assert!(on_disk.contains("keep-me"));
    let _ = fs::remove_file(path);
}

// ---------------------------------------------------------------------------
// OCRM-CONFIG-O06: Agent Workspace Directory Bounds
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_config_o06_agent_workspace_dirs() {
    use mahayana_host_runtime::storage::agent_paths::{
        assert_valid_sand_agent_id, resolve_sand_agent_dir,
    };
    // Safe directory resolution
    assert!(assert_valid_sand_agent_id("agent-safe-01").is_ok());
    assert!(assert_valid_sand_agent_id("../escape").is_err());
    assert!(assert_valid_sand_agent_id("agent/sub").is_err());
}

#[test]
fn test_ocrm_roster_include_write_guard_is_server_owned_and_fail_closed() {
    let included_path = temp_path("roster-include-owned");
    fs::write(
        &included_path,
        r#"{"version":1,"agents":{"entries":{"$include":"agents.json"}}}"#,
    )
    .expect("seed include-owned settings");
    let included = SettingsService::new(&included_path);

    // Ordinary settings writes must not bypass the guard merely because no
    // doctorFix marker was supplied by the caller.
    assert!(included
        .set_host_settings(&serde_json::json!({
            "agents": {"entries": {"local": {}}}
        }))
        .is_err());
    let unchanged: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&included_path).expect("read"))
            .expect("valid json");
    assert_eq!(
        unchanged["agents"]["entries"]["$include"],
        serde_json::json!("agents.json")
    );
    let _ = fs::remove_file(included_path);

    let target_include_path = temp_path("roster-include-target");
    let target_include = SettingsService::new(&target_include_path);
    // A client cannot smuggle an include-owned roster into a clean config.
    assert!(target_include
        .set_host_settings(&serde_json::json!({
            "agents": {"entries": {"$include": "agents.json"}}
        }))
        .is_err());
    let _ = fs::remove_file(target_include_path);
}

