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
pub enum RemoteWriteResult {
    Success,
    Error(String),
    Rejected(String),
    Other(String),
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

pub fn describe_write_failure(result: &RemoteWriteResult) -> String {
    match result {
        RemoteWriteResult::Success => "success".into(),
        RemoteWriteResult::Error(error) => format!("error: {error}"),
        RemoteWriteResult::Rejected(reason) => format!("rejected: {reason}"),
        RemoteWriteResult::Other(case) if !case.is_empty() => {
            format!("{case}: {case}")
        }
        RemoteWriteResult::Other(_) => "unknown: unknown".into(),
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
    ) -> Result<RemoteWriteResult, Self::Error>;
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
    let result = runner
        .write_file_bytes(context, box_path, data, TRANSFER_TOOL_CALL_ID)
        .await
        .map_err(|error| {
            SandBoxFileTransferError::Transfer(format!(
                "upload to box {box_path} failed: {error}"
            ))
        })?;
    match result {
        RemoteWriteResult::Success => Ok(()),
        RemoteWriteResult::Error(error) => {
            Err(SandBoxFileTransferError::Transfer(format!(
                "upload to box {box_path} failed (error): {error}"
            )))
        }
        RemoteWriteResult::Rejected(reason) => {
            Err(SandBoxFileTransferError::Transfer(format!(
                "upload to box {box_path} failed (rejected): {reason}"
            )))
        }
        RemoteWriteResult::Other(case) => {
            let case = if case.is_empty() { "unknown".into() } else { case };
            Err(SandBoxFileTransferError::Transfer(format!(
                "upload to box {box_path} failed ({case}): {case}"
            )))
        }
    }
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

    if let Err(error) = run_box_shell(
        context,
        runner,
        &format!(
            "mv -f -- {} {}",
            shell_single_quote(&part),
            shell_single_quote(box_path)
        ),
    )
    .await
    {
        let _ = run_box_shell(
            context,
            runner,
            &format!("rm -f -- {}", shell_single_quote(&part)),
        )
        .await;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct Runner {
        shell: Mutex<Vec<HostShellArgs>>,
        shell_results: Mutex<VecDeque<RemoteShellResult>>,
        writes: Mutex<Vec<(String, Vec<u8>, String)>>,
        write_result: Mutex<RemoteWriteResult>,
    }

    impl Runner {
        fn successful() -> Self {
            Self {
                shell: Mutex::new(Vec::new()),
                shell_results: Mutex::new(VecDeque::new()),
                writes: Mutex::new(Vec::new()),
                write_result: Mutex::new(RemoteWriteResult::Success),
            }
        }
    }

    impl FileTransferRemoteRunner<()> for Runner {
        type Error = Infallible;

        async fn execute_shell(
            &self,
            _context: &(),
            args: HostShellArgs,
        ) -> Result<RemoteShellResult, Self::Error> {
            self.shell.lock().unwrap().push(args);
            Ok(self
                .shell_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(RemoteShellResult::Success {
                    exit_code: 0,
                    stderr: String::new(),
                }))
        }

        async fn write_file_bytes(
            &self,
            _context: &(),
            box_path: &str,
            data: &[u8],
            tool_call_id: &str,
        ) -> Result<RemoteWriteResult, Self::Error> {
            self.writes.lock().unwrap().push((
                box_path.into(),
                data.to_vec(),
                tool_call_id.into(),
            ));
            Ok(self.write_result.lock().unwrap().clone())
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
    async fn write_preserves_typed_remote_result_branches() {
        for (result, marker) in [
            (RemoteWriteResult::Error("disk full".into()), "(error): disk full"),
            (RemoteWriteResult::Rejected("policy".into()), "(rejected): policy"),
            (RemoteWriteResult::Other("permission-denied".into()), "(permission-denied): permission-denied"),
        ] {
            let runner = Runner::successful();
            *runner.write_result.lock().unwrap() = result;
            let error = write_file_bytes_via_exec_daemon(
                &(),
                &runner,
                "/workspace/a",
                b"x",
            )
            .await
            .unwrap_err();
            assert!(matches!(
                error,
                SandBoxFileTransferError::Transfer(message)
                    if message.contains(marker)
            ));
        }
    }

    #[tokio::test]
    async fn upload_uses_part_file_then_atomic_remote_move() {
        let runner = Runner::successful();
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

    #[tokio::test]
    async fn typed_write_failure_cleans_part_file() {
        let runner = Runner::successful();
        *runner.write_result.lock().unwrap() =
            RemoteWriteResult::Rejected("policy".into());
        let error = upload_file_via_exec_daemon(
            &(),
            &runner,
            "/workspace/uploads/a.txt",
            b"hello",
            "deadbeef",
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            SandBoxFileTransferError::Transfer(message)
                if message.contains("(rejected): policy")
        ));
        let shell = runner.shell.lock().unwrap();
        assert_eq!(shell.len(), 2);
        assert!(shell[1].command.contains("rm -f"));
        assert!(shell[1].command.contains(".sand-deadbeef.part"));
    }

    #[tokio::test]
    async fn atomic_move_failure_also_cleans_part_file() {
        let runner = Runner::successful();
        runner.shell_results.lock().unwrap().extend([
            RemoteShellResult::Success {
                exit_code: 0,
                stderr: String::new(),
            },
            RemoteShellResult::Failure {
                exit_code: 1,
                signal: String::new(),
                stderr: "rename failed".into(),
                aborted: false,
            },
            RemoteShellResult::Success {
                exit_code: 0,
                stderr: String::new(),
            },
        ]);
        assert!(upload_file_via_exec_daemon(
            &(),
            &runner,
            "/workspace/uploads/a.txt",
            b"hello",
            "deadbeef",
        )
        .await
        .is_err());
        let shell = runner.shell.lock().unwrap();
        assert_eq!(shell.len(), 3);
        assert!(shell[1].command.contains("mv -f"));
        assert!(shell[2].command.contains("rm -f"));
    }
}
