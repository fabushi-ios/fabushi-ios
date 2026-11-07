use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::process::{Child, Command};
use std::time::Duration;

use mahayana_host_runtime::extensions::local_exec::gateway_local_exec_sand_box::{
    DEFAULT_MAX_LOCAL_EXEC_FILE_BYTES, GatewayLocalExecFailureReport,
    GatewayLocalExecOperationStatus, GatewayLocalExecSandBox,
    GatewayLocalToolGate, GatewayLocalToolScope,
    create_bridge_user_computers, local_exec_file_too_large_message,
};
use mahayana_host_runtime::extensions::local_exec::local_exec_failure_classifier::LocalExecFailureClass;
use mahayana_host_runtime::extensions::local_exec::local_exec_bridge::SandLocalExecBridge;
use mahayana_host_runtime::extensions::local_exec::local_exec_error::SandLocalExecError;
use serde_json::json;

#[derive(Default)]
struct FakeGate {
    blocked: Mutex<Option<String>>,
    calls: Mutex<Vec<(Option<String>, String, String)>>,
}

impl GatewayLocalToolGate for FakeGate {
    fn blocked_reason(&self) -> Option<String> {
        self.blocked.lock().expect("blocked").clone()
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn authorize(
        &self,
        scope: &GatewayLocalToolScope,
        action: &str,
        target: &str,
    ) -> Result<Option<String>, SandLocalExecError> {
        self.calls.lock().expect("calls").push((
            scope.agent_id.clone(),
            action.to_string(),
            target.to_string(),
        ));
        Ok(Some("approval-1".into()))
    }
}

fn bridge_with_provider() -> (
    SandLocalExecBridge,
    mahayana_host_runtime::extensions::local_exec::local_exec_bridge::LocalExecProviderRegistration,
    mpsc::Receiver<serde_json::Value>,
) {
    let bridge = SandLocalExecBridge::with_sources(
        Arc::new(|| 1000),
        Arc::new(|| "provider-1".to_string()),
    );
    let (send, receive) = mpsc::channel();
    let registration = bridge.register_provider(send);
    let _ = receive.recv_timeout(Duration::from_millis(100)).expect("welcome");
    bridge.submit_responses(json!({
        "providerId": "provider-1",
        "frames": [{
            "kind": "hello",
            "localRoot": "/Users/test",
            "terminalsFolder": "terminal-files",
            "computerId": "mac-1",
            "label": "My Mac"
        }]
    }));
    (bridge, registration, receive)
}

#[test]
fn sandbox_projects_provider_state_and_user_computers() {
    let (bridge, _registration, _receive) = bridge_with_provider();
    let gate = Arc::new(FakeGate::default());
    let sandbox = GatewayLocalExecSandBox::new(bridge.clone(), gate.clone());
    assert_eq!(sandbox.run_state(), "running");
    assert_eq!(sandbox.ensure_ready().terminals_folder, "terminal-files");
    assert!(sandbox.list_boxes().is_empty());

    let computers = create_bridge_user_computers(bridge, gate);
    assert_eq!(computers.list().len(), 1);
    let (computer, selected) = computers.resolve(Some("mac-1")).expect("computer");
    assert_eq!(computer.label, "My Mac");
    assert_eq!(selected.run_state(), "running");
}

#[test]
fn upload_and_download_enforce_gate_and_use_frozen_frames() {
    let (bridge, _registration, receive) = bridge_with_provider();
    let gate = Arc::new(FakeGate::default());
    let sandbox = GatewayLocalExecSandBox::new(bridge.clone(), gate.clone());

    let upload_box = sandbox.clone();
    let upload = thread::spawn(move || {
        upload_box.upload_file(Some("agent-1"), "/tmp/a.txt", b"hello")
    });

    let upload_start = receive.recv_timeout(Duration::from_secs(1)).expect("upload-start frame");
    assert_eq!(upload_start["kind"], "upload-start");
    assert_eq!(upload_start["path"], "/tmp/a.txt");
    assert_eq!(upload_start["totalBytes"], 5);
    assert_eq!(upload_start["approvalId"], "approval-1");
    let transfer_id = upload_start["transferId"]
        .as_str()
        .expect("upload-start transferId")
        .to_string();
    assert_eq!(upload_start["sha256"], "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    bridge.submit_responses(json!({
        "frames": [{
            "kind": "file",
            "requestId": upload_start["requestId"]
        }]
    }));
    let start_cancel = receive.recv_timeout(Duration::from_secs(1)).expect("upload-start cleanup cancel");
    assert_eq!(start_cancel["kind"], "cancel");
    assert_eq!(start_cancel["requestId"], upload_start["requestId"]);

    let upload_chunk = receive.recv_timeout(Duration::from_secs(1)).expect("upload-chunk frame");
    assert_eq!(upload_chunk["kind"], "upload-chunk");
    assert_eq!(upload_chunk["path"], "/tmp/a.txt");
    assert_eq!(upload_chunk["transferId"], transfer_id);
    assert_eq!(upload_chunk["offset"], 0);
    assert_eq!(upload_chunk["bytesBase64"], "aGVsbG8=");
    assert_eq!(upload_chunk["approvalId"], "approval-1");
    bridge.submit_responses(json!({
        "frames": [{
            "kind": "file",
            "requestId": upload_chunk["requestId"]
        }]
    }));
    let chunk_cancel = receive.recv_timeout(Duration::from_secs(1)).expect("upload-chunk cleanup cancel");
    assert_eq!(chunk_cancel["kind"], "cancel");
    assert_eq!(chunk_cancel["requestId"], upload_chunk["requestId"]);

    let upload_commit = receive.recv_timeout(Duration::from_secs(1)).expect("upload-commit frame");
    assert_eq!(upload_commit["kind"], "upload-commit");
    assert_eq!(upload_commit["path"], "/tmp/a.txt");
    assert_eq!(upload_commit["transferId"], transfer_id);
    assert_eq!(upload_commit["approvalId"], "approval-1");
    bridge.submit_responses(json!({
        "frames": [{
            "kind": "file",
            "requestId": upload_commit["requestId"]
        }]
    }));
    let commit_cancel = receive.recv_timeout(Duration::from_secs(1)).expect("upload-commit cleanup cancel");
    assert_eq!(commit_cancel["kind"], "cancel");
    assert_eq!(commit_cancel["requestId"], upload_commit["requestId"]);
    upload.join().expect("upload thread").expect("upload");

    let download_box = sandbox.clone();
    let download = thread::spawn(move || {
        download_box.download_file(Some("agent-1"), "/tmp/b.txt")
    });
    let download_frame = receive.recv_timeout(Duration::from_secs(1)).expect("download frame");
    assert_eq!(download_frame["kind"], "download");
    assert_eq!(download_frame["path"], "/tmp/b.txt");
    assert_eq!(download_frame["approvalId"], "approval-1");
    bridge.submit_responses(json!({
        "frames": [{
            "kind": "file",
            "requestId": download_frame["requestId"],
            "bytesBase64": "d29ybGQ="
        }]
    }));
    assert_eq!(
        download.join().expect("download thread").expect("download"),
        b"world"
    );

    let calls = gate.calls.lock().expect("calls");
    assert_eq!(
        calls.as_slice(),
        &[
            (Some("agent-1".into()), "write-file".into(), "/tmp/a.txt".into()),
            (Some("agent-1".into()), "read-file".into(), "/tmp/b.txt".into()),
        ]
    );
}

