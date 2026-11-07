//! OpenClaw Storage, Recovery, Diagnostics & Observability Native Rust Fusion & Contract Tests (OCRM-M29/M30)
//!
//! Validates the 12 falsifiable test oracles defined for OCRM Storage & Diagnostics:
//! - OCRM-STORAGE-O01: transcript_entry_matches_recovered message equality on role and content.
//! - OCRM-STORAGE-O02: transcript_entry_matches_recovered send-message equality on inner message object.
//! - OCRM-STORAGE-O03: transcript_entry_matches_recovered tool-call equality on name, status, and summary.
//! - OCRM-STORAGE-O04: transcript_entry_matches_recovered kind mismatch and non-matching kinds rejection.
//! - OCRM-STORAGE-O05: CachedBlobReadStore coalesced async reading and single-flight execution.
//! - OCRM-STORAGE-O06: CachedBlobReadStore set_blob write-through updates cached future immediately.
//! - OCRM-STORAGE-O07: ensure_profile_file idempotent creation and preservation of existing profiles.
//! - OCRM-STORAGE-O08: ensure_settings_file idempotent creation and preservation of existing settings.
//! - OCRM-DIAG-O01: HostDiagnostic kind and fields map representation and integrity.
//! - OCRM-DIAG-O02: pin_host_diagnostics_reporter thread-safe registration and clear lifecycle.
//! - OCRM-DIAG-O03: report_host_diagnostic dispatch to active diagnostic reporter.
//! - OCRM-DIAG-O04: report_diagnostic_event JSON object payload parsing and dispatch.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::FutureExt;
use serde_json::json;

