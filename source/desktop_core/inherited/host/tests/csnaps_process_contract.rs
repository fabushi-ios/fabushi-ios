use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use mahayana_host_runtime::extensions::codebase_telemetry::csnaps_process::{
    CsnapsDeadlines, CsnapsProcess, describe_csnaps_exit, validate_response_id,
};
use serde_json::{Map, json};

#[cfg(unix)]
fn fake_csnaps(script_body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!(
        "fabushi-csnaps-process-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&dir).expect("tempdir");
    let path = dir.join("csnaps");
    fs::write(&path, format!("#!/usr/bin/env python3\n{script_body}\n")).expect("script");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("permissions");
    dir
}

#[cfg(unix)]
const SERVER: &str = r#"
import json, struct, sys

def read_frame():
    header = sys.stdin.buffer.read(4)
    if not header:
        return None
    size = struct.unpack(">I", header)[0]
    return json.loads(sys.stdin.buffer.read(size))

def send(value):
    data = json.dumps(value, separators=(",", ":")).encode()
    sys.stdout.buffer.write(struct.pack(">I", len(data)) + data)
    sys.stdout.buffer.flush()

while True:
    req = read_frame()
    if req is None:
        break
    method = req["method"]
    if method == "ping":
        send({"id":req["id"],"ok":True,"result":{}})
    elif method == "initialize":
        send({"id":req["id"],"ok":True,"result":{"state":{"tracked":[]}}})
    elif method == "apply_codebase_specs":
        send({"id":req["id"],"ok":True,"result":{"state":{"tracked":[{"codebaseUuid":"123e4567-e89b-42d3-a456-426614174000","path":"/repo","kind":2,"environment":1}]},"rejected":[]}})
    elif method in ("snapshot","trigger_upload","flush_pending_uploads"):
        send({"id":req["id"],"ok":True,"result":{}})
    elif method == "shutdown":
        send({"id":req["id"],"ok":True,"result":{}})
        break
"#;

#[cfg(unix)]
#[test]
fn process_spawns_handshakes_routes_operations_and_closes() {
    let dir = fake_csnaps(SERVER);
    let spawned = CsnapsProcess::spawn(
        dir.join("csnaps"),
        Map::new(),
        CsnapsDeadlines::default(),
    ).expect("spawn");
    assert!(spawned.initial_state.tracked.is_empty());

    let (state, rejected) = spawned.handle.apply_codebase_specs(vec![json!({
        "path":"/repo","kind":2
    })]).expect("apply");
    assert!(rejected.is_empty());
    assert_eq!(state.tracked.len(), 1);
    assert_eq!(state.tracked[0].path, "/repo");

    spawned.handle.snapshot(json!({"type":"agent_request_start"})).expect("snapshot");
    spawned.handle.trigger_upload(Map::from_iter([("authToken".into(), json!("redacted"))])).expect("upload");
    spawned.handle.flush_pending_uploads(Map::new()).expect("flush");
    spawned.handle.close().expect("close");
    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn unknown_response_id_is_terminal_and_fails_closed() {
    let dir = fake_csnaps(r#"
import json, struct, sys, time
def read_frame():
    h=sys.stdin.buffer.read(4)
    if not h: return None
    n=struct.unpack(">I",h)[0]
    return json.loads(sys.stdin.buffer.read(n))
def send(v):
    d=json.dumps(v,separators=(",",":")).encode()
    sys.stdout.buffer.write(struct.pack(">I",len(d))+d); sys.stdout.buffer.flush()
req=read_frame()
send({"id":req["id"]+1,"ok":True,"result":{}})
time.sleep(0.2)
"#);
    let err = CsnapsProcess::spawn(
        dir.join("csnaps"),
        Map::new(),
        CsnapsDeadlines {
            ping: Duration::from_secs(1),
            ..CsnapsDeadlines::default()
        },
    ).expect_err("unknown id must fail");
    assert!(!err.to_string().is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn deadline_timeout_is_terminal_and_kills_child() {
    let dir = fake_csnaps(r#"
import struct, sys, time
h=sys.stdin.buffer.read(4)
if h:
    n=struct.unpack(">I",h)[0]
    sys.stdin.buffer.read(n)
time.sleep(5)
"#);
    let err = CsnapsProcess::spawn(
        dir.join("csnaps"),
        Map::new(),
        CsnapsDeadlines {
            ping: Duration::from_millis(50),
            exit: Duration::from_secs(1),
            ..CsnapsDeadlines::default()
        },
    ).expect_err("timeout must fail");
    assert!(err.to_string().contains("timed out"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn response_id_validation_rejects_unknown_ids_with_frozen_error() {
    assert!(validate_response_id(7, 7).is_ok());
    let error = validate_response_id(7, 8).expect_err("unknown id must fail");
    assert_eq!(error.to_string(), "csnaps returned an unknown request ID");
}

#[test]
fn exit_descriptions_match_frozen_contract() {
    assert_eq!(describe_csnaps_exit(Some(7), None), "csnaps process exited with code 7");
    assert_eq!(describe_csnaps_exit(None, Some("SIGTERM")), "csnaps process exited due to signal SIGTERM");
    assert_eq!(describe_csnaps_exit(None, None), "csnaps process exited");
}
