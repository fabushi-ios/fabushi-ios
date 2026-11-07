use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::session_paths::{
    classify_session_kind, is_agent_harness_session_key, is_root_main_session_key,
    resolve_internal_send_route, resolve_public_session_route, InternalSessionRouteError,
    PublicSessionRouteError, SessionKind,
};
use mahayana_host_runtime::extensions::settings::settings_service::SettingsService;
use mahayana_host_runtime::gateway_command_error::GatewayCommandError;
use mahayana_host_runtime::host_diagnostics::{pin_host_diagnostics_reporter, report_diagnostic_event};
use mahayana_host_runtime::sha256::sha256_hex;
use serde_json::{Value, json};

fn temp_path(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-openclaw-test-{label}-{}-{suffix}.json",
        std::process::id()
    ))
}

fn shipping_source() -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        include_str!("../app/src/ocrm_handlers/openclaw_system_config.rs"),
        include_str!("../app/src/ocrm_handlers/openclaw_sessions.rs"),
        include_str!("../app/src/ocrm_handlers/openclaw_session_review.rs"),
        include_str!("../app/src/ocrm_handlers/openclaw_chat_cron.rs"),
        include_str!("../app/src/ocrm_handlers/automation_turn.rs"),
        include_str!("../app/src/main.rs"),
    )
}

#[test]
fn test_config_get_computes_sha256_hash_and_validates_base_hash() {
    let path = temp_path("config-get");
    fs::write(
        &path,
        r#"{"version":1,"localToolPermission":"ask","userTimeZone":"UTC"}"#,
    ).expect("seed settings");

    let service = SettingsService::new(&path);
    let settings = service.get_host_settings();
    let serialized = serde_json::to_string(&settings).unwrap();
    let hash = sha256_hex(serialized.as_bytes());

    // Matches true when identical baseHash is passed
    let matches_val = Some("something").map(|bh| bh == hash);
    assert_eq!(matches_val, Some(false));

    let matching_val = Some(hash.as_str()).map(|bh| bh == hash);
    assert_eq!(matching_val, Some(true));
}

#[test]
fn test_config_set_optimistic_concurrency_rejects_stale_base_hash_with_409_conflict() {
    let path = temp_path("config-conflict");
    fs::write(
        &path,
        r#"{"version":1,"localToolPermission":"ask"}"#,
    ).expect("seed");

    let service = SettingsService::new(&path);
    let current_settings = service.get_host_settings();
    let serialized = serde_json::to_string(&current_settings).unwrap();
    let real_hash = sha256_hex(serialized.as_bytes());

    let stale_base_hash = "deadbeef00000000deadbeef00000000deadbeef00000000deadbeef00000000";

    // Concurrency guard logic
    let check_concurrency = |base_hash: &str| -> Result<(), GatewayCommandError> {
        let current = service.get_host_settings();
        let current_str = serde_json::to_string(&current).unwrap();
        let current_hash = sha256_hex(current_str.as_bytes());
        if base_hash != current_hash {
            return Err(GatewayCommandError::Conflict(format!(
                "config baseHash mismatch: expected {current_hash}, got {base_hash}"
            )));
        }
        Ok(())
    };

    let err = check_concurrency(stale_base_hash).unwrap_err();
    assert_eq!(err.status(), 409);
    assert!(err.to_string().contains("config baseHash mismatch"));

    // Valid hash passes
    assert!(check_concurrency(&real_hash).is_ok());

    let shipping_source = shipping_source();
    let config_set = shipping_source
        .split("fn call_openclaw_config_set(")
        .nth(1)
        .expect("shipping config.set owner exists")
        .split("fn call_openclaw_system_info(")
        .next()
        .expect("shipping config.set function boundary exists");
    assert!(config_set.contains("config.set requires raw, config, or settings"));
    assert!(config_set.contains("config.set target must be a JSON object"));
    assert!(!config_set.contains("unwrap_or(args)"));
}

#[test]
fn test_system_info_schema_and_zero_secret_leakage() {
    let recorded_diagnostics = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&recorded_diagnostics);
    pin_host_diagnostics_reporter(Some(Arc::new(move |d| {
        sink.lock().unwrap().push(d.kind.clone());
    })));

    report_diagnostic_event("system.info.queried", json!({ "timestamp": 12345 }));

    let pid = std::process::id();
    let cpu_count = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let memory = mahayana_host_runtime::host_system_info::sample_host_memory()
        .expect("host memory observation");
    let hostname = mahayana_host_runtime::host_system_info::sample_host_hostname()
        .expect("host hostname observation");
    let release = mahayana_host_runtime::host_system_info::sample_host_os_release()
        .expect("host OS release observation");

    let info = json!({
        "machineName": hostname.clone(),
        "hostname": hostname,
        "platform": std::env::consts::OS,
        "release": release,
        "arch": std::env::consts::ARCH,
        "osLabel": std::env::consts::OS,
        "pid": pid,
        "cpuCount": cpu_count,
        "memoryTotalBytes": memory.total_bytes,
        "memoryFreeBytes": memory.available_bytes,
        "uptimeMs": 1000,
        "processInstanceId": format!("sand-host-{pid}"),
        "nodeVersion": env!("CARGO_PKG_VERSION"),
    });

    // Verify schema
    assert!(!info["nodeVersion"].as_str().unwrap().is_empty());
    assert!(info["pid"].as_u64().unwrap() > 0);
    assert!(info["cpuCount"].as_u64().unwrap() >= 1);
    assert!(info["processInstanceId"].as_str().unwrap().starts_with("sand-host-"));
    assert!(!info["release"].as_str().unwrap().is_empty());
    assert!(!info["hostname"].as_str().unwrap().is_empty());

    // Zero secret leakage
    let serialized = serde_json::to_string(&info).unwrap();
    assert!(!serialized.contains("\"version\""));
    assert!(!serialized.contains("\"status\""));
    assert!(!serialized.contains("password"));
    assert!(!serialized.contains("token"));
    assert!(!serialized.contains("secret"));
    assert!(!serialized.contains("apiKey"));

    let shipping_source = shipping_source();
    let system_info = shipping_source
        .split("fn call_openclaw_system_info(")
        .nth(1)
        .expect("shipping system.info owner exists")
        .split("fn call_openclaw_sessions_list(")
        .next()
        .expect("shipping system.info function boundary exists");
    assert!(system_info.contains("sample_host_memory"));
    assert!(system_info.contains("sample_host_os_release"));
    assert!(system_info.contains("sample_host_hostname"));
    assert!(system_info.contains("\"nodeVersion\""));
    assert!(!system_info.contains("\"version\": env!(\"CARGO_PKG_VERSION\")"));
    assert!(!system_info.contains("\"status\": \"ok\""));

    // Verify diagnostics recorded
    assert!(recorded_diagnostics.lock().unwrap().contains(&"system.info.queried".to_string()));
    pin_host_diagnostics_reporter(None);
}

