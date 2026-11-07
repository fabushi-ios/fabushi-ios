//! Wallet Session Gift Movement & Transfer Queue
//! Upstream: `Telegram/SourceFiles/wallet/wallet_session_gift_moves.cpp`, `wallet_session_gift_moves.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GiftMoveStatus {
    Queued,
    Broadcasting,
    Confirmed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GiftMoveEntry {
    pub move_id: String,
    pub gift_item_id: String,
    pub target_address: String,
    pub status: GiftMoveStatus,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletSessionGiftMoves {
    pub pending_moves: Vec<GiftMoveEntry>,
    pub completed_count: usize,
}

impl Default for WalletSessionGiftMoves {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletSessionGiftMoves {
    pub fn new() -> Self {
        Self {
            pending_moves: Vec::new(),
            completed_count: 0,
        }
    }

    pub fn enqueue_move(&mut self, gift_item_id: impl Into<String>, target_address: impl Into<String>, timestamp_ms: u64) -> String {
        let item_id = gift_item_id.into();
        let target = target_address.into();
        let move_id = format!("mv_{}_{}", item_id, timestamp_ms);
        let entry = GiftMoveEntry {
            move_id: move_id.clone(),
            gift_item_id: item_id,
            target_address: target,
            status: GiftMoveStatus::Queued,
            timestamp_ms,
        };
        self.pending_moves.push(entry);
        move_id
    }

    pub fn mark_status(&mut self, move_id: &str, new_status: GiftMoveStatus) -> bool {
        if let Some(entry) = self.pending_moves.iter_mut().find(|m| m.move_id == move_id) {
            entry.status = new_status;
            if new_status == GiftMoveStatus::Confirmed {
                self.completed_count = self.completed_count.saturating_add(1);
            }
            true
        } else {
            false
        }
    }

    pub fn prune_confirmed(&mut self) -> usize {
        let prev = self.pending_moves.len();
        self.pending_moves.retain(|m| m.status != GiftMoveStatus::Confirmed);
        prev.saturating_sub(self.pending_moves.len())
    }
}
