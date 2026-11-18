use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

use super::security::{evaluate_command_risk_detailed, scan_content_for_injection};
use super::toolsets::{
    BoxFuture, HermesInlineButton, HermesToolContext, HermesToolExecutor, HermesToolResult,
};

/// All 6 supported Hermes terminal execution backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBackendKind {
    #[default]
    Local,
    Docker,
    Ssh,
    Singularity,
    Modal,
    Daytona,
}

impl TerminalBackendKind {
    pub const ALL: [TerminalBackendKind; 6] = [
        Self::Local,
        Self::Docker,
        Self::Ssh,
        Self::Singularity,
        Self::Modal,
        Self::Daytona,
    ];

    pub fn from_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "docker" => Some(Self::Docker),
            "ssh" => Some(Self::Ssh),
            "singularity" | "apptainer" => Some(Self::Singularity),
            "modal" => Some(Self::Modal),
            "daytona" => Some(Self::Daytona),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Docker => "docker",
            Self::Ssh => "ssh",
            Self::Singularity => "singularity",
            Self::Modal => "modal",
            Self::Daytona => "daytona",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalBackendConfig {
    pub kind: TerminalBackendKind,
    pub docker_container: Option<String>,
    pub docker_image: Option<String>,
    pub docker_user: Option<String>,
    pub ssh_host: Option<String>,
    pub ssh_user: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_key_path: Option<PathBuf>,
    pub singularity_image: Option<String>,
    pub modal_app: Option<String>,
    pub modal_sandbox_id: Option<String>,
    pub daytona_workspace: Option<String>,
    pub daytona_api_url: Option<String>,
}

impl Default for TerminalBackendConfig {
    fn default() -> Self {
        Self {
            kind: TerminalBackendKind::Local,
            docker_container: std::env::var("HERMES_DOCKER_CONTAINER").ok(),
            docker_image: std::env::var("HERMES_DOCKER_IMAGE").ok(),
            docker_user: std::env::var("HERMES_DOCKER_USER").ok(),
            ssh_host: std::env::var("HERMES_SSH_HOST").ok(),
            ssh_user: std::env::var("HERMES_SSH_USER").ok(),
            ssh_port: std::env::var("HERMES_SSH_PORT")
                .ok()
                .and_then(|p| p.parse().ok()),
            ssh_key_path: std::env::var("HERMES_SSH_KEY_PATH").ok().map(PathBuf::from),
            singularity_image: std::env::var("HERMES_SINGULARITY_IMAGE").ok(),
            modal_app: std::env::var("HERMES_MODAL_APP").ok(),
            modal_sandbox_id: std::env::var("HERMES_MODAL_SANDBOX_ID").ok(),
            daytona_workspace: std::env::var("HERMES_DAYTONA_WORKSPACE").ok(),
            daytona_api_url: std::env::var("HERMES_DAYTONA_API_URL").ok(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendCommandSpec {
    pub backend: TerminalBackendKind,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalExecOutput {
    pub backend: TerminalBackendKind,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub timed_out: bool,
    pub duration_ms: u64,
}

/// Unified trait implemented across all 6 Hermes terminal backends.
pub trait TerminalBackend: Send + Sync {
    fn kind(&self) -> TerminalBackendKind;
    fn build_command_spec(&self, cmd: &str, cwd: &Path) -> Result<BackendCommandSpec, String>;
    fn execute<'a>(
        &'a self,
        cmd: &'a str,
        cwd: &'a Path,
        timeout_ms: u64,
        max_output_bytes: usize,
        env_scrub: bool,
    ) -> BoxFuture<'a, Result<TerminalExecOutput, String>>;
}

pub struct ConfiguredTerminalBackend {
    pub config: TerminalBackendConfig,
}

impl ConfiguredTerminalBackend {
    pub fn new(config: TerminalBackendConfig) -> Self {
        Self { config }
    }
}

impl TerminalBackend for ConfiguredTerminalBackend {
    fn kind(&self) -> TerminalBackendKind {
        self.config.kind
    }

    fn build_command_spec(&self, cmd: &str, cwd: &Path) -> Result<BackendCommandSpec, String> {
        match self.config.kind {
            TerminalBackendKind::Local => Ok(BackendCommandSpec {
                backend: TerminalBackendKind::Local,
                program: "/bin/sh".into(),
                args: vec!["-c".into(), cmd.to_string()],
                cwd: Some(cwd.to_path_buf()),
            }),
            TerminalBackendKind::Docker => {
                if let Some(container) = self
                    .config
                    .docker_container
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                {
                    let mut args = vec!["exec".into()];
                    if let Some(user) = self.config.docker_user.as_deref().filter(|s| !s.is_empty()) {
                        args.push("-u".into());
                        args.push(user.into());
                    }
                    args.push(container.into());
                    args.push("/bin/sh".into());
                    args.push("-c".into());
                    args.push(cmd.to_string());
                    Ok(BackendCommandSpec {
                        backend: TerminalBackendKind::Docker,
                        program: "docker".into(),
                        args,
                        cwd: None,
                    })
                } else if let Some(image) = self
                    .config
                    .docker_image
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                {
                    let mut args = vec![
                        "run".into(),
                        "--rm".into(),
                        "-v".into(),
                        format!("{}:/workspace", cwd.display()),
                        "-w".into(),
                        "/workspace".into(),
                    ];
                    if let Some(user) = self.config.docker_user.as_deref().filter(|s| !s.is_empty()) {
                        args.push("-u".into());
                        args.push(user.into());
                    }
                    args.push(image.into());
                    args.push("/bin/sh".into());
                    args.push("-c".into());
                    args.push(cmd.to_string());
                    Ok(BackendCommandSpec {
                        backend: TerminalBackendKind::Docker,
                        program: "docker".into(),
                        args,
                        cwd: None,
                    })
                } else {
                    Err("Docker terminal backend is not configured: set docker_container (HERMES_DOCKER_CONTAINER) or docker_image (HERMES_DOCKER_IMAGE)".into())
                }
            }
            TerminalBackendKind::Ssh => {
                let host = self
                    .config
                    .ssh_host
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        "SSH terminal backend is not configured: set ssh_host (HERMES_SSH_HOST)"
                            .to_string()
                    })?;
                let mut args = vec![
                    "-o".into(),
                    "BatchMode=yes".into(),
                    "-o".into(),
                    "StrictHostKeyChecking=accept-new".into(),
                ];
                if let Some(port) = self.config.ssh_port {
                    args.push("-p".into());
                    args.push(port.to_string());
                }
                if let Some(key) = &self.config.ssh_key_path {
                    args.push("-i".into());
                    args.push(key.display().to_string());
                }
                let target = match self.config.ssh_user.as_deref().filter(|s| !s.is_empty()) {
                    Some(u) => format!("{u}@{host}"),
                    None => host.to_string(),
                };
                args.push(target);
                args.push(cmd.to_string());
                Ok(BackendCommandSpec {
                    backend: TerminalBackendKind::Ssh,
                    program: "ssh".into(),
                    args,
                    cwd: None,
                })
            }
            TerminalBackendKind::Singularity => {
                let image = self
                    .config
                    .singularity_image
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        "Singularity backend is not configured: set singularity_image (HERMES_SINGULARITY_IMAGE)".to_string()
                    })?;
                Ok(BackendCommandSpec {
                    backend: TerminalBackendKind::Singularity,
                    program: "singularity".into(),
                    args: vec![
                        "exec".into(),
                        image.into(),
                        "/bin/sh".into(),
                        "-c".into(),
                        cmd.to_string(),
                    ],
                    cwd: Some(cwd.to_path_buf()),
                })
            }
            TerminalBackendKind::Modal => {
                if let Some(sandbox) = self
                    .config
                    .modal_sandbox_id
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                {
                    Ok(BackendCommandSpec {
                        backend: TerminalBackendKind::Modal,
                        program: "modal".into(),
                        args: vec![
                            "shell".into(),
                            sandbox.into(),
                            "--cmd".into(),
                            cmd.to_string(),
                        ],
                        cwd: None,
                    })
                } else if let Some(app) = self
                    .config
                    .modal_app
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                {
                    Ok(BackendCommandSpec {
                        backend: TerminalBackendKind::Modal,
                        program: "modal".into(),
                        args: vec![
                            "run".into(),
                            app.into(),
                            "--command".into(),
                            cmd.to_string(),
                        ],
                        cwd: None,
                    })
                } else {
                    Err("Modal terminal backend is not configured: set modal_sandbox_id (HERMES_MODAL_SANDBOX_ID) or modal_app (HERMES_MODAL_APP)".into())
                }
            }
            TerminalBackendKind::Daytona => {
                let workspace = self
                    .config
                    .daytona_workspace
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        "Daytona terminal backend is not configured: set daytona_workspace (HERMES_DAYTONA_WORKSPACE)".to_string()
                    })?;
                Ok(BackendCommandSpec {
                    backend: TerminalBackendKind::Daytona,
                    program: "daytona".into(),
                    args: vec![
                        "exec".into(),
                        workspace.into(),
                        "--".into(),
                        "/bin/sh".into(),
                        "-c".into(),
                        cmd.to_string(),
                    ],
                    cwd: None,
                })
            }
        }
    }

    fn execute<'a>(
        &'a self,
        cmd: &'a str,
        cwd: &'a Path,
        timeout_ms: u64,
        max_output_bytes: usize,
        env_scrub: bool,
    ) -> BoxFuture<'a, Result<TerminalExecOutput, String>> {
        Box::pin(async move {
            #[cfg(target_os = "ios")]
            {
                let _ = (cmd, cwd, timeout_ms, max_output_bytes, env_scrub);
                return Err(
                    "Local subprocess execution is unavailable inside the iOS sandbox; route terminal commands through a remote ComputerWorkbench or cloud Hermes backend.".to_string()
                );
            }
            #[cfg(not(target_os = "ios"))]
            {
            let spec = self.build_command_spec(cmd, cwd)?;
            let started = Instant::now();
            let mut command = build_tokio_command(&spec, env_scrub);
            command.stdout(Stdio::piped()).stderr(Stdio::piped());

            let mut child = command.spawn().map_err(|e| {
                format!(
                    "Failed to spawn {} backend ('{}'): {e}",
                    spec.backend.as_str(),
                    spec.program
                )
            })?;

            let mut stdout_pipe = child.stdout.take();
            let mut stderr_pipe = child.stderr.take();

            let stdout_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(ref mut r) = stdout_pipe {
                    let _ = r.read_to_end(&mut buf).await;
                }
                buf
            });
            let stderr_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(ref mut r) = stderr_pipe {
                    let _ = r.read_to_end(&mut buf).await;
                }
                buf
            });

            let mut timed_out = false;
            let exit_code = match tokio::time::timeout(
                Duration::from_millis(timeout_ms.max(100)),
                child.wait(),
            )
            .await
            {
                Ok(Ok(status)) => status.code().unwrap_or(-1),
                Ok(Err(e)) => {
                    return Err(format!("Failed waiting for process: {e}"));
                }
                Err(_) => {
                    timed_out = true;
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    124
                }
            };

            let raw_out = stdout_task.await.unwrap_or_default();
            let raw_err = stderr_task.await.unwrap_or_default();
            let (stdout, out_trunc) = truncate_lossy(&raw_out, max_output_bytes);
            let (stderr, err_trunc) = truncate_lossy(&raw_err, max_output_bytes);

            Ok(TerminalExecOutput {
                backend: spec.backend,
                exit_code,
                stdout,
                stderr,
                truncated: out_trunc || err_trunc,
                timed_out,
                duration_ms: started.elapsed().as_millis() as u64,
            })
            }
        })
    }
}

