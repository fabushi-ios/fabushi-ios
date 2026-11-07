use std::collections::HashSet;
use std::sync::Arc;
use serde_json::json;

use mahayana_host_runtime::agents::agent_roster::*;
use mahayana_host_runtime::host_roster_bookkeeping::*;

struct TestAttachmentRosterPort(std::sync::Mutex<Option<String>>);
impl AttachmentRosterPort for TestAttachmentRosterPort {
    fn set_fallback_agent_id(&self, agent_id: Option<&str>) {
        *self.0.lock().unwrap() = agent_id.map(str::to_string);
    }
}

struct TestTranscriptRosterPort;
impl TranscriptRosterPort for TestTranscriptRosterPort {
    fn live_running_agent_ids(&self) -> Vec<String> {
        vec!["agent-alpha".to_string()]
    }
}

struct TestForeverBoxRosterPort;
impl ForeverBoxRosterPort for TestForeverBoxRosterPort {
    fn enroll_disk_pressure_reminder(&self, _agent_ids: &HashSet<String>) {}
    fn set_busy(&self, _is_busy: bool) {}
}

struct TestSnapshotBackstopPort;
impl SnapshotBackstopPort for TestSnapshotBackstopPort {
    fn is_enabled(&self) -> bool { false }
    fn schedule_snapshot(&self, _agent_id: &str) {}
}

struct TestBoxStoreSnapshotPort;
impl BoxStoreSnapshotPort for TestBoxStoreSnapshotPort {
    fn is_enabled(&self) -> bool { false }
    fn schedule_store_db_snapshot(&self, _agent_id: &str) {}
}

struct TestSourceMapRosterPort;
impl SourceMapRosterPort for TestSourceMapRosterPort {
    fn get_or_create(&self, _agent_id: &str) {}
}

#[test]
fn test_ocrm_roster_o01_entries_and_list_representations() {
    // 1. Empty config without roster returns implicit "main" agent
    let empty_cfg = json!({});
    assert!(!has_agent_roster_property(&empty_cfg));
    assert_eq!(list_agent_ids(&empty_cfg), vec!["main"]);

    // 2. Entries representation
    let entries_cfg = json!({
        "agents": {
            "entries": {
                "Alpha": { "model": "gemini" },
                "Beta": { "model": "claude" }
            }
        }
    });
    assert!(has_agent_roster_property(&entries_cfg));
    let ids = list_agent_ids(&entries_cfg);
    assert_eq!(ids, vec!["alpha", "beta"]);

    // 3. List representation
    let list_cfg = json!({
        "agents": {
            "list": [
                { "id": "Gamma", "workspace": "/gamma" },
                { "id": "Delta", "workspace": "/delta" }
            ]
        }
    });
    assert!(has_agent_roster_property(&list_cfg));
    let ids = list_agent_ids(&list_cfg);
    assert_eq!(ids, vec!["gamma", "delta"]);
}

#[test]
fn test_ocrm_roster_o02_default_and_sole_resolution() {
    // 1. Single agent is sole default
    let single_cfg = json!({
        "agents": {
            "entries": {
                "Omega": { "workspace": "/omega" }
            }
        }
    });
    assert_eq!(try_resolve_sole_agent_id(&single_cfg), Some("omega".to_string()));
    assert_eq!(try_resolve_default_agent_id(&single_cfg), Some("omega".to_string()));

    // 2. Multi-agent without marked default has no default
    let multi_unmarked = json!({
        "agents": {
            "entries": {
                "Agent1": {},
                "Agent2": {}
            }
        }
    });
    assert_eq!(try_resolve_sole_agent_id(&multi_unmarked), None);
    assert_eq!(try_resolve_raw_legacy_default_agent_id(&multi_unmarked), None);

    // 3. Multi-agent with single marked default
    let multi_marked = json!({
        "agents": {
            "entries": {
                "Agent1": { "default": false },
                "Agent2": { "default": true }
            }
        }
    });
    assert_eq!(try_resolve_raw_legacy_default_agent_id(&multi_marked), Some("agent2".to_string()));
    assert_eq!(try_resolve_default_agent_id(&multi_marked), Some("agent2".to_string()));

    // 4. Explicit ownership denies legacy default resolution
    let explicit_cfg = json!({
        "agents": {
            "ownership": "explicit",
            "entries": {
                "Agent1": { "default": true }
            }
        }
    });
    assert_eq!(try_resolve_raw_legacy_default_agent_id(&explicit_cfg), None);
}

#[test]
fn test_ocrm_roster_o03_acp_and_gateway_subagent_targets() {
    let cfg = json!({
        "agents": {
            "entries": {
                "worker1": {
                    "runtime": {
                        "type": "acp",
                        "acp": { "agent": "sub-bot-1" }
                    }
                }
            }
        },
        "acp": {
            "defaultAgent": "default-sub-bot",
            "allowedAgents": ["extra-bot", "*"]
        }
    });

    let targets = resolve_configured_acp_subagent_target_ids(&cfg);
    assert!(targets.contains("worker1"));
    assert!(targets.contains("sub-bot-1"));
    assert!(targets.contains("default-sub-bot"));
    assert!(targets.contains("extra-bot"));
    assert!(!targets.contains("*"));

    let gw_ids = list_gateway_owned_agent_ids(&cfg);
    assert!(gw_ids.contains(&"worker1".to_string()));
    assert!(gw_ids.contains(&"system".to_string()));
    assert!(gw_ids.contains(&"gateway".to_string()));
}

