//! Target Symbol: DataLocation
//! Telegram Desktop 1:1 Submodule: `data/data_location`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataLocation {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_radius_meters: Option<u32>,
    pub live_period_seconds: Option<u32>,
    pub venue_title: Option<String>,
    pub venue_address: Option<String>,
    pub venue_provider: Option<String>,
    pub venue_id: Option<String>,
}

impl Default for DataLocation {
    fn default() -> Self {
        Self {
            latitude: 0.0,
            longitude: 0.0,
            accuracy_radius_meters: None,
            live_period_seconds: None,
            venue_title: None,
            venue_address: None,
            venue_provider: None,
            venue_id: None,
        }
    }
}

impl DataLocation {
    pub fn point(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
            ..Default::default()
        }
    }

    pub fn live(latitude: f64, longitude: f64, period_seconds: u32) -> Self {
        Self {
            latitude,
            longitude,
            live_period_seconds: Some(period_seconds),
            ..Default::default()
        }
    }

    pub fn venue(
        latitude: f64,
        longitude: f64,
        title: impl Into<String>,
        address: impl Into<String>,
    ) -> Self {
        Self {
            latitude,
            longitude,
            venue_title: Some(title.into()),
            venue_address: Some(address.into()),
            ..Default::default()
        }
    }

    pub fn is_live(&self) -> bool {
        self.live_period_seconds.is_some()
    }

    pub fn is_venue(&self) -> bool {
        self.venue_title.is_some()
    }
}
