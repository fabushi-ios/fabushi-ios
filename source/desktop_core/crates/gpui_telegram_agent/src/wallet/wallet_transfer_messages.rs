//! Telegram Chat Service Messages for Wallet Transfers
//! Upstream: `Telegram/SourceFiles/wallet/wallet_transfer_messages.cpp`, `wallet_transfer_messages.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferServiceStatus {
    Pending,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTransferMessage {
    pub message_id: i64,
    pub chat_id: i64,
    pub sender_id: i64,
    pub recipient_id: i64,
    pub amount_nanos: u64,
    pub currency: String,
    pub comment: Option<String>,
    pub status: TransferServiceStatus,
    pub tx_hash: Option<String>,
    pub timestamp_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTransferMessages {
    pub messages: Vec<ChatTransferMessage>,
}

impl Default for WalletTransferMessages {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletTransferMessages {
    pub fn new() -> Self {
        Self { messages: Vec::new() }
    }

    pub fn inject_transfer_message(
        &mut self,
        message_id: i64,
        chat_id: i64,
        sender_id: i64,
        recipient_id: i64,
        amount_nanos: u64,
        comment: Option<String>,
        now_secs: i64,
    ) -> &ChatTransferMessage {
        let msg = ChatTransferMessage {
            message_id,
            chat_id,
            sender_id,
            recipient_id,
            amount_nanos,
            currency: "TON".into(),
            comment,
            status: TransferServiceStatus::Pending,
            tx_hash: None,
            timestamp_seconds: now_secs,
        };
        self.messages.push(msg);
        self.messages.last().expect("just pushed")
    }

    pub fn update_status(&mut self, message_id: i64, status: TransferServiceStatus, tx_hash: Option<String>) -> bool {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.message_id == message_id) {
            msg.status = status;
            if tx_hash.is_some() {
                msg.tx_hash = tx_hash;
            }
            true
        } else {
            false
        }
    }

    pub fn find_by_chat(&self, chat_id: i64) -> Vec<&ChatTransferMessage> {
        self.messages.iter().filter(|m| m.chat_id == chat_id).collect()
    }
}
