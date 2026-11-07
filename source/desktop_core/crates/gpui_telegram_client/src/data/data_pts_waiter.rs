//! Target Symbol: DataPtsWaiter
//! Telegram Desktop 1:1 Submodule: `DataPtsWaiter`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// Update synchronization sequence and PTS gap waiter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtsWaiter {
    pub good: i32,
    pub last: i32,
    pub count: i32,
    pub requesting: bool,
    pub waiting_for_skipped: bool,
}

impl Default for PtsWaiter {
    fn default() -> Self {
        Self {
            good: 0,
            last: 0,
            count: 0,
            requesting: false,
            waiting_for_skipped: false,
        }
    }
}

impl PtsWaiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn init(&mut self, pts: i32) {
        self.good = pts;
        self.last = pts;
        self.count = pts;
        self.waiting_for_skipped = false;
    }

    pub fn is_inited(&self) -> bool {
        self.good > 0
    }

    pub fn check_gap(&mut self, pts: i32, pts_count: i32) -> bool {
        if !self.is_inited() {
            self.init(pts);
            return true;
        }

        // If the update continuous matches our expected good PTS
        if pts == self.good + pts_count {
            self.good = pts;
            self.last = pts;
            true
        } else if pts <= self.good {
            // Duplicate / old update
            true
        } else {
            // Gap detected!
            self.waiting_for_skipped = true;
            false
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataPtsWaiter coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPtsWaiter {
    pub waiter: PtsWaiter,
}

impl DataPtsWaiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_pts(pts: i32) -> Self {
        let mut waiter = PtsWaiter::new();
        waiter.init(pts);
        Self { waiter }
    }

    pub fn current(&self) -> i32 {
        self.waiter.good
    }
}
