//! End-to-End integration tests for the built-in Tailscale-style userspace
//! Tailnet mesh: persistent cryptographic identity & zombie cleanup, poll/DERP
//! liveness past the 60s TTL, Magicsock Disco hole-punching Direct UDP with
//! ChaCha20Poly1305 AEAD & TSFR fragmentation, DERP WebSocket E2E fallback &
//! tenant isolation, AskEveryTime P2P interactive approval/denial, and instant
//! Server-to-Device AI Bot execution over DERP push.

use futures::{SinkExt, StreamExt};
use gpui_telegram_client::net::{AddContactHttpRequest, LoginRequest, RegisterRequest};
use gpui_telegram_client::{ClientConfig, TelegramClient};
use gpui_telegram_proto::computer_contracts::{
    AuthorizedControllerEntry, AuthorizedControllerKind, ComputerNodeDto, ComputerNodeKind,
    ComputerRunPolicy, PendingComputerCommandDto, RegisterLocalComputerRequest,
    RemoteComputerCommandRequest, RemoteComputerOp, UpdateBotComputerRequest,
    UpdateComputerPolicyRequest,
};
use gpui_telegram_proto::tailnet_contracts::{
    DerpClientFrame, DerpServerFrame, TailnetNetMap, TailnetNodeStatus, TailnetPeerLiveState,
    TailnetRegisterRequest, TailnetRelayMode,
};
use gpui_telegram_server::{build_router_with_state, config::ServerConfig, server::AppState};
use rusqlite::params;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMsg};

struct RealServerHandle {
    addr: SocketAddr,
    db_path: PathBuf,
    task: JoinHandle<()>,
}

