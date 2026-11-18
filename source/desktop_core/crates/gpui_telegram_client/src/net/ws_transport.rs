use crate::net::http_client::{ensure_runtime_handle, HttpClientError};
use crate::net::proxy::{open_tunneled_tcp, ClientProxyConfig};
use crate::net::ws_channel::WsChannel;
use futures::{SinkExt, StreamExt};
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async_tls_with_config, connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, info, warn};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    Connecting,
    Connected,
    Reconnecting { attempt: u32, next_retry_in_secs: u64 },
    Offline { reason: String },
}

impl ConnectionStatus {
    pub fn is_connected(&self) -> bool {
        matches!(self, ConnectionStatus::Connected)
    }
}

pub struct WsTransport {
    server_url: String,
    token: Arc<RwLock<Option<String>>>,
    proxy: Arc<RwLock<Option<ClientProxyConfig>>>,
    status_tx: watch::Sender<ConnectionStatus>,
    status_rx: watch::Receiver<ConnectionStatus>,
    ws_channel: WsChannel,
    is_running: Arc<RwLock<bool>>,
    generation: Arc<AtomicU64>,
    generation_tx: watch::Sender<u64>,
    active_task: Arc<Mutex<Option<AbortHandle>>>,
    lifecycle_lock: Mutex<()>,
    runtime_handle: Handle,
    max_reconnect_attempts: Arc<RwLock<u32>>,
}

fn compute_jittered_delay(attempt: u32) -> Duration {
    // 1s * 2^attempt, capped at 30s
    let base_ms = 1000u64.saturating_mul(1u64 << attempt.min(5)).min(30_000);

    // Spread +/- 25% (range: 0.75 .. 1.25)
    let mut rand_bytes = [0u8; 4];
    let _ = getrandom::getrandom(&mut rand_bytes);
    let rand_val = u32::from_le_bytes(rand_bytes) as f64 / u32::MAX as f64;
    let jitter_factor = 0.75 + (rand_val * 0.50);
    let jittered_ms = ((base_ms as f64) * jitter_factor) as u64;

    Duration::from_millis(jittered_ms.max(500).min(30_000))
}

impl WsTransport {
    pub fn new(server_url: String, ws_channel: WsChannel) -> Self {
        Self::new_with_runtime(server_url, ws_channel, None)
    }

    pub fn new_with_runtime(
        server_url: String,
        ws_channel: WsChannel,
        runtime_handle: Option<Handle>,
    ) -> Self {
        Self::new_with_runtime_and_max_attempts(
            server_url,
            ws_channel,
            runtime_handle,
            crate::config::DEFAULT_MAX_RECONNECT_ATTEMPTS,
        )
    }

