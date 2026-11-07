//! Target Symbol: DataNotifySettingsNotify
//! Telegram Desktop 1:1 Submodule: `data/notify/data_notify_settings`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryNotificationSettings {
    pub sound_enabled: bool,
    pub preview_enabled: bool,
    pub flash_chat: bool,
    pub ringtone_document_id: Option<u64>,
}

impl Default for CategoryNotificationSettings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            preview_enabled: true,
            flash_chat: true,
            ringtone_document_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataNotifySettingsNotify {
    pub private_chats: CategoryNotificationSettings,
    pub groups: CategoryNotificationSettings,
    pub channels: CategoryNotificationSettings,
    pub desktop_notifications_enabled: bool,
    pub native_notifications: bool,
}

impl Default for DataNotifySettingsNotify {
    fn default() -> Self {
        Self {
            private_chats: CategoryNotificationSettings::default(),
            groups: CategoryNotificationSettings::default(),
            channels: CategoryNotificationSettings {
                sound_enabled: false,
                preview_enabled: true,
                flash_chat: false,
                ringtone_document_id: None,
            },
            desktop_notifications_enabled: true,
            native_notifications: true,
        }
    }
}

impl DataNotifySettingsNotify {
    pub fn new() -> Self {
        Self::default()
    }
}
