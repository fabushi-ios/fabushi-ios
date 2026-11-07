//! Module 09 Subsystem Test Suite: Media Root Infrastructure & Theme Subsystems
//! Covers:
//! - FFmpeg byte IO wraps (ReadFileWrap, ReadBytesWrap, WriteBytesWrap)
//! - FFmpeg utility (rotation normalization, aspect ratio math, pts/time conversion, premultiply)
//! - Audio waveform synthesizer (fine span compacting, bar count, peak floor)
//! - Video canvas & encoding parameters (color adaptation, bitrate target, duration trim)
//! - System media controls manager & video delegate state machines
//! - Media player, stories, and view theme stylesheets

use gpui_telegram_media::media::ffmpeg_bytes_io_wrap::*;
use gpui_telegram_media::media::ffmpeg_frame_generator::*;
use gpui_telegram_media::media::ffmpeg_utility::*;
use gpui_telegram_media::media::media_audio_waveform::*;
use gpui_telegram_media::media::media_common::*;
use gpui_telegram_media::media::media_video_canvas::*;
use gpui_telegram_media::media::media_video_encode::*;
use gpui_telegram_media::media::media_video_frames::*;
use gpui_telegram_media::media::system_media_controls_manager::*;
use gpui_telegram_media::media::system_media_controls_video::*;

#[test]
fn test_read_bytes_wrap_read_and_seek() {
    let payload = vec![10, 20, 30, 40, 50, 60, 70, 80];
    let mut reader = ReadBytesWrap::new(payload);
    assert_eq!(reader.size(), 8);

    let mut buf = [0u8; 4];
    let read_count = reader.read(&mut buf);
    assert_eq!(read_count, 4);
    assert_eq!(buf, [10, 20, 30, 40]);
    assert_eq!(reader.offset, 4);

    // Seek to position 2 from beginning
    let pos = reader.seek(2, SEEK_SET);
    assert_eq!(pos, 2);
    let mut buf2 = [0u8; 2];
    reader.read(&mut buf2);
    assert_eq!(buf2, [30, 40]);

    // Seek size
    let size = reader.seek(0, AVSEEK_SIZE);
    assert_eq!(size, 8);

    // Read to EOF
    reader.seek(8, SEEK_SET);
    let eof = reader.read(&mut buf);
    assert_eq!(eof, AVERROR_EOF);
}

#[test]
fn test_write_bytes_wrap_chunked_reserve_and_overwrite() {
    let mut writer = WriteBytesWrap::new();
    let initial = b"Hello, World!";
    let written = writer.write(initial);
    assert_eq!(written, initial.len() as i32);
    assert_eq!(writer.size(), 13);
    assert!(writer.content.capacity() >= K_WRITE_RESERVE_CHUNK);

    // Seek back to overwrite "World" with "GPUI "
    writer.seek(7, SEEK_SET);
    writer.write(b"GPUI ");
    assert_eq!(writer.content.as_slice(), b"Hello, GPUI !");

    // Seek end and append
    writer.seek(0, SEEK_END);
    writer.write(b" 2026");
    assert_eq!(writer.content.as_slice(), b"Hello, GPUI ! 2026");
}

#[test]
fn test_ffmpeg_utility_rotation_and_transposition() {
    assert_eq!(normalize_rotation(0), 0);
    assert_eq!(normalize_rotation(90), 90);
    assert_eq!(normalize_rotation(450), 90);
    assert_eq!(normalize_rotation(-90), 270);
    assert_eq!(normalize_rotation(45), 0); // Non-orthogonal clamped to 0

    assert!(rotation_swap_width_height(90));
    assert!(rotation_swap_width_height(270));
    assert!(!rotation_swap_width_height(0));
    assert!(!rotation_swap_width_height(180));

    let (w, h) = transpose_size_by_rotation(1920, 1080, 90);
    assert_eq!((w, h), (1080, 1920));
    let (w2, h2) = transpose_size_by_rotation(1920, 1080, 180);
    assert_eq!((w2, h2), (1920, 1080));
}

#[test]
fn test_ffmpeg_utility_timestamp_math() {
    // 1 second with time base 1/1000
    let pts = 1000;
    let time_ms = pts_to_time(pts, 1, 1000);
    assert_eq!(time_ms, 1000);

    let roundtrip_pts = time_to_pts(time_ms, 1, 1000);
    assert_eq!(roundtrip_pts, pts);

    // Ceil test with fractional pts
    let time_ceil = pts_to_time_ceil(1001, 1, 1000);
    assert_eq!(time_ceil, 1001);
}

#[test]
fn test_ffmpeg_utility_pixel_premultiplication() {
    // Fully opaque: unmodified
    let (r, g, b, a) = premultiply_pixel(200, 150, 100, 255);
    assert_eq!((r, g, b, a), (200, 150, 100, 255));

    // Fully transparent: zeroed
    let (r0, g0, b0, a0) = premultiply_pixel(200, 150, 100, 0);
    assert_eq!((r0, g0, b0, a0), (0, 0, 0, 0));

    // 50% opacity (~128/255)
    let (rp, gp, bp, ap) = premultiply_pixel(200, 100, 50, 128);
    assert!(rp < 200 && gp < 100 && bp < 50);
    assert_eq!(ap, 128);

    // Un-premultiply roundtrip
    let (ru, gu, _bu, au) = unpremultiply_pixel(rp, gp, bp, ap);
    assert!((ru as i32 - 200).abs() <= 2);
    assert!((gu as i32 - 100).abs() <= 2);
    assert_eq!(au, 128);
}

