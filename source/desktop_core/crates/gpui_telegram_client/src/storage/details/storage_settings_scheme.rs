//! Target Symbol: StorageSettingsSchemeDetails
//! Telegram Desktop 1:1 Submodule: `StorageSettingsSchemeDetails`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::collections::HashMap;
use super::storage_file_utilities::FileKey;

// Telegram Desktop Settings Scheme Block IDs
pub const DBI_KEY: u32 = 0x00;
pub const DBI_USER: u32 = 0x01;
pub const DBI_DC_OPTION_OLD_OLD: u32 = 0x02;
pub const DBI_CHAT_SIZE_MAX_OLD: u32 = 0x03;
pub const DBI_MUTE_PEER_OLD: u32 = 0x04;
pub const DBI_SEND_KEY_OLD: u32 = 0x05;
pub const DBI_AUTO_START: u32 = 0x06;
pub const DBI_START_MINIMIZED: u32 = 0x07;
pub const DBI_SOUND_FLASH_BOUNCE_NOTIFY_OLD: u32 = 0x08;
pub const DBI_WORK_MODE_OLD: u32 = 0x09;
pub const DBI_SEEN_TRAY_TOOLTIP: u32 = 0x0a;
pub const DBI_DESKTOP_NOTIFY_OLD: u32 = 0x0b;
pub const DBI_AUTO_UPDATE: u32 = 0x0c;
pub const DBI_LAST_UPDATE_CHECK: u32 = 0x0d;
pub const DBI_WINDOW_POSITION_OLD: u32 = 0x0e;
pub const DBI_CONNECTION_TYPE_OLD_OLD: u32 = 0x0f;
pub const DBI_DEFAULT_ATTACH: u32 = 0x11;
pub const DBI_CATS_AND_DOGS_OLD: u32 = 0x12;
pub const DBI_REPLACE_EMOJI_OLD: u32 = 0x13;
pub const DBI_ASK_DOWNLOAD_PATH_OLD: u32 = 0x14;
pub const DBI_DOWNLOAD_PATH_OLD_OLD: u32 = 0x15;
pub const DBI_SCALE_OLD: u32 = 0x16;
pub const DBI_EMOJI_TAB_OLD: u32 = 0x17;
pub const DBI_RECENT_EMOJI_OLD_OLD_OLD: u32 = 0x18;
pub const DBI_LOGGED_PHONE_NUMBER_OLD: u32 = 0x19;
pub const DBI_MUTED_PEERS_OLD: u32 = 0x1a;
pub const DBI_NOTIFY_VIEW_OLD: u32 = 0x1c;
pub const DBI_SEND_TO_MENU: u32 = 0x1d;
pub const DBI_COMPRESS_PASTED_IMAGE_OLD: u32 = 0x1e;
pub const DBI_LANG_OLD: u32 = 0x1f;
pub const DBI_LANG_FILE_OLD: u32 = 0x20;
pub const DBI_TILE_BACKGROUND_OLD: u32 = 0x21;
pub const DBI_AUTO_LOCK_OLD: u32 = 0x22;
pub const DBI_DIALOG_LAST_PATH: u32 = 0x23;
pub const DBI_RECENT_EMOJI_OLD_OLD: u32 = 0x24;
pub const DBI_EMOJI_VARIANTS_OLD_OLD: u32 = 0x25;
pub const DBI_RECENT_STICKERS: u32 = 0x26;
pub const DBI_DC_OPTION_OLD: u32 = 0x27;
pub const DBI_TRY_IPV6_OLD: u32 = 0x28;
pub const DBI_SONG_VOLUME_OLD: u32 = 0x29;
pub const DBI_WINDOWS_NOTIFICATIONS_OLD: u32 = 0x30;
pub const DBI_INCLUDE_MUTED_OLD: u32 = 0x31;
pub const DBI_MEGAGROUP_SIZE_MAX_OLD: u32 = 0x32;
pub const DBI_DOWNLOAD_PATH_OLD: u32 = 0x33;
pub const DBI_AUTO_DOWNLOAD_OLD: u32 = 0x34;
pub const DBI_SAVED_GIFS_LIMIT_OLD: u32 = 0x35;
pub const DBI_SHOWING_SAVED_GIFS_OLD: u32 = 0x36;
pub const DBI_AUTO_PLAY_OLD: u32 = 0x37;
pub const DBI_ADAPTIVE_FOR_WIDE_OLD: u32 = 0x38;
pub const DBI_HIDDEN_PINNED_MESSAGES_OLD: u32 = 0x39;
pub const DBI_RECENT_EMOJI_OLD: u32 = 0x3a;
pub const DBI_EMOJI_VARIANTS_OLD: u32 = 0x3b;
pub const DBI_DIALOGS_MODE_OLD: u32 = 0x40;
pub const DBI_MODERATE_MODE_OLD: u32 = 0x41;
pub const DBI_VIDEO_VOLUME_OLD: u32 = 0x42;
pub const DBI_STICKERS_RECENT_LIMIT_OLD: u32 = 0x43;
pub const DBI_NATIVE_NOTIFICATIONS_OLD: u32 = 0x44;
pub const DBI_NOTIFICATIONS_COUNT_OLD: u32 = 0x45;
pub const DBI_NOTIFICATIONS_CORNER_OLD: u32 = 0x46;
pub const DBI_THEME_KEY_OLD: u32 = 0x47;
pub const DBI_DIALOGS_WIDTH_RATIO_OLD: u32 = 0x48;
pub const DBI_USE_EXTERNAL_VIDEO_PLAYER_OLD: u32 = 0x49;
pub const DBI_DC_OPTIONS_OLD: u32 = 0x4a;
pub const DBI_MTP_AUTHORIZATION: u32 = 0x4b;
pub const DBI_LAST_SEEN_WARNING_SEEN_OLD: u32 = 0x4c;
pub const DBI_SESSION_SETTINGS: u32 = 0x4d;
pub const DBI_LANG_PACK_KEY: u32 = 0x4e;
pub const DBI_CONNECTION_TYPE_OLD: u32 = 0x4f;
pub const DBI_STICKERS_FAVED_LIMIT_OLD: u32 = 0x50;
pub const DBI_SUGGEST_STICKERS_BY_EMOJI_OLD: u32 = 0x51;
pub const DBI_SUGGEST_EMOJI_OLD: u32 = 0x52;
pub const DBI_TXT_DOMAIN_STRING_OLD_OLD: u32 = 0x53;
pub const DBI_THEME_KEY: u32 = 0x54;
pub const DBI_TILE_BACKGROUND: u32 = 0x55;
pub const DBI_CACHE_SETTINGS_OLD: u32 = 0x56;
pub const DBI_POWER_SAVING: u32 = 0x57;
pub const DBI_SCALE_PERCENT: u32 = 0x58;
pub const DBI_PLAYBACK_SPEED_OLD: u32 = 0x59;
pub const DBI_LANGUAGES_KEY: u32 = 0x5a;
pub const DBI_CALL_SETTINGS_OLD: u32 = 0x5b;
pub const DBI_CACHE_SETTINGS: u32 = 0x5c;
pub const DBI_TXT_DOMAIN_STRING_OLD: u32 = 0x5d;
pub const DBI_APPLICATION_SETTINGS: u32 = 0x5e;
pub const DBI_DIALOGS_FILTERS_OLD: u32 = 0x5f;
pub const DBI_FALLBACK_PRODUCTION_CONFIG: u32 = 0x60;
pub const DBI_BACKGROUND_KEY: u32 = 0x61;

