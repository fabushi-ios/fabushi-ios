//! Authentic TON Import Wallet Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MnemonicWordCount {
    Words12 = 12,
    Words24 = 24,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonImportWalletScreen {
    pub word_count: MnemonicWordCount,
    pub words: Vec<String>,
    pub current_word_index: usize,
    pub validation_errors: Vec<Option<String>>,
    pub is_validating: bool,
    pub import_ready: bool,
    pub status_message: Option<String>,
}

impl TonImportWalletScreen {
    pub fn new(count: MnemonicWordCount) -> Self {
        let n = count as usize;
        Self {
            word_count: count,
            words: vec![String::new(); n],
            current_word_index: 0,
            validation_errors: vec![None; n],
            is_validating: false,
            import_ready: false,
            status_message: None,
        }
    }

    pub fn set_word(&mut self, index: usize, word: impl Into<String>) {
        if index < self.words.len() {
            let clean = word.into().trim().to_lowercase();
            self.words[index] = clean;
            self.validation_errors[index] = None;
            self.check_readiness();
        }
    }

    pub fn paste_phrase(&mut self, raw_phrase: &str) -> bool {
        let parsed_words: Vec<String> = raw_phrase
            .split_whitespace()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        if parsed_words.len() == (self.word_count as usize) {
            self.words = parsed_words;
            self.validation_errors = vec![None; self.words.len()];
            self.check_readiness();
            true
        } else {
            self.status_message = Some(format!(
                "Expected {} words, got {}",
                self.word_count as usize,
                parsed_words.len()
            ));
            false
        }
    }

    pub fn check_readiness(&mut self) -> bool {
        let all_filled = self.words.iter().all(|w| !w.is_empty() && w.len() >= 3);
        self.import_ready = all_filled;
        all_filled
    }

    pub fn mnemonic_phrase(&self) -> String {
        self.words.join(" ")
    }
}

impl Default for TonImportWalletScreen {
    fn default() -> Self {
        Self::new(MnemonicWordCount::Words24)
    }
}
