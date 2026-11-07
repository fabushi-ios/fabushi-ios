use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agent_isolation::transcript_mirror_worker::{
    TranscriptMirrorWorkerJob, run_transcript_mirror_worker_job,
};
use rusqlite::params;

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-transcript-worker-{label}-{}-{nanos}",
        std::process::id()
    ))
}

fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn push_bytes(field: u64, value: &[u8], out: &mut Vec<u8>) {
    encode_varint((field << 3) | 2, out);
    encode_varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

fn insert_blob(path: &Path, id: &[u8], data: &[u8]) {
    let db = rusqlite::Connection::open(path).expect("db");
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB NOT NULL) STRICT;",
    )
    .expect("schema");
    db.execute(
        "INSERT OR REPLACE INTO blobs (id, data) VALUES (?1, ?2)",
        params![id.iter().map(|byte| format!("{byte:02x}")).collect::<String>(), data],
    )
    .expect("insert");
}

#[test]
fn worker_reads_checkpoint_and_message_across_configured_sqlite_paths() {
    let root = temp_root("write");
    fs::create_dir_all(&root).expect("root");
    let first_db = root.join("conversation-blobs.db");
    let second_db = root.join("store.db");
    let transcripts = root.join("transcripts");

    let message_id = vec![0x11; 32];
    let state_id = vec![0x22; 32];
    insert_blob(&second_db, &message_id, br#"{"role":"user","content":"worker hello"}"#);
    let mut state = Vec::new();
    push_bytes(1, &message_id, &mut state);
    insert_blob(&first_db, &state_id, &state);

    let written = run_transcript_mirror_worker_job(&TranscriptMirrorWorkerJob {
        conversation_id: "agent-worker".into(),
        state_blob_id: state_id,
        blob_db_paths: vec![first_db, second_db],
        transcripts_dir: transcripts.clone(),
    })
    .expect("worker write");
    assert!(written);
    let content = fs::read_to_string(
        transcripts.join("agent-worker").join("agent-worker.jsonl"),
    )
    .expect("transcript");
    assert!(content.contains("worker hello"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn worker_reports_missing_checkpoint_blob() {
    let root = temp_root("missing");
    fs::create_dir_all(&root).expect("root");
    let db = root.join("conversation-blobs.db");
    insert_blob(&db, &[1], b"unrelated");
    let error = run_transcript_mirror_worker_job(&TranscriptMirrorWorkerJob {
        conversation_id: "agent-worker".into(),
        state_blob_id: vec![9],
        blob_db_paths: vec![db],
        transcripts_dir: root.join("transcripts"),
    })
    .expect_err("missing checkpoint");
    assert!(error.to_string().contains("unavailable"));
    let _ = fs::remove_dir_all(root);
}
