//! TON transaction comment input component with encryption toggle and character limits.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub const MAX_COMMENT_LENGTH: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonCommentInput {
    pub text: String,
    pub is_encrypted: bool,
    pub max_chars: usize,
}

impl TonCommentInput {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            is_encrypted: false,
            max_chars: MAX_COMMENT_LENGTH,
        }
    }

    pub fn set_text(&mut self, input: &str) -> bool {
        if input.chars().count() <= self.max_chars {
            self.text = input.to_string();
            true
        } else {
            false
        }
    }

    pub fn toggle_encryption(&mut self) {
        self.is_encrypted = !self.is_encrypted;
    }

    pub fn remaining_chars(&self) -> usize {
        self.max_chars.saturating_sub(self.text.chars().count())
    }
}
