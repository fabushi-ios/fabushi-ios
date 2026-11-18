//! Telegram Desktop 1:1 Parity Submodule: `media_audio_capture.rs`
//! Subsystem: `media` | Feature: `TCG-N06` (Audio Capture & Voice Recording)
//! Upstream: `Telegram/SourceFiles/media/audio/media_audio_capture.h:L16-L78`
//!           `Telegram/SourceFiles/media/audio/media_audio_capture.cpp:L25-L79`

use serde::{Deserialize, Serialize};

/// Upstream: `media_audio_capture.cpp:L25-L30`
pub const CAPTURE_FREQUENCY: i32 = 48000;
pub const CAPTURE_SKIP_DURATION_MS: i64 = 400;
pub const CAPTURE_FADE_IN_DURATION_MS: i64 = 300;
pub const CAPTURE_BUFFER_SLICE: usize = 256 * 1024;
pub const CAPTURE_UPDATE_DELTA_MS: i64 = 100;

/// Upstream: `media_audio_capture.h:L16-L21`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CaptureUpdate {
    pub samples: i32,
    pub level: u16,
    pub finished: bool,
}

/// Upstream: `media_audio_capture.h:L23-L30`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureError {
    Other,
    AudioInit,
    VideoInit,
    AudioTimeout,
    VideoTimeout,
    Encoding,
}

/// Upstream: `media_audio_capture.h:L32-L36`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CaptureChunk {
    pub finished_at_ms: i64,
    pub samples: Vec<u8>,
    pub frequency: i32,
}

/// Accumulate waveform peak from captured raw uchar samples
/// Upstream: `media_audio_capture.cpp:L42-L79`
pub fn collect_waveform(raw_samples: &[u8]) -> Vec<u8> {
    if raw_samples.is_empty() {
        return Vec::new();
    }
    const WAVEFORM_SAMPLES_COUNT: usize = 100;
    let count = raw_samples.len();
    if count < WAVEFORM_SAMPLES_COUNT {
        return vec![0; count];
    }
    let mut peaks = Vec::with_capacity(WAVEFORM_SAMPLES_COUNT);
    let mut current_peak: u16 = 0;
    let mut sum: usize = 0;

    for &b in raw_samples {
        let sample = (b as u16) * 256;
        if sample > current_peak {
            current_peak = sample;
        }
        sum += WAVEFORM_SAMPLES_COUNT;
        if sum >= count {
            sum -= count;
            peaks.push(current_peak);
            current_peak = 0;
        }
    }
    if peaks.is_empty() {
        return vec![0; WAVEFORM_SAMPLES_COUNT];
    }
    let sum_all: u64 = peaks.iter().map(|&p| p as u64).sum();
    let avg = sum_all as f64 / peaks.len() as f64;
    let peak_max = (avg * 1.8).max(2500.0) as u32;

    peaks
        .into_iter()
        .map(|p| {
            let clamped = (p as u32).min(peak_max);
            ((clamped * 31) / peak_max).min(31) as u8
        })
        .collect()
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaAudioCaptureAudio {
    pub is_available: bool,
    pub is_recording: bool,
    pub is_paused: bool,
    pub captured_bytes: usize,
    pub duration_ms: i64,
    pub last_update: CaptureUpdate,
    #[serde(skip)]
    pcm_samples: Vec<i16>,
}

impl MediaAudioCaptureAudio {
    pub fn new() -> Self {
        Self {
            is_available: true,
            is_recording: false,
            is_paused: false,
            captured_bytes: 0,
            duration_ms: 0,
            last_update: CaptureUpdate::default(),
            pcm_samples: Vec::new(),
        }
    }

    pub fn start(&mut self) {
        self.is_recording = true;
        self.is_paused = false;
        self.captured_bytes = 0;
        self.duration_ms = 0;
        self.pcm_samples.clear();
    }

    pub fn append_pcm_samples(&mut self, samples: &[i16]) {
        if !self.is_recording || self.is_paused || samples.is_empty() {
            return;
        }
        self.pcm_samples.extend_from_slice(samples);
        self.captured_bytes = self.pcm_samples.len().saturating_mul(2);
        self.duration_ms = ((self.pcm_samples.len() as i128 * 1000) / CAPTURE_FREQUENCY as i128) as i64;
        self.last_update = CaptureUpdate {
            samples: self.pcm_samples.len() as i32,
            level: super::wav::compute_pcm_rms_level(samples),
            finished: false,
        };
    }

    pub fn waveform_5bit(&self) -> (Vec<u8>, Vec<u8>) {
        super::wav::waveform_from_pcm_i16(&self.pcm_samples)
    }

    pub fn pause(&mut self, paused: bool) {
        if self.is_recording {
            self.is_paused = paused;
        }
    }

    pub fn stop(&mut self) -> CaptureChunk {
        self.is_recording = false;
        self.is_paused = false;
        self.last_update.finished = true;
        let wav = if self.pcm_samples.is_empty() {
            Vec::new()
        } else {
            super::wav::encode_wav_i16(&self.pcm_samples, CAPTURE_FREQUENCY as u32, 1)
                .unwrap_or_default()
        };
        CaptureChunk {
            finished_at_ms: self.duration_ms,
            samples: wav,
            frequency: CAPTURE_FREQUENCY,
        }
    }
}
