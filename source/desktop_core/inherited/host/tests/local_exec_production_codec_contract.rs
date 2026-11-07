use std::sync::Arc;

use mahayana_host_runtime::extensions::local_exec::agent_v1_exec_wire::{
    FROZEN_EXEC_PB_BLOB_SHA, FROZEN_GROK_018_COMMIT,
    FROZEN_LOCAL_EXEC_PRODUCTION_BLOB_SHA, FROZEN_READ_EXEC_PB_BLOB_SHA,
    FROZEN_SHELL_EXEC_PB_BLOB_SHA,
};
use mahayana_host_runtime::extensions::local_exec::production::{
    GatewayExecControl, ProductionExecClientPayload, ProductionShellStreamEvent,
    PRODUCTION_LOCAL_EXEC_CODEC,
};
use serde_json::json;

const CANONICAL_PROTO: &str = include_str!("../proto/agent/v1/local_exec.proto");
const BINDING_MODULE: &str =
    include_str!("../src/extensions/local_exec/agent_v1_exec_wire.rs");
const HOST_BUILD_SCRIPT: &str = include_str!("../build.rs");
const SHIPPING_MANAGER: &str = concat!(
    include_str!("../src/extensions/local_exec/gateway_local_exec_sand_box.rs"),
    "\n",
    include_str!("../src/extensions/local_exec/gateway_local_exec_sandbox_ocrm.rs"),
);
const SHIPPING_RUNNER: &str =
    include_str!("../src/runner/host_external_machine_dependencies.rs");

#[test]
fn production_codec_accepts_exec_client_json_and_ignores_unknown_fields() {
    let decoded = PRODUCTION_LOCAL_EXEC_CODEC
        .decode_client(json!({
            "id": 9,
            "execId": "exec-9",
            "futureField": {"nested": true}
        }))
        .expect("decode client");
    assert_eq!(decoded.generated().id, 9);
    assert_eq!(decoded.generated().exec_id, "exec-9");
    assert!(matches!(decoded.payload(), ProductionExecClientPayload::Other));
    assert!(
        PRODUCTION_LOCAL_EXEC_CODEC
            .decode_client(json!(["not", "a", "message"]))
            .is_err()
    );
}

#[test]
fn production_codec_projects_frozen_control_cases() {
    assert_eq!(
        PRODUCTION_LOCAL_EXEC_CODEC
            .decode_control(&json!({
                "throw": {
                    "id": 7,
                    "error": "boom",
                    "stackTrace": "stack",
                    "errorCode": "E_BOOM",
                    "unknownField": true
                }
            }))
            .expect("throw"),
        GatewayExecControl::Throw {
            error: "boom".into(),
            stack_trace: Some("stack".into()),
        }
    );
    assert_eq!(
        PRODUCTION_LOCAL_EXEC_CODEC
            .decode_control(&json!({"streamClose": {"id": 7}}))
            .expect("stream close"),
        GatewayExecControl::StreamClose
    );
    assert_eq!(
        PRODUCTION_LOCAL_EXEC_CODEC
            .decode_control(&json!({"heartbeat": {"id": 7}, "future": true}))
            .expect("heartbeat"),
        GatewayExecControl::Unknown
    );
    assert_eq!(
        PRODUCTION_LOCAL_EXEC_CODEC
            .decode_control(&json!({"futureControl": {"id": 7}}))
            .expect("unknown"),
        GatewayExecControl::Unknown
    );
    assert!(
        PRODUCTION_LOCAL_EXEC_CODEC
            .decode_control(&json!({
                "throw": {"error": "boom"},
                "streamClose": {}
            }))
            .is_err()
    );
}

#[test]
fn production_codec_creates_package_owned_remote_accessor() {
    let manager = Arc::new(String::from("manager"));
    let accessor = PRODUCTION_LOCAL_EXEC_CODEC.create_remote_accessor(Arc::clone(&manager));
    assert!(Arc::ptr_eq(&manager, &accessor.manager()));
}

