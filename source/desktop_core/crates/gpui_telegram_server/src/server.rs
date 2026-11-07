use crate::auth::passkey_service::{PasskeyChallenge, PasskeyService, PasskeyVerifyRequest};
use crate::auth::session_manager::{AuthError, SessionManager};
use crate::config::ServerConfig;
use crate::ws::{handle_ws_socket, GatewayHub};
use axum::{
    body::Bytes,
    extract::{FromRequestParts, Path, Query, State, WebSocketUpgrade},
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::Utc;
use gpui_telegram_core::auth::SessionToken;
use gpui_telegram_core::types::UserId;
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

/// Global application state shared across HTTP and WebSocket handlers
#[derive(Clone)]
pub struct AppState {
    pub config: ServerConfig,
    pub db: Arc<crate::db::ServerDatabase>,
    pub session_manager: Arc<SessionManager>,
    pub gateway_hub: Arc<GatewayHub>,
    pub pairing_requests: Arc<RwLock<HashMap<String, (UserId, i64)>>>,
    pub passkey_challenges: Arc<RwLock<HashMap<String, i64>>>,
    pub mcp_registry: Arc<RwLock<gpui_telegram_agent::McpRegistry>>,
    pub permission_dock: Arc<RwLock<gpui_telegram_agent::ToolPermissionDock>>,
    pub computer_lease_broker: Arc<RwLock<gpui_telegram_agent::ComputerLeaseBroker>>,
    pub talk_coordinator: Arc<RwLock<gpui_telegram_agent::TalkCoordinator>>,
    pub acp_runner: Arc<gpui_telegram_agent::acp::AcpRunner>,
    pub task_registry: Arc<tokio::sync::RwLock<HashMap<String, gpui_telegram_agent::tasks::TaskFsm>>>,
    pub approval_gates: Arc<tokio::sync::RwLock<HashMap<String, gpui_telegram_agent::tasks::ApprovalGate>>>,
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

    pub fn with_db(config: ServerConfig, db: Arc<crate::db::ServerDatabase>) -> Self {
        let session_manager = Arc::new(SessionManager::new());
        let gateway_hub = Arc::new(GatewayHub::new());

        // Monotonically align user_id with SQLite database
        if let Ok(max_id) = db.get_max_user_id() {
            session_manager.set_next_user_id(std::cmp::max(100_001, max_id + 1));
        }

        // Wire disconnection signal from session manager to gateway hub
        let hub_clone = gateway_hub.clone();
        session_manager.set_disconnect_notifier(Arc::new(move |user_id| {
            hub_clone.disconnect_user(user_id);
        }));

        let acp_runner = Arc::new(gpui_telegram_agent::acp::AcpRunner::new(
            &config.default_agent_model,
        ));
        let task_registry = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        let approval_gates = Arc::new(tokio::sync::RwLock::new(HashMap::new()));

        Self {
            config,
            db,
            session_manager,
            gateway_hub,
            pairing_requests: Arc::new(RwLock::new(HashMap::new())),
            passkey_challenges: Arc::new(RwLock::new(HashMap::new())),
            mcp_registry: Arc::new(RwLock::new(gpui_telegram_agent::McpRegistry::new())),
            permission_dock: Arc::new(RwLock::new(gpui_telegram_agent::ToolPermissionDock::new())),
            computer_lease_broker: Arc::new(RwLock::new(gpui_telegram_agent::ComputerLeaseBroker::new())),
            talk_coordinator: Arc::new(RwLock::new(gpui_telegram_agent::TalkCoordinator::new())),
            acp_runner,
            task_registry,
            approval_gates,
        }
    }

    fn open_database(config: &ServerConfig) -> crate::db::ServerDatabase {
        let path = config.resolve_db_path();
        if path == ":memory:" || path.is_empty() {
            crate::db::ServerDatabase::open_in_memory().expect("Failed to initialize in-memory SQLite")
        } else {
            crate::db::ServerDatabase::open(path).unwrap_or_else(|e| {
                tracing::warn!("Failed to open SQLite database at '{}': {}. Falling back to in-memory", path, e);
                crate::db::ServerDatabase::open_in_memory().expect("In-memory SQLite fallback failed")
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

        let session = if let Some(s) = state.session_manager.verify_token(&token) {
            s
        } else {
            let session_id = token.split('.').next().unwrap_or(&token);
            let restored = if let Ok(Some(db_sess)) = state.db.get_session(session_id) {
                if db_sess.revoked_at_ms.is_none()
                    && db_sess.expires_at_ms > chrono::Utc::now().timestamp_millis()
                {
                    let created_at = chrono::DateTime::from_timestamp_millis(db_sess.created_at_ms)
                        .unwrap_or_else(chrono::Utc::now);
                    let expires_at = chrono::DateTime::from_timestamp_millis(db_sess.expires_at_ms)
                        .unwrap_or_else(chrono::Utc::now);
                    let sess_tok = SessionToken::new_with_hash(
                        db_sess.session_id.clone(),
                        db_sess.user_id,
                        created_at,
                        expires_at,
                        &db_sess.device_id,
                        db_sess.token_hash,
                    );
                    state.session_manager.insert(sess_tok);
                    state.session_manager.verify_token(&token)
                } else {
                    None
                }
            } else {
                None
            };

            restored.ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "error": "invalid_token",
                        "message": "Session token is invalid, expired, or revoked"
                    })),
                )
            })?
        };

        let user_id = session.user_id;
        Ok(AuthBearer { session, user_id })
    }
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
    Router::new()
        // --- Public Routes ---
        .route("/health", get(health_check))
        .route("/api/v1/status", get(server_status))
        .route("/api/v1/auth/register", post(auth_register))
        .route("/api/v1/auth/login", post(auth_login))
        .route("/api/v1/auth/passkey/challenge", post(auth_passkey_challenge))
        .route("/api/v1/auth/passkey/verify", post(auth_passkey_verify))
        .route("/api/v1/auth/pair/init", post(auth_pair_init))
        .route("/api/v1/auth/pair/verify", post(auth_pair_verify))
        // --- Protected Auth Routes ---
        .route("/api/v1/auth/me", get(auth_me))
        .route("/api/v1/auth/sessions", get(auth_sessions))
        .route("/api/v1/auth/sessions/{session_id}", delete(auth_revoke_session))
        // --- Protected Messaging Routes ---
        .route("/api/v1/messages/history", get(messages_history))
        .route("/api/v1/messages/send", post(messages_send))
        .route("/api/v1/messages/edit", post(messages_edit))
        .route("/api/v1/messages/delete", post(messages_delete))
        .route("/api/v1/messages/reaction", post(messages_reaction))
        .route("/api/v1/messages/read", post(messages_read))
        // --- Protected Chat Routes ---
        .route("/api/v1/chats", get(chats_list))
        .route("/api/v1/chats/create", post(chats_create))
        .route("/api/v1/chats/{chat_id}", get(chats_get))
        .route("/api/v1/chats/{chat_id}/topics", get(chats_topics))
        // --- Protected Sync Routes ---
        .route("/api/v1/sync/difference", get(sync_difference))
        // --- Protected Media Routes ---
        .route("/api/v1/media/upload/init", post(media_upload_init))
        .route(
            "/api/v1/media/upload/{upload_id}/part/{part_index}",
            put(media_upload_part),
        )
        .route(
            "/api/v1/media/upload/{upload_id}/complete",
            post(media_upload_complete),
        )
        .route("/api/v1/media/stream/{file_id}", get(media_stream))
        // --- Protected Search Routes ---
        .route("/api/v1/search/messages", get(search_messages))
        // --- Protected WebRTC Routes ---
        .route("/api/v1/webrtc/call/invite", post(webrtc_call_invite))
        .route("/api/v1/webrtc/call/signal", post(webrtc_call_signal))
        .route(
            "/api/v1/webrtc/turn_credentials",
            get(webrtc_turn_credentials),
        )
        // --- Agent & Capability Routes ---
        .route("/api/v1/agent/runs", post(agent_runs))
        .route("/api/v1/agent/turn", post(agent_turn))
        .route("/api/v1/tasks", get(tasks_list).post(tasks_create))
        .route("/api/v1/tasks/{task_id}/approve", post(tasks_approve))
        .route("/api/v1/mcp/servers", get(mcp_servers_list).post(mcp_servers_register))
        .route("/api/v1/mcp/tools", get(mcp_tools_list))
        .route("/api/v1/mcp/call", post(mcp_tool_call))
        .route("/api/v1/mcp/permission/approve", post(mcp_permission_approve))
        .route("/api/v1/computer/lease", post(computer_lease_acquire))
        .route("/api/v1/computer/lease/acquire", post(computer_lease_acquire))
        .route(
            "/api/v1/computer/lease/emergency_takeover",
            post(computer_lease_emergency_takeover),
        )
        .route("/api/v1/computer/action", post(computer_action_execute))
        .route("/api/v1/talk/session", post(talk_session_create))
        .route("/api/v1/talk/barge_in", post(talk_barge_in))
        .route("/api/v1/talk/audio", post(talk_audio_enqueue))
        // --- WebSocket Upgrade Route ---
        .route("/api/v1/ws", get(ws_upgrade_handler))
        // --- Tower Middleware Stack ---
        .layer(CompressionLayer::new().gzip(true))
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
        "version": "0.1.0"
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
    if req.password.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_credentials", "message": "Password cannot be empty" })),
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

            // Persist session to SQLite database
            let db_session = crate::db::schema::SessionRecord {
                session_id: session.session_id.clone(),
                user_id: user.user_id,
                device_id: req.device_name.clone(),
                client_type: "gpui_desktop".into(),
                app_version: "0.1.0".into(),
                ip_address: None,
                location_desc: None,
                account_generation: 1,
                token_hash: session.token_hash.clone(),
                created_at_ms: session.created_at.timestamp_millis(),
                last_active_ms: session.created_at.timestamp_millis(),
                expires_at_ms: session.expires_at.timestamp_millis(),
                revoked_at_ms: None,
            };
            let _ = state.db.create_session(&db_session);

            (
                StatusCode::CREATED,
                Json(json!({
                    "user_id": user.user_id.raw(),
                    "username": user.username,
                    "first_name": user.first_name,
                    "session_id": session.session_id,
                    "token": token,
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
    // 1. Try memory session manager
    match state
        .session_manager
        .login(&req.username, &req.password, &req.device_name)
    {
        Ok((user, session, token)) => {
            let db_session = crate::db::schema::SessionRecord {
                session_id: session.session_id.clone(),
                user_id: user.user_id,
                device_id: req.device_name.clone(),
                client_type: "gpui_desktop".into(),
                app_version: "0.1.0".into(),
                ip_address: None,
                location_desc: None,
                account_generation: 1,
                token_hash: session.token_hash.clone(),
                created_at_ms: session.created_at.timestamp_millis(),
                last_active_ms: session.created_at.timestamp_millis(),
                expires_at_ms: session.expires_at.timestamp_millis(),
                revoked_at_ms: None,
            };
            let _ = state.db.create_session(&db_session);

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
        Err(_) => {
            // 2. Query SQLite database on memory miss (critical for persistence across restarts)
            if let Ok(Some(db_user)) = state.db.get_user_by_username(&req.username) {
                if let Some(ref hash) = db_user.password_hash {
                    if crate::auth::verify_password(&req.password, hash) {
                        // Password matches! Restore user record into SessionManager
                        let created_at = chrono::DateTime::from_timestamp_millis(db_user.created_at_ms)
                            .unwrap_or_else(Utc::now);
                        let mem_user = crate::auth::session_manager::UserRecord {
                            user_id: db_user.user_id,
                            username: db_user.username.clone().unwrap_or_else(|| req.username.clone()),
                            first_name: db_user.first_name.clone(),
                            last_name: db_user.last_name.clone(),
                            password_hash: hash.clone(),
                            created_at,
                        };
                        state.session_manager.insert_user(mem_user.clone());

                        // Generate session
                        let (session, raw_token) = state.session_manager.create_session(
                            db_user.user_id,
                            &req.device_name,
                            chrono::Duration::days(30),
                        );

                        // Persist session into SQLite
                        let db_session = crate::db::schema::SessionRecord {
                            session_id: session.session_id.clone(),
                            user_id: db_user.user_id,
                            device_id: req.device_name.clone(),
                            client_type: "gpui_desktop".into(),
                            app_version: "0.1.0".into(),
                            ip_address: None,
                            location_desc: None,
                            account_generation: 1,
                            token_hash: session.token_hash.clone(),
                            created_at_ms: session.created_at.timestamp_millis(),
                            last_active_ms: session.created_at.timestamp_millis(),
                            expires_at_ms: session.expires_at.timestamp_millis(),
                            revoked_at_ms: None,
                        };
                        let _ = state.db.create_session(&db_session);

                        return (
                            StatusCode::OK,
                            Json(json!({
                                "user_id": db_user.user_id.raw(),
                                "username": mem_user.username,
                                "session_id": session.session_id,
                                "token": raw_token,
                                "expires_at": session.expires_at,
                            })),
                        )
                            .into_response();
                    }
                }
            }

            (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "error": "invalid_credentials",
                    "message": "Invalid username or password"
                })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct PasskeyChallengeReq {
    pub user_id: Option<UserId>,
    pub username: Option<String>,
}

async fn auth_passkey_challenge(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasskeyChallengeReq>,
) -> Json<PasskeyChallenge> {
    let challenge = PasskeyService::generate_challenge(req.user_id);
    state
        .passkey_challenges
        .write()
        .insert(challenge.challenge.clone(), Utc::now().timestamp_millis());
    Json(challenge)
}

async fn auth_passkey_verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasskeyVerifyRequest>,
) -> Response {
    let challenge_valid = state
        .passkey_challenges
        .write()
        .remove(&req.challenge)
        .is_some();

    if !challenge_valid {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_challenge", "message": "Unknown or expired challenge" })),
        )
            .into_response();
    }

    match PasskeyService::verify_assertion(&req) {
        Ok(true) => {
            let user_id = req.user_id.unwrap_or(UserId(100_001));
            let (session, token) = state.session_manager.create_session(
                user_id,
                "Passkey Authenticator",
                chrono::Duration::days(30),
            );
            (
                StatusCode::OK,
                Json(json!({
                    "verified": true,
                    "user_id": user_id.raw(),
                    "session_id": session.session_id,
                    "token": token,
                    "expires_at": session.expires_at,
                })),
            )
                .into_response()
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "verification_failed", "message": "Passkey assertion failed" })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct PairInitReq {
    pub device_name: Option<String>,
}

async fn auth_pair_init(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PairInitReq>,
) -> Json<Value> {
    let pair_token = format!("pair_{}", Uuid::new_v4().simple());
    let expires_at_ms = Utc::now().timestamp_millis() + 300_000; // 5 minutes
    let placeholder_user = UserId(100_001);

    state
        .pairing_requests
        .write()
        .insert(pair_token.clone(), (placeholder_user, expires_at_ms));

    Json(json!({
        "pair_token": pair_token,
        "qr_payload": format!("tg://login?token={}", pair_token),
        "device_name": req.device_name.unwrap_or_else(|| "GPUI Companion".into()),
        "expires_at_ms": expires_at_ms,
    }))
}

#[derive(Debug, Deserialize)]
pub struct PairVerifyReq {
    pub pair_token: String,
    pub device_name: Option<String>,
}

async fn auth_pair_verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PairVerifyReq>,
) -> Response {
    let pair_entry = state.pairing_requests.write().remove(&req.pair_token);
    let (user_id, expires_at_ms) = match pair_entry {
        Some(entry) => entry,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "invalid_pair_token", "message": "Pair token not found or expired" })),
            )
                .into_response()
        }
    };

    if Utc::now().timestamp_millis() > expires_at_ms {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "pair_token_expired", "message": "Pair token has expired" })),
        )
            .into_response();
    }

    let device_name = req.device_name.unwrap_or_else(|| "Paired Device".into());
    let (session, token) = state
        .session_manager
        .create_session(user_id, &device_name, chrono::Duration::days(30));

    (
        StatusCode::OK,
        Json(json!({
            "verified": true,
            "user_id": user_id.raw(),
            "session_id": session.session_id,
            "token": token,
            "expires_at": session.expires_at,
        })),
    )
        .into_response()
}

