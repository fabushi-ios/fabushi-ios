//! Subsystem Integration & Unit Test Suite for Module 09 Slice 1
//! Scope: Audio playback engine, track states, waveforms, capture & player controls
//! Verifies: Mandates #11, #15, #17, #18, #19

use gpui_telegram_media::media::audio::media_audio::*;
use gpui_telegram_media::media::audio::media_audio_capture::*;
use gpui_telegram_media::media::audio::media_audio_capture_common::*;
use gpui_telegram_media::media::audio::media_audio_edit::*;
use gpui_telegram_media::media::audio::media_audio_ffmpeg_loader::*;
use gpui_telegram_media::media::audio::media_audio_loader::*;
use gpui_telegram_media::media::audio::media_audio_loaders::*;
use gpui_telegram_media::media::audio::media_audio_local_cache::*;
use gpui_telegram_media::media::audio::media_audio_track::*;
use gpui_telegram_media::media::audio::media_child_ffmpeg_loader::*;

use gpui_telegram_media::media::player::media_player_button::*;
use gpui_telegram_media::media::player::media_player_dropdown::*;
use gpui_telegram_media::media::player::media_player_float::*;
use gpui_telegram_media::media::player::media_player_instance::*;
use gpui_telegram_media::media::player::media_player_listen_tracker::*;
use gpui_telegram_media::media::player::media_player_panel::*;
use gpui_telegram_media::media::player::media_player_volume_controller::*;
use gpui_telegram_media::media::player::media_player_widget::*;

#[test]
fn test_player_state_transitions_and_queries() {
    let stopped = PlayerState::Stopped;
    assert!(stopped.is_stopped());
    assert!(stopped.is_stopped_or_stopping());
    assert!(!stopped.is_active());
    assert!(!stopped.show_pause_icon());

    let playing = PlayerState::Playing;
    assert!(!playing.is_stopped());
    assert!(!playing.is_paused());
    assert!(playing.is_active());
    assert!(playing.show_pause_icon());

    let pausing = PlayerState::Pausing;
    assert!(pausing.is_paused_or_pausing());
    assert!(pausing.is_fading());
    assert!(!pausing.show_pause_icon());

    let paused = PlayerState::Paused;
    assert!(paused.is_paused());
    assert!(!paused.is_active());
}

#[test]
fn test_track_state_speed_position_math() {
    // 48000 samples at 1.0x speed = 48000 samples
    let normal = speed_independent_position(48000, 1.0);
    assert_eq!(normal, 48000);

    // At 2.0x speed: 48000 samples elapsed -> 96000 speed-independent samples
    let fast = speed_independent_position(48000, 2.0);
    assert_eq!(fast, 96000);

    // Dependent position at 2.0x speed: 96000 -> 48000
    let dep = speed_dependent_position(96000, 2.0);
    assert_eq!(dep, 48000);

    // Duration ms calculation
    let state = TrackState {
        frequency: 48000,
        length_samples: 96000,
        position_samples: 48000,
        ..Default::default()
    };
    assert_eq!(state.duration_ms(), 2000);
    assert_eq!(state.position_ms(), 1000);
}

#[test]
fn test_cubic_volume_computation() {
    // Gain 0.0 -> 0.0
    assert!((compute_volume_cubic(0.0) - 0.0).abs() < 1e-6);
    // Gain 1.0 -> 1.0
    assert!((compute_volume_cubic(1.0) - 1.0).abs() < 1e-6);
    // Gain 0.5 -> 0.125 (0.5^3)
    assert!((compute_volume_cubic(0.5) - 0.125).abs() < 1e-6);
    // Clamping: > 1.0 clamps to 1.0, < 0.0 clamps to 0.0
    assert_eq!(compute_volume_cubic(1.5), 1.0);
    assert_eq!(compute_volume_cubic(-0.5), 0.0);
}

#[test]
fn test_waveform_100_bars_calculation() {
    // Exactly 100 bars generated
    let samples: Vec<i16> = (0..4800).map(|i| ((i % 100) * 300) as i16).collect();
    let waveform = calculate_voice_waveform_100_bars(&samples);
    assert_eq!(waveform.len(), 100);

    // All bar heights must be 5-bit (0..31)
    for &bar in &waveform {
        assert!(bar <= 31);
    }

    // Empty samples generates 100 zero bars
    let empty_wf = calculate_voice_waveform_100_bars(&[]);
    assert_eq!(empty_wf.len(), 100);
    assert!(empty_wf.iter().all(|&b| b == 0));
}

