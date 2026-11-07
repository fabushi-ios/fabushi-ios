//! Telegram Desktop 1:1 Parity Submodule: `ApiBot`
//! Subsystem: `api` | Feature ID: `TCG-N27`
//! Disposition: `rust_domain_or_server_1to1`
//! Upstream Sources: Telegram/SourceFiles/api/api_bot.cpp, Telegram/SourceFiles/api/api_bot.h
//! Inherited Paths: inherited/host/src/device_pairing_tokens.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BotButtonType {
    Callback(Vec<u8>),
    Url(String),
    WebApp(String),
    SwitchInline(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BotButton {
    pub text: String,
    pub button_type: BotButtonType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiBot {
    pub is_enabled: bool,
    pub version: u32,
    pub label: String,
    pub clicked_callbacks: Vec<(u64, i64, Vec<u8>)>,
    pub updated_at: i64,
}

impl Default for ApiBot {
    fn default() -> Self {
        Self {
            is_enabled: true,
            version: 1,
            label: "ApiBot".to_string(),
            clicked_callbacks: Vec::new(),
            updated_at: chrono::Utc::now().timestamp(),
        }
    }
}

impl ApiBot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_label(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..Self::default()
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.is_enabled = enabled;
        self.updated_at = chrono::Utc::now().timestamp();
    }

    pub fn send_callback_data(&mut self, peer_id: u64, msg_id: i64, data: Vec<u8>) {
        self.clicked_callbacks.push((peer_id, msg_id, data));
        self.updated_at = chrono::Utc::now().timestamp();
    }
}
