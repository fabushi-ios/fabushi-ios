//! Target Symbol: LocationPickersComponents
//! Telegram Desktop 1:1 Submodule: `LocationPickersComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PickedLocation {
    pub lat: f64,
    pub lon: f64,
    pub address: String,
    pub title: String,
}

/// Target Symbol for Module 03 Ledger: LocationPickersComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct LocationPickersComponents {
    pub current_pick: Option<PickedLocation>,
}

impl LocationPickersComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pick(&mut self, lat: f64, lon: f64, title: impl Into<String>, address: impl Into<String>) {
        self.current_pick = Some(PickedLocation {
            lat,
            lon,
            address: address.into(),
            title: title.into(),
        });
    }

    pub fn clear(&mut self) {
        self.current_pick = None;
    }
}
