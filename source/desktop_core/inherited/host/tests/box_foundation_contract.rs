use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::r#box::box_mcp::{
    BoxMcpControlClient, BoxMcpLoadError, BoxMcpLoadRequest, BoxMcpLoadResponse,
    ConnectErrorCode, ConnectErrorCodeSource, load_box_mcp_servers_via_transport,
};
use mahayana_host_runtime::r#box::box_store_backend_policy::{
    BoxStoreBackendKind, BoxStoreBackendPolicyCache, SAND_BOX_STORE_BACKEND_ENV,
    SAND_BOX_STORE_LOCAL_DIR_ENV, is_box_store_copy_in_enabled, is_box_store_sync_enabled,
    resolve_box_store_backend_policy,
};
use mahayana_host_runtime::r#box::protected_path_guard::{
    SandProtectedPathError, assert_path_outside_protected_roots,
};
use mahayana_host_runtime::ports::r#box::{
    NO_MONITOR_COMPUTER_USE_EXECUTOR, NoMonitorComputerUseExecutor,
    SAND_BOX_FIRST_FORK_WINDOW_INDEX, SAND_BOX_NO_MONITOR_AVAILABLE_MESSAGE,
    SAND_BOX_NOT_READY_MESSAGE, SAND_BOX_NOT_RESPONDING_MESSAGE,
    SandBoxDaemonUnreachableError, SandBoxNoMonitorAvailableError,
    box_not_ready_message_for_error, is_no_monitor_computer_use_executor,
    is_primary_window_index,
};

#[test]
fn box_port_preserves_readiness_messages_window_indices_and_no_monitor_identity() {
    let no_monitor = SandBoxNoMonitorAvailableError::default();
    assert_eq!(
        box_not_ready_message_for_error(&no_monitor),
        SAND_BOX_NO_MONITOR_AVAILABLE_MESSAGE
    );

    let timeout = SandBoxDaemonUnreachableError::new("timeout", "timed out");
    assert_eq!(
        box_not_ready_message_for_error(&timeout),
        SAND_BOX_NOT_RESPONDING_MESSAGE
    );
    let refused = SandBoxDaemonUnreachableError::new("refused", "connection refused");
    assert_eq!(
        box_not_ready_message_for_error(&refused),
        SAND_BOX_NOT_READY_MESSAGE
    );

    assert!(is_primary_window_index(0));
    assert!(is_primary_window_index(1));
    assert!(!is_primary_window_index(SAND_BOX_FIRST_FORK_WINDOW_INDEX));

    assert!(is_no_monitor_computer_use_executor(
        &NO_MONITOR_COMPUTER_USE_EXECUTOR
    ));
    let detached = NoMonitorComputerUseExecutor::detached();
    assert!(!is_no_monitor_computer_use_executor(&detached));
    let denied: Result<(), _> = NO_MONITOR_COMPUTER_USE_EXECUTOR.execute();
    assert!(denied.is_err());
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FakeConnectError {
    code: Option<ConnectErrorCode>,
    message: &'static str,
}

impl fmt::Display for FakeConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for FakeConnectError {}

impl ConnectErrorCodeSource for FakeConnectError {
    fn connect_error_code(&self) -> Option<ConnectErrorCode> {
        self.code.clone()
    }
}

struct RecordingMcpTransport {
    calls: Rc<RefCell<Vec<BoxMcpLoadRequest>>>,
    result: Result<Vec<String>, FakeConnectError>,
}

struct RecordingMcpClient {
    calls: Rc<RefCell<Vec<BoxMcpLoadRequest>>>,
    result: Result<Vec<String>, FakeConnectError>,
}

impl BoxMcpControlClient<String> for RecordingMcpClient {
    type Error = FakeConnectError;

    fn load_mcp_servers(
        &mut self,
        _ctx: &String,
        request: BoxMcpLoadRequest,
    ) -> Result<BoxMcpLoadResponse, Self::Error> {
        self.calls.borrow_mut().push(request);
        self.result
            .clone()
            .map(|loaded_server_names| BoxMcpLoadResponse { loaded_server_names })
    }
}

#[test]
fn box_mcp_port_forwards_config_and_classifies_unimplemented_connect_errors() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let transport = RecordingMcpTransport {
        calls: Rc::clone(&calls),
        result: Ok(vec!["filesystem".into(), "browser".into()]),
    };
    let loaded = load_box_mcp_servers_via_transport(
        &"ctx".to_string(),
        &transport,
        r#"{"mcpServers":{}}"#,
        |transport| RecordingMcpClient {
            calls: Rc::clone(&transport.calls),
            result: transport.result.clone(),
        },
    )
    .expect("MCP load should succeed");
    assert_eq!(loaded, vec!["filesystem", "browser"]);
    assert_eq!(
        calls.borrow().as_slice(),
        &[BoxMcpLoadRequest {
            mcp_config_json: r#"{"mcpServers":{}}"#.into(),
            remove_missing: true,
        }]
    );

    let unsupported = RecordingMcpTransport {
        calls: Rc::new(RefCell::new(Vec::new())),
        result: Err(FakeConnectError {
            code: Some(ConnectErrorCode::Number(12)),
            message: "unimplemented",
        }),
    };
    let error = load_box_mcp_servers_via_transport(
        &"ctx".to_string(),
        &unsupported,
        "{}",
        |transport| RecordingMcpClient {
            calls: Rc::clone(&transport.calls),
            result: transport.result.clone(),
        },
    )
    .expect_err("connect UNIMPLEMENTED must map to the box compatibility error");
    assert!(matches!(error, BoxMcpLoadError::Unsupported { .. }));
}