#[test]
fn test_frame_generator_aspect_ratio_scaling() {
    let generator = FfmpegFrameGenerator::new(vec![]);
    // 16:9 source (1920x1080) to 1:1 box (500x500) KeepAspectRatio
    let (w, h, dx, dy) = generator.compute_scaled_size(
        1920,
        1080,
        500,
        500,
        AspectRatioMode::KeepAspectRatio,
    );
    assert_eq!(w, 500);
    assert_eq!(h, 281);
    assert_eq!(dx, 0);
    assert_eq!(dy, (500 - 281) / 2);

    // KeepAspectRatioByExpanding
    let (ew, eh, _edx, _edy) = generator.compute_scaled_size(
        1920,
        1080,
        500,
        500,
        AspectRatioMode::KeepAspectRatioByExpanding,
    );
    assert!(ew >= 500 && eh >= 500);
}

#[test]
fn test_waveform_synthesis_algorithm() {
    // Generate synthetic sine-wave 16-bit PCM (48kHz, 1000 samples)
    let mut pcm = Vec::with_capacity(4800);
    for i in 0..4800 {
        let sample = ((i as f64 * 0.1).sin() * 15000.0) as i16;
        pcm.push(sample);
    }

    let waveform = compute_waveform_from_pcm(&pcm, 48000, 100);
    assert_eq!(waveform.count(), 100);
    assert!(!waveform.empty());
    // Minimum peak floor is 2500
    assert!(waveform.peak >= K_MINIMAL_PEAK);
    for &bar in &waveform.bars {
        assert!(bar <= waveform.peak);
    }
}

#[test]
fn test_media_common_speeds_and_frame_validation() {
    assert!(equal_speeds(1.7, 1.74));
    assert!(!equal_speeds(1.7, 1.81));
    assert!(equal_speeds(0.5, 0.52));

    assert!(valid_frame_size(1920, 1080, 1920 * 1080 * 4));
    assert!(!valid_frame_size(1920, 1080, 1000));
    assert!(!valid_frame_size(-10, 1080, 1000000));

    let mut common = MediaCommon::new();
    common.set_speed(1.7);
    assert!(common.is_sped_up());
    common.set_speed(5.0); // Clamped to K_SPEED_MAX = 2.5
    assert_eq!(common.speed, K_SPEED_MAX);
}

#[test]
fn test_video_canvas_color_adaptation() {
    let raw = RgbColor::new(100, 150, 200);
    let adapted = adapt_color(raw);
    assert!(adapted.r > 0 || adapted.g > 0 || adapted.b > 0);

    let bg = dominant_canvas_background(
        RgbColor::new(20, 40, 80),
        RgbColor::new(180, 120, 50),
    );
    assert!(bg.valid());
}

#[test]
fn test_video_encode_bitrate_and_trim() {
    // 1080p @ 30fps
    let bitrate_1080p = target_bitrate(1920, 1080, 30.0);
    assert!(bitrate_1080p >= K_MIN_BITRATE);
    assert!(bitrate_1080p <= K_MAX_BITRATE);

    // Downscaling: 1920x1080 downscaled to shorter side 720
    let (dw, dh) = downscaled_size(1920, 1080, 720);
    assert_eq!(dh, 720);
    assert_eq!(dw, 1280);

    // Duration trimming
    let dur = transcoded_duration(5000, 15000, 30000);
    assert_eq!(dur, 10000);
}

#[test]
fn test_system_media_controls_lifecycle_and_skip() {
    let mut manager = SystemMediaControlsManager::new();
    manager.set_track("Blinding Lights", "The Weeknd", 200_000);
    assert_eq!(manager.track.title, "Blinding Lights");
    assert_eq!(manager.track.duration_ms, 200_000);

    manager.set_playback_status(PlaybackStatus::Playing);
    assert_eq!(manager.status, PlaybackStatus::Playing);

    manager.set_repeat_mode(RepeatMode::One);
    assert_eq!(manager.loop_status, LoopStatus::Track);

    // Skip forward +15s
    manager.skip_forward();
    assert_eq!(manager.track.position_ms, K_SKIP_MS);

    // Skip backward -15s
    manager.skip_backward();
    assert_eq!(manager.track.position_ms, 0);

    // Video state sink integration
    let video_state = VideoState::new("Sample Video", "Author", 60_000);
    manager.video_start(video_state.clone());
    assert!(manager.active_video.is_some());
    manager.video_finish();
    assert!(manager.active_video.is_none());
}

#[test]
fn test_media_video_frames_cover_crop() {
    // 16:9 frame to square 200x200 cover
    let (x, y, w, h) = compute_cover_crop(1920, 1080, 200, 200);
    assert_eq!((w, h), (200, 200));
    assert!(x > 0);
    assert_eq!(y, 0);

    let info = FileInfo::new(1920, 1080, 60_000);
    assert!(info.valid());
}
