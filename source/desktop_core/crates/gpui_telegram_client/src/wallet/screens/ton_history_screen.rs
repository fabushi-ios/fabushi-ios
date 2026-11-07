//! Authentic TON History Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryFilter {
    All,
    Incoming,
    Outgoing,
    Tokens,
    Collectibles,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionHistoryEntry {
    pub tx_hash: String,
    pub logical_time: u64,
    pub timestamp: u64,
    pub amount_nanotons: i64, // Positive = received, Negative = sent
    pub fee_nanotons: u64,
    pub peer_address: String,
    pub comment: Option<String>,
    pub is_jetton: bool,
    pub token_symbol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonHistoryScreen {
    pub transactions: Vec<TransactionHistoryEntry>,
    pub active_filter: HistoryFilter,
    pub is_loading_more: bool,
    pub has_more_transactions: bool,
    pub selected_tx_hash: Option<String>,
}

impl TonHistoryScreen {
    pub fn new() -> Self {
        Self {
            transactions: Vec::new(),
            active_filter: HistoryFilter::All,
            is_loading_more: false,
            has_more_transactions: true,
            selected_tx_hash: None,
        }
    }

    pub fn set_filter(&mut self, filter: HistoryFilter) {
        self.active_filter = filter;
    }

    pub fn append_transactions(&mut self, new_txs: Vec<TransactionHistoryEntry>, has_more: bool) {
        for tx in new_txs {
            if !self.transactions.iter().any(|t| t.tx_hash == tx.tx_hash) {
                self.transactions.push(tx);
            }
        }
        self.transactions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        self.has_more_transactions = has_more;
        self.is_loading_more = false;
    }

    pub fn filtered_transactions(&self) -> Vec<&TransactionHistoryEntry> {
        self.transactions
            .iter()
            .filter(|tx| match self.active_filter {
                HistoryFilter::All => true,
                HistoryFilter::Incoming => tx.amount_nanotons > 0,
                HistoryFilter::Outgoing => tx.amount_nanotons < 0,
                HistoryFilter::Tokens => tx.is_jetton,
                HistoryFilter::Collectibles => false, // NFT specific filter
            })
            .collect()
    }

    pub fn select_transaction(&mut self, hash: impl Into<String>) {
        self.selected_tx_hash = Some(hash.into());
    }

    pub fn clear_selection(&mut self) {
        self.selected_tx_hash = None;
    }
}

impl Default for TonHistoryScreen {
    fn default() -> Self {
        Self::new()
    }
}