impl RealServerHandle {
    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn ws_base_url(&self) -> String {
        format!("ws://{}", self.addr)
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

fn build_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test tokio runtime")
}

fn open_persistent_client(base_url: String, cache_dir: &Path) -> Arc<TelegramClient> {
    std::fs::create_dir_all(cache_dir).expect("create client cache dir");
    let cfg = ClientConfig {
        server_url: base_url,
        http_timeout_secs: 20,
        ws_ping_interval_secs: 25,
        max_reconnect_attempts: 1,
        ..ClientConfig::default()
    };
    Arc::new(TelegramClient::open_with_dir(cfg, cache_dir).expect("open TelegramClient"))
}

fn assert_valid_rfc6598_tailnet_ip(ip_str: &str) {
    let parsed: Ipv4Addr = ip_str
        .parse()
        .unwrap_or_else(|e| panic!("invalid IPv4 tailnet_ip '{ip_str}': {e}"));
    let octets = parsed.octets();
    assert_eq!(octets[0], 100, "tailnet_ip must be 100.64/10: {ip_str}");
    assert!(
        (64..=127).contains(&octets[1]),
        "tailnet_ip second octet must be in 64..=127: {ip_str}"
    );
    assert!(
        (1..=254).contains(&octets[3]),
        "tailnet_ip host octet must be in 1..=254: {ip_str}"
    );
}

fn count_audit_rows(db_path: &Path, owner_user_id: u64) -> i64 {
    let conn = rusqlite::Connection::open(db_path).expect("open server db");
    conn.query_row(
        "SELECT COUNT(*) FROM computer_command_audit WHERE owner_user_id = ?1",
        params![owner_user_id as i64],
        |row| row.get(0),
    )
    .expect("query computer_command_audit count")
}

async fn wait_until_derp_ready(client: &TelegramClient) -> TailnetNodeStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let st = client.tailnet_status();
        if st.running && st.derp_connected && !st.tailnet_ip.is_empty() {
            return st;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for Tailnet DERP ready; last status={st:?}"
        );
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

async fn wait_until_peer_relay_mode(
    client: &TelegramClient,
    peer_device_id: &str,
    expected_mode: TailnetRelayMode,
) -> TailnetPeerLiveState {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let st = client.tailnet_status();
        if let Some(peer) = st
            .peers
            .iter()
            .find(|p| p.device_id == peer_device_id && p.online && p.relay_mode == expected_mode)
        {
            return peer.clone();
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for peer '{peer_device_id}' to reach {expected_mode:?}; status={st:?}"
        );
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

async fn wait_until_netmap_policy(
    client: &TelegramClient,
    expected_policy: ComputerRunPolicy,
) -> TailnetNetMap {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(nm) = client.get_tailnet_netmap_async().await {
            if nm
                .self_peer
                .as_ref()
                .is_some_and(|sp| sp.run_policy == expected_policy)
            {
                return nm;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for self_peer run_policy={expected_policy:?}"
        );
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
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

// ============================================================================
// Test 1 — Persistent Tailnet Identity, Zero Duplicate Nodes, Legacy Zombie Pruning
// ============================================================================

#[test]
fn test_persistent_tailnet_identity_eliminates_duplicate_devices_and_prunes_legacy_zombies() {
    let temp_dir = tempfile::Builder::new()
        .prefix("e2e_tailnet_id_")
        .tempdir()
        .expect("tempdir");

    let rt = build_rt();
    let srv = rt.block_on(spawn_real_server(&temp_dir));
    let cache_a = temp_dir.path().join("persistent_device_a");

    // Phase 1: Boot client_a1 from persistent cache_a and register on Tailnet.
    let client_a1 = open_persistent_client(srv.base_url(), &cache_a);
    let (user_id, dev_id_1, mpk_1, npk_1, node_id_1, tailnet_ip_1, token_1) = {
        let ca1 = Arc::clone(&client_a1);
        let db_path = srv.db_path.clone();
        let cache_dir_check = cache_a.clone();

        rt.block_on(async move {
            let auth = ca1
                .register(&RegisterRequest {
                    username: "alice_tailnet_id".to_string(),
                    password: "StrongPassword!123".to_string(),
                    first_name: "Alice".to_string(),
                    device_name: "MacBook-A".to_string(),
                })
                .await
                .expect("register alice");

            let dev_id = ca1.device_id();
            assert!(
                dev_id.starts_with("dev-ts-"),
                "persistent device_id must start with dev-ts-, got: {dev_id}"
            );

            let id_file = cache_dir_check.join("tailnet_identity.json");
            assert!(id_file.is_file(), "tailnet_identity.json must exist");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&id_file)
                    .expect("stat identity")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600, "tailnet_identity.json must be 0600");
            }

            let netmap = ca1
                .register_tailnet_async()
                .await
                .expect("signed tailnet registration");
            let self_peer = netmap.self_peer.expect("self_peer in netmap");
            assert_eq!(self_peer.device_id, dev_id);
            assert_valid_rfc6598_tailnet_ip(&self_peer.tailnet_ip);

            let list = ca1.list_computers_async().await.expect("list_computers");
            let listed_a = list
                .computers
                .iter()
                .find(|c| c.node_id == self_peer.node_id)
                .expect("registered local node present");
            assert_eq!(listed_a.host.as_deref(), Some(self_peer.tailnet_ip.as_str()));

            // Seed:
            // 1) A stale legacy ephemeral zombie (machine_public_key IS NULL, >60s old, matching LAN IP/OS)
            //    with a bot_computer_configs binding pointing to it.
            // 2) Another valid Tailnet peer (non-empty machine_public_key) that must NOT be pruned.
            let stale_ts = chrono::Utc::now().timestamp_millis() - 120_000;
            let conn = rusqlite::Connection::open(&db_path).expect("open db");
            conn.execute(
                r#"
                INSERT INTO computer_nodes (
                    node_id, owner_user_id, kind, display_name, ip_address, host,
                    os_name, resolution, device_id, run_policy,
                    authorized_controllers_json, last_seen_ms, created_at_ms, updated_at_ms,
                    tailnet_ip, machine_public_key, node_public_key, disco_public_key, endpoints_json
                ) VALUES (
                    ?1, ?2, 'local_device', ?3, ?4, NULL,
                    ?5, '1440x900', 'legacy-uuid-zombie', 'ask_every_time',
                    '[]', ?6, ?6, ?6,
                    NULL, NULL, NULL, NULL, '[]'
                )
                "#,
                params![
                    format!("local:{}:legacy-uuid-zombie", auth.user_id),
                    auth.user_id as i64,
                    &self_peer.display_name,
                    &self_peer.lan_ip,
                    &self_peer.os_name,
                    stale_ts,
                ],
            )
            .expect("insert legacy zombie row");

            conn.execute(
                r#"
                INSERT INTO bot_computer_configs (
                    owner_user_id, bot_id, bound_node_id, persona_prompt, preferred_model,
                    approval_mode, allow_cross_device_control, allow_cloud_self_config, updated_at_ms
                ) VALUES (?1, 9001, ?2, '', '', 'always_allow_authorized', 1, 1, ?3)
                "#,
                params![
                    auth.user_id as i64,
                    format!("local:{}:legacy-uuid-zombie", auth.user_id),
                    stale_ts,
                ],
            )
            .expect("insert bot binding referencing zombie");

            conn.execute(
                r#"
                INSERT INTO computer_nodes (
                    node_id, owner_user_id, kind, display_name, ip_address, host,
                    os_name, resolution, device_id, run_policy,
                    authorized_controllers_json, last_seen_ms, created_at_ms, updated_at_ms,
                    tailnet_ip, machine_public_key, node_public_key, disco_public_key, endpoints_json
                ) VALUES (
                    ?1, ?2, 'local_device', 'Other Mac', '10.99.99.99', '100.64.99.99',
                    'macOS', '1920x1080', 'dev-ts-otherpeer1234', 'ask_every_time',
                    '[]', ?3, ?3, ?3,
                    '100.64.99.99', ?4, ?5, ?6, '[]'
                )
                "#,
                params![
                    format!("local:{}:dev-ts-otherpeer1234", auth.user_id),
                    auth.user_id as i64,
                    stale_ts,
                    "11".repeat(32),
                    "22".repeat(32),
                    "33".repeat(32),
                ],
            )
            .expect("insert valid peer row");

            let st = ca1.tailnet_status();
            (
                auth.user_id,
                dev_id,
                st.machine_public_key,
                st.node_public_key,
                self_peer.node_id,
                self_peer.tailnet_ip,
                auth.token,
            )
        })
    };
    drop(client_a1);

    // Phase 2: Simulate process restart — reopen from the SAME cache_a directory.
    let client_a2 = open_persistent_client(srv.base_url(), &cache_a);
    let ca2 = Arc::clone(&client_a2);
    let base_url = srv.base_url();
    let db_path = srv.db_path.clone();

    rt.block_on(async move {
        assert_eq!(ca2.device_id(), dev_id_1, "reopened client must reuse device_id");
        let st2 = ca2.tailnet_status();
        assert_eq!(st2.machine_public_key, mpk_1);
        assert_eq!(st2.node_public_key, npk_1);

        ca2.login(&LoginRequest {
            username: "alice_tailnet_id".to_string(),
            password: "StrongPassword!123".to_string(),
            device_name: "MacBook-A-Restarted".to_string(),
        })
        .await
        .expect("re-login alice");

        let netmap2 = ca2
            .register_tailnet_async()
            .await
            .expect("re-register after restart");
        let self_peer2 = netmap2.self_peer.expect("self_peer after restart");
        assert_eq!(self_peer2.node_id, node_id_1, "must reuse identical node_id");
        assert_eq!(
            self_peer2.tailnet_ip, tailnet_ip_1,
            "must reuse identical tailnet_ip"
        );

        let list2 = ca2.list_computers_async().await.expect("list after restart");
        let local_nodes: Vec<&ComputerNodeDto> = list2
            .computers
            .iter()
            .filter(|c| c.kind == ComputerNodeKind::LocalDevice)
            .collect();

        // Expect exactly 2 LocalDevice rows: our persistent node_id_1 and the preserved distinct Tailnet peer.
        assert_eq!(
            local_nodes.len(),
            2,
            "legacy zombie must be pruned and no duplicate created; got: {local_nodes:?}"
        );
        assert!(
            !local_nodes
                .iter()
                .any(|c| c.device_id.as_deref() == Some("legacy-uuid-zombie")),
            "legacy zombie row must have been pruned"
        );
        assert!(
            local_nodes
                .iter()
                .any(|c| c.device_id.as_deref() == Some("dev-ts-otherpeer1234")),
            "distinct Tailnet peer with machine_public_key must be preserved"
        );

        let conn = rusqlite::Connection::open(&db_path).expect("open db");
        let bound_after: Option<String> = conn
            .query_row(
                "SELECT bound_node_id FROM bot_computer_configs WHERE owner_user_id = ?1 AND bot_id = 9001",
                params![user_id as i64],
                |row| row.get(0),
            )
            .expect("query bot_computer_configs after zombie prune");
        assert_eq!(
            bound_after, None,
            "bot binding to pruned zombie must be cleared to NULL"
        );

        // Verify tampered Ed25519 signature is rejected with HTTP 400.
        let mut bad_req = TailnetRegisterRequest {
            device_id: dev_id_1.clone(),
            display_name: "Tampered".to_string(),
            lan_ip: "192.168.1.9".to_string(),
            os_name: "macOS".to_string(),
            resolution: "1440x900".to_string(),
            machine_public_key: mpk_1.clone(),
            node_public_key: npk_1.clone(),
            disco_public_key: "44".repeat(32),
            endpoints: Vec::new(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            signature_hex: Some("00".repeat(64)),
        };
        let http = reqwest::Client::new();
        let resp = http
            .post(format!("{base_url}/api/v1/tailnet/register"))
            .bearer_auth(&token_1)
            .json(&bad_req)
            .send()
            .await
            .expect("send tampered register");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "invalid Ed25519 signature must be rejected with 400"
        );
        bad_req.signature_hex = Some("not-hex".to_string());
        let resp_bad_hex = http
            .post(format!("{base_url}/api/v1/tailnet/register"))
            .bearer_auth(&token_1)
            .json(&bad_req)
            .send()
            .await
            .expect("send malformed hex register");
        assert_eq!(resp_bad_hex.status(), reqwest::StatusCode::BAD_REQUEST);

        srv.shutdown().await;
    });

    drop(client_a2);
    drop(rt);
}

// ============================================================================
// Test 2 — Local Device Stays Online Past 60s TTL via Poll Renewal & Live DERP
// ============================================================================

#[test]
fn test_local_device_stays_online_past_60s_ttl_via_poll_and_derp() {
    let temp_dir = tempfile::Builder::new()
        .prefix("e2e_tailnet_ttl_")
        .tempdir()
        .expect("tempdir");

    let rt = build_rt();
    let srv = rt.block_on(spawn_real_server(&temp_dir));

    let client_a = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_a"));
    let client_b = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_b"));

    let ca = Arc::clone(&client_a);
    let cb = Arc::clone(&client_b);
    let db_path = srv.db_path.clone();

    rt.block_on(async move {
        let auth_a = ca
            .register(&RegisterRequest {
                username: "alice_tailnet_ttl".to_string(),
                password: "StrongPassword!123".to_string(),
                first_name: "Alice".to_string(),
                device_name: "Controller-A".to_string(),
            })
            .await
            .expect("register alice");

        cb.login(&LoginRequest {
            username: "alice_tailnet_ttl".to_string(),
            password: "StrongPassword!123".to_string(),
            device_name: "Worker-B".to_string(),
        })
        .await
        .expect("login B");

        let dev_b_id = cb.device_id();
        let node_b = cb
            .register_local_computer_async(&RegisterLocalComputerRequest {
                device_id: dev_b_id.clone(),
                display_name: "Worker B".to_string(),
                ip_address: "192.168.10.22".to_string(),
                os_name: "macOS".to_string(),
                resolution: "1920x1080".to_string(),
            })
            .await
            .expect("register B");
        assert!(node_b.online);

        // Part A: Backdate B's SQLite last_seen_ms to 120s ago (past 60s TTL).
        // Verify offline first, then HTTP `/api/v1/computers/local/poll` renews heartbeat!
        let stale_120s = chrono::Utc::now().timestamp_millis() - 120_000;
        {
            let conn = rusqlite::Connection::open(&db_path).expect("open db");
            conn.execute(
                "UPDATE computer_nodes SET last_seen_ms = ?1 WHERE owner_user_id = ?2 AND node_id = ?3",
                params![stale_120s, auth_a.user_id as i64, &node_b.node_id],
            )
            .expect("backdate B last_seen_ms");
        }

        let list_offline = ca.list_computers_async().await.expect("list offline check");
        let b_offline = list_offline
            .computers
            .iter()
            .find(|n| n.node_id == node_b.node_id)
            .expect("node B listed");
        assert!(
            !b_offline.online,
            "node B with 120s stale heartbeat and no DERP must appear offline"
        );

        let _ = cb
            .poll_local_computer_commands_async()
            .await
            .expect("poll_local_computer_commands_async");

        let list_renewed = ca.list_computers_async().await.expect("list after poll");
        let b_renewed = list_renewed
            .computers
            .iter()
            .find(|n| n.node_id == node_b.node_id)
            .expect("node B listed after poll");
        assert!(
            b_renewed.online,
            "GET /api/v1/computers/local/poll MUST renew last_seen_ms so node B stays online"
        );
        assert!(b_renewed.last_seen_ms > stale_120s);

        // Part B: Connect B to built-in DERP WebSocket, then backdate SQLite last_seen_ms
        // to 5 minutes ago. Active DERP socket MUST keep node B online across computers/list
        // and tailnet/netmap!
        cb.start_tailnet_mesh();
        let st_b = wait_until_derp_ready(&cb).await;
        assert!(st_b.derp_connected);

        let stale_300s = chrono::Utc::now().timestamp_millis() - 300_000;
        {
            let conn = rusqlite::Connection::open(&db_path).expect("open db");
            conn.execute(
                "UPDATE computer_nodes SET last_seen_ms = ?1 WHERE owner_user_id = ?2 AND node_id = ?3",
                params![stale_300s, auth_a.user_id as i64, &node_b.node_id],
            )
            .expect("backdate B last_seen_ms 300s");
        }

        let list_derp = ca.list_computers_async().await.expect("list while DERP up");
        let b_derp = list_derp
            .computers
            .iter()
            .find(|n| n.node_id == node_b.node_id)
            .expect("node B present while DERP up");
        assert!(
            b_derp.online,
            "Active DERP session MUST keep LocalDevice online past 60s SQLite TTL"
        );

        let netmap_a = ca.get_tailnet_netmap_async().await.expect("get netmap");
        let peer_b = netmap_a
            .peers
            .iter()
            .find(|p| p.node_id == node_b.node_id)
            .expect("peer B in netmap");
        assert!(peer_b.online);
        assert!(peer_b.derp_connected);
        assert_eq!(peer_b.relay_mode, TailnetRelayMode::DerpRelay);

        // Part C: Stopping B's DERP mesh drops session; with 300s-stale DB timestamp, B goes offline.
        cb.stop_tailnet_mesh();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let list_after_stop = ca.list_computers_async().await.expect("list after stop");
            let b_after = list_after_stop
                .computers
                .iter()
                .find(|n| n.node_id == node_b.node_id)
                .expect("node B listed after stop");
            if !b_after.online {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "node B stayed online after DERP closed with 300s stale DB timestamp"
            );
            tokio::time::sleep(Duration::from_millis(40)).await;
        }

        srv.shutdown().await;
    });