#[test]
fn box_store_backend_policy_matches_local_v2_default_and_cached_env_identity() {
    let mut environment = BTreeMap::new();
    assert_eq!(
        resolve_box_store_backend_policy(&environment).kind,
        BoxStoreBackendKind::AgentStore
    );

    environment.insert(SAND_BOX_STORE_BACKEND_ENV.into(), " V2 ".into());
    assert_eq!(
        resolve_box_store_backend_policy(&environment).kind,
        BoxStoreBackendKind::SandBoxStoreV2
    );

    let local_dir = std::env::temp_dir().join("fabushi-box-store");
    environment.insert(
        SAND_BOX_STORE_LOCAL_DIR_ENV.into(),
        local_dir.to_string_lossy().into_owned(),
    );
    let local = resolve_box_store_backend_policy(&environment);
    assert_eq!(local.kind, BoxStoreBackendKind::LocalFs);
    assert_eq!(
        local.local_dir.as_deref(),
        Some(local_dir.as_path())
    );

    let mut cache = BoxStoreBackendPolicyCache::default();
    let cached = cache.get(&environment);
    environment.remove(SAND_BOX_STORE_LOCAL_DIR_ENV);
    environment.insert(SAND_BOX_STORE_BACKEND_ENV.into(), "agent-store".into());
    assert_eq!(
        cache.get(&environment),
        cached,
        "the same environment object is cached like the Grok WeakMap policy"
    );

    environment.insert("SAND_BOX_STORE_SYNC".into(), "YES".into());
    environment.insert("SAND_BOX_STORE_COPY_IN".into(), "true".into());
    assert!(is_box_store_sync_enabled(&environment));
    assert!(is_box_store_copy_in_enabled(&environment));
}

#[test]
fn protected_path_guard_rejects_direct_and_relative_protected_paths() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-protected-path-{}-{suffix}",
        std::process::id()
    ));
    let protected = root.join("host-store");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&protected).expect("create protected root");
    std::fs::create_dir_all(&workspace).expect("create workspace");

    assert!(matches!(
        assert_path_outside_protected_roots(
            std::slice::from_ref(&protected),
            &protected.join("secret.db"),
            &workspace,
        ),
        Err(SandProtectedPathError(_))
    ));
    assert!(matches!(
        assert_path_outside_protected_roots(
            std::slice::from_ref(&protected),
            Path::new("../host-store/secret.db"),
            &workspace,
        ),
        Err(SandProtectedPathError(_))
    ));
    assert!(
        assert_path_outside_protected_roots(
            std::slice::from_ref(&protected),
            Path::new("safe/output.txt"),
            &workspace,
        )
        .is_ok()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let alias = root.join("alias");
        symlink(&protected, &alias).expect("create protected alias");
        assert!(matches!(
            assert_path_outside_protected_roots(
                std::slice::from_ref(&protected),
                &alias.join("nested").join("secret.db"),
                &workspace,
            ),
            Err(SandProtectedPathError(_))
        ));
    }

    std::fs::remove_dir_all(root).expect("remove protected-path fixture");
}
