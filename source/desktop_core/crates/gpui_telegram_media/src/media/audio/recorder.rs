use super::media_audio_capture::{CaptureUpdate, CAPTURE_FREQUENCY};
use super::wav::{compute_pcm_rms_level, encode_wav_i16, waveform_from_pcm_i16};
use gpui_telegram_core::error::{Result, TelegramError};
#[cfg(target_os = "macos")]
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedVoiceNote {
    pub wav_bytes: Vec<u8>,
    pub pcm_samples: Vec<i16>,
    pub sample_rate: u32,
    pub duration_ms: i64,
    pub waveform_bars: Vec<u8>,
    pub waveform_5bit: Vec<u8>,
}

struct RecorderShared {
    samples: Mutex<Vec<i16>>,
    level: AtomicU16,
    stopped: AtomicBool,
}

#[cfg(target_os = "macos")]
pub(crate) mod macos_aq_ffi {
    use std::ffi::c_void;

    pub type AudioQueueRef = *mut c_void;
    pub type AudioQueueBufferRef = *mut AudioQueueBuffer;

    #[repr(C)]
    pub struct AudioStreamBasicDescription {
        pub m_sample_rate: f64,
        pub m_format_id: u32,
        pub m_format_flags: u32,
        pub m_bytes_per_packet: u32,
        pub m_frames_per_packet: u32,
        pub m_bytes_per_frame: u32,
        pub m_channels_per_frame: u32,
        pub m_bits_per_channel: u32,
        pub m_reserved: u32,
    }

    #[repr(C)]
    pub struct AudioQueueBuffer {
        pub m_audio_data_bytes_capacity: u32,
        pub m_audio_data: *mut c_void,
        pub m_audio_data_byte_size: u32,
        pub m_user_data: *mut c_void,
        pub m_packet_description_capacity: u32,
        pub m_packet_descriptions: *mut c_void,
        pub m_packet_description_count: u32,
    }

    pub type InputCb = unsafe extern "C" fn(
        *mut c_void,
        AudioQueueRef,
        AudioQueueBufferRef,
        *const c_void,
        u32,
        *const c_void,
    );

    #[link(name = "AudioToolbox", kind = "framework")]
    unsafe extern "C" {
        pub fn AudioQueueNewInput(
            in_format: *const AudioStreamBasicDescription,
            in_callback_proc: InputCb,
            in_user_data: *mut c_void,
            in_callback_run_loop: *mut c_void,
            in_callback_run_loop_mode: *const c_void,
            in_flags: u32,
            out_aq: *mut AudioQueueRef,
        ) -> i32;
        pub fn AudioQueueAllocateBuffer(
            aq: AudioQueueRef,
            bytes: u32,
            out_buf: *mut AudioQueueBufferRef,
        ) -> i32;
        pub fn AudioQueueEnqueueBuffer(
            aq: AudioQueueRef,
            buf: AudioQueueBufferRef,
            n: u32,
            d: *const c_void,
        ) -> i32;
        pub fn AudioQueueStart(aq: AudioQueueRef, start_time: *const c_void) -> i32;
        pub fn AudioQueuePause(aq: AudioQueueRef) -> i32;
        pub fn AudioQueueStop(aq: AudioQueueRef, immediate: u8) -> i32;
        pub fn AudioQueueDispose(aq: AudioQueueRef, immediate: u8) -> i32;
    }
}

#[cfg(target_os = "macos")]
mod macos_aq {
    use super::macos_aq_ffi::*;
    use super::*;
    use std::ptr;

    pub struct ActiveQueue {
        queue: AudioQueueRef,
        raw_shared: *const RecorderShared,
    }
    unsafe impl Send for ActiveQueue {}

