use chrono::Utc;
use gpui_telegram_core::types::{ChatId, MessageFlags, MessageId, MsgId, PeerId, UserId, UserStatus};
use gpui_telegram_proto::contracts::CallSignalRequest;
use gpui_telegram_proto::message::{DeleteTombstone, MessageContent, MessageItem, ReactionCount, ReplyHeader};
use gpui_telegram_proto::sync::{
    PtsUpdate, ReadHistoryEvent, ReactionEvent, SyncGetDifferenceResponse, UpdateEvent,
};
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use gpui_telegram_webrtc::signaling::SignalingFrame;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Clone)]
struct WsSubscriberEntry {
    session_id: Option<String>,
    sender: mpsc::UnboundedSender<WsServerFrame>,
}

struct BusTaskGuard {
    tasks: Arc<RwLock<Vec<tokio::task::AbortHandle>>>,
}

impl Drop for BusTaskGuard {
    fn drop(&mut self) {
        let drained: Vec<_> = self.tasks.write().drain(..).collect();
        for handle in drained {
            handle.abort();
        }
    }
}

struct WsTaskGuard {
    tasks: Arc<RwLock<HashMap<u64, Vec<tokio::task::AbortHandle>>>>,
    shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Drop for WsTaskGuard {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        let drained: Vec<_> = self.tasks.write().drain().collect();
        for (_conn_id, handles) in drained {
            for handle in handles {
                handle.abort();
            }
        }
    }
}

/// GatewayHub manages real-time WebSocket connections across multiple devices per user,
/// chat event fan-out, and bidirectional frame handling.
#[derive(Clone)]
pub struct GatewayHub {
    /// Multi-device socket registry: UserId -> list of active socket entries
    subscribers: Arc<RwLock<HashMap<UserId, Vec<WsSubscriberEntry>>>>,
    /// Chat subscriptions: ChatId -> Set of subscribed UserIds
    chat_subscribers: Arc<RwLock<HashMap<i64, HashSet<UserId>>>>,
    /// Next allocated sequential message ID
    next_msg_id: Arc<AtomicI64>,
    /// Server presentation sequence counter (PTS)
    next_pts: Arc<AtomicU64>,
    /// Optional attached message pipeline for real persistence and deduplication
    pipeline: Arc<RwLock<Option<Arc<crate::messaging::pipeline::MessagePipeline>>>>,
    /// Optional attached server database for membership queries
    db: Arc<RwLock<Option<Arc<crate::db::ServerDatabase>>>>,
    /// Optional Tantivy search indexer for real-time WS message indexing
    search_indexer: Arc<RwLock<Option<Arc<crate::search::tantivy_indexer::TantivyIndexer>>>>,
    /// Optional WebRTC signaling hub for call session state & relay
    signaling_hub: Arc<RwLock<Option<Arc<crate::webrtc::signaling_hub::SignalingHub>>>>,
    /// Optional CrossNodeEventBus for cluster-wide stateless event fan-out
    event_bus: Arc<RwLock<Option<Arc<dyn crate::bus::CrossNodeEventBus>>>>,
    /// Ordered outbound bus publisher channel
    outbound_bus_tx: Arc<RwLock<Option<mpsc::UnboundedSender<(String, crate::bus::ServerEvent)>>>>,
    /// Background task abort handles for active event bus tasks
    bus_tasks: Arc<RwLock<Vec<tokio::task::AbortHandle>>>,
    /// Guard that aborts background event bus tasks when all GatewayHub handles drop
    bus_guard: Arc<BusTaskGuard>,
    /// Active WebSocket helper task abort handles keyed by connection id
    ws_tasks: Arc<RwLock<HashMap<u64, Vec<tokio::task::AbortHandle>>>>,
    /// Monotonic WebSocket connection id allocator
    next_ws_conn_id: Arc<AtomicU64>,
    /// Shutdown signal broadcast to active WebSocket connections
    shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
    /// Guard that signals and aborts active WebSocket tasks when all GatewayHub handles drop
    ws_guard: Arc<WsTaskGuard>,
    /// Optional callback invoked when a remote SessionRevoked event arrives
    session_revoked_handler: Arc<RwLock<Option<Arc<dyn Fn(u64, &str) + Send + Sync>>>>,
    /// Shared liveness token for clean background task shutdown
    liveness: Arc<()>,
}

struct WeakGatewayHub {
    subscribers: std::sync::Weak<RwLock<HashMap<UserId, Vec<WsSubscriberEntry>>>>,
    chat_subscribers: std::sync::Weak<RwLock<HashMap<i64, HashSet<UserId>>>>,
    next_msg_id: std::sync::Weak<AtomicI64>,
    next_pts: std::sync::Weak<AtomicU64>,
    pipeline: std::sync::Weak<RwLock<Option<Arc<crate::messaging::pipeline::MessagePipeline>>>>,
    db: std::sync::Weak<RwLock<Option<Arc<crate::db::ServerDatabase>>>>,
    search_indexer: std::sync::Weak<RwLock<Option<Arc<crate::search::tantivy_indexer::TantivyIndexer>>>>,
    signaling_hub: std::sync::Weak<RwLock<Option<Arc<crate::webrtc::signaling_hub::SignalingHub>>>>,
    event_bus: std::sync::Weak<RwLock<Option<Arc<dyn crate::bus::CrossNodeEventBus>>>>,
    outbound_bus_tx: std::sync::Weak<RwLock<Option<mpsc::UnboundedSender<(String, crate::bus::ServerEvent)>>>>,
    bus_tasks: std::sync::Weak<RwLock<Vec<tokio::task::AbortHandle>>>,
    bus_guard: std::sync::Weak<BusTaskGuard>,
    ws_tasks: std::sync::Weak<RwLock<HashMap<u64, Vec<tokio::task::AbortHandle>>>>,
    next_ws_conn_id: std::sync::Weak<AtomicU64>,
    shutdown_tx: std::sync::Weak<tokio::sync::watch::Sender<bool>>,
    ws_guard: std::sync::Weak<WsTaskGuard>,
    session_revoked_handler: std::sync::Weak<RwLock<Option<Arc<dyn Fn(u64, &str) + Send + Sync>>>>,
    liveness: std::sync::Weak<()>,
}

impl WeakGatewayHub {
    fn upgrade(&self) -> Option<GatewayHub> {
        Some(GatewayHub {
            subscribers: self.subscribers.upgrade()?,
            chat_subscribers: self.chat_subscribers.upgrade()?,
            next_msg_id: self.next_msg_id.upgrade()?,
            next_pts: self.next_pts.upgrade()?,
            pipeline: self.pipeline.upgrade()?,
            db: self.db.upgrade()?,
            search_indexer: self.search_indexer.upgrade()?,
            signaling_hub: self.signaling_hub.upgrade()?,
            event_bus: self.event_bus.upgrade()?,
            outbound_bus_tx: self.outbound_bus_tx.upgrade()?,
            bus_tasks: self.bus_tasks.upgrade()?,
            bus_guard: self.bus_guard.upgrade()?,
            ws_tasks: self.ws_tasks.upgrade()?,
            next_ws_conn_id: self.next_ws_conn_id.upgrade()?,
            shutdown_tx: self.shutdown_tx.upgrade()?,
            ws_guard: self.ws_guard.upgrade()?,
            session_revoked_handler: self.session_revoked_handler.upgrade()?,
            liveness: self.liveness.upgrade()?,
        })
    }
}

impl std::fmt::Debug for GatewayHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayHub")
            .field("subscribers_count", &self.subscribers.read().len())
            .field("chat_subscribers_count", &self.chat_subscribers.read().len())
            .finish()
    }
}

