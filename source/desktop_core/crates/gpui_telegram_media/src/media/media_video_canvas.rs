//! Telegram Desktop 1:1 Parity Submodule: `MediaVideoCanvas`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/media_video_canvas.h:L1-L29`
//! - `Telegram/SourceFiles/media/media_video_canvas.cpp:L1-L71`
//! Subsystem: `media` | Feature: `TCG-N08`

pub const K_SAMPLE_SIDE: i32 = 240;
pub const K_SAMPLE_TOP: f64 = 0.1;
pub const K_SAMPLE_BOTTOM: f64 = 0.9;

/// RGB color structure for canvas background analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Canvas background gradient corresponding to `Media::Encode::CanvasBackground` (media_video_canvas.h:L12-L19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CanvasBackground {
    pub top: RgbColor,
    pub bottom: RgbColor,
    pub is_valid: bool,
}

impl CanvasBackground {
    pub fn new(top: RgbColor, bottom: RgbColor) -> Self {
        Self {
            top,
            bottom,
            is_valid: true,
        }
    }

    pub fn valid(&self) -> bool {
        self.is_valid
    }
}

/// Converts RGB (0..255) to HSV (hue: 0..360, sat: 0..1, val: 0..1).
pub fn rgb_to_hsv(rgb: RgbColor) -> (f64, f64, f64) {
    let r = rgb.r as f64 / 255.0;
    let g = rgb.g as f64 / 255.0;
    let b = rgb.b as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let h = if hue < 0.0 { hue + 360.0 } else { hue };
    let s = if max == 0.0 { 0.0 } else { delta / max };
    let v = max;
    (h, s, v)
}

/// Converts HSV to RGB.
pub fn hsv_to_rgb(h: f64, s: f64, v: f64) -> RgbColor {
    let c = v * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match (h / 60.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    RgbColor::new(
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

/// Adapts color in HSV space (media_video_canvas.cpp:L17-L30).
pub fn adapt_color(color: RgbColor) -> RgbColor {
    let (hue, mut saturation, mut value) = rgb_to_hsv(color);
    value = (value - 0.05).clamp(0.15, 0.85);
    if saturation > 0.1 && saturation <= 0.95 {
        if saturation <= 0.5 {
            saturation = (saturation + 0.2).min(1.0);
        } else if saturation > 0.8 {
            saturation = (saturation - 0.4).max(0.0);
        }
    }
    hsv_to_rgb(hue, saturation, value)
}

/// Extracts dominant canvas gradient from top and bottom sampled colors (L34-L53).
pub fn dominant_canvas_background(raw_top: RgbColor, raw_bottom: RgbColor) -> CanvasBackground {
    CanvasBackground::new(adapt_color(raw_top), adapt_color(raw_bottom))
}

/// Parity target coordinator struct `MediaVideoCanvas`.
#[derive(Debug, Clone, Default)]
pub struct MediaVideoCanvas {
    pub background: CanvasBackground,
}

impl MediaVideoCanvas {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_background(&mut self, top: RgbColor, bottom: RgbColor) {
        self.background = dominant_canvas_background(top, bottom);
    }

    pub fn is_valid(&self) -> bool {
        self.background.valid()
    }
}
