use std::path::{Path, PathBuf};
use uuid::Uuid;

/// RAII temporary workspace under `std::env::temp_dir()` for isolated test DBs,
/// Tantivy indices, and CAS media directories, cleaned up on drop.
pub struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    /// Creates a unique directory under `std::env::temp_dir()`.
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        let dir = std::env::temp_dir().join(format!("gpui_tg_{prefix}_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self { path: dir })
    }

    /// Returns the root path of the temporary directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Joins a child relative path inside this temporary directory.
    pub fn child(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.path.join(rel)
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
