//! Synthetic Deterministic Mock Screen Source for Headless CI

use super::device_source::{
    DeviceState, MediaCaptureError, MediaDeviceInfo, MediaDeviceKind, MediaDeviceSource,
    ScreenFrameCallback,
};
use super::frames::ScreenFrame;
use super::mock_video::MockVideoSource;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct MockScreenSource {
    info: MediaDeviceInfo,
    state: DeviceState,
    width: u32,
    height: u32,
    fps: u32,
    seq_counter: u64,
    video_mock: MockVideoSource,
    callback: Arc<Mutex<Option<ScreenFrameCallback>>>,
    streaming_handle: Option<std::thread::JoinHandle<()>>,
    stop_signal: Arc<AtomicBool>,
    paused_signal: Arc<AtomicBool>,
}

impl MockScreenSource {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self {
            info: MediaDeviceInfo {
                id: "mock_screen_source".into(),
                name: format!("Mock Desktop Screen ({}x{} @ {}fps)", width, height, fps),
                kind: MediaDeviceKind::ScreenCapture,
                is_default: true,
            },
            state: DeviceState::Uninitialized,
            width: width.max(16),
            height: height.max(16),
            fps: fps.clamp(1, 60),
            seq_counter: 0,
            video_mock: MockVideoSource::new(width, height, fps),
            callback: Arc::new(Mutex::new(None)),
            streaming_handle: None,
            stop_signal: Arc::new(AtomicBool::new(false)),
            paused_signal: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn next_frame(&mut self) -> ScreenFrame {
        let mut video_frame = self.video_mock.next_frame();
        video_frame.sequence = self.seq_counter;
        let cursor_x = ((self.seq_counter * 5) % self.width as u64) as u32;
        let cursor_y = ((self.seq_counter * 3) % self.height as u64) as u32;
        self.seq_counter += 1;

        ScreenFrame {
            video_frame,
            display_id: 1,
            cursor_pos: Some((cursor_x, cursor_y)),
        }
    }
}

impl MediaDeviceSource for MockScreenSource {
    fn kind(&self) -> MediaDeviceKind {
        MediaDeviceKind::ScreenCapture
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
        let mut screen_mock = MockScreenSource::new(self.width, self.height, self.fps);
        let frame_interval_ms = (1000 / self.fps.max(1)) as u64;

        let handle = std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                if paused.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let frame = screen_mock.next_frame();
                if !stop.load(Ordering::Acquire) && !paused.load(Ordering::Acquire) {
                    let cb_opt = cb_arc.lock().clone();
                    if let Some(cb) = cb_opt {
                        cb(frame);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(frame_interval_ms));
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

    fn set_screen_callback(&mut self, cb: ScreenFrameCallback) -> Result<(), MediaCaptureError> {
        *self.callback.lock() = Some(cb);
        Ok(())
    }
}

impl Drop for MockScreenSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
