//! SFU / WebRTC AI Media Tap with <60ms VAD Barge-in Interruption
//!
//! Exposes the AiMediaTap trait and DefaultAiMediaTap implementation, monitoring
//! incoming and outgoing PCM streams for voice activity and triggering low-latency
//! barge-in interruptions within <60ms (2-3 consecutive 20ms frames or >= 60ms active audio).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Notify};

/// AI Media Tap trait for SFU and WebRTC voice calls
pub trait AiMediaTap: Send + Sync {
    /// Ingests incoming remote PCM audio frame
    fn on_incoming_pcm_frame(&self, frame: &[i16], sample_rate: u32, channels: u16);
    /// Ingests outgoing local PCM audio frame
    fn on_outgoing_pcm_frame(&self, frame: &[i16], sample_rate: u32, channels: u16);
    /// Queries whether incoming voice activity has triggered an AI barge-in interruption (<60ms)
    fn trigger_vad_barge_in(&self) -> bool;
}

/// Default implementation of AiMediaTap with RMS energy calculation, atomic VAD state,
/// and event-driven tokio Notify wake-up on barge-in.
pub struct DefaultAiMediaTap {
    sample_rate: u32,
    channels: u16,
    vad_energy_threshold: f32,
    required_consecutive_frames: u32,
    explicit_frames_override: bool,
    consecutive_active_frames: AtomicU32,
    consecutive_active_ms: AtomicU32,
    barge_in_triggered: AtomicBool,
    barge_in_notify: Arc<Notify>,
    last_incoming_rms_bits: AtomicU32,
    last_outgoing_rms_bits: AtomicU32,
    dropped_incoming_frames: AtomicU64,
    dropped_outgoing_frames: AtomicU64,
    incoming_audio_tx: Option<mpsc::Sender<Vec<i16>>>,
    outgoing_audio_tx: Option<mpsc::Sender<Vec<i16>>>,
}

impl DefaultAiMediaTap {
    /// Creates a new DefaultAiMediaTap with the specified sample rate, channels, and VAD threshold
    pub fn new(sample_rate: u32, channels: u16, vad_threshold: f32) -> Self {
        let safe_threshold = if vad_threshold.is_nan() {
            0.05
        } else {
            vad_threshold.clamp(0.001, 1.0)
        };
        Self {
            sample_rate: sample_rate.max(8000),
            channels: channels.clamp(1, 2),
            vad_energy_threshold: safe_threshold,
            required_consecutive_frames: 3, // 3 * 20ms = 60ms maximum latency
            explicit_frames_override: false,
            consecutive_active_frames: AtomicU32::new(0),
            consecutive_active_ms: AtomicU32::new(0),
            barge_in_triggered: AtomicBool::new(false),
            barge_in_notify: Arc::new(Notify::new()),
            last_incoming_rms_bits: AtomicU32::new(0),
            last_outgoing_rms_bits: AtomicU32::new(0),
            dropped_incoming_frames: AtomicU64::new(0),
            dropped_outgoing_frames: AtomicU64::new(0),
            incoming_audio_tx: None,
            outgoing_audio_tx: None,
        }
    }

    pub fn with_incoming_sender(mut self, tx: mpsc::Sender<Vec<i16>>) -> Self {
        self.incoming_audio_tx = Some(tx);
        self
    }

    pub fn with_outgoing_sender(mut self, tx: mpsc::Sender<Vec<i16>>) -> Self {
        self.outgoing_audio_tx = Some(tx);
        self
    }

    pub fn with_consecutive_frames(mut self, count: u32) -> Self {
        self.required_consecutive_frames = count.max(1);
        self.explicit_frames_override = true;
        self
    }

    /// Returns a clone of the `Notify` that fires immediately when VAD barge-in triggers.
    pub fn barge_in_notify(&self) -> Arc<Notify> {
        self.barge_in_notify.clone()
    }

    /// Awaits the next VAD barge-in transition (returns immediately if already triggered).
    pub async fn wait_for_barge_in(&self) {
        if self.barge_in_triggered.load(Ordering::SeqCst) {
            return;
        }
        let notified = self.barge_in_notify.notified();
        if self.barge_in_triggered.load(Ordering::SeqCst) {
            return;
        }
        notified.await;
    }

    /// Computes normalized Root-Mean-Square (RMS) amplitude in [0.0, 1.0]
    pub fn compute_rms(samples: &[i16]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let mut sum_sq = 0.0f64;
        for &s in samples {
            let norm = (s as f64) / 32768.0;
            sum_sq += norm * norm;
        }
        ((sum_sq / samples.len() as f64).sqrt() as f32).clamp(0.0, 1.0)
    }

