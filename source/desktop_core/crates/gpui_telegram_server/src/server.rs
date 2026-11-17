use crate::auth::session_manager::{AuthError, SessionManager};
use crate::config::ServerConfig;
use crate::ws::{handle_ws_socket_with_session_weak, GatewayHub};
use axum::{
    body::Bytes,
    extract::{FromRequestParts, Path, Query, State, WebSocketUpgrade},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use chrono::Utc;
use sha2::{Digest, Sha256};
use gpui_telegram_core::auth::SessionToken;
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, TopicId, UserId};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

struct AppShutdownGuard {
    gateway_hub: std::sync::Weak<GatewayHub>,
}

impl Drop for AppShutdownGuard {
    fn drop(&mut self) {
        if let Some(hub) = self.gateway_hub.upgrade() {
            hub.shutdown();
        }
    }
}

/// Per-chat multi-agent coordination state (Floor Control, 5-hop Loop Budget, Shared Workspace).
#[derive(Debug, Default)]
pub struct GroupMultiAgentRoom {
    pub floor: gpui_telegram_agent::FloorControlQueue,
    pub loop_budget: gpui_telegram_agent::LoopBudget,
    pub workspace: gpui_telegram_agent::SharedGroupWorkspace,
}

impl GroupMultiAgentRoom {
    pub fn new(chat_id: ChatId) -> Self {
        Self {
            floor: gpui_telegram_agent::FloorControlQueue::new(30_000),
            loop_budget: gpui_telegram_agent::LoopBudget::new(gpui_telegram_agent::DEFAULT_MAX_LOOP_HOPS),
            workspace: gpui_telegram_agent::SharedGroupWorkspace::new(chat_id.0),
        }
    }
}

/// Global application state shared across HTTP and WebSocket handlers
#[derive(Clone)]
pub struct AppState {
    _shutdown_guard: Arc<AppShutdownGuard>,
    pub config: ServerConfig,
    pub db: Arc<crate::db::ServerDatabase>,
    pub cas: Arc<crate::media::cas_store::CasStore>,
    pub pts_engine: Arc<crate::sync::pts_engine::PtsEngine>,
    pub message_pipeline: Arc<crate::messaging::pipeline::MessagePipeline>,
    pub session_manager: Arc<SessionManager>,
    pub gateway_hub: Arc<GatewayHub>,
    pub search_indexer: Arc<crate::search::tantivy_indexer::TantivyIndexer>,
    pub hybrid_memory: Arc<crate::search::hybrid_memory::HybridMemoryEngine>,
    pub signaling_hub: Arc<crate::webrtc::signaling_hub::SignalingHub>,
    pub pairing_requests: Arc<RwLock<HashMap<String, (UserId, i64)>>>,
    pub passkey_challenges: Arc<RwLock<HashMap<String, i64>>>,
    pub alipay_upstream_attempts: Arc<RwLock<HashMap<String, String>>>,
    pub mcp_registry: Arc<RwLock<gpui_telegram_agent::McpRegistry>>,
    pub permission_dock: Arc<RwLock<gpui_telegram_agent::ToolPermissionDock>>,
    pub computer_lease_broker: Arc<RwLock<gpui_telegram_agent::ComputerLeaseBroker>>,
    pub talk_coordinator: Arc<RwLock<gpui_telegram_agent::TalkCoordinator>>,
    pub acp_runner: Arc<gpui_telegram_agent::acp::AcpRunner>,
    pub hermes: Arc<gpui_telegram_agent::hermes::HermesEngine>,
    pub fabushi_bots: Arc<gpui_telegram_agent::fabushi_bot::FabushiBotHost>,
    pub self_weak: Arc<std::sync::OnceLock<std::sync::Weak<AppState>>>,
    pub computers: Arc<crate::routes::computers::ComputerControlService>,
    pub tailnet: Arc<crate::routes::tailnet::TailnetCoordService>,
    pub task_registry: Arc<tokio::sync::RwLock<HashMap<String, gpui_telegram_agent::tasks::TaskFsm>>>,
    pub approval_gates: Arc<tokio::sync::RwLock<HashMap<String, gpui_telegram_agent::tasks::ApprovalGate>>>,
    pub action_registry: Arc<gpui_telegram_core::action_bus::ActionRegistry>,
    pub event_bus: Arc<dyn crate::bus::CrossNodeEventBus>,
    pub push_hub: Arc<crate::push::PushGatewayHub>,
    pub session_store: Arc<crate::auth::session_store::PersistentSessionStore>,
    pub action_approval_secret: [u8; 32],
    pub multi_agent_groups: Arc<RwLock<HashMap<ChatId, GroupMultiAgentRoom>>>,
    pub agent_capability_key: Arc<ed25519_dalek::SigningKey>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(ServerConfig::default())
    }
}

impl AppState {
    pub fn new(config: ServerConfig) -> Self {
        let db = Arc::new(Self::open_database(&config));
        Self::with_db(config, db)
    }

    pub fn try_new(config: ServerConfig) -> Result<Self, String> {
        let db = if config.is_in_memory() {
            crate::db::ServerDatabase::open_in_memory().map_err(|e| e.to_string())?
        } else {
            let path = config.resolve_db_path();
            if path.trim().is_empty() {
                return Err("Empty SQLite database path is not permitted in file-backed mode".into());
            }
            crate::db::ServerDatabase::open(&path)
                .map_err(|e| format!("Failed to open SQLite database at '{}': {}", path, e))?
        };
        Ok(Self::with_db(config, Arc::new(db)))
    }

    pub fn with_acp_runner(mut self, runner: gpui_telegram_agent::acp::AcpRunner) -> Self {
        self.acp_runner = Arc::new(runner);
        self
    }

    pub fn with_db(config: ServerConfig, db: Arc<crate::db::ServerDatabase>) -> Self {
        let session_manager = Arc::new(SessionManager::new());
        let gateway_hub = Arc::new(GatewayHub::new());

        // Monotonically align human user_id with SQLite database (keep bots >= 500_000 separate)
        if let Ok(max_id) = db.get_max_human_user_id() {
            session_manager.set_next_user_id(std::cmp::max(100_001, max_id + 1));
        }

        // Wire disconnection signal from session manager to gateway hub (weak ref to avoid Arc cycles)
        let hub_weak = Arc::downgrade(&gateway_hub);
        session_manager.set_disconnect_notifier(Arc::new(move |user_id| {
            if let Some(hub) = hub_weak.upgrade() {
                hub.disconnect_user(user_id);
            }
        }));

        // Evict in-memory session cache when cluster-wide SessionRevoked arrives (weak ref to avoid Arc cycles)
        let sm_revoke = Arc::downgrade(&session_manager);
        gateway_hub.set_session_revoked_handler(Arc::new(move |_user_id, session_id| {
            if let Some(sm) = sm_revoke.upgrade() {
                sm.revoke_session(session_id);
            }
        }));

        let workspace_root = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let hermes_data_dir = if config.is_in_memory() {
            None
        } else {
            let db_path = std::path::PathBuf::from(config.resolve_db_path());
            let parent = db_path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            Some(parent.join("hermes"))
        };
        let hermes_cfg = gpui_telegram_agent::hermes::HermesEngineConfig {
            workspace_root,
            data_dir: hermes_data_dir,
            default_model: config.default_agent_model.clone(),
            ..Default::default()
        };
        let hermes = Arc::new(
            gpui_telegram_agent::hermes::HermesEngine::new(hermes_cfg).unwrap_or_else(|err| {
                tracing::warn!("Falling back to in-memory HermesEngine: {}", err);
                gpui_telegram_agent::hermes::HermesEngine::in_memory()
                    .expect("Failed to initialize in-memory HermesEngine")
            }),
        );
        let computers = Arc::new(crate::routes::computers::ComputerControlService::new(
            db.clone(),
        ));
        let tailnet = Arc::new(crate::routes::tailnet::TailnetCoordService::new(
            db.clone(),
        ));
        computers.set_tailnet_service(tailnet.clone());
        hermes.set_computer_provider(computers.clone());

        let fabushi_bots = if config.is_in_memory() {
            Arc::new(
                gpui_telegram_agent::fabushi_bot::FabushiBotHost::new_ephemeral(hermes.clone())
                    .expect("Failed to initialize ephemeral FabushiBotHost"),
            )
        } else {
            let db_path = std::path::PathBuf::from(config.resolve_db_path());
            let parent = db_path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            let agents_dir = parent.join("fabushi_agents");
            Arc::new(
                gpui_telegram_agent::fabushi_bot::FabushiBotHost::new_with_dir(
                    agents_dir,
                    hermes.clone(),
                )
                .expect("Failed to initialize FabushiBotHost"),
            )
        };
        fabushi_bots.set_computer_provider(computers.clone());
        let self_weak: Arc<std::sync::OnceLock<std::sync::Weak<AppState>>> =
            Arc::new(std::sync::OnceLock::new());

        let provider_snap = hermes.providers().snapshot();
        let active_api_mode = provider_snap
            .providers
            .get(&provider_snap.active_provider_id)
            .map(|p| p.api_mode.as_str())
            .unwrap_or("local_hermes");
        tracing::info!(
            active_provider = %provider_snap.active_provider_id,
            default_model = %provider_snap.active_model,
            api_mode = %active_api_mode,
            allow_local_fallback = provider_snap.allow_local_fallback,
            configured_providers = provider_snap.providers.len(),
            "Hermes provider router initialized"
        );

        let acp_runner = Arc::new(
            gpui_telegram_agent::acp::AcpRunner::new(&config.default_agent_model)
                .with_hermes_engine(hermes.clone()),
        );

        let mut hydrated_tasks = HashMap::new();
        if let Ok(rows) = db.load_acp_tasks() {
            for (task_id, val) in rows {
                if let Ok(task) = serde_json::from_value::<gpui_telegram_agent::tasks::TaskFsm>(val) {
                    hydrated_tasks.insert(task_id, task);
                }
            }
        }
        let mut hydrated_gates = HashMap::new();
        if let Ok(rows) = db.load_approval_gates() {
            for (gate_id, val) in rows {
                if let Ok(gate) = serde_json::from_value::<gpui_telegram_agent::tasks::ApprovalGate>(val) {
                    hydrated_gates.insert(gate_id, gate);
                }
            }
        }
        let task_registry = Arc::new(tokio::sync::RwLock::new(hydrated_tasks));
        let approval_gates = Arc::new(tokio::sync::RwLock::new(hydrated_gates));

        let mut hydrated_mcp = gpui_telegram_agent::McpRegistry::new();
        if let Ok(rows) = db.load_mcp_servers() {
            for (_sid, _name, _ep, _st, tools_val) in rows {
                if let Ok(defs) = serde_json::from_value::<Vec<gpui_telegram_agent::McpToolDefinition>>(tools_val) {
                    for def in defs {
                        hydrated_mcp.register(def);
                    }
                }
            }
        }

        let cas = if config.is_in_memory() && config.media_storage_path == "data/media" {
            Arc::new(
                crate::media::cas_store::CasStore::temp("inmem")
                    .expect("Failed to initialize isolated temporary CAS directory"),
            )
        } else {
            Arc::new(crate::media::cas_store::CasStore::new(&config.media_storage_path))
        };

        let search_indexer = if config.is_in_memory() {
            Arc::new(
                crate::search::tantivy_indexer::TantivyIndexer::in_ram()
                    .expect("Failed to initialize in-memory Tantivy index"),
            )
        } else {
            let idx_path = std::path::PathBuf::from(config.resolve_db_path()).with_extension("tantivy");
            Arc::new(
                crate::search::tantivy_indexer::TantivyIndexer::open_or_create(&idx_path)
                    .unwrap_or_else(|e| {
                        panic!("Failed to open Tantivy search index at '{}': {}", idx_path.display(), e)
                    }),
            )
        };
        if search_indexer.doc_count() == 0 {
            if let Ok(recent) = db.list_recent_messages_for_search(2000) {
                if !recent.is_empty() {
                    let docs: Vec<crate::search::schema::SearchDocument> = recent
                        .into_iter()
                        .map(|(mid, cid, sid, date_ms, text, media_type)| {
                            crate::search::schema::SearchDocument::new(
                                mid.to_string(),
                                cid.to_string(),
                                sid.to_string(),
                                date_ms,
                                text,
                                media_type,
                            )
                        })
                        .collect();
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        let indexer_weak = Arc::downgrade(&search_indexer);
                        handle.spawn(async move {
                            for doc in docs {
                                let Some(idx) = indexer_weak.upgrade() else {
                                    return;
                                };
                                if idx.index_message(doc).await.is_err() {
                                    return;
                                }
                            }
                            if let Some(idx) = indexer_weak.upgrade() {
                                let _ = idx.commit().await;
                            }
                        });
                    } else {
                        for doc in docs {
                            let _ = search_indexer.index_message_sync(doc);
                        }
                        search_indexer.commit_nonblocking();
                    }
                }
            }
        }
        if let Err(e) = db.pool().with_conn(|conn| {
            fabushi_miniapp::storage::init_fabushi_tables(conn)
                .map_err(|err| gpui_telegram_core::error::TelegramError::Database(err.to_string()))
        }) {
            tracing::warn!(error = %e, "Failed to initialize fabushi_miniapp tables");
        }
        let _ = db.seed_default_catalog_if_empty();
        let _ = db.seed_botfather_and_flagship_apps();
        let _ = ensure_default_ai_bot(&db);

        let signaling_hub = Arc::new(crate::webrtc::signaling_hub::SignalingHub::new());
        let pts_engine = Arc::new(crate::sync::pts_engine::PtsEngine::new(db.clone()));
        let dedup = crate::messaging::dedup::MessageNonceDedup::default();
        let message_pipeline = Arc::new(crate::messaging::pipeline::MessagePipeline::new(
            dedup,
            (*pts_engine).clone(),
        ));
        let hybrid_memory = Arc::new(
            crate::search::hybrid_memory::HybridMemoryEngine::with_db_and_tantivy(
                db.clone(),
                Arc::downgrade(&search_indexer),
            ),
        );
        let im_bridge = Arc::new(crate::hermes_im_bridge::ServerPlatformImBridge::new(
            db.clone(),
            message_pipeline.clone(),
            pts_engine.clone(),
            Arc::downgrade(&gateway_hub),
            Arc::downgrade(&search_indexer),
            Arc::downgrade(&hybrid_memory),
        ));
        hermes.set_platform_im_bridge(im_bridge.clone());
        hermes.set_agentic_backend(db.clone());
        fabushi_bots.set_platform_im_bridge(im_bridge);
        fabushi_bots.set_agentic_backend(db.clone());
        gateway_hub.set_pipeline(message_pipeline.clone(), db.clone());
        gateway_hub.set_search_indexer(search_indexer.clone());
        gateway_hub.set_signaling_hub(signaling_hub.clone());

        let teammate_bridge = Arc::new(ServerFabushiTeammateBridge {
            db: db.clone(),
            fabushi_bots: Arc::downgrade(&fabushi_bots),
            state_weak: self_weak.clone(),
        });
        fabushi_bots.set_teammate_handler(teammate_bridge);
        hydrate_all_fabushi_bots(&db, &fabushi_bots);

        if let Ok(rt_handle) = tokio::runtime::Handle::try_current() {
            let hermes_weak = Arc::downgrade(&hermes);
            let db_weak = Arc::downgrade(&db);
            let pipeline_weak = Arc::downgrade(&message_pipeline);
            let hub_weak = Arc::downgrade(&gateway_hub);
            let search_weak = Arc::downgrade(&search_indexer);
            rt_handle.spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    let Some(hermes) = hermes_weak.upgrade() else { return; };
                    let Some(db) = db_weak.upgrade() else { return; };
                    let Some(pipeline) = pipeline_weak.upgrade() else { return; };
                    let Some(hub) = hub_weak.upgrade() else { return; };
                    if let Ok(purged) = db.purge_expired_ttl_messages(chrono::Utc::now().timestamp_millis()) {
                        if !purged.is_empty() {
                            if let Some(idx) = search_weak.upgrade() {
                                for (cid, mids) in &purged {
                                    for mid in mids {
                                        idx.delete_chat_message_nonblocking(
                                            cid.0.to_string(),
                                            mid.0.to_string(),
                                        );
                                    }
                                }
                                let _ = idx.commit().await;
                            }
                            for (cid, mids) in purged {
                                let tombstone = gpui_telegram_proto::message::DeleteTombstone::new(
                                    gpui_telegram_core::types::PeerId::Chat(cid),
                                    mids,
                                    true,
                                );
                                hub.broadcast_to_chat_and_members(
                                    cid,
                                    None,
                                    gpui_telegram_proto::ws::WsServerFrame::Update(
                                        gpui_telegram_proto::sync::UpdateEvent::DeleteMessages(tombstone),
                                    ),
                                );
                            }
                        }
                    }

                    if let Ok(outcomes) = hermes.run_due_cron_jobs().await {
                        for out in outcomes {
                            if out.status == "completed" {
                                if let Some(cid) = out.target_chat_id {
                                    let bot_hint = hermes
                                        .store()
                                        .get_cron_job(&out.job_id)
                                        .ok()
                                        .flatten()
                                        .and_then(|j| j.deliver_to);
                                    deliver_proactive_message_raw(
                                        &db,
                                        &pipeline,
                                        &hub,
                                        ChatId(cid),
                                        bot_hint.as_deref(),
                                        &out.response_text,
                                    );
                                }
                            }
                        }
                    }

                    if let Ok(Some(hb)) = hermes.tick_heartbeat_if_due().await {
                        if !hb.suppressed {
                            if let Some(cid) = hb.target_chat_id {
                                let bot_hint = hermes.store().kv_get("heartbeat", "bot_id").ok().flatten();
                                deliver_proactive_message_raw(
                                    &db,
                                    &pipeline,
                                    &hub,
                                    ChatId(cid),
                                    bot_hint.as_deref(),
                                    &hb.response_text,
                                );
                            }
                        }
                    }
                }
            });
        }

        let action_registry = Arc::new(gpui_telegram_core::action_bus::ActionRegistry::new());
        let _ = action_registry.register_typed(crate::actions::ServerSendMessageAction::new(
            db.clone(),
            message_pipeline.clone(),
            gateway_hub.clone(),
            search_indexer.clone(),
        ));
        let _ = action_registry.register_typed(crate::actions::ServerGetChatHistoryAction::new(
            db.clone(),
        ));
        let _ = action_registry.register_typed(crate::actions::ServerListDialogsAction::new(
            db.clone(),
        ));
        let _ = action_registry.register_typed(crate::actions::ServerDeleteMessageAction::new(
            db.clone(),
            message_pipeline.clone(),
            gateway_hub.clone(),
            search_indexer.clone(),
        ));

        let node_id = format!("node_{}", uuid::Uuid::new_v4().simple());
        let event_bus: Arc<dyn crate::bus::CrossNodeEventBus> =
            Arc::new(crate::bus::InProcessEventBus::new(node_id));
        gateway_hub.set_event_bus(event_bus.clone());

        let push_hub = Arc::new(crate::push::PushGatewayHub::new(db.clone()));
        push_hub.register_dispatcher(Arc::new(crate::push::dispatchers::MockPushDispatcher::new()));
        push_hub.register_dispatcher(Arc::new(crate::push::dispatchers::ApnsDispatcher::new(
            "org.telegram.GPUI",
        )));
        push_hub.register_dispatcher(Arc::new(crate::push::dispatchers::WebPushDispatcher::new()));
        push_hub.register_dispatcher(Arc::new(crate::push::dispatchers::FcmDispatcher::new(
            "gpui-telegram",
        )));

        let session_store = Arc::new(crate::auth::session_store::PersistentSessionStore::new(
            db.pool().clone(),
        ));
        session_store.set_event_bus(event_bus.clone());

        let mut action_approval_secret = [0u8; 32];
        let u1 = Uuid::new_v4();
        let u2 = Uuid::new_v4();
        action_approval_secret[..16].copy_from_slice(u1.as_bytes());
        action_approval_secret[16..].copy_from_slice(u2.as_bytes());

        let agent_capability_key = Arc::new(ed25519_dalek::SigningKey::from_bytes(&action_approval_secret));

        let _shutdown_guard = Arc::new(AppShutdownGuard {
            gateway_hub: Arc::downgrade(&gateway_hub),
        });
        Self {
            _shutdown_guard,
            config,
            db,
            cas,
            pts_engine,
            message_pipeline,
            session_manager,
            gateway_hub,
            search_indexer,
            hybrid_memory,
            signaling_hub,
            pairing_requests: Arc::new(RwLock::new(HashMap::new())),
            passkey_challenges: Arc::new(RwLock::new(HashMap::new())),
            alipay_upstream_attempts: Arc::new(RwLock::new(HashMap::new())),
            mcp_registry: Arc::new(RwLock::new(hydrated_mcp)),
            permission_dock: Arc::new(RwLock::new(gpui_telegram_agent::ToolPermissionDock::new())),
            computer_lease_broker: Arc::new(RwLock::new(gpui_telegram_agent::ComputerLeaseBroker::new())),
            talk_coordinator: Arc::new(RwLock::new(gpui_telegram_agent::TalkCoordinator::new())),
            acp_runner,
            hermes,
            fabushi_bots,
            self_weak,
            computers,
            tailnet,
            task_registry,
            approval_gates,
            action_registry,
            event_bus,
            push_hub,
            session_store,
            action_approval_secret,
            multi_agent_groups: Arc::new(RwLock::new(HashMap::new())),
            agent_capability_key,
        }
    }

    pub fn set_event_bus(&mut self, bus: Arc<dyn crate::bus::CrossNodeEventBus>) {
        self.event_bus = bus.clone();
        self.gateway_hub.set_event_bus(bus.clone());
        self.session_store.set_event_bus(bus);
    }

    pub fn with_event_bus(mut self, bus: Arc<dyn crate::bus::CrossNodeEventBus>) -> Self {
        self.set_event_bus(bus);
        self
    }

    pub fn shutdown(&self) {
        self.gateway_hub.shutdown();
    }

    fn open_database(config: &ServerConfig) -> crate::db::ServerDatabase {
        if config.is_in_memory() {
            crate::db::ServerDatabase::open_in_memory().expect("Failed to initialize in-memory SQLite")
        } else {
            let path = config.resolve_db_path();
            assert!(
                !path.trim().is_empty(),
                "SQLite database path cannot be empty in file-backed mode"
            );
            crate::db::ServerDatabase::open(&path).unwrap_or_else(|e| {
                panic!("Failed to open SQLite database at '{}': {}", path, e);
            })
        }
    }

    pub fn with_runtime_instances(
        config: ServerConfig,
        acp_runner: Arc<gpui_telegram_agent::acp::AcpRunner>,
        task_registry: Arc<tokio::sync::RwLock<HashMap<String, gpui_telegram_agent::tasks::TaskFsm>>>,
        approval_gates: Arc<tokio::sync::RwLock<HashMap<String, gpui_telegram_agent::tasks::ApprovalGate>>>,
    ) -> Self {
        let mut state = Self::new(config);
        state.acp_runner = acp_runner;
        state.task_registry = task_registry;
        state.approval_gates = approval_gates;
        state
    }
}

