//! End-to-End integration tests for Cross-Device & Cloud Computer Control,
//! per-bot configuration, ChaCha20Poly1305 encrypted credential persistence,
//! multi-device account sync, and AI Bot natural-language Cloud Server self-config.
//!
//! Spawns real `gpui_telegram_server` TCP listeners via `axum::serve` and exercises
//! multiple real `TelegramClient` instances against the same user account.

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use gpui_telegram_client::net::{
    AddContactHttpRequest, LoginRequest, RegisterRequest,
};
use gpui_telegram_client::{ClientConfig, TelegramClient};
use gpui_telegram_proto::computer_contracts::{
    AuthorizedControllerEntry, AuthorizedControllerKind, CloudAuthMethod,
    ComputerNodeDto, ComputerNodeKind, ComputerRunPolicy, RegisterLocalComputerRequest,
    RemoteComputerCommandRequest, RemoteComputerCommandResult, RemoteComputerOp,
    UpdateBotComputerRequest, UpdateComputerPolicyRequest, UpsertCloudComputerRequest,
};
use gpui_telegram_server::{build_router_with_state, config::ServerConfig, server::AppState};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const SAMPLE_PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO7+X8sAAAAASUVORK5CYII=";

struct RealServerHandle {
    addr: SocketAddr,
    db_path: PathBuf,
    task: JoinHandle<()>,
}

impl RealServerHandle {
    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    async fn shutdown(self) {
        self.task.abort();
        tokio::task::yield_now().await;
    }
}

impl Drop for RealServerHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_real_server(temp_dir: &TempDir) -> RealServerHandle {
    let db_path = temp_dir.path().join("server.db");
    let media_dir = temp_dir.path().join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");

    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;

    let state = Arc::new(AppState::try_new(cfg).expect("boot real AppState"));
    let app = build_router_with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind real gpui_telegram_server listener");
    let addr = listener.local_addr().expect("server local_addr");
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    RealServerHandle {
        addr,
        db_path,
        task,
    }
}

