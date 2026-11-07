use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// Context passed to queued channel work.
#[derive(Clone, Default)]
pub struct ChannelRunQueueTaskContext {
    pub is_active: bool,
}

/// Per-key async run queue used by channel plugins to serialize account or thread work.
#[derive(Clone)]
pub struct ChannelRunQueue {
    active: Arc<AtomicBool>,
    queues: Arc<Mutex<HashMap<String, VecDeque<Arc<dyn Fn() + Send + Sync>>>>>,
    notify: Arc<Notify>,
}

impl Default for ChannelRunQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelRunQueue {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(true)),
            queues: Arc::new(Mutex::new(HashMap::new())),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn deactivate(&self) {
        self.active.store(false, Ordering::SeqCst);
        let mut queues = self.queues.lock().unwrap();
        queues.clear();
        self.notify.notify_waiters();
    }

    pub fn enqueue(
        &self,
        key: &str,
        task: impl Fn(ChannelRunQueueTaskContext) + Send + Sync + 'static,
    ) -> bool {
        if !self.is_active() {
            return false;
        }

        let active = Arc::clone(&self.active);
        let task_arc = Arc::new(move || {
            task(ChannelRunQueueTaskContext {
                is_active: active.load(Ordering::SeqCst),
            });
        });

        let mut queues = self.queues.lock().unwrap();
        queues
            .entry(key.to_string())
            .or_default()
            .push_back(task_arc);
        self.notify.notify_one();
        true
    }

    /// Process the next task for a key synchronously if available.
    pub fn run_next(&self, key: &str) -> bool {
        if !self.is_active() {
            return false;
        }

        let task = {
            let mut queues = self.queues.lock().unwrap();
            queues.get_mut(key).and_then(|q| q.pop_front())
        };

        if let Some(t) = task {
            // Deactivation may race with dequeue. Do not execute work that was
            // removed from the queue after the lifecycle became inactive.
            if !self.is_active() {
                return false;
            }
            t();
            true
        } else {
            false
        }
    }
}

/// Stall watchdog for channel transports that can be armed, disarmed, and ticked.
#[derive(Clone)]
pub struct ArmableStallWatchdog {
    timeout: Duration,
    armed: Arc<AtomicBool>,
    last_tick: Arc<Mutex<Instant>>,
}

impl ArmableStallWatchdog {
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            timeout: Duration::from_millis(timeout_ms),
            armed: Arc::new(AtomicBool::new(false)),
            last_tick: Arc::new(Mutex::new(Instant::now())),
        }
    }

    pub fn arm(&self) {
        let mut tick = self.last_tick.lock().unwrap();
        *tick = Instant::now();
        self.armed.store(true, Ordering::SeqCst);
    }

    pub fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }

    pub fn tick(&self) {
        if self.is_armed() {
            let mut tick = self.last_tick.lock().unwrap();
            *tick = Instant::now();
        }
    }

    pub fn is_stalled(&self) -> bool {
        if !self.is_armed() {
            return false;
        }
        let tick = self.last_tick.lock().unwrap();
        tick.elapsed() >= self.timeout
    }
}
