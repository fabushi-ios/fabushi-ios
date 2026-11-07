//! Gram NFT classification and parsing unit test suite.
//! Upstream: Telegram/SourceFiles/gram/tests/gram_nft_tests.cpp

#![allow(dead_code)]

use super::gram_api_nft::{
    canonical_raw_address, classify_nft_kind, NftItem, NftKind,
    FRAGMENT_NUMBER_COLLECTION, FRAGMENT_USERNAME_COLLECTION,
};
use serde::{Deserialize, Serialize};

/// Upstream target symbol: GramNftTestsTests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramNftTestsTests {
    pub pass_count: usize,
    pub fail_count: usize,
}

impl GramNftTestsTests {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn test_canonical_raw_address(&mut self) -> bool {
        let val = serde_json::Value::String("0:0E41DC1DC3C9067ED24248580E12B3359818D83DEE0304FABCF80845EAFAFDB2".into());
        let canonical = canonical_raw_address(&val);
        if canonical != FRAGMENT_NUMBER_COLLECTION {
            self.fail_count += 1;
            return false;
        }
        self.pass_count += 1;
        true
    }

    pub fn test_fragment_classification(&mut self) -> bool {
        let mut item_number = NftItem {
            address: "0:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef".into(),
            collection: FRAGMENT_NUMBER_COLLECTION.into(),
            real_owner: "0:1111111111111111111111111111111111111111111111111111111111111111".into(),
            index: "1".into(),
            content_uri: "".into(),
            domain: "+888 0001".into(),
            key: "".into(),
            collection_name: "Anonymous Telegram Numbers".into(),
            name: "+888 0001".into(),
            image: None,
            image_small: None,
            content_url: None,
            lottie: None,
            kind: NftKind::Generic,
            on_sale: false,
        };
        classify_nft_kind(&mut item_number);
        if item_number.kind != NftKind::TelegramNumber {
            self.fail_count += 1;
            return false;
        }

        let mut item_username = item_number.clone();
        item_username.collection = FRAGMENT_USERNAME_COLLECTION.into();
        classify_nft_kind(&mut item_username);
        if item_username.kind != NftKind::TelegramUsername {
            self.fail_count += 1;
            return false;
        }

        self.pass_count += 1;
        true
    }

    pub fn run_all(&mut self) -> bool {
        let t1 = self.test_canonical_raw_address();
        let t2 = self.test_fragment_classification();
        t1 && t2
    }
}
