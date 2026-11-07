use std::sync::{Arc, Mutex};

use futures::future;
use mahayana_host_runtime::runner::inactive_turn_agent_stream::{
    InactiveTurnAgentLifecycleHooks, InactiveTurnAgentOutputSink,
    InactiveTurnAgentStreamPath, InactiveTurnAgentStreamSource,
    InactiveTurnCheckpointSink, InactiveTurnStreamFuture,
};
use mahayana_host_runtime::runner::{
    OuterStreamFuture, OuterStreamPersistence, StreamCancelReason,
};

#[derive(Default)]
struct Events(Mutex<Vec<String>>);

impl Events {
    fn push(&self, event: impl Into<String>) {
        self.0.lock().expect("events").push(event.into());
    }

    fn snapshot(&self) -> Vec<String> {
        self.0.lock().expect("events").clone()
    }
}

struct Persistence<'a> {
    events: &'a Events,
    generation: u64,
    active_generation: u64,
}

impl OuterStreamPersistence<String, String> for Persistence<'_> {
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
        _context: &'a String,
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
        _context: &'a String,
        checkpoint: &'a String,
    ) -> OuterStreamFuture<'a, Result<(), String>> {
        self.events.push(format!("final:{checkpoint}"));
        Box::pin(future::ready(Ok(())))
    }

    fn commit_disk_pressure_reminder(&self) {
        self.events.push("commit");
    }

    fn release_disk_pressure_reminder(&self) {
        self.events.push("release");
    }

    fn is_awaiting_user_selection(&self) -> bool {
        false
    }

    fn is_quiescing_for_upgrade(&self) -> bool {
        false
    }

    fn mark_quiesced_for_upgrade(&self) {
        self.events.push("mark-quiesced");
    }

    fn cancel_run(&self, cancellation: StreamCancelReason) {
        self.events.push(format!("cancel:{}", cancellation.reason));
    }
}

struct Source(Arc<Events>);

impl InactiveTurnAgentStreamSource<String, String> for Source {
    type Error = String;

    fn start_stream<'a>(
        &'a self,
        context: &'a String,
        resume_from: Option<&'a String>,
        persist_checkpoint: &'a mut dyn InactiveTurnCheckpointSink<String, String>,
        _output: &'a mut dyn InactiveTurnAgentOutputSink,
    ) -> InactiveTurnStreamFuture<'a, Result<String, String>> {
        Box::pin(async move {
            self.0.push(format!(
                "start:{}:{}",
                context,
                resume_from.map(String::as_str).unwrap_or("none")
            ));
            let mut checkpoint = "checkpoint".to_string();
            persist_checkpoint.persist(context, &mut checkpoint).await?;
            self.0.push(format!("accepted:{checkpoint}"));
            Ok(format!("{checkpoint}:completed"))
        })
    }
}

struct Hooks<'a>(&'a Events);

impl InactiveTurnAgentLifecycleHooks<String> for Hooks<'_> {
    fn on_completed<'a>(
        &'a self,
        state: &'a String,
    ) -> InactiveTurnStreamFuture<'a, Result<(), String>> {
        self.0.push(format!("completed:{state}"));
        Box::pin(future::ready(Ok(())))
    }

    fn cleanup<'a>(&'a self) -> InactiveTurnStreamFuture<'a, Result<(), String>> {
        self.0.push("cleanup");
        Box::pin(future::ready(Ok(())))
    }
}

#[test]
fn inactive_generated_agent_stream_drains_checkpoint_and_owns_outer_lifecycle() {
    let events = Arc::new(Events::default());
    let source: Arc<dyn InactiveTurnAgentStreamSource<String, String, Error = String>> =
        Arc::new(Source(Arc::clone(&events)));
    let path = InactiveTurnAgentStreamPath::new(source);
    let persistence = Persistence {
        events: events.as_ref(),
        generation: 7,
        active_generation: 7,
    };
    let hooks = Hooks(events.as_ref());
    let context = "ctx".to_string();
    let resume = "resume".to_string();

    let state = futures::executor::block_on(path.run_lifecycle(
        &context,
        Some(&resume),
        &persistence,
        &hooks,
    ))
    .expect("inactive stream lifecycle");

    assert_eq!(state, "checkpoint:prepared:completed");
    assert_eq!(
        events.snapshot(),
        vec![
            "start:ctx:resume",
            "prepare",
            "step:checkpoint:prepared",
            "note:checkpoint:prepared",
            "commit",
            "accepted:checkpoint:prepared",
            "completed:checkpoint:prepared:completed",
            "cleanup",
            "final:checkpoint:prepared:completed",
            "commit",
            "release",
        ]
    );
}

#[test]
fn inactive_generated_agent_stream_skips_stale_checkpoint_persistence_but_releases_claim() {
    let events = Arc::new(Events::default());
    let source: Arc<dyn InactiveTurnAgentStreamSource<String, String, Error = String>> =
        Arc::new(Source(Arc::clone(&events)));
    let path = InactiveTurnAgentStreamPath::new(source);
    let persistence = Persistence {
        events: events.as_ref(),
        generation: 7,
        active_generation: 8,
    };
    let hooks = Hooks(events.as_ref());
    let context = "ctx".to_string();

    let state = futures::executor::block_on(path.run_lifecycle(
        &context,
        None,
        &persistence,
        &hooks,
    ))
    .expect("stale generation lifecycle");

    assert_eq!(state, "checkpoint:completed");
    assert_eq!(
        events.snapshot(),
        vec![
            "start:ctx:none",
            "accepted:checkpoint",
            "completed:checkpoint:completed",
            "cleanup",
            "release",
        ]
    );
}
