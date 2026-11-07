//! Authentic TON QR Receive Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonQrReceiveScreen {
    pub wallet_address: String,
    pub requested_amount_nanotons: Option<u64>,
    pub requested_comment: Option<String>,
    pub qr_data_uri: String,
    pub copied_to_clipboard: bool,
}

impl TonQrReceiveScreen {
    pub fn new(address: impl Into<String>) -> Self {
        let addr = address.into();
        let uri = format!("ton://transfer/{}", addr);
        Self {
            wallet_address: addr,
            requested_amount_nanotons: None,
            requested_comment: None,
            qr_data_uri: uri,
            copied_to_clipboard: false,
        }
    }

    pub fn set_amount(&mut self, nanotons: Option<u64>) {
        self.requested_amount_nanotons = nanotons;
        self.rebuild_uri();
    }

    pub fn set_comment(&mut self, comment: Option<String>) {
        self.requested_comment = comment;
        self.rebuild_uri();
    }

    fn rebuild_uri(&mut self) {
        let mut uri = format!("ton://transfer/{}", self.wallet_address);
        let mut params = Vec::new();
        if let Some(amount) = self.requested_amount_nanotons {
            params.push(format!("amount={}", amount));
        }
        if let Some(ref text) = self.requested_comment {
            params.push(format!("text={}", text));
        }
        if !params.is_empty() {
            uri.push('?');
            uri.push_str(&params.join("&"));
        }
        self.qr_data_uri = uri;
    }

    pub fn mark_copied(&mut self) {
        self.copied_to_clipboard = true;
    }
}

impl Default for TonQrReceiveScreen {
    fn default() -> Self {
        Self::new("")
    }
}
