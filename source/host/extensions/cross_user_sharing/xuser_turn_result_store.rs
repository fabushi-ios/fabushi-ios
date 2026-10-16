use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SAND_XUSER_TURN_RESULTS_FILE_NAME: &str = "host-xuser-turn-results.json";
pub const XUSER_TURN_RESULT_TTL_MS: u64 = 3 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
enum StoredTurn {
    Reserved { expires_at_ms: u64 },
    Completed { expires_at_ms: u64, messages: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnAdmission {
    New,
    Reserved,
    Completed(Vec<String>),
}

pub struct SandXuserTurnResultStore {
    file_path: PathBuf,
    ttl_ms: u64,
    now_ms: Box<dyn Fn() -> u64 + Send + Sync>,
    cache: Mutex<Option<BTreeMap<String, StoredTurn>>>,
}

impl SandXuserTurnResultStore {
    pub fn new(root: &Path, ttl_ms: u64, now_ms: Box<dyn Fn() -> u64 + Send + Sync>) -> Self {
        Self {
            file_path: root.join(SAND_XUSER_TURN_RESULTS_FILE_NAME),
            ttl_ms,
            now_ms,
            cache: Mutex::new(None),
        }
    }

    pub fn admit(&self, nonce: &str) -> Result<TurnAdmission, String> {
        let nonce = nonce.trim();
        if nonce.is_empty() {
            return Err("xuser turn nonce is required".into());
        }
        let now = (self.now_ms)();
        self.with_map(|map| {
            map.retain(|_, turn| expires_at_ms(turn) > now);
            match map.get(nonce) {
                Some(StoredTurn::Reserved { .. }) => Ok((TurnAdmission::Reserved, true)),
                Some(StoredTurn::Completed { messages, .. }) => {
                    Ok((TurnAdmission::Completed(messages.clone()), true))
                }
                None => {
                    map.insert(
                        nonce.to_string(),
                        StoredTurn::Reserved {
                            expires_at_ms: now.saturating_add(self.ttl_ms),
                        },
                    );
                    Ok((TurnAdmission::New, true))
                }
            }
        })
    }

    pub fn complete(&self, nonce: &str, messages: Vec<String>) -> Result<(), String> {
        let nonce = nonce.trim();
        if nonce.is_empty() {
            return Err("xuser turn nonce is required".into());
        }
        let now = (self.now_ms)();
        let messages = messages.into_iter().take(2).collect::<Vec<_>>();
        self.with_map(|map| {
            map.insert(
                nonce.to_string(),
                StoredTurn::Completed {
                    expires_at_ms: now.saturating_add(self.ttl_ms),
                    messages,
                },
            );
            Ok(((), true))
        })
    }

    fn with_map<R>(
        &self,
        operation: impl FnOnce(&mut BTreeMap<String, StoredTurn>) -> Result<(R, bool), String>,
    ) -> Result<R, String> {
        let mut guard = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_none() {
            *guard = Some(load(&self.file_path)?);
        }
        let map = guard.as_mut().expect("turn result cache initialized");
        let before = map.clone();
        let (output, changed) = operation(map)?;
        if changed {
            if let Err(error) = persist(&self.file_path, map) {
                *map = before;
                return Err(error);
            }
        }
        Ok(output)
    }
}

fn expires_at_ms(turn: &StoredTurn) -> u64 {
    match turn {
        StoredTurn::Reserved { expires_at_ms }
        | StoredTurn::Completed { expires_at_ms, .. } => *expires_at_ms,
    }
}

fn load(path: &Path) -> Result<BTreeMap<String, StoredTurn>, String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("read xuser turn result store {}: {error}", path.display())),
    };
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("decode xuser turn result store: {error}"))?;
    let rows = value.get("turns").and_then(serde_json::Value::as_object)
        .ok_or_else(|| "decode xuser turn result store: turns object is required".to_string())?;
    let mut output = BTreeMap::new();
    for (nonce, value) in rows {
        let turn: StoredTurn = serde_json::from_value(value.clone())
            .map_err(|error| format!("decode xuser turn result row: {error}"))?;
        output.insert(nonce.clone(), turn);
    }
    Ok(output)
}

fn persist(path: &Path, map: &BTreeMap<String, StoredTurn>) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "xuser turn result store has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create xuser turn result directory {}: {error}", parent.display()))?;
    let part = PathBuf::from(format!("{}.part", path.display()));
    let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"turns":map}))
        .map_err(|error| format!("encode xuser turn result store: {error}"))?;
    let mut file = File::create(&part)
        .map_err(|error| format!("create xuser turn result temp {}: {error}", part.display()))?;
    file.write_all(&bytes).and_then(|_| file.flush()).and_then(|_| file.sync_all())
        .map_err(|error| format!("write xuser turn result temp {}: {error}", part.display()))?;
    fs::rename(&part, path).map_err(|error| {
        let _ = fs::remove_file(&part);
        format!("replace xuser turn result store {}: {error}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-xuser-turn-results-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn completed_result_survives_restart_and_resends_without_reexecution() {
        let root = root();
        let clock = Arc::new(AtomicU64::new(10));
        {
            let now = Arc::clone(&clock);
            let store = SandXuserTurnResultStore::new(
                &root, 100, Box::new(move || now.load(Ordering::SeqCst))
            );
            assert_eq!(store.admit("nonce").unwrap(), TurnAdmission::New);
            assert_eq!(store.admit("nonce").unwrap(), TurnAdmission::Reserved);
            store.complete("nonce", vec!["one".into(), "two".into(), "three".into()]).unwrap();
        }
        {
            let now = Arc::clone(&clock);
            let store = SandXuserTurnResultStore::new(
                &root, 100, Box::new(move || now.load(Ordering::SeqCst))
            );
            assert_eq!(
                store.admit("nonce").unwrap(),
                TurnAdmission::Completed(vec!["one".into(), "two".into()])
            );
            clock.store(111, Ordering::SeqCst);
            assert_eq!(store.admit("nonce").unwrap(), TurnAdmission::New);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_outbox_fails_closed() {
        let root = root();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(SAND_XUSER_TURN_RESULTS_FILE_NAME), "{bad").unwrap();
        let store = SandXuserTurnResultStore::new(&root, 100, Box::new(|| 1));
        assert!(store.admit("nonce").is_err());
        let _ = fs::remove_dir_all(root);
    }
}