    drop(client_a);
    drop(client_b);
    drop(rt);
}

// ============================================================================
// Test 3 — Magicsock Disco Hole-Punching Direct UDP + E2E Encryption + TSFR
// ============================================================================

#[test]
fn test_magicsock_disco_hole_punching_direct_udp_e2e_encrypted_control() {
    let temp_dir = tempfile::Builder::new()
        .prefix("e2e_tailnet_udp_")
        .tempdir()
        .expect("tempdir");

    let rt = build_rt();
    let srv = rt.block_on(spawn_real_server(&temp_dir));

    let client_a = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_a"));
    let client_b = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_b"));

    let ca = Arc::clone(&client_a);
    let cb = Arc::clone(&client_b);
    let db_path = srv.db_path.clone();
    let sandbox_dir = temp_dir.path().join("udp_target_workdir");
    std::fs::create_dir_all(&sandbox_dir).expect("create sandbox dir");

    rt.block_on(async move {
        let auth_a = ca
            .register(&RegisterRequest {
                username: "alice_tailnet_udp".to_string(),
                password: "StrongPassword!123".to_string(),
                first_name: "Alice".to_string(),
                device_name: "Laptop-A".to_string(),
            })
            .await
            .expect("register alice");

        cb.login(&LoginRequest {
            username: "alice_tailnet_udp".to_string(),
            password: "StrongPassword!123".to_string(),
            device_name: "Workstation-B".to_string(),
        })
        .await
        .expect("login B");

        ca.start_tailnet_mesh();
        cb.start_tailnet_mesh();

        let st_a = wait_until_derp_ready(&ca).await;
        let st_b = wait_until_derp_ready(&cb).await;
        assert!(st_a.local_udp_port > 0, "A must bind UDP Magicsock");
        assert!(st_b.local_udp_port > 0, "B must bind UDP Magicsock");
        assert_valid_rfc6598_tailnet_ip(&st_a.tailnet_ip);
        assert_valid_rfc6598_tailnet_ip(&st_b.tailnet_ip);
        assert_ne!(st_a.tailnet_ip, st_b.tailnet_ip);

        // Wait for Disco Ping/Pong hole-punching to upgrade BOTH peers to DirectUdp!
        let peer_b_on_a =
            wait_until_peer_relay_mode(&ca, &st_b.device_id, TailnetRelayMode::DirectUdp).await;
        let peer_a_on_b =
            wait_until_peer_relay_mode(&cb, &st_a.device_id, TailnetRelayMode::DirectUdp).await;

        assert!(peer_b_on_a.active_endpoint.is_some());
        assert!(peer_b_on_a.rtt_ms.is_some());
        assert!(peer_a_on_b.active_endpoint.is_some());
        assert_eq!(ca.tailnet_status().relay_mode, TailnetRelayMode::DirectUdp);

        // Authorize `account_all_devices` (Root-Cause Bug #5 fix) + AlwaysAllowAuthorized on B.
        cb.update_computer_policy_async(
            &peer_b_on_a.node_id,
            &UpdateComputerPolicyRequest {
                run_policy: Some(ComputerRunPolicy::AlwaysAllowAuthorized),
                add_controller: Some(AuthorizedControllerEntry {
                    controller_id: "account_all_devices".to_string(),
                    kind: AuthorizedControllerKind::Device,
                    display_name: "All My Devices".to_string(),
                    added_at_ms: 0,
                }),
                remove_controller_id: None,
                display_name: None,
            },
        )
        .await
        .expect("authorize account_all_devices on B");

        // Ensure B's local netmap reflects the updated policy before issuing P2P commands.
        let _ = wait_until_netmap_policy(&cb, ComputerRunPolicy::AlwaysAllowAuthorized).await;

        let audit_before = count_audit_rows(&db_path, auth_a.user_id);

        // 1. TestConnection via Direct UDP P2P
        let test_res = ca
            .test_computer_connection_async(&peer_b_on_a.node_id)
            .await
            .expect("P2P TestConnection");
        assert!(test_res.ok, "TestConnection failed: {test_res:?}");
        assert!(
            test_res.command_id.starts_with("p2p:"),
            "must execute via Tailnet P2P, got command_id={}",
            test_res.command_id
        );

        // 2. ShellExec via Direct UDP P2P
        let shell_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: peer_b_on_a.node_id.clone(),
                caller_device_id: Some(st_a.device_id.clone()),
                caller_bot_id: None,
                caller_label: "Laptop A".to_string(),
                operation: RemoteComputerOp::ShellExec {
                    command: "echo tailnet-direct-udp-e2e-ok".to_string(),
                    cwd: None,
                    timeout_ms: Some(10_000),
                },
            })
            .await
            .expect("P2P ShellExec");
        assert!(shell_res.ok, "ShellExec failed: {shell_res:?}");
        assert!(shell_res.command_id.starts_with("p2p:"));
        assert!(shell_res.stdout.contains("tailnet-direct-udp-e2e-ok"));

        // 3. Large WriteFile + ReadFile (4,600 bytes -> exercises TSFR UDP fragmentation > 1100B!)
        let large_payload = "TSFR-CHUNK-PAYLOAD-0123456789abcdef\n".repeat(128);
        assert!(large_payload.len() > 4096);
        let large_file = sandbox_dir.join("large_tsfr_test.txt");

        let write_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: peer_b_on_a.node_id.clone(),
                caller_device_id: Some(st_a.device_id.clone()),
                caller_bot_id: None,
                caller_label: "Laptop A".to_string(),
                operation: RemoteComputerOp::WriteFile {
                    path: large_file.to_string_lossy().to_string(),
                    content: large_payload.clone(),
                    create_dirs: true,
                },
            })
            .await
            .expect("P2P large WriteFile");
        assert!(write_res.ok, "large WriteFile failed: {write_res:?}");
        assert!(write_res.command_id.starts_with("p2p:"));

        let read_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: peer_b_on_a.node_id.clone(),
                caller_device_id: Some(st_a.device_id.clone()),
                caller_bot_id: None,
                caller_label: "Laptop A".to_string(),
                operation: RemoteComputerOp::ReadFile {
                    path: large_file.to_string_lossy().to_string(),
                    max_bytes: None,
                },
            })
            .await
            .expect("P2P large ReadFile");
        assert!(read_res.ok, "large ReadFile failed: {read_res:?}");
        assert!(read_res.command_id.starts_with("p2p:"));
        assert_eq!(read_res.stdout, large_payload);

        // 4. ListDir via Direct UDP P2P
        let list_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: peer_b_on_a.node_id.clone(),
                caller_device_id: Some(st_a.device_id.clone()),
                caller_bot_id: None,
                caller_label: "Laptop A".to_string(),
                operation: RemoteComputerOp::ListDir {
                    path: sandbox_dir.to_string_lossy().to_string(),
                },
            })
            .await
            .expect("P2P ListDir");
        assert!(list_res.ok);
        assert!(list_res
            .entries
            .iter()
            .any(|e| e.contains("large_tsfr_test.txt")));

        // Verify zero server HTTP relay audit rows were written (everything bypassed HTTP control exec).
        let audit_after = count_audit_rows(&db_path, auth_a.user_id);
        assert_eq!(
            audit_after, audit_before,
            "Direct UDP P2P RPC must bypass server HTTP exec queue"
        );

        ca.stop_tailnet_mesh();
        cb.stop_tailnet_mesh();
        srv.shutdown().await;
    });

    drop(client_a);
    drop(client_b);
    drop(rt);
}

