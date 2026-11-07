//! Target Symbol: DataBusinessInfoBusiness
//! Telegram Desktop 1:1 Submodule: `data/business/data_business_info`
//! Subsystem: `data`, Feature ID: `TCG-N27`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataBusinessInfoBusiness {
    pub address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub intro_title: Option<String>,
    pub intro_description: Option<String>,
    pub intro_sticker_document_id: Option<u64>,
}

impl DataBusinessInfoBusiness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_location(&mut self, address: impl Into<String>, lat: f64, lon: f64) {
        self.address = Some(address.into());
        self.latitude = Some(lat);
        self.longitude = Some(lon);
    }

    pub fn set_intro(&mut self, title: impl Into<String>, desc: impl Into<String>, sticker_id: Option<u64>) {
        self.intro_title = Some(title.into());
        self.intro_description = Some(desc.into());
        self.intro_sticker_document_id = sticker_id;
    }
}