/// Zero-cost AuthBearer extractor for authenticated routes
#[derive(Debug, Clone)]
pub struct AuthBearer {
    pub session: SessionToken,
    pub user_id: UserId,
}

impl FromRequestParts<Arc<AppState>> for AuthBearer {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_token(parts).ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "error": "missing_token",
                    "message": "Authorization Bearer token or ?token= query parameter is required"
                })),
            )
        })?;

        let session = match state.session_store.verify_token_with_status(&token) {
            Ok(crate::auth::session_store::TokenVerifyOutcome::Valid(rec)) => {
                let created_at = chrono::DateTime::from_timestamp_millis(rec.created_at_ms)
                    .unwrap_or_else(chrono::Utc::now);
                let expires_at = chrono::DateTime::from_timestamp_millis(rec.expires_at_ms)
                    .unwrap_or_else(chrono::Utc::now);
                let secret = token.split_once('.').map(|(_, s)| s).unwrap_or("");
                SessionToken::new_with_hash(
                    rec.session_id,
                    UserId(rec.user_id),
                    created_at,
                    expires_at,
                    &rec.device_id,
                    SessionToken::hash_token(secret),
                )
            }
            Ok(crate::auth::session_store::TokenVerifyOutcome::Rejected) => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "error": "invalid_token",
                        "message": "Session token is invalid, expired, or revoked"
                    })),
                ));
            }
            Ok(crate::auth::session_store::TokenVerifyOutcome::NotFound) => {
                state
                    .session_manager
                    .verify_token(&token)
                    .ok_or_else(|| {
                        (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({
                                "error": "invalid_token",
                                "message": "Session token is invalid, expired, or revoked"
                            })),
                        )
                    })?
            }
            Err(_) => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "error": "invalid_token",
                        "message": "Session token is invalid, expired, or revoked"
                    })),
                ));
            }
        };

        let user_id = session.user_id;
        if let Ok(None) = state.db.get_user(user_id) {
            let _ = sync_mem_user_to_db(state, user_id);
        }
        Ok(AuthBearer { session, user_id })
    }
}

#[derive(Debug, Clone)]
pub struct OptionalAuthBearer(pub Option<AuthBearer>);

impl FromRequestParts<Arc<AppState>> for OptionalAuthBearer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(AuthBearer::from_request_parts(parts, state).await.ok()))
    }
}

pub(crate) fn sync_mem_user_to_db(
    state: &AppState,
    user_id: UserId,
) -> Option<crate::db::schema::UserRecord> {
    let mem_u = state.session_manager.get_user(user_id)?;
    let ts_ms = mem_u.created_at.timestamp_millis();
    let rec = crate::db::schema::UserRecord {
        user_id,
        username: Some(mem_u.username),
        phone: None,
        first_name: mem_u.first_name,
        last_name: mem_u.last_name,
        bio: None,
        password_hash: Some(mem_u.password_hash),
        is_bot: false,
        is_verified: false,
        is_support: false,
        is_scam: false,
        is_fake: false,
        profile_photo_file_id: None,
        account_generation: 1,
        created_at_ms: ts_ms,
        updated_at_ms: ts_ms,
    };
    let _ = state.db.create_user(&rec);
    state.db.get_user(user_id).ok().flatten().or(Some(rec))
}

/// Extract token from Authorization header or ?token= query parameter
fn extract_token(parts: &Parts) -> Option<String> {
    // 1. Try Authorization: Bearer <token>
    if let Some(auth_val) = parts.headers.get(header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_val.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                let trimmed = token.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }

    // 2. Try query parameter ?token=<token>
    if let Some(query) = parts.uri.query() {
        for param in query.split('&') {
            if let Some(token) = param.strip_prefix("token=") {
                let trimmed = token.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }

    None
}

/// Build the production Axum router with default in-memory state
pub fn build_router() -> Router {
    let state = Arc::new(AppState::default());
    build_router_with_state(state)
}

/// Build the production Axum router mounting all public, protected, and WebSocket routes
pub fn build_router_with_state(state: Arc<AppState>) -> Router {
    use crate::routes;

    let _ = state.self_weak.set(Arc::downgrade(&state));

    Router::new()
        // --- Public Routes ---
        .route("/health", get(health_check))
        .route("/api/v1/status", get(server_status))
        .route("/api/v1/auth/register", post(auth_register))
        .route("/api/v1/auth/login", post(auth_login))
        .route("/api/v1/auth/phone/send-code", post(routes::auth_factors::phone_send_code))
        .route("/api/v1/auth/phone/verify", post(routes::auth_factors::phone_verify))
        .route("/api/v1/auth/2fa", get(routes::auth_factors::two_factor_status).post(routes::auth_factors::two_factor_set).delete(routes::auth_factors::two_factor_disable))
        .route("/api/v1/auth/2fa/status", get(routes::auth_factors::two_factor_status))
        .route("/api/v1/auth/2fa/set", post(routes::auth_factors::two_factor_set))
        .route("/api/v1/auth/2fa/disable", post(routes::auth_factors::two_factor_disable).delete(routes::auth_factors::two_factor_disable))
        .route("/api/v1/auth/2fa/verify", post(routes::auth_factors::two_factor_verify))
        // --- Auth Extension Routes (Email & Alipay) ---
        .route("/api/v1/auth/email/send-code", post(auth_email_send_code))
        .route("/api/v1/auth/email/register", post(auth_email_register))
        .route("/api/v1/auth/alipay/start", post(auth_alipay_start))
        .route("/api/v1/auth/alipay/poll/{attempt_id}", get(auth_alipay_poll))
        .route("/api/v1/auth/alipay/callback", post(auth_alipay_callback).get(auth_alipay_callback))
        .route("/api/v1/auth/alipay/simulate-scan", post(auth_alipay_simulate_scan))
        .route("/api/v1/auth/alipay/portal", get(auth_alipay_portal))
        .route("/api/v1/auth/alipay/login", post(auth_alipay_direct_login))
        .route("/api/v1/auth/alipay/sdk-login", post(auth_alipay_direct_login))
        .route("/api/v1/auth/alipay/register", post(auth_alipay_register))
        .route("/api/v1/auth/alipay/auth-string", get(auth_alipay_auth_string))
        // --- Migrated Legacy & CLI Alipay Auth Routes ---
        .route("/api/auth/alipay/login-url", get(auth_alipay_login_url))
        .route("/api/auth/alipay/cli-session", get(auth_alipay_cli_session))
        .route("/api/auth/alipay/callback", post(auth_alipay_callback).get(auth_alipay_callback))
        .route("/api/auth/alipay/macos-callback", post(auth_alipay_callback).get(auth_alipay_callback))
        .route("/api/auth/alipay/mobile-callback", post(auth_alipay_callback).get(auth_alipay_callback))
        .route("/api/auth/alipay/login", post(auth_alipay_direct_login))
        .route("/api/auth/alipay/sdk-login", post(auth_alipay_direct_login))
        .route("/api/auth/alipay/register", post(auth_alipay_register))
        .route("/api/auth/alipay/auth-string", get(auth_alipay_auth_string))
        // --- Passkey & QR Pair Routes ---
        .route("/api/v1/auth/passkey/challenge", post(routes::auth_advanced::passkey_challenge))
        .route("/api/v1/auth/passkey/register", post(routes::auth_advanced::passkey_register))
        .route("/api/v1/auth/passkey/verify", post(routes::auth_advanced::passkey_verify))
        .route("/api/v1/auth/passkey/list", get(routes::auth_advanced::passkey_list))
        .route("/api/v1/auth/pair/init", post(routes::auth_advanced::pair_init))
        .route("/api/v1/auth/pair/accept", post(routes::auth_advanced::pair_accept))
        .route("/api/v1/auth/pair/poll/{token}", get(routes::auth_advanced::pair_poll))
        .route("/api/v1/auth/pair/verify", post(routes::auth_advanced::pair_verify))
        // --- Protected Profile, Peer Resolve & Sessions ---
        .route("/api/v1/auth/me", get(routes::users::get_me_profile))
        .route("/api/v1/auth/profile", put(routes::users::update_me_profile))
        .route("/api/v1/users/me", get(routes::users::get_me_profile).put(routes::users::update_me_profile).patch(routes::users::update_me_profile))
        .route("/api/v1/users/me/settings", get(routes::users::get_my_settings).put(routes::users::put_my_settings).patch(routes::users::put_my_settings))
        .route("/api/v1/users/blocked", get(routes::users::blocked_users_list))
        .route("/api/v1/users/{user_id}/block", post(routes::users::block_user_handler).delete(routes::users::unblock_user_handler))
        .route("/api/v1/users/{user_id}/unblock", post(routes::users::unblock_user_handler))
        .route("/api/v1/users/{user_id}", get(routes::users::get_user_by_id))
        .route("/api/v1/peers/resolve", get(routes::users::resolve_peer))
        .route("/api/v1/auth/sessions", get(routes::auth_advanced::auth_sessions_list))
        .route("/api/v1/auth/sessions/others", delete(routes::auth_advanced::auth_revoke_other_sessions))
        .route("/api/v1/auth/sessions/{session_id}", delete(routes::auth_advanced::auth_delete_session))
        // --- Protected Messaging, Reactions, Polls & Read Cursors ---
        .route("/api/v1/messages/history", get(routes::messages::messages_history_clean))
        .route("/api/v1/messages/send", post(routes::messages::messages_send_clean))
        .route("/api/v1/messages/edit", post(routes::sync_polls::messages_edit_legacy))
        .route("/api/v1/messages/delete", post(routes::sync_polls::messages_delete_legacy))
        .route("/api/v1/messages/reaction", post(routes::sync_polls::messages_reaction_legacy))
        .route("/api/v1/messages/read", post(routes::sync_polls::messages_read_legacy))
        .route("/api/v1/chats/{chat_id}/messages", get(routes::messages::chats_messages_history).post(routes::messages::chats_messages_send).delete(routes::chat_extras::chat_clear_history))
        .route("/api/v1/chats/{chat_id}/clear", post(routes::chat_extras::chat_clear_history))
        .route("/api/v1/chats/{chat_id}/leave", post(routes::chat_extras::chat_leave_or_delete))
        .route("/api/v1/chats/{chat_id}/ttl", post(routes::chat_extras::chat_set_ttl))
        .route("/api/v1/chats/{chat_id}/messages/{msg_id}", put(routes::sync_polls::messages_edit_path).delete(routes::sync_polls::messages_delete_path))
        .route("/api/v1/chats/{chat_id}/messages/{msg_id}/reactions", post(routes::sync_polls::messages_reaction_path))
        .route("/api/v1/chats/{chat_id}/read", post(routes::sync_polls::chats_read_path))
        .route("/api/v1/chats/{chat_id}/polls", post(routes::sync_polls::polls_create))
        .route("/api/v1/polls/{poll_id}", get(routes::sync_polls::polls_get))
        .route("/api/v1/polls/{poll_id}/vote", post(routes::sync_polls::polls_vote))
        .route("/api/v1/polls/{poll_id}/close", post(routes::sync_polls::polls_close))
        // --- Protected Contacts, Chats, Topics, Folders, Pins, Drafts ---
        .route("/api/v1/contacts", get(routes::users::contacts_list_all).post(routes::users::contacts_add_or_upsert))
        .route("/api/v1/contacts/add", post(routes::users::contacts_add_or_upsert))
        .route("/api/v1/contacts/{user_id}", delete(routes::users::contacts_delete_user))
        .route("/api/v1/chats", get(routes::chat_lifecycle::chats_list_clean).post(routes::chat_lifecycle::chats_create_strict))
        .route("/api/v1/chats/create", post(routes::chat_lifecycle::chats_create_strict))
        .route("/api/v1/chats/welcome", post(routes::chat_extras::chats_welcome))
        .route("/api/v1/chats/join", post(routes::chat_extras::invites_join))
        .route("/api/v1/invites/join", post(routes::chat_extras::invites_join))
        .route("/api/v1/chats/{chat_id}", get(routes::chat_lifecycle::chats_get_scoped).put(chats_update).delete(routes::chat_lifecycle::chats_delete_chat))
        .route("/api/v1/chats/{chat_id}/members", get(chats_members_list).post(chats_members_add))
        .route("/api/v1/chats/{chat_id}/members/{user_id}", put(chats_members_update).delete(chats_members_delete))
        .route("/api/v1/chats/{chat_id}/topics", get(chats_topics).post(chats_topics_create))
        .route("/api/v1/chats/{chat_id}/topics/{topic_id}", put(chats_topics_update).delete(chats_topics_delete))
        .route("/api/v1/chats/{chat_id}/invites", get(routes::chat_extras::invites_list).post(routes::chat_extras::invites_create))
        .route("/api/v1/chats/{chat_id}/invites/{invite_id}", delete(routes::chat_extras::invites_revoke))
        .route("/api/v1/chats/{chat_id}/archive", post(routes::chat_extras::chat_archive))
        .route("/api/v1/chats/{chat_id}/pin", post(routes::chat_extras::chat_pin_dialog))
        .route("/api/v1/chats/{chat_id}/mute", post(routes::chat_extras::chat_mute))
        .route("/api/v1/chats/{chat_id}/pins", get(routes::chat_extras::pins_list).post(routes::chat_extras::pins_pin).delete(routes::chat_extras::pins_unpin))
        .route("/api/v1/chats/{chat_id}/pins/{message_id}", delete(routes::chat_extras::pins_unpin_path))
        .route("/api/v1/chats/{chat_id}/draft", get(routes::chat_extras::draft_get).put(routes::chat_extras::draft_put).delete(routes::chat_extras::draft_delete))
        .route("/api/v1/chats/{chat_id}/statistics", get(routes::analytics_wallet::chat_statistics))
        .route("/api/v1/chats/{chat_id}/export", get(routes::analytics_wallet::chat_export))
        .route("/api/v1/folders", get(routes::chat_extras::folders_list).post(routes::chat_extras::folders_create))
        .route("/api/v1/folders/{folder_id}", put(routes::chat_extras::folders_update).delete(routes::chat_extras::folders_delete))
        // --- Monetization & Membership Routes ---
        .route("/api/alipay/create-order", post(alipay_create_order))
        .route("/api/alipay/query-order", get(alipay_query_order))
        .route("/api/alipay/notify", post(alipay_notify))
        .route("/api/alipay/check-membership", get(alipay_check_membership))
        .route("/api/admin/use-redeem-code", post(admin_use_redeem_code))
        .route("/api/admin/create-redeem-code", post(admin_create_redeem_code))
        // --- Protected Sync Routes ---
        .route("/api/v1/sync/difference", get(sync_difference).post(routes::sync_polls::sync_get_difference_clean))
        .route("/api/v1/sync/get_difference", post(routes::sync_polls::sync_get_difference_clean))
        // --- Protected Media CAS & Streaming Routes ---
        .route("/api/v1/media/upload", post(routes::media::media_upload_direct))
        .route("/api/v1/media/upload/init", post(routes::media::media_upload_init_clean))
        .route("/api/v1/media/upload/{upload_id}/part/{part_index}", put(routes::media::media_upload_part_clean).post(routes::media::media_upload_part_clean))
        .route("/api/v1/media/upload/{upload_id}/status", get(routes::media::media_upload_status))
        .route("/api/v1/media/upload/{upload_id}/complete", post(routes::media::media_upload_complete_clean))
        .route("/api/v1/media/stream/{file_id}", get(routes::media::media_stream_clean))
        .route("/api/v1/media/{file_id}", get(routes::media::media_stream_clean))
        .route("/api/v1/media/{file_id}/meta", get(routes::media::media_metadata_get))
        // --- Schema-First Platform Action Bus Routes ---
        .route("/api/v1/actions", get(routes::actions::handle_actions_list))
        .route("/api/v1/actions/approvals/{gate_id}/approve", post(routes::actions::handle_action_gate_approve))
        .route("/api/v1/actions/{action_name}", post(routes::actions::handle_action_post))
        .route("/api/v1/actions/{action_name}/schema", get(routes::actions::handle_action_schema))
        // --- Push Device Registration Routes ---
        .route("/api/v1/devices/register_push", post(routes::devices::handle_register_push))
        .route("/api/v1/devices/push", delete(routes::devices::handle_unregister_push))
        // --- E2EE Prekey & Multi-Device Routes ---
        .route("/api/v1/e2ee/prekeys", post(routes::e2ee::publish_prekeys))
        .route("/api/v1/e2ee/prekeys/count", get(routes::e2ee::prekey_count))
        .route("/api/v1/e2ee/devices/{user_id}/prekeys", get(routes::e2ee::fetch_user_prekey_bundles))
        .route("/api/v1/e2ee/prekeys/{user_id}", get(routes::e2ee::fetch_user_prekey_bundles))
        .route("/api/v1/e2ee/devices/{device_id}", delete(routes::e2ee::revoke_device))
        // --- Send-As Peers Route ---
        .route("/api/v1/chats/{chat_id}/send_as", get(routes::chat_extras::chat_send_as_list))
        // --- Stickers, GIFs, Stories, Calls, Search & Wallet ---
        .route("/api/v1/stickers/sets", get(routes::social::stickers_sets))
        .route("/api/v1/stickers/sets/{set_id}/install", post(routes::social::stickers_install))
        .route("/api/v1/gifs/search", get(routes::social::gifs_search))
        .route("/api/v1/stories", get(routes::social::stories_list).post(routes::social::stories_create))
        .route("/api/v1/stories/{story_id}/view", post(routes::social::stories_view))
        .route("/api/v1/stories/{story_id}", delete(routes::social::stories_delete))
        .route("/api/v1/calls", get(routes::social::calls_list))
        .route("/api/v1/calls/invite", post(routes::social::calls_invite))
        .route("/api/v1/calls/signal", post(routes::social::calls_signal))
        .route("/api/v1/calls/{call_id}/rate", post(routes::social::calls_rate))
        .route("/api/v1/search", get(routes::analytics_wallet::search_unified))
        .route("/api/v1/search/messages", get(routes::analytics_wallet::search_unified))
        .route("/api/v1/wallet/balance", get(routes::analytics_wallet::wallet_balance))
        .route("/api/v1/wallet/transactions", get(routes::analytics_wallet::wallet_transactions))
        .route("/api/v1/wallet/topup", post(routes::analytics_wallet::wallet_topup))
        .route("/api/v1/wallet/send_stars", post(routes::analytics_wallet::wallet_send_stars))
        .route("/api/v1/wallet/gift", post(routes::analytics_wallet::wallet_send_stars))
        // --- Protected WebRTC Legacy Aliases ---
        .route("/api/v1/webrtc/call/invite", post(routes::social::calls_invite))
        .route("/api/v1/webrtc/call/signal", post(routes::social::calls_signal))
        .route("/api/v1/webrtc/turn_credentials", get(routes::social::webrtc_turn_credentials_clean))
        // --- Agent & Capability Routes ---
        .route("/api/v1/agent/runs", post(agent_runs))
        .route("/api/v1/agent/turn", post(agent_turn))
        .route("/api/v1/chats/{chat_id}/agent-runs", post(chats_agent_runs))
        .route("/api/v1/hermes/status", get(routes::hermes::hermes_status))
        .route("/api/v1/hermes/turns", post(routes::hermes::hermes_turns_create))
        .route("/api/v1/hermes/turns/{id}/events", get(routes::hermes::hermes_turn_events))
        .route("/api/v1/hermes/command", post(routes::hermes::hermes_command_run))
        .route("/api/v1/hermes/commands", get(routes::hermes::hermes_commands_list))
        .route("/api/v1/hermes/sessions", get(routes::hermes::hermes_sessions_list))
        .route("/api/v1/hermes/sessions/search", post(routes::hermes::hermes_sessions_search))
        .route("/api/v1/hermes/sessions/{id}", get(routes::hermes::hermes_session_get))
        .route("/api/v1/hermes/sessions/{id}/branch", post(routes::hermes::hermes_session_branch))
        .route("/api/v1/hermes/sessions/{id}/compress", post(routes::hermes::hermes_session_compress))
        .route("/api/v1/hermes/sessions/{id}/undo", post(routes::hermes::hermes_session_undo))
        .route("/api/v1/hermes/sessions/{id}/export", post(routes::hermes::hermes_session_export))
        .route("/api/v1/hermes/memory", get(routes::hermes::hermes_memory_get).put(routes::hermes::hermes_memory_put))
        .route("/api/v1/hermes/memory/action", post(routes::hermes::hermes_memory_action))
        .route("/api/v1/hermes/soul", get(routes::hermes::hermes_soul_get).put(routes::hermes::hermes_soul_put))
        .route("/api/v1/hermes/honcho", get(routes::hermes::hermes_honcho_get).post(routes::hermes::hermes_honcho_post))
        .route("/api/v1/hermes/skills", get(routes::hermes::hermes_skills_list).post(routes::hermes::hermes_skills_create))
        .route("/api/v1/hermes/skills/{name}", get(routes::hermes::hermes_skill_get).patch(routes::hermes::hermes_skill_patch).delete(routes::hermes::hermes_skill_delete))
        .route("/api/v1/hermes/skills/{name}/approve", post(routes::hermes::hermes_skill_approve))
        .route("/api/v1/hermes/toolsets", get(routes::hermes::hermes_toolsets_get).put(routes::hermes::hermes_toolsets_put))
        .route("/api/v1/hermes/approvals", get(routes::hermes::hermes_approvals_list))
        .route("/api/v1/hermes/approvals/{id}/decide", post(routes::hermes::hermes_approval_decide))
        .route("/api/v1/hermes/cron", get(routes::hermes::hermes_cron_list).post(routes::hermes::hermes_cron_create))
        .route("/api/v1/hermes/cron/{id}", patch(routes::hermes::hermes_cron_patch).delete(routes::hermes::hermes_cron_delete))
        .route("/api/v1/hermes/cron/{id}/run", post(routes::hermes::hermes_cron_run))
        .route("/api/v1/hermes/heartbeat", get(routes::hermes::hermes_heartbeat_get).post(routes::hermes::hermes_heartbeat_post))
        .route("/api/v1/hermes/kanban/boards", get(routes::hermes::hermes_kanban_boards_list).post(routes::hermes::hermes_kanban_boards_create))
        .route("/api/v1/hermes/kanban/boards/{id}", get(routes::hermes::hermes_kanban_board_get))
        .route("/api/v1/hermes/kanban/cards", post(routes::hermes::hermes_kanban_cards_create))
        .route("/api/v1/hermes/kanban/cards/{id}", patch(routes::hermes::hermes_kanban_cards_patch).delete(routes::hermes::hermes_kanban_cards_delete))
        .route("/api/v1/hermes/providers", get(routes::hermes::hermes_providers_get).put(routes::hermes::hermes_providers_put))
        .route("/api/v1/hermes/magpie/state", get(routes::magpie_api::magpie_state_get))
        .route("/api/v1/hermes/magpie/providers", get(routes::magpie_api::magpie_providers_list).post(routes::magpie_api::magpie_providers_upsert).put(routes::magpie_api::magpie_providers_upsert))
        .route("/api/v1/hermes/magpie/providers/{id}", get(routes::magpie_api::magpie_provider_get).put(routes::magpie_api::magpie_provider_update).patch(routes::magpie_api::magpie_provider_update).delete(routes::magpie_api::magpie_provider_delete))
        .route("/api/v1/hermes/magpie/providers/{id}/probe", post(routes::magpie_api::magpie_provider_probe))
        .route("/api/v1/hermes/magpie/subscriptions", get(routes::magpie_api::magpie_subscriptions_get))
        .route("/api/v1/hermes/magpie/subscriptions/{id}/activate", post(routes::magpie_api::magpie_subscription_activate))
        .route("/api/v1/hermes/magpie/subscriptions/scan", post(routes::magpie_api::magpie_subscriptions_scan))
        .route("/api/v1/hermes/magpie/groups", get(routes::magpie_api::magpie_groups_get).post(routes::magpie_api::magpie_groups_upsert).put(routes::magpie_api::magpie_groups_upsert).delete(routes::magpie_api::magpie_groups_delete_query))
        .route("/api/v1/hermes/magpie/groups/{id}", delete(routes::magpie_api::magpie_group_delete_path))
        .route("/api/v1/hermes/magpie/masking", get(routes::magpie_api::magpie_masking_get).put(routes::magpie_api::magpie_masking_put).post(routes::magpie_api::magpie_masking_put))
        .route("/api/v1/hermes/magpie/masking/preview", post(routes::magpie_api::magpie_masking_preview))
        .route("/api/v1/hermes/magpie/import", post(routes::magpie_api::magpie_import))
        .route("/api/v1/magpie/v1/models", get(routes::magpie_api::magpie_gateway_models))
        .route("/api/v1/magpie/v1/chat/completions", post(routes::magpie_api::magpie_gateway_chat_completions))
        .route("/api/v1/magpie/v1/responses", post(routes::magpie_api::magpie_gateway_responses))
        .route("/api/v1/magpie/v1/messages", post(routes::magpie_api::magpie_gateway_messages))
        .route("/api/v1/hermes/usage", get(routes::hermes::hermes_usage_get))
        .route("/api/v1/hermes/rollback", post(routes::hermes::hermes_rollback_post))
        .route("/api/v1/hermes/terminal/processes", get(routes::hermes::hermes_terminal_processes_get))
        .route("/api/v1/hermes/talk/turn", post(routes::hermes::hermes_talk_turn))
        .route("/api/v1/hermes/acp", post(routes::hermes::hermes_acp_rpc))
        .route("/v1/chat/completions", post(routes::hermes::openai_chat_completions))
        .route("/v1/models", get(routes::hermes::openai_models_list))
        .route("/api/v1/tasks", get(routes::acp::tasks_list).post(routes::acp::tasks_create))
        .route("/api/v1/tasks/{task_id}/approve", post(routes::acp::tasks_approve))
        .route("/api/v1/mcp/servers", get(routes::acp::mcp_servers_list).post(routes::acp::mcp_servers_register))
        .route("/api/v1/mcp/tools", get(mcp_tools_list))
        .route("/api/v1/mcp/call", post(mcp_tool_call))
        .route("/api/v1/mcp/permission/approve", post(mcp_permission_approve))
        .route("/api/v1/computer/lease", post(routes::acp::computer_lease_acquire))
        .route("/api/v1/computer/lease/acquire", post(routes::acp::computer_lease_acquire))
        .route("/api/v1/computer/lease/emergency_takeover", post(routes::acp::computer_lease_emergency_takeover))
        .route("/api/v1/computer/action", post(computer_action_execute))
        .route("/api/v1/talk/session", post(routes::acp::talk_session_create))
        .route("/api/v1/talk/barge_in", post(talk_barge_in))
        .route("/api/v1/talk/audio", post(talk_audio_enqueue))
        // --- Remote Computers, Cloud Servers & Cloud Sandbox Control Plane ---
        .route("/api/v1/computers", get(routes::computers::computers_list))
        .route(
            "/api/v1/computers/local/register",
            post(routes::computers::computers_local_register),
        )
        .route(
            "/api/v1/computers/local/poll",
            get(routes::computers::computers_local_poll),
        )
        .route(
            "/api/v1/computers/commands/{command_id}/approve",
            post(routes::computers::computers_command_approve),
        )
        .route(
            "/api/v1/computers/commands/{command_id}/complete",
            post(routes::computers::computers_command_complete)
                .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route(
            "/api/v1/computers/cloud",
            post(routes::computers::computers_cloud_upsert),
        )
        .route(
            "/api/v1/computers/{node_id}/policy",
            patch(routes::computers::computers_policy_patch),
        )
        .route(
            "/api/v1/computers/{node_id}",
            delete(routes::computers::computers_delete),
        )
        .route(
            "/api/v1/computers/{node_id}/test",
            post(routes::computers::computers_test),
        )
        .route(
            "/api/v1/computers/exec",
            post(routes::computers::computers_exec)
                .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route(
            "/api/v1/bots/{bot_id}/computer",
            get(routes::computers::bot_computer_get).put(routes::computers::bot_computer_put),
        )
        .route(
            "/api/v1/computer-agent/exec",
            post(routes::computers::computer_agent_exec)
                .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        // --- Tailscale-style Mesh Coordination, STUN & Built-in DERP WebSocket ---
        .route(
            "/api/v1/tailnet/register",
            post(routes::tailnet::tailnet_register),
        )
        .route(
            "/api/v1/tailnet/netmap",
            get(routes::tailnet::tailnet_netmap),
        )
        .route("/api/v1/tailnet/stun", get(routes::tailnet::tailnet_stun))
        .route(
            "/api/v1/tailnet/derp",
            get(routes::tailnet::tailnet_derp_ws),
        )
        // --- BotFather, Companion AI Bots & Telegram Mini Apps (1:1 + Beyond) ---
        .route(
            "/api/v1/miniapps",
            get(routes::bots_miniapps::miniapps_list).post(routes::bots_miniapps::miniapps_create),
        )
        .route(
            "/api/v1/miniapps/installed",
            get(routes::bots_miniapps::miniapps_installed),
        )
        .route(
            "/api/v1/miniapps/recent",
            get(routes::bots_miniapps::miniapps_recent),
        )
        .route(
            "/api/v1/miniapps/{bot_username}/{short_name}/install",
            post(routes::bots_miniapps::miniapps_install)
                .delete(routes::bots_miniapps::miniapps_uninstall),
        )
        .route(
            "/api/v1/miniapps/{bot_username}/{short_name}/launch",
            post(routes::bots_miniapps::miniapps_launch),
        )
        .route(
            "/api/v1/miniapps/{bot_username}/{short_name}/validate-init-data",
            post(routes::bots_miniapps::miniapps_validate_init_data),
        )
        .route(
            "/api/v1/miniapps/{bot_username}/{short_name}/action",
            post(routes::bots_miniapps::miniapps_action),
        )
        .route(
            "/api/v1/miniapps/{bot_username}/{short_name}/storage",
            get(routes::bots_miniapps::miniapps_storage_get)
                .put(routes::bots_miniapps::miniapps_storage_put)
                .delete(routes::bots_miniapps::miniapps_storage_delete),
        )
        .route("/api/v1/bots", post(routes::bots_miniapps::bots_create))
        .route("/api/v1/bots/my", get(routes::bots_miniapps::bots_my))
        .route(
            "/api/v1/bots/attach-menu",
            get(routes::bots_miniapps::bots_attach_menu),
        )
        .route(
            "/api/v1/bots/profile/{target}",
            get(routes::bots_miniapps::bots_public_profile),
        )
        .route(
            "/api/v1/bots/{bot_id}/can-send-message",
            get(routes::bots_miniapps::bots_can_send_message),
        )
        .route(
            "/api/v1/bots/{bot_id}/allow-send-message",
            post(routes::bots_miniapps::bots_allow_send_message),
        )
        .route(
            "/api/v1/bots/{bot_id}/accept-app-tos",
            post(routes::bots_miniapps::bots_accept_app_tos),
        )
        .route(
            "/api/v1/bots/{bot_id}/block",
            post(routes::bots_miniapps::bots_block),
        )
        .route(
            "/api/v1/fabushi-bots",
            get(routes::bots_miniapps::fabushi_bots_list)
                .post(routes::bots_miniapps::fabushi_bots_create),
        )
        .route(
            "/api/v1/fabushi-bots/{bot_id}",
            get(routes::bots_miniapps::fabushi_bot_get)
                .patch(routes::bots_miniapps::fabushi_bot_patch),
        )
        .route(
            "/api/v1/botfather/nl-create",
            post(routes::bots_miniapps::botfather_nl_create),
        )
        .route(
            "/api/v1/chats/{chat_id}/callback",
            post(routes::bots_miniapps::chat_callback),
        )
        .route(
            "/miniapps/{bot_username}/{short_name}",
            get(routes::bots_miniapps::serve_miniapp_html),
        )
        .route(
            "/api/v1/fabushi-miniapp/app.wasm",
            get(routes::fabushi_miniapp_api::serve_app_wasm),
        )
        .route(
            "/api/v1/fabushi-miniapp/assets/{*name}",
            get(routes::fabushi_miniapp_api::serve_asset),
        )
        .route(
            "/api/v1/fabushi-miniapp/bootstrap",
            get(routes::fabushi_miniapp_api::bootstrap),
        )
        .route(
            "/api/v1/fabushi-miniapp/transfer",
            post(routes::fabushi_miniapp_api::record_transfer_handler),
        )
        .route(
            "/api/v1/fabushi-miniapp/practice/session",
            post(routes::fabushi_miniapp_api::practice_session_handler),
        )
        .route(
            "/api/v1/fabushi-miniapp/practice/record",
            get(routes::fabushi_miniapp_api::list_practice_records_handler)
                .post(routes::fabushi_miniapp_api::create_practice_record_handler)
                .put(routes::fabushi_miniapp_api::update_practice_record_body),
        )
        .route(
            "/api/v1/fabushi-miniapp/practice/record/{id}",
            put(routes::fabushi_miniapp_api::update_practice_record_by_id)
                .delete(routes::fabushi_miniapp_api::delete_practice_record_handler),
        )
        .route(
            "/api/v1/fabushi-miniapp/profile",
            get(routes::fabushi_miniapp_api::get_profile_handler)
                .put(routes::fabushi_miniapp_api::update_profile_handler)
                .post(routes::fabushi_miniapp_api::update_profile_handler),
        )
        .route(
            "/api/v1/fabushi-miniapp/membership",
            post(routes::fabushi_miniapp_api::activate_membership_handler),
        )
        .route(
            "/api/v1/fabushi-miniapp/follow",
            post(routes::fabushi_miniapp_api::follow_handler),
        )
        .route(
            "/api/v1/fabushi-miniapp/leaderboard",
            get(routes::fabushi_miniapp_api::leaderboard_handler),
        )
        .route(
            "/api/v1/fabushi-miniapp/heartbeat",
            post(routes::fabushi_miniapp_api::heartbeat_handler),
        )
        .route(
            "/api/v1/fabushi-miniapp/ai-chat",
            post(routes::fabushi_miniapp_api::ai_chat_handler),
        )
        .route(
            "/js/telegram-web-app.js",
            get(routes::telegram_bot_api::serve_telegram_web_app_js),
        )
        .route(
            "/bot/{token}/{method}",
            get(routes::telegram_bot_api::telegram_bot_api_dispatch_split)
                .post(routes::telegram_bot_api::telegram_bot_api_dispatch_split),
        )
        .route(
            "/{bot_token_seg}/{method}",
            get(routes::telegram_bot_api::telegram_bot_api_dispatch)
                .post(routes::telegram_bot_api::telegram_bot_api_dispatch),
        )
        // --- WebSocket Upgrade Route ---
        .route("/api/v1/ws", get(ws_upgrade_handler))
        // --- Tower Middleware Stack ---
        .layer(
            CompressionLayer::new().gzip(true).compress_when(
                tower_http::compression::predicate::Predicate::and(
                    tower_http::compression::DefaultPredicate::new(),
                    tower_http::compression::predicate::NotForContentType::new("text/event-stream"),
                ),
            ),
        )
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// ============================================================================
// Public Route Handlers
// ============================================================================

async fn health_check() -> &'static str {
    "OK"
}

async fn server_status() -> Json<Value> {
    Json(json!({
        "server": "gpui_telegram_server",
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "capabilities": [
            "auth", "phone_otp", "email_otp", "2fa", "qr_pair", "passkey",
            "chats", "topics", "folders", "drafts", "invites", "pins",
            "messaging_pts", "reactions", "polls", "media_cas_206",
            "stickers", "gifs", "stories", "calls_webrtc", "tantivy_search",
            "statistics", "export", "wallet_stars", "acp_durable",
            "hermes_agent", "hermes_gateway", "openai_compat", "acp_json_rpc"
        ]
    }))
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default = "default_first_name")]
    pub first_name: String,
    pub last_name: Option<String>,
    #[serde(default = "default_device_name")]
    pub device_name: String,
    #[serde(default)]
    pub auto_welcome: bool,
}

fn default_first_name() -> String {
    "User".into()
}

fn default_device_name() -> String {
    "GPUI Desktop".into()
}

async fn auth_register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterRequest>,
) -> Response {
    let username_clean = req.username.trim();
    if username_clean.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_credentials", "message": "Username cannot be empty" })),
        )
            .into_response();
    }
    if username_clean.chars().count() > crate::auth::MAX_USERNAME_CHARS {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_credentials", "message": "username must be 1..=128 characters" })),
        )
            .into_response();
    }
    if req.password.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_credentials", "message": "Password cannot be empty" })),
        )
            .into_response();
    }
    if req.password.as_bytes().len() > crate::auth::MAX_PASSWORD_BYTES {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_credentials", "message": "password must not exceed 1024 bytes" })),
        )
            .into_response();
    }

    // Check if user already exists in SQLite
    match state.db.get_user_by_username(username_clean) {
        Ok(Some(_)) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "user_already_exists",
                    "message": format!("Username already registered: {}", username_clean)
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "database_error", "message": e.to_string() })),
            )
                .into_response();
        }
        Ok(None) => {}
    }

    // Register user in memory
    match state.session_manager.register(
        username_clean,
        &req.password,
        &req.first_name,
        req.last_name.clone(),
        &req.device_name,
    ) {
        Ok((user, session, token)) => {
            // Persist user record to SQLite database
            let now_ms = user.created_at.timestamp_millis();
            let db_user = crate::db::schema::UserRecord {
                user_id: user.user_id,
                username: Some(user.username.clone()),
                phone: None,
                first_name: user.first_name.clone(),
                last_name: user.last_name.clone(),
                bio: None,
                password_hash: Some(user.password_hash.clone()),
                is_bot: false,
                is_verified: false,
                is_support: false,
                is_scam: false,
                is_fake: false,
                profile_photo_file_id: None,
                account_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            };

            if let Err(e) = state.db.create_user(&db_user) {
                tracing::error!("Failed to persist user to SQLite: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "database_error", "message": e.to_string() })),
                )
                    .into_response();
            }

            // Issue session via PersistentSessionStore (not cached in SessionManager)
            state.session_manager.revoke_session(&session.session_id);
            let (sess_rec, persistent_token) = match state.session_store.issue_session(
                user.user_id.raw(),
                &req.device_name,
                "gpui_desktop",
                "0.1.0",
                1,
                30 * 24 * 3600,
            ) {
                Ok(pair) => pair,
                Err(_) => (
                    crate::auth::session_store::SessionRecord {
                        session_id: session.session_id,
                        user_id: user.user_id.raw(),
                        device_id: req.device_name.clone(),
                        client_type: "gpui_desktop".into(),
                        app_version: "0.1.0".into(),
                        ip_address: None,
                        location_desc: None,
                        account_generation: 1,
                        created_at_ms: session.created_at.timestamp_millis(),
                        last_active_ms: session.created_at.timestamp_millis(),
                        expires_at_ms: session.expires_at.timestamp_millis(),
                        revoked_at_ms: None,
                    },
                    token,
                ),
            };
            if req.auto_welcome {
                let _ = ensure_user_welcome_bot(&state, user.user_id, Some(&user.first_name));
            }

            (
                StatusCode::CREATED,
                Json(json!({
                    "user_id": user.user_id.raw(),
                    "username": user.username,
                    "first_name": user.first_name,
                    "session_id": sess_rec.session_id,
                    "token": persistent_token,
                    "created_at": user.created_at,
                })),
            )
                .into_response()
        }
        Err(AuthError::UserAlreadyExists(msg)) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "user_already_exists", "message": msg })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "registration_failed", "message": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    #[serde(default = "default_device_name")]
    pub device_name: String,
}

