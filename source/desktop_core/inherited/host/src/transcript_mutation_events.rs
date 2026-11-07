use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Map, Value};

pub type TranscriptMutation = Map<String, Value>;

pub const TRANSCRIPT_MUTATION_ORIGIN_KEY: &str = "originDbPath";

/// Internal database identity for post-commit projection only. Callers compute
/// this from their trusted writer/owner path, never a notification-supplied
/// path. The value must not enter Gateway events or public message metadata.
pub fn transcript_mutation_origin(db_path: &Path) -> Option<String> {
    let canonical = std::fs::canonicalize(db_path).ok()?;
    canonical.to_str().map(str::to_owned)
}
type TranscriptMutationListener = Arc<dyn Fn(&TranscriptMutation) + Send + Sync + 'static>;

fn listeners() -> &'static Mutex<BTreeMap<u64, TranscriptMutationListener>> {
    static LISTENERS: OnceLock<Mutex<BTreeMap<u64, TranscriptMutationListener>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn next_listener_id() -> u64 {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug)]
pub struct TranscriptMutationSubscription {
    id: u64,
    active: bool,
}

impl TranscriptMutationSubscription {
    pub fn unsubscribe(mut self) {
        if self.active {
            if let Ok(mut listeners) = listeners().lock() {
                listeners.remove(&self.id);
            }
            self.active = false;
        }
    }
}

pub fn subscribe_transcript_mutations<F>(listener: F) -> TranscriptMutationSubscription
where
    F: Fn(&TranscriptMutation) + Send + Sync + 'static,
{
    let id = next_listener_id();
    listeners()
        .lock()
        .expect("transcript mutation listener registry poisoned")
        .insert(id, Arc::new(listener));
    TranscriptMutationSubscription { id, active: true }
}

pub fn publish_transcript_mutation(mutation: &TranscriptMutation) {
    let snapshot = listeners()
        .lock()
        .map(|listeners| listeners.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for listener in snapshot {
        let _ = catch_unwind(AssertUnwindSafe(|| listener(mutation)));
    }
}
