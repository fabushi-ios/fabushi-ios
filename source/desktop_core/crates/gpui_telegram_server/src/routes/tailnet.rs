use crate::db::ServerDatabase;
use crate::routes::{err_resp, map_telegram_err};
use crate::server::{AppState, AuthBearer};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, Extension, State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use futures::{SinkExt, StreamExt};
use gpui_telegram_core::error::Result as CoreResult;
use gpui_telegram_proto::computer_contracts::PendingComputerCommandDto;
use gpui_telegram_proto::tailnet_contracts::{
    DerpClientFrame, DerpServerFrame, TailnetNetMap, TailnetPeerDescriptor, TailnetRegisterRequest,
    TailnetStunResponse,
};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Clone)]
struct DerpSessionEntry {
    session_gen: u64,
    owner_user_id: u64,
    device_id: String,
    node_public_key: String,
    tx: mpsc::UnboundedSender<DerpServerFrame>,
}

#[derive(Default)]
struct DerpRegistry {
    by_device: HashMap<(u64, String), DerpSessionEntry>,
    by_node_key: HashMap<(u64, String), DerpSessionEntry>,
}

/// Built-in Tailscale-style coordination + DERP relay state stored in `AppState`.
pub struct TailnetCoordService {
    db: Arc<ServerDatabase>,
    seq: AtomicU64,
    next_session_gen: AtomicU64,
    registry: RwLock<DerpRegistry>,
}

impl TailnetCoordService {
    pub fn new(db: Arc<ServerDatabase>) -> Self {
        Self {
            db,
            seq: AtomicU64::new(1),
            next_session_gen: AtomicU64::new(1),
            registry: RwLock::new(DerpRegistry::default()),
        }
    }

    pub fn next_conn_generation(&self) -> u64 {
        self.next_session_gen.fetch_add(1, Ordering::SeqCst)
    }

    pub fn is_device_derp_connected(&self, owner_user_id: u64, device_id: &str) -> bool {
        let trimmed = device_id.trim();
        if trimmed.is_empty() {
            return false;
        }
        self.registry
            .read()
            .by_device
            .contains_key(&(owner_user_id, trimmed.to_string()))
    }

