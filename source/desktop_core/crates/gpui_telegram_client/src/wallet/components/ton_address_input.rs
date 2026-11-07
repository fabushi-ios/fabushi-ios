//! TON Address Input component with validation and clipboard paste.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AddressValidationState {
    Empty,
    ValidRaw,
    ValidUserFriendly { bounceable: bool, testnet: bool },
    Invalid(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonAddressInput {
    pub text: String,
    pub placeholder: String,
    pub validation_state: AddressValidationState,
    pub is_focused: bool,
}

impl TonAddressInput {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            placeholder: "Enter TON address or domain...".to_string(),
            validation_state: AddressValidationState::Empty,
            is_focused: false,
        }
    }

    pub fn set_text(&mut self, input: &str) {
        let trimmed = input.trim();
        self.text = trimmed.to_string();
        if trimmed.is_empty() {
            self.validation_state = AddressValidationState::Empty;
        } else if trimmed.contains(':') && trimmed.len() >= 66 {
            self.validation_state = AddressValidationState::ValidRaw;
        } else if trimmed.len() == 48 && (trimmed.starts_with("EQ") || trimmed.starts_with("UQ")) {
            self.validation_state = AddressValidationState::ValidUserFriendly {
                bounceable: trimmed.starts_with("EQ"),
                testnet: false,
            };
        } else {
            self.validation_state = AddressValidationState::Invalid("Invalid TON address format".to_string());
        }
    }

    pub fn is_valid(&self) -> bool {
        matches!(
            self.validation_state,
            AddressValidationState::ValidRaw | AddressValidationState::ValidUserFriendly { .. }
        )
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.validation_state = AddressValidationState::Empty;
    }
}
