//! Calls Audio & Camera Device Selection Menu UI
//! Upstream: Telegram/SourceFiles/calls/ui/calls_device_menu.h:L25-L35, calls_device_menu.cpp:L24-L250
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceType {
    Playback,
    Capture,
    Camera,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub is_inactive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsDeviceMenuUi {
    pub device_type: DeviceType,
    pub devices: Vec<DeviceInfo>,
    pub selected_device_id: String,
    pub is_open: bool,
}

impl CallsDeviceMenuUi {
    pub fn new(device_type: DeviceType) -> Self {
        Self {
            device_type,
            devices: Vec::new(),
            selected_device_id: "default".to_string(),
            is_open: false,
        }
    }

    pub fn set_devices(&mut self, list: Vec<DeviceInfo>) {
        self.devices = list;
        if !self.devices.iter().any(|d| d.id == self.selected_device_id) {
            self.selected_device_id = "default".to_string();
        }
    }

    pub fn select_device(&mut self, device_id: impl Into<String>) {
        self.selected_device_id = device_id.into();
    }

    pub fn active_devices(&self) -> Vec<&DeviceInfo> {
        self.devices.iter().filter(|d| !d.is_inactive).collect()
    }

    pub fn toggle_menu(&mut self) -> bool {
        self.is_open = !self.is_open;
        self.is_open
    }
}
