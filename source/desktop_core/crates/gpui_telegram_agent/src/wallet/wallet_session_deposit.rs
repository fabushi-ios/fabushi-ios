//! Wallet Session Gift & Custody Deposit Pipeline
//! Upstream: `Telegram/SourceFiles/wallet/wallet_session_deposit.cpp`, `wallet_session_deposit.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::{Result, TelegramError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DepositAdmissionState {
    Submitted,
    Verifying,
    Admitted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GiftDepositIntent {
    pub gift_id: String,
    pub recipient_address: String,
    pub amount_nanos: u64,
    pub payload_boc: Vec<u8>,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletSessionDeposit {
    pub active_intents: Vec<GiftDepositIntent>,
    pub admission_state: DepositAdmissionState,
    pub total_deposited_nanos: u64,
}

impl Default for WalletSessionDeposit {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletSessionDeposit {
    pub fn new() -> Self {
        Self {
            active_intents: Vec::new(),
            admission_state: DepositAdmissionState::Submitted,
            total_deposited_nanos: 0,
        }
    }

    pub fn submit_intent(&mut self, intent: GiftDepositIntent) -> Result<usize> {
        if intent.amount_nanos == 0 {
            return Err(TelegramError::Internal("Deposit amount cannot be zero".into()));
        }
        if intent.recipient_address.is_empty() {
            return Err(TelegramError::Internal("Recipient address cannot be empty".into()));
        }
        self.active_intents.push(intent);
        self.admission_state = DepositAdmissionState::Verifying;
        Ok(self.active_intents.len())
    }

    pub fn confirm_deposit(&mut self, gift_id: &str) -> Option<GiftDepositIntent> {
        if let Some(pos) = self.active_intents.iter().position(|i| i.gift_id == gift_id) {
            let intent = self.active_intents.remove(pos);
            self.total_deposited_nanos = self.total_deposited_nanos.saturating_add(intent.amount_nanos);
            if self.active_intents.is_empty() {
                self.admission_state = DepositAdmissionState::Admitted;
            }
            Some(intent)
        } else {
            None
        }
    }

    pub fn pending_count(&self) -> usize {
        self.active_intents.len()
    }
}