fn is_sensitive_env_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.contains("KEY")
        || upper.contains("SECRET")
        || upper.contains("TOKEN")
        || upper.contains("PASSWORD")
        || upper.contains("CREDENTIAL")
        || upper.starts_with("AWS_")
        || upper.starts_with("OPENAI_")
        || upper.starts_with("ANTHROPIC_")
        || upper.starts_with("DEEPSEEK_")
        || upper.starts_with("OPENROUTER_")
        || upper == "DATABASE_URL"
}

fn build_tokio_command(spec: &BackendCommandSpec, env_scrub: bool) -> Command {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    if let Some(dir) = &spec.cwd {
        command.current_dir(dir);
    }
    if env_scrub {
        for (k, _) in std::env::vars() {
            if is_sensitive_env_var(&k) {
                command.env_remove(&k);
            }
        }
    }
    command
}

fn truncate_lossy(bytes: &[u8], max_bytes: usize) -> (String, bool) {
    if bytes.len() <= max_bytes {
        (String::from_utf8_lossy(bytes).into_owned(), false)
    } else {
        let mut s = String::from_utf8_lossy(&bytes[..max_bytes]).into_owned();
        s.push_str("\n...[output truncated]...");
        (s, true)
    }
}

/// Live background process state tracked in `ProcessRegistry`.
struct ManagedProcess {
    process_id: String,
    os_pid: Option<u32>,
    command: String,
    cwd: PathBuf,
    backend: TerminalBackendKind,
    started_at: DateTime<Utc>,
    buffer: Arc<Mutex<ProcessIoBuffer>>,
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    child: Arc<tokio::sync::Mutex<Option<Child>>>,
}

