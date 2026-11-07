//! Target Symbol: StorageFileLock
//! Telegram Desktop 1:1 Submodule: `StorageFileLock`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::path::{Path, PathBuf};
use super::storage_file_lock_posix::StorageFileLockPosix;
use super::storage_file_lock_win::StorageFileLockWin;

#[derive(Debug, Default)]
pub struct FileLock {
    path: Option<PathBuf>,
    posix_lock: StorageFileLockPosix,
    #[allow(dead_code)]
    win_lock: StorageFileLockWin,
}

impl FileLock {
    pub fn new() -> Self {
        Self {
            path: None,
            posix_lock: StorageFileLockPosix::new(),
            win_lock: StorageFileLockWin::new(),
        }
    }

    pub fn lock(&mut self, path: impl AsRef<Path>) -> Result<bool, String> {
        let p = path.as_ref();
        self.path = Some(p.to_path_buf());
        #[cfg(unix)]
        {
            self.posix_lock.lock(p)
        }
        #[cfg(windows)]
        {
            self.win_lock.lock(p)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(true)
        }
    }

    pub fn unlock(&mut self) {
        #[cfg(unix)]
        {
            self.posix_lock.unlock();
        }
        #[cfg(windows)]
        {
            self.win_lock.unlock();
        }
        self.path = None;
    }

    pub fn is_locked(&self) -> bool {
        #[cfg(unix)]
        {
            self.posix_lock.is_locked()
        }
        #[cfg(windows)]
        {
            self.win_lock.is_locked()
        }
        #[cfg(not(any(unix, windows)))]
        {
            self.path.is_some()
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        self.unlock();
    }
}

#[derive(Debug, Default)]
pub struct StorageFileLock {
    inner: FileLock,
}

impl StorageFileLock {
    pub fn new() -> Self {
        Self {
            inner: FileLock::new(),
        }
    }

    pub fn try_lock(&mut self, path: impl AsRef<Path>) -> Result<bool, String> {
        self.inner.lock(path)
    }

    pub fn unlock(&mut self) {
        self.inner.unlock();
    }

    pub fn is_locked(&self) -> bool {
        self.inner.is_locked()
    }
}