impl Default for GatewayHub {
    fn default() -> Self {
        let bus_tasks = Arc::new(RwLock::new(Vec::new()));
        let bus_guard = Arc::new(BusTaskGuard {
            tasks: bus_tasks.clone(),
        });
        let ws_tasks = Arc::new(RwLock::new(HashMap::new()));
        let (shutdown_tx_raw, _shutdown_rx) = tokio::sync::watch::channel(false);
        let shutdown_tx = Arc::new(shutdown_tx_raw);
        let ws_guard = Arc::new(WsTaskGuard {
            tasks: ws_tasks.clone(),
            shutdown_tx: shutdown_tx.clone(),
        });
        Self {
            subscribers: Arc::new(RwLock::new(HashMap::new())),
            chat_subscribers: Arc::new(RwLock::new(HashMap::new())),
            next_msg_id: Arc::new(AtomicI64::new(1001)),
            next_pts: Arc::new(AtomicU64::new(1)),
            pipeline: Arc::new(RwLock::new(None)),
            db: Arc::new(RwLock::new(None)),
            search_indexer: Arc::new(RwLock::new(None)),
            signaling_hub: Arc::new(RwLock::new(None)),
            event_bus: Arc::new(RwLock::new(None)),
            outbound_bus_tx: Arc::new(RwLock::new(None)),
            bus_tasks,
            bus_guard,
            ws_tasks,
            next_ws_conn_id: Arc::new(AtomicU64::new(0)),
            shutdown_tx,
            ws_guard,
            session_revoked_handler: Arc::new(RwLock::new(None)),
            liveness: Arc::new(()),
        }
    }
}

