use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::transcript_mirror::transcript_journal_codec::{
    DeferredTranscriptStep, TranscriptCheckpoint,
};
use mahayana_host_runtime::transcript_mirror::transcript_mirror::{
    DerivedTranscriptOccurrences, FileTranscriptMirror, JournalOutcome,
    TranscriptDeriver, TranscriptOccurrence,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-transcript-mirror-{label}-{}-{suffix}",
        std::process::id()
    ))
}

struct FakeDeriver;

impl TranscriptDeriver<String> for FakeDeriver {
    fn derive(
        &self,
        _store: &String,
        previous: &TranscriptCheckpoint,
        checkpoint: &TranscriptCheckpoint,
        finalize_checkpoint: bool,
        deferred: Option<DeferredTranscriptStep>,
    ) -> Result<DerivedTranscriptOccurrences, String> {
        let start = previous.turns.len().saturating_sub(1);
        let mut occurrences = Vec::new();
        for index in start..checkpoint.turns.len() {
            if previous.turns.get(index) == checkpoint.turns.get(index)
                && deferred.map(|value| value.turn_index) != Some(index)
            {
                continue;
            }
            occurrences.push(TranscriptOccurrence {
                id: format!("turn:{index}:text"),
                line: serde_json::json!({
                    "role":"assistant",
                    "message":{"content":[{"type":"text","text":format!("turn-{index}")}]}
                })
                .to_string(),
            });
        }
        Ok(DerivedTranscriptOccurrences {
            occurrences,
            deferred_step: (!finalize_checkpoint).then_some(DeferredTranscriptStep {
                turn_index: checkpoint.turns.len().saturating_sub(1),
                step_index: 0,
            }),
        })
    }

    fn initial(
        &self,
        _store: &String,
        checkpoint: &TranscriptCheckpoint,
    ) -> Result<Vec<TranscriptOccurrence>, String> {
        Ok(checkpoint
            .turns
            .iter()
            .enumerate()
            .map(|(index, _)| TranscriptOccurrence {
                id: format!("turn:{index}:initial"),
                line: serde_json::json!({
                    "role":"user",
                    "message":{"content":[{"type":"text","text":format!("initial-{index}")}]}
                })
                .to_string(),
            })
            .collect())
    }
}

