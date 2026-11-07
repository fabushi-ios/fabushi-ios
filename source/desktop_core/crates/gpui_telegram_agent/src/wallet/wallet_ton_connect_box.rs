//! TON Connect Connection Modal Dialog View Model
//! Upstream: `Telegram/SourceFiles/wallet/wallet_ton_connect_box.cpp`, `wallet_ton_connect_box.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};
use super::wallet_ton_connect::TonConnectManifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TonConnectBoxPhase {
    Notice,
    Restore,
    Confirm,
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTonConnectBox {
    pub manifest: TonConnectManifest,
    pub phase: TonConnectBoxPhase,
    pub permissions_requested: Vec<String>,
    pub is_submitting: bool,
    pub error_message: Option<String>,
}

impl Default for WalletTonConnectBox {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletTonConnectBox {
    pub fn new() -> Self {
        Self {
            manifest: TonConnectManifest {
                url: String::new(),
                name: String::new(),
                icon_url: String::new(),
                terms_of_use_url: None,
                privacy_policy_url: None,
            },
            phase: TonConnectBoxPhase::Confirm,
            permissions_requested: Vec::new(),
            is_submitting: false,
            error_message: None,
        }
    }

    pub fn with_manifest(manifest: TonConnectManifest) -> Self {
        let mut b = Self::new();
        b.manifest = manifest;
        b
    }

    pub fn effective_kind(&self) -> TonConnectBoxPhase {
        match self.phase {
            TonConnectBoxPhase::Notice => TonConnectBoxPhase::Notice,
            TonConnectBoxPhase::Restore => TonConnectBoxPhase::Restore,
            _ => TonConnectBoxPhase::Confirm,
        }
    }

    pub fn set_submitting(&mut self, submitting: bool) {
        self.is_submitting = submitting;
    }

    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.phase = TonConnectBoxPhase::Error;
        self.error_message = Some(msg.into());
        self.is_submitting = false;
    }

    pub fn set_success(&mut self) {
        self.phase = TonConnectBoxPhase::Success;
        self.is_submitting = false;
        self.error_message = None;
    }
}
