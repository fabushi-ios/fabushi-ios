use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptMutation {
    pub kind: String,
    pub fields: Map<String, Value>,
}

type Listener = Arc<dyn Fn(&TranscriptMutation) + Send + Sync + 'static>;

fn listeners() -> &'static Mutex<BTreeMap<u64, Listener>> {
    static LISTENERS: OnceLock<Mutex<BTreeMap<u64, Listener>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn next_listener_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug)]
pub struct TranscriptMutationSubscription {
    id: u64,
    active: bool,
}

impl TranscriptMutationSubscription {
    pub fn unsubscribe(&mut self) {
        if self.active {
            if let Ok(mut map) = listeners().lock() {
                map.remove(&self.id);
            }
            self.active = false;
        }
    }
}

impl Drop for TranscriptMutationSubscription {
    fn drop(&mut self) {
        self.unsubscribe();
    }
}

pub fn subscribe_transcript_mutations(listener: Listener) -> TranscriptMutationSubscription {
    let id = next_listener_id();
    if let Ok(mut map) = listeners().lock() {
        map.insert(id, listener);
    }
    TranscriptMutationSubscription { id, active: true }
}

pub fn publish_transcript_mutation(mutation: &TranscriptMutation) {
    let current = listeners()
        .lock()
        .map(|map| map.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for listener in current {
        let _ = catch_unwind(AssertUnwindSafe(|| listener(mutation)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn publish_isolated_listener_failures_and_unsubscribe_stops_delivery() {
        let seen = Arc::new(Mutex::new(0usize));
        let sink = seen.clone();
        let _throwing = subscribe_transcript_mutations(Arc::new(|_| panic!("listener failure")));
        let mut subscription = subscribe_transcript_mutations(Arc::new(move |_| {
            *sink.lock().unwrap() += 1;
        }));
        let mutation = TranscriptMutation { kind: "append".into(), fields: Map::new() };
        publish_transcript_mutation(&mutation);
        assert_eq!(*seen.lock().unwrap(), 1);
        subscription.unsubscribe();
        publish_transcript_mutation(&mutation);
        assert_eq!(*seen.lock().unwrap(), 1);
    }
}
