//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupViewportOpenglGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/calls_group_viewport_opengl.h`
//! - `Telegram/SourceFiles/calls/group/calls_group_viewport_opengl.cpp:L29-L125`

use serde::{Deserialize, Serialize};

pub const K_SCALE_FOR_BLUR_TEXTURE_INDEX: u32 = 3;
pub const K_FIRST_BLUR_PASS_TEXTURE_INDEX: u32 = 4;
pub const K_NOISE_TEXTURE_SIZE: u32 = 256;
pub const K_BLUR_TEXTURE_SIZE_FACTOR: f32 = 4.0;
pub const K_BLUR_OPACITY: f32 = 0.65;
pub const K_DITHER_NOISE_AMOUNT: f32 = 0.002;

pub const K_QUADS: usize = 9;
pub const K_QUAD_VERTICES: usize = K_QUADS * 4;
pub const K_QUAD_VALUES: usize = K_QUAD_VERTICES * 4;
pub const K_VALUES: usize = K_QUAD_VALUES + 8;

pub const K_BLUR_RADIUS: i32 = 15;
pub const K_BLUR_DIAMETER: usize = (2 * K_BLUR_RADIUS + 1) as usize;

/// Upstream triangular box filter blur weighting
/// calls_group_viewport_opengl.cpp:L65: boxWeight = fradius + 1.0 - abs(float(i) - fradius)
pub fn blur_box_weight(step: i32, radius: i32) -> f32 {
    let fradius = radius as f32;
    (fradius + 1.0 - (step as f32 - fradius).abs()).max(0.0)
}

/// Upstream Perlin noise generator pseudo-random hash
/// calls_group_viewport_opengl.cpp:L86-L94: rnm(vec2 tc)
pub fn pseudo_noise_sample(x: f32, y: f32) -> f32 {
    let dot = x * 12.9898 + y * 78.233;
    let s = dot.sin() * 43758.5453;
    s - s.floor()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GlProgramType {
    Argb32,
    Yuv420,
}

/// OpenGL Hardware Accelerated Viewport Renderer
/// Target Symbol: `CallsGroupViewportOpenglGroup`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallsGroupViewportOpenglGroup {
    pub is_initialized: bool,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub dither_noise_amount: f32,
    pub blur_opacity: f32,
    pub active_program: GlProgramType,
    pub allocated_textures: u32,
}

impl Default for CallsGroupViewportOpenglGroup {
    fn default() -> Self {
        Self {
            is_initialized: false,
            viewport_width: 1280,
            viewport_height: 720,
            dither_noise_amount: K_DITHER_NOISE_AMOUNT,
            blur_opacity: K_BLUR_OPACITY,
            active_program: GlProgramType::Yuv420,
            allocated_textures: 0,
        }
    }
}

impl CallsGroupViewportOpenglGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn initialize(&mut self, width: u32, height: u32) {
        self.is_initialized = true;
        self.viewport_width = width.max(1);
        self.viewport_height = height.max(1);
        self.allocated_textures = 5; // 5 textures per TileData
    }

    pub fn deinitialize(&mut self) {
        self.is_initialized = false;
        self.allocated_textures = 0;
    }

    pub fn set_program(&mut self, program: GlProgramType) {
        self.active_program = program;
    }

    pub fn compute_blur_weights(&self) -> Vec<f32> {
        let mut weights = Vec::with_capacity(K_BLUR_DIAMETER);
        let mut sum = 0.0f32;
        for i in 0..K_BLUR_DIAMETER {
            let w = blur_box_weight(i as i32, K_BLUR_RADIUS);
            weights.push(w);
            sum += w;
        }
        if sum > 0.0 {
            for w in &mut weights {
                *w /= sum;
            }
        }
        weights
    }

    pub fn generate_noise_strip(&self, len: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            let u = (i % (K_NOISE_TEXTURE_SIZE as usize)) as f32 / K_NOISE_TEXTURE_SIZE as f32;
            let v = (i / (K_NOISE_TEXTURE_SIZE as usize)) as f32 / K_NOISE_TEXTURE_SIZE as f32;
            out.push(pseudo_noise_sample(u, v));
        }
        out
    }
}
