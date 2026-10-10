use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
