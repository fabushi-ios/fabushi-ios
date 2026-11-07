use std::fs;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use mahayana_host_runtime::extensions::local_exec::extension::start_local_exec_extension;
use mahayana_host_runtime::extensions::local_exec::gateway_local_exec_sand_box::{
    GatewayLocalToolGate, GatewayLocalToolScope,
};
use mahayana_host_runtime::extensions::local_exec::{
    local_exec_error::SandLocalExecError,
    local_exec_failure_classifier::{
        CommandErrorResult, CommandTermination, LocalExecFailureClass,
        LocalExecFailureClassification, classify_local_exec_failure, format_command_error,
        format_command_output_for_user,
    },
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::HostTelemetryService;

#[test]
fn local_exec_ask_gate_reports_refusal_with_agent_identity() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "fabushi-local-exec-telemetry-{}-{unique}.jsonl",
        std::process::id()
    ));
    let telemetry = HostTelemetryService::open_with_identity_tags(&path, Default::default())
        .expect("open telemetry");
    let before_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis();
    let extension = start_local_exec_extension(telemetry.logs.clone());

    assert!(!extension.check_live_computer_for_ask(Some("agent-local-1")));
    let after_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis();

    let raw = fs::read_to_string(&path).expect("read telemetry record");
    let record: serde_json::Value =
        serde_json::from_str(raw.lines().next().expect("telemetry line"))
            .expect("parse telemetry record");
    assert_eq!(record["event"], "sand.local_exec.refused");
    assert_eq!(record["payload"]["metadata"]["cause"], "no_providers");
    assert_eq!(record["payload"]["metadata"]["site"], "ask_gate");
    assert_eq!(
        record["payload"]["metadata"]["conversation_id"],
        "agent-local-1"
    );
    assert_eq!(record["payload"]["metadata"]["provider_count"], "0");
    assert_eq!(record["payload"]["metadata"]["live_provider_count"], "0");
    assert_eq!(record["payload"]["metadata"]["ever_registered"], "false");
    // Construction and refusal use a real clock; crossing a millisecond is valid.
    let empty_for_ms = record["payload"]["metadata"]["empty_for_ms"]
        .as_str()
        .expect("elapsed time string")
        .parse::<u64>()
        .expect("unsigned elapsed milliseconds");
    assert!(
        u128::from(empty_for_ms) <= after_ms.saturating_sub(before_ms),
        "refusal duration must fit within the production call interval"
    );
    let _ = fs::remove_file(path);
}

#[test]
fn local_exec_error_preserves_provider_message() {
    let error = SandLocalExecError::new("computer disconnected");
    assert_eq!(error.to_string(), "computer disconnected");
}

#[test]
fn failure_classifier_matches_grok_spawn_errno_contract() {
    assert_eq!(
        classify_local_exec_failure("Error: spawn rg ENOENT"),
        LocalExecFailureClassification {
            error_class: LocalExecFailureClass::SpawnEnoent,
            errno: Some("ENOENT".into()),
        }
    );
    assert_eq!(
        classify_local_exec_failure("spawnSync tool EACCES"),
        LocalExecFailureClassification {
            error_class: LocalExecFailureClass::SpawnPermissions,
            errno: Some("EACCES".into()),
        }
    );
    assert_eq!(
        classify_local_exec_failure("spawn helper ETIMEDOUT"),
        LocalExecFailureClassification {
            error_class: LocalExecFailureClass::SpawnOther,
            errno: Some("ETIMEDOUT".into()),
        }
    );
    assert_eq!(
        classify_local_exec_failure("request failed EPERM"),
        LocalExecFailureClassification {
            error_class: LocalExecFailureClass::Other,
            errno: Some("EPERM".into()),
        }
    );
}

#[test]
fn command_output_formatting_strips_ansi_cr_redraw_and_bounds_tail() {
    let output = "old\nline\rnew\n\x1b[31mred\x1b[0m";
    assert_eq!(format_command_output_for_user(output, 100), "old\nnew\nred");
    let long = "😀abcdef";
    assert_eq!(format_command_output_for_user(long, 5), "…\n😀abc");
}

#[test]
fn command_error_merges_bounded_streams_and_preserves_termination_reason() {
    let error = format_command_error(
        "\u{1b}[31msecret\u{1b}[0m\narg",
        &CommandErrorResult {
            code: Some(17),
            termination: CommandTermination::Exit,
            signal: None,
            output_limit_exceeded: false,
        },
        "stderr line",
        "stdout line",
        5_000,
    );
    assert_eq!(
        error,
        "secret arg failed (exit code 17):\nstderr: stderr line\nstdout: stdout line"
    );
}

#[test]
fn command_error_uses_timeout_and_signal_without_raw_command_newlines() {
    let error = format_command_error(
        "run\npassword",
        &CommandErrorResult {
            code: None,
            termination: CommandTermination::Timeout,
            signal: Some("SIGTERM".into()),
            output_limit_exceeded: false,
        },
        "",
        "",
        2_500,
    );
    assert_eq!(
        error,
        "run password failed (timed out after 2.5 seconds; signal SIGTERM)"
    );
}