pub const DBI_ENCRYPTED_WITH_SALT: u32 = 333;
pub const DBI_ENCRYPTED: u32 = 444;
pub const DBI_VERSION: u32 = 666;

#[derive(Debug, Clone, Default)]
pub struct ReadSettingsContext {
    pub legacy_has_custom_day_background: bool,
    pub cache_total_size_limit: i64,
    pub cache_total_time_limit: i32,
    pub cache_big_file_total_size_limit: i64,
    pub cache_big_file_total_time_limit: i32,
    pub theme_key_legacy: FileKey,
    pub theme_key_day: FileKey,
    pub theme_key_night: FileKey,
    pub background_key_day: FileKey,
    pub background_key_night: FileKey,
    pub background_keys_read: bool,
    pub tile_day: bool,
    pub tile_night: bool,
    pub tile_read: bool,
    pub lang_pack_key: FileKey,
    pub languages_key: FileKey,
    pub mtp_authorization: Vec<u8>,
    pub mtp_legacy_main_dc_id: i32,
    pub mtp_legacy_user_id: i32,
    pub legacy_read: bool,
    pub application_settings_raw: Vec<u8>,
    pub session_settings_raw: Vec<u8>,
    pub blocks: HashMap<u32, Vec<u8>>,
}

impl ReadSettingsContext {
    pub fn new() -> Self {
        Self {
            tile_night: true,
            ..Default::default()
        }
    }