#[test]
fn unavailable_computer_fails_before_permission_approval() {
    let bridge = SandLocalExecBridge::with_sources(
        Arc::new(|| 1000),
        Arc::new(|| "provider-1".to_string()),
    );
    let gate = Arc::new(FakeGate::default());
    let sandbox = GatewayLocalExecSandBox::new(bridge, gate.clone());

    let error = sandbox
        .upload_file(Some("agent"), "/tmp/file", b"hello")
        .expect_err("missing provider");
    assert_eq!(
        error.message,
        mahayana_host_runtime::extensions::local_exec::local_exec_bridge::SAND_NO_LOCAL_MACHINE_MESSAGE
    );
    assert!(gate.calls.lock().expect("calls").is_empty());

    let (bridge, _registration, _receive) = bridge_with_provider();
    let gate = Arc::new(FakeGate::default());
    let sandbox = GatewayLocalExecSandBox::new(bridge, gate.clone()).for_computer("missing");
    let error = sandbox
        .download_file(Some("agent"), "/tmp/file")
        .expect_err("unknown computer");
    assert_eq!(
        error.message,
        mahayana_host_runtime::extensions::local_exec::local_exec_bridge::SAND_NO_LOCAL_MACHINE_MESSAGE
    );
    assert!(gate.calls.lock().expect("calls").is_empty());
}

