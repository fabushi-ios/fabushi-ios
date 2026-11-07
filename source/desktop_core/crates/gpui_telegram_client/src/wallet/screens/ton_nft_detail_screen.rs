//! Authentic TON NFT Detail Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NftAttribute {
    pub trait_type: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonNftDetailScreen {
    pub item_address: String,
    pub collection_address: Option<String>,
    pub collection_name: Option<String>,
    pub name: String,
    pub description: String,
    pub image_url: String,
    pub attributes: Vec<NftAttribute>,
    pub owner_address: String,
    pub is_transferable: bool,
    pub is_transfer_in_progress: bool,
}

impl TonNftDetailScreen {
    pub fn new(address: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            item_address: address.into(),
            collection_address: None,
            collection_name: None,
            name: name.into(),
            description: String::new(),
            image_url: String::new(),
            attributes: Vec::new(),
            owner_address: String::new(),
            is_transferable: true,
            is_transfer_in_progress: false,
        }
    }

    pub fn set_collection(&mut self, address: impl Into<String>, name: impl Into<String>) {
        self.collection_address = Some(address.into());
        self.collection_name = Some(name.into());
    }

    pub fn add_attribute(&mut self, trait_type: impl Into<String>, value: impl Into<String>) {
        self.attributes.push(NftAttribute {
            trait_type: trait_type.into(),
            value: value.into(),
        });
    }

    pub fn start_transfer(&mut self) -> bool {
        if self.is_transferable && !self.is_transfer_in_progress {
            self.is_transfer_in_progress = true;
            true
        } else {
            false
        }
    }
}

impl Default for TonNftDetailScreen {
    fn default() -> Self {
        Self::new("", "")
    }
}
