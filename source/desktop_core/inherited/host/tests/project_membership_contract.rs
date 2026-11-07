use std::collections::BTreeSet;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::memory::project_membership::{
    AgentProjectMembership, MEMBERSHIP_FILENAME, get_agent_projects_path, to_safe_slug_set,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use serde_json::json;

fn root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-project-membership-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn membership_filters_unsafe_slugs_and_round_trips_sorted_state() {
    let root = root("roundtrip");
    let agent_dir = root.join("agent");
    let membership = AgentProjectMembership::new(&agent_dir);

    assert_eq!(
        get_agent_projects_path(&agent_dir),
        agent_dir.join(MEMBERSHIP_FILENAME)
    );
    assert_eq!(
        to_safe_slug_set(Some(&json!(["zeta", "../escape", "alpha", "alpha"]))),
        BTreeSet::from(["alpha".to_string(), "zeta".to_string()])
    );

    assert!(membership.join("zeta").unwrap());
    assert!(membership.join("alpha").unwrap());
    assert!(!membership.join("../escape").unwrap());
    assert_eq!(
        membership.read(),
        BTreeSet::from(["alpha".to_string(), "zeta".to_string()])
    );
    let raw = fs::read_to_string(membership.path()).unwrap();
    assert!(raw.find("alpha").unwrap() < raw.find("zeta").unwrap());

    assert!(membership.leave("missing").unwrap());
    assert!(membership.leave("alpha").unwrap());
    assert_eq!(membership.read(), BTreeSet::from(["zeta".to_string()]));

    assert!(membership.prune_missing(|slug| slug != "zeta").unwrap());
    assert!(membership.read().is_empty());

    fs::write(membership.path(), "{not-json").unwrap();
    assert!(membership.read().is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_session_materialization_owns_the_agent_project_membership() {
    let root = root("shipping");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let session = workers
        .materialize_session_with_active(None, "user", None, None)
        .expect("materialize shipping session");
    assert_eq!(
        session.project_membership.path(),
        root.join(&session.record.id).join(MEMBERSHIP_FILENAME)
    );
    assert!(session.project_membership.join("release-project").unwrap());
    assert_eq!(
        session.project_membership.read(),
        BTreeSet::from(["release-project".to_string()])
    );
    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}
