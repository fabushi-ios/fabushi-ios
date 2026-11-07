use chrono::Utc;
use gpui_telegram_core::types::{MessageFlags, MessageId, PeerId, UserId};
use gpui_telegram_proto::message::{DeleteTombstone, MessageItem, ReactionCount, ReplyHeader};
use gpui_telegram_proto::sync::{
    PtsUpdate, ReadHistoryEvent, ReactionEvent, SyncGetDifferenceResponse, UpdateEvent,
};
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// GatewayHub manages real-time WebSocket connections across multiple devices per user,
/// chat event fan-out, and bidirectional frame handling.
#[derive(Debug, Clone)]
pub struct GatewayHub {
    /// Multi-device socket registry: UserId -> list of active socket senders
    subscribers: Arc<RwLock<HashMap<UserId, Vec<mpsc::UnboundedSender<WsServerFrame>>>>>,
    /// Chat subscriptions: ChatId -> Set of subscribed UserIds
    chat_subscribers: Arc<RwLock<HashMap<i64, HashSet<UserId>>>>,
    /// Next allocated sequential message ID
    next_msg_id: Arc<AtomicI64>,
    /// Server presentation sequence counter (PTS)
    next_pts: Arc<AtomicU64>,
}

impl Default for GatewayHub {
    fn default() -> Self {
        Self {
            subscribers: Arc::new(RwLock::new(HashMap::new())),
            chat_subscribers: Arc::new(RwLock::new(HashMap::new())),
            next_msg_id: Arc::new(AtomicI64::new(1001)),
            next_pts: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl GatewayHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an active WebSocket connection sender for a user
    pub fn register(&self, user_id: UserId, sender: mpsc::UnboundedSender<WsServerFrame>) {
        let mut subs = self.subscribers.write();
        subs.entry(user_id).or_default().push(sender);
        info!("Registered new WebSocket connection for {}", user_id);
    }

    /// Unregister a specific connection sender for a user
    pub fn unregister_sender(&self, user_id: UserId, sender: &mpsc::UnboundedSender<WsServerFrame>) {
        let mut subs = self.subscribers.write();
        if let Some(senders) = subs.get_mut(&user_id) {
            senders.retain(|s| !s.is_closed() && !s.same_channel(sender));
            if senders.is_empty() {
                subs.remove(&user_id);
            }
        }
        info!("Unregistered WebSocket connection for {}", user_id);
    }

    /// Unregister all connections for a user
    pub fn unregister(&self, user_id: UserId) {
        self.subscribers.write().remove(&user_id);
    }

    /// Disconnect all active sockets for a user (called on logout/revocation)
    pub fn disconnect_user(&self, user_id: UserId) {
        let senders = self.subscribers.write().remove(&user_id);
        if let Some(list) = senders {
            for sender in list {
                let _ = sender.send(WsServerFrame::Error {
                    code: 401,
                    message: "Session has been revoked".into(),
                    details: None,
                });
            }
        }
    }

    /// Get active connection count for a specific user
    pub fn user_connection_count(&self, user_id: UserId) -> usize {
        self.subscribers
            .read()
            .get(&user_id)
            .map(|list| list.iter().filter(|s| !s.is_closed()).count())
            .unwrap_or(0)
    }

    /// Total count of all active WebSocket connections across all users
    pub fn total_connections(&self) -> usize {
        self.subscribers
            .read()
            .values()
            .map(|list| list.iter().filter(|s| !s.is_closed()).count())
            .sum()
    }

    /// Broadcast frame to all active device sockets belonging to a user
    pub fn broadcast_to_user(&self, user_id: UserId, frame: WsServerFrame) {
        let mut subs = self.subscribers.write();
        if let Some(senders) = subs.get_mut(&user_id) {
            senders.retain(|tx| tx.send(frame.clone()).is_ok());
            if senders.is_empty() {
                subs.remove(&user_id);
            }
        }
    }

    /// Broadcast frame to all subscribers of a chat
    pub fn broadcast_to_chat(&self, chat_id: i64, frame: WsServerFrame) {
        let targets: Vec<UserId> = self
            .chat_subscribers
            .read()
            .get(&chat_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();

        for target in targets {
            self.broadcast_to_user(target, frame.clone());
        }
    }

    /// Subscribe user to chat events
    pub fn subscribe_chat(&self, chat_id: i64, user_id: UserId) {
        self.chat_subscribers
            .write()
            .entry(chat_id)
            .or_default()
            .insert(user_id);
    }

    /// Unsubscribe user from chat events
    pub fn unsubscribe_chat(&self, chat_id: i64, user_id: UserId) {
        let mut chats = self.chat_subscribers.write();
        if let Some(subs) = chats.get_mut(&chat_id) {
            subs.remove(&user_id);
            if subs.is_empty() {
                chats.remove(&chat_id);
            }
        }
    }

    /// Current PTS sequence number
    pub fn current_pts(&self) -> u64 {
        self.next_pts.load(Ordering::Acquire)
    }

    /// Allocate the next PTS sequence number
    pub fn next_pts(&self) -> u64 {
        self.next_pts.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Handle incoming client WebSocket frame
    pub fn handle_client_frame(
        &self,
        user_id: UserId,
        frame: WsClientFrame,
        reply_tx: &mpsc::UnboundedSender<WsServerFrame>,
    ) {
        match frame {
            WsClientFrame::Ping { nonce } => {
                let _ = reply_tx.send(WsServerFrame::Pong { nonce });
            }
            WsClientFrame::Sync(req) => {
                let current_pts = self.current_pts();
                let resp = SyncGetDifferenceResponse::new(
                    req.chat_id,
                    Vec::new(),
                    Vec::new(),
                    current_pts,
                    false,
                );
                let _ = reply_tx.send(WsServerFrame::SyncDiff(resp));
            }
            WsClientFrame::SendMessage(req) => {
                let msg_id = MessageId(self.next_msg_id.fetch_add(1, Ordering::SeqCst));
                let pts = self.next_pts();
                let now = Utc::now();
                let peer_id = PeerId::User(user_id);
                let to_peer = PeerId::Chat(req.chat_id);

                let message = MessageItem {
                    id: msg_id,
                    chat_id: req.chat_id,
                    peer_id,
                    to_peer,
                    sender_id: user_id,
                    topic_id: req.topic_id,
                    flags: MessageFlags {
                        is_out: true,
                        ..Default::default()
                    },
                    date: now,
                    edit_date: None,
                    content: req.content,
                    reply_to: req.reply_to.map(ReplyHeader::new),
                    forward_info: None,
                    reactions: Vec::new(),
                    client_nonce: Some(req.client_nonce),
                };

                // 1. Reply Ack to the sender socket
                let _ = reply_tx.send(WsServerFrame::Ack {
                    correlation_id: 0,
                    client_nonce: req.client_nonce,
                    message_id: msg_id.0,
                    pts,
                });

                // 2. Broadcast PtsPush to all devices of user and to chat subscribers
                let update = UpdateEvent::NewMessage(message);
                let pts_update = PtsUpdate::new(req.chat_id, pts, 1, update, now.timestamp_millis());

                self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update.clone()));
                self.broadcast_to_chat(req.chat_id.0, WsServerFrame::PtsPush(pts_update));
            }
            WsClientFrame::EditMessage(req) => {
                let pts = self.next_pts();
                let now = Utc::now();
                let peer_id = PeerId::User(user_id);
                let to_peer = PeerId::Chat(req.chat_id);

                let message = MessageItem {
                    id: req.message_id,
                    chat_id: req.chat_id,
                    peer_id,
                    to_peer,
                    sender_id: user_id,
                    topic_id: None,
                    flags: MessageFlags {
                        is_out: true,
                        is_edited: true,
                        ..Default::default()
                    },
                    date: now,
                    edit_date: Some(now),
                    content: req.content,
                    reply_to: None,
                    forward_info: None,
                    reactions: Vec::new(),
                    client_nonce: None,
                };

                let update = UpdateEvent::EditMessage(message);
                let pts_update = PtsUpdate::new(req.chat_id, pts, 1, update, now.timestamp_millis());

                self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update.clone()));
                self.broadcast_to_chat(req.chat_id.0, WsServerFrame::PtsPush(pts_update));
            }
            WsClientFrame::DeleteMessages(req) => {
                let pts = self.next_pts();
                let now = Utc::now();
                let tombstone = DeleteTombstone::new(
                    PeerId::Chat(req.chat_id),
                    req.message_ids,
                    req.for_everyone,
                );
                let update = UpdateEvent::DeleteMessages(tombstone);
                let pts_update = PtsUpdate::new(req.chat_id, pts, 1, update, now.timestamp_millis());

                self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update.clone()));
                self.broadcast_to_chat(req.chat_id.0, WsServerFrame::PtsPush(pts_update));
            }
            WsClientFrame::SendReaction(req) => {
                let pts = self.next_pts();
                let now = Utc::now();
                let count = if req.add { 1 } else { 0 };
                let rx_count = ReactionCount::new(req.emoji, count, req.add);
                let event = ReactionEvent::new(req.chat_id, req.message_id, user_id, vec![rx_count]);
                let update = UpdateEvent::ReactionUpdated(event);
                let pts_update = PtsUpdate::new(req.chat_id, pts, 1, update, now.timestamp_millis());

                self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update.clone()));
                self.broadcast_to_chat(req.chat_id.0, WsServerFrame::PtsPush(pts_update));
            }
            WsClientFrame::ReadHistory(req) => {
                let pts = self.next_pts();
                let now = Utc::now();
                let event = ReadHistoryEvent::new(req.chat_id, req.max_id, false, 0);
                let update = UpdateEvent::ReadHistory(event);
                let pts_update = PtsUpdate::new(req.chat_id, pts, 1, update, now.timestamp_millis());

                self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update.clone()));
                self.broadcast_to_chat(req.chat_id.0, WsServerFrame::PtsPush(pts_update));
            }
            WsClientFrame::SendTyping(event) => {
                // Broadcast ephemeral typing event to chat subscribers WITHOUT advancing PTS
                let frame = WsServerFrame::Update(UpdateEvent::UserTyping(event.clone()));
                self.broadcast_to_chat(event.chat_id.0, frame);
            }
            WsClientFrame::SubscribeChat { chat_id } => {
                self.subscribe_chat(chat_id, user_id);
            }
            WsClientFrame::UnsubscribeChat { chat_id } => {
                self.unsubscribe_chat(chat_id, user_id);
            }
        }
    }
}