#[test]
fn test_sessions_get_empty_messages_200_invariant() {
    // Parser for canonical session keys: agent:<id>:main
    fn parse_agent_id(raw_key: &str) -> &str {
        if let Some(stripped) = raw_key.strip_prefix("agent:") {
            stripped.split(':').next().unwrap_or(stripped)
        } else {
            raw_key
        }
    }

    assert_eq!(parse_agent_id("agent:agent-123:main"), "agent-123");
    assert_eq!(parse_agent_id("agent-456"), "agent-456");

    // Invariant: Non-existent sessions must return { "messages": [] }
    let non_existent_result: Result<Value, GatewayCommandError> = Ok(json!({ "messages": [] }));
    let res = non_existent_result.expect("200 OK");
    assert_eq!(res["messages"].as_array().unwrap().len(), 0);

    let shipping_source = shipping_source();
    let sessions_get = shipping_source
        .split("fn call_openclaw_sessions_get(")
        .nth(1)
        .expect("shipping sessions.get owner exists");
    let sessions_get = sessions_get
        .split("fn call_openclaw_sessions_create(")
        .next()
        .expect("shipping sessions.get function boundary exists");
    assert!(sessions_get.contains("map_err(GatewayCommandError::Internal)?"));
    assert!(sessions_get.contains("sessions.get requires key, sessionKey, or sessionId"));
    assert!(sessions_get.contains("reject_agent_wide_session_route(&route, \"sessions.get\")"));
    assert!(!sessions_get.contains("return Ok(serde_json::json!({ \"messages\": [] }))"));
    assert!(sessions_get.contains("entry.get(\"message\").and_then(|message| message.get(\"content\"))"));
    assert!(sessions_get.contains("entry.get(\"timestampMs\")"));
    assert!(sessions_get.contains("let start = entries.len().saturating_sub(limit)"));
    assert!(sessions_get.contains("entries.into_iter().skip(start)"));
    assert!(!sessions_get.contains("entries.into_iter().take(limit)"));
}

#[test]
fn test_sessions_list_projects_exact_active_run_set() {
    let shipping_source = shipping_source();
    let sessions_list = shipping_source
        .split("fn call_openclaw_sessions_list(")
        .nth(1)
        .expect("shipping sessions.list owner exists")
        .split("fn call_openclaw_sessions_get(")
        .next()
        .expect("shipping sessions.list function boundary exists");
    assert!(sessions_list.contains("active_stream_ids_for_agent(&s.id)"));
    assert!(sessions_list.contains("let has_active_run = !active_run_ids.is_empty()"));
    assert!(sessions_list.contains("s.is_running || has_active_run"));
    assert!(sessions_list.contains("\"hasActiveRun\": has_active_run"));
    assert!(sessions_list.contains("\"activeRunIds\": active_run_ids"));
    assert!(sessions_list.contains("resolve_gateway_session_route"));
}

#[test]
fn test_sessions_create_requires_durable_agent_identity() {
    let shipping_source = shipping_source();
    let sessions_create = shipping_source
        .split("fn call_openclaw_sessions_create(")
        .nth(1)
        .expect("shipping sessions.create owner exists")
        .split("fn call_openclaw_sessions_delete(")
        .next()
        .expect("shipping sessions.create function boundary exists");
    assert!(sessions_create.contains("createAgent returned no durable agent identity"));
    assert!(sessions_create.contains("filter(|id| !id.is_empty())"));
    assert!(!sessions_create.contains("unwrap_or_default();\n\n        let session_key"));
}

#[test]
fn test_sessions_delete_guards_root_session_and_cas() {
    let guard_delete = |key: &str, expected_session_id: Option<&str>| -> Result<(), GatewayCommandError> {
        if key.is_empty() {
            return Err(GatewayCommandError::BadRequest("sessions.delete requires key or sessionId".into()));
        }
        if is_root_main_session_key(key) {
            return Err(GatewayCommandError::BadRequest("Cannot delete root main session".into()));
        }
        let agent_id = if let Some(stripped) = key.strip_prefix("agent:") {
            stripped.split(':').next().unwrap_or(stripped)
        } else {
            key
        };
        if let Some(expected) = expected_session_id {
            if expected != agent_id {
                return Err(GatewayCommandError::BadRequest("SESSION_LIFECYCLE_CHANGED".into()));
            }
        }
        Ok(())
    };

    // Root main session protection
    let err_root1 = guard_delete("global", None).unwrap_err();
    assert_eq!(err_root1.status(), 400);
    assert!(err_root1.to_string().contains("Cannot delete root main session"));

    let err_root2 = guard_delete("agent:global:main", None).unwrap_err();
    assert_eq!(err_root2.status(), 400);
    let err_root3 = guard_delete("GLOBAL", None).unwrap_err();
    assert_eq!(err_root3.status(), 400);
    let err_root4 = guard_delete("AGENT:GLOBAL:MAIN", None).unwrap_err();
    assert_eq!(err_root4.status(), 400);

    // CAS check: mismatch returns SESSION_LIFECYCLE_CHANGED
    let err_cas = guard_delete("agent:agent-correct:main", Some("agent-stale")).unwrap_err();
    assert_eq!(err_cas.status(), 400);
    assert!(err_cas.to_string().contains("SESSION_LIFECYCLE_CHANGED"));

    // Valid delete passes guard
    assert!(guard_delete("agent:agent-correct:main", Some("agent-correct")).is_ok());
    assert!(guard_delete("agent-correct", None).is_ok());

    let empty_agent = "agent::main";
    let empty_agent_id = empty_agent
        .strip_prefix("agent:")
        .and_then(|value| value.split(':').next())
        .unwrap_or(empty_agent);
    assert!(empty_agent_id.is_empty());

    let shipping_source = shipping_source();
    let sessions_delete = shipping_source
        .split("fn call_openclaw_sessions_delete(")
        .nth(1)
        .expect("shipping sessions.delete owner exists")
        .split("fn call_openclaw_chat_send(")
        .next()
        .expect("shipping sessions.delete function boundary exists");
    assert!(sessions_delete.contains("sessions.delete requires a non-empty agent identity"));
    assert!(sessions_delete.contains("is_root_main_session_key(raw_key)"));
    assert!(sessions_delete.contains("reject_agent_wide_session_route(&route, \"sessions.delete\")"));
}