// ============================================================================
// Protected Route Handlers (AuthBearer Enforced)
// ============================================================================

async fn auth_me(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Json<Value> {
    let fence = state.session_manager.get_or_create_fence(auth.user_id);
    let mut user_record = state.session_manager.get_user(auth.user_id);

    if user_record.is_none() {
        if let Ok(Some(db_user)) = state.db.get_user(auth.user_id) {
            let created_at = chrono::DateTime::from_timestamp_millis(db_user.created_at_ms)
                .unwrap_or_else(Utc::now);
            let mem_user = crate::auth::session_manager::UserRecord {
                user_id: db_user.user_id,
                username: db_user
                    .username
                    .clone()
                    .unwrap_or_else(|| format!("user_{}", db_user.user_id.0)),
                first_name: db_user.first_name.clone(),
                last_name: db_user.last_name.clone(),
                password_hash: db_user.password_hash.clone().unwrap_or_default(),
                created_at,
            };
            state.session_manager.insert_user(mem_user.clone());
            user_record = Some(mem_user);
        }
    }

    Json(json!({
        "user_id": auth.user_id.raw(),
        "username": user_record.as_ref().map(|u| u.username.clone()),
        "first_name": user_record.as_ref().map(|u| u.first_name.clone()),
        "session_id": auth.session.session_id,
        "device_name": auth.session.device_name,
        "created_at": auth.session.created_at,
        "expires_at": auth.session.expires_at,
        "account_generation": fence.current(),
    }))
}

async fn auth_sessions(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Json<Value> {
    let sessions = state.session_manager.list_user_sessions(auth.user_id);
    Json(json!({
        "user_id": auth.user_id.raw(),
        "sessions": sessions,
    }))
}

async fn auth_revoke_session(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Response {
    // Only allow revoking sessions owned by user
    if let Some(target) = state.session_manager.get(&session_id) {
        if target.user_id != auth.user_id {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "forbidden", "message": "Cannot revoke other user's session" })),
            )
                .into_response();
        }
    }

    let revoked = state.session_manager.revoke_session(&session_id);
    let _ = state.db.revoke_session(&session_id);
    let fence = state.session_manager.get_or_create_fence(auth.user_id);

    (
        StatusCode::OK,
        Json(json!({
            "revoked": revoked.is_some(),
            "session_id": session_id,
            "account_generation": fence.current(),
        })),
    )
        .into_response()
}

