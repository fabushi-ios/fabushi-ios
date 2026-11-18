use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::ChatId;
use gpui_telegram_proto::sync::PtsUpdate;
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
use std::ops::RangeInclusive;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    Contiguous,
    Duplicate,
    GapDetected {
        last_seen: u64,
        incoming: u64,
        missing_range: RangeInclusive<u64>,
    },
}

#[derive(Debug, Default, Clone)]
pub struct ClientSyncCoordinator {
    last_seen_pts: Arc<RwLock<HashMap<ChatId, u64>>>,
    pending_gap_buffers: Arc<RwLock<HashMap<ChatId, BTreeMap<u64, PtsUpdate>>>>,
}

impl ClientSyncCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last_pts(&self, chat_id: ChatId) -> u64 {
        *self.last_seen_pts.read().get(&chat_id).unwrap_or(&0)
    }

    pub fn set_pts(&self, chat_id: ChatId, pts: u64) {
        self.last_seen_pts.write().insert(chat_id, pts);
    }

    pub fn evaluate_update(&self, update: &PtsUpdate) -> SyncOutcome {
        let mut map = self.last_seen_pts.write();
        let last = map.entry(update.chat_id).or_insert(0);

        // Initial unseeded chat state: adopt incoming pts if uninitialized
        if *last == 0 {
            *last = update.pts;
            return SyncOutcome::Contiguous;
        }

        // Ephemeral updates (pts_count == 0)
        if update.pts_count == 0 {
            if update.pts <= *last {
                return SyncOutcome::Contiguous;
            } else {
                return SyncOutcome::GapDetected {
                    last_seen: *last,
                    incoming: update.pts,
                    missing_range: (*last + 1)..=update.pts,
                };
            }
        }

        let prev = update.prev_pts();
        if prev == *last {
            // Contiguous sequence!
            *last = update.pts;
            SyncOutcome::Contiguous
        } else if update.pts <= *last {
            // Stale or duplicate update
            SyncOutcome::Duplicate
        } else {
            // Sequence gap detected: prev > *last
            SyncOutcome::GapDetected {
                last_seen: *last,
                incoming: update.pts,
                missing_range: (*last + 1)..=prev,
            }
        }
    }

    pub fn buffer_gap_update(&self, update: PtsUpdate) {
        let mut buffers = self.pending_gap_buffers.write();
        buffers
            .entry(update.chat_id)
            .or_default()
            .insert(update.pts, update);
    }

    pub fn drain_contiguous_buffer(&self, chat_id: ChatId) -> Vec<PtsUpdate> {
        let mut ready = Vec::new();
        let mut buffers = self.pending_gap_buffers.write();
        let mut last_pts_guard = self.last_seen_pts.write();
        let current_pts = last_pts_guard.entry(chat_id).or_insert(0);

        if let Some(queue) = buffers.get_mut(&chat_id) {
            while let Some((&pts, _)) = queue.iter().next() {
                let prev = queue[&pts].prev_pts();
                if prev == *current_pts {
                    let update = queue.remove(&pts).unwrap();
                    *current_pts = update.pts;
                    ready.push(update);
                } else if pts <= *current_pts {
                    // Stale in buffer
                    queue.remove(&pts);
                } else {
                    // Next gap encountered
                    break;
                }
            }
        }
        ready
    }

    pub fn apply_update(&self, update: PtsUpdate) -> Result<()> {
        match self.evaluate_update(&update) {
            SyncOutcome::Contiguous | SyncOutcome::Duplicate => Ok(()),
            SyncOutcome::GapDetected {
                last_seen,
                incoming,
                ..
            } => {
                self.buffer_gap_update(update);
                Err(TelegramError::PtsSequenceGap {
                    last_seen,
                    incoming,
                })
            }
        }
    }
}
