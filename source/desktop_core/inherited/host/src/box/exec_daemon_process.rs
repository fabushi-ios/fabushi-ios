use std::env;
use std::fs;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use super::box_remote_accessor::{ping_box_transport_classified, BoxEndpoint};
use super::generated_production::{create_production_box_control_client, ProductionBoxTransport};
use super::loopback_sand_box::{DEFAULT_AUTH_TOKEN, EXEC_DAEMON_PORT};
use crate::runner::bounded_line_framer::BoundedLineFramer;

pub const BOX_EXEC_DAEMON_START_TIMEOUT_MS: u64 = 20_000;
pub const BOX_EXEC_DAEMON_STOP_TIMEOUT_MS: u64 = 5_000;
pub const BOX_EXEC_DAEMON_READY_POLL_MS: u64 = 100;
pub const BOX_EXEC_DAEMON_PING_TIMEOUT_MS: u64 = 500;
const BOX_EXEC_DAEMON_LOG_LINE_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxExecDaemonProcessOptions {
    pub executable_path: PathBuf,
    pub host: String,
    pub port: u16,
    pub auth_token: String,
    pub workspace_root: PathBuf,
    pub terminals_directory: PathBuf,
    pub start_timeout_ms: u64,
    pub stop_timeout_ms: u64,
}

impl BoxExecDaemonProcessOptions {
    pub fn from_process_env(app_data_dir: &Path) -> Result<Self, String> {
        let host = env::var("SAND_BOX_HOST")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "127.0.0.1".into());
        if host != "127.0.0.1" {
            return Err(format!(
                "managed box exec-daemon requires loopback SAND_BOX_HOST, got {host}"
            ));
        }
        let port = env::var("SAND_BOX_EXEC_DAEMON_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(EXEC_DAEMON_PORT);
        let auth_token = env::var("SAND_BOX_AUTH_TOKEN")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_AUTH_TOKEN.into());
        let workspace_root = env::var_os("SAND_BOX_WORKSPACE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| app_data_dir.join("feature-host/runtime/workspace"));
        let terminals_directory = env::var_os("SAND_BOX_TERMINALS_DIRECTORY")
            .map(PathBuf::from)
            .unwrap_or_else(|| app_data_dir.join("feature-host/runtime/terminals"));
        let current_exe = env::current_exe()
            .map_err(|error| format!("could not resolve Host executable: {error}"))?;
        Ok(Self {
            executable_path: resolve_box_exec_daemon_executable(&current_exe)?,
            host,
            port,
            auth_token,
            workspace_root,
            terminals_directory,
            start_timeout_ms: BOX_EXEC_DAEMON_START_TIMEOUT_MS,
            stop_timeout_ms: BOX_EXEC_DAEMON_STOP_TIMEOUT_MS,
        })
    }
}

pub fn managed_box_exec_daemon_enabled() -> bool {
    !env_flag("SAND_STANDALONE_BOX_EXEC_DAEMON")
}

pub fn resolve_box_exec_daemon_executable(host_executable: &Path) -> Result<PathBuf, String> {
    let directory = host_executable.parent().ok_or_else(|| {
        format!(
            "cannot resolve box exec-daemon sibling for Host executable {}",
            host_executable.display()
        )
    })?;
    Ok(directory.join(box_exec_daemon_binary_name()))
}

pub fn box_exec_daemon_binary_name() -> &'static str {
    if cfg!(windows) {
        "box-exec-daemon.exe"
    } else {
        "box-exec-daemon"
    }
}

#[derive(Debug)]
pub struct OwnedBoxExecDaemon {
    child: Option<Child>,
    parent_pipe: Option<ChildStdin>,
    pub pid: u32,
    pub executable_path: PathBuf,
    host: String,
    port: u16,
    stop_timeout_ms: u64,
}

impl OwnedBoxExecDaemon {
    pub fn close(&mut self) -> Result<(), String> {
        self.close_inner(true)
    }

    fn close_inner(&mut self, strict: bool) -> Result<(), String> {
        // The managed daemon treats EOF on this pipe as parent death. Dropping
        // it first gives normal Host shutdown the same path as an abrupt Host
        // crash, so the daemon cannot survive a generation replacement.
        self.parent_pipe.take();
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        let eof_grace = Duration::from_millis(self.stop_timeout_ms.min(1_000));
        let exited_on_parent_disconnect = wait_for_child_exit(&mut child, eof_grace)?;
        let forced = if exited_on_parent_disconnect {
            false
        } else {
            terminate_child(
                &mut child,
                self.pid,
                Duration::from_millis(self.stop_timeout_ms),
            )?
        };
        if port_accepts_connections(&self.host, self.port, Duration::from_millis(300))? {
            return Err(format!(
                "box exec-daemon shutdown left {}:{} bound",
                self.host, self.port
            ));
        }
        if forced && strict {
            return Err(format!(
                "box exec-daemon pid {} required forced shutdown",
                self.pid
            ));
        }
        Ok(())
    }
}

impl Drop for OwnedBoxExecDaemon {
    fn drop(&mut self) {
        let _ = self.close_inner(false);
    }
}

pub fn start_managed_box_exec_daemon_from_process_env(
    app_data_dir: &Path,
) -> Result<Option<OwnedBoxExecDaemon>, String> {
    if !managed_box_exec_daemon_enabled() {
        return Ok(None);
    }
    let options = BoxExecDaemonProcessOptions::from_process_env(app_data_dir)?;
    start_box_exec_daemon_process(options).map(Some)
}