#[test]
fn test_ocrm_roster_o04_include_provenance() {
    let empty_path: [&str; 0] = [];
    assert!(include_contribution_owns_agent_roster(&empty_path, &json!({ "agents": { "entries": {} } })));
    assert!(!include_contribution_owns_agent_roster(&empty_path, &json!({ "unrelated": 123 })));

    let agents_path = ["agents"];
    assert!(include_contribution_owns_agent_roster(&agents_path, &json!({ "entries": {} })));
    assert!(include_contribution_owns_agent_roster(&agents_path, &json!({ "list": [] })));

    let entry_def = ["agents", "entries", "worker", "default"];
    assert!(include_contribution_owns_agent_roster(&entry_def, &json!(true)));

    let bindings_path = ["bindings"];
    assert!(include_contribution_owns_bindings(&bindings_path, &json!({})));

    let parsed_with_include = json!({
        "agents": {
            "entries": {
                "$include": "./external-agents.json"
            }
        }
    });
    let resolved = json!({
        "agents": {
            "entries": {
                "ext": {}
            }
        }
    });
    assert!(config_include_owns_agent_roster_values(&parsed_with_include, &resolved, false));

    // Root/external include ownership is only decidable when the trusted
    // resolver supplies the provenance fact; a client omission must not turn
    // that fact into a safe write.
    let parsed_root_include = json!({ "$include": "./root.json" });
    assert!(config_include_owns_agent_roster_values(
        &parsed_root_include,
        &resolved,
        true
    ));
    assert!(!config_include_owns_agent_roster_values(
        &json!({ "unrelated": { "$include": "./other.json" } }),
        &resolved,
        false
    ));
}

#[test]
fn test_ocrm_roster_o05_implicit_defaults_materialization() {
    let unauthored = json!({ "general": { "verbose": true } });
    let materialized = apply_implicit_agent_roster_defaults(&unauthored);
    assert_eq!(
        materialized["agents"]["entries"]["main"],
        json!({})
    );

    let explicit = json!({
        "agents": {
            "ownership": "explicit",
            "entries": {}
        }
    });
    let kept_explicit = apply_implicit_agent_roster_defaults(&explicit);
    assert_eq!(kept_explicit, explicit);
}

#[test]
fn test_ocrm_roster_o06_security_audit_invalid_default_count() {
    // 1. Multiple default markers across configured agents produces warning finding
    let conflicting_defaults = json!({
        "agents": {
            "entries": {
                "agent-a": { "default": true },
                "agent-b": { "default": true }
            }
        }
    });
    let findings = collect_agent_roster_findings(&conflicting_defaults);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].check_id, "config.agent_roster.invalid_default_count");
    assert_eq!(findings[0].severity, "warn");

    // 2. Single marked default produces no finding
    let valid_default = json!({
        "agents": {
            "entries": {
                "agent-a": { "default": true },
                "agent-b": { "default": false }
            }
        }
    });
    assert!(collect_agent_roster_findings(&valid_default).is_empty());
}

#[test]
fn test_ocrm_roster_o07_survivor_workspace_pinning() {
    let source_cfg = json!({
        "agents": {
            "entries": {
                "survivor": {},
                "victim": {}
            }
        }
    });
    let target_cfg = json!({
        "agents": {
            "entries": {
                "survivor": {}
            }
        }
    });

    let (pinned, inserted) = pin_survivor_workspace_for_roster_collapse(
        &source_cfg,
        &target_cfg,
        Some("/var/fabushi/workspaces/survivor"),
    );

    assert_eq!(inserted.len(), 1);
    assert_eq!(
        inserted[0],
        vec!["agents", "entries", "survivor", "workspace"]
    );
    assert_eq!(
        pinned["agents"]["entries"]["survivor"]["workspace"],
        "/var/fabushi/workspaces/survivor"
    );
}

#[test]
fn test_ocrm_roster_o08_doctor_include_write_staging() {
    let parsed_clean = json!({ "agents": { "entries": { "local": {} } } });
    let resolved = json!({ "agents": { "entries": { "local": {} } } });
    assert!(can_write_doctor_include(&parsed_clean, &resolved, false));

    let parsed_included = json!({ "agents": { "entries": { "$include": "agents.json" } } });
    assert!(!can_write_doctor_include(&parsed_included, &resolved, false));
}

#[test]
fn test_ocrm_roster_o09_bookkeeping_apply_config_wiring() {
    let port = Arc::new(TestAttachmentRosterPort(std::sync::Mutex::new(None)));
    let ports = HostRosterPorts {
        attachments: Arc::clone(&port) as Arc<dyn AttachmentRosterPort>,
        transcript: Arc::new(TestTranscriptRosterPort),
        forever_box: Arc::new(TestForeverBoxRosterPort),
        state_backstop: Arc::new(TestSnapshotBackstopPort),
        box_store_sync: Arc::new(TestBoxStoreSnapshotPort),
        source_map: Arc::new(TestSourceMapRosterPort),
    };

    let mut bookkeeping = HostRosterBookkeeping::new(ports);
    let cfg = json!({
        "agents": {
            "entries": {
                "active-agent": {}
            }
        }
    });
    bookkeeping.apply_config(&cfg);
    assert_eq!(bookkeeping.latest_active_agent_id(), Some("active-agent"));
    assert_eq!(*port.0.lock().unwrap(), Some("active-agent".to_string()));
}
