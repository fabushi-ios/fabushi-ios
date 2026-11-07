//! Target Symbol: DataBotApp
//! Telegram Desktop 1:1 Submodule: `data/data_bot_app`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataBotApp {
    pub id: u64,
    pub access_hash: u64,
    pub short_name: String,
    pub title: String,
    pub description: String,
    pub photo_id: Option<u64>,
    pub document_id: Option<u64>,
    pub hash: u64,
    pub is_inactive: bool,
    pub request_write_access: bool,
}

impl Default for DataBotApp {
    fn default() -> Self {
        Self {
            id: 0,
            access_hash: 0,
            short_name: String::new(),
            title: String::new(),
            description: String::new(),
            photo_id: None,
            document_id: None,
            hash: 0,
            is_inactive: false,
            request_write_access: false,
        }
    }
}

impl DataBotApp {
    pub fn new(id: u64, access_hash: u64, short_name: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id,
            access_hash,
            short_name: short_name.into(),
            title: title.into(),
            ..Default::default()
        }
    }
}