#[test]
fn oversized_file_and_blocked_gate_fail_before_provider_side_effect() {
    let (bridge, _registration, receive) = bridge_with_provider();
    let gate = Arc::new(FakeGate::default());
    let sandbox = GatewayLocalExecSandBox::new(bridge, gate.clone()).with_max_file_bytes(4);
    let error = sandbox
        .upload_file(Some("agent"), "/tmp/large", b"12345")
        .expect_err("large file rejected");
    assert_eq!(error.message, local_exec_file_too_large_message(5, 4));
    assert!(receive.recv_timeout(Duration::from_millis(20)).is_err());

    *gate.blocked.lock().expect("blocked") = Some("disabled".into());
    let error = sandbox
        .download_file(Some("agent"), "/tmp/file")
        .expect_err("blocked");
    assert_eq!(error.message, "disabled");
    assert!(receive.recv_timeout(Duration::from_millis(20)).is_err());

    assert_eq!(DEFAULT_MAX_LOCAL_EXEC_FILE_BYTES, 100 * 1024 * 1024);
}


#[test]
fn exec_manager_streams_shell_and_read_through_permissioned_provider() {
    let (bridge, _registration, receive) = bridge_with_provider();
    let gate = Arc::new(FakeGate::default());
    let manager = GatewayLocalExecSandBox::new(bridge.clone(), gate.clone())
        .remote_resource_accessor()
        .manager();

    let shell_manager = Arc::clone(&manager);
    let shell = thread::spawn(move || {
        shell_manager.execute_shell(
            &GatewayLocalToolScope {
                agent_id: Some("agent-1".into()),
                tool_call_id: Some("shell-call".into()),
                action: Some("run-command".into()),
            },
            "pwd",
            "/tmp",
            "shell-call",
        )
    });
    let shell_frame = receive.recv_timeout(Duration::from_secs(1)).expect("shell frame");
    assert_eq!(shell_frame["kind"], "exec");
    assert_eq!(shell_frame["approvalId"], "approval-1");
    assert_eq!(shell_frame["serverMessage"]["shellStreamArgs"]["command"], "pwd");
    bridge.submit_responses(json!({"frames":[{
        "kind":"client",
        "requestId":shell_frame["requestId"],
        "message":{"id":1,"shellStream":{"stdout":{"data":"hello\n"}}}
    }]}));
    bridge.submit_responses(json!({"frames":[{
        "kind":"client",
        "requestId":shell_frame["requestId"],
        "message":{"id":1,"shellStream":{"exit":{"code":0,"cwd":"/tmp","aborted":false}}}
    }]}));
    let shell_result = shell.join().expect("shell thread").expect("shell");
    assert_eq!(shell_result.stdout, "hello\n");
    assert_eq!(shell_result.exit_code, Some(0));
    let _ = receive.recv_timeout(Duration::from_secs(1)).expect("shell cancel");

    let read_manager = Arc::clone(&manager);
    let read = thread::spawn(move || {
        read_manager.execute_read(
            &GatewayLocalToolScope {
                agent_id: Some("agent-1".into()),
                tool_call_id: Some("read-call".into()),
                action: Some("read-file".into()),
            },
            "/tmp/a.txt",
            "read-call",
            Some(2),
            Some(4),
            Some("utf-8"),
        )
    });
    let read_frame = receive.recv_timeout(Duration::from_secs(1)).expect("read frame");
    assert_eq!(read_frame["kind"], "exec");
    assert_eq!(read_frame["serverMessage"]["readArgs"]["path"], "/tmp/a.txt");
    bridge.submit_responses(json!({"frames":[{
        "kind":"client",
        "requestId":read_frame["requestId"],
        "message":{"id":2,"readResult":{"success":{"path":"/tmp/a.txt","content":"hello"}}}
    }]}));
    let read_result = read.join().expect("read thread").expect("read");
    assert_eq!(read_result["success"]["content"], "hello");
    let _ = receive.recv_timeout(Duration::from_secs(1)).expect("read cancel");

    let calls = gate.calls.lock().expect("calls");
    assert!(calls.iter().any(|(_, action, target)| action == "run-command" && target == "pwd"));
    assert!(calls.iter().any(|(_, action, target)| action == "read-file" && target == "/tmp/a.txt"));
}

