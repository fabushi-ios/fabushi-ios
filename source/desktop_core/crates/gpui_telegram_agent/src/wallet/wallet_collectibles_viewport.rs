//! Authentic TON Wallet Collectibles Viewport Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_collectibles_viewport.cpp, Telegram/SourceFiles/wallet/wallet_collectibles_viewport.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCollectiblesViewport {
    pub width: i32,
    pub height: i32,
    pub visible_top: i32,
    pub visible_bottom: i32,
    pub page_y: i32,
    pub page_height: i32,
}

impl WalletCollectiblesViewport {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            visible_top: 0,
            visible_bottom: 0,
            page_y: 0,
            page_height: 0,
        }
    }

    /// Upstream: `visibleTopBottomUpdated` calculation
    pub fn update_visible_bounds(&mut self, top: i32, bottom: i32) -> (i32, i32) {
        self.visible_top = top;
        self.visible_bottom = bottom;

        // Calculate clamped visible page range
        let effective_top = top.max(self.page_y);
        let effective_bottom = bottom.min(self.page_y + self.page_height);
        (effective_top, effective_bottom)
    }

    pub fn resize_get_height(&mut self, new_width: i32, content_height: i32) -> i32 {
        self.width = new_width;
        self.page_height = content_height;
        self.height = content_height;
        self.height
    }
}

impl Default for WalletCollectiblesViewport {
    fn default() -> Self {
        Self::new()
    }
}
