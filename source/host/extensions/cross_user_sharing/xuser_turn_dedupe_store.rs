use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SAND_XUSER_TURN_DEDUPE_FILE_NAME: &str = "host-xuser-turn-nonces.json";
pub const XUSER_TURN_DEDUPE_TTL_MS: u64 = 3 * 24 * 60 * 60 * 1_000;

pub struct SandXuserTurnDedupeStore {
    file_path: PathBuf,
    ttl_ms: u64,
    now_ms: Box<dyn Fn() -> u64 + Send + Sync>,
    cache: Mutex<Option<BTreeMap<String, u64>>>,
}

impl SandXuserTurnDedupeStore {
    pub fn new(root: &Path, ttl_ms: u64, now_ms: Box<dyn Fn() -> u64 + Send + Sync>) -> Self {
        Self {
            file_path: root.join(SAND_XUSER_TURN_DEDUPE_FILE_NAME),
            ttl_ms,
            now_ms,
            cache: Mutex::new(None),
        }
    }

    pub fn mark_seen_if_new(&self, nonce: &str) -> bool {
        if nonce.trim().is_empty() {
            return false;
        }
        let mut guard = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_none() {
            *guard = Some(parse_dedupe_file(
                fs::read_to_string(&self.file_path).ok().as_deref(),
            ));
        }
        let map = guard.as_mut().expect("dedupe cache initialized");
        let now = (self.now_ms)();
        let before = map.len();
        map.retain(|_, expires_at| *expires_at > now);
        let pruned = before != map.len();
        if map.contains_key(nonce) {
            if pruned {
                persist(&self.file_path, map);
            }
            return false;
        }
        map.insert(nonce.to_string(), now.saturating_add(self.ttl_ms));
        persist(&self.file_path, map);
        true
    }
}

fn parse_dedupe_file(raw: Option<&str>) -> BTreeMap<String, u64> {
    let mut map = BTreeMap::new();
    let Some(raw) = raw else { return map };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else { return map };
    let Some(seen) = value.get("seen").and_then(serde_json::Value::as_array) else { return map };
    for item in seen {
        let Some(nonce) = item.get("nonce").and_then(serde_json::Value::as_str).filter(|value| !value.is_empty()) else { continue };
        let Some(expires_at_ms) = item.get("expiresAtMs").and_then(serde_json::Value::as_u64) else { continue };
        map.insert(nonce.to_string(), expires_at_ms);
    }
    map
}

fn persist(path: &Path, map: &BTreeMap<String, u64>) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let part = PathBuf::from(format!("{}.part", path.display()));
    let seen = map.iter().map(|(nonce, expires_at_ms)| {
        serde_json::json!({"nonce": nonce, "expiresAtMs": expires_at_ms})
    }).collect::<Vec<_>>();
    if fs::write(&part, serde_json::json!({"version": 1, "seen": seen}).to_string()).is_ok() {
        let _ = fs::rename(part, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-xuser-dedupe-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn durable_nonce_survives_restart_expires_and_fails_closed() {
        let root = root();
        let now = Arc::new(AtomicU64::new(1_000));
        let clock = {
            let now = Arc::clone(&now);
            Box::new(move || now.load(Ordering::SeqCst)) as Box<dyn Fn() -> u64 + Send + Sync>
        };
        let store = SandXuserTurnDedupeStore::new(&root, 100, clock);
        assert!(!store.mark_seen_if_new(""));
        assert!(store.mark_seen_if_new("nonce-1"));
        assert!(!store.mark_seen_if_new("nonce-1"));
        drop(store);

        let clock = {
            let now = Arc::clone(&now);
            Box::new(move || now.load(Ordering::SeqCst)) as Box<dyn Fn() -> u64 + Send + Sync>
        };
        let reopened = SandXuserTurnDedupeStore::new(&root, 100, clock);
        assert!(!reopened.mark_seen_if_new("nonce-1"));
        now.store(1_101, Ordering::SeqCst);
        assert!(reopened.mark_seen_if_new("nonce-1"));
        assert!(root.join(SAND_XUSER_TURN_DEDUPE_FILE_NAME).is_file());
        assert!(!root.join(format!("{SAND_XUSER_TURN_DEDUPE_FILE_NAME}.part")).exists());
        let _ = fs::remove_dir_all(root);
    }
}