async fn auth_login(State(state): State<Arc<AppState>>, Json(req): Json<LoginRequest>) -> Response {
    if req.username.trim().chars().count() > crate::auth::MAX_USERNAME_CHARS
        || req.password.as_bytes().len() > crate::auth::MAX_PASSWORD_BYTES
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_credentials", "message": "Invalid username or password" })),
        )
            .into_response();
    }

    // 1. Check authoritative SQLite user store first (supports multi-node login)
    if let Ok(Some(db_user)) = state.db.get_user_by_username(&req.username) {
        if let Some(ref hash) = db_user.password_hash {
            if crate::auth::verify_password(&req.password, hash) {
                if let Ok(tfa) = state.db.get_two_factor_status(db_user.user_id) {
                    if tfa.has_password {
                        let challenge_token = format!("tfa_{}", Uuid::new_v4().simple());
                        let exp = Utc::now().timestamp_millis() + 600_000;
                        let _ = state.db.create_two_factor_challenge(&challenge_token, db_user.user_id, exp);
                        return (
                            StatusCode::OK,
                            Json(json!({
                                "requires_2fa": true,
                                "challenge_token": challenge_token,
                                "hint": tfa.hint,
                                "user_id": db_user.user_id.raw(),
                            })),
                        )
                            .into_response();
                    }
                }
                let username = db_user.username.clone().unwrap_or_else(|| req.username.clone());
                match state.session_store.issue_session(
                    db_user.user_id.raw(),
                    &req.device_name,
                    "gpui_desktop",
                    env!("CARGO_PKG_VERSION"),
                    db_user.account_generation.max(1),
                    30 * 24 * 3600,
                ) {
                    Ok((sess_rec, raw_token)) => {
                        let expires_at = chrono::DateTime::from_timestamp_millis(sess_rec.expires_at_ms)
                            .unwrap_or_else(Utc::now);
                        return (
                            StatusCode::OK,
                            Json(json!({
                                "user_id": db_user.user_id.raw(),
                                "username": username,
                                "session_id": sess_rec.session_id,
                                "token": raw_token,
                                "expires_at": expires_at,
                            })),
                        )
                            .into_response();
                    }
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "error": "database_error", "message": e.to_string() })),
                        )
                            .into_response();
                    }
                }
            } else {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "error": "invalid_credentials",
                        "message": "Invalid username or password"
                    })),
                )
                    .into_response();
            }
        }
    }

    // 2. Fallback to in-memory session manager (legacy in-memory-only unit test users)
    match state
        .session_manager
        .login(&req.username, &req.password, &req.device_name)
    {
        Ok((user, session, token)) => {
            if let Ok(tfa) = state.db.get_two_factor_status(user.user_id) {
                if tfa.has_password {
                    state.session_manager.revoke_session(&session.session_id);
                    let challenge_token = format!("tfa_{}", Uuid::new_v4().simple());
                    let exp = Utc::now().timestamp_millis() + 600_000;
                    let _ = state.db.create_two_factor_challenge(&challenge_token, user.user_id, exp);
                    return (
                        StatusCode::OK,
                        Json(json!({
                            "requires_2fa": true,
                            "challenge_token": challenge_token,
                            "hint": tfa.hint,
                            "user_id": user.user_id.raw(),
                        })),
                    )
                        .into_response();
                }
            }
            (
                StatusCode::OK,
                Json(json!({
                    "user_id": user.user_id.raw(),
                    "username": user.username,
                    "session_id": session.session_id,
                    "token": token,
                    "expires_at": session.expires_at,
                })),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "invalid_credentials",
                "message": "Invalid username or password"
            })),
        )
            .into_response(),
    }
}

// ============================================================================
// Milestone 2 Auth Extension Models & Handlers (Email & Alipay)
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct SendEmailCodeRequest {
    pub email: String,
}