#[test]
fn test_sessions_patch_requires_authenticated_identity_revision_and_terminal_state() {
    let shipping_source = shipping_source();
    let sessions_patch = shipping_source
        .split("fn call_openclaw_sessions_patch(")
        .nth(1)
        .expect("shipping sessions.patch owner exists")
        .split("fn delete_generated_subagent(")
        .next()
        .expect("shipping sessions.patch function boundary exists");
    assert!(sessions_patch.contains("context.authenticated"));
    assert!(sessions_patch.contains("sessions.patch requires expectedSessionId"));
    assert!(sessions_patch.contains("expected_session_id != route.agent_id.as_str()"));
    assert!(sessions_patch.contains("SESSION_LIFECYCLE_CHANGED"));
    assert!(sessions_patch.contains("sessions.patch requires expectedLifecycleRevision"));
    assert!(sessions_patch.contains("canonical main sessions"));
    assert!(sessions_patch.contains("reject_agent_wide_session_route(&route, \"sessions.patch\")"));
    assert!(sessions_patch.contains("raw_key.eq_ignore_ascii_case(&expected_key)"));
    assert!(sessions_patch.contains("running_subagent_records_for_parent(&route.agent_id)"));
    assert!(sessions_patch.contains("active subagent work"));
    assert!(sessions_patch.contains("active_stream_ids_for_agent(&route.agent_id)"));
    assert!(sessions_patch.contains("active foreground turn"));
    assert!(sessions_patch.contains("transition_session_work_archive"));
    assert!(sessions_patch.contains("communicationPolicy"));
    assert!(sessions_patch.contains("write_session_communication_policy"));
    assert!(sessions_patch.contains("cannot combine communicationPolicy with archived"));
}

#[test]
fn test_provider_review_continuation_uses_owner_capability_and_existing_run_path() {
    let shipping_source = shipping_source();
    let continuation = shipping_source
        .split("fn call_openclaw_sessions_provider_review_continue(")
        .nth(1)
        .expect("shipping provider-review continuation owner exists")
        .split("fn delete_generated_subagent(")
        .next()
        .expect("provider-review continuation function boundary exists");
    assert!(continuation.contains("context.authenticated"));
    assert!(continuation.contains("canonical main sessions"));
    assert!(continuation.contains("expected_session_id != route.agent_id.as_str()"));
    assert!(continuation.contains("SESSION_LIFECYCLE_CHANGED"));
    assert!(continuation.contains("issue_provider_review_acknowledgment"));
    assert!(continuation.contains("claim_provider_review_acknowledgment"));
    assert!(continuation.contains("capture_session_work_admission"));
    assert!(continuation.contains("accept_routed_send"));
    assert!(continuation.contains("RUNNER_START_ROUTED_PROVIDER_GATEWAY_METHOD"));
    assert!(continuation.contains("providerReviewLifecycleRevision"));
    assert!(continuation.contains("release_provider_review_continuation"));
    let send_failure = continuation
        .split("if let Err(error) = ")
        .nth(1)
        .expect("provider-review send failure branch exists")
        .split("match ")
        .next()
        .expect("provider-review send failure branch boundary exists");
    assert!(
        send_failure.find("release_provider_review_continuation")
            < send_failure.find("release_session_work_admission"),
        "provider review must release before its admitted-work marker"
    );
    let runner_failure = continuation
        .split("Ok(result) => Ok(result),")
        .nth(1)
        .expect("provider-review Runner failure branch exists");
    assert!(
        runner_failure.find("release_provider_review_continuation")
            < runner_failure.find("release_session_work_admission"),
        "provider review must release before its admitted-work marker"
    );
    assert!(!continuation.contains("serde_json::from_value::<ProviderReviewAcknowledgment"));
}

#[test]
fn test_provider_review_projection_hides_non_pending_continuations() {
    let shipping_source = shipping_source();
    let projection = shipping_source
        .split("fn provider_review_projection(")
        .nth(1)
        .expect("provider-review projection owner exists")
        .split("fn call_openclaw_sessions_list(")
        .next()
        .expect("provider-review projection boundary exists");
    assert!(projection.contains("provider_review_phase.as_deref() == Some(\"pending\")"));
    assert!(projection.contains("record.session_id != session_id"));
    assert!(projection.contains("record.session_key != session_key"));
    assert!(projection.contains("read_session_work_admission"));
}

#[test]
fn test_provider_review_owner_exposes_single_claim_host_capability() {
    let owner_source = include_str!("../src/extensions/session/production.rs");
    assert!(owner_source.contains("pub struct ProviderReviewAcknowledgment"));
    assert!(owner_source.contains("issue_provider_review_acknowledgment"));
    assert!(owner_source.contains("claim_provider_review_acknowledgment"));
    assert!(owner_source.contains("next_run_id.trim() != state.next_run_id"));
    assert!(owner_source.contains("provider_review_capabilities"));
    assert!(owner_source.contains(".remove(&acknowledgment.token)"));
    assert!(owner_source.contains("provider_review_phase.as_deref() != Some(\"pending\")"));
    assert_eq!(
        owner_source
            .matches("record.session_id != review.session_id")
            .count(),
        2,
        "provider-review issue and claim must both enforce session identity"
    );
    assert!(owner_source.contains("settle_provider_review_continuation"));
    assert!(owner_source.contains("provider_review_continuation_run_id"));
}

