//! Synthetic Deterministic Mock Audio Source for Headless CI

use super::device_source::{
    AudioFrameCallback, DeviceState, MediaCaptureError, MediaDeviceInfo, MediaDeviceKind,
    MediaDeviceSource,
};
use super::frames::AudioFrame;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum MockAudioMode {
    SineWave { frequency_hz: f32 },
    Silence,
    DeterministicCounter,
}

pub struct MockAudioSource {
    info: MediaDeviceInfo,
    state: DeviceState,
    sample_rate: u32,
    channels: u16,
    mode: MockAudioMode,
    callback: Arc<Mutex<Option<AudioFrameCallback>>>,
    seq_counter: u64,
    phase: f32,
    stop_signal: Arc<AtomicBool>,
    paused_signal: Arc<AtomicBool>,
    streaming_handle: Option<std::thread::JoinHandle<()>>,
}

impl MockAudioSource {
    pub fn new_sine(frequency_hz: f32, sample_rate: u32, channels: u16) -> Self {
        Self {
            info: MediaDeviceInfo {
                id: "mock_audio_sine".into(),
                name: format!("Mock Sine Wave ({} Hz)", frequency_hz),
                kind: MediaDeviceKind::AudioInput,
                is_default: true,
            },
            state: DeviceState::Uninitialized,
            sample_rate: sample_rate.max(8000),
            channels: channels.clamp(1, 2),
            mode: MockAudioMode::SineWave { frequency_hz },
            callback: Arc::new(Mutex::new(None)),
            seq_counter: 0,
            phase: 0.0,
            stop_signal: Arc::new(AtomicBool::new(false)),
            paused_signal: Arc::new(AtomicBool::new(false)),
            streaming_handle: None,
        }
    }

    pub fn new_silence(sample_rate: u32, channels: u16) -> Self {
        Self {
            info: MediaDeviceInfo {
                id: "mock_audio_silence".into(),
                name: "Mock Silence Generator".into(),
                kind: MediaDeviceKind::AudioInput,
                is_default: false,
            },
            state: DeviceState::Uninitialized,
            sample_rate: sample_rate.max(8000),
            channels: channels.clamp(1, 2),
            mode: MockAudioMode::Silence,
            callback: Arc::new(Mutex::new(None)),
            seq_counter: 0,
            phase: 0.0,
            stop_signal: Arc::new(AtomicBool::new(false)),
            paused_signal: Arc::new(AtomicBool::new(false)),
            streaming_handle: None,
        }
    }

    /// Synchronous frame generator (ideal for unit tests and instant CI checks).
    pub fn next_frame(&mut self, duration_ms: u32) -> AudioFrame {
        let count = ((self.sample_rate as u64 * duration_ms as u64 * self.channels as u64) / 1000)
            as usize;
        let mut samples = Vec::with_capacity(count);

        match self.mode {
            MockAudioMode::SineWave { frequency_hz } => {
                let step = (2.0 * std::f32::consts::PI * frequency_hz) / self.sample_rate as f32;
                for _ in 0..(count / self.channels as usize) {
                    let val = (self.phase.sin() * 16384.0).clamp(-32767.0, 32767.0) as i16;
                    for _ in 0..self.channels {
                        samples.push(val);
                    }
                    self.phase = (self.phase + step) % (2.0 * std::f32::consts::PI);
                }
            }
            MockAudioMode::Silence => {
                samples.resize(count, 0);
            }
            MockAudioMode::DeterministicCounter => {
                for i in 0..count {
                    samples.push((i % 32767) as i16);
                }
            }
        }

        let frame = AudioFrame::new(samples, self.sample_rate, self.channels, self.seq_counter);
        self.seq_counter += 1;
        frame
    }
}

impl MediaDeviceSource for MockAudioSource {
    fn kind(&self) -> MediaDeviceKind {
        MediaDeviceKind::AudioInput
    }