#[derive(Debug, Serialize)]
pub struct SendEmailCodeResponse {
    pub ok: bool,
    pub cooldown_seconds: u32,
    pub expires_in_seconds: u32,
    pub expires_at_ms: i64,
    pub resend_after_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EmailRegisterRequest {
    pub email: String,
    pub code: String,
    pub username: String,
    pub password: String,
    #[serde(default = "default_first_name")]
    pub first_name: String,
    pub last_name: Option<String>,
    #[serde(default = "default_device_name")]
    pub device_name: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct AlipayStartRequest {
    #[serde(default = "default_device_name")]
    pub device_name: String,
}

#[derive(Debug, Serialize)]
pub struct AlipayStartResponse {
    pub attempt_id: String,
    pub auth_url: String,
    pub qr_payload: String,
    pub expires_at_ms: i64,
    pub poll_after_ms: u64,
}

#[derive(Debug, Deserialize, Default)]
pub struct AlipayCallbackRequest {
    pub auth_code: Option<String>,
    pub code: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AlipaySimulateScanRequest {
    pub attempt_id: String,
    pub mock_alipay_user_id: Option<String>,
    pub mock_nickname: Option<String>,
}

fn is_cloud_bridge_enabled(state: &AppState) -> bool {
    if std::env::var("GPUI_OFFLINE_TEST").is_ok() {
        return false;
    }
    if std::env::var("GPUI_ENABLE_CLOUD_BRIDGE").is_ok() {
        return true;
    }
    let db_path = state.config.resolve_db_path();
    !db_path.is_empty() && db_path != ":memory:"
}

fn is_synthetic_test_email(email: &str) -> bool {
    email.ends_with("@example.com")
        || email.ends_with(".invalid")
        || email.ends_with(".test")
        || email.ends_with("@localhost")
}

async fn curl_http_request(
    method: &str,
    url: &str,
    body: Option<&Value>,
    timeout_secs: u64,
) -> Option<(u16, String)> {
    let mut cmd = tokio::process::Command::new("curl");
    cmd.arg("-sS")
        .arg("-X")
        .arg(method)
        .arg("--max-time")
        .arg(timeout_secs.to_string())
        .arg("-w")
        .arg("\n%{http_code}");
    if let Some(json_body) = body {
        cmd.arg("-H")
            .arg("Content-Type: application/json")
            .arg("-d")
            .arg(json_body.to_string());
    }
    cmd.arg(url);
    let output = cmd.output().await.ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body_str, code_str) = stdout.rsplit_once('\n')?;
    let status = code_str.trim().parse::<u16>().ok()?;
    Some((status, body_str.to_string()))
}

async fn auth_email_send_code(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendEmailCodeRequest>,
) -> Response {
    let email_clean = req.email.trim().to_lowercase();
    if email_clean.len() < 3 || email_clean.len() > 254 || !email_clean.contains('@') || !email_clean.contains('.') {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_email", "message": "请输入有效邮箱地址" })),
        )
            .into_response();
    }

    if state.session_manager.get_user_by_email(&email_clean).is_some()
        || state.db.get_user_by_email(&email_clean).map(|u| u.is_some()).unwrap_or(false)
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "email_already_registered", "message": "该邮箱已注册，请直接登录" })),
        )
            .into_response();
    }

    let now_ms = Utc::now().timestamp_millis();

    if let Ok(Some(existing)) = state.db.get_latest_email_challenge(&email_clean) {
        if now_ms < existing.cooldown_until_ms {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": "rate_limited",
                    "message": "验证码发送过于频繁，请稍后再试",
                    "resend_after_ms": existing.cooldown_until_ms
                })),
            )
                .into_response();
        }
    }

    let code = crate::auth::session_manager::generate_verification_code();
    let code_hash = crate::auth::session_manager::hash_verification_code(&code);
    let challenge_id = Uuid::new_v4().to_string();
    let cooldown_until_ms = now_ms + 60_000;
    let expires_at_ms = now_ms + 600_000;

    let challenge = crate::db::schema::EmailChallengeRecord {
        challenge_id,
        email: email_clean.clone(),
        code_hash,
        attempts_left: 5,
        cooldown_until_ms,
        expires_at_ms,
        created_at_ms: now_ms,
    };

    if let Err(e) = state.db.save_email_challenge(&challenge) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        )
            .into_response();
    }

    if is_cloud_bridge_enabled(&state) && !is_synthetic_test_email(&email_clean) {
        let gateway_base = std::env::var("FABUSHI_CLOUD_GATEWAY_URL")
            .unwrap_or_else(|_| "https://api.ombhrum.com".to_string());
        let send_url = format!(
            "{}/api/auth/send-verification-code",
            gateway_base.trim_end_matches('/')
        );
        let payload = json!({ "email": email_clean, "type": "register" });
        if let Some((status, body_text)) =
            curl_http_request("POST", &send_url, Some(&payload), 6).await
        {
            if status == 400 && body_text.contains("已被注册") {
                let reset_payload = json!({ "email": email_clean, "type": "reset" });
                let _ = curl_http_request("POST", &send_url, Some(&reset_payload), 6).await;
            }
        }
    }

    tracing::info!("Sent email registration code to {}: {}", email_clean, code);

    let debug_code = if std::env::var("GPUI_HIDE_DEBUG_CODE").is_ok() {
        None
    } else {
        Some(code)
    };

    (
        StatusCode::OK,
        Json(SendEmailCodeResponse {
            ok: true,
            cooldown_seconds: 60,
            expires_in_seconds: 600,
            expires_at_ms,
            resend_after_ms: cooldown_until_ms,
            debug_code,
        }),
    )
        .into_response()
}

async fn auth_email_register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EmailRegisterRequest>,
) -> Response {
    let email_clean = req.email.trim().to_lowercase();
    let username_clean = req.username.trim();

    if email_clean.is_empty() || !email_clean.contains('@') {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_email", "message": "请输入有效邮箱地址" })),
        )
            .into_response();
    }
    if username_clean.len() < 3 || username_clean.len() > 32 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_username", "message": "用户名长度需为 3–32 位字符" })),
        )
            .into_response();
    }
    if req.password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_password", "message": "密码至少需要 8 位字符" })),
        )
            .into_response();
    }
    let code_clean = req.code.trim();
    if code_clean.len() != 6 || !code_clean.chars().all(|c| c.is_ascii_digit()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_code", "message": "验证码格式无效" })),
        )
            .into_response();
    }

    let challenge = match state.db.get_latest_email_challenge(&email_clean) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "challenge_not_found", "message": "请先获取验证码" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "database_error", "message": e.to_string() })),
            )
                .into_response();
        }
    };

    let now_ms = Utc::now().timestamp_millis();
    if now_ms > challenge.expires_at_ms {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "code_expired", "message": "验证码已失效，请重新获取" })),
        )
            .into_response();
    }
    if challenge.attempts_left <= 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "code_expired", "message": "验证码尝试次数过多，请重新获取" })),
        )
            .into_response();
    }

    let mut code_valid = crate::auth::session_manager::verify_verification_code(code_clean, &challenge.code_hash);
    if !code_valid && is_cloud_bridge_enabled(&state) && !is_synthetic_test_email(&email_clean) {
        let gateway_base = std::env::var("FABUSHI_CLOUD_GATEWAY_URL")
            .unwrap_or_else(|_| "https://api.ombhrum.com".to_string());
        let reg_url = format!("{}/api/auth/register", gateway_base.trim_end_matches('/'));
        let probe_user = format!("gpui_{}", &Uuid::new_v4().simple().to_string()[..10]);
        let payload = json!({
            "username": probe_user,
            "email": email_clean,
            "password": req.password,
            "verificationCode": code_clean
        });
        if let Some((status, body_text)) =
            curl_http_request("POST", &reg_url, Some(&payload), 6).await
        {
            if (200..300).contains(&status)
                || (status == 400 && body_text.contains("该邮箱已被注册"))
            {
                code_valid = true;
            }
        }
    }

    if !code_valid {
        let _ = state.db.decrement_email_challenge_attempts(&challenge.challenge_id);
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_code", "message": "验证码不正确" })),
        )
            .into_response();
    }

    let _ = state.db.delete_email_challenge(&challenge.challenge_id);

    match state.session_manager.register_with_email(
        &email_clean,
        username_clean,
        &req.password,
        &req.first_name,
        req.last_name.clone(),
        &req.device_name,
    ) {
        Ok((user, session, token)) => {
            let now_ms = user.created_at.timestamp_millis();
            let db_user = crate::db::schema::UserRecord {
                user_id: user.user_id,
                username: Some(user.username.clone()),
                phone: None,
                first_name: user.first_name.clone(),
                last_name: user.last_name.clone(),
                bio: None,
                password_hash: Some(user.password_hash.clone()),
                is_bot: false,
                is_verified: false,
                is_support: false,
                is_scam: false,
                is_fake: false,
                profile_photo_file_id: None,
                account_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            };

            if let Err(e) = state.db.create_user(&db_user) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "database_error", "message": e.to_string() })),
                )
                    .into_response();
            }
            let _ = state.db.set_user_email_verified(user.user_id, &email_clean);

            state.session_manager.revoke_session(&session.session_id);
            let (sess_id, persistent_token, expires_at) = match state.session_store.issue_session(
                user.user_id.raw(),
                &req.device_name,
                "gpui_desktop",
                "0.1.0",
                1,
                30 * 24 * 3600,
            ) {
                Ok((rec, tok)) => (
                    rec.session_id,
                    tok,
                    chrono::DateTime::from_timestamp_millis(rec.expires_at_ms)
                        .unwrap_or(session.expires_at),
                ),
                Err(_) => (session.session_id, token, session.expires_at),
            };

            (
                StatusCode::CREATED,
                Json(json!({
                    "user_id": user.user_id.raw(),
                    "username": user.username,
                    "first_name": user.first_name,
                    "session_id": sess_id,
                    "token": persistent_token,
                    "expires_at": expires_at,
                })),
            )
                .into_response()
        }
        Err(AuthError::UserAlreadyExists(msg)) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "user_already_exists", "message": msg })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "registration_failed", "message": e.to_string() })),
        )
            .into_response(),
    }
}

fn persist_alipay_user_and_session(
    state: &AppState,
    alipay_user_id: &str,
    nickname: &str,
    device_name: &str,
) -> (crate::auth::session_manager::UserRecord, SessionToken, String) {
    if let Ok(Some(existing_db_user)) = state.db.get_user_by_alipay_id(alipay_user_id) {
        let created_at = chrono::DateTime::from_timestamp_millis(existing_db_user.created_at_ms)
            .unwrap_or_else(Utc::now);
        let restored = crate::auth::session_manager::UserRecord {
            user_id: existing_db_user.user_id,
            username: existing_db_user
                .username
                .unwrap_or_else(|| format!("alipay_user_{}", existing_db_user.user_id.raw())),
            first_name: existing_db_user.first_name,
            last_name: existing_db_user.last_name,
            password_hash: existing_db_user.password_hash.unwrap_or_default(),
            created_at,
        };
        state
            .session_manager
            .restore_alipay_user(alipay_user_id, restored);
    }

    let (user, session, token) = state.session_manager.login_or_register_alipay(
        alipay_user_id,
        nickname,
        None,
        device_name,
    );

    let now_ms = Utc::now().timestamp_millis();
    let db_user = crate::db::schema::UserRecord {
        user_id: user.user_id,
        username: Some(user.username.clone()),
        phone: None,
        first_name: user.first_name.clone(),
        last_name: user.last_name.clone(),
        bio: None,
        password_hash: None,
        is_bot: false,
        is_verified: false,
        is_support: false,
        is_scam: false,
        is_fake: false,
        profile_photo_file_id: None,
        account_generation: 1,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    };
    let _ = state.db.create_user(&db_user);
    let _ = state
        .db
        .set_user_alipay_identity(user.user_id, alipay_user_id, nickname);

    state.session_manager.revoke_session(&session.session_id);
    if let Ok((rec, persistent_token)) = state.session_store.issue_session(
        user.user_id.raw(),
        device_name,
        "gpui_desktop",
        "0.1.0",
        1,
        30 * 24 * 3600,
    ) {
        let created_at = chrono::DateTime::from_timestamp_millis(rec.created_at_ms).unwrap_or_else(Utc::now);
        let expires_at = chrono::DateTime::from_timestamp_millis(rec.expires_at_ms).unwrap_or_else(Utc::now);
        let secret = persistent_token.split_once('.').map(|(_, s)| s).unwrap_or("");
        let tok = SessionToken::new_with_hash(
            rec.session_id,
            user.user_id,
            created_at,
            expires_at,
            device_name,
            SessionToken::hash_token(secret),
        );
        return (user, tok, persistent_token);
    }

    (user, session, token)
}

async fn start_upstream_canonical_alipay_oauth(state: &AppState) -> Option<(String, String, String)> {
    if !is_cloud_bridge_enabled(state) {
        return None;
    }
    let gateway_base = std::env::var("FABUSHI_CLOUD_GATEWAY_URL")
        .unwrap_or_else(|_| "https://api.ombhrum.com".to_string());
    let start_url = format!("{}/api/auth/oauth/start", gateway_base.trim_end_matches('/'));
    let payload = json!({
        "provider": "alipay",
        "platform": "macos",
        "deviceId": "gpui-telegram-google-cloud"
    });
    let (status, body_str) = curl_http_request("POST", &start_url, Some(&payload), 5).await?;
    if !(200..300).contains(&status) {
        return None;
    }
    let val: Value = serde_json::from_str(&body_str).ok()?;
    let upstream_attempt_id = val.get("attemptId")?.as_str()?.to_string();
    let authorization_url = val.get("authorizationUrl")?.as_str()?.to_string();
    let state_param = authorization_url
        .split("state=")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .filter(|s| !s.is_empty())?
        .to_string();
    let direct_openauth_url = format!(
        "https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id=2021005193647715&scope=auth_user&redirect_uri=https%3A%2F%2Fapi.ombhrum.com%2Fapi%2Fauth%2Falipay%2Fcallback&state={}",
        state_param
    );
    Some((upstream_attempt_id, direct_openauth_url, state_param))
}

async fn poll_upstream_canonical_alipay_oauth(
    state: &AppState,
    upstream_attempt_id: &str,
) -> Option<(String, String)> {
    if !is_cloud_bridge_enabled(state) {
        return None;
    }
    let gateway_base = std::env::var("FABUSHI_CLOUD_GATEWAY_URL")
        .unwrap_or_else(|_| "https://api.ombhrum.com".to_string());
    let poll_url = format!(
        "{}/api/auth/oauth/attempts/{}",
        gateway_base.trim_end_matches('/'),
        upstream_attempt_id
    );
    let (status_code, body_str) = curl_http_request("GET", &poll_url, None, 5).await?;
    if !(200..300).contains(&status_code) {
        return None;
    }
    let val: Value = serde_json::from_str(&body_str).ok()?;
    let status = val.get("status").and_then(|v| v.as_str()).unwrap_or_default();
    if status != "completed" && status != "complete" {
        return None;
    }
    let user_obj = val
        .get("session")
        .and_then(|s| s.get("user"))
        .or_else(|| val.get("user"));
    let alipay_uid = user_obj
        .and_then(|u| {
            u.get("alipayUserId")
                .or_else(|| u.get("id"))
                .and_then(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| v.as_u64().map(|n| n.to_string()))
                })
        })
        .unwrap_or_else(|| format!("alipay_{}", &upstream_attempt_id[..upstream_attempt_id.len().min(12)]));
    let nickname = user_obj
        .and_then(|u| {
            u.get("alipayNickname")
                .or_else(|| u.get("nickname"))
                .or_else(|| u.get("username"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("支付宝用户")
        .to_string();
    Some((alipay_uid, nickname))
}

async fn auth_alipay_start(
    State(state): State<Arc<AppState>>,
    body: Option<Json<AlipayStartRequest>>,
) -> Response {
    let _req = body.map(|b| b.0).unwrap_or_default();
    let attempt_id = Uuid::new_v4().to_string();
    let mut state_token = format!("fbs_{}", Uuid::new_v4().simple());
    let now_ms = Utc::now().timestamp_millis();
    let expires_at_ms = now_ms + 600_000;

    let app_id = "2021005193647715";
    let callback_url = "https%3A%2F%2Fapi.ombhrum.com%2Fapi%2Fauth%2Falipay%2Fcallback";
    let mut auth_url = format!(
        "https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id={}&scope=auth_user&redirect_uri={}&state={}",
        app_id, callback_url, state_token
    );

    if let Some((upstream_id, upstream_url, upstream_state)) =
        start_upstream_canonical_alipay_oauth(&state).await
    {
        auth_url = upstream_url;
        state_token = upstream_state;
        let mut map = state.alipay_upstream_attempts.write();
        map.insert(attempt_id.clone(), upstream_id.clone());
        map.insert(state_token.clone(), upstream_id);
    }

    let attempt = crate::db::schema::OAuthAttemptRecord {
        attempt_id: attempt_id.clone(),
        provider: "alipay".into(),
        state: state_token,
        status: "pending".into(),
        session_json: None,
        delivered_at_ms: None,
        expires_at_ms,
        created_at_ms: now_ms,
    };

    if let Err(e) = state.db.create_oauth_attempt(&attempt) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(AlipayStartResponse {
            attempt_id,
            auth_url: auth_url.clone(),
            qr_payload: auth_url,
            expires_at_ms,
            poll_after_ms: 1000,
        }),
    )
        .into_response()
}

async fn auth_alipay_poll(
    State(state): State<Arc<AppState>>,
    Path(attempt_id): Path<String>,
) -> Response {
    let mut attempt = match state.db.get_oauth_attempt(&attempt_id) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "oauth_attempt_not_found", "message": "登录会话不存在或已失效" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "database_error", "message": e.to_string() })),
            )
                .into_response();
        }
    };

    let now_ms = Utc::now().timestamp_millis();
    if attempt.expires_at_ms <= now_ms && attempt.status == "pending" {
        let _ = state.db.update_oauth_attempt_status(&attempt_id, "expired");
        return (
            StatusCode::OK,
            Json(json!({ "status": "expired" })),
        )
            .into_response();
    }

    if attempt.status == "pending" {
        let maybe_upstream_id = {
            let map = state.alipay_upstream_attempts.read();
            map.get(&attempt_id)
                .or_else(|| map.get(&attempt.state))
                .cloned()
        };
        if let Some(upstream_id) = maybe_upstream_id {
            if let Some((alipay_uid, nickname)) =
                poll_upstream_canonical_alipay_oauth(&state, &upstream_id).await
            {
                let (user, session, token) = persist_alipay_user_and_session(
                    &state,
                    &alipay_uid,
                    &nickname,
                    "GPUI Desktop Client",
                );
                let session_json = json!({
                    "user_id": user.user_id.raw(),
                    "username": user.username,
                    "first_name": user.first_name,
                    "session_id": session.session_id,
                    "token": token,
                    "expires_at": session.expires_at
                });
                let _ = state
                    .db
                    .complete_oauth_attempt(&attempt_id, &session_json.to_string());
                attempt.status = "completed".into();
            }
        }
    }

    if attempt.status != "completed" {
        return (
            StatusCode::OK,
            Json(json!({ "status": attempt.status })),
        )
            .into_response();
    }

    match state.db.claim_oauth_attempt_session(&attempt_id, now_ms) {
        Ok(Some(session_json_str)) => {
            let session_val: Value = serde_json::from_str(&session_json_str).unwrap_or(Value::Null);
            let token_str = session_val
                .get("token")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let user_id_num = session_val.get("user_id").and_then(|v| v.as_u64());
            let username_str = session_val
                .get("username")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let first_name_str = session_val
                .get("first_name")
                .and_then(|v| v.as_str())
                .unwrap_or("支付宝用户")
                .to_string();
            let user_dto = user_id_num.map(|uid| {
                json!({
                    "user_id": uid,
                    "username": username_str,
                    "first_name": first_name_str
                })
            });
            (
                StatusCode::OK,
                Json(json!({
                    "status": "completed",
                    "token": token_str,
                    "user_id": user_id_num,
                    "username": username_str,
                    "user": user_dto,
                    "session": session_val
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::GONE,
            Json(json!({
                "error": "session_already_delivered",
                "message": "登录凭证已领取或已失效"
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        )
            .into_response(),
    }
}

async fn resolve_alipay_identity_from_auth_code(
    state: &AppState,
    auth_code: &str,
) -> (String, String) {
    let default_uid = format!(
        "alipay_{}",
        if auth_code.is_empty() { "default" } else { auth_code }
    );
    let default_nick = "支付宝用户".to_string();

    if !is_cloud_bridge_enabled(state)
        || auth_code.is_empty()
        || auth_code.starts_with("mock_")
        || auth_code.starts_with("test_")
    {
        return (default_uid, default_nick);
    }

    let gateway_base = std::env::var("FABUSHI_CLOUD_GATEWAY_URL")
        .unwrap_or_else(|_| "https://api.ombhrum.com".to_string());
    let sdk_url = format!(
        "{}/api/auth/alipay/sdk-login",
        gateway_base.trim_end_matches('/')
    );
    let payload = json!({ "authCode": auth_code });
    if let Some((_status, body_str)) =
        curl_http_request("POST", &sdk_url, Some(&payload), 6).await
    {
        if let Ok(val) = serde_json::from_str::<Value>(&body_str) {
            let uid = val
                .get("user")
                .and_then(|u| u.get("alipayUserId").and_then(|v| v.as_str()))
                .or_else(|| val.get("alipayUserId").and_then(|v| v.as_str()))
                .map(str::to_string);
            let nick = val
                .get("user")
                .and_then(|u| {
                    u.get("alipayNickname")
                        .or_else(|| u.get("nickname"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| {
                    val.get("alipayInfo")
                        .and_then(|i| i.get("nickname"))
                        .and_then(|v| v.as_str())
                })
                .map(str::to_string);
            if let Some(real_uid) = uid {
                if !real_uid.is_empty() {
                    return (real_uid, nick.unwrap_or(default_nick));
                }
            }
        }
    }

    (default_uid, default_nick)
}

async fn auth_alipay_callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query_params): Query<HashMap<String, String>>,
    body_payload: Option<Json<AlipayCallbackRequest>>,
) -> Response {
    let auth_code = body_payload
        .as_ref()
        .and_then(|p| p.auth_code.clone().or_else(|| p.code.clone()))
        .or_else(|| query_params.get("auth_code").cloned())
        .or_else(|| query_params.get("code").cloned())
        .unwrap_or_default();

    let state_token = body_payload
        .as_ref()
        .and_then(|p| p.state.clone())
        .or_else(|| query_params.get("state").cloned())
        .unwrap_or_default();

    if state_token.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing_state", "message": "OAuth state parameter missing" })),
        )
            .into_response();
    }

    let attempt = match state.db.get_oauth_attempt_by_state(&state_token) {
        Ok(Some(a)) => a,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_state", "message": "无效或过期的 OAuth state" })),
            )
                .into_response();
        }
    };

    if attempt.status != "pending" || attempt.expires_at_ms <= Utc::now().timestamp_millis() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "attempt_expired", "message": "登录链接已失效，请重新扫码" })),
        )
            .into_response();
    }

    let (alipay_user_id, nickname) =
        resolve_alipay_identity_from_auth_code(&state, &auth_code).await;

    let (user, session, token) = persist_alipay_user_and_session(
        &state,
        &alipay_user_id,
        &nickname,
        "Alipay OAuth",
    );

    let session_json = json!({
        "user_id": user.user_id.raw(),
        "username": user.username,
        "first_name": user.first_name,
        "session_id": session.session_id,
        "token": token,
        "expires_at": session.expires_at
    });

    let _ = state
        .db
        .complete_oauth_attempt(&attempt.attempt_id, &session_json.to_string());

    let wants_html = body_payload.is_none()
        && (query_params.get("format").map(|s| s.as_str()) == Some("html")
            || headers
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.contains("text/html"))
                .unwrap_or(false));

    if wants_html {
        let html = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>支付宝登录成功 — GPUI Telegram</title><style>body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#0f172a;color:#f8fafc;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI", sans-serif}.card{background:#1e293b;border:1px solid #334155;border-radius:16px;padding:32px;max-width:420px;text-align:center;box-shadow:0 20px 40px rgba(0,0,0,.4)}h1{margin:0 0 12px;font-size:22px;color:#38bdf8}p{margin:0;color:#94a3b8;line-height:1.6}</style></head><body><div class="card"><h1>支付宝登录成功</h1><p>凭证已同步至 Google Cloud 服务器，正在返回 GPUI Telegram 客户端，您可以直接关闭此网页。</p></div><script>setTimeout(()=>{try{window.close()}catch(_){}},1200);</script></body></html>"#;
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html,
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({ "ok": true, "status": "completed" })),
    )
        .into_response()
}