    pub fn new_with_runtime_and_max_attempts(
        server_url: String,
        ws_channel: WsChannel,
        runtime_handle: Option<Handle>,
        max_reconnect_attempts: u32,
    ) -> Self {
        let (status_tx, status_rx) = watch::channel(ConnectionStatus::Connecting);
        let (generation_tx, _) = watch::channel(0u64);
        Self {
            server_url,
            token: Arc::new(RwLock::new(None)),
            proxy: Arc::new(RwLock::new(None)),
            status_tx,
            status_rx,
            ws_channel,
            is_running: Arc::new(RwLock::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            generation_tx,
            active_task: Arc::new(Mutex::new(None)),
            lifecycle_lock: Mutex::new(()),
            runtime_handle: ensure_runtime_handle(runtime_handle),
            max_reconnect_attempts: Arc::new(RwLock::new(max_reconnect_attempts)),
        }
    }

    pub fn set_proxy(&self, proxy: Option<ClientProxyConfig>) {
        *self.proxy.write() = proxy;
    }

    pub fn proxy(&self) -> Option<ClientProxyConfig> {
        self.proxy.read().clone()
    }

    pub fn with_proxy(self, proxy: Option<ClientProxyConfig>) -> Self {
        self.set_proxy(proxy);
        self
    }

    pub fn max_reconnect_attempts(&self) -> u32 {
        *self.max_reconnect_attempts.read()
    }

    pub fn set_max_reconnect_attempts(&self, max: u32) {
        *self.max_reconnect_attempts.write() = max;
    }

    pub fn with_max_reconnect_attempts(self, max: u32) -> Self {
        self.set_max_reconnect_attempts(max);
        self
    }

    pub fn status(&self) -> ConnectionStatus {
        self.status_rx.borrow().clone()
    }

    pub fn is_connected(&self) -> bool {
        self.status().is_connected()
    }

    pub fn subscribe_status(&self) -> watch::Receiver<ConnectionStatus> {
        self.status_rx.clone()
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.write() = token;
    }

    pub fn token(&self) -> Option<String> {
        self.token.read().clone()
    }

    pub fn start(&self) {
        let _lifecycle = self.lifecycle_lock.lock();
        {
            let mut running = self.is_running.write();
            if *running {
                return;
            }
            *running = true;
        }

        if let Some(prev_abort) = self.active_task.lock().take() {
            prev_abort.abort();
        }

        let my_gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.generation_tx.send(my_gen);
        let mut gen_rx = self.generation_tx.subscribe();
        let current_gen = Arc::clone(&self.generation);

        let server_url = self.server_url.clone();
        let token_slot = self.token.clone();
        let proxy_slot = self.proxy.clone();
        let status_tx = self.status_tx.clone();
        let ws_channel = self.ws_channel.clone();
        let is_running = self.is_running.clone();
        let max_reconnect_attempts = self.max_reconnect_attempts.clone();

        let handle = Handle::try_current().unwrap_or_else(|_| self.runtime_handle.clone());
        let join_handle = handle.spawn(async move {
            let mut attempt = 0u32;
            let is_active = || *is_running.read() && current_gen.load(Ordering::SeqCst) == my_gen;

            while is_active() {
                let current_token = token_slot.read().clone();
                let token = match current_token {
                    Some(t) if !t.is_empty() => t,
                    _ => {
                        if is_active() {
                            let _ = status_tx.send(ConnectionStatus::Offline {
                                reason: "No authentication token configured".into(),
                            });
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                            _ = gen_rx.changed() => { break; }
                        }
                        continue;
                    }
                };

                let ws_url = Self::format_ws_url(&server_url, &token);
                let active_proxy = proxy_slot.read().clone();
                if !is_active() {
                    break;
                }
                let _ = status_tx.send(ConnectionStatus::Connecting);

                let connect_res = tokio::select! {
                    res = connect_ws_with_proxy(&ws_url, active_proxy.as_ref()) => res,
                    _ = gen_rx.changed() => break,
                };

                match connect_res {
                    Ok(ws_stream) => {
                        if !is_active() {
                            break;
                        }
                        info!("WebSocket connected to {}", ws_url);
                        attempt = 0;
                        let _ = status_tx.send(ConnectionStatus::Connected);

                        let (mut ws_write, mut ws_read) = ws_stream.split();
                        let (outbound_tx, mut outbound_rx) =
                            mpsc::unbounded_channel::<WsClientFrame>();
                        let (inbound_tx, inbound_rx) =
                            mpsc::unbounded_channel::<WsServerFrame>();
                        ws_channel.attach_channels(outbound_tx, inbound_rx);

                        let mut ping_interval =
                            tokio::time::interval(Duration::from_secs(25));

                        loop {
                            if !is_active() {
                                break;
                            }
                            tokio::select! {
                                _ = gen_rx.changed() => {
                                    break;
                                }
                                _ = ping_interval.tick() => {
                                    let nonce = chrono::Utc::now().timestamp_millis() as u64;
                                    let ping_frame = WsClientFrame::Ping { nonce };
                                    if let Ok(json) = serde_json::to_string(&ping_frame) {
                                        if ws_write.send(Message::Text(json.into())).await.is_err() {
                                            warn!("Failed to send ping frame; breaking socket loop");
                                            break;
                                        }
                                    }
                                }
                                outbound_msg = outbound_rx.recv() => {
                                    match outbound_msg {
                                        Some(frame) => {
                                            if let Ok(json) = serde_json::to_string(&frame) {
                                                if ws_write.send(Message::Text(json.into())).await.is_err() {
                                                    break;
                                                }
                                            }
                                        }
                                        None => break,
                                    }
                                }
                                inbound_msg = ws_read.next() => {
                                    match inbound_msg {
                                        Some(Ok(Message::Text(text))) => {
                                            if let Ok(frame) = serde_json::from_str::<WsServerFrame>(&text) {
                                                let _ = inbound_tx.send(frame.clone());
                                                ws_channel.dispatch_inbound(frame);
                                            }
                                        }
                                        Some(Ok(Message::Ping(_))) => {}
                                        Some(Ok(Message::Pong(_))) => {}
                                        Some(Ok(Message::Close(_))) | None => break,
                                        Some(Err(e)) => {
                                            debug!("WebSocket stream error: {e}");
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    Err(WsConnectError::Unauthorized) => {
                        warn!("WebSocket handshake rejected with HTTP 401 Unauthorized. Token revoked or invalid.");
                        if is_active() {
                            let _ = status_tx.send(ConnectionStatus::Offline {
                                reason: "Session unauthorized (401)".into(),
                            });
                            *is_running.write() = false;
                        }
                        break;
                    }
                    Err(e) => {
                        warn!("WebSocket connect failure: {e}");
                        if matches!(e, WsConnectError::Proxy(_)) && is_active() {
                            let _ = status_tx.send(ConnectionStatus::Offline {
                                reason: e.to_string(),
                            });
                        }
                    }
                }

                if !is_active() {
                    break;
                }

                attempt += 1;
                let max_attempts = *max_reconnect_attempts.read();
                if max_attempts > 0 && attempt > max_attempts {
                    warn!(
                        "WebSocket reconnection exceeded max attempts ({}). Terminating loop.",
                        max_attempts
                    );
                    if is_active() {
                        let _ = status_tx.send(ConnectionStatus::Offline {
                            reason: "Max reconnection attempts exceeded".into(),
                        });
                        *is_running.write() = false;
                    }
                    break;
                }

                let retry_delay = compute_jittered_delay(attempt);
                let retry_secs = retry_delay.as_secs().max(1);
                if !is_active() {
                    break;
                }
                let _ = status_tx.send(ConnectionStatus::Reconnecting {
                    attempt,
                    next_retry_in_secs: retry_secs,
                });

                tokio::select! {
                    _ = tokio::time::sleep(retry_delay) => {}
                    _ = gen_rx.changed() => { break; }
                }
            }
        });
        *self.active_task.lock() = Some(join_handle.abort_handle());
    }

    pub fn stop(&self) {
        let _lifecycle = self.lifecycle_lock.lock();
        *self.is_running.write() = false;
        let next_gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.generation_tx.send(next_gen);
        if let Some(abort) = self.active_task.lock().take() {
            abort.abort();
        }
        let _ = self.status_tx.send(ConnectionStatus::Offline {
            reason: "Client disconnected".into(),
        });
    }

    pub fn format_ws_url(http_base: &str, token: &str) -> String {
        let clean = http_base.trim().trim_end_matches('/');
        let base = if clean.starts_with("https://") {
            clean.replacen("https://", "wss://", 1)
        } else if clean.starts_with("http://") {
            clean.replacen("http://", "ws://", 1)
        } else {
            format!("ws://{}", clean)
        };
        format!("{}/api/v1/ws?token={}", base, token)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WsConnectError {
    #[error("WebSocket unauthorized (401)")]
    Unauthorized,
    #[error("Proxy tunnel error: {0}")]
    Proxy(String),
    #[error("WebSocket handshake/transport error: {0}")]
    Transport(String),
}

impl From<tokio_tungstenite::tungstenite::Error> for WsConnectError {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        match &err {
            tokio_tungstenite::tungstenite::Error::Http(resp)
                if resp.status() == tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED =>
            {
                WsConnectError::Unauthorized
            }
            _ => WsConnectError::Transport(err.to_string()),
        }
    }
}

pub async fn connect_ws_with_proxy(
    ws_url: &str,
    proxy: Option<&ClientProxyConfig>,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, WsConnectError> {
    match proxy {
        None => {
            let (stream, _) = connect_async(ws_url).await?;
            Ok(stream)
        }
        Some(cfg) if cfg.is_direct() => {
            let (stream, _) = connect_async(ws_url).await?;
            Ok(stream)
        }
        Some(cfg) => {
            let parsed = reqwest::Url::parse(ws_url)
                .map_err(|e| WsConnectError::Proxy(format!("Invalid WebSocket URL: {e}")))?;
            let host = parsed
                .host_str()
                .ok_or_else(|| WsConnectError::Proxy("WebSocket URL missing host".into()))?
                .trim_matches(|c| c == '[' || c == ']')
                .to_string();
            let port = parsed.port_or_known_default().unwrap_or_else(|| {
                if parsed.scheme().eq_ignore_ascii_case("wss") {
                    443
                } else {
                    80
                }
            });
            match open_tunneled_tcp(&host, port, cfg).await {
                Ok(tcp) => {
                    let (ws_stream, _) =
                        client_async_tls_with_config(ws_url, tcp, None, None).await?;
                    Ok(ws_stream)
                }
                Err(HttpClientError::Proxy(msg)) if cfg.fallback_to_direct => {
                    warn!("Proxy tunnel failed ({msg}); falling back to direct WS connection");
                    let (stream, _) = connect_async(ws_url).await?;
                    Ok(stream)
                }
                Err(e) => Err(WsConnectError::Proxy(e.to_string())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_ws_url() {
        let url = WsTransport::format_ws_url("http://34.46.77.133:8080", "secret_token_123");
        assert_eq!(url, "ws://34.46.77.133:8080/api/v1/ws?token=secret_token_123");

        let url_ssl = WsTransport::format_ws_url("https://secure.example.com/", "token_xyz");
        assert_eq!(url_ssl, "wss://secure.example.com/api/v1/ws?token=token_xyz");
    }

    #[test]
    fn test_connection_status_helpers() {
        let s = ConnectionStatus::Connected;
        assert!(s.is_connected());

        let r = ConnectionStatus::Reconnecting {
            attempt: 1,
            next_retry_in_secs: 2,
        };
        assert!(!r.is_connected());
    }

    #[tokio::test]
    async fn test_max_reconnect_attempts_termination() {
        let ws_channel = WsChannel::new();
        let transport = WsTransport::new("http://127.0.0.1:1".to_string(), ws_channel)
            .with_max_reconnect_attempts(1);
        transport.set_token(Some("dummy_token".to_string()));
        let mut status_rx = transport.subscribe_status();

        transport.start();

        let mut saw_offline_max_attempts = false;
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if status_rx.changed().await.is_ok() {
                let status = status_rx.borrow().clone();
                if let ConnectionStatus::Offline { reason } = status {
                    if reason.contains("Max reconnection attempts exceeded") {
                        saw_offline_max_attempts = true;
                        break;
                    }
                }
            }
        }
        transport.stop();
        assert!(
            saw_offline_max_attempts,
            "Transport must terminate and transition to Offline with max reconnection attempts exceeded"
        );
    }
}
