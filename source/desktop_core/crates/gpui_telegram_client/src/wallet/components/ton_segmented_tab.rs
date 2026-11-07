//! TON Segmented Tab bar component for wallet section switching.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonSegmentedTab {
    pub tabs: Vec<String>,
    pub selected_index: usize,
}

impl TonSegmentedTab {
    pub fn new(tabs: Vec<String>) -> Self {
        Self {
            tabs,
            selected_index: 0,
        }
    }

    pub fn select(&mut self, index: usize) -> bool {
        if index < self.tabs.len() {
            self.selected_index = index;
            true
        } else {
            false
        }
    }

    pub fn current_tab(&self) -> Option<&str> {
        self.tabs.get(self.selected_index).map(|s| s.as_str())
    }
}
