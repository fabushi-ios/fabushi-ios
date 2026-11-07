use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Rate limiter for webhook endpoints enforcing sliding-window admission.
#[derive(Debug, Clone)]
pub struct WebhookRateLimiter {
    max_requests_per_window: usize,
    window_duration: Duration,
    request_records: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
}

impl Default for WebhookRateLimiter {
    fn default() -> Self {
        Self::new(60, Duration::from_secs(60))
    }
}

impl WebhookRateLimiter {
    pub fn new(max_requests_per_window: usize, window_duration: Duration) -> Self {
        Self {
            max_requests_per_window,
            window_duration,
            request_records: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn check_admission(&self, key: &str) -> Result<(), &'static str> {
        let mut lock = self.request_records.lock().unwrap();
        let now = Instant::now();
        let cutoff = now.checked_sub(self.window_duration).unwrap_or(now);
        let records = lock.entry(key.to_string()).or_default();
        while let Some(&front) = records.front() {
            if front < cutoff {
                records.pop_front();
            } else {
                break;
            }
        }
        if records.len() >= self.max_requests_per_window {
            return Err("RATE_LIMIT_EXCEEDED: webhook rate limit exceeded");
        }
        records.push_back(now);
        Ok(())
    }
}
