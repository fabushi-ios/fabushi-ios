//! Synthetic Deterministic Mock Video Source for Headless CI

use super::device_source::{
    DeviceState, MediaCaptureError, MediaDeviceInfo, MediaDeviceKind, MediaDeviceSource,
    VideoFrameCallback,
};
use super::frames::VideoFrame;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct MockVideoSource {
    info: MediaDeviceInfo,
    state: DeviceState,
    width: u32,
    height: u32,
    fps: u32,
    seq_counter: u64,
    box_x: i32,
    box_y: i32,
    dx: i32,
    dy: i32,
    callback: Arc<Mutex<Option<VideoFrameCallback>>>,
    streaming_handle: Option<std::thread::JoinHandle<()>>,
    stop_signal: Arc<AtomicBool>,
    paused_signal: Arc<AtomicBool>,
}

impl MockVideoSource {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self {
            info: MediaDeviceInfo {
                id: "mock_video_source".into(),
                name: format!("Mock Video Camera ({}x{} @ {}fps)", width, height, fps),
                kind: MediaDeviceKind::VideoInput,
                is_default: true,
            },
            state: DeviceState::Uninitialized,
            width: width.max(16),
            height: height.max(16),
            fps: fps.clamp(1, 120),
            seq_counter: 0,
            box_x: 10,
            box_y: 10,
            dx: 4,
            dy: 4,
            callback: Arc::new(Mutex::new(None)),
            streaming_handle: None,
            stop_signal: Arc::new(AtomicBool::new(false)),
            paused_signal: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Generates next 30 FPS synthetic test frame in RGBA8 with bouncing box and shifting color gradient.
    pub fn next_frame(&mut self) -> VideoFrame {
        let w = self.width as usize;
        let h = self.height as usize;
        let mut data = vec![0u8; w * h * 4];
        let seq = self.seq_counter;

        // Dynamic gradient background
        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) * 4;
                data[idx] = ((x * 255 / w) as u64 + seq * 3) as u8; // R
                data[idx + 1] = ((y * 255 / h) as u64 + seq * 5) as u8; // G
                data[idx + 2] = (seq * 7) as u8; // B
                data[idx + 3] = 255; // A
            }
        }

        // 60x60 bouncing white box with boundary bounce
        let box_size = 60i32.min(self.width as i32 / 2).max(8);
        self.box_x += self.dx;
        self.box_y += self.dy;
        if self.box_x <= 0 || self.box_x + box_size >= self.width as i32 {
            self.dx = -self.dx;
        }
        if self.box_y <= 0 || self.box_y + box_size >= self.height as i32 {
            self.dy = -self.dy;
        }
        self.box_x = self.box_x.clamp(0, (self.width as i32).saturating_sub(box_size));
        self.box_y = self.box_y.clamp(0, (self.height as i32).saturating_sub(box_size));

        for by in self.box_y..(self.box_y + box_size) {
            for bx in self.box_x..(self.box_x + box_size) {
                if bx >= 0 && bx < self.width as i32 && by >= 0 && by < self.height as i32 {
                    let idx = ((by as usize) * w + (bx as usize)) * 4;
                    data[idx] = 255;
                    data[idx + 1] = 255;
                    data[idx + 2] = 255;
                    data[idx + 3] = 255;
                }
            }
        }

        let frame = VideoFrame::new_rgba8(self.width, self.height, data, self.seq_counter);
        self.seq_counter += 1;
        frame
    }
}

impl MediaDeviceSource for MockVideoSource {
    fn kind(&self) -> MediaDeviceKind {
        MediaDeviceKind::VideoInput
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
        let mut source = MockVideoSource::new(self.width, self.height, self.fps);
        let frame_interval_ms = (1000 / self.fps.max(1)) as u64;

        let handle = std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                if paused.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let frame = source.next_frame();
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

    fn set_video_callback(&mut self, cb: VideoFrameCallback) -> Result<(), MediaCaptureError> {
        *self.callback.lock() = Some(cb);
        Ok(())
    }
}

impl Drop for MockVideoSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