    unsafe extern "C" fn input_callback(
        user_data: *mut c_void, aq: AudioQueueRef, buf: AudioQueueBufferRef,
        _start_time: *const c_void, _num_pkts: u32, _pkt_descs: *const c_void,
    ) {
        if user_data.is_null() || buf.is_null() { return; }
        let shared = unsafe { &*(user_data as *const RecorderShared) };
        if shared.stopped.load(Ordering::Acquire) { return; }
        let b = unsafe { &*buf };
        let byte_len = b.m_audio_data_byte_size as usize;
        if !b.m_audio_data.is_null() && byte_len >= 2 {
            let bytes = unsafe { std::slice::from_raw_parts(b.m_audio_data as *const u8, byte_len) };
            let mut chunk = Vec::with_capacity(bytes.len() / 2);
            for pair in bytes.chunks_exact(2) {
                chunk.push(i16::from_le_bytes([pair[0], pair[1]]));
            }
            shared.level.store(compute_pcm_rms_level(&chunk), Ordering::Release);
            if let Ok(mut guard) = shared.samples.lock() {
                guard.extend_from_slice(&chunk);
            }
        }
        let _ = unsafe { AudioQueueEnqueueBuffer(aq, buf, 0, ptr::null()) };
    }

    pub fn start_queue(sample_rate: u32, shared: &Arc<RecorderShared>) -> Result<ActiveQueue> {
        let format = AudioStreamBasicDescription {
            m_sample_rate: sample_rate as f64,
            m_format_id: u32::from_be_bytes(*b"lpcm"),
            m_format_flags: 12, // kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked
            m_bytes_per_packet: 2,
            m_frames_per_packet: 1,
            m_bytes_per_frame: 2,
            m_channels_per_frame: 1,
            m_bits_per_channel: 16,
            m_reserved: 0,
        };
        let raw_shared = Arc::into_raw(Arc::clone(shared));
        let mut queue: AudioQueueRef = ptr::null_mut();
        let status = unsafe {
            AudioQueueNewInput(&format, input_callback, raw_shared as *mut c_void, ptr::null_mut(), ptr::null(), 0, &mut queue)
        };
        if status != 0 || queue.is_null() {
            unsafe { drop(Arc::from_raw(raw_shared)); }
            return Err(TelegramError::PermissionDenied(format!(
                "CoreAudio AudioQueueNewInput failed (OSStatus={status})"
            )));
        }
        for _ in 0..3 {
            let mut buf: AudioQueueBufferRef = ptr::null_mut();
            let s1 = unsafe { AudioQueueAllocateBuffer(queue, 4096, &mut buf) };
            let s2 = if s1 == 0 { unsafe { AudioQueueEnqueueBuffer(queue, buf, 0, ptr::null()) } } else { s1 };
            if s2 != 0 {
                shared.stopped.store(true, Ordering::Release);
                unsafe { AudioQueueDispose(queue, 1); drop(Arc::from_raw(raw_shared)); }
                return Err(TelegramError::Internal(format!("AudioQueue buffer setup failed (OSStatus={s2})")));
            }
        }
        let s_start = unsafe { AudioQueueStart(queue, ptr::null()) };
        if s_start != 0 {
            shared.stopped.store(true, Ordering::Release);
            unsafe { AudioQueueDispose(queue, 1); drop(Arc::from_raw(raw_shared)); }
            return Err(TelegramError::PermissionDenied(format!(
                "CoreAudio AudioQueueStart failed (OSStatus={s_start})"
            )));
        }
        Ok(ActiveQueue { queue, raw_shared })
    }

    pub fn stop_queue(aq: ActiveQueue, shared: &RecorderShared) {
        shared.stopped.store(true, Ordering::Release);
        unsafe {
            AudioQueueStop(aq.queue, 1);
            AudioQueueDispose(aq.queue, 1);
            drop(Arc::from_raw(aq.raw_shared));
        }
    }
}

/// Live microphone voice note recorder capturing 48kHz mono 16-bit PCM via CoreAudio AudioQueue.
pub struct VoiceRecorder {
    sample_rate: u32,
    shared: Arc<RecorderShared>,
    recording: bool,
    #[cfg(target_os = "macos")]
    active: Option<macos_aq::ActiveQueue>,
}

impl Default for VoiceRecorder {
    fn default() -> Self { Self::new(CAPTURE_FREQUENCY as u32) }
}

