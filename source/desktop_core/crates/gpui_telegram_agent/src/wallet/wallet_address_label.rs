//! Visual TON address label formatting and line-wrapping calculation.
//! Upstream: Telegram/SourceFiles/wallet/wallet_address_label.cpp, wallet_address_label.h

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub const ADDRESS_GROUP_SIZE: usize = 4;

pub fn details_address_lines(address: &str, max_width_chars: usize) -> Vec<String> {
    let mut groups = Vec::new();
    let chars: Vec<char> = address.chars().collect();
    for chunk in chars.chunks(ADDRESS_GROUP_SIZE) {
        groups.push(chunk.iter().collect::<String>());
    }

    if groups.is_empty() {
        return Vec::new();
    }

    let count = groups.len();
    for lines_count in 1..=count {
        let mut result = Vec::new();
        let mut fits = true;
        let mut offset = 0;

        for i in 0..lines_count {
            let size = (count / lines_count) + if i < (count % lines_count) { 1 } else { 0 };
            let line = groups[offset..offset + size].join(" ");
            offset += size;
            if max_width_chars > 0 && line.len() > max_width_chars {
                fits = false;
            }
            result.push(line);
        }

        if fits || lines_count == count {
            return result;
        }
    }
    vec![groups.join(" ")]
}

/// Upstream target symbol: WalletAddressLabel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletAddressLabel {
    pub raw_address: String,
    pub formatted_lines: Vec<String>,
    pub copy_tooltip_shown: bool,
}

impl WalletAddressLabel {
    pub fn new(address: impl Into<String>, max_width: usize) -> Self {
        let addr = address.into();
        let lines = details_address_lines(&addr, max_width);
        Self {
            raw_address: addr,
            formatted_lines: lines,
            copy_tooltip_shown: false,
        }
    }

    pub fn set_width(&mut self, max_width: usize) {
        self.formatted_lines = details_address_lines(&self.raw_address, max_width);
    }

    pub fn text_display(&self) -> String {
        self.formatted_lines.join("\n")
    }

    pub fn on_copied(&mut self) {
        self.copy_tooltip_shown = true;
    }
}
