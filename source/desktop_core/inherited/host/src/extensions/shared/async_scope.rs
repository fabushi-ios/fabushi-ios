use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct AsyncWorkTracker {
    active_count: Arc<AtomicUsize>,
}

impl AsyncWorkTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enter(&self) -> AsyncWorkGuard {
        self.active_count.fetch_add(1, Ordering::SeqCst);
        AsyncWorkGuard {
            active_count: Arc::clone(&self.active_count),
        }
    }

    pub fn active_tasks(&self) -> usize {
        self.active_count.load(Ordering::SeqCst)
    }
}

pub struct AsyncWorkGuard {
    active_count: Arc<AtomicUsize>,
}

impl Drop for AsyncWorkGuard {
    fn drop(&mut self) {
        self.active_count.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn calculate_exponential_retry(attempt: u32, base_ms: u64, max_ms: u64) -> Duration {
    let factor = 1u64.checked_shl(attempt.min(10)).unwrap_or(1024);
    let ms = base_ms.saturating_mul(factor).min(max_ms);
    Duration::from_millis(ms)
}

pub fn normalize_unique_strings(strings: &[&str]) -> Vec<String> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for s in strings {
        let trimmed = s.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
            result.push(trimmed.to_string());
        }
    }
    result
}
