//! Authentic TON Receive Address Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonReceiveScreen {
    pub bounceable_address: String,
    pub non_bounceable_address: String,
    pub show_non_bounceable: bool,
    pub warning_text: String,
    pub copied: bool,
}

impl TonReceiveScreen {
    pub fn new(bounceable: impl Into<String>, non_bounceable: impl Into<String>) -> Self {
        Self {
            bounceable_address: bounceable.into(),
            non_bounceable_address: non_bounceable.into(),
            show_non_bounceable: false,
            warning_text: "Send only TON and Jetton tokens to this address on The Open Network.".into(),
            copied: false,
        }
    }

    pub fn active_address(&self) -> &str {
        if self.show_non_bounceable {
            &self.non_bounceable_address
        } else {
            &self.bounceable_address
        }
    }

    pub fn toggle_bounceable(&mut self) {
        self.show_non_bounceable = !self.show_non_bounceable;
    }

    pub fn mark_copied(&mut self) {
        self.copied = true;
    }
}

impl Default for TonReceiveScreen {
    fn default() -> Self {
        Self::new("", "")
    }
}
