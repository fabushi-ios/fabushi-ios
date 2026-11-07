use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use mahayana_host_runtime::extensions::box_store_sync::box_object_store::BoxObjectStore;
use mahayana_host_runtime::extensions::box_store_sync::box_store_download::{
    BoxStoreByteBudget, BoxStoreDownloadOptions, BoxStoreDownloadTrace, download_manifest,
    symlink_target_stays_within_root,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_manifest_format::BoxStoreManifestEntry;
use mahayana_host_runtime::extensions::box_store_sync::box_store_pack::{
    BOX_STORE_PACK_INDEX_KEY, BOX_STORE_PACKS_PREFIX, PACK_INDEX_VERSION, PackEntry, PackIndex,
    PackSource, build_pack_file, serialize_pack_index, sha256_hex,
};

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-box-store-download-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[derive(Default)]
struct CountingStore {
    objects: Mutex<HashMap<String, Vec<u8>>>,
    gets: Mutex<Vec<String>>,
    file_gets: Mutex<Vec<String>>,
    prefetches: Mutex<Vec<Vec<String>>>,
    fail_prefetch: bool,
}

impl CountingStore {
    fn with_objects(objects: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        Self {
            objects: Mutex::new(objects.into_iter().collect()),
            ..Self::default()
        }
    }

    fn with_prefetch_failure(objects: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        Self {
            objects: Mutex::new(objects.into_iter().collect()),
            fail_prefetch: true,
            ..Self::default()
        }
    }

    fn get_count(&self, key: &str) -> usize {
        self.gets
            .lock()
            .expect("gets")
            .iter()
            .filter(|candidate| candidate.as_str() == key)
            .count()
    }

    fn file_get_count(&self, key: &str) -> usize {
        self.file_gets
            .lock()
            .expect("file gets")
            .iter()
            .filter(|candidate| candidate.as_str() == key)
            .count()
    }

    fn get_order(&self) -> Vec<String> {
        self.gets.lock().expect("gets").clone()
    }
}

impl BoxObjectStore for CountingStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        self.gets.lock().expect("gets").push(key.to_string());
        Ok(self.objects.lock().expect("objects").get(key).cloned())
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        self.objects
            .lock()
            .expect("objects")
            .insert(key.to_string(), bytes.to_vec());
        Ok(())
    }

    fn get_to_file(
        &self,
        key: &str,
        dest_path: &Path,
        max_bytes: Option<u64>,
    ) -> Result<Option<u64>, String> {
        self.file_gets
            .lock()
            .expect("file gets")
            .push(key.to_string());
        let Some(bytes) = self.objects.lock().expect("objects").get(key).cloned() else {
            return Ok(None);
        };
        if max_bytes.is_some_and(|max| bytes.len() as u64 > max) {
            return Err("object exceeds maximum restore bytes".into());
        }
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(dest_path, &bytes).map_err(|error| error.to_string())?;
        Ok(Some(bytes.len() as u64))
    }

    fn prefetch_reads(&self, keys: &[String]) -> Result<(), String> {
        self.prefetches
            .lock()
            .expect("prefetches")
            .push(keys.to_vec());
        if self.fail_prefetch {
            Err("synthetic prefetch failure".into())
        } else {
            Ok(())
        }
    }

    fn put_from_file(&self, key: &str, src_path: &Path) -> Result<(), String> {
        let bytes = fs::read(src_path).map_err(|error| error.to_string())?;
        self.put(key, &bytes)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        let mut keys = self
            .objects
            .lock()
            .expect("objects")
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        Ok(keys)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.objects.lock().expect("objects").remove(key);
        Ok(())
    }
}

fn file_entry(payload: &[u8], mode: u32) -> (String, BoxStoreManifestEntry) {
    let sha = format!("{:x}", Sha256::digest(payload));
    (
        sha.clone(),
        BoxStoreManifestEntry::File {
            sha,
            size: payload.len() as u64,
            mode,
        },
    )
}

