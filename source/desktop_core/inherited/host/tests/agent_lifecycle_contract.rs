use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::agent_lifecycle::{
    AgentDeletionRuntimeDeps, AgentLifecycleGatewayError, ProductionAgentLifecycle,
    dispatch_production_agent_lifecycle_gateway_call,
    dispatch_production_agent_lifecycle_gateway_call_with_runtime,
    dispatch_production_agent_lifecycle_gateway_call_with_runtimes,
};
use mahayana_host_runtime::extensions::transcript::production_runtime::ProductionTranscriptRuntime;
use mahayana_host_runtime::extensions::transcript::roster_emit::ProductionRosterEmit;
use serde_json::json;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-agent-lifecycle-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn deletion_runtime_runs_owner_hooks_around_durable_session_delete() {
    let root = temp_root("runtime-cleanup");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");

    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let hook = |label: &'static str, calls: Arc<Mutex<Vec<String>>>| {
        Arc::new(move |agent_id: &str| {
            calls.lock().expect("calls").push(format!("{label}:{agent_id}"));
            Ok(())
        }) as mahayana_host_runtime::extensions::transcript::agent_lifecycle::AgentDeletionHook
    };
    let lifecycle = ProductionAgentLifecycle::with_deletion_runtime(
        Arc::clone(&production),
        AgentDeletionRuntimeDeps {
            mark_deleting: Some(hook("mark", Arc::clone(&calls))),
            clear_deleting: Some(hook("rollback", Arc::clone(&calls))),
            cancel_runner: Some(hook("runner", Arc::clone(&calls))),
            forget_ack: Some(hook("ack", Arc::clone(&calls))),
            sharing_departure: Some(hook("sharing", Arc::clone(&calls))),
            clear_trays: Some(hook("trays", Arc::clone(&calls))),
            dispose_background_work: Some(hook("background", Arc::clone(&calls))),
            drain_runner: Some(hook("drain", Arc::clone(&calls))),
            release_box: Some(hook("box", Arc::clone(&calls))),
            forget_agent_state: Some(hook("forgotten", Arc::clone(&calls))),
            forget_handoff: Some(hook("handoff", Arc::clone(&calls))),
            clear_pending_wakes: Some(hook("pending", Arc::clone(&calls))),
        },
    );

    lifecycle.delete_agent(&record.id).expect("delete");
    assert!(!store.agent_dir_exists(&record.id));
    let expected = vec![
        format!("mark:{}", record.id),
        format!("runner:{}", record.id),
        format!("ack:{}", record.id),
        format!("sharing:{}", record.id),
        format!("trays:{}", record.id),
        format!("background:{}", record.id),
        format!("drain:{}", record.id),
        format!("box:{}", record.id),
        format!("forgotten:{}", record.id),
        format!("handoff:{}", record.id),
        format!("pending:{}", record.id),
    ];
    assert_eq!(*calls.lock().expect("calls"), expected);

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn deletion_runtime_rolls_back_deleting_fence_when_predelete_drain_fails() {
    let root = temp_root("runtime-rollback");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));

    let mark_calls = Arc::clone(&calls);
    let rollback_calls = Arc::clone(&calls);
    let lifecycle = ProductionAgentLifecycle::with_deletion_runtime(
        Arc::clone(&production),
        AgentDeletionRuntimeDeps {
            mark_deleting: Some(Arc::new(move |agent_id| {
                mark_calls
                    .lock()
                    .expect("mark calls")
                    .push(format!("mark:{agent_id}"));
                Ok(())
            })),
            clear_deleting: Some(Arc::new(move |agent_id| {
                rollback_calls
                    .lock()
                    .expect("rollback calls")
                    .push(format!("rollback:{agent_id}"));
                Ok(())
            })),
            drain_runner: Some(Arc::new(|_| Err("drain failed".into()))),
            ..AgentDeletionRuntimeDeps::default()
        },
    );

    let error = lifecycle.delete_agent(&record.id).expect_err("delete must fail closed");
    assert_eq!(error, "drain failed");
    assert!(store.agent_exists(&record.id));
    assert_eq!(
        *calls.lock().expect("calls"),
        vec![
            format!("mark:{}", record.id),
            format!("rollback:{}", record.id),
        ]
    );

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn deleting_active_agent_selects_existing_successor_and_returns_its_transcript() {
    let root = temp_root("active-successor");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let first = store
        .create_session(None, "user", None)
        .expect("first agent");
    let second = store
        .create_session(None, "user", None)
        .expect("second agent");
    store
        .write_active_agent_id(&first.id)
        .expect("active pointer");
    production
        .append_agent_transcript_entries(
            &second.id,
            &[json!({
                "id":"successor-entry",
                "kind":"message",
                "role":"user",
                "content":"survives delete"
            })],
        )
        .expect("successor transcript");

    let lifecycle = ProductionAgentLifecycle::new(Arc::clone(&production));
    let result = lifecycle.delete_agent(&first.id).expect("delete active");

    assert!(!store.agent_dir_exists(&first.id));
    assert!(store.agent_exists(&second.id));
    assert_eq!(store.read_active_agent_id().as_deref(), Some(second.id.as_str()));
    assert_eq!(result["transcript"].as_array().map(Vec::len), Some(1));
    assert_eq!(result["transcript"][0]["id"], "successor-entry");

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn deleting_non_active_agent_keeps_active_transcript_and_delete_all_creates_fallback() {
    let root = temp_root("batch");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let active = store
        .create_session(None, "user", None)
        .expect("active");
    let other = store
        .create_session(None, "user", None)
        .expect("other");
    store
        .write_active_agent_id(&active.id)
        .expect("active pointer");
    production
        .append_agent_transcript_entries(
            &active.id,
            &[json!({
                "id":"active-entry",
                "kind":"message",
                "role":"user",
                "content":"keep me"
            })],
        )
        .expect("active transcript");

    let lifecycle = ProductionAgentLifecycle::new(Arc::clone(&production));
    let kept = lifecycle
        .delete_agents(&[other.id.clone(), other.id.clone()])
        .expect("delete duplicate non-active");
    assert_eq!(store.read_active_agent_id().as_deref(), Some(active.id.as_str()));
    assert_eq!(kept["transcript"][0]["id"], "active-entry");

    let fallback = lifecycle
        .delete_agents(std::slice::from_ref(&active.id))
        .expect("delete last active");
    let fallback_id = store
        .read_active_agent_id()
        .expect("fallback becomes active");
    assert_ne!(fallback_id, active.id);
    assert!(store.agent_exists(&fallback_id));
    assert_eq!(fallback["transcript"], json!([]));
    assert!(store.active_agent_pointer_path().exists());

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn duplicate_agent_gateway_uses_clone_owner_and_activates_empty_copy() {
    use mahayana_host_runtime::agents::agent_profile::SandAgentProfile;
    use mahayana_host_runtime::extensions::session::agent_db::{
        read_persisted_agent_metadata_projection, read_persisted_agent_origin,
        read_persisted_agent_purpose,
    };

    let root = temp_root("duplicate");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let source = store
        .create_session(
            Some(&SandAgentProfile {
                name: "Alpha".into(),
                description: "source".into(),
                title: String::new(),
                avatar_shape: String::new(),
                avatar_color: String::new(),
            }),
            "dev",
            Some("disk-saver"),
        )
        .expect("source");
    production
        .append_agent_transcript_entries(
            &source.id,
            &[json!({
                "id":"source-entry",
                "kind":"message",
                "role":"user",
                "content":"do not copy chat"
            })],
        )
        .expect("source transcript");
    store
        .write_active_agent_id(&source.id)
        .expect("active source");

    let duplicated = dispatch_production_agent_lifecycle_gateway_call(
        &production,
        "duplicateAgent",
        &json!({"id":source.id}),
    )
    .expect("handled")
    .expect("duplicated");

    let clone_id = duplicated["agent"]["id"]
        .as_str()
        .expect("clone id")
        .to_string();
    assert_ne!(clone_id, source.id);
    assert_eq!(duplicated["agent"]["name"], "Alpha copy");
    assert_eq!(duplicated["transcript"], json!([]));
    assert_eq!(store.read_active_agent_id().as_deref(), Some(clone_id.as_str()));
    assert_eq!(
        read_persisted_agent_metadata_projection(
            &production.session_db_path(&clone_id).expect("clone db"),
            500
        )
        .expect("metadata")
        .expect("metadata present")
        .agent_id,
        clone_id
    );
    assert_eq!(
        read_persisted_agent_origin(
            &production.session_db_path(&clone_id).expect("clone db"),
            500
        )
        .expect("origin"),
        "user"
    );
    assert_eq!(
        read_persisted_agent_purpose(
            &production.session_db_path(&clone_id).expect("clone db"),
            500
        )
        .expect("purpose"),
        None
    );
    assert_eq!(
        store.read_agent_transcript_entries(&source.id)
            .expect("source transcript")
            .len(),
        1
    );

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn lifecycle_gateway_owns_create_background_and_update_agent_mutations() {
    let root = temp_root("create-update-owner");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));

    let created = dispatch_production_agent_lifecycle_gateway_call(
        &production,
        "createAgent",
        &json!({
            "name":"Foreground",
            "description":"",
            "origin":"user",
            "isIntroductionSuppressed":true
        }),
    )
    .expect("createAgent handled")
    .expect("foreground created");
    let foreground_id = created["agent"]["id"]
        .as_str()
        .expect("foreground id")
        .to_string();
    assert_eq!(created["agent"]["isActive"], true);
    assert_eq!(store.read_active_agent_id().as_deref(), Some(foreground_id.as_str()));
    assert!(!production
        .get_agent_introduction_pending(&foreground_id)
        .expect("foreground introduction"));

    let background = dispatch_production_agent_lifecycle_gateway_call(
        &production,
        "createBackgroundAgent",
        &json!({
            "name":"Background",
            "description":"quiet worker",
            "origin":"automation",
            "isIntroductionSuppressed":true
        }),
    )
    .expect("createBackgroundAgent handled")
    .expect("background created");
    let background_id = background["agent"]["id"]
        .as_str()
        .expect("background id")
        .to_string();
    assert_eq!(background["agent"]["isActive"], false);
    assert_eq!(store.read_active_agent_id().as_deref(), Some(foreground_id.as_str()));

    let updated = dispatch_production_agent_lifecycle_gateway_call(
        &production,
        "updateAgent",
        &json!({
            "id": background_id,
            "profile": {
                "name":" Background Updated ",
                "description":" lifecycle owner ",
                "title":" Worker ",
                "avatarShape":" rounded ",
                "avatarColor":" violet "
            }
        }),
    )
    .expect("updateAgent handled")
    .expect("background updated");
    assert_eq!(updated["name"], "Background Updated");
    assert_eq!(updated["description"], "lifecycle owner");
    assert_eq!(updated["title"], "Worker");

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn lifecycle_mutations_publish_through_the_single_roster_owner() {
    let root = temp_root("roster-owner");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let transcript = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let roster = Arc::new(ProductionRosterEmit::new(
        Arc::clone(&production),
        transcript,
        Arc::new(move |event| sink.lock().expect("events").push(event)),
    ));
    let deletion = AgentDeletionRuntimeDeps::default();

    let created = dispatch_production_agent_lifecycle_gateway_call_with_runtimes(
        &production,
        &deletion,
        Some(Arc::clone(&roster)),
        "createAgent",
        &json!({"name":"Roster Agent","description":""}),
    )
    .expect("create handled")
    .expect("create");
    let agent_id = created["agent"]["id"].as_str().expect("agent id").to_string();
    assert!(created["agent"]["snapshotEpoch"].is_string());
    assert!(created["agent"]["snapshotSeq"].as_u64().is_some());
    let created_snapshot_epoch = created["agent"]["snapshotEpoch"].clone();
    let created_snapshot_seq = created["agent"]["snapshotSeq"].as_u64().expect("created snapshot seq");
    assert_eq!(events.lock().expect("events")[0]["channel"], "agents");

    let updated = dispatch_production_agent_lifecycle_gateway_call_with_runtimes(
        &production,
        &deletion,
        Some(Arc::clone(&roster)),
        "updateAgent",
        &json!({
            "id": agent_id,
            "profile": {"name":"Renamed","description":"profile changed"}
        }),
    )
    .expect("update handled")
    .expect("update");
    assert_eq!(updated["snapshotEpoch"], created_snapshot_epoch);
    let updated_snapshot_seq = updated["snapshotSeq"].as_u64().expect("updated snapshot seq");
    assert!(updated_snapshot_seq > created_snapshot_seq);
    let observed = events.lock().expect("events").clone();
    let delta = observed
        .iter()
        .find(|event| event["channel"] == "agent-upserted")
        .expect("agent-upserted event");
    assert_eq!(delta["payload"]["agent"]["snapshotEpoch"], created_snapshot_epoch);
    assert!(
        delta["payload"]["agent"]["snapshotSeq"]
            .as_u64()
            .expect("delta snapshot seq")
            > updated_snapshot_seq,
        "frozen updateAgent reserves the RPC snapshot stamp before emitting the roster delta"
    );
    assert!(observed.iter().any(|event| event["channel"] == "agent-upserted"));
    assert!(observed.iter().any(|event| event["channel"] == "profile-changed"));

    dispatch_production_agent_lifecycle_gateway_call_with_runtimes(
        &production,
        &deletion,
        Some(Arc::clone(&roster)),
        "deleteAgent",
        &json!({"id": agent_id}),
    )
    .expect("delete handled")
    .expect("delete");
    assert_eq!(
        events.lock().expect("events").last().expect("last event")["channel"],
        "agents"
    );

    production.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_gateway_contract_validates_delete_arguments_and_uses_lifecycle_owner() {
    let root = temp_root("gateway");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("agent");
    store
        .write_active_agent_id(&record.id)
        .expect("active pointer");

    assert_eq!(
        dispatch_production_agent_lifecycle_gateway_call(
            &production,
            "deleteAgent",
            &json!({"id":""}),
        ),
        Some(Err(AgentLifecycleGatewayError::BadRequest(
            "missing or invalid id".into()
        )))
    );
    let deleted = dispatch_production_agent_lifecycle_gateway_call(
        &production,
        "deleteAgents",
        &json!({"ids":[record.id]}),
    )
    .expect("handled")
    .expect("deleted");
    assert_eq!(deleted["transcript"], json!([]));
    let fallback_id = store
        .read_active_agent_id()
        .expect("delete gateway activates fallback");
    assert_ne!(fallback_id, record.id);
    assert!(store.agent_exists(&fallback_id));

    assert!(dispatch_production_agent_lifecycle_gateway_call(
        &production,
        "not-a-lifecycle-method",
        &json!({}),
    )
    .is_none());

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn guarded_delete_rejects_stale_revision_before_destructive_hooks() {
    let root = temp_root("stale-revision");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");
    let summary = production
        .summarize_agent_by_id(&record.id, None)
        .expect("summary")
        .expect("summary present");

    let result = dispatch_production_agent_lifecycle_gateway_call(
        &production,
        "deleteAgent",
        &json!({
            "id": record.id.clone(),
            "expectedSessionId": record.id.clone(),
            "expectedLifecycleRevision": "stale-revision",
        }),
    )
    .expect("handled");
    assert_eq!(
        result,
        Err(AgentLifecycleGatewayError::BadRequest(
            "SESSION_LIFECYCLE_CHANGED".into()
        ))
    );
    assert!(store.agent_exists(&record.id));
    assert!(!summary.lifecycle_revision.is_empty());

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn guarded_delete_rechecks_revision_after_drain_mutation() {
    let root = temp_root("drain-revision");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");
    let summary = production
        .summarize_agent_by_id(&record.id, None)
        .expect("summary")
        .expect("summary present");
    let agent_id = record.id.clone();
    let mutation_owner = Arc::clone(&production);
    let runtime = AgentDeletionRuntimeDeps {
        drain_runner: Some(Arc::new(move |_| {
            mutation_owner
                .open_agent_db_owner(&agent_id)
                .and_then(|owner| {
                    owner
                        .set_metadata("drainMutation", json!(true))
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                })
        })),
        ..AgentDeletionRuntimeDeps::default()
    };

    let result = dispatch_production_agent_lifecycle_gateway_call_with_runtime(
        &production,
        &runtime,
        "deleteAgent",
        &json!({
            "id": record.id.clone(),
            "expectedSessionId": record.id.clone(),
            "expectedLifecycleRevision": summary.lifecycle_revision,
        }),
    )
    .expect("handled");
    assert_eq!(
        result,
        Err(AgentLifecycleGatewayError::BadRequest(
            "SESSION_LIFECYCLE_CHANGED".into()
        ))
    );
    assert!(store.agent_exists(&record.id));

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn guarded_delete_rejects_stale_updated_at_before_destructive_hooks() {
    let root = temp_root("stale-updated-at");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");
    let summary = production
        .summarize_agent_by_id(&record.id, None)
        .expect("summary")
        .expect("summary present");

    let result = dispatch_production_agent_lifecycle_gateway_call(
        &production,
        "deleteAgent",
        &json!({
            "id": record.id.clone(),
            "expectedSessionId": record.id.clone(),
            "expectedLifecycleRevision": summary.lifecycle_revision,
            "expectedSessionUpdatedAt": summary.updated_at + 1.0,
        }),
    )
    .expect("handled");
    assert_eq!(
        result,
        Err(AgentLifecycleGatewayError::BadRequest(
            "SESSION_LIFECYCLE_CHANGED".into()
        ))
    );
    assert!(store.agent_exists(&record.id));

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn guarded_delete_allows_matching_identity_revision_and_updated_at() {
    let root = temp_root("guarded-success");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");
    let summary = production
        .summarize_agent_by_id(&record.id, None)
        .expect("summary")
        .expect("summary present");

    let result = dispatch_production_agent_lifecycle_gateway_call(
        &production,
        "deleteAgent",
        &json!({
            "id": record.id.clone(),
            "expectedSessionId": record.id.clone(),
            "expectedLifecycleRevision": summary.lifecycle_revision,
            "expectedSessionUpdatedAt": summary.updated_at,
        }),
    )
    .expect("handled")
    .expect("guarded delete");
    assert_eq!(result["transcript"], json!([]));
    assert!(!store.agent_exists(&record.id));

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn guarded_delete_rejects_declared_invalid_updated_at() {
    let root = temp_root("invalid-updated-at");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");

    let result = dispatch_production_agent_lifecycle_gateway_call(
        &production,
        "deleteAgent",
        &json!({
            "id": record.id.clone(),
            "expectedSessionUpdatedAt": "not-a-number",
        }),
    )
    .expect("handled");
    assert_eq!(
        result,
        Err(AgentLifecycleGatewayError::BadRequest(
            "expectedSessionUpdatedAt must be a finite number".into()
        ))
    );
    assert!(store.agent_exists(&record.id));

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn lifecycle_roster_owner_covers_unread_notifications_hidden_and_avatar_mutations() {
    let root = temp_root("roster-setting-mutations");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let transcript = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let roster = Arc::new(ProductionRosterEmit::new(
        Arc::clone(&production),
        transcript,
        Arc::new(move |event| sink.lock().expect("events").push(event)),
    ));
    let deletion = AgentDeletionRuntimeDeps::default();

    let created = dispatch_production_agent_lifecycle_gateway_call_with_runtimes(
        &production,
        &deletion,
        Some(Arc::clone(&roster)),
        "createAgent",
        &json!({"name":"Settings Agent","description":""}),
    )
    .expect("create handled")
    .expect("create");
    let agent_id = created["agent"]["id"].as_str().expect("agent id").to_string();

    for (method, args, field, expected) in [
        (
            "setAgentUnread",
            json!({"id": agent_id, "isUnread": true, "atMs": 1234.0}),
            "hasUnread",
            json!(true),
        ),
        (
            "setAgentNotifyOnUpdates",
            json!({"id": agent_id, "isEnabled": false}),
            "notifyOnUpdatesEnabled",
            json!(false),
        ),
        (
            "setAgentHiddenFromSidebar",
            json!({"id": agent_id, "isHidden": true}),
            "isHiddenFromSidebar",
            json!(true),
        ),
    ] {
        let before = events.lock().expect("events").len();
        let result = dispatch_production_agent_lifecycle_gateway_call_with_runtimes(
            &production,
            &deletion,
            Some(Arc::clone(&roster)),
            method,
            &args,
        )
        .expect("setting handled")
        .expect("setting");
        assert_eq!(result, serde_json::Value::Null);
        let observed = events.lock().expect("events");
        assert!(observed.len() > before);
        let delta = observed.last().expect("setting delta");
        assert_eq!(delta["channel"], "agent-upserted");
        assert_eq!(delta["payload"]["agent"][field], expected);
    }

    let png =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Z2S8AAAAASUVORK5CYII=";
    let before_avatar = events.lock().expect("events").len();
    let avatar_summary = dispatch_production_agent_lifecycle_gateway_call_with_runtimes(
        &production,
        &deletion,
        Some(Arc::clone(&roster)),
        "setAgentAvatarBytes",
        &json!({"id": agent_id, "pngBase64": png}),
    )
    .expect("avatar handled")
    .expect("avatar");
    assert!(avatar_summary["snapshotEpoch"].is_string());
    assert!(avatar_summary["snapshotSeq"].as_u64().is_some());

    let observed = events.lock().expect("events").clone();
    let avatar_events = &observed[before_avatar..];
    assert_eq!(avatar_events.len(), 2);
    assert_eq!(avatar_events[0]["channel"], "agent-upserted");
    assert_eq!(avatar_events[1]["channel"], "profile-changed");

    let avatar = dispatch_production_agent_lifecycle_gateway_call_with_runtimes(
        &production,
        &deletion,
        Some(Arc::clone(&roster)),
        "getAgentAvatar",
        &json!({"id": agent_id}),
    )
    .expect("get avatar handled")
    .expect("get avatar");
    assert!(avatar["version"].as_str().is_some_and(|value| !value.is_empty()));
    assert!(avatar["dataUrl"]
        .as_str()
        .is_some_and(|value| value.starts_with("data:image/png;base64,")));

    production.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_gateway_orders_lifecycle_owner_before_generic_session_gateway() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest_dir.join("app/src/main.rs")).expect("shipping main");
    let lifecycle = main
        .find(".dispatch_agent_lifecycle_gateway_call(")
        .expect("manager-owned lifecycle dispatcher");
    let session = main
        .find(".dispatch_session_gateway_call(method, &args)")
        .expect("manager-owned session dispatcher");
    assert!(lifecycle < session, "lifecycle mutations must win before the generic session fallback");

    let lifecycle_source =
        fs::read_to_string(manifest_dir.join("src/extensions/transcript/agent_lifecycle.rs"))
            .expect("lifecycle source");
    for method in [
        "setAgentUnread",
        "setAgentNotifyOnUpdates",
        "setAgentHiddenFromSidebar",
        "setAgentAvatarBytes",
        "getAgentAvatar",
    ] {
        assert!(
            lifecycle_source.contains(&format!("\"{method}\" => lifecycle.")),
            "{method} must be owned by the production lifecycle dispatcher"
        );
    }
}

#[test]
fn generated_subagent_cleanup_keeps_the_gateway_lifecycle_owner_and_captured_revision() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest_dir.join("app/src/main.rs")).expect("shipping main");
    let openclaw_sessions =
        fs::read_to_string(manifest_dir.join("app/src/ocrm_handlers/openclaw_sessions.rs"))
            .unwrap_or_default();
    let launch = main
        .find("let child_lifecycle_revision = self")
        .expect("subagent launch must capture lifecycle revision");
    let request = main
        .find("\"lifecycleRevision\": child_lifecycle_revision")
        .expect("subagent request must carry lifecycle revision");
    let completion = main
        .find("let cleanup_revision = completion.lifecycle_revision.clone()")
        .expect("completion must retain lifecycle revision");
    let owner = main
        .find("fn delete_generated_subagent(")
        .expect("cleanup must have one Gateway-owned entrypoint");
    let guarded_delete_offset = main
        .get(owner..)
        .and_then(|source| source.find("\"expectedLifecycleRevision\": lifecycle_revision"))
        .or_else(|| {
            openclaw_sessions
                .find("fn delete_generated_subagent(")
                .and_then(|offset| openclaw_sessions.get(offset..))
                .and_then(|source| source.find("\"expectedLifecycleRevision\": lifecycle_revision"))
        })
        .expect("cleanup must pass the captured revision to the lifecycle owner");
    assert!(launch < request);
    assert!(completion != launch && guarded_delete_offset > 0);
    assert!(main.contains("generated_subagent_cleanup_gateway"));
}

#[test]
fn deletion_owner_publishes_and_clears_the_durable_session_admission_fence() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let production = format!(
        "{}\n{}",
        fs::read_to_string(manifest_dir.join("src/extensions/session/production.rs"))
            .expect("session production source"),
        fs::read_to_string(
            manifest_dir.join("src/extensions/session/ocrm_production_compaction.rs")
        )
        .unwrap_or_default()
    );
    let agent_db = fs::read_to_string(manifest_dir.join("src/extensions/session/agent_db.rs"))
        .expect("session database source");
    let agent_session = fs::read_to_string(manifest_dir.join("src/extensions/session/agent_session.rs"))
        .expect("session facade source");
    assert!(agent_db.contains("pub deletion_pending: bool"));
    assert!(production.contains("record.deletion_pending = true"));
    assert!(production.contains("record.deletion_pending = false"));
    assert!(production.contains("pub fn mark_agent_activity"));
    assert!(production.contains("pub fn mark_agent_activity")
        && production.contains("self.assert_agent_work_admission(agent_id)?;"));
    assert!(production.contains("pub fn set_agent_awaiting_user_response"));
    assert!(production.contains("pub fn set_agent_awaiting_user_response_for_tab"));
    assert!(production.contains("pub fn set_agent_introduction_pending"));
    assert!(production.contains("pub fn set_agent_automation_spend_guard_state"));
    assert!(production.contains("pub fn set_agent_memory_prompt_snapshot"));
    assert!(production.contains("pub fn set_agent_profile_prompt_snapshot"));
    assert!(production.contains("pub fn mark_agent_viewed"));
    assert!(production.contains("pub fn set_agent_unread"));
    assert!(production.contains("pub fn add_agent_conversation_partner"));
    assert!(agent_db.contains("admission.deletion_pending"));
    assert!(production.contains("publishing deletion fence"));
    assert!(production.contains("clearing deletion fence"));
    assert!(agent_session.contains("self.production.begin_agent_delete(agent_id)?"));
    assert!(agent_session.contains("self.production.clear_agent_delete_fast_path(agent_id)"));
    assert!(agent_session.contains("Ok(()) => self.production.end_agent_delete(agent_id)"));
    assert!(agent_session.contains("Err(error) => {"));
}
