use std::collections::{HashMap, VecDeque};

pub const SSE_ECHO_STAGE: &str = "echo-coordinator-sse";
pub const MAX_IN_FLIGHT_TRANSPORT_REPORTS: usize = 64;
pub const PENDING_SEND_ECHO_MAX: usize = 64;
pub const PENDING_SEND_ECHO_TTL_MS: u64 = 120_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportIdentity {
    pub account_slot: String,
    pub client_nonce: Option<String>,
    pub traceparent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportStageReport {
    pub account_slot: String,
    pub client_nonce: String,
    pub stage: String,
    pub attempt: u32,
    pub traceparent: Option<String>,
    pub start_epoch_ms: u64,
    pub duration_ms: u64,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendTrace {
    account_slot: String,
    client_nonce: String,
    traceparent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageToken {
    report: TransportStageReport,
    started_monotonic_ms: u64,
    settled: bool,
}

impl StageToken {
    pub fn settle(&mut self, now_monotonic_ms: u64, is_error: bool) -> Option<TransportStageReport> {
        if self.settled {
            return None;
        }
        self.settled = true;
        let mut report = self.report.clone();
        report.duration_ms = now_monotonic_ms.saturating_sub(self.started_monotonic_ms);
        report.is_error = is_error;
        Some(report)
    }

    pub fn complete(&mut self, now_monotonic_ms: u64) -> Option<TransportStageReport> {
        self.settle(now_monotonic_ms, false)
    }

    pub fn fail(&mut self, now_monotonic_ms: u64) -> Option<TransportStageReport> {
        self.settle(now_monotonic_ms, true)
    }
}

impl SendTrace {
    pub fn begin_stage(
        &self,
        stage: impl Into<String>,
        attempt: u32,
        start_epoch_ms: u64,
        start_monotonic_ms: u64,
    ) -> StageToken {
        StageToken {
            report: TransportStageReport {
                account_slot: self.account_slot.clone(),
                client_nonce: self.client_nonce.clone(),
                stage: stage.into(),
                attempt,
                traceparent: self.traceparent.clone(),
                start_epoch_ms,
                duration_ms: 0,
                is_error: false,
            },
            started_monotonic_ms: start_monotonic_ms,
            settled: false,
        }
    }

    pub fn mark_stage(
        &self,
        stage: impl Into<String>,
        attempt: u32,
        now_epoch_ms: u64,
    ) -> TransportStageReport {
        TransportStageReport {
            account_slot: self.account_slot.clone(),
            client_nonce: self.client_nonce.clone(),
            stage: stage.into(),
            attempt,
            traceparent: self.traceparent.clone(),
            start_epoch_ms: now_epoch_ms,
            duration_ms: 0,
            is_error: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingEcho {
    traceparent: String,
    armed_at_monotonic_ms: u64,
}

#[derive(Debug, Default)]
pub struct TransportStageRecorder {
    pending_echoes: HashMap<String, PendingEcho>,
    pending_order: VecDeque<String>,
}

impl TransportStageRecorder {
    fn key(account_slot: &str, client_nonce: &str) -> String {
        format!("{account_slot}\0{client_nonce}")
    }

    fn remove_key(&mut self, key: &str) -> Option<PendingEcho> {
        let removed = self.pending_echoes.remove(key);
        if removed.is_some() {
            self.pending_order.retain(|candidate| candidate != key);
        }
        removed
    }

    fn purge_expired(&mut self, now_monotonic_ms: u64) {
        let expired = self
            .pending_echoes
            .iter()
            .filter_map(|(key, pending)| {
                (now_monotonic_ms.saturating_sub(pending.armed_at_monotonic_ms)
                    > PENDING_SEND_ECHO_TTL_MS)
                    .then(|| key.clone())
            })
            .collect::<Vec<_>>();
        for key in expired {
            self.remove_key(&key);
        }
    }

    pub fn begin_send(
        &mut self,
        identity: TransportIdentity,
        now_monotonic_ms: u64,
    ) -> Option<SendTrace> {
        let client_nonce = identity
            .client_nonce
            .filter(|value| !value.is_empty())?;
        let traceparent = identity.traceparent.filter(|value| !value.is_empty());

        self.purge_expired(now_monotonic_ms);
        if let Some(traceparent_value) = traceparent.as_ref() {
            let key = Self::key(&identity.account_slot, &client_nonce);
            self.remove_key(&key);
            while self.pending_echoes.len() >= PENDING_SEND_ECHO_MAX {
                let Some(oldest) = self.pending_order.pop_front() else {
                    break;
                };
                self.pending_echoes.remove(&oldest);
            }
            self.pending_order.push_back(key.clone());
            self.pending_echoes.insert(
                key,
                PendingEcho {
                    traceparent: traceparent_value.clone(),
                    armed_at_monotonic_ms: now_monotonic_ms,
                },
            );
        }

        Some(SendTrace {
            account_slot: identity.account_slot,
            client_nonce,
            traceparent,
        })
    }

    pub fn record_send_echo(
        &mut self,
        account_slot: &str,
        client_nonce: &str,
        now_epoch_ms: u64,
        now_monotonic_ms: u64,
    ) -> Option<TransportStageReport> {
        self.purge_expired(now_monotonic_ms);
        let key = Self::key(account_slot, client_nonce);
        let pending = self.remove_key(&key)?;
        Some(TransportStageReport {
            account_slot: account_slot.to_string(),
            client_nonce: client_nonce.to_string(),
            stage: SSE_ECHO_STAGE.to_string(),
            attempt: 0,
            traceparent: Some(pending.traceparent),
            start_epoch_ms: now_epoch_ms,
            duration_ms: 0,
            is_error: false,
        })
    }

    pub fn pending_echo_count(&self) -> usize {
        self.pending_echoes.len()
    }
}

#[derive(Debug, Default)]
pub struct TransportReportLimiter {
    in_flight: usize,
}

impl TransportReportLimiter {
    pub fn try_begin(&mut self) -> bool {
        if self.in_flight >= MAX_IN_FLIGHT_TRANSPORT_REPORTS {
            return false;
        }
        self.in_flight += 1;
        true
    }

    pub fn settle(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight
    }
}