#[test]
fn test_audio_speed_clamp_and_snapping() {
    // Snapping near 1.0 within threshold 0.05
    let speed_near_1 = slider_value_to_speed(speed_to_slider_value(1.02));
    assert_eq!(speed_near_1, 1.0);

    // Snapping near 1.5
    let speed_near_15 = slider_value_to_speed(speed_to_slider_value(1.48));
    assert_eq!(speed_near_15, 1.5);

    // Bounds clamping
    assert_eq!(slider_value_to_speed(0.0), 0.5);
    assert_eq!(slider_value_to_speed(1.0), 2.5);
}

#[test]
fn test_audio_capture_waveform_collector() {
    let raw: Vec<u8> = (0..500).map(|i| (i % 256) as u8).collect();
    let wf = collect_waveform(&raw);
    assert_eq!(wf.len(), 100);
    for &b in &wf {
        assert!(b <= 31);
    }
}


#[test]
fn test_audio_edit_trim_and_concat_validation() {
    assert!(validate_trim_range(100, 0, 500));
    assert!(!validate_trim_range(0, 0, 500)); // empty content
    assert!(!validate_trim_range(100, -10, 500)); // negative from
    assert!(!validate_trim_range(100, 500, 500)); // till <= from
    assert!(!validate_trim_range(100, 600, 500)); // till < from

    let content = vec![1, 2, 3, 4];
    let trimmed = trim_audio_to_range(&content, 100, 400);
    assert_eq!(trimmed.duration_ms, 300);

    let concatenated = concat_audio(&[1, 2], &[3, 4]);
    assert_eq!(concatenated.content, vec![1, 2, 3, 4]);
}

#[test]
fn test_media_player_button_speed_label_formatting() {
    assert_eq!(format_speed_text(1.0), "1X");
    assert_eq!(format_speed_text(1.5), "1.5X");
    assert_eq!(format_speed_text(2.0), "2X");
    assert_eq!(format_speed_text(0.8), "0.8X");

    let mut button = MediaPlayerButtonPlayer::new();
    assert_eq!(button.state, PlayButtonState::Play);
    button.toggle_play_pause();
    assert_eq!(button.state, PlayButtonState::Pause);
    button.toggle_play_pause();
    assert_eq!(button.state, PlayButtonState::Play);
}

#[test]
fn test_music_listen_tracker_duration_filtering() {
    let mut tracker = MediaPlayerListenTrackerPlayer::new();
    tracker.start(1001);

    // 2 seconds listened: below 3s threshold, report() should return None
    tracker.update_progress(2000);
    assert!(!tracker.should_report());
    assert_eq!(tracker.finalize(), None);
    assert_eq!(tracker.report_count, 0);

    // 5 seconds listened: exceeds 3s threshold, report() should return Some(5)
    tracker.start(1001);
    tracker.update_progress(5000);
    assert!(tracker.should_report());
    assert_eq!(tracker.finalize(), Some(5));
    assert_eq!(tracker.report_count, 1);
    assert_eq!(tracker.last_reported_seconds, 5);
}

#[test]
fn test_volume_controller_remembered_state() {
    let mut vol = MediaPlayerVolumeControllerPlayer::new();
    assert_eq!(vol.volume, 1.0);
    assert!(!vol.is_muted);

    // Mute toggles volume to 0 and saves 1.0
    vol.toggle_mute();
    assert_eq!(vol.volume, 0.0);
    assert!(vol.is_muted);

    // Unmute restores saved volume
    vol.toggle_mute();
    assert_eq!(vol.volume, 1.0);
    assert!(!vol.is_muted);

    // Wheel adjustment
    vol.handle_wheel_delta(-2); // -0.10
    assert!((vol.volume - 0.90).abs() < 1e-6);
}