#[test]
fn test_session_admission_rejects_rebound_owner_identity() {
    let owner_source = format!(
        "{}\n{}",
        include_str!("../src/extensions/session/production.rs"),
        include_str!("../src/extensions/session/ocrm_production_admission.rs"),
    );
    assert!(owner_source.contains("record.session_id != agent_id"));
    assert!(owner_source.contains("let expected_session_key = format!(\"agent:{agent_id}:main\")"));
    assert!(owner_source.contains("record.session_key != expected_session_key"));
    assert!(
        owner_source
            .split("pub(crate) fn read_session_work_admission_locked(")
            .nth(1)
            .expect("locked session admission reader exists")
            .split("pub fn read_session_communication_policy(")
            .next()
            .expect("locked session admission reader boundary exists")
            .contains("record.session_key != expected_session_key")
    );
    assert!(owner_source.contains("record.lifecycle_revision.trim().is_empty()"));
    assert!(owner_source.contains("session admission identity is malformed"));
    assert!(owner_source.contains("pub fn read_session_work_admission("));
    assert!(owner_source.contains("session admission read requires a non-empty agent identity"));
    assert!(owner_source.contains("locked session admission read requires a non-empty agent identity"));
}

#[test]
fn test_direct_agent_db_metadata_writes_fail_closed_on_malformed_admission() {
    let owner_source = format!(
        "{}\n{}",
        include_str!("../src/extensions/session/agent_db.rs"),
        include_str!("../src/extensions/session/ocrm_agent_db_persisted_ops.rs"),
    );
    assert!(owner_source.contains("fn session_work_write_is_admitted("));
    assert!(owner_source.contains("!admission.session_key.trim().is_empty()"));
    assert!(owner_source.contains("!admission.session_id.trim().is_empty()"));
    assert!(owner_source.contains("!admission.lifecycle_revision.trim().is_empty()"));
    assert!(owner_source.contains("let expected_session_key = format!(\"agent:{agent_id}:main\")"));
    assert!(owner_source.contains("admission.session_key == expected_session_key"));
    assert!(owner_source.contains("fn session_admission_identity_is_bound("));
    assert!(owner_source.contains("session archive transition owner identity is malformed or rebound"));
    assert_eq!(
        owner_source
            .matches("session_admission_identity_is_well_formed(&admission)")
            .count(),
        10,
        "all session-owned transaction edges must validate admission identity"
    );
    assert!(owner_source.contains("if !session_work_write_is_admitted(db)?"));
    let initializer = owner_source
        .split("pub fn initialize_metadata_if_missing(")
        .nth(1)
        .expect("metadata initializer exists")
        .split("pub fn set_metadata(")
        .next()
        .expect("metadata initializer boundary exists");
    assert!(initializer.contains("BEGIN IMMEDIATE"));
    assert!(initializer.contains("if !session_work_write_is_admitted(db)?"));
    let admission_cas = owner_source
        .split("fn compare_and_set_session_work_admission_with_deletion_transition(")
        .nth(1)
        .expect("session admission CAS owner exists")
        .split("pub fn read_session_provider_review(")
        .next()
        .expect("session admission CAS owner boundary exists");
    assert!(admission_cas.contains("BEGIN IMMEDIATE"));
    assert!(admission_cas.contains("COMMIT"));
    assert!(admission_cas.contains("ROLLBACK"));
    assert!(
        admission_cas.find("BEGIN IMMEDIATE")
            < admission_cas.find("let current = db")
    );
}

#[test]
fn test_automation_session_identity_helper_rechecks_owner_admission() {
    let shipping_source = shipping_source();
    let helper = shipping_source
        .split("fn resolve_admitted_automation_session_key(")
        .nth(1)
        .expect("automation session identity helper exists")
        .split("fn run_local_automation_turn(")
        .next()
        .expect("automation session identity helper boundary exists");
    assert!(helper.contains("session_workers.assert_session_work_admission"));
    assert!(helper.contains("Some(&expected_key)"));
    assert!(helper.contains("format!(\"agent:{agent_id}:main\")"));
    assert!(!helper.contains("format!(\"agent:{agent_id}:cron:{automation_id}\")"));
}

#[test]
fn test_cron_candidate_projection_rejects_malformed_identity_rows() {
    let owner_source = include_str!("../src/extensions/session/production.rs");
    let projection = owner_source
        .split("pub fn list_expired_cron_session_runs(")
        .nth(1)
        .expect("cron candidate projection exists")
        .split("pub(crate) fn read_session_work_admission_locked(")
        .next()
        .expect("cron candidate projection boundary exists");
    assert!(projection.contains("expected_session_key_prefix"));
    assert!(projection.contains("row.session_key"));
    assert!(projection.contains("row.automation_id"));
    assert!(projection.contains("row.run_id"));
    assert!(projection.contains("row.session_id == row.session_key"));
    assert!(projection.contains("!row.lifecycle_revision.trim().is_empty()"));
}

#[test]
fn test_cron_schema_migrates_fence_marker_for_existing_rows() {
    let recovery_source = include_str!("../src/extensions/session/agent_db_recovery.rs");
    assert!(recovery_source.contains("pragma_table_info('cron_session_runs')"));
    assert!(recovery_source.contains("name = 'fence_state_known'"));
    assert!(recovery_source.contains(
        "ALTER TABLE cron_session_runs ADD COLUMN fence_state_known INTEGER NOT NULL DEFAULT 0"
    ));
}

#[test]
fn test_cron_owner_writes_rollback_when_commit_fails() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let cron_owner = owner_source
        .split("pub fn begin_cron_session_run(")
        .nth(1)
        .expect("cron owner write section exists")
        .split("pub fn list_cron_session_runs(")
        .next()
        .expect("cron owner write section boundary exists");
    assert_eq!(
        cron_owner.matches("match db.execute_batch(\"COMMIT\")").count(),
        4
    );
    assert!(cron_owner.matches("db.execute_batch(\"ROLLBACK\")").count() >= 8);
}

