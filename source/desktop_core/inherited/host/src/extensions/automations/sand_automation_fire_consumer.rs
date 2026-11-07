use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use serde_json::{Value, json};

use super::backend_transport::{AutomationsBackendError, AutomationsBackendTransport};

pub const AUTOMATION_FIRE_ERROR_BACKOFF_MS: u64 = 30_000;
pub const MAX_NEXT_POLL_DELAY_MS: u64 = 60_000;
pub const NOTIFY_DRAIN_FLOOR_MS: u64 = 4_000;
pub const NOTIFY_SAFETY_POLL_MS: u64 = 120_000;

#[derive(Debug, Clone, PartialEq)]
pub struct AutomationFireEnvelope {
    pub agent_id: String,
    pub automation_id: String,
    pub event: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationFireFailure {
    pub agent_id: String,
    pub automation_id: String,
    pub error: String,
}

#[derive(Debug, Default)]
pub struct SandAutomationFireConsumer {
    queue: VecDeque<AutomationFireEnvelope>,
    firing: bool,
    stopped: bool,
}

impl SandAutomationFireConsumer {
    pub fn start(&mut self) { self.stopped = false; }
    pub fn stop(&mut self) { self.stopped = true; }

    pub fn enqueue(&mut self, envelope: AutomationFireEnvelope) {
        if !self.stopped {
            self.queue.push_back(envelope);
        }
    }

    pub fn is_firing(&self) -> bool { self.firing }
    pub fn is_stopped(&self) -> bool { self.stopped }
    pub fn pending_len(&self) -> usize { self.queue.len() }