    pub fn read_setting(&mut self, block_id: u32, payload: &[u8], _version: i32) -> bool {
        self.blocks.insert(block_id, payload.to_vec());
        match block_id {
            DBI_MTP_AUTHORIZATION => {
                self.mtp_authorization = payload.to_vec();
                true
            }
            DBI_LANG_PACK_KEY => {
                if payload.len() >= 8 {
                    self.lang_pack_key = u64::from_le_bytes(payload[..8].try_into().unwrap());
                }
                true
            }
            DBI_LANGUAGES_KEY => {
                if payload.len() >= 8 {
                    self.languages_key = u64::from_le_bytes(payload[..8].try_into().unwrap());
                }
                true
            }
            DBI_THEME_KEY => {
                if payload.len() >= 16 {
                    self.theme_key_day = u64::from_le_bytes(payload[..8].try_into().unwrap());
                    self.theme_key_night = u64::from_le_bytes(payload[8..16].try_into().unwrap());
                } else if payload.len() >= 8 {
                    self.theme_key_day = u64::from_le_bytes(payload[..8].try_into().unwrap());
                }
                true
            }
            DBI_BACKGROUND_KEY => {
                if payload.len() >= 16 {
                    self.background_key_day = u64::from_le_bytes(payload[..8].try_into().unwrap());
                    self.background_key_night = u64::from_le_bytes(payload[8..16].try_into().unwrap());
                    self.background_keys_read = true;
                }
                true
            }
            DBI_TILE_BACKGROUND => {
                if payload.len() >= 2 {
                    self.tile_day = payload[0] != 0;
                    self.tile_night = payload[1] != 0;
                    self.tile_read = true;
                }
                true
            }
            DBI_CACHE_SETTINGS => {
                if payload.len() >= 24 {
                    self.cache_total_size_limit = i64::from_le_bytes(payload[..8].try_into().unwrap());
                    self.cache_total_time_limit = i32::from_le_bytes(payload[8..12].try_into().unwrap());
                    self.cache_big_file_total_size_limit = i64::from_le_bytes(payload[12..20].try_into().unwrap());
                    self.cache_big_file_total_time_limit = i32::from_le_bytes(payload[20..24].try_into().unwrap());
                }
                true
            }
            DBI_APPLICATION_SETTINGS => {
                self.application_settings_raw = payload.to_vec();
                true
            }
            DBI_SESSION_SETTINGS => {
                self.session_settings_raw = payload.to_vec();
                true
            }
            _ => true,
        }
    }
}

pub struct SettingsSchemeWriter {
    buffer: Vec<u8>,
}

impl Default for SettingsSchemeWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsSchemeWriter {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn write_block(&mut self, block_id: u32, data: &[u8]) {
        self.buffer.extend_from_slice(&block_id.to_le_bytes());
        self.buffer.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.buffer.extend_from_slice(data);
    }

    pub fn finish(self) -> Vec<u8> {
        self.buffer
    }
}

pub struct SettingsSchemeReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> SettingsSchemeReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub fn read_next_block(&mut self) -> Option<(u32, &'a [u8])> {
        if self.offset + 8 > self.data.len() {
            return None;
        }
        let block_id = u32::from_le_bytes(self.data[self.offset..self.offset + 4].try_into().unwrap());
        let len = u32::from_le_bytes(self.data[self.offset + 4..self.offset + 8].try_into().unwrap()) as usize;
        self.offset += 8;
        if self.offset + len > self.data.len() {
            return None;
        }
        let payload = &self.data[self.offset..self.offset + len];
        self.offset += len;
        Some((block_id, payload))
    }
}

#[derive(Debug, Clone, Default)]
pub struct StorageSettingsSchemeDetails {
    pub last_parsed_blocks: usize,
}

impl StorageSettingsSchemeDetails {
    pub fn new() -> Self {
        Self { last_parsed_blocks: 0 }
    }

    pub fn parse_settings(&mut self, data: &[u8], version: i32) -> ReadSettingsContext {
        let mut context = ReadSettingsContext::new();
        let mut reader = SettingsSchemeReader::new(data);
        let mut count = 0;
        while let Some((block_id, payload)) = reader.read_next_block() {
            context.read_setting(block_id, payload, version);
            count += 1;
        }
        self.last_parsed_blocks = count;
        context
    }
}