#[derive(Clone)]
struct AuxAgentState {
    expected_token: String,
    workdir: PathBuf,
    recorded_ops: Arc<Mutex<Vec<String>>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AuxExecPayload {
    Wrapped {
        operation: RemoteComputerOp,
        #[serde(default)]
        workdir: Option<String>,
        #[serde(default)]
        node_id: Option<String>,
        #[serde(default)]
        command_id: Option<String>,
    },
    Direct(RemoteComputerOp),
}

struct AuxAgentHandle {
    addr: SocketAddr,
    recorded_ops: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl AuxAgentHandle {
    fn endpoint_url(&self) -> String {
        format!("http://{}/api/v1/computer-agent/exec", self.addr)
    }

    fn ops_snapshot(&self) -> Vec<String> {
        self.recorded_ops.lock().expect("lock ops").clone()
    }

    async fn shutdown(self) {
        self.task.abort();
        tokio::task::yield_now().await;
    }
}

impl Drop for AuxAgentHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn resolve_aux_path(workdir: &Path, raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == "~" {
        return Ok(workdir.to_path_buf());
    }
    let candidate = PathBuf::from(trimmed);
    if candidate.is_absolute() {
        Ok(candidate)
    } else {
        Ok(workdir.join(candidate))
    }
}

async fn aux_agent_exec_handler(
    headers: HeaderMap,
    State(st): State<AuxAgentState>,
    Json(payload): Json<AuxExecPayload>,
) -> (StatusCode, Json<RemoteComputerCommandResult>) {
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");
    if provided != st.expected_token {
        return (
            StatusCode::UNAUTHORIZED,
            Json(RemoteComputerCommandResult {
                command_id: "unauth".to_string(),
                node_id: "cloud:aux".to_string(),
                ok: false,
                exit_code: 401,
                stdout: String::new(),
                stderr: "Unauthorized Bearer token".to_string(),
                entries: Vec::new(),
                frame_data_url: None,
                executed_by: "aux_cloud_agent".to_string(),
                duration_ms: 0,
            }),
        );
    }

    let (op, workdir_override, node_id, command_id) = match payload {
        AuxExecPayload::Wrapped {
            operation,
            workdir,
            node_id,
            command_id,
        } => (
            operation,
            workdir.filter(|s| !s.trim().is_empty()).map(PathBuf::from),
            node_id.unwrap_or_else(|| "cloud:aux".to_string()),
            command_id.unwrap_or_else(|| "aux-cmd".to_string()),
        ),
        AuxExecPayload::Direct(operation) => (
            operation,
            None,
            "cloud:aux".to_string(),
            "aux-cmd".to_string(),
        ),
    };

    let effective_workdir = workdir_override.unwrap_or_else(|| st.workdir.clone());
    let started = Instant::now();
    let mut res = RemoteComputerCommandResult {
        command_id,
        node_id,
        ok: false,
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
        entries: Vec::new(),
        frame_data_url: None,
        executed_by: "aux_cloud_agent".to_string(),
        duration_ms: 0,
    };

    match &op {
        RemoteComputerOp::TestConnection => {
            st.recorded_ops.lock().unwrap().push("test_connection".to_string());
            res.ok = true;
            res.exit_code = 0;
            res.stdout = "aux-agent-connected".to_string();
        }
        RemoteComputerOp::ShellExec { command, cwd, .. } => {
            st.recorded_ops
                .lock()
                .unwrap()
                .push(format!("shell_exec:{command}"));
            let run_cwd = cwd
                .as_deref()
                .and_then(|c| resolve_aux_path(&effective_workdir, c).ok())
                .unwrap_or_else(|| effective_workdir.clone());
            match tokio::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(command)
                .current_dir(&run_cwd)
                .output()
                .await
            {
                Ok(out) => {
                    res.exit_code = out.status.code().unwrap_or(if out.status.success() { 0 } else { 1 });
                    res.ok = out.status.success();
                    res.stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    res.stderr = String::from_utf8_lossy(&out.stderr).to_string();
                }
                Err(e) => {
                    res.stderr = format!("spawn sh failed: {e}");
                }
            }
        }
        RemoteComputerOp::WriteFile {
            path,
            content,
            create_dirs,
        } => {
            st.recorded_ops
                .lock()
                .unwrap()
                .push(format!("write_file:{path}"));
            match resolve_aux_path(&effective_workdir, path) {
                Ok(target) => {
                    if *create_dirs {
                        if let Some(parent) = target.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                    }
                    match std::fs::write(&target, content) {
                        Ok(()) => {
                            res.ok = true;
                            res.exit_code = 0;
                            res.stdout = format!("wrote {} bytes", content.len());
                        }
                        Err(e) => res.stderr = format!("write_file failed: {e}"),
                    }
                }
                Err(e) => res.stderr = e,
            }
        }
        RemoteComputerOp::ReadFile { path, .. } => {
            st.recorded_ops
                .lock()
                .unwrap()
                .push(format!("read_file:{path}"));
            match resolve_aux_path(&effective_workdir, path) {
                Ok(target) => match std::fs::read_to_string(&target) {
                    Ok(txt) => {
                        res.ok = true;
                        res.exit_code = 0;
                        res.stdout = txt;
                    }
                    Err(e) => res.stderr = format!("read_file failed: {e}"),
                },
                Err(e) => res.stderr = e,
            }
        }
        RemoteComputerOp::ListDir { path } => {
            st.recorded_ops
                .lock()
                .unwrap()
                .push(format!("list_dir:{path}"));
            match resolve_aux_path(&effective_workdir, path) {
                Ok(target) => match std::fs::read_dir(&target) {
                    Ok(rd) => {
                        let mut entries = Vec::new();
                        for entry in rd.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let tag = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                                "[dir]"
                            } else {
                                "[file]"
                            };
                            entries.push(format!("{tag} {name}"));
                        }
                        entries.sort();
                        res.ok = true;
                        res.exit_code = 0;
                        res.stdout = entries.join("\n");
                        res.entries = entries;
                    }
                    Err(e) => res.stderr = format!("list_dir failed: {e}"),
                },
                Err(e) => res.stderr = e,
            }
        }
        RemoteComputerOp::CaptureScreen => {
            st.recorded_ops.lock().unwrap().push("capture_screen".to_string());
            res.ok = true;
            res.exit_code = 0;
            res.stdout = "captured aux frame".to_string();
            res.frame_data_url = Some(SAMPLE_PNG_DATA_URL.to_string());
        }
        RemoteComputerOp::GuiInput { action, .. } => {
            st.recorded_ops
                .lock()
                .unwrap()
                .push(format!("gui_input:{action}"));
            res.ok = true;
            res.exit_code = 0;
            res.stdout = format!("gui_input ok: {action}");
        }
    }

    res.duration_ms = started.elapsed().as_millis() as u64;
    (StatusCode::OK, Json(res))
}

async fn spawn_aux_cloud_agent_server(
    expected_token: impl Into<String>,
    workdir: PathBuf,
) -> AuxAgentHandle {
    let recorded_ops = Arc::new(Mutex::new(Vec::new()));
    let state = AuxAgentState {
        expected_token: expected_token.into(),
        workdir,
        recorded_ops: Arc::clone(&recorded_ops),
    };
    let app = Router::new()
        .route("/api/v1/computer-agent/exec", post(aux_agent_exec_handler))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind aux cloud server");
    let addr = listener.local_addr().expect("aux cloud local_addr");
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    AuxAgentHandle {
        addr,
        recorded_ops,
        task,
    }
}

fn open_isolated_client(base_url: String, cache_dir: &Path, device_id: &str) -> Arc<TelegramClient> {
    std::fs::create_dir_all(cache_dir).expect("create client cache dir");
    let cfg = ClientConfig {
        server_url: base_url,
        http_timeout_secs: 20,
        ws_ping_interval_secs: 25,
        max_reconnect_attempts: 1,
        ..ClientConfig::default()
    };
    let client = Arc::new(TelegramClient::open_with_dir(cfg, cache_dir).expect("open TelegramClient"));
    client.set_device_id(device_id);
    client
}

