use std::fs;

use mahayana_host_runtime::extensions::memory::memory_service::{
    FileMemoryStore, MemoryKind, MemoryOrigin, SynthesisApplyResult, SynthesisChange,
};

fn temp_store() -> (std::path::PathBuf, FileMemoryStore) {
    let root = std::env::temp_dir().join(format!(
        "fabushi-memory-synthesis-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root).unwrap();
    let store = FileMemoryStore::with_synthesis_metadata(root.join("memory"), true);
    (root, store)
}

#[test]
fn explicit_memories_are_protected_from_synthesis_updates_and_removals() {
    let (root, store) = temp_store();
    let explicit = store
        .add_memory(
            "Prefers concise replies",
            1_700_000_000_000,
            MemoryKind::Profile,
        )
        .unwrap()
        .unwrap();
    let snapshot = store.prepare_synthesis();
    assert_eq!(snapshot.memories.len(), 1);
    assert_eq!(snapshot.memories[0].origin, MemoryOrigin::Explicit);

    let update = store
        .apply_synthesis(
            &snapshot,
            &[SynthesisChange::Update {
                id: explicit.id.clone(),
                content: "Prefers long replies".into(),
                kind: MemoryKind::Profile,
            }],
            1_700_000_100_000,
        )
        .unwrap();
    assert_eq!(update, SynthesisApplyResult::Invalid);

    let remove = store
        .apply_synthesis(
            &snapshot,
            &[SynthesisChange::Remove { id: explicit.id }],
            1_700_000_100_000,
        )
        .unwrap();
    assert_eq!(remove, SynthesisApplyResult::Invalid);
    assert_eq!(store.count_memories(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn synthesis_create_update_remove_round_trip_marks_origin_and_temporal_review() {
    let (root, store) = temp_store();
    let empty = store.prepare_synthesis();
    assert_eq!(
        store
            .apply_synthesis(
                &empty,
                &[SynthesisChange::Create {
                    content: "Working on project Apollo".into(),
                    kind: MemoryKind::Log,
                }],
                1_700_000_000_000,
            )
            .unwrap(),
        SynthesisApplyResult::Committed
    );
    let snapshot = store.prepare_synthesis();
    assert_eq!(snapshot.memories.len(), 1);
    assert_eq!(snapshot.memories[0].origin, MemoryOrigin::Synthesis);
    assert!(!store.is_temporal_review_due(1_700_000_000_001));

    let id = snapshot.memories[0].id.clone();
    assert_eq!(
        store
            .apply_synthesis(
                &snapshot,
                &[SynthesisChange::Update {
                    id,
                    content: "Project Apollo design is in review".into(),
                    kind: MemoryKind::Log,
                }],
                1_700_000_200_000,
            )
            .unwrap(),
        SynthesisApplyResult::Committed
    );
    let updated = store.prepare_synthesis();
    assert_eq!(
        updated.memories[0].content,
        "Project Apollo design is in review"
    );

    let id = updated.memories[0].id.clone();
    assert_eq!(
        store
            .apply_synthesis(
                &updated,
                &[SynthesisChange::Remove { id }],
                1_700_000_300_000,
            )
            .unwrap(),
        SynthesisApplyResult::Committed
    );
    assert_eq!(store.count_memories(), 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn stale_snapshot_cannot_commit_and_user_deletion_tombstones_recreation() {
    let (root, store) = temp_store();
    let snapshot = store.prepare_synthesis();
    store
        .add_memory("User moved to Paris", 1_700_000_000_000, MemoryKind::Log)
        .unwrap();
    assert_eq!(
        store
            .apply_synthesis(
                &snapshot,
                &[SynthesisChange::Create {
                    content: "Should not commit".into(),
                    kind: MemoryKind::Log,
                }],
                1_700_000_100_000,
            )
            .unwrap(),
        SynthesisApplyResult::Stale
    );

    let current = store.list_memories(10)[0].clone();
    assert!(store.remove_memory(&current.id).unwrap());
    let after_delete = store.prepare_synthesis();
    assert_eq!(
        store
            .apply_synthesis(
                &after_delete,
                &[SynthesisChange::Create {
                    content: "User moved to Paris".into(),
                    kind: MemoryKind::Log,
                }],
                1_700_000_200_000,
            )
            .unwrap(),
        SynthesisApplyResult::Committed
    );
    assert_eq!(store.count_memories(), 0);
    let _ = fs::remove_dir_all(root);
}
