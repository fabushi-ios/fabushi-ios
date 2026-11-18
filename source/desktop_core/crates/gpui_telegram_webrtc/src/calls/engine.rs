use super::calls_call::{CallState, CallType, CallsCall};
use super::calls_controller::{CallVoipState, CallsController};
use super::calls_controller_tgvoip::TgVoipEndpoint;
use super::calls_controller_webrtc::{CallsControllerWebrtc, WebrtcSessionState};
use super::calls_emoji_fingerprint::CallsEmojiFingerprint;
use super::calls_instance::{CallSound, CallsInstance};
#[cfg(not(target_os = "ios"))]
use super::peer_transport::WebRtcPeerTransport;
use crate::ice::TurnToken;
use crate::signaling::{CallSession, CallState as SignalState, SignalingFrame};
use gpui_telegram_core::types::UserId;
use std::collections::VecDeque;
use std::net::UdpSocket;
#[cfg(not(target_os = "ios"))]
use std::sync::Arc;
#[cfg(not(target_os = "ios"))]
use tokio::sync::Mutex;

/// Pure-Rust SHA-256 digest (FIPS 180-4) for WebRTC DTLS fingerprint & 4-emoji verification.
pub fn sha256_digest(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 { msg.push(0); }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4*i], chunk[4*i+1], chunk[4*i+2], chunk[4*i+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(t1);
            d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        for (slot, val) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) { *slot = slot.wrapping_add(val); }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() { out[4*i..4*i+4].copy_from_slice(&word.to_be_bytes()); }
    out
}

#[derive(Debug, Clone, Default)]
pub struct AudioLevelMeter {
    pub level: f32,
    pub peak: f32,
    pub active_samples: u64,
}

impl AudioLevelMeter {
    pub fn push_pcm_i16(&mut self, samples: &[i16]) -> f32 {
        if samples.is_empty() {
            self.level *= 0.85;
            return self.level;
        }
        let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let norm = ((sum_sq / samples.len() as f64).sqrt() / 32767.0).clamp(0.0, 1.0) as f32;
        self.level = self.level * 0.4 + norm * 0.6;
        self.peak = (self.peak * 0.95).max(norm);
        self.active_samples = self.active_samples.saturating_add(samples.len() as u64);
        self.level
    }
    pub fn is_speaking(&self) -> bool { self.level >= 0.03 }
}

pub fn gather_local_ice_candidates() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if let Ok(addr) = sock.local_addr() {
            out.push(format!("candidate:1 1 UDP 2122260223 127.0.0.1 {} typ host", addr.port()));
        }
    }
    out
}

pub fn build_sdp(call_id: &str, fingerprint_hex: &str, is_video: bool, role: &str) -> String {
    let mut sdp = format!(
        "v=0\r\no=- {call_id} 2 IN IP4 127.0.0.1\r\ns=gpui-telegram-call\r\nt=0 0\r\n\
         m=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=rtpmap:111 opus/48000/2\r\n\
         a=setup:{role}\r\na=fingerprint:sha-256 {fingerprint_hex}\r\n"
    );
    if is_video {
        sdp.push_str("m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 VP8/90000\r\n");
    }
    sdp
}

/// Real-time WebRTC signaling & call state coordinator bridging CallsInstance, CallsControllerWebrtc,
/// CallSession, AudioLevelMeter, and 4-emoji fingerprint verification.
pub struct WebRtcCallEngine {
    pub instance: CallsInstance,
    pub controller: CallsController,
    pub webrtc: CallsControllerWebrtc,
    pub session: Option<CallSession>,
    pub meter: AudioLevelMeter,
    pub turn_tokens: Vec<TurnToken>,
    pub local_sdp: Option<String>,
    pub remote_sdp: Option<String>,
    #[cfg(not(target_os = "ios"))]
    pub peer_transport: Option<Arc<Mutex<WebRtcPeerTransport>>>,
    outbound: VecDeque<SignalingFrame>,
    _local_udp: Option<UdpSocket>,
}

