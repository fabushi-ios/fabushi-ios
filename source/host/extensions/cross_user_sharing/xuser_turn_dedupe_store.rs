use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
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

    pub fn mark_seen_if_new(&self, nonce: &str) -> Result<bool, String> {
        let nonce = nonce.trim();
        if nonce.is_empty() {
            return Ok(false);
        }
        let mut guard = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_none() {
            *guard = Some(load(&self.file_path)?);
        }
        let current = guard.as_ref().expect("dedupe cache initialized");
        let now = (self.now_ms)();
        let mut candidate = current.clone();
        candidate.retain(|_, expires_at| *expires_at > now);

        if candidate.contains_key(nonce) {
            if &candidate != current {
                persist(&self.file_path, &candidate)?;
                *guard = Some(candidate);
            }
            return Ok(false);
        }

        candidate.insert(nonce.to_string(), now.saturating_add(self.ttl_ms));
        // Fail closed: only publish the in-memory admission after the durable
        // temp-write + fsync + atomic rename has completed.
        persist(&self.file_path, &candidate)?;
        *guard = Some(candidate);
        Ok(true)
    }
}

fn load(path: &Path) -> Result<BTreeMap<String, u64>, String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("read xuser nonce store {}: {error}", path.display())),
    };
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("decode xuser nonce store: {error}"))?;
    let seen = value.get("seen").and_then(serde_json::Value::as_array)
        .ok_or_else(|| "decode xuser nonce store: seen array is required".to_string())?;
    let mut map = BTreeMap::new();
    for item in seen {
        let nonce = item.get("nonce").and_then(serde_json::Value::as_str)
            .map(str::trim).filter(|value| !value.is_empty())
            .ok_or_else(|| "decode xuser nonce store: nonce is required".to_string())?;
        let expires_at_ms = item.get("expiresAtMs").and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "decode xuser nonce store: expiresAtMs is required".to_string())?;
        map.insert(nonce.to_string(), expires_at_ms);
    }
    Ok(map)
}

fn persist(path: &Path, map: &BTreeMap<String, u64>) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "xuser nonce store has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create xuser nonce directory {}: {error}", parent.display()))?;
    let part = PathBuf::from(format!("{}.part", path.display()));
    let seen = map.iter().map(|(nonce, expires_at_ms)| {
        serde_json::json!({"nonce": nonce, "expiresAtMs": expires_at_ms})
    }).collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"seen":seen}))
        .map_err(|error| format!("encode xuser nonce store: {error}"))?;
    let mut file = File::create(&part)
        .map_err(|error| format!("create xuser nonce temp {}: {error}", part.display()))?;
    file.write_all(&bytes).and_then(|_| file.flush()).and_then(|_| file.sync_all())
        .map_err(|error| format!("write xuser nonce temp {}: {error}", part.display()))?;
    fs::rename(&part, path).map_err(|error| {
        let _ = fs::remove_file(&part);
        format!("replace xuser nonce store {}: {error}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-xuser-dedupe-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn durable_nonce_survives_restart_and_expires() {
        let root = root("restart");
        let now = Arc::new(AtomicU64::new(1_000));
        {
            let clock = Arc::clone(&now);
            let store = SandXuserTurnDedupeStore::new(
                &root, 100, Box::new(move || clock.load(Ordering::SeqCst))
            );
            assert_eq!(store.mark_seen_if_new("").unwrap(), false);
            assert_eq!(store.mark_seen_if_new("nonce-1").unwrap(), true);
            assert_eq!(store.mark_seen_if_new("nonce-1").unwrap(), false);
        }
        {
            let clock = Arc::clone(&now);
            let store = SandXuserTurnDedupeStore::new(
                &root, 100, Box::new(move || clock.load(Ordering::SeqCst))
            );
            assert_eq!(store.mark_seen_if_new("nonce-1").unwrap(), false);
            now.store(1_101, Ordering::SeqCst);
            assert_eq!(store.mark_seen_if_new("nonce-1").unwrap(), true);
        }
        assert!(!root.join(format!("{SAND_XUSER_TURN_DEDUPE_FILE_NAME}.part")).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_store_fails_closed_instead_of_reexecuting_turn() {
        let root = root("malformed");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(SAND_XUSER_TURN_DEDUPE_FILE_NAME), "{broken").unwrap();
        let store = SandXuserTurnDedupeStore::new(&root, 100, Box::new(|| 1));
        assert!(store.mark_seen_if_new("nonce").is_err());
        let _ = fs::remove_dir_all(root);
    }
}