    pub fn connected_device_ids_for_user(&self, owner_user_id: u64) -> HashSet<String> {
        self.registry
            .read()
            .by_device
            .iter()
            .filter_map(|((uid, dev_id), _)| {
                if *uid == owner_user_id {
                    Some(dev_id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn register_session(
        &self,
        session_gen: u64,
        owner_user_id: u64,
        device_id: String,
        node_public_key: String,
        tx: mpsc::UnboundedSender<DerpServerFrame>,
    ) {
        let mut reg = self.registry.write();
        if let Some(prev) = reg.by_device.get(&(owner_user_id, device_id.clone())) {
            if prev.session_gen > session_gen {
                return;
            }
            if !prev.node_public_key.is_empty() && prev.node_public_key != node_public_key {
                let old_key = (owner_user_id, prev.node_public_key.clone());
                if reg
                    .by_node_key
                    .get(&old_key)
                    .is_some_and(|e| e.session_gen <= session_gen)
                {
                    reg.by_node_key.remove(&old_key);
                }
            }
        }
        if !node_public_key.is_empty() {
            if let Some(prev_nk) = reg.by_node_key.get(&(owner_user_id, node_public_key.clone())) {
                if prev_nk.session_gen > session_gen {
                    return;
                }
                if prev_nk.device_id != device_id {
                    let old_dev = (owner_user_id, prev_nk.device_id.clone());
                    if reg
                        .by_device
                        .get(&old_dev)
                        .is_some_and(|e| e.session_gen <= session_gen)
                    {
                        reg.by_device.remove(&old_dev);
                    }
                }
            }
        }

        let entry = DerpSessionEntry {
            session_gen,
            owner_user_id,
            device_id: device_id.clone(),
            node_public_key: node_public_key.clone(),
            tx,
        };
        reg.by_device
            .insert((owner_user_id, device_id), entry.clone());
        if !node_public_key.is_empty() {
            reg.by_node_key
                .insert((owner_user_id, node_public_key), entry);
        }
    }

    pub fn unregister_session(
        &self,
        session_gen: u64,
        owner_user_id: u64,
        device_id: &str,
        node_public_key: &str,
    ) -> bool {
        let mut reg = self.registry.write();
        let mut removed_device = false;
        let dev_key = (owner_user_id, device_id.to_string());
        if reg
            .by_device
            .get(&dev_key)
            .is_some_and(|e| e.session_gen == session_gen)
        {
            reg.by_device.remove(&dev_key);
            removed_device = true;
        }
        if !node_public_key.is_empty() {
            let nk_key = (owner_user_id, node_public_key.to_string());
            if reg
                .by_node_key
                .get(&nk_key)
                .is_some_and(|e| e.session_gen == session_gen)
            {
                reg.by_node_key.remove(&nk_key);
            }
        }
        removed_device
    }

    pub fn build_netmap_for_user(
        &self,
        owner_user_id: u64,
        self_device_id: Option<&str>,
        now_ms: i64,
    ) -> CoreResult<TailnetNetMap> {
        let connected = self.connected_device_ids_for_user(owner_user_id);
        let all_peers = self
            .db
            .list_tailnet_peers(owner_user_id, now_ms, &connected)?;

        let self_did = self_device_id.map(str::trim).filter(|s| !s.is_empty());
        let mut self_peer: Option<TailnetPeerDescriptor> = None;
        let mut peers = Vec::with_capacity(all_peers.len());

        for p in all_peers {
            if self_did.is_some_and(|did| p.device_id == did) && self_peer.is_none() {
                self_peer = Some(p);
            } else {
                peers.push(p);
            }
        }

        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        Ok(TailnetNetMap {
            owner_user_id,
            seq,
            generated_at_ms: now_ms,
            derp_region: "primary".to_string(),
            derp_ws_path: "/api/v1/tailnet/derp".to_string(),
            stun_http_path: "/api/v1/tailnet/stun".to_string(),
            self_peer,
            peers,
        })
    }

    pub fn broadcast_netmap_for_user(&self, owner_user_id: u64) {
        self.broadcast_netmap_for_user_except(owner_user_id, None);
    }

    fn broadcast_netmap_for_user_except(&self, owner_user_id: u64, except_gen: Option<u64>) {
        let targets: Vec<(String, mpsc::UnboundedSender<DerpServerFrame>)> = {
            let reg = self.registry.read();
            reg.by_device
                .values()
                .filter(|e| {
                    e.owner_user_id == owner_user_id && Some(e.session_gen) != except_gen
                })
                .map(|e| (e.device_id.clone(), e.tx.clone()))
                .collect()
        };
        if targets.is_empty() {
            return;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        for (dev_id, tx) in targets {
            if let Ok(netmap) = self.build_netmap_for_user(owner_user_id, Some(&dev_id), now_ms) {
                let _ = tx.send(DerpServerFrame::NetMapUpdate { netmap });
            }
        }
    }

    pub fn dispatch_command_to_device(
        &self,
        owner_user_id: u64,
        device_id: &str,
        cmd: PendingComputerCommandDto,
    ) -> bool {
        let trimmed = device_id.trim();
        if trimmed.is_empty() {
            return false;
        }
        let tx_opt = {
            let reg = self.registry.read();
            reg.by_device
                .get(&(owner_user_id, trimmed.to_string()))
                .map(|e| e.tx.clone())
        };
        let Some(tx) = tx_opt else {
            return false;
        };
        tx.send(DerpServerFrame::ServerCommandDispatch { command: cmd })
            .is_ok()
    }

    pub fn forward_packet_to_peer(
        &self,
        owner_user_id: u64,
        src_node_key: &str,
        dst_node_key: &str,
        packet_b64: String,
    ) -> bool {
        let dst = dst_node_key.trim();
        if dst.is_empty() {
            return false;
        }
        let tx_opt = {
            let reg = self.registry.read();
            reg.by_node_key
                .get(&(owner_user_id, dst.to_string()))
                .map(|e| e.tx.clone())
        };
        let Some(tx) = tx_opt else {
            return false;
        };
        tx.send(DerpServerFrame::RecvPacket {
            src_node_key: src_node_key.to_string(),
            packet_b64,
        })
        .is_ok()
    }
}

fn header_device_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-device-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn decode_hex(raw: &str) -> Result<Vec<u8>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty hex string".to_string());
    }
    if trimmed.len() % 2 != 0 {
        return Err("odd number of hex characters".to_string());
    }
    let bytes = trimmed.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = hex_nibble(pair[0]).ok_or_else(|| "invalid hex character".to_string())?;
        let lo = hex_nibble(pair[1]).ok_or_else(|| "invalid hex character".to_string())?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn verify_tailnet_register_signature(
    owner_user_id: u64,
    req: &TailnetRegisterRequest,
    sig_hex: &str,
) -> Result<(), Response> {
    let pk_bytes = decode_hex(&req.machine_public_key).map_err(|e| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_machine_public_key",
            &format!("Invalid machine_public_key hex: {e}"),
        )
    })?;
    let pk_arr: [u8; 32] = pk_bytes.try_into().map_err(|_| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_machine_public_key",
            "machine_public_key must be 32 bytes (64 hex chars)",
        )
    })?;
    let verifying_key = VerifyingKey::from_bytes(&pk_arr).map_err(|e| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_machine_public_key",
            &format!("Invalid Ed25519 machine_public_key: {e}"),
        )
    })?;

    let sig_bytes = decode_hex(sig_hex).map_err(|e| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_signature",
            &format!("Invalid signature_hex: {e}"),
        )
    })?;
    let signature = Signature::from_slice(&sig_bytes).map_err(|e| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_signature",
            &format!("Malformed Ed25519 signature: {e}"),
        )
    })?;

    let canonical = req.canonical_signing_message(owner_user_id);
    verifying_key
        .verify(canonical.as_bytes(), &signature)
        .map_err(|e| {
            err_resp(
                StatusCode::BAD_REQUEST,
                "signature_verification_failed",
                &format!("Tailnet registration signature invalid: {e}"),
            )
        })?;
    Ok(())
}

