use std::io;

use mahayana_host_runtime::r#box::box_file_transfer::{
    BoxFileTransferOperationError, BoxShellTransientError, FileTransferAccessor,
    ShellExecResult, TRANSFER_TOOL_CALL_ID, WriteExecResult, as_transient_box_shell_error,
    describe_shell_failure, is_signal_kill_failure, run_box_shell, shell_single_quote,
    upload_file_via_exec_daemon,
};
use mahayana_host_runtime::r#box::box_shell_command::HostShellArgs;

struct RecordingTransferAccessor {
    shells: Vec<HostShellArgs>,
    writes: Vec<(String, Vec<u8>, String)>,
    shell_results: Vec<ShellExecResult>,
    write_result: WriteExecResult,
}

impl RecordingTransferAccessor {
    fn successful() -> Self {
        Self {
            shells: Vec::new(),
            writes: Vec::new(),
            shell_results: Vec::new(),
            write_result: WriteExecResult::Success,
        }
    }
}

impl FileTransferAccessor<String> for RecordingTransferAccessor {
    type Error = io::Error;

    fn execute_shell(
        &mut self,
        _ctx: &String,
        args: HostShellArgs,
    ) -> Result<ShellExecResult, Self::Error> {
        self.shells.push(args);
        if self.shell_results.is_empty() {
            Ok(ShellExecResult::Success { exit_code: 0 })
        } else {
            Ok(self.shell_results.remove(0))
        }
    }

    fn execute_write(
        &mut self,
        _ctx: &String,
        path: &str,
        file_bytes: &[u8],
        tool_call_id: &str,
    ) -> Result<WriteExecResult, Self::Error> {
        self.writes
            .push((path.into(), file_bytes.to_vec(), tool_call_id.into()));
        Ok(self.write_result.clone())
    }
}

#[test]
fn box_file_transfer_quotes_shell_and_classifies_transient_failures() {
    assert_eq!(shell_single_quote("abc"), "'abc'");
    assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    assert!(is_signal_kill_failure(-1, ""));
    assert!(is_signal_kill_failure(1, "SIGKILL"));
    assert!(!is_signal_kill_failure(1, ""));

    let killed = ShellExecResult::Failure {
        exit_code: -1,
        signal: "SIGKILL".into(),
        stderr: "killed".into(),
        aborted: true,
    };
    assert_eq!(
        describe_shell_failure(&killed),
        "killed by signal SIGKILL (aborted): killed"
    );
    assert!(matches!(
        as_transient_box_shell_error(&killed, "box shell command"),
        Some(BoxShellTransientError::SignalKilled(_))
    ));
    assert!(matches!(
        as_transient_box_shell_error(
            &ShellExecResult::Timeout { timeout_ms: 1500 },
            "box shell command"
        ),
        Some(BoxShellTransientError::Unavailable(_))
    ));
}

#[test]
fn box_file_transfer_runs_shell_through_host_shell_args_contract() {
    let mut accessor = RecordingTransferAccessor::successful();
    run_box_shell(&"ctx".into(), &mut accessor, "mkdir -p -- '/workspace/a'")
        .expect("box shell succeeds");
    assert_eq!(accessor.shells.len(), 1);
    let args = &accessor.shells[0];
    assert_eq!(
        args.command,
        "bash -lc 'mkdir -p -- '\\''/workspace/a'\\'''"
    );
    assert_eq!(args.parsing_result.executable_commands.len(), 1);
    assert_eq!(args.parsing_result.executable_commands[0].name, "bash");
    assert_eq!(
        args.parsing_result.executable_commands[0].full_text,
        args.command
    );
    assert_eq!(args.working_directory, "/");
    assert_eq!(args.tool_call_id, TRANSFER_TOOL_CALL_ID);
    assert!(args.skip_approval);
}

#[test]
fn box_file_upload_uses_part_write_atomic_move_and_cleanup_on_write_failure() {
    let mut accessor = RecordingTransferAccessor::successful();
    upload_file_via_exec_daemon(
        &"ctx".into(),
        &mut accessor,
        "/workspace/uploads/report.txt",
        b"hello",
    )
    .expect("upload should succeed");

    assert_eq!(accessor.shells.len(), 2);
    assert!(accessor.shells[0].command.contains("mkdir -p --"));
    assert!(accessor.shells[0].command.contains("/workspace/uploads"));
    assert_eq!(accessor.writes.len(), 1);
    assert!(accessor.writes[0].0.starts_with("/workspace/uploads/report.txt.sand-"));
    assert!(accessor.writes[0].0.ends_with(".part"));
    assert_eq!(accessor.writes[0].1, b"hello");
    assert_eq!(accessor.writes[0].2, TRANSFER_TOOL_CALL_ID);
    assert!(accessor.shells[1].command.contains("mv -f --"));
    assert!(accessor.shells[1].command.contains("/workspace/uploads/report.txt"));

    let mut failing = RecordingTransferAccessor::successful();
    failing.write_result = WriteExecResult::Rejected {
        reason: "read only".into(),
    };
    let error = upload_file_via_exec_daemon(
        &"ctx".into(),
        &mut failing,
        "/workspace/uploads/report.txt",
        b"hello",
    )
    .expect_err("write rejection must fail");
    assert!(matches!(error, BoxFileTransferOperationError::Transfer(_)));
    assert_eq!(failing.shells.len(), 2);
    assert!(failing.shells[1].command.contains("rm -f --"));
}
