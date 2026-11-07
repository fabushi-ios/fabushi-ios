use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::FutureExt;
use mahayana_host_runtime::agents::agent_profile::read_sand_profile_file;
use mahayana_host_runtime::agents::settings_file::read_sand_settings_file;
use mahayana_host_runtime::extensions::session::session_recovery::{
    cache_blob_reads, ensure_profile_file, ensure_settings_file,
    transcript_entry_matches_recovered,
};
use serde_json::json;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-session-recovery-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn recovered_transcript_matching_uses_frozen_kind_specific_fields() {
    assert!(transcript_entry_matches_recovered(
        &json!({"kind":"message","role":"user","content":"hello","ignored":1}),
        &json!({"kind":"message","role":"user","content":"hello","ignored":2}),
    ));
    assert!(!transcript_entry_matches_recovered(
        &json!({"kind":"message","role":"user","content":"hello"}),
        &json!({"kind":"message","role":"assistant","content":"hello"}),
    ));
    assert!(transcript_entry_matches_recovered(
        &json!({"kind":"send-message","message":{"type":"text","content":"hello"}}),
        &json!({"kind":"send-message","message":{"type":"text","content":"hello"}}),
    ));
    assert!(transcript_entry_matches_recovered(
        &json!({"kind":"tool-call","name":"search","status":"done","summary":"ok","other":1}),
        &json!({"kind":"tool-call","name":"search","status":"done","summary":"ok","other":2}),
    ));
    assert!(!transcript_entry_matches_recovered(
        &json!({"kind":"notice","text":"x"}),
        &json!({"kind":"notice","text":"x"}),
    ));
}

#[test]
fn blob_read_cache_coalesces_reads_and_write_through_updates_cached_value() {
    let reads = Arc::new(AtomicUsize::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let flushes = Arc::new(AtomicUsize::new(0));

    let store = cache_blob_reads(
        {
            let reads = Arc::clone(&reads);
            move |id| {
                let reads = Arc::clone(&reads);
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    Ok(Some(id))
                }
                .boxed()
            }
        },
        {
            let writes = Arc::clone(&writes);
            move |_id, _data| {
                let writes = Arc::clone(&writes);
                async move {
                    writes.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
                .boxed()
            }
        },
        {
            let flushes = Arc::clone(&flushes);
            move || {
                let flushes = Arc::clone(&flushes);
                async move {
                    flushes.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
                .boxed()
            }
        },
    );

    let first = store.get_blob(&[1, 2, 3]);
    let second = store.get_blob(&[1, 2, 3]);
    let (first, second) = futures::executor::block_on(async { futures::join!(first, second) });
    assert_eq!(first.expect("first read"), Some(vec![1, 2, 3]));
    assert_eq!(second.expect("second read"), Some(vec![1, 2, 3]));
    assert_eq!(reads.load(Ordering::SeqCst), 1);

    futures::executor::block_on(store.set_blob(&[1, 2, 3], vec![9]))
        .expect("set blob");
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        futures::executor::block_on(store.get_blob(&[1, 2, 3])).expect("cached read"),
        Some(vec![9])
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1);

    futures::executor::block_on(store.flush()).expect("flush");
    assert_eq!(flushes.load(Ordering::SeqCst), 1);
}

#[test]
fn recovery_file_helpers_create_missing_profile_and_settings_without_overwriting() {
    let root = temp_root("files");
    let agent_dir = root.join("agent-a");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    fs::write(&db_path, b"").expect("db placeholder");

    let profile_path = ensure_profile_file(&db_path, Some("  Agent A  "), "  description  ")
        .expect("profile path");
    let profile = read_sand_profile_file(&profile_path).expect("profile");
    assert_eq!(profile.name, "Agent A");
    assert_eq!(profile.description, "description");

    fs::write(
        &profile_path,
        r#"{"name":"Keep Me","description":"existing","title":"","avatarShape":"","avatarColor":""}"#,
    )
    .expect("replace profile");
    ensure_profile_file(&db_path, Some("Changed"), "changed").expect("existing profile");
    assert_eq!(
        read_sand_profile_file(&profile_path)
            .expect("preserved profile")
            .name,
        "Keep Me"
    );

    let settings_path = ensure_settings_file(&db_path).expect("settings path");
    let settings = read_sand_settings_file(&settings_path);
    assert!(settings.notify_on_agent_updates);
    assert!(!settings.hidden_from_sidebar);
    assert_eq!(
        fs::read_to_string(&settings_path).expect("settings text"),
        "{}\n"
    );

    let _ = fs::remove_dir_all(root);
}
