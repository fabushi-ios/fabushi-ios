//! TON Mnemonic Word Chip component for backup and verification screens.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonMnemonicWordChip {
    pub index: usize,
    pub word: String,
    pub is_selected: bool,
    pub is_verified: bool,
}

impl TonMnemonicWordChip {
    pub fn new(index: usize, word: impl Into<String>) -> Self {
        Self {
            index,
            word: word.into(),
            is_selected: false,
            is_verified: false,
        }
    }

    pub fn display_label(&self) -> String {
        format!("{}. {}", self.index, self.word)
    }

    pub fn toggle_selected(&mut self) {
        self.is_selected = !self.is_selected;
    }
}