#[test]
fn shell_send_failure_finishes_pre_send_operation_as_failed() {
    let (bridge, _registration, receive) = bridge_with_provider();
    drop(receive);
    let gate = Arc::new(FakeGate::default());
    let manager = GatewayLocalExecSandBox::new(bridge, gate)
        .remote_resource_accessor()
        .manager();

    let error = manager
        .execute_shell(
            &GatewayLocalToolScope {
                agent_id: Some("agent-1".into()),
                tool_call_id: Some("send-failure".into()),
                action: Some("run-command".into()),
            },
            "pwd",
            "/tmp",
            "send-failure",
        )
        .expect_err("provider send failure must fail the shell operation");
    assert!(!error.message.is_empty());
    let record = manager
        .operation("send-failure")
        .expect("failed pre-send operation remains auditable");
    assert_eq!(record.status, GatewayLocalExecOperationStatus::Failed);
}


#[test]
fn exec_manager_reports_control_throw_with_frozen_failure_classification() {
    let (bridge, _registration, receive) = bridge_with_provider();
    let gate = Arc::new(FakeGate::default());
    let reports = Arc::new(Mutex::new(Vec::<GatewayLocalExecFailureReport>::new()));
    let report_sink = Arc::clone(&reports);
    let manager = GatewayLocalExecSandBox::new(bridge.clone(), gate)
        .with_failure_reporter(Arc::new(move |report| {
            report_sink.lock().expect("reports").push(report);
        }))
        .remote_resource_accessor()
        .manager();

    let shell_manager = Arc::clone(&manager);
    let shell = thread::spawn(move || {
        shell_manager.execute_shell(
            &GatewayLocalToolScope {
                agent_id: Some("agent-telemetry".into()),
                tool_call_id: Some("shell-failure".into()),
                action: Some("run-command".into()),
            },
            "rg needle",
            "/tmp",
            "shell-failure",
        )
    });
    let frame = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("exec frame");
    bridge.submit_responses(json!({"frames":[{
        "kind":"control",
        "requestId":frame["requestId"],
        "message":{"throw":{
            "error":"Error: spawn rg ENOENT",
            "stackTrace":"provider-stack"
        }}
    }]}));

    let error = shell
        .join()
        .expect("shell thread")
        .expect_err("provider throw must fail");
    assert!(error.to_string().contains("Error: spawn rg ENOENT"));
    assert!(error.to_string().contains("provider-stack"));
    let _ = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("exec cleanup cancel");

    assert_eq!(
        reports.lock().expect("reports").as_slice(),
        &[GatewayLocalExecFailureReport {
            error_class: LocalExecFailureClass::SpawnEnoent,
            errno: Some("ENOENT".into()),
            site: "exec".into(),
            conversation_id: Some("agent-telemetry".into()),
        }]
    );
}


#[test]
fn exec_manager_journals_stable_operation_id_and_cancels_provider_request() {
    let (bridge, _registration, receive) = bridge_with_provider();
    let gate = Arc::new(FakeGate::default());
    let manager = GatewayLocalExecSandBox::new(bridge.clone(), gate)
        .remote_resource_accessor()
        .manager();

    let shell_manager = Arc::clone(&manager);
    let shell = thread::spawn(move || {
        shell_manager.execute_shell(
            &GatewayLocalToolScope {
                agent_id: Some("agent-journal".into()),
                tool_call_id: Some("operation-stable-1".into()),
                action: Some("run-command".into()),
            },
            "sleep 30",
            "/tmp",
            "operation-stable-1",
        )
    });

    let exec = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("exec frame");
    let bridge_request_id = exec["requestId"]
        .as_str()
        .expect("bridge request id")
        .to_string();
    let active = manager
        .operation_for_agent("agent-journal", "operation-stable-1")
        .expect("active operation");
    assert_eq!(active.operation_id, "operation-stable-1");
    assert_eq!(active.agent_id.as_deref(), Some("agent-journal"));
    assert_eq!(active.bridge_request_id, bridge_request_id);
    assert!(
        matches!(
            active.status,
            GatewayLocalExecOperationStatus::Accepted | GatewayLocalExecOperationStatus::Running
        ),
        "operation must be durably accepted before provider execution and may advance to running"
    );
    assert!(manager.operation_for_agent("other-agent", "operation-stable-1").is_none());
    assert!(!manager.cancel_operation_for_agent("other-agent", "operation-stable-1"));

    assert!(manager.cancel_operation_for_agent("agent-journal", "operation-stable-1"));
    assert!(!manager.cancel_operation("missing-operation"));
    let cancel = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("provider cancel");
    assert_eq!(cancel["kind"], "cancel");
    assert_eq!(cancel["requestId"], bridge_request_id);

    bridge.submit_responses(json!({"frames":[{
        "kind":"control",
        "requestId":bridge_request_id,
        "message":{"streamClose":{"id":1}}
    }]}));
    let result = shell.join().expect("shell thread").expect("cancelled shell");
    assert_eq!(result.exit_code, None);

    let settled = manager
        .operation_for_agent("agent-journal", "operation-stable-1")
        .expect("settled operation");
    assert_eq!(settled.status, GatewayLocalExecOperationStatus::Cancelled);
    assert_eq!(settled.bridge_request_id, bridge_request_id);
    assert_eq!(manager.operation_history_for_agent("agent-journal").len(), 1);
    assert!(manager.operation_history_for_agent("other-agent").is_empty());
}

