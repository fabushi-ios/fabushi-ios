use std::collections::BTreeMap;

use mahayana_host_runtime::extensions::box_store_sync::production::{
    ProductionBoxStoreSyncMode, resolve_production_box_store_sync_mode,
};

#[test]
fn production_mode_is_disabled_without_sync_flag() {
    let env = BTreeMap::new();
    assert_eq!(
        resolve_production_box_store_sync_mode(&env),
        ProductionBoxStoreSyncMode::Disabled
    );
}

#[test]
fn production_mode_selects_local_fs_only_with_safe_absolute_configuration() {
    let local_dir = std::env::temp_dir().join("fabushi-box-store");
    let mut env = BTreeMap::from([
        ("SAND_BOX_STORE_SYNC".to_string(), "1".to_string()),
        (
            "SAND_BOX_STORE_LOCAL_DIR".to_string(),
            local_dir.to_string_lossy().into_owned(),
        ),
        ("SAND_BOX_STORE_ID".to_string(), "123e4567-e89b-12d3-a456-426614174000".to_string()),
    ]);
    assert_eq!(
        resolve_production_box_store_sync_mode(&env),
        ProductionBoxStoreSyncMode::LocalFs {
            base_dir: local_dir.clone(),
            store_id_override: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
        }
    );

    env.insert("SAND_BOX_STORE_ID".to_string(), "../escape".to_string());
    assert_eq!(
        resolve_production_box_store_sync_mode(&env),
        ProductionBoxStoreSyncMode::LocalFs {
            base_dir: local_dir,
            store_id_override: None,
        }
    );
}

#[test]
fn production_mode_uses_source_map_when_local_store_id_is_not_overridden() {
    let local_dir = std::env::temp_dir().join("fabushi-box-store");
    let env = BTreeMap::from([
        ("SAND_BOX_STORE_SYNC".to_string(), "1".to_string()),
        (
            "SAND_BOX_STORE_LOCAL_DIR".to_string(),
            local_dir.to_string_lossy().into_owned(),
        ),
    ]);
    assert_eq!(
        resolve_production_box_store_sync_mode(&env),
        ProductionBoxStoreSyncMode::LocalFs {
            base_dir: local_dir,
            store_id_override: None,
        }
    );

    let host_extensions = include_str!("../src/host_production_extensions.rs");
    assert!(host_extensions.contains("get_or_create_box_store()"));
    assert!(host_extensions.contains("resolve_store_id:"));
    assert!(host_extensions.contains("backend_url: Some(self.backend_url.clone())"));
    assert!(host_extensions.contains("get_access_token: Some("));
    assert!(host_extensions.contains("get_machine_id: Some("));
}

#[test]
fn production_mode_wires_agent_store_and_sand_box_store_v2() {
    let env = BTreeMap::from([
        ("SAND_BOX_STORE_SYNC".to_string(), "1".to_string()),
        ("SAND_BOX_STORE_BACKEND".to_string(), "v2".to_string()),
    ]);
    assert_eq!(
        resolve_production_box_store_sync_mode(&env),
        ProductionBoxStoreSyncMode::SandBoxStoreV2
    );

    let env = BTreeMap::from([
        ("SAND_BOX_STORE_SYNC".to_string(), "1".to_string()),
    ]);
    assert_eq!(
        resolve_production_box_store_sync_mode(&env),
        ProductionBoxStoreSyncMode::AgentStore
    );
}

#[test]
fn shipping_host_starts_box_store_sync_after_mcp_and_stops_it_before_mcp() {
    let host_extensions = include_str!("../src/host_production_extensions.rs");
    assert!(host_extensions.contains("HostExtensionId::BoxStoreSync"));
    assert!(host_extensions.contains("pub fn start_box_store_sync("));
    assert!(host_extensions.contains("production BoxStoreSync requires MCP to be started first"));

    let shipping_main = include_str!("../app/src/main.rs");
    let start_mcp = shipping_main
        .find("production_extensions.start_mcp(")
        .expect("shipping MCP start");
    let start_box_store = shipping_main
        .find("production_extensions.start_box_store_sync(")
        .expect("shipping BoxStoreSync start");
    assert!(
        start_mcp < start_box_store,
        "BoxStoreSync must start only after its frozen MCP dependency"
    );

    let stop_box_store = shipping_main
        .find("production_extensions.stop_box_store_sync()")
        .expect("shipping BoxStoreSync stop");
    let stop_mcp = shipping_main
        .find("production_extensions.stop_mcp()")
        .expect("shipping MCP stop");
    assert!(
        stop_box_store < stop_mcp,
        "BoxStoreSync must stop before MCP so it cannot outlive its dependency"
    );
}

