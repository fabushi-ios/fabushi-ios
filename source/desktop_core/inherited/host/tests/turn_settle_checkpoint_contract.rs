use std::sync::Mutex;

use futures::future;
use mahayana_host_runtime::runner::{
    DurableTurnCheckpointStore, SettledTokenDetails, TokenDetailsPersistenceTracker,
    TranscriptCheckpointMirror, TurnCheckpointFuture, TurnCheckpointPersistenceError,
    persist_checkpoint_with_mirror, should_run_completed_turn_side_effects,
    should_run_turn_memory,
};

#[derive(Default)]
struct Events(Mutex<Vec<String>>);

impl Events {
    fn push(&self, value: impl Into<String>) {
        self.0.lock().unwrap().push(value.into());
    }

    fn snapshot(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

struct Mirror<'a> {
    events: &'a Events,
}

impl TranscriptCheckpointMirror<String, Vec<u8>> for Mirror<'_> {
    fn prepare_checkpoint<'a>(
        &'a self,
        _transcript_id: &'a str,
        _checkpoint: &'a String,
        _blob_store: &'a Vec<u8>,
        finalize: bool,
        force: bool,
    ) -> TurnCheckpointFuture<'a, Result<(), String>> {
        self.events.push(format!("prepare:{finalize}:{force}"));
        Box::pin(future::ready(Ok(())))
    }

    fn abort_checkpoint<'a>(
        &'a self,
        _transcript_id: &'a str,
    ) -> TurnCheckpointFuture<'a, Result<(), String>> {
        self.events.push("abort");
        Box::pin(future::ready(Ok(())))
    }

    fn commit_checkpoint<'a>(
        &'a self,
        _transcript_id: &'a str,
        root: Option<&'a str>,
    ) -> TurnCheckpointFuture<'a, Result<(), String>> {
        self.events.push(format!("commit:{}", root.unwrap_or("none")));
        Box::pin(future::ready(Ok(())))
    }

    fn skip_checkpoint<'a>(
        &'a self,
        _transcript_id: &'a str,
        _checkpoint: &'a String,
        _blob_store: &'a Vec<u8>,
    ) -> TurnCheckpointFuture<'a, Result<(), String>> {
        self.events.push("skip");
        Box::pin(future::ready(Ok(())))
    }
}

struct Store<'a> {
    events: &'a Events,
    fail: bool,
}

impl DurableTurnCheckpointStore<String> for Store<'_> {
    fn handle_checkpoint<'a>(
        &'a self,
        _checkpoint: &'a String,
    ) -> TurnCheckpointFuture<'a, Result<(), String>> {
        self.events.push("durable");
        Box::pin(future::ready(if self.fail {
            Err("disk".into())
        } else {
            Ok(())
        }))
    }

    fn latest_root_blob_id(&self) -> Option<String> {
        Some("root-1".into())
    }
}

#[test]
fn mirror_prepare_precedes_durable_checkpoint_and_commit_follows_it() {
    let events = Events::default();
    let mirror = Mirror { events: &events };
    let store = Store {
        events: &events,
        fail: false,
    };
    futures::executor::block_on(persist_checkpoint_with_mirror(
        Some(&mirror),
        Some(&store),
        "agent-a",
        &"checkpoint".to_string(),
        &vec![],
        true,
        false,
        true,
        |_| Ok(()),
    ))
    .expect("checkpoint");
    assert_eq!(
        events.snapshot(),
        vec![
            "prepare:true:true",
            "durable",
            "commit:root-1",
        ]
    );
}

#[test]
fn durable_failure_best_effort_aborts_prepared_mirror() {
    let events = Events::default();
    let mirror = Mirror { events: &events };
    let store = Store {
        events: &events,
        fail: true,
    };
    let error = futures::executor::block_on(persist_checkpoint_with_mirror(
        Some(&mirror),
        Some(&store),
        "agent-a",
        &"checkpoint".to_string(),
        &vec![],
        false,
        false,
        true,
        |_| Ok(()),
    ))
    .expect_err("durable failure");
    assert_eq!(
        error,
        TurnCheckpointPersistenceError::Durable("disk".into())
    );
    assert_eq!(
        events.snapshot(),
        vec!["prepare:false:false", "durable", "abort"]
    );
}

#[test]
fn disabled_journal_skips_after_durable_checkpoint() {
    let events = Events::default();
    let mirror = Mirror { events: &events };
    let store = Store {
        events: &events,
        fail: false,
    };
    futures::executor::block_on(persist_checkpoint_with_mirror(
        Some(&mirror),
        Some(&store),
        "agent-a",
        &"checkpoint".to_string(),
        &vec![],
        false,
        false,
        false,
        |_| Ok(()),
    ))
    .expect("checkpoint");
    assert_eq!(events.snapshot(), vec!["durable", "skip"]);
}

#[test]
fn local_state_fallback_commits_without_root_metadata() {
    let events = Events::default();
    let mirror = Mirror { events: &events };
    futures::executor::block_on(
        persist_checkpoint_with_mirror::<String, Vec<u8>, _, Store<'_>, _>(
            Some(&mirror),
            None,
            "agent-a",
            &"checkpoint".to_string(),
            &vec![],
            false,
            true,
            true,
            |_| {
                events.push("local");
                Ok(())
            },
        ),
    )
    .expect("local checkpoint");
    assert_eq!(
        events.snapshot(),
        vec!["prepare:false:true", "local", "commit:none"]
    );
}


#[test]
fn summary_archive_growth_suppresses_only_the_stale_token_snapshot() {
    let stale = SettledTokenDetails {
        used_tokens: 120,
        max_tokens: 1_000,
    };
    let fresh = SettledTokenDetails {
        used_tokens: 140,
        max_tokens: 1_000,
    };
    let mut tracker = TokenDetailsPersistenceTracker::new(2);

    assert!(!tracker.should_suppress(2, Some(stale)));
    assert!(tracker.should_suppress(3, Some(stale)));
    assert!(
        tracker.should_suppress(3, Some(stale)),
        "the exact pre-compaction snapshot stays stale"
    );
    assert!(
        !tracker.should_suppress(3, Some(fresh)),
        "a genuinely new token snapshot becomes persistable"
    );
}

#[test]
fn completed_turn_side_effect_policy_matches_frozen_owner_boundaries() {
    assert!(should_run_completed_turn_side_effects(
        false, false, false, false, true,
    ));
    assert!(!should_run_completed_turn_side_effects(
        true, false, false, false, true,
    ));
    assert!(!should_run_completed_turn_side_effects(
        false, true, false, false, true,
    ));
    assert!(!should_run_completed_turn_side_effects(
        false, false, true, false, true,
    ));
    assert!(!should_run_completed_turn_side_effects(
        false, false, false, true, true,
    ));
    assert!(!should_run_completed_turn_side_effects(
        false, false, false, false, false,
    ));
}

#[test]
fn turn_memory_evidence_bypasses_memorable_filter_but_stale_runs_do_not_write() {
    assert!(should_run_turn_memory(
        true, false, false, true, true, false,
    ));
    assert!(!should_run_turn_memory(
        false, false, false, true, true, true,
    ));
    assert!(!should_run_turn_memory(
        true, false, false, true, false, false,
    ));
    assert!(should_run_turn_memory(
        true, false, false, true, false, true,
    ));
}
