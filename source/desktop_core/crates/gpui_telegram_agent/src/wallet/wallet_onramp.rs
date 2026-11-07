//! Authentic TON Wallet Fiat Onramp Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_onramp.cpp, Telegram/SourceFiles/wallet/wallet_onramp.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_CRYPTO_CURRENCY: &str = "TON";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnrampProvider {
    pub name: String,
    pub base_url: String,
    pub supported_currencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletOnramp {
    pub providers: Vec<OnrampProvider>,
    pub crypto_currency: String,
}

impl WalletOnramp {
    pub fn new() -> Self {
        Self {
            providers: vec![
                OnrampProvider {
                    name: "MoonPay".into(),
                    base_url: "https://buy.moonpay.com".into(),
                    supported_currencies: vec!["USD".into(), "EUR".into(), "GBP".into()],
                },
                OnrampProvider {
                    name: "Neocrypto".into(),
                    base_url: "https://buy.neocrypto.net".into(),
                    supported_currencies: vec!["EUR".into(), "USD".into(), "RUB".into()],
                },
            ],
            crypto_currency: K_CRYPTO_CURRENCY.into(),
        }
    }

    /// Generates session URL with query parameters
    pub fn generate_url(
        &self,
        provider_name: &str,
        wallet_address: &str,
        fiat_currency: &str,
    ) -> Option<String> {
        let provider = self.providers.iter().find(|p| p.name == provider_name)?;
        Some(format!(
            "{}?cryptoCurrency={}&walletAddress={}&currencyCode={}",
            provider.base_url, self.crypto_currency, wallet_address, fiat_currency
        ))
    }
}

impl Default for WalletOnramp {
    fn default() -> Self {
        Self::new()
    }
}