#[test]
fn journal_claim_recover_prepare_commit_owns_atomic_wal_lifecycle() {
    let root = temp_root("lifecycle");
    let reports = Arc::new(Mutex::new(Vec::<JournalOutcome>::new()));
    let reporter = {
        let reports = Arc::clone(&reports);
        Arc::new(move |outcome: &JournalOutcome| {
            reports.lock().expect("reports").push(outcome.clone());
        })
    };
    let mirror = FileTranscriptMirror::with_reporter(
        &root,
        reporter,
        Arc::new(FakeDeriver),
    );
    let store = "store".to_string();
    let first = TranscriptCheckpoint {
        turns: vec![vec![1]],
    };
    let second = TranscriptCheckpoint {
        turns: vec![vec![1], vec![2]],
    };

    assert!(!mirror.owns_conversation("agent-a").expect("ownership"));
    mirror.claim_conversation("agent-a").expect("claim");
    assert!(mirror.owns_conversation("agent-a").expect("ownership"));

    mirror
        .recover("agent-a", &first, &store)
        .expect("initial recovery");
    let jsonl = mirror.jsonl_path_for("agent-a").expect("jsonl");
    assert_eq!(fs::read_to_string(&jsonl).expect("initial jsonl").lines().count(), 1);

    mirror
        .prepare_checkpoint("agent-a", &second, &store, true)
        .expect("prepare");
    assert!(mirror.pending_path_for("agent-a").expect("pending").is_file());
    assert_eq!(fs::read_to_string(&jsonl).expect("pre-commit jsonl").lines().count(), 1);

    mirror.commit_checkpoint("agent-a").expect("commit");
    assert!(!mirror.pending_path_for("agent-a").expect("pending").exists());
    assert_eq!(fs::read_to_string(&jsonl).expect("committed jsonl").lines().count(), 2);
    assert_eq!(mirror.durable_checkpoint("agent-a"), Some(second));

    let reports = reports.lock().expect("reports");
    assert!(reports.iter().any(|outcome| outcome.op == "replay" && outcome.outcome == "ok"));
    assert!(reports.iter().any(|outcome| outcome.op == "checkpoint" && outcome.entry_count == Some(1)));
    assert!(reports.iter().any(|outcome| outcome.op == "append" && outcome.entry_count == Some(1)));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn recovery_replays_prepared_wal_after_durable_checkpoint_wins_crash_race() {
    let root = temp_root("replay");
    let store = "store".to_string();
    let first = TranscriptCheckpoint {
        turns: vec![vec![1]],
    };
    let second = TranscriptCheckpoint {
        turns: vec![vec![1], vec![2]],
    };

    {
        let mirror = FileTranscriptMirror::new(&root, Arc::new(FakeDeriver));
        mirror.claim_conversation("agent-a").expect("claim");
        mirror.recover("agent-a", &first, &store).expect("recover first");
        mirror
            .prepare_checkpoint("agent-a", &second, &store, true)
            .expect("prepare second");
    }

    let recovered = FileTranscriptMirror::new(&root, Arc::new(FakeDeriver));
    recovered
        .recover("agent-a", &second, &store)
        .expect("replay pending WAL");
    let jsonl = recovered.jsonl_path_for("agent-a").expect("jsonl");
    assert_eq!(fs::read_to_string(jsonl).expect("jsonl").lines().count(), 2);
    assert!(!recovered.pending_path_for("agent-a").expect("pending").exists());
    assert_eq!(recovered.durable_checkpoint("agent-a"), Some(second));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn prepare_rejects_canonical_file_mutation_outside_journal() {
    let root = temp_root("external");
    let mirror = FileTranscriptMirror::new(&root, Arc::new(FakeDeriver));
    let store = "store".to_string();
    let first = TranscriptCheckpoint {
        turns: vec![vec![1]],
    };
    let second = TranscriptCheckpoint {
        turns: vec![vec![1], vec![2]],
    };
    mirror.recover("agent-a", &first, &store).expect("recover");

    let jsonl = mirror.jsonl_path_for("agent-a").expect("jsonl");
    let mut file = OpenOptions::new().append(true).open(&jsonl).expect("open");
    writeln!(
        file,
        "{}",
        serde_json::json!({"role":"user","message":{"content":[]}})
    )
    .expect("external append");

    let error = mirror
        .prepare_checkpoint("agent-a", &second, &store, true)
        .expect_err("external mutation must fail");
    assert!(error.to_string().contains("changed outside the journal"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn abort_and_skip_remove_pending_and_deferred_state() {
    let root = temp_root("abort-skip");
    let mirror = FileTranscriptMirror::new(&root, Arc::new(FakeDeriver));
    let store = "store".to_string();
    let first = TranscriptCheckpoint {
        turns: vec![vec![1]],
    };
    let second = TranscriptCheckpoint {
        turns: vec![vec![1], vec![2]],
    };
    mirror.recover("agent-a", &first, &store).expect("recover");

    mirror
        .prepare_checkpoint("agent-a", &second, &store, false)
        .expect("prepare");
    mirror.abort_checkpoint("agent-a").expect("abort");
    assert!(!mirror.pending_path_for("agent-a").expect("pending").exists());

    mirror
        .prepare_checkpoint("agent-a", &second, &store, false)
        .expect("prepare again");
    mirror
        .skip_checkpoint("agent-a", &second)
        .expect("skip");
    assert!(!mirror.pending_path_for("agent-a").expect("pending").exists());
    assert!(!mirror.cursor_path_for("agent-a").expect("cursor").exists());
    assert_eq!(mirror.durable_checkpoint("agent-a"), Some(second));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unsafe_conversation_ids_are_rejected_before_filesystem_access() {
    let root = temp_root("unsafe");
    let mirror = FileTranscriptMirror::new(&root, Arc::new(FakeDeriver));
    assert!(mirror.jsonl_path_for("../escape").is_err());
    assert!(mirror.claim_conversation("..").is_err());
    assert!(!root.exists());
}
