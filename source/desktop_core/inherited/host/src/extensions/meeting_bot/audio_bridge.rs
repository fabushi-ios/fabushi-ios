//! OpenClaw Meeting Bot Audio Bridge & Base64 Payload Codec
//!
//! Mirrors:
//! - `src/meeting-bot/audio-base64.ts`
//! - `src/meeting-bot/audio-base64.test.ts`
//! - `src/meeting-bot/audio-backend.ts`
//! - `src/meeting-bot/audio-bridge-source.ts`
//! - `src/meeting-bot/node-audio-config.ts`
//! - `src/meeting-bot/sox-audio-command.ts`
//! - `src/meeting-bot/voice-call-gateway.ts`
//! - `src/meeting-bot/output-loopback-verifier.ts`
//! - `src/meeting-bot/transcripts-bridge.ts`

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};

pub fn is_meeting_audio_base64(payload: &str) -> bool {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return false;
    }
    STANDARD.decode(trimmed).is_ok()
}

pub fn decode_meeting_audio_base64(payload: &str, action: &str) -> Result<Vec<u8>, String> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return Err(format!("{} base64 must be a valid audio payload", action));
    }
    STANDARD
        .decode(trimmed)
        .map_err(|_| format!("{} base64 must be a valid audio payload", action))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioSampleRate {
    Rate16kHz = 16000,
    Rate24kHz = 24000,
    Rate48kHz = 48000,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingAudioChunk {
    pub session_id: String,
    pub timestamp_ms: u64,
    pub sample_rate: AudioSampleRate,
    pub channels: u8,
    pub data: Vec<u8>,
}

impl MeetingAudioChunk {
    pub fn new(
        session_id: impl Into<String>,
        timestamp_ms: u64,
        sample_rate: AudioSampleRate,
        channels: u8,
        data: Vec<u8>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            timestamp_ms,
            sample_rate,
            channels,
            data,
        }
    }

    pub fn duration_ms(&self) -> u64 {
        if self.channels == 0 {
            return 0;
        }
        let bytes_per_sample = 2u64; // 16-bit PCM
        let samples = self.data.len() as u64 / (self.channels as u64 * bytes_per_sample);
        (samples * 1000) / (self.sample_rate as u64)
    }
}
