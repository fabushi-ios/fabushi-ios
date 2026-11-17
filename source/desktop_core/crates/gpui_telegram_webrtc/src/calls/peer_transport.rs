//! Real WebRTC RTCPeerConnection Transport & Dual Live/MockLoopback Modes
//!
//! Provides WebRtcPeerTransport wrapping RTCPeerConnection for live P2P media
//! streaming with fallback to an in-memory MockLoopback transport for deterministic CI testing.

use crate::ice::TurnToken;
use crate::media::ai_media_tap::AiMediaTap;
use crate::media::opus_codec::{
    CodecError, OpusDepacketizer, OpusPacketizer, RtpPacket, OPUS_PAYLOAD_TYPE, VP8_PAYLOAD_TYPE,
};
use crate::signaling::SignalingFrame;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_OPUS, MIME_TYPE_VP8};
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp::codecs::vp8::Vp8Packet;
use webrtc::rtp::packetizer::Depacketizer as _;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};

const LOOPBACK_CHANNEL_CAPACITY: usize = 1024;
const LIVE_FRAME_MAGIC: &[u8; 4] = b"GVF1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportMode {
    Live,
    MockLoopback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerConnectionState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerTransportConfig {
    pub mode: TransportMode,
    pub ice_servers: Vec<TurnToken>,
    pub enable_audio: bool,
    pub enable_video: bool,
    pub audio_sample_rate: u32,
    pub audio_channels: u16,
    pub video_width: u32,
    pub video_height: u32,
    pub video_fps: u32,
}