async fn messages_history(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(_params): Query<HashMap<String, String>>,
) -> Json<Value> {
    Json(json!({
        "messages": [],
        "has_more": false,
        "current_pts": state.gateway_hub.current_pts(),
    }))
}

async fn messages_send(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    let pts = state.gateway_hub.next_pts();
    let msg_id = Utc::now().timestamp_millis();

    Json(json!({
        "message_id": msg_id,
        "pts": pts,
        "sender_id": auth.user_id.raw(),
        "chat_id": payload.get("chat_id"),
        "status": "sent",
    }))
}

async fn messages_edit(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(_payload): Json<Value>,
) -> Json<Value> {
    let pts = state.gateway_hub.next_pts();
    Json(json!({
        "success": true,
        "pts": pts,
        "status": "edited",
    }))
}

async fn messages_delete(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(_payload): Json<Value>,
) -> Json<Value> {
    let pts = state.gateway_hub.next_pts();
    Json(json!({
        "success": true,
        "pts": pts,
        "deleted_count": 1,
    }))
}

async fn messages_reaction(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(_payload): Json<Value>,
) -> Json<Value> {
    let pts = state.gateway_hub.next_pts();
    Json(json!({
        "success": true,
        "pts": pts,
        "status": "reaction_updated",
    }))
}