#[derive(Default)]
struct ProcessIoBuffer {
    stdout: String,
    stderr: String,
    running: bool,
    exit_code: Option<i32>,
    read_cursor_out: usize,
    read_cursor_err: usize,
}

impl ProcessIoBuffer {
    fn push_out(&mut self, chunk: &str) {
        self.stdout.push_str(chunk);
        const CAP: usize = 512 * 1024;
        if self.stdout.len() > CAP {
            let drop_bytes = self.stdout.len() - CAP;
            let mut cut = drop_bytes;
            while cut < self.stdout.len() && !self.stdout.is_char_boundary(cut) {
                cut += 1;
            }
            self.stdout.drain(..cut);
            self.read_cursor_out = self.read_cursor_out.saturating_sub(cut);
        }
    }

    fn push_err(&mut self, chunk: &str) {
        self.stderr.push_str(chunk);
        const CAP: usize = 256 * 1024;
        if self.stderr.len() > CAP {
            let drop_bytes = self.stderr.len() - CAP;
            let mut cut = drop_bytes;
            while cut < self.stderr.len() && !self.stderr.is_char_boundary(cut) {
                cut += 1;
            }
            self.stderr.drain(..cut);
            self.read_cursor_err = self.read_cursor_err.saturating_sub(cut);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSummary {
    pub process_id: String,
    pub pid: Option<u32>,
    pub command: String,
    pub cwd: String,
    pub backend: TerminalBackendKind,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub started_at: DateTime<Utc>,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessPollResult {
    pub process_id: String,
    pub pid: Option<u32>,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub stdout_delta: String,
    pub stderr_delta: String,
}

/// Live registry of persistent background child processes keyed by `process_id` / handle.
pub struct ProcessRegistry {
    next_id: AtomicU64,
    processes: Mutex<HashMap<String, Arc<ManagedProcess>>>,
}

impl Default for ProcessRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            processes: Mutex::new(HashMap::new()),
        }
    }

    pub async fn spawn_background(
        &self,
        backend_cfg: &TerminalBackendConfig,
        cmd: &str,
        cwd: &Path,
        env_scrub: bool,
    ) -> Result<ProcessSummary, String> {
        let backend = ConfiguredTerminalBackend::new(backend_cfg.clone());
        let spec = backend.build_command_spec(cmd, cwd)?;
        let mut command = build_tokio_command(&spec, env_scrub);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|e| {
            format!(
                "Failed to spawn background process via {}: {e}",
                spec.backend.as_str()
            )
        })?;
        let os_pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let id_num = self.next_id.fetch_add(1, Ordering::SeqCst);
        let process_id = format!("proc_{id_num}");
        let buffer = Arc::new(Mutex::new(ProcessIoBuffer {
            running: true,
            ..Default::default()
        }));

        if let Some(mut out_reader) = stdout {
            let buf_ref = Arc::clone(&buffer);
            tokio::spawn(async move {
                let mut tmp = [0u8; 4096];
                while let Ok(n) = out_reader.read(&mut tmp).await {
                    if n == 0 {
                        break;
                    }
                    let text = String::from_utf8_lossy(&tmp[..n]);
                    if let Ok(mut b) = buf_ref.lock() {
                        b.push_out(&text);
                    }
                }
            });
        }

        if let Some(mut err_reader) = stderr {
            let buf_ref = Arc::clone(&buffer);
            tokio::spawn(async move {
                let mut tmp = [0u8; 4096];
                while let Ok(n) = err_reader.read(&mut tmp).await {
                    if n == 0 {
                        break;
                    }
                    let text = String::from_utf8_lossy(&tmp[..n]);
                    if let Ok(mut b) = buf_ref.lock() {
                        b.push_err(&text);
                    }
                }
            });
        }

        let proc_arc = Arc::new(ManagedProcess {
            process_id: process_id.clone(),
            os_pid,
            command: cmd.to_string(),
            cwd: cwd.to_path_buf(),
            backend: spec.backend,
            started_at: Utc::now(),
            buffer,
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            child: Arc::new(tokio::sync::Mutex::new(Some(child))),
        });

        let summary = Self::summarize_entry(&proc_arc).await;
        if let Ok(mut map) = self.processes.lock() {
            map.insert(process_id, proc_arc);
        }
        Ok(summary)
    }

