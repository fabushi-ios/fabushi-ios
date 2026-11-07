use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::connector_secret_store::SandConnectorSecretStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-connector-secrets-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn connector_secret_store_matches_frozen_validation_merge_and_remove_contract() {
    let root = temp_root("store");
    let store = SandConnectorSecretStore::new(&root);

    assert!(!store
        .set_secret("../agent", "slack", "token", "bad")
        .expect("invalid agent"));
    assert!(!store
        .set_secret("agent-a", "../slack", "token", "bad")
        .expect("invalid platform"));
    assert!(!store
        .set_secret("agent-a", "slack", "", "bad")
        .expect("invalid field"));

    assert!(store
        .set_secret("agent-a", "slack", "token", "secret")
        .expect("set token"));
    assert!(store
        .set_secret("agent-a", "slack", "team", "team-a")
        .expect("merge team"));
    assert_eq!(
        store.get_secret("agent-a", "slack", "token").as_deref(),
        Some("secret")
    );
    assert_eq!(
        store.get_secret("agent-a", "slack", "team").as_deref(),
        Some("team-a")
    );
    assert!(fs::read_to_string(store.file_path("agent-a", "slack"))
        .expect("secret json")
        .ends_with('\n'));

    assert!(store
        .set_secret("agent-a", "slack", "empty", "")
        .expect("set empty"));
    assert_eq!(store.get_secret("agent-a", "slack", "empty"), None);

    fs::write(store.file_path("agent-a", "slack"), "not-json")
        .expect("corrupt secret file");
    assert!(store.read("agent-a", "slack").is_empty());

    assert!(store
        .set_secret("agent-a", "slack", "token", "recovered")
        .expect("recover write"));
    assert!(store
        .remove_agent_platform("agent-a", "slack")
        .expect("remove platform"));
    assert_eq!(store.get_secret("agent-a", "slack", "token"), None);
    assert!(!store
        .remove_agent_platform("agent-a", "slack")
        .expect("idempotent remove"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_session_exposes_connector_credential_ownership_at_canonical_root() {
    let agents_root = temp_root("production").join("agents");
    let workers = ProductionSessionWorkers::with_agents_root(&agents_root, 500);
    let created = workers
        .materialize_new_session(None, "user", None)
        .expect("materialize");

    assert!(workers
        .store_connector_credential(&created.id, "discord", "token", "abc")
        .expect("store credential"));
    assert_eq!(
        workers
            .get_connector_secret(&created.id, "discord", "token")
            .expect("get secret")
            .as_deref(),
        Some("abc")
    );
    assert!(workers
        .remove_connector_platform_secret(&created.id, "discord")
        .expect("remove secret"));
    assert_eq!(
        workers
            .get_connector_secret(&created.id, "discord", "token")
            .expect("get removed"),
        None
    );

    workers.shutdown();
    let _ = fs::remove_dir_all(
        agents_root
            .parent()
            .expect("agents parent"),
    );
}
