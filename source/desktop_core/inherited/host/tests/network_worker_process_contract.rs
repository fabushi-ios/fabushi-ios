use std::fs;
use std::path::PathBuf;

use mahayana_host_runtime::network_worker_process::{
    NETWORK_WORKER_START_TIMEOUT_MS, NETWORK_WORKER_STOP_TIMEOUT_MS, NetworkWorkerProcessOptions,
    network_worker_binary_name, resolve_network_worker_executable, start_network_worker,
};

fn temporary_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("{label}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

#[test]
fn sibling_binary_resolution_matches_packaged_resources_bin_contract() {
    let host = if cfg!(windows) {
        PathBuf::from(r"C:\Fabushi\resources\bin\mahayana-app-host.exe")
    } else {
        PathBuf::from("/Applications/Fabushi.app/Contents/Resources/bin/mahayana-app-host")
    };
    let expected = host
        .parent()
        .expect("host parent")
        .join(network_worker_binary_name());
    assert_eq!(
        resolve_network_worker_executable(&host).expect("resolved sibling"),
        expected
    );
}

#[test]
fn process_options_debug_redacts_network_credentials() {
    let root = temporary_root("fabushi-network-debug");
    let auth_secret = "tskey-auth-secret-never-log";
    let state_secret = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let options = NetworkWorkerProcessOptions {
        executable_path: root.join(network_worker_binary_name()),
        hostname: "device-node-01234567".into(),
        control_url: "https://coord.example.test".into(),
        auth_key: auth_secret.into(),
        state_dir: root.join("state"),
        state_key_b64: state_secret.into(),
        start_timeout_ms: NETWORK_WORKER_START_TIMEOUT_MS,
        stop_timeout_ms: NETWORK_WORKER_STOP_TIMEOUT_MS,
    };
    let rendered = format!("{options:?}");
    assert!(!rendered.contains(auth_secret));
    assert!(!rendered.contains(state_secret));
    assert!(rendered.contains("<redacted>"));
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn supervisor_passes_secrets_only_over_private_stdin_and_closes_child() {
    use std::os::unix::fs::PermissionsExt;

    let root = temporary_root("fabushi-network-supervisor");
    let executable = root.join("fake-network-worker");
    let auth_secret = "tskey-auth-private-0123456789";
    let state_secret = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let script = format!(
        r#"#!/bin/sh
case "$*" in
  *{auth_secret}*|*{state_secret}*) exit 41 ;;
esac
if env | grep -F "{auth_secret}" >/dev/null 2>&1; then exit 42; fi
if env | grep -F "{state_secret}" >/dev/null 2>&1; then exit 43; fi
IFS= read -r init || exit 44
case "$init" in
  *{auth_secret}*{state_secret}*) ;;
  *) exit 45 ;;
esac
printf '%s\n' '{{"type":"ready"}}'
while IFS= read -r request; do
  case "$request" in
    *'"type":"status"'*)
      request_id="$(printf '%s' "$request" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')"
      printf '%s\n' "{{\"type\":\"status\",\"requestId\":\"$request_id\",\"backendState\":\"Running\",\"tailscaleIps\":[\"100.64.0.10\"]}}"
      ;;
    *'"type":"streamDial"'*)
      request_id="$(printf '%s' "$request" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')"
      session_id="$(printf '%s' "$request" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')"
      printf '%s\n' "{{\"type\":\"streamDialed\",\"requestId\":\"$request_id\",\"sessionId\":\"$session_id\"}}"
      ;;
    *'"type":"streamListen"'*)
      request_id="$(printf '%s' "$request" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')"
      session_id="$(printf '%s' "$request" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')"
      printf '%s\n' "{{\"type\":\"streamAccepted\",\"requestId\":\"$request_id\",\"sessionId\":\"$session_id\"}}"
      ;;
    *'"type":"streamWrite"'*)
      request_id="$(printf '%s' "$request" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')"
      session_id="$(printf '%s' "$request" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')"
      printf '%s\n' "{{\"type\":\"streamWritten\",\"requestId\":\"$request_id\",\"sessionId\":\"$session_id\",\"bytes\":4}}"
      ;;
    *'"type":"streamRead"'*)
      request_id="$(printf '%s' "$request" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')"
      session_id="$(printf '%s' "$request" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')"
      printf '%s\n' "{{\"type\":\"streamData\",\"requestId\":\"$request_id\",\"sessionId\":\"$session_id\",\"dataB64\":\"cG9uZw==\",\"bytes\":4}}"
      ;;
    *'"type":"streamCancel"'*)
      request_id="$(printf '%s' "$request" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')"
      session_id="$(printf '%s' "$request" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')"
      printf '%s\n' "{{\"type\":\"streamCancelled\",\"requestId\":\"$request_id\",\"sessionId\":\"$session_id\"}}"
      ;;
    *'"type":"streamClose"'*)
      request_id="$(printf '%s' "$request" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')"
      session_id="$(printf '%s' "$request" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')"
      printf '%s\n' "{{\"type\":\"streamClosed\",\"requestId\":\"$request_id\",\"sessionId\":\"$session_id\"}}"
      ;;
    *'"type":"shutdown"'*) printf '%s\n' '{{"type":"shutdown"}}'; exit 0 ;;
  esac
done
exit 0
"#
    );
    fs::write(&executable, script).expect("write fake worker");
    let mut permissions = fs::metadata(&executable)
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("make fake worker executable");

    let mut owned = start_network_worker(NetworkWorkerProcessOptions {
        executable_path: executable.clone(),
        hostname: "device-node-01234567".into(),
        control_url: "https://coord.example.test".into(),
        auth_key: auth_secret.into(),
        state_dir: root.join("state"),
        state_key_b64: state_secret.into(),
        start_timeout_ms: 2_000,
        stop_timeout_ms: 2_000,
    })
    .expect("start fake worker");
    assert!(owned.pid > 0);
    assert_eq!(owned.executable_path, executable);
    let status = owned.status().expect("read fake worker status");
    assert_eq!(status.backend_state, "Running");
    assert_eq!(status.tailscale_ips, vec!["100.64.0.10"]);
    let stream = owned
        .stream_dial("100.64.0.20", 443, 1_000)
        .expect("open fake stream");
    assert!(stream.starts_with("stream-"));
    assert_eq!(
        owned
            .stream_write(&stream, b"ping", 1_000)
            .expect("write fake stream"),
        4
    );
    let read = owned
        .stream_read(&stream, 32, 1_000)
        .expect("read fake stream");
    assert_eq!(read.data, b"pong");
    assert!(!read.eof);
    owned.stream_close(&stream).expect("close fake stream");
    let accepted = owned
        .stream_listen("100.64.0.30", 8443, 1_000)
        .expect("accept peer-bound fake stream");
    assert!(accepted.starts_with("stream-"));
    owned
        .stream_cancel(&accepted)
        .expect("cancel accepted fake stream");
    assert!(
        owned
            .stream_write(&stream, &[0_u8; 32 * 1024 + 1], 1_000)
            .is_err()
    );
    owned.close().expect("close fake worker");
    let _ = fs::remove_dir_all(root);
}
