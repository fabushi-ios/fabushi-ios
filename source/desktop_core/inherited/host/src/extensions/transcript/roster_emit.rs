use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::extensions::session::agent_session::SandAgentSessionStore;
use crate::extensions::session::production::ProductionSessionWorkers;

use super::production_runtime::ProductionTranscriptRuntime;
use super::replica_writer::ReplicaStamp;
use super::roster_projection::{
    OutlineQueueResult, OutlineStreamCoalescingPolicy, OutlineUpdate, RosterProjection,
};

pub const ROSTER_REPLICA_KEY: &str = "roster";

pub type RosterEventSink = Arc<dyn Fn(Value) + Send + Sync + 'static>;
pub type TimelineWakeSink = Arc<dyn Fn(&str, Value) + Send + Sync + 'static>;

struct OutlineStreamSchedulerState {
    policy: Option<OutlineStreamCoalescingPolicy<Value>>,
    shutdown: bool,
}

struct OutlineStreamScheduler {
    started_at: Instant,
    shared: Arc<(Mutex<OutlineStreamSchedulerState>, Condvar)>,
    worker: Mutex<Option<JoinHandle<()>>>,
    event_sink: RosterEventSink,
}

impl OutlineStreamScheduler {
    fn new(event_sink: RosterEventSink) -> Self {
        let started_at = Instant::now();
        let shared = Arc::new((
            Mutex::new(OutlineStreamSchedulerState {
                policy: None,
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let worker_shared = Arc::clone(&shared);
        let worker_sink = Arc::clone(&event_sink);
        let worker_started_at = started_at.clone();
        let worker = thread::Builder::new()
            .name("fabushi-outline-coalescer".to_string())
            .spawn(move || run_outline_stream_worker(worker_started_at, worker_shared, worker_sink))
            .expect("outline stream coalescing worker must start");
        Self {
            started_at,
            shared,
            worker: Mutex::new(Some(worker)),
            event_sink,
        }
    }

    fn now_ms(&self) -> u64 {
        monotonic_ms(&self.started_at)
    }

    fn configure(&self, delay_ms: u64) {
        let (state, wake) = &*self.shared;
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.policy = Some(OutlineStreamCoalescingPolicy::new(delay_ms, self.now_ms()));
        wake.notify_all();
    }

    fn delay_ms(&self) -> u64 {
        let (state, _) = &*self.shared;
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .policy
            .as_ref()
            .map_or(0, |policy| policy.delay_ms())
    }

    fn queue(&self, agent_id: &str, item_id: &str, item: Value) {
        let now_ms = self.now_ms();
        let (state, wake) = &*self.shared;
        let result = {
            let mut state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(policy) = state.policy.as_mut() {
                policy.queue(now_ms, agent_id, item_id, item)
            } else {
                OutlineQueueResult {
                    flushed: vec![OutlineUpdate {
                        agent_id: agent_id.to_string(),
                        item,
                    }],
                    deadline: None,
                }
            }
        };
        wake.notify_all();
        for update in result.flushed {
            emit_outline_update(&self.event_sink, update);
        }
    }

    fn flush(&self) {
        let now_ms = self.now_ms();
        let (state, wake) = &*self.shared;
        let update = {
            let mut state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state
                .policy
                .as_mut()
                .and_then(|policy| policy.flush(now_ms))
        };
        wake.notify_all();
        if let Some(update) = update {
            emit_outline_update(&self.event_sink, update);
        }
    }

    fn stop(&self) {
        let (state, wake) = &*self.shared;
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(policy) = state.policy.as_mut() {
            policy.stop();
        }
        state.policy = None;
        wake.notify_all();
    }

    fn has_pending(&self) -> bool {
        let (state, _) = &*self.shared;
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .policy
            .as_ref()
            .is_some_and(|policy| policy.has_pending())
    }
}

impl Drop for OutlineStreamScheduler {
    fn drop(&mut self) {
        let (state, wake) = &*self.shared;
        {
            let mut state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.shutdown = true;
            wake.notify_all();
        }
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }
}

fn run_outline_stream_worker(
    started_at: Instant,
    shared: Arc<(Mutex<OutlineStreamSchedulerState>, Condvar)>,
    event_sink: RosterEventSink,
) {
    loop {
        let update = {
            let (state, wake) = &*shared;
            let mut state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                if state.shutdown {
                    return;
                }
                let Some(deadline) = state
                    .policy
                    .as_ref()
                    .and_then(|policy| policy.next_deadline())
                else {
                    state = wake
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    continue;
                };
                let now_ms = monotonic_ms(&started_at);
                if now_ms >= deadline.at_ms {
                    break state
                        .policy
                        .as_mut()
                        .and_then(|policy| policy.flush_deadline(now_ms, deadline.generation));
                }
                let wait_for = Duration::from_millis(deadline.at_ms.saturating_sub(now_ms));
                let (next, _) = wake
                    .wait_timeout(state, wait_for)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state = next;
            }
        };
        if let Some(update) = update {
            emit_outline_update(&event_sink, update);
        }
    }
}

fn monotonic_ms(started_at: &Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn emit_outline_update(event_sink: &RosterEventSink, update: OutlineUpdate<Value>) {
    (event_sink)(json!({
        "channel": "outline",
        "payload": {
            "type": "updated",
            "agentId": update.agent_id,
            "item": update.item
        }
    }));
}

#[derive(Debug, Default)]
struct RosterEmitState {
    cached_agent_summaries: Vec<Value>,
    roster_cache_seeded: bool,
}

pub struct ProductionRosterEmit {
    sessions: Arc<ProductionSessionWorkers>,
    transcript: Arc<ProductionTranscriptRuntime>,
    event_sink: RosterEventSink,
    timeline_wake_sink: Mutex<Option<TimelineWakeSink>>,
    outline_stream: OutlineStreamScheduler,
    projection: Mutex<RosterProjection<Value>>,
    state: Mutex<RosterEmitState>,
}

impl ProductionRosterEmit {
    pub fn new(
        sessions: Arc<ProductionSessionWorkers>,
        transcript: Arc<ProductionTranscriptRuntime>,
        event_sink: RosterEventSink,
    ) -> Self {
        let outline_stream = OutlineStreamScheduler::new(Arc::clone(&event_sink));
        Self {
            sessions,
            transcript,
            event_sink,
            timeline_wake_sink: Mutex::new(None),
            outline_stream,
            projection: Mutex::new(RosterProjection::default()),
            state: Mutex::new(RosterEmitState::default()),
        }
    }

    pub fn emit_agents(&self) -> Result<(), String> {
        let active_agent_id = self.active_agent_id();
        let summaries = self
            .sessions
            .list_agent_summaries(active_agent_id.as_deref())?;
        let mut agents = serde_json::to_value(summaries)
            .map_err(|error| format!("could not encode roster summaries: {error}"))?;
        self.transcript.decorate_agent_summaries(&mut agents);
        let rows = agents.as_array().cloned().unwrap_or_default();
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "roster emit state mutex poisoned".to_string())?;
            state.cached_agent_summaries = rows.clone();
            state.roster_cache_seeded = true;
        }
        let ordered = stamp_json(self.transcript.next_replica_stamp(ROSTER_REPLICA_KEY));
        (self.event_sink)(json!({
            "channel": "agents",
            "payload": {
                "activeAgentId": active_agent_id.unwrap_or_default(),
                "agents": rows,
                "ordered": ordered,
                "coverage": { "kind": "complete-roster" }
            }
        }));
        Ok(())
    }

    pub fn emit_automations<T: Serialize>(
        &self,
        agent_id: &str,
        automations: &T,
    ) -> Result<bool, String> {
        if self.active_agent_id().as_deref() != Some(agent_id) {
            return Ok(false);
        }
        let automations = serde_json::to_value(automations)
            .map_err(|error| format!("could not encode automations: {error}"))?;
        (self.event_sink)(json!({
            "channel": "automations",
            "payload": {
                "agentId": agent_id,
                "automations": automations
            }
        }));
        Ok(true)
    }

    pub fn emit_agent_update(&self, agent_id: &str) -> Result<(), String> {
        if agent_id.trim().is_empty() {
            return self.emit_agents();
        }
        let seeded = self
            .state
            .lock()
            .map(|state| state.roster_cache_seeded)
            .unwrap_or(false);
        if !seeded {
            return self.emit_agents();
        }

        let active_agent_id = self.active_agent_id();
        let Some(summary) = self
            .sessions
            .summarize_agent_by_id(agent_id, active_agent_id.as_deref())?
        else {
            return self.emit_agents();
        };
        let mut decorated = serde_json::to_value(vec![summary])
            .map_err(|error| format!("could not encode roster summary: {error}"))?;
        self.transcript.decorate_agent_summaries(&mut decorated);
        let Some(agent) = decorated.as_array().and_then(|rows| rows.first()).cloned() else {
            return self.emit_agents();
        };

        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "roster emit state mutex poisoned".to_string())?;
            if let Some(index) = state
                .cached_agent_summaries
                .iter()
                .position(|row| row.get("id").and_then(Value::as_str) == Some(agent_id))
            {
                state.cached_agent_summaries[index] = agent.clone();
            } else {
                state.cached_agent_summaries.push(agent.clone());
            }
        }

        let ordered = stamp_json(self.transcript.next_replica_stamp(ROSTER_REPLICA_KEY));
        (self.event_sink)(json!({
            "channel": "agent-upserted",
            "payload": {
                "activeAgentId": active_agent_id.unwrap_or_default(),
                "agent": agent,
                "ordered": ordered
            }
        }));
        Ok(())
    }

