use std::path::{Path, PathBuf};
use std::sync::Arc;

use mahayana_host_runtime::agent_isolation::{
    AgentWorkerPool, create_production_agent_store_worker_backend,
    open_configured_conversation_blob_db,
};
use mahayana_host_runtime::extensions::session::conversation_size_limits::{
    ConversationGcTarget, ConversationSizeLimits, ConversationSizeMaintenance,
    ConversationSizePolicy, HARD_LIMIT_DEFAULT_BYTES, SOFT_LIMIT_DEFAULT_BYTES,
    SandConversationTooLargeError, run_conversation_gc,
};
use rusqlite::params;
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("fabushi-{label}-{}.sqlite", Uuid::new_v4()))
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }
}

fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn push_len(field: u64, value: &[u8], out: &mut Vec<u8>) {
    encode_varint((field << 3) | 2, out);
    encode_varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

fn digest_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn seed(db_path: &Path, rows: &[(&str, &[u8])]) {
    let db = open_configured_conversation_blob_db(db_path, 5_000).expect("open db");
    for (id, data) in rows {
        db.execute(
            "INSERT INTO blobs (id, data) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            params![id, data],
        )
        .expect("seed");
    }
}

#[test]
fn policy_matches_grok_default_config_and_env_precedence() {
    let mut env = std::collections::BTreeMap::from([
        ("SAND_CONVERSATION_GC".to_string(), "on".to_string()),
        ("SAND_CONVERSATION_SOFT_LIMIT_BYTES".to_string(), "12345.9".to_string()),
    ]);
    let policy = ConversationSizePolicy::from_lookup(
        |name| env.remove(name),
        ConversationSizeLimits {
            soft_limit_mb: Some(99.0),
            hard_limit_mb: Some(2.0),
        },
        false,
    );
    assert_eq!(
        policy,
        ConversationSizePolicy {
            enabled: true,
            soft_limit_bytes: 12_345,
            hard_limit_bytes: 2 * 1024 * 1024,
        }
    );

    let defaults = ConversationSizePolicy::from_lookup(
        |_| None,
        ConversationSizeLimits::default(),
        false,
    );
    assert_eq!(defaults.soft_limit_bytes, SOFT_LIMIT_DEFAULT_BYTES);
    assert_eq!(defaults.hard_limit_bytes, HARD_LIMIT_DEFAULT_BYTES);
    assert!(!defaults.enabled);
}

#[test]
fn hard_turn_gate_collects_orphans_before_runner_execution() {
    let blob_db = path("capacity-collect");
    let session_db = path("capacity-session");

    let reachable = b"reachable".to_vec();
    let reachable_id = Sha256::digest(&reachable).to_vec();
    let reachable_hex = digest_hex(&reachable);
    let orphan = vec![0x5a; 512 * 1024];
    let orphan_hex = digest_hex(&orphan);
    let mut root = Vec::new();
    push_len(1, &reachable_id, &mut root);
    let root_hex = digest_hex(&root);

    seed(
        &blob_db,
        &[
            (&reachable_hex, reachable.as_slice()),
            (&orphan_hex, orphan.as_slice()),
            (&root_hex, root.as_slice()),
        ],
    );

    let pool = Arc::new(AgentWorkerPool::new(
        create_production_agent_store_worker_backend(5_000),
    ));
    ConversationSizeMaintenance::default()
        .ensure_conversation_capacity_for_turn(
            Arc::clone(&pool),
            ConversationGcTarget {
                agent_id: "agent-capacity".into(),
                blob_db_path: blob_db.clone(),
                legacy_blob_db_path: session_db.clone(),
                retained_root_id_hex: root_hex,
            },
            ConversationSizePolicy {
                enabled: true,
                soft_limit_bytes: 1,
                hard_limit_bytes: 64 * 1024,
            },
        )
        .expect("gc should shrink below the hard limit");

    futures::executor::block_on(pool.close_all());
    let db = open_configured_conversation_blob_db(&blob_db, 5_000).expect("reopen db");
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM blobs WHERE id = ?1",
            params![orphan_hex],
            |row| row.get(0),
        )
        .expect("count orphan");
    assert_eq!(count, 0);
    drop(db);
    cleanup(&blob_db);
    cleanup(&session_db);
}

#[test]
fn hard_turn_gate_rejects_only_after_successful_gc_stays_over_cap() {
    let blob_db = path("capacity-too-large");
    let session_db = path("capacity-too-large-session");

    let reachable = vec![0x2a; 512 * 1024];
    let reachable_id = Sha256::digest(&reachable).to_vec();
    let reachable_hex = digest_hex(&reachable);
    let mut root = Vec::new();
    push_len(1, &reachable_id, &mut root);
    let root_hex = digest_hex(&root);

    seed(
        &blob_db,
        &[(&reachable_hex, reachable.as_slice()), (&root_hex, root.as_slice())],
    );

    let pool = Arc::new(AgentWorkerPool::new(
        create_production_agent_store_worker_backend(5_000),
    ));
    let result = ConversationSizeMaintenance::default().ensure_conversation_capacity_for_turn(
        Arc::clone(&pool),
        ConversationGcTarget {
            agent_id: "agent-too-large".into(),
            blob_db_path: blob_db.clone(),
            legacy_blob_db_path: session_db.clone(),
            retained_root_id_hex: root_hex,
        },
        ConversationSizePolicy {
            enabled: true,
            soft_limit_bytes: 1,
            hard_limit_bytes: 64 * 1024,
        },
    );

    assert!(matches!(
        result,
        Err(SandConversationTooLargeError {
            limit_bytes: 65_536,
            ..
        })
    ));
    futures::executor::block_on(pool.close_all());
    cleanup(&blob_db);
    cleanup(&session_db);
}


#[test]
fn empty_persisted_root_returns_no_root_verdict() {
    let blob_db = path("capacity-no-root");
    let session_db = path("capacity-no-root-session");
    let pool = Arc::new(AgentWorkerPool::new(
        create_production_agent_store_worker_backend(5_000),
    ));
    let target = ConversationGcTarget {
        agent_id: "agent-no-root".into(),
        blob_db_path: blob_db.clone(),
        legacy_blob_db_path: session_db.clone(),
        retained_root_id_hex: String::new(),
    };

    assert_eq!(
        run_conversation_gc(pool.as_ref(), &target).expect("no-root verdict"),
        None
    );

    futures::executor::block_on(pool.close_all());
    cleanup(&blob_db);
    cleanup(&session_db);
}
