//! Authentic TON Wallet Engine & Network Lifecycle Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_engine.cpp, Telegram/SourceFiles/wallet/wallet_engine.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_MAX_TRACKED_EARLY_CANCELS: usize = 64;
pub const K_ROUTED_SEND_TIMEOUT_MS: u64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineLifecycleState {
    Disconnected,
    Connecting,
    Synchronizing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletEngine {
    pub state: EngineLifecycleState,
    pub tracked_early_cancels: Vec<String>,
    pub active_seqno: u32,
    pub balance_nanotons: u64,
    pub last_sync_time: u64,
    pub timeout_ms: u64,
}

impl WalletEngine {
    pub fn new() -> Self {
        Self {
            state: EngineLifecycleState::Disconnected,
            tracked_early_cancels: Vec::new(),
            active_seqno: 0,
            balance_nanotons: 0,
            last_sync_time: 0,
            timeout_ms: K_ROUTED_SEND_TIMEOUT_MS,
        }
    }

    pub fn set_state(&mut self, new_state: EngineLifecycleState) {
        self.state = new_state;
    }

    pub fn track_early_cancel(&mut self, tx_hash: impl Into<String>) {
        let hash = tx_hash.into();
        if self.tracked_early_cancels.len() >= K_MAX_TRACKED_EARLY_CANCELS {
            self.tracked_early_cancels.remove(0);
        }
        self.tracked_early_cancels.push(hash);
    }

    pub fn is_early_cancelled(&self, tx_hash: &str) -> bool {
        self.tracked_early_cancels.iter().any(|h| h == tx_hash)
    }

    pub fn update_balance(&mut self, nanotons: u64, seqno: u32, timestamp: u64) {
        self.balance_nanotons = nanotons;
        self.active_seqno = seqno;
        self.last_sync_time = timestamp;
        self.state = EngineLifecycleState::Ready;
    }
}

impl Default for WalletEngine {
    fn default() -> Self {
        Self::new()
    }
}