    pub fn drain(
        &mut self,
        mut fire: impl FnMut(&AutomationFireEnvelope) -> Result<(), String>,
    ) -> Vec<AutomationFireFailure> {
        if self.firing || self.stopped {
            return Vec::new();
        }
        self.firing = true;
        let mut failures = Vec::new();
        while let Some(envelope) = self.queue.pop_front() {
            if let Err(error) = fire(&envelope) {
                failures.push(AutomationFireFailure {
                    agent_id: envelope.agent_id.clone(),
                    automation_id: envelope.automation_id.clone(),
                    error,
                });
            }
        }
        self.firing = false;
        failures
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendAutomationFire {
    pub id: String,
    pub sand_agent_id: String,
    pub automation_id: String,
    pub timestamp_ms: u64,
    pub definition_revision: Option<String>,
    pub scheduled_for_ms: Option<u64>,
    pub event: Option<Value>,
}

impl BackendAutomationFire {
    pub(super) fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            id: value.get("id")?.as_str()?.to_string(),
            sand_agent_id: value.get("sandAgentId")?.as_str()?.to_string(),
            automation_id: value.get("automationId")?.as_str()?.to_string(),
            timestamp_ms: value.get("timestampMs")?.as_u64()?,
            definition_revision: value
                .get("definitionRevision")
                .and_then(Value::as_str)
                .map(str::to_string),
            scheduled_for_ms: value.get("scheduledForMs").and_then(Value::as_u64),
            event: value.get("event").filter(|value| !value.is_null()).cloned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FireCompletion {
    pub status: String,
    pub error_message: Option<String>,
}

impl FireCompletion {
    pub fn succeeded() -> Self {
        Self { status: "succeeded".into(), error_message: None }
    }

    pub fn failed(error_message: impl Into<String>) -> Self {
        Self { status: "failed".into(), error_message: Some(error_message.into()) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BackendFireState {
    Running,
    Completed(FireCompletion),
    Reported,
}

pub struct AutomationFireBackendRuntime {
    transport: Arc<dyn AutomationsBackendTransport>,
    states: BTreeMap<String, BackendFireState>,
    stopped: bool,
    backoff_until_ms: u64,
    poll_not_before_ms: u64,
    drained_while_unschedulable: bool,
    notify_pending: bool,
    last_poll_at_ms: Option<u64>,
    notify_seq: u64,
    drained_notify_seq: u64,
}

impl AutomationFireBackendRuntime {
    pub fn new(transport: Arc<dyn AutomationsBackendTransport>) -> Self {
        Self {
            transport,
            states: BTreeMap::new(),
            stopped: false,
            backoff_until_ms: 0,
            poll_not_before_ms: 0,
            drained_while_unschedulable: false,
            notify_pending: false,
            last_poll_at_ms: None,
            notify_seq: 0,
            drained_notify_seq: 0,
        }
    }

    pub fn start(&mut self) {
        self.stopped = false;
        self.backoff_until_ms = 0;
        self.poll_not_before_ms = 0;
        self.drained_while_unschedulable = false;
        self.last_poll_at_ms = None;
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        self.backoff_until_ms = 0;
        self.poll_not_before_ms = 0;
    }

    pub fn request_drain(&mut self) {
        self.notify_pending = true;
        self.notify_seq = self.notify_seq.saturating_add(1);
        self.poll_not_before_ms = 0;
    }

    pub fn reset_poll_delay(&mut self) {
        self.poll_not_before_ms = 0;
    }

    pub fn pending_len(&self) -> usize {
        self.states.len()
    }

    pub fn is_running(&self, id: &str) -> bool {
        matches!(self.states.get(id), Some(BackendFireState::Running))
    }

    pub fn abandon(&mut self, id: &str) {
        self.states.remove(id);
    }

    pub fn complete(
        &mut self,
        id: &str,
        completion: FireCompletion,
    ) -> Result<(), AutomationsBackendError> {
        if !self.states.contains_key(id) {
            return Ok(());
        }
        self.states
            .insert(id.to_string(), BackendFireState::Completed(completion.clone()));
        match self.report_completion(id, &completion) {
            Ok(()) => {
                self.states.insert(id.to_string(), BackendFireState::Reported);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub fn tick(
        &mut self,
        now_ms: u64,
        notify_connected: bool,
        safety_poll_enabled: bool,
        dispatch: impl FnMut(&BackendAutomationFire) -> bool,
    ) -> Result<usize, AutomationsBackendError> {
        self.tick_with_state(
            now_ms,
            notify_connected,
            safety_poll_enabled,
            true,
            true,
            dispatch,
        )
    }

    pub fn tick_with_state(
        &mut self,
        now_ms: u64,
        notify_connected: bool,
        safety_poll_enabled: bool,
        is_ready: bool,
        has_server_schedulable: bool,
        mut dispatch: impl FnMut(&BackendAutomationFire) -> bool,
    ) -> Result<usize, AutomationsBackendError> {
        if self.stopped
            || !is_ready
            || now_ms < self.backoff_until_ms
            || now_ms < self.poll_not_before_ms
        {
            return Ok(0);
        }

        let has_owed_work = !self.states.is_empty();
        self.drained_notify_seq = self.notify_seq;
        if !has_owed_work {
            if notify_connected {
                let should_drain = match self.last_poll_at_ms {
                    None => true,
                    Some(last_poll_at_ms) => {
                        let since_last_poll_ms = now_ms.saturating_sub(last_poll_at_ms);
                        (self.notify_pending && since_last_poll_ms >= NOTIFY_DRAIN_FLOOR_MS)
                            || (safety_poll_enabled
                                && since_last_poll_ms >= NOTIFY_SAFETY_POLL_MS)
                    }
                };
                if !should_drain {
                    return Ok(0);
                }
            }
            if !has_server_schedulable && self.drained_while_unschedulable {
                return Ok(0);
            }
        }

        let result: Result<usize, AutomationsBackendError> = (|| {
            let ack_run_uuids = self
                .states
                .iter()
                .filter_map(|(id, state)| {
                    matches!(state, BackendFireState::Reported).then_some(id.clone())
                })
                .collect::<Vec<_>>();
            let response = self.transport.post_json(
                "/sand/automation-events/poll",
                &json!({ "ackRunUuids": ack_run_uuids }),
            )?;
            let events = response
                .get("events")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if self.notify_seq == self.drained_notify_seq {
                self.notify_pending = false;
            }
            self.last_poll_at_ms = Some(now_ms);
            let returned = events
                .iter()
                .filter_map(|event| event.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<BTreeSet<_>>();

            self.states.retain(|id, state| {
                !matches!(state, BackendFireState::Reported) || returned.contains(id)
            });

            let mut delivered = 0usize;
            for raw in events {
                let Some(event) = BackendAutomationFire::from_value(&raw) else { continue };
                if self.states.contains_key(&event.id) {
                    continue;
                }
                self.states.insert(event.id.clone(), BackendFireState::Running);
                if dispatch(&event) {
                    delivered += 1;
                } else {
                    self.states.remove(&event.id);
                }
            }

            self.retry_completions();

            if !has_owed_work {
                self.drained_while_unschedulable = !has_server_schedulable;
            }

            let next_poll_after_ms = response
                .get("nextPollAfterMs")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(MAX_NEXT_POLL_DELAY_MS);
            if delivered == 0 && self.states.is_empty() && next_poll_after_ms > 0 {
                self.poll_not_before_ms = now_ms.saturating_add(next_poll_after_ms);
            } else {
                self.poll_not_before_ms = 0;
            }
            Ok(delivered)
        })();

        if result.is_err() {
            self.backoff_until_ms = now_ms.saturating_add(AUTOMATION_FIRE_ERROR_BACKOFF_MS);
        }
        result
    }

    fn retry_completions(&mut self) {
        let completions = self
            .states
            .iter()
            .filter_map(|(id, state)| match state {
                BackendFireState::Completed(completion) => Some((id.clone(), completion.clone())),
                BackendFireState::Running | BackendFireState::Reported => None,
            })
            .collect::<Vec<_>>();
        for (id, completion) in completions {
            match self.report_completion(&id, &completion) {
                Ok(()) => {
                    self.states.insert(id, BackendFireState::Reported);
                }
                Err(_) => {}
            }
        }
    }

    fn report_completion(
        &self,
        id: &str,
        completion: &FireCompletion,
    ) -> Result<(), AutomationsBackendError> {
        self.transport.post_json(
            "/sand/automation-runs/complete",
            &json!({
                "runUuid": id,
                "status": &completion.status,
                "errorMessage": &completion.error_message,
            }),
        )?;
        Ok(())
    }
}