// ============================================================================
// Test 4 — DERP WebSocket E2E Encrypted Fallback When UDP Is Blocked + Isolation
// ============================================================================

#[test]
fn test_derp_websocket_e2e_encrypted_fallback_when_udp_blocked() {
    let temp_dir = tempfile::Builder::new()
        .prefix("e2e_tailnet_derp_")
        .tempdir()
        .expect("tempdir");

    let rt = build_rt();
    let srv = rt.block_on(spawn_real_server(&temp_dir));

    let client_a = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_a"));
    let client_b = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_b"));
    let client_bob = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_bob"));

    let ca = Arc::clone(&client_a);
    let cb = Arc::clone(&client_b);
    let cbob = Arc::clone(&client_bob);
    let db_path = srv.db_path.clone();
    let ws_base = srv.ws_base_url();

    rt.block_on(async move {
        let auth_a = ca
            .register(&RegisterRequest {
                username: "alice_tailnet_derp".to_string(),
                password: "StrongPassword!123".to_string(),
                first_name: "Alice".to_string(),
                device_name: "Device-A".to_string(),
            })
            .await
            .expect("register alice");

        cb.login(&LoginRequest {
            username: "alice_tailnet_derp".to_string(),
            password: "StrongPassword!123".to_string(),
            device_name: "Device-B".to_string(),
        })
        .await
        .expect("login B");

        // Block UDP before starting mesh so Disco never uses UDP and peers remain in DerpRelay mode.
        ca.set_tailnet_force_derp_only(true);
        cb.set_tailnet_force_derp_only(true);
        ca.start_tailnet_mesh();
        cb.start_tailnet_mesh();

        let st_a = wait_until_derp_ready(&ca).await;
        let st_b = wait_until_derp_ready(&cb).await;

        let peer_b_on_a =
            wait_until_peer_relay_mode(&ca, &st_b.device_id, TailnetRelayMode::DerpRelay).await;
        assert_eq!(peer_b_on_a.active_endpoint, None);
        assert_eq!(ca.tailnet_status().relay_mode, TailnetRelayMode::DerpRelay);

        // Authorize Device A specifically + AlwaysAllowAuthorized on B.
        cb.update_computer_policy_async(
            &peer_b_on_a.node_id,
            &UpdateComputerPolicyRequest {
                run_policy: Some(ComputerRunPolicy::AlwaysAllowAuthorized),
                add_controller: Some(AuthorizedControllerEntry {
                    controller_id: st_a.device_id.clone(),
                    kind: AuthorizedControllerKind::Device,
                    display_name: "Device A".to_string(),
                    added_at_ms: 0,
                }),
                remove_controller_id: None,
                display_name: None,
            },
        )
        .await
        .expect("authorize dev-a on B");
        let _ = wait_until_netmap_policy(&cb, ComputerRunPolicy::AlwaysAllowAuthorized).await;

        let audit_before = count_audit_rows(&db_path, auth_a.user_id);

        // Part A: Explicit force-DERP E2E encrypted PeerAPI command execution.
        let derp_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: peer_b_on_a.node_id.clone(),
                caller_device_id: Some(st_a.device_id.clone()),
                caller_bot_id: None,
                caller_label: "Device A".to_string(),
                operation: RemoteComputerOp::ShellExec {
                    command: "echo derp-e2e-relay-ok".to_string(),
                    cwd: None,
                    timeout_ms: Some(10_000),
                },
            })
            .await
            .expect("DERP E2E ShellExec");
        assert!(derp_res.ok, "DERP ShellExec failed: {derp_res:?}");
        assert!(derp_res.command_id.starts_with("p2p:"));
        assert!(derp_res.stdout.contains("derp-e2e-relay-ok"));
        assert_eq!(
            count_audit_rows(&db_path, auth_a.user_id),
            audit_before,
            "DERP E2E PeerAPI must not fall back to HTTP server exec"
        );

        // Part B: Sudden UDP blackhole fallback — A believes B is DirectUdp, B drops UDP,
        // and A automatically falls back over DERP after the 1.5s UDP timeout.
        ca.set_tailnet_force_derp_only(false);
        cb.set_tailnet_force_derp_only(false);
        let _ = wait_until_peer_relay_mode(&ca, &st_b.device_id, TailnetRelayMode::DirectUdp).await;
        // Now blackhole UDP only on B while A still caches B as DirectUdp!
        cb.set_tailnet_force_derp_only(true);
        let blackhole_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: peer_b_on_a.node_id.clone(),
                caller_device_id: Some(st_a.device_id.clone()),
                caller_bot_id: None,
                caller_label: "Device A".to_string(),
                operation: RemoteComputerOp::ShellExec {
                    command: "echo udp-blackhole-fallback-derp-ok".to_string(),
                    cwd: None,
                    timeout_ms: Some(10_000),
                },
            })
            .await
            .expect("UDP blackhole -> DERP fallback");
        assert!(blackhole_res.ok, "blackhole fallback failed: {blackhole_res:?}");
        assert!(blackhole_res.command_id.starts_with("p2p:"));
        assert!(blackhole_res
            .stdout
            .contains("udp-blackhole-fallback-derp-ok"));
        assert_eq!(count_audit_rows(&db_path, auth_a.user_id), audit_before);

        // Part C: Cross-user DERP isolation — User Bob's DERP relay cannot receive Alice's SendPacket
        // and Alice's netmap must not contain Bob.
        let auth_bob = cbob
            .register(&RegisterRequest {
                username: "bob_isolated_derp".to_string(),
                password: "StrongPassword!123".to_string(),
                first_name: "Bob".to_string(),
                device_name: "Bob-Box".to_string(),
            })
            .await
            .expect("register bob");
        let bob_netmap = cbob.register_tailnet_async().await.expect("register bob tailnet");
        let bob_peer = bob_netmap.self_peer.expect("bob self_peer");

        let alice_netmap = ca.get_tailnet_netmap_async().await.expect("alice netmap");
        assert!(
            !alice_netmap.peers.iter().any(|p| p.device_id == bob_peer.device_id),
            "Alice netmap must never expose Bob's peer"
        );

        let bob_ws_url = format!("{ws_base}/api/v1/tailnet/derp?token={}", auth_bob.token);
        let (bob_ws, _) = connect_async(&bob_ws_url).await.expect("connect bob DERP WS");
        let (mut bob_tx, mut bob_rx) = bob_ws.split();
        let bob_hello = DerpClientFrame::Hello {
            device_id: bob_peer.device_id.clone(),
            node_public_key: bob_peer.node_public_key.clone(),
            disco_public_key: bob_peer.disco_public_key.clone(),
            endpoints: Vec::new(),
        };
        bob_tx
            .send(WsMsg::Text(serde_json::to_string(&bob_hello).unwrap().into()))
            .await
            .expect("bob send Hello");
        // Drain Welcome frame.
        let welcome_raw = tokio::time::timeout(Duration::from_secs(3), bob_rx.next())
            .await
            .expect("bob welcome timeout")
            .expect("bob stream item")
            .expect("bob ws ok");
        assert!(welcome_raw.into_text().unwrap().contains("welcome"));

        let alice_ws_url = format!("{ws_base}/api/v1/tailnet/derp?token={}", auth_a.token);
        let (alice_ws, _) = connect_async(&alice_ws_url)
            .await
            .expect("connect alice probe WS");
        let (mut alice_tx, mut alice_rx) = alice_ws.split();
        let alice_hello = DerpClientFrame::Hello {
            device_id: "probe-alice-dev".to_string(),
            node_public_key: "aa".repeat(32),
            disco_public_key: "bb".repeat(32),
            endpoints: Vec::new(),
        };
        alice_tx
            .send(WsMsg::Text(serde_json::to_string(&alice_hello).unwrap().into()))
            .await
            .expect("alice probe send Hello");
        let _ = tokio::time::timeout(Duration::from_secs(3), alice_rx.next()).await;

        let cross_tenant_packet = DerpClientFrame::SendPacket {
            dst_node_key: bob_peer.node_public_key.clone(),
            packet_b64: "AQIDBA==".to_string(),
        };
        alice_tx
            .send(WsMsg::Text(
                serde_json::to_string(&cross_tenant_packet).unwrap().into(),
            ))
            .await
            .expect("alice send cross-tenant packet");

        // Bob must NOT receive any RecvPacket from Alice.
        let bob_recv = tokio::time::timeout(Duration::from_millis(400), async {
            while let Some(Ok(msg)) = bob_rx.next().await {
                if let Ok(txt) = msg.into_text() {
                    if let Ok(DerpServerFrame::RecvPacket { .. }) =
                        serde_json::from_str::<DerpServerFrame>(&txt)
                    {
                        return Some(txt);
                    }
                }
            }
            None
        })
        .await;
        assert!(
            matches!(bob_recv, Err(_) | Ok(None)),
            "DERP server MUST isolate users and drop cross-tenant SendPacket; bob got: {bob_recv:?}"
        );

        ca.stop_tailnet_mesh();
        cb.stop_tailnet_mesh();
        cbob.stop_tailnet_mesh();
        srv.shutdown().await;
    });

    drop(client_a);
    drop(client_b);
    drop(client_bob);
    drop(rt);
}