    pub fn finalize_summary_for_rpc<T: Serialize>(&self, summary: T) -> Result<Value, String> {
        let mut decorated = serde_json::to_value(vec![summary])
            .map_err(|error| format!("could not encode roster summary: {error}"))?;
        self.transcript.decorate_agent_summaries(&mut decorated);
        decorated
            .as_array()
            .and_then(|rows| rows.first())
            .cloned()
            .ok_or_else(|| "could not finalize roster summary for RPC".to_string())
    }

    pub fn publish_profile_changed(&self, agent_id: &str) {
        (self.event_sink)(json!({
            "channel": "profile-changed",
            "payload": { "agentId": agent_id }
        }));
    }

    pub fn publish_transcript_snapshot(&self, agent_id: &str, entries: &[Value]) {
        (self.event_sink)(json!({
            "channel": "transcript",
            "payload": {
                "type": "snapshot",
                "activeAgentId": agent_id,
                "entries": entries,
            }
        }));
    }

    pub fn publish_transcript_appended(&self, agent_id: &str, entry: &Value) {
        (self.event_sink)(json!({
            "channel": "transcript",
            "payload": {
                "type": "appended",
                "agentId": agent_id,
                "entry": entry,
            }
        }));
    }

    pub fn publish_transcript_updated(&self, agent_id: &str, entry: &Value) {
        (self.event_sink)(json!({
            "channel": "transcript",
            "payload": {
                "type": "updated",
                "agentId": agent_id,
                "entry": entry,
            }
        }));
    }

