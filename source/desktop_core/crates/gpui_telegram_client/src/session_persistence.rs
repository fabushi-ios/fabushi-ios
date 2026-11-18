//! Persistent Client Session, Multi-Account Store, Export Engine & Wallet Ledger
//! Bridges `gpui_telegram_desktop` runtime with `TelegramClient`, `LocalStore`,
//! `ClientSyncCoordinator`, `ExportManager`, and TON/Stars `Wallet`.

use crate::client::TelegramClient;
use gpui_telegram_core::types::{ChatId, MessageId, PeerId, UserId};
use gpui_telegram_proto::message::{
    MessageContent as ProtoContent, MessageItem as ProtoMessageItem,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountSessionRecord {
    pub account_id: u32,
    pub display_name: String,
    pub username: String,
    pub phone: String,
    pub bio: String,
    pub emoji_status: String,
    pub is_premium: bool,
    pub is_verified: bool,
    pub avatar_initials: String,
    pub avatar_color: u32,
    pub unread_chats: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProxyEndpointRecord {
    pub id: u32,
    pub kind: String, // "Direct", "SOCKS5", "HTTP"
    pub host: String,
    pub port: u16,
    pub secret_or_user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub ping_ms: u32,
    pub enabled: bool,
}

impl ProxyEndpointRecord {
    pub fn tg_deep_link(&self) -> String {
        match self.kind.as_str() {
            "MTProto" => format!(
                "tg://proxy?server={}&port={}&secret={}",
                self.host,
                self.port,
                if self.secret_or_user.is_empty() {
                    "ee00000000000000000000000000000001"
                } else {
                    &self.secret_or_user
                }
            ),
            "SOCKS5" => format!("tg://socks?server={}&port={}", self.host, self.port),
            _ => format!("http://{}:{}", self.host, self.port),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WalletTransactionRecord {
    pub tx_id: String,
    pub title: String,
    pub peer: String,
    pub amount_ton: f64,
    pub amount_stars: i64,
    pub timestamp: String,
    pub is_incoming: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveDeviceSession {
    pub session_id: String,
    pub device_model: String,
    pub platform: String,
    pub ip_region: String,
    pub last_active: String,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedClientState {
    pub active_account_id: u32,
    pub accounts: Vec<AccountSessionRecord>,
    pub custom_folders: Vec<(String, u32)>,
    pub drafts_by_chat: HashMap<u64, String>,
    pub archived_chat_ids: Vec<u64>,
    pub blocked_peers: Vec<String>,
    pub local_passcode_pin: Option<String>,
    pub two_factor_password_hint: Option<String>,
    pub auto_delete_ttl_label: String,
    pub theme_name: String,
    pub accent_color: u32,
    pub font_size_px: u32,
    pub bubble_radius_px: u32,
    pub send_with_enter: bool,
    pub language_id: String,
    pub show_translate_bar: bool,
    pub proxies: Vec<ProxyEndpointRecord>,
    pub active_devices: Vec<ActiveDeviceSession>,
    pub ton_balance: f64,
    pub stars_balance: u64,
    pub wallet_transactions: Vec<WalletTransactionRecord>,
    pub pts_counter: u64,
    pub cache_media_mb: f64,
    pub cache_files_mb: f64,
}

impl Default for PersistedClientState {
    fn default() -> Self {
        Self::empty()
    }
}

impl PersistedClientState {
    pub fn new_demo() -> Self {
        Self {
            active_account_id: 1,
            accounts: vec![
                AccountSessionRecord {
                    account_id: 1,
                    display_name: "Bhrum Developer".to_string(),
                    username: "bhrum_dev".to_string(),
                    phone: "+1 541 555 0145".to_string(),
                    bio: "Rust & GPUI 120fps Desktop Architecture • Modern Protocol".to_string(),
                    emoji_status: "🦀".to_string(),
                    is_premium: true,
                    is_verified: true,
                    avatar_initials: "BD".to_string(),
                    avatar_color: 0x2AABEE,
                    unread_chats: 8,
                },
                AccountSessionRecord {
                    account_id: 2,
                    display_name: "Research Lab".to_string(),
                    username: "dharani_lab".to_string(),
                    phone: "+86 138 0013 8000".to_string(),
                    bio: "Sanskrit Phonology & Compression Systems".to_string(),
                    emoji_status: "🪷".to_string(),
                    is_premium: true,
                    is_verified: false,
                    avatar_initials: "RL".to_string(),
                    avatar_color: 0x9B59B6,
                    unread_chats: 2,
                },
            ],
            custom_folders: Vec::new(),
            drafts_by_chat: HashMap::new(),
            archived_chat_ids: vec![5],
            blocked_peers: vec!["@spam_crypto_bot".to_string()],
            local_passcode_pin: None,
            two_factor_password_hint: Some("GPUI Cloud Vault".to_string()),
            auto_delete_ttl_label: "Off".to_string(),
            theme_name: "Night".to_string(),
            accent_color: 0x2AABEE,
            font_size_px: 14,
            bubble_radius_px: 14,
            send_with_enter: true,
            language_id: "zh-CN".to_string(),
            show_translate_bar: true,
            proxies: vec![
                ProxyEndpointRecord {
                    id: 1,
                    kind: "Direct".to_string(),
                    host: "149.154.167.50".to_string(),
                    port: 443,
                    secret_or_user: String::new(),
                    username: None,
                    password: None,
                    ping_ms: 18,
                    enabled: true,
                },
                ProxyEndpointRecord {
                    id: 2,
                    kind: "HTTP".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 8081,
                    secret_or_user: String::new(),
                    username: None,
                    password: None,
                    ping_ms: 12,
                    enabled: false,
                },
                ProxyEndpointRecord {
                    id: 3,
                    kind: "SOCKS5".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 1080,
                    secret_or_user: String::new(),
                    username: None,
                    password: None,
                    ping_ms: 4,
                    enabled: false,
                },
            ],
            active_devices: vec![
                ActiveDeviceSession {
                    session_id: "sess-mac-gpui-01".to_string(),
                    device_model: "Apple MacBook Pro (M-Series Metal GPUI)".to_string(),
                    platform: "Telegram Desktop GPUI 7.4.2 • macOS".to_string(),
                    ip_region: "Eugene, OR, United States".to_string(),
                    last_active: "Online now".to_string(),
                    is_current: true,
                },
                ActiveDeviceSession {
                    session_id: "sess-ios-02".to_string(),
                    device_model: "iPhone 16 Pro".to_string(),
                    platform: "Telegram iOS 11.4".to_string(),
                    ip_region: "Oregon, United States".to_string(),
                    last_active: "2 hours ago".to_string(),
                    is_current: false,
                },
                ActiveDeviceSession {
                    session_id: "sess-android-03".to_string(),
                    device_model: "Pixel 9 Pro (arm64-v8a)".to_string(),
                    platform: "Telegram Android 11.4".to_string(),
                    ip_region: "Oregon, United States".to_string(),
                    last_active: "Yesterday".to_string(),
                    is_current: false,
                },
            ],
            ton_balance: 128.45,
            stars_balance: 2450,
            wallet_transactions: vec![
                WalletTransactionRecord {
                    tx_id: "TON-TX-90812".to_string(),
                    title: "Channel Ad Revenue Share".to_string(),
                    peer: "Rust Core Devs".to_string(),
                    amount_ton: 42.50,
                    amount_stars: 0,
                    timestamp: "Today, 11:20".to_string(),
                    is_incoming: true,
                },
                WalletTransactionRecord {
                    tx_id: "STARS-TX-77410".to_string(),
                    title: "Cloud Agent Workspace Subscription".to_string(),
                    peer: "@agent_orchestrator_bot".to_string(),
                    amount_ton: 0.0,
                    amount_stars: -250,
                    timestamp: "Yesterday, 14:38".to_string(),
                    is_incoming: false,
                },
                WalletTransactionRecord {
                    tx_id: "TON-TX-65109".to_string(),
                    title: "TON Deposit from Fragment".to_string(),
                    peer: "Fragment.com".to_string(),
                    amount_ton: 85.95,
                    amount_stars: 0,
                    timestamp: "Nov 10, 09:15".to_string(),
                    is_incoming: true,
                },
            ],
            pts_counter: 1024,
            cache_media_mb: 184.6,
            cache_files_mb: 62.4,
        }
    }
}

impl PersistedClientState {
    pub fn empty() -> Self {
        Self {
            active_account_id: 0,
            accounts: Vec::new(),
            custom_folders: Vec::new(),
            drafts_by_chat: HashMap::new(),
            archived_chat_ids: Vec::new(),
            blocked_peers: Vec::new(),
            local_passcode_pin: None,
            two_factor_password_hint: None,
            auto_delete_ttl_label: "Off".to_string(),
            theme_name: "Night".to_string(),
            accent_color: 0x2AABEE,
            font_size_px: 14,
            bubble_radius_px: 14,
            send_with_enter: true,
            language_id: "en".to_string(),
            show_translate_bar: false,
            proxies: Vec::new(),
            active_devices: Vec::new(),
            ton_balance: 0.0,
            stars_balance: 0,
            wallet_transactions: Vec::new(),
            pts_counter: 0,
            cache_media_mb: 0.0,
            cache_files_mb: 0.0,
        }
    }
}

pub struct SessionPersistenceBridge {
    pub client: TelegramClient,
    pub state: PersistedClientState,
    pub storage_path: PathBuf,
}

impl SessionPersistenceBridge {
    pub fn new(server_url: impl Into<String>) -> Self {
        let storage_path = Self::resolve_storage_path();
        let state = Self::load_from_disk(&storage_path).unwrap_or_else(PersistedClientState::empty);
        Self {
            client: TelegramClient::new_with_url(server_url.into()),
            state,
            storage_path,
        }
    }

    pub fn new_demo() -> Self {
        let storage_path = Self::resolve_storage_path();
        Self {
            client: TelegramClient::new_with_url("http://127.0.0.1:8080".to_string()),
            state: PersistedClientState::new_demo(),
            storage_path,
        }
    }

    fn resolve_storage_path() -> PathBuf {
        if let Ok(custom) = std::env::var("GPUI_TELEGRAM_STATE_PATH") {
            return PathBuf::from(custom);
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".gpui_telegram_session_v1.json");
        }
        std::env::temp_dir().join("gpui_telegram_session_v1.json")
    }

    fn load_from_disk(path: &Path) -> Option<PersistedClientState> {
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn save_to_disk(&self) {
        if let Ok(serialized) = serde_json::to_string_pretty(&self.state) {
            let _ = fs::write(&self.storage_path, serialized);
        }
    }

    pub fn record_outgoing_proto_message(&mut self, chat_id: u64, msg_id: u64, text: &str) {
        self.state.pts_counter = self.state.pts_counter.saturating_add(1);
        let cid = ChatId(chat_id as i64);
        let item = ProtoMessageItem::new(
            MessageId(msg_id as i64),
            cid,
            UserId(self.state.active_account_id as u64),
            PeerId::Chat(cid),
            ProtoContent::plain_text(text),
        )
        .with_client_nonce(self.state.pts_counter);
        self.client.local_store.insert_message(item);
        self.save_to_disk();
    }

    pub fn active_account(&self) -> Option<AccountSessionRecord> {
        self.state
            .accounts
            .iter()
            .find(|a| a.account_id == self.state.active_account_id)
            .cloned()
            .or_else(|| self.state.accounts.first().cloned())
    }

    pub fn export_chat_archive(
        &self,
        chat_title: &str,
        format_json: bool,
        lines: &[(String, String, String)],
    ) -> String {
        let safe_name: String = chat_title
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let ext = if format_json { "json" } else { "html" };
        let out_path = std::env::temp_dir().join(format!("telegram_export_{}.{}", safe_name, ext));

        let payload = if format_json {
            let entries: Vec<serde_json::Value> = lines
                .iter()
                .map(|(sender, ts, body)| {
                    serde_json::json!({
                        "from": sender,
                        "date": ts,
                        "text": body,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&serde_json::json!({
                "name": chat_title,
                "type": "telegram_chat_export",
                "messages_count": entries.len(),
                "messages": entries,
            }))
            .unwrap_or_default()
        } else {
            let mut html = String::from(
                "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Telegram Export</title>\
                 <style>body{font-family:-apple-system,sans-serif;background:#0e1621;color:#fff;padding:24px;}\
                 .msg{background:#182533;padding:12px;border-radius:10px;margin-bottom:10px;}\
                 .meta{color:#5288c1;font-size:12px;font-weight:bold;}</style></head><body>",
            );
            html.push_str(&format!(
                "<h2>{} — Exported Archive</h2>",
                Self::escape_html(chat_title)
            ));
            for (sender, ts, body) in lines {
                html.push_str(&format!(
                    "<div class=\"msg\"><div class=\"meta\">{} • {}</div><div>{}</div></div>",
                    Self::escape_html(sender),
                    Self::escape_html(ts),
                    Self::escape_html(body)
                ));
            }
            html.push_str("</body></html>");
            html
        };

        let _ = fs::write(&out_path, payload);
        out_path.to_string_lossy().to_string()
    }

    fn escape_html(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
}

impl Default for SessionPersistenceBridge {
    fn default() -> Self {
        Self::new("http://127.0.0.1:8080")
    }
}