async fn poll_bot_reply_text(
    client: &TelegramClient,
    chat_id: u64,
    bot_id: u64,
    after_msg_id: u64,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let msgs = client
            .get_chat_messages(chat_id, Some(30))
            .await
            .expect("get_chat_messages");
        if let Some(reply) = msgs
            .iter()
            .filter(|m| m.sender_id == bot_id && m.message_id > after_msg_id)
            .max_by_key(|m| m.message_id)
        {
            return reply.text.clone();
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for bot {bot_id} reply in chat {chat_id} after msg {after_msg_id}; msgs={msgs:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn build_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test tokio runtime")
}

struct BgPollerGuard {
    stop_flag: Arc<AtomicBool>,
    saw_expected_bot: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl BgPollerGuard {
    async fn stop(self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        self.task.abort();
        tokio::task::yield_now().await;
    }
}

impl Drop for BgPollerGuard {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        self.task.abort();
    }
}

fn spawn_weak_device_poller(
    client: &Arc<TelegramClient>,
    expected_bot_id: Option<u64>,
) -> BgPollerGuard {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_bg = Arc::clone(&stop_flag);
    let saw_expected_bot = Arc::new(AtomicBool::new(false));
    let saw_bg = Arc::clone(&saw_expected_bot);
    let weak_client = Arc::downgrade(client);
    let task = tokio::spawn(async move {
        while !stop_bg.load(Ordering::Relaxed) {
            let Some(strong) = weak_client.upgrade() else {
                break;
            };
            let res = strong.process_ready_local_computer_commands_async().await;
            drop(strong);
            if let (Ok(done), Some(bid)) = (res, expected_bot_id) {
                if done.iter().any(|c| c.caller_bot_id == Some(bid)) {
                    saw_bg.store(true, Ordering::Relaxed);
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    });
    BgPollerGuard {
        stop_flag,
        saw_expected_bot,
        task,
    }
}

// ============================================================================
// Scenario A — Cross-device Manual Remote Control (AskEveryTime + AlwaysAllowAuthorized)
// ============================================================================

#[test]
fn e2e_scenario_a_cross_device_manual_remote_control() {
    let temp_dir = tempfile::Builder::new()
        .prefix("e2e_computer_a_")
        .tempdir()
        .expect("tempdir");

    let rt = build_rt();
    let srv = rt.block_on(spawn_real_server(&temp_dir));

    // Create TelegramClients on the synchronous stack so their internal runtimes
    // are dropped strictly outside async runtime context.
    let client_a = open_isolated_client(
        srv.base_url(),
        &temp_dir.path().join("client_a"),
        "dev-a",
    );
    let client_b = open_isolated_client(
        srv.base_url(),
        &temp_dir.path().join("client_b"),
        "dev-b",
    );

    let ca = Arc::clone(&client_a);
    let cb = Arc::clone(&client_b);
    let sandbox_work = temp_dir.path().join("device_b_sandbox");
    std::fs::create_dir_all(&sandbox_work).expect("create sandbox workdir");

    rt.block_on(async move {
        ca.register(&RegisterRequest {
            username: "alice_scen_a".to_string(),
            password: "StrongPassword!123".to_string(),
            first_name: "Alice".to_string(),
            device_name: "Device-A".to_string(),
        })
        .await
        .expect("register alice");
        ca.set_device_id("dev-a");

        cb.login(&LoginRequest {
            username: "alice_scen_a".to_string(),
            password: "StrongPassword!123".to_string(),
            device_name: "Device-B".to_string(),
        })
        .await
        .expect("login device b");
        cb.set_device_id("dev-b");

        let node_a = ca
            .register_local_computer_async(&RegisterLocalComputerRequest {
                device_id: "dev-a".to_string(),
                display_name: "Workstation A".to_string(),
                ip_address: "10.0.0.10".to_string(),
                os_name: "macOS".to_string(),
                resolution: "1440x900".to_string(),
            })
            .await
            .expect("register dev-a");

        let node_b = cb
            .register_local_computer_async(&RegisterLocalComputerRequest {
                device_id: "dev-b".to_string(),
                display_name: "Studio B".to_string(),
                ip_address: "10.0.0.11".to_string(),
                os_name: "macOS".to_string(),
                resolution: "1920x1080".to_string(),
            })
            .await
            .expect("register dev-b");

        let list_a = ca.list_computers_async().await.expect("list_computers");
        let a_view = list_a
            .computers
            .iter()
            .find(|n| n.node_id == node_a.node_id)
            .expect("device A present");
        let b_view = list_a
            .computers
            .iter()
            .find(|n| n.node_id == node_b.node_id)
            .expect("device B present");
        assert!(a_view.is_current_device);
        assert!(a_view.online);
        assert!(!b_view.is_current_device);
        assert!(b_view.online);
        assert_eq!(b_view.run_policy, ComputerRunPolicy::AskEveryTime);

        // 1. AskEveryTime approval flow: A issues ShellExec -> B sees requires_user_approval, approves & processes
        let target_b_id = node_b.node_id.clone();
        let exec_req = RemoteComputerCommandRequest {
            node_id: target_b_id,
            caller_device_id: Some("dev-a".to_string()),
            caller_bot_id: None,
            caller_label: "Device A".to_string(),
            operation: RemoteComputerOp::ShellExec {
                command: "echo hello-from-b".to_string(),
                cwd: None,
                timeout_ms: Some(10_000),
            },
        };
        let exec_fut = ca.execute_remote_computer_op_async(&exec_req);
        let approve_fut = async {
            let pending = loop {
                let items = cb
                    .poll_local_computer_commands_async()
                    .await
                    .expect("poll dev-b");
                if let Some(cmd) = items.into_iter().next() {
                    break cmd;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            };
            assert!(pending.requires_user_approval);
            assert!(!pending.approved);

            let approved_cmd = cb
                .approve_computer_command_async(&pending.command_id, true)
                .await
                .expect("approve cmd on B");
            assert!(approved_cmd.approved);

            let processed = cb
                .process_ready_local_computer_commands_async()
                .await
                .expect("process approved cmd on B");
            assert!(processed.iter().any(|c| c.command_id == pending.command_id));
        };
        let (exec_res, ()) = tokio::join!(exec_fut, approve_fut);
        let exec_res = exec_res.expect("execute_remote_computer_op_async");
        assert!(exec_res.ok, "expected ShellExec ok, got: {exec_res:?}");
        assert!(
            exec_res.stdout.contains("hello-from-b"),
            "unexpected stdout: {}",
            exec_res.stdout
        );

        // 2. Update B policy -> AlwaysAllowAuthorized + authorize Device A & AccountAllBots
        cb.update_computer_policy_async(
            &node_b.node_id,
            &UpdateComputerPolicyRequest {
                run_policy: Some(ComputerRunPolicy::AlwaysAllowAuthorized),
                add_controller: Some(AuthorizedControllerEntry {
                    controller_id: "dev-a".to_string(),
                    kind: AuthorizedControllerKind::Device,
                    display_name: "Workstation A".to_string(),
                    added_at_ms: 0,
                }),
                remove_controller_id: None,
                display_name: None,
            },
        )
        .await
        .expect("authorize dev-a on node_b");

        let updated_b = cb
            .update_computer_policy_async(
                &node_b.node_id,
                &UpdateComputerPolicyRequest {
                    run_policy: None,
                    add_controller: Some(AuthorizedControllerEntry {
                        controller_id: "all_bots".to_string(),
                        kind: AuthorizedControllerKind::AccountAllBots,
                        display_name: "All Bots".to_string(),
                        added_at_ms: 0,
                    }),
                    remove_controller_id: None,
                    display_name: None,
                },
            )
            .await
            .expect("authorize all_bots on node_b");
        assert_eq!(updated_b.run_policy, ComputerRunPolicy::AlwaysAllowAuthorized);
        assert_eq!(updated_b.authorized_controllers.len(), 2);

        // Background poller on Device B auto-executes authorized ops without manual approval
        let bg_poller = spawn_weak_device_poller(&cb, None);

        let test_file = sandbox_work.join("cross_device_payload.txt");
        let write_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: node_b.node_id.clone(),
                caller_device_id: Some("dev-a".to_string()),
                caller_bot_id: None,
                caller_label: "Device A".to_string(),
                operation: RemoteComputerOp::WriteFile {
                    path: test_file.to_string_lossy().to_string(),
                    content: "multi-device-write-ok".to_string(),
                    create_dirs: true,
                },
            })
            .await
            .expect("WriteFile on B");
        assert!(write_res.ok, "WriteFile failed: {write_res:?}");

        let read_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: node_b.node_id.clone(),
                caller_device_id: Some("dev-a".to_string()),
                caller_bot_id: None,
                caller_label: "Device A".to_string(),
                operation: RemoteComputerOp::ReadFile {
                    path: test_file.to_string_lossy().to_string(),
                    max_bytes: None,
                },
            })
            .await
            .expect("ReadFile on B");
        assert!(read_res.ok);
        assert!(read_res.stdout.contains("multi-device-write-ok"));

        let list_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: node_b.node_id.clone(),
                caller_device_id: Some("dev-a".to_string()),
                caller_bot_id: None,
                caller_label: "Device A".to_string(),
                operation: RemoteComputerOp::ListDir {
                    path: sandbox_work.to_string_lossy().to_string(),
                },
            })
            .await
            .expect("ListDir on B");
        assert!(list_res.ok);
        assert!(
            list_res
                .entries
                .iter()
                .any(|e| e.contains("cross_device_payload.txt")),
            "expected written file in ListDir entries: {:?}",
            list_res.entries
        );

        let screen_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: node_b.node_id.clone(),
                caller_device_id: Some("dev-a".to_string()),
                caller_bot_id: None,
                caller_label: "Device A".to_string(),
                operation: RemoteComputerOp::CaptureScreen,
            })
            .await
            .expect("CaptureScreen on B");
        assert!(screen_res.ok, "CaptureScreen failed: {screen_res:?}");
        assert!(
            screen_res
                .frame_data_url
                .as_deref()
                .is_some_and(|u| u.starts_with("data:image/png;base64,")),
            "unexpected frame_data_url: {:?}",
            screen_res.frame_data_url
        );

        bg_poller.stop().await;
        srv.shutdown().await;
    });

    drop(rt);
}

