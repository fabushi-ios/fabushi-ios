//! Authentic TON Wallet Gift Move Presentation Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_gift_move_presentation.cpp, Telegram/SourceFiles/wallet/wallet_gift_move_presentation.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GiftMoveSendingRow {
    pub operation_id: String,
    pub gift_slug: String,
    pub recipient_user_id: u64,
    pub predecessor: String,
    pub served_id: String,
    pub is_confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletGiftMovePresentation {
    pub active_rows: Vec<GiftMoveSendingRow>,
    pub is_preview_active: bool,
    pub current_operation_id: Option<String>,
}

impl WalletGiftMovePresentation {
    pub fn new() -> Self {
        Self {
            active_rows: Vec::new(),
            is_preview_active: false,
            current_operation_id: None,
        }
    }

    pub fn start_preview(&mut self, operation_id: impl Into<String>, slug: impl Into<String>, recipient: u64) {
        let op_id = operation_id.into();
        self.current_operation_id = Some(op_id.clone());
        self.is_preview_active = true;
        self.active_rows.push(GiftMoveSendingRow {
            operation_id: op_id,
            gift_slug: slug.into(),
            recipient_user_id: recipient,
            predecessor: String::new(),
            served_id: String::new(),
            is_confirmed: false,
        });
    }

    pub fn confirm_row(&mut self, operation_id: &str, served_id: impl Into<String>) {
        if let Some(row) = self.active_rows.iter_mut().find(|r| r.operation_id == operation_id) {
            row.served_id = served_id.into();
            row.is_confirmed = true;
        }
    }

    pub fn close_preview(&mut self) {
        self.is_preview_active = false;
        self.current_operation_id = None;
    }
}

impl Default for WalletGiftMovePresentation {
    fn default() -> Self {
        Self::new()
    }
}