    async fn refresh_exit_status(proc_entry: &ManagedProcess) {
        let mut child_guard = proc_entry.child.lock().await;
        if let Some(child) = child_guard.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                if let Ok(mut buf) = proc_entry.buffer.lock() {
                    buf.running = false;
                    buf.exit_code = status.code();
                }
                *child_guard = None;
            }
        }
    }

    async fn summarize_entry(proc_entry: &ManagedProcess) -> ProcessSummary {
        Self::refresh_exit_status(proc_entry).await;
        let (running, exit_code, stdout_bytes, stderr_bytes) = proc_entry
            .buffer
            .lock()
            .map(|b| (b.running, b.exit_code, b.stdout.len(), b.stderr.len()))
            .unwrap_or((false, None, 0, 0));
        ProcessSummary {
            process_id: proc_entry.process_id.clone(),
            pid: proc_entry.os_pid,
            command: proc_entry.command.clone(),
            cwd: proc_entry.cwd.display().to_string(),
            backend: proc_entry.backend,
            running,
            exit_code,
            started_at: proc_entry.started_at,
            stdout_bytes,
            stderr_bytes,
        }
    }

    fn get(&self, process_id: &str) -> Result<Arc<ManagedProcess>, String> {
        self.processes
            .lock()
            .map_err(|_| "ProcessRegistry lock poisoned".to_string())?
            .get(process_id)
            .cloned()
            .ok_or_else(|| format!("Process '{process_id}' not found"))
    }

    pub async fn list(&self) -> Vec<ProcessSummary> {
        let entries: Vec<Arc<ManagedProcess>> = self
            .processes
            .lock()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        let mut summaries = Vec::with_capacity(entries.len());
        for entry in entries {
            summaries.push(Self::summarize_entry(&entry).await);
        }
        summaries.sort_by(|a, b| a.process_id.cmp(&b.process_id));
        summaries
    }

    pub async fn poll(&self, process_id: &str, wait_ms: u64) -> Result<ProcessPollResult, String> {
        let entry = self.get(process_id)?;
        if wait_ms > 0 {
            tokio::time::sleep(Duration::from_millis(wait_ms.min(5000))).await;
        }
        Self::refresh_exit_status(&entry).await;

        let mut buf = entry
            .buffer
            .lock()
            .map_err(|_| "Process buffer lock poisoned".to_string())?;
        let out_start = buf.read_cursor_out.min(buf.stdout.len());
        let err_start = buf.read_cursor_err.min(buf.stderr.len());
        let stdout_delta = buf.stdout[out_start..].to_string();
        let stderr_delta = buf.stderr[err_start..].to_string();
        buf.read_cursor_out = buf.stdout.len();
        buf.read_cursor_err = buf.stderr.len();

        Ok(ProcessPollResult {
            process_id: entry.process_id.clone(),
            pid: entry.os_pid,
            running: buf.running,
            exit_code: buf.exit_code,
            stdout_delta,
            stderr_delta,
        })
    }

    pub async fn write(&self, process_id: &str, input: &str, eof: bool) -> Result<usize, String> {
        let entry = self.get(process_id)?;
        let mut stdin_guard = entry.stdin.lock().await;
        let Some(stdin) = stdin_guard.as_mut() else {
            return Err(format!("Stdin for process '{process_id}' is already closed"));
        };
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|e| format!("Failed writing to stdin of '{process_id}': {e}"))?;
        let _ = stdin.flush().await;
        if eof {
            *stdin_guard = None;
        }
        Ok(input.len())
    }

    pub async fn kill(&self, process_id: &str) -> Result<ProcessSummary, String> {
        let entry = self.get(process_id)?;
        {
            let mut stdin_guard = entry.stdin.lock().await;
            *stdin_guard = None;
        }
        {
            let mut child_guard = entry.child.lock().await;
            if let Some(mut child) = child_guard.take() {
                let _ = child.kill().await;
                let status = child.wait().await.ok();
                if let Ok(mut buf) = entry.buffer.lock() {
                    buf.running = false;
                    buf.exit_code = status.and_then(|s| s.code()).or(Some(-9));
                }
            }
        }
        Ok(Self::summarize_entry(&entry).await)
    }
}

pub async fn execute_terminal_tool(
    name: &str,
    args: Value,
    ctx: &HermesToolContext,
) -> HermesToolResult {
    match name {
        "terminal" => tool_terminal(args, ctx).await,
        "process_list" => tool_process_list(ctx).await,
        "process_poll" => tool_process_poll(args, ctx).await,
        "process_write" => tool_process_write(args, ctx).await,
        "process_kill" => tool_process_kill(args, ctx).await,
        "execute_code" => tool_execute_code(args, ctx).await,
        _ => HermesToolResult::err(name, format!("Unknown terminal tool '{name}'")),
    }
}

async fn check_danger_gate(
    tool_name: &str,
    command_text: &str,
    ctx: &HermesToolContext,
) -> Result<Option<HermesToolResult>, String> {
    let detailed = evaluate_command_risk_detailed(command_text);
    let check_res = {
        let mut sec = ctx
            .security
            .write()
            .map_err(|_| "SecurityEngine lock poisoned".to_string())?;
        match sec
            .approvals
            .check_execution(&ctx.session_id, tool_name, command_text, &detailed)
        {
            Ok(true) => Ok(None),
            Ok(false) => {
                let pending = sec.approvals.enqueue(
                    &ctx.session_id,
                    tool_name,
                    command_text,
                    &detailed,
                );
                if let Some(cb) = &ctx.approval_callback {
                    if let Some(action) = cb(&pending) {
                        let decided = sec.approvals.decide(&pending.id, action, "callback")?;
                        if !matches!(decided.status, super::security::ApprovalStatus::Denied) {
                            return Ok(None);
                        }
                        return Err(format!("Command denied by approval callback: {}", pending.reason));
                    }
                }
                Ok(Some(pending))
            }
            Err(denied) => Err(denied),
        }
    }?;

    if let Some(pending) = check_res {
        if let Some(bridge) = &ctx.state_bridge {
            let _ = bridge.record_approval(pending.clone()).await;
        }
        let buttons = vec![
            HermesInlineButton {
                label: "Approve Once".into(),
                callback_data: format!("/approve {}", pending.id),
            },
            HermesInlineButton {
                label: "Deny".into(),
                callback_data: format!("/deny {}", pending.id),
            },
            HermesInlineButton {
                label: "YOLO Session".into(),
                callback_data: "/yolo".into(),
            },
        ];
        return Ok(Some(
            HermesToolResult::err(
                tool_name,
                format!(
                    "Approval required ({}) [{}]: {}",
                    pending.risk_level, pending.id, pending.reason
                ),
            )
            .with_inline_buttons(buttons),
        ));
    }

    Ok(None)
}

async fn tool_terminal(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(cmd) = args.get("command").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("terminal", "Missing required 'command' parameter");
    };

    match check_danger_gate("terminal", cmd, ctx).await {
        Ok(Some(approval_prompt_res)) => return approval_prompt_res,
        Ok(None) => {}
        Err(denied_msg) => return HermesToolResult::err("terminal", denied_msg),
    }

    let cwd = if let Some(wd) = args.get("workdir").or_else(|| args.get("cwd")).and_then(|v| v.as_str()) {
        let Ok(sec) = ctx.security.read() else {
            return HermesToolResult::err("terminal", "SecurityEngine lock poisoned");
        };
        match sec.validate_path(Path::new(wd)) {
            Ok(p) => p,
            Err(e) => return HermesToolResult::err("terminal", e),
        }
    } else {
        ctx.workspace_root.clone()
    };

    let mut backend_cfg = ctx.terminal_backend.clone();
    if let Some(b_str) = args.get("backend").and_then(|v| v.as_str()) {
        match TerminalBackendKind::from_str(b_str) {
            Some(kind) => backend_cfg.kind = kind,
            None => {
                return HermesToolResult::err(
                    "terminal",
                    format!("Unsupported terminal backend '{b_str}'"),
                )
            }
        }
    }

    let background = args
        .get("background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if background {
        return match ctx.processes.spawn_background(&backend_cfg, cmd, &cwd, true).await {
            Ok(summary) => HermesToolResult::ok(
                "terminal",
                format!(
                    "Started background process '{}' (pid={:?}) via {}",
                    summary.process_id,
                    summary.pid,
                    summary.backend.as_str()
                ),
                Some(serde_json::to_value(&summary).unwrap_or_default()),
            ),
            Err(e) => HermesToolResult::err("terminal", e),
        };
    }

    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(30_000);
    let max_output_bytes = args
        .get("max_output_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(65_536) as usize;

    let backend = ConfiguredTerminalBackend::new(backend_cfg);
    match backend.execute(cmd, &cwd, timeout_ms, max_output_bytes, true).await {
        Ok(out) => {
            let clean_stdout = scan_content_for_injection(&out.stdout).sanitized_text;
            let clean_stderr = scan_content_for_injection(&out.stderr).sanitized_text;
            let mut summary = format!(
                "[backend={} exit_code={} duration_ms={}]",
                out.backend.as_str(),
                out.exit_code,
                out.duration_ms
            );
            if out.timed_out {
                summary.push_str(" [TIMED OUT]");
            }
            if !clean_stdout.is_empty() {
                summary.push('\n');
                summary.push_str(&clean_stdout);
            }
            if !clean_stderr.is_empty() {
                summary.push_str("\n[stderr]\n");
                summary.push_str(&clean_stderr);
            }

            let structured = json!({
                "backend": out.backend.as_str(),
                "exit_code": out.exit_code,
                "stdout": clean_stdout,
                "stderr": clean_stderr,
                "truncated": out.truncated,
                "timed_out": out.timed_out,
                "duration_ms": out.duration_ms,
            });
            let mut res = HermesToolResult::ok("terminal", summary, Some(structured));
            if out.exit_code != 0 {
                res.success = false;
                res.error = Some(format!("Command exited with code {}", out.exit_code));
            }
            res
        }
        Err(e) => HermesToolResult::err("terminal", e),
    }
}

