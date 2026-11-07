use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SAND_XUSER_TURN_DEDUPE_FILE_NAME: &str = "host-xuser-turn-nonces.json";
pub const XUSER_TURN_DEDUPE_TTL_MS: u64 = 3 * 24 * 60 * 60 * 1_000;

pub trait XuserTurnDedupe: Send + Sync {
    fn mark_seen_if_new(&self, nonce: &str) -> bool;
}

pub fn parse_dedupe_file(raw: Option<&str>) -> BTreeMap<String, u64> {
    let mut map = BTreeMap::new();
    let Some(raw) = raw else { return map };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return map;
    };
    let Some(seen) = value.get("seen").and_then(|v| v.as_array()) else {
        return map;
    };
    for item in seen {
        let Some(nonce) = item
            .get("nonce")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        let Some(expires) = item.get("expiresAtMs").and_then(|v| v.as_u64()) else {
            continue;
        };
        map.insert(nonce.into(), expires);
    }
    map
}

fn persist(path: &Path, map: &BTreeMap<String, u64>) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let part = PathBuf::from(format!("{}.part", path.display()));
    let seen = map
        .iter()
        .map(|(nonce, expires)| serde_json::json!({"nonce":nonce,"expiresAtMs":expires}))
        .collect::<Vec<_>>();
    if fs::write(
        &part,
        serde_json::json!({"version":1,"seen":seen}).to_string(),
    )
    .is_ok()
    {
        let _ = fs::rename(part, path);
    }
}

pub struct SandXuserTurnDedupeStore {
    pub file_path: PathBuf,
    ttl_ms: u64,
    now_ms: Box<dyn Fn() -> u64 + Send + Sync>,
    cache: Mutex<Option<BTreeMap<String, u64>>>,
}

impl SandXuserTurnDedupeStore {
    pub fn new(
        root: &Path,
        ttl_ms: u64,
        now_ms: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            file_path: root.join(SAND_XUSER_TURN_DEDUPE_FILE_NAME),
            ttl_ms,
            now_ms,
            cache: Mutex::new(None),
        }
    }

    pub fn mark_seen_if_new(&self, nonce: &str) -> bool {
        <Self as XuserTurnDedupe>::mark_seen_if_new(self, nonce)
    }
}

impl XuserTurnDedupe for SandXuserTurnDedupeStore {
    fn mark_seen_if_new(&self, nonce: &str) -> bool {
        if nonce.is_empty() {
            return false;
        }
        let mut guard = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(parse_dedupe_file(
                fs::read_to_string(&self.file_path).ok().as_deref(),
            ));
        }
        let map = guard.as_mut().expect("dedupe cache initialized");
        let now = (self.now_ms)();
        let before = map.len();
        map.retain(|_, expires| *expires > now);
        let mut mutated = map.len() != before;
        if map.contains_key(nonce) {
            if mutated {
                persist(&self.file_path, map);
            }
            return false;
        }
        map.insert(nonce.into(), now.saturating_add(self.ttl_ms));
        mutated = true;
        if mutated {
            persist(&self.file_path, map);
        }
        true
    }
}

pub struct InMemoryXuserTurnDedupe {
    ttl_ms: u64,
    now_ms: Box<dyn Fn() -> u64 + Send + Sync>,
    map: Mutex<BTreeMap<String, u64>>,
}

impl InMemoryXuserTurnDedupe {
    pub fn new(
        ttl_ms: u64,
        now_ms: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            ttl_ms,
            now_ms,
            map: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn mark_seen_if_new(&self, nonce: &str) -> bool {
        <Self as XuserTurnDedupe>::mark_seen_if_new(self, nonce)
    }
}

impl XuserTurnDedupe for InMemoryXuserTurnDedupe {
    fn mark_seen_if_new(&self, nonce: &str) -> bool {
        if nonce.is_empty() {
            return false;
        }
        let now = (self.now_ms)();
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        map.retain(|_, expires| *expires > now);
        if map.contains_key(nonce) {
            return false;
        }
        map.insert(nonce.into(), now.saturating_add(self.ttl_ms));
        true
    }
}