impl GatewayHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_with_pipeline(
        pipeline: Arc<crate::messaging::pipeline::MessagePipeline>,
        db: Arc<crate::db::ServerDatabase>,
    ) -> Self {
        let hub = Self::default();
        *hub.pipeline.write() = Some(pipeline);
        *hub.db.write() = Some(db);
        hub
    }

    pub fn set_pipeline(
        &self,
        pipeline: Arc<crate::messaging::pipeline::MessagePipeline>,
        db: Arc<crate::db::ServerDatabase>,
    ) {
        *self.pipeline.write() = Some(pipeline);
        *self.db.write() = Some(db);
    }

    pub fn set_search_indexer(&self, indexer: Arc<crate::search::tantivy_indexer::TantivyIndexer>) {
        *self.search_indexer.write() = Some(indexer);
    }

    pub fn set_signaling_hub(&self, sh: Arc<crate::webrtc::signaling_hub::SignalingHub>) {
        *self.signaling_hub.write() = Some(sh);
    }

    pub fn signaling_hub(&self) -> Option<Arc<crate::webrtc::signaling_hub::SignalingHub>> {
        self.signaling_hub.read().clone()
    }

    pub fn set_session_revoked_handler(&self, handler: Arc<dyn Fn(u64, &str) + Send + Sync>) {
        *self.session_revoked_handler.write() = Some(handler);
    }

    pub fn set_event_bus(&self, bus: Arc<dyn crate::bus::CrossNodeEventBus>) {
        {
            let mut tasks = self.bus_tasks.write();
            for handle in tasks.drain(..) {
                handle.abort();
            }
        }
        *self.outbound_bus_tx.write() = None;
        *self.event_bus.write() = Some(bus);
        self.ensure_bus_started();
    }

    /// Subscribe to the hub shutdown signal (`true` when shutting down).
    pub fn subscribe_shutdown(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    /// Returns `true` if `shutdown()` has been initiated on this hub.
    pub fn is_shutdown(&self) -> bool {
        *self.shutdown_tx.borrow()
    }

    fn register_ws_tasks(&self, handles: Vec<tokio::task::AbortHandle>) -> u64 {
        let conn_id = self.next_ws_conn_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.ws_tasks.write().insert(conn_id, handles);
        if self.is_shutdown() {
            if let Some(removed) = self.ws_tasks.write().remove(&conn_id) {
                for handle in removed {
                    handle.abort();
                }
            }
        }
        conn_id
    }

    fn unregister_ws_tasks(&self, conn_id: u64) {
        self.ws_tasks.write().remove(&conn_id);
    }

    /// Signals all active WebSocket connections to close, aborts background bus & WS helper tasks,
    /// and clears strong references to `TantivyIndexer`, `ServerDatabase`, `MessagePipeline`,
    /// `SignalingHub`, and `CrossNodeEventBus`.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);

        let ws_drained: Vec<_> = self.ws_tasks.write().drain().collect();
        let mut forward_handles_for_reap = Vec::new();
        for (_conn_id, handles) in ws_drained {
            // Index 0 is forward_task (which sends Message::Close(None) on shutdown_tx);
            // abort helper tasks (like sig_task) immediately, and give forward_task a brief
            // bounded grace period to flush the WebSocket Close frame cleanly.
            for (idx, handle) in handles.into_iter().enumerate() {
                if idx == 0 {
                    forward_handles_for_reap.push(handle);
                } else {
                    handle.abort();
                }
            }
        }
        if !forward_handles_for_reap.is_empty() {
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    for handle in forward_handles_for_reap {
                        handle.abort();
                    }
                });
            }
        }

        let bus_drained: Vec<_> = self.bus_tasks.write().drain(..).collect();
        for handle in bus_drained {
            handle.abort();
        }

        let old_outbound = self.outbound_bus_tx.write().take();
        drop(old_outbound);

        let old_subs: Vec<_> = self.subscribers.write().drain().collect();
        drop(old_subs);

        let old_chat_subs: Vec<_> = self.chat_subscribers.write().drain().collect();
        drop(old_chat_subs);

        let old_search = self.search_indexer.write().take();
        drop(old_search);

        let old_pipeline = self.pipeline.write().take();
        drop(old_pipeline);

        let old_db = self.db.write().take();
        drop(old_db);

        let old_sh = self.signaling_hub.write().take();
        drop(old_sh);

        let old_bus = self.event_bus.write().take();
        drop(old_bus);

        let old_revoked = self.session_revoked_handler.write().take();
        drop(old_revoked);
    }

    fn downgrade(&self) -> WeakGatewayHub {
        WeakGatewayHub {
            subscribers: Arc::downgrade(&self.subscribers),
            chat_subscribers: Arc::downgrade(&self.chat_subscribers),
            next_msg_id: Arc::downgrade(&self.next_msg_id),
            next_pts: Arc::downgrade(&self.next_pts),
            pipeline: Arc::downgrade(&self.pipeline),
            db: Arc::downgrade(&self.db),
            search_indexer: Arc::downgrade(&self.search_indexer),
            signaling_hub: Arc::downgrade(&self.signaling_hub),
            event_bus: Arc::downgrade(&self.event_bus),
            outbound_bus_tx: Arc::downgrade(&self.outbound_bus_tx),
            bus_tasks: Arc::downgrade(&self.bus_tasks),
            bus_guard: Arc::downgrade(&self.bus_guard),
            ws_tasks: Arc::downgrade(&self.ws_tasks),
            next_ws_conn_id: Arc::downgrade(&self.next_ws_conn_id),
            shutdown_tx: Arc::downgrade(&self.shutdown_tx),
            ws_guard: Arc::downgrade(&self.ws_guard),
            session_revoked_handler: Arc::downgrade(&self.session_revoked_handler),
            liveness: Arc::downgrade(&self.liveness),
        }
    }

    pub fn ensure_bus_started(&self) {
        if self.is_shutdown() {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Some(bus) = self.event_bus.read().clone() else {
            return;
        };

        let mut tasks = self.bus_tasks.write();
        if !tasks.is_empty() && self.outbound_bus_tx.read().is_some() {
            return;
        }
        for handle in tasks.drain(..) {
            handle.abort();
        }

        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<(String, crate::bus::ServerEvent)>();
        *self.outbound_bus_tx.write() = Some(out_tx);

        let bus_out = bus.clone();
        let liveness_out = Arc::downgrade(&self.liveness);
        let out_handle = rt.spawn(async move {
            while let Some((channel, event)) = out_rx.recv().await {
                if liveness_out.upgrade().is_none() {
                    break;
                }
                if let Err(e) = bus_out.publish(&channel, &event).await {
                    warn!("CrossNodeEventBus publish failed on {channel}: {e}");
                }
            }
        });
        tasks.push(out_handle.abort_handle());

        let hub_weak = self.downgrade();
        let liveness_in = Arc::downgrade(&self.liveness);
        let in_handle = rt.spawn(async move {
            let local_node_id = bus.node_id().to_string();
            let mut sub_user = match bus.subscribe("user:*").await {
                Ok(s) => s,
                Err(e) => {
                    warn!("GatewayHub failed to subscribe user:*: {e}");
                    return;
                }
            };
            let mut sub_chat = match bus.subscribe("chat:*").await {
                Ok(s) => s,
                Err(e) => {
                    warn!("GatewayHub failed to subscribe chat:*: {e}");
                    return;
                }
            };
            let mut sub_sys = match bus.subscribe("system:*").await {
                Ok(s) => s,
                Err(e) => {
                    warn!("GatewayHub failed to subscribe system:*: {e}");
                    return;
                }
            };

            loop {
                if liveness_in.upgrade().is_none() {
                    break;
                }

                let evt_res = tokio::select! {
                    r = sub_user.recv() => r,
                    r = sub_chat.recv() => r,
                    r = sub_sys.recv() => r,
                };

                match evt_res {
                    Ok(evt) => {
                        if evt.origin_node_id == local_node_id {
                            continue;
                        }
                        let Some(hub) = hub_weak.upgrade() else {
                            break;
                        };
                        hub.handle_remote_event(evt);
                    }
                    Err(crate::bus::EventBusError::ChannelLagged(n)) => {
                        warn!("GatewayHub cluster subscription lagged {n} events");
                        continue;
                    }
                    Err(crate::bus::EventBusError::ChannelClosed) => {
                        break;
                    }
                    Err(e) => {
                        warn!("GatewayHub cluster subscription error: {e}");
                        continue;
                    }
                }
            }
        });
        tasks.push(in_handle.abort_handle());
    }

    fn handle_remote_event(&self, evt: crate::bus::ServerEvent) {
        match evt.payload {
            crate::bus::EventPayload::RawJson { event_type, data }
                if event_type == "ws_server_frame" =>
            {
                let Ok(frame) = serde_json::from_value::<WsServerFrame>(data) else {
                    return;
                };
                if let Some(uid_str) = evt.channel.strip_prefix("user:") {
                    if let Ok(uid) = uid_str.parse::<u64>() {
                        self.broadcast_to_user_local(UserId(uid), frame);
                    }
                } else if let Some(cid_str) = evt.channel.strip_prefix("chat:") {
                    if let Ok(cid) = cid_str.parse::<i64>() {
                        let targets = self.filter_and_prune_chat_subscribers(cid);
                        for uid in targets {
                            self.broadcast_to_user_local(uid, frame.clone());
                        }
                    }
                } else if evt.channel.starts_with("system:") {
                    let targets: Vec<UserId> = self.subscribers.read().keys().copied().collect();
                    for uid in targets {
                        self.broadcast_to_user_local(uid, frame.clone());
                    }
                }
            }
            crate::bus::EventPayload::SessionRevoked {
                user_id,
                session_id,
                reason: _,
            } => {
                if let Some(handler) = self.session_revoked_handler.read().clone() {
                    handler(user_id, &session_id);
                }
                self.disconnect_session(UserId(user_id), &session_id);
            }
            _ => {}
        }
    }

    pub fn event_bus(&self) -> Option<Arc<dyn crate::bus::CrossNodeEventBus>> {
        self.event_bus.read().clone()
    }

    /// Register an active WebSocket connection sender for a user
    pub fn register(&self, user_id: UserId, sender: mpsc::UnboundedSender<WsServerFrame>) {
        self.register_session(user_id, None, sender);
    }

    /// Register an active WebSocket connection sender associated with an optional session_id
    pub fn register_session(
        &self,
        user_id: UserId,
        session_id: Option<String>,
        sender: mpsc::UnboundedSender<WsServerFrame>,
    ) {
        if self.is_shutdown() {
            return;
        }
        self.ensure_bus_started();
        let mut subs = self.subscribers.write();
        subs.entry(user_id).or_default().push(WsSubscriberEntry {
            session_id,
            sender,
        });
        info!("Registered new WebSocket connection for {}", user_id);
    }

    /// Unregister a specific connection sender for a user
    pub fn unregister_sender(&self, user_id: UserId, sender: &mpsc::UnboundedSender<WsServerFrame>) {
        let mut subs = self.subscribers.write();
        if let Some(entries) = subs.get_mut(&user_id) {
            entries.retain(|e| !e.sender.is_closed() && !e.sender.same_channel(sender));
            if entries.is_empty() {
                subs.remove(&user_id);
            }
        }
        info!("Unregistered WebSocket connection for {}", user_id);
    }

    /// Unregister all connections for a user
    pub fn unregister(&self, user_id: UserId) {
        self.subscribers.write().remove(&user_id);
    }

    /// Disconnect active sockets for a specific session
    pub fn disconnect_session(&self, user_id: UserId, session_id: &str) {
        let mut removed = Vec::new();
        {
            let mut subs = self.subscribers.write();
            if let Some(entries) = subs.get_mut(&user_id) {
                let mut i = 0;
                while i < entries.len() {
                    let matches = entries[i].session_id.as_deref() == Some(session_id);
                    if matches {
                        removed.push(entries.swap_remove(i));
                    } else {
                        i += 1;
                    }
                }
                if entries.is_empty() {
                    subs.remove(&user_id);
                }
            }
        }
        for entry in removed {
            let _ = entry.sender.send(WsServerFrame::Error {
                code: 401,
                message: "Session has been revoked".into(),
                details: None,
            });
        }
    }

    /// Disconnect all active sockets for a user (called on logout/revocation)
    pub fn disconnect_user(&self, user_id: UserId) {
        let entries = self.subscribers.write().remove(&user_id);
        if let Some(list) = entries {
            for entry in list {
                let _ = entry.sender.send(WsServerFrame::Error {
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
            .map(|list| list.iter().filter(|e| !e.sender.is_closed()).count())
            .unwrap_or(0)
    }

    /// Total count of all active WebSocket connections across all users
    pub fn total_connections(&self) -> usize {
        self.subscribers
            .read()
            .values()
            .map(|list| list.iter().filter(|e| !e.sender.is_closed()).count())
            .sum()
    }

    /// Local-only broadcast frame to all active device sockets belonging to a user on this process
    pub fn broadcast_to_user_local(&self, user_id: UserId, frame: WsServerFrame) {
        let mut subs = self.subscribers.write();
        if let Some(entries) = subs.get_mut(&user_id) {
            entries.retain(|e| e.sender.send(frame.clone()).is_ok());
            if entries.is_empty() {
                subs.remove(&user_id);
            }
        }
    }

    fn publish_to_event_bus(&self, channel: String, frame: &WsServerFrame) {
        self.ensure_bus_started();
        let Some(bus) = self.event_bus.read().clone() else {
            return;
        };
        let payload = crate::bus::EventPayload::RawJson {
            event_type: "ws_server_frame".to_string(),
            data: serde_json::to_value(frame).unwrap_or_default(),
        };
        let event = crate::bus::ServerEvent::new(&channel, bus.node_id(), payload);
        if let Some(tx) = self.outbound_bus_tx.read().clone() {
            let _ = tx.send((channel, event));
        } else if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = bus.publish(&channel, &event).await;
            });
        }
    }

    /// Broadcast frame to all active device sockets belonging to a user (local sockets + CrossNodeEventBus)
    pub fn broadcast_to_user(&self, user_id: UserId, frame: WsServerFrame) {
        self.broadcast_to_user_local(user_id, frame.clone());
        self.publish_to_event_bus(format!("user:{}", user_id.0), &frame);
    }

    /// Broadcast frame to all subscribers of a chat (local subscribers + CrossNodeEventBus)
    pub fn broadcast_to_chat(&self, chat_id: i64, frame: WsServerFrame) {
        let targets = self.filter_and_prune_chat_subscribers(chat_id);

        for target in targets {
            self.broadcast_to_user_local(target, frame.clone());
        }

        self.publish_to_event_bus(format!("chat:{}", chat_id), &frame);
    }

    /// Subscribe user to chat events
    pub fn subscribe_chat(&self, chat_id: i64, user_id: UserId) {
        self.ensure_bus_started();
        self.chat_subscribers
            .write()
            .entry(chat_id)
            .or_default()
            .insert(user_id);
    }

    /// Returns a set of user IDs that are subscribed to a chat
    pub fn get_chat_subscribers(&self, chat_id: i64) -> HashSet<UserId> {
        self.chat_subscribers
            .read()
            .get(&chat_id)
            .cloned()
            .unwrap_or_default()
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

    fn filter_and_prune_chat_subscribers(&self, chat_id: i64) -> Vec<UserId> {
        let subs: Vec<UserId> = self
            .chat_subscribers
            .read()
            .get(&chat_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        if subs.is_empty() {
            return subs;
        }
        let db_opt = self.db.read().clone();
        let Some(db) = db_opt else {
            return subs;
        };
        let mut active = Vec::with_capacity(subs.len());
        let mut stale = Vec::new();
        for uid in subs {
            match Self::is_member_in_db(&db, ChatId(chat_id), uid) {
                Ok(_) => active.push(uid),
                Err(403) | Err(404) => stale.push(uid),
                Err(_) => {}
            }
        }
        for uid in stale {
            self.unsubscribe_chat(chat_id, uid);
        }
        active
    }

    /// Collect all DB members + explicit subscribers of a chat and push frame to each.
    pub fn broadcast_to_chat_and_members(
        &self,
        chat_id: ChatId,
        ensure_user: Option<UserId>,
        frame: WsServerFrame,
    ) {
        let db_opt = self.db.read().clone();
        let mut targets = HashSet::new();
        if let Some(uid) = ensure_user {
            if db_opt
                .as_ref()
                .map_or(true, |db| Self::is_member_in_db(db, chat_id, uid).is_ok())
            {
                targets.insert(uid);
            }
        }
        if let Some(ref db) = db_opt {
            if let Ok(members) = db.get_chat_members(chat_id) {
                for uid in members {
                    if Self::is_member_in_db(db, chat_id, uid).is_ok() {
                        targets.insert(uid);
                    }
                }
            }
        }
        targets.extend(self.filter_and_prune_chat_subscribers(chat_id.0));
        for uid in targets {
            self.broadcast_to_user(uid, frame.clone());
        }
    }

    fn resolve_db(
        &self,
        pipeline: Option<&Arc<crate::messaging::pipeline::MessagePipeline>>,
    ) -> Option<Arc<crate::db::ServerDatabase>> {
        self.db
            .read()
            .clone()
            .or_else(|| pipeline.map(|p| p.db().clone()))
    }

    fn is_member_in_db(db: &crate::db::ServerDatabase, chat_id: ChatId, user_id: UserId) -> Result<bool, u16> {
        match db.get_chat(chat_id) {
            Ok(Some(chat)) => {
                if chat.creator_id == user_id {
                    return Ok(true);
                }
                match db.get_chat_member(chat_id, user_id) {
                    Ok(Some(m)) => {
                        if crate::db::ServerDatabase::is_inactive_member_role(&m.role) {
                            Err(403)
                        } else {
                            Ok(true)
                        }
                    }
                    Ok(None) => Err(403),
                    Err(_) => Err(500),
                }
            }
            Ok(None) => Err(404),
            Err(_) => Err(500),
        }
    }

    fn check_send_permitted_in_db(
        db: &crate::db::ServerDatabase,
        chat_id: ChatId,
        user_id: UserId,
    ) -> Result<(), (u16, &'static str, Option<&'static str>)> {
        let chat = match db.get_chat(chat_id) {
            Ok(Some(chat)) => chat,
            Ok(None) => return Err((404, "Chat not found", None)),
            Err(_) => return Err((500, "Database error", None)),
        };

        let member_opt = match db.get_chat_member(chat_id, user_id) {
            Ok(m) => m,
            Err(_) => return Err((500, "Database error", None)),
        };

        if chat.creator_id != user_id {
            match member_opt {
                Some(ref m) if !crate::db::ServerDatabase::is_inactive_member_role(&m.role) => {}
                _ => return Err((403, "Not a member of this chat", None)),
            }
        }

        if let Some(ref m) = member_opt {
            if m.role.trim().eq_ignore_ascii_case("restricted") && (m.banned_rights & 1) != 0 {
                return Err((403, "User is restricted from sending messages", None));
            }
        }

        if matches!(chat.chat_type, 0 | 4) {
            if let Ok(members) = db.get_chat_members(chat_id) {
                for peer_uid in members {
                    if peer_uid != user_id
                        && (db.is_user_blocked(peer_uid, user_id).unwrap_or(false)
                            || db.is_user_blocked(user_id, peer_uid).unwrap_or(false))
                    {
                        return Err((
                            403,
                            "Cannot send messages in a blocked direct chat",
                            Some("user_is_blocked"),
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    fn is_admin_in_db(db: &crate::db::ServerDatabase, chat_id: ChatId, user_id: UserId) -> bool {
        if let Ok(Some(chat)) = db.get_chat(chat_id) {
            if chat.creator_id == user_id {
                return true;
            }
        }
        if let Ok(Some(member)) = db.get_chat_member(chat_id, user_id) {
            return crate::db::ServerDatabase::is_admin_member_role(&member.role);
        }
        false
    }

    pub fn broadcast_user_status(&self, user_id: UserId, is_online: bool) {
        let peers: HashSet<UserId> = if let Some(ref db) = *self.db.read() {
            let mut set = HashSet::new();
            if let Ok(chats) = db.list_user_chats(user_id) {
                for chat in chats {
                    if let Ok(members) = db.get_chat_members(chat.chat_id) {
                        for m in members {
                            if m != user_id {
                                set.insert(m);
                            }
                        }
                    }
                }
            }
            set
        } else {
            return;
        };
        if peers.is_empty() {
            return;
        }
        let now = Utc::now();
        let status = if is_online {
            UserStatus::Online {
                expires_at: now + chrono::Duration::minutes(5),
            }
        } else {
            UserStatus::Offline {
                was_online: Some(now),
            }
        };
        let frame = WsServerFrame::Update(UpdateEvent::UserStatusChanged { user_id, status });
        for peer in peers {
            self.broadcast_to_user(peer, frame.clone());
        }
    }

    fn index_ws_message(&self, chat_id: ChatId, msg_id: i64, sender_id: UserId, date_ms: i64, content: &MessageContent) {
        if matches!(content, MessageContent::Encrypted(_)) {
            return;
        }
        if let Some(ref indexer) = *self.search_indexer.read() {
            let text = match content {
                MessageContent::Text { text, .. } => text.clone(),
                MessageContent::Photo { caption, .. }
                | MessageContent::Video { caption, .. } => caption.clone().unwrap_or_default(),
                MessageContent::Document { file_name, .. } => file_name.clone(),
                MessageContent::Poll(p) => p.question.clone(),
                _ => String::new(),
            };
            if !text.is_empty() {
                indexer.index_message_nonblocking(crate::search::schema::SearchDocument::new(
                    msg_id.to_string(),
                    chat_id.0.to_string(),
                    sender_id.0.to_string(),
                    date_ms,
                    text,
                    None,
                ));
                indexer.commit_nonblocking();
            }
        }
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
                let maybe_pipeline = self.pipeline.read().clone();
                let maybe_db = self.resolve_db(maybe_pipeline.as_ref());
                if let Some(ref db) = maybe_db {
                    if let Err(code) = Self::is_member_in_db(db, req.chat_id, user_id) {
                        let _ = reply_tx.send(WsServerFrame::Error {
                            code,
                            message: match code {
                                404 => "Chat not found",
                                500 => "Database error",
                                _ => "Not authorized for chat",
                            }
                            .into(),
                            details: None,
                        });
                        return;
                    }
                }
                if let Some(pipeline) = maybe_pipeline {
                    match pipeline.pts_engine().get_difference(&req) {
                        Ok(resp) => {
                            let _ = reply_tx.send(WsServerFrame::SyncDiff(resp));
                        }
                        Err(e) => {
                            let _ = reply_tx.send(WsServerFrame::Error {
                                code: 500,
                                message: format!("Sync error: {e}"),
                                details: None,
                            });
                        }
                    }
                } else {
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
            }
            WsClientFrame::SendMessage(req) => {
                if let MessageContent::Encrypted(ref env) = req.content {
                    if env.sender_user_id != user_id.0 {
                        let _ = reply_tx.send(WsServerFrame::Error {
                            code: 403,
                            message: "EncryptedEnvelope sender_user_id must match authenticated user".into(),
                            details: None,
                        });
                        return;
                    }
                }
                let maybe_pipeline = self.pipeline.read().clone();
                let maybe_db = self.resolve_db(maybe_pipeline.as_ref());

                if let Some(ref db) = maybe_db {
                    if let Err((code, message, details)) =
                        Self::check_send_permitted_in_db(db, req.chat_id, user_id)
                    {
                        let _ = reply_tx.send(WsServerFrame::Error {
                            code,
                            message: message.into(),
                            details: details.map(str::to_string),
                        });
                        return;
                    }
                }

                if let Some(pipeline) = maybe_pipeline {
                    let msg = MessageItem {
                        id: MessageId(0),
                        chat_id: req.chat_id,
                        peer_id: PeerId::User(user_id),
                        to_peer: PeerId::Chat(req.chat_id),
                        sender_id: user_id,
                        topic_id: req.topic_id,
                        flags: MessageFlags {
                            is_out: true,
                            is_silent: req.silent,
                            ..Default::default()
                        },
                        date: Utc::now(),
                        edit_date: None,
                        content: req.content.clone(),
                        reply_to: req.reply_to.map(ReplyHeader::new),
                        forward_info: None,
                        reactions: Vec::new(),
                        client_nonce: Some(req.client_nonce),
                        inline_keyboard: req.inline_keyboard.clone(),
                        ttl_seconds: None,
                        expires_at_ms: None,
                    };

                    match pipeline.process_incoming_message_strict(msg) {
                        Ok(pts_update) => {
                            let (msg_id, date_ms) = match &pts_update.event {
                                UpdateEvent::NewMessage(item) => (item.id.0, item.date.timestamp_millis()),
                                _ => (0, pts_update.timestamp_ms),
                            };
                            if let Some(ref db) = maybe_db {
                                let kb_json = req.inline_keyboard.as_ref().and_then(|k| serde_json::to_string(k).ok());
                                if kb_json.is_some() || req.scheduled_at_ms.is_some() {
                                    let _ = db.save_message_extras(req.chat_id, MsgId(msg_id), kb_json.as_deref(), req.scheduled_at_ms);
                                }
                            }
                            self.index_ws_message(req.chat_id, msg_id, user_id, date_ms, &req.content);
                            let _ = reply_tx.send(WsServerFrame::Ack {
                                correlation_id: 0,
                                client_nonce: req.client_nonce,
                                message_id: msg_id,
                                pts: pts_update.pts,
                            });
                            self.broadcast_to_chat_and_members(req.chat_id, Some(user_id), WsServerFrame::PtsPush(pts_update));
                        }
                        Err(e) => {
                            let _ = reply_tx.send(WsServerFrame::Error {
                                code: 500,
                                message: format!("Failed to process message: {e}"),
                                details: None,
                            });
                        }
                    }
                } else {
                    let msg_id = MessageId(self.next_msg_id.fetch_add(1, Ordering::SeqCst));
                    let pts = self.next_pts();
                    let now = Utc::now();
                    let message = MessageItem {
                        id: msg_id,
                        chat_id: req.chat_id,
                        peer_id: PeerId::User(user_id),
                        to_peer: PeerId::Chat(req.chat_id),
                        sender_id: user_id,
                        topic_id: req.topic_id,
                        flags: MessageFlags { is_out: true, ..Default::default() },
                        date: now,
                        edit_date: None,
                        content: req.content,
                        reply_to: req.reply_to.map(ReplyHeader::new),
                        forward_info: None,
                        reactions: Vec::new(),
                        client_nonce: Some(req.client_nonce),
                        inline_keyboard: req.inline_keyboard.clone(),
                        ttl_seconds: None,
                        expires_at_ms: None,
                    };
                    let _ = reply_tx.send(WsServerFrame::Ack {
                        correlation_id: 0,
                        client_nonce: req.client_nonce,
                        message_id: msg_id.0,
                        pts,
                    });
                    let pts_update = PtsUpdate::new(req.chat_id, pts, 1, UpdateEvent::NewMessage(message), now.timestamp_millis());
                    self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update.clone()));
                    self.broadcast_to_chat(req.chat_id.0, WsServerFrame::PtsPush(pts_update));
                }
            }
            WsClientFrame::EditMessage(req) => {
                if let MessageContent::Encrypted(ref env) = req.content {
                    if env.sender_user_id != user_id.0 {
                        let _ = reply_tx.send(WsServerFrame::Error {
                            code: 403,
                            message: "EncryptedEnvelope sender_user_id must match authenticated user".into(),
                            details: None,
                        });
                        return;
                    }
                }
                let maybe_pipeline = self.pipeline.read().clone();
                let maybe_db = self.resolve_db(maybe_pipeline.as_ref());
                if let Some(ref db) = maybe_db {
                    if let Err(code) = Self::is_member_in_db(db, req.chat_id, user_id) {
                        let _ = reply_tx.send(WsServerFrame::Error { code, message: "Not authorized for chat".into(), details: None });
                        return;
                    }
                    match db.get_message(req.chat_id, MsgId(req.message_id.0)) {
                        Ok(Some(rec)) if rec.deleted_at_ms.is_none() => {
                            if rec.sender_id != user_id && !Self::is_admin_in_db(db, req.chat_id, user_id) {
                                let _ = reply_tx.send(WsServerFrame::Error { code: 403, message: "Cannot edit another user's message".into(), details: None });
                                return;
                            }
                        }
                        Ok(_) => {
                            let _ = reply_tx.send(WsServerFrame::Error { code: 404, message: "Message not found".into(), details: None });
                            return;
                        }
                        Err(e) => {
                            let _ = reply_tx.send(WsServerFrame::Error { code: 500, message: e.to_string(), details: None });
                            return;
                        }
                    }
                }
                if let Some(pipeline) = maybe_pipeline {
                    match pipeline.edit_message(req.chat_id, MsgId(req.message_id.0), req.content.clone()) {
                        Ok(pts_update) => {
                            self.index_ws_message(req.chat_id, req.message_id.0, user_id, pts_update.timestamp_ms, &req.content);
                            self.broadcast_to_chat_and_members(req.chat_id, Some(user_id), WsServerFrame::PtsPush(pts_update));
                        }
                        Err(e) => {
                            let _ = reply_tx.send(WsServerFrame::Error { code: 500, message: e.to_string(), details: None });
                        }
                    }
                } else {
                    let pts = self.next_pts();
                    let now = Utc::now();
                    let message = MessageItem {
                        id: req.message_id, chat_id: req.chat_id, peer_id: PeerId::User(user_id),
                        to_peer: PeerId::Chat(req.chat_id), sender_id: user_id, topic_id: None,
                        flags: MessageFlags { is_out: true, is_edited: true, ..Default::default() },
                        date: now, edit_date: Some(now), content: req.content, reply_to: None,
                        forward_info: None, reactions: Vec::new(), client_nonce: None,
                        inline_keyboard: None,
                        ttl_seconds: None,
                        expires_at_ms: None,
                    };
                    let pts_update = PtsUpdate::new(req.chat_id, pts, 1, UpdateEvent::EditMessage(message), now.timestamp_millis());
                    self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update.clone()));
                    self.broadcast_to_chat(req.chat_id.0, WsServerFrame::PtsPush(pts_update));
                }
            }
            WsClientFrame::DeleteMessages(req) => {
                let maybe_pipeline = self.pipeline.read().clone();
                let maybe_db = self.resolve_db(maybe_pipeline.as_ref());
                if let Some(ref db) = maybe_db {
                    if let Err(code) = Self::is_member_in_db(db, req.chat_id, user_id) {
                        let _ = reply_tx.send(WsServerFrame::Error { code, message: "Not authorized for chat".into(), details: None });
                        return;
                    }
                    let is_admin = Self::is_admin_in_db(db, req.chat_id, user_id);
                    if !is_admin {
                        for mid in &req.message_ids {
                            if let Ok(Some(rec)) = db.get_message(req.chat_id, MsgId(mid.0)) {
                                if rec.sender_id != user_id {
                                    let _ = reply_tx.send(WsServerFrame::Error { code: 403, message: "Cannot delete another user's message".into(), details: None });
                                    return;
                                }
                            }
                        }
                    }
                }
                if let Some(pipeline) = maybe_pipeline {
                    let prev_pins = maybe_db
                        .as_ref()
                        .and_then(|db| db.list_pinned_messages(req.chat_id).ok())
                        .unwrap_or_default();
                    let removed_pinned = prev_pins
                        .iter()
                        .any(|pid| req.message_ids.iter().any(|mid| mid.0 == *pid));
                    let msg_ids: Vec<MsgId> = req.message_ids.iter().map(|m| MsgId(m.0)).collect();
                    match pipeline.delete_messages(req.chat_id, msg_ids, req.for_everyone) {
                        Ok(pts_update) => {
                            if let Some(ref indexer) = *self.search_indexer.read() {
                                for mid in &req.message_ids {
                                    indexer.delete_message_nonblocking(mid.0.to_string());
                                }
                                indexer.commit_nonblocking();
                            }
                            self.broadcast_to_chat_and_members(req.chat_id, Some(user_id), WsServerFrame::PtsPush(pts_update));
                            if removed_pinned {
                                if let Some(ref db) = maybe_db {
                                    let pinned_ids = db.list_pinned_messages(req.chat_id).unwrap_or_default();
                                    self.broadcast_to_chat_and_members(
                                        req.chat_id,
                                        None,
                                        WsServerFrame::Update(UpdateEvent::PinnedMessagesChanged {
                                            chat_id: req.chat_id,
                                            pinned_ids: pinned_ids.into_iter().map(MsgId).collect(),
                                        }),
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            let _ = reply_tx.send(WsServerFrame::Error { code: 500, message: e.to_string(), details: None });
                        }
                    }
                } else {
                    let pts = self.next_pts();
                    let tombstone = DeleteTombstone::new(PeerId::Chat(req.chat_id), req.message_ids, req.for_everyone);
                    let pts_update = PtsUpdate::new(req.chat_id, pts, 1, UpdateEvent::DeleteMessages(tombstone), Utc::now().timestamp_millis());
                    self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update.clone()));
                    self.broadcast_to_chat(req.chat_id.0, WsServerFrame::PtsPush(pts_update));
                }
            }
            WsClientFrame::SendReaction(req) => {
                if let Some(db) = self.db.read().clone() {
                    if let Err(code) = Self::is_member_in_db(&db, req.chat_id, user_id) {
                        let _ = reply_tx.send(WsServerFrame::Error { code, message: "Not authorized for chat".into(), details: None });
                        return;
                    }
                    let action = if req.add { "add" } else { "remove" };
                    match db.toggle_reaction_atomic(req.chat_id, MsgId(req.message_id.0), user_id, &req.emoji, action) {
                        Ok((pts, _details, counts)) => {
                            let ev = ReactionEvent::new(req.chat_id, req.message_id, user_id, counts);
                            let pts_update = PtsUpdate::new(req.chat_id, pts, 1, UpdateEvent::ReactionUpdated(ev), Utc::now().timestamp_millis());
                            self.broadcast_to_chat_and_members(req.chat_id, Some(user_id), WsServerFrame::PtsPush(pts_update));
                        }
                        Err(e) => {
                            let _ = reply_tx.send(WsServerFrame::Error { code: 400, message: e.to_string(), details: None });
                        }
                    }
                } else {
                    let pts = self.next_pts();
                    let rx_count = ReactionCount::new(req.emoji, if req.add { 1 } else { 0 }, req.add);
                    let ev = ReactionEvent::new(req.chat_id, req.message_id, user_id, vec![rx_count]);
                    let pts_update = PtsUpdate::new(req.chat_id, pts, 1, UpdateEvent::ReactionUpdated(ev), Utc::now().timestamp_millis());
                    self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update.clone()));
                    self.broadcast_to_chat(req.chat_id.0, WsServerFrame::PtsPush(pts_update));
                }
            }
            WsClientFrame::ReadHistory(req) => {
                if let Some(db) = self.db.read().clone() {
                    if let Err(code) = Self::is_member_in_db(&db, req.chat_id, user_id) {
                        let _ = reply_tx.send(WsServerFrame::Error { code, message: "Not authorized for chat".into(), details: None });
                        return;
                    }
                    match db.mark_chat_read_atomic(req.chat_id, user_id, MsgId(req.max_id.0), 0) {
                        Ok((inbox_max, _outbox_max, unread, pts, senders)) => {
                            let now_ms = Utc::now().timestamp_millis();
                            let reader_ev = ReadHistoryEvent::new(req.chat_id, MessageId(inbox_max.0), false, unread);
                            let pts_update = PtsUpdate::new(req.chat_id, pts, 1, UpdateEvent::ReadHistory(reader_ev), now_ms);
                            self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update));
                            let outbox_update = PtsUpdate::new(
                                req.chat_id,
                                pts,
                                0,
                                UpdateEvent::ReadHistoryOut { max_id: MessageId(inbox_max.0) },
                                now_ms,
                            );
                            for sender in senders {
                                self.broadcast_to_user(sender, WsServerFrame::PtsPush(outbox_update.clone()));
                            }
                        }
                        Err(e) => {
                            let _ = reply_tx.send(WsServerFrame::Error { code: 500, message: e.to_string(), details: None });
                        }
                    }
                } else {
                    let pts = self.next_pts();
                    let ev = ReadHistoryEvent::new(req.chat_id, req.max_id, false, 0);
                    let pts_update = PtsUpdate::new(req.chat_id, pts, 1, UpdateEvent::ReadHistory(ev), Utc::now().timestamp_millis());
                    self.broadcast_to_user(user_id, WsServerFrame::PtsPush(pts_update.clone()));
                    self.broadcast_to_chat(req.chat_id.0, WsServerFrame::PtsPush(pts_update));
                }
            }
            WsClientFrame::SendTyping(mut event) => {
                event.user_id = user_id;
                let chat_id = event.chat_id;
                let maybe_pipeline = self.pipeline.read().clone();
                if let Some(ref db) = self.resolve_db(maybe_pipeline.as_ref()) {
                    if Self::check_send_permitted_in_db(db, chat_id, user_id).is_err() {
                        return;
                    }
                }
                let frame = WsServerFrame::Update(UpdateEvent::UserTyping(event));
                self.broadcast_to_chat_and_members(chat_id, None, frame);
            }
            WsClientFrame::SubscribeChat { chat_id } => {
                if let Some(db) = self.db.read().clone() {
                    if let Err(code) = Self::is_member_in_db(&db, ChatId(chat_id), user_id) {
                        let _ = reply_tx.send(WsServerFrame::Error {
                            code,
                            message: "Not authorized for chat".into(),
                            details: None,
                        });
                        return;
                    }
                }
                self.subscribe_chat(chat_id, user_id);
            }
            WsClientFrame::UnsubscribeChat { chat_id } => {
                self.unsubscribe_chat(chat_id, user_id);
            }
            WsClientFrame::VotePoll { chat_id, message_id: _, poll_id, option_ids } => {
                if let Some(db) = self.db.read().clone() {
                    let Ok(poll_id_i64) = i64::try_from(poll_id) else {
                        let _ = reply_tx.send(WsServerFrame::Error {
                            code: 404,
                            message: "Poll not found".into(),
                            details: None,
                        });
                        return;
                    };
                    let poll_cid = match db.get_poll_for_user(poll_id_i64, user_id) {
                        Ok(Some((cid, _))) => cid,
                        Ok(None) => {
                            let _ = reply_tx.send(WsServerFrame::Error {
                                code: 404,
                                message: "Poll not found".into(),
                                details: None,
                            });
                            return;
                        }
                        Err(e) => {
                            let _ = reply_tx.send(WsServerFrame::Error {
                                code: 500,
                                message: e.to_string(),
                                details: None,
                            });
                            return;
                        }
                    };
                    if poll_cid != chat_id {
                        let _ = reply_tx.send(WsServerFrame::Error {
                            code: 403,
                            message: "Poll does not belong to specified chat".into(),
                            details: None,
                        });
                        return;
                    }
                    if let Err(code) = Self::is_member_in_db(&db, poll_cid, user_id) {
                        let _ = reply_tx.send(WsServerFrame::Error { code, message: "Not authorized for chat".into(), details: None });
                        return;
                    }
                    match db.vote_poll_atomic(poll_id_i64, user_id, &option_ids) {
                        Ok((cid, mid, pts, poll)) => {
                            let ev = UpdateEvent::PollUpdated { chat_id: cid, message_id: MessageId(mid.0), poll };
                            let pts_update = PtsUpdate::new(cid, pts, 1, ev, Utc::now().timestamp_millis());
                            self.broadcast_to_chat_and_members(cid, Some(user_id), WsServerFrame::PtsPush(pts_update));
                        }
                        Err(e) => {
                            let _ = reply_tx.send(WsServerFrame::Error { code: 400, message: e.to_string(), details: None });
                        }
                    }
                }
            }
            WsClientFrame::CallSignal {
                call_id,
                signal_type,
                target_user_id,
                sdp,
                candidate,
                sdp_mid,
                sdp_mline_index,
            } => {
                let db = self.db.read().clone();
                let sh = self.signaling_hub.read().clone();
                let resolved_peer = if let Some(ref d) = db {
                    match d.get_call(&call_id) {
                        Ok(Some(rec)) => {
                            if rec.caller_id != user_id.0 && rec.callee_id != user_id.0 {
                                let _ = reply_tx.send(WsServerFrame::Error {
                                    code: 403,
                                    message: "Not authorized for this call".into(),
                                    details: None,
                                });
                                return;
                            }
                            let expected_peer = if rec.caller_id == user_id.0 {
                                rec.callee_id
                            } else {
                                rec.caller_id
                            };
                            if let Some(tuid) = target_user_id {
                                if tuid != expected_peer {
                                    let _ = reply_tx.send(WsServerFrame::Error {
                                        code: 403,
                                        message: "Target user is not a participant of this call".into(),
                                        details: None,
                                    });
                                    return;
                                }
                            }
                            match d.apply_call_signal(
                                &call_id,
                                user_id.0,
                                &signal_type,
                                Utc::now().timestamp_millis(),
                            ) {
                                Ok(crate::db::social_store::CallSignalOutcome::Applied(_)) => {
                                    Some(UserId(expected_peer))
                                }
                                Ok(crate::db::social_store::CallSignalOutcome::NotFound) => {
                                    let _ = reply_tx.send(WsServerFrame::Error {
                                        code: 404,
                                        message: "Call not found".into(),
                                        details: None,
                                    });
                                    return;
                                }
                                Ok(crate::db::social_store::CallSignalOutcome::Forbidden(msg)) => {
                                    let _ = reply_tx.send(WsServerFrame::Error {
                                        code: 403,
                                        message: msg.into(),
                                        details: None,
                                    });
                                    return;
                                }
                                Ok(crate::db::social_store::CallSignalOutcome::Conflict { message, .. }) => {
                                    let _ = reply_tx.send(WsServerFrame::Error {
                                        code: 409,
                                        message: message.into(),
                                        details: None,
                                    });
                                    return;
                                }
                                Err(e) => {
                                    let _ = reply_tx.send(WsServerFrame::Error {
                                        code: 500,
                                        message: e.to_string(),
                                        details: None,
                                    });
                                    return;
                                }
                            }
                        }
                        Ok(None) => {
                            if signal_type == "invite" {
                                if sh.as_ref().and_then(|s| s.get_session(&call_id)).is_some() {
                                    let _ = reply_tx.send(WsServerFrame::Error {
                                        code: 409,
                                        message: "Call already exists".into(),
                                        details: None,
                                    });
                                    return;
                                }
                                match target_user_id {
                                    Some(tuid) if tuid != user_id.0 => {
                                        let peer_uid = UserId(tuid);
                                        if d.is_user_blocked(peer_uid, user_id).unwrap_or(false)
                                            || d.is_user_blocked(user_id, peer_uid).unwrap_or(false)
                                        {
                                            let _ = reply_tx.send(WsServerFrame::Error {
                                                code: 403,
                                                message: "Cannot initiate a call with a blocked user".into(),
                                                details: Some("user_is_blocked".into()),
                                            });
                                            return;
                                        }
                                        Some(peer_uid)
                                    }
                                    _ => {
                                        let _ = reply_tx.send(WsServerFrame::Error {
                                            code: 400,
                                            message: "Invalid target_user_id for call invite".into(),
                                            details: None,
                                        });
                                        return;
                                    }
                                }
                            } else if let Some(sess) = sh.as_ref().and_then(|s| s.get_session(&call_id)) {
                                match sess.peer_of(user_id) {
                                    Ok(peer) if target_user_id.is_none() || target_user_id == Some(peer.0) => {
                                        Some(peer)
                                    }
                                    _ => {
                                        let _ = reply_tx.send(WsServerFrame::Error {
                                            code: 403,
                                            message: "Not authorized for this call".into(),
                                            details: None,
                                        });
                                        return;
                                    }
                                }
                            } else {
                                let _ = reply_tx.send(WsServerFrame::Error {
                                    code: 404,
                                    message: "Call not found".into(),
                                    details: None,
                                });
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = reply_tx.send(WsServerFrame::Error {
                                code: 500,
                                message: e.to_string(),
                                details: None,
                            });
                            return;
                        }
                    }
                } else if signal_type == "invite" {
                    target_user_id.filter(|&tuid| tuid != user_id.0).map(UserId)
                } else if let Some(sess) = sh.as_ref().and_then(|s| s.get_session(&call_id)) {
                    match sess.peer_of(user_id) {
                        Ok(peer) if target_user_id.is_none() || target_user_id == Some(peer.0) => Some(peer),
                        _ => {
                            let _ = reply_tx.send(WsServerFrame::Error {
                                code: 403,
                                message: "Not authorized for this call".into(),
                                details: None,
                            });
                            return;
                        }
                    }
                } else {
                    None
                };
                let sig_req = CallSignalRequest {
                    call_id: call_id.clone(),
                    signal_type: signal_type.clone(),
                    sdp: sdp.clone(),
                    candidate: candidate.clone(),
                    sdp_mid: sdp_mid.clone(),
                    sdp_mline_index,
                    reason: None,
                };
                if let Some(sh_arc) = sh {
                    let sig_frame = match signal_type.as_str() {
                        "invite" => resolved_peer.map(|to_user| SignalingFrame::Invite {
                            call_id: call_id.clone(),
                            from_user: user_id,
                            to_user,
                            is_video: false,
                        }),
                        "ringing" => Some(SignalingFrame::Ringing { call_id: call_id.clone() }),
                        "accept" => Some(SignalingFrame::Accept { call_id: call_id.clone() }),
                        "offer" => sdp.map(|s| SignalingFrame::Offer { call_id: call_id.clone(), sdp: s }),
                        "answer" => sdp.map(|s| SignalingFrame::Answer { call_id: call_id.clone(), sdp: s }),
                        "ice_candidate" => candidate.map(|c| SignalingFrame::IceCandidate {
                            call_id: call_id.clone(),
                            candidate: c,
                            sdp_mid,
                            sdp_mline_index: sdp_mline_index.map(|v| v as u16),
                        }),
                        "hangup" => Some(SignalingFrame::Hangup {
                            call_id: call_id.clone(),
                            reason: "user_hangup".into(),
                        }),
                        _ => None,
                    };
                    if let Some(frame) = sig_frame {
                        if let Ok(handle) = tokio::runtime::Handle::try_current() {
                            handle.spawn(async move {
                                let _ = sh_arc.dispatch_frame(user_id, frame).await;
                            });
                        }
                    }
                }
                if let Some(peer) = resolved_peer {
                    let frame = WsServerFrame::Update(UpdateEvent::CallUpdated {
                        call_id,
                        state: signal_type,
                        signal: Some(sig_req),
                    });
                    self.broadcast_to_user(peer, frame);
                }
            }
            WsClientFrame::AckUpdates { chat_id: _, pts } => {
                let _ = reply_tx.send(WsServerFrame::Ack {
                    correlation_id: 0,
                    client_nonce: 0,
                    message_id: 0,
                    pts,
                });
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
    handle_ws_socket_with_session(
        socket,
        user_id,
        None,
        hub,
        heartbeat_interval_secs,
        inactivity_timeout_secs,
    )
    .await
}

/// Upgraded WebSocket connection handler bound to a specific session_id
pub async fn handle_ws_socket_with_session(
    socket: axum::extract::ws::WebSocket,
    user_id: UserId,
    session_id: Option<String>,
    hub: Arc<GatewayHub>,
    heartbeat_interval_secs: u64,
    inactivity_timeout_secs: u64,
) {
    let hub_weak = Arc::downgrade(&hub);
    drop(hub);
    handle_ws_socket_with_session_weak(
        socket,
        user_id,
        session_id,
        hub_weak,
        heartbeat_interval_secs,
        inactivity_timeout_secs,
    )
    .await
}

struct WsTaskCleanup {
    hub_weak: std::sync::Weak<GatewayHub>,
    conn_id: u64,
    forward_abort: tokio::task::AbortHandle,
    sig_abort: Option<tokio::task::AbortHandle>,
}

impl Drop for WsTaskCleanup {
    fn drop(&mut self) {
        self.forward_abort.abort();
        if let Some(ref sig) = self.sig_abort {
            sig.abort();
        }
        if let Some(hub) = self.hub_weak.upgrade() {
            hub.unregister_ws_tasks(self.conn_id);
        }
    }
}

/// Upgraded WebSocket connection handler using a `Weak<GatewayHub>` so idle WebSocket tasks
/// never pin `GatewayHub` or `TantivyIndexer` across server shutdown, exiting immediately on
/// `GatewayHub::shutdown()` or hub drop.
pub async fn handle_ws_socket_with_session_weak(
    socket: axum::extract::ws::WebSocket,
    user_id: UserId,
    session_id: Option<String>,
    hub_weak: std::sync::Weak<GatewayHub>,
    heartbeat_interval_secs: u64,
    inactivity_timeout_secs: u64,
) {
    use axum::extract::ws::Message;
    use futures::{SinkExt, StreamExt};
    use std::time::Duration;

    let Some(hub) = hub_weak.upgrade() else {
        return;
    };
    if hub.is_shutdown() {
        return;
    }
    let mut shutdown_rx = hub.subscribe_shutdown();
    if *shutdown_rx.borrow_and_update() {
        return;
    }

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsServerFrame>();
    let (close_tx, mut close_rx) = tokio::sync::oneshot::channel::<()>();

    hub.register_session(user_id, session_id, tx.clone());
    hub.broadcast_user_status(user_id, true);
    let sig_task = if let Some(sh) = hub.signaling_hub() {
        let (sig_tx, mut sig_rx) = mpsc::channel::<SignalingFrame>(64);
        sh.register_client(user_id, sig_tx);
        let tx_sig = tx.clone();
        let mut sig_shutdown_rx = hub.subscribe_shutdown();
        Some(tokio::spawn(async move {
            loop {
                if *sig_shutdown_rx.borrow_and_update() {
                    break;
                }
                tokio::select! {
                    _ = sig_shutdown_rx.changed() => {
                        break;
                    }
                    maybe_sf = sig_rx.recv() => {
                        let Some(sf) = maybe_sf else {
                            break;
                        };
                        let (call_id, state, signal) = match &sf {
                            SignalingFrame::Invite { call_id, .. } => (call_id.clone(), "ringing".into(), None),
                            SignalingFrame::Ringing { call_id } => (call_id.clone(), "ringing".into(), None),
                            SignalingFrame::Accept { call_id } => (call_id.clone(), "connected".into(), None),
                            SignalingFrame::Offer { call_id, sdp } => (
                                call_id.clone(),
                                "offer".into(),
                                Some(CallSignalRequest {
                                    call_id: call_id.clone(), signal_type: "offer".into(),
                                    sdp: Some(sdp.clone()), candidate: None, sdp_mid: None, sdp_mline_index: None, reason: None,
                                }),
                            ),
                            SignalingFrame::Answer { call_id, sdp } => (
                                call_id.clone(),
                                "answer".into(),
                                Some(CallSignalRequest {
                                    call_id: call_id.clone(), signal_type: "answer".into(),
                                    sdp: Some(sdp.clone()), candidate: None, sdp_mid: None, sdp_mline_index: None, reason: None,
                                }),
                            ),
                            SignalingFrame::IceCandidate { call_id, candidate, sdp_mid, sdp_mline_index } => (
                                call_id.clone(),
                                "ice_candidate".into(),
                                Some(CallSignalRequest {
                                    call_id: call_id.clone(), signal_type: "ice_candidate".into(),
                                    sdp: None, candidate: Some(candidate.clone()), sdp_mid: sdp_mid.clone(),
                                    sdp_mline_index: sdp_mline_index.map(|v| v as u32), reason: None,
                                }),
                            ),
                            SignalingFrame::Hangup { call_id, reason } => (
                                call_id.clone(),
                                "ended".into(),
                                Some(CallSignalRequest {
                                    call_id: call_id.clone(), signal_type: "hangup".into(),
                                    sdp: None, candidate: None, sdp_mid: None, sdp_mline_index: None, reason: Some(reason.clone()),
                                }),
                            ),
                        };
                        if tx_sig.send(WsServerFrame::Update(UpdateEvent::CallUpdated { call_id, state, signal })).is_err() {
                            break;
                        }
                    }
                }
            }
        }))
    } else {
        None
    };

    // Task forwarding outgoing frames to the WebSocket client
    let mut forward_shutdown_rx = hub.subscribe_shutdown();
    let forward_task = tokio::spawn(async move {
        let mut close_tx = Some(close_tx);
        loop {
            if *forward_shutdown_rx.borrow_and_update() {
                let _ = tokio::time::timeout(
                    Duration::from_millis(150),
                    ws_sender.send(Message::Close(None)),
                )
                .await;
                let _ = tokio::time::timeout(
                    Duration::from_millis(50),
                    ws_sender.close(),
                )
                .await;
                if let Some(tx_done) = close_tx.take() {
                    let _ = tx_done.send(());
                }
                break;
            }
            tokio::select! {
                _ = forward_shutdown_rx.changed() => {
                    let _ = tokio::time::timeout(
                        Duration::from_millis(150),
                        ws_sender.send(Message::Close(None)),
                    )
                    .await;
                    let _ = tokio::time::timeout(
                        Duration::from_millis(50),
                        ws_sender.close(),
                    )
                    .await;
                    if let Some(tx_done) = close_tx.take() {
                        let _ = tx_done.send(());
                    }
                    break;
                }
                maybe_frame = rx.recv() => {
                    let Some(frame) = maybe_frame else {
                        break;
                    };
                    let is_revoked_close = matches!(&frame, WsServerFrame::Error { code: 401, .. });
                    match serde_json::to_string(&frame) {
                        Ok(json) => {
                            if ws_sender.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                            if is_revoked_close {
                                let _ = ws_sender.send(Message::Close(None)).await;
                                if let Some(tx_done) = close_tx.take() {
                                    let _ = tx_done.send(());
                                }
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Failed to serialize WsServerFrame: {e}");
                        }
                    }
                }
            }
        }
    });

    let mut tracked_handles = vec![forward_task.abort_handle()];
    let sig_abort = sig_task.as_ref().map(|t| t.abort_handle());
    if let Some(ref sa) = sig_abort {
        tracked_handles.push(sa.clone());
    }
    let conn_id = hub.register_ws_tasks(tracked_handles);
    let cleanup_guard = WsTaskCleanup {
        hub_weak: hub_weak.clone(),
        conn_id,
        forward_abort: forward_task.abort_handle(),
        sig_abort,
    };

    // Drop strong Arc<GatewayHub> before entering the idle select loop.
    drop(hub);

    let tx_clone = tx.clone();
    let mut last_activity = tokio::time::Instant::now();
    let inactivity_limit = Duration::from_secs(inactivity_timeout_secs);
    let ping_interval = Duration::from_secs(heartbeat_interval_secs);
    let mut heartbeat_timer = tokio::time::interval(ping_interval);

    loop {
        if *shutdown_rx.borrow_and_update() {
            let _ = tokio::time::timeout(Duration::from_millis(200), &mut close_rx).await;
            break;
        }
        tokio::select! {
            _ = shutdown_rx.changed() => {
                let _ = tokio::time::timeout(Duration::from_millis(200), &mut close_rx).await;
                break;
            }
            _ = &mut close_rx => {
                break;
            }
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
                                let Some(hub) = hub_weak.upgrade() else {
                                    break;
                                };
                                if hub.is_shutdown() {
                                    break;
                                }
                                hub.handle_client_frame(user_id, client_frame, &tx_clone);
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

    drop(cleanup_guard);
    if let Some(hub) = hub_weak.upgrade() {
        hub.unregister_sender(user_id, &tx);
        if !hub.is_shutdown() && hub.user_connection_count(user_id) == 0 {
            hub.broadcast_user_status(user_id, false);
            if let Some(sh) = hub.signaling_hub() {
                sh.unregister_client(user_id);
            }
        }
    }
}
