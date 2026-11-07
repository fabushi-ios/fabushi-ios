//! TON NFT Grid Item component rendering digital collectibles and Fragment assets.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonNftGridItem {
    pub address: String,
    pub name: String,
    pub collection_name: String,
    pub image_url: Option<String>,
    pub is_selected: bool,
    pub on_sale: bool,
}

impl TonNftGridItem {
    pub fn new(address: impl Into<String>, name: impl Into<String>, collection: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            name: name.into(),
            collection_name: collection.into(),
            image_url: None,
            is_selected: false,
            on_sale: false,
        }
    }

    pub fn set_image(&mut self, url: impl Into<String>) {
        self.image_url = Some(url.into());
    }

    pub fn toggle_select(&mut self) {
        self.is_selected = !self.is_selected;
    }
}
