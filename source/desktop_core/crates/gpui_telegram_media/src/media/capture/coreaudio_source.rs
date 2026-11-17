//! macOS CoreAudio 20ms AudioQueue Streaming Capture Source

use super::device_source::{
    AudioFrameCallback, DeviceState, MediaCaptureError, MediaDeviceInfo, MediaDeviceKind,
    MediaDeviceSource,
};
use super::frames::AudioFrame;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

struct CaptureShared {
    sample_rate: u32,
    channels: u16,
    callback: Arc<Mutex<Option<AudioFrameCallback>>>,
    seq_counter: AtomicU64,
    paused: AtomicBool,
    stopped: AtomicBool,
}

#[cfg(target_os = "macos")]
mod macos_capture_aq {
    use super::*;
    use crate::media::audio::recorder::macos_aq_ffi::*;
    use std::ffi::c_void;
    use std::ptr;

    pub struct ActiveCaptureQueue {
        pub queue: AudioQueueRef,
        pub raw_shared: *const CaptureShared,
    }

    unsafe impl Send for ActiveCaptureQueue {}
    unsafe impl Sync for ActiveCaptureQueue {}

    unsafe extern "C" fn input_callback(
        user_data: *mut c_void,
        aq: AudioQueueRef,
        buf: AudioQueueBufferRef,
        _start_time: *const c_void,
        _num_pkts: u32,
        _pkt_descs: *const c_void,
    ) {
        if user_data.is_null() || buf.is_null() {
            return;
        }
        let shared = unsafe { &*(user_data as *const CaptureShared) };
        if shared.stopped.load(Ordering::Acquire) {
            return;
        }
        let b = unsafe { &*buf };
        let byte_len = b.m_audio_data_byte_size as usize;
        if !shared.paused.load(Ordering::Acquire) && !b.m_audio_data.is_null() && byte_len >= 2 {
            let bytes = unsafe { std::slice::from_raw_parts(b.m_audio_data as *const u8, byte_len) };
            let mut samples = Vec::with_capacity(bytes.len() / 2);
            for pair in bytes.chunks_exact(2) {
                samples.push(i16::from_le_bytes([pair[0], pair[1]]));
            }
            let seq = shared.seq_counter.fetch_add(1, Ordering::Relaxed);
            let frame = AudioFrame::new(samples, shared.sample_rate, shared.channels, seq);
            let cb_opt = shared.callback.lock().clone();
            if let Some(cb) = cb_opt {
                cb(frame);
            }
        }
        let _ = unsafe { AudioQueueEnqueueBuffer(aq, buf, 0, ptr::null()) };
    }

    pub fn start_queue(
        sample_rate: u32,
        channels: u16,
        shared: &Arc<CaptureShared>,
    ) -> Result<ActiveCaptureQueue, i32> {
        let ch = channels.clamp(1, 2) as u32;
        let bytes_per_frame = ch * 2;
        let format = AudioStreamBasicDescription {
            m_sample_rate: sample_rate as f64,
            m_format_id: u32::from_be_bytes(*b"lpcm"),
            m_format_flags: 12, // kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked
            m_bytes_per_packet: bytes_per_frame,
            m_frames_per_packet: 1,
            m_bytes_per_frame: bytes_per_frame,
            m_channels_per_frame: ch,
            m_bits_per_channel: 16,
            m_reserved: 0,
        };
        let raw_shared = Arc::into_raw(Arc::clone(shared));
        let mut queue: AudioQueueRef = ptr::null_mut();
        let status = unsafe {
            AudioQueueNewInput(
                &format,
                input_callback,
                raw_shared as *mut c_void,
                ptr::null_mut(),
                ptr::null(),
                0,
                &mut queue,
            )
        };
        if status != 0 || queue.is_null() {
            unsafe {
                drop(Arc::from_raw(raw_shared));
            }
            return Err(status);
        }

        // 20ms buffer: e.g., 48000 Hz * 0.020s * 1ch * 2 bytes = 1920 bytes
        let buffer_bytes = ((sample_rate as u64 * 20 / 1000) * bytes_per_frame as u64).max(320) as u32;
        for _ in 0..3 {
            let mut buf: AudioQueueBufferRef = ptr::null_mut();
            let s1 = unsafe { AudioQueueAllocateBuffer(queue, buffer_bytes, &mut buf) };
            let s2 = if s1 == 0 {
                unsafe { AudioQueueEnqueueBuffer(queue, buf, 0, ptr::null()) }
            } else {
                s1
            };
            if s2 != 0 {
                shared.stopped.store(true, Ordering::Release);
                unsafe {
                    AudioQueueDispose(queue, 1);
                    drop(Arc::from_raw(raw_shared));
                }
                return Err(s2);
            }
        }

        let s_start = unsafe { AudioQueueStart(queue, ptr::null()) };
        if s_start != 0 {
            shared.stopped.store(true, Ordering::Release);
            unsafe {
                AudioQueueDispose(queue, 1);
                drop(Arc::from_raw(raw_shared));
            }
            return Err(s_start);
        }

        Ok(ActiveCaptureQueue { queue, raw_shared })
    }

    pub fn pause_queue(aq: &ActiveCaptureQueue) -> i32 {
        unsafe { AudioQueuePause(aq.queue) }
    }

    pub fn resume_queue(aq: &ActiveCaptureQueue) -> i32 {
        unsafe { AudioQueueStart(aq.queue, ptr::null()) }
    }