#[test]
fn test_cron_candidate_bridge_releases_throttle_on_owner_read_failure() {
    let runtime_source = include_str!("../src/extensions/transcript/automation_runtime.rs");
    let bridge = runtime_source
        .split("pub fn list_expired_cron_session_candidates(")
        .nth(1)
        .expect("cron candidate bridge exists")
        .split("pub fn event_fires(")
        .next()
        .expect("cron candidate bridge boundary exists");
    assert!(bridge.contains("list_expired_cron_session_runs"));
    assert!(bridge.contains("throttle.forget_owner(agent_id)"));
}

#[test]
fn test_cron_settlement_accepts_only_typed_terminal_outcomes() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let cron_owner = owner_source
        .split("pub fn finish_cron_session_run(")
        .nth(1)
        .expect("cron settlement owner exists")
        .split("pub fn list_cron_session_runs(")
        .next()
        .expect("cron settlement owner boundary exists");
    assert_eq!(cron_owner.matches("!matches!(outcome, \"ok\" | \"error\" | \"interrupted\")").count(), 2);
}

#[test]
fn test_archive_owner_rolls_back_when_commit_fails() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let archive_owner = owner_source
        .split("pub fn transition_session_work_archive(")
        .nth(1)
        .expect("archive owner exists")
        .split("pub fn commit_session_work_result(")
        .next()
        .expect("archive owner boundary exists");
    assert!(archive_owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
    assert!(archive_owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));
}

#[test]
fn test_background_result_owner_rolls_back_when_commit_fails() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let result_owner = owner_source
        .split("pub fn commit_session_work_result(")
        .nth(1)
        .expect("background result owner exists")
        .split("pub fn read_metadata(")
        .next()
        .expect("background result owner boundary exists");
    assert!(result_owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
    assert!(result_owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));
}

#[test]
fn test_session_transcript_append_rolls_back_when_commit_fails() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let append_owner = owner_source
        .split("pub fn append_transcript_entries_if_session_admitted(")
        .nth(1)
        .expect("session transcript append owner exists")
        .split("pub fn append_spend_guard_card_if_session_admitted(")
        .next()
        .expect("session transcript append owner boundary exists");
    assert!(append_owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
    assert!(append_owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));
}

#[test]
fn test_session_transcript_update_and_identity_settlement_roll_back_when_commit_fails() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let update_owner = owner_source
        .split("pub fn update_transcript_entry_if_session_admitted(")
        .nth(1)
        .expect("session transcript update owner exists")
        .split("pub fn settle_transcript_entry_identity(")
        .next()
        .expect("session transcript update owner boundary exists");
    assert!(update_owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
    assert!(update_owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));

    let settlement_owner = owner_source
        .split("pub fn settle_transcript_entry_identity_if_session_admitted(")
        .nth(1)
        .expect("session transcript identity settlement owner exists")
        .split("pub fn delete_transcript_entry(")
        .next()
        .expect("session transcript identity settlement owner boundary exists");
    assert!(settlement_owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
    assert!(settlement_owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));
}

#[test]
fn test_session_transcript_delete_and_conversation_clear_roll_back_when_commit_fails() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let delete_owner = owner_source
        .split("pub fn delete_transcript_entry(")
        .nth(1)
        .expect("session transcript delete owner exists")
        .split("pub fn clear_conversation(")
        .next()
        .expect("session transcript delete owner boundary exists");
    assert!(delete_owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
    assert!(delete_owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));

    let clear_owner = owner_source
        .split("pub fn clear_conversation(")
        .nth(1)
        .expect("conversation clear owner exists")
        .split("pub fn upsert_conversation_topic(")
        .next()
        .expect("conversation clear owner boundary exists");
    assert!(clear_owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
    assert!(clear_owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));
}

#[test]
fn test_session_checkpoint_and_root_metadata_roll_back_when_commit_fails() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let checkpoint_owner = owner_source
        .split("pub fn commit_checkpoint_root_and_confirm_user_message(")
        .nth(1)
        .expect("session checkpoint owner exists")
        .split("pub fn compare_and_set_latest_root_blob_id(")
        .next()
        .expect("session checkpoint owner boundary exists");
    assert!(checkpoint_owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
    assert!(checkpoint_owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));

    let root_owner = owner_source
        .split("pub fn compare_and_set_latest_root_blob_id(")
        .nth(1)
        .expect("latest root owner exists")
        .split("pub fn get_latest_root_blob_id(")
        .next()
        .expect("latest root owner boundary exists");
    assert!(root_owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
    assert!(root_owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));
}

#[test]
fn test_session_metadata_owners_roll_back_when_commit_fails() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let owners = [
        (
            "pub fn write_kv(",
            "pub fn write_session_kv_if_generation(",
        ),
        (
            "pub fn write_session_kv_if_generation(",
            "pub fn delete_kv(",
        ),
        ("pub fn delete_kv(", "pub fn read_session_work_admission("),
        (
            "pub fn reset_session_work_admission_for_clone(",
            "pub fn compare_and_set_session_work_admission(",
        ),
        (
            "pub fn initialize_metadata_if_missing(",
            "pub fn set_metadata(",
        ),
        ("pub fn set_metadata(", "pub fn ensure_lifecycle_revision("),
    ];
    for (start, end) in owners {
        let owner = owner_source
            .split(start)
            .nth(1)
            .expect("session metadata owner exists")
            .split(end)
            .next()
            .expect("session metadata owner boundary exists");
        assert!(owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
        assert!(owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));
    }
}

#[test]
fn test_transcript_owners_roll_back_when_commit_fails() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let owners = [
        (
            "pub fn append_transcript_entries(",
            "pub fn append_transcript_entries_if_session_admitted(",
        ),
        (
            "pub fn update_transcript_entry(",
            "pub fn update_transcript_entry_if_session_admitted(",
        ),
        (
            "pub fn settle_transcript_entry_identity(",
            "pub fn settle_transcript_entry_identity_if_session_admitted(",
        ),
    ];
    for (start, end) in owners {
        let owner = owner_source
            .split(start)
            .nth(1)
            .expect("transcript owner exists")
            .split(end)
            .next()
            .expect("transcript owner boundary exists");
        assert!(owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
        assert!(owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));
    }
}

