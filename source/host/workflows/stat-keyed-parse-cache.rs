use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

pub const DEFAULT_STAT_PARSE_CACHE_CAPACITY: usize = 2_048;
pub const RACY_MTIME_TICK_WINDOW_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatFingerprint {
    pub stat_key: String,
    pub newest_mtime_ms: u64,
}

pub fn mtime_tick_could_still_hide_an_edit(
    fingerprint: &StatFingerprint,
    now_ms: u64,
) -> bool {
    now_ms.saturating_sub(fingerprint.newest_mtime_ms) < RACY_MTIME_TICK_WINDOW_MS
}

fn metadata_identity(metadata: &fs::Metadata) -> (u64, i128, u64, u64) {
    #[cfg(unix)]
    {
        let seconds = i128::from(metadata.mtime());
        let nanos = i128::from(metadata.mtime_nsec());
        let mtime_ns = seconds.saturating_mul(1_000_000_000).saturating_add(nanos);
        let newest_mtime_ms = if mtime_ns <= 0 {
            0
        } else {
            u64::try_from(mtime_ns / 1_000_000).unwrap_or(u64::MAX)
        };
        return (
            metadata.ino(),
            mtime_ns,
            metadata.len(),
            newest_mtime_ms,
        );
    }

    #[cfg(not(unix))]
    {
        let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
        let duration = modified.duration_since(UNIX_EPOCH).unwrap_or_default();
        let mtime_ns = i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX);
        (
            0,
            mtime_ns,
            metadata.len(),
            duration.as_millis().try_into().unwrap_or(u64::MAX),
        )
    }
}