pub fn start_box_exec_daemon_process(
    options: BoxExecDaemonProcessOptions,
) -> Result<OwnedBoxExecDaemon, String> {
    if port_accepts_connections(&options.host, options.port, Duration::from_millis(300))? {
        return Err(format!(
            "refusing contaminated box exec-daemon startup: {}:{} is already bound",
            options.host, options.port
        ));
    }
    let metadata = fs::metadata(&options.executable_path).map_err(|error| {
        format!(
            "box exec-daemon executable is unavailable at {}: {error}",
            options.executable_path.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "box exec-daemon executable is not a file: {}",
            options.executable_path.display()
        ));
    }
    fs::create_dir_all(&options.workspace_root).map_err(|error| {
        format!(
            "could not create box workspace {}: {error}",
            options.workspace_root.display()
        )
    })?;
    fs::create_dir_all(&options.terminals_directory).map_err(|error| {
        format!(
            "could not create box terminals directory {}: {error}",
            options.terminals_directory.display()
        )
    })?;

    let mut command = Command::new(&options.executable_path);
    command
        .current_dir(
            options
                .executable_path
                .parent()
                .unwrap_or_else(|| Path::new(".")),
        )
        .env("SAND_BOX_EXEC_DAEMON_PORT", options.port.to_string())
        .env("SAND_BOX_EXEC_DAEMON_AUTH_TOKEN", &options.auth_token)
        .env("SAND_BOX_WORKSPACE_ROOT", &options.workspace_root)
        .env("SAND_BOX_TERMINALS_DIRECTORY", &options.terminals_directory)
        .env("SAND_BOX_PARENT_PIPE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(|error| {
        format!(
            "could not spawn box exec-daemon {}: {error}",
            options.executable_path.display()
        )
    })?;
    let pid = child.id();
    crate::process_spawn_diagnostics::global_child_process_spawn_tracker().record_spawn(
        &options.executable_path.to_string_lossy(),
        &[],
        Some("exec-daemon"),
    );
    let parent_pipe = match child.stdin.take() {
        Some(parent_pipe) => parent_pipe,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("managed box exec-daemon did not expose its parent-lifetime pipe".into());
        }
    };
    if let Some(stdout) = child.stdout.take() {
        spawn_log_reader(stdout, false);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_log_reader(stderr, true);
    }

    let endpoint = BoxEndpoint::new(
        options.host.clone(),
        options.port,
        options.auth_token.clone(),
    );
    let transport = ProductionBoxTransport::from_endpoint(&endpoint);
    let deadline = Instant::now() + Duration::from_millis(options.start_timeout_ms);
    let readiness = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break Err(format!("box exec-daemon exited before readiness: {status}"));
        }
        let outcome = ping_box_transport_classified(
            &(),
            &transport,
            create_production_box_control_client,
            BOX_EXEC_DAEMON_PING_TIMEOUT_MS,
        );
        if outcome.outcome == "ok" {
            break Ok(());
        }
        if Instant::now() >= deadline {
            break Err(format!(
                "box exec-daemon did not answer authenticated generated Ping at {}:{}; last outcome={}{}",
                options.host,
                options.port,
                outcome.outcome,
                outcome
                    .cause_summary
                    .as_deref()
                    .map(|cause| format!(" cause={cause}"))
                    .unwrap_or_default(),
            ));
        }
        thread::sleep(Duration::from_millis(BOX_EXEC_DAEMON_READY_POLL_MS));
    };

    if let Err(error) = readiness {
        drop(parent_pipe);
        let _ = terminate_child(
            &mut child,
            pid,
            Duration::from_millis(options.stop_timeout_ms),
        );
        return Err(error);
    }

    Ok(OwnedBoxExecDaemon {
        child: Some(child),
        parent_pipe: Some(parent_pipe),
        pid,
        executable_path: options.executable_path,
        host: options.host,
        port: options.port,
        stop_timeout_ms: options.stop_timeout_ms,
    })
}

fn spawn_log_reader<R>(mut reader: R, _stderr: bool)
where
    R: Read + Send + 'static,
{
    let mut framer = BoundedLineFramer::new(
        BOX_EXEC_DAEMON_LOG_LINE_MAX_BYTES,
        "box exec-daemon log line exceeded the maximum byte length",
    );
    let _ = thread::Builder::new()
        .name("box-exec-daemon-log".into())
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                let read = match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) => {
                        eprintln!("[box-exec-daemon] log read failed: {error}");
                        break;
                    }
                };
                match framer.push(&buffer[..read]) {
                    Ok(lines) => {
                        for line in lines {
                            eprintln!("[box-exec-daemon] {}", String::from_utf8_lossy(&line));
                        }
                    }
                    Err(error) => {
                        eprintln!("[box-exec-daemon] log framing failed: {error}");
                        break;
                    }
                }
            }
        });
}

fn port_accepts_connections(host: &str, port: u16, timeout: Duration) -> Result<bool, String> {
    let mut addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("invalid box exec-daemon endpoint {host}:{port}: {error}"))?;
    let address = addresses
        .next()
        .ok_or_else(|| format!("box exec-daemon endpoint {host}:{port} resolved no addresses"))?;
    Ok(TcpStream::connect_timeout(&address, timeout).is_ok())
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> Result<bool, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn terminate_child(child: &mut Child, pid: u32, timeout: Duration) -> Result<bool, String> {
    crate::runner::process_tree::terminate_process_tree(child, pid, timeout, "box exec-daemon")
}
