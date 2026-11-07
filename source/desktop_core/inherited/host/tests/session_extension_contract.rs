use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::experiments::{
    HostExperimentsExtension, HostExperimentsOptions,
};
use mahayana_host_runtime::extensions::session::box_handoff_service::{
    BoxHandoffDeps, HandoffRequest, HandoffStartResult, HandoffTelemetry, HandoffTrigger,
};
use mahayana_host_runtime::extensions::session::conversation_size_limits::{
    ConversationSizeLimits, current_conversation_size_limits, pin_conversation_size_limits_reader,
};
use mahayana_host_runtime::extensions::session::extension::{
    conversation_size_limits_from_experiments, start_session_extension,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::session::session_maintenance::{
    is_legacy_store_blob_retirement_enabled,
};

fn temp_root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-session-extension-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn session_extension_owns_store_handoff_and_experiment_pin_lifecycle() {
    let root = temp_root();
    let experiments = Arc::new(HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: true,
        env_gate_overrides: Some(
            "sand_legacy_store_blob_retirement=true,sand_stale_root_gc=true,grok_bot_conversation_gc=true"
                .into(),
        ),
    }));
    experiments.replace_dynamic_config_overrides(BTreeMap::from([(
        "grok_bot_conversation_size_limits".into(),
        BTreeMap::from([
            ("soft_limit_mb".into(), serde_json::json!(64)),
            ("hard_limit_mb".into(), serde_json::json!(128)),
        ]),
    )]));
    assert_eq!(
        conversation_size_limits_from_experiments(&experiments),
        ConversationSizeLimits {
            soft_limit_mb: Some(64.0),
            hard_limit_mb: Some(128.0),
        }
    );
    let legacy_members = root.join("agent-legacy").join("members");
    fs::create_dir_all(&legacy_members).expect("legacy member dir");
    fs::write(legacy_members.join("stale.json"), b"stale").expect("legacy member file");

    let store = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let extension = start_session_extension(
        Arc::clone(&experiments),
        Arc::clone(&store),
        BoxHandoffDeps::default(),
    );

    assert_eq!(extension.store().agents_root(), root.as_path());
    assert_eq!(
        current_conversation_size_limits(),
        ConversationSizeLimits {
            soft_limit_mb: Some(64.0),
            hard_limit_mb: Some(128.0),
        }
    );
    assert!(
        !legacy_members.exists(),
        "Session startup maintenance must remove the frozen legacy members directory"
    );
    assert!(is_legacy_store_blob_retirement_enabled());

    let started = extension.start_handoff(HandoffRequest {
        agent_id: "agent-a".into(),
        instruction: "Take control".into(),
        telemetry: HandoffTelemetry::default(),
    });
    assert!(matches!(started, HandoffStartResult::Started { .. }));
    assert!(extension.pending_handoff("agent-a").is_some());
    assert!(extension
        .end_handoff("agent-a", HandoffTrigger::Name("done".into()))
        .expect("end"));
    assert!(extension.pending_handoff("agent-a").is_none());

    experiments.replace_feature_flag_overrides(BTreeMap::from([(
        "sand_legacy_store_blob_retirement".into(),
        false,
    )]));
    assert!(!is_legacy_store_blob_retirement_enabled());

    extension.shutdown();
    drop(extension);
    pin_conversation_size_limits_reader(None);
    let _ = fs::remove_dir_all(root);
}