#[test]
fn exec_manager_does_not_mark_cancel_when_provider_rejects_delivery() {
    let (bridge, _registration, receive) = bridge_with_provider();
    let gate = Arc::new(FakeGate::default());
    let manager = GatewayLocalExecSandBox::new(bridge.clone(), gate)
        .remote_resource_accessor()
        .manager();

    let shell_manager = Arc::clone(&manager);
    let shell = thread::spawn(move || {
        shell_manager.execute_shell(
            &GatewayLocalToolScope {
                agent_id: Some("agent-undelivered-cancel".into()),
                tool_call_id: Some("operation-undelivered-cancel".into()),
                action: Some("run-command".into()),
            },
            "echo provider-disconnected",
            "/tmp",
            "operation-undelivered-cancel",
        )
    });

    let exec = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("exec frame");
    let request_id = exec["requestId"].as_str().expect("request id").to_string();
    drop(receive);

    assert!(!manager.cancel_operation_for_agent(
        "agent-undelivered-cancel",
        "operation-undelivered-cancel"
    ));

    bridge.submit_responses(json!({"frames":[{
        "kind":"client",
        "requestId":request_id,
        "message":{"id":1,"shellStream":{"exit":{"code":0,"cwd":"/tmp","aborted":false}}}
    }]}));
    let result = shell
        .join()
        .expect("shell thread")
        .expect("provider exit remains observable");
    assert_eq!(result.exit_code, Some(0));
    assert!(!result.aborted);

    let settled = manager
        .operation_for_agent("agent-undelivered-cancel", "operation-undelivered-cancel")
        .expect("settled operation");
    assert_eq!(settled.status, GatewayLocalExecOperationStatus::Completed);
}


fn real_shell_output(command: &str) -> std::process::Output {
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", command])
            .output()
            .expect("spawn real Windows shell")
    }
    #[cfg(not(windows))]
    {
        Command::new("/bin/sh")
            .args(["-c", command])
            .output()
            .expect("spawn real POSIX shell")
    }
}

#[test]
fn exec_manager_drives_real_os_process_to_bounded_terminal_result() {
    let (bridge, _registration, receive) = bridge_with_provider();
    let gate = Arc::new(FakeGate::default());
    let manager = GatewayLocalExecSandBox::new(bridge.clone(), gate)
        .remote_resource_accessor()
        .manager();
    let working_dir = std::env::temp_dir().to_string_lossy().into_owned();
    let shell_manager = Arc::clone(&manager);
    let shell_working_dir = working_dir.clone();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let agent_id = format!("agent-real-process-{unique}");
    let op_id = format!("operation-real-process-{unique}");
    let shell_agent_id = agent_id.clone();
    let shell_op_id = op_id.clone();
    let shell = thread::spawn(move || {
        shell_manager.execute_shell(
            &GatewayLocalToolScope {
                agent_id: Some(shell_agent_id),
                tool_call_id: Some(shell_op_id.clone()),
                action: Some("run-command".into()),
            },
            "echo fabushi-terminal-real-process",
            &shell_working_dir,
            &shell_op_id,
        )
    });

    let exec = receive
        .recv_timeout(Duration::from_secs(2))
        .expect("real process exec frame");
    assert_eq!(exec["kind"], "exec");
    let request_id = exec["requestId"].as_str().expect("request id").to_string();
    let command = exec["serverMessage"]["shellStreamArgs"]["command"]
        .as_str()
        .expect("shell command");
    let output = real_shell_output(command);
    assert!(output.status.success(), "real child process failed: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    bridge.submit_responses(json!({"frames":[{
        "kind":"client",
        "requestId":request_id,
        "message":{"id":1,"shellStream":{"stdout":{"data":stdout}}}
    }]}));
    bridge.submit_responses(json!({"frames":[{
        "kind":"client",
        "requestId":request_id,
        "message":{"id":1,"shellStream":{"exit":{"code":0,"cwd":working_dir,"aborted":false}}}
    }]}));

    let result = shell.join().expect("shell thread").expect("real shell result");
    assert!(result.stdout.contains("fabushi-terminal-real-process"));
    assert_eq!(result.exit_code, Some(0));
    let settled = manager
        .operation_for_agent(&agent_id, &op_id)
        .expect("settled real operation");
    assert_eq!(settled.status, GatewayLocalExecOperationStatus::Completed);
    assert!(settled.stdout.contains("fabushi-terminal-real-process"));
    assert!(!settled.stdout_truncated);
    let _ = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("real process cleanup cancel");
}

