use std::fmt;

use thiserror::Error;
use uuid::Uuid;

use super::box_shell_command::{HostShellArgs, HostShellArgsInput, build_host_shell_args};

pub const TRANSFER_TOOL_CALL_ID: &str = "sand-box-file-transfer";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct SandBoxFileTransferError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxShellTransientError {
    SignalKilled(String),
    Unavailable(String),
}

impl fmt::Display for BoxShellTransientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignalKilled(message) | Self::Unavailable(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for BoxShellTransientError {}

pub fn is_signal_kill_failure(exit_code: i32, signal: &str) -> bool {
    !signal.is_empty() || exit_code < 0
}

pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace("'", "'\\''"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellExecResult {
    Success {
        exit_code: i32,
    },
    Failure {
        exit_code: i32,
        signal: String,
        stderr: String,
        aborted: bool,
    },
    SpawnError {
        error: String,
    },
    PermissionDenied {
        error: String,
    },
    Rejected {
        reason: String,
    },
    Timeout {
        timeout_ms: u64,
    },
    Other {
        case: String,
    },
}

pub fn describe_shell_failure(result: &ShellExecResult) -> String {
    match result {
        ShellExecResult::Failure {
            exit_code,
            signal,
            stderr,
            aborted,
        } => {
            let head = if *exit_code < 0 || !signal.is_empty() {
                format!(
                    "killed by signal {}{}",
                    if signal.is_empty() { "(unknown)" } else { signal },
                    if *aborted { " (aborted)" } else { "" }
                )
            } else {
                format!(
                    "exit {exit_code}{}",
                    if *aborted { " (aborted)" } else { "" }
                )
            };
            if stderr.is_empty() {
                head
            } else {
                format!("{head}: {stderr}")
            }
        }
        ShellExecResult::SpawnError { error } => {
            format!("exec-daemon spawn error: {error}")
        }
        ShellExecResult::PermissionDenied { error } => {
            format!("permission denied: {error}")
        }
        ShellExecResult::Rejected { reason } => {
            format!("exec-daemon rejected the command: {reason}")
        }
        ShellExecResult::Timeout { timeout_ms } => {
            format!("exec-daemon timed out after {timeout_ms}ms")
        }
        ShellExecResult::Success { .. } => "success".into(),
        ShellExecResult::Other { case } if !case.is_empty() => case.clone(),
        ShellExecResult::Other { .. } => "unknown (no result from exec-daemon)".into(),
    }
}

pub fn as_transient_box_shell_error(
    result: &ShellExecResult,
    context: &str,
) -> Option<BoxShellTransientError> {
    match result {
        ShellExecResult::Failure {
            exit_code, signal, ..
        } if is_signal_kill_failure(*exit_code, signal) => {
            Some(BoxShellTransientError::SignalKilled(format!(
                "{context} signal-killed ({})",
                describe_shell_failure(result)
            )))
        }
        ShellExecResult::SpawnError { .. }
        | ShellExecResult::Timeout { .. }
        | ShellExecResult::Rejected { .. } => {
            Some(BoxShellTransientError::Unavailable(format!(
                "{context} ({})",
                describe_shell_failure(result)
            )))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteExecResult {
    Success,
    Error { error: String },
    Rejected { reason: String },
    Other { case: String },
}

pub trait FileTransferAccessor<Ctx> {
    type Error;

    fn execute_shell(
        &mut self,
        ctx: &Ctx,
        args: HostShellArgs,
    ) -> Result<ShellExecResult, Self::Error>;

    fn execute_write(
        &mut self,
        ctx: &Ctx,
        path: &str,
        file_bytes: &[u8],
        tool_call_id: &str,
    ) -> Result<WriteExecResult, Self::Error>;
}

#[derive(Debug)]
pub enum BoxFileTransferOperationError<E> {
    Transport(E),
    Transient(BoxShellTransientError),
    Transfer(SandBoxFileTransferError),
}

impl<E: fmt::Display> fmt::Display for BoxFileTransferOperationError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(formatter),
            Self::Transient(error) => error.fmt(formatter),
            Self::Transfer(error) => error.fmt(formatter),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for BoxFileTransferOperationError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Transient(error) => Some(error),
            Self::Transfer(error) => Some(error),
        }
    }
}

pub fn run_box_shell<Ctx, Accessor>(
    ctx: &Ctx,
    accessor: &mut Accessor,
    script: &str,
) -> Result<(), BoxFileTransferOperationError<Accessor::Error>>
where
    Accessor: FileTransferAccessor<Ctx>,
{
    let command = format!("bash -lc {}", shell_single_quote(script));
    let result = accessor
        .execute_shell(
            ctx,
            build_host_shell_args(HostShellArgsInput {
                command,
                name: "bash".into(),
                working_directory: "/".into(),
                tool_call_id: TRANSFER_TOOL_CALL_ID.into(),
            }),
        )
        .map_err(BoxFileTransferOperationError::Transport)?;

    if matches!(result, ShellExecResult::Success { exit_code: 0 }) {
        return Ok(());
    }
    if let Some(error) = as_transient_box_shell_error(&result, "box shell command") {
        return Err(BoxFileTransferOperationError::Transient(error));
    }
    Err(BoxFileTransferOperationError::Transfer(
        SandBoxFileTransferError(format!(
            "box shell command failed ({})",
            describe_shell_failure(&result)
        )),
    ))
}

pub fn write_file_bytes_via_exec_daemon<Ctx, Accessor>(
    ctx: &Ctx,
    accessor: &mut Accessor,
    box_path: &str,
    data: &[u8],
) -> Result<(), BoxFileTransferOperationError<Accessor::Error>>
where
    Accessor: FileTransferAccessor<Ctx>,
{
    let result = accessor
        .execute_write(ctx, box_path, data, TRANSFER_TOOL_CALL_ID)
        .map_err(BoxFileTransferOperationError::Transport)?;
    match result {
        WriteExecResult::Success => Ok(()),
        WriteExecResult::Error { error } => Err(BoxFileTransferOperationError::Transfer(
            SandBoxFileTransferError(format!(
                "upload to box {box_path} failed (error): {error}"
            )),
        )),
        WriteExecResult::Rejected { reason } => Err(BoxFileTransferOperationError::Transfer(
            SandBoxFileTransferError(format!(
                "upload to box {box_path} failed (rejected): {reason}"
            )),
        )),
        WriteExecResult::Other { case } => Err(BoxFileTransferOperationError::Transfer(
            SandBoxFileTransferError(format!(
                "upload to box {box_path} failed ({case}): {case}"
            )),
        )),
    }
}

fn posix_dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".into();
    }
    match trimmed.rsplit_once('/') {
        Some(("", _)) => "/".into(),
        Some((parent, _)) if !parent.is_empty() => parent.into(),
        _ => ".".into(),
    }
}

fn random_part_suffix() -> String {
    Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(16)
        .collect()
}

pub fn upload_file_via_exec_daemon<Ctx, Accessor>(
    ctx: &Ctx,
    accessor: &mut Accessor,
    box_path: &str,
    data: &[u8],
) -> Result<(), BoxFileTransferOperationError<Accessor::Error>>
where
    Accessor: FileTransferAccessor<Ctx>,
{
    run_box_shell(
        ctx,
        accessor,
        &format!(
            "mkdir -p -- {}",
            shell_single_quote(&posix_dirname(box_path))
        ),
    )?;
    let part = format!("{box_path}.sand-{}.part", random_part_suffix());
    if let Err(error) = write_file_bytes_via_exec_daemon(ctx, accessor, &part, data) {
        let _ = run_box_shell(
            ctx,
            accessor,
            &format!("rm -f -- {}", shell_single_quote(&part)),
        );
        return Err(error);
    }
    run_box_shell(
        ctx,
        accessor,
        &format!(
            "mv -f -- {} {}",
            shell_single_quote(&part),
            shell_single_quote(box_path)
        ),
    )
}
