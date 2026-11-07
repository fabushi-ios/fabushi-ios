//! Wallet Real-Time Event & Transaction Streaming Engine
//! Upstream: `Telegram/SourceFiles/wallet/wallet_stream.cpp`, `wallet_stream.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};

pub const COALESCE_DELAY_MS: u64 = 250;
pub const HISTORY_RECHECK_DELAY_MS: u64 = 3000;
pub const HISTORY_LAST_RECHECK_DELAY_MS: u64 = 10000;
pub const KEEPALIVE_INTERVAL_MS: u64 = 10000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamEvent {
    BalanceUpdate { address: String, new_balance_nanos: u64 },
    TransactionReceived { tx_hash: String, amount_nanos: u64 },
    CollectibleReceived { nft_address: String },
    Heartbeat { timestamp_ms: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletStream {
    pub state: StreamConnectionState,
    pub endpoint_url: String,
    pub last_event_at_ms: u64,
    pub buffered_events: Vec<StreamEvent>,
    pub reconnect_attempts: usize,
}

impl Default for WalletStream {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletStream {
    pub fn new() -> Self {
        Self {
            state: StreamConnectionState::Disconnected,
            endpoint_url: String::new(),
            last_event_at_ms: 0,
            buffered_events: Vec::new(),
            reconnect_attempts: 0,
        }
    }

    pub fn with_endpoint(url: impl Into<String>) -> Self {
        let mut s = Self::new();
        s.endpoint_url = url.into();
        s.state = StreamConnectionState::Connecting;
        s
    }

    pub fn on_connected(&mut self) {
        self.state = StreamConnectionState::Connected;
        self.reconnect_attempts = 0;
    }

    pub fn push_event(&mut self, event: StreamEvent, timestamp_ms: u64) {
        self.last_event_at_ms = timestamp_ms;
        self.buffered_events.push(event);
    }

    pub fn drain_events(&mut self) -> Vec<StreamEvent> {
        std::mem::take(&mut self.buffered_events)
    }

    pub fn schedule_reconnect(&mut self) -> u64 {
        self.state = StreamConnectionState::Reconnecting;
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
        if self.reconnect_attempts == 1 {
            HISTORY_RECHECK_DELAY_MS
        } else {
            HISTORY_LAST_RECHECK_DELAY_MS
        }
    }
}