    pub fn reset_barge_in(&self) {
        self.barge_in_triggered.store(false, Ordering::SeqCst);
        self.consecutive_active_frames.store(0, Ordering::SeqCst);
        self.consecutive_active_ms.store(0, Ordering::SeqCst);
    }

    pub fn last_incoming_rms(&self) -> f32 {
        f32::from_bits(self.last_incoming_rms_bits.load(Ordering::Relaxed))
    }

    pub fn last_outgoing_rms(&self) -> f32 {
        f32::from_bits(self.last_outgoing_rms_bits.load(Ordering::Relaxed))
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn dropped_incoming_frames(&self) -> u64 {
        self.dropped_incoming_frames.load(Ordering::Relaxed)
    }

    pub fn dropped_outgoing_frames(&self) -> u64 {
        self.dropped_outgoing_frames.load(Ordering::Relaxed)
    }
}

impl AiMediaTap for DefaultAiMediaTap {
    fn on_incoming_pcm_frame(&self, frame: &[i16], sample_rate: u32, channels: u16) {
        let rms = Self::compute_rms(frame);
        self.last_incoming_rms_bits.store(rms.to_bits(), Ordering::Relaxed);

        let effective_sr = if sample_rate >= 8000 {
            sample_rate
        } else {
            self.sample_rate.max(8000)
        };
        let effective_ch = if channels > 0 { channels as u32 } else { self.channels as u32 };
        let samples_per_channel = (frame.len() as u32) / effective_ch.max(1);
        let frame_ms = ((samples_per_channel as u64 * 1000) / effective_sr as u64).max(1) as u32;

        if rms >= self.vad_energy_threshold {
            let count = self.consecutive_active_frames.fetch_add(1, Ordering::SeqCst) + 1;
            let active_ms = self.consecutive_active_ms.fetch_add(frame_ms, Ordering::SeqCst) + frame_ms;
            let triggered = if self.explicit_frames_override {
                count >= self.required_consecutive_frames
            } else {
                count >= self.required_consecutive_frames || active_ms >= 60
            };
            if triggered && !self.barge_in_triggered.swap(true, Ordering::SeqCst) {
                self.barge_in_notify.notify_waiters();
            }
        } else {
            self.consecutive_active_frames.store(0, Ordering::SeqCst);
            self.consecutive_active_ms.store(0, Ordering::SeqCst);
        }

        if let Some(ref tx) = self.incoming_audio_tx {
            if tx.try_send(frame.to_vec()).is_err() {
                self.dropped_incoming_frames.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn on_outgoing_pcm_frame(&self, frame: &[i16], _sample_rate: u32, _channels: u16) {
        let rms = Self::compute_rms(frame);
        self.last_outgoing_rms_bits.store(rms.to_bits(), Ordering::Relaxed);

        if let Some(ref tx) = self.outgoing_audio_tx {
            if tx.try_send(frame.to_vec()).is_err() {
                self.dropped_outgoing_frames.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn trigger_vad_barge_in(&self) -> bool {
        self.barge_in_triggered.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_barge_in_notify_wakes_immediately() {
        let tap = Arc::new(DefaultAiMediaTap::new(48_000, 1, 0.05));
        let tap_clone = tap.clone();
        let handle = tokio::spawn(async move {
            tap_clone.wait_for_barge_in().await;
            true
        });
        // Feed 3 x 20ms speech frames = 60ms
        let speech = vec![10_000i16; 960];
        tap.on_incoming_pcm_frame(&speech, 48_000, 1);
        tap.on_incoming_pcm_frame(&speech, 48_000, 1);
        assert!(!tap.trigger_vad_barge_in());
        tap.on_incoming_pcm_frame(&speech, 48_000, 1);
        assert!(tap.trigger_vad_barge_in());
        assert!(tokio::time::timeout(std::time::Duration::from_millis(200), handle).await.unwrap().unwrap());
    }

    #[test]
    fn test_nan_vad_threshold_defaults_safely_and_triggers_on_speech() {
        let tap = DefaultAiMediaTap::new(48_000, 1, f32::NAN);
        let silence = vec![0i16; 960];
        tap.on_incoming_pcm_frame(&silence, 48_000, 1);
        assert!(!tap.trigger_vad_barge_in());

        let speech = vec![10_000i16; 960];
        tap.on_incoming_pcm_frame(&speech, 48_000, 1);
        tap.on_incoming_pcm_frame(&speech, 48_000, 1);
        tap.on_incoming_pcm_frame(&speech, 48_000, 1);
        assert!(tap.trigger_vad_barge_in());
    }
}