impl VoiceRecorder {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(8000),
            shared: Arc::new(RecorderShared {
                samples: Mutex::new(Vec::new()),
                level: AtomicU16::new(0),
                stopped: AtomicBool::new(false),
            }),
            recording: false,
            #[cfg(target_os = "macos")]
            active: None,
        }
    }

    pub fn start(&mut self) -> Result<()> {
        if let Ok(mut guard) = self.shared.samples.lock() { guard.clear(); }
        self.shared.level.store(0, Ordering::Release);
        self.shared.stopped.store(false, Ordering::Release);
        #[cfg(target_os = "macos")]
        {
            if let Some(prev) = self.active.take() { macos_aq::stop_queue(prev, &self.shared); }
            let aq = macos_aq::start_queue(self.sample_rate, &self.shared)?;
            self.active = Some(aq);
            self.recording = true;
            return Ok(());
        }
        #[cfg(not(target_os = "macos"))]
        Err(TelegramError::Internal("Microphone capture is only implemented on macOS".into()))
    }

    pub fn feed_external_pcm_i16(&mut self, samples: &[i16]) {
        self.recording = true;
        self.shared.level.store(compute_pcm_rms_level(samples), Ordering::Release);
        if let Ok(mut guard) = self.shared.samples.lock() { guard.extend_from_slice(samples); }
    }

    pub fn is_recording(&self) -> bool { self.recording }
    pub fn current_level(&self) -> u16 { self.shared.level.load(Ordering::Acquire) }

    pub fn poll_update(&self) -> CaptureUpdate {
        let len = self.shared.samples.lock().map(|g| g.len() as i32).unwrap_or(0);
        CaptureUpdate { samples: len, level: self.current_level(), finished: !self.recording }
    }

    pub fn stop(&mut self) -> Result<RecordedVoiceNote> {
        #[cfg(target_os = "macos")]
        if let Some(aq) = self.active.take() { macos_aq::stop_queue(aq, &self.shared); }
        self.recording = false;
        let pcm_samples = self.shared.samples.lock().map(|g| g.clone()).unwrap_or_default();
        let duration_ms = ((pcm_samples.len() as i128 * 1000) / self.sample_rate as i128) as i64;
        let wav_bytes = encode_wav_i16(&pcm_samples, self.sample_rate, 1)?;
        let (waveform_bars, waveform_5bit) = waveform_from_pcm_i16(&pcm_samples);
        Ok(RecordedVoiceNote {
            wav_bytes,
            pcm_samples,
            sample_rate: self.sample_rate,
            duration_ms,
            waveform_bars,
            waveform_5bit,
        })
    }
}

impl Drop for VoiceRecorder {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(aq) = self.active.take() { macos_aq::stop_queue(aq, &self.shared); }
    }
}

#[cfg(target_os = "macos")]
mod macos_devices {
    use std::ffi::c_void;

    #[repr(C)]
    struct AudioObjectPropertyAddress {
        m_selector: u32,
        m_scope: u32,
        m_element: u32,
    }

    #[repr(C)]
    struct CFRange {
        location: isize,
        length: isize,
    }

    type CFStringRef = *const c_void;

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyDataSize(
            in_object_id: u32,
            in_address: *const AudioObjectPropertyAddress,
            in_qualifier_data_size: u32,
            in_qualifier_data: *const c_void,
            out_data_size: *mut u32,
        ) -> i32;
        fn AudioObjectGetPropertyData(
            in_object_id: u32,
            in_address: *const AudioObjectPropertyAddress,
            in_qualifier_data_size: u32,
            in_qualifier_data: *const c_void,
            io_data_size: *mut u32,
            out_data: *mut c_void,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetLength(the_string: CFStringRef) -> isize;
        fn CFStringGetCharacters(the_string: CFStringRef, range: CFRange, buffer: *mut u16);
        fn CFRelease(cf: *const c_void);
    }

