//! Authentic TON Send Recipient Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentRecipient {
    pub name: String,
    pub address: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonSendRecipientScreen {
    pub recipient_input: String,
    pub resolved_address: Option<String>,
    pub is_resolving_dns: bool,
    pub is_valid: bool,
    pub error_message: Option<String>,
    pub recent_recipients: Vec<RecentRecipient>,
}

impl TonSendRecipientScreen {
    pub fn new() -> Self {
        Self {
            recipient_input: String::new(),
            resolved_address: None,
            is_resolving_dns: false,
            is_valid: false,
            error_message: None,
            recent_recipients: Vec::new(),
        }
    }

    pub fn set_input(&mut self, text: impl Into<String>) {
        let input = text.into().trim().to_string();
        self.recipient_input = input;
        self.validate();
    }

    pub fn validate(&mut self) -> bool {
        let text = self.recipient_input.trim();
        if text.is_empty() {
            self.is_valid = false;
            self.error_message = None;
            self.resolved_address = None;
            return false;
        }

        if text.ends_with(".ton") || text.ends_with(".t.me") {
            // Domain needing DNS resolution
            self.is_resolving_dns = true;
            self.is_valid = false;
            self.error_message = None;
            return false;
        }

        // TON standard user-friendly address is 48 chars
        if text.len() == 48 {
            self.is_valid = true;
            self.resolved_address = Some(text.to_string());
            self.error_message = None;
            self.is_resolving_dns = false;
            true
        } else {
            self.is_valid = false;
            self.error_message = Some("Invalid TON address length (expected 48 characters)".into());
            self.resolved_address = None;
            false
        }
    }

    pub fn on_dns_resolved(&mut self, address: impl Into<String>) {
        self.resolved_address = Some(address.into());
        self.is_resolving_dns = false;
        self.is_valid = true;
        self.error_message = None;
    }

    pub fn add_recent(&mut self, recipient: RecentRecipient) {
        if !self.recent_recipients.iter().any(|r| r.address == recipient.address) {
            self.recent_recipients.push(recipient);
        }
    }
}

impl Default for TonSendRecipientScreen {
    fn default() -> Self {
        Self::new()
    }
}
