//! Gram API NFT querying, classification, and metadata parsing.
//! Upstream: Telegram/SourceFiles/gram/api/gram_api_nft.cpp, gram_api_nft.h

#![allow(dead_code)]

use super::gram_api_request::{api_details, HttpRequest};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NftKind {
    Generic,
    TelegramGift,
    TelegramNumber,
    TelegramUsername,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NftWebDocument {
    pub url: Vec<u8>,
    pub access_hash: u64,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NftItem {
    pub address: String,
    pub collection: String,
    pub real_owner: String,
    pub index: String,
    pub content_uri: String,
    pub domain: String,
    pub key: String,
    pub collection_name: String,
    pub name: String,
    pub image: Option<NftWebDocument>,
    pub image_small: Option<NftWebDocument>,
    pub content_url: Option<NftWebDocument>,
    pub lottie: Option<NftWebDocument>,
    pub kind: NftKind,
    pub on_sale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NftPage {
    pub list: Vec<NftItem>,
    pub has_next: bool,
}

pub const FRAGMENT_NUMBER_COLLECTION: &str =
    "0:0e41dc1dc3c9067ed24248580e12b3359818d83dee0304fabcf80845eafafdb2";
pub const FRAGMENT_USERNAME_COLLECTION: &str =
    "0:80d78a35f955a14b679faa887ff4cd5bfc0f43b4a4eea2a7e6927f3701b273c2";

pub fn canonical_raw_address(value: &serde_json::Value) -> String {
    let s = match value.as_str() {
        Some(val) => val.trim().to_ascii_lowercase(),
        None => return String::new(),
    };
    if let Some((wc, hex)) = s.split_once(':') {
        if (wc == "0" || wc == "-1") && hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return format!("{}:{}", wc, hex);
        }
    }
    s
}

pub fn classify_nft_kind(item: &mut NftItem) {
    let coll = item.collection.to_ascii_lowercase();
    if coll == FRAGMENT_NUMBER_COLLECTION {
        item.kind = NftKind::TelegramNumber;
    } else if coll == FRAGMENT_USERNAME_COLLECTION {
        item.kind = NftKind::TelegramUsername;
    } else if item.content_uri.contains("gift") || item.name.to_ascii_lowercase().contains("gift") {
        item.kind = NftKind::TelegramGift;
    } else {
        item.kind = NftKind::Generic;
    }
}

pub fn nft_item_by_address_request(address: &str) -> HttpRequest {
    HttpRequest {
        post: false,
        endpoint: "/api/v2/getNftItem".to_string(),
        query: format!("address={}", api_details::percent_encoded(address)),
        payload: Vec::new(),
    }
}

pub fn parse_nft_items(json_bytes: &[u8], limit: usize) -> Option<NftPage> {
    let document: serde_json::Value = serde_json::from_slice(json_bytes).ok()?;
    let items_array = document.get("nft_items").and_then(|v| v.as_array())?;

    let mut list = Vec::new();
    for entry in items_array {
        let mut item = NftItem {
            address: canonical_raw_address(entry.get("address").unwrap_or(&serde_json::Value::Null)),
            collection: canonical_raw_address(entry.get("collection_address").unwrap_or(&serde_json::Value::Null)),
            real_owner: canonical_raw_address(entry.get("owner_address").unwrap_or(&serde_json::Value::Null)),
            index: entry.get("index").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            content_uri: entry.get("content_uri").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            domain: entry.get("domain").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            key: entry.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            collection_name: entry.get("collection_name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            name: entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            image: None,
            image_small: None,
            content_url: None,
            lottie: None,
            kind: NftKind::Generic,
            on_sale: entry.get("on_sale").and_then(|v| v.as_bool()).unwrap_or(false),
        };
        classify_nft_kind(&mut item);
        list.push(item);
    }
    let has_next = list.len() >= limit && limit > 0;
    Some(NftPage { list, has_next })
}

/// Upstream target symbol: GramApiNftApi
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramApiNftApi {
    pub cached_items: Vec<NftItem>,
}

impl GramApiNftApi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build_item_request(&self, address: &str) -> HttpRequest {
        nft_item_by_address_request(address)
    }

    pub fn load_page(&mut self, json_payload: &[u8], limit: usize) -> Option<NftPage> {
        let page = parse_nft_items(json_payload, limit);
        if let Some(ref p) = page {
            self.cached_items = p.list.clone();
        }
        page
    }

    pub fn items_count(&self) -> usize {
        self.cached_items.len()
    }
}
