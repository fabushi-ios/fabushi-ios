use gpui_telegram_proto::message::MessageItem;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

pub const DEFAULT_DEDUP_CAPACITY: usize = 10_000;
pub const DEFAULT_DEDUP_TTL_MS: i64 = 7 * 24 * 3600 * 1000; // 7 days

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedNonceEntry {
    pub message: MessageItem,
    pub pts: u64,
    pub timestamp_ms: i64,
}

#[derive(Debug)]
struct LruSlidingWindow {
    capacity: usize,
    ttl_ms: i64,
    entries: HashMap<(u64, u64), CachedNonceEntry>,
    order: VecDeque<(u64, u64)>,
}

impl LruSlidingWindow {
    fn new(capacity: usize, ttl_ms: i64) -> Self {
        Self {
            capacity: capacity.max(1),
            ttl_ms,
            entries: HashMap::with_capacity(capacity.min(1024)),
            order: VecDeque::with_capacity(capacity.min(1024)),
        }
    }

    fn get(&mut self, sender_id: u64, nonce: u64, now_ms: i64) -> Option<CachedNonceEntry> {
        let key = (sender_id, nonce);
        if let Some(entry) = self.entries.get(&key) {
            if self.ttl_ms > 0 && now_ms - entry.timestamp_ms > self.ttl_ms {
                self.entries.remove(&key);
                return None;
            }
            Some(entry.clone())
        } else {
            None
        }
    }

    fn insert(&mut self, sender_id: u64, nonce: u64, entry: CachedNonceEntry) {
        let key = (sender_id, nonce);

        if self.entries.contains_key(&key) {
            self.entries.insert(key, entry);
            return;
        }

        // Evict oldest items if at capacity
        while self.entries.len() >= self.capacity {
            if let Some(oldest_key) = self.order.pop_front() {
                self.entries.remove(&oldest_key);
            } else {
                break;
            }
        }

        self.entries.insert(key, entry);
        self.order.push_back(key);
    }

    fn prune_expired(&mut self, now_ms: i64) {
        if self.ttl_ms <= 0 {
            return;
        }
        let cutoff = now_ms - self.ttl_ms;
        self.entries.retain(|_, v| v.timestamp_ms >= cutoff);
        self.order.retain(|k| self.entries.contains_key(k));
    }
}

/// Two-tier deduplication: fast thread-safe in-memory LRU sliding window.
#[derive(Debug, Clone)]
pub struct MessageNonceDedup {
    inner: Arc<RwLock<LruSlidingWindow>>,
}

impl Default for MessageNonceDedup {
    fn default() -> Self {
        Self::new(DEFAULT_DEDUP_CAPACITY, DEFAULT_DEDUP_TTL_MS)
    }
}

impl MessageNonceDedup {
    pub fn new(capacity: usize, ttl_ms: i64) -> Self {
        Self {
            inner: Arc::new(RwLock::new(LruSlidingWindow::new(capacity, ttl_ms))),
        }
    }

    /// Looks up a cached message by (sender_id, nonce).
    pub fn get(&self, sender_id: u64, nonce: u64) -> Option<CachedNonceEntry> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.inner.write().get(sender_id, nonce, now_ms)
    }

    /// Inserts a message into the LRU cache.
    pub fn insert(&self, sender_id: u64, nonce: u64, entry: CachedNonceEntry) {
        self.inner.write().insert(sender_id, nonce, entry);
    }

    /// Checks if a nonce exists, returning true if present.
    pub fn contains(&self, sender_id: u64, nonce: u64) -> bool {
        self.get(sender_id, nonce).is_some()
    }

    /// Returns the current number of cached nonces.
    pub fn len(&self) -> usize {
        self.inner.read().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Prunes expired nonces from memory.
    pub fn prune_expired(&self) {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.inner.write().prune_expired(now_ms);
    }

    /// Backward-compatibility helper: returns true if inserted (not duplicate), false if seen.
    pub fn check_and_insert(&self, sender_id: u64, nonce: u64) -> bool {
        let mut window = self.inner.write();
        let now_ms = chrono::Utc::now().timestamp_millis();
        if window.get(sender_id, nonce, now_ms).is_some() {
            false
        } else {
            // Placeholder insertion
            let placeholder = CachedNonceEntry {
                message: MessageItem::new(
                    gpui_telegram_core::types::MsgId(0),
                    gpui_telegram_core::types::ChatId(0),
                    gpui_telegram_core::types::UserId(sender_id),
                    gpui_telegram_core::types::PeerId::User(gpui_telegram_core::types::UserId(sender_id)),
                    gpui_telegram_proto::message::MessageContent::Text { text: String::new(), entities: Vec::new() },
                ),
                pts: 0,
                timestamp_ms: now_ms,
            };
            window.insert(sender_id, nonce, placeholder);
            true
        }
    }
}