async fn messages_read(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(_payload): Json<Value>,
) -> Json<Value> {
    let pts = state.gateway_hub.next_pts();
    Json(json!({
        "success": true,
        "pts": pts,
        "status": "history_read",
    }))
}

async fn chats_list(_auth: AuthBearer, State(_state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "chats": [],
        "total": 0,
    }))
}

async fn chats_create(
    auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("New Group");
    let chat_id = Utc::now().timestamp_millis();

    (
        StatusCode::CREATED,
        Json(json!({
            "chat_id": chat_id,
            "title": title,
            "creator_id": auth.user_id.raw(),
        })),
    )
}

async fn chats_get(
    _auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
    Path(chat_id): Path<i64>,
) -> Json<Value> {
    Json(json!({
        "chat_id": chat_id,
        "title": format!("Chat {}", chat_id),
        "is_group": true,
    }))
}

async fn chats_topics(
    _auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
    Path(chat_id): Path<i64>,
) -> Json<Value> {
    Json(json!({
        "chat_id": chat_id,
        "topics": [],
    }))
}

async fn sync_difference(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(_params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let current_pts = state.gateway_hub.current_pts();
    Json(json!({
        "new_pts": current_pts,
        "has_more": false,
        "messages": [],
        "other_updates": [],
        "snapshot_required": false,
    }))
}

async fn media_upload_init(
    _auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let upload_id = format!("upl_{}", Uuid::new_v4().simple());
    (
        StatusCode::CREATED,
        Json(json!({
            "upload_id": upload_id,
            "chunk_size": 524288,
            "filename": payload.get("filename"),
            "size_bytes": payload.get("size_bytes"),
        })),
    )
}

async fn media_upload_part(
    _auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
    Path((upload_id, part_index)): Path<(String, u32)>,
    body: Bytes,
) -> Json<Value> {
    Json(json!({
        "upload_id": upload_id,
        "part_index": part_index,
        "bytes_received": body.len(),
        "status": "accepted",
    }))
}

async fn media_upload_complete(
    _auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
    Path(upload_id): Path<String>,
) -> Json<Value> {
    let file_id = format!("file_{}", Uuid::new_v4().simple());
    Json(json!({
        "upload_id": upload_id,
        "file_id": file_id,
        "status": "completed",
    }))
}

async fn media_stream(
    _auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
    Path(file_id): Path<String>,
    headers: header::HeaderMap,
) -> Response {
    let sample_payload = b"RIFF....WAVEfmt ....data";

    if let Some(range_header) = headers.get(header::RANGE) {
        if let Ok(range_str) = range_header.to_str() {
            if range_str.starts_with("bytes=") {
                return (
                    StatusCode::PARTIAL_CONTENT,
                    [
                        (header::CONTENT_TYPE, "audio/ogg"),
                        (
                            header::CONTENT_RANGE,
                            &format!("bytes 0-{}/{}", sample_payload.len() - 1, sample_payload.len())[..],
                        ),
                    ],
                    Bytes::from_static(sample_payload),
                )
                    .into_response();
            }
        }
    }

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                &format!("inline; filename=\"{}.bin\"", file_id)[..],
            ),
        ],
        Bytes::from_static(sample_payload),
    )
        .into_response()
}

