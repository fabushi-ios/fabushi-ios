//! TON QR Code Scanner Overlay component with scanning viewfinder animation.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonQrScannerOverlay {
    pub is_active: bool,
    pub torch_enabled: bool,
    pub scanned_code: Option<String>,
    pub error_message: Option<String>,
}

impl TonQrScannerOverlay {
    pub fn new() -> Self {
        Self {
            is_active: false,
            torch_enabled: false,
            scanned_code: None,
            error_message: None,
        }
    }

    pub fn start(&mut self) {
        self.is_active = true;
        self.scanned_code = None;
        self.error_message = None;
    }

    pub fn stop(&mut self) {
        self.is_active = false;
        self.torch_enabled = false;
    }

    pub fn on_scan_success(&mut self, code: impl Into<String>) {
        self.scanned_code = Some(code.into());
        self.is_active = false;
    }

    pub fn toggle_torch(&mut self) {
        self.torch_enabled = !self.torch_enabled;
    }
}
