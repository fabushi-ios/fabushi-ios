use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct WsChannel {
    outbound_tx: Arc<RwLock<Option<mpsc::UnboundedSender<WsClientFrame>>>>,
    inbound_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<WsServerFrame>>>>,
}

impl WsChannel {
    pub fn new() -> Self {
        Self {
            outbound_tx: Arc::new(RwLock::new(None)),
            inbound_rx: Arc::new(RwLock::new(None)),
        }
    }

    pub fn send(&self, frame: WsClientFrame) -> bool {
        if let Some(tx) = self.outbound_tx.read().as_ref() {
            tx.send(frame).is_ok()
        } else {
            false
        }
    }

    pub fn try_recv(&self) -> Option<WsServerFrame> {
        let mut rx_guard = self.inbound_rx.write();
        if let Some(rx) = rx_guard.as_mut() {
            rx.try_recv().ok()
        } else {
            None
        }
    }

    pub fn attach_channels(
        &self,
        outbound: mpsc::UnboundedSender<WsClientFrame>,
        inbound: mpsc::UnboundedReceiver<WsServerFrame>,
    ) {
        *self.outbound_tx.write() = Some(outbound);
        *self.inbound_rx.write() = Some(inbound);
    }
}

impl Default for WsChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_channel_lifecycle() {
        let channel = WsChannel::new();
        assert!(channel.try_recv().is_none());

        let (client_tx, mut client_rx) = mpsc::unbounded_channel::<WsClientFrame>();
        let (server_tx, server_rx) = mpsc::unbounded_channel::<WsServerFrame>();

        channel.attach_channels(client_tx, server_rx);

        let ping_frame = WsClientFrame::Ping { nonce: 42 };
        assert!(channel.send(ping_frame));

        let received_client = client_rx.try_recv().expect("client frame sent");
        match received_client {
            WsClientFrame::Ping { nonce } => assert_eq!(nonce, 42),
            _ => panic!("unexpected frame type"),
        }

        let pong_frame = WsServerFrame::Pong { nonce: 42 };
        server_tx.send(pong_frame).expect("send server frame");

        let received_server = channel.try_recv().expect("server frame received");
        match received_server {
            WsServerFrame::Pong { nonce } => assert_eq!(nonce, 42),
            _ => panic!("unexpected server frame"),
        }
    }
}