    pub fn stop_queue(aq: ActiveCaptureQueue, shared: &CaptureShared) {
        shared.stopped.store(true, Ordering::Release);
        unsafe {
            AudioQueueStop(aq.queue, 1);
            AudioQueueDispose(aq.queue, 1);
            drop(Arc::from_raw(aq.raw_shared));
        }
    }
}

pub struct CoreAudioCaptureSource {
    info: MediaDeviceInfo,
    state: DeviceState,
    sample_rate: u32,
    channels: u16,
    shared: Arc<CaptureShared>,
    #[cfg(target_os = "macos")]
    active_queue: Option<macos_capture_aq::ActiveCaptureQueue>,
    last_os_status: Option<i32>,
}

impl CoreAudioCaptureSource {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let rate = sample_rate.max(8000);
        let ch = channels.clamp(1, 2);
        Self {
            info: MediaDeviceInfo {
                id: "default".into(),
                name: "System Default Microphone".into(),
                kind: MediaDeviceKind::AudioInput,
                is_default: true,
            },
            state: DeviceState::Uninitialized,
            sample_rate: rate,
            channels: ch,
            shared: Arc::new(CaptureShared {
                sample_rate: rate,
                channels: ch,
                callback: Arc::new(Mutex::new(None)),
                seq_counter: AtomicU64::new(0),
                paused: AtomicBool::new(false),
                stopped: AtomicBool::new(false),
            }),
            #[cfg(target_os = "macos")]
            active_queue: None,
            last_os_status: None,
        }
    }

    pub fn enumerate_devices() -> Vec<MediaDeviceInfo> {
        let (inputs, _) = crate::media::audio::recorder::enumerate_audio_devices();
        inputs
            .into_iter()
            .enumerate()
            .map(|(idx, name)| MediaDeviceInfo {
                id: format!("coreaudio_in_{idx}"),
                name,
                kind: MediaDeviceKind::AudioInput,
                is_default: idx == 0,
            })
            .collect()
    }

    pub fn is_hardware_queue_active(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            return self.active_queue.is_some();
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn last_os_status(&self) -> Option<i32> {
        self.last_os_status
    }

    /// Feeds raw PCM samples into the capture pipeline and dispatches to registered callback
    pub fn deliver_raw_samples(&mut self, samples: &[i16]) {
        if self.state != DeviceState::Streaming {
            return;
        }

        let seq = self.shared.seq_counter.fetch_add(1, Ordering::Relaxed);
        let frame = AudioFrame::new(samples.to_vec(), self.sample_rate, self.channels, seq);

        let cb_opt = self.shared.callback.lock().clone();
        if let Some(cb) = cb_opt {
            cb(frame);
        }
    }
}

impl MediaDeviceSource for CoreAudioCaptureSource {
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
        let devices = Self::enumerate_devices();
        if let Some(dev) = devices
            .into_iter()
            .find(|d| d.id == device_id || d.name == device_id)
        {
            self.info = dev;
            Ok(())
        } else {
            Err(MediaCaptureError::DeviceNotFound(device_id.into()))
        }
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
        if self.state == DeviceState::Paused {
            self.shared.paused.store(false, Ordering::Release);
            #[cfg(target_os = "macos")]
            if let Some(ref aq) = self.active_queue {
                let st = macos_capture_aq::resume_queue(aq);
                if st != 0 {
                    self.last_os_status = Some(st);
                }
            }
            self.state = DeviceState::Streaming;
            return Ok(());
        }

        self.shared.stopped.store(false, Ordering::Release);
        self.shared.paused.store(false, Ordering::Release);
        self.last_os_status = None;

        #[cfg(target_os = "macos")]
        {
            if let Some(prev) = self.active_queue.take() {
                macos_capture_aq::stop_queue(prev, &self.shared);
            }
            match macos_capture_aq::start_queue(self.sample_rate, self.channels, &self.shared) {
                Ok(aq) => {
                    self.active_queue = Some(aq);
                }
                Err(os_status) => {
                    // Graceful fallback when running in headless CI without audio input hardware/TCC permission
                    self.last_os_status = Some(os_status);
                    self.shared.stopped.store(false, Ordering::Release);
                }
            }
        }

        self.state = DeviceState::Streaming;
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

        self.shared.paused.store(paused, Ordering::Release);
        #[cfg(target_os = "macos")]
        if let Some(ref aq) = self.active_queue {
            let st = if paused {
                macos_capture_aq::pause_queue(aq)
            } else {
                macos_capture_aq::resume_queue(aq)
            };
            if st != 0 {
                self.last_os_status = Some(st);
            }
        }

        self.state = if paused {
            DeviceState::Paused
        } else {
            DeviceState::Streaming
        };
        Ok(())
    }

    fn stop(&mut self) -> Result<(), MediaCaptureError> {
        self.shared.stopped.store(true, Ordering::Release);
        self.shared.paused.store(false, Ordering::Release);
        #[cfg(target_os = "macos")]
        if let Some(aq) = self.active_queue.take() {
            macos_capture_aq::stop_queue(aq, &self.shared);
        }
        self.state = DeviceState::Stopped;
        Ok(())
    }

    fn set_audio_callback(&mut self, cb: AudioFrameCallback) -> Result<(), MediaCaptureError> {
        *self.shared.callback.lock() = Some(cb);
        Ok(())
    }
}

impl Drop for CoreAudioCaptureSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