#[test]
fn local_exec_provider_lifecycle_is_persisted_through_host_telemetry() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "fabushi-local-exec-provider-telemetry-{}-{unique}.jsonl",
        std::process::id()
    ));
    let telemetry = HostTelemetryService::open_with_identity_tags(&path, Default::default())
        .expect("open telemetry");
    let extension = start_local_exec_extension(telemetry.logs.clone());
    let (send, receive) = mpsc::channel();
    let registration = extension.register_provider(send);
    let welcome = receive
        .recv_timeout(Duration::from_millis(100))
        .expect("welcome");
    let provider_id = welcome["providerId"]
        .as_str()
        .expect("provider id")
        .to_string();
    extension.submit_responses(json!({
        "providerId": provider_id,
        "frames": [{
            "kind": "hello",
            "localRoot": "/Users/test",
            "terminalsFolder": "terminals",
            "computerId": "mac-telemetry",
            "supervised": true,
            "variant": "sand"
        }]
    }));
    drop(registration);

    let records = fs::read_to_string(&path)
        .expect("read telemetry")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("telemetry record"))
        .filter(|record| record["event"] == "sand.local_exec.provider")
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["payload"]["metadata"]["phase"], "registered");
    assert_eq!(records[1]["payload"]["metadata"]["phase"], "hello");
    assert_eq!(
        records[1]["payload"]["metadata"]["computer_id_present"],
        "true"
    );
    assert_eq!(records[1]["payload"]["metadata"]["supervised"], "true");
    assert_eq!(records[1]["payload"]["metadata"]["variant"], "sand");
    assert_eq!(records[2]["payload"]["metadata"]["phase"], "detached");
    assert_eq!(records[2]["payload"]["metadata"]["had_hello"], "true");
    assert_eq!(records[2]["payload"]["metadata"]["emptied"], "true");
    let _ = fs::remove_file(path);
}

struct AllowLocalExecGate;

impl GatewayLocalToolGate for AllowLocalExecGate {
    fn blocked_reason(&self) -> Option<String> {
        None
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn authorize(
        &self,
        _scope: &GatewayLocalToolScope,
        _action: &str,
        _target: &str,
    ) -> Result<Option<String>, SandLocalExecError> {
        Ok(None)
    }
}

#[test]
fn production_extension_reports_exec_failure_through_host_telemetry() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "fabushi-local-exec-failure-telemetry-{}-{unique}.jsonl",
        std::process::id()
    ));
    let telemetry = HostTelemetryService::open_with_identity_tags(&path, Default::default())
        .expect("open telemetry");
    let extension = start_local_exec_extension(telemetry.logs.clone());

    let (send, receive) = mpsc::channel();
    let registration = extension.register_provider(send);
    let welcome = receive
        .recv_timeout(Duration::from_millis(100))
        .expect("welcome");
    let provider_id = welcome["providerId"]
        .as_str()
        .expect("provider id")
        .to_string();
    extension.submit_responses(json!({
        "providerId": provider_id,
        "frames": [{
            "kind": "hello",
            "localRoot": "/Users/test",
            "terminalsFolder": "terminals",
            "computerId": "mac-failure",
            "label": "Failure Mac"
        }]
    }));

    let gate: Arc<dyn GatewayLocalToolGate> = Arc::new(AllowLocalExecGate);
    let manager = extension.sandbox(gate).remote_resource_accessor().manager();
    let call_id = format!("failure-call-{unique}");
    let call_id_for_shell = call_id.clone();
    let shell = thread::spawn(move || {
        manager.execute_shell(
            &GatewayLocalToolScope {
                agent_id: Some("agent-local-exec".into()),
                tool_call_id: Some(call_id_for_shell.clone()),
                action: Some("run-command".into()),
            },
            "tool",
            "/tmp",
            &call_id_for_shell,
        )
    });
    let frame = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("exec frame");
    extension.submit_responses(json!({
        "providerId": provider_id,
        "frames": [{
            "kind": "control",
            "requestId": frame["requestId"],
            "message": {
                "throw": {
                    "error": "Error: spawn tool EACCES"
                }
            }
        }]
    }));
    let error = shell
        .join()
        .expect("shell thread")
        .expect_err("provider throw must fail");
    assert!(error.to_string().contains("spawn tool EACCES"));
    let _ = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("exec cleanup cancel");
    drop(registration);

    let records = fs::read_to_string(&path)
        .expect("read telemetry")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("telemetry record"))
        .filter(|record| record["event"] == "sand.local_exec.exec_failed")
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0]["payload"]["metadata"]["error_class"],
        "spawn_permissions"
    );
    assert_eq!(records[0]["payload"]["metadata"]["errno"], "EACCES");
    assert_eq!(records[0]["payload"]["metadata"]["site"], "exec");
    assert_eq!(
        records[0]["payload"]["metadata"]["conversation_id"],
        "agent-local-exec"
    );
    let _ = fs::remove_file(path);
}