fn extract_stun_observed_addr(
    headers: &HeaderMap,
    connect_info: Option<&ConnectInfo<SocketAddr>>,
) -> (String, u16) {
    let forwarded_port = headers
        .get("x-forwarded-port")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u16>().ok());

    let candidate = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        });

    if let Some(raw) = candidate {
        if let Ok(sa) = raw.parse::<SocketAddr>() {
            return (sa.ip().to_string(), sa.port());
        }
        let cleaned = raw.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = cleaned.parse::<IpAddr>() {
            let port = forwarded_port
                .or_else(|| connect_info.map(|c| c.0.port()))
                .unwrap_or(0);
            return (ip.to_string(), port);
        }
        return (
            raw.to_string(),
            forwarded_port
                .or_else(|| connect_info.map(|c| c.0.port()))
                .unwrap_or(0),
        );
    }

    if let Some(ci) = connect_info {
        return (ci.0.ip().to_string(), ci.0.port());
    }

    ("127.0.0.1".to_string(), forwarded_port.unwrap_or(0))
}

// ============================================================================
// HTTP & WebSocket Route Handlers
// ============================================================================

pub async fn tailnet_register(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<TailnetRegisterRequest>,
) -> Response {
    let device_id = req.device_id.trim();
    if device_id.is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_device_id",
            "device_id must not be empty",
        );
    }

    if let Some(ref sig_hex) = req.signature_hex {
        if let Err(resp) = verify_tailnet_register_signature(auth.user_id.0, &req, sig_hex) {
            return resp;
        }
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    if let Err(e) = state
        .db
        .upsert_tailnet_computer_node(auth.user_id.0, &req, now_ms)
    {
        return map_telegram_err(e);
    }

    state.tailnet.broadcast_netmap_for_user(auth.user_id.0);

    match state
        .tailnet
        .build_netmap_for_user(auth.user_id.0, Some(device_id), now_ms)
    {
        Ok(netmap) => (StatusCode::OK, Json(netmap)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn tailnet_netmap(
    auth: AuthBearer,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let req_dev = header_device_id(&headers);
    if let Some(ref dev_id) = req_dev {
        let _ = state
            .db
            .touch_local_device_heartbeat_by_device_id(auth.user_id.0, dev_id, now_ms);
    }
    match state
        .tailnet
        .build_netmap_for_user(auth.user_id.0, req_dev.as_deref(), now_ms)
    {
        Ok(netmap) => (StatusCode::OK, Json(netmap)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn tailnet_stun(
    _auth: AuthBearer,
    headers: HeaderMap,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> Response {
    let (observed_ip, observed_port) =
        extract_stun_observed_addr(&headers, connect_info.as_ref().map(|e| &e.0));
    let server_time_ms = chrono::Utc::now().timestamp_millis();
    (
        StatusCode::OK,
        Json(TailnetStunResponse {
            observed_ip,
            observed_port,
            server_time_ms,
        }),
    )
        .into_response()
}

pub async fn tailnet_derp_ws(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
) -> Response {
    let owner_user_id = auth.user_id.0;
    ws.on_upgrade(move |socket| handle_derp_socket(socket, owner_user_id, state))
}

async fn handle_derp_socket(
    socket: WebSocket,
    owner_user_id: u64,
    state: Arc<AppState>,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<DerpServerFrame>();

    let forward_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&frame) {
                if ws_sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    let conn_gen = state.tailnet.next_conn_generation();
    let mut registered: Option<(String, String)> = None;
    let mut last_activity = tokio::time::Instant::now();
    let inactivity_limit = Duration::from_secs(state.config.ws_inactivity_timeout_secs.max(120));
    let mut heartbeat_timer = tokio::time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            _ = heartbeat_timer.tick() => {
                if last_activity.elapsed() >= inactivity_limit {
                    break;
                }
            }
            msg = ws_receiver.next() => {
                let parsed_frame = match msg {
                    Some(Ok(Message::Text(text))) => {
                        last_activity = tokio::time::Instant::now();
                        serde_json::from_str::<DerpClientFrame>(&text)
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        last_activity = tokio::time::Instant::now();
                        serde_json::from_slice::<DerpClientFrame>(&bytes)
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {
                        last_activity = tokio::time::Instant::now();
                        continue;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                };

                let frame = match parsed_frame {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = tx.send(DerpServerFrame::Error {
                            code: "invalid_frame".to_string(),
                            message: format!("Malformed DerpClientFrame: {e}"),
                        });
                        continue;
                    }
                };

                match frame {
                    DerpClientFrame::Hello {
                        device_id,
                        node_public_key,
                        disco_public_key,
                        endpoints,
                    } => {
                        let dev_id = device_id.trim().to_string();
                        let node_key = node_public_key.trim().to_string();
                        let disco_key = disco_public_key.trim().to_string();
                        if dev_id.is_empty() {
                            let _ = tx.send(DerpServerFrame::Error {
                                code: "invalid_hello".to_string(),
                                message: "device_id must not be empty".to_string(),
                            });
                            continue;
                        }

                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let _ = state.db.touch_tailnet_hello_or_keepalive(
                            owner_user_id,
                            &dev_id,
                            if node_key.is_empty() { None } else { Some(&node_key) },
                            if disco_key.is_empty() { None } else { Some(&disco_key) },
                            &endpoints,
                            now_ms,
                        );

                        if let Some((old_dev, old_nk)) = registered.take() {
                            if old_dev != dev_id || old_nk != node_key {
                                state.tailnet.unregister_session(conn_gen, owner_user_id, &old_dev, &old_nk);
                            }
                        }

                        state.tailnet.register_session(
                            conn_gen,
                            owner_user_id,
                            dev_id.clone(),
                            node_key.clone(),
                            tx.clone(),
                        );
                        registered = Some((dev_id.clone(), node_key));

                        match state
                            .tailnet
                            .build_netmap_for_user(owner_user_id, Some(&dev_id), now_ms)
                        {
                            Ok(netmap) => {
                                let _ = tx.send(DerpServerFrame::Welcome {
                                    derp_region: "primary".to_string(),
                                    server_time_ms: now_ms,
                                    netmap,
                                });
                            }
                            Err(e) => {
                                let _ = tx.send(DerpServerFrame::Error {
                                    code: "netmap_error".to_string(),
                                    message: e.to_string(),
                                });
                            }
                        }

                        state
                            .tailnet
                            .broadcast_netmap_for_user_except(owner_user_id, Some(conn_gen));
                    }
                    DerpClientFrame::SendPacket {
                        dst_node_key,
                        packet_b64,
                    } => {
                        let Some((_, ref src_node_key)) = registered else {
                            let _ = tx.send(DerpServerFrame::Error {
                                code: "not_registered".to_string(),
                                message: "Must send Hello before SendPacket".to_string(),
                            });
                            continue;
                        };
                        if !state.tailnet.forward_packet_to_peer(
                            owner_user_id,
                            src_node_key,
                            &dst_node_key,
                            packet_b64,
                        ) {
                            let _ = tx.send(DerpServerFrame::Error {
                                code: "peer_unavailable".to_string(),
                                message: "Target peer is not connected in your tailnet".to_string(),
                            });
                        }
                    }
                    DerpClientFrame::KeepAlive {
                        client_time_ms,
                        endpoints,
                    } => {
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        if let Some((ref dev_id, _)) = registered {
                            let _ = state.db.touch_tailnet_hello_or_keepalive(
                                owner_user_id,
                                dev_id,
                                None,
                                None,
                                &endpoints,
                                now_ms,
                            );
                        }
                        let _ = tx.send(DerpServerFrame::KeepAliveAck {
                            client_time_ms,
                            server_time_ms: now_ms,
                        });
                    }
                    DerpClientFrame::CommandResult { command_id, result } => {
                        if let Err(e) = state.computers.complete_pending_command(
                            owner_user_id,
                            &command_id,
                            result,
                        ) {
                            let _ = tx.send(DerpServerFrame::Error {
                                code: "command_result_error".to_string(),
                                message: e.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    forward_task.abort();
    if let Some((dev_id, node_key)) = registered {
        if state
            .tailnet
            .unregister_session(conn_gen, owner_user_id, &dev_id, &node_key)
        {
            state.tailnet.broadcast_netmap_for_user(owner_user_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session_manager::hash_password;
    use crate::config::ServerConfig;
    use crate::db::schema::UserRecord;
    use crate::server::build_router_with_state;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use ed25519_dalek::{Signer, SigningKey};
    use gpui_telegram_core::types::UserId;
    use gpui_telegram_proto::tailnet_contracts::{TailnetEndpoint, TailnetEndpointKind};
    use tower::ServiceExt;

    fn encode_lower_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0f) as usize] as char);
        }
        s
    }

    fn make_state_and_token(user_id: u64, device_id: &str) -> (Arc<AppState>, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = ServerConfig::default();
        cfg.database_url = ":memory:".to_string();
        cfg.media_storage_path = dir.path().join("media").to_string_lossy().to_string();
        let db = Arc::new(ServerDatabase::open_in_memory().expect("db"));
        let state = Arc::new(AppState::with_db(cfg, db));

        let now_ms = chrono::Utc::now().timestamp_millis();
        state
            .db
            .create_user(&UserRecord {
                user_id: UserId(user_id),
                username: Some(format!("u_{user_id}")),
                phone: None,
                first_name: "Tailnet".to_string(),
                last_name: None,
                bio: None,
                password_hash: Some(hash_password("pw").unwrap()),
                is_bot: false,
                is_verified: false,
                is_support: false,
                is_scam: false,
                is_fake: false,
                profile_photo_file_id: None,
                account_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            })
            .expect("create user");
        let (_sess, token) = state.session_manager.create_session(
            UserId(user_id),
            device_id,
            chrono::Duration::days(1),
        );
        (state, token, dir)
    }

    #[tokio::test]
    async fn test_tailnet_register_signature_verification_and_stun() {
        let (state, token, _dir) = make_state_and_token(501, "dev-a");
        let app = build_router_with_state(state.clone());

        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let machine_pub_hex = encode_lower_hex(&signing_key.verifying_key().to_bytes());
        let mut req = TailnetRegisterRequest {
            device_id: "dev-ts-501a".to_string(),
            display_name: "Test Mac".to_string(),
            lan_ip: "192.168.1.25".to_string(),
            os_name: "macOS".to_string(),
            resolution: "2560x1440".to_string(),
            machine_public_key: machine_pub_hex,
            node_public_key: "bb".repeat(32),
            disco_public_key: "cc".repeat(32),
            endpoints: vec![TailnetEndpoint {
                addr: "192.168.1.25:41641".to_string(),
                kind: TailnetEndpointKind::Lan,
            }],
            timestamp_ms: 1_730_000_000_000,
            signature_hex: Some("deadbeef".to_string()),
        };

        // Invalid signature returns 400.
        let bad_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tailnet/register")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&req).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad_resp.status(), StatusCode::BAD_REQUEST);

        // Valid signature registers and returns TailnetNetMap with self_peer.
        let canonical = req.canonical_signing_message(501);
        let sig = signing_key.sign(canonical.as_bytes());
        req.signature_hex = Some(encode_lower_hex(&sig.to_bytes()));

        let ok_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tailnet/register")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&req).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok_resp.status(), StatusCode::OK);
        let bytes = to_bytes(ok_resp.into_body(), 1024 * 1024).await.unwrap();
        let netmap: TailnetNetMap = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(netmap.owner_user_id, 501);
        assert_eq!(netmap.derp_ws_path, "/api/v1/tailnet/derp");
        assert_eq!(netmap.stun_http_path, "/api/v1/tailnet/stun");
        let self_peer = netmap.self_peer.expect("self_peer");
        assert_eq!(self_peer.device_id, "dev-ts-501a");
        assert!(self_peer.tailnet_ip.starts_with("100."));

        // STUN works cleanly without ConnectInfo attached, honoring X-Forwarded-For or fallback.
        let stun_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/tailnet/stun")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("X-Forwarded-For", "203.0.113.77:51820")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stun_resp.status(), StatusCode::OK);
        let stun_bytes = to_bytes(stun_resp.into_body(), 1024 * 1024).await.unwrap();
        let stun: TailnetStunResponse = serde_json::from_slice(&stun_bytes).unwrap();
        assert_eq!(stun.observed_ip, "203.0.113.77");
        assert_eq!(stun.observed_port, 51820);

        let stun_fallback_resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/tailnet/stun")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stun_fallback_resp.status(), StatusCode::OK);
        let fallback_bytes = to_bytes(stun_fallback_resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let stun_fb: TailnetStunResponse = serde_json::from_slice(&fallback_bytes).unwrap();
        assert_eq!(stun_fb.observed_ip, "127.0.0.1");
    }

    #[test]
    fn test_derp_session_generation_prevents_stale_unregister_and_isolates_users() {
        let db = Arc::new(ServerDatabase::open_in_memory().expect("db"));
        let svc = TailnetCoordService::new(db);

        let (tx_old, _rx_old) = mpsc::unbounded_channel();
        let (tx_new, mut rx_new) = mpsc::unbounded_channel();
        let (tx_other_user, mut rx_other) = mpsc::unbounded_channel();

        let g1 = svc.next_conn_generation();
        let g2 = svc.next_conn_generation();
        let g3 = svc.next_conn_generation();

        svc.register_session(g1, 10, "dev-1".into(), "nk-1".into(), tx_old);
        svc.register_session(g2, 10, "dev-1".into(), "nk-1".into(), tx_new);
        svc.register_session(g3, 20, "dev-2".into(), "nk-1".into(), tx_other_user);

        // Old socket closing must NOT remove newer session g2.
        assert!(!svc.unregister_session(g1, 10, "dev-1", "nk-1"));
        assert!(svc.is_device_derp_connected(10, "dev-1"));

        // Packet forwarding is strictly scoped to owner_user_id=10, never leaking to user 20.
        assert!(svc.forward_packet_to_peer(10, "nk-sender", "nk-1", "aGVsbG8=".into()));
        assert!(rx_new.try_recv().is_ok());
        assert!(rx_other.try_recv().is_err());

        // Closing newer session g2 removes it.
        assert!(svc.unregister_session(g2, 10, "dev-1", "nk-1"));
        assert!(!svc.is_device_derp_connected(10, "dev-1"));
    }
}