    fn device_info(&self) -> MediaDeviceInfo {
        self.info.clone()
    }

    fn state(&self) -> DeviceState {
        self.state
    }

    fn select_device(&mut self, device_id: &str) -> Result<(), MediaCaptureError> {
        self.info.id = device_id.to_string();
        Ok(())
    }

    fn start(&mut self) -> Result<(), MediaCaptureError> {
        if self.state == DeviceState::Streaming {
            return Ok(());
        }
        if self.state == DeviceState::Failed {
            return Err(MediaCaptureError::InvalidState(
                self.state,
                DeviceState::Streaming,
            ));
        }
        if self.state == DeviceState::Paused && self.streaming_handle.is_some() {
            self.paused_signal.store(false, Ordering::SeqCst);
            self.state = DeviceState::Streaming;
            return Ok(());
        }

        if let Some(prev) = self.streaming_handle.take() {
            self.stop_signal.store(true, Ordering::SeqCst);
            let _ = prev.join();
        }

        self.state = DeviceState::Streaming;
        self.stop_signal.store(false, Ordering::SeqCst);
        self.paused_signal.store(false, Ordering::SeqCst);

        let stop = self.stop_signal.clone();
        let paused = self.paused_signal.clone();
        let cb_arc = self.callback.clone();
        let sample_rate = self.sample_rate;
        let channels = self.channels;
        let mode = self.mode.clone();

        let handle = std::thread::spawn(move || {
            let mut seq = 0u64;
            let mut phase = 0.0f32;
            let count = ((sample_rate as u64 * 20 * channels as u64) / 1000) as usize;

            while !stop.load(Ordering::Acquire) {
                if paused.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }

                let mut samples = Vec::with_capacity(count);
                match mode {
                    MockAudioMode::SineWave { frequency_hz } => {
                        let step = (2.0 * std::f32::consts::PI * frequency_hz) / sample_rate as f32;
                        for _ in 0..(count / channels as usize) {
                            let val = (phase.sin() * 16384.0).clamp(-32767.0, 32767.0) as i16;
                            for _ in 0..channels {
                                samples.push(val);
                            }
                            phase = (phase + step) % (2.0 * std::f32::consts::PI);
                        }
                    }
                    MockAudioMode::Silence => {
                        samples.resize(count, 0);
                    }
                    MockAudioMode::DeterministicCounter => {
                        for i in 0..count {
                            samples.push((i % 32767) as i16);
                        }
                    }
                }

                let frame = AudioFrame::new(samples, sample_rate, channels, seq);
                seq += 1;

                if !stop.load(Ordering::Acquire) && !paused.load(Ordering::Acquire) {
                    let cb_opt = cb_arc.lock().clone();
                    if let Some(cb) = cb_opt {
                        cb(frame);
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });

        self.streaming_handle = Some(handle);
        Ok(())
    }

    fn pause(&mut self, paused: bool) -> Result<(), MediaCaptureError> {
        if self.state == DeviceState::Stopped
            || self.state == DeviceState::Uninitialized
            || self.state == DeviceState::Failed
        {
            return Err(MediaCaptureError::InvalidState(
                self.state,
                DeviceState::Paused,
            ));
        }
        self.paused_signal.store(paused, Ordering::SeqCst);
        self.state = if paused {
            DeviceState::Paused
        } else {
            DeviceState::Streaming
        };
        Ok(())
    }

    fn stop(&mut self) -> Result<(), MediaCaptureError> {
        self.stop_signal.store(true, Ordering::SeqCst);
        self.paused_signal.store(false, Ordering::SeqCst);
        if let Some(handle) = self.streaming_handle.take() {
            let _ = handle.join();
        }
        self.state = DeviceState::Stopped;
        Ok(())
    }

    fn set_audio_callback(&mut self, cb: AudioFrameCallback) -> Result<(), MediaCaptureError> {
        *self.callback.lock() = Some(cb);
        Ok(())
    }
}

impl Drop for MockAudioSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
