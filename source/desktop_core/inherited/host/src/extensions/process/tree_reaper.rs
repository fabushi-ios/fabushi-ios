use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSignal {
    Term,
    Kill,
}

#[derive(Clone, Default)]
pub struct ChildProcessTreeTracker {
    tracked_pids: Arc<Mutex<HashSet<u32>>>,
}

impl ChildProcessTreeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_child(&self, pid: u32) {
        if pid > 0 {
            let mut pids = self.tracked_pids.lock().unwrap();
            pids.insert(pid);
        }
    }

    pub fn unregister_child(&self, pid: u32) -> bool {
        let mut pids = self.tracked_pids.lock().unwrap();
        pids.remove(&pid)
    }

    pub fn is_tracked(&self, pid: u32) -> bool {
        let pids = self.tracked_pids.lock().unwrap();
        pids.contains(&pid)
    }

    pub fn active_count(&self) -> usize {
        let pids = self.tracked_pids.lock().unwrap();
        pids.len()
    }

    pub fn signal_tree(&self, pid: u32, signal: ProcessSignal) -> Result<(), String> {
        #[cfg(unix)]
        {
            let sig = match signal {
                ProcessSignal::Term => libc::SIGTERM,
                ProcessSignal::Kill => libc::SIGKILL,
            };
            // Send to process group if negative, else direct
            let ret = unsafe { libc::kill(-(pid as libc::pid_t), sig) };
            if ret != 0 {
                // Fallback to direct PID kill if process group is unavailable
                let ret2 = unsafe { libc::kill(pid as libc::pid_t, sig) };
                if ret2 != 0 {
                    let err = std::io::Error::last_os_error();
                    // ESRCH means process has already exited
                    if err.raw_os_error() != Some(libc::ESRCH) {
                        return Err(format!("Failed to signal child process tree: {err}"));
                    }
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (pid, signal);
            Ok(())
        }
    }
}