#[test]
fn test_playlist_shuffle_and_repeat_modes() {
    let mut instance = MediaPlayerInstancePlayer::new();
    instance.playlist = vec![101, 102, 103];
    instance.playlist_index = 0;
    instance.current_track_id = 101;

    // Normal progression
    assert!(instance.next());
    assert_eq!(instance.current_track_id, 102);
    assert!(instance.next());
    assert_eq!(instance.current_track_id, 103);
    // At end without repeat: stops
    assert!(!instance.next());

    // With RepeatMode::All: loops to beginning
    instance.repeat_mode = PlayerRepeatMode::All;
    instance.playlist_index = 2;
    assert!(instance.next());
    assert_eq!(instance.playlist_index, 0);
    assert_eq!(instance.current_track_id, 101);
}

#[test]
fn test_round_video_float_bounds_and_aspect() {
    let mut float_player = MediaPlayerFloatPlayer::new();
    float_player.show_video_note(555);
    assert!(float_player.is_visible);
    assert_eq!(float_player.message_id, 555);

    // Dragging within 800x600 screen bounds
    float_player.drag_to(500, 700, 800, 600);
    assert_eq!(float_player.position.x, 500); // 500 <= 800-160 (640)
    assert_eq!(float_player.position.y, 440); // clamped to 600-160 = 440
}

#[test]
fn test_audio_loaders_and_cache() {
    let mut loader = MediaAudioLoaderAudio::new();
    assert_eq!(loader.buffer_size(), 96000); // 48000 * 2
    loader.save_decoded_samples(vec![1, 2, 3]);
    assert!(loader.holds_saved_samples);
    let taken = loader.take_saved_decoded_samples();
    assert_eq!(taken, vec![1, 2, 3]);
    assert!(!loader.holds_saved_samples);

    let mut cache = MediaAudioLocalCacheAudio::with_disk_path("/tmp/cache".to_string());
    cache.insert(42, vec![10, 20]);
    assert_eq!(cache.get(42), Some(&[10, 20][..]));
    assert_eq!(cache.disk_path_for_id(42), "/tmp/cache/42.wav");

    let mut track = MediaAudioTrackAudio::new();
    track.sample_peak_each(100);
    track.peaks = vec![16384, 32767];
    assert!((track.get_peak_value(50) - 0.5).abs() < 0.01);
    assert!((track.get_peak_value(150) - 1.0).abs() < 0.01);

    let mut ffmpeg = MediaAudioFfmpegLoaderAudio::new();
    assert!(ffmpeg.open(44100, 2000, 500));
    assert_eq!(ffmpeg.total_samples(), 88200);

    let mut child = MediaChildFfmpegLoaderAudio::new();
    child.enqueue_packets(5);
    assert_eq!(child.queued_packets, 5);

    let mut multi = MediaAudioLoadersAudio::new();
    multi.start_loading(LoaderTrackType::Song, 77);
    assert!(multi.is_loading);
    multi.clear(LoaderTrackType::Song);
    assert!(!multi.is_loading);

    let common = MediaAudioCaptureCommonAudio::with_result(vec![1], vec![2], 1500, false);
    assert!((common.duration_seconds() - 1.5).abs() < 1e-6);
    assert!(!common.is_empty());
}

#[test]
fn test_player_panel_and_widget_states() {
    let mut panel = MediaPlayerPanelPlayer::new();
    panel.set_items(vec![
        PlaylistItem {
            id: 1,
            title: "Track 1".to_string(),
            performer: "Artist 1".to_string(),
            duration_seconds: 180,
        },
        PlaylistItem {
            id: 2,
            title: "Track 2".to_string(),
            performer: "Artist 2".to_string(),
            duration_seconds: 240,
        },
    ]);
    assert_eq!(panel.items.len(), 2);
    assert_eq!(panel.current_item().unwrap().title, "Track 1");
    assert!(panel.select_index(1));
    assert_eq!(panel.current_item().unwrap().title, "Track 2");

    let mut widget = MediaPlayerWidgetPlayer::new();
    widget.set_track("My Song".to_string(), "Singer".to_string(), 120_000);
    assert!(widget.is_visible);
    widget.current_position_ms = 60_000;
    assert!((widget.progress_ratio() - 0.5).abs() < 1e-6);
    assert_eq!(widget.formatted_time(), "01:00 / 02:00");

    assert_eq!(widget.cycle_repeat_state(), PlayerRepeatState::All);
    assert_eq!(widget.cycle_order_state(), PlayerOrderState::Reverse);
}