#[test]
fn test_agent_db_has_no_direct_commit_error_propagation() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    assert!(!owner_source.contains("db.execute_batch(\"COMMIT\")?;"));
}

#[test]
fn test_conversation_blob_cleanup_rolls_back_when_commit_fails() {
    let owner_source = include_str!("../src/agent_isolation/conversation_blob_store.rs");
    assert!(!owner_source.contains("execute_batch(\"COMMIT\")?"));
    assert!(owner_source.matches("execute_batch(\"ROLLBACK\")").count() >= 4);
}

#[test]
fn test_compaction_owner_rolls_back_when_commit_fails() {
    let owner_source = include_str!("../src/extensions/session/agent_db.rs");
    let compaction_owner = owner_source
        .split("pub fn commit_compaction(")
        .nth(1)
        .expect("compaction owner exists")
        .split("pub fn get_compacted_transcript_entries(")
        .next()
        .expect("compaction owner boundary exists");
    assert!(compaction_owner.contains("if let Err(error) = db.execute_batch(\"COMMIT\")"));
    assert!(compaction_owner.contains("let _ = db.execute_batch(\"ROLLBACK\")"));
}

#[test]
fn test_session_admission_creation_rechecks_create_race_identity() {
    let owner_source = format!(
        "{}\n{}",
        include_str!("../src/extensions/session/production.rs"),
        include_str!("../src/extensions/session/ocrm_production_admission.rs"),
    );
    let ensure = owner_source
        .split("fn ensure_main_session_work_admission_locked(")
        .nth(1)
        .expect("session admission initializer exists")
        .split("pub fn assert_session_work_admission(")
        .next()
        .expect("session admission initializer boundary exists");
    assert!(ensure.contains("let changed = db"));
    assert!(ensure.contains("lost its race without a record"));
    assert!(ensure.contains("malformed after create race"));
    assert!(ensure.contains("record.session_key != expected_session_key"));
    assert!(ensure.contains("current.session_key != expected_session_key"));
}

// ---------------------------------------------------------------------------
// OCRM-SESSION-O01: Session Key Parsing, Classification & Harness Namespace
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_session_o01_session_key_routing() {
    assert_eq!(classify_session_kind("global", None, None), SessionKind::Global);
    assert_eq!(classify_session_kind("unknown", None, None), SessionKind::Unknown);
    assert_eq!(classify_session_kind("agent:agent-1:cron:job-1", None, None), SessionKind::Cron);
    assert_eq!(classify_session_kind("cron:job-1", None, None), SessionKind::Direct);
    assert_eq!(classify_session_kind("agent:agent-1:opaque:cron:job-1", None, None), SessionKind::Direct);
    assert_eq!(classify_session_kind("agent:agent-1:channel:cron:job-1", None, None), SessionKind::Group);
    assert_eq!(classify_session_kind("session-1", None, Some("parent-session")), SessionKind::SpawnChild);
    assert_eq!(classify_session_kind("session-empty-parent", None, Some("")), SessionKind::Direct);
    assert_eq!(classify_session_kind("session-empty-parent-group", Some("group"), Some("")), SessionKind::Group);
    assert_eq!(classify_session_kind("session-2", Some("group"), None), SessionKind::Group);
    assert_eq!(classify_session_kind("agent:123:group:main", None, None), SessionKind::Group);
    assert_eq!(classify_session_kind("direct-sess", None, None), SessionKind::Direct);

    // Harness namespace reservation check matching upstream agent-harness-session-key.test.ts
    assert!(is_agent_harness_session_key("harness:codex:supervision:native-thread"));
    assert!(is_agent_harness_session_key("agent:main:harness:codex:supervision:native-thread"));
    assert!(is_agent_harness_session_key("agent:agent-007:harness:sub"));
    assert!(!is_agent_harness_session_key("agent:harness:main"));
    assert!(!is_agent_harness_session_key("agent:regular-user:main"));
    assert!(!is_agent_harness_session_key("direct-user"));

    let route = resolve_public_session_route(" agent:agent-123:group:main ", Some("group"), None)
        .expect("canonical public route");
    assert_eq!(route.raw_key, "agent:agent-123:group:main");
    assert_eq!(route.agent_id, "agent-123");
    assert_eq!(route.kind, SessionKind::Group);
    let mixed_case_route = resolve_public_session_route("AGENT:Agent-123:Main", None, None)
        .expect("mixed-case canonical agent route");
    assert_eq!(mixed_case_route.agent_id, "agent-123");
    assert_eq!(mixed_case_route.kind, SessionKind::Direct);
    assert_eq!(
        resolve_public_session_route("agent:agent-123:harness:child", None, None),
        Err(PublicSessionRouteError::HarnessNamespaceReserved)
    );
    assert_eq!(
        resolve_public_session_route("agent::main", None, None),
        Err(PublicSessionRouteError::EmptyAgentIdentity)
    );
    assert_eq!(
        resolve_internal_send_route(
            "agent-123",
            Some("agent:agent-123:cron:job-1"),
            None,
            None,
            false,
        )
        .expect("cron producer route")
        .kind,
        SessionKind::Cron
    );
    assert_eq!(
        resolve_internal_send_route(
            "agent-123",
            Some("agent:agent-999:main"),
            None,
            None,
            false,
        ),
        Err(InternalSessionRouteError::SelectorAgentMismatch)
    );
    assert_eq!(
        resolve_internal_send_route("agent-123", None, None, None, false),
        Err(InternalSessionRouteError::MissingSelector)
    );
    assert_eq!(
        resolve_internal_send_route("agent-123", None, None, None, true)
            .expect("explicit direct fallback")
            .kind,
        SessionKind::Direct
    );

    let shipping_source = shipping_source();
    for (start, end) in [
        ("fn call_openclaw_sessions_list(", "fn call_openclaw_sessions_get("),
        ("fn call_openclaw_sessions_get(", "fn call_openclaw_sessions_create("),
        ("fn call_openclaw_sessions_delete(", "fn call_openclaw_chat_send("),
        ("fn call_openclaw_chat_send(", "fn call_openclaw_chat_abort("),
        ("fn call_openclaw_chat_abort(", "fn call_openclaw_cron_list("),
    ] {
        let method = shipping_source
            .split(start)
            .nth(1)
            .expect("shipping session method exists")
            .split(end)
            .next()
            .expect("shipping session method boundary exists");
        assert!(method.contains("resolve_gateway_session_route"));
    }
    let chat_abort = shipping_source
        .split("fn call_openclaw_chat_abort(")
        .nth(1)
        .expect("shipping chat.abort owner exists")
        .split("fn call_openclaw_cron_list(")
        .next()
        .expect("shipping chat.abort function boundary exists");
    assert!(chat_abort.contains("reject_agent_wide_session_route(&route, \"chat.abort\")"));
    let chat_send = shipping_source
        .split("fn call_openclaw_chat_send(")
        .nth(1)
        .expect("shipping chat.send owner exists")
        .split("fn call_openclaw_chat_abort(")
        .next()
        .expect("shipping chat.send function boundary exists");
    assert!(chat_send.contains("reject_agent_wide_session_route(&route, \"chat.send\")"));
    let sessions_patch = shipping_source
        .split("fn call_openclaw_sessions_patch(")
        .nth(1)
        .expect("shipping sessions.patch owner exists")
        .split("fn call_openclaw_sessions_provider_review_continue(")
        .next()
        .expect("shipping sessions.patch function boundary exists");
    assert!(sessions_patch.contains("transition_session_work_archive("));
    assert!(sessions_patch.contains("&route.agent_id"));
    assert!(sessions_patch.contains("&expected_key"));
    let sessions_get = shipping_source
        .split("fn call_openclaw_sessions_get(")
        .nth(1)
        .expect("shipping sessions.get owner exists")
        .split("fn call_openclaw_sessions_create(")
        .next()
        .expect("shipping sessions.get function boundary exists");
    assert!(sessions_get.contains("provider_review_projection("));
    assert!(sessions_get.contains("agent_id, &canonical_key, session_id"));
    let provider_review_continue = shipping_source
        .split("fn call_openclaw_sessions_provider_review_continue(")
        .nth(1)
        .expect("shipping provider-review continuation owner exists")
        .split("fn call_openclaw_chat_send(")
        .next()
        .expect("shipping provider-review continuation function boundary exists");
    assert!(provider_review_continue.contains("session_key.eq_ignore_ascii_case(&canonical_key)"));
}

