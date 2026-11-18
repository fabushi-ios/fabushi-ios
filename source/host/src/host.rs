use crate::compat::LegacyTestHostState;
use crate::dedup::DedupStore;
use crate::desktop_helpers::{AppLanguage, AppPreferences, VoiceGestureState};
use crate::error::BridgeError;
use crate::events::EventBridgeRegistry;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use fabushi_miniapp::storage::init_fabushi_tables;
use gpui_telegram_agent::{
    ComputerLeaseBroker, LocalAgenticStore, SlashCommandRegistry, TaskFsm, ToolPermissionDock,
};
use gpui_telegram_client::config::{ClientConfig, StoredSession, DEFAULT_SERVER_URL};
use gpui_telegram_client::wallet::service::ton_wallet_service::TonWalletService;
use gpui_telegram_client::{AuthResponse, ClientProxyConfig, TelegramClient};
use gpui_telegram_core::auth::GenerationFence;
use gpui_telegram_proto::semantic_tree::SemanticWorkspaceTree;
use gpui_telegram_webrtc::calls::WebRtcCallEngine;
use gpui_telegram_webrtc::media::DefaultAiMediaTap;
use magpie_gateway::{MaskingConfig, MaskingEngine, UsageLedger};
use parking_lot::{Mutex, RwLock};
use reqwest::Method;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