fn real_cancellable_child() -> Child {
    #[cfg(windows)]
    {
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 30"])
            .spawn()
            .expect("spawn cancellable Windows child")
    }
    #[cfg(not(windows))]
    {
        Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .expect("spawn cancellable POSIX child")
    }
}

#[test]
fn exec_manager_cancel_maps_to_real_child_termination_and_history() {
    let (bridge, _registration, receive) = bridge_with_provider();
    let gate = Arc::new(FakeGate::default());
    let manager = GatewayLocalExecSandBox::new(bridge.clone(), gate)
        .remote_resource_accessor()
        .manager();
    let working_dir = std::env::temp_dir().to_string_lossy().into_owned();
    let shell_manager = Arc::clone(&manager);
    let shell_working_dir = working_dir.clone();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let agent_id = format!("agent-real-cancel-{unique}");
    let op_id = format!("operation-real-cancel-{unique}");
    let shell_agent_id = agent_id.clone();
    let shell_op_id = op_id.clone();
    let shell = thread::spawn(move || {
        shell_manager.execute_shell(
            &GatewayLocalToolScope {
                agent_id: Some(shell_agent_id),
                tool_call_id: Some(shell_op_id.clone()),
                action: Some("run-command".into()),
            },
            "echo fabushi-terminal-cancel-started",
            &shell_working_dir,
            &shell_op_id,
        )
    });

    let exec = receive
        .recv_timeout(Duration::from_secs(2))
        .expect("cancel exec frame");
    let request_id = exec["requestId"].as_str().expect("request id").to_string();
    let command = exec["serverMessage"]["shellStreamArgs"]["command"]
        .as_str()
        .expect("shell command");
    assert_eq!(command, "echo fabushi-terminal-cancel-started");
    let mut child = real_cancellable_child();
    assert!(
        child.try_wait().expect("poll cancellable child").is_none(),
        "real child must still be running before cancellation"
    );
    bridge.submit_responses(json!({"frames":[{
        "kind":"client",
        "requestId":request_id,
        "message":{"id":2,"shellStream":{"stdout":{"data":"fabushi-terminal-cancel-started\\n"}}}
    }]}));

    assert!(manager.cancel_operation_for_agent(&agent_id, &op_id));
    let cancel = receive
        .recv_timeout(Duration::from_secs(2))
        .expect("provider cancel frame");
    assert_eq!(cancel["kind"], "cancel");
    assert_eq!(cancel["requestId"], request_id);
    child.kill().expect("terminate cancellable real child");
    let child_status = child.wait().expect("wait for cancelled real child");
    assert!(!child_status.success(), "cancelled real child must not report success");
    bridge.submit_responses(json!({"frames":[{
        "kind":"client",
        "requestId":request_id,
        "message":{"id":2,"shellStream":{"exit":{"code":130,"cwd":working_dir,"aborted":true}}}
    }]}));

    let result = shell.join().expect("cancel shell thread").expect("cancelled result");
    assert_eq!(result.exit_code, Some(130));
    assert!(result.aborted);
    let settled = manager
        .operation_for_agent(&agent_id, &op_id)
        .expect("settled cancelled operation");
    assert_eq!(settled.status, GatewayLocalExecOperationStatus::Cancelled);
    assert_eq!(manager.operation_history_for_agent(&agent_id).len(), 1);
}