    fn device_stream_bytes(dev_id: u32, scope: u32) -> u32 {
        let addr = AudioObjectPropertyAddress {
            m_selector: u32::from_be_bytes(*b"stm#"),
            m_scope: scope,
            m_element: 0,
        };
        let mut size = 0u32;
        let st = unsafe {
            AudioObjectGetPropertyDataSize(dev_id, &addr, 0, std::ptr::null(), &mut size)
        };
        if st == 0 { size } else { 0 }
    }

    fn device_name(dev_id: u32) -> Option<String> {
        let addr = AudioObjectPropertyAddress {
            m_selector: u32::from_be_bytes(*b"lnam"),
            m_scope: u32::from_be_bytes(*b"glob"),
            m_element: 0,
        };
        let mut cf_str: CFStringRef = std::ptr::null();
        let mut size = std::mem::size_of::<CFStringRef>() as u32;
        let st = unsafe {
            AudioObjectGetPropertyData(
                dev_id,
                &addr,
                0,
                std::ptr::null(),
                &mut size,
                (&mut cf_str as *mut CFStringRef).cast::<c_void>(),
            )
        };
        if st != 0 || cf_str.is_null() {
            return None;
        }
        let len = unsafe { CFStringGetLength(cf_str) };
        let name = if len > 0 && len < 512 {
            let mut utf16 = vec![0u16; len as usize];
            unsafe {
                CFStringGetCharacters(
                    cf_str,
                    CFRange { location: 0, length: len },
                    utf16.as_mut_ptr(),
                );
            }
            String::from_utf16_lossy(&utf16).trim().to_string()
        } else {
            String::new()
        };
        unsafe { CFRelease(cf_str) };
        if name.is_empty() { None } else { Some(name) }
    }

    pub fn query_host_devices() -> (Vec<String>, Vec<String>) {
        let sys_obj = 1u32;
        let addr = AudioObjectPropertyAddress {
            m_selector: u32::from_be_bytes(*b"dev#"),
            m_scope: u32::from_be_bytes(*b"glob"),
            m_element: 0,
        };
        let mut byte_size = 0u32;
        let st = unsafe {
            AudioObjectGetPropertyDataSize(sys_obj, &addr, 0, std::ptr::null(), &mut byte_size)
        };
        if st != 0 || byte_size < 4 {
            return (Vec::new(), Vec::new());
        }
        let count = (byte_size as usize) / std::mem::size_of::<u32>();
        let mut ids = vec![0u32; count];
        let st2 = unsafe {
            AudioObjectGetPropertyData(
                sys_obj,
                &addr,
                0,
                std::ptr::null(),
                &mut byte_size,
                ids.as_mut_ptr().cast::<c_void>(),
            )
        };
        if st2 != 0 {
            return (Vec::new(), Vec::new());
        }
        let in_scope = u32::from_be_bytes(*b"inpt");
        let out_scope = u32::from_be_bytes(*b"outp");
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for id in ids {
            if id == 0 {
                continue;
            }
            let has_in = device_stream_bytes(id, in_scope) > 0;
            let has_out = device_stream_bytes(id, out_scope) > 0;
            if !has_in && !has_out {
                continue;
            }
            if let Some(name) = device_name(id) {
                if has_in && !inputs.contains(&name) {
                    inputs.push(name.clone());
                }
                if has_out && !outputs.contains(&name) {
                    outputs.push(name);
                }
            }
        }
        (inputs, outputs)
    }
}

/// Enumerates real host audio input (microphone) and output (speaker/headphone) devices,
/// with graceful fallback defaults when running in headless CI without audio hardware.
pub fn enumerate_audio_devices() -> (Vec<String>, Vec<String>) {
    #[cfg(target_os = "macos")]
    let (mut inputs, mut outputs) = macos_devices::query_host_devices();
    #[cfg(not(target_os = "macos"))]
    let (mut inputs, mut outputs): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());

    if inputs.is_empty() {
        inputs.push("System Default Microphone".to_string());
    }
    if outputs.is_empty() {
        outputs.push("System Default Output".to_string());
    }
    (inputs, outputs)
}