#[test]
fn production_owner_wires_real_chrome_watcher_periodic_cycle_and_remote_providers() {
    let production = include_str!("../src/extensions/box_store_sync/production.rs").replace("\r\n", "\n");
    assert!(production.contains("ChromeSessionWatcher::with_logger"));
    assert!(production.contains("run_local_cycle(true, false, false, false, false, true, false)"));
    assert!(production.contains("plan_periodic_cycle("));
    assert!(production.contains("plan.include_store_dbs"));
    assert!(production.contains("plan.skip_live_handle_store_dbs"));
    assert!(production.contains("plan.include_idle_only"));
    assert!(production.contains("stage_box_chrome_session_with_report(|report|"));
    assert!(production.contains("chrome_session_stage_telemetry(&report)"));
    assert!(production.contains("report_chrome_session_stage(telemetry.level, &telemetry.metadata);"));
    assert!(production.contains("sync_store_db_snapshots("));
    assert!(production.contains("StoreDbSnapshotUpload::new("));
    assert!(production.contains("run_vacuum_off_thread(&source_path, &temp_path)"));
    assert!(production.contains("write_manifest("));
    assert!(production.contains("transfer: BoxStoreTransfer"));
    assert!(production.contains("self.transfer.sync_tree_category("));
    let transfer = include_str!("../src/extensions/box_store_sync/box_store_transfer.rs");
    assert!(transfer.contains("store.put_from_file(&key, &temp_path)"));
    assert!(transfer.contains("for_each_bounded(candidates, concurrency"));
    assert!(transfer.contains("SNAPSHOT_OUT_LARGE_CONCURRENCY"));
    assert!(production.contains("ProductionBoxStoreSyncMode::AgentStore"));
    assert!(production.contains("ProductionBoxStoreSyncMode::SandBoxStoreV2"));
    assert!(production.contains("resolve_box_object_store_provider("));
    assert!(production.contains("BoxObjectStoreProviderDependencies {"));
    assert!(production.contains("has_live_sand_agent_db_handle"));
    assert!(production.contains("run_store_db_debounce_loop"));
    assert!(production.contains("AgentDbCaptureQueues"));
    assert!(production.contains("get_sand_agent_db_write_generation"));
    assert!(production.contains("sync_local_packs"));
    assert!(production.contains("PACK_TMP_DIR_NAME"));
    assert!(production.contains("BOX_STORE_PACK_RETIRED_KEY"));
    assert!(production.contains("BOX_STORE_PACK_INDEX_KEY"));
    assert!(production.contains("plan.include_packs"));

    let provider_owner = include_str!("../src/extensions/box_store_sync/box_object_store.rs");
    assert!(provider_owner.contains("BoxStoreBackendKind::AgentStore =>"));
    assert!(provider_owner.contains("AgentStoreObjectStoreProvider::new(remote)?"));
    assert!(provider_owner.contains("AgentStoreClient::new(deps)?"));
    assert!(provider_owner.contains("client.get_object(&self.source_id, key)?"));
    let agent_store_impl = provider_owner
        .split_once("impl BoxObjectStore for AgentStoreObjectStore {")
        .map(|(_, body)| body)
        .expect("AgentStoreObjectStore production BoxObjectStore implementation");
    let agent_store_put = agent_store_impl
        .split_once("fn put(&self, key: &str, bytes: &[u8])")
        .map(|(_, body)| body)
        .and_then(|body| body.split_once("fn put_if_unchanged(").map(|(put, _)| put))
        .expect("AgentStoreObjectStore production put method");
    let agent_store_put_compact: String = agent_store_put
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert!(
        agent_store_put_compact
            .contains(".client.put_bytes(&self.source_id,key,bytes,precondition)?;"),
        "AgentStoreObjectStore::put must call AgentStoreClient::put_bytes with the resolved precondition"
    );
    let agent_store_put_file = agent_store_impl
        .split_once("fn put_from_file(&self, key: &str, src_path: &Path)")
        .map(|(_, body)| body)
        .and_then(|body| body.split_once("fn list(&self, prefix: &str)").map(|(put_file, _)| put_file))
        .expect("AgentStoreObjectStore production put_from_file method");
    let agent_store_put_file_compact: String = agent_store_put_file
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert!(
        agent_store_put_file_compact
            .contains(".client.put_file_content_addressed(&self.source_id,key,src_path)?"),
        "AgentStoreObjectStore::put_from_file must use the content-addressed AgentStore upload path"
    );

    let agent_store_list = agent_store_impl
        .split_once("fn list(&self, prefix: &str)")
        .map(|(_, body)| body)
        .and_then(|body| body.split_once("fn delete(&self, key: &str)").map(|(list, _)| list))
        .expect("AgentStoreObjectStore production list method");
    let agent_store_list_compact: String = agent_store_list
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert!(
        agent_store_list_compact.contains(".client.list_objects(&self.source_id,prefix)"),
        "AgentStoreObjectStore::list must delegate to AgentStoreClient::list_objects"
    );

    let agent_store = include_str!("../src/extensions/box_store_sync/agent_store_sand_files.rs");
    assert!(agent_store.contains("fn token_for(&self, source_id: &str)"));
    assert!(agent_store.contains("PRESIGN_AGENT_STORE_READS_PATH"));
    assert!(agent_store.contains("PRESIGN_AGENT_STORE_WRITES_PATH"));
    assert!(agent_store.contains("validate_presigned_url(&self.inner.deps.backend_url"));
    assert!(agent_store.contains("if response.status().as_u16() == 409"));
    assert!(agent_store.contains("status == 412 || status == 409"));
    assert!(agent_store.contains("pub fn get_object_to_file("));
    assert!(agent_store.contains("pub fn put_file_content_addressed("));
    assert!(agent_store.contains("pub fn presign_read_batch("));
    assert!(agent_store.contains("pub fn list_objects("));
    assert!(provider_owner.contains("BoxStoreBackendKind::SandBoxStoreV2 =>"));
    assert!(provider_owner.contains("SandBoxStoreServiceProvider::new(client)?"));

    let shipping_main = include_str!("../app/src/main.rs");
    assert!(shipping_main.contains("schedule_store_db_snapshot(&agent_id)"));
    let retire = shipping_main
        .find("retire_idle_live_session(&worker_retire_sessions, &agent_id)")
        .expect("live Agent DB session retirement");
    let schedule = shipping_main
        .find("schedule_store_db_snapshot(&agent_id)")
        .expect("turn-end store.db scheduling");
    assert!(
        retire < schedule,
        "turn-end capture must be scheduled only after the live Agent DB owner retires"
    );
}


