use std::fs;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use mahayana_host_runtime::extensions::source_map::extension::{
    SOURCE_MAP_DEPENDENCIES, source_map_extension_id,
};
use mahayana_host_runtime::extensions::source_map::source_map_service::{
    BOX_STORE_SOURCE_KEY, SandSourceMap, SandSourceMode,
};
use uuid::Uuid;

fn temp_source_map_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("fabushi-source-map-{name}-{}", Uuid::new_v4()))
        .join("source-map.json")
}

#[test]
fn source_map_extension_preserves_grok_owner_and_dependency_boundary() {
    assert_eq!(source_map_extension_id(), HostExtensionId::SourceMap);
    assert!(SOURCE_MAP_DEPENDENCIES.is_empty());
}

#[test]
fn source_map_creates_stable_ids_persists_modes_and_box_store_identity() {
    let path = temp_source_map_path("persist");
    let counter = Arc::new(AtomicUsize::new(0));
    let ids = Arc::clone(&counter);
    let map = SandSourceMap::with_create_id(path.clone(), move || {
        format!("source-{}", ids.fetch_add(1, Ordering::SeqCst) + 1)
    });

    let first = map.get_or_create("agent-a").expect("create agent source");
    assert_eq!(first.source_id, "source-1");
    assert_eq!(first.mode, SandSourceMode::Local);
    assert_eq!(
        map.get_or_create("agent-a").expect("reuse agent source"),
        first
    );

    let promoted = map
        .set_mode("agent-a", SandSourceMode::AgentStore)
        .expect("promote source mode");
    assert_eq!(promoted.source_id, first.source_id);
    assert_eq!(promoted.mode, SandSourceMode::AgentStore);

    let box_store = map
        .get_or_create_box_store()
        .expect("create box-store source");
    assert_eq!(box_store.source_id, "source-2");
    assert_eq!(map.get_box_store(), Some(box_store.clone()));

    let persisted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("read persisted map"))
            .expect("parse persisted map");
    assert_eq!(
        persisted["agent-a"]["sourceId"].as_str(),
        Some(first.source_id.as_str())
    );
    assert_eq!(persisted["agent-a"]["mode"].as_str(), Some("agent-store"));
    assert_eq!(
        persisted[BOX_STORE_SOURCE_KEY]["sourceId"].as_str(),
        Some(box_store.source_id.as_str())
    );

    let reopened = SandSourceMap::with_create_id(path.clone(), || "unexpected".to_string());
    assert_eq!(
        reopened
            .get_or_create("agent-a")
            .expect("load persisted agent source"),
        promoted
    );
    assert_eq!(reopened.get_box_store(), Some(box_store));

    let _ = fs::remove_dir_all(path.parent().expect("source-map parent"));
}

#[test]
fn source_map_tolerates_corrupt_or_invalid_entries_like_the_frozen_service() {
    let path = temp_source_map_path("invalid");
    fs::create_dir_all(path.parent().expect("source-map parent")).expect("create source-map dir");
    fs::write(
        &path,
        r#"{
          "empty": {"sourceId":"","mode":"local"},
          "invalid-mode": {"sourceId":"legacy","mode":"unknown"},
          "not-an-entry": 7
        }"#,
    )
    .expect("write invalid source map");

    let map = SandSourceMap::with_create_id(path.clone(), || "fresh-source".to_string());
    assert!(map.get_box_store().is_none());
    let entry = map.get_or_create("agent-b").expect("recover source map");
    assert_eq!(entry.source_id, "fresh-source");
    assert_eq!(entry.mode, SandSourceMode::Local);

    let persisted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("read recovered map"))
            .expect("parse recovered map");
    assert!(persisted.get("empty").is_none());
    assert!(persisted.get("invalid-mode").is_none());
    assert!(persisted.get("not-an-entry").is_none());
    assert_eq!(persisted["agent-b"]["sourceId"], "fresh-source");

    let _ = fs::remove_dir_all(path.parent().expect("source-map parent"));
}