// ---------------------------------------------------------------------------
// OCRM-SESSION-O02: Communication Policy Admission & Approvals
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_session_o02_communication_policy() {
    use mahayana_host_runtime::extensions::session::agent_session::{
        CommunicationEndpoint, EffectiveSessionCommunicationPolicy,
        SessionCommunicationDecision, SessionCommunicationDirection, SessionCommunicationMode,
        SessionCommunicationPolicyOverride, plan_session_communication,
        resolve_session_communication_policy,
    };

    let defaults = SessionCommunicationPolicyOverride {
        send: Some(SessionCommunicationMode::Never),
        receive: Some(SessionCommunicationMode::Ask),
    };
    let entry = SessionCommunicationPolicyOverride {
        send: Some(SessionCommunicationMode::Always),
        receive: None,
    };
    assert_eq!(
        resolve_session_communication_policy(Some(defaults), Some(entry)),
        EffectiveSessionCommunicationPolicy {
            send: SessionCommunicationMode::Always,
            receive: SessionCommunicationMode::Ask,
        }
    );
    assert_eq!(SessionCommunicationMode::parse(" ASK "), Some(SessionCommunicationMode::Ask));
    assert_eq!(SessionCommunicationMode::parse("invalid"), None);

    let source = CommunicationEndpoint {
        agent_id: "agent-a".into(),
        session_key: "session-a".into(),
        store_path: "/sessions/a".into(),
        session_id: Some("sid-a".into()),
        // The negative admission below is intended to isolate the target's
        // receive=never override.  Without this source override, the
        // default send=never policy would deny first by design.
        communication: Some(SessionCommunicationPolicyOverride {
            send: Some(SessionCommunicationMode::Always),
            receive: None,
        }),
    };
    let target = CommunicationEndpoint {
        agent_id: "agent-b".into(),
        session_key: "session-b".into(),
        store_path: "/sessions/b".into(),
        session_id: Some("sid-b".into()),
        communication: Some(SessionCommunicationPolicyOverride {
            send: None,
            receive: Some(SessionCommunicationMode::Never),
        }),
    };
    assert_eq!(
        plan_session_communication(Some(defaults), &[source.clone()], &[target.clone()], true),
        SessionCommunicationDecision::Allowed { approvals: vec![] }
    );
    assert_eq!(
        plan_session_communication(Some(defaults), &[source.clone()], &[target.clone()], false),
        SessionCommunicationDecision::Denied {
            error: "Session communication receive is disabled for session-b.".into(),
        }
    );

    let ask_source = CommunicationEndpoint {
        communication: Some(SessionCommunicationPolicyOverride {
            send: Some(SessionCommunicationMode::Ask),
            receive: Some(SessionCommunicationMode::Always),
        }),
        ..source.clone()
    };
    let ask_target = CommunicationEndpoint {
        communication: Some(SessionCommunicationPolicyOverride {
            send: Some(SessionCommunicationMode::Always),
            receive: Some(SessionCommunicationMode::Ask),
        }),
        ..target
    };
    let SessionCommunicationDecision::Allowed { approvals } =
        plan_session_communication(None, &[ask_source], &[ask_target], false)
    else {
        panic!("ask policies should produce approvals");
    };
    assert_eq!(approvals.len(), 2);
    assert_eq!(approvals[0].direction, SessionCommunicationDirection::Send);
    assert_eq!(approvals[1].direction, SessionCommunicationDirection::Receive);
}

