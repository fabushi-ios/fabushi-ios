use std::collections::HashMap;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMessage {
    pub message_id: i64,
    pub chat_id: i64,
    pub sender_id: u64,
    pub text: String,
    pub timestamp: i64,
    pub reply_to: Option<i64>,
    pub client_nonce: u64,
    #[serde(skip)]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct MessageDedupStore {
    entries: HashMap<(u64, u64), CachedMessage>,
    ttl_ms: i64,
    capacity: usize,
}

impl MessageDedupStore {
    pub const DEFAULT_TTL_MS: i64 = 24 * 3600 * 1000;
    pub const DEFAULT_CAPACITY: usize = 100_000;

    pub fn new() -> Self {
        Self::with_config(Self::DEFAULT_TTL_MS, Self::DEFAULT_CAPACITY)
    }

    pub fn with_config(ttl_ms: i64, capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            ttl_ms,
            capacity,
        }
    }

    pub fn get(&mut self, sender_id: u64, nonce: u64, now: DateTime<Utc>) -> Option<CachedMessage> {
        self.cleanup_expired(now);
        if let Some(entry) = self.entries.get(&(sender_id, nonce)) {
            let age_ms = now.signed_duration_since(entry.created_at).num_milliseconds();
            if age_ms < self.ttl_ms {
                return Some(entry.clone());
            }
        }
        None
    }

    pub fn insert(&mut self, sender_id: u64, nonce: u64, msg: CachedMessage, now: DateTime<Utc>) {
        self.cleanup_expired(now);
        if self.entries.len() >= self.capacity {
            if let Some(oldest_key) = self.entries.iter()
                .min_by_key(|(_, v)| v.created_at)
                .map(|(k, _)| *k)
            {
                self.entries.remove(&oldest_key);
            }
        }
        self.entries.insert((sender_id, nonce), msg);
    }

    pub fn cleanup_expired(&mut self, now: DateTime<Utc>) {
        let ttl_ms = self.ttl_ms;
        self.entries.retain(|_, v| {
            now.signed_duration_since(v.created_at).num_milliseconds() < ttl_ms
        });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn main() {
    println!("Testing MessageDedupStore with capacity 100,000...");
    let capacity = 100_000;
    let mut store = MessageDedupStore::with_config(MessageDedupStore::DEFAULT_TTL_MS, capacity);
    let t0 = Utc::now();

    println!("Inserting 100,000 entries...");
    for i in 1..=capacity {
        let msg = CachedMessage {
            message_id: i as i64,
            chat_id: 1,
            sender_id: 1,
            text: format!("Msg {i}"),
            timestamp: (t0 + Duration::milliseconds(i as i64)).timestamp_millis(),
            reply_to: None,
            client_nonce: i as u64,
            created_at: t0 + Duration::milliseconds(i as i64),
        };
        // Avoid cleanup_expired on every step by calling internal insert or letting it run
        store.entries.insert((1, i as u64), msg);
    }

    assert_eq!(store.len(), 100_000);
    println!("Store filled to capacity (100,000). Oldest entry is nonce 1.");

    // Now insert entry 100,001 via store.insert
    let t_next = t0 + Duration::milliseconds(100_001);
    let msg_next = CachedMessage {
        message_id: 100_001,
        chat_id: 1,
        sender_id: 1,
        text: "Msg 100001".to_string(),
        timestamp: t_next.timestamp_millis(),
        reply_to: None,
        client_nonce: 100_001,
        created_at: t_next,
    };

    println!("Inserting 100,001st entry via store.insert (triggers eviction)...");
    let start_evict = std::time::Instant::now();
    store.insert(1, 100_001, msg_next, t_next);
    let evict_duration = start_evict.elapsed();
    println!("Eviction completed in {:?}", evict_duration);

    assert_eq!(store.len(), 100_000, "Store length must remain capped at capacity 100,000");

    // Oldest item (nonce 1) should be evicted
    assert!(store.get(1, 1, t_next).is_none(), "Oldest entry (nonce 1) must be evicted");

    // Newer item (nonce 2) must still exist
    assert!(store.get(1, 2, t_next).is_some(), "Entry (nonce 2) must still exist");

    // Newest item (nonce 100,001) must exist
    assert!(store.get(1, 100_001, t_next).is_some(), "Newest entry (nonce 100,001) must exist");

    println!("✓ Capacity eviction beyond 100,000 entries verified successfully!");
}