async fn tool_process_list(ctx: &HermesToolContext) -> HermesToolResult {
    let list = ctx.processes.list().await;
    let text = if list.is_empty() {
        "No background processes tracked.".into()
    } else {
        serde_json::to_string_pretty(&list).unwrap_or_default()
    };
    HermesToolResult::ok("process_list", text, Some(json!({ "processes": list })))
}

async fn tool_process_poll(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(pid) = args.get("process_id").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("process_poll", "Missing required 'process_id'");
    };
    let wait_ms = args.get("wait_ms").and_then(|v| v.as_u64()).unwrap_or(0);
    match ctx.processes.poll(pid, wait_ms).await {
        Ok(poll) => {
            let text = format!(
                "process={} running={} exit_code={:?}\nstdout:\n{}\nstderr:\n{}",
                poll.process_id, poll.running, poll.exit_code, poll.stdout_delta, poll.stderr_delta
            );
            HermesToolResult::ok(
                "process_poll",
                text,
                Some(serde_json::to_value(&poll).unwrap_or_default()),
            )
        }
        Err(e) => HermesToolResult::err("process_poll", e),
    }
}

async fn tool_process_write(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(pid) = args.get("process_id").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("process_write", "Missing required 'process_id'");
    };
    let Some(input) = args.get("input").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("process_write", "Missing required 'input'");
    };
    let eof = args.get("eof").and_then(|v| v.as_bool()).unwrap_or(false);

    match ctx.processes.write(pid, input, eof).await {
        Ok(written) => HermesToolResult::ok(
            "process_write",
            format!("Wrote {written} bytes to process '{pid}'"),
            Some(json!({ "process_id": pid, "bytes_written": written, "eof": eof })),
        ),
        Err(e) => HermesToolResult::err("process_write", e),
    }
}

async fn tool_process_kill(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(pid) = args.get("process_id").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("process_kill", "Missing required 'process_id'");
    };
    match ctx.processes.kill(pid).await {
        Ok(summary) => HermesToolResult::ok(
            "process_kill",
            format!("Killed process '{}' (exit_code={:?})", summary.process_id, summary.exit_code),
            Some(serde_json::to_value(&summary).unwrap_or_default()),
        ),
        Err(e) => HermesToolResult::err("process_kill", e),
    }
}

/// Sandboxed `execute_code` tool:
/// - Evaluates restricted safe expressions directly in pure Rust (`expr` / `math` / `rust_expr` / `json`).
/// - Runs bounded subprocess interpreters (`python3` / `node` / `sh`) with scrubbed env and RPC tool bridge.
async fn tool_execute_code(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let language = args
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("expr")
        .trim()
        .to_ascii_lowercase();
    let Some(code) = args.get("code").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("execute_code", "Missing required 'code' parameter");
    };
    let input_val = args.get("input").cloned().unwrap_or(Value::Null);

    match language.as_str() {
        "expr" | "math" | "rust_expr" => match eval_safe_expression(code, &input_val) {
            Ok(val) => HermesToolResult::ok(
                "execute_code",
                format!("{val}"),
                Some(json!({ "language": language, "sandbox": "pure_rust", "result": val })),
            ),
            Err(e) => HermesToolResult::err("execute_code", e),
        },
        "json" => match serde_json::from_str::<Value>(code) {
            Ok(v) => HermesToolResult::ok(
                "execute_code",
                serde_json::to_string_pretty(&v).unwrap_or_default(),
                Some(json!({ "language": "json", "sandbox": "pure_rust", "result": v })),
            ),
            Err(e) => HermesToolResult::err("execute_code", format!("Invalid JSON: {e}")),
        },
        "python" | "python3" | "node" | "javascript" | "js" | "sh" | "bash" => {
            run_subprocess_sandbox(&language, code, &args, &input_val, ctx).await
        }
        _ => HermesToolResult::err(
            "execute_code",
            format!("Unsupported execute_code language '{language}'"),
        ),
    }
}

