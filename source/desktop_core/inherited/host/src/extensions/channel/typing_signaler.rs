//! Typing indicator lifecycle management and periodic heartbeat signaling (OC-S3).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Callback closure invoked on each typing heartbeat tick.
pub type TypingCallback = Arc<dyn Fn(&str, Option<&str>) -> Result<(), String> + Send + Sync>;

/// State tracker for an active typing indicator session.
struct ActiveTypingSession {
    target_id: String,
    thread_id: Option<String>,
    started_at: Instant,
    max_duration: Duration,
    stop_tx: watch::Sender<bool>,
}

/// Manages channel typing indicator lifecycle across running agent turns.
///
/// External chat platforms (Discord, Slack, Telegram, LINE) typically expire typing
/// indicators after 4–10 seconds. `TypingSignaler` sends periodic keepalive signals
/// every 4 seconds while an agent turn is actively executing, and stops immediately
/// upon completion, error, or automatic safety timeout.
#[derive(Clone)]
pub struct TypingSignaler {
    interval: Duration,
    default_timeout: Duration,
    sessions: Arc<Mutex<HashMap<String, ActiveTypingSession>>>,
}

impl Default for TypingSignaler {
    fn default() -> Self {
        Self::new(Duration::from_secs(4))
    }
}

impl TypingSignaler {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            default_timeout: Duration::from_secs(120), // 2-minute safety backstop
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Starts a periodic typing indicator heartbeat for the given session.
    ///
    /// If an existing session with `session_id` is already running, it is stopped first.
    pub fn start(
        &self,
        session_id: &str,
        target_id: &str,
        thread_id: Option<&str>,
        callback: TypingCallback,
    ) {
        self.stop(session_id);

        let (stop_tx, mut stop_rx) = watch::channel(false);
        let target = target_id.to_string();
        let thread = thread_id.map(String::from);
        let interval = self.interval;
        let max_duration = self.default_timeout;

        // Immediate first beat
        let _ = callback(&target, thread.as_deref());

        // Spawn background interval runner with safety timeout if tokio reactor is active
        let target_clone = target.clone();
        let thread_clone = thread.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let start_time = Instant::now();
                let mut ticker = tokio::time::interval(interval);
                ticker.tick().await; // consume immediate tick

                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            if start_time.elapsed() >= max_duration {
                                break; // Auto-timeout safety backstop
                            }
                            if callback(&target_clone, thread_clone.as_deref()).is_err() {
                                break; // Channel delivery failed or connection closed
                            }
                        }
                        changed = stop_rx.changed() => {
                            if changed.is_ok() && *stop_rx.borrow() {
                                break;
                            }
                        }
                    }
                }
            });
        }

        let mut lock = self.sessions.lock().unwrap();
        lock.insert(
            session_id.to_string(),
            ActiveTypingSession {
                target_id: target,
                thread_id: thread,
                started_at: Instant::now(),
                max_duration,
                stop_tx,
            },
        );
    }

    /// Stops the typing indicator heartbeat for the given session.
    pub fn stop(&self, session_id: &str) {
        let mut lock = self.sessions.lock().unwrap();
        if let Some(session) = lock.remove(session_id) {
            let _ = session.stop_tx.send(true);
        }
    }

    /// Returns whether a typing session is currently registered and active.
    pub fn is_active(&self, session_id: &str) -> bool {
        let lock = self.sessions.lock().unwrap();
        lock.contains_key(session_id)
    }

    /// Clears and cancels all active typing sessions.
    pub fn stop_all(&self) {
        let mut lock = self.sessions.lock().unwrap();
        for (_, session) in lock.drain() {
            let _ = session.stop_tx.send(true);
        }
    }

    /// Total count of active typing sessions.
    pub fn active_count(&self) -> usize {
        let lock = self.sessions.lock().unwrap();
        lock.len()
    }

    /// Inspect session targets for active typing indicators.
    pub fn list_active_session_ids(&self) -> Vec<String> {
        let lock = self.sessions.lock().unwrap();
        lock.keys().cloned().collect()
    }

    /// Returns the elapsed duration since a session began typing.
    pub fn session_elapsed(&self, session_id: &str) -> Option<Duration> {
        let lock = self.sessions.lock().unwrap();
        lock.get(session_id).map(|s| s.started_at.elapsed())
    }
}

/// RAII guard that automatically stops a typing session when dropped.
pub struct TypingScopeGuard {
    signaler: TypingSignaler,
    session_id: String,
}

impl TypingScopeGuard {
    pub fn new(
        signaler: TypingSignaler,
        session_id: impl Into<String>,
        target_id: &str,
        thread_id: Option<&str>,
        callback: TypingCallback,
    ) -> Self {
        let sid = session_id.into();
        signaler.start(&sid, target_id, thread_id, callback);
        Self {
            signaler,
            session_id: sid,
        }
    }
}

impl Drop for TypingScopeGuard {
    fn drop(&mut self) {
        self.signaler.stop(&self.session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_typing_signaler_lifecycle() {
        let signaler = TypingSignaler::new(Duration::from_millis(50));
        let counter = Arc::new(AtomicUsize::new(0));

        let c = counter.clone();
        let callback: TypingCallback = Arc::new(move |_target, _thread| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        signaler.start("sess-1", "channel-42", None, callback);
        assert!(signaler.is_active("sess-1"));
        assert_eq!(signaler.active_count(), 1);

        // Immediate tick happened
        assert!(counter.load(Ordering::SeqCst) >= 1);

        // Wait a couple of intervals
        tokio::time::sleep(Duration::from_millis(130)).await;
        let count_during = counter.load(Ordering::SeqCst);
        assert!(count_during >= 2);

        // Stop session
        signaler.stop("sess-1");
        assert!(!signaler.is_active("sess-1"));
        assert_eq!(signaler.active_count(), 0);

        // Verify ticks stop incrementing
        tokio::time::sleep(Duration::from_millis(80)).await;
        let count_after = counter.load(Ordering::SeqCst);
        assert!(count_after == count_during || count_after == count_during + 1);
    }

    #[tokio::test]
    async fn test_typing_scope_guard_raii() {
        let signaler = TypingSignaler::new(Duration::from_millis(50));
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();

        {
            let _guard = TypingScopeGuard::new(
                signaler.clone(),
                "guard-sess",
                "chat-1",
                None,
                Arc::new(move |_t, _th| {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            );
            assert!(signaler.is_active("guard-sess"));
        }

        // Dropped -> stopped
        assert!(!signaler.is_active("guard-sess"));
    }
}