#[test]
fn production_codec_projects_generated_shell_and_read_shapes() {
    let shell_request = PRODUCTION_LOCAL_EXEC_CODEC
        .shell_stream_server_message(4, "pwd", "/tmp", "tool-shell");
    assert_eq!(shell_request["id"], 4);
    assert_eq!(shell_request["shellStreamArgs"]["command"], "pwd");
    assert_eq!(shell_request["shellStreamArgs"]["workingDirectory"], "/tmp");
    assert_eq!(shell_request["shellStreamArgs"]["toolCallId"], "tool-shell");

    let shell = PRODUCTION_LOCAL_EXEC_CODEC
        .decode_client(json!({
            "id": 4,
            "shellStream": {"stdout": {"data": "hello\n"}},
            "futureField": true
        }))
        .expect("shell stream");
    assert_eq!(
        shell.payload(),
        &ProductionExecClientPayload::ShellStream(
            ProductionShellStreamEvent::Stdout("hello\n".into())
        )
    );

    let read_request = PRODUCTION_LOCAL_EXEC_CODEC
        .read_server_message(5, "/tmp/a.txt", "tool-read", Some(3), Some(7), Some("utf-8"));
    assert_eq!(read_request["readArgs"]["path"], "/tmp/a.txt");
    assert_eq!(read_request["readArgs"]["offset"], 3);
    assert_eq!(read_request["readArgs"]["limit"], 7);
    assert_eq!(read_request["readArgs"]["encodingHint"], "utf-8");

    let read = PRODUCTION_LOCAL_EXEC_CODEC
        .decode_client(json!({
            "id": 5,
            "readResult": {"success": {"path": "/tmp/a.txt", "content": "hello"}}
        }))
        .expect("read result");
    assert!(matches!(
        read.payload(),
        ProductionExecClientPayload::ReadResult(value)
            if value["success"]["content"] == "hello"
    ));
}

#[test]
fn production_codec_uses_generated_oneof_and_permission_error_semantics() {
    assert!(
        PRODUCTION_LOCAL_EXEC_CODEC
            .decode_client(json!({
                "shellStream": {"stdout": {"data": "hello"}},
                "readResult": {"success": {"path": "/tmp/a", "content": "world"}}
            }))
            .is_err(),
        "generated ExecClientMessage oneof must reject multiple branches"
    );

    let denied = PRODUCTION_LOCAL_EXEC_CODEC
        .decode_client(json!({
            "id": 11,
            "shellStream": {
                "permissionDenied": {
                    "command": "rm",
                    "workingDirectory": "/tmp",
                    "error": "policy denied",
                    "isReadonly": false,
                    "futureField": true
                }
            },
            "futureField": true
        }))
        .expect("permission denied");
    assert_eq!(
        denied.payload(),
        &ProductionExecClientPayload::ShellStream(
            ProductionShellStreamEvent::PermissionDenied("policy denied".into())
        )
    );
}

#[test]
fn generated_binding_owns_the_complete_exec_oneof_shape() {
    let unsupported = PRODUCTION_LOCAL_EXEC_CODEC
        .decode_client(json!({
            "id": 13,
            "writeResult": {"futurePayload": true}
        }))
        .expect("generated unsupported branch");
    assert!(matches!(
        unsupported.payload(),
        ProductionExecClientPayload::Other
    ));

    assert!(
        PRODUCTION_LOCAL_EXEC_CODEC
            .decode_client(json!({
                "writeResult": {"ok": true},
                "readResult": {"success": {"path": "/tmp/a", "content": "x"}}
            }))
            .is_err(),
        "full generated oneof must reject conflicts even outside Shell/Read"
    );
}

