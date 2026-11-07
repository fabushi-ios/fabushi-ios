//! Telegram Stories Multi-Segment Progress Slider
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_slider.h:L22-L63`
//! - `Telegram/SourceFiles/media/stories/media_stories_slider.cpp:L18-L155`

use serde::{Deserialize, Serialize};

/// Opacity for unplayed story segments.
/// Upstream: `media_stories_slider.cpp:L20`
pub const K_OPACITY_INACTIVE: f32 = 0.4;

/// Opacity for completed and active story segments.
/// Upstream: `media_stories_slider.cpp:L21`
pub const K_OPACITY_ACTIVE: f32 = 1.0;

/// Default segment bar thickness (single) and gap (skip) in pixels.
/// Upstream: `media_stories_slider.cpp:L91-L92`
pub const K_DEFAULT_SLIDER_WIDTH: f32 = 2.0;
pub const K_DEFAULT_SLIDER_SKIP: f32 = 3.0;

/// Story slider configuration data.
/// Upstream: `media_stories_slider.h:L26-L33`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SliderData {
    pub index: usize,
    pub total: usize,
    pub video_stream: bool,
}

/// A computed progress bar segment rectangle.
/// Upstream: `media_stories_slider.cpp:L90-L154`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SegmentGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub opacity: f32,
    pub is_active: bool,
    pub active_fill_width: f32,
}

/// Stories slider layout and geometry calculator.
/// Upstream: `media_stories_slider.h:L35-L61`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStoriesSliderStories {
    pub data: SliderData,
    pub progress: f32,
    pub single_height: f32,
    pub skip_gap: f32,
}

impl Default for MediaStoriesSliderStories {
    fn default() -> Self {
        Self {
            data: SliderData::default(),
            progress: 0.0,
            single_height: K_DEFAULT_SLIDER_WIDTH,
            skip_gap: K_DEFAULT_SLIDER_SKIP,
        }
    }
}

impl MediaStoriesSliderStories {
    pub fn new(total: usize, index: usize) -> Self {
        let total = total.max(1);
        let index = index.min(total - 1);
        Self {
            data: SliderData {
                index,
                total,
                video_stream: false,
            },
            progress: 0.0,
            single_height: K_DEFAULT_SLIDER_WIDTH,
            skip_gap: K_DEFAULT_SLIDER_SKIP,
        }
    }

    pub fn set_progress(&mut self, progress: f32) {
        self.progress = progress.clamp(0.0, 1.0);
    }

    pub fn set_index(&mut self, index: usize) {
        if self.data.total > 0 {
            self.data.index = index.min(self.data.total - 1);
            self.progress = 0.0;
        }
    }

    /// Computes segment layout geometries for a given container width.
    /// Upstream: `media_stories_slider.cpp:L90-L112`
    pub fn compute_layout(&self, container_width: f32) -> Vec<SegmentGeometry> {
        let single = self.single_height;
        let skip = self.skip_gap;

        let max = (((container_width + skip) / (single + skip)).floor() as usize).max(1);
        let count = self.data.total.clamp(1, max);
        let one = (container_width - (count as f32 - 1.0) * skip) / count as f32;
        let radius = single / 2.0;

        let mut segments = Vec::with_capacity(count);
        let mut left = 0.0;

        for i in 0..count {
            let is_active = i == self.data.index;
            let opacity = if i < self.data.index {
                K_OPACITY_ACTIVE
            } else if i > self.data.index {
                K_OPACITY_INACTIVE
            } else {
                K_OPACITY_ACTIVE
            };

            let active_fill = if is_active {
                (one * self.progress).max(single).min(one)
            } else if i < self.data.index {
                one
            } else {
                0.0
            };

            segments.push(SegmentGeometry {
                x: left,
                y: 0.0,
                width: one,
                height: single,
                radius,
                opacity,
                is_active,
                active_fill_width: active_fill,
            });

            left += one + skip;
        }

        segments
    }
}