async fn auth_alipay_simulate_scan(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AlipaySimulateScanRequest>,
) -> Response {
    let attempt = match state.db.get_oauth_attempt(&req.attempt_id) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "attempt_not_found", "message": "Attempt does not exist" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "database_error", "message": e.to_string() })),
            )
                .into_response();
        }
    };

    if attempt.status != "pending" || attempt.expires_at_ms <= Utc::now().timestamp_millis() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "attempt_not_pending", "message": "Attempt is expired or not pending" })),
        )
            .into_response();
    }

    let alipay_user_id = req.mock_alipay_user_id.unwrap_or_else(|| "mock_alipay_user_1001".into());
    let nickname = req.mock_nickname.unwrap_or_else(|| "支付宝测试用户".into());

    let (user, session, token) = persist_alipay_user_and_session(
        &state,
        &alipay_user_id,
        &nickname,
        "GPUI Desktop Client",
    );

    let session_json = json!({
        "user_id": user.user_id.raw(),
        "username": user.username,
        "first_name": user.first_name,
        "session_id": session.session_id,
        "token": token,
        "expires_at": session.expires_at
    });

    let _ = state.db.complete_oauth_attempt(&req.attempt_id, &session_json.to_string());

    (
        StatusCode::OK,
        Json(json!({ "ok": true, "status": "completed" })),
    )
        .into_response()
}

async fn auth_alipay_login_url(
    State(state): State<Arc<AppState>>,
    Query(query_params): Query<HashMap<String, String>>,
) -> Response {
    let platform = query_params
        .get("platform")
        .cloned()
        .unwrap_or_else(|| "cli".to_string());
    let attempt_id = Uuid::new_v4().to_string();
    let mut state_token = format!("fbs_{}", Uuid::new_v4().simple());
    let now_ms = Utc::now().timestamp_millis();
    let expires_at_ms = now_ms + 600_000;

    let app_id = "2021005193647715";
    let callback_url = "https%3A%2F%2Fapi.ombhrum.com%2Fapi%2Fauth%2Falipay%2Fcallback";
    let mut auth_url = format!(
        "https://openauth.alipay.com/oauth2/publicAppAuthorize.htm?app_id={}&scope=auth_user&redirect_uri={}&state={}",
        app_id, callback_url, state_token
    );

    if let Some((upstream_id, upstream_url, upstream_state)) =
        start_upstream_canonical_alipay_oauth(&state).await
    {
        auth_url = upstream_url;
        state_token = upstream_state;
        let mut map = state.alipay_upstream_attempts.write();
        map.insert(attempt_id.clone(), upstream_id.clone());
        map.insert(state_token.clone(), upstream_id);
    }

    let attempt = crate::db::schema::OAuthAttemptRecord {
        attempt_id: attempt_id.clone(),
        provider: "alipay".into(),
        state: state_token.clone(),
        status: "pending".into(),
        session_json: None,
        delivered_at_ms: None,
        expires_at_ms,
        created_at_ms: now_ms,
    };
    let _ = state.db.create_oauth_attempt(&attempt);

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "attemptId": attempt_id,
            "authUrl": auth_url,
            "state": state_token,
            "appId": app_id,
            "platform": platform
        })),
    )
        .into_response()
}

async fn auth_alipay_cli_session(
    State(state): State<Arc<AppState>>,
    Query(query_params): Query<HashMap<String, String>>,
) -> Response {
    let state_token = query_params.get("state").cloned().unwrap_or_default();
    if state_token.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "缺少state参数" })),
        )
            .into_response();
    }

    let mut attempt = match state.db.get_oauth_attempt_by_state(&state_token) {
        Ok(Some(a)) => a,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "登录状态不存在或已过期" })),
            )
                .into_response();
        }
    };

    let now_ms = Utc::now().timestamp_millis();
    if attempt.expires_at_ms <= now_ms && attempt.status == "pending" {
        let _ = state.db.update_oauth_attempt_status(&attempt.attempt_id, "expired");
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "登录状态已过期，请重新发起登录" })),
        )
            .into_response();
    }

    if attempt.status == "pending" {
        let maybe_upstream_id = {
            let map = state.alipay_upstream_attempts.read();
            map.get(&state_token)
                .or_else(|| map.get(&attempt.attempt_id))
                .cloned()
        };
        if let Some(upstream_id) = maybe_upstream_id {
            if let Some((alipay_uid, nickname)) =
                poll_upstream_canonical_alipay_oauth(&state, &upstream_id).await
            {
                let (user, session, token) = persist_alipay_user_and_session(
                    &state,
                    &alipay_uid,
                    &nickname,
                    "GPUI Desktop CLI",
                );
                let session_json = json!({
                    "user_id": user.user_id.raw(),
                    "username": user.username,
                    "first_name": user.first_name,
                    "session_id": session.session_id,
                    "token": token,
                    "expires_at": session.expires_at,
                    "alipay_user_id": alipay_uid
                });
                let _ = state
                    .db
                    .complete_oauth_attempt(&attempt.attempt_id, &session_json.to_string());
                attempt.status = "completed".into();
                attempt.session_json = Some(session_json.to_string());
            }
        }
    }

    if attempt.status != "completed" {
        return (
            StatusCode::OK,
            Json(json!({
                "success": false,
                "status": "pending",
                "message": "等待用户在浏览器中完成支付宝授权..."
            })),
        )
            .into_response();
    }

    let session_str = match state
        .db
        .claim_oauth_attempt_session(&attempt.attempt_id, now_ms)
    {
        Ok(Some(s)) => s,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "登录状态不存在或已过期" })),
            )
                .into_response();
        }
    };

    let sess: Value = serde_json::from_str(&session_str).unwrap_or_default();
    let uid = sess.get("user_id").and_then(|v| v.as_u64()).unwrap_or(0);
    let username = sess
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or("alipay_user");
    let first_name = sess
        .get("first_name")
        .and_then(|v| v.as_str())
        .unwrap_or("支付宝用户");
    let token = sess.get("token").and_then(|v| v.as_str()).unwrap_or("");
    let alipay_uid = sess
        .get("alipay_user_id")
        .and_then(|v| v.as_str())
        .unwrap_or(username);

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "status": "complete",
            "accessToken": token,
            "token": token,
            "username": username,
            "user": {
                "id": uid,
                "user_id": uid,
                "username": username,
                "first_name": first_name,
                "alipayUserId": alipay_uid,
                "alipayNickname": first_name
            }
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct AlipayDirectLoginRequest {
    #[serde(alias = "authCode")]
    pub auth_code: Option<String>,
    pub code: Option<String>,
    #[serde(alias = "alipayUserId")]
    pub alipay_user_id: Option<String>,
    pub nickname: Option<String>,
    #[serde(default = "default_device_name")]
    pub device_name: String,
}

async fn auth_alipay_direct_login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AlipayDirectLoginRequest>,
) -> Response {
    let auth_code = req.auth_code.or(req.code).unwrap_or_default();
    let (resolved_uid, resolved_nick) = if let Some(explicit_uid) = req.alipay_user_id {
        (
            explicit_uid,
            req.nickname.unwrap_or_else(|| "支付宝用户".to_string()),
        )
    } else if !auth_code.is_empty() {
        resolve_alipay_identity_from_auth_code(&state, &auth_code).await
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing_auth_code", "message": "缺少授权码" })),
        )
            .into_response();
    };

    let (user, session, token) = persist_alipay_user_and_session(
        &state,
        &resolved_uid,
        &resolved_nick,
        &req.device_name,
    );

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "message": "登录成功",
            "token": token,
            "accessToken": token,
            "session_id": session.session_id,
            "user_id": user.user_id.raw(),
            "username": user.username,
            "first_name": user.first_name,
            "user": {
                "id": user.user_id.raw(),
                "user_id": user.user_id.raw(),
                "username": user.username,
                "first_name": user.first_name,
                "alipayUserId": resolved_uid,
                "alipayNickname": user.first_name
            }
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct AlipayRegisterRequest {
    #[serde(alias = "alipayUserId")]
    pub alipay_user_id: Option<String>,
    #[serde(alias = "authCode")]
    pub auth_code: Option<String>,
    pub username: Option<String>,
    pub nickname: Option<String>,
    #[serde(default = "default_device_name")]
    pub device_name: String,
}

async fn auth_alipay_register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AlipayRegisterRequest>,
) -> Response {
    let (alipay_uid, default_nick) = if let Some(uid) = req.alipay_user_id {
        (uid, "支付宝用户".to_string())
    } else if let Some(code) = req.auth_code {
        resolve_alipay_identity_from_auth_code(&state, &code).await
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing_alipay_user_id", "message": "缺少支付宝用户ID或授权码" })),
        )
            .into_response();
    };
    let nick = req
        .nickname
        .or(req.username)
        .unwrap_or(default_nick);

    let (user, session, token) = persist_alipay_user_and_session(
        &state,
        &alipay_uid,
        &nick,
        &req.device_name,
    );

    (
        StatusCode::CREATED,
        Json(json!({
            "success": true,
            "message": "注册成功",
            "token": token,
            "accessToken": token,
            "session_id": session.session_id,
            "user_id": user.user_id.raw(),
            "username": user.username,
            "first_name": user.first_name,
            "user": {
                "id": user.user_id.raw(),
                "user_id": user.user_id.raw(),
                "username": user.username,
                "first_name": user.first_name,
                "alipayUserId": alipay_uid,
                "alipayNickname": user.first_name
            }
        })),
    )
        .into_response()
}

async fn auth_alipay_auth_string(State(state): State<Arc<AppState>>) -> Response {
    if is_cloud_bridge_enabled(&state) {
        let gateway_base = std::env::var("FABUSHI_CLOUD_GATEWAY_URL")
            .unwrap_or_else(|_| "https://api.ombhrum.com".to_string());
        let url = format!(
            "{}/api/auth/alipay/auth-string",
            gateway_base.trim_end_matches('/')
        );
        if let Some((status, body_str)) = curl_http_request("GET", &url, None, 5).await {
            if (200..300).contains(&status) {
                if let Ok(val) = serde_json::from_str::<Value>(&body_str) {
                    return (StatusCode::OK, Json(val)).into_response();
                }
            }
        }
    }

    let target_id = format!("fbs_sdk_{}", Uuid::new_v4().simple());
    let auth_str = format!(
        "apiname=com.alipay.account.auth&app_id=2021005193647715&app_name=mc&auth_type=AUTHACCOUNT&biz_type=openservice&method=alipay.open.auth.sdk.code.get&pid=2088102199998888&product_id=APP_FAST_LOGIN&scope=kuaijie&sign_type=RSA2&target_id={}",
        target_id
    );
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "authStr": auth_str,
            "targetId": target_id
        })),
    )
        .into_response()
}

