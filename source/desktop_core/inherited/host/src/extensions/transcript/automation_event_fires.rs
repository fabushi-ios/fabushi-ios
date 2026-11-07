use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::automations::automation::{AutomationRecord, MAX_EVENTS_IN_AUTOMATION_WAKE};

use super::automation_run_path::{AutomationRunTrigger, FireAutomationOutcome};

pub const EVENT_FIRE_DEBOUNCE_MS: u64 = 750;
pub const MAX_QUEUED_EVENT_FIRES_PER_AUTOMATION: usize = 500;
pub const MAX_REPORTED_DROPPED_FIRES: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct DroppedFire {
    pub agent_id: String,
    pub trigger: AutomationRunTrigger,
    pub reason: String,
    pub scheduled_for_ms: Option<f64>,
    pub run_uuid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EventFireBatch {
    pub agent_id: String,
    pub automation: AutomationRecord,
    pub events: Vec<Value>,
    pub run_uuid: Option<String>,
    pub coalesced_run_uuids: Vec<String>,
}

pub type EventBatchExecutor = Arc<
    dyn Fn(EventFireBatch) -> Result<Option<FireAutomationOutcome>, String> + Send + Sync + 'static,
>;
pub type DroppedFireReporter = Arc<dyn Fn(DroppedFire) + Send + Sync + 'static>;

struct FireItem {
    event: Value,
    run_uuid: Option<String>,
    resolve: mpsc::SyncSender<Option<FireAutomationOutcome>>,
}

struct FireBatch {
    agent_id: String,
    automation: AutomationRecord,
    items: VecDeque<FireItem>,
    executor: EventBatchExecutor,
    debounce_armed: bool,
    flushing: bool,
    flush_immediately: bool,
}

#[derive(Default)]
struct EventFireState {
    pending: HashMap<String, FireBatch>,
    reported_dropped_fire_uuids: HashSet<String>,
    reported_dropped_fire_order: VecDeque<String>,
}

#[derive(Clone)]
pub struct AutomationEventFires {
    state: Arc<Mutex<EventFireState>>,
    reporter: Arc<Mutex<Option<DroppedFireReporter>>>,
    disposed: Arc<AtomicBool>,
    debounce_ms: u64,
}

impl Default for AutomationEventFires {
    fn default() -> Self {
        Self::new(EVENT_FIRE_DEBOUNCE_MS)
    }
}

impl AutomationEventFires {
    pub fn new(debounce_ms: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(EventFireState::default())),
            reporter: Arc::new(Mutex::new(None)),
            disposed: Arc::new(AtomicBool::new(false)),
            debounce_ms,
        }
    }

    pub fn set_dropped_fire_reporter(&self, reporter: Option<DroppedFireReporter>) {
        *self
            .reporter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = reporter;
    }

    pub fn enqueue_event_automation_fire(
        &self,
        agent_id: impl Into<String>,
        automation: AutomationRecord,
        event: Value,
        run_uuid: Option<String>,
        executor: EventBatchExecutor,
    ) -> mpsc::Receiver<Option<FireAutomationOutcome>> {
        let (resolve, receiver) = mpsc::sync_channel(1);
        if self.disposed.load(Ordering::Acquire) {
            let _ = resolve.send(None);
            return receiver;
        }

        let agent_id = agent_id.into();
        let run_key = format!("{agent_id}:{}", automation.id);
        let mut dropped = Vec::new();
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let batch = state.pending.entry(run_key.clone()).or_insert_with(|| FireBatch {
                agent_id: agent_id.clone(),
                automation: automation.clone(),
                items: VecDeque::new(),
                executor: Arc::clone(&executor),
                debounce_armed: false,
                flushing: false,
                flush_immediately: false,
            });
            batch.automation = automation;
            batch.executor = executor;
            batch.items.push_back(FireItem {
                event,
                run_uuid,
                resolve,
            });
            while batch.items.len() > MAX_QUEUED_EVENT_FIRES_PER_AUTOMATION {
                if let Some(item) = batch.items.pop_front() {
                    dropped.push((batch.agent_id.clone(), item));
                }
            }
        }

        for (dropped_agent_id, item) in dropped {
            self.report_fire_dropped(DroppedFire {
                agent_id: dropped_agent_id,
                trigger: AutomationRunTrigger::Event,
                reason: "event_batch_overflow".into(),
                scheduled_for_ms: None,
                run_uuid: item.run_uuid.clone(),
            });
            let _ = item.resolve.send(Some(FireAutomationOutcome::Error));
        }

        self.schedule_event_batch_flush(run_key);
        receiver
    }

    pub fn report_fire_dropped(&self, dropped: DroppedFire) {
        if let Some(run_uuid) = dropped.run_uuid.as_ref() {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.reported_dropped_fire_uuids.contains(run_uuid) {
                return;
            }
            state
                .reported_dropped_fire_uuids
                .insert(run_uuid.clone());
            state
                .reported_dropped_fire_order
                .push_back(run_uuid.clone());
            while state.reported_dropped_fire_order.len() > MAX_REPORTED_DROPPED_FIRES {
                if let Some(oldest) = state.reported_dropped_fire_order.pop_front() {
                    state.reported_dropped_fire_uuids.remove(&oldest);
                }
            }
        }

        let reporter = self
            .reporter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(reporter) = reporter {
            reporter(dropped);
        }
    }

    pub fn pending_batch_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .len()
    }

    pub fn reported_dropped_fire_uuid_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reported_dropped_fire_uuids
            .len()
    }

    pub fn dispose(&self) {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return;
        }
        let batches = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut state.pending)
        };
        for (_, mut batch) in batches {
            while let Some(item) = batch.items.pop_front() {
                let _ = item.resolve.send(None);
            }
        }
    }

    fn schedule_event_batch_flush(&self, run_key: String) {
        if self.disposed.load(Ordering::Acquire) {
            return;
        }
        let wait_ms = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(batch) = state.pending.get_mut(&run_key) else {
                return;
            };
            if batch.items.is_empty() || batch.flushing || batch.debounce_armed {
                return;
            }
            batch.debounce_armed = true;
            let wait_ms = if batch.flush_immediately {
                0
            } else {
                self.debounce_ms
            };
            batch.flush_immediately = false;
            wait_ms
        };

        let runtime = self.clone();
        let _ = thread::Builder::new()
            .name("sand-automation-event-fire".into())
            .spawn(move || {
                if wait_ms > 0 {
                    thread::sleep(Duration::from_millis(wait_ms));
                }
                runtime.flush_event_batch(run_key);
            });
    }

    fn flush_event_batch(&self, run_key: String) {
        if self.disposed.load(Ordering::Acquire) {
            return;
        }

        let (items, execution) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(batch) = state.pending.get_mut(&run_key) else {
                return;
            };
            batch.debounce_armed = false;
            if batch.items.is_empty() || batch.flushing {
                return;
            }
            batch.flushing = true;
            let take = batch.items.len().min(MAX_EVENTS_IN_AUTOMATION_WAKE);
            let items = batch.items.drain(..take).collect::<Vec<_>>();
            let run_uuids = items
                .iter()
                .filter_map(|item| item.run_uuid.clone())
                .collect::<Vec<_>>();
            let execution = (
                Arc::clone(&batch.executor),
                EventFireBatch {
                    agent_id: batch.agent_id.clone(),
                    automation: batch.automation.clone(),
                    events: items.iter().map(|item| item.event.clone()).collect(),
                    run_uuid: run_uuids.first().cloned(),
                    coalesced_run_uuids: run_uuids.into_iter().skip(1).collect(),
                },
            );
            (items, execution)
        };

        let (executor, batch) = execution;
        let outcome = match executor(batch) {
            Ok(outcome) => outcome,
            Err(error) => {
                eprintln!("[sand:automation] event wake dispatch failed for {run_key}: {error}");
                None
            }
        };
        for item in items {
            let _ = item.resolve.send(outcome);
        }

        let schedule_next = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(batch) = state.pending.get_mut(&run_key) else {
                return;
            };
            batch.flushing = false;
            if batch.items.is_empty() {
                state.pending.remove(&run_key);
                false
            } else {
                batch.flush_immediately = true;
                true
            }
        };
        if schedule_next {
            self.schedule_event_batch_flush(run_key);
        }
    }
}

impl Drop for AutomationEventFires {
    fn drop(&mut self) {
        if Arc::strong_count(&self.disposed) == 1 {
            self.dispose();
        }
    }
}