pub const IOS_DEFAULT_SERVER_URL: &str = DEFAULT_SERVER_URL; // "http://34.46.77.133:8080"

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedHostConfig {
    pub server_url: String,
    #[serde(default)]
    pub preferences: AppPreferences,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ClientProxyConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passcode_phc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apns_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voip_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MultiAccountStore {
    pub active_user_id: Option<u64>,
    pub accounts: BTreeMap<u64, StoredSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MagpieProviderEntry {
    pub id: String,
    pub preset: String,
    pub base_url: String,
    pub api_key_masked: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub enabled: bool,
    pub priority: u32,
}

pub struct MagpieHostState {
    pub providers: BTreeMap<String, MagpieProviderEntry>,
    pub masking: MaskingEngine,
    pub usage: UsageLedger,
}

impl Default for MagpieHostState {
    fn default() -> Self {
        Self {
            providers: BTreeMap::new(),
            masking: MaskingEngine::new(MaskingConfig::default()),
            usage: UsageLedger::default(),
        }
    }
}

pub struct TalkRuntimeState {
    pub active: bool,
    pub session_id: Option<String>,
    pub peer_or_bot_id: Option<String>,
    pub tts_playing: bool,
    pub barge_in_count: u64,
    pub last_vad_rms: f32,
}

impl Default for TalkRuntimeState {
    fn default() -> Self {
        Self {
            active: false,
            session_id: None,
            peer_or_bot_id: None,
            tts_playing: false,
            barge_in_count: 0,
            last_vad_rms: 0.0,
        }
    }
}

pub struct TelegramAppHost {
    pub app_data_dir: PathBuf,
    pub is_test_mode: bool,
    pub events: Arc<EventBridgeRegistry>,
    pub client: RwLock<Arc<TelegramClient>>,
    pub dedup_store: Mutex<DedupStore>,
    pub generation_fence: Mutex<GenerationFence>,
    pub cua_broker: Mutex<ComputerLeaseBroker>,
    pub call_engine: Mutex<WebRtcCallEngine>,
    pub ai_media_tap: DefaultAiMediaTap,
    pub talk_state: Mutex<TalkRuntimeState>,
    pub slash_registry: SlashCommandRegistry,
    pub agentic_store: Mutex<LocalAgenticStore>,
    pub task_registry: Mutex<HashMap<String, TaskFsm>>,
    pub tool_dock: Mutex<ToolPermissionDock>,
    pub magpie: Mutex<MagpieHostState>,
    pub dharma_db: Mutex<Connection>,
    pub wallet_service: TonWalletService,
    pub accounts: Mutex<MultiAccountStore>,
    pub host_config: Mutex<PersistedHostConfig>,
    pub semantic_tree: Mutex<SemanticWorkspaceTree>,
    pub voice_gesture: Mutex<VoiceGestureState>,
    pub simulate_offline: Mutex<bool>,
    pub is_backgrounded: AtomicBool,
    pub legacy_test_state: Mutex<LegacyTestHostState>,
    pub next_local_msg_id: AtomicI64,
    pub next_pts: AtomicU64,
    shutdown_flag: Arc<AtomicBool>,
    _passphrase: Option<Zeroizing<String>>,
}

impl TelegramAppHost {
    pub fn new(
        app_data_dir: PathBuf,
        passphrase: Option<String>,
        is_test_mode: bool,
    ) -> Result<Self, String> {
        fs::create_dir_all(&app_data_dir)
            .map_err(|e| format!("Failed to create app_data_dir {}: {e}", app_data_dir.display()))?;

        let cfg_path = app_data_dir.join("host_config.json");
        let persisted: Option<PersistedHostConfig> = fs::read_to_string(&cfg_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());

        let env_url = ["FABUSHI_SERVER_URL", "GPUI_TELEGRAM_SERVER_URL"]
            .into_iter()
            .find_map(|k| {
                std::env::var(k)
                    .ok()
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            });

        let resolved_url = env_url
            .or_else(|| persisted.as_ref().map(|c| c.server_url.clone()))
            .map(|u| ClientConfig::normalize_url(&u))
            .unwrap_or_else(|| IOS_DEFAULT_SERVER_URL.to_string());

        let mut preferences = persisted
            .as_ref()
            .map(|p| p.preferences.clone())
            .unwrap_or_default();
        if let Ok(loc) = std::env::var("GPUI_TELEGRAM_LOCALE") {
            if !loc.trim().is_empty() {
                preferences.language = AppLanguage::from_code(&loc).code().to_string();
            }
        }

        let proxy = persisted.as_ref().and_then(|p| p.proxy.clone());
        let host_cfg = PersistedHostConfig {
            server_url: resolved_url.clone(),
            preferences: preferences.clone(),
            proxy: proxy.clone(),
            passcode_phc: persisted.as_ref().and_then(|p| p.passcode_phc.clone()),
            apns_token: persisted.as_ref().and_then(|p| p.apns_token.clone()),
            voip_token: persisted.as_ref().and_then(|p| p.voip_token.clone()),
        };

        let initial_offline = std::env::var("GPUI_TELEGRAM_SIMULATE_OFFLINE")
            .ok()
            .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);

        let mut client_cfg = ClientConfig::default();
        client_cfg.server_url = resolved_url.clone();
        client_cfg.proxy = proxy;

        let store_dir = app_data_dir.join("cache");
        fs::create_dir_all(&store_dir)
            .map_err(|e| format!("Failed to create cache dir {}: {e}", store_dir.display()))?;

        let client = TelegramClient::open_with_dir(client_cfg, &store_dir)
            .map_err(|e| format!("Failed to initialize TelegramClient LocalStore: {e}"))?;
        if initial_offline {
            client.network_listener.set_online(false);
        }

        // Load multi-account store and restore active session if present
        let accounts_path = app_data_dir.join("accounts.json");
        let accounts_store: MultiAccountStore = fs::read_to_string(&accounts_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        if let Some(active_uid) = accounts_store.active_user_id {
            if let Some(sess) = accounts_store.accounts.get(&active_uid) {
                if sess.server_url == resolved_url {
                    client.set_auth_session(AuthResponse {
                        user_id: sess.user_id,
                        username: sess.username.clone(),
                        session_id: sess.session_id.clone(),
                        token: sess.token.clone(),
                        first_name: sess.username.clone(),
                        created_at: sess.created_at_ms,
                        expires_at: sess.created_at_ms + 30 * 86_400_000,
                    });
                }
            }
        }

        // Initialize 全球法布施 SQLite database inside app_data_dir
        let dharma_db_path = app_data_dir.join("fabushi_miniapp.sqlite3");
        let dharma_conn = Connection::open(&dharma_db_path)
            .map_err(|e| format!("Failed to open fabushi_miniapp SQLite DB: {e}"))?;
        init_fabushi_tables(&dharma_conn)
            .map_err(|e| format!("Failed to initialize fabushi_miniapp schema: {e}"))?;

        let hermes_dir = app_data_dir.join("hermes");
        fs::create_dir_all(&hermes_dir)
            .map_err(|e| format!("Failed to create hermes dir: {e}"))?;
        let agentic_store: LocalAgenticStore =
            fs::read_to_string(hermes_dir.join("agentic_store.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(LocalAgenticStore::new);

        let mut semantic_tree = SemanticWorkspaceTree {
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            window_layout: "CompactTab".to_string(),
            selected_chat_id: None,
            composer_text: String::new(),
            active_modal: None,
            is_call_active: false,
            window: Default::default(),
            nav_rail: Default::default(),
            sidebar: Default::default(),
            stage: Default::default(),
            composer: Default::default(),
            inspector: Default::default(),
            active_modal_node: None,
            state_hash: String::new(),
        };
        semantic_tree.window.layout = "CompactTab".to_string();
        semantic_tree.window.width_px = 393.0;
        semantic_tree.window.height_px = 852.0;
        semantic_tree.window.active_panel = "chats".to_string();
        semantic_tree.window.is_authenticated = client.current_user_id().is_some();
        semantic_tree.nav_rail.active_destination = "chats".to_string();
        semantic_tree.recompute_state_hash();

        let events = Arc::new(EventBridgeRegistry::new());
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let client_arc = Arc::new(client);

        // Background domain event pump forwarding WsTransport / Sync events over C-ABI callback
        {
            let pump_events = Arc::clone(&events);
            let pump_client = Arc::clone(&client_arc);
            let pump_stop = Arc::clone(&shutdown_flag);
            client_arc.runtime().spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_millis(50));
                while !pump_stop.load(Ordering::Relaxed) {
                    ticker.tick().await;
                    if pump_events.is_registered() {
                        for ev in pump_client.poll_events() {
                            pump_events.emit_domain_event(&ev);
                        }
                    }
                }
            });
        }

        let zeroized_pass = passphrase.filter(|s| !s.is_empty()).map(Zeroizing::new);

        Ok(Self {
            app_data_dir,
            is_test_mode,
            events,
            client: RwLock::new(client_arc),
            dedup_store: Mutex::new(DedupStore::new()),
            generation_fence: Mutex::new(GenerationFence::new()),
            cua_broker: Mutex::new(ComputerLeaseBroker::new()),
            call_engine: Mutex::new(WebRtcCallEngine::default()),
            ai_media_tap: DefaultAiMediaTap::new(16_000, 1, 0.05),
            talk_state: Mutex::new(TalkRuntimeState::default()),
            slash_registry: SlashCommandRegistry::new(),
            agentic_store: Mutex::new(agentic_store),
            task_registry: Mutex::new(HashMap::new()),
            tool_dock: Mutex::new(ToolPermissionDock::new()),
            magpie: Mutex::new(MagpieHostState::default()),
            dharma_db: Mutex::new(dharma_conn),
            wallet_service: TonWalletService::new(),
            accounts: Mutex::new(accounts_store),
            host_config: Mutex::new(host_cfg),
            semantic_tree: Mutex::new(semantic_tree),
            voice_gesture: Mutex::new(VoiceGestureState::Idle),
            simulate_offline: Mutex::new(initial_offline),
            is_backgrounded: AtomicBool::new(false),
            legacy_test_state: Mutex::new(LegacyTestHostState::default()),
            next_local_msg_id: AtomicI64::new(1_000),
            next_pts: AtomicU64::new(1),
            shutdown_flag,
            _passphrase: zeroized_pass,
        })
    }

    pub fn current_client(&self) -> Arc<TelegramClient> {
        Arc::clone(&self.client.read())
    }

    pub fn server_url(&self) -> String {
        self.host_config.lock().server_url.clone()
    }

    pub fn block_on<F, T>(&self, fut: F) -> T
    where
        F: std::future::Future<Output = T> + Send,
        T: Send,
    {
        let client = self.current_client();
        let rt = Arc::clone(client.runtime());
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|s| {
                s.spawn(move || rt.block_on(fut))
                    .join()
                    .expect("TelegramAppHost async worker panicked")
            })
        } else {
            rt.block_on(fut)
        }
    }

    pub fn persist_host_config(&self) -> Result<(), BridgeError> {
        let cfg = self.host_config.lock().clone();
        let raw = serde_json::to_string_pretty(&cfg)
            .map_err(|e| BridgeError::internal(format!("Serialize host_config failed: {e}")))?;
        fs::write(self.app_data_dir.join("host_config.json"), raw.as_bytes())
            .map_err(|e| BridgeError::storage(format!("Write host_config.json failed: {e}")))
    }

    pub fn persist_accounts(&self) -> Result<(), BridgeError> {
        let store = self.accounts.lock().clone();
        let raw = serde_json::to_string_pretty(&store)
            .map_err(|e| BridgeError::internal(format!("Serialize accounts.json failed: {e}")))?;
        fs::write(self.app_data_dir.join("accounts.json"), raw.as_bytes())
            .map_err(|e| BridgeError::storage(format!("Write accounts.json failed: {e}")))
    }

    pub fn record_authenticated_session(&self, auth: &AuthResponse) {
        let server_url = self.server_url();
        let stored = StoredSession {
            server_url,
            user_id: auth.user_id,
            username: auth.username.clone(),
            token: auth.token.clone(),
            session_id: auth.session_id.clone(),
            created_at_ms: auth.created_at,
        };
        {
            let mut acc = self.accounts.lock();
            if acc.accounts.len() < 6 || acc.accounts.contains_key(&auth.user_id) {
                acc.accounts.insert(auth.user_id, stored);
            }
            acc.active_user_id = Some(auth.user_id);
        }
        let _ = self.persist_accounts();
        {
            let mut tree = self.semantic_tree.lock();
            tree.window.is_authenticated = true;
            tree.recompute_state_hash();
        }
        self.events.emit(
            "authStateChanged",
            &json!({
                "authenticated": true,
                "user_id": auth.user_id,
                "username": auth.username,
            }),
        );
    }

    pub fn set_passcode(&self, raw_pin: &str) -> Result<(), BridgeError> {
        if raw_pin.trim().len() < 4 {
            return Err(BridgeError::invalid_params(
                "Passcode must be at least 4 characters",
            ));
        }
        let salt_bytes: [u8; 16] = *uuid::Uuid::new_v4().as_bytes();
        let salt = SaltString::encode_b64(&salt_bytes)
            .map_err(|e| BridgeError::crypto(format!("Salt generation failed: {e}")))?;
        let phc = Argon2::default()
            .hash_password(raw_pin.as_bytes(), &salt)
            .map_err(|e| BridgeError::crypto(format!("Argon2id hash failed: {e}")))?
            .to_string();
        self.host_config.lock().passcode_phc = Some(phc);
        self.persist_host_config()
    }

    pub fn verify_passcode(&self, raw_pin: &str) -> Result<bool, BridgeError> {
        let Some(phc) = self.host_config.lock().passcode_phc.clone() else {
            return Ok(true);
        };
        let parsed = PasswordHash::new(&phc)
            .map_err(|e| BridgeError::crypto(format!("Corrupt stored passcode hash: {e}")))?;
        Ok(Argon2::default()
            .verify_password(raw_pin.as_bytes(), &parsed)
            .is_ok())
    }

    /// ScenePhase `.background` handler: suspends WS/Outbox/Tailnet and checkpoints SQLite WAL
    /// so iOS `0xdead10cc` never kills the suspended process.
    pub fn enter_background(&self) -> Value {
        self.is_backgrounded.store(true, Ordering::SeqCst);
        let client = self.current_client();
        client.disconnect_ws();
        client.outbox.stop();
        client.stop_tailnet_mesh();
        let _ = self
            .dharma_db
            .lock()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        self.events.emit(
            "connectionStateChanged",
            &json!({ "state": "suspended_background" }),
        );
        json!({
            "backgrounded": true,
            "wal_checkpointed": true,
            "ws_disconnected": true
        })
    }

    /// ScenePhase `.active` handler: restores network state, reconnects WS/Outbox when authenticated.
    pub fn resume_foreground(&self) -> Value {
        self.is_backgrounded.store(false, Ordering::SeqCst);
        let client = self.current_client();
        let offline = *self.simulate_offline.lock();
        client.network_listener.set_online(!offline);
        let has_token = client.token().is_some();
        if has_token && !offline {
            client.connect_ws();
        }
        self.events.emit(
            "connectionStateChanged",
            &json!({ "state": if offline { "offline" } else { "online" } }),
        );
        json!({
            "backgrounded": false,
            "reconnected": has_token && !offline,
            "has_session": has_token
        })
    }

    /// Generic HTTP JSON helper hitting the configured `gpui_telegram_server` URL (`http://34.46.77.133:8080` or loopback).
    pub fn api_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        require_auth: bool,
    ) -> Result<Value, BridgeError> {
        let client = self.current_client();
        let token_opt = client.token();
        if require_auth && token_opt.is_none() {
            return Err(BridgeError::unauthorized("No active session token"));
        }
        let base = self.server_url();
        let full_url = format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'));
        let device_id = client.device_id();

        self.block_on(async move {
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|e| BridgeError::network(e.to_string()))?;

            let mut req = http
                .request(method, &full_url)
                .header("X-Device-Id", device_id);
            if let Some(tok) = token_opt {
                req = req.bearer_auth(tok);
            }
            if let Some(json_body) = body {
                req = req.json(&json_body);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| BridgeError::network(e.to_string()))?;
            let status = resp.status().as_u16();
            let text = resp
                .text()
                .await
                .map_err(|e| BridgeError::network(e.to_string()))?;
            if !(200..300).contains(&status) {
                let err_msg = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| {
                        v.get("error")
                            .or_else(|| v.get("message"))
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                    })
                    .unwrap_or_else(|| text.clone());
                return Err(BridgeError::with_http_status(
                    match status {
                        401 => "unauthorized",
                        403 => "forbidden",
                        404 => "not_found",
                        409 => "conflict",
                        429 => "rate_limited",
                        400 | 422 => "invalid_params",
                        _ => "network_error",
                    },
                    err_msg,
                    status,
                ));
            }
            if text.trim().is_empty() {
                Ok(json!({ "ok": true }))
            } else {
                serde_json::from_str(&text)
                    .or_else(|_| Ok(json!({ "raw": text })))
            }
        })
    }

    pub fn shutdown(&self) {
        self.shutdown_flag.store(true, Ordering::SeqCst);
        self.events.clear();
        let client = self.current_client();
        client.disconnect_ws();
        client.stop_tailnet_mesh();
        client.outbox.stop();
    }
}
