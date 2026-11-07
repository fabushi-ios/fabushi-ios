//! TON Token Selector component for switching between native TON and Jettons.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenItem {
    pub symbol: String,
    pub name: String,
    pub contract_address: Option<String>,
    pub balance_nano: i64,
    pub decimals: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonTokenSelector {
    pub available_tokens: Vec<TokenItem>,
    pub selected_symbol: String,
    pub is_dropdown_open: bool,
}

impl TonTokenSelector {
    pub fn new() -> Self {
        let native = TokenItem {
            symbol: "TON".to_string(),
            name: "Toncoin".to_string(),
            contract_address: None,
            balance_nano: 0,
            decimals: 9,
        };
        Self {
            available_tokens: vec![native],
            selected_symbol: "TON".to_string(),
            is_dropdown_open: false,
        }
    }

    pub fn add_jetton(&mut self, token: TokenItem) {
        if !self.available_tokens.iter().any(|t| t.symbol == token.symbol) {
            self.available_tokens.push(token);
        }
    }

    pub fn select_token(&mut self, symbol: &str) -> bool {
        if self.available_tokens.iter().any(|t| t.symbol == symbol) {
            self.selected_symbol = symbol.to_string();
            self.is_dropdown_open = false;
            true
        } else {
            false
        }
    }
}
