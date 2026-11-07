//! Legacy TgVoip Controller Adapter
//! Upstream: Telegram/SourceFiles/calls/calls_controller_tgvoip.h:L14-L97
#![allow(dead_code)]

use super::calls_controller::{AudioConfig, CallVoipState, NetworkType, TrafficStats};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TgVoipEndpoint {
    pub endpoint_id: i64,
    pub ipv4: String,
    pub ipv6: String,
    pub port: u16,
    pub peer_tag: [u8; 16],
    pub is_tcp: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TgVoipProxy {
    pub host: String,
    pub port: u16,
    pub login: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsControllerTgvoip {
    pub endpoints: Vec<TgVoipEndpoint>,
    pub proxy: Option<TgVoipProxy>,
    pub audio_config: AudioConfig,
    pub network_type: NetworkType,
    pub state: CallVoipState,
    pub traffic_stats: TrafficStats,
    pub persistent_state: Vec<u8>,
    pub signal_bars: i32,
    pub stopped: bool,
}

impl CallsControllerTgvoip {
    pub const VERSION: &'static str = "legacy-tgvoip-2.4";

    pub fn new(endpoints: Vec<TgVoipEndpoint>, proxy: Option<TgVoipProxy>) -> Self {
        Self {
            endpoints,
            proxy,
            audio_config: AudioConfig::default(),
            network_type: NetworkType::Wifi,
            state: CallVoipState::WaitInit,
            traffic_stats: TrafficStats::default(),
            persistent_state: Vec::new(),
            signal_bars: 0,
            stopped: false,
        }
    }

    pub fn version() -> String {
        Self::VERSION.to_string()
    }

    pub fn set_mute_microphone(&mut self, mute: bool) {
        self.audio_config.mute_microphone = mute;
    }

    pub fn set_echo_cancellation_strength(&mut self, strength: i32) {
        self.audio_config.echo_cancellation_strength = strength;
    }

    pub fn set_audio_devices(&mut self, input_id: String, output_id: String) {
        self.audio_config.audio_input_device = input_id;
        self.audio_config.audio_output_device = output_id;
    }

    pub fn update_signal_bars(&mut self, bars: i32) {
        self.signal_bars = bars.clamp(0, 4);
    }

    pub fn stop(&mut self) -> TrafficStats {
        self.stopped = true;
        self.state = CallVoipState::Failed;
        self.traffic_stats.clone()
    }
}
