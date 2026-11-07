use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mahayana_host_runtime::extensions::experiments::{
    HostExperimentsExtension, HostExperimentsOptions, create_cursor_checksum,
};
use mahayana_host_runtime::extensions::inference::sand_model_experiment::SandModelExperimentArm;

fn service() -> HostExperimentsExtension {
    HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: false,
        env_gate_overrides: None,
    })
}

#[test]
fn hydrated_statsig_surface_matches_frozen_gate_model_and_property_semantics() {
    let service = service();
    let property = service.get_feature_gate_property("sand_agent_network");
    assert!(!property.get());

    let notifications = Arc::new(AtomicUsize::new(0));
    let notifications_for_listener = Arc::clone(&notifications);
    let stop_property = property.subscribe(Arc::new(move |enabled| {
        assert!(enabled);
        notifications_for_listener.fetch_add(1, Ordering::SeqCst);
    }));

    let bootstrap = serde_json::json!({
        "user": { "userID": "user-42" },
        "feature_gates": {
            "sand_agent_network": { "value": true },
            "sand_browser_use_subagent": { "value": true },
            "sand_computer_use_playwright": { "value": true }
        },
        "dynamic_configs": {
            "sand_model_selection": {
                "value": { "enabled": true },
                "rule_id": "model-rule",
                "group_name": "Treatment",
                "secondary_exposures": []
            },
            "sand_default_model": {
                "value": {
                    "modelId": "claude-opus-4-8",
                    "maxMode": false,
                    "parameters": [{ "id": "effort", "value": "low" }]
                }
            },
            "sand_automations_model": {
                "value": {
                    "modelId": "claude-sonnet-4-5",
                    "maxMode": false,
                    "parameters": []
                }
            },
            "sand_computer_use_playwright_config": {
                "value": {
                    "modelId": "claude-opus-4-8",
                    "maxMode": false,
                    "parameters": [{ "id": "thinking", "value": "false" }]
                }
            },
            "sand_browser_use_model": {
                "value": {
                    "modelId": "claude-opus-4-8",
                    "maxMode": false,
                    "parameters": [{ "id": "effort", "value": "low" }]
                }
            }
        }
    })
    .to_string();

    assert!(service.hydrate_statsig_bootstrap(&bootstrap, true).unwrap());
    assert!(service.has_live_statsig_bootstrap());
    assert!(service.has_authenticated_statsig_bootstrap());
    assert!(service.has_hydrated_statsig_user_id());
    assert!(service.wait_for_hydrated_statsig_user_id(Duration::ZERO));
    assert!(service.check_gate("sand_agent_network", Some(Duration::ZERO)));
    assert!(property.get());
    assert_eq!(notifications.load(Ordering::SeqCst), 1);

    let state = service.get_sand_model_experiment_state().unwrap();
    assert!(state.active);
    assert_eq!(state.arm, SandModelExperimentArm::Treatment);

    let default_model = service.get_configured_default_model().unwrap();
    assert_eq!(default_model.model_id, "claude-opus-4-8");
    assert!(!default_model.max_mode);
    assert_eq!(default_model.parameters.len(), 1);

    let automation_model = service.get_configured_automations_model().unwrap();
    assert_eq!(automation_model.model_id, "claude-sonnet-4-5");

    assert_eq!(
        service
            .get_computer_use_model_override()
            .unwrap()
            .get("modelId")
            .and_then(serde_json::Value::as_str),
        Some("claude-opus-4-8")
    );
    assert_eq!(
        service
            .get_browser_use_model_override()
            .unwrap()
            .get("modelId")
            .and_then(serde_json::Value::as_str),
        Some("claude-opus-4-8")
    );

    stop_property();
    service.replace_feature_flag_overrides(BTreeMap::from([(
        "sand_agent_network".into(),
        false,
    )]));
    assert!(!service.is_agent_network_enabled());
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
}

#[test]
fn model_config_validation_and_dispose_fail_closed() {
    let service = service();
    let invalid = serde_json::json!({
        "user": { "userID": "user-42" },
        "feature_gates": {},
        "dynamic_configs": {
            "sand_default_model": {
                "value": {
                    "modelId": "default",
                    "maxMode": true,
                    "parameters": []
                }
            }
        }
    })
    .to_string();
    assert!(service.hydrate_statsig_bootstrap(&invalid, true).unwrap());
    assert!(service.get_configured_default_model().is_none());

    service.dispose();
    assert!(!service.has_authenticated_statsig_bootstrap());
    assert!(!service.has_live_statsig_bootstrap());
    assert!(!service.has_hydrated_statsig_user_id());
    assert!(!service.wait_for_hydrated_statsig_user_id(Duration::from_millis(1)));
}

#[test]
fn cursor_checksum_preserves_the_frozen_six_byte_timestamp_domain() {
    let high_timestamp = (1_u64 << 40).saturating_mul(1_000_000);
    assert_ne!(
        create_cursor_checksum("machine", 0),
        create_cursor_checksum("machine", high_timestamp)
    );
}

#[test]
fn shipping_owner_and_inference_consumer_are_wired_to_the_same_experiments_owner() {
    const EXTENSION: &str = include_str!("../src/extensions/experiments/extension.rs");
    const PRODUCTION: &str = include_str!("../src/host_production_extensions.rs");
    const INFERENCE: &str = include_str!("../src/extensions/inference/production.rs");

    assert_eq!(
        PRODUCTION.matches("start_authenticated_statsig_bootstrap(").count(),
        1
    );
    assert!(PRODUCTION.contains("get_sand_root_dir(),"));
    assert!(
        PRODUCTION.find("_statsig_bootstrap: ProductionStatsigBootstrapRuntime").unwrap()
            > PRODUCTION.find("automations: Mutex<Option<Arc<ProductionAutomationsLifecycle>>>").unwrap()
    );

    for needle in [
        "event.is_first_credential",
        "has_authenticated_statsig_bootstrap()",
        "auth_revision",
        "parse_retry_after_header",
        "self.experiments.dispose()",
    ] {
        assert!(EXTENSION.contains(needle), "missing lifecycle relation: {needle}");
    }

    for needle in [
        "get_configured_default_model()",
        "get_configured_automations_model()",
        "get_computer_use_model_override()",
        "get_browser_use_model_override()",
        "log_sand_model_experiment_exposure()",
    ] {
        assert!(INFERENCE.contains(needle), "missing shipping consumer: {needle}");
    }
}
