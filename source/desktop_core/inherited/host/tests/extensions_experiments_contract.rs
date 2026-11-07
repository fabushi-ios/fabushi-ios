use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mahayana_host_runtime::extensions::browser_ua::BrowserUaExperimentsApi;
use mahayana_host_runtime::extensions::experiments::{
    EXPERIMENTS_DEPENDENCIES, HostExperimentsExtension, HostExperimentsOptions,
    experiments_extension_id,
};
use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;

#[test]
fn experiments_extension_preserves_grok_gate_defaults_and_dev_overrides() {
    assert_eq!(experiments_extension_id(), HostExtensionId::Experiments);
    assert_eq!(
        EXPERIMENTS_DEPENDENCIES,
        &[HostExtensionId::Auth, HostExtensionId::Settings]
    );

    let default_service = HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: true,
        env_gate_overrides: None,
    });
    assert!(!default_service.is_agent_network_enabled());
    assert!(!default_service.is_ua_token_kill_switch_enabled());
    assert!(default_service.check_feature_gate("sand_multitask"));
    assert!(default_service.check_feature_gate("sand_spotlight"));
    assert!(default_service.check_feature_gate("enable_sparse_plugin_clones"));
    assert_eq!(
        default_service.get_dynamic_config("grok_bot_conversation_size_limits"),
        BTreeMap::from([
            ("hard_limit_mb".into(), serde_json::json!(1024)),
            ("soft_limit_mb".into(), serde_json::json!(256)),
        ])
    );

    let dev_override = HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: true,
        env_gate_overrides: Some(
            "sand_agent_network=1,sand_browser_ua_token_kill_switch=on".into(),
        ),
    });
    assert!(dev_override.is_agent_network_enabled());
    assert!(BrowserUaExperimentsApi::is_ua_token_kill_switch_enabled(
        &dev_override
    ));

    let packaged = HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: false,
        env_gate_overrides: Some("sand_agent_network=1".into()),
    });
    assert!(!packaged.is_agent_network_enabled());
}

#[test]
fn settings_overrides_win_and_experiment_subscribers_are_disposable() {
    let service = HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: true,
        env_gate_overrides: Some("sand_agent_network=1".into()),
    });
    let notifications = Arc::new(AtomicUsize::new(0));
    let sink = Arc::clone(&notifications);
    let stop = service.subscribe(Arc::new(move || {
        sink.fetch_add(1, Ordering::SeqCst);
    }));

    service.replace_feature_flag_overrides(BTreeMap::from([
        ("sand_agent_network".into(), false),
    ]));
    assert!(!service.is_agent_network_enabled());
    assert_eq!(notifications.load(Ordering::SeqCst), 1);

    stop();
    service.replace_feature_flag_overrides(BTreeMap::from([
        ("sand_agent_network".into(), true),
    ]));
    assert!(service.is_agent_network_enabled());
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
}

#[test]
fn dynamic_config_overrides_merge_over_frozen_fallback_and_notify_subscribers() {
    let service = HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: false,
        env_gate_overrides: None,
    });
    let notifications = Arc::new(AtomicUsize::new(0));
    let sink = Arc::clone(&notifications);
    let stop = service.subscribe(Arc::new(move || {
        sink.fetch_add(1, Ordering::SeqCst);
    }));

    service.replace_dynamic_config_overrides(BTreeMap::from([(
        "grok_bot_conversation_size_limits".into(),
        BTreeMap::from([("soft_limit_mb".into(), serde_json::json!(64))]),
    )]));

    assert_eq!(
        service.get_dynamic_config("grok_bot_conversation_size_limits"),
        BTreeMap::from([
            ("hard_limit_mb".into(), serde_json::json!(1024)),
            ("soft_limit_mb".into(), serde_json::json!(64)),
        ])
    );
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    stop();
}


#[test]
fn authenticated_statsig_bootstrap_pins_memory_gate_once() {
    use std::sync::Mutex;

    let service = HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: false,
        env_gate_overrides: None,
    });
    let pins = Arc::new(Mutex::new(Vec::new()));
    let pins_for_callback = Arc::clone(&pins);
    service.pin_gate_on_authenticated_bootstrap(
        "sand_memory_dreaming",
        Arc::new(move |enabled| {
            pins_for_callback.lock().unwrap().push(enabled);
        }),
    );

    let treatment = serde_json::json!({
        "user": { "userID": "user-1" },
        "feature_gates": {
            "sand_memory_dreaming": { "value": true }
        }
    })
    .to_string();
    assert!(!service
        .hydrate_statsig_bootstrap(&treatment, false)
        .unwrap());
    assert!(service.check_feature_gate("sand_memory_dreaming"));
    assert!(pins.lock().unwrap().is_empty());
    assert!(!service.has_authenticated_statsig_bootstrap());

    assert!(service
        .hydrate_statsig_bootstrap(&treatment, true)
        .unwrap());
    assert_eq!(*pins.lock().unwrap(), vec![true]);
    assert!(service.has_authenticated_statsig_bootstrap());

    let control = serde_json::json!({
        "user": { "userID": "user-1" },
        "feature_gates": {
            "sand_memory_dreaming": { "value": false }
        }
    })
    .to_string();
    assert!(service
        .hydrate_statsig_bootstrap(&control, true)
        .unwrap());
    assert_eq!(*pins.lock().unwrap(), vec![true]);
    assert!(!service.check_feature_gate("sand_memory_dreaming"));
}

#[test]
fn unauthenticated_statsig_payload_never_resolves_authenticated_pin() {
    use std::sync::Mutex;

    let service = HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: false,
        env_gate_overrides: None,
    });
    let pins = Arc::new(Mutex::new(Vec::new()));
    let pins_for_callback = Arc::clone(&pins);
    service.pin_gate_on_authenticated_bootstrap(
        "sand_memory_dreaming",
        Arc::new(move |enabled| pins_for_callback.lock().unwrap().push(enabled)),
    );
    let anonymous = serde_json::json!({
        "user": {},
        "feature_gates": {
            "sand_memory_dreaming": { "value": true }
        }
    })
    .to_string();
    assert!(!service
        .hydrate_statsig_bootstrap(&anonymous, true)
        .unwrap());
    assert!(pins.lock().unwrap().is_empty());
}
