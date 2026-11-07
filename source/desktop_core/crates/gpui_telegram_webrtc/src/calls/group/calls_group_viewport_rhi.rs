//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupViewportRhiGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/calls_group_viewport_rhi.h`
//! - `Telegram/SourceFiles/calls/group/calls_group_viewport_rhi.cpp:L31-L100`

use serde::{Deserialize, Serialize};

pub const K_NOISE_TEXTURE_SIZE: u32 = 256;
pub const K_BLUR_TEXTURE_SIZE_FACTOR: f32 = 4.0;
pub const K_BLUR_OPACITY: f32 = 0.65;

/// Modern QRhi GroupFrameUniforms block layout (128 bytes)
/// calls_group_viewport_rhi.cpp:L36-L50
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GroupFrameUniforms {
    pub viewport: [f32; 2],
    pub _pad0: f32,
    pub flip_y: f32,
    pub frame_bg: [f32; 4],
    pub shadow: [f32; 4],
    pub paused: f32,
    pub _pad1: [f32; 3],
    pub round_rect: [f32; 4],
    pub radius_outline: [f32; 2],
    pub _pad2: [f32; 2],
    pub round_bg: [f32; 4],
    pub outline_fg: [f32; 4],
}

impl Default for GroupFrameUniforms {
    fn default() -> Self {
        Self {
            viewport: [1280.0, 720.0],
            _pad0: 0.0,
            flip_y: 1.0,
            frame_bg: [0.094, 0.133, 0.176, 1.0],
            shadow: [0.0, 0.0, 0.0, 0.314],
            paused: 0.0,
            _pad1: [0.0; 3],
            round_rect: [0.0, 0.0, 1280.0, 720.0],
            radius_outline: [12.0, 2.0],
            _pad2: [0.0; 2],
            round_bg: [0.1, 0.1, 0.1, 1.0],
            outline_fg: [0.2, 0.6, 1.0, 1.0],
        }
    }
}

/// BlurUniforms block layout (16 bytes)
/// calls_group_viewport_rhi.cpp:L52-L56
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct BlurUniforms {
    pub texel_offset: f32,
    pub _pad: [f32; 3],
}

/// ImageUniforms block layout (16 bytes)
/// calls_group_viewport_rhi.cpp:L58-L63
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ImageUniforms {
    pub viewport: [f32; 2],
    pub g_opacity: f32,
    pub flip_y: f32,
}

/// Vulkan NDC Y-down flip compensation
/// calls_group_viewport_rhi.cpp:L71-L74: NdcFlipY
pub fn ndc_flip_y(is_vulkan: bool) -> f32 {
    if is_vulkan { -1.0 } else { 1.0 }
}

/// Offscreen rendering quad (16 floats)
/// calls_group_viewport_rhi.cpp:L77-L85: OffscreenQuad
pub fn offscreen_quad(is_vulkan: bool) -> [f32; 16] {
    let flip_y = ndc_flip_y(is_vulkan);
    [
        -1.0, -1.0 * flip_y, 0.0, 1.0,
         1.0, -1.0 * flip_y, 1.0, 1.0,
        -1.0,  1.0 * flip_y, 0.0, 0.0,
         1.0,  1.0 * flip_y, 1.0, 0.0,
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RhiBackend {
    Vulkan,
    Metal,
    DirectX12,
    OpenGL,
}

/// Modern QRhi Viewport Renderer
/// Target Symbol: `CallsGroupViewportRhiGroup`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallsGroupViewportRhiGroup {
    pub backend: RhiBackend,
    pub is_initialized: bool,
    pub frame_uniforms: GroupFrameUniforms,
    pub blur_uniforms: BlurUniforms,
    pub image_uniforms: ImageUniforms,
    pub offscreen_passes_executed: u32,
}

impl Default for CallsGroupViewportRhiGroup {
    fn default() -> Self {
        Self {
            backend: RhiBackend::Metal,
            is_initialized: false,
            frame_uniforms: GroupFrameUniforms::default(),
            blur_uniforms: BlurUniforms::default(),
            image_uniforms: ImageUniforms::default(),
            offscreen_passes_executed: 0,
        }
    }
}

impl CallsGroupViewportRhiGroup {
    pub fn new(backend: RhiBackend) -> Self {
        let flip_y = ndc_flip_y(matches!(backend, RhiBackend::Vulkan));
        let mut group = Self {
            backend,
            ..Self::default()
        };
        group.frame_uniforms.flip_y = flip_y;
        group.image_uniforms.flip_y = flip_y;
        group
    }

    pub fn initialize(&mut self, width: u32, height: u32) {
        self.is_initialized = true;
        self.frame_uniforms.viewport = [width as f32, height as f32];
        self.image_uniforms.viewport = [width as f32, height as f32];
    }

    pub fn execute_offscreen_passes(&mut self) -> [f32; 16] {
        self.offscreen_passes_executed = self.offscreen_passes_executed.saturating_add(1);
        offscreen_quad(matches!(self.backend, RhiBackend::Vulkan))
    }

    pub fn update_blur_offset(&mut self, offset: f32) {
        self.blur_uniforms.texel_offset = offset;
    }
}