#[test]
fn production_manifest_owner_consumes_frozen_retry_hydration_and_conflict_contract() {
    let production = include_str!("../src/extensions/box_store_sync/production.rs");
    assert!(production.contains("load_manifest_for_write("));
    assert!(production.contains("write_manifest_with_retry("));
    assert!(production.contains("fully_hydrated"));
    assert!(production.contains("BOX_STORE_HYDRATION_HANDOFF_FILE_NAME"));
    assert!(production.contains("BOX_STORE_MANIFEST_RETRY_ATTEMPTS"));
    assert!(production.contains("BOX_STORE_MANIFEST_RETRY_DELAY_MS"));
    assert!(
        !production.contains("fn serialize_manifest("),
        "shipping production must not retain a second manifest serializer"
    );

    let manifest = include_str!("../src/extensions/box_store_sync/box_store_manifest.rs");
    for expected in [
        "BoxStoreManifestParseError::UnsupportedVersion",
        "BoxStoreManifestParseError::PathConflict",
        "configured_manifest_entries(",
        "load_manifest_for_write(",
        "write_manifest_with_retry(",
        "accept_matching_canonical_on_conflict",
        "manifest_write_conflict",
        "mark_legacy_hydration_incomplete(",
        "mark_legacy_hydration_complete_for_handoff(",
        "prepare_canonical_manifest_reset(",
    ] {
        assert!(
            manifest.contains(expected),
            "canonical manifest owner must retain frozen behavior marker {expected}"
        );
    }

    let object_store = include_str!("../src/extensions/box_store_sync/box_object_store.rs");
    assert!(object_store.contains("put_if_unchanged_detailed("));
    assert!(object_store.contains("conflict_rel_path"));
    assert!(object_store.contains("base_etag.or(fallback_base_etag)"));
    assert!(object_store.contains("agent-store-etag"));
}