// ============================================================================
// Scenario B — Every AI Bot Independently Configurable + Cross-Device Bot Exec
// ============================================================================

#[test]
fn e2e_scenario_b_per_bot_config_and_cross_device_bot_remote_exec() {
    let temp_dir = tempfile::Builder::new()
        .prefix("e2e_computer_b_")
        .tempdir()
        .expect("tempdir");

    let rt = build_rt();
    let srv = rt.block_on(spawn_real_server(&temp_dir));
    let client_a = open_isolated_client(
        srv.base_url(),
        &temp_dir.path().join("client_a"),
        "dev-a",
    );
    let client_b = open_isolated_client(
        srv.base_url(),
        &temp_dir.path().join("client_b"),
        "dev-b",
    );

    let ca = Arc::clone(&client_a);
    let cb = Arc::clone(&client_b);

    rt.block_on(async move {
        ca.register(&RegisterRequest {
            username: "alice_scen_b".to_string(),
            password: "StrongPassword!123".to_string(),
            first_name: "Alice".to_string(),
            device_name: "Device-A".to_string(),
        })
        .await
        .expect("register alice");
        ca.set_device_id("dev-a");

        cb.login(&LoginRequest {
            username: "alice_scen_b".to_string(),
            password: "StrongPassword!123".to_string(),
            device_name: "Device-B".to_string(),
        })
        .await
        .expect("login device b");
        cb.set_device_id("dev-b");

        let node_b = cb
            .register_local_computer_async(&RegisterLocalComputerRequest {
                device_id: "dev-b".to_string(),
                display_name: "Studio B".to_string(),
                ip_address: "10.0.0.22".to_string(),
                os_name: "macOS".to_string(),
                resolution: "1920x1080".to_string(),
            })
            .await
            .expect("register dev-b");

        // Authorize All Bots + AlwaysAllowAuthorized on Device B
        cb.update_computer_policy_async(
            &node_b.node_id,
            &UpdateComputerPolicyRequest {
                run_policy: Some(ComputerRunPolicy::AlwaysAllowAuthorized),
                add_controller: Some(AuthorizedControllerEntry {
                    controller_id: "all_bots".to_string(),
                    kind: AuthorizedControllerKind::AccountAllBots,
                    display_name: "All Account Bots".to_string(),
                    added_at_ms: 0,
                }),
                remove_controller_id: None,
                display_name: None,
            },
        )
        .await
        .expect("authorize all bots on B");

        // Create two distinct AI bots
        let bot_1 = ca
            .add_contact(AddContactHttpRequest {
                first_name: "Alpha Ops Bot".to_string(),
                last_name: None,
                username: Some("alpha_ops_bot".to_string()),
                phone: None,
                is_bot: true,
                bot_system_prompt: Some("You are Alpha Ops Bot.".to_string()),
            })
            .await
            .expect("create bot_1");

        let bot_2 = ca
            .add_contact(AddContactHttpRequest {
                first_name: "Beta Research Bot".to_string(),
                last_name: None,
                username: Some("beta_research_bot".to_string()),
                phone: None,
                is_bot: true,
                bot_system_prompt: Some("You are Beta Research Bot.".to_string()),
            })
            .await
            .expect("create bot_2");

        // Independently configure bot_1 (bound to Device B) and bot_2 (sandbox, AllowReadOnly)
        let saved_p1 = ca
            .update_bot_computer_profile_async(
                bot_1.user_id,
                &UpdateBotComputerRequest {
                    bound_node_id: Some(node_b.node_id.clone()),
                    clear_bound_node: false,
                    persona_prompt: Some("Alpha Persona Custom".to_string()),
                    preferred_model: Some("hermes-alpha-1".to_string()),
                    approval_mode: Some(ComputerRunPolicy::AlwaysAllowAuthorized),
                    allow_cross_device_control: Some(true),
                    allow_cloud_self_config: Some(true),
                },
            )
            .await
            .expect("update bot_1 profile");
        assert_eq!(saved_p1.bound_node_id.as_deref(), Some(node_b.node_id.as_str()));
        assert_eq!(saved_p1.persona_prompt, "Alpha Persona Custom");
        assert_eq!(saved_p1.preferred_model, "hermes-alpha-1");

        let saved_p2 = ca
            .update_bot_computer_profile_async(
                bot_2.user_id,
                &UpdateBotComputerRequest {
                    bound_node_id: None,
                    clear_bound_node: false,
                    persona_prompt: Some("Beta Persona Custom".to_string()),
                    preferred_model: Some("hermes-beta-2".to_string()),
                    approval_mode: Some(ComputerRunPolicy::AllowReadOnly),
                    allow_cross_device_control: Some(false),
                    allow_cloud_self_config: Some(true),
                },
            )
            .await
            .expect("update bot_2 profile");
        assert_eq!(saved_p2.persona_prompt, "Beta Persona Custom");
        assert_eq!(saved_p2.preferred_model, "hermes-beta-2");

        // Verify per-bot settings did not clobber each other
        let fetched_p1 = ca
            .get_bot_computer_profile_async(bot_1.user_id)
            .await
            .expect("get bot_1 profile");
        let fetched_p2 = ca
            .get_bot_computer_profile_async(bot_2.user_id)
            .await
            .expect("get bot_2 profile");
        assert_eq!(fetched_p1.bound_node_id.as_deref(), Some(node_b.node_id.as_str()));
        assert_eq!(fetched_p1.persona_prompt, "Alpha Persona Custom");
        assert_eq!(fetched_p2.bound_node_id, None);
        assert_eq!(fetched_p2.persona_prompt, "Beta Persona Custom");
        assert!(!fetched_p2.allow_cross_device_control);

        // Background poller on Device B tracks caller_bot_id execution & runs commands
        let bg_poller = spawn_weak_device_poller(&cb, Some(bot_1.user_id));

        let before_msgs = ca
            .get_chat_messages(bot_1.chat_id, Some(20))
            .await
            .expect("msgs before");
        let max_id_before = before_msgs.iter().map(|m| m.message_id).max().unwrap_or(0);

        // Send message on Device A to bot_1 instructing it to run a command on its bound computer (Device B)
        ca.send_text_message(
            bot_1.chat_id as i64,
            "帮我在电脑上运行 `echo bot1-bound-exec-ok`",
            None,
        )
        .await
        .expect("send chat command to bot_1");

        let reply_text = poll_bot_reply_text(&ca, bot_1.chat_id, bot_1.user_id, max_id_before).await;
        assert!(
            reply_text.contains("bot1-bound-exec-ok"),
            "expected bot_1 reply to include real command output from Device B, got: {reply_text}"
        );
        assert!(
            bg_poller.saw_expected_bot.load(Ordering::Relaxed),
            "Device B poller must have observed caller_bot_id == Some({})",
            bot_1.user_id
        );

        bg_poller.stop().await;
        srv.shutdown().await;
    });

    drop(rt);
}

