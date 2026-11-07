//! Calls Controller Interface & Audio/Network Configurations
//! Upstream: Telegram/SourceFiles/calls/calls_controller.h:L14-L58, calls_controller.cpp:L15-L53
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkType {
    Unknown,
    Wifi,
    Ethernet,
    CellularGprs,
    CellularEdge,
    Cellular3g,
    CellularHspa,
    CellularLte,
    OtherHighSpeed,
    OtherLowSpeed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallVoipState {
    WaitInit,
    WaitInitAck,
    Established,
    Failed,
    Reconnecting,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrafficStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    pub mute_microphone: bool,
    pub gain_control_enabled: bool,
    pub echo_cancellation_strength: i32,
    pub audio_input_device: String,
    pub audio_output_device: String,
    pub input_volume: f32,
    pub output_volume: f32,
    pub ducking_enabled: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mute_microphone: false,
            gain_control_enabled: true,
            echo_cancellation_strength: 3,
            audio_input_device: "default".to_string(),
            audio_output_device: "default".to_string(),
            input_volume: 1.0,
            output_volume: 1.0,
            ducking_enabled: true,
        }
    }
}

pub trait VoipControllerBackend: Send + Sync {
    fn version(&self) -> String;
    fn set_network_type(&mut self, net_type: NetworkType);
    fn set_mute_microphone(&mut self, muted: bool);
    fn set_audio_input_device(&mut self, id: String);
    fn set_audio_output_device(&mut self, id: String);
    fn set_input_volume(&mut self, level: f32);
    fn set_output_volume(&mut self, level: f32);
    fn receive_signaling_data(&mut self, data: &[u8]) -> bool;
    fn get_traffic_stats(&self) -> TrafficStats;
    fn stop(&mut self);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsController {
    pub active_version: String,
    pub network_type: NetworkType,
    pub audio_config: AudioConfig,
    pub stats: TrafficStats,
    pub state: CallVoipState,
    pub preferred_relay_id: i64,
    pub last_error: Option<String>,
}

impl CallsController {
    pub const WEBRTC_VERSION: &'static str = "2.4.4";
    pub const TGVOIP_VERSION: &'static str = "legacy-tgvoip";
    pub const MAX_LAYER: i32 = 92;

    pub fn new(version: impl Into<String>) -> Self {
        Self {
            active_version: version.into(),
            network_type: NetworkType::Wifi,
            audio_config: AudioConfig::default(),
            stats: TrafficStats::default(),
            state: CallVoipState::WaitInit,
            preferred_relay_id: 0,
            last_error: None,
        }
    }

    pub fn collect_controller_versions() -> Vec<String> {
        vec![
            Self::WEBRTC_VERSION.to_string(),
            Self::TGVOIP_VERSION.to_string(),
        ]
    }

    pub fn is_webrtc(&self) -> bool {
        self.active_version == Self::WEBRTC_VERSION
    }

    pub fn update_state(&mut self, new_state: CallVoipState) {
        self.state = new_state;
    }

    pub fn set_mute(&mut self, muted: bool) {
        self.audio_config.mute_microphone = muted;
    }

    pub fn record_traffic(&mut self, sent: u64, received: u64) {
        self.stats.bytes_sent = self.stats.bytes_sent.saturating_add(sent);
        self.stats.bytes_received = self.stats.bytes_received.saturating_add(received);
    }
}