#[test]
fn shipping_box_store_sync_core_writer_lock_is_consumed_by_every_mutating_owner() {
    let core = include_str!("../src/extensions/box_store_sync/box_store_sync.rs");
    for expected in [
        "pub struct BoxStoreWriterLock",
        "FileExt::try_lock_exclusive(&file)",
        "BOX_STORE_WRITER_LOCK_FILE_NAME",
        "assert_no_symlink_in_path(lock_path)",
        "FileExt::unlock(&self.file)",
    ] {
        assert!(
            core.contains(expected),
            "box-store-sync core must retain writer-lock marker {expected}"
        );
    }

    let production = include_str!("../src/extensions/box_store_sync/production.rs");
    for expected in [
        "writer_lock: Mutex<Option<BoxStoreWriterLock>>",
        "fn ensure_writer_lock(&self) -> Result<bool, String>",
        "match BoxStoreWriterLock::try_acquire(&lock_path, &window_id)",
        "self.log(&format!(\"writer lock error: {error}\"))",
        "return Ok(false);",
        "self.inner.release_writer_lock();",
    ] {
        assert!(
            production.contains(expected),
            "shipping ProductionBoxStoreSync owner must consume core writer-lock marker {expected}"
        );
    }
    assert!(
        production.matches("self.ensure_writer_lock()").count() >= 4,
        "snapshot, turn-end capture, clear, and forget must all fence mutation through the long-lived writer lock"
    );
    assert!(
        !production.contains("BoxStoreWriterLock::try_acquire(&lock_path, &window_id)?"),
        "writer-lock acquisition errors must be normalized to the frozen locked outcome instead of escaping the shipping service"
    );

    let clear = production
        .split_once("fn clear_store_now(&self) -> Result<(), String>")
        .map(|(_, body)| body)
        .and_then(|body| body.split_once("fn reset_canonical_store(").map(|(clear, _)| clear))
        .expect("clear_store_now production body");
    let acquire = clear
        .find("if !self.ensure_writer_lock()?")
        .expect("clear writer-lock acquire");
    let legacy_reset = clear
        .find("self.reset_canonical_store(&store_id, legacy.as_ref())")
        .expect("legacy canonical reset");
    assert!(
        acquire < legacy_reset,
        "SandBoxStoreV2 legacy reset must never write before the canonical writer lock is held"
    );
}


#[test]
fn shipping_cycle_uses_canonical_frozen_telemetry_mapper_and_host_owner() {
    let production = include_str!("../src/extensions/box_store_sync/production.rs");
    assert!(production.contains("fn report_cycle_telemetry(&self"));
    assert!(production.contains("box_store_sync_cycle_telemetry(summary)"));
    assert!(production.contains("(self.deps.report_box_store_sync_cycle)(telemetry.level, &telemetry.metadata);"));
    assert!(production.contains("self.inner.report_sync_lifecycle(\"warn\", false, \"disabled\")"));
    assert!(production.contains("self.inner.report_sync_lifecycle(\"info\", true, \"enabled\")"));
    assert!(production.contains("total_failures = categories"));
    assert!(production.contains("metadata_failures = categories"));
    assert!(production.contains("store_db_entries: count_store_db_manifest_entries(Some(&entries))"));
    assert!(production.contains("agent_dir_entries: count_agent_dir_manifest_entries(Some(&entries))"));
    assert!(production.contains("self.report_cycle_telemetry("));

    let owner = include_str!("../src/host_production_extensions.rs").replace("\r\n", "\n");
    assert!(owner.contains("report_box_store_sync_cycle: Arc::new"));
    assert!(owner.contains("box_store_sync_logs.report_box_store_sync_cycle(level, metadata)"));

    let telemetry = include_str!("../src/extensions/telemetry/host_telemetry_service.rs").replace("\r\n", "\n");
    assert!(telemetry.contains("pub fn report_box_store_sync_cycle("));
    assert!(telemetry.contains("event: Some(\"sand.box_store_sync\")"));

    let report_cycle = production
        .split_once("fn report_cycle_telemetry(&self")
        .map(|(_, body)| body)
        .and_then(|body| body.split_once("fn report_sync_lifecycle").map(|(body, _)| body))
        .expect("shipping report_cycle_telemetry body");
    assert!(
        !report_cycle.contains("report_host_extension_diagnostic"),
        "canonical Box Store cycle telemetry must not be disguised as a Host extension diagnostic"
    );
}


