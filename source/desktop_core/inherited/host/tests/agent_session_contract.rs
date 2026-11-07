use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::settings_file::{
    get_sand_settings_path, read_sand_settings_file,
};
use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::inference::model_picker_service::{
    ApplySessionModelSelectionParams, ModelCatalogEntry, SessionModelSelectionRequest,
    SessionModelSelectionRuntime,
};
use mahayana_host_runtime::extensions::session::session_profile_files::AgentProfileUpdate;
use mahayana_host_runtime::extensions::memory::memory_service::MemoryKind;
use mahayana_host_runtime::automations::automation::AutomationSpec;
use mahayana_host_runtime::workflows::workflow_library::WorkflowSpec;
use mahayana_host_runtime::extensions::session::agent_db_serde::{
    AwaitingUserResponse, SpendGuardState,
};
use mahayana_host_runtime::extensions::session::agent_db_transcript_pages::{
    TranscriptPageQuery, TranscriptWindowQuery,
};
use mahayana_host_runtime::extensions::session::agent_db::SessionWorkAdmissionRecord;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::transcript_mutation_events::subscribe_transcript_mutations;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-agent-session-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn active_agent_pointer_roster_and_settings_are_owned_by_session_facade() {
    let root = temp_root("active");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let first = store
        .create_session(None, "user", None)
        .expect("first agent");
    let second = store
        .create_session(None, "user", None)
        .expect("second agent");

    assert_eq!(store.read_active_agent_id(), None);
    store.write_active_agent_id(&second.id).expect("active pointer");
    assert_eq!(store.read_active_agent_id().as_deref(), Some(second.id.as_str()));

    assert_eq!(store.count_owned_agents().expect("owned agents"), 2);
    let agents_list = store.list_agents().expect("roster");
    // Frozen Grok roster semantics hide blank non-active agents without a durable
    // footprint; the active blank agent remains visible.
    assert_eq!(agents_list.len(), 1);
    assert_eq!(
        agents_list.iter().filter(|agent| agent.is_active).count(),
        1
    );
    assert!(agents_list.iter().any(|agent| agent.id == second.id && agent.is_active));

    store
        .set_session_notify_on_updates(&first.id, false)
        .expect("notifications");
    store
        .set_session_hidden_from_sidebar(&first.id, true)
        .expect("hidden");
    let settings = read_sand_settings_file(get_sand_settings_path(agents.join(&first.id)));
    assert!(!settings.notify_on_agent_updates);
    assert!(settings.hidden_from_sidebar);

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn delete_session_closes_owned_blob_store_clears_directory_and_publishes_removal() {
    let root = temp_root("delete");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("agent");
    let _ = store.open_session(&record.id).expect("open existing");
    assert_eq!(production.active_agent_store_owner_count(), 1);
    let removals = Arc::new(AtomicUsize::new(0));
    let subscription = subscribe_transcript_mutations({
        let removals = Arc::clone(&removals);
        let id = record.id.clone();
        move |mutation| {
            if mutation.get("kind").and_then(serde_json::Value::as_str) == Some("agent-removed")
                && mutation.get("agentId").and_then(serde_json::Value::as_str) == Some(id.as_str())
            {
                removals.fetch_add(1, Ordering::SeqCst);
            }
        }
    });

    store.delete_session(&record.id).expect("delete");
    assert!(!store.agent_dir_exists(&record.id));
    assert!(!store.agent_exists(&record.id));
    assert_eq!(production.active_agent_store_owner_count(), 0);
    assert_eq!(removals.load(Ordering::SeqCst), 1);

    subscription.unsubscribe();
    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn deletion_fence_rejects_new_transcript_work_before_durable_write() {
    let root = temp_root("work-admission-delete-fence");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("agent");
    let session_key = production
        .read_session_work_admission(&record.id)
        .expect("read session admission")
        .expect("main session admission")
        .session_key;

    production
        .begin_agent_delete(&record.id)
        .expect("publish deletion fence");
    let error = production
        .append_agent_transcript_entries(
            &record.id,
            &[serde_json::json!({
                "id": "blocked-entry",
                "kind": "message",
                "role": "user",
                "content": "must not be admitted",
            })],
        )
        .expect_err("deletion-pending work must be rejected");
    assert!(error.contains("new session work is not admitted"));
    let awaiting = AwaitingUserResponse {
        tab_id: "tab-delete-fence".into(),
        reason: "approval".into(),
        since: 41.0,
    };
    let awaiting_error = production
        .set_agent_awaiting_user_response(&record.id, Some(&awaiting))
        .expect_err("deletion-pending awaiting state must be rejected");
    assert!(awaiting_error.contains("new session work is not admitted"));
    let awaiting_tab_error = production
        .set_agent_awaiting_user_response_for_tab(&record.id, &awaiting.tab_id, None, None)
        .expect_err("deletion-pending tab awaiting state must be rejected");
    assert!(awaiting_tab_error.contains("new session work is not admitted"));
    let session_admitted_error = production
        .append_agent_transcript_entries_if_session_admitted(
            &record.id,
            &session_key,
            &[serde_json::json!({
                "id": "blocked-session-entry",
                "kind": "message",
                "role": "user",
                "content": "session-bound work must not be persisted during deletion",
                "sessionKey": session_key.clone(),
            })],
        )
        .expect_err("deletion-pending session append must be rejected");
    assert!(session_admitted_error.contains("new session work is not admitted"));
    let introduction_error = production
        .set_agent_introduction_pending(&record.id, false)
        .expect_err("deletion-pending introduction state must be rejected");
    assert!(introduction_error.contains("new session work is not admitted"));
    let spend_guard_error = production
        .set_agent_automation_spend_guard_state(&record.id, &SpendGuardState::default())
        .expect_err("deletion-pending spend-guard state must be rejected");
    assert!(spend_guard_error.contains("new session work is not admitted"));
    let memory_snapshot_error = production
        .set_agent_memory_prompt_snapshot(&record.id, &serde_json::json!({"render":"blocked"}))
        .expect_err("deletion-pending memory snapshot must be rejected");
    assert!(memory_snapshot_error.contains("new session work is not admitted"));
    let profile_snapshot_error = production
        .set_agent_profile_prompt_snapshot(&record.id, &serde_json::json!({"name":"blocked"}))
        .expect_err("deletion-pending profile snapshot must be rejected");
    assert!(profile_snapshot_error.contains("new session work is not admitted"));
    let profile_error = production
        .update_agent_profile(
            &record.id,
            &AgentProfileUpdate {
                name: "blocked profile".into(),
                description: "must not write during deletion".into(),
                title: None,
                avatar_shape: None,
                avatar_color: None,
            },
            None,
        )
        .expect_err("deletion-pending profile file mutation must be rejected");
    assert!(profile_error.contains("new session work is not admitted"));
    let avatar_error = production
        .set_agent_avatar_bytes(&record.id, Some(b"blocked-avatar"), None)
        .expect_err("deletion-pending avatar mutation must be rejected");
    assert!(avatar_error.contains("new session work is not admitted"));
    let memory_clear_error = production
        .clear_agent_memories(&record.id)
        .expect_err("deletion-pending external memory clear must be rejected");
    assert!(memory_clear_error.contains("new session work is not admitted"));
    let memory_snapshot_clear_error = production
        .clear_agent_memory_prompt_snapshot(&record.id)
        .expect_err("deletion-pending memory snapshot clear must be rejected");
    assert!(memory_snapshot_clear_error.contains("new session work is not admitted"));
    let memory_remove_error = production
        .remove_agent_memory(&record.id, "blocked-memory")
        .expect_err("deletion-pending external memory removal must be rejected");
    assert!(memory_remove_error.contains("new session work is not admitted"));
    let viewed_error = production
        .mark_agent_viewed(&record.id, 50.0, false)
        .expect_err("deletion-pending viewed state must be rejected");
    assert!(viewed_error.contains("new session work is not admitted"));
    let unread_error = production
        .set_agent_unread(&record.id, true, 51.0)
        .expect_err("deletion-pending unread state must be rejected");
    assert!(unread_error.contains("new session work is not admitted"));
    let partner_error = production
        .add_agent_conversation_partner(&record.id, "partner-delete-fence")
        .expect_err("deletion-pending partner state must be rejected");
    assert!(partner_error.contains("new session work is not admitted"));
    let activity_error = production
        .mark_agent_activity(&record.id, 42.0)
        .expect_err("deletion-pending activity must be rejected");
    assert!(activity_error.contains("new session work is not admitted"));
    production
        .end_agent_delete(&record.id)
        .expect("clear deletion fence");
    assert!(production
        .set_agent_awaiting_user_response(&record.id, Some(&awaiting))
        .expect("awaiting state is admitted after deletion fence clears"));
    assert!(production
        .set_agent_awaiting_user_response_for_tab(&record.id, &awaiting.tab_id, None, None)
        .expect("tab awaiting state is admitted after deletion fence clears"));
    production
        .set_agent_introduction_pending(&record.id, false)
        .expect("introduction state is admitted after deletion fence clears");
    production
        .append_agent_transcript_entries(
            &record.id,
            &[serde_json::json!({
                "id": "admitted-entry",
                "kind": "message",
                "role": "user",
                "content": "admitted after cleanup fence",
            })],
        )
        .expect("work is admitted after deletion fence clears");

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn model_selection_does_not_report_applied_when_owner_rejects_persistence() {
    let root = temp_root("model-selection-delete-fence");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("session");

    production
        .begin_agent_delete(&record.id)
        .expect("publish deletion fence");

    let catalog = [ModelCatalogEntry {
        provider: "cursor".into(),
        id: "model-a".into(),
        name: None,
        context_window: None,
        context_tokens: Some(8192),
        api: None,
        base_url: None,
        reasoning: None,
    }];
    let result = store.apply_session_model_selection(
        &record.id,
        ApplySessionModelSelectionParams {
            agent_id: &record.id,
            session_key: &format!("agent:{}:main", record.id),
            default_provider: "cursor",
            default_model: "model-default",
            current_provider: "cursor",
            current_model: "model-default",
            model_catalog: &catalog,
            visibility_policy: None,
            operator_authority: None,
            placement_context: None,
            candidate_execution_mode: None,
            initial_generation: None,
            current_generation: None,
            request: SessionModelSelectionRequest {
                provider: "cursor".into(),
                model: "model-a".into(),
                is_default: false,
                reset_to_default: false,
                alias: None,
                profile_override: None,
                runtime: SessionModelSelectionRuntime::Unchanged,
            },
        },
        100,
    )
    .expect_err("rejected persistence must not report Applied");
    // Deletion now rejects at owner admission, before the model policy or
    // commit edge can report an outcome.
    assert!(result.contains("new session work is not admitted"));

    production
        .end_agent_delete(&record.id)
        .expect("clear deletion fence");
    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn archived_main_session_rejects_routed_work_admission() {
    let root = temp_root("work-admission-archive");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("agent");
    let current = production
        .read_session_work_admission(&record.id)
        .expect("read admission")
        .expect("main admission");
    let mut archived = SessionWorkAdmissionRecord {
        archived_at_ms: Some(100.0),
        archived_by: Some("human-1".into()),
        archive_reason: Some("review".into()),
        lifecycle_revision: "archived-revision".into(),
        ..current.clone()
    };
    assert!(production
        .compare_and_set_session_work_admission(
            &record.id,
            Some(&current.lifecycle_revision),
            &archived,
        )
        .expect("archive transition"));
    let error = production
        .assert_session_work_admission(&record.id, Some(&current.session_key))
        .expect_err("archived session must reject work");
    assert!(error.contains("is archived"));
    let transcript_error = production
        .append_agent_transcript_entries(
            &record.id,
            &[serde_json::json!({
                "id": "archived-entry",
                "kind": "message",
                "role": "user",
                "content": "must not bypass archive fence",
            })],
        )
        .expect_err("archived session must reject agent-only transcript writes");
    assert!(transcript_error.contains("session is archived"));
    archived.archived_at_ms = None;
    archived.lifecycle_revision = "restored-revision".into();
    assert!(production
        .compare_and_set_session_work_admission(
            &record.id,
            Some("archived-revision"),
            &archived,
        )
        .expect("restore transition"));
    production
        .assert_session_work_admission(&record.id, Some(&current.session_key))
        .expect("restored session admits work");
    let mismatch = production
        .assert_session_work_admission(&record.id, Some("agent:wrong:main"))
        .expect_err("rebound session key must fail closed");
    assert!(mismatch.contains("key mismatch"));

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_delete_publishes_and_clears_owner_local_deletion_fence() {
    let root = temp_root("work-admission-deletion-fence");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("agent");
    let owner = production
        .open_agent_db_owner(&record.id)
        .expect("owner before deletion");
    let initial = owner
        .read_session_work_admission()
        .expect("initial admission")
        .expect("admission");

    production
        .begin_agent_delete(&record.id)
        .expect("publish deletion fence");
    let read_error = production
        .read_session_work_admission(&record.id)
        .expect_err("ordinary owner opens must remain fenced");
    assert!(read_error.contains("owner open is not admitted"));
    let fenced = owner
        .read_session_work_admission()
        .expect("observe durable admission through retained handle")
        .expect("admission");
    assert!(fenced.deletion_pending);
    assert_eq!(fenced.session_key, initial.session_key);
    assert_eq!(fenced.session_id, initial.session_id);
    production
        .end_agent_delete(&record.id)
        .expect("clear deletion fence");
    assert!(!production
        .read_session_work_admission(&record.id)
        .expect("read cleared admission")
        .expect("admission")
        .deletion_pending);

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pending_media_admission_blocks_archive_until_same_generation_release() {
    let root = temp_root("work-admission-media");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("agent");
    let session_key = format!("agent:{}:main", record.id);
    let missing_path = production
        .capture_session_media_admission(
            &record.id,
            &session_key,
            "send-media:missing-path",
            None,
        )
        .expect_err("new media custody requires a durable materialized path");
    assert!(missing_path.contains("requires a materialized path"));
    assert!(production
        .read_session_work_admission(&record.id)
        .expect("read admission after missing path")
        .expect("session admission after missing path")
        .admitted_media_work_id
        .is_none());
    let media_path = root.join("agents").join("attachments").join("outgoing.bin");
    let revision = production
        .capture_session_media_admission(
            &record.id,
            &session_key,
            "send-media:tool-1",
            Some(&media_path),
        )
        .expect("capture media generation");
    let admitted = production
        .read_session_work_admission(&record.id)
        .expect("read admission")
        .expect("main admission");
    assert_eq!(
        admitted.admitted_media_work_id.as_deref(),
        Some("send-media:tool-1")
    );
    assert_eq!(
        admitted.admitted_media_paths,
        vec![media_path.to_string_lossy().to_string()]
    );

    let archive_error = production
        .transition_session_work_archive(
            &record.id,
            &session_key,
            &record.id,
            Some(&revision),
            true,
            Some("gateway-operator"),
            Some("manual review"),
            42.0,
        )
        .expect_err("pending media must block archive");
    assert!(archive_error.contains("pending outgoing media"));

    assert!(production
        .release_session_media_admission(&record.id, &revision, "send-media:tool-1")
        .expect("release media admission"));
    assert!(!production
        .release_session_media_admission(&record.id, &revision, "send-media:tool-1")
        .expect("duplicate release"));
    production
        .transition_session_work_archive(
            &record.id,
            &session_key,
            &record.id,
            Some(&revision),
            true,
            Some("gateway-operator"),
            Some("manual review"),
            43.0,
        )
        .expect("archive after media settlement");

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn reopened_session_owner_rolls_back_unsettled_materialized_media() {
    let root = temp_root("work-admission-media-recovery");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("agent");
    let session_key = format!("agent:{}:main", record.id);
    let media_path = agents.join(&record.id).join("attachments").join("outgoing.bin");
    fs::create_dir_all(media_path.parent().expect("media parent")).expect("media dir");
    fs::write(&media_path, b"pending media").expect("media file");
    let revision = production
        .capture_session_media_admission(
            &record.id,
            &session_key,
            "send-media:tool-recovery",
            Some(&media_path),
        )
        .expect("capture media generation");
    assert!(!revision.is_empty());
    assert!(production.close_agent_db_owner(&record.id, false));

    production
        .open_agent_db_owner(&record.id)
        .expect("reopen session owner");
    assert!(!media_path.exists());
    let admission = production
        .read_session_work_admission(&record.id)
        .expect("read recovered admission")
        .expect("recovered admission");
    assert!(admission.admitted_media_work_id.is_none());
    assert!(admission.admitted_media_paths.is_empty());

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn background_work_generation_recheck_rejects_stale_terminal_result() {
    let root = temp_root("work-admission-generation");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("agent");

    let expected_revision = production
        .capture_session_work_admission(
            &record.id,
            &format!("agent:{}:main", record.id),
            "background-work-1",
        )
        .expect("capture active generation");
    let current = production
        .read_session_work_admission(&record.id)
        .expect("read admission")
        .expect("main admission");
    let mut rebound = current.clone();
    rebound.lifecycle_revision = "rebound-generation".into();
    assert!(production
        .compare_and_set_session_work_admission(
            &record.id,
            Some(&current.lifecycle_revision),
            &rebound,
        )
        .expect("rebind generation"));

    let error = production
        .assert_session_work_admission_generation(
            &record.id,
            &current.session_key,
            &expected_revision,
        )
        .expect_err("stale terminal result must be rejected");
    assert!(error.contains("generation changed"));

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn background_result_receipt_is_hidden_from_production_transcript_projection() {
    let root = temp_root("work-admission-receipt-projection");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("agent");
    let session_key = format!("agent:{}:main", record.id);
    let lifecycle_revision = production
        .capture_session_work_admission(&record.id, &session_key, "background-work-1")
        .expect("capture generation");
    let result = production
        .commit_session_work_result(
            &record.id,
            &session_key,
            &lifecycle_revision,
            "background-work-1",
            "background-idem-1",
            "background output",
            1,
        )
        .expect("commit receipt");
    assert!(matches!(
        result,
        mahayana_host_runtime::extensions::session::agent_db::SessionWorkResultCommitOutcome::Committed(_)
    ));
    let visible = production
        .read_agent_transcript_entries(&record.id)
        .expect("read visible transcript");
    assert!(visible.iter().all(|entry| {
        !matches!(
            entry.get("kind").and_then(serde_json::Value::as_str),
            Some("background-result-receipt" | "outgoing-media-settlement")
        )
    }));

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_facade_exposes_frozen_mint_profile_summary_transcript_and_stat_surfaces() {
    let root = temp_root("frozen-facade");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));

    let minted = store
        .mint_agent(|agent_id| Ok(agent_id.to_string()))
        .expect("mint agent id");
    assert!(!minted.is_empty());
    assert!(!agents.join(&minted).exists());

    let record = store
        .create_session(None, "user", None)
        .expect("materialized agent");
    store
        .write_agent_profile_file(
            &record.id,
            &AgentProfileUpdate {
                name: "  Facade Agent  ".into(),
                description: "  profile description  ".into(),
                title: Some("  Builder  ".into()),
                avatar_shape: Some("  rounded  ".into()),
                avatar_color: Some("  violet  ".into()),
            },
        )
        .expect("profile write");

    production
        .append_agent_transcript_entries(
            &record.id,
            &[serde_json::json!({
                "id":"facade-entry",
                "kind":"message",
                "role":"user",
                "content":"hello facade",
                "timestampMs":10
            })],
        )
        .expect("transcript append");

    let open = store
        .open_session(&record.id)
        .expect("open session")
        .expect("prepared session");
    assert_eq!(
        store
            .get_transcript_entries(&open)
            .expect("session transcript")
            .len(),
        1
    );

    let open_summary = store
        .summarize_open_session(&open)
        .expect("open summary")
        .expect("summary");
    assert_eq!(open_summary.name, "Facade Agent");
    assert!(open_summary.is_active);
    assert_eq!(open_summary.title, "Builder");

    let inactive_summary = store
        .summarize_session(&open, None)
        .expect("session summary")
        .expect("summary");
    assert!(!inactive_summary.is_active);
    assert_eq!(inactive_summary.description, "profile description");

    let stat = store.stat_open_db(&open).expect("db stat");
    assert!(stat.size > 0);
    assert!(stat.mtime_ms >= 0.0);

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_facade_delegates_interaction_state_memory_capacity_and_time_zone() {
    let root = temp_root("interaction-state");
    let agents = root.join("agents");
    let production = Arc::new(
        ProductionSessionWorkers::with_agents_root_and_user_time_zone_resolver(
            &agents,
            500,
            Arc::new(|| Some("America/Los_Angeles".to_string())),
        ),
    );
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");

    assert_eq!(store.list_agent_ids().expect("agent ids"), vec![record.id.clone()]);
    assert!(!store.is_agent_cap_reached().expect("cap"));
    assert_eq!(store.get_user_time_zone().as_deref(), Some("America/Los_Angeles"));

    let memory = store.create_memory_store(agents.join(&record.id));
    assert_eq!(memory.get_location(), agents.join(&record.id).join("memory"));
    assert!(!store.agent_has_content(agents.join(&record.id)));
    memory
        .add_memory("Remember this", 1, MemoryKind::Profile)
        .expect("memory write")
        .expect("memory record");
    assert!(store.agent_has_content(agents.join(&record.id)));

    let prepared = store
        .open_session(&record.id)
        .expect("open")
        .expect("prepared");
    assert!(store.get_session_outline(&prepared).expect("session outline").is_empty());
    assert!(store.get_agent_outline(&record.id).expect("agent outline").is_empty());
    assert_eq!(
        store
            .get_agent_transcript_entries(&record.id)
            .expect("transcript alias"),
        store
            .read_agent_transcript_entries(&record.id)
            .expect("transcript owner")
    );
    assert!(store
        .mark_session_activity(&prepared, 10.0)
        .expect("session activity"));
    assert!(store
        .mark_session_viewed(&prepared, 20.0, false)
        .expect("session viewed"));
    assert!(store
        .mark_session_viewed_now(&prepared, 25.0, false)
        .expect("session viewed now"));

    let awaiting = AwaitingUserResponse {
        tab_id: "tab-a".into(),
        reason: "approval".into(),
        since: 30.0,
    };
    assert!(store
        .set_awaiting_user_response(&record.id, Some(&awaiting))
        .expect("set awaiting"));
    assert!(!store
        .set_awaiting_user_response_for_tab(
            &record.id,
            "tab-b",
            None,
            Some(40.0),
        )
        .expect("wrong tab"));
    assert!(store
        .set_awaiting_user_response_for_tab(
            &record.id,
            "tab-a",
            None,
            Some(40.0),
        )
        .expect("matching tab"));

    production
        .set_agent_memory_prompt_snapshot(
            &record.id,
            &serde_json::json!({"render":"snapshot","compactionEpoch":1}),
        )
        .expect("set memory snapshot");
    assert!(store
        .clear_agent_memory_prompt_snapshot(&record.id)
        .expect("clear memory snapshot"));

    production
        .append_agent_transcript_entries(
            &record.id,
            &[
                serde_json::json!({
                    "id":"approval",
                    "kind":"send-message",
                    "timestampMs":50,
                    "message":{
                        "type":"auto-review-approval",
                        "approval":{"requestId":"req-approval","status":"pending"}
                    }
                }),
                serde_json::json!({
                    "id":"permission",
                    "kind":"send-message",
                    "timestampMs":60,
                    "message":{
                        "type":"local-tool-permission",
                        "ask":{"requestId":"req-permission","status":"pending"}
                    }
                }),
            ],
        )
        .expect("pending transcript");
    assert_eq!(
        store
            .expire_pending_auto_review_approvals(
                &record.id,
                Some("req-approval"),
            )
            .expect("expire approval"),
        vec!["req-approval".to_string()]
    );
    assert_eq!(
        store
            .expire_pending_local_tool_permission_asks(
                &record.id,
                Some("req-permission"),
                None,
            )
            .expect("expire permission"),
        vec!["req-permission".to_string()]
    );

    store
        .ensure_conversation_capacity_for_turn(&prepared)
        .expect("default capacity policy");

    assert!(store
        .store_connector_credential(&record.id, "slack", "token", "secret")
        .expect("credential"));
    let configs = store.list_channel_configs(&record.id).expect("channel configs");
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].platform, "slack");
    assert_eq!(configs[0].token, "secret");

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_facade_delegates_automation_workflow_and_channel_stores() {
    let root = temp_root("stores");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");

    let automations = store
        .create_agent_automation(
            &record.id,
            &AutomationSpec {
                name: "Daily review".into(),
                prompt: "Review the inbox".into(),
                trigger: serde_json::json!({"type":"cron","schedule":"0 9 * * *"}),
                is_enabled: Some(true),
            },
        )
        .expect("create automation");
    assert_eq!(automations.len(), 1);
    let automation_id = automations[0].id.clone();
    assert!(automations[0].is_enabled);

    let automations = store
        .set_agent_automation_enabled(&record.id, &automation_id, false)
        .expect("disable automation");
    assert!(!automations[0].is_enabled);
    let automations = store
        .update_agent_automation(
            &record.id,
            &automation_id,
            &AutomationSpec {
                name: "Daily review updated".into(),
                prompt: "Review everything".into(),
                trigger: serde_json::json!({"type":"cron","schedule":"30 9 * * *"}),
                is_enabled: Some(true),
            },
        )
        .expect("update automation");
    assert_eq!(automations[0].name, "Daily review updated");
    assert!(automations[0].is_enabled);
    assert_eq!(
        store.list_agent_automations(&record.id).expect("list automations").len(),
        1
    );
    assert!(store
        .remove_agent_automation(&record.id, &automation_id)
        .expect("remove automation")
        .is_empty());

    let workflows = store
        .create_agent_workflow(
            &record.id,
            &WorkflowSpec {
                name: "Research".into(),
                description: "Research workflow".into(),
                body: "Read sources and summarize.".into(),
                trigger: None,
                source_ref: None,
            },
        )
        .expect("create workflow");
    assert_eq!(workflows.len(), 1);
    let workflow_id = workflows[0].id.clone();
    assert_eq!(
        store
            .get_agent_workflow(&record.id, &workflow_id)
            .expect("get workflow")
            .expect("workflow")
            .name,
        "Research"
    );
    let workflows = store
        .update_agent_workflow(
            &record.id,
            &workflow_id,
            &WorkflowSpec {
                name: "Research updated".into(),
                description: "Updated".into(),
                body: "Read more sources and summarize.".into(),
                trigger: None,
                source_ref: None,
            },
        )
        .expect("update workflow");
    assert_eq!(workflows[0].name, "Research updated");
    let workflows = store
        .set_agent_workflow_enabled(&record.id, &workflow_id, false)
        .expect("disable workflow");
    assert!(!workflows[0].is_enabled_for_agent);
    assert_eq!(
        store.list_agent_workflows(&record.id).expect("list workflows").len(),
        1
    );
    assert!(store
        .remove_agent_workflow(&record.id, &workflow_id)
        .expect("remove workflow")
        .is_empty());

    let channel_store = store.open_channel_store(&record.id).expect("channel store");
    assert!(channel_store.get_location().ends_with("channels"));

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_facade_delegates_transcript_and_channel_owners_without_duplication() {
    let root = temp_root("delegation");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", None)
        .expect("agent");

    production
        .append_agent_transcript_entries(
            &record.id,
            &[
                serde_json::json!({
                    "id":"entry-1",
                    "kind":"message",
                    "role":"user",
                    "content":"hello",
                    "timestampMs":10
                }),
                serde_json::json!({
                    "id":"entry-2",
                    "kind":"tool-call",
                    "name":"search",
                    "timestampMs":20
                }),
                serde_json::json!({
                    "id":"entry-3",
                    "kind":"message",
                    "role":"assistant",
                    "content":"reply",
                    "timestampMs":30
                }),
                serde_json::json!({
                    "id":"entry-4",
                    "kind":"message",
                    "role":"assistant",
                    "content":"branch reply",
                    "replyTo":"entry-1",
                    "branched":true,
                    "timestampMs":40
                })
            ],
        )
        .expect("transcript");
    assert_eq!(
        store
            .read_agent_transcript_entries(&record.id)
            .expect("read transcript")
            .len(),
        4
    );

    let page = store
        .read_agent_transcript_page(
            &record.id,
            TranscriptPageQuery {
                before_seq: None,
                since_ms: Some(0),
                until_ms: 100,
                limit: 10,
            },
        )
        .expect("page");
    assert_eq!(
        page.entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-1"]
    );

    let window = store
        .read_agent_transcript_window(
            &record.id,
            TranscriptWindowQuery {
                before_seq: None,
                limit: 10,
            },
        )
        .expect("window");
    assert_eq!(
        window
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-1", "entry-3"]
    );

    let tail = store
        .read_agent_transcript_tail(
            &record.id,
            TranscriptWindowQuery {
                before_seq: None,
                limit: 10,
            },
        )
        .expect("tail");
    assert_eq!(
        tail.entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-1", "entry-2", "entry-3", "entry-4"]
    );

    let thread = store
        .read_agent_thread(&record.id, "entry-1")
        .expect("thread");
    assert_eq!(
        thread
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-1", "entry-4"]
    );

    assert!(store
        .store_connector_credential(&record.id, "slack", "token", "secret")
        .expect("connector"));
    assert_eq!(
        store.list_agent_channels(&record.id).expect("channels").len(),
        1
    );
    assert!(store.disconnect_channel(&record.id, "slack").expect("disconnect"));

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn session_facade_owns_workflow_import_local_skill_port_and_cross_agent_automation_projection() {
    let root = temp_root("facade-imports");
    let agents = root.join("agents");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(&agents, 500));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let first = store.create_session(None, "user", None).expect("first agent");
    let second = store.create_session(None, "user", None).expect("second agent");

    let markdown = store
        .import_agent_workflow_markdown(
            &first.id,
            "---\nname: Imported markdown\n---\nDo the imported thing.\n",
            None,
        )
        .expect("markdown import");
    assert_eq!(markdown.result.imported.len(), 1);
    assert!(markdown.result.skipped.is_empty());

    let live = store
        .import_agent_workflow_source(
            &first.id,
            "https://example.test/SKILL.md",
            Some("Live skill"),
        )
        .expect("live source import");
    assert_eq!(live.result.imported.len(), 1);
    assert!(live.result.skipped.is_empty());
    assert!(live
        .workflows
        .iter()
        .any(|workflow| workflow.source_ref.as_deref() == Some("https://example.test/SKILL.md")));

    let home = root.join("home");
    let cwd = root.join("cwd");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(cwd.join(".cursor/rules")).expect("rules dir");
    fs::write(home.join("CLAUDE.md"), "# Claude memory\n").expect("home skill");
    fs::write(cwd.join(".cursor/rules/review.md"), "# Review\n").expect("rule");
    let local = store
        .port_agent_local_skills_from(&first.id, &home, &cwd)
        .expect("local skill port");
    assert_eq!(local.result.imported.len(), 2);
    assert!(local.result.skipped.is_empty());

    for (agent_id, name) in [
        (&first.id, "First daily"),
        (&second.id, "Second daily"),
    ] {
        store
            .create_agent_automation(
                agent_id,
                &AutomationSpec {
                    name: name.into(),
                    prompt: "Do work".into(),
                    trigger: serde_json::json!({"type":"cron","schedule":"0 9 * * *"}),
                    is_enabled: Some(true),
                },
            )
            .expect("automation");
    }
    let all = store.list_all_automations().expect("all automations");
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|row| row.agent_id == first.id));
    assert!(all.iter().any(|row| row.agent_id == second.id));
    assert_eq!(
        store
            .list_all_automation_definitions()
            .expect("all definitions")
            .len(),
        2
    );

    store.close_worker_pool();
    let _ = fs::remove_dir_all(root);
}
