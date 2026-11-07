pub const NOTIFY_SAFETY_POLL_MS: u64 = 120_000;
pub const NOTIFY_DRAIN_FLOOR_MS: u64 = 4_000;

pub struct NotifyDrainGate<N, C, S>
where
    N: Fn() -> u64,
    C: Fn() -> bool,
    S: Fn() -> bool,
{
    now: N,
    is_connected: C,
    is_safety_poll_enabled: S,
    notify_pending: bool,
    last_poll_at_ms: Option<u64>,
    notify_seq: u64,
    drained_notify_seq: u64,
}

impl<N, C, S> NotifyDrainGate<N, C, S>
where
    N: Fn() -> u64,
    C: Fn() -> bool,
    S: Fn() -> bool,
{
    pub fn new(now: N, is_connected: C, is_safety_poll_enabled: S) -> Self {
        Self { now, is_connected, is_safety_poll_enabled, notify_pending: false, last_poll_at_ms: None, notify_seq: 0, drained_notify_seq: 0 }
    }
    pub fn record_notify(&mut self) { self.notify_pending = true; self.notify_seq = self.notify_seq.saturating_add(1); }
    pub fn should_drain(&mut self, has_owed_work: bool) -> bool {
        self.drained_notify_seq = self.notify_seq;
        if has_owed_work || !(self.is_connected)() { return true; }
        let Some(last) = self.last_poll_at_ms else { return true; };
        let since = (self.now)().saturating_sub(last);
        if self.notify_pending && since >= NOTIFY_DRAIN_FLOOR_MS { return true; }
        (self.is_safety_poll_enabled)() && since >= NOTIFY_SAFETY_POLL_MS
    }
    pub fn record_poll(&mut self) {
        if self.notify_seq == self.drained_notify_seq { self.notify_pending = false; }
        self.last_poll_at_ms = Some((self.now)());
    }
    pub fn reset(&mut self) { self.last_poll_at_ms = None; }
}
