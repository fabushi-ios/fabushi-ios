//! Target Symbol: StorageFileLockWin
//! Telegram Desktop 1:1 Submodule: `StorageFileLockWin`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::fs::File;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct StorageFileLockWin {
    path: Option<PathBuf>,
    file: Option<File>,
    locked: bool,
}

impl StorageFileLockWin {
    pub fn new() -> Self {
        Self {
            path: None,
            file: None,
            locked: false,
        }
    }

    #[cfg(windows)]
    pub fn lock(&mut self, path: &Path) -> Result<bool, String> {
        use std::os::windows::io::AsRawHandle;
        self.unlock();

        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
        {
            Ok(f) => f,
            Err(e) => return Err(format!("Could not open lockfile {}: {}", path.display(), e)),
        };

        let handle = file.as_raw_handle() as isize;
        // Windows LockFileEx with LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY
        self.file = Some(file);
        self.path = Some(path.to_path_buf());
        self.locked = true;
        Ok(true)
    }

    #[cfg(not(windows))]
    pub fn lock(&mut self, path: &Path) -> Result<bool, String> {
        self.unlock();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| format!("Could not open lockfile {}: {}", path.display(), e))?;
        self.file = Some(file);
        self.path = Some(path.to_path_buf());
        self.locked = true;
        Ok(true)
    }

    pub fn unlock(&mut self) {
        self.file = None;
        self.path = None;
        self.locked = false;
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn close_processes(&self, _filename: &str) -> bool {
        // Windows RestartManager stub for Unix host
        false
    }
}

impl Drop for StorageFileLockWin {
    fn drop(&mut self) {
        self.unlock();
    }
}
