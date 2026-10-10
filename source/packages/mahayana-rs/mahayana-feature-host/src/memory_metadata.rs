use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use mahayana_host_protocol::{MemoryKind, MemoryRecord};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryOrigin {
    Explicit,
    Synthesis,
    Legacy,
}

const METADATA_DIRNAME: &str = ".dreaming";
const EXPLICIT_DIRNAME: &str = "explicit";
const SYNTHESIZED_DIRNAME: &str = "synthesized";
const TOMBSTONE_DIRNAME: &str = "tombstones";
const REFRESH_FILENAME: &str = "next-refresh-at";

fn metadata_root(memory_dir: &Path) -> PathBuf {
    memory_dir.join(METADATA_DIRNAME)
}

fn origin_path(memory_dir: &Path, content: &str, origin: MemoryOrigin) -> PathBuf {
    let dir = match origin {
        MemoryOrigin::Explicit => EXPLICIT_DIRNAME,
        MemoryOrigin::Synthesis | MemoryOrigin::Legacy => SYNTHESIZED_DIRNAME,
    };
    metadata_root(memory_dir)
        .join(dir)
        .join(format!("{}.memory", crate::implementation::memory_id_for(content)))
}

fn tombstone_path(memory_dir: &Path, content: &str) -> PathBuf {
    metadata_root(memory_dir)
        .join(TOMBSTONE_DIRNAME)
        .join(format!("{}.deleted", crate::implementation::memory_id_for(content)))
}

