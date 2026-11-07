//! Authentic TON Wallet Move-to-Gifts Smart Contract Transfer Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_move_to_gifts.cpp, Telegram/SourceFiles/wallet/wallet_move_to_gifts.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_ADDRESS_WAIT_MS: u64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveToGiftsState {
    Idle,
    ResolvingAddress,
    EstimatingFee,
    AwaitingConfirmation,
    Submitting,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletMoveToGifts {
    pub state: MoveToGiftsState,
    pub gift_id: u64,
    pub contract_address: String,
    pub estimated_gas_nanotons: u64,
    pub error_message: Option<String>,
}

impl WalletMoveToGifts {
    pub fn new(gift_id: u64) -> Self {
        Self {
            state: MoveToGiftsState::Idle,
            gift_id,
            contract_address: String::new(),
            estimated_gas_nanotons: 50_000_000, // 0.05 TON default reserve
            error_message: None,
        }
    }

    pub fn start_resolution(&mut self) {
        self.state = MoveToGiftsState::ResolvingAddress;
        self.error_message = None;
    }

    pub fn on_address_resolved(&mut self, address: impl Into<String>, gas_fee: u64) {
        self.contract_address = address.into();
        self.estimated_gas_nanotons = gas_fee;
        self.state = MoveToGiftsState::AwaitingConfirmation;
    }

    pub fn submit(&mut self) {
        self.state = MoveToGiftsState::Submitting;
    }

    pub fn complete(&mut self) {
        self.state = MoveToGiftsState::Completed;
    }

    pub fn fail(&mut self, err: impl Into<String>) {
        self.state = MoveToGiftsState::Failed;
        self.error_message = Some(err.into());
    }
}

impl Default for WalletMoveToGifts {
    fn default() -> Self {
        Self::new(0)
    }
}
