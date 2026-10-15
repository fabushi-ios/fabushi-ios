use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const SYNC_DIR_NAME: &str = ".sync";
const JOURNAL_FILE_NAME: &str = "conflict-events.jsonl";
const PENDING_FILE_NAME: &str = "conflict-events.pending.jsonl";
const JOURNAL_ROTATE_BYTES: usize = 4 * 1024 * 1024;
const EVENT_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConflictJournalEvent {
    pub v: u8,
    pub event_id: String,
    pub journal_epoch: String,
    pub seq: u64,
    pub ts_ms: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_rel_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_abs_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PendingConflictEmit {
    kind: String,
    store_id: String,
    original_rel_path: String,
    original_abs_path: String,
    source: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn ensure_safe_identity(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || value.contains('/')
        || value.contains('\\')
        || value == "."
        || value == ".."
        || value.contains('\0')
    {
        return Err(format!("unsafe Agent Store conflict {label}"));
    }
    Ok(())
}

fn ensure_safe_rel_path(rel_path: &str) -> Result<(), String> {
    if rel_path == "<path-conflict>" {
        return Ok(());
    }
    if rel_path.is_empty() || rel_path.contains('\0') || Path::new(rel_path).is_absolute() {
        return Err("unsafe Agent Store conflict relative path".into());
    }
    for component in Path::new(rel_path).components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or_else(|| "Agent Store conflict path is not UTF-8".to_string())?;
                if value.is_empty()
                    || value
                        .as_bytes()
                        .iter()
                        .any(|byte| *byte < 32 || *byte == 127)
                {
                    return Err("unsafe Agent Store conflict path segment".into());
                }
            }
            _ => return Err("unsafe Agent Store conflict path component".into()),
        }
    }
    Ok(())
}

