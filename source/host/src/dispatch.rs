use crate::compat::{dispatch_legacy_test_method, LegacyTestHostState};
use crate::error::{ok_envelope, BridgeError};
use crate::events::EventBridgeRegistry;
use gpui_telegram_agent::ComputerLeaseBroker;
use gpui_telegram_client::config::ClientConfig;
use gpui_telegram_client::net::{LoginRequest, RegisterRequest};
use gpui_telegram_client::outbox::NetworkState;
use gpui_telegram_client::TelegramClient;
use gpui_telegram_core::types::{ChatId, MessageId, PeerId, UserId};
use gpui_telegram_media::{decode_waveform_to_floats, encode_waveform_from_floats};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use gpui_telegram_webrtc::media::{AiMediaTap, DefaultAiMediaTap};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use zeroize::Zeroizing;

pub const IOS_DEFAULT_SERVER_URL: &str = "http://104.197.143.175:8080";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedServerConfig {
    server_url: String,
    #[serde(default)]
    locale: String,
}

#[derive(Debug, Deserialize)]
pub struct RpcEnvelope {
    /// Supports both `{ "command": "...", "params": {...} }` (`TelegramBridgeRuntime`)
    /// and `{ "method": "...", "params": {...} }` (`MahayanaHostRuntime`).
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub handle_id: Option<u64>,
    #[serde(default)]
    pub params: Value,
}

pub struct TelegramAppHost {
    pub app_data_dir: PathBuf,
    pub is_test_mode: bool,
    pub events: Arc<EventBridgeRegistry>,
    pub client: RwLock<Arc<TelegramClient>>,
    pub cua_broker: Mutex<ComputerLeaseBroker>,
    pub ai_media_tap: DefaultAiMediaTap,
    server_url: Mutex<String>,
    locale: Mutex<String>,
    simulate_offline: Mutex<bool>,
    legacy_test_state: Mutex<LegacyTestHostState>,
    next_local_msg_id: AtomicI64,
    next_pts: AtomicU64,
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

        let server_cfg_path = app_data_dir.join("server_config.json");
        let persisted: Option<PersistedServerConfig> = fs::read_to_string(&server_cfg_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());

        let env_url = std::env::var("GPUI_TELEGRAM_SERVER_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let resolved_url = env_url
            .or_else(|| persisted.as_ref().map(|c| c.server_url.clone()))
            .map(|u| ClientConfig::normalize_url(&u))
            .unwrap_or_else(|| IOS_DEFAULT_SERVER_URL.to_string());

        let resolved_locale = std::env::var("GPUI_TELEGRAM_LOCALE")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| {
                persisted
                    .as_ref()
                    .map(|c| c.locale.trim().to_string())
                    .filter(|v| !v.is_empty())
            })
            .unwrap_or_else(|| "en".to_string());

