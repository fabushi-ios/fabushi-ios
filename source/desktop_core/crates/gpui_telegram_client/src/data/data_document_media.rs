//! Target Symbol: DataDocumentMedia
//! Telegram Desktop 1:1 Submodule: `data/data_document_media`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaRenderState {
    Unloaded,
    ThumbnailLoaded { width: u32, height: u32 },
    FullMediaLoaded { byte_count: usize },
    PlaybackActive { position_ms: u64, duration_ms: u64 },
    RenderFailed { error: String },
}

impl Default for MediaRenderState {
    fn default() -> Self {
        Self::Unloaded
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataDocumentMedia {
    pub document_id: u64,
    pub render_state: MediaRenderState,
    pub thumbnail_cache: Option<Vec<u8>>,
    pub blurred_placeholder: Option<Vec<u8>>,
    pub is_spoiler_revealed: bool,
    pub auto_play_gif: bool,
    pub loop_playback: bool,
}

impl Default for DataDocumentMedia {
    fn default() -> Self {
        Self {
            document_id: 0,
            render_state: MediaRenderState::Unloaded,
            thumbnail_cache: None,
            blurred_placeholder: None,
            is_spoiler_revealed: false,
            auto_play_gif: true,
            loop_playback: true,
        }
    }
}

impl DataDocumentMedia {
    pub fn new(document_id: u64) -> Self {
        Self {
            document_id,
            ..Default::default()
        }
    }

    pub fn set_thumbnail(&mut self, bytes: Vec<u8>, width: u32, height: u32) {
        self.thumbnail_cache = Some(bytes);
        self.render_state = MediaRenderState::ThumbnailLoaded { width, height };
    }

    pub fn set_blurred_placeholder(&mut self, bytes: Vec<u8>) {
        self.blurred_placeholder = Some(bytes);
    }

    pub fn set_full_loaded(&mut self, byte_count: usize) {
        self.render_state = MediaRenderState::FullMediaLoaded { byte_count };
    }

    pub fn set_playback(&mut self, position_ms: u64, duration_ms: u64) {
        self.render_state = MediaRenderState::PlaybackActive {
            position_ms,
            duration_ms,
        };
    }

    pub fn set_failed(&mut self, error: impl Into<String>) {
        self.render_state = MediaRenderState::RenderFailed {
            error: error.into(),
        };
    }

    pub fn reveal_spoiler(&mut self) {
        self.is_spoiler_revealed = true;
    }

    pub fn is_loaded(&self) -> bool {
        matches!(
            self.render_state,
            MediaRenderState::FullMediaLoaded { .. } | MediaRenderState::PlaybackActive { .. }
        )
    }
}
