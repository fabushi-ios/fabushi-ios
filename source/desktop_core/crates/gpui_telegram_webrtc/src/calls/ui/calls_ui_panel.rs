//! Calls UI Panel Coordinator
//! Upstream: Telegram/SourceFiles/calls/ui/calls_device_menu.h:L1-L36

pub use super::calls_device_menu::{CallsDeviceMenuUi, DeviceInfo, DeviceType};
pub use crate::calls::calls_panel::{CallsPanel, PanelActionState};

/// Coordinates active call window controls together with audio/video device selection menus.
#[derive(Debug, Clone)]
pub struct CallsUiPanelState {
    pub panel: CallsPanel,
    pub playback_menu: CallsDeviceMenuUi,
    pub capture_menu: CallsDeviceMenuUi,
    pub camera_menu: CallsDeviceMenuUi,
}

impl CallsUiPanelState {
    pub fn new(call_id: i64, user_id: u64, is_incoming: bool) -> Self {
        Self {
            panel: CallsPanel::new(call_id, user_id, is_incoming),
            playback_menu: CallsDeviceMenuUi::new(DeviceType::Playback),
            capture_menu: CallsDeviceMenuUi::new(DeviceType::Capture),
            camera_menu: CallsDeviceMenuUi::new(DeviceType::Camera),
        }
    }

    pub fn close_all_menus(&mut self) {
        self.playback_menu.is_open = false;
        self.capture_menu.is_open = false;
        self.camera_menu.is_open = false;
    }

    pub fn toggle_device_menu(&mut self, device_type: DeviceType) {
        let target_open = match device_type {
            DeviceType::Playback => !self.playback_menu.is_open,
            DeviceType::Capture => !self.capture_menu.is_open,
            DeviceType::Camera => !self.camera_menu.is_open,
        };
        self.close_all_menus();
        match device_type {
            DeviceType::Playback => self.playback_menu.is_open = target_open,
            DeviceType::Capture => self.capture_menu.is_open = target_open,
            DeviceType::Camera => self.camera_menu.is_open = target_open,
        }
    }
}
