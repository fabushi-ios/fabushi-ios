use crate::net::http_client::ensure_runtime_handle;
use crate::net::ws_channel::WsChannel;
use futures::{SinkExt, StreamExt};
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
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
    status_tx: watch::Sender<ConnectionStatus>,
    status_rx: watch::Receiver<ConnectionStatus>,
    ws_channel: WsChannel,
    is_running: Arc<RwLock<bool>>,
    runtime_handle: Handle,
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
        let (status_tx, status_rx) = watch::channel(ConnectionStatus::Connecting);
        Self {
            server_url,
            token: Arc::new(RwLock::new(None)),
            status_tx,
            status_rx,
            ws_channel,
            is_running: Arc::new(RwLock::new(false)),
            runtime_handle: ensure_runtime_handle(runtime_handle),
        }
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
        let mut running = self.is_running.write();
        if *running {
            return;
        }
        *running = true;

        let server_url = self.server_url.clone();
        let token_slot = self.token.clone();
        let status_tx = self.status_tx.clone();
        let ws_channel = self.ws_channel.clone();
        let is_running = self.is_running.clone();

        let handle = Handle::try_current().unwrap_or_else(|_| self.runtime_handle.clone());
        handle.spawn(async move {
            let mut attempt = 0u32;
            let mut delay = Duration::from_millis(1000);

            while *is_running.read() {
                let current_token = token_slot.read().clone();
                let token = match current_token {
                    Some(t) if !t.is_empty() => t,
                    _ => {
                        let _ = status_tx.send(ConnectionStatus::Offline {
                            reason: "No authentication token configured".into(),
                        });
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        continue;
                    }
                };

                let ws_url = Self::format_ws_url(&server_url, &token);
                let _ = status_tx.send(ConnectionStatus::Connecting);

                match connect_async(&ws_url).await {
                    Ok((ws_stream, _response)) => {
                        info!("WebSocket connected to {}", ws_url);
                        attempt = 0;
                        delay = Duration::from_millis(1000);
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
                            tokio::select! {
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
                                                let _ = inbound_tx.send(frame);
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
                    Err(e) => {
                        warn!("WebSocket connect failure: {e}");
                    }
                }

                if !*is_running.read() {
                    break;
                }

                attempt += 1;
                let retry_secs = delay.as_secs().max(1);
                let _ = status_tx.send(ConnectionStatus::Reconnecting {
                    attempt,
                    next_retry_in_secs: retry_secs,
                });

                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        });
    }

    pub fn stop(&self) {
        *self.is_running.write() = false;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_ws_url() {
        let url = WsTransport::format_ws_url("http://34.170.16.35:8080", "secret_token_123");
        assert_eq!(url, "ws://34.170.16.35:8080/api/v1/ws?token=secret_token_123");

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
}
