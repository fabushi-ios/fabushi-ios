//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupRtmpGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_rtmp.h (L1-L75)
//!   - Telegram/SourceFiles/calls/group/calls_group_rtmp.cpp (L1-L359)

use super::calls_group_common::RtmpInfo;
use serde::{Deserialize, Serialize};

/// Recommended OBS/external software video encoding settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtmpEncodingSettings {
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
    pub keyframe_interval_seconds: u32,
    pub audio_sample_rate_hz: u32,
    pub audio_bitrate_kbps: u32,
}

impl Default for RtmpEncodingSettings {
    fn default() -> Self {
        Self {
            max_width: 1920,
            max_height: 1080,
            max_fps: 60,
            keyframe_interval_seconds: 2,
            audio_sample_rate_hz: 48000,
            audio_bitrate_kbps: 192,
        }
    }
}

/// Parity component for `CallsGroupRtmpGroup`.
#[derive(Debug, Clone)]
pub struct CallsGroupRtmpGroup {
    pub channel_id: u64,
    pub dc_id: u32,
    pub stream_hash: String,
    pub encoding_settings: RtmpEncodingSettings,
    pub is_streaming_active: bool,
}

impl Default for CallsGroupRtmpGroup {
    fn default() -> Self {
        Self {
            channel_id: 0,
            dc_id: 2,
            stream_hash: "default_stream_hash".to_string(),
            encoding_settings: RtmpEncodingSettings::default(),
            is_streaming_active: false,
        }
    }
}

impl CallsGroupRtmpGroup {
    pub fn new(channel_id: u64, dc_id: u32, stream_hash: &str) -> Self {
        Self {
            channel_id,
            dc_id,
            stream_hash: stream_hash.to_string(),
            encoding_settings: RtmpEncodingSettings::default(),
            is_streaming_active: false,
        }
    }

    /// Server URL for RTMP ingest.
    /// Upstream: `calls_group_rtmp.cpp:L90-L105`
    pub fn server_url(&self) -> String {
        format!("rtmps://dc{}-rtmp.telegram.org:443/live/", self.dc_id)
    }

    /// Full stream key formatted as `{channel_id}_{stream_hash}`.
    pub fn stream_key(&self) -> String {
        format!("{}_{}", self.channel_id, self.stream_hash)
    }

    /// Masked stream key for UI display (e.g. `12345_••••••••`).
    pub fn masked_stream_key(&self) -> String {
        let prefix = format!("{}_", self.channel_id);
        let bullets = "•".repeat(self.stream_hash.len().min(8));
        format!("{}{}", prefix, bullets)
    }

    pub fn to_rtmp_info(&self) -> RtmpInfo {
        RtmpInfo {
            url: self.server_url(),
            key: self.stream_key(),
        }
    }

    pub fn reset_stream_key(&mut self, new_hash: &str) {
        self.stream_hash = new_hash.to_string();
    }
}
