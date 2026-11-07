//! Telegram Desktop 1:1 Parity Submodule: `DesktopCaptureChooseSourceGroupUi`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/ui/desktop_capture_choose_source.h`
//! - `Telegram/SourceFiles/calls/group/ui/desktop_capture_choose_source.cpp:L31-L120`

use serde::{Deserialize, Serialize};

/// Upstream 3x2 screen capture picker grid dimensions
/// desktop_capture_choose_source.cpp:L31-L32
pub const K_COLUMNS: usize = 3;
pub const K_ROWS: usize = 2;
pub const K_PAGE_SIZE: usize = K_COLUMNS * K_ROWS; // 6 sources per page

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureSourceType {
    Screen,
    Window,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureSourceItem {
    pub device_id: String,
    pub title: String,
    pub source_type: CaptureSourceType,
    pub is_selected: bool,
}

/// Screen & Window Capture Source Chooser
/// Target Symbol: `DesktopCaptureChooseSourceGroupUi`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopCaptureChooseSourceGroupUi {
    pub sources: Vec<CaptureSourceItem>,
    pub selected_device_id: Option<String>,
    pub with_audio: bool,
    pub with_audio_supported: bool,
    pub current_page: usize,
}

impl Default for DesktopCaptureChooseSourceGroupUi {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            selected_device_id: None,
            with_audio: true,
            with_audio_supported: true,
            current_page: 0,
        }
    }
}

impl DesktopCaptureChooseSourceGroupUi {
    pub fn new(audio_supported: bool) -> Self {
        Self {
            sources: Vec::new(),
            selected_device_id: None,
            with_audio: audio_supported,
            with_audio_supported: audio_supported,
            current_page: 0,
        }
    }

    pub fn add_source(&mut self, id: &str, title: &str, source_type: CaptureSourceType) {
        let is_first = self.sources.is_empty();
        self.sources.push(CaptureSourceItem {
            device_id: id.to_string(),
            title: title.to_string(),
            source_type,
            is_selected: is_first,
        });
        if is_first {
            self.selected_device_id = Some(id.to_string());
        }
    }

    pub fn select_source(&mut self, device_id: &str) -> bool {
        let found = self.sources.iter().any(|s| s.device_id == device_id);
        if found {
            for s in &mut self.sources {
                s.is_selected = s.device_id == device_id;
            }
            self.selected_device_id = Some(device_id.to_string());
            true
        } else {
            false
        }
    }

    pub fn toggle_with_audio(&mut self) -> bool {
        if self.with_audio_supported {
            self.with_audio = !self.with_audio;
        }
        self.with_audio
    }

    pub fn visible_sources_for_page(&self, page: usize) -> &[CaptureSourceItem] {
        let start = page * K_PAGE_SIZE;
        if start >= self.sources.len() {
            &[]
        } else {
            let end = (start + K_PAGE_SIZE).min(self.sources.len());
            &self.sources[start..end]
        }
    }

    pub fn total_pages(&self) -> usize {
        if self.sources.is_empty() {
            1
        } else {
            (self.sources.len() + K_PAGE_SIZE - 1) / K_PAGE_SIZE
        }
    }
}