// ============================================================================
// Test 5 — AskEveryTime Interactive P2P Approval & Denial (with ApprovalPending)
// ============================================================================

#[test]
fn test_ask_every_time_interactive_p2p_approval_and_denial() {
    let temp_dir = tempfile::Builder::new()
        .prefix("e2e_tailnet_ask_")
        .tempdir()
        .expect("tempdir");

    let rt = build_rt();
    let srv = rt.block_on(spawn_real_server(&temp_dir));

    let client_a = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_a"));
    let client_b = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_b"));

    let ca = Arc::clone(&client_a);
    let cb = Arc::clone(&client_b);

    rt.block_on(async move {
        ca.register(&RegisterRequest {
            username: "alice_tailnet_ask".to_string(),
            password: "StrongPassword!123".to_string(),
            first_name: "Alice".to_string(),
            device_name: "Device-A".to_string(),
        })
        .await
        .expect("register alice");

        cb.login(&LoginRequest {
            username: "alice_tailnet_ask".to_string(),
            password: "StrongPassword!123".to_string(),
            device_name: "Device-B".to_string(),
        })
        .await
        .expect("login B");

        ca.start_tailnet_mesh();
        cb.start_tailnet_mesh();
        let st_a = wait_until_derp_ready(&ca).await;
        let st_b = wait_until_derp_ready(&cb).await;
        let peer_b =
            wait_until_peer_relay_mode(&ca, &st_b.device_id, TailnetRelayMode::DirectUdp).await;

        // Verify default policy on B is AskEveryTime.
        let nm_b = wait_until_netmap_policy(&cb, ComputerRunPolicy::AskEveryTime).await;
        assert_eq!(nm_b.self_peer.unwrap().run_policy, ComputerRunPolicy::AskEveryTime);

        // Part A: Approve after waiting 1.7s (>1.5s Direct UDP timeout) to prove
        // PeerApiFrame::ApprovalPending suppresses duplicate DERP retry and keeps waiting cleanly.
        let approve_req = RemoteComputerCommandRequest {
            node_id: peer_b.node_id.clone(),
            caller_device_id: Some(st_a.device_id.clone()),
            caller_bot_id: None,
            caller_label: "Device A".to_string(),
            operation: RemoteComputerOp::ShellExec {
                command: "echo p2p-interactive-approved".to_string(),
                cwd: None,
                timeout_ms: Some(10_000),
            },
        };
        let exec_approve_fut = ca.execute_remote_computer_op_async(&approve_req);

        let approve_fut = async {
            let pending: PendingComputerCommandDto = loop {
                let items = cb.tailnet_pending_approvals();
                if let Some(first) = items.into_iter().next() {
                    break first;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            };
            assert!(pending.command_id.starts_with("p2p:"));
            assert!(pending.requires_user_approval);
            assert!(!pending.approved);
            assert_eq!(pending.caller_device_id.as_deref(), Some(st_a.device_id.as_str()));

            // Hold past the 1.5s UDP fallback window; ensure no duplicate approval queued!
            tokio::time::sleep(Duration::from_millis(1700)).await;
            assert_eq!(
                cb.tailnet_pending_approvals().len(),
                1,
                "ApprovalPending must prevent duplicate 1.5s DERP retry"
            );

            let res = cb
                .decide_tailnet_approval_async(&pending.command_id, true)
                .await
                .expect("pending approval must exist");
            assert!(res.is_ok());
        };

        let (approve_exec_res, ()) = tokio::join!(exec_approve_fut, approve_fut);
        let approve_exec_res = approve_exec_res.expect("approved P2P execution");
        assert!(approve_exec_res.ok, "expected approved execution ok: {approve_exec_res:?}");
        assert!(approve_exec_res.command_id.starts_with("p2p:"));
        assert!(approve_exec_res
            .stdout
            .contains("p2p-interactive-approved"));
        assert!(cb.tailnet_pending_approvals().is_empty());

        // Part B: Deny interactive P2P request -> exit_code 403, command does not run.
        let denied_marker = temp_dir.path().join("must_not_be_created.txt");
        let deny_req = RemoteComputerCommandRequest {
            node_id: peer_b.node_id.clone(),
            caller_device_id: Some(st_a.device_id.clone()),
            caller_bot_id: None,
            caller_label: "Device A".to_string(),
            operation: RemoteComputerOp::WriteFile {
                path: denied_marker.to_string_lossy().to_string(),
                content: "forbidden".to_string(),
                create_dirs: false,
            },
        };
        let exec_deny_fut = ca.execute_remote_computer_op_async(&deny_req);

        let deny_fut = async {
            let pending: PendingComputerCommandDto = loop {
                let items = cb.tailnet_pending_approvals();
                if let Some(first) = items.into_iter().next() {
                    break first;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            };
            let res = cb
                .decide_tailnet_approval_async(&pending.command_id, false)
                .await
                .expect("pending approval for denial must exist");
            assert!(res.is_ok());
        };

        let (deny_exec_res, ()) = tokio::join!(exec_deny_fut, deny_fut);
        let deny_exec_res = deny_exec_res.expect("denied P2P result");
        assert!(!deny_exec_res.ok);
        assert_eq!(deny_exec_res.exit_code, 403);
        assert!(
            deny_exec_res.stderr.contains("Denied"),
            "unexpected deny stderr: {}",
            deny_exec_res.stderr
        );
        assert!(
            !denied_marker.exists(),
            "denied WriteFile must NOT touch disk"
        );

        assert!(
            cb.decide_tailnet_approval_async("p2p:missing-id", true)
                .await
                .is_none()
        );

        ca.stop_tailnet_mesh();
        cb.stop_tailnet_mesh();
        srv.shutdown().await;
    });

    drop(client_a);
    drop(client_b);
    drop(rt);
}

// ============================================================================
// Test 6 — Server AI Bot Instant DERP Push Execution (Zero HTTP Polling)
// ============================================================================

#[test]
fn test_server_ai_bot_instant_derp_push_execution() {
    let temp_dir = tempfile::Builder::new()
        .prefix("e2e_tailnet_bot_derp_")
        .tempdir()
        .expect("tempdir");

    let rt = build_rt();
    let srv = rt.block_on(spawn_real_server(&temp_dir));

    let client_a = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_a"));
    let client_b = open_persistent_client(srv.base_url(), &temp_dir.path().join("client_b"));

    let ca = Arc::clone(&client_a);
    let cb = Arc::clone(&client_b);
    let db_path = srv.db_path.clone();

    rt.block_on(async move {
        let auth_a = ca
            .register(&RegisterRequest {
                username: "alice_tailnet_bot".to_string(),
                password: "StrongPassword!123".to_string(),
                first_name: "Alice".to_string(),
                device_name: "Phone-A".to_string(),
            })
            .await
            .expect("register alice");

        cb.login(&LoginRequest {
            username: "alice_tailnet_bot".to_string(),
            password: "StrongPassword!123".to_string(),
            device_name: "Workstation-B".to_string(),
        })
        .await
        .expect("login B");

        // IMPORTANT: Do NOT start any HTTP `/api/v1/computers/local/poll` background loop on B!
        // Only start the embedded Tailnet engine (DERP WebSocket).
        cb.start_tailnet_mesh();
        let st_b = wait_until_derp_ready(&cb).await;
        let netmap_b = cb.get_tailnet_netmap_async().await.expect("B netmap");
        let node_b = netmap_b.self_peer.expect("B self_peer");
        assert_eq!(node_b.device_id, st_b.device_id);

        // Authorize AccountAllBots + AlwaysAllowAuthorized on B.
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
        .expect("authorize all_bots on B");

        // Create AI Bot on Device A and bind it to Device B.
        let bot = ca
            .add_contact(AddContactHttpRequest {
                first_name: "Tailnet Ops Bot".to_string(),
                last_name: None,
                username: Some("tailnet_ops_bot".to_string()),
                phone: None,
                is_bot: true,
                bot_system_prompt: Some("You are Tailnet Ops Bot.".to_string()),
            })
            .await
            .expect("create AI bot");

        ca.update_bot_computer_profile_async(
            bot.user_id,
            &UpdateBotComputerRequest {
                bound_node_id: Some(node_b.node_id.clone()),
                clear_bound_node: false,
                persona_prompt: None,
                preferred_model: None,
                approval_mode: Some(ComputerRunPolicy::AlwaysAllowAuthorized),
                allow_cross_device_control: Some(true),
                allow_cloud_self_config: Some(true),
            },
        )
        .await
        .expect("bind bot to B");

        // Even if SQLite last_seen_ms is >60s stale, active DERP connection keeps B online & reachable.
        let stale_ts = chrono::Utc::now().timestamp_millis() - 180_000;
        {
            let conn = rusqlite::Connection::open(&db_path).expect("open db");
            conn.execute(
                "UPDATE computer_nodes SET last_seen_ms = ?1 WHERE owner_user_id = ?2 AND node_id = ?3",
                params![stale_ts, auth_a.user_id as i64, &node_b.node_id],
            )
            .expect("backdate B last_seen_ms");
        }

        // Part A: Chat command triggers server AI bot -> instant DerpServerFrame::ServerCommandDispatch
        // -> Device B executes and replies via DerpClientFrame::CommandResult with zero HTTP polling!
        let before_msgs = ca
            .get_chat_messages(bot.chat_id, Some(20))
            .await
            .expect("msgs before");
        let after_id = before_msgs.iter().map(|m| m.message_id).max().unwrap_or(0);

        let started_chat = Instant::now();
        ca.send_text_message(
            bot.chat_id as i64,
            "帮我在电脑上运行 `echo instant-derp-bot-ok`",
            None,
        )
        .await
        .expect("send chat command to bot");

        let reply = poll_bot_reply_text(&ca, bot.chat_id, bot.user_id, after_id).await;
        assert!(
            reply.contains("instant-derp-bot-ok"),
            "bot reply must contain output executed via instant DERP push, got: {reply}"
        );
        assert!(
            started_chat.elapsed() < Duration::from_secs(10),
            "instant DERP bot execution took unexpectedly long: {:?}",
            started_chat.elapsed()
        );

        // Part B: Direct bot-attributed server execution (`caller_bot_id: Some(bot.user_id)`)
        // verifies server -> DERP push -> CommandResult completes swiftly and writes server audit.
        let audit_before = count_audit_rows(&db_path, auth_a.user_id);
        let started_direct = Instant::now();
        let direct_bot_res = ca
            .execute_remote_computer_op_async(&RemoteComputerCommandRequest {
                node_id: node_b.node_id.clone(),
                caller_device_id: Some(ca.device_id()),
                caller_bot_id: Some(bot.user_id),
                caller_label: "Tailnet Ops Bot".to_string(),
                operation: RemoteComputerOp::ShellExec {
                    command: "echo bot-derp-direct-push-ok".to_string(),
                    cwd: None,
                    timeout_ms: Some(10_000),
                },
            })
            .await
            .expect("bot server-to-DERP execution");
        assert!(direct_bot_res.ok, "direct bot DERP exec failed: {direct_bot_res:?}");
        assert!(direct_bot_res.stdout.contains("bot-derp-direct-push-ok"));
        assert!(
            started_direct.elapsed() < Duration::from_secs(5),
            "ServerCommandDispatch over DERP should execute almost instantaneously, took {:?}",
            started_direct.elapsed()
        );
        assert!(
            count_audit_rows(&db_path, auth_a.user_id) > audit_before,
            "server-dispatched bot execution via DERP must still log computer_command_audit"
        );

        cb.stop_tailnet_mesh();
        srv.shutdown().await;
    });

    drop(client_a);
    drop(client_b);
    drop(rt);
}
