use crate::box_shell_command::{
    HostShellArgs, HostShellArgsInput, build_host_shell_args,
};

pub const TRANSFER_TOOL_CALL_ID: &str = "sand-box-file-transfer";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteShellResult {
    Success { exit_code: i32, stderr: String },
    Failure {
        exit_code: i32,
        signal: String,
        stderr: String,
        aborted: bool,
    },
    SpawnError(String),
    PermissionDenied(String),
    Rejected(String),
    Timeout(u64),
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandBoxFileTransferError {
    Transfer(String),
    SignalKilled(String),
    ShellUnavailable(String),
}

pub fn is_signal_kill_failure(exit_code: i32, signal: &str) -> bool {
    !signal.is_empty() || exit_code < 0
}

pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn describe_shell_failure(result: &RemoteShellResult) -> String {
    match result {
        RemoteShellResult::Failure {
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
            if stderr.is_empty() { head } else { format!("{head}: {stderr}") }
        }
        RemoteShellResult::SpawnError(error) => {
            format!("exec-daemon spawn error: {error}")
        }
        RemoteShellResult::PermissionDenied(error) => {
            format!("permission denied: {error}")
        }
        RemoteShellResult::Rejected(reason) => {
            format!("exec-daemon rejected the command: {reason}")
        }
        RemoteShellResult::Timeout(ms) => {
            format!("exec-daemon timed out after {ms}ms")
        }
        RemoteShellResult::Success { exit_code, stderr } => {
            format!("exit {exit_code}: {stderr}")
        }
        RemoteShellResult::Missing => "unknown (no result from exec-daemon)".into(),
    }
}

pub fn as_transient_box_shell_error(
    result: &RemoteShellResult,
    context: &str,
) -> Option<SandBoxFileTransferError> {
    match result {
        RemoteShellResult::Failure {
            exit_code,
            signal,
            ..
        } if is_signal_kill_failure(*exit_code, signal) => {
            Some(SandBoxFileTransferError::SignalKilled(format!(
                "{context} signal-killed ({})",
                describe_shell_failure(result)
            )))
        }
        RemoteShellResult::SpawnError(_)
        | RemoteShellResult::Timeout(_)
        | RemoteShellResult::Rejected(_) => {
            Some(SandBoxFileTransferError::ShellUnavailable(format!(
                "{context} ({})",
                describe_shell_failure(result)
            )))
        }
        _ => None,
    }
}

pub trait FileTransferRemoteRunner<Context> {
    type Error: std::fmt::Display;

    async fn execute_shell(
        &self,
        context: &Context,
        args: HostShellArgs,
    ) -> Result<RemoteShellResult, Self::Error>;

    async fn write_file_bytes(
        &self,
        context: &Context,
        box_path: &str,
        data: &[u8],
        tool_call_id: &str,
    ) -> Result<(), Self::Error>;
}

pub async fn run_box_shell<Context, Runner>(
    context: &Context,
    runner: &Runner,
    script: &str,
) -> Result<(), SandBoxFileTransferError>
where
    Runner: FileTransferRemoteRunner<Context>,
{
    let command = format!("bash -lc {}", shell_single_quote(script));
    let result = runner
        .execute_shell(
            context,
            build_host_shell_args(HostShellArgsInput {
                command,
                name: "bash".into(),
                working_directory: "/".into(),
                tool_call_id: TRANSFER_TOOL_CALL_ID.into(),
            }),
        )
        .await
        .map_err(|error| {
            SandBoxFileTransferError::ShellUnavailable(error.to_string())
        })?;

    if matches!(
        result,
        RemoteShellResult::Success {
            exit_code: 0,
            ..
        }
    ) {
        return Ok(());
    }
    if let Some(transient) =
        as_transient_box_shell_error(&result, "box shell command")
    {
        return Err(transient);
    }
    Err(SandBoxFileTransferError::Transfer(format!(
        "box shell command failed ({})",
        describe_shell_failure(&result)
    )))
}

pub async fn write_file_bytes_via_exec_daemon<Context, Runner>(
    context: &Context,
    runner: &Runner,
    box_path: &str,
    data: &[u8],
) -> Result<(), SandBoxFileTransferError>
where
    Runner: FileTransferRemoteRunner<Context>,
{
    runner
        .write_file_bytes(context, box_path, data, TRANSFER_TOOL_CALL_ID)
        .await
        .map_err(|error| {
            SandBoxFileTransferError::Transfer(format!(
                "upload to box {box_path} failed: {error}"
            ))
        })
}

fn parent_posix(path: &str) -> &str {
    path.rsplit_once('/').map(|(parent, _)| {
        if parent.is_empty() { "/" } else { parent }
    }).unwrap_or(".")
}

pub async fn upload_file_via_exec_daemon<Context, Runner>(
    context: &Context,
    runner: &Runner,
    box_path: &str,
    data: &[u8],
    part_nonce: &str,
) -> Result<(), SandBoxFileTransferError>
where
    Runner: FileTransferRemoteRunner<Context>,
{
    run_box_shell(
        context,
        runner,
        &format!(
            "mkdir -p -- {}",
            shell_single_quote(parent_posix(box_path))
        ),
    )
    .await?;

    let part = format!("{box_path}.sand-{part_nonce}.part");
    if let Err(error) =
        write_file_bytes_via_exec_daemon(context, runner, &part, data).await
    {
        let _ = run_box_shell(
            context,
            runner,
            &format!("rm -f -- {}", shell_single_quote(&part)),
        )
        .await;
        return Err(error);
    }

    run_box_shell(
        context,
        runner,
        &format!(
            "mv -f -- {} {}",
            shell_single_quote(&part),
            shell_single_quote(box_path)
        ),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::Mutex;

    struct Runner {
        shell: Mutex<Vec<HostShellArgs>>,
        writes: Mutex<Vec<(String, Vec<u8>, String)>>,
    }

    impl FileTransferRemoteRunner<()> for Runner {
        type Error = Infallible;

        async fn execute_shell(
            &self,
            _context: &(),
            args: HostShellArgs,
        ) -> Result<RemoteShellResult, Self::Error> {
            self.shell.lock().unwrap().push(args);
            Ok(RemoteShellResult::Success {
                exit_code: 0,
                stderr: String::new(),
            })
        }

        async fn write_file_bytes(
            &self,
            _context: &(),
            box_path: &str,
            data: &[u8],
            tool_call_id: &str,
        ) -> Result<(), Self::Error> {
            self.writes.lock().unwrap().push((
                box_path.into(),
                data.to_vec(),
                tool_call_id.into(),
            ));
            Ok(())
        }
    }

    #[test]
    fn quotes_shell_values_and_classifies_transient_failures() {
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
        assert!(matches!(
            as_transient_box_shell_error(
                &RemoteShellResult::Failure {
                    exit_code: -1,
                    signal: "KILL".into(),
                    stderr: String::new(),
                    aborted: false,
                },
                "transfer"
            ),
            Some(SandBoxFileTransferError::SignalKilled(_))
        ));
        assert!(matches!(
            as_transient_box_shell_error(
                &RemoteShellResult::Timeout(500),
                "transfer"
            ),
            Some(SandBoxFileTransferError::ShellUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn upload_uses_part_file_then_atomic_remote_move() {
        let runner = Runner {
            shell: Mutex::new(Vec::new()),
            writes: Mutex::new(Vec::new()),
        };
        upload_file_via_exec_daemon(
            &(),
            &runner,
            "/workspace/uploads/a.txt",
            b"hello",
            "deadbeef",
        )
        .await
        .unwrap();

        let writes = runner.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(
            writes[0].0,
            "/workspace/uploads/a.txt.sand-deadbeef.part"
        );
        assert_eq!(writes[0].1, b"hello");
        assert_eq!(writes[0].2, TRANSFER_TOOL_CALL_ID);
        let shell = runner.shell.lock().unwrap();
        assert_eq!(shell.len(), 2);
        assert!(shell[0].command.contains("mkdir -p"));
        assert!(shell[1].command.contains("mv -f"));
    }
}
