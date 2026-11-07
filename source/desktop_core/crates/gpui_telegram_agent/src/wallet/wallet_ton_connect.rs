//! TON Connect 2.0 Core Coordinator & Session Manager
//! Upstream: `Telegram/SourceFiles/wallet/wallet_ton_connect.cpp`, `wallet_ton_connect.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use gpui_telegram_core::error::{Result, TelegramError};

pub const WALLET_RESOLVE_TIMEOUT_MS: u64 = 20000;
pub const MANIFEST_POLL_INTERVAL_MS: u64 = 5000;
pub const MANIFEST_POLL_LIMIT: usize = 12;
pub const APP_NAME_LIMIT: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TonConnectManifest {
    pub url: String,
    pub name: String,
    pub icon_url: String,
    pub terms_of_use_url: Option<String>,
    pub privacy_policy_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TonConnectSessionInfo {
    pub session_id: String,
    pub dapp_public_key: String,
    pub manifest: TonConnectManifest,
    pub connected_at_ms: u64,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTonConnect {
    pub active_sessions: HashMap<String, TonConnectSessionInfo>,
    pub cached_manifests: HashMap<String, TonConnectManifest>,
}

impl Default for WalletTonConnect {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletTonConnect {
    pub fn new() -> Self {
        Self {
            active_sessions: HashMap::new(),
            cached_manifests: HashMap::new(),
        }
    }

    pub fn cache_manifest(&mut self, mut manifest: TonConnectManifest) {
        if manifest.name.len() > APP_NAME_LIMIT {
            manifest.name.truncate(APP_NAME_LIMIT);
        }
        self.cached_manifests.insert(manifest.url.clone(), manifest);
    }

    pub fn connect_session(
        &mut self,
        session_id: impl Into<String>,
        dapp_public_key: impl Into<String>,
        manifest: TonConnectManifest,
        now_ms: u64,
    ) -> Result<TonConnectSessionInfo> {
        let sid = session_id.into();
        let pkey = dapp_public_key.into();
        if pkey.is_empty() {
            return Err(TelegramError::Internal("DApp public key cannot be empty".into()));
        }
        let info = TonConnectSessionInfo {
            session_id: sid.clone(),
            dapp_public_key: pkey,
            manifest,
            connected_at_ms: now_ms,
            is_active: true,
        };
        self.active_sessions.insert(sid, info.clone());
        Ok(info)
    }

    pub fn disconnect_session(&mut self, session_id: &str) -> bool {
        if let Some(session) = self.active_sessions.get_mut(session_id) {
            session.is_active = false;
            true
        } else {
            false
        }
    }

    pub fn get_session(&self, session_id: &str) -> Option<&TonConnectSessionInfo> {
        self.active_sessions.get(session_id)
    }

    pub fn count_active(&self) -> usize {
        self.active_sessions.values().filter(|s| s.is_active).count()
    }
}