#[test]
fn shipping_store_db_capture_uses_frozen_mapper_and_unique_host_owner() {
    let production = include_str!("../src/extensions/box_store_sync/production.rs").replace("\r\n", "\n");
    assert!(production.contains("sync_store_db_snapshots_with_trace("));
    assert!(production.contains("box_store_db_capture_telemetry(&BoxStoreDbCaptureTelemetrySummary"));
    assert!(production.contains("(self.deps.report_box_store_db_capture)(telemetry.level, &telemetry.metadata);"));
    assert!(production.contains("\"flush\""));
    assert!(production.contains("\"turn_end\""));
    assert!(production.contains("queue_duration_ms"));
    assert!(production.contains("capture_duration_ms"));
    assert!(production.contains("blob_upload_duration_ms"));
    assert!(production.contains("manifest_commit_duration_ms"));
    assert!(production.contains("StoreDbCaptureFailurePhase::Capture"));
    assert!(production.contains("StoreDbCaptureFailurePhase::BlobUpload"));
    assert!(production.contains("StoreDbCaptureFailurePhase::ManifestCommit"));

    let turn_end = production
        .split_once("fn run_local_agent_db_snapshot_unqueued(")
        .map(|(_, body)| body)
        .and_then(|body| body.split_once("pub fn resolve_production_box_store_sync_mode").map(|(body, _)| body))
        .expect("shipping turn-end capture body");
    assert!(turn_end.contains("self.report_store_db_capture("));
    assert!(
        !turn_end.contains("self.diagnostic("),
        "canonical turn-end store.db capture telemetry must not fall back to Host-extension diagnostics"
    );

    let owner = include_str!("../src/host_production_extensions.rs").replace("\r\n", "\n");
    assert!(owner.contains("report_box_store_db_capture: Arc::new"));
    assert!(owner.contains("box_store_db_capture_logs.report_box_store_db_capture(level, metadata)"));

    let telemetry = include_str!("../src/extensions/telemetry/host_telemetry_service.rs").replace("\r\n", "\n");
    assert!(telemetry.contains("pub fn report_box_store_db_capture("));
    assert!(telemetry.contains("event: Some(\"sand.box_store_db_capture\")"));

    let mapper = include_str!("../src/extensions/box_store_sync/store_db_capture.rs");
    assert!(mapper.contains("StoreDbCaptureOutcome::Error | StoreDbCaptureOutcome::Skipped => \"warn\""));
    assert!(mapper.contains("StoreDbCaptureOutcome::Oversize"));
    assert!(mapper.contains("\"failure_phase\".into()"));
}