fn atomic_write(path: &Path, body: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = PathBuf::from(format!("{}.{}.tmp", path.display(), std::process::id()));
    fs::write(&temporary, body)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(first) if path.exists() => {
            fs::remove_file(path)?;
            fs::rename(&temporary, path).map_err(|_| first)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn remove_if_exists(path: PathBuf) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn clear_origins(memory_dir: &Path, content: &str) -> io::Result<()> {
    remove_if_exists(origin_path(memory_dir, content, MemoryOrigin::Explicit))?;
    remove_if_exists(origin_path(memory_dir, content, MemoryOrigin::Synthesis))
}

pub(crate) fn mark_origin(
    memory_dir: &Path,
    content: &str,
    origin: MemoryOrigin,
) -> io::Result<()> {
    if origin == MemoryOrigin::Legacy {
        return Ok(());
    }
    atomic_write(&origin_path(memory_dir, content, origin), b"")
}

pub(crate) fn memory_origin(memory_dir: &Path, content: &str) -> MemoryOrigin {
    if origin_path(memory_dir, content, MemoryOrigin::Explicit).is_file() {
        MemoryOrigin::Explicit
    } else if origin_path(memory_dir, content, MemoryOrigin::Synthesis).is_file() {
        MemoryOrigin::Synthesis
    } else {
        MemoryOrigin::Legacy
    }
}

pub(crate) fn mark_tombstone(memory_dir: &Path, content: &str) -> io::Result<()> {
    atomic_write(&tombstone_path(memory_dir, content), b"")
}

pub(crate) fn clear_tombstone(memory_dir: &Path, content: &str) -> io::Result<()> {
    remove_if_exists(tombstone_path(memory_dir, content))
}

pub(crate) fn is_tombstoned(memory_dir: &Path, content: &str) -> bool {
    tombstone_path(memory_dir, content).is_file()
}

pub(crate) fn mark_explicit(memory_dir: &Path, content: &str) -> io::Result<()> {
    clear_tombstone(memory_dir, content)?;
    clear_origins(memory_dir, content)?;
    mark_origin(memory_dir, content, MemoryOrigin::Explicit)
}

pub(crate) fn mark_explicit_removal(memory_dir: &Path, content: &str) -> io::Result<()> {
    clear_origins(memory_dir, content)?;
    mark_tombstone(memory_dir, content)
}

pub(crate) fn refresh_path(memory_dir: &Path) -> PathBuf {
    metadata_root(memory_dir).join(REFRESH_FILENAME)
}

const SYNTHESIS_INPUT_LIMIT: usize = 512;
const SYNTHESIS_REFRESH_INTERVAL_MS: i64 = 86_400_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SynthesisMemory {
    pub id: String,
    pub content: String,
    pub created_at: i64,
    pub kind: MemoryKind,
    pub origin: MemoryOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SynthesisSnapshot {
    pub fingerprint: String,
    pub memories: Vec<SynthesisMemory>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SynthesisChange {
    Create { content: String, kind: MemoryKind },
    Update { id: String, content: String, kind: MemoryKind },
    Remove { id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SynthesisApplyResult {
    Committed,
    Stale,
    Invalid,
}

fn memory_source_files(memory_dir: &Path) -> Vec<PathBuf> {
    let mut files = vec![memory_dir.join("profile.md")];
    let mut logs = fs::read_dir(memory_dir.join("log"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    logs.sort();
    files.extend(logs);
    files
}

fn fingerprint(memory_dir: &Path) -> String {
    let mut hasher = Sha256::new();
    for path in memory_source_files(memory_dir) {
        let raw = fs::read(&path).unwrap_or_default();
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(raw);
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub(crate) fn prepare_synthesis_snapshot(
    memory_dir: &Path,
) -> Result<SynthesisSnapshot, String> {
    let fingerprint = fingerprint(memory_dir);
    let mut memories = crate::implementation::list_memories(memory_dir, usize::MAX)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|memory| SynthesisMemory {
            origin: memory_origin(memory_dir, &memory.content),
            id: memory.id,
            content: memory.content,
            created_at: memory.created_at,
            kind: memory.kind,
        })
        .collect::<Vec<_>>();
    memories.sort_by(|left, right| {
        usize::from(right.origin == MemoryOrigin::Explicit)
            .cmp(&usize::from(left.origin == MemoryOrigin::Explicit))
            .then_with(|| {
                usize::from(right.kind == MemoryKind::Profile)
                    .cmp(&usize::from(left.kind == MemoryKind::Profile))
            })
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    memories.truncate(SYNTHESIS_INPUT_LIMIT);
    Ok(SynthesisSnapshot {
        fingerprint,
        memories,
    })
}

fn add_synthesized(
    memory_dir: &Path,
    content: &str,
    created_at: i64,
    kind: MemoryKind,
) -> Result<(), String> {
    if is_tombstoned(memory_dir, content) {
        return Ok(());
    }
    let added = crate::implementation::add_memory(memory_dir, content, created_at, kind)
        .map_err(|error| error.to_string())?;
    if added.is_some() {
        clear_origins(memory_dir, content).map_err(|error| error.to_string())?;
        mark_origin(memory_dir, content, MemoryOrigin::Synthesis)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) fn apply_synthesis(
    memory_dir: &Path,
    snapshot: &SynthesisSnapshot,
    changes: &[SynthesisChange],
    now_ms: i64,
) -> Result<SynthesisApplyResult, String> {
    if fingerprint(memory_dir) != snapshot.fingerprint {
        return Ok(SynthesisApplyResult::Stale);
    }

    let allowed = snapshot
        .memories
        .iter()
        .map(|memory| memory.id.clone())
        .collect::<HashSet<_>>();
    let current = crate::implementation::list_memories(memory_dir, usize::MAX)
        .map_err(|error| error.to_string())?;
    let mut changed = HashSet::new();

    for change in changes {
        match change {
            SynthesisChange::Create { content, kind } => {
                if content.trim().is_empty() {
                    return Ok(SynthesisApplyResult::Invalid);
                }
                add_synthesized(memory_dir, content, now_ms, *kind)?;
            }
            SynthesisChange::Update { id, content, kind } => {
                if !allowed.contains(id) || !changed.insert(id.clone()) || content.trim().is_empty() {
                    return Ok(SynthesisApplyResult::Invalid);
                }
                let Some(existing) = current.iter().find(|memory| &memory.id == id) else {
                    return Ok(SynthesisApplyResult::Invalid);
                };
                if memory_origin(memory_dir, &existing.content) == MemoryOrigin::Explicit {
                    return Ok(SynthesisApplyResult::Invalid);
                }
                crate::implementation::remove_memory(memory_dir, id)
                    .map_err(|error| error.to_string())?;
                clear_origins(memory_dir, &existing.content).map_err(|error| error.to_string())?;
                add_synthesized(memory_dir, content, now_ms, *kind)?;
            }
            SynthesisChange::Remove { id } => {
                if !allowed.contains(id) || !changed.insert(id.clone()) {
                    return Ok(SynthesisApplyResult::Invalid);
                }
                let Some(existing) = current.iter().find(|memory| &memory.id == id) else {
                    return Ok(SynthesisApplyResult::Invalid);
                };
                if memory_origin(memory_dir, &existing.content) == MemoryOrigin::Explicit {
                    return Ok(SynthesisApplyResult::Invalid);
                }
                crate::implementation::remove_memory(memory_dir, id)
                    .map_err(|error| error.to_string())?;
                clear_origins(memory_dir, &existing.content).map_err(|error| error.to_string())?;
            }
        }
    }

    mark_temporal_review(memory_dir, now_ms).map_err(|error| error.to_string())?;
    Ok(SynthesisApplyResult::Committed)
}

pub(crate) fn is_temporal_review_due(memory_dir: &Path, now_ms: i64) -> bool {
    fs::read_to_string(refresh_path(memory_dir))
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .is_none_or(|next| next <= now_ms)
}

pub(crate) fn mark_temporal_review(memory_dir: &Path, now_ms: i64) -> io::Result<()> {
    let next = now_ms.saturating_add(SYNTHESIS_REFRESH_INTERVAL_MS);
    atomic_write(&refresh_path(memory_dir), format!("{next}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-memory-metadata-{label}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos()
        ))
    }

    #[test]
    fn explicit_add_clears_tombstone_and_marks_origin() {
        let root = root("explicit");
        mark_tombstone(&root, "Prefers concise answers").expect("tombstone");
        assert!(is_tombstoned(&root, "Prefers concise answers"));
        mark_explicit(&root, "Prefers concise answers").expect("explicit");
        assert!(!is_tombstoned(&root, "Prefers concise answers"));
        assert_eq!(
            memory_origin(&root, "Prefers concise answers"),
            MemoryOrigin::Explicit
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_remove_tombstones_and_clears_origins() {
        let root = root("remove");
        mark_origin(&root, "Old plan", MemoryOrigin::Synthesis).expect("synthesis");
        assert_eq!(memory_origin(&root, "Old plan"), MemoryOrigin::Synthesis);
        mark_explicit_removal(&root, "Old plan").expect("remove");
        assert_eq!(memory_origin(&root, "Old plan"), MemoryOrigin::Legacy);
        assert!(is_tombstoned(&root, "Old plan"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn synthesis_snapshot_stale_fences_and_protects_explicit_memory() {
        let root = root("snapshot");
        let explicit = crate::implementation::add_memory(
            &root,
            "Never overwrite this",
            1_700_000_000_000,
            MemoryKind::Profile,
        )
        .expect("add")
        .expect("record");
        mark_explicit(&root, &explicit.content).expect("explicit metadata");
        let snapshot = prepare_synthesis_snapshot(&root).expect("snapshot");
        let result = apply_synthesis(
            &root,
            &snapshot,
            &[SynthesisChange::Update {
                id: explicit.id.clone(),
                content: "Changed".into(),
                kind: MemoryKind::Profile,
            }],
            1_710_000_000_000,
        )
        .expect("apply");
        assert_eq!(result, SynthesisApplyResult::Invalid);

        crate::implementation::add_memory(
            &root,
            "Concurrent change",
            1_720_000_000_000,
            MemoryKind::Log,
        )
        .expect("concurrent add");
        let stale = apply_synthesis(&root, &snapshot, &[], 1_730_000_000_000)
            .expect("stale apply");
        assert_eq!(stale, SynthesisApplyResult::Stale);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn synthesis_respects_tombstones_and_marks_synthesized_origin_and_refresh() {
        let root = root("synthesis");
        mark_tombstone(&root, "Do not resurrect").expect("tombstone");
        let snapshot = prepare_synthesis_snapshot(&root).expect("snapshot");
        let result = apply_synthesis(
            &root,
            &snapshot,
            &[
                SynthesisChange::Create {
                    content: "Do not resurrect".into(),
                    kind: MemoryKind::Log,
                },
                SynthesisChange::Create {
                    content: "New synthesized fact".into(),
                    kind: MemoryKind::Log,
                },
            ],
            1_740_000_000_000,
        )
        .expect("apply");
        assert_eq!(result, SynthesisApplyResult::Committed);
        let memories = crate::implementation::list_memories(&root, 100).expect("list");
        assert!(!memories.iter().any(|memory| memory.content == "Do not resurrect"));
        assert!(memories.iter().any(|memory| memory.content == "New synthesized fact"));
        assert_eq!(
            memory_origin(&root, "New synthesized fact"),
            MemoryOrigin::Synthesis
        );
        assert!(!is_temporal_review_due(&root, 1_740_000_000_000));
        assert!(is_temporal_review_due(
            &root,
            1_740_000_000_000 + SYNTHESIS_REFRESH_INTERVAL_MS
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn metadata_paths_match_desktop_dreaming_layout() {
        let root = root("layout");
        assert!(origin_path(&root, "fact", MemoryOrigin::Explicit)
            .to_string_lossy()
            .contains("/.dreaming/explicit/"));
        assert!(origin_path(&root, "fact", MemoryOrigin::Synthesis)
            .to_string_lossy()
            .contains("/.dreaming/synthesized/"));
        assert!(tombstone_path(&root, "fact")
            .to_string_lossy()
            .contains("/.dreaming/tombstones/"));
        assert!(refresh_path(&root)
            .to_string_lossy()
            .ends_with("/.dreaming/next-refresh-at"));
        let _ = fs::remove_dir_all(root);
    }
}
