use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use mahayana_host_runtime::r#box::box_remote_accessor::{
    BoxEndpoint, ping_box_transport_classified,
};
use mahayana_host_runtime::r#box::exec_daemon_process::{
    BoxExecDaemonProcessOptions, box_exec_daemon_binary_name,
    resolve_box_exec_daemon_executable, start_box_exec_daemon_process,
};
use mahayana_host_runtime::r#box::generated_production::{
    ProductionBoxTransport, create_production_box_control_client,
};

fn free_loopback_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind free port");
    listener.local_addr().expect("local addr").port()
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
        .join(box_exec_daemon_binary_name());
    assert_eq!(
        resolve_box_exec_daemon_executable(&host).expect("resolved sibling"),
        expected
    );
}

#[test]
fn supervisor_rejects_contaminated_port_before_spawning() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
    let port = listener.local_addr().expect("addr").port();
    let root = std::env::temp_dir().join(format!(
        "fabushi-box-exec-supervisor-contaminated-{}",
        uuid::Uuid::new_v4()
    ));
    let result = start_box_exec_daemon_process(BoxExecDaemonProcessOptions {
        executable_path: root.join("does-not-matter"),
        host: "127.0.0.1".into(),
        port,
        auth_token: "token".into(),
        workspace_root: root.join("workspace"),
        terminals_directory: root.join("terminals"),
        start_timeout_ms: 500,
        stop_timeout_ms: 500,
    });
    let error = match result {
        Ok(mut daemon) => {
            let _ = daemon.close();
            panic!("contaminated startup unexpectedly succeeded");
        }
        Err(error) => error,
    };
    assert!(error.contains("already bound"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_supervisor_spawns_authenticates_and_closes_real_daemon_when_provided() {
    let Some(executable) = std::env::var_os("FABUSHI_BOX_EXEC_DAEMON_TEST_BIN") else {
        eprintln!(
            "FABUSHI_BOX_EXEC_DAEMON_TEST_BIN not set; exact-head workflow runs this contract explicitly"
        );
        return;
    };
    let executable = PathBuf::from(executable);
    assert!(
        executable.is_file(),
        "daemon test binary missing: {}",
        executable.display()
    );

    let root = std::env::temp_dir().join(format!(
        "fabushi-box-exec-supervisor-{}",
        uuid::Uuid::new_v4()
    ));
    let port = free_loopback_port();
    let auth_token = format!("test-{}", uuid::Uuid::new_v4());
    let mut owned = start_box_exec_daemon_process(BoxExecDaemonProcessOptions {
        executable_path: executable.clone(),
        host: "127.0.0.1".into(),
        port,
        auth_token: auth_token.clone(),
        workspace_root: root.join("workspace"),
        terminals_directory: root.join("terminals"),
        start_timeout_ms: 5_000,
        stop_timeout_ms: 2_000,
    })
    .expect("start real daemon");
    assert!(owned.pid > 0);
    assert_eq!(owned.executable_path, executable);

    let endpoint = BoxEndpoint::new("127.0.0.1", port, auth_token);
    let transport = ProductionBoxTransport::from_endpoint(&endpoint);
    let ping = ping_box_transport_classified(
        &(),
        &transport,
        create_production_box_control_client,
        500,
    );
    assert_eq!(ping.outcome, "ok");

    owned.close().expect("graceful daemon shutdown");
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        TcpListener::bind(("127.0.0.1", port)).is_ok(),
        "daemon port must be released after close"
    );
    let _ = fs::remove_dir_all(root);
}
