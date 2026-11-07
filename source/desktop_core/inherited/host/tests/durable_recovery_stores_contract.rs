use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::transcript::production_runtime::ProductionTranscriptRuntime;
use mahayana_host_runtime::extensions::transcript::sand_pending_wake_store::{
    AutomationQuietOrigin, DurablePendingWakeMarker, PendingWakeKind, QuietWakeOrigin,
    SandPendingWakeStore, parse_pending_wake_file,
};
use mahayana_host_runtime::extensions::transcript::sand_upgrade_resume_store::{
    upgrade_resume_marker_identity_is_well_formed, SandUpgradeResumeStore, UpgradeResumeMarker,
    parse_upgrade_resume_file,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-durable-recovery-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn pending(agent_id: &str, kind: PendingWakeKind, work_id: &str, at: f64) -> DurablePendingWakeMarker {
    DurablePendingWakeMarker {
        agent_id: agent_id.into(),
        kind,
        work_id: work_id.into(),
        marked_at_ms: at,
        quiet_origin: None,
        title: None,
        subagent_type: None,
        interrupted_by_recreate: false,
        delivery_operation_id: None,
        address_token: None,
        failure_reason: None,
        session_key: None,
    }
}

#[test]
fn pending_wake_parser_preserves_frozen_coercion_and_filters_invalid_rows() {
    let rows = parse_pending_wake_file(Some(r#"{"pending":[
        {"agentId":"a","kind":"cloud-agent","workId":"w","markedAtMs":12,
         "quietOrigin":{"automation":{"id":"auto","name":"Daily"}},"title":"Cloud"},
        {"agentId":"b","kind":"shell","workId":"s","markedAtMs":"bad",
         "quietOrigin":{"automation":{"id":"","name":"ignored"}},"interruptedByRecreate":true},
        {"agentId":"","kind":"shell","workId":"bad"},
        {"agentId":"c","kind":"unknown","workId":"bad"}
    ]}"#));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].marked_at_ms, 12.0);
    assert_eq!(
        rows[0].quiet_origin,
        Some(QuietWakeOrigin {
            automation: Some(AutomationQuietOrigin {
                id: "auto".into(),
                name: "Daily".into(),
            }),
        })
    );
    assert_eq!(rows[1].marked_at_ms, 0.0);
    assert_eq!(
        rows[1].quiet_origin,
        Some(QuietWakeOrigin { automation: None })
    );
    assert!(rows[1].interrupted_by_recreate);
    assert!(parse_pending_wake_file(Some("not-json")).is_empty());
}

#[test]
fn channel_delivery_failure_marker_round_trips_operation_identity() {
    let root = temp_root("channel-delivery");
    fs::create_dir_all(&root).expect("root");
    let store = SandPendingWakeStore::new(&root);
    let mut marker = pending("agent-channel", PendingWakeKind::ChannelDelivery, "op-1", 10.0);
    marker.delivery_operation_id = Some("op-1".into());
    marker.address_token = Some("telegram:123".into());
    marker.failure_reason = Some("provider unavailable".into());
    assert!(store.mark_pending(marker));
    let restored = store.list_pending().pop().expect("channel marker");
    assert_eq!(restored.kind, PendingWakeKind::ChannelDelivery);
    assert_eq!(restored.delivery_operation_id.as_deref(), Some("op-1"));
    assert_eq!(restored.address_token.as_deref(), Some("telegram:123"));
    assert_eq!(restored.failure_reason.as_deref(), Some("provider unavailable"));
}

#[test]
fn pending_wake_store_upserts_clears_and_prunes_atomically() {
    let root = temp_root("pending");
    let store = SandPendingWakeStore::new(&root);
    assert!(store.mark_pending(pending("a", PendingWakeKind::Shell, "w", 10.0)));
    assert!(store.mark_pending(DurablePendingWakeMarker {
        title: Some("updated".into()),
        ..pending("a", PendingWakeKind::Shell, "w", 20.0)
    }));
    assert_eq!(store.list_pending().len(), 1);
    assert_eq!(store.list_pending()[0].title.as_deref(), Some("updated"));
    assert!(store.has_pending("a", PendingWakeKind::Shell, "w"));
    assert!(!store.file_path().with_extension("json.part").exists());

    assert!(store.mark_pending(pending("b", PendingWakeKind::Subagent, "x", 1.0)));
    let pruned = store.prune_stale(5.0, 20.0);
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].agent_id, "b");
    assert!(store.clear_one("a", PendingWakeKind::Shell, "w"));
    assert!(store.list_pending().is_empty());
    assert!(!store.file_path().exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_resume_store_coerces_upserts_and_deletes_file_when_empty() {
    let root = temp_root("upgrade");
    let store = SandUpgradeResumeStore::new(&root);
    store.mark_pending(UpgradeResumeMarker {
        agent_id: "a".into(),
        marked_at_ms: 1.0,
        source: Some("turn".into()),
        automation_id: None,
        automation_run_id: None,
    });
    store.mark_pending(UpgradeResumeMarker {
        agent_id: "a".into(),
        marked_at_ms: 2.0,
        source: Some("automation".into()),
        automation_id: Some("auto".into()),
        automation_run_id: Some("run".into()),
    });
    assert_eq!(store.list_pending().len(), 1);
    assert_eq!(store.list_pending()[0].marked_at_ms, 2.0);
    assert_eq!(
        parse_upgrade_resume_file(Some(r#"{"pending":[{"agentId":"b","markedAtMs":"bad"},{"agentId":7}]}"#))
            .len(),
        1
    );
    store.clear("a");
    assert!(store.list_pending().is_empty());
    assert!(!store.file_path().exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_resume_store_clears_only_the_observed_marker() {
    let root = temp_root("upgrade-cas");
    let store = SandUpgradeResumeStore::new(&root);
    let old = UpgradeResumeMarker {
        agent_id: "a".into(),
        marked_at_ms: 1.0,
        source: Some("turn".into()),
        automation_id: None,
        automation_run_id: None,
    };
    let newer = UpgradeResumeMarker {
        marked_at_ms: 2.0,
        ..old.clone()
    };
    store.mark_pending(old.clone());
    store.mark_pending(newer.clone());
    assert!(!store.clear_if_matches(&old));
    assert_eq!(store.list_pending(), vec![newer.clone()]);
    assert!(store.clear_if_matches(&newer));
    assert!(store.list_pending().is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_resume_marker_rejects_incomplete_automation_identity() {
    let marker = UpgradeResumeMarker {
        agent_id: "agent-a".into(),
        marked_at_ms: 1.0,
        source: Some("automation".into()),
        automation_id: None,
        automation_run_id: Some("run-a".into()),
    };
    assert!(!upgrade_resume_marker_identity_is_well_formed(&marker));
}

#[test]
fn pending_wake_store_does_not_clear_or_overwrite_a_newer_marker() {
    let root = temp_root("pending-cas");
    let store = SandPendingWakeStore::new(&root);
    let old = pending("agent-a", PendingWakeKind::Shell, "work", 1.0);
    let newer = DurablePendingWakeMarker {
        marked_at_ms: 2.0,
        title: Some("newer".into()),
        ..old.clone()
    };
    assert!(store.mark_pending(newer.clone()));
    assert!(!store.clear_if_matches(&old));
    assert_eq!(store.list_pending(), vec![newer.clone()]);
    assert!(!store.mark_if_absent_or_matches(&old));
    assert_eq!(store.list_pending(), vec![newer]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_transcript_runtime_owns_both_durable_recovery_stores_and_cleans_deleted_agent() {
    let root = temp_root("runtime");
    let runtime = ProductionTranscriptRuntime::new(Some(&root));
    let pending_store = runtime.pending_wake_store().expect("pending store");
    let upgrade_store = runtime.upgrade_resume_store().expect("upgrade store");

    assert!(pending_store.mark_pending(pending(
        "agent-a",
        PendingWakeKind::CloudAgent,
        "work",
        5.0,
    )));
    upgrade_store.mark_pending(UpgradeResumeMarker {
        agent_id: "agent-a".into(),
        marked_at_ms: 6.0,
        source: Some("turn".into()),
        automation_id: None,
        automation_run_id: None,
    });

    runtime.clear_agent_durable_recovery("agent-a");
    assert!(pending_store.list_pending().is_empty());
    assert!(upgrade_store.list_pending().is_empty());
    let _ = fs::remove_dir_all(root);
}
