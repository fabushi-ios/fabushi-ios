//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupViewportRasterGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/calls_group_viewport_raster.h`
//! - `Telegram/SourceFiles/calls/group/calls_group_viewport_raster.cpp:L25-L135`

use serde::{Deserialize, Serialize};

/// Upstream software fallback large blur radius (15px)
/// calls_group_viewport_raster.cpp:L25
pub const K_BLUR_RADIUS: u32 = 15;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RasterTileData {
    pub endpoint_id: u64,
    pub has_userpic_frame: bool,
    pub has_blurred_frame: bool,
    pub is_stale: bool,
    pub is_paused: bool,
}

/// Software Fallback Raster Viewport Renderer
/// Target Symbol: `CallsGroupViewportRasterGroup`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CallsGroupViewportRasterGroup {
    pub tile_entries: Vec<RasterTileData>,
    pub fallback_bg_color: u32,
    pub is_fullscreen: bool,
}

impl CallsGroupViewportRasterGroup {
    pub fn new() -> Self {
        Self {
            tile_entries: Vec::new(),
            fallback_bg_color: 0xFF18222D, // Telegram dark call background
            is_fullscreen: false,
        }
    }

    /// Begin paint pass: mark all tiles as stale
    /// calls_group_viewport_raster.cpp:L46-L48
    pub fn begin_paint_pass(&mut self) {
        for tile in &mut self.tile_entries {
            tile.is_stale = true;
        }
    }

    /// Process a visible tile: touch and update blur/userpic flags
    /// calls_group_viewport_raster.cpp:L92-L120
    pub fn paint_tile(&mut self, endpoint_id: u64, is_paused: bool, has_video_stream: bool) {
        if let Some(tile) = self.tile_entries.iter_mut().find(|t| t.endpoint_id == endpoint_id) {
            tile.is_stale = false;
            tile.is_paused = is_paused;
            tile.has_userpic_frame = !has_video_stream;
            tile.has_blurred_frame = is_paused;
        } else {
            self.tile_entries.push(RasterTileData {
                endpoint_id,
                has_userpic_frame: !has_video_stream,
                has_blurred_frame: is_paused,
                is_stale: false,
                is_paused,
            });
        }
    }

    /// End paint pass: purge stale tile records
    /// calls_group_viewport_raster.cpp:L64-L70
    pub fn end_paint_pass(&mut self) -> usize {
        let before = self.tile_entries.len();
        self.tile_entries.retain(|t| !t.is_stale);
        before - self.tile_entries.len()
    }

    pub fn blur_radius(&self) -> u32 {
        K_BLUR_RADIUS
    }

    pub fn tile_count(&self) -> usize {
        self.tile_entries.len()
    }
}