fn ensure_regular_or_missing(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("refusing symlinked Agent Store conflict path: {}", path.display()))
        }
        Ok(metadata) if !metadata.is_file() => {
            Err(format!("refusing non-file Agent Store conflict path: {}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn ensure_directory_not_symlink(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("refusing symlinked Agent Store conflict directory: {}", path.display()))
        }
        Ok(metadata) if !metadata.is_dir() => {
            Err(format!("refusing non-directory Agent Store conflict path: {}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|error| error.to_string())?;
            let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!("unsafe Agent Store conflict directory: {}", path.display()));
            }
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn sync_dir(account_root: &Path) -> Result<PathBuf, String> {
    ensure_directory_not_symlink(account_root)?;
    let sync = account_root.join(SYNC_DIR_NAME);
    ensure_directory_not_symlink(&sync)?;
    Ok(sync)
}

fn read_optional_text(path: &Path) -> Result<Option<String>, String> {
    ensure_regular_or_missing(path)?;
    match fs::read_to_string(path) {
        Ok(body) => Ok(Some(body)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn atomic_write_private(path: &Path, body: &[u8]) -> Result<(), String> {
    ensure_regular_or_missing(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Agent Store conflict path has no parent".to_string())?;
    ensure_directory_not_symlink(parent)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("conflict");
    let temp = parent.join(format!(".{file_name}.tmp-{}", Uuid::new_v4().simple()));
    let result = (|| -> Result<(), String> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options.open(&temp).map_err(|error| error.to_string())?;
        file.write_all(body).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        fs::rename(&temp, path).map_err(|error| error.to_string())?;
        let parent_dir = OpenOptions::new()
            .read(true)
            .open(parent)
            .map_err(|error| error.to_string())?;
        parent_dir.sync_all().map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn remove_optional(path: &Path) -> Result<(), String> {
    ensure_regular_or_missing(path)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn pending_key(emit: &PendingConflictEmit) -> String {
    format!(
        "{}\0{}\0{}",
        emit.store_id, emit.kind, emit.original_rel_path
    )
}

fn event_key(event: &ConflictJournalEvent) -> Option<String> {
    Some(format!(
        "{}\0{}\0{}",
        event.store_id.as_deref()?,
        event.kind,
        event.original_rel_path.as_deref()?
    ))
}

fn parse_pending(path: &Path) -> Result<Vec<PendingConflictEmit>, String> {
    let Some(body) = read_optional_text(path)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let emit: PendingConflictEmit = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if emit.kind != "write_conflict" {
            continue;
        }
        if ensure_safe_identity(&emit.store_id, "store identity").is_err()
            || ensure_safe_rel_path(&emit.original_rel_path).is_err()
        {
            continue;
        }
        let key = pending_key(&emit);
        if seen.insert(key) {
            out.push(emit);
        }
    }
    Ok(out)
}

fn persist_pending(path: &Path, pending: &[PendingConflictEmit]) -> Result<(), String> {
    if pending.is_empty() {
        return remove_optional(path);
    }
    let mut body = Vec::new();
    for emit in pending {
        serde_json::to_writer(&mut body, emit).map_err(|error| error.to_string())?;
        body.push(b'\n');
    }
    atomic_write_private(path, &body)
}

fn parse_events(path: &Path) -> Result<Vec<ConflictJournalEvent>, String> {
    let Some(body) = read_optional_text(path)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let event: ConflictJournalEvent = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if event.v == EVENT_SCHEMA_VERSION
            && matches!(
                event.kind.as_str(),
                "write_conflict"
                    | "create_conflict"
                    | "truncate_conflict_failed"
                    | "conflict_fallback_failed"
                    | "gap"
            )
        {
            out.push(event);
        }
    }
    Ok(out)
}

fn serialize_events(events: &[ConflictJournalEvent]) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    for event in events {
        serde_json::to_writer(&mut body, event).map_err(|error| error.to_string())?;
        body.push(b'\n');
    }
    Ok(body)
}

fn flush_pending(sync: &Path) -> Result<Vec<ConflictJournalEvent>, String> {
    let pending_path = sync.join(PENDING_FILE_NAME);
    let pending = parse_pending(&pending_path)?;
    if pending.is_empty() {
        remove_optional(&pending_path)?;
        return Ok(Vec::new());
    }

    let journal_path = sync.join(JOURNAL_FILE_NAME);
    let mut events = parse_events(&journal_path)?;
    let mut seen = events
        .iter()
        .filter_map(event_key)
        .collect::<HashSet<_>>();
    let epoch = events
        .last()
        .map(|event| event.journal_epoch.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let mut next_seq = events
        .iter()
        .map(|event| event.seq)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut appended = Vec::new();

    for emit in pending {
        let key = pending_key(&emit);
        if !seen.insert(key) {
            continue;
        }
        let event = ConflictJournalEvent {
            v: EVENT_SCHEMA_VERSION,
            event_id: Uuid::new_v4().to_string(),
            journal_epoch: epoch.clone(),
            seq: next_seq,
            ts_ms: now_ms(),
            kind: emit.kind,
            store_id: Some(emit.store_id),
            original_rel_path: Some(emit.original_rel_path),
            original_abs_path: Some(emit.original_abs_path),
            source: Some(emit.source),
            remote_only: None,
        };
        next_seq = next_seq.saturating_add(1);
        events.push(event.clone());
        appended.push(event);
    }

    let mut body = serialize_events(&events)?;
    if body.len() > JOURNAL_ROTATE_BYTES {
        let rotated_epoch = Uuid::new_v4().to_string();
        let mut rotated = Vec::with_capacity(appended.len() + 1);
        rotated.push(ConflictJournalEvent {
            v: EVENT_SCHEMA_VERSION,
            event_id: Uuid::new_v4().to_string(),
            journal_epoch: rotated_epoch.clone(),
            seq: 1,
            ts_ms: now_ms(),
            kind: "gap".into(),
            store_id: None,
            original_rel_path: None,
            original_abs_path: None,
            source: None,
            remote_only: None,
        });
        for (index, event) in appended.iter_mut().enumerate() {
            event.journal_epoch = rotated_epoch.clone();
            event.seq = index as u64 + 2;
            rotated.push(event.clone());
        }
        events = rotated;
        body = serialize_events(&events)?;
    }

    atomic_write_private(&journal_path, &body)?;
    // If cleanup fails, the next recovery pass replays the sidecar. Journal
    // dedup makes that replay idempotent.
    remove_optional(&pending_path)?;
    Ok(appended)
}

pub fn record_merge_conflicts(
    account_root: &Path,
    store_id: &str,
    rel_paths: &[String],
) -> Result<Vec<ConflictJournalEvent>, String> {
    ensure_safe_identity(store_id, "store identity")?;
    let sync = sync_dir(account_root)?;
    let pending_path = sync.join(PENDING_FILE_NAME);
    let mut pending = parse_pending(&pending_path)?;
    let mut seen = pending.iter().map(pending_key).collect::<HashSet<_>>();
    let target_root = account_root.join(store_id);

    let unique_paths = rel_paths.iter().cloned().collect::<BTreeSet<_>>();
    for rel_path in unique_paths {
        ensure_safe_rel_path(&rel_path)?;
        let original_abs_path = if rel_path == "<path-conflict>" {
            target_root.clone()
        } else {
            target_root.join(&rel_path)
        };
        let emit = PendingConflictEmit {
            kind: "write_conflict".into(),
            store_id: store_id.to_string(),
            original_rel_path: rel_path,
            original_abs_path: original_abs_path.to_string_lossy().into_owned(),
            source: "local_sync".into(),
        };
        if seen.insert(pending_key(&emit)) {
            pending.push(emit);
        }
    }

    // Pending is committed first so a crash or journal write failure cannot
    // erase an observed conflict.
    persist_pending(&pending_path, &pending)?;
    flush_pending(&sync)
}

pub fn recover_pending(account_root: &Path) -> Result<Vec<ConflictJournalEvent>, String> {
    let sync = sync_dir(account_root)?;
    flush_pending(&sync)
}

pub fn read_conflict_events(account_root: &Path) -> Result<Vec<ConflictJournalEvent>, String> {
    let sync = sync_dir(account_root)?;
    parse_events(&sync.join(JOURNAL_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-agent-store-conflict-{label}-{}",
            Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn durable_journal_dedups_restart_and_keeps_account_scope() {
        let account_a = root("a");
        let account_b = root("b");
        let paths = vec!["memory/state.json".to_string(), "notes/today.md".to_string()];

        let first = record_merge_conflicts(&account_a, "agent-1", &paths).expect("record");
        assert_eq!(first.len(), 2);
        let repeated = record_merge_conflicts(&account_a, "agent-1", &paths).expect("dedup");
        assert!(repeated.is_empty());
        let other = record_merge_conflicts(
            &account_b,
            "agent-1",
            &["memory/state.json".to_string()],
        )
        .expect("other account");
        assert_eq!(other.len(), 1);

        let events_a = read_conflict_events(&account_a).expect("read account a");
        let events_b = read_conflict_events(&account_b).expect("read account b");
        assert_eq!(events_a.len(), 2);
        assert_eq!(events_b.len(), 1);
        assert!(events_a.iter().all(|event| event.source.as_deref() == Some("local_sync")));

        let _ = fs::remove_dir_all(account_a);
        let _ = fs::remove_dir_all(account_b);
    }

    #[test]
    fn pending_sidecar_recovers_after_journal_failure() {
        let account = root("pending");
        let sync = sync_dir(&account).expect("sync dir");
        let journal = sync.join(JOURNAL_FILE_NAME);
        fs::create_dir(&journal).expect("block journal with directory");

        let error = record_merge_conflicts(
            &account,
            "agent-1",
            &["memory/state.json".to_string()],
        )
        .expect_err("journal failure must surface");
        assert!(error.contains("non-file"));
        assert!(sync.join(PENDING_FILE_NAME).is_file());

        fs::remove_dir(&journal).expect("remove blocker");
        let recovered = recover_pending(&account).expect("recover pending");
        assert_eq!(recovered.len(), 1);
        assert!(!sync.join(PENDING_FILE_NAME).exists());
        assert_eq!(read_conflict_events(&account).expect("events").len(), 1);

        let _ = fs::remove_dir_all(account);
    }

    #[cfg(unix)]
    #[test]
    fn sync_directory_symlink_is_refused() {
        let account = root("symlink");
        fs::create_dir_all(&account).unwrap();
        let outside = root("outside");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, account.join(SYNC_DIR_NAME)).unwrap();

        let error = record_merge_conflicts(
            &account,
            "agent-1",
            &["memory/state.json".to_string()],
        )
        .expect_err("symlinked sync directory must fail closed");
        assert!(error.contains("symlinked"));

        let _ = fs::remove_file(account.join(SYNC_DIR_NAME));
        let _ = fs::remove_dir_all(account);
        let _ = fs::remove_dir_all(outside);
    }
}
