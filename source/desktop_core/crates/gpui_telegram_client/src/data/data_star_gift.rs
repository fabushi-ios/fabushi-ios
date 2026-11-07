//! Target Symbol: DataStarGift
//! Telegram Desktop 1:1 Submodule: `data/data_star_gift`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UniqueGiftRarity {
    #[default]
    Default,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniqueGift {
    pub collectible_id: u64,
    pub slug: String,
    pub title: String,
    pub number: u32,
    pub rarity: UniqueGiftRarity,
    pub model_document_id: u64,
    pub owner_address: Option<String>,
    pub resale_stars: Option<i64>,
    pub resale_ton: Option<i64>,
    pub can_transfer_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataStarGift {
    pub id: u64,
    pub stars_cost: i64,
    pub stars_converted: i64,
    pub document_id: u64,
    pub limited_left: Option<u32>,
    pub limited_total: Option<u32>,
    pub is_birthday: bool,
    pub is_sold_out: bool,
    pub unique_gift: Option<UniqueGift>,
}

impl Default for DataStarGift {
    fn default() -> Self {
        Self {
            id: 0,
            stars_cost: 0,
            stars_converted: 0,
            document_id: 0,
            limited_left: None,
            limited_total: None,
            is_birthday: false,
            is_sold_out: false,
            unique_gift: None,
        }
    }
}

impl DataStarGift {
    pub fn standard(id: u64, stars_cost: i64, document_id: u64) -> Self {
        Self {
            id,
            stars_cost,
            stars_converted: (stars_cost * 85) / 100, // 85% conversion to stars
            document_id,
            ..Default::default()
        }
    }

    pub fn limited(id: u64, stars_cost: i64, document_id: u64, total: u32) -> Self {
        Self {
            id,
            stars_cost,
            stars_converted: (stars_cost * 85) / 100,
            document_id,
            limited_total: Some(total),
            limited_left: Some(total),
            ..Default::default()
        }
    }

    pub fn with_unique(mut self, unique: UniqueGift) -> Self {
        self.unique_gift = Some(unique);
        self
    }

    pub fn is_limited(&self) -> bool {
        self.limited_total.is_some()
    }

    pub fn is_collectible(&self) -> bool {
        self.unique_gift.is_some()
    }

    pub fn rarity_name(rarity: UniqueGiftRarity) -> &'static str {
        match rarity {
            UniqueGiftRarity::Default => "Common",
            UniqueGiftRarity::Uncommon => "Uncommon",
            UniqueGiftRarity::Rare => "Rare",
            UniqueGiftRarity::Epic => "Epic",
            UniqueGiftRarity::Legendary => "Legendary",
        }
    }
}