#[test]
fn shipping_manifest_conflict_uses_frozen_metadata_and_unique_host_owner() {
    let production = include_str!("../src/extensions/box_store_sync/production.rs").replace("\r\n", "\n");
    assert_eq!(
        production.matches("write_manifest_with_retry_and_conflict_reporter(").count(),
        4,
        "all four shipping manifest-write paths must carry the exhausted-conflict reporter"
    );
    assert_eq!(
        production.matches("self.report_manifest_write_conflict(info)").count(),
        4,
        "every shipping manifest-write path must report through the same typed owner"
    );
    assert!(production.contains(
        "fn report_manifest_write_conflict(&self, info: &BoxStoreManifestConflictInfo)"
    ));
    for field in [
        r#""store_id""#,
        r#""attempts""#,
        r#""accepted""#,
        r#""covered""#,
        r#""canonical_matches_attempt""#,
        r#""live_view_changed""#,
        r#""attempted_entries""#,
        r#""last_base_etag""#,
        r#""last_baseline_source""#,
        r#""last_conflict_rel_path""#,
        r#""canonical_readable""#,
        r#""canonical_entry_count""#,
        r#""canonical_updated_at_ms""#,
        r#""canonical_writer_window_id""#,
        r#""our_window_id""#,
    ] {
        assert!(
            production.contains(field),
            "shipping manifest-conflict mapper is missing frozen field {field}"
        );
    }
    assert!(production.contains(
        r#"(self.deps.report_box_store_manifest_conflict)("warn", &metadata);"#
    ));

    let manifest = include_str!("../src/extensions/box_store_sync/box_store_manifest.rs");
    assert!(manifest.contains("on_manifest_write_conflict(&conflict_info);"));
    assert!(manifest.contains("report_manifest_write_conflict_diagnostic(&conflict_info);"));

    let owner = include_str!("../src/host_production_extensions.rs").replace("\r\n", "\n");
    assert!(owner.contains("report_box_store_manifest_conflict: Arc::new"));
    assert!(owner.contains(
        "box_store_manifest_conflict_logs
                        .report_box_store_manifest_conflict(level, metadata)"
    ));

    let telemetry = include_str!("../src/extensions/telemetry/host_telemetry_service.rs").replace("\r\n", "\n");
    assert!(telemetry.contains("pub fn report_box_store_manifest_conflict("));
    assert!(telemetry.contains(r#"event: Some("sand.box_store_manifest_conflict")"#));

    let report = production
        .split_once("fn report_manifest_write_conflict(&self")
        .map(|(_, body)| body)
        .and_then(|body| {
            body.split_once("fn report_store_db_capture")
                .map(|(report, _)| report)
        })
        .expect("shipping manifest conflict mapper");
    assert!(
        !report.contains("report_host_extension_diagnostic"),
        "canonical manifest-conflict structured telemetry must not be disguised as a Host-extension diagnostic"
    );
}

#[test]
fn shipping_chrome_session_stage_uses_frozen_mapper_and_unique_host_owner() {
    let production = include_str!("../src/extensions/box_store_sync/production.rs").replace("\r\n", "\n");
    assert!(production.contains("stage_box_chrome_session_with_report(|report|"));
    assert!(production.contains("chrome_session_stage_telemetry(&report)"));
    assert!(production.contains(
        "report_chrome_session_stage(telemetry.level, &telemetry.metadata);"
    ));
    assert!(production.contains("&self.deps.report_chrome_session_stage"));

    let owner = include_str!("../src/host_production_extensions.rs").replace("\r\n", "\n");
    assert!(owner.contains("report_chrome_session_stage: Arc::new"));
    assert!(owner.contains(
        "chrome_session_stage_logs.report_chrome_session_stage(level, metadata)"
    ));

    let telemetry = include_str!("../src/extensions/telemetry/host_telemetry_service.rs").replace("\r\n", "\n");
    assert!(telemetry.contains("pub fn report_chrome_session_stage("));
    assert!(telemetry.contains(r#"event: Some("sand.chrome_session_stage")"#));

    let mapper = include_str!("../src/extensions/box_store_sync/chrome_session_stage.rs").replace("\r\n", "\n");
    for field in [
        r#""staged""#,
        r#""skipped""#,
        r#""skipped_dbs""#,
        r#""error_class""#,
        r#""failure_db""#,
        r#""failure_phase""#,
        r#""failure_operation""#,
        r#""failure_path_stage""#,
        r#""failure_cause""#,
        r#""errno""#,
        r#""sqlite_code""#,
    ] {
        assert!(mapper.contains(field), "Chrome stage mapper missing {field}");
    }
    assert!(mapper.contains(r#"level: if report.skipped > 0 { "warn" } else { "info" }"#));

    let stage = production
        .split_once("fn sync_chrome_session(")
        .map(|(_, body)| body)
        .and_then(|body| body.split_once("#[derive(Clone)]\nstruct ProductionStoreDbSnapshotRuntime").map(|(body, _)| body))
        .expect("shipping chrome session stage body");
    assert!(
        !stage.contains("report_host_extension_diagnostic"),
        "canonical Chrome session stage telemetry must not degrade to Host-extension diagnostics"
    );
}
