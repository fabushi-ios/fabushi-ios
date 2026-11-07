pub use gpui_telegram_media::waveform::{
    decode_waveform_5bit, decode_waveform_to_floats, encode_waveform_5bit,
    encode_waveform_from_floats, normalize_pcm_peaks, TELEGRAM_WAVEFORM_BARS,
    TELEGRAM_WAVEFORM_BYTES,
};

pub struct VadBargeInDetector {
    pub energy_threshold: f32,
    pub consecutive_speech_frames: u32,
    pub consecutive_silence_frames: u32,
}

impl Default for VadBargeInDetector {
    fn default() -> Self {
        Self::new(300.0)
    }
}

impl VadBargeInDetector {
    pub fn new(energy_threshold: f32) -> Self {
        Self {
            energy_threshold,
            consecutive_speech_frames: 0,
            consecutive_silence_frames: 0,
        }
    }

    /// Process a 20ms PCM audio frame.
    /// Detects voice activity and returns true when user speech exceeds threshold
    /// for >= 3 consecutive 20ms frames (<60ms latency barge-in trigger).
    pub fn process_pcm_frame(&mut self, samples: &[i16]) -> bool {
        if samples.is_empty() {
            return false;
        }

        let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let rms = (sum_sq / (samples.len() as f64)).sqrt() as f32;

        if rms > self.energy_threshold {
            self.consecutive_speech_frames += 1;
            self.consecutive_silence_frames = 0;
        } else {
            self.consecutive_speech_frames = 0;
            self.consecutive_silence_frames += 1;
        }

        self.consecutive_speech_frames >= 3
    }

    /// Detects user speech completion when silence persists for >= 500ms (25 frames @ 20ms)
    pub fn is_silence_detected(&self) -> bool {
        self.consecutive_silence_frames >= 25
    }

    pub fn reset(&mut self) {
        self.consecutive_speech_frames = 0;
        self.consecutive_silence_frames = 0;
    }

    /// Encodes raw PCM audio samples into Telegram standard 5-bit packed waveform
    pub fn encode_pcm_to_5bit_waveform(&self, samples: &[i16]) -> Vec<u8> {
        if samples.is_empty() {
            return Vec::new();
        }

        let bars_count = TELEGRAM_WAVEFORM_BARS;
        let chunk_size = (samples.len() + bars_count - 1) / bars_count;
        let mut peaks = Vec::with_capacity(bars_count);

        for chunk in samples.chunks(chunk_size.max(1)) {
            let max_val = chunk.iter().map(|&s| s.abs() as u16).max().unwrap_or(0);
            peaks.push(max_val);
        }

        let normalized = normalize_pcm_peaks(&peaks);
        encode_waveform_5bit(&normalized)
    }
}