async fn run_subprocess_sandbox(
    language: &str,
    code: &str,
    args: &Value,
    input_val: &Value,
    ctx: &HermesToolContext,
) -> HermesToolResult {
    match check_danger_gate("execute_code", code, ctx).await {
        Ok(Some(approval_res)) => return approval_res,
        Ok(None) => {}
        Err(denied) => return HermesToolResult::err("execute_code", denied),
    }

    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(10_000)
        .min(60_000);
    let allowed_tools: Vec<String> = args
        .get("allowed_tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();

    let (program, prog_args): (&str, Vec<&str>) = match language {
        "python" | "python3" => ("python3", vec!["-I", "-c", code]),
        "node" | "javascript" | "js" => ("node", vec!["-e", code]),
        "sh" | "bash" => ("/bin/sh", vec!["-c", code]),
        _ => return HermesToolResult::err("execute_code", "Unsupported interpreter"),
    };

    let sandbox_dir = std::env::temp_dir().join(format!(
        "hermes_sandbox_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let _ = std::fs::create_dir_all(&sandbox_dir);
    struct SandboxDirGuard(PathBuf);
    impl Drop for SandboxDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _guard = SandboxDirGuard(sandbox_dir.clone());

    let mut cmd = Command::new(program);
    cmd.args(&prog_args)
        .current_dir(&sandbox_dir)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin")
        .env("TMPDIR", sandbox_dir.to_string_lossy().as_ref())
        .env("LANG", "C.UTF-8")
        .env("HERMES_INPUT_JSON", input_val.to_string())
        .env("HERMES_ALLOWED_TOOLS", allowed_tools.join(","))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return HermesToolResult::err(
                "execute_code",
                format!("Failed to launch sandboxed '{program}': {e}"),
            )
        }
    };

    let mut child_stdin = child.stdin.take();
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut r) = stderr_pipe {
            let _ = r.read_to_end(&mut buf).await;
        }
        buf
    });

    let mut stdout_collected = String::new();
    let mut rpc_calls = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);

    if let Some(out_pipe) = stdout_pipe {
        let mut lines = BufReader::new(out_pipe).lines();
        loop {
            let next_line = tokio::time::timeout_at(deadline, lines.next_line()).await;
            match next_line {
                Ok(Ok(Some(line))) => {
                    if let Some(rpc_json_str) = line.strip_prefix("__HERMES_RPC__:") {
                        rpc_calls += 1;
                        let reply = handle_rpc_bridge_call(rpc_json_str, &allowed_tools, ctx).await;
                        if let Some(stdin) = child_stdin.as_mut() {
                            let _ = stdin.write_all(format!("{reply}\n").as_bytes()).await;
                            let _ = stdin.flush().await;
                        }
                    } else {
                        if stdout_collected.len() < 65_536 {
                            stdout_collected.push_str(&line);
                            stdout_collected.push('\n');
                        }
                    }
                }
                Ok(Ok(None)) => break,
                Ok(Err(_)) => break,
                Err(_) => {
                    let _ = child.kill().await;
                    return HermesToolResult::err(
                        "execute_code",
                        format!("Sandboxed {language} execution timed out after {timeout_ms}ms"),
                    );
                }
            }
        }
    }

    drop(child_stdin);
    let status = match tokio::time::timeout_at(deadline, child.wait()).await {
        Ok(Ok(s)) => s,
        _ => {
            let _ = child.kill().await;
            return HermesToolResult::err("execute_code", "Sandboxed process timed out on exit");
        }
    };
    let err_bytes = stderr_task.await.unwrap_or_default();
    let (stderr_str, _) = truncate_lossy(&err_bytes, 32_768);
    let exit_code = status.code().unwrap_or(-1);

    let mut res = HermesToolResult::ok(
        "execute_code",
        if stderr_str.is_empty() {
            stdout_collected.trim_end().to_string()
        } else {
            format!("{}\n[stderr]\n{}", stdout_collected.trim_end(), stderr_str.trim_end())
        },
        Some(json!({
            "language": language,
            "sandbox": "subprocess_env_cleared",
            "exit_code": exit_code,
            "stdout": stdout_collected,
            "stderr": stderr_str,
            "rpc_calls": rpc_calls,
        })),
    );
    if exit_code != 0 {
        res.success = false;
        res.error = Some(format!("Sandboxed {language} exited with code {exit_code}"));
    }
    res
}

async fn handle_rpc_bridge_call(
    raw_json: &str,
    allowed_tools: &[String],
    ctx: &HermesToolContext,
) -> String {
    let Ok(req) = serde_json::from_str::<Value>(raw_json) else {
        return json!({ "ok": false, "error": "Invalid RPC JSON" }).to_string();
    };
    let tool = req.get("tool").and_then(|v| v.as_str()).unwrap_or("");
    let args = req.get("args").cloned().unwrap_or(json!({}));
    if tool == "execute_code" || !allowed_tools.iter().any(|t| t == tool) {
        return json!({
            "ok": false,
            "error": format!("Tool '{tool}' is not in execute_code allowed_tools whitelist")
        })
        .to_string();
    }
    let result = Box::pin(HermesToolExecutor::execute(tool, args, ctx)).await;
    json!({
        "ok": result.success,
        "output": result.output,
        "structured": result.structured,
        "error": result.error,
    })
    .to_string()
}

/// Pure-Rust restricted safe expression evaluator (numbers, strings, booleans, arithmetic, math functions).
pub fn eval_safe_expression(expr: &str, input: &Value) -> Result<Value, String> {
    let tokens = tokenize_expr(expr)?;
    let mut parser = ExprParser {
        tokens: &tokens,
        pos: 0,
        input,
    };
    let val = parser.parse_or()?;
    if parser.pos < parser.tokens.len() {
        return Err(format!("Unexpected trailing token {:?}", parser.tokens[parser.pos]));
    }
    Ok(val)
}

#[derive(Debug, Clone, PartialEq)]
enum ExprTok {
    Num(f64),
    Str(String),
    Bool(bool),
    Ident(String),
    LParen,
    RParen,
    Comma,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Bang,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
}

fn tokenize_expr(s: &str) -> Result<Vec<ExprTok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                out.push(ExprTok::LParen);
                i += 1;
            }
            ')' => {
                out.push(ExprTok::RParen);
                i += 1;
            }
            ',' => {
                out.push(ExprTok::Comma);
                i += 1;
            }
            '+' => {
                out.push(ExprTok::Plus);
                i += 1;
            }
            '-' => {
                out.push(ExprTok::Minus);
                i += 1;
            }
            '*' => {
                out.push(ExprTok::Star);
                i += 1;
            }
            '/' => {
                out.push(ExprTok::Slash);
                i += 1;
            }
            '%' => {
                out.push(ExprTok::Percent);
                i += 1;
            }
            '^' => {
                out.push(ExprTok::Caret);
                i += 1;
            }
            '=' if i + 1 < chars.len() && chars[i + 1] == '=' => {
                out.push(ExprTok::EqEq);
                i += 2;
            }
            '!' if i + 1 < chars.len() && chars[i + 1] == '=' => {
                out.push(ExprTok::NotEq);
                i += 2;
            }
            '!' => {
                out.push(ExprTok::Bang);
                i += 1;
            }
            '<' if i + 1 < chars.len() && chars[i + 1] == '=' => {
                out.push(ExprTok::Le);
                i += 2;
            }
            '<' => {
                out.push(ExprTok::Lt);
                i += 1;
            }
            '>' if i + 1 < chars.len() && chars[i + 1] == '=' => {
                out.push(ExprTok::Ge);
                i += 2;
            }
            '>' => {
                out.push(ExprTok::Gt);
                i += 1;
            }
            '&' if i + 1 < chars.len() && chars[i + 1] == '&' => {
                out.push(ExprTok::AndAnd);
                i += 2;
            }
            '|' if i + 1 < chars.len() && chars[i + 1] == '|' => {
                out.push(ExprTok::OrOr);
                i += 2;
            }
            '"' | '\'' => {
                let quote = c;
                i += 1;
                let mut lit = String::new();
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    lit.push(chars[i]);
                    i += 1;
                }
                if i >= chars.len() {
                    return Err("Unterminated string literal".into());
                }
                i += 1;
                out.push(ExprTok::Str(lit));
            }
            d if d.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                let n: f64 = num_str
                    .parse()
                    .map_err(|_| format!("Invalid number '{num_str}'"))?;
                out.push(ExprTok::Num(n));
            }
            a if a.is_ascii_alphabetic() || a == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                match word.as_str() {
                    "true" => out.push(ExprTok::Bool(true)),
                    "false" => out.push(ExprTok::Bool(false)),
                    _ => out.push(ExprTok::Ident(word)),
                }
            }
            other => return Err(format!("Unsupported character '{other}' in expression")),
        }
    }
    Ok(out)
}