impl Default for PeerTransportConfig {
    fn default() -> Self {
        Self {
            mode: TransportMode::MockLoopback,
            ice_servers: Vec::new(),
            enable_audio: true,
            enable_video: true,
            audio_sample_rate: 48_000,
            audio_channels: 1,
            video_width: 640,
            video_height: 360,
            video_fps: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoFrameData {
    pub width: u32,
    pub height: u32,
    pub timestamp_ms: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum WebRtcTransportError {
    #[error("WebRTC internal error: {0}")]
    Internal(String),
    #[error("Invalid transport state: {0}")]
    InvalidState(String),
    #[error("Codec error: {0}")]
    Codec(#[from] CodecError),
    #[error("Channel closed")]
    ChannelClosed,
}

pub type AudioFrameCallback = Arc<dyn Fn(&[i16], u32, u16) + Send + Sync>;
pub type VideoFrameCallback = Arc<dyn Fn(&VideoFrameData) + Send + Sync>;
pub type IceCandidateCallback = Arc<dyn Fn(SignalingFrame) + Send + Sync>;
pub type StateChangeCallback = Arc<dyn Fn(PeerConnectionState) + Send + Sync>;

fn encode_live_video_payload(frame: &VideoFrameData) -> Vec<u8> {
    let mut out = Vec::with_capacity(24 + frame.data.len());
    out.extend_from_slice(LIVE_FRAME_MAGIC);
    out.extend_from_slice(&frame.width.to_le_bytes());
    out.extend_from_slice(&frame.height.to_le_bytes());
    out.extend_from_slice(&frame.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&(frame.data.len() as u32).to_le_bytes());
    out.extend_from_slice(&frame.data);
    out
}

fn decode_live_video_payload(bytes: &[u8]) -> Option<VideoFrameData> {
    if bytes.len() < 24 || &bytes[0..4] != LIVE_FRAME_MAGIC {
        return None;
    }
    let width = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let height = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let timestamp_ms = u64::from_le_bytes([
        bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17], bytes[18], bytes[19],
    ]);
    let data_len = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]) as usize;
    if bytes.len() < 24 + data_len {
        return None;
    }
    Some(VideoFrameData {
        width,
        height,
        timestamp_ms,
        data: bytes[24..24 + data_len].to_vec(),
    })
}

pub struct WebRtcPeerTransport {
    config: PeerTransportConfig,
    mode: TransportMode,
    state: Arc<parking_lot::RwLock<PeerConnectionState>>,

    // Media & Codec (decoupled interior locks so TX and RX never deadlock)
    packetizer: Arc<parking_lot::Mutex<OpusPacketizer>>,
    depacketizer: Arc<parking_lot::Mutex<OpusDepacketizer>>,
    ai_media_tap: Arc<parking_lot::RwLock<Option<Arc<dyn AiMediaTap>>>>,

    // Callbacks (shared with live RTCPeerConnection async handlers)
    on_audio: Arc<parking_lot::RwLock<Option<AudioFrameCallback>>>,
    on_video: Arc<parking_lot::RwLock<Option<VideoFrameCallback>>>,
    on_ice_candidate: Arc<parking_lot::RwLock<Option<IceCandidateCallback>>>,
    on_state_change: Arc<parking_lot::RwLock<Option<StateChangeCallback>>>,

    // MockLoopback decoupled senders & receivers
    loopback_audio_tx: Arc<parking_lot::RwLock<Option<mpsc::Sender<RtpPacket>>>>,
    loopback_video_tx: Arc<parking_lot::RwLock<Option<mpsc::Sender<VideoFrameData>>>>,
    loopback_audio_rx: Arc<Mutex<Option<mpsc::Receiver<RtpPacket>>>>,
    loopback_video_rx: Arc<Mutex<Option<mpsc::Receiver<VideoFrameData>>>>,
    loopback_rx_handle: Arc<parking_lot::Mutex<Option<tokio::task::AbortHandle>>>,

    // Live WebRTC instance and local media tracks
    peer_connection: Option<Arc<RTCPeerConnection>>,
    live_audio_track: Option<Arc<TrackLocalStaticRTP>>,
    live_video_track: Option<Arc<TrackLocalStaticSample>>,
}

impl WebRtcPeerTransport {
    pub async fn new(config: PeerTransportConfig) -> Result<Self, WebRtcTransportError> {
        let channels = config.audio_channels;
        let mode = config.mode;
        let mut transport = Self {
            config: config.clone(),
            mode,
            state: Arc::new(parking_lot::RwLock::new(PeerConnectionState::New)),
            packetizer: Arc::new(parking_lot::Mutex::new(OpusPacketizer::new(
                channels, 0x12345678,
            ))),
            depacketizer: Arc::new(parking_lot::Mutex::new(OpusDepacketizer::new(channels))),
            ai_media_tap: Arc::new(parking_lot::RwLock::new(None)),
            on_audio: Arc::new(parking_lot::RwLock::new(None)),
            on_video: Arc::new(parking_lot::RwLock::new(None)),
            on_ice_candidate: Arc::new(parking_lot::RwLock::new(None)),
            on_state_change: Arc::new(parking_lot::RwLock::new(None)),
            loopback_audio_tx: Arc::new(parking_lot::RwLock::new(None)),
            loopback_video_tx: Arc::new(parking_lot::RwLock::new(None)),
            loopback_audio_rx: Arc::new(Mutex::new(None)),
            loopback_video_rx: Arc::new(Mutex::new(None)),
            loopback_rx_handle: Arc::new(parking_lot::Mutex::new(None)),
            peer_connection: None,
            live_audio_track: None,
            live_video_track: None,
        };

        if mode == TransportMode::Live {
            transport.init_live_webrtc().await?;
        }

        Ok(transport)
    }

    async fn init_live_webrtc(&mut self) -> Result<(), WebRtcTransportError> {
        static INIT_RUSTLS: std::sync::Once = std::sync::Once::new();
        INIT_RUSTLS.call_once(|| {
            if rustls::crypto::CryptoProvider::get_default().is_none() {
                let _ = rustls::crypto::ring::default_provider().install_default();
            }
        });

        let mut media_engine = MediaEngine::default();

        // Register Opus Audio codec (PT 111, clock rate 48000, 2 channels)
        media_engine
            .register_codec(
                RTCRtpCodecParameters {
                    capability: RTCRtpCodecCapability {
                        mime_type: MIME_TYPE_OPUS.to_string(),
                        clock_rate: 48_000,
                        channels: 2,
                        sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                        rtcp_feedback: vec![],
                    },
                    payload_type: OPUS_PAYLOAD_TYPE,
                    ..Default::default()
                },
                RTPCodecType::Audio,
            )
            .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;

        // Register VP8 Video codec (PT 96, clock rate 90000)
        media_engine
            .register_codec(
                RTCRtpCodecParameters {
                    capability: RTCRtpCodecCapability {
                        mime_type: MIME_TYPE_VP8.to_string(),
                        clock_rate: 90_000,
                        channels: 0,
                        sdp_fmtp_line: "".to_string(),
                        rtcp_feedback: vec![],
                    },
                    payload_type: VP8_PAYLOAD_TYPE,
                    ..Default::default()
                },
                RTPCodecType::Video,
            )
            .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;

        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)
            .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;

        let mut setting_engine = SettingEngine::default();
        setting_engine.set_receive_mtu(8192);

        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_setting_engine(setting_engine)
            .build();

        let ice_servers: Vec<RTCIceServer> = self
            .config
            .ice_servers
            .iter()
            .map(|t| RTCIceServer {
                urls: t.uris.clone(),
                username: t.username.clone(),
                credential: t.credential.clone(),
                ..Default::default()
            })
            .collect();

        let rtc_config = RTCConfiguration {
            ice_servers,
            ..Default::default()
        };

        let pc = Arc::new(
            api.new_peer_connection(rtc_config)
                .await
                .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?,
        );

        // Attach local Opus audio track (PT 111)
        if self.config.enable_audio {
            let audio_track = Arc::new(TrackLocalStaticRTP::new(
                RTCRtpCodecCapability {
                    mime_type: MIME_TYPE_OPUS.to_string(),
                    clock_rate: 48_000,
                    channels: 2,
                    sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                    rtcp_feedback: vec![],
                },
                "audio".to_string(),
                "gpui-telegram-stream".to_string(),
            ));
            pc.add_track(Arc::clone(&audio_track) as Arc<dyn TrackLocal + Send + Sync>)
                .await
                .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;
            self.live_audio_track = Some(audio_track);
        }

        // Attach local VP8 video track (PT 96)
        if self.config.enable_video {
            let video_track = Arc::new(TrackLocalStaticSample::new(
                RTCRtpCodecCapability {
                    mime_type: MIME_TYPE_VP8.to_string(),
                    clock_rate: 90_000,
                    channels: 0,
                    sdp_fmtp_line: "".to_string(),
                    rtcp_feedback: vec![],
                },
                "video".to_string(),
                "gpui-telegram-stream".to_string(),
            ));
            pc.add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
                .await
                .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;
            self.live_video_track = Some(video_track);
        }

        // Wire ICE candidate discovery callback
        let ice_cb_arc = Arc::clone(&self.on_ice_candidate);
        pc.on_ice_candidate(Box::new(move |candidate| {
            let ice_cb_arc = Arc::clone(&ice_cb_arc);
            Box::pin(async move {
                if let Some(cand) = candidate {
                    if let Ok(init) = cand.to_json() {
                        if let Some(cb) = ice_cb_arc.read().clone() {
                            cb(SignalingFrame::IceCandidate {
                                call_id: String::new(),
                                candidate: init.candidate,
                                sdp_mid: init.sdp_mid,
                                sdp_mline_index: init.sdp_mline_index,
                            });
                        }
                    }
                }
            })
        }));

        // Wire PeerConnection state change callback
        let state_arc = Arc::clone(&self.state);
        let state_cb_arc = Arc::clone(&self.on_state_change);
        pc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            let mapped = match s {
                RTCPeerConnectionState::New => PeerConnectionState::New,
                RTCPeerConnectionState::Connecting => PeerConnectionState::Connecting,
                RTCPeerConnectionState::Connected => PeerConnectionState::Connected,
                RTCPeerConnectionState::Disconnected => PeerConnectionState::Disconnected,
                RTCPeerConnectionState::Failed => PeerConnectionState::Failed,
                RTCPeerConnectionState::Closed => PeerConnectionState::Closed,
                RTCPeerConnectionState::Unspecified => PeerConnectionState::New,
            };
            *state_arc.write() = mapped;
            if let Some(cb) = state_cb_arc.read().clone() {
                cb(mapped);
            }
            Box::pin(async {})
        }));

        // Wire incoming remote RTP track reader
        let depacketizer = Arc::clone(&self.depacketizer);
        let ai_tap = Arc::clone(&self.ai_media_tap);
        let on_audio = Arc::clone(&self.on_audio);
        let on_video = Arc::clone(&self.on_video);
        let sample_rate = self.config.audio_sample_rate;
        let channels = self.config.audio_channels;
        pc.on_track(Box::new(move |track, _receiver, _transceiver| {
            let depacketizer = Arc::clone(&depacketizer);
            let ai_tap = Arc::clone(&ai_tap);
            let on_audio = Arc::clone(&on_audio);
            let on_video = Arc::clone(&on_video);
            Box::pin(async move {
                let kind = track.kind();
                tokio::spawn(async move {
                    if kind == RTPCodecType::Audio {
                        while let Ok((rtp_pkt, _attr)) = track.read_rtp().await {
                            let pkt = RtpPacket {
                                version: rtp_pkt.header.version,
                                padding: rtp_pkt.header.padding,
                                extension: rtp_pkt.header.extension,
                                csrc_count: rtp_pkt.header.csrc.len() as u8,
                                marker: rtp_pkt.header.marker,
                                payload_type: rtp_pkt.header.payload_type,
                                sequence_number: rtp_pkt.header.sequence_number,
                                timestamp: rtp_pkt.header.timestamp,
                                ssrc: rtp_pkt.header.ssrc,
                                payload: rtp_pkt.payload.to_vec(),
                            };
                            let decoded = {
                                let mut dep = depacketizer.lock();
                                dep.depacketize(&pkt).ok()
                            };
                            if let Some(samples) = decoded {
                                if let Some(tap) = ai_tap.read().clone() {
                                    tap.on_incoming_pcm_frame(&samples, sample_rate, channels);
                                }
                                if let Some(cb) = on_audio.read().clone() {
                                    cb(&samples, sample_rate, channels);
                                }
                            }
                        }
                    } else if kind == RTPCodecType::Video {
                        let mut vp8_dep = Vp8Packet::default();
                        let mut frame_buf: Vec<u8> = Vec::new();
                        while let Ok((rtp_pkt, _attr)) = track.read_rtp().await {
                            if let Ok(chunk) = vp8_dep.depacketize(&rtp_pkt.payload) {
                                if vp8_dep.s == 1 && !frame_buf.is_empty() {
                                    frame_buf.clear();
                                }
                                frame_buf.extend_from_slice(&chunk);
                                if rtp_pkt.header.marker {
                                    if let Some(frame) = decode_live_video_payload(&frame_buf) {
                                        if let Some(cb) = on_video.read().clone() {
                                            cb(&frame);
                                        }
                                    }
                                    frame_buf.clear();
                                }
                            }
                        }
                    }
                });
            })
        }));

        self.peer_connection = Some(pc);
        self.set_state(PeerConnectionState::New);

        Ok(())
    }

    /// Creates a connected pair of transports in MockLoopback mode for headless CI testing
    pub fn create_loopback_pair(config: PeerTransportConfig) -> (Self, Self) {
        let mut cfg = config;
        cfg.mode = TransportMode::MockLoopback;

        let (a_audio_tx, a_audio_rx) = mpsc::channel::<RtpPacket>(LOOPBACK_CHANNEL_CAPACITY);
        let (a_video_tx, a_video_rx) = mpsc::channel::<VideoFrameData>(LOOPBACK_CHANNEL_CAPACITY);

        let (b_audio_tx, b_audio_rx) = mpsc::channel::<RtpPacket>(LOOPBACK_CHANNEL_CAPACITY);
        let (b_video_tx, b_video_rx) = mpsc::channel::<VideoFrameData>(LOOPBACK_CHANNEL_CAPACITY);

        let transport_a = Self {
            config: cfg.clone(),
            mode: TransportMode::MockLoopback,
            state: Arc::new(parking_lot::RwLock::new(PeerConnectionState::Connected)),
            packetizer: Arc::new(parking_lot::Mutex::new(OpusPacketizer::new(
                cfg.audio_channels,
                0x11111111,
            ))),
            depacketizer: Arc::new(parking_lot::Mutex::new(OpusDepacketizer::new(
                cfg.audio_channels,
            ))),
            ai_media_tap: Arc::new(parking_lot::RwLock::new(None)),
            on_audio: Arc::new(parking_lot::RwLock::new(None)),
            on_video: Arc::new(parking_lot::RwLock::new(None)),
            on_ice_candidate: Arc::new(parking_lot::RwLock::new(None)),
            on_state_change: Arc::new(parking_lot::RwLock::new(None)),
            loopback_audio_tx: Arc::new(parking_lot::RwLock::new(Some(b_audio_tx))),
            loopback_video_tx: Arc::new(parking_lot::RwLock::new(Some(b_video_tx))),
            loopback_audio_rx: Arc::new(Mutex::new(Some(a_audio_rx))),
            loopback_video_rx: Arc::new(Mutex::new(Some(a_video_rx))),
            loopback_rx_handle: Arc::new(parking_lot::Mutex::new(None)),
            peer_connection: None,
            live_audio_track: None,
            live_video_track: None,
        };

        let transport_b = Self {
            config: cfg.clone(),
            mode: TransportMode::MockLoopback,
            state: Arc::new(parking_lot::RwLock::new(PeerConnectionState::Connected)),
            packetizer: Arc::new(parking_lot::Mutex::new(OpusPacketizer::new(
                cfg.audio_channels,
                0x22222222,
            ))),
            depacketizer: Arc::new(parking_lot::Mutex::new(OpusDepacketizer::new(
                cfg.audio_channels,
            ))),
            ai_media_tap: Arc::new(parking_lot::RwLock::new(None)),
            on_audio: Arc::new(parking_lot::RwLock::new(None)),
            on_video: Arc::new(parking_lot::RwLock::new(None)),
            on_ice_candidate: Arc::new(parking_lot::RwLock::new(None)),
            on_state_change: Arc::new(parking_lot::RwLock::new(None)),
            loopback_audio_tx: Arc::new(parking_lot::RwLock::new(Some(a_audio_tx))),
            loopback_video_tx: Arc::new(parking_lot::RwLock::new(Some(a_video_tx))),
            loopback_audio_rx: Arc::new(Mutex::new(Some(b_audio_rx))),
            loopback_video_rx: Arc::new(Mutex::new(Some(b_video_rx))),
            loopback_rx_handle: Arc::new(parking_lot::Mutex::new(None)),
            peer_connection: None,
            live_audio_track: None,
            live_video_track: None,
        };

        (transport_a, transport_b)
    }

    /// Takes ownership of the loopback receiver channels for independent async pump tasks
    pub async fn take_loopback_receivers(
        &self,
    ) -> (
        Option<mpsc::Receiver<RtpPacket>>,
        Option<mpsc::Receiver<VideoFrameData>>,
    ) {
        let audio_rx = self.loopback_audio_rx.lock().await.take();
        let video_rx = self.loopback_video_rx.lock().await.take();
        (audio_rx, video_rx)
    }

    /// Spawns a background loopback receiver task on this transport (safe with concurrent sends)
    pub async fn spawn_loopback_receiver(this: Arc<Self>) -> Option<tokio::task::JoinHandle<()>> {
        let (audio_rx_opt, video_rx_opt) = this.take_loopback_receivers().await;
        if audio_rx_opt.is_none() && video_rx_opt.is_none() {
            return None;
        }
        let depacketizer = Arc::clone(&this.depacketizer);
        let ai_tap = Arc::clone(&this.ai_media_tap);
        let on_audio = Arc::clone(&this.on_audio);
        let on_video = Arc::clone(&this.on_video);
        let sample_rate = this.config.audio_sample_rate;
        let channels = this.config.audio_channels;

        let handle = tokio::spawn(async move {
            let mut audio_rx = audio_rx_opt;
            let mut video_rx = video_rx_opt;
            loop {
                tokio::select! {
                    Some(rtp) = async {
                        match audio_rx.as_mut() {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        let samples = {
                            let mut dep = depacketizer.lock();
                            dep.depacketize(&rtp).ok()
                        };
                        if let Some(samples) = samples {
                            if let Some(tap) = ai_tap.read().clone() {
                                tap.on_incoming_pcm_frame(&samples, sample_rate, channels);
                            }
                            if let Some(cb) = on_audio.read().clone() {
                                cb(&samples, sample_rate, channels);
                            }
                        }
                    }
                    Some(frame) = async {
                        match video_rx.as_mut() {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        if let Some(cb) = on_video.read().clone() {
                            cb(&frame);
                        }
                    }
                    else => break,
                }
            }
        });
        *this.loopback_rx_handle.lock() = Some(handle.abort_handle());
        Some(handle)
    }

    /// Receives next audio frame from MockLoopback channel, depacketizes it, and dispatches callbacks.
    /// Uses decoupled interior locks so concurrent `send_audio_pcm` / `send_video_frame` / `recv_loopback_video` never deadlock.
    pub async fn recv_loopback_audio(&self) -> Result<Vec<i16>, WebRtcTransportError> {
        let packet = {
            let mut rx_guard = self.loopback_audio_rx.lock().await;
            let rx = rx_guard
                .as_mut()
                .ok_or(WebRtcTransportError::ChannelClosed)?;
            rx.recv().await
        };
        if let Some(packet) = packet {
            let samples = {
                let mut dep = self.depacketizer.lock();
                dep.depacketize(&packet)?
            };
            if let Some(tap) = self.ai_media_tap.read().clone() {
                tap.on_incoming_pcm_frame(
                    &samples,
                    self.config.audio_sample_rate,
                    self.config.audio_channels,
                );
            }
            if let Some(cb) = self.on_audio.read().clone() {
                cb(
                    &samples,
                    self.config.audio_sample_rate,
                    self.config.audio_channels,
                );
            }
            return Ok(samples);
        }
        Err(WebRtcTransportError::ChannelClosed)
    }

    /// Receives next video frame from MockLoopback channel and dispatches callback.
    /// Uses decoupled interior locks so concurrent `send_audio_pcm` / `send_video_frame` / `recv_loopback_audio` never deadlock.
    pub async fn recv_loopback_video(&self) -> Result<VideoFrameData, WebRtcTransportError> {
        let frame = {
            let mut rx_guard = self.loopback_video_rx.lock().await;
            let rx = rx_guard
                .as_mut()
                .ok_or(WebRtcTransportError::ChannelClosed)?;
            rx.recv().await
        };
        if let Some(frame) = frame {
            if let Some(cb) = self.on_video.read().clone() {
                cb(&frame);
            }
            return Ok(frame);
        }
        Err(WebRtcTransportError::ChannelClosed)
    }

    /// Spawns the loopback receiver task to dispatch incoming packets to registered callbacks
    pub fn start_loopback_receiver(
        this: Arc<Mutex<Self>>,
        mut audio_rx: mpsc::Receiver<RtpPacket>,
        mut video_rx: mpsc::Receiver<VideoFrameData>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(rtp) = audio_rx.recv() => {
                        let (depacketizer, ai_tap, on_audio, sample_rate, channels) = {
                            let guard = this.lock().await;
                            (
                                Arc::clone(&guard.depacketizer),
                                Arc::clone(&guard.ai_media_tap),
                                Arc::clone(&guard.on_audio),
                                guard.config.audio_sample_rate,
                                guard.config.audio_channels,
                            )
                        };
                        let samples = {
                            let mut dep = depacketizer.lock();
                            dep.depacketize(&rtp).ok()
                        };
                        if let Some(samples) = samples {
                            if let Some(tap) = ai_tap.read().clone() {
                                tap.on_incoming_pcm_frame(&samples, sample_rate, channels);
                            }
                            if let Some(cb) = on_audio.read().clone() {
                                cb(&samples, sample_rate, channels);
                            }
                        }
                    }
                    Some(frame) = video_rx.recv() => {
                        let on_video = {
                            let guard = this.lock().await;
                            Arc::clone(&guard.on_video)
                        };
                        if let Some(cb) = on_video.read().clone() {
                            cb(&frame);
                        }
                    }
                    else => break,
                }
            }
        })
    }

    pub fn set_ai_media_tap(&self, tap: Arc<dyn AiMediaTap>) {
        *self.ai_media_tap.write() = Some(tap);
    }

    pub fn set_audio_frame_callback(&self, cb: AudioFrameCallback) {
        *self.on_audio.write() = Some(cb);
    }

    pub fn set_video_frame_callback(&self, cb: VideoFrameCallback) {
        *self.on_video.write() = Some(cb);
    }

    pub fn set_ice_candidate_callback(&self, cb: IceCandidateCallback) {
        *self.on_ice_candidate.write() = Some(cb);
    }

    pub fn set_state_change_callback(&self, cb: StateChangeCallback) {
        *self.on_state_change.write() = Some(cb);
    }

    pub fn set_loopback_audio_tx(&self, tx: mpsc::Sender<RtpPacket>) {
        *self.loopback_audio_tx.write() = Some(tx);
    }

    pub fn set_loopback_video_tx(&self, tx: mpsc::Sender<VideoFrameData>) {
        *self.loopback_video_tx.write() = Some(tx);
    }

    fn set_state(&self, new_state: PeerConnectionState) {
        *self.state.write() = new_state;
        if let Some(cb) = self.on_state_change.read().clone() {
            cb(new_state);
        }
    }

    pub async fn create_offer(&mut self) -> Result<String, WebRtcTransportError> {
        if self.mode == TransportMode::Live {
            if let Some(ref pc) = self.peer_connection {
                let offer = pc
                    .create_offer(None)
                    .await
                    .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;
                pc.set_local_description(offer.clone())
                    .await
                    .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;
                self.set_state(PeerConnectionState::Connecting);
                return Ok(offer.sdp);
            }
        }

        // MockLoopback offer synthesis
        self.set_state(PeerConnectionState::Connecting);
        let sdp = "v=0\r\no=- 1000 2 IN IP4 127.0.0.1\r\ns=gpui-webrtc-mock\r\nt=0 0\r\n\
             m=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=rtpmap:111 opus/48000/2\r\n\
             m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 VP8/90000\r\n"
            .to_string();
        Ok(sdp)
    }

    pub async fn create_answer(&mut self) -> Result<String, WebRtcTransportError> {
        if self.mode == TransportMode::Live {
            if let Some(ref pc) = self.peer_connection {
                let answer = pc
                    .create_answer(None)
                    .await
                    .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;
                pc.set_local_description(answer.clone())
                    .await
                    .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;
                self.set_state(PeerConnectionState::Connecting);
                return Ok(answer.sdp);
            }
        }

        // MockLoopback answer synthesis
        self.set_state(PeerConnectionState::Connected);
        let sdp = "v=0\r\no=- 1000 2 IN IP4 127.0.0.1\r\ns=gpui-webrtc-mock\r\nt=0 0\r\n\
             m=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=rtpmap:111 opus/48000/2\r\n\
             m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 VP8/90000\r\n"
            .to_string();
        Ok(sdp)
    }

    pub async fn set_remote_description(
        &mut self,
        sdp: &str,
        is_offer: bool,
    ) -> Result<(), WebRtcTransportError> {
        if self.mode == TransportMode::Live {
            if let Some(ref pc) = self.peer_connection {
                let desc = if is_offer {
                    RTCSessionDescription::offer(sdp.to_string())
                        .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?
                } else {
                    RTCSessionDescription::answer(sdp.to_string())
                        .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?
                };
                pc.set_remote_description(desc)
                    .await
                    .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;
                return Ok(());
            }
        }

        // MockLoopback state transition
        self.set_state(PeerConnectionState::Connected);
        Ok(())
    }

    pub async fn add_ice_candidate(
        &self,
        candidate: &str,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    ) -> Result<(), WebRtcTransportError> {
        if self.mode == TransportMode::Live {
            if candidate.trim().is_empty() {
                return Ok(());
            }
            if let Some(ref pc) = self.peer_connection {
                let init = RTCIceCandidateInit {
                    candidate: candidate.to_string(),
                    sdp_mid,
                    sdp_mline_index,
                    username_fragment: None,
                };
                pc.add_ice_candidate(init)
                    .await
                    .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;
            }
        }
        Ok(())
    }

    /// Sends raw PCM audio samples (encodes with Opus and delivers to live peer or loopback)
    pub async fn send_audio_pcm(&self, samples: &[i16]) -> Result<(), WebRtcTransportError> {
        // AI Media Tap outgoing monitoring
        if let Some(tap) = self.ai_media_tap.read().clone() {
            tap.on_outgoing_pcm_frame(
                samples,
                self.config.audio_sample_rate,
                self.config.audio_channels,
            );
        }

        let packets = {
            let mut pack = self.packetizer.lock();
            pack.push_pcm(samples)
        };

        if self.mode == TransportMode::Live {
            if let Some(ref track) = self.live_audio_track {
                for packet in packets {
                    let rtp_pkt = webrtc::rtp::packet::Packet {
                        header: webrtc::rtp::header::Header {
                            version: packet.version,
                            padding: packet.padding,
                            extension: packet.extension,
                            marker: packet.marker,
                            payload_type: packet.payload_type,
                            sequence_number: packet.sequence_number,
                            timestamp: packet.timestamp,
                            ssrc: packet.ssrc,
                            ..Default::default()
                        },
                        payload: packet.payload.into(),
                    };
                    track
                        .write_rtp(&rtp_pkt)
                        .await
                        .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;
                }
            }
            return Ok(());
        }

        let tx_opt = self.loopback_audio_tx.read().clone();
        if let Some(tx) = tx_opt {
            for packet in packets {
                tx.send(packet)
                    .await
                    .map_err(|_| WebRtcTransportError::ChannelClosed)?;
            }
        }

        Ok(())
    }

    /// Sends a video frame (to live peer VP8 track or MockLoopback channel)
    pub async fn send_video_frame(
        &self,
        frame: &VideoFrameData,
    ) -> Result<(), WebRtcTransportError> {
        if self.mode == TransportMode::Live {
            if let Some(ref track) = self.live_video_track {
                let encoded = encode_live_video_payload(frame);
                let fps = self.config.video_fps.max(1) as u64;
                let sample = Sample {
                    data: encoded.into(),
                    duration: Duration::from_millis(1000 / fps),
                    ..Default::default()
                };
                track
                    .write_sample(&sample)
                    .await
                    .map_err(|e| WebRtcTransportError::Internal(e.to_string()))?;
            }
            return Ok(());
        }

        let tx_opt = self.loopback_video_tx.read().clone();
        if let Some(tx) = tx_opt {
            tx.send(frame.clone())
                .await
                .map_err(|_| WebRtcTransportError::ChannelClosed)?;
        }
        Ok(())
    }

    /// Delivers an incoming RTP packet directly into depacketizer and callbacks (for testing)
    pub fn deliver_incoming_rtp(&self, packet: &RtpPacket) -> Result<(), WebRtcTransportError> {
        let samples = {
            let mut dep = self.depacketizer.lock();
            dep.depacketize(packet)?
        };
        if let Some(tap) = self.ai_media_tap.read().clone() {
            tap.on_incoming_pcm_frame(
                &samples,
                self.config.audio_sample_rate,
                self.config.audio_channels,
            );
        }
        if let Some(cb) = self.on_audio.read().clone() {
            cb(
                &samples,
                self.config.audio_sample_rate,
                self.config.audio_channels,
            );
        }
        Ok(())
    }

    /// Delivers an incoming video frame directly into callbacks (for testing)
    pub fn deliver_incoming_video(&self, frame: &VideoFrameData) {
        if let Some(cb) = self.on_video.read().clone() {
            cb(frame);
        }
    }

    pub fn state(&self) -> PeerConnectionState {
        *self.state.read()
    }

    pub fn mode(&self) -> TransportMode {
        self.mode
    }

    pub fn config(&self) -> &PeerTransportConfig {
        &self.config
    }

    pub async fn close(&mut self) -> Result<(), WebRtcTransportError> {
        if let Some(pc) = self.peer_connection.take() {
            let _ = pc.close().await;
        }
        if let Some(handle) = self.loopback_rx_handle.lock().take() {
            handle.abort();
        }
        self.set_state(PeerConnectionState::Closed);
        Ok(())
    }
}
