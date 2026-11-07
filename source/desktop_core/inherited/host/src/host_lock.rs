use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

pub const DEFAULT_TAKEOVER_TIMEOUT_MS: u64 = 3_000;
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 100;
pub const MAX_ACQUIRE_ATTEMPTS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostLockOutcome {
    Created,
    ReclaimedDead,
    ReclaimedForeign,
    TookOver,
}

#[derive(Debug)]
pub struct HostLockHandle {
    path: PathBuf,
    pid: u32,
}

impl HostLockHandle {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn release(&self) -> io::Result<()> {
        if read_lock_pid(&self.path) == Some(self.pid) {
            match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

impl Drop for HostLockHandle {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[derive(Debug)]
pub struct HostLockAcquisition {
    pub outcome: HostLockOutcome,
    pub lock: HostLockHandle,
    pub previous_pid: Option<u32>,
}

pub fn read_lock_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
}

fn remove_lock(path: &Path) {
    let _ = fs::remove_file(path);
}

fn try_create_lock(path: &Path, pid: u32) -> io::Result<bool> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            write!(file, "{pid}")?;
            file.flush()?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn read_process_command(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    if let Ok(value) = fs::read(format!("/proc/{pid}/cmdline")) {
        if !value.is_empty() {
            return Some(
                String::from_utf8_lossy(&value)
                    .replace('\0', " ")
                    .trim()
                    .to_string(),
            );
        }
    }

    #[cfg(unix)]
    {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
            .ok()?;
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }

    #[cfg(windows)]
    {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .ok()?;
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !value.is_empty() && !value.starts_with("INFO:") {
                return Some(value);
            }
        }
    }

    None
}

pub fn default_is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(windows)]
    {
        read_process_command(pid).is_some()
    }
}

pub fn is_sand_host_process(pid: u32) -> bool {
    read_process_command(pid).is_some_and(|command| {
        command.contains("host-main") || command.contains("mahayana-app-host")
    })
}

fn terminate_process(pid: u32, force: bool) {
    #[cfg(unix)]
    {
        let signal = if force { "-KILL" } else { "-TERM" };
        let _ = Command::new("kill")
            .args([signal, &pid.to_string()])
            .status();
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command.args(["/PID", &pid.to_string(), "/T"]);
        if force {
            command.arg("/F");
        }
        let _ = command.status();
    }
}

pub fn acquire_host_lock_with<IsAlive, IsHost, Terminate, Delay>(
    path: &Path,
    pid: u32,
    mut is_alive: IsAlive,
    mut is_host_process: IsHost,
    mut terminate: Terminate,
    mut delay: Delay,
    takeover_timeout_ms: u64,
    poll_interval_ms: u64,
) -> io::Result<HostLockAcquisition>
where
    IsAlive: FnMut(u32) -> bool,
    IsHost: FnMut(u32) -> bool,
    Terminate: FnMut(u32, bool),
    Delay: FnMut(Duration),
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut outcome = HostLockOutcome::Created;
    let mut previous_pid = None;

    for _ in 0..MAX_ACQUIRE_ATTEMPTS {
        if try_create_lock(path, pid)? {
            return Ok(HostLockAcquisition {
                outcome,
                lock: HostLockHandle {
                    path: path.to_path_buf(),
                    pid,
                },
                previous_pid,
            });
        }

        let holder = read_lock_pid(path);
        let Some(holder) = holder else {
            outcome = HostLockOutcome::ReclaimedDead;
            remove_lock(path);
            continue;
        };
        if holder == pid {
            remove_lock(path);
            continue;
        }
        if !is_alive(holder) {
            outcome = HostLockOutcome::ReclaimedDead;
            previous_pid = Some(holder);
            remove_lock(path);
            continue;
        }
        if !is_host_process(holder) {
            outcome = HostLockOutcome::ReclaimedForeign;
            previous_pid = Some(holder);
            remove_lock(path);
            continue;
        }

        outcome = HostLockOutcome::TookOver;
        previous_pid = Some(holder);
        terminate(holder, false);
        let poll = poll_interval_ms.max(1);
        let attempts = takeover_timeout_ms.div_ceil(poll).max(1);
        for _ in 0..attempts {
            if !is_alive(holder) {
                break;
            }
            delay(Duration::from_millis(poll));
        }
        if is_alive(holder) {
            terminate(holder, true);
            for _ in 0..attempts {
                if !is_alive(holder) {
                    break;
                }
                delay(Duration::from_millis(poll));
            }
        }
        remove_lock(path);
    }

    fs::write(path, pid.to_string())?;
    Ok(HostLockAcquisition {
        outcome,
        lock: HostLockHandle {
            path: path.to_path_buf(),
            pid,
        },
        previous_pid,
    })
}

pub fn acquire_host_lock(path: &Path, pid: u32) -> io::Result<HostLockAcquisition> {
    acquire_host_lock_with(
        path,
        pid,
        default_is_process_alive,
        is_sand_host_process,
        terminate_process,
        thread::sleep,
        DEFAULT_TAKEOVER_TIMEOUT_MS,
        DEFAULT_POLL_INTERVAL_MS,
    )
}