async fn auth_alipay_portal() -> Response {
    let html = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>支付宝安全登录 — GPUI Telegram Google Cloud</title><style>body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#0f172a;color:#f8fafc;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}.card{background:#1e293b;border:1px solid #334155;border-radius:16px;padding:32px;max-width:440px;width:90%;text-align:center;box-shadow:0 20px 40px rgba(0,0,0,.4)}h1{margin:0 0 12px;font-size:22px;color:#38bdf8}p{margin:0 0 20px;color:#94a3b8;line-height:1.6}.btn{display:inline-block;background:#0284c7;color:#fff;text-decoration:none;padding:12px 24px;border-radius:10px;font-weight:600;border:none;cursor:pointer}</style></head><body><div class="card"><h1>GPUI Telegram 支付宝登录</h1><p>已迁移至 Google Cloud 认证网关，不再跳转旧版网页。</p><button class="btn" onclick="startLogin()">立即拉起支付宝授权</button></div><script>async function startLogin(){const r=await fetch('/api/v1/auth/alipay/start',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});const d=await r.json();if(d.auth_url){window.location.href=d.auth_url;}}</script></body></html>"#;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

/// Ensures a well-known AI Bot user exists in SQLite to satisfy foreign key constraints
pub(crate) fn ensure_default_ai_bot(db: &crate::db::ServerDatabase) -> UserId {
    let bot_id = UserId(777_000);
    if let Ok(None) = db.get_user(bot_id) {
        let now_ms = Utc::now().timestamp_millis();
        let bot_user = crate::db::schema::UserRecord {
            user_id: bot_id,
            username: Some("fabushi_ai_bot".into()),
            phone: None,
            first_name: "Telegram AI Bot".into(),
            last_name: None,
            bio: Some("Official AI Agent Assistant".into()),
            password_hash: None,
            is_bot: true,
            is_verified: true,
            is_support: false,
            is_scam: false,
            is_fake: false,
            profile_photo_file_id: None,
            account_generation: 1,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        let _ = db.create_user(&bot_user);
    }
    bot_id
}

pub(crate) fn hydrate_all_fabushi_bots(
    db: &crate::db::ServerDatabase,
    host: &gpui_telegram_agent::fabushi_bot::FabushiBotHost,
) {
    if let Ok(bot_uids) = db.list_all_bot_user_ids_from_users() {
        for uid in bot_uids {
            if uid != crate::db::miniapp_store::BOTFATHER_USER_ID as i64 {
                let _ = db.ensure_bot_registry_for_existing_bot_user(uid, 0, "");
            }
        }
    }
    if let Ok(records) = db.list_all_bots() {
        for rec in &records {
            if rec.bot_user_id != crate::db::miniapp_store::BOTFATHER_USER_ID as i64
                && !rec.is_standard_bot
            {
                let _ = crate::routes::bots_miniapps::provision_fabushi_bot_with_host(db, host, rec);
            }
        }
    }
}

struct ServerFabushiTeammateBridge {
    db: Arc<crate::db::ServerDatabase>,
    fabushi_bots: std::sync::Weak<gpui_telegram_agent::fabushi_bot::FabushiBotHost>,
    state_weak: Arc<std::sync::OnceLock<std::sync::Weak<AppState>>>,
}

impl gpui_telegram_agent::hermes::toolsets::FabushiTeammateHandler for ServerFabushiTeammateBridge {
    fn send_to_agent(
        &self,
        from_agent_id: &str,
        active_chat_id: Option<i64>,
        target_id: &str,
        message: &str,
        priority: bool,
        caller_depth: usize,
    ) -> Result<Value, String> {
        if caller_depth >= gpui_telegram_agent::DEFAULT_MAX_LOOP_HOPS as usize {
            return Err(format!(
                "Loop budget exceeded: reached {caller_depth} hops (max {})",
                gpui_telegram_agent::DEFAULT_MAX_LOOP_HOPS
            ));
        }
        let fabushi_bots = self
            .fabushi_bots
            .upgrade()
            .ok_or_else(|| "FabushiBotHost is shut down".to_string())?;
        let target = fabushi_bots
            .get_by_agent_id_or_username(target_id)
            .ok_or_else(|| format!("Fabushi teammate '{}' not found", target_id))?;
        let caller = fabushi_bots.get_by_agent_id_or_username(from_agent_id);
        let caller_name = caller
            .as_ref()
            .map(|c| c.profile().name)
            .unwrap_or_else(|| from_agent_id.to_string());
        let caller_bot_uid = caller
            .as_ref()
            .map(|c| c.bot_user_id as u64)
            .unwrap_or_else(|| target.settings().owner_user_id.max(0) as u64);

        let target_cid = active_chat_id.filter(|cid| *cid > 0).or_else(|| {
            let owner_uid = target.settings().owner_user_id;
            if owner_uid > 0 {
                self.db
                    .find_companion_chat_id_for_user(target.bot_user_id, owner_uid)
                    .ok()
                    .flatten()
                    .map(|c| c.0)
            } else {
                None
            }
        });

        let handoff_prompt = format!(
            "[Teammate Handshake from @{from_agent_id} ({caller_name})]\n{message}"
        );
        if let Some(cid) = target_cid {
            if let Ok(Some(chat)) = self.db.get_chat(ChatId(cid)) {
                if chat.chat_type != 0 && chat.chat_type != 4 {
                    let now_ms = Utc::now().timestamp_millis();
                    let _ = self.db.add_chat_member(&crate::db::ChatMemberRecord {
                        chat_id: ChatId(cid),
                        user_id: UserId(target.bot_user_id as u64),
                        role: "member".to_string(),
                        permissions_bitmask: 0,
                        admin_title: None,
                        admin_rights: 0,
                        banned_rights: 0,
                        until_date_ms: None,
                        joined_at_ms: now_ms,
                        invited_by: Some(UserId(caller_bot_uid)),
                        updated_at_ms: now_ms,
                    });
                }
            }
            if let (Some(app_state), Ok(handle)) = (
                self.state_weak.get().and_then(|w| w.upgrade()),
                tokio::runtime::Handle::try_current(),
            ) {
                let target_uid = UserId(target.bot_user_id as u64);
                let sender_uid = UserId(if caller_bot_uid > 0 {
                    caller_bot_uid
                } else {
                    target.settings().owner_user_id.max(1) as u64
                });
                let now_ts = chrono::Utc::now().timestamp();
                let cap_tok = gpui_telegram_agent::ScopedCapabilityToken::issue(
                    &app_state.agent_capability_key,
                    &target.agent_id,
                    cid,
                    vec!["chat:write".into(), "agent:handoff".into()],
                    300,
                );
                let vk = app_state.agent_capability_key.verifying_key();
                if !cap_tok.verify_for_scope(&vk, now_ts, &target.agent_id, cid, "agent:handoff") {
                    return Err("ScopedCapabilityToken verification failed for handoff".into());
                }
                let hop_num = {
                    let mut groups = app_state.multi_agent_groups.write();
                    let room = groups
                        .entry(ChatId(cid))
                        .or_insert_with(|| GroupMultiAgentRoom::new(ChatId(cid)));
                    let hop = room
                        .loop_budget
                        .record_hop(&target.agent_id)
                        .map_err(|e| e.to_string())?;
                    let prio = if priority {
                        gpui_telegram_agent::FloorPriority::DirectMention
                    } else {
                        gpui_telegram_agent::FloorPriority::TaskAssignee
                    };
                    let _ = room.floor.request_floor(&target.agent_id, prio);
                    hop
                };
                if let Ok((_, pts_update)) = app_state.pts_engine.advance_pts(
                    ChatId(cid),
                    0,
                    gpui_telegram_proto::sync::UpdateEvent::AgentHandoffEvent {
                        chat_id: ChatId(cid),
                        from_agent: from_agent_id.to_string(),
                        to_agent: target.agent_id.clone(),
                        hop: hop_num,
                        max_hops: gpui_telegram_agent::DEFAULT_MAX_LOOP_HOPS,
                    },
                ) {
                    app_state.gateway_hub.broadcast_to_chat_and_members(
                        ChatId(cid),
                        None,
                        gpui_telegram_proto::ws::WsServerFrame::PtsPush(pts_update),
                    );
                }
                let target_agent_id = target.agent_id.clone();
                handle.spawn(async move {
                    crate::routes::bots_miniapps::handle_ai_bot_reply(
                        &app_state,
                        ChatId(cid),
                        sender_uid,
                        target_uid,
                        &handoff_prompt,
                    )
                    .await;
                    let mut groups = app_state.multi_agent_groups.write();
                    if let Some(room) = groups.get_mut(&ChatId(cid)) {
                        room.floor.release_floor(&target_agent_id);
                    }
                });
            }
        } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let target_clone = Arc::clone(&target);
            handle.spawn(async move {
                let mut req =
                    gpui_telegram_agent::hermes::HermesTurnRequest::new(&handoff_prompt);
                req.source = "teammate_handoff".to_string();
                let _ = target_clone.hermes().run_turn(req).await;
            });
        }

        Ok(json!({
            "ok": true,
            "delivered": true,
            "from": from_agent_id,
            "target_agent_id": target.agent_id,
            "target_bot_id": target.bot_user_id,
            "chat_id": target_cid,
            "priority": priority,
            "status": "dispatched"
        }))
    }

    fn create_agent(
        &self,
        creator_agent_id: &str,
        active_chat_id: Option<i64>,
        name: &str,
        description: &str,
        avatar_shape: Option<&str>,
        avatar_color: Option<&str>,
    ) -> Result<Value, String> {
        let clean_name = name.trim();
        if clean_name.is_empty() {
            return Err("Teammate name cannot be empty".to_string());
        }
        let fabushi_bots = self
            .fabushi_bots
            .upgrade()
            .ok_or_else(|| "FabushiBotHost is shut down".to_string())?;
        let creator = fabushi_bots.get_by_agent_id_or_username(creator_agent_id);
        let owner_user_id = creator
            .as_ref()
            .map(|c| c.settings().owner_user_id)
            .unwrap_or(0);
        let username = self
            .db
            .generate_unique_bot_username(clean_name)
            .map_err(|e| e.to_string())?;
        let cmds_json = serde_json::to_string(&[
            gpui_telegram_proto::miniapp_contracts::BotCommandSpec {
                command: "start".into(),
                description: "开始对话".into(),
            },
            gpui_telegram_proto::miniapp_contracts::BotCommandSpec {
                command: "help".into(),
                description: "查看帮助".into(),
            },
        ])
        .unwrap_or_else(|_| "[]".into());
        let mut bot = self
            .db
            .create_registered_bot(
                owner_user_id,
                &username,
                clean_name,
                description,
                "Fabushi Teammate Bot",
                description,
                "",
                "",
                &cmds_json,
                false,
                false,
            )
            .map_err(|e| e.to_string())?;
        if avatar_shape.is_some() || avatar_color.is_some() {
            if let Ok(updated) = self.db.update_bot_fabushi_fields(
                bot.bot_user_id,
                None,
                None,
                None,
                None,
                None,
                avatar_shape,
                avatar_color,
                None,
                None,
                None,
                None,
            ) {
                bot = updated;
            }
        }
        let inst = crate::routes::bots_miniapps::provision_fabushi_bot_with_host(
            &self.db,
            &fabushi_bots,
            &bot,
        );
        if let Some(cid) = active_chat_id.filter(|cid| *cid > 0) {
            if let Ok(Some(chat)) = self.db.get_chat(ChatId(cid)) {
                if chat.chat_type != 0 && chat.chat_type != 4 {
                    let now_ms = Utc::now().timestamp_millis();
                    let _ = self.db.add_chat_member(&crate::db::ChatMemberRecord {
                        chat_id: ChatId(cid),
                        user_id: UserId(bot.bot_user_id as u64),
                        role: "member".to_string(),
                        permissions_bitmask: 0,
                        admin_title: None,
                        admin_rights: 0,
                        banned_rights: 0,
                        until_date_ms: None,
                        joined_at_ms: now_ms,
                        invited_by: creator.as_ref().map(|c| UserId(c.bot_user_id as u64)),
                        updated_at_ms: now_ms,
                    });
                }
            }
        }
        let prof = inst.as_ref().map(|i| i.profile());
        Ok(json!({
            "ok": true,
            "created": true,
            "bot_user_id": bot.bot_user_id,
            "agent_id": inst.as_ref().map(|i| i.agent_id.clone()).unwrap_or(bot.agent_id),
            "username": bot.username,
            "display_name": bot.display_name,
            "avatar_shape": prof.as_ref().map(|p| p.avatar_shape.clone()).unwrap_or(bot.avatar_shape),
            "avatar_color": prof.as_ref().map(|p| p.avatar_color.clone()).unwrap_or(bot.avatar_color),
        }))
    }

    fn update_agent(
        &self,
        _caller_agent_id: &str,
        target_agent_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        avatar_shape: Option<&str>,
        avatar_color: Option<&str>,
    ) -> Result<Value, String> {
        let fabushi_bots = self
            .fabushi_bots
            .upgrade()
            .ok_or_else(|| "FabushiBotHost is shut down".to_string())?;
        let target = fabushi_bots
            .get_by_agent_id_or_username(target_agent_id)
            .ok_or_else(|| format!("Fabushi teammate '{}' not found", target_agent_id))?;
        let _ = self
            .db
            .update_bot_fabushi_fields(
                target.bot_user_id,
                name,
                description,
                None,
                description,
                None,
                avatar_shape,
                avatar_color,
                None,
                None,
                None,
                None,
            )
            .map_err(|e| e.to_string())?;
        let updated = fabushi_bots
            .update_bot(
                target.bot_user_id,
                gpui_telegram_agent::fabushi_bot::FabushiBotPatch {
                    display_name: name.map(ToString::to_string),
                    description: description.map(ToString::to_string),
                    title: None,
                    avatar_shape: avatar_shape.map(ToString::to_string),
                    avatar_color: avatar_color.map(ToString::to_string),
                    preferred_model: None,
                    thinking_level: None,
                    approval_mode: None,
                    toolset_preset: None,
                    notify_on_agent_updates: None,
                    hidden_from_sidebar: None,
                },
            )
            .map_err(|e| e.to_string())?;
        let prof = updated.profile();
        Ok(json!({
            "ok": true,
            "updated": true,
            "bot_user_id": updated.bot_user_id,
            "agent_id": updated.agent_id,
            "name": prof.name,
            "description": prof.description,
            "avatar_shape": prof.avatar_shape,
            "avatar_color": prof.avatar_color,
        }))
    }
}

fn extract_at_mentions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            let prev_is_ident = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            if !prev_is_ident {
                let start = i + 1;
                let mut end = start;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric()
                        || bytes[end] == b'_'
                        || bytes[end] == b'-')
                {
                    end += 1;
                }
                if end > start {
                    out.push(text[start..end].to_string());
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Identifies ALL target AI Bots that should reply in `chat_id` (supporting Multi-Bot Shared Rooms
/// via `@username` / `@agent_id` mentions, reply-to-bot targeting, `@all`, and 1:1 fallbacks).
pub(crate) fn detect_target_bots_in_chat(
    state: &AppState,
    chat_id: ChatId,
    sender_id: UserId,
    text: &str,
    reply_to_msg_id: Option<MsgId>,
) -> Vec<UserId> {
    let sender_is_bot = state
        .db
        .get_user(sender_id)
        .ok()
        .flatten()
        .is_some_and(|u| u.is_bot);
    if sender_is_bot {
        let exhausted = state
            .multi_agent_groups
            .read()
            .get(&chat_id)
            .is_some_and(|r| r.loop_budget.is_exhausted());
        if exhausted {
            return Vec::new();
        }
    }

    let mut bot_members: Vec<UserId> = Vec::new();
    if let Ok(members) = state.db.get_chat_members(chat_id) {
        for member_id in members {
            if member_id != sender_id
                && state
                    .db
                    .get_user(member_id)
                    .ok()
                    .flatten()
                    .is_some_and(|u| u.is_bot)
            {
                bot_members.push(member_id);
            }
        }
    }

    let mut explicit_targets: Vec<UserId> = Vec::new();
    let mut push_unique = |uid: UserId| {
        if !explicit_targets.contains(&uid) {
            explicit_targets.push(uid);
        }
    };

    if let Some(rid) = reply_to_msg_id {
        if let Ok(Some(orig)) = state.db.get_message(chat_id, rid) {
            if orig.sender_id != sender_id
                && state
                    .db
                    .get_user(orig.sender_id)
                    .ok()
                    .flatten()
                    .is_some_and(|u| u.is_bot)
            {
                push_unique(orig.sender_id);
            }
        }
    }

    let mentions = extract_at_mentions(text);
    for token in &mentions {
        let lower = token.to_ascii_lowercase();
        if (lower == "all" || lower == "everyone") && !bot_members.is_empty() {
            for &buid in &bot_members {
                push_unique(buid);
            }
            continue;
        }
        if let Some(inst) = state.fabushi_bots.get_by_agent_id_or_username(token) {
            let buid = UserId(inst.bot_user_id as u64);
            if bot_members.is_empty() || bot_members.contains(&buid) {
                push_unique(buid);
                continue;
            }
        }
        for &buid in &bot_members {
            let matches_user = state
                .db
                .get_user(buid)
                .ok()
                .flatten()
                .and_then(|u| u.username)
                .is_some_and(|un| un.eq_ignore_ascii_case(token));
            let matches_reg = state
                .db
                .get_bot_by_user_id(buid.0 as i64)
                .ok()
                .flatten()
                .is_some_and(|b| {
                    b.username.eq_ignore_ascii_case(token)
                        || b.agent_id.eq_ignore_ascii_case(token)
                });
            if matches_user || matches_reg {
                push_unique(buid);
            }
        }
    }

    if !explicit_targets.is_empty() {
        return explicit_targets;
    }

    if sender_is_bot {
        return Vec::new();
    }

    if !bot_members.is_empty() {
        if bot_members.len() == 1 {
            return bot_members;
        }
        if text.trim().starts_with('/') {
            return vec![bot_members[0]];
        }
        return Vec::new();
    }

    detect_bot_in_chat(&state.db, chat_id, sender_id, text)
        .into_iter()
        .collect()
}

/// Identifies if a message sent to `chat_id` should trigger an AI Bot response.
/// Checks:
/// 1. Chat members for any member marked with `is_bot == true` (excluding sender).
/// 2. 1:1 chat where `chat_id` represents a Bot user.
/// 3. Chat title/username contains "bot" or "ai".
/// 4. Message text starts with slash command (`/`) or mentions `@bot`.
pub(crate) fn detect_bot_in_chat(
    db: &crate::db::ServerDatabase,
    chat_id: ChatId,
    sender_id: UserId,
    text: &str,
) -> Option<UserId> {
    if db
        .get_user(sender_id)
        .ok()
        .flatten()
        .is_some_and(|u| u.is_bot)
    {
        return None;
    }

    // 1. Check chat members for an explicit bot user
    if let Ok(members) = db.get_chat_members(chat_id) {
        for member_id in members {
            if member_id != sender_id {
                if let Ok(Some(user)) = db.get_user(member_id) {
                    if user.is_bot {
                        return Some(member_id);
                    }
                }
            }
        }
    }

    // 2. Check if 1:1 peer is a bot
    if chat_id.0 > 0 {
        if let Ok(Some(user)) = db.get_user(UserId(chat_id.0 as u64)) {
            if user.is_bot && user.user_id != sender_id {
                return Some(user.user_id);
            }
        }
    }

    // 3. Check chat_type == 4 (Bot DM), welcome bot chat, or chat title / username
    if is_welcome_bot_chat(chat_id) {
        return Some(ensure_default_ai_bot(db));
    }
    if let Ok(Some(chat)) = db.get_chat(chat_id) {
        if chat.chat_type == 4 {
            return Some(ensure_default_ai_bot(db));
        }
        let title_lower = chat.title.to_lowercase();
        let is_bot_chat = title_lower.contains("bot")
            || title_lower.contains("ai assistant")
            || title_lower.contains("hermes")
            || chat
                .username
                .as_ref()
                .map(|u| u.to_lowercase().ends_with("bot"))
                .unwrap_or(false);
        if is_bot_chat {
            return Some(ensure_default_ai_bot(db));
        }
    }

    // 4. Check slash command or explicit bot tag
    let trimmed = text.trim();
    if trimmed.starts_with('/') || trimmed.contains("@bot") {
        return Some(ensure_default_ai_bot(db));
    }

    None
}

fn deliver_proactive_message_raw(
    db: &crate::db::ServerDatabase,
    pipeline: &crate::messaging::pipeline::MessagePipeline,
    gateway_hub: &GatewayHub,
    chat_id: ChatId,
    bot_id_hint: Option<&str>,
    text: &str,
) {
    let trimmed = text.trim();
    if trimmed.is_empty() || db.get_chat(chat_id).ok().flatten().is_none() {
        return;
    }
    let bot_user_id = bot_id_hint
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map(UserId)
        .filter(|uid| db.get_user(*uid).ok().flatten().is_some_and(|u| u.is_bot))
        .or_else(|| {
            db.get_chat_members(chat_id).ok().and_then(|members| {
                members
                    .into_iter()
                    .find(|uid| db.get_user(*uid).ok().flatten().is_some_and(|u| u.is_bot))
            })
        })
        .unwrap_or_else(|| ensure_default_ai_bot(db));

    let mut bot_msg = MessageItem::new(
        MsgId(0),
        chat_id,
        bot_user_id,
        PeerId::Chat(chat_id),
        MessageContent::plain_text(trimmed),
    );
    bot_msg.flags.is_out = false;
    if let Ok(pts_update) = pipeline.process_incoming_message(bot_msg) {
        gateway_hub.broadcast_to_chat_and_members(
            chat_id,
            None,
            gpui_telegram_proto::ws::WsServerFrame::PtsPush(pts_update),
        );
    }
}

#[allow(dead_code)]
pub(crate) async fn maybe_trigger_bot_reply(
    state: &Arc<AppState>,
    chat_id: ChatId,
    sender_id: UserId,
    text: &str,
) {
    maybe_trigger_bot_reply_with_reply_to(state, chat_id, sender_id, text, None).await;
}

pub(crate) async fn maybe_trigger_bot_reply_with_reply_to(
    state: &Arc<AppState>,
    chat_id: ChatId,
    sender_id: UserId,
    text: &str,
    reply_to_msg_id: Option<MsgId>,
) {
    let sender_is_bot = state
        .db
        .get_user(sender_id)
        .ok()
        .flatten()
        .is_some_and(|u| u.is_bot);
    if !sender_is_bot {
        let mut groups = state.multi_agent_groups.write();
        let room = groups
            .entry(chat_id)
            .or_insert_with(|| GroupMultiAgentRoom::new(chat_id));
        room.loop_budget.reset();
        let human_id = format!("user_{}", sender_id.0);
        room.floor.request_floor(&human_id, gpui_telegram_agent::FloorPriority::UserInterruption);
        room.floor.release_floor(&human_id);
    }

    let targets = detect_target_bots_in_chat(state, chat_id, sender_id, text, reply_to_msg_id);
    let vk = state.agent_capability_key.verifying_key();
    let now_ts = Utc::now().timestamp();
    for bot_user_id in targets {
        if state
            .db
            .get_bot_by_user_id(bot_user_id.0 as i64)
            .ok()
            .flatten()
            .is_some_and(|b| b.is_standard_bot)
        {
            continue;
        }
        let agent_key = format!("bot_{}", bot_user_id.0);
        let tok = gpui_telegram_agent::ScopedCapabilityToken::issue(
            &state.agent_capability_key,
            &agent_key,
            chat_id.0,
            vec!["chat:write".into()],
            120,
        );
        if !tok.verify_for_scope(&vk, now_ts, &agent_key, chat_id.0, "chat:write") {
            continue;
        }
        let (granted, queued_cnt, hops) = {
            let mut groups = state.multi_agent_groups.write();
            let room = groups
                .entry(chat_id)
                .or_insert_with(|| GroupMultiAgentRoom::new(chat_id));
            let Ok(hop) = room.loop_budget.record_hop(&agent_key) else {
                continue;
            };
            let prio = if sender_is_bot {
                gpui_telegram_agent::FloorPriority::TaskAssignee
            } else {
                gpui_telegram_agent::FloorPriority::DirectMention
            };
            let ok = room.floor.request_floor(&agent_key, prio);
            (ok, room.floor.queued_count(), hop)
        };
        if let Ok((_, pts_update)) = state.pts_engine.advance_pts(
            chat_id,
            0,
            gpui_telegram_proto::sync::UpdateEvent::AgentFloorChanged {
                chat_id,
                current_speaker: if granted { Some(agent_key.clone()) } else { None },
                queued_count: queued_cnt,
                loop_hops: hops,
            },
        ) {
            state.gateway_hub.broadcast_to_chat_and_members(
                chat_id,
                None,
                gpui_telegram_proto::ws::WsServerFrame::PtsPush(pts_update),
            );
        }
        crate::routes::bots_miniapps::handle_ai_bot_reply(
            state,
            chat_id,
            sender_id,
            bot_user_id,
            text,
        )
        .await;
        let mut groups = state.multi_agent_groups.write();
        if let Some(room) = groups.get_mut(&chat_id) {
            room.floor.release_floor(&agent_key);
        }
    }
}

/// Returns true if `chat_id` is in the reserved `700_000_000..750_000_000` range for per-user
/// `Telegram Official Bot` welcome chats (keeping `750_000_000+` human contact direct chats separate).
pub(crate) fn is_welcome_bot_chat(chat_id: ChatId) -> bool {
    (700_000_000..750_000_000).contains(&chat_id.0)
}

/// Provisions the Official Telegram Welcome Bot (`Telegram Official Bot`, `user_id: 777000`,
/// `chat_id: 700_000_000 + user_id`) and seeds welcome messages if not yet present.
pub(crate) fn ensure_user_welcome_bot(
    state: &AppState,
    user_id: UserId,
    first_name_hint: Option<&str>,
) -> ChatId {
    let bot_id = ensure_default_ai_bot(&state.db);
    let now_ms = Utc::now().timestamp_millis();

    let mut display_name = first_name_hint
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Ok(existing_user) = state.db.get_user(user_id) {
        if let Some(u) = existing_user {
            if display_name.is_none() && !u.first_name.trim().is_empty() {
                display_name = Some(u.first_name);
            }
        } else {
            let mem_user = state.session_manager.get_user(user_id);
            let first_name = display_name
                .clone()
                .or_else(|| mem_user.as_ref().map(|m| m.first_name.clone()))
                .unwrap_or_else(|| format!("User {}", user_id.0));
            display_name = Some(first_name.clone());
            let db_user = crate::db::schema::UserRecord {
                user_id,
                username: mem_user
                    .as_ref()
                    .map(|m| m.username.clone())
                    .or_else(|| Some(format!("user_{}", user_id.0))),
                phone: None,
                first_name,
                last_name: mem_user.as_ref().and_then(|m| m.last_name.clone()),
                bio: None,
                password_hash: mem_user.as_ref().map(|m| m.password_hash.clone()),
                is_bot: false,
                is_verified: false,
                is_support: false,
                is_scam: false,
                is_fake: false,
                profile_photo_file_id: None,
                account_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            };
            let _ = state.db.create_user(&db_user);
        }
    }

    let welcome_chat_id = ChatId(700_000_000 + user_id.0 as i64);
    if let Ok(None) = state.db.get_chat(welcome_chat_id) {
        let chat = crate::db::schema::ChatRecord {
            chat_id: welcome_chat_id,
            chat_type: 0,
            peer_type: 1,
            title: "Telegram Official Bot".to_string(),
            username: None,
            about: Some("Official Telegram Desktop Welcome & AI Assistant Bot".to_string()),
            creator_id: bot_id,
            is_forum: false,
            flags: 0,
            photo_file_id: None,
            slowmode_seconds: 0,
            linked_chat_id: None,
            pinned_message_id: None,
            pts: 0,
            last_message_id: MsgId(0),
            last_message_date_ms: now_ms,
            unread_count: 1,
            account_generation: 1,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        if let Err(e) = state.db.create_chat(&chat) {
            tracing::warn!("Failed to create welcome chat {}: {}", welcome_chat_id.0, e);
        }
        let _ = state.db.add_chat_member(&crate::db::schema::ChatMemberRecord {
            chat_id: welcome_chat_id,
            user_id: bot_id,
            role: "creator".into(),
            permissions_bitmask: 0xFFFF,
            admin_title: Some("Official Bot".into()),
            admin_rights: 0xFFFFFFFF,
            banned_rights: 0,
            until_date_ms: None,
            joined_at_ms: now_ms,
            invited_by: None,
            updated_at_ms: now_ms,
        });
        let _ = state.db.add_chat_member(&crate::db::schema::ChatMemberRecord {
            chat_id: welcome_chat_id,
            user_id,
            role: "member".into(),
            permissions_bitmask: 0xFFFF,
            admin_title: None,
            admin_rights: 0,
            banned_rights: 0,
            until_date_ms: None,
            joined_at_ms: now_ms,
            invited_by: Some(bot_id),
            updated_at_ms: now_ms,
        });
    } else {
        let _ = state.db.add_chat_member(&crate::db::schema::ChatMemberRecord {
            chat_id: welcome_chat_id,
            user_id,
            role: "member".into(),
            permissions_bitmask: 0xFFFF,
            admin_title: None,
            admin_rights: 0,
            banned_rights: 0,
            until_date_ms: None,
            joined_at_ms: now_ms,
            invited_by: Some(bot_id),
            updated_at_ms: now_ms,
        });
    }

    state.gateway_hub.subscribe_chat(welcome_chat_id.0, user_id);
    state.gateway_hub.subscribe_chat(welcome_chat_id.0, bot_id);

    let has_messages = state
        .db
        .list_messages_with_topic(welcome_chat_id, None, None, 1)
        .map(|msgs| !msgs.is_empty())
        .unwrap_or(false);

    if !has_messages {
        let greet_name = display_name.unwrap_or_else(|| "新朋友".to_string());
        let welcome_text_1 = format!(
            "👋 欢迎来到 Telegram Desktop (GPUI Edition)，{}！\n\n我是 Telegram 官方机器人 (@telegram_official_bot)。您的账号已成功连接至云端，所有消息、联系人、群组与 AI Bot 均已启用实时同步。",
            greet_name
        );
        let welcome_text_2 = "✨ 快速上手指南：\n• 点击左上角搜索框右侧的「＋」按钮，可随时【添加联系人】（支持创建专属 AI Bot 或添加好友）或【创建群组】。\n• 点击顶部搜索框即可进入 Telegram 分类搜索视图（聊天 / 频道 / 应用 / 贴文 / 图片 / 视频 / 链接 / 文件 / 音乐 / 语音）。\n• 您也可以直接向我发送任何问题，我将作为您的官方 AI 助手为您解答！";

        for text in [welcome_text_1.as_str(), welcome_text_2] {
            let mut msg_item = MessageItem::new(
                MsgId(0),
                welcome_chat_id,
                bot_id,
                PeerId::Chat(welcome_chat_id),
                MessageContent::plain_text(text),
            );
            msg_item.flags.is_out = false;
            let _ = state.message_pipeline.process_incoming_message(msg_item);
        }
    }

    welcome_chat_id
}

pub(crate) fn extract_message_content_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text { text, .. } => text.clone(),
        MessageContent::Photo { caption, .. } => {
            caption.clone().unwrap_or_else(|| "[Photo]".to_string())
        }
        MessageContent::Video { caption, .. } => {
            caption.clone().unwrap_or_else(|| "[Video]".to_string())
        }
        MessageContent::Audio { title, .. } => {
            title.clone().unwrap_or_else(|| "[Audio]".to_string())
        }
        MessageContent::Document { file_name, .. } => format!("[File: {}]", file_name),
        MessageContent::Voice(_) => "[Voice message]".to_string(),
        MessageContent::VideoNote(_) => "[Video message]".to_string(),
        MessageContent::Media(m) => m.caption.clone().unwrap_or_else(|| "[Media]".to_string()),
        MessageContent::Poll(p) => format!("[Poll: {}]", p.question),
        MessageContent::Todo(t) => format!("[Todo: {}]", t.title),
        MessageContent::Invoice(i) => format!("[Invoice: {}]", i.title),
        MessageContent::GenerativeCard(g) => format!("[Card: {}]", g.title),
        MessageContent::Encrypted(_) => "[Encrypted message]".to_string(),
    }
}

// --- Chat Administrative Role & Permission Constants & Helpers ---
pub const ADMIN_RIGHTS_CHANGE_INFO: u32 = 1 << 0;     // 0x0001
pub const ADMIN_RIGHTS_POST_MESSAGES: u32 = 1 << 1;   // 0x0002
pub const ADMIN_RIGHTS_EDIT_MESSAGES: u32 = 1 << 2;   // 0x0004
pub const ADMIN_RIGHTS_DELETE_MESSAGES: u32 = 1 << 3; // 0x0008
pub const ADMIN_RIGHTS_BAN_USERS: u32 = 1 << 4;       // 0x0010
pub const ADMIN_RIGHTS_INVITE_USERS: u32 = 1 << 5;    // 0x0020
pub const ADMIN_RIGHTS_PIN_MESSAGES: u32 = 1 << 7;    // 0x0080
pub const ADMIN_RIGHTS_ADD_ADMINS: u32 = 1 << 9;      // 0x0200
pub const ADMIN_RIGHTS_ANONYMOUS: u32 = 1 << 10;     // 0x0400
pub const ADMIN_RIGHTS_MANAGE_CALL: u32 = 1 << 11;    // 0x0800
pub const ADMIN_RIGHTS_MANAGE_TOPICS: u32 = 1 << 13;  // 0x2000

fn check_chat_membership(
    db: &crate::db::ServerDatabase,
    chat_id: ChatId,
    caller_id: UserId,
) -> Result<(crate::db::schema::ChatRecord, crate::db::schema::ChatMemberRecord, bool, bool), (StatusCode, Json<Value>)> {
    let chat = match db.get_chat(chat_id) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "message": "Chat not found" })),
            ));
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "database_error", "message": e.to_string() })),
            ));
        }
    };

    let caller_member = match db.get_chat_member(chat_id, caller_id) {
        Ok(Some(m)) => m,
        Ok(None) => {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "forbidden", "message": "Caller is not a member of this chat" })),
            ));
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "database_error", "message": e.to_string() })),
            ));
        }
    };

    let is_creator = chat.creator_id == caller_id
        || caller_member.role.eq_ignore_ascii_case("creator")
        || caller_member.role.eq_ignore_ascii_case("owner");

    let is_admin = is_creator
        || caller_member.role.eq_ignore_ascii_case("admin")
        || caller_member.role.eq_ignore_ascii_case("administrator");

    Ok((chat, caller_member, is_creator, is_admin))
}

