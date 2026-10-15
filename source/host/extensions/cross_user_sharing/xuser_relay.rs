use serde_json::Value;
use std::collections::BTreeSet;
use crate::notify_drain_gate::NotifyDrainGate;
use std::sync::{Arc, Mutex};

pub const XUSER_RELAY_POLL_INTERVAL_MS: u64 = 4_000;
pub const XUSER_RELAY_ERROR_BACKOFF_MS: u64 = 30_000;

pub trait XuserRelayHost: Send + Sync {
    fn poll(&self, ack_ids: &[String]) -> Result<Value, String>;
}

pub type XuserRelayEventHandler = Arc<dyn Fn(&Value) -> Result<bool, String> + Send + Sync>;

#[derive(Default)]
struct RelayState {
    started: bool,
    ticking: bool,
    backoff_until_ms: u64,
    pending_ack_ids: Vec<String>,
}

pub struct SandXuserRelayRuntime {
    host: Arc<dyn XuserRelayHost>,
    on_event: XuserRelayEventHandler,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    state: Mutex<RelayState>,
    gate: Mutex<NotifyDrainGate>,
    is_notify_connected: Arc<dyn Fn() -> bool + Send + Sync>,
    is_notify_safety_poll_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl SandXuserRelayRuntime {
    pub fn new(
        host: Arc<dyn XuserRelayHost>,
        on_event: XuserRelayEventHandler,
        now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
        is_notify_connected: Arc<dyn Fn() -> bool + Send + Sync>,
        is_notify_safety_poll_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            host,
            on_event,
            now_ms,
            state: Mutex::new(RelayState::default()),
            gate: Mutex::new(NotifyDrainGate::default()),
            is_notify_connected,
            is_notify_safety_poll_enabled,
        }
    }

    pub fn start(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).started = true;
        self.gate.lock().unwrap_or_else(|p| p.into_inner()).reset();
    }

    pub fn record_notify(&self) {
        self.gate.lock().unwrap_or_else(|p| p.into_inner()).record_notify();
    }

    pub fn request_drain(&self) -> Result<bool, String> {
        self.record_notify();
        self.tick()
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.started = false;
        state.ticking = false;
        state.backoff_until_ms = 0;
    }

    pub fn reset_account(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.pending_ack_ids.clear();
        state.backoff_until_ms = 0;
        drop(state);
        self.gate.lock().unwrap_or_else(|p| p.into_inner()).reset();
    }

    pub fn tick(&self) -> Result<bool, String> {
        let now = (self.now_ms)();
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !state.started || state.ticking || now < state.backoff_until_ms {
                return Ok(false);
            }
            let should_drain = self
                .gate
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .should_drain(
                    now,
                    !state.pending_ack_ids.is_empty(),
                    (self.is_notify_connected)(),
                    (self.is_notify_safety_poll_enabled)(),
                );
            if !should_drain {
                return Ok(false);
            }
            state.ticking = true;
            state.pending_ack_ids.clone()
        };
        let payload = match self.host.poll(&pending) {
            Ok(payload) => payload,
            Err(error) => {
                let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                state.ticking = false;
                state.backoff_until_ms = now.saturating_add(XUSER_RELAY_ERROR_BACKOFF_MS);
                return Err(error);
            }
        };
        let old = pending.into_iter().collect::<BTreeSet<_>>();
        let mut next = Vec::new();
        let mut seen = BTreeSet::new();
        if let Some(events) = payload.get("events").and_then(Value::as_array) {
            for event in events {
                let Some(id) = event.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) else { continue };
                if event.get("kind").and_then(Value::as_str).is_none() { continue; }
                let handled = old.contains(id) || (self.on_event)(event).unwrap_or(false);
                if handled && seen.insert(id.to_string()) {
                    next.push(id.to_string());
                }
            }
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.pending_ack_ids = next;
        state.ticking = false;
        state.backoff_until_ms = 0;
        drop(state);
        self.gate.lock().unwrap_or_else(|p| p.into_inner()).record_poll(now);
        Ok(true)
    }

    #[cfg(test)]
    fn backoff_until_ms(&self) -> u64 {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).backoff_until_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    struct FailingHost { calls: AtomicUsize }
    impl XuserRelayHost for FailingHost {
        fn poll(&self, _: &[String]) -> Result<Value, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err("offline".into())
        }
    }

    #[test]
    fn relay_timing_constants_match_desktop_frozen_contract() {
        assert_eq!(XUSER_RELAY_POLL_INTERVAL_MS, 4_000);
        assert_eq!(XUSER_RELAY_ERROR_BACKOFF_MS, 30_000);
    }

    #[test]
    fn notify_gate_suppresses_polling_until_floor_or_safety_wake() {
        let now = Arc::new(AtomicU64::new(1_000));
        let host = Arc::new(FailingHost { calls: AtomicUsize::new(0) });
        let clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
        };
        let runtime = SandXuserRelayRuntime::new(
            host.clone(),
            Arc::new(|_| Ok(true)),
            clock,
            Arc::new(|| true),
            Arc::new(|| false),
        );
        runtime.start();
        assert!(runtime.tick().is_err());
        now.store(31_000, Ordering::SeqCst);
        assert!(runtime.tick().is_err());
        assert_eq!(host.calls.load(Ordering::SeqCst), 2);
        runtime.record_notify();
        now.store(34_999, Ordering::SeqCst);
        assert!(!runtime.tick().unwrap());
        now.store(35_000, Ordering::SeqCst);
        assert!(runtime.tick().is_err());
        assert_eq!(host.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn relay_failure_backs_off_and_does_not_spin() {
        let now = Arc::new(AtomicU64::new(1_000));
        let host = Arc::new(FailingHost { calls: AtomicUsize::new(0) });
        let clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
        };
        let runtime = SandXuserRelayRuntime::new(
            host.clone(),
            Arc::new(|_| Ok(true)),
            clock,
            Arc::new(|| false),
            Arc::new(|| false),
        );
        runtime.start();
        assert!(runtime.tick().is_err());
        assert_eq!(runtime.backoff_until_ms(), 1_000 + XUSER_RELAY_ERROR_BACKOFF_MS);
        assert!(!runtime.tick().unwrap());
        assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    }
}