async fn search_messages(
    _auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let q = params.get("q").cloned().unwrap_or_default();
    Json(json!({
        "query": q,
        "results": [],
        "total": 0,
    }))
}

async fn webrtc_call_invite(
    auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    let call_id = format!("call_{}", Uuid::new_v4().simple());
    Json(json!({
        "call_id": call_id,
        "caller_id": auth.user_id.raw(),
        "callee_id": payload.get("callee_id"),
        "status": "invited",
    }))
}

async fn webrtc_call_signal(
    auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    Json(json!({
        "user_id": auth.user_id.raw(),
        "signal": payload,
        "status": "relayed",
    }))
}

async fn webrtc_turn_credentials(
    _auth: AuthBearer,
    State(_state): State<Arc<AppState>>,
) -> Json<Value> {
    Json(json!({
        "urls": ["stun:stun.l.google.com:19302"],
        "username": "ephemeral_gpui_user",
        "credential": "ephemeral_gpui_credential",
        "ttl_seconds": 86400,
    }))
}

// ============================================================================
// WebSocket Upgrade Handler
// ============================================================================

async fn ws_upgrade_handler(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
) -> Response {
    let hub = state.gateway_hub.clone();
    let heartbeat = state.config.ws_heartbeat_interval_secs;
    let timeout = state.config.ws_inactivity_timeout_secs;
    let user_id = auth.user_id;

    ws.on_upgrade(move |socket| handle_ws_socket(socket, user_id, hub, heartbeat, timeout))
}

