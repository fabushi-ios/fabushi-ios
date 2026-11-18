use gpui_telegram_media::capture::{
    AudioFrame, CoreAudioCaptureSource, DeviceState, MediaDeviceKind, MediaDeviceSource,
    MockAudioSource, MockScreenSource, MockVideoSource, VideoFrame, VideoPixelFormat,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn test_mock_audio_source_sine_generation_and_rms() {
    let mut source = MockAudioSource::new_sine(440.0, 48_000, 1);
    assert_eq!(source.kind(), MediaDeviceKind::AudioInput);
    assert_eq!(source.state(), DeviceState::Uninitialized);

    // 20ms at 48kHz mono = 960 samples
    let frame = source.next_frame(20);
    assert_eq!(frame.samples.len(), 960);
    assert_eq!(frame.sample_rate, 48_000);
    assert_eq!(frame.channels, 1);
    assert_eq!(frame.duration_ms(), 20);
    assert_eq!(frame.sequence, 0);

    // RMS level for 440Hz sine wave with peak 16384 should be > 5000
    let rms = frame.rms_level();
    assert!(rms > 5000, "RMS level was {}", rms);

    // Next frame sequence increment
    let frame2 = source.next_frame(20);
    assert_eq!(frame2.sequence, 1);
    assert_eq!(frame2.samples.len(), 960);
}

#[test]
fn test_mock_audio_source_silence() {
    let mut source = MockAudioSource::new_silence(48_000, 1);
    let frame = source.next_frame(20);
    assert_eq!(frame.samples.len(), 960);
    assert_eq!(frame.rms_level(), 0);
    assert!(frame.samples.iter().all(|&s| s == 0));
}

#[test]
fn test_mock_video_source_frame_dimensions_and_bouncing_box() {
    let mut source = MockVideoSource::new(320, 240, 30);
    assert_eq!(source.kind(), MediaDeviceKind::VideoInput);

    let frame = source.next_frame();
    assert_eq!(frame.width, 320);
    assert_eq!(frame.height, 240);
    assert_eq!(frame.format, VideoPixelFormat::Rgba8);
    assert_eq!(frame.data.len(), 320 * 240 * 4);
    assert_eq!(frame.sequence, 0);

    // Next frame
    let frame2 = source.next_frame();
    assert_eq!(frame2.sequence, 1);
    assert_ne!(frame.data, frame2.data, "Frames must vary over time");
}

#[test]
fn test_video_frame_rgba8_to_i420_conversion() {
    let mut source = MockVideoSource::new(64, 48, 30);
    let frame = source.next_frame();

    let i420 = frame.to_i420();
    let expected_len = (64 * 48) + 2 * (32 * 24);
    assert_eq!(i420.len(), expected_len);
}

#[test]
fn test_mock_screen_source_cursor_tracking() {
    let mut source = MockScreenSource::new(640, 480, 30);
    assert_eq!(source.kind(), MediaDeviceKind::ScreenCapture);

    let frame = source.next_frame();
    assert_eq!(frame.video_frame.width, 640);
    assert_eq!(frame.video_frame.height, 480);
    assert_eq!(frame.display_id, 1);
    assert!(frame.cursor_pos.is_some());

    let (c1_x, c1_y) = frame.cursor_pos.unwrap();
    let frame2 = source.next_frame();
    let (c2_x, c2_y) = frame2.cursor_pos.unwrap();
    assert!(c1_x != c2_x || c1_y != c2_y, "Cursor must track position");
}

#[test]
fn test_coreaudio_capture_source_device_selection_and_lifecycle() {
    let mut source = CoreAudioCaptureSource::new(48_000, 1);
    assert_eq!(source.kind(), MediaDeviceKind::AudioInput);
    assert_eq!(source.state(), DeviceState::Uninitialized);

    let devices = CoreAudioCaptureSource::enumerate_devices();
    assert!(!devices.is_empty(), "Should enumerate at least default device");

    let first_id = devices[0].id.clone();
    assert!(source.select_device(&first_id).is_ok());

    assert!(source.start().is_ok());
    assert_eq!(source.state(), DeviceState::Streaming);
    assert!(source.is_streaming());

    let delivered_count = Arc::new(AtomicUsize::new(0));
    let del_clone = delivered_count.clone();
    assert!(source
        .set_audio_callback(Arc::new(move |frame| {
            del_clone.fetch_add(frame.samples.len(), Ordering::SeqCst);
        }))
        .is_ok());

    // Deliver raw samples
    let samples = vec![500i16; 960];
    source.deliver_raw_samples(&samples);
    assert_eq!(delivered_count.load(Ordering::SeqCst), 960);

    // Pause
    assert!(source.pause(true).is_ok());
    assert_eq!(source.state(), DeviceState::Paused);
    assert!(source.is_paused());

    // When paused, deliver_raw_samples does not invoke callback
    source.deliver_raw_samples(&samples);
    assert_eq!(delivered_count.load(Ordering::SeqCst), 960);

    // Stop
    assert!(source.stop().is_ok());
    assert_eq!(source.state(), DeviceState::Stopped);
}

#[test]
fn test_mock_audio_streaming_thread_callback() {
    let mut source = MockAudioSource::new_sine(440.0, 48_000, 1);
    let frame_counter = Arc::new(AtomicUsize::new(0));
    let fc_clone = frame_counter.clone();

    source
        .set_audio_callback(Arc::new(move |_frame| {
            fc_clone.fetch_add(1, Ordering::SeqCst);
        }))
        .expect("Callback setup should succeed");

    source.start().expect("Start streaming should succeed");
    std::thread::sleep(std::time::Duration::from_millis(70));
    source.stop().expect("Stop streaming should succeed");

    let count = frame_counter.load(Ordering::SeqCst);
    assert!(count >= 2, "Expected at least 2 frames in 70ms, got {}", count);
}

#[test]
fn test_video_frame_odd_dimensions_and_short_buffers_no_panic() {
    for &(w, h) in &[(1u32, 1u32), (3, 3), (17, 15), (1921, 1081)] {
        let rgba = vec![0x7Fu8; (w as usize) * (h as usize) * 4];
        let frame = VideoFrame::new_rgba8(w, h, rgba, 0);
        let i420 = frame.to_i420();
        let expected_i420 = (w as usize) * (h as usize) + 2 * w.div_ceil(2) as usize * h.div_ceil(2) as usize;
        assert_eq!(i420.len(), expected_i420, "Unexpected I420 length for {}x{}", w, h);

        let i420_frame = VideoFrame {
            width: w,
            height: h,
            format: VideoPixelFormat::I420,
            data: i420,
            stride_y: w as usize,
            stride_uv: w.div_ceil(2) as usize,
            timestamp_us: 0,
            sequence: 0,
        };
        let back_rgba = i420_frame.to_rgba8();
        assert_eq!(back_rgba.len(), (w as usize) * (h as usize) * 4);

        let nv12_len = (w as usize) * (h as usize) + (w.div_ceil(2) as usize * 2) * h.div_ceil(2) as usize;
        let nv12_frame = VideoFrame {
            width: w,
            height: h,
            format: VideoPixelFormat::Nv12,
            data: vec![128u8; nv12_len],
            stride_y: w as usize,
            stride_uv: w.div_ceil(2) as usize * 2,
            timestamp_us: 0,
            sequence: 0,
        };
        assert_eq!(nv12_frame.to_rgba8().len(), (w as usize) * (h as usize) * 4);
    }

    // Truncated / empty / overflowing buffers must safely return empty Vec without panicking
    let empty_frame = VideoFrame::new_rgba8(640, 480, vec![], 0);
    assert!(empty_frame.to_i420().is_empty());
    assert!(empty_frame.to_rgba8().is_empty());

    let short_frame = VideoFrame::new_rgba8(32, 32, vec![0u8; 10], 0);
    assert!(short_frame.to_i420().is_empty());
    assert!(short_frame.to_rgba8().is_empty());

    let overflow_frame = VideoFrame::new_rgba8(u32::MAX, 2, vec![0u8; 8], 0);
    assert!(overflow_frame.to_i420().is_empty());
    assert!(overflow_frame.to_rgba8().is_empty());
}

#[test]
fn test_mock_sources_pause_resume_and_drop_lifecycle() {
    // 1. MockAudioSource pause halts callbacks & resume/start does not leak threads & drop stops thread
    let audio_count = Arc::new(AtomicUsize::new(0));
    {
        let mut audio = MockAudioSource::new_sine(440.0, 48_000, 1);
        let c = audio_count.clone();
        audio
            .set_audio_callback(Arc::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
        audio.start().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(65));
        assert!(audio_count.load(Ordering::SeqCst) >= 2);

        audio.pause(true).unwrap();
        assert_eq!(audio.state(), DeviceState::Paused);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let count_at_pause = audio_count.load(Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(90));
        assert_eq!(
            audio_count.load(Ordering::SeqCst),
            count_at_pause,
            "Paused MockAudioSource must not emit frames"
        );

        // Resuming via start() while Paused must resume cleanly
        audio.start().unwrap();
        assert_eq!(audio.state(), DeviceState::Streaming);
        std::thread::sleep(std::time::Duration::from_millis(65));
        assert!(audio_count.load(Ordering::SeqCst) > count_at_pause);
        // Drop without calling stop()
    }
    let count_after_drop = audio_count.load(Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(65));
    assert_eq!(
        audio_count.load(Ordering::SeqCst),
        count_after_drop,
        "Dropped MockAudioSource must stop emitting frames"
    );

    // 2. MockVideoSource pause & drop
    let video_count = Arc::new(AtomicUsize::new(0));
    {
        let mut video = MockVideoSource::new(32, 32, 30);
        let c = video_count.clone();
        video
            .set_video_callback(Arc::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
        video.start().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(80));
        video.pause(true).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(45));
        let paused_snap = video_count.load(Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(90));
        assert_eq!(
            video_count.load(Ordering::SeqCst),
            paused_snap,
            "Paused MockVideoSource must not emit frames"
        );
    }
    let video_drop_snap = video_count.load(Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(80));
    assert_eq!(video_count.load(Ordering::SeqCst), video_drop_snap);

    // 3. MockScreenSource pause & drop
    let screen_count = Arc::new(AtomicUsize::new(0));
    {
        let mut screen = MockScreenSource::new(32, 32, 30);
        let c = screen_count.clone();
        screen
            .set_screen_callback(Arc::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
        screen.start().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(80));
        screen.pause(true).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(45));
        let paused_snap = screen_count.load(Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(90));
        assert_eq!(
            screen_count.load(Ordering::SeqCst),
            paused_snap,
            "Paused MockScreenSource must not emit frames"
        );
    }
    let screen_drop_snap = screen_count.load(Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(80));
    assert_eq!(screen_count.load(Ordering::SeqCst), screen_drop_snap);
}
