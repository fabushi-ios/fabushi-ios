//! WebRTC VOIP Call Controller Adapter
//! Upstream: Telegram/SourceFiles/calls/calls_controller_webrtc.h:L18-L60, calls_controller_webrtc.cpp:L17-L175

use super::calls_controller::{AudioConfig, CallVoipState, NetworkType, TrafficStats, VoipControllerBackend};
use super::calls_controller_tgvoip::{TgVoipEndpoint, TgVoipProxy};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallConnectionDescription {
    pub ip: String,
    pub ipv6: String,
    pub peer_tag: [u8; 16],
    pub connection_id: i64,
    pub port: u16,
}

impl From<&TgVoipEndpoint> for CallConnectionDescription {
    fn from(ep: &TgVoipEndpoint) -> Self {
        Self {
            ip: ep.ipv4.clone(),
            ipv6: ep.ipv6.clone(),
            peer_tag: ep.peer_tag,
            connection_id: ep.endpoint_id,
            port: ep.port,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebrtcSessionState {
    Initializing,
    Reconnecting,
    Connected,
    Failed,
}

impl WebrtcSessionState {
    pub fn to_voip_state(self) -> CallVoipState {
        match self {
            Self::Initializing => CallVoipState::WaitInit,
            Self::Reconnecting => CallVoipState::Reconnecting,
            Self::Connected => CallVoipState::Established,
            Self::Failed => CallVoipState::Failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsControllerWebrtc {
    pub primary_endpoint: Option<CallConnectionDescription>,
    pub alternative_endpoints: Vec<CallConnectionDescription>,
    pub proxy: Option<TgVoipProxy>,
    pub encryption_key: Vec<u8>,
    pub is_outgoing: bool,
    pub allow_p2p: bool,
    pub max_layer: i32,
    pub audio_config: AudioConfig,
    pub network_type: NetworkType,
    pub session_state: WebrtcSessionState,
    pub traffic_stats: TrafficStats,
    pub signaling_packets: Vec<Vec<u8>>,
    pub is_stopped: bool,
}

impl CallsControllerWebrtc {
    pub const VERSION: &'static str = "2.4.4";

    pub fn new(
        endpoints: &[TgVoipEndpoint],
        proxy: Option<TgVoipProxy>,
        encryption_key: Vec<u8>,
        is_outgoing: bool,
        allow_p2p: bool,
    ) -> Self {
        let primary = endpoints.first().map(CallConnectionDescription::from);
        let alternatives = endpoints.iter().skip(1).map(CallConnectionDescription::from).collect();

        Self {
            primary_endpoint: primary,
            alternative_endpoints: alternatives,
            proxy,
            encryption_key,
            is_outgoing,
            allow_p2p,
            max_layer: 92,
            audio_config: AudioConfig::default(),
            network_type: NetworkType::Wifi,
            session_state: WebrtcSessionState::Initializing,
            traffic_stats: TrafficStats::default(),
            signaling_packets: Vec::new(),
            is_stopped: false,
        }
    }

    pub fn version() -> String {
        Self::VERSION.to_string()
    }

    pub fn set_is_muted(&mut self, muted: bool) {
        self.audio_config.mute_microphone = muted;
    }

    pub fn receive_signaling_data(&mut self, data: &[u8]) -> bool {
        if self.is_stopped || data.is_empty() {
            return false;
        }
        self.signaling_packets.push(data.to_vec());
        self.traffic_stats.bytes_received = self.traffic_stats.bytes_received.saturating_add(data.len() as u64);
        true
    }

    pub fn set_session_state(&mut self, state: WebrtcSessionState) {
        self.session_state = state;
    }

    pub fn current_voip_state(&self) -> CallVoipState {
        self.session_state.to_voip_state()
    }

    pub fn get_debug_info(&self) -> String {
        format!(
            "WebRTC v{}, State: {:?}, P2P: {}, Packets: {}",
            Self::VERSION,
            self.session_state,
            self.allow_p2p,
            self.signaling_packets.len()
        )
    }

    pub fn stop(&mut self) {
        self.is_stopped = true;
        self.session_state = WebrtcSessionState::Failed;
    }
}

impl VoipControllerBackend for CallsControllerWebrtc {
    fn version(&self) -> String { Self::version() }
    fn set_network_type(&mut self, net_type: NetworkType) { self.network_type = net_type; }
    fn set_mute_microphone(&mut self, muted: bool) { self.set_is_muted(muted); }
    fn set_audio_input_device(&mut self, id: String) { self.audio_config.audio_input_device = id; }
    fn set_audio_output_device(&mut self, id: String) { self.audio_config.audio_output_device = id; }
    fn set_input_volume(&mut self, level: f32) { self.audio_config.input_volume = level.clamp(0.0, 2.0); }
    fn set_output_volume(&mut self, level: f32) { self.audio_config.output_volume = level.clamp(0.0, 2.0); }
    fn receive_signaling_data(&mut self, data: &[u8]) -> bool { self.receive_signaling_data(data) }
    fn get_traffic_stats(&self) -> TrafficStats { self.traffic_stats.clone() }
    fn stop(&mut self) { self.stop(); }
}