// ============================================================================
// Scenario C — Manual Cloud Server + ChaCha20Poly1305 SQLite + Multi-Device Sync
// ============================================================================

#[test]
fn e2e_scenario_c_cloud_server_encrypted_sqlite_and_cross_device_sync() {
    let temp_dir = tempfile::Builder::new()
        .prefix("e2e_computer_c_")
        .tempdir()
        .expect("tempdir");

    let rt = build_rt();
    let srv = rt.block_on(spawn_real_server(&temp_dir));

    let aux_workdir = temp_dir.path().join("aux_cloud_c_work");
    std::fs::create_dir_all(&aux_workdir).expect("create aux_workdir");
    let secret_token = "cloud-secret-token-xyz";
    let aux = rt.block_on(spawn_aux_cloud_agent_server(
        secret_token,
        aux_workdir.clone(),
    ));

    let client_a = open_isolated_client(
        srv.base_url(),
        &temp_dir.path().join("client_a"),
        "dev-a",
    );
    let client_c = open_isolated_client(
        srv.base_url(),
        &temp_dir.path().join("client_c"),
        "dev-c",
    );

    let ca = Arc::clone(&client_a);
    let cc = Arc::clone(&client_c);
    let db_path = srv.db_path.clone();

    rt.block_on(async move {
        ca.register(&RegisterRequest {
            username: "alice_scen_c".to_string(),
            password: "StrongPassword!123".to_string(),
            first_name: "Alice".to_string(),
            device_name: "Device-A".to_string(),
        })
        .await
        .expect("register alice");
        ca.set_device_id("dev-a");

        // 1. Device A creates & verifies CloudServer
        let saved_cloud: ComputerNodeDto = ca
            .upsert_cloud_computer_async(&UpsertCloudComputerRequest {
                node_id: None,
                display_name: "Prod Cloud Box".to_string(),
                host: "127.0.0.1".to_string(),
                port: aux.addr.port(),
                username: "deploy".to_string(),
                auth_method: CloudAuthMethod::AgentHttpToken,
                secret: Some(secret_token.to_string()),
                agent_endpoint_url: Some(aux.endpoint_url()),
                workdir: Some(aux_workdir.to_string_lossy().to_string()),
                bind_bot_id: None,
                verify_connection: true,
            })
            .await
            .expect("upsert verified CloudServer");
        assert_eq!(saved_cloud.kind, ComputerNodeKind::CloudServer);
        assert!(saved_cloud.has_saved_credential);
        assert!(saved_cloud.online);
        assert!(
            aux.ops_snapshot().iter().any(|op| op == "test_connection"),
            "verify_connection must execute TestConnection probe on cloud server: {:?}",
            aux.ops_snapshot()
        );

        // 2. Inspect raw SQLite computer_nodes table: encrypted_secret non-empty & NO plaintext leak
        let conn = rusqlite::Connection::open(&db_path).expect("open raw server.db");
        let (enc_secret, nonce): (Option<Vec<u8>>, Option<Vec<u8>>) = conn
            .query_row(
                "SELECT encrypted_secret, secret_nonce FROM computer_nodes WHERE node_id = ?1",
                rusqlite::params![saved_cloud.node_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query raw computer_nodes row");
        drop(conn);

        let enc_bytes = enc_secret.expect("encrypted_secret must be stored");
        let nonce_bytes = nonce.expect("secret_nonce must be stored");
        assert!(!enc_bytes.is_empty(), "encrypted_secret cannot be empty");
        assert_eq!(nonce_bytes.len(), 12, "ChaCha20Poly1305 uses 12-byte nonce");
        assert!(
            !enc_bytes
                .windows(secret_token.len())
                .any(|w| w == secret_token.as_bytes()),
            "raw SQLite encrypted_secret must NOT contain plaintext token!"
        );

        // 3. Log in on fresh Device C -> cloud-synced CloudServer immediately appears, without secret leak
        cc.login(&LoginRequest {
            username: "alice_scen_c".to_string(),
            password: "StrongPassword!123".to_string(),
            device_name: "Device-C".to_string(),
        })
        .await
        .expect("login device c");
        cc.set_device_id("dev-c");

        let list_c = cc.list_computers_async().await.expect("list_computers on C");
        let list_c_json = serde_json::to_string(&list_c).expect("serialize list_c");
        assert!(
            !list_c_json.contains(secret_token),
            "list_computers DTO leaked raw cloud secret!"
        );
        let synced_cloud = list_c
            .computers
            .iter()
            .find(|n| n.node_id == saved_cloud.node_id)
            .expect("CloudServer synced to fresh Device C");
        assert!(synced_cloud.has_saved_credential);

        // 4. Device C manually runs ShellExec / WriteFile / ReadFile on the synced CloudServer
        let shell_c = cc
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: synced_cloud.node_id.clone(),
                caller_device_id: Some("dev-c".to_string()),
                caller_bot_id: None,
                caller_label: "Device C".to_string(),
                operation: RemoteComputerOp::ShellExec {
                    command: "echo cloud-exec-from-dev-c".to_string(),
                    cwd: None,
                    timeout_ms: Some(10_000),
                },
            })
            .await
            .expect("ShellExec on CloudServer from C");
        assert!(shell_c.ok);
        assert!(shell_c.stdout.contains("cloud-exec-from-dev-c"));

        let write_c = cc
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: synced_cloud.node_id.clone(),
                caller_device_id: Some("dev-c".to_string()),
                caller_bot_id: None,
                caller_label: "Device C".to_string(),
                operation: RemoteComputerOp::WriteFile {
                    path: "synced_from_c.txt".to_string(),
                    content: "cloud-file-content-99".to_string(),
                    create_dirs: true,
                },
            })
            .await
            .expect("WriteFile on CloudServer");
        assert!(write_c.ok);

        let read_c = cc
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: synced_cloud.node_id.clone(),
                caller_device_id: Some("dev-c".to_string()),
                caller_bot_id: None,
                caller_label: "Device C".to_string(),
                operation: RemoteComputerOp::ReadFile {
                    path: "synced_from_c.txt".to_string(),
                    max_bytes: None,
                },
            })
            .await
            .expect("ReadFile on CloudServer");
        assert!(read_c.ok);
        assert_eq!(read_c.stdout, "cloud-file-content-99");

        // 5. Bind bot_2 to CloudServer and verify Device C has bot_2 operate CloudServer via chat
        let bot_2 = cc
            .add_contact(AddContactHttpRequest {
                first_name: "Cloud Runner Bot".to_string(),
                last_name: None,
                username: Some("cloud_runner_bot".to_string()),
                phone: None,
                is_bot: true,
                bot_system_prompt: Some("You are Cloud Runner Bot.".to_string()),
            })
            .await
            .expect("create bot_2");

        cc.update_bot_computer_profile_async(
            bot_2.user_id,
            &UpdateBotComputerRequest {
                bound_node_id: Some(synced_cloud.node_id.clone()),
                clear_bound_node: false,
                persona_prompt: None,
                preferred_model: None,
                approval_mode: Some(ComputerRunPolicy::AlwaysAllowAuthorized),
                allow_cross_device_control: Some(true),
                allow_cloud_self_config: Some(true),
            },
        )
        .await
        .expect("bind bot_2 to CloudServer");

        let before = cc
            .get_chat_messages(bot_2.chat_id, Some(20))
            .await
            .expect("msgs before");
        let after_id = before.iter().map(|m| m.message_id).max().unwrap_or(0);

        cc.send_text_message(
            bot_2.chat_id as i64,
            "在云电脑上运行 `echo bot2-on-cloud-ok`",
            None,
        )
        .await
        .expect("ask bot_2 to execute on cloud");

        let reply = poll_bot_reply_text(&cc, bot_2.chat_id, bot_2.user_id, after_id).await;
        assert!(
            reply.contains("bot2-on-cloud-ok"),
            "expected bot_2 chat reply to include CloudServer stdout, got: {reply}"
        );

        aux.shutdown().await;
        srv.shutdown().await;
    });

    drop(rt);
}

