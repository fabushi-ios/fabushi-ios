use std::collections::HashMap;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone)]
pub enum InFlightState {
    InProgress,
    Completed(CachedMessage),
    Aborted,
}

#[derive(Debug)]
pub struct InFlightSlot {
    state: Mutex<InFlightState>,
    cvar: Condvar,
    pub created_at: DateTime<Utc>,
}

impl InFlightSlot {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            state: Mutex::new(InFlightState::InProgress),
            cvar: Condvar::new(),
            created_at: now,
        }
    }

    pub fn complete(&self, msg: CachedMessage) {
        let mut guard = self.state.lock();
        *guard = InFlightState::Completed(msg);
        self.cvar.notify_all();
    }

    pub fn abort(&self) {
        let mut guard = self.state.lock();
        *guard = InFlightState::Aborted;
        self.cvar.notify_all();
    }

    pub fn wait(&self) -> Result<CachedMessage, DedupWaitError> {
        let mut guard = self.state.lock();
        while let InFlightState::InProgress = *guard {
            self.cvar.wait(&mut guard);
        }
        match &*guard {
            InFlightState::Completed(msg) => Ok(msg.clone()),
            InFlightState::Aborted => Err(DedupWaitError::Aborted),
            InFlightState::InProgress => unreachable!(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupWaitError {
    Aborted,
}

#[derive(Debug, Clone)]
pub enum DedupEntry {
    InFlight(Arc<InFlightSlot>),
    Completed(CachedMessage),
}

impl DedupEntry {
    pub fn created_at(&self) -> DateTime<Utc> {
        match self {
            DedupEntry::InFlight(slot) => slot.created_at,
            DedupEntry::Completed(msg) => msg.created_at,
        }
    }
}

pub enum ReservationOutcome {
    Leader(Arc<InFlightSlot>),
    Follower(Arc<InFlightSlot>),
    AlreadyExists(CachedMessage),
}

#[derive(Debug)]
pub struct MessageDedupStore {
    // Key: (sender_id, client_nonce)
    entries: HashMap<(u64, u64), DedupEntry>,
    ttl_ms: i64,
    capacity: usize,
}

impl Default for MessageDedupStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageDedupStore {
    pub const DEFAULT_TTL_MS: i64 = 24 * 3600 * 1000; // 24 hours (86,400,000 ms)
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

    pub fn reserve(&mut self, sender_id: u64, nonce: u64, now: DateTime<Utc>) -> ReservationOutcome {
        self.cleanup_expired(now);

        let key = (sender_id, nonce);
        if let Some(entry) = self.entries.get(&key) {
            match entry {
                DedupEntry::InFlight(slot) => {
                    return ReservationOutcome::Follower(Arc::clone(slot));
                }
                DedupEntry::Completed(cached) => {
                    let age_ms = now.signed_duration_since(cached.created_at).num_milliseconds();
                    if age_ms < self.ttl_ms {
                        return ReservationOutcome::AlreadyExists(cached.clone());
                    }
                }
            }
        }

        if self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            if let Some(oldest_key) = self.entries.iter()
                .min_by_key(|(_, v)| v.created_at())
                .map(|(k, _)| *k)
            {
                self.entries.remove(&oldest_key);
            }
        }

        let slot = Arc::new(InFlightSlot::new(now));
        self.entries.insert(key, DedupEntry::InFlight(Arc::clone(&slot)));
        ReservationOutcome::Leader(slot)
    }

    pub fn complete(&mut self, sender_id: u64, nonce: u64, msg: CachedMessage) {
        let key = (sender_id, nonce);
        if let Some(DedupEntry::InFlight(slot)) = self.entries.get(&key) {
            slot.complete(msg.clone());
        }
        self.entries.insert(key, DedupEntry::Completed(msg));
    }

    pub fn abort(&mut self, sender_id: u64, nonce: u64) {
        let key = (sender_id, nonce);
        match self.entries.get(&key) {
            Some(DedupEntry::InFlight(_)) => {
                if let Some(DedupEntry::InFlight(slot)) = self.entries.remove(&key) {
                    slot.abort();
                }
            }
            _ => {}
        }
    }

    pub fn get(&mut self, sender_id: u64, nonce: u64, now: DateTime<Utc>) -> Option<CachedMessage> {
        self.cleanup_expired(now);
        if let Some(DedupEntry::Completed(entry)) = self.entries.get(&(sender_id, nonce)) {
            let age_ms = now.signed_duration_since(entry.created_at).num_milliseconds();
            if age_ms < self.ttl_ms {
                return Some(entry.clone());
            }
        }
        None
    }

    pub fn insert(&mut self, sender_id: u64, nonce: u64, msg: CachedMessage, now: DateTime<Utc>) {
        self.cleanup_expired(now);
        let key = (sender_id, nonce);
        if let Some(DedupEntry::InFlight(slot)) = self.entries.get(&key) {
            slot.complete(msg.clone());
        }
        if self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            if let Some(oldest_key) = self.entries.iter()
                .min_by_key(|(_, v)| v.created_at())
                .map(|(k, _)| *k)
            {
                self.entries.remove(&oldest_key);
            }
        }
        self.entries.insert(key, DedupEntry::Completed(msg));
    }

    pub fn cleanup_expired(&mut self, now: DateTime<Utc>) {
        let ttl_ms = self.ttl_ms;
        self.entries.retain(|_, v| {
            now.signed_duration_since(v.created_at()).num_milliseconds() < ttl_ms
        });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_dedup_unique_and_duplicate() {
        let mut store = MessageDedupStore::new();
        let now = Utc::now();

        let cached = CachedMessage {
            message_id: 1,
            chat_id: 10,
            sender_id: 42,
            text: "Hello".to_string(),
            timestamp: now.timestamp_millis(),
            reply_to: None,
            client_nonce: 9999,
            created_at: now,
        };

        assert!(store.get(42, 9999, now).is_none());
        store.insert(42, 9999, cached.clone(), now);

        let retrieved = store.get(42, 9999, now).expect("Should find cached message");
        assert_eq!(retrieved.message_id, 1);
        assert_eq!(retrieved.client_nonce, 9999);

        // Different sender, same nonce -> None
        assert!(store.get(43, 9999, now).is_none());
    }

    #[test]
    fn test_dedup_24h_ttl_expiry() {
        let mut store = MessageDedupStore::new();
        let start = Utc::now();

        let cached = CachedMessage {
            message_id: 2,
            chat_id: 10,
            sender_id: 42,
            text: "Test TTL".to_string(),
            timestamp: start.timestamp_millis(),
            reply_to: None,
            client_nonce: 12345,
            created_at: start,
        };

        store.insert(42, 12345, cached, start);

        // 23 hours later: still valid
        let within_ttl = start + Duration::hours(23);
        assert!(store.get(42, 12345, within_ttl).is_some());

        // 24 hours + 1 minute later: expired
        let after_ttl = start + Duration::hours(24) + Duration::minutes(1);
        assert!(store.get(42, 12345, after_ttl).is_none());
    }

    #[test]
    fn test_dedup_capacity_eviction() {
        let mut store = MessageDedupStore::with_config(MessageDedupStore::DEFAULT_TTL_MS, 3);
        let t0 = Utc::now();

        for i in 1..=3 {
            let msg = CachedMessage {
                message_id: i,
                chat_id: 10,
                sender_id: 1,
                text: format!("Msg {i}"),
                timestamp: (t0 + Duration::seconds(i)).timestamp_millis(),
                reply_to: None,
                client_nonce: i as u64,
                created_at: t0 + Duration::seconds(i),
            };
            store.insert(1, i as u64, msg, t0 + Duration::seconds(i));
        }

        assert_eq!(store.len(), 3);

        // Insert 4th item at t0 + 4s, oldest (nonce 1) should be evicted
        let t4 = t0 + Duration::seconds(4);
        let msg4 = CachedMessage {
            message_id: 4,
            chat_id: 10,
            sender_id: 1,
            text: "Msg 4".to_string(),
            timestamp: t4.timestamp_millis(),
            reply_to: None,
            client_nonce: 4,
            created_at: t4,
        };
        store.insert(1, 4, msg4, t4);

        assert_eq!(store.len(), 3);
        assert!(store.get(1, 1, t4).is_none());
        assert!(store.get(1, 2, t4).is_some());
        assert!(store.get(1, 3, t4).is_some());
        assert!(store.get(1, 4, t4).is_some());
    }

    #[test]
    fn test_boundary_nonces() {
        let mut store = MessageDedupStore::new();
        let now = Utc::now();

        // Nonce 0
        let msg0 = CachedMessage {
            message_id: 100,
            chat_id: 1,
            sender_id: 1,
            text: "Zero".to_string(),
            timestamp: now.timestamp_millis(),
            reply_to: None,
            client_nonce: 0,
            created_at: now,
        };
        store.insert(1, 0, msg0, now);
        assert_eq!(store.get(1, 0, now).unwrap().message_id, 100);

        // Nonce u64::MAX
        let msg_max = CachedMessage {
            message_id: 101,
            chat_id: 1,
            sender_id: 1,
            text: "Max".to_string(),
            timestamp: now.timestamp_millis(),
            reply_to: None,
            client_nonce: u64::MAX,
            created_at: now,
        };
        store.insert(1, u64::MAX, msg_max, now);
        assert_eq!(store.get(1, u64::MAX, now).unwrap().message_id, 101);
    }

    #[test]
    fn test_in_flight_leader_follower_completion() {
        let mut store = MessageDedupStore::new();
        let now = Utc::now();

        let outcome1 = store.reserve(1, 1001, now);
        let leader_slot = match outcome1 {
            ReservationOutcome::Leader(slot) => slot,
            _ => panic!("Expected Leader"),
        };

        let outcome2 = store.reserve(1, 1001, now);
        let follower_slot = match outcome2 {
            ReservationOutcome::Follower(slot) => slot,
            _ => panic!("Expected Follower"),
        };

        let handle = std::thread::spawn(move || {
            follower_slot.wait().expect("Follower should succeed")
        });

        let cached = CachedMessage {
            message_id: 555,
            chat_id: 10,
            sender_id: 1,
            text: "Hello".to_string(),
            timestamp: now.timestamp_millis(),
            reply_to: None,
            client_nonce: 1001,
            created_at: now,
        };

        leader_slot.complete(cached.clone());
        store.complete(1, 1001, cached);

        let result = handle.join().unwrap();
        assert_eq!(result.message_id, 555);

        // Third reservation should see AlreadyExists
        let outcome3 = store.reserve(1, 1001, now);
        match outcome3 {
            ReservationOutcome::AlreadyExists(msg) => assert_eq!(msg.message_id, 555),
            _ => panic!("Expected AlreadyExists"),
        }
    }

    #[test]
    fn test_in_flight_leader_abort_follower_recovery() {
        let mut store = MessageDedupStore::new();
        let now = Utc::now();

        let outcome1 = store.reserve(2, 2002, now);
        let _leader_slot = match outcome1 {
            ReservationOutcome::Leader(slot) => slot,
            _ => panic!("Expected Leader"),
        };

        let outcome2 = store.reserve(2, 2002, now);
        let follower_slot = match outcome2 {
            ReservationOutcome::Follower(slot) => slot,
            _ => panic!("Expected Follower"),
        };

        let handle = std::thread::spawn(move || {
            follower_slot.wait()
        });

        // Abort the leader
        store.abort(2, 2002);

        let err = handle.join().unwrap();
        assert_eq!(err, Err(DedupWaitError::Aborted));

        // After abort, next reserve becomes Leader again
        let outcome3 = store.reserve(2, 2002, now);
        match outcome3 {
            ReservationOutcome::Leader(_) => {}
            _ => panic!("Expected new Leader after abort"),
        }
    }
}
