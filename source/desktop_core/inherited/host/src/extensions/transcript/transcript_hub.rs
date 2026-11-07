use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

pub const RUNNER_UNATTACHED_MESSAGE: &str =
    "Sand agent runner factory is not attached.";

pub type TranscriptEntry = Value;

pub trait Disposable: Send + Sync {
    fn dispose(&self);
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
    fn schedule(
        &self,
        delay_ms: u64,
        callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Box<dyn Disposable>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RealClock;

#[derive(Debug)]
struct TimerDisposable {
    cancelled: Arc<AtomicBool>,
}

impl Disposable for TimerDisposable {
    fn dispose(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl Clock for RealClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }

    fn schedule(
        &self,
        delay_ms: u64,
        callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Box<dyn Disposable> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let task_cancelled = Arc::clone(&cancelled);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay_ms));
            if !task_cancelled.load(Ordering::Acquire) {
                callback();
            }
        });
        Box::new(TimerDisposable { cancelled })
    }
}

pub fn transcript_entry_id(entry: &TranscriptEntry) -> Option<&str> {
    entry.get("id").and_then(Value::as_str)
}

pub fn transcript_entry_kind(entry: &TranscriptEntry) -> Option<&str> {
    entry.get("kind").and_then(Value::as_str)
}