    pub fn set_outline_stream_coalescing_ms(&self, delay_ms: u64) {
        self.outline_stream.configure(delay_ms);
    }

    pub fn outline_stream_coalescing_ms(&self) -> u64 {
        self.outline_stream.delay_ms()
    }

    pub fn queue_outline_stream_update(&self, agent_id: &str, item_id: &str, item: Value) {
        self.outline_stream.queue(agent_id, item_id, item);
    }

    pub fn apply_runner_text_delta(&self, agent_id: &str, stream_id: &str, accumulated: &str) {
        if agent_id.trim().is_empty() || stream_id.trim().is_empty() || accumulated.is_empty() {
            return;
        }
        let (item_id, is_first) = self
            .projection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stream_outline_item_id_or_insert_with(stream_id, || uuid::Uuid::new_v4().to_string());
        let item = json!({
            "kind": "assistant-text",
            "id": item_id,
            "text": accumulated,
        });
        if is_first {
            (self.event_sink)(json!({
                "channel": "outline",
                "payload": {
                    "type": "appended",
                    "agentId": agent_id,
                    "item": item
                }
            }));
        } else {
            self.outline_stream.queue(agent_id, &item_id, item);
        }
    }

    pub fn finish_runner_outline_stream(&self, stream_id: &str) {
        let removed = self
            .projection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish_outline_stream(stream_id);
        if removed {
            self.outline_stream.flush();
        }
    }

    pub fn flush_outline_stream_update(&self) {
        self.outline_stream.flush();
    }

    pub fn stop_outline_stream_coalescing(&self) {
        self.outline_stream.stop();
        self.projection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear_outline_streams();
    }

    pub fn has_pending_outline_stream_update(&self) -> bool {
        self.outline_stream.has_pending()
    }

    pub fn bind_timeline_wake_sink(&self, sink: Option<TimelineWakeSink>) {
        *self
            .timeline_wake_sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = sink;
    }

    pub fn publish_timeline_event(&self, agent_id: &str, event: Value) {
        (self.event_sink)(json!({
            "channel": "timeline",
            "payload": {
                "agentId": agent_id,
                "event": event.clone()
            }
        }));
        let sink = self
            .timeline_wake_sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(sink) = sink {
            sink(agent_id, event);
        }
    }

    pub fn publish_name_changed(&self, agent_id: &str, from: &str, to: &str) {
        self.publish_timeline_event(
            agent_id,
            json!({
                "type": "name-changed",
                "from": from,
                "to": to
            }),
        );
    }

    pub fn cached_agent_summaries(&self) -> Vec<Value> {
        self.state
            .lock()
            .map(|state| state.cached_agent_summaries.clone())
            .unwrap_or_default()
    }

    pub fn cache_seeded(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.roster_cache_seeded)
            .unwrap_or(false)
    }

    fn active_agent_id(&self) -> Option<String> {
        SandAgentSessionStore::new(Arc::clone(&self.sessions)).read_active_agent_id()
    }
}

fn stamp_json(stamp: ReplicaStamp) -> Value {
    json!({
        "replicaKey": stamp.replica_key,
        "epoch": stamp.epoch,
        "sequence": stamp.sequence
    })
}
