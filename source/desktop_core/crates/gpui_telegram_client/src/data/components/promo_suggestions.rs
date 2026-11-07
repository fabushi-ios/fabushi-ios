//! Target Symbol: PromoSuggestionsComponents
//! Telegram Desktop 1:1 Submodule: `PromoSuggestionsComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PromoSuggestion {
    pub peer_id: PeerId,
    pub title: String,
    pub description: String,
}

/// Target Symbol for Module 03 Ledger: PromoSuggestionsComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PromoSuggestionsComponents {
    pub suggestions: Vec<PromoSuggestion>,
}

impl PromoSuggestionsComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, suggestion: PromoSuggestion) {
        self.suggestions.push(suggestion);
    }

    pub fn count(&self) -> usize {
        self.suggestions.len()
    }
}
