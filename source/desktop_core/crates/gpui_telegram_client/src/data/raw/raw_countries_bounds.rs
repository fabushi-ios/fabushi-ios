//! Target Symbol: RawCountriesBoundsRaw
//! Telegram Desktop 1:1 Submodule: `RawCountriesBoundsRaw`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Latitude/Longitude bounding box for country geolocation limits.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct GeoBounds {
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
}

impl GeoBounds {
    pub const fn new(min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> Self {
        Self { min_lat, min_lon, max_lat, max_lon }
    }

    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        lat >= self.min_lat && lat <= self.max_lat && lon >= self.min_lon && lon <= self.max_lon
    }
}

/// Target Symbol for Module 03 Ledger: RawCountriesBoundsRaw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RawCountriesBoundsRaw {
    pub bounds: HashMap<String, GeoBounds>,
}

impl RawCountriesBoundsRaw {
    pub fn new() -> Self {
        let mut bounds = HashMap::new();
        // Seed representative country bounds for ISO-3166-1 alpha-2 codes
        bounds.insert("US".to_string(), GeoBounds::new(24.396308, -125.0, 49.384358, -66.93457));
        bounds.insert("GB".to_string(), GeoBounds::new(49.871159, -8.64937, 60.860699, 1.76896));
        bounds.insert("DE".to_string(), GeoBounds::new(47.270111, 5.866342, 55.058301, 15.041896));
        bounds.insert("FR".to_string(), GeoBounds::new(41.33374, -5.142222, 51.089062, 9.560094));
        bounds.insert("JP".to_string(), GeoBounds::new(24.04575, 122.933333, 45.52314, 153.986667));
        bounds.insert("CN".to_string(), GeoBounds::new(18.153521, 73.499733, 53.56097, 134.775456));
        Self { bounds }
    }

    pub fn find_country(&self, lat: f64, lon: f64) -> Option<&str> {
        for (code, box_bounds) in &self.bounds {
            if box_bounds.contains(lat, lon) {
                return Some(code.as_str());
            }
        }
        None
    }
}