#[test]
fn duplicate_small_blob_is_fetched_once_and_local_hits_are_reused() {
    let root = temp_root("small-dedupe");
    let payload = b"shared-small-payload";
    let (sha, entry) = file_entry(payload, 0o600);
    let key = format!("blobs/{sha}");
    let store = CountingStore::with_objects([(key.clone(), payload.to_vec())]);
    let manifest = BTreeMap::from([
        ("home/box/a/store.db".to_string(), entry.clone()),
        ("home/box/b/store.db".to_string(), entry),
    ]);

    let first = download_manifest(
        &store,
        &root,
        &manifest,
        BoxStoreDownloadOptions::default(),
    );
    assert!(first.failures.is_empty(), "{:?}", first.failures);
    assert_eq!((first.files, first.verified), (2, 2));
    assert_eq!(first.bytes, (payload.len() * 2) as u64);
    assert_eq!(store.get_count(&key), 1);
    assert_eq!(fs::read(root.join("home/box/a/store.db")).unwrap(), payload);
    assert_eq!(fs::read(root.join("home/box/b/store.db")).unwrap(), payload);

    let local_only = CountingStore::default();
    let second = download_manifest(
        &local_only,
        &root,
        &manifest,
        BoxStoreDownloadOptions::default(),
    );
    assert!(second.failures.is_empty(), "{:?}", second.failures);
    assert_eq!((second.files, second.verified), (2, 2));
    assert_eq!(local_only.get_count(&key), 0);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn critical_files_precede_bulk_and_prefetch_failure_falls_back_to_blob_reads() {
    let root = temp_root("phase-order");
    let critical = b"critical";
    let bulk = b"bulk";
    let (critical_sha, critical_entry) = file_entry(critical, 0o600);
    let (bulk_sha, bulk_entry) = file_entry(bulk, 0o600);
    let critical_key = format!("blobs/{critical_sha}");
    let bulk_key = format!("blobs/{bulk_sha}");
    let store = CountingStore::with_prefetch_failure([
        (critical_key.clone(), critical.to_vec()),
        (bulk_key.clone(), bulk.to_vec()),
    ]);
    let manifest = BTreeMap::from([
        (
            "home/box/sand-data/agents/a/store.db".to_string(),
            critical_entry,
        ),
        ("home/box/workspace/note.txt".to_string(), bulk_entry),
    ]);
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let log_sink = Arc::clone(&logs);

    let summary = download_manifest(
        &store,
        &root,
        &manifest,
        BoxStoreDownloadOptions {
            on_log: Some(Arc::new(move |message| {
                log_sink.lock().expect("logs").push(message);
            })),
            ..BoxStoreDownloadOptions::default()
        },
    );

    assert!(summary.failures.is_empty(), "{:?}", summary.failures);
    let order = store.get_order();
    let critical_pos = order
        .iter()
        .position(|key| key == &critical_key)
        .expect("critical blob read");
    let bulk_pos = order
        .iter()
        .position(|key| key == &bulk_key)
        .expect("bulk blob read");
    assert!(critical_pos < bulk_pos, "read order: {order:?}");
    assert!(
        logs.lock()
            .expect("logs")
            .iter()
            .any(|line| line.contains("falling back to per-blob presign"))
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn duplicate_large_blob_streams_once_then_copies_from_the_verified_local_file() {
    let root = temp_root("large-dedupe");
    let payload = vec![b'x'; 128 * 1024];
    let (sha, entry) = file_entry(&payload, 0o600);
    let key = format!("blobs/{sha}");
    let store = CountingStore::with_objects([(key.clone(), payload.clone())]);
    let manifest = BTreeMap::from([
        ("home/box/workspace/one.bin".to_string(), entry.clone()),
        ("home/box/workspace/two.bin".to_string(), entry),
    ]);

    let summary = download_manifest(
        &store,
        &root,
        &manifest,
        BoxStoreDownloadOptions {
            large_object_threshold: 1,
            ..BoxStoreDownloadOptions::default()
        },
    );

    assert!(summary.failures.is_empty(), "{:?}", summary.failures);
    assert_eq!((summary.files, summary.verified), (2, 2));
    assert_eq!(store.file_get_count(&key), 1);
    assert_eq!(fs::read(root.join("home/box/workspace/one.bin")).unwrap(), payload);
    assert_eq!(
        fs::read(root.join("home/box/workspace/two.bin")).unwrap(),
        fs::read(root.join("home/box/workspace/one.bin")).unwrap()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn byte_budget_admits_an_oversized_item_when_idle() {
    let budget = BoxStoreByteBudget::new(10);
    assert!(budget.try_acquire(20));
    assert_eq!(budget.in_flight_bytes(), 20);
    assert!(!budget.try_acquire(1));
    budget.release(20);
    assert!(budget.try_acquire(10));
    budget.release(10);
}

#[test]
fn pack_restore_reports_each_restored_path_and_skips_loose_blobs() {
    let root = temp_root("pack-progress");
    let source_root = root.join("pack-sources");
    let target_root = root.join("target");
    fs::create_dir_all(&source_root).expect("create pack source root");
    fs::create_dir_all(&target_root).expect("create target root");

    let mut sources = Vec::new();
    let mut manifest = BTreeMap::new();
    let mut expected = Vec::new();
    for index in 0..16_u64 {
        let rel_path = format!("workspace/pack-{index}.txt");
        let bytes = format!("pack-progress-{index}\n").into_bytes();
        let sha = sha256_hex(&bytes);
        let source = source_root.join(format!("source-{index}"));
        fs::write(&source, &bytes).expect("write pack source");
        sources.push(PackSource {
            abs_path: source,
            sha: sha.clone(),
            size: bytes.len() as u64,
            vmtime: index + 1,
        });
        manifest.insert(
            rel_path.clone(),
            BoxStoreManifestEntry::File {
                sha,
                size: bytes.len() as u64,
                mode: 0o600,
            },
        );
        expected.push((rel_path, bytes));
    }

    let pack_path = root.join("pack.gz");
    let built = build_pack_file(&pack_path, &sources, || false)
        .expect("build pack")
        .expect("pack build not aborted");
    assert_eq!(built.members.len(), 16);
    let pack_bytes = fs::read(&pack_path).expect("read pack");
    let pack_id = sha256_hex(&pack_bytes);
    let pack_key = format!("{BOX_STORE_PACKS_PREFIX}/{pack_id}");
    let index = PackIndex {
        version: PACK_INDEX_VERSION,
        max_vmtime: 16,
        packs: vec![PackEntry {
            id: pack_id,
            bytes: pack_bytes.len() as u64,
            members: built.members,
        }],
    };
    let store = CountingStore::with_objects([
        (
            BOX_STORE_PACK_INDEX_KEY.to_string(),
            serialize_pack_index(&index)
                .expect("serialize pack index")
                .into_bytes(),
        ),
        (pack_key.clone(), pack_bytes),
    ]);
    let progress = Arc::new(Mutex::new(Vec::<usize>::new()));
    let progress_sink = Arc::clone(&progress);

    let summary = download_manifest(
        &store,
        &target_root,
        &manifest,
        BoxStoreDownloadOptions {
            on_progress: Some(Arc::new(move |snapshot| {
                progress_sink.lock().expect("progress").push(snapshot.files);
            })),
            ..BoxStoreDownloadOptions::default()
        },
    );

    assert!(summary.failures.is_empty(), "{:?}", summary.failures);
    assert_eq!((summary.files, summary.verified), (16, 16));
    assert_eq!(store.file_get_count(&pack_key), 1);
    let mut progress_files = progress.lock().expect("progress").clone();
    progress_files.sort_unstable();
    assert_eq!(progress_files, (1_usize..=16).collect::<Vec<_>>());
    for (rel_path, bytes) in expected {
        assert_eq!(
            fs::read(target_root.join(rel_path)).expect("read restored pack member"),
            bytes
        );
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn all_local_pack_members_skip_pack_download_and_are_counted_by_bulk_phase() {
    let root = temp_root("pack-local-skip");
    let target_root = root.join("target");
    fs::create_dir_all(&target_root).expect("create target root");

    let mut manifest = BTreeMap::new();
    let mut members = Vec::new();
    for index in 0..16_u64 {
        let rel_path = format!("workspace/local-{index}.txt");
        let bytes = format!("already-local-{index}\n").into_bytes();
        let sha = sha256_hex(&bytes);
        let destination = target_root.join(&rel_path);
        fs::create_dir_all(destination.parent().expect("destination parent")).expect("create parent");
        fs::write(&destination, &bytes).expect("write local file");
        manifest.insert(
            rel_path,
            BoxStoreManifestEntry::File {
                sha: sha.clone(),
                size: bytes.len() as u64,
                mode: 0o600,
            },
        );
        members.push(mahayana_host_runtime::extensions::box_store_sync::box_store_pack::PackMember {
            sha,
            size: bytes.len() as u64,
            offset: index,
            clen: 1,
            vmtime: index + 1,
        });
    }
    let pack_key = format!("{BOX_STORE_PACKS_PREFIX}/unused-local-pack");
    let index = PackIndex {
        version: PACK_INDEX_VERSION,
        max_vmtime: 16,
        packs: vec![PackEntry {
            id: "unused-local-pack".into(),
            bytes: 16,
            members,
        }],
    };
    let store = CountingStore::with_objects([(
        BOX_STORE_PACK_INDEX_KEY.to_string(),
        serialize_pack_index(&index)
            .expect("serialize pack index")
            .into_bytes(),
    )]);

    let summary = download_manifest(
        &store,
        &target_root,
        &manifest,
        BoxStoreDownloadOptions::default(),
    );

    assert!(summary.failures.is_empty(), "{:?}", summary.failures);
    assert_eq!((summary.files, summary.verified), (16, 16));
    assert_eq!(store.file_get_count(&pack_key), 0);

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn symlink_restore_reports_all_frozen_steps_and_allows_absolute_targets_inside_root() {
    let root = temp_root("symlink-trace");
    fs::create_dir_all(root.join("home/box/link")).unwrap();
    let inside_absolute = root.join("home/box/inside");
    assert!(symlink_target_stays_within_root(
        &root,
        &root.join("home/box/link"),
        inside_absolute.to_str().unwrap(),
    ));
    assert!(!symlink_target_stays_within_root(
        &root,
        &root.join("home/box/link"),
        "/etc/passwd",
    ));

    let manifest = BTreeMap::from([(
        "home/box/link".to_string(),
        BoxStoreManifestEntry::Symlink {
            target: "../target".into(),
        },
    )]);
    let traces = Arc::new(Mutex::new(Vec::<BoxStoreDownloadTrace>::new()));
    let trace_sink = Arc::clone(&traces);

    let summary = download_manifest(
        &CountingStore::default(),
        &root,
        &manifest,
        BoxStoreDownloadOptions {
            download_concurrency: 1,
            on_trace: Some(Arc::new(move |trace| {
                trace_sink.lock().expect("traces").push(trace);
            })),
            ..BoxStoreDownloadOptions::default()
        },
    );
    assert!(summary.failures.is_empty(), "{:?}", summary.failures);
    assert_eq!((summary.files, summary.verified), (1, 1));
    assert_eq!(fs::read_link(root.join("home/box/link")).unwrap(), PathBuf::from("../target"));

    let traces = traces.lock().expect("traces");
    let started_steps = traces
        .iter()
        .filter(|trace| trace.event == "symlink-step-started")
        .filter_map(|trace| trace.symlink_step.clone())
        .collect::<Vec<_>>();
    for step in [
        "prepare-parent",
        "match-existing",
        "create-temp",
        "inspect-destination",
        "remove-directory",
        "rename-temp",
        "apply-owner",
        "cleanup-temp",
        "verify-target",
    ] {
        assert!(
            started_steps.iter().any(|candidate| candidate == step),
            "missing symlink trace step {step}: {started_steps:?}"
        );
    }
    let finished = traces
        .iter()
        .find(|trace| trace.event == "symlink-entry-finished")
        .expect("symlink entry finished trace");
    assert_eq!(finished.symlinks_started, 1);
    assert_eq!(finished.symlinks_completed, 1);
    assert_eq!(finished.symlinks_in_flight, 0);
    assert_eq!(finished.symlink_outcome.as_deref(), Some("restored"));
    for step in [
        "prepare-parent",
        "match-existing",
        "create-temp",
        "inspect-destination",
        "remove-directory",
        "rename-temp",
        "apply-owner",
        "verify-target",
        "cleanup-temp",
    ] {
        assert_eq!(finished.active_symlink_steps.get(step), Some(&0));
    }

    let _ = fs::remove_dir_all(root);
}