/// Upgraded WebSocket connection handler with 30s ping / 60s inactivity timeout
pub async fn handle_ws_socket(
    socket: axum::extract::ws::WebSocket,
    user_id: UserId,
    hub: Arc<GatewayHub>,
    heartbeat_interval_secs: u64,
    inactivity_timeout_secs: u64,
) {
    use axum::extract::ws::Message;
    use futures::{SinkExt, StreamExt};
    use std::time::Duration;

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsServerFrame>();

    hub.register(user_id, tx.clone());

    // Task forwarding outgoing frames to the WebSocket client
    let forward_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            match serde_json::to_string(&frame) {
                Ok(json) => {
                    if ws_sender.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    error!("Failed to serialize WsServerFrame: {e}");
                }
            }
        }
    });

    let tx_clone = tx.clone();
    let hub_clone = hub.clone();
    let mut last_activity = tokio::time::Instant::now();
    let inactivity_limit = Duration::from_secs(inactivity_timeout_secs);
    let ping_interval = Duration::from_secs(heartbeat_interval_secs);
    let mut heartbeat_timer = tokio::time::interval(ping_interval);

    loop {
        tokio::select! {
            _ = heartbeat_timer.tick() => {
                if last_activity.elapsed() >= inactivity_limit {
                    warn!("WebSocket client {} timed out after {}s inactivity", user_id, inactivity_timeout_secs);
                    break;
                }
            }
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        last_activity = tokio::time::Instant::now();
                        match serde_json::from_str::<WsClientFrame>(&text) {
                            Ok(client_frame) => {
                                hub_clone.handle_client_frame(user_id, client_frame, &tx_clone);
                            }
                            Err(e) => {
                                let _ = tx_clone.send(WsServerFrame::Error {
                                    code: 400,
                                    message: format!("Malformed client frame: {e}"),
                                    details: None,
                                });
                            }
                        }
                    }
                    Some(Ok(Message::Ping(_payload))) => {
                        last_activity = tokio::time::Instant::now();
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_activity = tokio::time::Instant::now();
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    Some(Err(e)) => {
                        debug!("WebSocket error for {}: {e}", user_id);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    forward_task.abort();
    hub.unregister_sender(user_id, &tx);
}
