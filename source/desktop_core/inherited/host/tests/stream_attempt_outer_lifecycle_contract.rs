use std::sync::Mutex;

use futures::future;
use mahayana_host_runtime::runner::{
    OuterCheckpointDisposition, OuterStreamFuture, OuterStreamPersistence,
    StreamAttemptPolicy, StreamAttemptRuntime, StreamCancelReason,
    persist_outer_stream_checkpoint,
    persist_outer_stream_final_state, release_outer_stream_persistence,
};

#[derive(Default)]
struct Events(Mutex<Vec<String>>);

impl Events {
    fn push(&self, value: impl Into<String>) {
        self.0.lock().expect("events").push(value.into());
    }

    fn snapshot(&self) -> Vec<String> {
        self.0.lock().expect("events").clone()
    }
}

struct Persistence<'a> {
    events: &'a Events,
    generation: u64,
    active_generation: u64,
    awaiting: bool,
    quiescing: bool,
}

impl OuterStreamPersistence<(), String> for Persistence<'_> {
    fn generation(&self) -> u64 {
        self.generation
    }

    fn run_generation(&self) -> u64 {
        self.active_generation
    }

    fn prepare_checkpoint_for_persistence(&self, checkpoint: &mut String) {
        self.events.push("prepare");
        checkpoint.push_str(":prepared");
    }

    fn persist_step_checkpoint<'a>(
        &'a self,
        _context: &'a (),
        checkpoint: &'a String,
    ) -> OuterStreamFuture<'a, Result<(), String>> {
        self.events.push(format!("step:{checkpoint}"));
        Box::pin(future::ready(Ok(())))
    }

    fn note_checkpoint(&self, checkpoint: &String) {
        self.events.push(format!("note:{checkpoint}"));
    }

    fn persist_final_state<'a>(
        &'a self,
        _context: &'a (),
        checkpoint: &'a String,
    ) -> OuterStreamFuture<'a, Result<(), String>> {
        self.events.push(format!("final:{checkpoint}"));
        Box::pin(future::ready(Ok(())))
    }

    fn commit_disk_pressure_reminder(&self) {
        self.events.push("commit-disk");
    }

    fn release_disk_pressure_reminder(&self) {
        self.events.push("release-disk");
    }

    fn note_automation_status_reminder(&self) {
        self.events.push("automation");
    }

    fn is_awaiting_user_selection(&self) -> bool {
        self.awaiting
    }

    fn is_quiescing_for_upgrade(&self) -> bool {
        self.quiescing
    }

    fn mark_quiesced_for_upgrade(&self) {
        self.events.push("mark-quiesced");
    }

    fn cancel_run(&self, cancellation: StreamCancelReason) {
        self.events.push(format!(
            "cancel:{}:{}",
            cancellation.intentional, cancellation.reason
        ));
    }
}

#[test]
fn outer_checkpoint_is_generation_gated_and_preserves_frozen_order() {
    let events = Events::default();
    let persistence = Persistence {
        events: &events,
        generation: 7,
        active_generation: 7,
        awaiting: false,
        quiescing: false,
    };
    let mut checkpoint = "state".to_string();
    let outcome = futures::executor::block_on(
        persist_outer_stream_checkpoint(&persistence, &(), &mut checkpoint),
    )
    .expect("checkpoint");
    assert_eq!(outcome, OuterCheckpointDisposition::Persisted);
    assert_eq!(checkpoint, "state:prepared");
    assert_eq!(
        events.snapshot(),
        vec![
            "prepare",
            "step:state:prepared",
            "note:state:prepared",
            "commit-disk",
            "automation",
        ]
    );

    let stale_events = Events::default();
    let stale = Persistence {
        events: &stale_events,
        generation: 7,
        active_generation: 8,
        awaiting: false,
        quiescing: false,
    };
    let mut stale_checkpoint = "state".to_string();
    assert_eq!(
        futures::executor::block_on(
            persist_outer_stream_checkpoint(&stale, &(), &mut stale_checkpoint),
        )
        .expect("stale"),
        OuterCheckpointDisposition::IgnoredStaleGeneration
    );
    assert!(stale_events.snapshot().is_empty());
    assert_eq!(stale_checkpoint, "state");
}

#[test]
fn awaiting_and_upgrade_checkpoint_settlement_cancel_for_frozen_reasons() {
    let awaiting_events = Events::default();
    let awaiting = Persistence {
        events: &awaiting_events,
        generation: 1,
        active_generation: 1,
        awaiting: true,
        quiescing: true,
    };
    let mut state = "a".to_string();
    futures::executor::block_on(
        persist_outer_stream_checkpoint(&awaiting, &(), &mut state),
    )
    .expect("awaiting");
    assert!(awaiting_events
        .snapshot()
        .contains(&"cancel:true:awaiting user selection".to_string()));
    assert!(!awaiting_events
        .snapshot()
        .contains(&"mark-quiesced".to_string()));

    let upgrade_events = Events::default();
    let upgrade = Persistence {
        events: &upgrade_events,
        generation: 2,
        active_generation: 2,
        awaiting: false,
        quiescing: true,
    };
    let mut state = "b".to_string();
    futures::executor::block_on(
        persist_outer_stream_checkpoint(&upgrade, &(), &mut state),
    )
    .expect("upgrade");
    assert!(upgrade_events.snapshot().contains(&"mark-quiesced".to_string()));
    assert!(upgrade_events.snapshot().contains(
        &"cancel:true:quiescing for forced host upgrade".to_string()
    ));
}

#[test]
fn final_state_commit_and_release_are_separate_outer_finally_steps() {
    let events = Events::default();
    let persistence = Persistence {
        events: &events,
        generation: 3,
        active_generation: 3,
        awaiting: false,
        quiescing: false,
    };
    assert_eq!(
        futures::executor::block_on(
            persist_outer_stream_final_state(&persistence, &(), &"final".to_string()),
        )
        .expect("final state"),
        OuterCheckpointDisposition::Persisted
    );
    release_outer_stream_persistence(&persistence);
    assert_eq!(
        events.snapshot(),
        vec!["final:final", "commit-disk", "release-disk"]
    );
}


#[test]
fn stream_attempt_runtime_fences_stale_generation_and_owns_resettable_deadline() {
    let runtime = StreamAttemptRuntime::new(StreamAttemptPolicy::default());
    let first = runtime.begin_attempt();
    let first_epoch = runtime.deadline_epoch(first).expect("first epoch");
    assert!(runtime.reset_first_output_deadline(first));
    assert!(
        runtime.deadline_epoch(first).expect("reset epoch") > first_epoch,
        "deadline reset must be observable by the watchdog owner"
    );

    let second = runtime.begin_attempt();
    assert!(!runtime.is_current(first));
    assert!(!runtime.mark_stream_output(first));
    assert!(!runtime.reset_first_output_deadline(first));
    assert!(runtime.is_current(second));
    assert!(runtime.deadline_armed(second));
    assert!(runtime.mark_stream_output(second));
    assert!(!runtime.deadline_armed(second));
    assert!(runtime.stream_output_produced(second));
    assert!(runtime.settle_attempt(second));
    assert!(!runtime.is_current(second));
}
