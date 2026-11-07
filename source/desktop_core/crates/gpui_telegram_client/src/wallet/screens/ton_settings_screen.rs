//! Authentic TON Settings Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TonNetwork {
    Mainnet,
    Testnet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonSettingsScreen {
    pub active_currency: String,
    pub available_currencies: Vec<String>,
    pub network: TonNetwork,
    pub connected_apps_count: usize,
    pub app_version: String,
    pub notifications_enabled: bool,
}

impl TonSettingsScreen {
    pub fn new() -> Self {
        Self {
            active_currency: "USD".into(),
            available_currencies: vec![
                "USD".into(),
                "EUR".into(),
                "GBP".into(),
                "RUB".into(),
                "AED".into(),
            ],
            network: TonNetwork::Mainnet,
            connected_apps_count: 0,
            app_version: "1.0.0".into(),
            notifications_enabled: true,
        }
    }

    pub fn set_currency(&mut self, currency: impl Into<String>) {
        self.active_currency = currency.into();
    }

    pub fn set_network(&mut self, net: TonNetwork) {
        self.network = net;
    }

    pub fn toggle_notifications(&mut self) {
        self.notifications_enabled = !self.notifications_enabled;
    }
}

impl Default for TonSettingsScreen {
    fn default() -> Self {
        Self::new()
    }
}
