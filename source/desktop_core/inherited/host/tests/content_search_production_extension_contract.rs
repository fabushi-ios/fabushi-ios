use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::content_search::extension::{
    GLOBAL_SEARCH_GATE, project_transcript_mutation,
    start_production_content_search_extension_with_paths,
};
use mahayana_host_runtime::extensions::content_search::search_index_service::{
    SearchIndexHealth, TranscriptMutation,
};
use mahayana_host_runtime::extensions::experiments::{
    HostExperimentsExtension, HostExperimentsOptions,
};
use mahayana_host_runtime::transcript_mutation_events::publish_transcript_mutation;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-content-search-production-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn mutation(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().cloned().expect("object mutation")
}

#[test]
fn production_owner_binds_real_experiments_bus_service_and_health_sink() {
    let root = temp_root("owner");
    let agents = root.join("agents");
    fs::create_dir_all(&agents).unwrap();

    let experiments = Arc::new(HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: false,
        env_gate_overrides: None,
    }));
    experiments.replace_feature_flag_overrides(BTreeMap::from([
        (GLOBAL_SEARCH_GATE.to_string(), true),
    ]));
    let health = Arc::new(Mutex::new(Vec::<SearchIndexHealth>::new()));
    let health_sink = Arc::clone(&health);
    let extension = start_production_content_search_extension_with_paths(
        &root,
        &agents,
        Arc::clone(&experiments),
        Arc::new(move |event| health_sink.lock().unwrap().push(event)),
    );

    assert!(extension.is_enabled());
    assert_eq!(extension.max_matches_per_agent(), 5);
    assert_eq!(extension.max_results(), 50);
    assert!(extension.when_idle());
    assert!(extension.is_search_ready());

    publish_transcript_mutation(&mutation(serde_json::json!({
        "kind": "entries-upserted",
        "agentId": "agent-a",
        "entries": [{
            "id": "entry-1",
            "kind": "message",
            "role": "user",
            "content": "production bus needle",
            "timestampMs": 10
        }]
    })));
    assert!(extension.when_idle());
    let rows = extension.search_messages("needle", 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entry_id, "entry-1");

    experiments.replace_feature_flag_overrides(BTreeMap::from([
        (GLOBAL_SEARCH_GATE.to_string(), false),
    ]));
    assert!(!extension.is_enabled());
    publish_transcript_mutation(&mutation(serde_json::json!({
        "kind": "entries-upserted",
        "agentId": "agent-a",
        "entries": [{
            "id": "entry-2",
            "kind": "message",
            "role": "user",
            "content": "must stay unsubscribed"
        }]
    })));
    assert!(extension.when_idle());
    assert!(extension.search_messages("unsubscribed", 10).unwrap().is_empty());

    drop(extension);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_mutation_projection_is_typed_and_fails_closed() {
    let projected = project_transcript_mutation(&mutation(serde_json::json!({
        "kind": "entry-deleted",
        "agentId": "agent-a",
        "entryId": "entry-1"
    })));
    assert_eq!(
        projected,
        Some(TranscriptMutation::EntryDeleted {
            agent_id: "agent-a".into(),
            entry_id: "entry-1".into(),
        })
    );

    assert!(project_transcript_mutation(&mutation(serde_json::json!({
        "kind": "entries-upserted",
        "agentId": "agent-a",
        "entries": [{"kind": "message"}]
    }))).is_some(), "IndexEntry accepts an empty id like frozen TS type; writer later ignores only store-reindex malformed rows");

    assert!(project_transcript_mutation(&mutation(serde_json::json!({
        "kind": "unknown",
        "agentId": "agent-a"
    }))).is_none());
}