        let initial_offline = std::env::var("GPUI_TELEGRAM_SIMULATE_OFFLINE")
            .ok()
            .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);

        let mut cfg = ClientConfig::default();
        cfg.server_url = resolved_url.clone();

        let store_dir = app_data_dir.join("cache");
        fs::create_dir_all(&store_dir)
            .map_err(|e| format!("Failed to create cache dir {}: {e}", store_dir.display()))?;

        let client = TelegramClient::open_with_dir(cfg, &store_dir)
            .map_err(|e| format!("Failed to initialize TelegramClient LocalStore: {e}"))?;
        if initial_offline {
            client.network_listener.set_state(NetworkState::Offline);
        }

        let events = Arc::new(EventBridgeRegistry::new());
        let cua_broker = Mutex::new(ComputerLeaseBroker::new());
        let ai_media_tap = DefaultAiMediaTap::new(16_000, 1, 0.05);
        let zeroized_pass = passphrase.filter(|s| !s.is_empty()).map(Zeroizing::new);

        Ok(Self {
            app_data_dir,
            is_test_mode,
            events,
            client: RwLock::new(Arc::new(client)),
            cua_broker,
            ai_media_tap,
            server_url: Mutex::new(resolved_url),
            locale: Mutex::new(resolved_locale),
            simulate_offline: Mutex::new(initial_offline),
            legacy_test_state: Mutex::new(LegacyTestHostState::default()),
            next_local_msg_id: AtomicI64::new(1_000),
            next_pts: AtomicU64::new(1),
            _passphrase: zeroized_pass,
        })
    }

    pub fn current_client(&self) -> Arc<TelegramClient> {
        Arc::clone(&self.client.read())
    }

    pub fn shutdown(&self) {
        self.events.clear();
        let client = self.current_client();
        client.disconnect_ws();
        client.stop_tailnet_mesh();
    }

    fn save_persisted_config(&self, server_url: &str, locale: &str) -> Result<(), BridgeError> {
        let cfg = PersistedServerConfig {
            server_url: server_url.to_string(),
            locale: locale.to_string(),
        };
        let encoded = serde_json::to_string_pretty(&cfg)
            .map_err(|e| BridgeError::internal(format!("Failed to serialize server_config: {e}")))?;
        fs::write(self.app_data_dir.join("server_config.json"), encoded.as_bytes())
            .map_err(|e| BridgeError::internal(format!("Failed to write server_config.json: {e}")))
    }

    fn extract_text(content: &MessageContent) -> String {
        match content {
            MessageContent::Text { text, .. } => text.clone(),
            MessageContent::Media(item) => item.caption.clone().unwrap_or_default(),
            MessageContent::Photo { caption, .. } | MessageContent::Video { caption, .. } => {
                caption.clone().unwrap_or_default()
            }
            MessageContent::Document { file_name, .. } => file_name.clone(),
            MessageContent::Voice(_) => "[Voice Message]".to_string(),
            MessageContent::VideoNote(_) => "[Video Message]".to_string(),
            MessageContent::Audio { title, .. } => {
                title.clone().unwrap_or_else(|| "[Audio]".to_string())
            }
            MessageContent::Poll(poll) => poll.question.clone(),
            MessageContent::Todo(todo) => todo.title.clone(),
            MessageContent::Invoice(inv) => inv.title.clone(),
            MessageContent::GenerativeCard(card) => card.title.clone(),
            MessageContent::Encrypted(_) => "[Encrypted Message]".to_string(),
        }
    }

    pub fn dispatch(&self, request_json: &str) -> String {
        let env: RpcEnvelope = match serde_json::from_str(request_json) {
            Ok(parsed) => parsed,
            Err(e) => return BridgeError::invalid_json(format!("Malformed JSON: {e}")).to_envelope_string(),
        };

        let Some(cmd) = env
            .command
            .as_deref()
            .or(env.method.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return BridgeError::new(
                "MISSING_COMMAND",
                "Missing non-empty 'command' or 'method' field",
            )
            .to_envelope_string();
        };

        if self.is_test_mode && cmd == "debug.trigger_panic" {
            panic!("Intentional test panic for FFI catch_unwind verification");
        }

        if self.is_test_mode {
            if let Some(legacy_reply) =
                dispatch_legacy_test_method(&self.legacy_test_state, cmd, &env.params)
            {
                return legacy_reply;
            }
        }

        let client = self.current_client();

        match cmd {
            // ================================================================
            // 1. Connectivity, Server Configuration & Diagnostics (`config.*` + `telegram.ping`)
            // ================================================================
            "telegram.ping" | "config.ping" => {
                let nonce = env.params.get("nonce").and_then(Value::as_u64).unwrap_or(0);
                let url = self.server_url.lock().clone();
                ok_envelope(json!({
                    "pong": true,
                    "nonce": nonce,
                    "version": env!("CARGO_PKG_VERSION"),
                    "serverUrl": url,
                    "isTestMode": self.is_test_mode
                }))
            }

            "config.get_server_config" => {
                let url = self.server_url.lock().clone();
                let locale = self.locale.lock().clone();
                let offline = *self.simulate_offline.lock();
                ok_envelope(json!({
                    "server_url": url,
                    "locale": locale,
                    "simulate_offline": offline,
                    "device_id": client.device_id(),
                    "version": env!("CARGO_PKG_VERSION"),
                }))
            }

            "config.set_server_config" => {
                let Some(raw_url) = env.params.get("server_url").and_then(Value::as_str) else {
                    return BridgeError::invalid_params("Missing server_url").to_envelope_string();
                };
                if raw_url.trim().is_empty() {
                    return BridgeError::invalid_params("server_url cannot be empty")
                        .to_envelope_string();
                }
                let normalized = ClientConfig::normalize_url(raw_url);
                let next_locale = env
                    .params
                    .get("locale")
                    .and_then(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| self.locale.lock().clone());

                if let Err(err) = self.save_persisted_config(&normalized, &next_locale) {
                    return err.to_envelope_string();
                }

                let prev_url = {
                    let mut guard = self.server_url.lock();
                    let prev = guard.clone();
                    *guard = normalized.clone();
                    prev
                };
                *self.locale.lock() = next_locale.clone();
                if prev_url != normalized {
                    client.disconnect_ws();
                    client.stop_tailnet_mesh();
                    client.clear_session();
                    client.local_store.clear_all();

                    let mut next_cfg = ClientConfig::default();
                    next_cfg.server_url = normalized.clone();
                    let store_dir = self.app_data_dir.join("cache");
                    match TelegramClient::open_with_dir(next_cfg, &store_dir) {
                        Ok(new_client) => {
                            if *self.simulate_offline.lock() {
                                new_client.network_listener.set_state(NetworkState::Offline);
                            }
                            *self.client.write() = Arc::new(new_client);
                        }
                        Err(e) => {
                            return BridgeError::internal(format!(
                                "Failed to rebind TelegramClient to {normalized}: {e}"
                            ))
                            .to_envelope_string();
                        }
                    }
                }

                ok_envelope(json!({
                    "server_url": normalized,
                    "locale": next_locale,
                    "switched": prev_url != normalized
                }))
            }

            "config.ping_health" => {
                let http = client.http_client.clone();
                let start = std::time::Instant::now();
                let healthy = client
                    .runtime()
                    .block_on(async move { http.check_health().await });
                let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
                match healthy {
                    Ok(ok) => ok_envelope(json!({
                        "healthy": ok,
                        "latency_ms": latency_ms,
                        "server_url": self.server_url.lock().clone()
                    })),
                    Err(err) => ok_envelope(json!({
                        "healthy": false,
                        "latency_ms": latency_ms,
                        "server_url": self.server_url.lock().clone(),
                        "error": err.to_string()
                    })),
                }
            }

            "config.set_simulate_offline" => {
                let offline = env
                    .params
                    .get("offline")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                *self.simulate_offline.lock() = offline;
                client.network_listener.set_state(if offline {
                    NetworkState::Offline
                } else {
                    NetworkState::Online
                });
                let state_str = if offline { "offline" } else { "online" };
                self.events
                    .emit("connectionStateChanged", &json!({ "state": state_str }));
                ok_envelope(json!({
                    "simulate_offline": offline,
                    "state": state_str
                }))
            }

            "config.check_release_update" => {
                let current_version = env!("CARGO_PKG_VERSION");
                ok_envelope(json!({
                    "current_version": current_version,
                    "latest_version": current_version,
                    "update_available": false,
                    "channel": "ios-testflight"
                }))
            }

            // ================================================================
            // 2. Cloud Authentication Baseline (`auth.*`)
            // ================================================================
            "auth.login_password" => {
                let Some(username) = env.params.get("username").and_then(Value::as_str) else {
                    return BridgeError::invalid_params("Missing username").to_envelope_string();
                };
                let Some(password) = env.params.get("password").and_then(Value::as_str) else {
                    return BridgeError::invalid_params("Missing password").to_envelope_string();
                };
                let device_name = env
                    .params
                    .get("device_name")
                    .and_then(Value::as_str)
                    .unwrap_or("Fabushi iOS")
                    .to_string();
                let req = LoginRequest {
                    username: username.to_string(),
                    password: password.to_string(),
                    device_name,
                };
                let http = client.http_client.clone();
                match client.runtime().block_on(async move { http.login(&req).await }) {
                    Ok(auth) => {
                        let url = self.server_url.lock().clone();
                        client.local_store.bind_account_scope(&url, auth.user_id);
                        client.set_auth_session(auth.clone());
                        ok_envelope(serde_json::to_value(auth).unwrap_or_else(|_| json!({})))
                    }
                    Err(err) => {
                        BridgeError::new("AUTH_LOGIN_FAILED", err.to_string()).to_envelope_string()
                    }
                }
            }

            "auth.register_password" => {
                let Some(username) = env.params.get("username").and_then(Value::as_str) else {
                    return BridgeError::invalid_params("Missing username").to_envelope_string();
                };
                let Some(password) = env.params.get("password").and_then(Value::as_str) else {
                    return BridgeError::invalid_params("Missing password").to_envelope_string();
                };
                let first_name = env
                    .params
                    .get("first_name")
                    .and_then(Value::as_str)
                    .unwrap_or(username)
                    .to_string();
                let device_name = env
                    .params
                    .get("device_name")
                    .and_then(Value::as_str)
                    .unwrap_or("Fabushi iOS")
                    .to_string();
                let req = RegisterRequest {
                    username: username.to_string(),
                    password: password.to_string(),
                    first_name,
                    device_name,
                };
                let http = client.http_client.clone();
                match client
                    .runtime()
                    .block_on(async move { http.register(&req).await })
                {
                    Ok(auth) => {
                        let url = self.server_url.lock().clone();
                        client.local_store.bind_account_scope(&url, auth.user_id);
                        client.set_auth_session(auth.clone());
                        ok_envelope(serde_json::to_value(auth).unwrap_or_else(|_| json!({})))
                    }
                    Err(err) => BridgeError::new("AUTH_REGISTER_FAILED", err.to_string())
                        .to_envelope_string(),
                }
            }

            "auth.get_me" => {
                let token = env
                    .params
                    .get("token")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .or_else(|| client.token());
                let Some(token) = token else {
                    return BridgeError::with_http_status(
                        "UNAUTHORIZED",
                        "No active authentication token",
                        401,
                    )
                    .to_envelope_string();
                };
                let http = client.http_client.clone();
                match client
                    .runtime()
                    .block_on(async move { http.get_me(&token).await })
                {
                    Ok(me) => ok_envelope(serde_json::to_value(me).unwrap_or_else(|_| json!({}))),
                    Err(err) => {
                        BridgeError::new("AUTH_GET_ME_FAILED", err.to_string()).to_envelope_string()
                    }
                }
            }

            "auth.logout" => {
                client.disconnect_ws();
                client.clear_session();
                client.local_store.clear_all();
                ok_envelope(json!({ "logged_out": true }))
            }

            // ================================================================
            // 3. Truthful LocalStore Dialogs, Messages & PTS (`dialogs.*` / `messages.*` / `telegram.*`)
            // ================================================================
            "telegram.listChats" | "dialogs.list_chats" => {
                let chats = client.local_store.list_chats();
                let serialized: Vec<Value> = chats
                    .into_iter()
                    .map(|c| {
                        let last_text = client
                            .local_store
                            .get_messages(ChatId::new(c.chat_id))
                            .ok()
                            .and_then(|msgs| msgs.last().map(|m| Self::extract_text(&m.content)))
                            .unwrap_or_default();
                        json!({
                            "id": c.chat_id,
                            "chat_id": c.chat_id,
                            "title": c.title,
                            "kind": c.kind,
                            "unreadCount": c.unread_count,
                            "unread_count": c.unread_count,
                            "lastMessage": last_text,
                            "lastMessageTimestamp": c.last_message_date_ms,
                            "isPinned": c.is_pinned,
                            "isMuted": c.is_muted,
                            "isArchived": c.is_archived,
                            "isForum": c.is_forum,
                            "isOnline": false,
                            "deliveryStatus": "read",
                            "pts": c.pts
                        })
                    })
                    .collect();
                ok_envelope(json!({ "chats": serialized }))
            }

            "telegram.sendMessage" | "messages.send" => {
                let Some(chat_id) = env.params.get("chat_id").and_then(Value::as_i64) else {
                    return BridgeError::invalid_params("Missing or invalid chat_id")
                        .to_envelope_string();
                };
                let Some(text) = env.params.get("text").and_then(Value::as_str) else {
                    return BridgeError::invalid_params("Missing text").to_envelope_string();
                };
                if text.trim().is_empty() {
                    return BridgeError::invalid_params("Message text cannot be empty")
                        .to_envelope_string();
                }

                let sender_id = env
                    .params
                    .get("sender_id")
                    .and_then(Value::as_u64)
                    .or_else(|| client.current_user_id())
                    .unwrap_or(1);
                let client_nonce = env
                    .params
                    .get("client_nonce")
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| client.next_client_nonce());

                let chat_id_typed = ChatId::new(chat_id);
                if let Ok(existing) = client.local_store.get_messages(chat_id_typed) {
                    if let Some(found) = existing
                        .into_iter()
                        .find(|m| m.client_nonce == Some(client_nonce))
                    {
                        return ok_envelope(json!({
                            "message_id": found.id.0,
                            "chat_id": chat_id,
                            "client_nonce": client_nonce,
                            "pts": self.next_pts.load(Ordering::SeqCst),
                            "deduplicated": true,
                            "timestamp": found.date.timestamp_millis(),
                            "delivery_status": "sent"
                        }));
                    }
                }

                let msg_id_raw = self.next_local_msg_id.fetch_add(1, Ordering::SeqCst);
                let pts = self.next_pts.fetch_add(1, Ordering::SeqCst);
                let mut item = MessageItem::new(
                    MessageId::new(msg_id_raw),
                    chat_id_typed,
                    UserId::new(sender_id),
                    PeerId::Chat(chat_id_typed),
                    MessageContent::plain_text(text),
                );
                item.client_nonce = Some(client_nonce);
                let ts_ms = item.date.timestamp_millis();

                client.local_store.insert_message(item);
                self.events.emit(
                    "newMessage",
                    &json!({
                        "id": msg_id_raw,
                        "chat_id": chat_id,
                        "sender_id": sender_id,
                        "text": text,
                        "timestamp": ts_ms,
                        "client_nonce": client_nonce
                    }),
                );
                self.events.emit(
                    "ptsUpdate",
                    &json!({
                        "pts": pts,
                        "pts_count": 1
                    }),
                );

                ok_envelope(json!({
                    "message_id": msg_id_raw,
                    "chat_id": chat_id,
                    "client_nonce": client_nonce,
                    "pts": pts,
                    "deduplicated": false,
                    "timestamp": ts_ms,
                    "delivery_status": "sent"
                }))
            }

            "telegram.getMessages" | "messages.get_history" => {
                let Some(chat_id) = env.params.get("chat_id").and_then(Value::as_i64) else {
                    return BridgeError::invalid_params("Missing chat_id").to_envelope_string();
                };
                match client.local_store.get_messages(ChatId::new(chat_id)) {
                    Ok(items) => {
                        let list: Vec<Value> = items
                            .into_iter()
                            .map(|m| {
                                json!({
                                    "id": m.id.0,
                                    "chat_id": m.chat_id.0,
                                    "sender_id": m.sender_id.0,
                                    "text": Self::extract_text(&m.content),
                                    "timestamp": m.date.timestamp_millis(),
                                    "client_nonce": m.client_nonce,
                                    "is_outgoing": m.flags.out,
                                })
                            })
                            .collect();
                        ok_envelope(json!({ "messages": list }))
                    }
                    Err(e) => {
                        BridgeError::internal(format!("Failed to load messages: {e}"))
                            .to_envelope_string()
                    }
                }
            }

            "telegram.syncPts" => {
                let local_pts = env.params.get("local_pts").and_then(Value::as_u64).unwrap_or(0);
                let current_pts = self.next_pts.load(Ordering::SeqCst).saturating_sub(1);
                let has_gap = current_pts > local_pts;
                if has_gap {
                    self.events.emit(
                        "syncDifference",
                        &json!({
                            "pts": current_pts,
                            "has_more": false
                        }),
                    );
                }
                ok_envelope(json!({
                    "local_pts": local_pts,
                    "server_pts": current_pts,
                    "gap_recovered": has_gap
                }))
            }

            // ================================================================
            // 4. Media Waveforms, VAD Barge-in & CUA Lease Fencing
            // ================================================================
            "telegram.media.encodeWaveform" => {
                let samples: Vec<f32> = env
                    .params
                    .get("samples")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_f64().map(|f| f as f32))
                            .collect()
                    })
                    .unwrap_or_default();
                let encoded = encode_waveform_from_floats(&samples);
                ok_envelope(json!({
                    "waveform_bytes": encoded,
                    "byte_count": encoded.len()
                }))
            }

            "telegram.media.decodeWaveform" => {
                let raw_bytes: Vec<u8> = env
                    .params
                    .get("waveform_bytes")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_u64().map(|b| (b & 0xFF) as u8))
                            .collect()
                    })
                    .unwrap_or_default();
                let bar_count = env
                    .params
                    .get("bar_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(100) as usize;
                let floats = decode_waveform_to_floats(&raw_bytes, bar_count);
                ok_envelope(json!({ "bars": floats }))
            }

            "telegram.vad.processFrame" => {
                let pcm: Vec<i16> = env
                    .params
                    .get("pcm")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_i64().map(|s| s.clamp(-32768, 32767) as i16))
                            .collect()
                    })
                    .unwrap_or_default();
                let sample_rate = env
                    .params
                    .get("sample_rate")
                    .and_then(Value::as_u64)
                    .unwrap_or(16_000) as u32;
                self.ai_media_tap.on_incoming_pcm_frame(&pcm, sample_rate, 1);
                let barge_in = self.ai_media_tap.trigger_vad_barge_in();
                ok_envelope(json!({
                    "barge_in_triggered": barge_in,
                    "incoming_rms": self.ai_media_tap.last_incoming_rms()
                }))
            }

            "telegram.cua.acquireLease" => {
                let agent_id = env
                    .params
                    .get("agent_id")
                    .and_then(Value::as_str)
                    .unwrap_or("ios-agent");
                let device_id = env
                    .params
                    .get("device_id")
                    .and_then(Value::as_str)
                    .unwrap_or("ios-device");
                let mut broker = self.cua_broker.lock();
                match broker.acquire_lease(agent_id, device_id) {
                    Some(lease) => ok_envelope(json!({
                        "acquired": true,
                        "lease_id": lease.lease_id,
                        "generation": lease.generation,
                        "window_index": lease.window_index
                    })),
                    None => ok_envelope(json!({ "acquired": false })),
                }
            }

            "telegram.cua.emergencyTakeover" => {
                let mut broker = self.cua_broker.lock();
                let new_gen = broker.emergency_takeover();
                ok_envelope(json!({
                    "preempted": true,
                    "new_generation": new_gen
                }))
            }

            "telegram.cua.releaseLease" => {
                let lease_id = env
                    .params
                    .get("lease_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let mut broker = self.cua_broker.lock();
                let released = broker.release_lease(lease_id);
                ok_envelope(json!({
                    "released": released,
                    "generation": broker.current_generation()
                }))
            }

            // ================================================================
            // Fail-Closed Rejection (Zero wildcard 'acknowledged' stubs!)
            // ================================================================
            other => BridgeError::unknown_command(other).to_envelope_string(),
        }
    }
}