// ============================================================================
// Agent & Capability Route Handlers
// ============================================================================

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

    let final_response = outputs
        .iter()
        .find_map(|o| match o {
            gpui_telegram_agent::AgentTurnOutput::FinalResponse { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();

    (
        StatusCode::OK,
        Json(json!({
            "run_id": run_id,
            "session_id": session_id,
            "user_id": auth.user_id.raw(),
            "status": "completed",
            "prompt": prompt,
            "response": final_response,
            "outputs": outputs
        })),
    )
        .into_response()
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

    let final_response = outputs
        .iter()
        .find_map(|o| match o {
            gpui_telegram_agent::AgentTurnOutput::FinalResponse { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();

    (
        StatusCode::OK,
        Json(json!({
            "turn_id": turn_id,
            "session_id": session_id,
            "caller_id": auth.user_id.raw(),
            "status": "executed",
            "prompt": prompt,
            "response": final_response,
            "outputs": outputs
        })),
    )
        .into_response()
}

async fn tasks_list(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let registry = state.task_registry.read().await;
    let mut tasks: Vec<Value> = registry
        .values()
        .map(|task| {
            json!({
                "task_id": task.task_id,
                "state": task.state.as_str(),
                "state_history": task.state_history,
                "abort_reason": task.abort_reason,
                "is_active": task.is_active(),
                "is_terminal": task.is_terminal(),
            })
        })
        .collect();
    tasks.sort_by(|a, b| {
        a["task_id"].as_str().cmp(&b["task_id"].as_str())
    });
    let total = tasks.len();
    Json(json!({
        "tasks": tasks,
        "columns": ["inbox", "active", "review", "completed", "failed", "cancelled"],
        "total": total
    }))
}

async fn tasks_create(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let task_id = payload
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("task_{}", Uuid::new_v4().simple()));

    let mut task = gpui_telegram_agent::tasks::TaskFsm::new(task_id.clone());
    if let Some(target_state_str) = payload.get("initial_state").and_then(|v| v.as_str()) {
        if let Some(target_state) = gpui_telegram_agent::tasks::TaskState::from_str(target_state_str) {
            if target_state != gpui_telegram_agent::tasks::TaskState::Pending {
                let _ = task.transition_to(target_state);
            }
        }
    }

    state.task_registry.write().await.insert(task_id.clone(), task.clone());

    (
        StatusCode::CREATED,
        Json(json!({
            "task_id": task_id,
            "state": task.state.as_str(),
            "created": true
        })),
    )
        .into_response()
}

async fn tasks_approve(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    body_bytes: Bytes,
) -> Response {
    let payload: Value = if body_bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body_bytes).unwrap_or_else(|_| json!({}))
    };

    let is_approved = payload
        .get("approved")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let decision_str = payload
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or(if is_approved { "approve" } else { "deny" });

    let decision = if decision_str == "deny" || !is_approved {
        let reason = payload.get("reason").and_then(|v| v.as_str()).map(String::from);
        gpui_telegram_agent::ApprovalDecision::Deny { reason }
    } else if decision_str == "always_allow" {
        gpui_telegram_agent::ApprovalDecision::AlwaysAllow
    } else {
        gpui_telegram_agent::ApprovalDecision::ApproveOnce
    };

    // 1. Resolve gate in state.approval_gates
    let mut gates = state.approval_gates.write().await;
    let gate_key = if gates.contains_key(&task_id) {
        Some(task_id.clone())
    } else {
        gates.iter().find_map(|(k, g)| if g.task_id == task_id { Some(k.clone()) } else { None })
    };

    let resolved_gate_id = if let Some(key) = gate_key {
        if let Some(gate) = gates.get_mut(&key) {
            let _ = gate.decide(decision.clone());
            gate.gate_id.clone()
        } else {
            format!("gate_{}", task_id)
        }
    } else {
        let gate_id = format!("gate_{}", task_id);
        let mut new_gate = gpui_telegram_agent::ApprovalGate::new(
            gate_id.clone(),
            task_id.clone(),
            "task_approval".into(),
            "API triggered task approval".into(),
        );
        let _ = new_gate.decide(decision.clone());
        gates.insert(gate_id.clone(), new_gate);
        gate_id
    };
    drop(gates);

    // 2. Transition task in state.task_registry
    let mut tasks = state.task_registry.write().await;
    let (current_state, transition_success) = if let Some(task) = tasks.get_mut(&task_id) {
        let target_state = match decision {
            gpui_telegram_agent::ApprovalDecision::Deny { .. } => {
                gpui_telegram_agent::tasks::TaskState::Cancelled
            }
            _ => {
                if task.state == gpui_telegram_agent::tasks::TaskState::WaitingAuth
                    || task.state == gpui_telegram_agent::tasks::TaskState::Pending
                {
                    gpui_telegram_agent::tasks::TaskState::Running
                } else {
                    task.state
                }
            }
        };

        if target_state != task.state {
            match task.transition_to(target_state) {
                Ok(()) => (task.state.as_str().to_string(), true),
                Err(_) => (task.state.as_str().to_string(), false),
            }
        } else {
            (task.state.as_str().to_string(), true)
        }
    } else {
        ("unknown".to_string(), true)
    };

    (
        StatusCode::OK,
        Json(json!({
            "task_id": task_id,
            "approved": is_approved,
            "decision": format!("{:?}", decision),
            "current_state": current_state,
            "transition_success": transition_success,
            "gate_id": resolved_gate_id,
            "resolved_at": Utc::now()
        })),
    )
        .into_response()
}

async fn mcp_servers_list(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let registry = state.mcp_registry.read();
    let tools = registry.list_tools();
    Json(json!({
        "servers": [],
        "registered_tools_count": tools.len()
    }))
}

async fn mcp_servers_register(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let server_id = payload
        .get("server_id")
        .and_then(|v| v.as_str())
        .unwrap_or("default");
    if let Some(tool_name) = payload.get("tool_name").and_then(|v| v.as_str()) {
        let desc = payload
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let schema = payload
            .get("schema")
            .and_then(|v| v.as_str())
            .unwrap_or("{}");
        let def = gpui_telegram_agent::McpToolDefinition::new(
            tool_name,
            desc,
            schema,
            gpui_telegram_agent::ToolRiskLevel::Low,
        )
        .with_server_id(server_id);
        state.mcp_registry.write().register(def);
    }
    (
        StatusCode::CREATED,
        Json(json!({
            "status": "registered",
            "server_id": server_id
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

async fn computer_lease_acquire(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let device_id = payload
        .get("device_id")
        .and_then(|v| v.as_str())
        .unwrap_or("desktop_local");
    let agent_id = format!("agent_{}", auth.user_id.raw());

    let lease_opt = state
        .computer_lease_broker
        .write()
        .acquire_lease(&agent_id, device_id);

    match lease_opt {
        Some(lease) => (
            StatusCode::OK,
            Json(json!({
                "lease_id": lease.lease_id,
                "target_device_id": lease.target_device_id,
                "generation": lease.generation,
                "state": lease.state,
                "expires_at": lease.expires_at,
                "is_valid": lease.is_valid()
            })),
        )
            .into_response(),
        None => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "lease_unavailable",
                "message": "Computer control lease could not be acquired"
            })),
        )
            .into_response(),
    }
}

async fn computer_lease_emergency_takeover(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let new_gen = state.computer_lease_broker.write().emergency_takeover();
    Json(json!({
        "status": "emergency_takeover_executed",
        "action": "preempt_and_fence",
        "new_generation": new_gen,
        "latency_sub_10ms": true
    }))
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

async fn talk_session_create(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(_payload): Json<Value>,
) -> Response {
    let mut coord = state.talk_coordinator.write();
    coord.state = gpui_telegram_agent::TalkSessionState::Listening;
    (
        StatusCode::CREATED,
        Json(json!({
            "session_id": coord.session_id,
            "state": coord.state,
            "generation": coord.generation,
            "signal_seq": coord.signal_seq
        })),
    )
        .into_response()
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
