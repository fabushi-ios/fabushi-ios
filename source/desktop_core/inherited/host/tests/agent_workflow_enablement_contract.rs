use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_workflow_enablement::{
    AgentWorkflowEnablement, get_agent_workflow_enablement_path,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-workflow-enablement-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn workflow_enablement_preserves_frozen_disabled_and_legacy_enabled_contract() {
    let root = temp_root("roundtrip");
    fs::create_dir_all(&root).expect("root");
    let path = get_agent_workflow_enablement_path(&root);
    fs::write(
        &path,
        r#"{"disabled":["b","a"],"enabled":["legacy"]}"#,
    )
    .expect("seed");
    let store = AgentWorkflowEnablement::new(&root);

    assert!(!store.is_enabled("a"));
    assert!(store.is_enabled("other"));
    assert!(store.has_explicit_entries());
    assert!(store.set_enabled("c", false).expect("disable c"));
    assert!(!store.is_enabled("c"));
    assert!(!store.set_enabled("c", false).expect("idempotent"));
    assert!(store.set_enabled("a", true).expect("enable a"));
    assert!(store.forget("b").expect("forget b"));
    assert!(store.enable_all(&["c".into(), "missing".into()]).expect("enable all"));
    assert!(store.is_enabled("a"));
    assert!(store.is_enabled("b"));
    assert!(store.is_enabled("c"));

    let parsed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("file")).expect("json");
    assert_eq!(parsed["disabled"], serde_json::json!([]));
    assert_eq!(parsed["enabled"], serde_json::json!(["legacy"]));
    assert!(fs::read_to_string(&path).expect("text").ends_with('\n'));

    let _ = fs::remove_dir_all(root);
}