#[test]
fn generated_protobuf_json_preserves_read_bytes_and_int64_semantics() {
    let read = PRODUCTION_LOCAL_EXEC_CODEC
        .decode_client(json!({
            "readResult": {
                "success": {
                    "path": "/tmp/blob",
                    "data": "aGVsbG8=",
                    "fileSize": "9007199254740993",
                    "truncated": true
                }
            }
        }))
        .expect("protobuf-json read result");
    assert!(matches!(
        read.payload(),
        ProductionExecClientPayload::ReadResult(value)
            if value["success"]["data"] == "aGVsbG8="
                && value["success"]["fileSize"] == "9007199254740993"
                && value["success"]["truncated"] == true
    ));
}

#[test]
fn generated_binding_provenance_is_auditable_and_not_a_parallel_codec() {
    assert_eq!(
        FROZEN_GROK_018_COMMIT,
        "a9f633e09d49a85829b8236331b9e21f7e612634"
    );
    assert_eq!(
        FROZEN_LOCAL_EXEC_PRODUCTION_BLOB_SHA,
        "4d46d34794266cecf5cfc185e93941a04bd51d4f"
    );
    assert_eq!(
        FROZEN_EXEC_PB_BLOB_SHA,
        "b3d569d1ad5f923444d472a08efb8bad58c5941f"
    );
    assert_eq!(
        FROZEN_SHELL_EXEC_PB_BLOB_SHA,
        "43079a2f5df12a5f570589ce312fd7fcfb71bd1f"
    );
    assert_eq!(
        FROZEN_READ_EXEC_PB_BLOB_SHA,
        "3156e4b4412f990b1798ebbdee3e1df37e4980b1"
    );

    for needle in [
        FROZEN_GROK_018_COMMIT,
        FROZEN_LOCAL_EXEC_PRODUCTION_BLOB_SHA,
        FROZEN_EXEC_PB_BLOB_SHA,
        FROZEN_SHELL_EXEC_PB_BLOB_SHA,
        FROZEN_READ_EXEC_PB_BLOB_SHA,
        "message ExecClientMessage",
        "message ExecClientControlMessage",
        "message ExecServerMessage",
        "oneof message",
    ] {
        assert!(CANONICAL_PROTO.contains(needle), "missing proto provenance/schema: {needle}");
    }

    assert!(BINDING_MODULE.contains("agent.v1.rs"));
    assert!(BINDING_MODULE.contains("agent.v1.serde.rs"));
    assert!(!BINDING_MODULE.contains("from_json_ignoring_unknown_fields"));
    assert!(!BINDING_MODULE.contains("prost::Oneof"));
    assert!(!BINDING_MODULE.contains("#[derive(Clone, PartialEq, Message)]"));

    for needle in [
        "generate_agent_v1_local_exec_bindings",
        "pbjson_build::Builder::new()",
        "ignore_unknown_fields",
        "exec_pb.ts",
        "shell_exec_pb.ts",
        "read_exec_pb.ts",
    ] {
        assert!(HOST_BUILD_SCRIPT.contains(needle), "missing generated-binding build evidence: {needle}");
    }
}

#[test]
fn generated_codec_is_uniquely_consumed_by_shipping_manager_and_runner() {
    let shipping_manager_compact: String = SHIPPING_MANAGER.split_whitespace().collect();
    for needle in [
        "PRODUCTION_LOCAL_EXEC_CODEC.shell_stream_server_message",
        "PRODUCTION_LOCAL_EXEC_CODEC.read_server_message",
        "PRODUCTION_LOCAL_EXEC_CODEC.decode_client",
        "PRODUCTION_LOCAL_EXEC_CODEC.decode_control",
        ".create_remote_accessor(Arc::new(self.exec_manager()))",
    ] {
        assert!(
            shipping_manager_compact.contains(needle),
            "shipping permissioned manager is missing generated-codec consumer: {needle}"
        );
    }

    for needle in [
        ".sandbox(gate)",
        ".remote_resource_accessor()",
        ".manager()",
        ".execute_shell(",
        ".execute_read(",
    ] {
        assert!(
            SHIPPING_RUNNER.contains(needle),
            "shipping Runner ExternalShell/ExternalRead chain is missing: {needle}"
        );
    }
}
