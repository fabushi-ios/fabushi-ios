//! Authentic TON Wallet Send Preparation Pipeline Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_send_preparation.cpp, Telegram/SourceFiles/wallet/wallet_send_preparation.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_SEND_REFUSAL_RETRIES: u32 = 3;
pub const K_SIGNING_READY_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SendPreparationStage {
    Idle,
    EstimatingFee,
    ReadyToSign,
    Signing,
    Submitting,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletSendPreparation {
    pub stage: SendPreparationStage,
    pub recipient_address: String,
    pub amount_nanotons: u64,
    pub estimated_fee_nanotons: Option<u64>,
    pub revision: u64,
    pub refusal_retries: u32,
    pub error_description: Option<String>,
}

impl WalletSendPreparation {
    pub fn new() -> Self {
        Self {
            stage: SendPreparationStage::Idle,
            recipient_address: String::new(),
            amount_nanotons: 0,
            estimated_fee_nanotons: None,
            revision: 0,
            refusal_retries: 0,
            error_description: None,
        }
    }

    pub fn prepare_transfer(
        &mut self,
        recipient: impl Into<String>,
        amount_nanotons: u64,
    ) {
        self.recipient_address = recipient.into();
        self.amount_nanotons = amount_nanotons;
        self.revision += 1;
        self.stage = SendPreparationStage::EstimatingFee;
        self.error_description = None;
    }

    pub fn on_fee_estimated(&mut self, fee_nanotons: u64, for_revision: u64) {
        if for_revision == self.revision {
            self.estimated_fee_nanotons = Some(fee_nanotons);
            self.stage = SendPreparationStage::ReadyToSign;
        }
    }

    pub fn sign_and_submit(&mut self) -> bool {
        if self.stage == SendPreparationStage::ReadyToSign {
            self.stage = SendPreparationStage::Submitting;
            true
        } else {
            false
        }
    }

    pub fn complete(&mut self) {
        self.stage = SendPreparationStage::Completed;
    }

    pub fn fail(&mut self, err: impl Into<String>) {
        self.stage = SendPreparationStage::Failed;
        self.error_description = Some(err.into());
    }
}

impl Default for WalletSendPreparation {
    fn default() -> Self {
        Self::new()
    }
}