fn fingerprint_of(paths: &[PathBuf]) -> Result<Option<StatFingerprint>, std::io::Error> {
    let mut parts = Vec::with_capacity(paths.len());
    let mut newest_mtime_ms = 0u64;
    for path in paths {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let (inode, mtime_ns, size, mtime_ms) = metadata_identity(&metadata);
        parts.push(format!("{inode}:{mtime_ns}:{size}"));
        newest_mtime_ms = newest_mtime_ms.max(mtime_ms);
    }
    Ok(Some(StatFingerprint {
        stat_key: parts.join("|"),
        newest_mtime_ms,
    }))
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[derive(Debug, Clone)]
struct CacheEntry<Value> {
    stat_key: String,
    value: Value,
}

/// iOS/Rust equivalent of Grok's insertion-ordered stat-keyed parse cache.
///
/// Cache hits do not refresh insertion order, matching JavaScript Map behavior.
/// A fingerprint from the current filesystem clock tick is intentionally not
/// cached because coarse mtimes can otherwise hide a same-tick edit.
pub struct StatKeyedParseCache<Value> {
    capacity: usize,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    entries: HashMap<String, CacheEntry<Value>>,
    insertion_order: VecDeque<String>,
}

impl<Value> Default for StatKeyedParseCache<Value>
where
    Value: Clone,
{
    fn default() -> Self {
        Self::new(DEFAULT_STAT_PARSE_CACHE_CAPACITY)
    }
}

impl<Value> StatKeyedParseCache<Value>
where
    Value: Clone,
{
    pub fn new(capacity: usize) -> Self {
        Self::with_clock(capacity, Arc::new(system_now_ms))
    }

    pub fn with_clock(
        capacity: usize,
        now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            capacity,
            now_ms,
            entries: HashMap::new(),
            insertion_order: VecDeque::new(),
        }
    }

    fn key(paths: &[PathBuf]) -> String {
        paths
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\0")
    }

    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
        if let Some(index) = self.insertion_order.iter().position(|entry| entry == key) {
            self.insertion_order.remove(index);
        }
    }

    fn insert(&mut self, key: String, entry: CacheEntry<Value>) {
        self.remove(&key);
        if self.capacity == 0 {
            return;
        }
        while self.entries.len() >= self.capacity {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.insertion_order.push_back(key.clone());
        self.entries.insert(key, entry);
    }

    pub fn read<P, Parse>(&mut self, stat_paths: &[P], parse: Parse) -> Option<Value>
    where
        P: AsRef<Path>,
        Parse: FnOnce() -> Value,
    {
        let paths = stat_paths
            .iter()
            .map(|path| path.as_ref().to_path_buf())
            .collect::<Vec<_>>();
        let key = Self::key(&paths);
        let fingerprint = match fingerprint_of(&paths) {
            Ok(Some(fingerprint)) => fingerprint,
            Ok(None) => {
                self.remove(&key);
                return None;
            }
            Err(_) => return Some(parse()),
        };

        if let Some(hit) = self.entries.get(&key) {
            if hit.stat_key == fingerprint.stat_key {
                return Some(hit.value.clone());
            }
        }

        let value = parse();
        self.remove(&key);
        if mtime_tick_could_still_hide_an_edit(&fingerprint, (self.now_ms)()) {
            return Some(value);
        }
        self.insert(
            key,
            CacheEntry {
                stat_key: fingerprint.stat_key,
                value: value.clone(),
            },
        );
        Some(value)
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
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn temp_dir(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fabushi-stat-parse-cache-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn old_enough_clock(path: &Path) -> Arc<dyn Fn() -> u64 + Send + Sync> {
        let modified = fs::metadata(path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Arc::new(move || modified.saturating_add(RACY_MTIME_TICK_WINDOW_MS + 10))
    }

    #[test]
    fn stable_fingerprint_reuses_parsed_value() {
        let root = temp_dir("hit");
        let file = root.join("workflow.md");
        fs::write(&file, "one").unwrap();
        let parses = AtomicUsize::new(0);
        let mut cache = StatKeyedParseCache::with_clock(8, old_enough_clock(&file));

        let first = cache
            .read(&[&file], || {
                parses.fetch_add(1, Ordering::SeqCst);
                fs::read_to_string(&file).unwrap()
            })
            .unwrap();
        let second = cache
            .read(&[&file], || {
                parses.fetch_add(1, Ordering::SeqCst);
                "unexpected".to_owned()
            })
            .unwrap();

        assert_eq!(first, "one");
        assert_eq!(second, "one");
        assert_eq!(parses.load(Ordering::SeqCst), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn changed_size_or_mtime_reparses_and_missing_path_evicts() {
        let root = temp_dir("edit");
        let file = root.join("workflow.md");
        fs::write(&file, "one").unwrap();
        let clock = old_enough_clock(&file);
        let mut cache = StatKeyedParseCache::with_clock(8, clock);
        assert_eq!(
            cache.read(&[&file], || fs::read_to_string(&file).unwrap()),
            Some("one".to_owned())
        );
        std::thread::sleep(Duration::from_millis(2));
        let mut handle = fs::OpenOptions::new().append(true).open(&file).unwrap();
        handle.write_all(b"-two").unwrap();
        handle.sync_all().unwrap();

        assert_eq!(
            cache.read(&[&file], || fs::read_to_string(&file).unwrap()),
            Some("one-two".to_owned())
        );
        fs::remove_file(&file).unwrap();
        assert_eq!(cache.read(&[&file], || "must-not-run".to_owned()), None);
        assert!(cache.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recent_mtime_is_parsed_but_not_cached() {
        let root = temp_dir("racy");
        let file = root.join("workflow.md");
        fs::write(&file, "one").unwrap();
        let modified = fs::metadata(&file)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut cache = StatKeyedParseCache::with_clock(8, Arc::new(move || modified + 1));
        let parses = AtomicUsize::new(0);

        for _ in 0..2 {
            cache
                .read(&[&file], || {
                    parses.fetch_add(1, Ordering::SeqCst);
                    "parsed".to_owned()
                })
                .unwrap();
        }
        assert_eq!(parses.load(Ordering::SeqCst), 2);
        assert!(cache.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capacity_evicts_oldest_insertion_without_refreshing_hits() {
        let root = temp_dir("capacity");
        let a = root.join("a");
        let b = root.join("b");
        let c = root.join("c");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        fs::write(&c, "c").unwrap();
        let newest = [&a, &b, &c]
            .iter()
            .map(|path| {
                fs::metadata(path)
                    .unwrap()
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64
            })
            .max()
            .unwrap();
        let mut cache = StatKeyedParseCache::with_clock(
            2,
            Arc::new(move || newest + RACY_MTIME_TICK_WINDOW_MS + 10),
        );

        cache.read(&[&a], || "a".to_owned());
        cache.read(&[&b], || "b".to_owned());
        assert_eq!(cache.read(&[&a], || "bad".to_owned()).as_deref(), Some("a"));
        cache.read(&[&c], || "c".to_owned());

        let reparsed = cache
            .read(&[&a], || "a-reparsed".to_owned())
            .unwrap();
        assert_eq!(reparsed, "a-reparsed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn racy_window_matches_reference_boundary() {
        let fingerprint = StatFingerprint {
            stat_key: "1:2:3".into(),
            newest_mtime_ms: 10_000,
        };
        assert!(mtime_tick_could_still_hide_an_edit(&fingerprint, 11_999));
        assert!(!mtime_tick_could_still_hide_an_edit(&fingerprint, 12_000));
    }
}
