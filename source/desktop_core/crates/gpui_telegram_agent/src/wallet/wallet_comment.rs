//! Authentic TON Wallet Comment & Decryption Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_comment.cpp, Telegram/SourceFiles/wallet/wallet_comment.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_RESOLVE_INTERVAL_MS: u64 = 500;
pub const K_RESOLVE_TICKS: u32 = 40;
pub const K_RESOLVE_ATTEMPTS: u32 = 4;
pub const K_MAX_COMMENT_LENGTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentDecryptState {
    None,
    Resolving { attempt: u32 },
    Decrypted(String),
    EncryptedForeign,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletComment {
    pub raw_text: String,
    pub is_encrypted: bool,
    pub decrypt_state: CommentDecryptState,
    pub resolve_attempts: u32,
}

impl WalletComment {
    pub fn new_plain(text: impl Into<String>) -> Self {
        let raw = text.into();
        Self {
            raw_text: raw.clone(),
            is_encrypted: false,
            decrypt_state: CommentDecryptState::Decrypted(raw),
            resolve_attempts: 0,
        }
    }

    pub fn new_encrypted(cipher_data: impl Into<String>) -> Self {
        Self {
            raw_text: cipher_data.into(),
            is_encrypted: true,
            decrypt_state: CommentDecryptState::None,
            resolve_attempts: 0,
        }
    }

    pub fn activate_resolution(&mut self) {
        if self.is_encrypted {
            self.resolve_attempts = 1;
            self.decrypt_state = CommentDecryptState::Resolving { attempt: 1 };
        }
    }

    pub fn step_resolution(&mut self, next_attempt: u32) {
        if next_attempt > K_RESOLVE_ATTEMPTS {
            self.decrypt_state = CommentDecryptState::Failed;
        } else {
            self.resolve_attempts = next_attempt;
            self.decrypt_state = CommentDecryptState::Resolving { attempt: next_attempt };
        }
    }

    pub fn resolve_success(&mut self, plain: impl Into<String>) {
        self.decrypt_state = CommentDecryptState::Decrypted(plain.into());
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.decrypt_state, CommentDecryptState::Resolving { .. })
    }

    pub fn plaintext(&self) -> Option<&str> {
        match &self.decrypt_state {
            CommentDecryptState::Decrypted(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

impl Default for WalletComment {
    fn default() -> Self {
        Self::new_plain("")
    }
}