// ============================================================================
// Scenario D — AI Bot Self-Configures Cloud Server via Chat + Multi-Device Sync
// ============================================================================

#[test]
fn e2e_scenario_d_bot_self_configures_cloud_server_and_syncs_across_devices() {
    let temp_dir = tempfile::Builder::new()
        .prefix("e2e_computer_d_")
        .tempdir()
        .expect("tempdir");

    let rt = build_rt();
    let srv = rt.block_on(spawn_real_server(&temp_dir));

    let aux_workdir = temp_dir.path().join("aux_cloud_d_work");
    std::fs::create_dir_all(&aux_workdir).expect("create aux_workdir d");
    let secret_token = "auto-config-token-999";
    let aux = rt.block_on(spawn_aux_cloud_agent_server(
        secret_token,
        aux_workdir.clone(),
    ));

    let client_a = open_isolated_client(
        srv.base_url(),
        &temp_dir.path().join("client_a"),
        "dev-a",
    );
    let client_c = open_isolated_client(
        srv.base_url(),
        &temp_dir.path().join("client_c"),
        "dev-c",
    );

    let ca = Arc::clone(&client_a);
    let cc = Arc::clone(&client_c);

    rt.block_on(async move {
        ca.register(&RegisterRequest {
            username: "alice_scen_d".to_string(),
            password: "StrongPassword!123".to_string(),
            first_name: "Alice".to_string(),
            device_name: "Device-A".to_string(),
        })
        .await
        .expect("register alice");
        ca.set_device_id("dev-a");

        let self_bot = ca
            .add_contact(AddContactHttpRequest {
                first_name: "AutoCloud Bot".to_string(),
                last_name: None,
                username: Some("autocloud_bot".to_string()),
                phone: None,
                is_bot: true,
                bot_system_prompt: Some("You help configure and manage cloud servers.".to_string()),
            })
            .await
            .expect("create autocloud bot");

        let before = ca
            .get_chat_messages(self_bot.chat_id, Some(20))
            .await
            .expect("msgs before");
        let after_id = before.iter().map(|m| m.message_id).max().unwrap_or(0);

        // User sends natural-language prompt providing URL, port, and Bearer token
        let prompt = format!(
            "请帮我配置并绑定这台云电脑：name: SelfConfigGPU agent_url: {} host: 127.0.0.1 port: {} user: ubuntu token: {} workdir: {}",
            aux.endpoint_url(),
            aux.addr.port(),
            secret_token,
            aux_workdir.to_string_lossy()
        );
        ca.send_text_message(self_bot.chat_id as i64, prompt, None)
            .await
            .expect("send self-config prompt");

        let reply = poll_bot_reply_text(&ca, self_bot.chat_id, self_bot.user_id, after_id).await;
        assert!(
            !reply.contains(secret_token),
            "bot chat reply MUST redact raw token! reply={reply}"
        );
        assert!(
            reply.contains("SelfConfigGPU"),
            "expected confirmation of configured cloud server in reply, got: {reply}"
        );

        // Verify the bot verified connectivity via TestConnection before persisting
        assert!(
            aux.ops_snapshot().iter().any(|op| op == "test_connection"),
            "AI Bot cloud self-config MUST run TestConnection probe prior to saving! ops={:?}",
            aux.ops_snapshot()
        );

        // Verify bot bound itself to the newly configured CloudServer
        let profile = ca
            .get_bot_computer_profile_async(self_bot.user_id)
            .await
            .expect("get bot profile after self-config");
        let bound_id = profile
            .bound_node_id
            .expect("AI Bot should bind itself to SelfConfigGPU");

        // Fresh Device C logs in and immediately sees & operates the AI-configured CloudServer
        cc.login(&LoginRequest {
            username: "alice_scen_d".to_string(),
            password: "StrongPassword!123".to_string(),
            device_name: "Device-C".to_string(),
        })
        .await
        .expect("login device c");
        cc.set_device_id("dev-c");

        let list_c = cc.list_computers_async().await.expect("list_computers on C");
        assert!(
            !serde_json::to_string(&list_c)
                .unwrap()
                .contains(secret_token),
            "list_computers must never leak secret token"
        );
        let node_on_c = list_c
            .computers
            .iter()
            .find(|n| n.node_id == bound_id)
            .expect("Device C immediately sees AI-configured CloudServer");
        assert_eq!(node_on_c.display_name, "SelfConfigGPU");
        assert!(node_on_c.has_saved_credential);
        assert!(node_on_c.bound_bot_ids.contains(&self_bot.user_id));

        let exec_on_c = cc
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: node_on_c.node_id.clone(),
                caller_device_id: Some("dev-c".to_string()),
                caller_bot_id: None,
                caller_label: "Device C".to_string(),
                operation: RemoteComputerOp::ShellExec {
                    command: "echo device-c-verified-self-config".to_string(),
                    cwd: None,
                    timeout_ms: Some(10_000),
                },
            })
            .await
            .expect("Device C runs remote command on AI-configured cloud server");
        assert!(exec_on_c.ok);
        assert!(exec_on_c.stdout.contains("device-c-verified-self-config"));

        aux.shutdown().await;
        srv.shutdown().await;
    });

    drop(rt);
}
