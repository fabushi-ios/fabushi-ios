use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::agent_db::{
    CompactionCommit, CompactionCommitOutcome, PendingMemoryMutation,
    PendingProfileMutation, ProfileFileSnapshot,
    ProviderReviewContinuationOutcome, SandAgentDb,
    SandAgentDbOptions, SessionProviderReview, SessionWorkAdmissionRecord,
    SessionWorkResultCommitOutcome,
};
use mahayana_host_runtime::extensions::session::agent_db_recovery::DbRecoveryOptions;
use mahayana_host_runtime::extensions::session::agent_db_transcript_pages::{
    TranscriptPageQuery, TranscriptWindowQuery,
};
use mahayana_host_runtime::extensions::session::agent_db_serde::{
    AwaitingUserResponse, SandProfile, SpendGuardState,
};
use mahayana_host_runtime::extensions::memory::memory_service::{
    MemoryKind, MemoryRecoveryRecord, MemoryRecord,
};
use mahayana_host_runtime::storage::store_db::live_db_handle_count;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-agent-db-owner-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn owner_registers_live_handle_notifies_shipping_mutations_and_releases_on_close() {
    let root = temp_root("lifecycle");
    let agent_dir = root.join("agent-owner");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");

    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    assert_eq!(live_db_handle_count(&db_path), 1);

    let metadata_hits = Arc::new(AtomicUsize::new(0));
    let profile_hits = Arc::new(AtomicUsize::new(0));
    let awaiting_hits = Arc::new(AtomicUsize::new(0));

    let _metadata_sub = owner.subscribe_metadata(
        "latestRootBlobId",
        {
            let hits = Arc::clone(&metadata_hits);
            Arc::new(move || {
                hits.fetch_add(1, Ordering::SeqCst);
            })
        },
    );
    let _profile_sub = owner.subscribe_sand_profile({
        let hits = Arc::clone(&profile_hits);
        Arc::new(move || {
            hits.fetch_add(1, Ordering::SeqCst);
        })
    });
    let _awaiting_sub = owner.subscribe_awaiting_user_response({
        let hits = Arc::clone(&awaiting_hits);
        Arc::new(move || {
            hits.fetch_add(1, Ordering::SeqCst);
        })
    });

    let initial_revision = owner
        .get_metadata("lifecycleRevision")
        .expect("initial revision")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("initial revision value");

    assert!(owner
        .compare_and_set_latest_root_blob_id(&[], &[0xaa, 0xbb])
        .expect("root cas"));
    let root_revision = owner
        .get_metadata("lifecycleRevision")
        .expect("root revision")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("root revision value");
    assert_ne!(root_revision, initial_revision);
    assert!(owner
        .set_sand_profile(&SandProfile {
            description: "profile".into(),
            avatar_path: Some("/tmp/avatar.png".into()),
        })
        .expect("profile"));
    assert!(owner
        .set_awaiting_user_response(Some(&AwaitingUserResponse {
            tab_id: "tab-a".into(),
            reason: "approval".into(),
            since: 1.0,
        }))
        .expect("awaiting"));

    assert_eq!(metadata_hits.load(Ordering::SeqCst), 1);
    assert_eq!(profile_hits.load(Ordering::SeqCst), 1);
    assert_eq!(awaiting_hits.load(Ordering::SeqCst), 1);

    assert!(owner.clear_conversation().expect("clear conversation"));
    let cleared_revision = owner
        .get_metadata("lifecycleRevision")
        .expect("cleared revision")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("cleared revision value");
    assert_ne!(cleared_revision, root_revision);
    assert!(owner
        .get_awaiting_user_response()
        .expect("awaiting after clear")
        .is_none());
    assert_eq!(metadata_hits.load(Ordering::SeqCst), 2);
    assert_eq!(profile_hits.load(Ordering::SeqCst), 1);
    assert_eq!(awaiting_hits.load(Ordering::SeqCst), 2);

    owner.close(true);
    assert!(owner.is_closed());
    assert_eq!(live_db_handle_count(&db_path), 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn owner_drops_busy_writes_and_reports_the_operation() {
    let root = temp_root("busy");
    let agent_dir = root.join("agent-busy");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");

    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    let callback_observed = Arc::clone(&observed);
    let owner = SandAgentDb::open_with_options(
        &db_path,
        SandAgentDbOptions {
            recovery: DbRecoveryOptions {
                busy_timeout_ms: 20,
                ..DbRecoveryOptions::default()
            },
            on_busy_error: Some(Arc::new(move |operation, error| {
                callback_observed
                    .lock()
                    .expect("busy events")
                    .push(format!("{operation}:{error}"));
            })),
        },
    )
    .expect("owner");

    let locker = rusqlite::Connection::open(&db_path).expect("locker");
    locker.execute_batch("BEGIN IMMEDIATE").expect("write lock");
    assert!(!owner.write_kv("busy-test", "value").expect("busy write"));
    locker.execute_batch("ROLLBACK").expect("unlock");
    drop(locker);

    let observed = observed.lock().expect("busy observations");
    assert_eq!(observed.len(), 1);
    assert!(observed[0].starts_with("writeKv:busy-test:"));
    drop(observed);

    owner.close(false);
    assert_eq!(live_db_handle_count(&db_path), 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_work_admission_record_migrates_and_compare_and_sets_generation() {
    let root = temp_root("work-admission-record");
    let agent_dir = root.join("agent-work");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    let first = SessionWorkAdmissionRecord {
        session_key: "agent:agent-work:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: Some("work-1".into()),
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &first)
        .expect("create admission record"));
    assert!(!owner
        .compare_and_set_session_work_admission(None, &first)
        .expect("create-only conflict"));
    owner
        .append_transcript_entries(&[serde_json::json!({
            "id": "permission-1",
            "kind": "send-message",
            "sessionKey": first.session_key.clone(),
            "message": {"type": "local-tool-permission", "ask": {"status": "pending"}},
        })])
        .expect("seed permission entry");
    let updated = owner
        .update_transcript_entry_if_session_admitted(
            &first.session_key,
            "permission-1",
            &serde_json::json!({
                "id": "permission-1",
                "kind": "send-message",
                "message": {"type": "local-tool-permission", "ask": {"status": "allowed"}},
            }),
        )
        .expect("session-admitted update")
        .expect("updated permission entry");
    assert_eq!(updated["message"]["ask"]["status"], "allowed");

    owner
        .append_transcript_entries(&[serde_json::json!({
            "id": "human-pending",
            "kind": "message",
            "role": "user",
            "content": "pending remote Human",
            "sessionKey": first.session_key.clone(),
            "clientNonce": "human-nonce-1",
        })])
        .expect("seed session-bound Human entry");
    let settled_human = owner
        .settle_transcript_entry_identity_if_session_admitted(
            &first.session_key,
            "human-pending",
            &serde_json::json!({
                "id": "human-remote-1",
                "kind": "message",
                "role": "user",
                "content": "pending remote Human",
                "sessionKey": first.session_key.clone(),
                "remoteMessageId": "remote-1",
            }),
        )
        .expect("session-admitted Human identity settlement")
        .expect("settled Human entry");
    assert_eq!(settled_human["id"], "human-remote-1");

    owner
        .append_transcript_entries(&[serde_json::json!({
            "id": "legacy-permission-1",
            "kind": "send-message",
            "message": {"type": "local-tool-permission", "ask": {"status": "pending"}},
        })])
        .expect("seed unbound permission entry");
    let provenance_error = owner
        .update_transcript_entry_if_session_admitted(
            &first.session_key,
            "legacy-permission-1",
            &serde_json::json!({
                "id": "legacy-permission-1",
                "kind": "send-message",
                "message": {"type": "local-tool-permission", "ask": {"status": "allowed"}},
            }),
        )
        .expect_err("session admission must not update an unbound entry");
    assert!(provenance_error
        .to_string()
        .contains("target transcript entry provenance"));

    let rebound = SessionWorkAdmissionRecord {
        session_key: "agent:other-agent:main".into(),
        ..first.clone()
    };
    assert!(!owner
        .compare_and_set_session_work_admission(Some("revision-1"), &rebound)
        .expect("identity-rebinding CAS is rejected"));
    assert_eq!(
        owner
            .read_session_work_admission()
            .expect("read admission after rejected rebind")
            .expect("admission remains present")
            .session_key,
        first.session_key
    );

    let mut archived = first.clone();
    archived.lifecycle_revision = "revision-2".into();
    archived.archived_at_ms = Some(42.0);
    archived.archived_by = Some("human-1".into());
    archived.archive_reason = Some("manual review".into());
    assert!(owner
        .compare_and_set_session_work_admission(Some("revision-1"), &archived)
        .expect("guarded archive transition"));
    assert!(!owner
        .compare_and_set_session_work_admission(Some("revision-1"), &first)
        .expect("stale transition"));
    let archived_update = owner
        .update_transcript_entry_if_session_admitted(
            &first.session_key,
            "permission-1",
            &serde_json::json!({
                "id": "permission-1",
                "kind": "send-message",
                "message": {"type": "local-tool-permission", "ask": {"status": "denied"}},
            }),
        )
        .expect_err("archived session update must reject");
    assert!(archived_update
        .to_string()
        .contains("session is not admitted for transcript update"));
    assert_eq!(
        owner
            .read_session_work_admission()
            .expect("read admission record")
            .expect("record")
            .lifecycle_revision,
        "revision-2"
    );
    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_scoped_kv_write_rejects_stale_generation_atomically() {
    let root = temp_root("session-kv-generation");
    let agent_dir = root.join("agent-session-kv");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    let active = SessionWorkAdmissionRecord {
        session_key: "agent:agent-session-kv:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &active)
        .expect("create admission"));
    assert!(owner
        .write_session_kv_if_generation(
            "sessionCommunicationPolicy",
            "{\"send\":\"always\"}",
            "session-1",
            "revision-1",
        )
        .expect("write current policy"));
    assert_eq!(
        owner
            .read_kv("sessionCommunicationPolicy")
            .expect("read policy")
            .as_deref(),
        Some("{\"send\":\"always\"}")
    );

    let mut next = active.clone();
    next.lifecycle_revision = "revision-2".into();
    assert!(owner
        .compare_and_set_session_work_admission(Some("revision-1"), &next)
        .expect("advance generation"));
    assert!(!owner
        .write_session_kv_if_generation(
            "sessionCommunicationPolicy",
            "{\"send\":\"never\"}",
            "session-1",
            "revision-1",
        )
        .expect("stale policy write"));
    assert_eq!(
        owner
            .read_kv("sessionCommunicationPolicy")
            .expect("read retained policy")
            .as_deref(),
        Some("{\"send\":\"always\"}")
    );

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_work_admission_deletion_fence_migrates_and_persists() {
    let root = temp_root("work-admission-deletion-fence");
    let agent_dir = root.join("agent-delete-fence");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");

    owner
        .write_kv(
            "sessionWorkAdmission",
            &serde_json::json!({
                "session_key": "agent:agent-delete-fence:main",
                "session_id": "session-1",
                "lifecycle_revision": "revision-1"
            })
            .to_string(),
        )
        .expect("write legacy admission record");
    let migrated = owner
        .read_session_work_admission()
        .expect("read migrated admission")
        .expect("migrated admission exists");
    assert!(!migrated.deletion_pending);

    let mut fenced = migrated.clone();
    fenced.deletion_pending = true;
    assert!(owner
        .transition_session_deletion_pending(Some("revision-1"), false, &fenced)
        .expect("publish deletion fence"));
    assert!(owner
        .read_session_work_admission()
        .expect("read deletion fence")
        .expect("fenced admission exists")
        .deletion_pending);
    let mut ordinary_clear = fenced.clone();
    ordinary_clear.deletion_pending = false;
    assert!(!owner
        .compare_and_set_session_work_admission(Some("revision-1"), &ordinary_clear)
        .expect("ordinary CAS cannot clear deletion fence"));

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn owner_metadata_memory_writes_reject_deletion_fence_at_transaction_edge() {
    let root = temp_root("metadata-deletion-fence");
    let agent_dir = root.join("agent-metadata-deletion-fence");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-metadata-deletion-fence:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("create admission"));
    assert!(owner
        .record_request_id("request-1", 1.0, None, None)
        .expect("record request"));
    assert!(owner
        .record_episode_turn(&mahayana_host_runtime::extensions::session::agent_db_serde::EpisodeTurn {
            ts: 1.0,
            user: "before".into(),
            agent: "fence".into(),
        })
        .expect("record episode"));
    assert!(owner
        .set_metadata("lastActivityAt", serde_json::json!(1.0))
        .expect("record metadata"));

    let mut fenced = admission.clone();
    fenced.deletion_pending = true;
    assert!(owner
        .transition_session_deletion_pending(Some("revision-1"), false, &fenced)
        .expect("publish deletion fence"));
    assert!(!owner
        .record_request_id("request-2", 2.0, None, None)
        .expect("fenced request write"));
    assert!(!owner
        .record_episode_turn(&mahayana_host_runtime::extensions::session::agent_db_serde::EpisodeTurn {
            ts: 2.0,
            user: "after".into(),
            agent: "fence".into(),
        })
        .expect("fenced episode write"));
    assert!(!owner
        .clear_pending_episode_turns()
        .expect("fenced episode clear"));
    assert!(!owner
        .clear_transient_state()
        .expect("fenced transient clear"));
    assert!(!owner
        .set_metadata("lastActivityAt", serde_json::json!(2.0))
        .expect("fenced metadata write"));
    assert_eq!(
        owner
            .append_transcript_entries(&[serde_json::json!({
                "id": "after-fence",
                "kind": "message",
                "role": "assistant",
                "content": "rejected",
            })])
            .expect("fenced transcript append"),
        0
    );
    assert!(owner
        .update_transcript_entry(
            "checkpoint-user",
            &serde_json::json!({
                "id": "checkpoint-user",
                "kind": "message",
                "role": "user",
                "content": "rejected update",
            }),
        )
        .expect("fenced transcript update")
        .is_none());
    assert!(!owner
        .delete_transcript_entry("checkpoint-user")
        .expect("fenced transcript delete"));
    assert!(owner
        .settle_transcript_entry_identity(
            "checkpoint-user",
            &serde_json::json!({
                "id": "checkpoint-user-settled",
                "kind": "message",
                "role": "user",
                "content": "rejected settlement",
            }),
        )
        .expect("fenced transcript identity settlement")
        .is_none());
    assert!(!owner
        .clear_conversation()
        .expect("fenced conversation clear"));
    assert!(!owner
        .set_awaiting_user_response(Some(
            &mahayana_host_runtime::extensions::session::agent_db_serde::AwaitingUserResponse {
                tab_id: "fenced-tab".into(),
                reason: "fenced".into(),
                since: 2.0,
            },
        ))
        .expect("fenced KV write"));
    assert_eq!(
        owner
            .get_metadata("lastActivityAt")
            .expect("metadata history")
            .and_then(|value| value.as_f64()),
        Some(1.0)
    );
    assert_eq!(owner.get_request_ids().expect("request history").len(), 1);
    assert_eq!(owner.get_pending_episode_turns().expect("episode history").len(), 1);

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn owner_rejects_checkpoint_confirmation_when_deletion_fence_is_present() {
    let root = temp_root("checkpoint-deletion-fence");
    let agent_dir = root.join("agent-checkpoint-deletion-fence");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    assert!(owner
        .set_metadata("latestRootBlobId", serde_json::json!("aa"))
        .expect("seed checkpoint root"));
    owner
        .append_transcript_entries(&[serde_json::json!({
            "id": "checkpoint-user",
            "kind": "message",
            "role": "user",
            "content": "checkpoint",
        })])
        .expect("seed user entry");
    let revision = owner
        .get_metadata("lifecycleRevision")
        .expect("revision")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("revision value");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-checkpoint-deletion-fence:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: revision.clone(),
        deletion_pending: true,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("publish deletion fence"));

    assert!(owner
        .commit_checkpoint_root_and_confirm_user_message(
            &[0xaa],
            &[0xbb],
            "checkpoint-user",
        )
        .expect("fenced checkpoint outcome")
        .is_none());
    assert!(!owner
        .compare_and_set_latest_root_blob_id(&[0xaa], &[0xcc])
        .expect("fenced root CAS"));
    assert_eq!(
        owner
            .get_metadata("latestRootBlobId")
            .expect("checkpoint root")
            .and_then(|value| value.as_str().map(ToOwned::to_owned)),
        Some("aa".into())
    );
    assert!(!owner
        .get_transcript_entries()
        .expect("transcript")
        .iter()
        .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some("checkpoint-user"))
        .and_then(|entry| entry.get("confirmed"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false));

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn provider_review_owner_is_generation_bound_idempotent_and_conflict_safe() {
    let root = temp_root("provider-review-owner");
    let agent_dir = root.join("agent-review");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-review:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("create admission"));
    let review = SessionProviderReview {
        id: "review-1".into(),
        session_id: "session-1".into(),
        run_id: "run-1".into(),
        provider: "openai".into(),
        model: "model-1".into(),
        runtime_id: "openclaw".into(),
        api: Some("openai-chatgpt-responses".into()),
        native_thread_id: None,
        native_turn_id: None,
        explanation: "provider-authored refusal".into(),
        continuation_message: Some("Review and continue".into()),
        error_type: Some("policy".into()),
    };
    let mut deletion_fenced = admission.clone();
    deletion_fenced.deletion_pending = true;
    assert!(owner
        .transition_session_deletion_pending(Some("revision-1"), false, &deletion_fenced)
        .expect("publish deletion fence"));
    assert!(owner
        .record_session_provider_review("revision-1", &review)
        .is_err());
    assert!(owner
        .clear_session_deletion_pending("revision-1", &admission)
        .expect("clear deletion fence"));
    assert_eq!(owner
        .record_session_provider_review("revision-1", &review)
        .expect("record review"), review);
    let repeated = SessionProviderReview { id: "review-2".into(), ..review.clone() };
    assert_eq!(owner
        .record_session_provider_review("revision-1", &repeated)
        .expect("idempotent review"), review);
    let conflicting = SessionProviderReview { explanation: "different".into(), ..review.clone() };
    assert!(owner
        .record_session_provider_review("revision-1", &conflicting)
        .is_err());
    let mut accepted = admission.clone();
    accepted.admitted_work_id = Some("run-2".into());
    accepted.provider_review_id = Some(review.id.clone());
    accepted.provider_review_phase = Some("accepted".into());
    accepted.provider_review_continuation_run_id = Some("run-2".into());
    accepted.provider_review = Some(review.clone());
    assert!(owner
        .compare_and_set_session_work_admission(Some("revision-1"), &accepted)
        .expect("mark review accepted"));
    let continuation_refusal = SessionProviderReview {
        id: "review-3".into(),
        run_id: "run-2".into(),
        explanation: "provider-authored follow-up refusal".into(),
        ..review.clone()
    };
    assert_eq!(
        owner
            .record_session_provider_review("revision-1", &continuation_refusal)
            .expect("replace after accepted continuation"),
        continuation_refusal
    );
    let replaced = owner
        .read_session_work_admission()
        .expect("read replaced review admission")
        .expect("replaced review admission exists");
    assert_eq!(replaced.provider_review_phase.as_deref(), Some("pending"));
    assert!(replaced.admitted_work_id.is_none());
    assert!(owner
        .record_session_provider_review("stale", &review)
        .is_err());
    assert_eq!(owner
        .read_session_provider_review()
        .expect("read review")
        .expect("review exists")
        .id, "review-3");
    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn provider_review_continuation_settlement_is_run_and_generation_bound() {
    let root = temp_root("provider-review-settlement");
    let agent_dir = root.join("agent-review-settlement");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let owner = SandAgentDb::open(agent_dir.join("store.db"), 50).expect("owner");
    let review = SessionProviderReview {
        id: "review-settle".into(),
        session_id: "session-settle".into(),
        run_id: "refusal-run".into(),
        provider: "openai".into(),
        model: "model-1".into(),
        runtime_id: "openclaw".into(),
        api: Some("openai-chatgpt-responses".into()),
        native_thread_id: None,
        native_turn_id: None,
        explanation: "provider-authored refusal".into(),
        continuation_message: Some("continue".into()),
        error_type: None,
    };
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-review-settlement:main".into(),
        session_id: "session-settle".into(),
        lifecycle_revision: "revision-settle".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: Some("continuation-run".into()),
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: Some(review.id.clone()),
        provider_review_phase: Some("accepted".into()),
        provider_review_continuation_run_id: Some("continuation-run".into()),
        provider_review: Some(review),
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("create accepted review"));
    let mut deletion_fenced = admission.clone();
    deletion_fenced.deletion_pending = true;
    assert!(owner
        .transition_session_deletion_pending(
            Some("revision-settle"),
            false,
            &deletion_fenced,
        )
        .expect("publish deletion fence"));
    assert!(!owner
        .release_provider_review_continuation("revision-settle", "continuation-run")
        .expect("fenced release is explicit"));
    assert!(!owner
        .settle_provider_review_continuation(
            "revision-settle",
            "continuation-run",
            ProviderReviewContinuationOutcome::Completed,
        )
        .expect("fenced settlement is explicit"));
    assert!(owner
        .clear_session_deletion_pending("revision-settle", &admission)
        .expect("clear deletion fence"));
    assert!(!owner
        .settle_provider_review_continuation(
            "stale-revision",
            "continuation-run",
            ProviderReviewContinuationOutcome::Completed,
        )
        .expect("stale settlement is explicit"));
    let mut rebound_work = admission.clone();
    rebound_work.admitted_work_id = Some("different-run".into());
    assert!(owner
        .compare_and_set_session_work_admission(Some("revision-settle"), &rebound_work)
        .expect("publish rebound continuation work"));
    assert!(!owner
        .release_provider_review_continuation("revision-settle", "continuation-run")
        .expect("rebound release is explicit"));
    assert!(!owner
        .settle_provider_review_continuation(
            "revision-settle",
            "continuation-run",
            ProviderReviewContinuationOutcome::Completed,
        )
        .expect("rebound settlement is explicit"));
    assert!(owner
        .compare_and_set_session_work_admission(Some("revision-settle"), &admission)
        .expect("restore matching continuation work"));
    assert!(owner
        .settle_provider_review_continuation(
            "revision-settle",
            "continuation-run",
            ProviderReviewContinuationOutcome::Completed,
        )
        .expect("complete continuation"));
    let settled = owner
        .read_session_work_admission()
        .expect("read settled admission")
        .expect("settled admission exists");
    assert_eq!(settled.provider_review_phase.as_deref(), Some("completed"));
    assert!(settled.provider_review.is_none());
    assert!(settled.admitted_work_id.is_none());
    assert!(!owner
        .settle_provider_review_continuation(
            "revision-settle",
            "continuation-run",
            ProviderReviewContinuationOutcome::Completed,
        )
        .expect("duplicate settlement is ignored"));
    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_archive_transition_is_guarded_idempotent_and_blocks_active_work() {
    let root = temp_root("work-admission-archive-transition");
    let agent_dir = root.join("agent-work");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    let record = SessionWorkAdmissionRecord {
        session_key: "agent:agent-work:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &record)
        .expect("create admission record"));

    let archived = owner
        .transition_session_work_archive(
            "agent:agent-work:main",
            "session-1",
            Some("revision-1"),
            true,
            Some("gateway-operator"),
            Some("manual review"),
            42.0,
        )
        .expect("archive transition");
    let archived_revision = archived.lifecycle_revision.clone();
    assert_ne!(archived_revision, "revision-1");
    assert_eq!(archived.archived_at_ms, Some(42.0));
    assert_eq!(archived.archive_reason.as_deref(), Some("manual review"));
    let identity_error = owner
        .transition_session_work_archive(
            "agent:other-agent:main",
            "session-1",
            Some("revision-1"),
            true,
            Some("other-operator"),
            Some("wrong identity"),
            43.0,
        )
        .expect_err("archive must reject a mismatched canonical session key");
    assert!(identity_error.to_string().contains("identity changed"));
    assert_eq!(
        owner
            .transition_session_work_archive(
                "agent:agent-work:main",
                "session-1",
                Some(&archived_revision),
                true,
                Some("other-operator"),
                Some("different reason"),
                43.0,
            )
            .expect("idempotent archive")
            .archived_at_ms,
        Some(42.0)
    );
    let restored = owner
        .transition_session_work_archive(
            "agent:agent-work:main",
            "session-1",
            Some(&archived_revision),
            false,
            None,
            None,
            44.0,
        )
        .expect("restore transition");
    let restored_revision = restored.lifecycle_revision.clone();
    assert_ne!(restored_revision, archived_revision);
    assert!(restored.archived_at_ms.is_none());
    let stale_after_restore = owner
        .transition_session_work_archive(
            "agent:agent-work:main",
            "session-1",
            Some("revision-1"),
            true,
            Some("stale-worker"),
            None,
            44.5,
        )
        .expect_err("pre-archive worker must not resume after restore");
    assert!(stale_after_restore.to_string().contains("lifecycle changed"));

    let mut active = restored;
    active.admitted_work_id = Some("work-1".into());
    assert!(owner
        .compare_and_set_session_work_admission(Some(&restored_revision), &active)
        .expect("admit active work"));
    let error = owner
        .transition_session_work_archive(
            "agent:agent-work:main",
            "session-1",
            Some(&restored_revision),
            true,
            Some("gateway-operator"),
            None,
            45.0,
        )
        .expect_err("active work must block archive");
    assert!(error.to_string().contains("admitted work"));

    active.admitted_work_id = None;
    active.admitted_media_work_id = Some("send-media:tool-1".into());
    assert!(owner
        .compare_and_set_session_work_admission(Some(&restored_revision), &active)
        .expect("admit pending media"));
    let media_error = owner
        .transition_session_work_archive(
            "agent:agent-work:main",
            "session-1",
            Some(&restored_revision),
            true,
            Some("gateway-operator"),
            None,
            46.0,
        )
        .expect_err("pending media must block archive");
    assert!(media_error.to_string().contains("pending outgoing media"));
    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pending_memory_journal_blocks_archive_revision_transition() {
    let root = temp_root("pending-memory-archive");
    let agent_dir = root.join("agent-work");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let owner = SandAgentDb::open(agent_dir.join("store.db"), 50).expect("owner");
    let record = SessionWorkAdmissionRecord {
        session_key: "agent:agent-work:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &record)
        .expect("create admission"));
    let mutation = PendingMemoryMutation {
        mutation_id: "memory-remove:test".into(),
        operation: "remove".into(),
        session_key: record.session_key.clone(),
        session_id: record.session_id.clone(),
        lifecycle_revision: record.lifecycle_revision.clone(),
        records: vec![MemoryRecoveryRecord {
            record: MemoryRecord {
                id: "memory-id".into(),
                content: "fact".into(),
                created_at: 1,
                kind: MemoryKind::Profile,
            },
            origin: mahayana_host_runtime::extensions::memory::memory_service::MemoryOrigin::Explicit,
            tombstoned: false,
        }],
        prior_snapshot: None,
    };
    assert!(owner
        .prepare_pending_memory_mutation(&mutation)
        .expect("prepare journal"));
    let error = owner
        .transition_session_work_archive(
            &record.session_key,
            &record.session_id,
            Some(&record.lifecycle_revision),
            true,
            Some("operator"),
            Some("test"),
            1.0,
        )
        .expect_err("archive must not bypass pending memory journal");
    assert!(error.to_string().contains("pending memory mutation"));
    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_journal_commit_clears_snapshot_and_journal_together() {
    let root = temp_root("pending-memory-commit");
    let agent_dir = root.join("agent-work");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let owner = SandAgentDb::open(agent_dir.join("store.db"), 50).expect("owner");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-work:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("create admission"));
    owner
        .set_memory_prompt_snapshot(&serde_json::json!({"old": true}))
        .expect("set snapshot");
    let mutation = PendingMemoryMutation {
        mutation_id: "memory-clear:test".into(),
        operation: "clear".into(),
        session_key: admission.session_key.clone(),
        session_id: admission.session_id.clone(),
        lifecycle_revision: admission.lifecycle_revision.clone(),
        records: Vec::new(),
        prior_snapshot: None,
    };
    assert!(owner
        .prepare_pending_memory_mutation(&mutation)
        .expect("prepare journal"));
    assert!(owner
        .commit_pending_memory_mutation(&mutation)
        .expect("commit journal"));
    assert!(owner
        .read_pending_memory_mutation()
        .expect("read journal")
        .is_none());
    assert!(owner
        .get_memory_prompt_snapshot()
        .expect("read snapshot")
        .is_none());
    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn profile_mutation_commit_updates_legacy_pointer_and_clears_journal() {
    let root = temp_root("pending-profile-commit");
    let agent_dir = root.join("agent-work");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let owner = SandAgentDb::open(agent_dir.join("store.db"), 50).expect("owner");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-work:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("create admission"));
    let previous_profile = SandProfile {
        description: "description".into(),
        avatar_path: Some("avatar.png".into()),
    };
    owner
        .set_sand_profile(&previous_profile)
        .expect("set previous profile");
    let mutation = PendingProfileMutation {
        mutation_id: "avatar-remove:test".into(),
        operation: "avatar-remove".into(),
        session_key: admission.session_key.clone(),
        session_id: admission.session_id.clone(),
        lifecycle_revision: admission.lifecycle_revision.clone(),
        before_files: vec![ProfileFileSnapshot {
            filename: "avatar.png".into(),
            contents: Some(vec![1, 2, 3]),
        }],
        after_files: Vec::new(),
        previous_profile: previous_profile.clone(),
        next_profile: Some(SandProfile {
            description: previous_profile.description.clone(),
            avatar_path: None,
        }),
    };
    assert!(owner
        .prepare_pending_profile_mutation(&mutation)
        .expect("prepare profile journal"));
    let mut sealed = mutation.clone();
    sealed.after_files = vec![ProfileFileSnapshot {
        filename: "avatar.png".into(),
        contents: None,
    }];
    assert!(owner
        .seal_pending_profile_mutation(&sealed)
        .expect("seal profile journal"));
    let archive_error = owner
        .transition_session_work_archive(
            &admission.session_key,
            &admission.session_id,
            Some(&admission.lifecycle_revision),
            true,
            Some("operator"),
            Some("pending profile mutation"),
            1.0,
        )
        .expect_err("archive must not bypass profile journal");
    assert!(archive_error.to_string().contains("pending profile mutation"));
    assert!(owner
        .commit_pending_profile_mutation(&sealed)
        .expect("commit profile journal"));
    assert_eq!(
        owner.get_sand_profile().expect("read profile").avatar_path,
        None
    );
    assert!(owner
        .read_pending_profile_mutation()
        .expect("read profile journal")
        .is_none());
    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn admitted_transcript_append_is_rejected_after_archive_without_partial_write() {
    let root = temp_root("work-admission-transcript-append");
    let agent_dir = root.join("agent-work");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    let session_key = "agent:agent-work:main";
    let record = SessionWorkAdmissionRecord {
        session_key: session_key.into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &record)
        .expect("create admission record"));

    let first_entry = serde_json::json!({
        "id": "entry-before-archive",
        "kind": "message",
        "role": "user",
        "content": "accepted",
        "sessionKey": session_key
    });
    assert_eq!(
        owner
            .append_transcript_entries_if_session_admitted(session_key, &[first_entry])
            .expect("append while admitted"),
        1
    );

    let unbound_entry = serde_json::json!({
        "id": "entry-without-provenance",
        "kind": "message",
        "role": "user",
        "content": "must be rejected",
    });
    let provenance_error = owner
        .append_transcript_entries_if_session_admitted(session_key, &[unbound_entry])
        .expect_err("session-bound append must require entry provenance");
    assert!(provenance_error
        .to_string()
        .contains("matching entry provenance"));

    owner
        .transition_session_work_archive(
            "agent:agent-work:main",
            "session-1",
            Some("revision-1"),
            true,
            Some("test"),
            Some("archive"),
            42.0,
        )
        .expect("archive transition");

    let rejected_entry = serde_json::json!({
        "id": "entry-after-archive",
        "kind": "message",
        "role": "user",
        "content": "must not be written",
        "sessionKey": session_key
    });
    let error = owner
        .append_transcript_entries_if_session_admitted(session_key, &[rejected_entry])
        .expect_err("archived session must reject transcript append");
    assert!(error
        .to_string()
        .contains("session is not admitted for transcript append"));
    let ids = owner
        .get_transcript_entries()
        .expect("read transcript")
        .into_iter()
        .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str).map(str::to_owned))
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["entry-before-archive"]);

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn media_settlement_appends_and_releases_in_one_owner_transaction() {
    let root = temp_root("media-settlement");
    let agent_dir = root.join("agent-media-settlement");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-media-settlement:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: Some("send-media:tool-1".into()),
        admitted_media_paths: vec!["/tmp/send-media-tool-1".into()],
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("create admission"));

    let stale_identity = owner
        .append_transcript_entries_and_release_media(
            &[serde_json::json!({
                "id": "send-stale-identity",
                "kind": "send-message",
                "content": "must not settle",
            })],
            "agent:agent-media-settlement:rebound",
            "revision-1",
            "send-media:tool-1",
        )
        .expect_err("rebound session identity must reject media settlement");
    assert!(stale_identity
        .to_string()
        .contains("media settlement generation or identity is stale"));

    let malformed_entry = owner
        .append_transcript_entries_and_release_media(
            &[serde_json::json!({
                "content": "must not settle without an id or kind"
            })],
            "agent:agent-media-settlement:main",
            "revision-1",
            "send-media:tool-1",
        )
        .expect_err("malformed media transcript must retain custody");
    assert!(malformed_entry
        .to_string()
        .contains("requires transcript entry identities"));
    assert_eq!(
        owner
            .read_session_work_admission()
            .expect("read admission after malformed settlement")
            .expect("admission after malformed settlement")
            .admitted_media_work_id
            .as_deref(),
        Some("send-media:tool-1")
    );
    assert!(owner
        .get_transcript_entry_by_id("outgoing-media-settlement:send-media:tool-1")
        .expect("read malformed settlement receipt")
        .is_none());

    assert_eq!(
        owner
            .append_transcript_entries_and_release_media(
                &[serde_json::json!({
                    "id": "send-1",
                    "kind": "send-message",
                    "content": "owned media"
                })],
                "agent:agent-media-settlement:main",
                "revision-1",
                "send-media:tool-1",
            )
            .expect("atomic media settlement"),
        1
    );
    assert_eq!(
        owner
            .read_session_work_admission()
            .expect("read admission")
            .expect("admission")
            .admitted_media_work_id,
        None
    );
    assert!(owner
        .read_session_work_admission()
        .expect("read admission paths")
        .expect("admission paths")
        .admitted_media_paths
        .is_empty());
    assert!(owner
        .get_transcript_entry_by_id("send-1")
        .expect("read transcript")
        .is_some());
    let receipt = owner
        .get_transcript_entry_by_id("outgoing-media-settlement:send-media:tool-1")
        .expect("read media settlement receipt")
        .expect("media settlement receipt");
    assert_eq!(
        receipt.get("kind").and_then(serde_json::Value::as_str),
        Some("outgoing-media-settlement")
    );
    assert_eq!(
        receipt.get("sessionKey").and_then(serde_json::Value::as_str),
        Some("agent:agent-media-settlement:main")
    );
    assert_eq!(
        owner
            .append_transcript_entries_and_release_media(
                &[serde_json::json!({
                    "id": "send-1",
                    "kind": "send-message",
                    "content": "owned media"
                })],
                "agent:agent-media-settlement:main",
                "revision-1",
                "send-media:tool-1",
            )
            .expect("identical media settlement replay"),
        0
    );
    let rebound_replay = owner
        .append_transcript_entries_and_release_media(
            &[serde_json::json!({
                "id": "send-1",
                "kind": "send-message",
                "content": "owned media"
            })],
            "agent:other-agent:main",
            "revision-1",
            "send-media:tool-1",
        )
        .expect_err("media settlement replay must retain session identity");
    assert!(rebound_replay.to_string().contains("receipt is rebound"));
    let conflicting_replay = owner
        .append_transcript_entries_and_release_media(
            &[serde_json::json!({
                "id": "send-1",
                "kind": "send-message",
                "content": "different content"
            })],
            "agent:agent-media-settlement:main",
            "revision-1",
            "send-media:tool-1",
        )
        .expect_err("media settlement replay must retain transcript identity");
    assert!(conflicting_replay
        .to_string()
        .contains("conflicts with the committed transcript"));

    let mut fenced = owner
        .read_session_work_admission()
        .expect("read admission after media settlement")
        .expect("admission after media settlement");
    fenced.deletion_pending = true;
    assert!(owner
        .transition_session_deletion_pending(Some("revision-1"), false, &fenced)
        .expect("publish deletion fence"));
    let fenced_replay = owner
        .append_transcript_entries_and_release_media(
            &[serde_json::json!({
                "id": "send-1",
                "kind": "send-message",
                "content": "owned media"
            })],
            "agent:agent-media-settlement:main",
            "revision-1",
            "send-media:tool-1",
        )
        .expect_err("media settlement replay must recheck the live admission fence");
    assert!(fenced_replay
        .to_string()
        .contains("media settlement replay is stale or session is not admitted"));

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn background_result_receipt_is_idempotent_and_conflicts_fail_closed() {
    let root = temp_root("work-admission-receipt");
    let agent_dir = root.join("agent-work");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-work:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: Some("work-1".into()),
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("create admission"));

    let committed = owner
        .commit_session_work_result(
            &admission.session_key,
            &admission.lifecycle_revision,
            "work-1",
            "idem-1",
            "background output",
            2,
        )
        .expect("commit receipt");
    assert!(matches!(committed, SessionWorkResultCommitOutcome::Committed(_)));

    let duplicate = owner
        .commit_session_work_result(
            &admission.session_key,
            &admission.lifecycle_revision,
            "work-1",
            "idem-1",
            "background output",
            2,
        )
        .expect("duplicate receipt");
    assert!(matches!(
        duplicate,
        SessionWorkResultCommitOutcome::AlreadyCommitted(_)
    ));

    let conflict = owner
        .commit_session_work_result(
            &admission.session_key,
            &admission.lifecycle_revision,
            "work-1",
            "idem-1",
            "different output",
            2,
        )
        .expect("conflicting receipt");
    assert_eq!(conflict, SessionWorkResultCommitOutcome::ConflictingResult);

    let mut fenced = owner
        .read_session_work_admission()
        .expect("read admission after background receipt")
        .expect("admission after background receipt");
    fenced.deletion_pending = true;
    assert!(owner
        .transition_session_deletion_pending(Some("revision-1"), false, &fenced)
        .expect("publish deletion fence"));
    let fenced_replay = owner
        .commit_session_work_result(
            &admission.session_key,
            &admission.lifecycle_revision,
            "work-1",
            "idem-1",
            "background output",
            2,
        )
        .expect("fenced background replay");
    assert_eq!(fenced_replay, SessionWorkResultCommitOutcome::RejectedStale);

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn stale_background_result_cas_rolls_back_receipt_insert() {
    let root = temp_root("work-admission-stale-receipt");
    let agent_dir = root.join("agent-work");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-work:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: Some("work-stale".into()),
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("create admission"));

    // Force the owner CAS to lose its lifecycle race after the receipt INSERT
    // but before the compare-and-set. The surrounding transaction must roll
    // back both the trigger update and the receipt insert.
    let trigger = rusqlite::Connection::open(&db_path).expect("trigger connection");
    trigger
        .execute_batch(
            "CREATE TRIGGER stale_work_admission_after_receipt
             AFTER INSERT ON transcript_entries
             WHEN NEW.id = 'background-result:idem-stale'
             BEGIN
               UPDATE kv
               SET value = replace(value, 'revision-1', 'revision-2')
               WHERE key = 'sessionWorkAdmission';
             END;",
        )
        .expect("install stale-race trigger");

    let outcome = owner
        .commit_session_work_result(
            &admission.session_key,
            &admission.lifecycle_revision,
            "work-stale",
            "idem-stale",
            "stale output",
            0,
        )
        .expect("stale result outcome");
    assert_eq!(outcome, SessionWorkResultCommitOutcome::RejectedStale);

    assert!(owner
        .get_transcript_entries()
        .expect("transcript remains readable")
        .iter()
        .all(|entry| entry.get("id").and_then(serde_json::Value::as_str)
            != Some("background-result:idem-stale")));
    let retained = owner
        .read_session_work_admission()
        .expect("read admission")
        .expect("admission remains");
    assert_eq!(retained.lifecycle_revision, "revision-1");
    assert_eq!(retained.admitted_work_id.as_deref(), Some("work-stale"));

    trigger
        .execute_batch("DROP TRIGGER stale_work_admission_after_receipt")
        .expect("remove stale-race trigger");
    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn spend_guard_card_owner_transaction_rejects_durable_deletion_fence() {
    let root = temp_root("spend-guard-deletion-fence");
    let agent_dir = root.join("agent-work");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-work:main".into(),
        session_id: "session-1".into(),
        lifecycle_revision: "revision-1".into(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("create admission"));
    let entry = serde_json::json!({
        "id": "spend-card",
        "kind": "send-message",
        "sessionKey": admission.session_key.clone(),
        "message": {"type": "widget"},
    });
    let state = SpendGuardState {
        nudged_at_ms: Some(42.0),
        card_entry_ids: vec!["spend-card".into()],
        ..SpendGuardState::default()
    };
    assert!(owner
        .append_spend_guard_card_if_session_admitted(
            &admission.session_key,
            &entry,
            &state,
            42.0,
        )
        .expect("spend card commit"));
    assert_eq!(owner.get_automation_spend_guard_state().expect("state"), state);
    assert_eq!(owner.get_transcript_entries().expect("entries").len(), 1);

    let mut fenced = admission.clone();
    fenced.deletion_pending = true;
    assert!(owner
        .transition_session_deletion_pending(
            Some(&admission.lifecycle_revision),
            false,
            &fenced,
        )
        .expect("publish deletion fence"));
    let error = owner
        .append_spend_guard_card_if_session_admitted(
            &admission.session_key,
            &serde_json::json!({
                "id": "spend-card-after-delete",
                "kind": "send-message",
                "sessionKey": admission.session_key.clone(),
                "message": {"type": "widget"},
            }),
            &SpendGuardState::default(),
            43.0,
        )
        .expect_err("durable deletion fence must reject spend card transaction");
    assert!(error.to_string().contains("session is not admitted"));
    assert!(owner
        .get_transcript_entries()
        .expect("entries remain readable")
        .iter()
        .all(|entry| entry.get("id").and_then(serde_json::Value::as_str)
            != Some("spend-card-after-delete")));
    owner.close(false);
    let _ = fs::remove_dir_all(root);
}


#[test]
fn owner_routes_unread_request_episode_snapshot_and_spend_guard_mutations() {
    let root = temp_root("state");
    let agent_dir = root.join("agent-state");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");

    assert!(owner.mark_activity(10.0).expect("activity"));
    let unread = owner.get_unread_state().expect("unread after activity");
    assert_eq!(unread.last_activity_at, 10.0);
    assert_eq!(unread.unread_count, 1.0);

    assert!(owner.mark_viewed(12.0, false).expect("viewed"));
    assert!(owner.mark_unread(20.0).expect("manual unread"));
    let unread = owner.get_unread_state().expect("manual unread state");
    assert!(unread.is_manually_unread);
    assert_eq!(unread.unread_count, 1.0);
    assert!(owner.mark_read(21.0).expect("read"));
    assert!(!owner.get_unread_state().expect("read state").is_manually_unread);

    assert!(owner.set_introduction_pending(true).expect("intro"));
    assert!(owner.get_introduction_pending().expect("intro state"));

    let spend = mahayana_host_runtime::extensions::session::agent_db_serde::SpendGuardState {
        nudged_at_ms: Some(42.0),
        snoozed_until_ms: None,
        opted_out: true,
        card_entry_ids: vec!["card-a".into()],
        paused_automation_ids: vec!["auto-a".into()],
    };
    assert!(owner
        .set_automation_spend_guard_state(&spend)
        .expect("spend guard"));
    assert_eq!(
        owner.get_automation_spend_guard_state().expect("spend state"),
        spend
    );

    assert!(owner
        .record_request_id(" request-1 ", 30.0, Some(" prompt "), Some("turn"))
        .expect("request"));
    assert!(!owner
        .record_request_id("request-1", 31.0, None, None)
        .expect("duplicate request"));
    let requests = owner.get_request_ids().expect("requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].id, "request-1");
    assert_eq!(requests[0].prompt.as_deref(), Some("prompt"));

    assert!(owner
        .record_episode_turn(&mahayana_host_runtime::extensions::session::agent_db_serde::EpisodeTurn {
            ts: 40.0,
            user: "question".into(),
            agent: "answer".into(),
        })
        .expect("episode"));
    assert_eq!(owner.get_pending_episode_turns().expect("episodes").len(), 1);

    assert!(owner
        .set_memory_prompt_snapshot(&serde_json::json!({
            "render": "memory",
            "compactionEpoch": 2
        }))
        .expect("memory snapshot"));
    assert_eq!(
        owner
            .get_memory_prompt_snapshot()
            .expect("memory snapshot read")
            .map(|snapshot| snapshot.render),
        Some("memory".into())
    );

    assert!(owner
        .set_agent_profile_prompt_snapshot(&serde_json::json!({"name":"Agent"}))
        .expect("profile snapshot"));
    assert_eq!(
        owner
            .get_agent_profile_prompt_snapshot()
            .expect("profile snapshot read"),
        Some(serde_json::json!({"name":"Agent"}))
    );

    assert!(owner.clear_transient_state().expect("clear transient"));
    assert_eq!(owner.get_unread_state().expect("cleared unread").unread_count, 0.0);
    assert!(owner.get_request_ids().expect("cleared requests").is_empty());
    assert!(owner.get_pending_episode_turns().expect("cleared episodes").is_empty());
    assert!(owner
        .get_memory_prompt_snapshot()
        .expect("cleared memory snapshot")
        .is_none());
    assert!(owner
        .get_agent_profile_prompt_snapshot()
        .expect("cleared profile snapshot")
        .is_none());

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}


#[test]
fn owner_routes_partner_origin_purpose_and_transcript_mutations() {
    let root = temp_root("transcript-owner");
    let agent_dir = root.join("agent-transcript");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");

    assert_eq!(owner.get_agent_origin().expect("default origin"), "user");
    assert!(owner.set_agent_origin("dev").expect("set origin"));
    assert_eq!(owner.get_agent_origin().expect("dev origin"), "dev");

    assert!(owner.set_agent_purpose("research").expect("purpose"));
    assert_eq!(
        owner.get_agent_purpose().expect("purpose read").as_deref(),
        Some("research")
    );
    assert!(owner.clear_agent_purpose().expect("clear purpose"));
    assert!(owner.get_agent_purpose().expect("purpose cleared").is_none());

    assert!(!owner
        .add_conversation_partner("agent-transcript")
        .expect("reject self"));
    assert!(owner
        .add_conversation_partner(" partner-b ")
        .expect("partner"));
    assert!(!owner
        .add_conversation_partner("partner-b")
        .expect("partner dedupe"));
    assert_eq!(
        owner.get_conversation_partner_ids().expect("partners"),
        vec!["partner-b".to_string()]
    );

    assert_eq!(
        owner
            .append_transcript_entries(&[
                serde_json::json!({
                    "id":"entry-1",
                    "kind":"message",
                    "role":"user",
                    "content":"hello"
                }),
                serde_json::json!({
                    "id":"entry-2",
                    "kind":"notice",
                    "text":"notice"
                }),
            ])
            .expect("append"),
        2
    );
    assert_eq!(
        owner
            .append_transcript_entries(&[
                serde_json::json!({
                    "id":"entry-1",
                    "kind":"message",
                    "role":"user",
                    "content":"duplicate"
                }),
            ])
            .expect("duplicate append"),
        0
    );
    assert_eq!(
        owner
            .update_transcript_entry(
                "entry-1",
                &serde_json::json!({
                    "id":"entry-1",
                    "kind":"message",
                    "role":"user",
                    "content":"edited"
                }),
            )
            .expect("update")
            .and_then(|entry| entry.get("content").cloned()),
        Some(serde_json::Value::String("edited".into()))
    );
    assert!(owner
        .update_transcript_entry(
            "missing",
            &serde_json::json!({
                "id":"missing",
                "kind":"message",
                "role":"user",
                "content":"missing"
            }),
        )
        .expect("missing update")
        .is_none());
    assert!(owner.delete_transcript_entry("entry-2").expect("delete"));
    assert!(owner.delete_transcript_entry("entry-2").expect("idempotent committed delete"));
    assert_eq!(
        owner
            .append_transcript_entries(&[
                serde_json::json!({
                    "id":"branch-1",
                    "kind":"message",
                    "role":"assistant",
                    "content":"branch",
                    "replyTo":"entry-1",
                    "branched":true
                }),
            ])
            .expect("append branch"),
        1
    );

    assert_eq!(
        owner
            .get_transcript_entries()
            .expect("owner transcript entries")
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-1", "branch-1"]
    );
    let page = owner
        .get_transcript_page(TranscriptPageQuery {
            before_seq: None,
            since_ms: None,
            until_ms: i64::MAX,
            limit: 10,
        })
        .expect("owner transcript page");
    assert_eq!(
        page.entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-1"]
    );
    let window = owner
        .get_transcript_window(TranscriptWindowQuery {
            before_seq: None,
            limit: 10,
        })
        .expect("owner transcript window");
    assert_eq!(
        window
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-1"]
    );
    assert_eq!(window.thread_counts.get("entry-1"), Some(&1));
    let tail = owner
        .get_transcript_tail(TranscriptWindowQuery {
            before_seq: None,
            limit: 10,
        })
        .expect("owner transcript tail");
    assert_eq!(
        tail.entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-1", "branch-1"]
    );
    assert_eq!(
        owner
            .get_transcript_entry_by_id("entry-1")
            .expect("entry by id")
            .and_then(|entry| entry.get("content").cloned()),
        Some(serde_json::Value::String("edited".into()))
    );
    assert_eq!(
        owner
            .get_branched_entries()
            .expect("branched entries")
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["branch-1"]
    );
    assert_eq!(
        owner
            .get_thread_entries("entry-1")
            .expect("thread entries")
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-1", "branch-1"]
    );

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn owner_commits_guarded_compaction_and_replays_from_cut_point() {
    let root = temp_root("compaction");
    let agent_dir = root.join("agent-compaction");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");

    owner
        .append_transcript_entries(&[
            serde_json::json!({"id":"old", "kind":"message", "role":"user", "content":"old"}),
            serde_json::json!({"id":"kept", "kind":"message", "role":"user", "content":"kept"}),
        ])
        .expect("history");
    let expected_revision = owner
        .get_metadata("lifecycleRevision")
        .expect("revision")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("revision value");
    let commit = CompactionCommit {
        entry_id: "compaction-1".into(),
        expected_transcript_tail_id: "kept".into(),
        first_kept_entry_id: Some("kept".into()),
        summary: "summary".into(),
        tokens_before: 123,
        details: Some(serde_json::json!({"files": ["a.txt"]})),
        source_turn_id: "turn-1".into(),
        expected_lifecycle_revision: expected_revision.clone(),
    };

    let outcome = owner.commit_compaction(&commit).expect("commit");
    let committed = match outcome {
        CompactionCommitOutcome::Committed(entry) => entry,
        other => panic!("expected committed outcome, got {other:?}"),
    };
    assert_eq!(committed.get("kind").and_then(serde_json::Value::as_str), Some("compaction"));
    assert_ne!(
        owner
            .get_metadata("lifecycleRevision")
            .expect("next revision")
            .and_then(|value| value.as_str().map(ToOwned::to_owned)),
        Some(expected_revision.clone())
    );
    let replay = owner
        .get_compacted_transcript_entries()
        .expect("replay projection");
    assert_eq!(
        replay
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["compaction-1", "kept"]
    );

    assert!(matches!(
        owner.commit_compaction(&commit).expect("duplicate commit"),
        CompactionCommitOutcome::AlreadyCommitted(_)
    ));
    let mut normalized_duplicate = commit.clone();
    normalized_duplicate.summary = "  summary  ".into();
    assert!(matches!(
        owner
            .commit_compaction(&normalized_duplicate)
            .expect("normalized duplicate commit"),
        CompactionCommitOutcome::AlreadyCommitted(_)
    ));
    let mut conflict = commit.clone();
    conflict.summary = "different".into();
    assert!(matches!(
        owner.commit_compaction(&conflict),
        Err(mahayana_host_runtime::extensions::session::agent_db::AgentDbProjectionError::CompactionConflict(_))
    ));

    let mut stale = commit.clone();
    stale.entry_id = "compaction-stale".into();
    assert!(matches!(
        owner.commit_compaction(&stale).expect("stale commit"),
        CompactionCommitOutcome::RejectedStale
    ));
    let current_revision = owner
        .get_metadata("lifecycleRevision")
        .expect("current compaction revision")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("current compaction revision value");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-compaction:main".into(),
        session_id: "session-compaction".into(),
        lifecycle_revision: current_revision.clone(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("create compaction admission"));
    let mut fenced = admission.clone();
    fenced.deletion_pending = true;
    assert!(owner
        .transition_session_deletion_pending(Some(&current_revision), false, &fenced)
        .expect("publish compaction deletion fence"));
    assert!(matches!(
        owner.commit_compaction(&commit).expect("fenced compaction replay"),
        CompactionCommitOutcome::RejectedStale
    ));
    let mut cleared = fenced;
    cleared.deletion_pending = false;
    assert!(owner
        .clear_session_deletion_pending(&current_revision, &cleared)
        .expect("clear compaction deletion fence"));

    let mut missing_cut = commit.clone();
    missing_cut.entry_id = "compaction-missing".into();
    missing_cut.expected_transcript_tail_id = "compaction-1".into();
    missing_cut.first_kept_entry_id = Some("missing".into());
    missing_cut.expected_lifecycle_revision = owner
        .get_metadata("lifecycleRevision")
        .expect("current revision")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("current revision value");
    assert!(matches!(
        owner.commit_compaction(&missing_cut),
        Err(mahayana_host_runtime::extensions::session::agent_db::AgentDbProjectionError::CompactionInvalid(_))
    ));

    let mut empty_summary = commit.clone();
    empty_summary.entry_id = "compaction-empty-summary".into();
    empty_summary.expected_lifecycle_revision = owner
        .get_metadata("lifecycleRevision")
        .expect("current revision")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("current revision value");
    empty_summary.summary = "  ".into();
    assert!(matches!(
        owner.commit_compaction(&empty_summary),
        Err(mahayana_host_runtime::extensions::session::agent_db::AgentDbProjectionError::CompactionInvalid(_))
    ));

    let mut oversized_summary = empty_summary;
    oversized_summary.entry_id = "compaction-oversized-summary".into();
    oversized_summary.summary = "x".repeat(16_001);
    assert!(matches!(
        owner.commit_compaction(&oversized_summary),
        Err(mahayana_host_runtime::extensions::session::agent_db::AgentDbProjectionError::CompactionInvalid(_))
    ));

    owner.close(true);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn owner_rejects_compaction_when_transcript_tail_changes_after_planning() {
    let root = temp_root("compaction-tail-fence");
    let agent_dir = root.join("agent-compaction-tail-fence");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");

    owner
        .append_transcript_entries(&[
            serde_json::json!({"id":"old", "kind":"message", "role":"user", "content":"old"}),
            serde_json::json!({"id":"tail-before", "kind":"message", "role":"user", "content":"planned"}),
        ])
        .expect("history");
    let expected_revision = owner
        .get_metadata("lifecycleRevision")
        .expect("revision")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("revision value");
    owner
        .append_transcript_entries(&[
            serde_json::json!({"id":"tail-after", "kind":"message", "role":"assistant", "content":"concurrent"}),
        ])
        .expect("concurrent append");

    let commit = CompactionCommit {
        entry_id: "compaction-tail-stale".into(),
        expected_transcript_tail_id: "tail-before".into(),
        first_kept_entry_id: Some("tail-before".into()),
        summary: "summary".into(),
        tokens_before: 123,
        details: None,
        source_turn_id: "turn-stale".into(),
        expected_lifecycle_revision: expected_revision,
    };
    assert!(matches!(
        owner.commit_compaction(&commit).expect("stale tail outcome"),
        CompactionCommitOutcome::RejectedStale
    ));
    assert!(owner
        .get_transcript_entries()
        .expect("transcript remains readable")
        .iter()
        .all(|entry| entry.get("kind").and_then(serde_json::Value::as_str) != Some("compaction")));

    owner.close(true);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn owner_rejects_compaction_when_deletion_fence_wins_inside_commit_transaction() {
    let root = temp_root("compaction-deletion-fence");
    let agent_dir = root.join("agent-compaction-deletion-fence");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");

    owner
        .append_transcript_entries(&[
            serde_json::json!({"id":"old", "kind":"message", "role":"user", "content":"old"}),
            serde_json::json!({"id":"tail", "kind":"message", "role":"user", "content":"planned"}),
        ])
        .expect("history");
    let expected_revision = owner
        .get_metadata("lifecycleRevision")
        .expect("revision")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("revision value");
    let admission = SessionWorkAdmissionRecord {
        session_key: "agent:agent-compaction-deletion-fence:main".into(),
        session_id: "agent-compaction-deletion-fence".into(),
        lifecycle_revision: expected_revision.clone(),
        deletion_pending: true,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    assert!(owner
        .compare_and_set_session_work_admission(None, &admission)
        .expect("publish deletion fence"));

    let commit = CompactionCommit {
        entry_id: "compaction-deletion-fenced".into(),
        expected_transcript_tail_id: "tail".into(),
        first_kept_entry_id: Some("tail".into()),
        summary: "summary".into(),
        tokens_before: 123,
        details: None,
        source_turn_id: "turn-deletion-fenced".into(),
        expected_lifecycle_revision: expected_revision,
    };
    assert!(matches!(
        owner.commit_compaction(&commit).expect("fenced compaction outcome"),
        CompactionCommitOutcome::RejectedStale
    ));
    assert!(owner
        .get_transcript_entries()
        .expect("transcript remains readable")
        .iter()
        .all(|entry| entry.get("kind").and_then(serde_json::Value::as_str) != Some("compaction")));

    owner.close(true);
    let _ = fs::remove_dir_all(root);
}


#[test]
fn owner_closes_generic_metadata_versions_and_legacy_blob_surface() {
    let root = temp_root("metadata-surface");
    let agent_dir = root.join("agent-metadata");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");

    assert_eq!(
        owner
            .get_metadata("agentId")
            .expect("agent id metadata")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .as_deref(),
        Some("agent-metadata")
    );
    let name_hits = Arc::new(AtomicUsize::new(0));
    let _name_sub = owner.subscribe_metadata("name", {
        let hits = Arc::clone(&name_hits);
        Arc::new(move || {
            hits.fetch_add(1, Ordering::SeqCst);
        })
    });
    assert!(owner
        .set_metadata("name", serde_json::Value::String("Renamed".into()))
        .expect("set name"));
    assert_eq!(name_hits.load(Ordering::SeqCst), 1);
    assert!(owner
        .set_metadata("name", serde_json::Value::String("Renamed".into()))
        .expect("idempotent set name"));
    assert_eq!(name_hits.load(Ordering::SeqCst), 1);

    let root_hits = Arc::new(AtomicUsize::new(0));
    let _root_sub = owner.subscribe_metadata("latestRootBlobId", {
        let hits = Arc::clone(&root_hits);
        Arc::new(move || {
            hits.fetch_add(1, Ordering::SeqCst);
        })
    });
    assert!(owner
        .compare_and_set_latest_root_blob_id(&[], &[0xaa, 0xbb])
        .expect("root cas"));
    assert!(!owner
        .compare_and_set_latest_root_blob_id(&[], &[0xcc])
        .expect("stale root cas"));
    assert_eq!(owner.get_latest_root_blob_id().expect("latest root"), vec![0xaa, 0xbb]);
    assert_eq!(root_hits.load(Ordering::SeqCst), 1);

    assert!(owner.set_hidden_entry_repair_version(2).expect("hidden version"));
    assert_eq!(owner.get_hidden_entry_repair_version().expect("hidden version read"), 2);
    assert!(owner.set_stale_root_cleanup_version(3).expect("stale version"));
    assert_eq!(owner.get_stale_root_cleanup_version().expect("stale version read"), 3);

    let legacy = rusqlite::Connection::open(&db_path).expect("legacy writer");
    legacy
        .execute(
            "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
            rusqlite::params!["aa", vec![1_u8, 2, 3]],
        )
        .expect("legacy blob");
    drop(legacy);
    assert!(owner.has_legacy_conversation_blobs().expect("legacy present"));
    assert!(owner
        .retire_legacy_conversation_blobs(4)
        .expect("retire legacy blobs"));
    assert!(!owner.has_legacy_conversation_blobs().expect("legacy retired"));
    assert_eq!(
        owner
            .get_legacy_blob_retirement_version()
            .expect("legacy version"),
        4
    );

    let snapshot = owner.serde_snapshot().expect("serde snapshot");
    assert!(snapshot.request_ids.is_empty());

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn owner_matches_frozen_main_transcript_and_missing_thread_semantics() {
    let root = temp_root("main-transcript");
    let agent_dir = root.join("agent-main");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let owner = SandAgentDb::open(&db_path, 50).expect("owner");

    for entry in [
        serde_json::json!({
            "id":"root",
            "kind":"message",
            "role":"user",
            "content":"root"
        }),
        serde_json::json!({
            "id":"branch",
            "kind":"message",
            "role":"assistant",
            "content":"branch",
            "replyTo":"root",
            "branched":true
        }),
        serde_json::json!({
            "id":"nested",
            "kind":"message",
            "role":"assistant",
            "content":"nested",
            "replyTo":"branch",
            "branched":true
        }),
        serde_json::json!({
            "id":"orphan",
            "kind":"message",
            "role":"assistant",
            "content":"orphan",
            "replyTo":"missing-root",
            "branched":true
        }),
    ] {
        assert!(owner.append_transcript_entry(&entry).expect("append entry"));
    }
    assert!(!owner
        .append_transcript_entry(&serde_json::json!({
            "id":"root",
            "kind":"message",
            "role":"user",
            "content":"duplicate"
        }))
        .expect("duplicate append"));

    assert_eq!(
        owner
            .get_main_transcript_entries()
            .expect("main transcript")
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["root", "orphan"]
    );
    assert_eq!(
        owner
            .get_thread_entries("root")
            .expect("root thread")
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["root", "branch", "nested"]
    );
    assert!(owner
        .get_thread_entries("missing-root")
        .expect("missing root thread")
        .is_empty());

    owner.close(false);
    let _ = fs::remove_dir_all(root);
}
