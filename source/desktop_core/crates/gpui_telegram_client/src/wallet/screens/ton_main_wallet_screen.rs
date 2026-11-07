//! Authentic TON Main Wallet Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetItem {
    pub symbol: String,
    pub name: String,
    pub balance_nanos: u64,
    pub decimals: u8,
    pub fiat_value: f64,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonMainWalletScreen {
    pub address_bounceable: String,
    pub address_raw: String,
    pub total_balance_nanotons: u64,
    pub fiat_currency: String,
    pub total_fiat_value: f64,
    pub assets: Vec<AssetItem>,
    pub is_refreshing: bool,
    pub active_view_tab: String,
    pub status_banner: Option<String>,
}

impl TonMainWalletScreen {
    pub fn new(bounceable: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            address_bounceable: bounceable.into(),
            address_raw: raw.into(),
            total_balance_nanotons: 0,
            fiat_currency: "USD".into(),
            total_fiat_value: 0.0,
            assets: Vec::new(),
            is_refreshing: false,
            active_view_tab: "assets".into(),
            status_banner: None,
        }
    }

    pub fn set_balance(&mut self, nanotons: u64, usd_rate: f64) {
        self.total_balance_nanotons = nanotons;
        let ton_fiat = (nanotons as f64 / 1_000_000_000.0) * usd_rate;
        let jettons_fiat: f64 = self.assets.iter().map(|a| a.fiat_value).sum();
        self.total_fiat_value = ton_fiat + jettons_fiat;
    }

    pub fn add_asset(&mut self, asset: AssetItem) {
        if let Some(pos) = self.assets.iter().position(|a| a.symbol == asset.symbol) {
            self.assets[pos] = asset;
        } else {
            self.assets.push(asset);
        }
    }

    pub fn set_tab(&mut self, tab_name: impl Into<String>) {
        self.active_view_tab = tab_name.into();
    }
}

impl Default for TonMainWalletScreen {
    fn default() -> Self {
        Self::new("", "")
    }
}