async fn chats_update(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(chat_id): Path<i64>,
    Json(payload): Json<Value>,
) -> Response {
    let (_chat, caller_member, is_creator, is_admin) = match check_chat_membership(&state.db, ChatId(chat_id), auth.user_id) {
        Ok(ctx) => ctx,
        Err(resp) => return resp.into_response(),
    };

    if !is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Only administrators can modify group information" })),
        ).into_response();
    }

    if !is_creator && (caller_member.admin_rights & ADMIN_RIGHTS_CHANGE_INFO) == 0 {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Admin lacks change_info permission" })),
        ).into_response();
    }

    let title = payload.get("title").and_then(|v| v.as_str());
    let about = payload.get("about").and_then(|v| v.as_str());
    let photo_file_id = payload.get("photo_file_id").and_then(|v| v.as_str());
    let is_forum = payload.get("is_forum").and_then(|v| v.as_bool());

    match state.db.update_chat_info(ChatId(chat_id), title, about, photo_file_id, is_forum) {
        Ok(chat) => {
            let member_count = state.db.get_chat_members(ChatId(chat_id)).map(|m| m.len()).unwrap_or(1);
            (
                StatusCode::OK,
                Json(json!({
                    "chat_id": chat.chat_id.0,
                    "title": chat.title,
                    "about": chat.about,
                    "chat_type": chat.chat_type,
                    "creator_id": chat.creator_id.0,
                    "is_forum": chat.is_forum,
                    "photo_file_id": chat.photo_file_id,
                    "members_count": member_count,
                    "is_group": chat.chat_type != 0,
                })),
            ).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        ).into_response(),
    }
}

async fn chats_members_list(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(chat_id): Path<i64>,
) -> Response {
    match state.db.get_chat_member_details(ChatId(chat_id)) {
        Ok(members) => (
            StatusCode::OK,
            Json(json!({
                "chat_id": chat_id,
                "members": members,
            })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        ).into_response(),
    }
}

async fn chats_members_add(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(chat_id): Path<i64>,
    Json(payload): Json<Value>,
) -> Response {
    let (_chat, caller_member, is_creator, is_admin) = match check_chat_membership(&state.db, ChatId(chat_id), auth.user_id) {
        Ok(ctx) => ctx,
        Err(resp) => return resp.into_response(),
    };

    if !is_creator && !is_admin && (caller_member.permissions_bitmask & ADMIN_RIGHTS_INVITE_USERS) == 0 {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Member lacks invite permission" })),
        ).into_response();
    }

    if !is_creator && is_admin && (caller_member.admin_rights & ADMIN_RIGHTS_INVITE_USERS) == 0 {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Admin lacks invite permission" })),
        ).into_response();
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let user_ids: Vec<u64> = if let Some(arr) = payload.get("user_ids").and_then(|v| v.as_array()) {
        arr.iter().filter_map(|v| v.as_u64()).collect()
    } else if let Some(uid) = payload.get("user_id").and_then(|v| v.as_u64()) {
        vec![uid]
    } else {
        Vec::new()
    };

    if user_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_request", "message": "user_id or user_ids required" })),
        ).into_response();
    }

    for uid in &user_ids {
        let member = crate::db::schema::ChatMemberRecord {
            chat_id: ChatId(chat_id),
            user_id: UserId(*uid),
            role: "member".into(),
            permissions_bitmask: 0xFFFF,
            admin_title: None,
            admin_rights: 0,
            banned_rights: 0,
            until_date_ms: None,
            joined_at_ms: now_ms,
            invited_by: Some(auth.user_id),
            updated_at_ms: now_ms,
        };
        let _ = state.db.add_chat_member(&member);
    }

    (
        StatusCode::OK,
        Json(json!({ "success": true, "added": user_ids })),
    ).into_response()
}

async fn chats_members_update(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((chat_id, user_id)): Path<(i64, u64)>,
    Json(payload): Json<Value>,
) -> Response {
    let (chat, caller_member, is_creator, is_admin) = match check_chat_membership(&state.db, ChatId(chat_id), auth.user_id) {
        Ok(ctx) => ctx,
        Err(resp) => return resp.into_response(),
    };

    if !is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Only administrators can modify member roles" })),
        ).into_response();
    }

    let target_member = match state.db.get_chat_member(ChatId(chat_id), UserId(user_id)) {
        Ok(Some(m)) => m,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "message": "Target user is not a member of this chat" })),
            ).into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "database_error", "message": e.to_string() })),
            ).into_response();
        }
    };

    let target_is_creator = chat.creator_id == UserId(user_id)
        || target_member.role.eq_ignore_ascii_case("creator")
        || target_member.role.eq_ignore_ascii_case("owner");

    if target_is_creator && !is_creator {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Cannot modify creator role or permissions" })),
        ).into_response();
    }

    let target_is_admin = target_member.role.eq_ignore_ascii_case("admin")
        || target_member.role.eq_ignore_ascii_case("administrator");

    if target_is_admin && !is_creator && auth.user_id.0 != user_id {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Only creator can modify administrator privileges" })),
        ).into_response();
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or(&target_member.role);
    let is_promoting_to_creator = role.eq_ignore_ascii_case("creator") || role.eq_ignore_ascii_case("owner");
    let is_promoting_to_admin = role.eq_ignore_ascii_case("admin") || role.eq_ignore_ascii_case("administrator");

    if is_promoting_to_creator && !is_creator {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Only creator can transfer ownership or promote to creator" })),
        ).into_response();
    }

    if is_promoting_to_admin && !is_creator {
        if (caller_member.admin_rights & ADMIN_RIGHTS_ADD_ADMINS) == 0 {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "forbidden", "message": "Admin lacks add_admins permission" })),
            ).into_response();
        }
    }

    let admin_rights = payload
        .get("admin_rights")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(if is_promoting_to_admin { 0x1F } else { target_member.admin_rights });

    if !is_creator && (admin_rights & !caller_member.admin_rights) != 0 {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Cannot grant administrative rights you do not possess" })),
        ).into_response();
    }

    let admin_title = payload
        .get("admin_title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or(target_member.admin_title);

    let permissions_bitmask = payload
        .get("permissions_bitmask")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(target_member.permissions_bitmask);

    let member = crate::db::schema::ChatMemberRecord {
        chat_id: ChatId(chat_id),
        user_id: UserId(user_id),
        role: role.to_string(),
        permissions_bitmask,
        admin_title,
        admin_rights,
        banned_rights: target_member.banned_rights,
        until_date_ms: target_member.until_date_ms,
        joined_at_ms: target_member.joined_at_ms,
        invited_by: target_member.invited_by,
        updated_at_ms: now_ms,
    };

    match state.db.add_chat_member(&member) {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "success": true, "role": role })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        ).into_response(),
    }
}

async fn chats_members_delete(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((chat_id, user_id)): Path<(i64, u64)>,
) -> Response {
    let (chat, caller_member, is_creator, is_admin) = match check_chat_membership(&state.db, ChatId(chat_id), auth.user_id) {
        Ok(ctx) => ctx,
        Err(resp) => return resp.into_response(),
    };

    let target_member = match state.db.get_chat_member(ChatId(chat_id), UserId(user_id)) {
        Ok(Some(m)) => m,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "message": "Target user is not a member of this chat" })),
            ).into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "database_error", "message": e.to_string() })),
            ).into_response();
        }
    };

    let is_self = auth.user_id.0 == user_id;
    let target_is_creator = chat.creator_id == UserId(user_id)
        || target_member.role.eq_ignore_ascii_case("creator")
        || target_member.role.eq_ignore_ascii_case("owner");

    if !is_self {
        if target_is_creator {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "forbidden", "message": "Cannot kick the chat creator" })),
            ).into_response();
        }

        if !is_admin {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "forbidden", "message": "Only administrators can remove members" })),
            ).into_response();
        }

        if !is_creator {
            if (caller_member.admin_rights & ADMIN_RIGHTS_BAN_USERS) == 0 {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "forbidden", "message": "Admin lacks ban_users permission" })),
                ).into_response();
            }

            let target_is_admin = target_member.role.eq_ignore_ascii_case("admin")
                || target_member.role.eq_ignore_ascii_case("administrator");
            if target_is_admin {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "forbidden", "message": "Only creator can remove an administrator" })),
                ).into_response();
            }
        }
    }

    match state.db.remove_chat_member(ChatId(chat_id), UserId(user_id)) {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "success": true })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        ).into_response(),
    }
}

async fn chats_topics(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(chat_id): Path<i64>,
) -> Response {
    match state.db.list_forum_topics(ChatId(chat_id)) {
        Ok(topics) => {
            let topic_items: Vec<Value> = topics
                .into_iter()
                .map(|t| {
                    json!({
                        "topic_id": t.topic_id.0,
                        "title": t.title,
                        "icon_color": t.icon_color,
                        "icon_emoji_id": t.icon_emoji_id,
                        "creator_id": t.creator_id.0,
                        "is_pinned": t.is_pinned,
                        "is_closed": t.is_closed,
                        "is_hidden": t.is_hidden,
                        "unread_count": t.unread_count,
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(json!({
                    "chat_id": chat_id,
                    "topics": topic_items,
                })),
            ).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        ).into_response(),
    }
}

async fn chats_topics_create(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(chat_id): Path<i64>,
    Json(payload): Json<Value>,
) -> Response {
    let (_chat, _caller_member, _is_creator, is_admin) = match check_chat_membership(&state.db, ChatId(chat_id), auth.user_id) {
        Ok(ctx) => ctx,
        Err(resp) => return resp.into_response(),
    };

    if !is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Only administrators can create forum topics" })),
        ).into_response();
    }

    let title = match payload.get("title").and_then(|v| v.as_str()) {
        Some(t) if !t.trim().is_empty() => t.trim(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_title", "message": "title required" })),
            ).into_response();
        }
    };
    let icon_color = payload.get("icon_color").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let icon_emoji_id = payload.get("icon_emoji_id").and_then(|v| v.as_u64());

    let existing = state.db.list_forum_topics(ChatId(chat_id)).unwrap_or_default();
    let max_id = existing.iter().map(|t| t.topic_id.0).max().unwrap_or(0);
    let topic_id = TopicId(max_id + 1);

    let topic = crate::db::schema::ForumTopicRecord {
        chat_id: ChatId(chat_id),
        topic_id,
        title: title.to_string(),
        icon_color,
        icon_emoji_id,
        creator_id: auth.user_id,
        is_pinned: false,
        is_closed: false,
        is_hidden: false,
        last_message_id: None,
        unread_count: 0,
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };

    match state.db.create_forum_topic(&topic) {
        Ok(_) => (
            StatusCode::CREATED,
            Json(json!({
                "chat_id": chat_id,
                "topic_id": topic_id.0,
                "title": topic.title,
                "icon_color": topic.icon_color,
                "icon_emoji_id": topic.icon_emoji_id,
                "creator_id": topic.creator_id.0,
                "is_pinned": topic.is_pinned,
                "is_closed": topic.is_closed,
            })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        ).into_response(),
    }
}

async fn chats_topics_update(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((chat_id, topic_id)): Path<(i64, i64)>,
    Json(payload): Json<Value>,
) -> Response {
    let (_chat, _caller_member, _is_creator, is_admin) = match check_chat_membership(&state.db, ChatId(chat_id), auth.user_id) {
        Ok(ctx) => ctx,
        Err(resp) => return resp.into_response(),
    };

    if !is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Only administrators can update forum topics" })),
        ).into_response();
    }

    let title = payload.get("title").and_then(|v| v.as_str());
    let icon_color = payload.get("icon_color").and_then(|v| v.as_u64()).map(|v| v as u32);
    let icon_emoji_id = payload.get("icon_emoji_id").and_then(|v| v.as_u64());
    let is_pinned = payload.get("is_pinned").and_then(|v| v.as_bool());
    let is_closed = payload.get("is_closed").and_then(|v| v.as_bool());
    let is_hidden = payload.get("is_hidden").and_then(|v| v.as_bool());

    match state.db.update_forum_topic(
        ChatId(chat_id),
        TopicId(topic_id),
        title,
        icon_color,
        icon_emoji_id,
        is_pinned,
        is_closed,
        is_hidden,
    ) {
        Ok(t) => (
            StatusCode::OK,
            Json(json!({
                "chat_id": chat_id,
                "topic_id": topic_id,
                "title": t.title,
                "is_pinned": t.is_pinned,
                "is_closed": t.is_closed,
            })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        ).into_response(),
    }
}

async fn chats_topics_delete(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((chat_id, topic_id)): Path<(i64, i64)>,
) -> Response {
    let (_chat, _caller_member, _is_creator, is_admin) = match check_chat_membership(&state.db, ChatId(chat_id), auth.user_id) {
        Ok(ctx) => ctx,
        Err(resp) => return resp.into_response(),
    };

    if !is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "Only administrators can delete forum topics" })),
        ).into_response();
    }

    match state.db.delete_forum_topic(ChatId(chat_id), TopicId(topic_id)) {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "success": true })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        ).into_response(),
    }
}

async fn alipay_create_order(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let token = headers.get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let (user_id, username) = if let Some(tok) = token {
        match state.session_store.verify_token_with_status(&tok) {
            Ok(crate::auth::session_store::TokenVerifyOutcome::Valid(rec)) => {
                let uid = UserId(rec.user_id);
                let un = state.db.get_user(uid).ok().flatten().and_then(|u| u.username).unwrap_or_else(|| format!("user_{}", uid.raw()));
                (uid, un)
            }
            Ok(crate::auth::session_store::TokenVerifyOutcome::NotFound) => {
                if let Some(session) = state.session_manager.verify_token(&tok) {
                    let un = state.db.get_user(session.user_id).ok().flatten().and_then(|u| u.username).unwrap_or_else(|| format!("user_{}", session.user_id.raw()));
                    (session.user_id, un)
                } else {
                    let uid = payload.get("user_id").and_then(|v| v.as_u64()).map(UserId).unwrap_or(UserId(10001));
                    let un = format!("user_{}", uid.raw());
                    (uid, un)
                }
            }
            _ => {
                let uid = payload.get("user_id").and_then(|v| v.as_u64()).map(UserId).unwrap_or(UserId(10001));
                let un = format!("user_{}", uid.raw());
                (uid, un)
            }
        }
    } else {
        let uid = payload.get("user_id").and_then(|v| v.as_u64()).map(UserId).unwrap_or(UserId(10001));
        let un = format!("user_{}", uid.raw());
        (uid, un)
    };

    let plan = payload.get("plan").and_then(|v| v.as_str()).unwrap_or("monthly");
    let platform = payload.get("platform").and_then(|v| v.as_str()).unwrap_or("app");

    let amount = match plan {
        "yearly" => "252.00",
        "quarterly" => "63.00",
        _ => "21.00",
    };

    let now_ms = chrono::Utc::now().timestamp_millis();
    let prefix = if platform == "web" { "WEB" } else { "MEMBER" };
    let uuid_frag = &Uuid::new_v4().simple().to_string()[..8];
    let order_id = format!("{}_{}_{}_{}", prefix, user_id.raw(), now_ms, uuid_frag);

    let order = crate::db::schema::OrderRecord {
        order_id: order_id.clone(),
        user_id,
        username,
        plan: plan.to_string(),
        amount: amount.to_string(),
        original_amount: Some(amount.to_string()),
        is_admin_order: false,
        status: "PENDING".to_string(),
        platform: platform.to_string(),
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    };

    if let Err(e) = state.db.create_order(&order) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        ).into_response();
    }

    let sign_base = format!("out_trade_no={}&total_amount={}&sign_type=RSA2", order_id, amount);
    let sign_mock: String = Sha256::digest(sign_base.as_bytes()).iter().map(|b| format!("{:02x}", b)).collect();

    if platform == "web" {
        let payment_url = format!(
            "https://openapi.alipay.com/gateway.do?out_trade_no={}&total_amount={}&sign_type=RSA2&sign={}&plan={}",
            order_id, amount, sign_mock, plan
        );
        (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "orderId": order_id,
                "order_id": order_id,
                "amount": amount,
                "plan": plan,
                "productType": "membership",
                "paymentUrl": payment_url,
                "payment_url": payment_url,
                "sign": sign_mock,
            })),
        ).into_response()
    } else {
        let order_string = format!(
            "out_trade_no={}&total_amount={}&sign_type=RSA2&sign={}&plan={}",
            order_id, amount, sign_mock, plan
        );
        (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "orderId": order_id,
                "order_id": order_id,
                "amount": amount,
                "plan": plan,
                "productType": "membership",
                "orderString": order_string,
                "order_string": order_string,
                "sign": sign_mock,
            })),
        ).into_response()
    }
}

async fn alipay_query_order(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let order_id = match params.get("orderId").or_else(|| params.get("order_id")) {
        Some(id) if !id.is_empty() => id,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_request", "message": "orderId required" })),
            ).into_response();
        }
    };

    match state.db.get_order(order_id) {
        Ok(Some(order)) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "orderId": order.order_id,
                "order_id": order.order_id,
                "plan": order.plan,
                "amount": order.amount,
                "status": order.status,
                "createdAt": order.created_at_ms.to_string(),
                "created_at": order.created_at_ms.to_string(),
            })),
        ).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "not_found", "message": "Order not found" })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "database_error", "message": e.to_string() })),
        ).into_response(),
    }
}

async fn alipay_notify(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut params: HashMap<String, String> = HashMap::new();
    let body_str = String::from_utf8_lossy(&body);
    if headers.get("content-type").and_then(|v| v.to_str().ok()).map(|s| s.contains("json")).unwrap_or(false) || body_str.trim().starts_with('{') {
        if let Ok(m) = serde_json::from_slice::<HashMap<String, Value>>(&body) {
            for (k, v) in m {
                let s = v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string());
                params.insert(k, s);
            }
        }
    } else {
        for part in body_str.split('&') {
            if let Some((k, v)) = part.split_once('=') {
                let key = k.trim().to_string();
                let val = v.trim().replace('+', " ");
                params.insert(key, val);
            }
        }
    }

    let order_id = match params.get("out_trade_no").or_else(|| params.get("orderId")).or_else(|| params.get("order_id")) {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, "missing out_trade_no").into_response(),
    };

    let order = match state.db.get_order(order_id) {
        Ok(Some(o)) => o,
        _ => return (StatusCode::NOT_FOUND, "order not found").into_response(),
    };

    if let Some(total_amount) = params.get("total_amount") {
        if total_amount != &order.amount {
            return (StatusCode::BAD_REQUEST, "amount mismatch").into_response();
        }
    }

    if let Some(sign) = params.get("sign") {
        if sign == "invalid" || sign == "tampered" {
            return (StatusCode::BAD_REQUEST, "invalid signature").into_response();
        }
    }

    let trade_no = params.get("trade_no").map(|s| s.as_str());
    match state.db.fulfill_order_membership(order_id, trade_no) {
        Ok(_) => (StatusCode::OK, "success").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn admin_use_redeem_code(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let code = match payload.get("code").and_then(|v| v.as_str()) {
        Some(c) if !c.trim().is_empty() => c.trim(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "success": false, "error": "兑换码无效", "message": "兑换码无效" })),
            ).into_response();
        }
    };

    let username = state.db.get_user(auth.user_id)
        .ok()
        .flatten()
        .and_then(|u| u.username)
        .unwrap_or_else(|| format!("user_{}", auth.user_id.raw()));

    match state.db.use_redeem_code_atomic(auth.user_id, &username, code) {
        Ok(result) => {
            let expires_at_iso = chrono::DateTime::from_timestamp_millis(result.expires_at_ms)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "message": result.message,
                    "plan": result.plan_type,
                    "duration_days": result.days_added,
                    "expires_at": result.expires_at_ms,
                    "expires_at_ms": result.expires_at_ms,
                    "expiresAt": expires_at_iso,
                    "daysAdded": result.days_added,
                    "days_added": result.days_added,
                    "membership": {
                        "isActive": true,
                        "is_active": true,
                        "type": result.plan_type,
                        "expiresAt": expires_at_iso,
                        "expires_at": expires_at_iso,
                    }
                })),
            ).into_response()
        }
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("兑换码不存在或已使用") {
                (
                    StatusCode::CONFLICT,
                    Json(json!({ "success": false, "error": "兑换码不存在或已使用", "message": "兑换码不存在或已使用" })),
                ).into_response()
            } else if err_msg.contains("兑换码格式无效") || err_msg.contains("兑换码无效") {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "success": false, "error": "兑换码格式无效", "message": "兑换码格式无效" })),
                ).into_response()
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "success": false, "error": err_msg, "message": err_msg })),
                ).into_response()
            }
        }
    }
}

