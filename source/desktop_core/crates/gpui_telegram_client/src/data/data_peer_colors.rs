//! Target Symbol: DataPeerColors
//! Telegram Desktop 1:1 Submodule: `DataPeerColors`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ColorProfileSet {
    pub palette: Vec<u32>,
    pub bg: Vec<u32>,
    pub story: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ColorProfileData {
    pub light: ColorProfileSet,
    pub dark: ColorProfileSet,
}

/// Target Symbol for Module 03 Ledger: DataPeerColors coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPeerColors {
    pub profiles: Vec<ColorProfileData>,
}

impl DataPeerColors {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_profile(&mut self, profile: ColorProfileData) {
        self.profiles.push(profile);
    }

    pub fn default_color(color_index: u8) -> u32 {
        // Telegram default 7 peer colors (Red, Orange, Violet, Green, Cyan, Blue, Pink)
        const DEFAULT_PALETTE: [u32; 7] = [
            0xE17076, 0xFAA774, 0xA695E7, 0x7BC862, 0x6EC9CB, 0x65AADD, 0xEE7AAE,
        ];
        DEFAULT_PALETTE[(color_index as usize) % DEFAULT_PALETTE.len()]
    }
}