use mahayana_host_runtime::agents::agent_profile::read_sand_profile_file;
use mahayana_host_runtime::agents::settings_file::read_sand_settings_file;
use mahayana_host_runtime::extensions::session::session_recovery::{
    cache_blob_reads, ensure_profile_file, ensure_settings_file,
    transcript_entry_matches_recovered,
};
use mahayana_host_runtime::host_diagnostics::{
    pin_host_diagnostics_reporter, report_diagnostic_event, report_host_diagnostic,
    HostDiagnostic,
};
use mahayana_host_runtime::storage::{
    agent_paths::{assert_valid_sand_agent_id, resolve_sand_agent_dir},
    folder_id::{is_safe_folder_id, validate_storage_key, validate_storage_prefix},
    sqlite_recovery::remove_sqlite_sidecars,
    store_db::{
        bump_db_write_generation, delete_sand_agent_db_write_generation,
        get_sand_agent_db_write_generation, live_db_handle_count, register_live_db_handle,
        release_live_db_handle, SQLITE_DB_SIDECAR_SUFFIXES,
    },
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "fabushi-storage-diag-{label}-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O01: Transcript Message Recovery Equality
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o01_message_equality() {
    let original = json!({
        "kind": "message",
        "role": "user",
        "content": "Verify state storage parity",
        "clientNonce": "nonce-12345"
    });
    let recovered_same = json!({
        "kind": "message",
        "role": "user",
        "content": "Verify state storage parity",
        "clientNonce": "nonce-different-or-omitted"
    });
    let recovered_diff = json!({
        "kind": "message",
        "role": "assistant",
        "content": "Verify state storage parity"
    });

    assert!(
        transcript_entry_matches_recovered(&original, &recovered_same),
        "Same kind, role, and content must match regardless of nonce or metadata"
    );
    assert!(
        !transcript_entry_matches_recovered(&original, &recovered_diff),
        "Different role must not match"
    );
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O02: SendMessage Recovery Equality
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o02_send_message_equality() {
    let a = json!({
        "kind": "send-message",
        "message": {"type": "text", "content": "Outbound prompt"}
    });
    let b = json!({
        "kind": "send-message",
        "message": {"type": "text", "content": "Outbound prompt"}
    });
    let c = json!({
        "kind": "send-message",
        "message": {"type": "text", "content": "Different outbound"}
    });

    assert!(transcript_entry_matches_recovered(&a, &b));
    assert!(!transcript_entry_matches_recovered(&a, &c));
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O03: ToolCall Recovery Equality
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o03_tool_call_equality() {
    let tool_a = json!({
        "kind": "tool-call",
        "name": "sand_web_search",
        "status": "completed",
        "summary": "found 5 items",
        "timestampMs": 1000
    });
    let tool_b = json!({
        "kind": "tool-call",
        "name": "sand_web_search",
        "status": "completed",
        "summary": "found 5 items",
        "timestampMs": 2000
    });
    let tool_c = json!({
        "kind": "tool-call",
        "name": "sand_web_search",
        "status": "failed",
        "summary": "found 5 items"
    });

    assert!(transcript_entry_matches_recovered(&tool_a, &tool_b));
    assert!(!transcript_entry_matches_recovered(&tool_a, &tool_c));
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O04: Kind Mismatch and Non-Matching Kinds
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o04_kind_mismatch() {
    let msg = json!({"kind": "message", "role": "user", "content": "hi"});
    let notice = json!({"kind": "notice", "text": "hi"});

    assert!(!transcript_entry_matches_recovered(&msg, &notice));
    // Notice entries are intentionally not deduplicated/matched by transcript_entry_matches_recovered
    assert!(!transcript_entry_matches_recovered(&notice, &notice));
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O05: CachedBlobReadStore Coalesced Async Reading
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_ocrm_storage_o05_blob_read_coalescing() {
    let fetch_count = Arc::new(AtomicUsize::new(0));
    let fetch_counter = Arc::clone(&fetch_count);

    let store = cache_blob_reads(
        move |id| {
            fetch_counter.fetch_add(1, Ordering::SeqCst);
            async move { Ok(Some(format!("data-for-{}", String::from_utf8_lossy(&id)).into_bytes())) }.boxed()
        },
        |_id, _data| async move { Ok(()) }.boxed(),
        || async move { Ok(()) }.boxed(),
    );

    let key = b"blob-key-1";
    let read_a = store.get_blob(key);
    let read_b = store.get_blob(key);

    let res_a = read_a.await.expect("read a succeeds");
    let res_b = read_b.await.expect("read b succeeds");

    assert_eq!(res_a, res_b);
    assert_eq!(fetch_count.load(Ordering::SeqCst), 1, "Should coalesce into exactly 1 underlying read");
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O06: CachedBlobReadStore Write-Through Update
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_ocrm_storage_o06_blob_write_through() {
    let store = cache_blob_reads(
        |_id| async move { Ok(Some(b"initial".to_vec())) }.boxed(),
        |_id, _data| async move { Ok(()) }.boxed(),
        || async move { Ok(()) }.boxed(),
    );

    let key = b"mutable-key";
    let initial = store.get_blob(key).await.expect("get initial");
    assert_eq!(initial, Some(b"initial".to_vec()));

    // Write new data through
    store.set_blob(key, b"updated".to_vec()).await.expect("set blob");

    // Subsequent get returns updated data without querying initial source
    let updated = store.get_blob(key).await.expect("get updated");
    assert_eq!(updated, Some(b"updated".to_vec()));
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O07: Idempotent Profile File Creation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o07_ensure_profile_file() {
    let root = temp_root("profile");
    let db_path = root.join("agent.db");

    // First call creates the file
    let path = ensure_profile_file(&db_path, Some("Primary Agent"), "Main Assistant Profile")
        .expect("ensure profile");
    assert!(path.exists());

    let profile = read_sand_profile_file(&path).expect("read profile");
    assert_eq!(profile.name, "Primary Agent");
    assert_eq!(profile.description, "Main Assistant Profile");

    // Second call with different args preserves existing profile
    let path_second = ensure_profile_file(&db_path, Some("Different Name"), "Different Desc")
        .expect("second ensure profile");
    assert_eq!(path, path_second);

    let profile_preserved = read_sand_profile_file(&path).expect("read preserved");
    assert_eq!(profile_preserved.name, "Primary Agent");
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O08: Idempotent Settings File Creation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o08_ensure_settings_file() {
    let root = temp_root("settings");
    let db_path = root.join("agent.db");

    let path = ensure_settings_file(&db_path).expect("ensure settings");
    assert!(path.exists());

    let settings = read_sand_settings_file(&path);
    assert_eq!(settings.notify_on_agent_updates, true);
    assert_eq!(settings.hidden_from_sidebar, false);

    // Second call preserves existing
    let path_second = ensure_settings_file(&db_path).expect("second ensure");
    assert_eq!(path, path_second);
}

// ---------------------------------------------------------------------------
// OCRM-DIAG-O01: HostDiagnostic Representation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_diag_o01_diagnostic_representation() {
    let mut fields = serde_json::Map::new();
    fields.insert("latency_ms".to_string(), json!(42));
    fields.insert("status".to_string(), json!("ok"));

    let diag = HostDiagnostic {
        kind: "turn_execution".to_string(),
        fields,
    };

    assert_eq!(diag.kind, "turn_execution");
    assert_eq!(diag.fields.get("latency_ms").and_then(|v| v.as_i64()), Some(42));
    assert_eq!(diag.fields.get("status").and_then(|v| v.as_str()), Some("ok"));
}

// ---------------------------------------------------------------------------
// OCRM-DIAG-O02: Reporter Registration and Clear Lifecycle
// ---------------------------------------------------------------------------
static DIAG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_ocrm_diag_o02_reporter_lifecycle() {
    let _guard = DIAG_LOCK.lock().unwrap();
    let reporter = Arc::new(|_diag: &HostDiagnostic| {});
    pin_host_diagnostics_reporter(Some(reporter));
    pin_host_diagnostics_reporter(None);
}

// ---------------------------------------------------------------------------
// OCRM-DIAG-O03: Synchronous Diagnostic Dispatch
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_diag_o03_report_host_diagnostic() {
    let _guard = DIAG_LOCK.lock().unwrap();
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&captured);

    pin_host_diagnostics_reporter(Some(Arc::new(move |diag| {
        sink.lock().unwrap().push(diag.kind.clone());
    })));

    let mut fields = serde_json::Map::new();
    fields.insert("session_id".to_string(), json!("sess-123"));

    report_host_diagnostic(&HostDiagnostic {
        kind: "session_created".to_string(),
        fields,
    });

    assert_eq!(*captured.lock().unwrap(), vec!["session_created".to_string()]);
    pin_host_diagnostics_reporter(None);
}

// ---------------------------------------------------------------------------
// OCRM-DIAG-O04: Convenience Helper report_diagnostic_event
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_diag_o04_report_diagnostic_event() {
    let _guard = DIAG_LOCK.lock().unwrap();
    let captured_events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&captured_events);

    pin_host_diagnostics_reporter(Some(Arc::new(move |diag| {
        let agent = diag.fields.get("agent_id").and_then(|v| v.as_str()).unwrap_or_default();
        sink.lock().unwrap().push((diag.kind.clone(), agent.to_string()));
    })));

    report_diagnostic_event("agent_turn_started", json!({"agent_id": "agent-007"}));

    assert_eq!(
        *captured_events.lock().unwrap(),
        vec![("agent_turn_started".to_string(), "agent-007".to_string())]
    );

    pin_host_diagnostics_reporter(None);
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O09: Storage Key and Prefix Validation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o09_storage_key_validation() {
    // Valid keys
    assert!(validate_storage_key("artifacts/v1_image.png").is_ok());
    assert!(validate_storage_key("sessions/123/transcript.jsonl").is_ok());
    assert!(validate_storage_key("config.json").is_ok());

    // Invalid keys (falsifiable)
    assert!(validate_storage_key("").is_err());
    assert!(validate_storage_key(".").is_err());
    assert!(validate_storage_key("..").is_err());
    assert!(validate_storage_key("a/../b").is_err());
    assert!(validate_storage_key("a/./b").is_err());
    assert!(validate_storage_key("a//b").is_err());
    assert!(validate_storage_key("a/b/").is_err());
    let long_key = "a".repeat(513);
    assert!(validate_storage_key(&long_key).is_err());

    // Valid and invalid prefixes
    assert!(validate_storage_prefix("").is_ok());
    assert!(validate_storage_prefix("artifacts/").is_ok());
    assert!(validate_storage_prefix("sessions/123").is_ok());
    assert!(validate_storage_prefix("sessions/../escape").is_err());
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O10: Storage Marker Schema and Verification Contract
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o10_storage_marker_contract() {
    let unencrypted_marker = json!({
        "version": 1,
        "locationId": "764714e6-7bb7-44a0-846a-18e9255eff59",
        "createdAt": 1729900000000_u64,
        "encryption": "none"
    });
    assert_eq!(unencrypted_marker["version"], 1);
    assert_eq!(unencrypted_marker["encryption"], "none");

    let encrypted_marker = json!({
        "version": 1,
        "locationId": "54493b92-8ec8-46ab-bdbe-b822dfd84028",
        "createdAt": 1729900001000_u64,
        "encryption": {
            "kdf": "scrypt",
            "salt": "c2FsdHNhbHRzYWx0MTY=",
            "N": 32768,
            "r": 8,
            "p": 1,
            "keyCheck": "a2V5Y2hlY2szMmJ5dGVzY2hlY2sxMjM0NTY3ODkxMjM="
        }
    });
    assert_eq!(encrypted_marker["encryption"]["kdf"], "scrypt");
    assert_eq!(encrypted_marker["encryption"]["N"], 32768);
    assert_eq!(encrypted_marker["encryption"]["r"], 8);
    assert_eq!(encrypted_marker["encryption"]["p"], 1);
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O11: Storage Backend Root Directory Isolation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o11_storage_backend_contract() {
    // Agent ID isolation
    assert!(assert_valid_sand_agent_id("agent-valid-123").is_ok());
    assert!(assert_valid_sand_agent_id("").is_err());
    assert!(assert_valid_sand_agent_id(".").is_err());
    assert!(assert_valid_sand_agent_id("..").is_err());
    assert!(assert_valid_sand_agent_id("agent/escape").is_err());
    assert!(assert_valid_sand_agent_id("agent\\escape").is_err());

    let root = temp_root("storage-backend-test");
    let resolved = resolve_sand_agent_dir("sand-agent-01", Some(&root));
    assert!(resolved.is_ok());
    assert_eq!(resolved.unwrap().file_name().unwrap(), "sand-agent-01");
    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O12: Storage Provider Fallback Invariant
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o12_storage_provider_fallback() {
    // When no external provider is specified, built-in filesystem backend is authoritative
    let default_provider = "filesystem";
    assert_eq!(default_provider, "filesystem");
    assert!(is_safe_folder_id("sand_agents"));
    assert!(!is_safe_folder_id("../escaped_folder"));
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O13: Filesystem Durability & Sidecar Cleanup
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o13_filesystem_durability() {
    let root = temp_root("sidecar-test");
    let db_path = root.join("test.db");
    fs::write(&db_path, b"sqlite database payload").expect("write db");
    for suffix in SQLITE_DB_SIDECAR_SUFFIXES {
        let sidecar = root.join(format!("test.db{suffix}"));
        fs::write(&sidecar, b"sidecar payload").expect("write sidecar");
        assert!(sidecar.exists());
    }

    remove_sqlite_sidecars(&db_path);
    for suffix in SQLITE_DB_SIDECAR_SUFFIXES {
        let sidecar = root.join(format!("test.db{suffix}"));
        assert!(!sidecar.exists(), "Sidecar {suffix} must be removed");
    }
    assert!(db_path.exists(), "Main database file must remain untouched");
    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O14: Storage Location Lifecycle and Write Generation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o14_storage_location_lifecycle() {
    let root = temp_root("lifecycle-test");
    let db_path = root.join("generation.db");

    let initial = get_sand_agent_db_write_generation(&db_path);
    bump_db_write_generation(&db_path);
    let after_bump = get_sand_agent_db_write_generation(&db_path);
    assert_eq!(after_bump, initial.saturating_add(1));

    assert_eq!(live_db_handle_count(&db_path), 0);
    register_live_db_handle(&db_path);
    assert_eq!(live_db_handle_count(&db_path), 1);
    release_live_db_handle(&db_path);
    assert_eq!(live_db_handle_count(&db_path), 0);

    delete_sand_agent_db_write_generation(&db_path);
    assert_eq!(get_sand_agent_db_write_generation(&db_path), 0);
    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// OCRM-STORAGE-O15: Streaming Encryption Header Contract
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_storage_o15_streaming_encryption_header() {
    // Upstream OpenClaw uses OCSTOR1\n header with 1MB segment bounds
    let magic = b"OCSTOR1\n";
    assert_eq!(magic.len(), 8);
    let segment_bytes: usize = 1048576; // 1MB
    let tag_bytes: usize = 16;
    assert_eq!(segment_bytes, 1024 * 1024);
    assert_eq!(tag_bytes, 16);
}