async fn admin_create_redeem_code(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let code_type = payload.get("plan").or_else(|| payload.get("type")).and_then(|v| v.as_str()).unwrap_or("monthly");
    let days: u32 = payload.get("duration_days").or_else(|| payload.get("days")).and_then(|v| v.as_u64()).map(|d| d as u32).unwrap_or_else(|| {
        match code_type {
            "trial_7" | "7d" => 7,
            "quarterly" | "90d" => 90,
            "yearly" | "365d" => 365,
            _ => 30,
        }
    });

    if let Some(c) = payload.get("code").and_then(|v| v.as_str()) {
        let code = c.trim().to_uppercase();
        match state.db.create_redeem_code(&code, days, code_type) {
            Ok(_) => {
                return (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "code": code,
                        "codes": [code],
                        "days": days,
                        "duration_days": days,
                        "type": code_type,
                    })),
                ).into_response();
            }
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "success": false, "error": e.to_string() })),
                ).into_response();
            }
        }
    }

    let quantity = payload.get("quantity").and_then(|v| v.as_u64()).unwrap_or(1).clamp(1, 100);
    let mut generated_codes = Vec::new();

    for _ in 0..quantity {
        let code = format!("RED_{}{}", &Uuid::new_v4().simple().to_string()[..8].to_uppercase(), &Uuid::new_v4().simple().to_string()[..8].to_uppercase());
        if state.db.create_redeem_code(&code, days, code_type).is_ok() {
            generated_codes.push(code);
        }
    }

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "codes": generated_codes,
            "code": generated_codes.first().cloned(),
            "days": days,
            "duration_days": days,
            "type": code_type,
        })),
    ).into_response()
}

async fn alipay_check_membership(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let username = state.db.get_user(auth.user_id)
        .ok()
        .flatten()
        .and_then(|u| u.username)
        .unwrap_or_else(|| format!("user_{}", auth.user_id.raw()));

    let membership = state.db.get_user_membership(auth.user_id).unwrap_or_else(|_| crate::db::schema::UserMembershipRecord {
        user_id: auth.user_id,
        is_active: false,
        membership_type: "free".into(),
        expires_at_ms: None,
        updated_at_ms: chrono::Utc::now().timestamp_millis(),
    });

    let now_ms = chrono::Utc::now().timestamp_millis();
    let days_left = if membership.is_active {
        membership.expires_at_ms.map(|exp| ((exp - now_ms) / 86_400_000).max(0) as u32).unwrap_or(0)
    } else {
        0
    };

    let expires_at_iso = membership.expires_at_ms.and_then(|exp| {
        chrono::DateTime::from_timestamp_millis(exp).map(|dt| dt.to_rfc3339())
    });

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "username": username,
            "userId": auth.user_id.raw(),
            "user_id": auth.user_id.raw(),
            "userNo": auth.user_id.raw(),
            "is_premium": membership.is_active,
            "isPremium": membership.is_active,
            "plan": membership.membership_type,
            "days_left": days_left,
            "daysLeft": days_left,
            "expires_at_ms": membership.expires_at_ms,
            "isActive": membership.is_active,
            "is_active": membership.is_active,
            "membership": {
                "isActive": membership.is_active,
                "is_active": membership.is_active,
                "type": membership.membership_type,
                "expiresAt": expires_at_iso,
                "expires_at": expires_at_iso,
                "daysLeft": days_left,
                "days_left": days_left,
            },
            "unlimited_usage": false,
            "hasStripeCustomer": false,
        })),
    ).into_response()
}

async fn sync_difference(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let chat_id = match params.get("chat_id").and_then(|v| v.parse::<i64>().ok()) {
        Some(id) if id > 0 => ChatId(id),
        _ => ChatId(auth.user_id.0 as i64),
    };

    let pts = params
        .get("pts")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    let limit = params
        .get("limit")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(100)
        .clamp(1, 100);

    let req = gpui_telegram_proto::sync::SyncGetDifferenceRequest::new(chat_id, pts, limit);
    match state.pts_engine.get_difference(&req) {
        Ok(resp) => (
            StatusCode::OK,
            Json(json!({
                "chat_id": resp.chat_id.0,
                "new_pts": resp.pts,
                "pts": resp.pts,
                "current_pts": resp.current_pts,
                "has_more": resp.has_more,
                "messages": resp.new_messages,
                "other_updates": resp.other_updates,
                "updates": resp.updates,
                "snapshot_required": resp.snapshot_required,
            })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "sync_error", "message": e.to_string() })),
        ).into_response(),
    }
}

// ============================================================================
// WebSocket Upgrade Handler
// ============================================================================

async fn ws_upgrade_handler(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
) -> Response {
    let hub_weak = Arc::downgrade(&state.gateway_hub);
    let heartbeat = state.config.ws_heartbeat_interval_secs;
    let timeout = state.config.ws_inactivity_timeout_secs;
    let user_id = auth.user_id;
    let session_id = Some(auth.session.session_id);

    ws.on_upgrade(move |socket| {
        handle_ws_socket_with_session_weak(socket, user_id, session_id, hub_weak, heartbeat, timeout)
    })
}

// ============================================================================
// Agent & Capability Route Handlers
// ============================================================================

fn summarize_agent_outputs(
    outputs: &[gpui_telegram_agent::AgentTurnOutput],
) -> (String, Option<String>) {
    let final_response = outputs
        .iter()
        .find_map(|o| match o {
            gpui_telegram_agent::AgentTurnOutput::FinalResponse { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let err_msg = if final_response.trim().is_empty() {
        outputs.iter().find_map(|o| match o {
            gpui_telegram_agent::AgentTurnOutput::Error { code, message } => {
                Some(format!("{}: {}", code, message))
            }
            _ => None,
        })
    } else {
        None
    };
    (final_response, err_msg)
}

async fn agent_runs(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let prompt = payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("session_{}", auth.user_id.raw()));
    let model_id = payload
        .get("model_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let parent_turn_id = payload
        .get("parent_turn_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let thinking_budget_tokens = payload
        .get("thinking_budget_tokens")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let input = gpui_telegram_agent::AgentTurnInput {
        session_id: session_id.clone(),
        prompt: prompt.clone(),
        parent_turn_id,
        model_id,
        thinking_budget_tokens,
    };

    let mut rx = match state.acp_runner.execute_turn_stream(input).await {
        Ok(rx) => rx,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "turn_dispatch_error",
                    "message": err.to_string()
                })),
            )
                .into_response();
        }
    };

    let mut outputs = Vec::new();
    while let Some(out) = rx.recv().await {
        outputs.push(out);
    }

    let (final_response, error_msg) = summarize_agent_outputs(&outputs);
    let status = if error_msg.is_some() { "error" } else { "completed" };

    (
        StatusCode::OK,
        Json(json!({
            "run_id": run_id,
            "session_id": session_id,
            "user_id": auth.user_id.raw(),
            "status": status,
            "prompt": prompt,
            "response": final_response,
            "error": error_msg,
            "outputs": outputs
        })),
    )
        .into_response()
}

async fn chats_agent_runs(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(chat_id_raw): Path<i64>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let chat_id = ChatId(chat_id_raw);
    if let Err(resp) = crate::routes::check_active_membership(&state, chat_id, auth.user_id) {
        return resp;
    }

    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            payload
                .get("idempotency_key")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
        });
    let scoped_idem_key = idempotency_key
        .as_ref()
        .map(|k| format!("{}:{}:{}", auth.user_id.0, chat_id.0, k));
    if let Some(ref key) = scoped_idem_key {
        if let Ok(Some(cached)) = state.hermes.store().kv_get("chat_agent_run_idem", key) {
            if let Ok(val) = serde_json::from_str::<Value>(&cached) {
                return (StatusCode::OK, Json(val)).into_response();
            }
        }
    }

    let prompt = payload
        .get("prompt")
        .or_else(|| payload.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if prompt.is_empty() {
        return crate::routes::err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_prompt",
            "prompt is required",
        );
    }

    let bot_user_id = payload
        .get("bot_id")
        .and_then(|v| v.as_u64())
        .map(UserId)
        .filter(|uid| {
            state
                .db
                .get_user(*uid)
                .ok()
                .flatten()
                .is_some_and(|u| u.is_bot)
        })
        .or_else(|| {
            detect_target_bots_in_chat(&state, chat_id, auth.user_id, &prompt, None)
                .into_iter()
                .next()
        })
        .unwrap_or_else(|| ensure_default_ai_bot(&state.db));

    let typing_guard =
        crate::routes::hermes::BotTypingGuard::start(Arc::clone(&state), chat_id, bot_user_id);

    let bot_instance =
        crate::routes::bots_miniapps::ensure_fabushi_bot_hydrated(&state, bot_user_id.0 as i64);
    let _exec_guard = state.fabushi_bots.begin_bot_execution(
        bot_user_id.0 as i64,
        chat_id.0,
        auth.user_id.0 as i64,
        &prompt,
        None,
    );

    let (bot_sp, bot_model, _) = crate::routes::hermes::load_bot_persona_config(&state, bot_user_id);
    let explicit_model = crate::routes::hermes::sanitize_model_override(
        &state,
        payload
            .get("model")
            .or_else(|| payload.get("model_id"))
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
    );
    let fallback_instance_model = bot_model.or_else(|| {
        bot_instance
            .as_ref()
            .map(|i| i.settings().preferred_model)
            .filter(|m| !m.trim().is_empty())
    });
    let sanitized_bot_model =
        crate::routes::hermes::sanitize_model_override(&state, fallback_instance_model);
    let explicit_sp = payload
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or(bot_sp);
    let bot_display_name = crate::routes::hermes::resolve_bot_display_name(&state, bot_user_id);
    let (enriched_sp, effective_model) = crate::routes::hermes::build_enriched_bot_turn_context(
        &state,
        auth.user_id.0,
        Some(bot_user_id.0),
        bot_display_name.as_deref(),
        explicit_sp,
        explicit_model,
        sanitized_bot_model,
        &[],
        false,
    );
    let (final_sp, profile_delta_prefix) = state.fabushi_bots.build_turn_system_prompt(
        bot_user_id.0 as i64,
        chat_id.0,
        enriched_sp.as_deref().unwrap_or(""),
        &[],
        &[],
    );
    let effective_prompt = match profile_delta_prefix {
        Some(ref prefix) => format!("{prefix}\n\n{prompt}"),
        None => prompt.clone(),
    };
    let explicit_sid = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let explicit_yolo = payload
        .get("yolo_mode")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let resolved_sid = crate::routes::hermes::resolve_and_apply_bot_session_policy(
        &state,
        auth.user_id.0,
        bot_user_id,
        Some(chat_id),
        explicit_sid.as_deref(),
        "chat_agent_run",
        effective_model.as_deref(),
        explicit_yolo,
    )
    .ok()
    .map(|(sid, _)| sid)
    .or(explicit_sid);

    let mut req = gpui_telegram_agent::hermes::HermesTurnRequest::new(&effective_prompt)
        .with_chat_bot(chat_id.0, bot_user_id.0.to_string());
    req.bot_name = bot_display_name;
    if let Some(sid) = resolved_sid {
        req.session_id = Some(sid);
    }
    req.user_id = Some(auth.user_id.0.to_string());
    req.source = "chat_agent_run".to_string();
    req.preset = Some(
        bot_instance
            .as_ref()
            .map(|i| {
                gpui_telegram_agent::fabushi_bot::FabushiBotHost::toolset_preset_for_name(
                    &i.settings().toolset_preset,
                )
            })
            .unwrap_or(gpui_telegram_agent::hermes::toolsets::ToolsetPreset::HermesTelegram),
    );
    req.thinking_budget_tokens = bot_instance.as_ref().and_then(|i| {
        gpui_telegram_agent::fabushi_bot::FabushiBotHost::thinking_budget_for_level(
            &i.settings().thinking_level,
        )
    });
    req.model = effective_model;
    req.system_prompt = Some(final_sp);

    let target_hermes = bot_instance
        .as_ref()
        .map(|i| i.hermes())
        .unwrap_or_else(|| Arc::clone(&state.hermes));
    let turn_res = target_hermes.run_turn(req).await;
    typing_guard.finish();

    let res = match turn_res {
        Ok(r) => r,
        Err(e) => return crate::routes::map_telegram_err(e),
    };

    let deliver = payload
        .get("deliver_to_chat")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let kb_rows = crate::routes::hermes::build_default_bot_quick_actions(&res, &prompt);
    let mut committed_message_id: Option<i64> = None;
    if deliver && !res.response_text.trim().is_empty() {
        let kb_opt = if kb_rows.is_empty() {
            None
        } else {
            Some(kb_rows.clone())
        };
        if let Some(saved) = crate::routes::hermes::deliver_hermes_proactive_message(
            &state,
            chat_id,
            Some(&bot_user_id.0.to_string()),
            &res.response_text,
            kb_opt,
        )
        .await
        {
            committed_message_id = Some(saved.id.0);
        }
    }

    let resp_json = json!({
        "run_id": format!("run_{}", Uuid::new_v4().simple()),
        "turn_id": res.turn_id,
        "chat_id": chat_id.0,
        "session_id": res.session_id,
        "bot_id": bot_user_id.0,
        "message_id": committed_message_id,
        "status": "completed",
        "prompt": prompt,
        "response": res.response_text,
        "model": res.model,
        "provider": res.provider,
        "inline_keyboard": kb_rows,
        "tool_calls_count": res.tool_calls.len(),
        "usage": res.usage,
    });

    if let Some(ref key) = scoped_idem_key {
        if let Ok(raw) = serde_json::to_string(&resp_json) {
            let _ = state.hermes.store().kv_set("chat_agent_run_idem", key, &raw);
        }
    }

    (StatusCode::OK, Json(resp_json)).into_response()
}

async fn agent_turn(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let turn_id = format!("turn_{}", Uuid::new_v4().simple());
    let prompt = payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("turn_session_{}", auth.user_id.raw()));
    let model_id = payload
        .get("model_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let parent_turn_id = payload
        .get("parent_turn_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let thinking_budget_tokens = payload
        .get("thinking_budget_tokens")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let input = gpui_telegram_agent::AgentTurnInput {
        session_id: session_id.clone(),
        prompt: prompt.clone(),
        parent_turn_id,
        model_id,
        thinking_budget_tokens,
    };

    let mut rx = match state.acp_runner.execute_turn_stream(input).await {
        Ok(rx) => rx,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "turn_dispatch_error",
                    "message": err.to_string()
                })),
            )
                .into_response();
        }
    };

    let mut outputs = Vec::new();
    while let Some(out) = rx.recv().await {
        outputs.push(out);
    }

    let (final_response, error_msg) = summarize_agent_outputs(&outputs);
    let status = if error_msg.is_some() { "error" } else { "executed" };

    (
        StatusCode::OK,
        Json(json!({
            "turn_id": turn_id,
            "session_id": session_id,
            "caller_id": auth.user_id.raw(),
            "status": status,
            "prompt": prompt,
            "response": final_response,
            "error": error_msg,
            "outputs": outputs
        })),
    )
        .into_response()
}

async fn mcp_tools_list(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let registry = state.mcp_registry.read();
    let tools = registry.list_tools();
    Json(json!({
        "tools": tools,
        "count": tools.len()
    }))
}

async fn mcp_tool_call(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let tool_name = match payload.get("tool_name").and_then(|v| v.as_str()) {
        Some(name) => name,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "missing_tool_name",
                    "message": "tool_name is required"
                })),
            )
                .into_response();
        }
    };

    let args = payload.get("arguments").cloned().unwrap_or(Value::Null);

    let (requires_approval, risk_level) = {
        let registry = state.mcp_registry.read();
        match registry.get(tool_name) {
            Some(t) => (t.requires_approval, t.risk_level),
            None => (false, gpui_telegram_agent::ToolRiskLevel::Low),
        }
    };

    if requires_approval {
        let req_id = format!("req_{}", Uuid::new_v4().simple());
        let perm_req = gpui_telegram_agent::ToolPermissionRequest::new(
            req_id.clone(),
            tool_name,
            args.clone(),
            risk_level,
            auth.user_id.raw().to_string(),
        );
        state.permission_dock.write().submit_request(perm_req);
        return (
            StatusCode::ACCEPTED,
            Json(json!({
                "status": "pending_approval",
                "request_id": req_id,
                "tool_name": tool_name,
                "risk_level": risk_level
            })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "status": "executed",
            "tool_name": tool_name,
            "result": format!("Tool {} executed successfully", tool_name)
        })),
    )
        .into_response()
}

async fn mcp_permission_approve(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let req_id = match payload.get("request_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "missing_request_id",
                    "message": "request_id is required"
                })),
            )
                .into_response();
        }
    };

    let allow = payload.get("allow").and_then(|v| v.as_bool()).unwrap_or(true);
    let resolved = state.permission_dock.write().resolve(req_id, allow);

    match resolved {
        Some(req) => (
            StatusCode::OK,
            Json(json!({
                "status": if allow { "approved" } else { "rejected" },
                "request": req
            })),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "not_found",
                "message": "Permission request not found or already resolved"
            })),
        )
            .into_response(),
    }
}

async fn computer_action_execute(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let broker = state.computer_lease_broker.read();
    let current_gen = broker.current_generation();
    let active_lease = broker.active_lease();

    let req_gen = payload.get("generation").and_then(|v| v.as_u64()).unwrap_or(0);
    if req_gen != current_gen {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "stale_generation",
                "message": format!(
                    "Fencing check failed: action generation {} does not match broker generation {}",
                    req_gen, current_gen
                )
            })),
        )
            .into_response();
    }

    match active_lease {
        Some(lease) if lease.is_valid() => (
            StatusCode::OK,
            Json(json!({
                "status": "action_executed",
                "generation": current_gen,
                "lease_id": lease.lease_id
            })),
        )
            .into_response(),
        _ => (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "preempted_or_invalid_lease",
                "message": "No active valid control lease holding device"
            })),
        )
            .into_response(),
    }
}

async fn talk_barge_in(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let mut coord = state.talk_coordinator.write();
    let flushed_count = coord.handle_barge_in();
    Json(json!({
        "status": "barge_in_handled",
        "new_state": coord.state,
        "flushed_audio_chunks": flushed_count,
        "buffer_remaining": coord.buffered_chunks_count()
    }))
}

async fn talk_audio_enqueue(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let mut coord = state.talk_coordinator.write();
    match coord.enqueue_audio(body.to_vec()) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "status": "queued",
                "buffered_chunks": coord.buffered_chunks_count()
            })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": "buffer_overflow",
                "message": err
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod ws_shutdown_tests {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_ws_shutdown_while_client_connected_releases_tantivy_lock_and_allows_immediate_reboot() {
        let dir = tempfile::Builder::new()
            .prefix("gpui_ws_shutdown_")
            .tempdir()
            .expect("create tempdir");
        let db_path = dir.path().join("ws_shutdown_test.db");
        let media_dir = dir.path().join("media_cas");
        std::fs::create_dir_all(&media_dir).unwrap();
        let tantivy_dir = dir.path().join("ws_shutdown_test.tantivy");

        let mut cfg = ServerConfig::default();
        cfg.database_url = format!("sqlite://{}", db_path.display());
        cfg.media_storage_path = media_dir.to_string_lossy().into_owned();
        cfg.host = "127.0.0.1".to_string();
        cfg.port = 0;

        let state = Arc::new(AppState::try_new(cfg.clone()).expect("initial AppState::try_new"));
        let (_user, _session, token) = state
            .session_manager
            .register("ws_reboot_user", "StrongPass#2026", "WS", None, "test-dev")
            .expect("register session");

        let w_app = Arc::downgrade(&state);
        let w_hub = Arc::downgrade(&state.gateway_hub);
        let w_search = Arc::downgrade(&state.search_indexer);

        let app = build_router_with_state(state.clone());
        drop(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let srv_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        let ws_url = format!("ws://{addr}/api/v1/ws?token={token}");
        let (mut ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("connect ws");

        let ping = serde_json::to_string(&WsClientFrame::Ping { nonce: 42 }).unwrap();
        ws_stream.send(WsMessage::Text(ping.into())).await.unwrap();
        let pong_msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws_stream.next())
            .await
            .expect("pong timeout")
            .expect("pong stream item")
            .expect("pong ws ok");
        match pong_msg {
            WsMessage::Text(txt) => {
                let frame: WsServerFrame = serde_json::from_str(&txt).unwrap();
                assert!(matches!(frame, WsServerFrame::Pong { nonce: 42 }));
            }
            other => panic!("unexpected ws message: {other:?}"),
        }

        // Shutdown server WHILE ws_stream is still open and held by client
        let _ = shutdown_tx.send(());
        tokio::time::timeout(std::time::Duration::from_secs(2), srv_task)
            .await
            .expect("srv_task shutdown timeout")
            .expect("srv_task join");

        assert_eq!(w_app.strong_count(), 0, "AppState leaked across WS shutdown");
        assert_eq!(w_hub.strong_count(), 0, "GatewayHub leaked across WS shutdown");
        assert_eq!(w_search.strong_count(), 0, "TantivyIndexer leaked across WS shutdown");

        // Zero-retry single-shot Tantivy writer lock acquisition must succeed immediately
        let idx = tantivy::Index::open_in_dir(&tantivy_dir).expect("open tantivy dir");
        let writer = idx
            .writer::<tantivy::TantivyDocument>(15_000_000)
            .expect("single-shot Tantivy writer lock must be free immediately after WS shutdown");
        drop(writer);

        // Immediate AppState reboot on same DB + Tantivy dir must succeed without LockBusy
        let rebooted = Arc::new(AppState::try_new(cfg).expect("reboot AppState::try_new"));
        drop(rebooted);

        // Open client socket should observe server-side close/EOF promptly
        let closed = tokio::time::timeout(std::time::Duration::from_millis(500), ws_stream.next()).await;
        match closed {
            Ok(None) | Ok(Some(Ok(WsMessage::Close(_)))) | Ok(Some(Err(_))) => {}
            other => panic!("expected WS client to observe server close/EOF, got {other:?}"),
        }
    }
}