struct ExprParser<'a> {
    tokens: &'a [ExprTok],
    pos: usize,
    input: &'a Value,
}

impl<'a> ExprParser<'a> {
    fn peek(&self) -> Option<&ExprTok> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<ExprTok> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn parse_or(&mut self) -> Result<Value, String> {
        let mut lhs = self.parse_and()?;
        while matches!(self.peek(), Some(ExprTok::OrOr)) {
            self.next();
            let rhs = self.parse_and()?;
            lhs = Value::Bool(as_bool(&lhs) || as_bool(&rhs));
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Value, String> {
        let mut lhs = self.parse_cmp()?;
        while matches!(self.peek(), Some(ExprTok::AndAnd)) {
            self.next();
            let rhs = self.parse_cmp()?;
            lhs = Value::Bool(as_bool(&lhs) && as_bool(&rhs));
        }
        Ok(lhs)
    }

    fn parse_cmp(&mut self) -> Result<Value, String> {
        let lhs = self.parse_add()?;
        match self.peek() {
            Some(ExprTok::EqEq) => {
                self.next();
                let rhs = self.parse_add()?;
                Ok(Value::Bool(values_eq(&lhs, &rhs)))
            }
            Some(ExprTok::NotEq) => {
                self.next();
                let rhs = self.parse_add()?;
                Ok(Value::Bool(!values_eq(&lhs, &rhs)))
            }
            Some(ExprTok::Lt) => {
                self.next();
                Ok(Value::Bool(as_f64(&lhs)? < as_f64(&self.parse_add()?)?))
            }
            Some(ExprTok::Le) => {
                self.next();
                Ok(Value::Bool(as_f64(&lhs)? <= as_f64(&self.parse_add()?)?))
            }
            Some(ExprTok::Gt) => {
                self.next();
                Ok(Value::Bool(as_f64(&lhs)? > as_f64(&self.parse_add()?)?))
            }
            Some(ExprTok::Ge) => {
                self.next();
                Ok(Value::Bool(as_f64(&lhs)? >= as_f64(&self.parse_add()?)?))
            }
            _ => Ok(lhs),
        }
    }

    fn parse_add(&mut self) -> Result<Value, String> {
        let mut lhs = self.parse_mul()?;
        loop {
            match self.peek() {
                Some(ExprTok::Plus) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = match (&lhs, &rhs) {
                        (Value::String(a), Value::String(b)) => Value::String(format!("{a}{b}")),
                        _ => num_to_val(as_f64(&lhs)? + as_f64(&rhs)?)?,
                    };
                }
                Some(ExprTok::Minus) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = num_to_val(as_f64(&lhs)? - as_f64(&rhs)?)?;
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Value, String> {
        let mut lhs = self.parse_pow()?;
        loop {
            match self.peek() {
                Some(ExprTok::Star) => {
                    self.next();
                    let rhs = self.parse_pow()?;
                    lhs = num_to_val(as_f64(&lhs)? * as_f64(&rhs)?)?;
                }
                Some(ExprTok::Slash) => {
                    self.next();
                    let rhs = as_f64(&self.parse_pow()?)?;
                    if rhs == 0.0 {
                        return Err("Division by zero".into());
                    }
                    lhs = num_to_val(as_f64(&lhs)? / rhs)?;
                }
                Some(ExprTok::Percent) => {
                    self.next();
                    let rhs = as_f64(&self.parse_pow()?)?;
                    if rhs == 0.0 {
                        return Err("Modulo by zero".into());
                    }
                    lhs = num_to_val(as_f64(&lhs)? % rhs)?;
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_pow(&mut self) -> Result<Value, String> {
        let lhs = self.parse_unary()?;
        if matches!(self.peek(), Some(ExprTok::Caret)) {
            self.next();
            let rhs = self.parse_pow()?;
            return num_to_val(as_f64(&lhs)?.powf(as_f64(&rhs)?));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some(ExprTok::Minus) => {
                self.next();
                num_to_val(-as_f64(&self.parse_unary()?)?)
            }
            Some(ExprTok::Bang) => {
                self.next();
                Ok(Value::Bool(!as_bool(&self.parse_unary()?)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Value, String> {
        match self.next() {
            Some(ExprTok::Num(n)) => num_to_val(n),
            Some(ExprTok::Str(s)) => Ok(Value::String(s)),
            Some(ExprTok::Bool(b)) => Ok(Value::Bool(b)),
            Some(ExprTok::LParen) => {
                let v = self.parse_or()?;
                if !matches!(self.next(), Some(ExprTok::RParen)) {
                    return Err("Missing closing ')'".into());
                }
                Ok(v)
            }
            Some(ExprTok::Ident(name)) => {
                if matches!(self.peek(), Some(ExprTok::LParen)) {
                    self.next();
                    let mut fn_args = Vec::new();
                    if !matches!(self.peek(), Some(ExprTok::RParen)) {
                        loop {
                            fn_args.push(self.parse_or()?);
                            if matches!(self.peek(), Some(ExprTok::Comma)) {
                                self.next();
                            } else {
                                break;
                            }
                        }
                    }
                    if !matches!(self.next(), Some(ExprTok::RParen)) {
                        return Err(format!("Missing ')' after call to '{name}'"));
                    }
                    eval_builtin_fn(&name, &fn_args)
                } else if name == "pi" || name == "PI" {
                    num_to_val(std::f64::consts::PI)
                } else if name == "e" || name == "E" {
                    num_to_val(std::f64::consts::E)
                } else if name == "input" {
                    Ok(self.input.clone())
                } else if let Some(field) = name.strip_prefix("input.") {
                    let mut cur = self.input;
                    for seg in field.split('.') {
                        cur = cur.get(seg).unwrap_or(&Value::Null);
                    }
                    Ok(cur.clone())
                } else if let Some(v) = self.input.get(&name) {
                    Ok(v.clone())
                } else {
                    Err(format!("Unknown identifier '{name}'"))
                }
            }
            other => Err(format!("Unexpected token in expression: {other:?}")),
        }
    }
}

fn eval_builtin_fn(name: &str, args: &[Value]) -> Result<Value, String> {
    match name {
        "sqrt" => num_to_val(as_f64(args.first().ok_or("sqrt needs 1 arg")?)?.sqrt()),
        "abs" => num_to_val(as_f64(args.first().ok_or("abs needs 1 arg")?)?.abs()),
        "floor" => num_to_val(as_f64(args.first().ok_or("floor needs 1 arg")?)?.floor()),
        "ceil" => num_to_val(as_f64(args.first().ok_or("ceil needs 1 arg")?)?.ceil()),
        "round" => num_to_val(as_f64(args.first().ok_or("round needs 1 arg")?)?.round()),
        "sin" => num_to_val(as_f64(args.first().ok_or("sin needs 1 arg")?)?.sin()),
        "cos" => num_to_val(as_f64(args.first().ok_or("cos needs 1 arg")?)?.cos()),
        "pow" => {
            if args.len() != 2 {
                return Err("pow requires 2 arguments".into());
            }
            num_to_val(as_f64(&args[0])?.powf(as_f64(&args[1])?))
        }
        "min" => {
            let mut m = f64::INFINITY;
            for a in args {
                m = m.min(as_f64(a)?);
            }
            num_to_val(m)
        }
        "max" => {
            let mut m = f64::NEG_INFINITY;
            for a in args {
                m = m.max(as_f64(a)?);
            }
            num_to_val(m)
        }
        "len" => match args.first() {
            Some(Value::String(s)) => Ok(json!(s.chars().count())),
            Some(Value::Array(a)) => Ok(json!(a.len())),
            Some(Value::Object(o)) => Ok(json!(o.len())),
            _ => Err("len expects string, array, or object".into()),
        },
        _ => Err(format!("Unknown sandbox function '{name}'")),
    }
}

fn as_f64(v: &Value) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| format!("Expected number, got {v}"))
}

fn as_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn values_eq(a: &Value, b: &Value) -> bool {
    if let (Some(fa), Some(fb)) = (a.as_f64(), b.as_f64()) {
        (fa - fb).abs() < 1e-12
    } else {
        a == b
    }
}

fn num_to_val(n: f64) -> Result<Value, String> {
    if !n.is_finite() {
        return Err("Numeric result is non-finite".into());
    }
    if (n - n.trunc()).abs() < 1e-12 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
        Ok(json!(n as i64))
    } else {
        Ok(json!(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::security::ApprovalMode;

    #[test]
    fn test_all_6_terminal_backend_command_builders() {
        let cwd = Path::new("/tmp/ws");
        for kind in TerminalBackendKind::ALL {
            let mut cfg = TerminalBackendConfig::default();
            cfg.kind = kind;
            cfg.docker_container = Some("hermes-box".into());
            cfg.ssh_host = Some("example.internal".into());
            cfg.singularity_image = Some("/opt/img.sif".into());
            cfg.modal_sandbox_id = Some("sb_123".into());
            cfg.daytona_workspace = Some("ws_abc".into());

            let backend = ConfiguredTerminalBackend::new(cfg);
            let spec = backend.build_command_spec("echo ok", cwd).unwrap();
            assert_eq!(spec.backend, kind);
            assert!(!spec.program.is_empty());
        }

        // Unconfigured docker returns clean diagnostic error rather than panicking
        let mut unconf = TerminalBackendConfig::default();
        unconf.kind = TerminalBackendKind::Docker;
        unconf.docker_container = None;
        unconf.docker_image = None;
        assert!(ConfiguredTerminalBackend::new(unconf)
            .build_command_spec("echo ok", cwd)
            .is_err());
    }

    #[tokio::test]
    async fn test_terminal_danger_gate_and_background_process_registry() {
        let tmp = std::env::temp_dir().join(format!("hermes_term_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&tmp).unwrap();
        let ctx = HermesToolContext::new(&tmp).with_approval_mode(ApprovalMode::Prompt);

        // Dangerous command blocked pending approval
        let danger = HermesToolExecutor::execute(
            "terminal",
            json!({ "command": "rm -rf /tmp/never_run" }),
            &ctx,
        )
        .await;
        assert!(!danger.success);
        assert!(!danger.inline_buttons.is_empty());

        // Safe foreground command runs
        let fg = HermesToolExecutor::execute(
            "terminal",
            json!({ "command": "echo hermes_live_ok" }),
            &ctx,
        )
        .await;
        assert!(fg.success, "{}", fg.output);
        assert!(fg.output.contains("hermes_live_ok"));

        // Live background process poll + write + kill
        let bg = HermesToolExecutor::execute(
            "terminal",
            json!({ "command": "cat", "background": true }),
            &ctx,
        )
        .await;
        assert!(bg.success, "{}", bg.output);
        let pid = bg.structured.unwrap()["process_id"].as_str().unwrap().to_string();

        let wr = HermesToolExecutor::execute(
            "process_write",
            json!({ "process_id": pid, "input": "ping_bg\n" }),
            &ctx,
        )
        .await;
        assert!(wr.success);

        let poll = HermesToolExecutor::execute(
            "process_poll",
            json!({ "process_id": pid, "wait_ms": 80 }),
            &ctx,
        )
        .await;
        assert!(poll.success);
        assert!(poll.output.contains("ping_bg"));

        let kill = HermesToolExecutor::execute(
            "process_kill",
            json!({ "process_id": pid }),
            &ctx,
        )
        .await;
        assert!(kill.success);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_execute_code_pure_rust_eval() {
        let ctx = HermesToolContext::new(".");
        let res = HermesToolExecutor::execute(
            "execute_code",
            json!({
                "language": "expr",
                "code": "sqrt(144) + pow(2, 5) + input.x",
                "input": { "x": 6 }
            }),
            &ctx,
        )
        .await;
        assert!(res.success, "{}", res.output);
        assert_eq!(res.output, "50");
    }
}
