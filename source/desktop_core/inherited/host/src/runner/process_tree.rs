//! Cross-platform termination for a managed process and its descendants.
//!
//! Supervisors own the root `Child`, but a shell can leave grandchildren
//! behind.  Keeping discovery and signalling here makes every managed
//! supervisor use the same cleanup semantics.

use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

/// Terminate `child` and descendants, returning whether forceful cleanup was
/// required.  The root child remains owned by the caller and is waited here.
pub fn terminate_process_tree(
    child: &mut Child,
    pid: u32,
    timeout: Duration,
    label: &str,
) -> Result<bool, String> {
    if child
        .try_wait()
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Ok(false);
    }

    let descendants = descendant_pids(pid);
    if let Err(error) = signal_tree(pid, &descendants, false) {
        child.kill().map_err(|kill_error| {
            format!(
                "could not terminate {label} pid {pid}: {error}; force kill failed: {kill_error}"
            )
        })?;
        let _ = child.wait();
        return Ok(true);
    }

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(25));
    }

    // Escalate only the originally captured numeric targets. Re-enumerating
    // after the grace period could authorize a signal to a recycled PID.
    let _ = signal_tree(pid, &descendants, true);
    child
        .kill()
        .map_err(|error| format!("could not force-stop {label} pid {pid}: {error}"))?;
    let _ = child.wait();
    Ok(true)
}

fn descendant_pids(root: u32) -> Vec<u32> {
    #[cfg(unix)]
    {
        let Ok(output) = Command::new("ps").args(["-axo", "pid=,ppid="]).output() else {
            return Vec::new();
        };
        let mut children = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let mut fields = line.split_whitespace();
            let Some(pid) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
                continue;
            };
            let Some(ppid) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
                continue;
            };
            children.push((pid, ppid));
        }

        let mut descendants = Vec::new();
        let mut frontier = vec![root];
        while let Some(parent) = frontier.pop() {
            for (pid, _ppid) in children.iter().copied().filter(|(_, ppid)| *ppid == parent) {
                if pid != root && !descendants.contains(&pid) {
                    descendants.push(pid);
                    frontier.push(pid);
                }
            }
        }
        descendants
    }

    #[cfg(windows)]
    {
        let Ok(output) = Command::new("wmic")
            .args(["process", "get", "ProcessId,ParentProcessId", "/format:csv"])
            .output()
        else {
            return Vec::new();
        };
        let mut children = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let fields = line.split(',').collect::<Vec<_>>();
            if fields.len() < 3 {
                continue;
            }
            let Some(ppid) = fields[1].trim().parse::<u32>().ok() else {
                continue;
            };
            let Some(pid) = fields[2].trim().parse::<u32>().ok() else {
                continue;
            };
            children.push((pid, ppid));
        }
        let mut descendants = Vec::new();
        let mut frontier = vec![root];
        while let Some(parent) = frontier.pop() {
            for (pid, ppid) in children.iter().copied().filter(|(_, ppid)| *ppid == parent) {
                if pid != root && !descendants.contains(&pid) {
                    descendants.push(pid);
                    frontier.push(pid);
                }
            }
        }
        descendants
    }
}

fn signal_tree(root: u32, descendants: &[u32], force: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        if process_group_id(root) == Some(root) {
            let result = unsafe { libc::kill(-(root as libc::pid_t), signal) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(format!("could not signal process group {root}: {error}"));
                }
            }
            return Ok(());
        }
        for pid in descendants
            .iter()
            .rev()
            .copied()
            .chain(std::iter::once(root))
        {
            let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(format!("could not signal pid {pid}: {error}"));
                }
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command.args(["/PID", &root.to_string(), "/T"]);
        if force {
            command.arg("/F");
        }
        let status = command.status().map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("taskkill exited with {status}"))
        }
    }
}

#[cfg(unix)]
fn process_group_id(pid: u32) -> Option<u32> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pgid="])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .and_then(|value| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::descendant_pids;
    use std::process::Command;

    #[cfg(unix)]
    #[test]
    fn discovers_real_shell_descendant() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 2 & wait"])
            .spawn()
            .expect("spawn shell");
        std::thread::sleep(std::time::Duration::from_millis(50));
        let descendants = descendant_pids(child.id());
        assert!(
            !descendants.is_empty(),
            "shell descendant was not discovered"
        );
        let _ = child.kill();
        let _ = child.wait();
    }
}
