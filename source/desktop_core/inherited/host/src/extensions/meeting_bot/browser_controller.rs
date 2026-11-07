//! OpenClaw Meeting Bot Browser Controller & Serialized Tab Action Lock
//!
//! Mirrors:
//! - `src/meeting-bot/browser-act-lock.ts`
//! - `src/meeting-bot/browser-act-lock.test.ts`
//! - `src/meeting-bot/browser-controller.ts`
//! - `src/meeting-bot/browser-controller.test.ts`
//! - `src/meeting-bot/browser-navigation-errors.ts`
//! - `src/meeting-bot/browser-node.ts`
//! - `src/meeting-bot/chrome-node-result.ts`
//! - `src/meeting-bot/platform-adapter-contract.ts`

use std::collections::HashMap;
use std::sync::Mutex;
use serde::{Deserialize, Serialize};

pub const BROWSER_ACT_TIMEOUT_MESSAGE: &str =
    "Meeting browser operation timed out waiting for browser tab control.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserNavigationState {
    Joining,
    InCall,
    Muted,
    Unmuted,
    Leaving,
    Left,
}

#[derive(Debug, Default)]
pub struct MeetingBrowserActLock {
    active_targets: Mutex<HashMap<String, u64>>, // target_id -> expiration_ms
}

impl MeetingBrowserActLock {
    pub fn new() -> Self {
        Self {
            active_targets: Mutex::new(HashMap::new()),
        }
    }

    pub fn acquire_lock(&self, target_id: &str, now_ms: u64, deadline_ms: u64) -> Result<(), &'static str> {
        if now_ms >= deadline_ms {
            return Err(BROWSER_ACT_TIMEOUT_MESSAGE);
        }

        let mut map = self.active_targets.lock().unwrap();
        if let Some(&exp) = map.get(target_id) {
            if now_ms < exp {
                // Target is still busy
                return Err(BROWSER_ACT_TIMEOUT_MESSAGE);
            }
        }

        map.insert(target_id.to_string(), deadline_ms);
        Ok(())
    }

    pub fn release_lock(&self, target_id: &str) {
        let mut map = self.active_targets.lock().unwrap();
        map.remove(target_id);
    }
}