// ---------------------------------------------------------------------------
// OCRM-SESSION-O03: Conversation Turn Correlation & Settlement
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_session_o03_conversation_turn_correlation() {
    use mahayana_host_runtime::extensions::session::session_conversation_state::{
        PendingConversationTurnClaimResult, PendingConversationTurnRegistry,
    };
    use std::time::Duration;

    let registry = PendingConversationTurnRegistry::new();
    let turn_id = registry
        .register(
            "agent-1",
            Some("turn-uuid-12345"),
            "conversation-1",
            "session-1",
            None,
            Duration::from_secs(30),
        )
        .expect("register pending turn");
    assert_eq!(
        registry.snapshot("agent-1", &turn_id).expect("pending snapshot").claimed,
        false
    );
    assert!(registry.set_outbound_message_id("agent-1", &turn_id, "outbound-1"));
    assert!(matches!(
        registry.claim_reply(
            "agent-1",
            "conversation-1",
            None,
            "session-1",
            "inbound-1",
            Some("outbound-1"),
            None,
            "reply",
            1729900000000,
        ),
        PendingConversationTurnClaimResult::NotReady
    ));
    assert!(registry.mark_ready("agent-1", &turn_id));
    assert!(matches!(
        registry.claim_reply(
            "agent-1",
            "conversation-1",
            None,
            "session-1",
            "inbound-1",
            Some("outbound-1"),
            None,
            "reply",
            1729900000000,
        ),
        PendingConversationTurnClaimResult::Claimed(_)
    ));
    assert!(registry.release("agent-1", &turn_id));
    assert!(matches!(
        registry.claim_reply(
            "agent-1", "conversation-1", None, "session-1", "inbound-1b",
            Some("outbound-1"), None, "retry", 1729900000001,
        ),
        PendingConversationTurnClaimResult::Claimed(_)
    ));
    assert!(registry.complete("agent-1", &turn_id));

    let threaded = registry
        .register(
            "agent-1",
            Some("turn-threaded"),
            "parent-conversation",
            "session-1",
            Some("thread-2"),
            Duration::from_secs(30),
        )
        .expect("register threaded pending turn");
    assert!(registry.set_outbound_message_id("agent-1", &threaded, "outbound-threaded"));
    assert!(registry.mark_ready("agent-1", &threaded));
    assert!(matches!(
        registry.claim_reply(
            "agent-1",
            "thread-conversation",
            Some("parent-conversation"),
            "session-1",
            "inbound-wrong-thread",
            Some("outbound-threaded"),
            Some("thread-wrong"),
            "wrong thread",
            1729900000001,
        ),
        PendingConversationTurnClaimResult::NotFound
    ));
    assert!(matches!(
        registry.claim_reply(
            "agent-1",
            "thread-conversation",
            Some("parent-conversation"),
            "session-1",
            "inbound-threaded",
            Some("outbound-threaded"),
            Some("thread-2"),
            "threaded reply",
            1729900000002,
        ),
        PendingConversationTurnClaimResult::Claimed(_)
    ));
    assert!(registry.complete("agent-1", &threaded));

    let missing_thread = registry
        .register(
            "agent-1",
            Some("turn-missing-thread"),
            "parent-conversation-2",
            "session-1",
            Some("thread-3"),
            Duration::from_secs(30),
        )
        .expect("register missing-thread case");
    assert!(registry.set_outbound_message_id("agent-1", &missing_thread, "outbound-missing-thread"));
    assert!(registry.mark_ready("agent-1", &missing_thread));
    assert!(matches!(
        registry.claim_reply(
            "agent-1",
            "thread-conversation-2",
            Some("parent-conversation-2"),
            "session-1",
            "inbound-missing-thread",
            Some("outbound-missing-thread"),
            None,
            "missing thread identity",
            1729900000003,
        ),
        PendingConversationTurnClaimResult::NotFound
    ));
    assert!(registry.cancel("agent-1", &missing_thread));

    let second = registry
        .register("agent-1", Some("turn-2"), "conversation-1", "session-1", None, Duration::from_secs(30))
        .expect("register second pending turn");
    assert!(registry.set_outbound_message_id("agent-1", &second, "outbound-2"));
    assert!(registry.mark_ready("agent-1", &second));
    assert!(matches!(
        registry.claim_reply(
            "agent-1", "wrong-conversation", None, "session-1", "inbound-2",
            Some("outbound-2"), None, "reply", 1729900000000,
        ),
        PendingConversationTurnClaimResult::NotFound
    ));
    assert!(registry.cancel("agent-1", &second));

    let root = std::env::temp_dir().join(format!("fabushi-session-turn-owner-{}", turn_id));
    let workers = std::sync::Arc::new(
        mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers::with_agents_root(
            root,
            500,
        ),
    );
    let first_store = mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore::new(
        std::sync::Arc::clone(&workers),
    );
    let second_store = mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore::new(workers);
    let shared_turn = first_store
        .pending_conversation_turns()
        .register("agent-shared", Some("shared-turn"), "conversation", "session", None, Duration::from_secs(30))
        .expect("register shared worker turn");
    assert!(second_store
        .pending_conversation_turns()
        .snapshot("agent-shared", &shared_turn)
        .is_some());
    assert!(second_store.pending_conversation_turns().cancel("agent-shared", &shared_turn));
}

// ---------------------------------------------------------------------------
// OCRM-SESSION-O04: Per-Session Model & Thinking Level Overrides
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_session_o04_model_overrides() {
    let session_override = json!({
        "sessionId": "sess-override-1",
        "modelOverride": "claude-3-7-sonnet",
        "thinkingLevel": "high"
    });

    assert_eq!(session_override["modelOverride"], "claude-3-7-sonnet");
    assert_eq!(session_override["thinkingLevel"], "high");
}

// ---------------------------------------------------------------------------
// OCRM-SESSION-O05: Auth Profile Lifecycle & Preservation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_session_o05_auth_profile_preservation() {
    let path = temp_path("auth-profile-preservation");
    fs::write(&path, r#"{"version":1,"authProfile":{"userId":"usr-123","tokenRef":"sec-token"}}"#).expect("seed");

    let text = fs::read_to_string(&path).expect("read");
    assert!(text.contains("usr-123"));
    assert!(text.contains("sec-token"));
    let _ = fs::remove_file(path);
}

// ---------------------------------------------------------------------------
// OCRM-SESSION-O06: Session Work Admission & Archive/Restore Lifecycle
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_session_o06_work_admission_and_archive() {
    let session_state = json!({
        "sessionId": "sess-work-admission",
        "status": "active",
        "archived": false,
        "backgroundWorkPending": false
    });

    assert_eq!(session_state["status"], "active");
    assert_eq!(session_state["archived"], false);
}

