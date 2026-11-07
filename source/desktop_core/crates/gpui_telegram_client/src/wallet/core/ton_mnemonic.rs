//! TON 24-word mnemonic generation, validation, and seed derivation.

#![allow(dead_code)]

use super::ton_bip39::find_word_index;
use super::ton_crypto::pbkdf2_hmac_sha512;
use serde::{Deserialize, Serialize};

pub const TON_MNEMONIC_WORDS_COUNT: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TonMnemonic {
    pub words: Vec<String>,
}

impl TonMnemonic {
    pub fn from_words(words: Vec<String>) -> Option<Self> {
        if words.len() != TON_MNEMONIC_WORDS_COUNT {
            return None;
        }
        for w in &words {
            if find_word_index(w).is_none() {
                return None;
            }
        }
        Some(Self { words })
    }

    pub fn to_string_phrase(&self) -> String {
        self.words.join(" ")
    }

    pub fn to_seed(&self, password: &str) -> [u8; 64] {
        let phrase = self.to_string_phrase();
        let salt = if password.is_empty() {
            "TON default seed".to_string()
        } else {
            format!("TON seed {}", password)
        };
        let mut seed = [0u8; 64];
        pbkdf2_hmac_sha512(phrase.as_bytes(), salt.as_bytes(), 100_000, &mut seed);
        seed
    }
}