impl Default for WebRtcCallEngine {
    fn default() -> Self { Self::new(Vec::new()) }
}

impl WebRtcCallEngine {
    pub fn new(turn_tokens: Vec<TurnToken>) -> Self {
        Self {
            instance: CallsInstance::new(),
            controller: CallsController::new(CallsController::WEBRTC_VERSION),
            webrtc: CallsControllerWebrtc::new(&[], None, Vec::new(), true, true),
            session: None,
            meter: AudioLevelMeter::default(),
            turn_tokens,
            local_sdp: None,
            remote_sdp: None,
            #[cfg(not(target_os = "ios"))]
            peer_transport: None,
            outbound: VecDeque::new(),
            _local_udp: UdpSocket::bind("0.0.0.0:0").ok(),
        }
    }

    #[cfg(not(target_os = "ios"))]
    pub fn attach_peer_transport(&mut self, transport: WebRtcPeerTransport) {
        self.peer_transport = Some(Arc::new(Mutex::new(transport)));
    }

    #[cfg(not(target_os = "ios"))]
    pub fn with_peer_transport(mut self, transport: WebRtcPeerTransport) -> Self {
        self.attach_peer_transport(transport);
        self
    }

    pub fn start_outgoing(&mut self, call_id: i64, local_user: UserId, remote_user: UserId, is_video: bool) {
        self.instance.start_outgoing_call(call_id, remote_user.raw(), is_video);
        let local_port = self
            ._local_udp
            .as_ref()
            .and_then(|s| s.local_addr().ok())
            .map(|a| a.port())
            .unwrap_or(0);
        let ep = TgVoipEndpoint {
            endpoint_id: call_id,
            ipv4: "127.0.0.1".to_string(),
            ipv6: "::1".to_string(),
            port: local_port,
            peer_tag: [0u8; 16],
            is_tcp: false,
        };
        self.webrtc = CallsControllerWebrtc::new(&[ep], None, Vec::new(), true, true);
        let call_key = call_id.to_string();
        self.session = Some(CallSession::new(call_key.clone(), local_user, remote_user, is_video));
        let digest = sha256_digest(format!("offer:{call_key}:{}", local_user.raw()).as_bytes());
        let fp_hex = digest.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":");
        let offer = build_sdp(&call_key, &fp_hex, is_video, "actpass");
        self.local_sdp = Some(offer.clone());
        self.outbound.push_back(SignalingFrame::Invite { call_id: call_key.clone(), from_user: local_user, to_user: remote_user, is_video });
        self.outbound.push_back(SignalingFrame::Offer { call_id: call_key.clone(), sdp: offer });
        for cand in gather_local_ice_candidates() {
            self.outbound.push_back(SignalingFrame::IceCandidate {
                call_id: call_key.clone(), candidate: cand, sdp_mid: Some("0".into()), sdp_mline_index: Some(0),
            });
        }
    }

    pub fn receive_incoming(&mut self, call_id: i64, caller: UserId, callee: UserId, is_video: bool) {
        let mut call = CallsCall::new(call_id, caller.raw(), CallType::Incoming, is_video);
        call.state = CallState::WaitingIncoming;
        self.instance.current_call = Some(call);
        let local_port = self
            ._local_udp
            .as_ref()
            .and_then(|s| s.local_addr().ok())
            .map(|a| a.port())
            .unwrap_or(0);
        let ep = TgVoipEndpoint {
            endpoint_id: call_id,
            ipv4: "127.0.0.1".to_string(),
            ipv6: "::1".to_string(),
            port: local_port,
            peer_tag: [0u8; 16],
            is_tcp: false,
        };
        self.webrtc = CallsControllerWebrtc::new(&[ep], None, Vec::new(), false, true);
        let mut sess = CallSession::new(call_id.to_string(), caller, callee, is_video);
        sess.state = SignalState::Ringing;
        self.session = Some(sess);
        self.outbound.push_back(SignalingFrame::Ringing { call_id: call_id.to_string() });
    }

    pub fn accept_incoming(&mut self) -> Option<SignalingFrame> {
        let call = self.instance.current_call.as_mut()?;
        call.answer();
        let call_key = call.id.to_string();
        let is_video = call.video_enabled;
        if let Some(sess) = &mut self.session { sess.state = SignalState::Connecting; }
        let digest = sha256_digest(format!("answer:{call_key}").as_bytes());
        let fp_hex = digest.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":");
        let answer = build_sdp(&call_key, &fp_hex, is_video, "active");
        self.local_sdp = Some(answer.clone());
        self.outbound.push_back(SignalingFrame::Accept { call_id: call_key.clone() });
        let ans_frame = SignalingFrame::Answer { call_id: call_key, sdp: answer };
        self.outbound.push_back(ans_frame.clone());
        self.finalize_fingerprint_if_ready();
        Some(ans_frame)
    }

    pub fn apply_signaling_frame(&mut self, frame: SignalingFrame) {
        let bytes = serde_json::to_vec(&frame).unwrap_or_default();
        self.webrtc.receive_signaling_data(&bytes);
        self.controller.record_traffic(0, bytes.len() as u64);
        match frame {
            SignalingFrame::Invite { call_id, from_user, to_user, is_video } => {
                let numeric = call_id.parse::<i64>().unwrap_or(1);
                self.receive_incoming(numeric, from_user, to_user, is_video);
            }
            SignalingFrame::Ringing { .. } => {
                if let Some(c) = &mut self.instance.current_call { c.state = CallState::Ringing; }
                if let Some(s) = &mut self.session { s.state = SignalState::Ringing; }
            }
            SignalingFrame::Accept { .. } => {
                if let Some(c) = &mut self.instance.current_call { c.state = CallState::ExchangingKeys; }
                if let Some(s) = &mut self.session { s.state = SignalState::Connecting; }
            }
            SignalingFrame::Offer { sdp, .. } | SignalingFrame::Answer { sdp, .. } => {
                self.remote_sdp = Some(sdp);
                self.finalize_fingerprint_if_ready();
            }
            SignalingFrame::IceCandidate { .. } => { self.finalize_fingerprint_if_ready(); }
            SignalingFrame::Hangup { .. } => { self.hangup_local("remote_hangup"); }
        }
    }

    fn finalize_fingerprint_if_ready(&mut self) {
        if let (Some(loc), Some(rem), Some(call)) = (&self.local_sdp, &self.remote_sdp, &mut self.instance.current_call) {
            let (first, second) = if loc.as_str() <= rem.as_str() { (loc.as_str(), rem.as_str()) } else { (rem.as_str(), loc.as_str()) };
            let mut combined = Vec::with_capacity(first.len() + second.len());
            combined.extend_from_slice(first.as_bytes());
            combined.extend_from_slice(second.as_bytes());
            let key_sha = sha256_digest(&combined);
            call.keys_exchanged(key_sha);
            self.webrtc.encryption_key = key_sha.to_vec();
            self.webrtc.set_session_state(WebrtcSessionState::Connected);
            self.controller.update_state(CallVoipState::Established);
            if let Some(s) = &mut self.session { s.state = SignalState::Connected; }
        }
    }

    pub fn emoji_fingerprint(&self) -> Option<CallsEmojiFingerprint> {
        self.instance.current_call.as_ref()?.key_sha256.map(CallsEmojiFingerprint::from_sha256)
    }

    pub fn hangup_local(&mut self, reason: &str) {
        if let Some(sess) = &mut self.session {
            self.outbound.push_back(SignalingFrame::Hangup { call_id: sess.call_id.clone(), reason: reason.into() });
            sess.state = SignalState::Ended;
        }
        self.webrtc.stop();
        self.controller.update_state(CallVoipState::Failed);
        self.instance.destroy_current_call();
        self.instance.play_sound(CallSound::Ended);
    }

    pub fn drain_outbound_frames(&mut self) -> Vec<SignalingFrame> { self.outbound.drain(..).collect() }
    pub fn push_audio_level_pcm(&mut self, samples: &[i16]) -> f32 { self.meter.push_pcm_i16(samples) }
}
