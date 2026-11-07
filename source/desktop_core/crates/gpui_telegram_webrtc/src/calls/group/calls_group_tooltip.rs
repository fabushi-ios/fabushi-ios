//! Group Call In-Call Tooltip Manager: `CallsGroupTooltip`
//! Manages priority sticky hints and notifications within the voice chat panel.

use serde::{Deserialize, Serialize};

/// Tooltip message priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TooltipPriority {
    Low,
    Normal,
    High,
    Critical,
}

/// Active tooltip descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveTooltip {
    pub key: String,
    pub text: String,
    pub priority: TooltipPriority,
    pub timeout_ms: u64,
}

/// Parity component for `CallsGroupTooltip`.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupTooltip {
    pub current: Option<ActiveTooltip>,
}

impl CallsGroupTooltip {
    pub fn new() -> Self {
        Self { current: None }
    }

    pub fn show(&mut self, key: &str, text: &str, priority: TooltipPriority, timeout_ms: u64) {
        if let Some(ref cur) = self.current {
            if cur.priority > priority {
                return;
            }
        }
        self.current = Some(ActiveTooltip {
            key: key.to_string(),
            text: text.to_string(),
            priority,
            timeout_ms,
        });
    }

    pub fn dismiss(&mut self, key: &str) {
        if let Some(ref cur) = self.current {
            if cur.key == key {
                self.current = None;
            }
        }
    }

    pub fn clear(&mut self) {
        self.current = None;
    }
}
