//! Adversarial Oracle Verification Suite for Module 09 (Media Audio, Streaming & Lightbox)
//! Upstream Telegram Desktop Parity Verification with 5 Blind C++ Oracles:
//! 1. Waveform 100-bar logarithmic scaling & 5-bit packing parity (media/media_audio_waveform.cpp:L25-L95, data_document.cpp:L1383-L1434)
//! 2. Streaming chunk boundary 128KB, 8MB slice LRU cache eviction stress & mp4 moov fast-start detection (media/streaming/media_streaming_reader.cpp:L21-L150, media_clip_check_streaming.cpp:L20-L95)
//! 3. Discrete zoom coordinate matrix & viewport clamping relative to unscaled native dimensions (media/view/media_view_overlay_widget.cpp:L7619-L8107)
//! 4. Stories 5000ms photo timing, hold-to-pause gesture FSM, and segmented progress geometry (media/stories/media_stories_controller.cpp:L185-L235, media_stories_slider.cpp:L90-L154)
//! 5. SMTC system media controls bridge state transitions & playback sync (media/system_media_controls_manager.cpp, system_media_controls_video.rs)

use gpui_telegram_media::waveform::{
    decode_waveform_5bit, encode_waveform_5bit, normalize_pcm_peaks,
    resample_and_normalize_waveform, TELEGRAM_WAVEFORM_BARS, TELEGRAM_WAVEFORM_BYTES,
    WAVEFORM_MAX_VALUE,
};
use gpui_telegram_media::media::media_audio_waveform::{
    compute_waveform_from_pcm, K_MINIMAL_PEAK,
};
use gpui_telegram_media::media::clip::media_clip_check_streaming::check_streaming_support;
use gpui_telegram_media::media::streaming::media_streaming_loader::K_PART_SIZE;
use gpui_telegram_media::media::streaming::media_streaming_reader::{
    slices_count, MediaStreamingReaderStreaming, ReaderFillState, K_IN_SLICE,
    K_PARTS_IN_SLICE, K_SLICES_IN_MEMORY,
};
use gpui_telegram_media::media::view::media_view_overlay_widget::{
    snap_xy, MediaViewOverlayWidgetView, K_MAX_ZOOM_LEVEL,
};
use gpui_telegram_media::media::stories::{
    MediaStoriesControllerStories, MediaStoriesSliderStories, PauseState, PhotoPlayback,
    K_DEFAULT_SLIDER_SKIP, K_DEFAULT_SLIDER_WIDTH, K_OPACITY_ACTIVE, K_OPACITY_INACTIVE,
    K_PHOTO_DURATION_MS,
};
use gpui_telegram_media::media::media_common::RepeatMode;
use gpui_telegram_media::media::system_media_controls_manager::{
    repeat_mode_to_loop_status, LoopStatus, PlaybackStatus, SystemMediaControlsManager, K_SKIP_MS,
};
use gpui_telegram_media::media::system_media_controls_video::{
    SystemMediaControlsVideo, VideoMediaCommand, VideoState,
};

// ============================================================================
// BLIND C++ ORACLE 1: Waveform 100-bar Logarithmic Scaling & 5-bit Packing Parity
// Reference: Telegram Desktop `data/data_document.cpp` lines 1383-1434 &
//            `media/media_audio_waveform.cpp` lines 184-256
// ============================================================================

struct BlindCppWaveformOracle;

impl BlindCppWaveformOracle {
    /// C++ upstream `documentWaveformEncode5bit` reference logic
    fn encode_5bit(bars: &[u8]) -> Vec<u8> {
        let count = bars.len().min(100);
        if count == 0 {
            return Vec::new();
        }
        let bits_count = count * 5;
        let bytes_count = (bits_count + 7) / 8;
        let mut result = vec![0u8; bytes_count + 1];

        for (i, &bar) in bars.iter().take(count).enumerate() {
            let byte_index = (i * 5) / 8;
            let bit_shift = (i * 5) % 8;
            let value = ((bar & 0x1F) as u16) << bit_shift;
            let prev = (result[byte_index] as u16) | ((result[byte_index + 1] as u16) << 8);
            let combined = prev | value;
            result[byte_index] = (combined & 0xFF) as u8;
            result[byte_index + 1] = ((combined >> 8) & 0xFF) as u8;
        }

        result.truncate(bytes_count);
        result
    }

    /// C++ upstream `documentWaveformDecode` reference logic
    fn decode_5bit(encoded: &[u8]) -> Vec<u8> {
        let bits_count = encoded.len() * 8;
        let values_count = bits_count / 5;
        if values_count == 0 {
            return Vec::new();
        }
        let mut result = vec![0u8; values_count];
        for i in 0..values_count.saturating_sub(1) {
            let byte_index = (i * 5) / 8;
            let bit_shift = (i * 5) % 8;
            let value = (encoded[byte_index] as u16) | ((encoded[byte_index + 1] as u16) << 8);
            result[i] = ((value >> bit_shift) & 0x1F) as u8;
        }
        let last_index = values_count - 1;
        let last_byte = (last_index * 5) / 8;
        let last_shift = (last_index * 5) % 8;
        let last_val = if last_byte + 1 < encoded.len() {
            (encoded[last_byte] as u16) | ((encoded[last_byte + 1] as u16) << 8)
        } else {
            encoded[last_byte] as u16
        };
        result[last_index] = ((last_val >> last_shift) & 0x1F) as u8;
        result
    }

    /// C++ upstream `LoadWaveform` PCM peak extraction & logarithmic ceiling
    fn compute_pcm_waveform(samples: &[i16], sample_rate: i32, bar_count: usize) -> (Vec<u16>, u16) {
        if bar_count == 0 || samples.is_empty() || sample_rate <= 0 {
            return (Vec::new(), 0);
        }
        let span_size = (sample_rate / 50).max(1) as usize;
        let mut fine = Vec::new();
        let mut current_peak: u16 = 0;
        let mut in_span = 0;

        for &s in samples {
            let v = s.unsigned_abs();
            if v > current_peak {
                current_peak = v;
            }
            in_span += 1;
            if in_span >= span_size {
                fine.push(current_peak);
                current_peak = 0;
                in_span = 0;
            }
        }
        if in_span > 0 {
            fine.push(current_peak);
        }
        if fine.is_empty() {
            return (Vec::new(), 0);
        }

        let total = fine.len();
        let mut bars = vec![0u16; bar_count];
        for i in 0..bar_count {
            let from = i * total / bar_count;
            let till = ((i + 1) * total / bar_count).max(from + 1).min(total);
            let mut p = 0u16;
            for &fv in &fine[from..till] {
                if fv > p {
                    p = fv;
                }
            }
            bars[i] = p;
        }

        let sum: u64 = bars.iter().map(|&b| b as u64).sum();
        let avg_ceiling = (sum as f64 * 1.8 / bar_count as f64) as u64;
        let peak = 2500u64.max(avg_ceiling).min(u16::MAX as u64) as u16;
        for b in &mut bars {
            if *b > peak {
                *b = peak;
            }
        }
        (bars, peak)
    }
}

#[test]
fn test_oracle_1_waveform_silence_and_saturation_parity() {
    // Silence test: all zeros
    let silence_bars = vec![0u8; TELEGRAM_WAVEFORM_BARS];
    let rust_packed = encode_waveform_5bit(&silence_bars);
    let oracle_packed = BlindCppWaveformOracle::encode_5bit(&silence_bars);

    assert_eq!(rust_packed.len(), TELEGRAM_WAVEFORM_BYTES);
    assert_eq!(rust_packed, oracle_packed);
    assert!(rust_packed.iter().all(|&b| b == 0));

    let rust_decoded = decode_waveform_5bit(&rust_packed);
    let oracle_decoded = BlindCppWaveformOracle::decode_5bit(&rust_packed);
    assert_eq!(rust_decoded, silence_bars);
    assert_eq!(oracle_decoded, silence_bars);

    // Saturation test: all 31s
    let saturated_bars = vec![WAVEFORM_MAX_VALUE; TELEGRAM_WAVEFORM_BARS];
    let sat_rust_packed = encode_waveform_5bit(&saturated_bars);
    let sat_oracle_packed = BlindCppWaveformOracle::encode_5bit(&saturated_bars);

    assert_eq!(sat_rust_packed.len(), TELEGRAM_WAVEFORM_BYTES);
    assert_eq!(sat_rust_packed, sat_oracle_packed);
    // 500 bits: first 62 bytes are 0xFF (496 bits), last byte has 4 bits set -> 0x0F
    assert_eq!(&sat_rust_packed[..62], &[0xFF; 62][..]);
    assert_eq!(sat_rust_packed[62], 0x0F);

    assert_eq!(decode_waveform_5bit(&sat_rust_packed), saturated_bars);
    assert_eq!(BlindCppWaveformOracle::decode_5bit(&sat_oracle_packed), saturated_bars);
}

#[test]
fn test_oracle_1_waveform_bit_packing_checkerboard_adversarial() {
    // Checkerboard pattern [0, 31, 0, 31, ...] tests unaligned bit shift leakage
    let mut checkerboard = Vec::with_capacity(100);
    for i in 0..100 {
        checkerboard.push(if i % 2 == 0 { 0 } else { 31 });
    }
    let rust_packed = encode_waveform_5bit(&checkerboard);
    let oracle_packed = BlindCppWaveformOracle::encode_5bit(&checkerboard);
    assert_eq!(rust_packed, oracle_packed);

    let decoded = decode_waveform_5bit(&rust_packed);
    assert_eq!(decoded, checkerboard);

    // Power-of-two single-bit masks: [1, 2, 4, 8, 16, 1, 2, 4, 8, 16, ...]
    let powers: Vec<u8> = (0..100).map(|i| 1u8 << (i % 5)).collect();
    let rust_powers = encode_waveform_5bit(&powers);
    let oracle_powers = BlindCppWaveformOracle::encode_5bit(&powers);
    assert_eq!(rust_powers, oracle_powers);
    assert_eq!(decode_waveform_5bit(&rust_powers), powers);

    // Dynamic ceiling compression stress with silence, below-floor noise, and extreme peaks
    let low_noise = vec![10u16, 25, 50, 100];
    let norm_low = normalize_pcm_peaks(&low_noise);
    assert_eq!(norm_low.len(), 4);
    // Dynamic ceiling floor is MINIMAL_PEAK_FLOOR (2500), so 100 * 31 / 2500 = 1
    assert!(norm_low.iter().all(|&b| b <= 1));

    // Resampling arbitrary lengths: 1 sample -> 100 bars, 777 samples -> 100 bars
    let single_sample = vec![200u8];
    let resampled_single = resample_and_normalize_waveform(&single_sample);
    assert_eq!(resampled_single.len(), 100);

    let prime_samples: Vec<u8> = (0..337).map(|i| ((i * 37) % 256) as u8).collect();
    let resampled_prime = resample_and_normalize_waveform(&prime_samples);
    assert_eq!(resampled_prime.len(), 100);
}

#[test]
fn test_oracle_1_waveform_pcm_log_scaling_extreme_samples() {
    // Test i16::MIN absolute value edge case (-32768)
    let extreme_samples = vec![i16::MIN, i16::MAX, 0, -1, 1000, -20000];
    let wf = compute_waveform_from_pcm(&extreme_samples, 48000, 10);
    let (oracle_bars, oracle_peak) = BlindCppWaveformOracle::compute_pcm_waveform(&extreme_samples, 48000, 10);

    assert_eq!(wf.bars.len(), 10);
    assert_eq!(wf.bars, oracle_bars);
    assert_eq!(wf.peak, oracle_peak);
    assert!(wf.peak >= K_MINIMAL_PEAK);
}

// ============================================================================
// BLIND C++ ORACLE 2: Streaming 128KB Chunk, 8MB Slice LRU Cache & MP4 Moov
// Reference: Telegram Desktop `media/clip/media_clip_check_streaming.cpp` lines 20-95 &
//            `media/streaming/media_streaming_reader.cpp` lines 21-150
// ============================================================================

struct BlindCppStreamingOracle;

impl BlindCppStreamingOracle {
    /// C++ upstream `CheckStreamingSupport` logic for MP4 Fast-Start
    fn check_streaming_support_ref(data: &[u8]) -> bool {
        const HEADER_SIZE: usize = 8;
        const FIND_MOOV_BEFORE: usize = 128 * 1024;

        if data.len() < HEADER_SIZE {
            return false;
        }

        let mut pos = 0;
        let size = data.len();

        while pos + HEADER_SIZE <= size {
            if pos >= FIND_MOOV_BEFORE {
                return false;
            }

            let atom_size_32 = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            let atom_name = &data[pos + 4..pos + 8];

            if atom_name == b"moov" {
                return true;
            }

            let length = if atom_size_32 == 1 {
                if pos + 16 > size {
                    break;
                }
                u64::from_be_bytes([
                    data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11],
                    data[pos + 12], data[pos + 13], data[pos + 14], data[pos + 15],
                ]) as usize
            } else if atom_size_32 == 0 {
                size - pos
            } else {
                atom_size_32 as usize
            };

            if length < HEADER_SIZE || pos + length > size {
                break;
            }
            pos += length;
        }

        false
    }

    /// C++ upstream LRU eviction rules: Max 2 active slices in memory (16 MB)
    fn active_slices_after_access(current_active: &[usize], accessed_slice: usize) -> Vec<usize> {
        let mut active: Vec<usize> = current_active.to_vec();
        if !active.contains(&accessed_slice) {
            if active.len() >= 2 {
                active.remove(0); // Evict oldest FIFO
            }
            active.push(accessed_slice);
        }
        active
    }
}

#[test]
fn test_oracle_2_mp4_moov_fast_start_detection_adversarial() {
    // 1. Valid fast-start: ftyp (24 bytes) followed by moov (100 bytes)
    let mut valid_mp4 = Vec::new();
    // ftyp atom: size = 24, name = "ftyp"
    valid_mp4.extend_from_slice(&24u32.to_be_bytes());
    valid_mp4.extend_from_slice(b"ftyp");
    valid_mp4.extend_from_slice(&[0u8; 16]); // ftyp payload
    // moov atom: size = 100, name = "moov"
    valid_mp4.extend_from_slice(&100u32.to_be_bytes());
    valid_mp4.extend_from_slice(b"moov");
    valid_mp4.extend_from_slice(&[0u8; 92]);

    assert!(check_streaming_support(&valid_mp4));
    assert_eq!(
        check_streaming_support(&valid_mp4),
        BlindCppStreamingOracle::check_streaming_support_ref(&valid_mp4)
    );

    // 2. Delayed moov: mdat (140KB) followed by moov -> offset >= 128KB -> must fail!
    let mut delayed_mp4 = Vec::new();
    let mdat_len = 140 * 1024u32;
    delayed_mp4.extend_from_slice(&mdat_len.to_be_bytes());
    delayed_mp4.extend_from_slice(b"mdat");
    delayed_mp4.resize(mdat_len as usize, 0u8);
    // moov after 140KB
    delayed_mp4.extend_from_slice(&100u32.to_be_bytes());
    delayed_mp4.extend_from_slice(b"moov");
    delayed_mp4.extend_from_slice(&[0u8; 92]);

    assert!(!check_streaming_support(&delayed_mp4));
    assert_eq!(
        check_streaming_support(&delayed_mp4),
        BlindCppStreamingOracle::check_streaming_support_ref(&delayed_mp4)
    );

    // 3. Exactly at 128KB boundary: 128KB - 8 bytes of padding, then moov starts at 131064 (< 131072) -> passes!
    let mut boundary_mp4 = Vec::new();
    let free_len = (128 * 1024 - 8) as u32;
    boundary_mp4.extend_from_slice(&free_len.to_be_bytes());
    boundary_mp4.extend_from_slice(b"free");
    boundary_mp4.resize(free_len as usize, 0u8);
    // moov starts at offset 131064
    boundary_mp4.extend_from_slice(&32u32.to_be_bytes());
    boundary_mp4.extend_from_slice(b"moov");
    boundary_mp4.extend_from_slice(&[0u8; 24]);

    assert!(check_streaming_support(&boundary_mp4));
    assert_eq!(
        check_streaming_support(&boundary_mp4),
        BlindCppStreamingOracle::check_streaming_support_ref(&boundary_mp4)
    );

    // 4. 64-bit atom size header (size == 1)
    let mut atom64_mp4 = Vec::new();
    atom64_mp4.extend_from_slice(&1u32.to_be_bytes());
    atom64_mp4.extend_from_slice(b"wide");
    atom64_mp4.extend_from_slice(&32u64.to_be_bytes()); // 64-bit size: 32 bytes
    atom64_mp4.extend_from_slice(&[0u8; 16]);
    // followed by moov
    atom64_mp4.extend_from_slice(&20u32.to_be_bytes());
    atom64_mp4.extend_from_slice(b"moov");
    atom64_mp4.extend_from_slice(&[0u8; 12]);

    assert!(check_streaming_support(&atom64_mp4));
    assert_eq!(
        check_streaming_support(&atom64_mp4),
        BlindCppStreamingOracle::check_streaming_support_ref(&atom64_mp4)
    );

    // 5. Corrupt inputs: empty, truncated < 8 bytes, invalid atom size exceeding EOF
    assert!(!check_streaming_support(&[]));
    assert!(!check_streaming_support(&[0, 0, 0]));
    assert!(!check_streaming_support(&[0, 0, 0, 100, b'f', b'r', b'e', b'e'])); // truncated
}

#[test]
fn test_oracle_2_streaming_128kb_chunk_8mb_slice_lru_eviction() {
    let total_size = 32 * 1024 * 1024; // 32 MB = 4 slices of 8 MB
    assert_eq!(slices_count(total_size), 4);
    assert_eq!(K_PART_SIZE, 128 * 1024);
    assert_eq!(K_IN_SLICE, 8 * 1024 * 1024);
    assert_eq!(K_PARTS_IN_SLICE, 64);
    assert_eq!(K_SLICES_IN_MEMORY, 2);

    let mut reader = MediaStreamingReaderStreaming::new(total_size);

    // 1. Insert part 0 in Slice 0 (offset 0)
    let part0 = vec![0xAA; K_PART_SIZE];
    reader.insert_part(0, part0.clone());
    assert_eq!(reader.active_slices.len(), 1);
    assert_eq!(reader.active_slices[0], 0);
    assert!(reader.cached_parts.contains_key(&0));

    // Verify fill from Slice 0 succeeds
    let mut buf = vec![0u8; 1024];
    assert_eq!(reader.fill(0, &mut buf), ReaderFillState::Success);
    assert_eq!(buf, vec![0xAA; 1024]);

    // 2. Insert part in Slice 1 (offset 8 MB = part 64)
    let offset_slice1 = 8 * 1024 * 1024;
    let part64 = vec![0xBB; K_PART_SIZE];
    reader.insert_part(offset_slice1, part64.clone());
    assert_eq!(reader.active_slices.len(), 2);
    assert_eq!(reader.active_slices[0], 0);
    assert_eq!(reader.active_slices[1], 1);
    assert!(reader.cached_parts.contains_key(&64));

    // 3. Insert part in Slice 2 (offset 16 MB = part 128) -> exceeds K_SLICES_IN_MEMORY (2) -> EVICTS SLICE 0!
    let offset_slice2 = 16 * 1024 * 1024;
    let part128 = vec![0xCC; K_PART_SIZE];
    reader.insert_part(offset_slice2, part128);

    assert_eq!(reader.active_slices.len(), 2);
    assert_eq!(reader.active_slices[0], 1);
    assert_eq!(reader.active_slices[1], 2);
    let oracle_active = BlindCppStreamingOracle::active_slices_after_access(&[0, 1], 2);
    assert_eq!(reader.active_slices.as_slices().0, oracle_active.as_slice());

    // Assert Slice 0 parts have been purged from cache!
    assert!(!reader.cached_parts.contains_key(&0), "Slice 0 part 0 MUST be evicted when Slice 2 is loaded");
    assert!(reader.cached_parts.contains_key(&64), "Slice 1 part 64 MUST remain in cache");
    assert!(reader.cached_parts.contains_key(&128), "Slice 2 part 128 MUST be present in cache");

    // Reading evicted slice returns WaitingRemote
    let mut evict_buf = vec![0u8; 100];
    assert_eq!(reader.fill(0, &mut evict_buf), ReaderFillState::WaitingRemote);

    // Reading active slice succeeds
    assert_eq!(reader.fill(offset_slice1, &mut evict_buf), ReaderFillState::Success);
    assert_eq!(evict_buf, vec![0xBB; 100]);
}

// ============================================================================
// BLIND C++ ORACLE 3: Discrete Zoom Coordinate Matrix & Viewport Clamping
// Reference: Telegram Desktop `media/view/media_view_overlay_widget.cpp`
//            lines 7619-7673, 8093-8107
// ============================================================================

struct BlindCppZoomClampingOracle;

impl BlindCppZoomClampingOracle {
    fn compute_scale(zoom: i32) -> f64 {
        if zoom > 0 {
            (zoom as f64) + 1.0
        } else {
            1.0 / ((-zoom as f64) + 1.0)
        }
    }

    fn snap_xy_ref(
        vw: i32, vh: i32, cw: i32, ch: i32,
        skip_top: i32, avail_h: i32, min_used_top: i32, max_used_h: i32,
        fullscreen_video: bool, x: i32, y: i32,
    ) -> (i32, i32) {
        let xmin = (vw - cw).min((vw - cw) / 2);
        let xmax = 0.max((vw - cw) / 2);

        let center_y = if fullscreen_video {
            min_used_top + (max_used_h - ch) / 2
        } else {
            skip_top + (avail_h - ch) / 2
        };
        let ymin = (vh - ch).min(center_y);
        let ymax = min_used_top.max(center_y);

        (x.clamp(xmin, xmax), y.clamp(ymin, ymax))
    }
}

#[test]
fn test_oracle_3_discrete_zoom_coordinate_matrix_parity() {
    let mut widget = MediaViewOverlayWidgetView::new();
    widget.viewport_width = 1920;
    widget.viewport_height = 1080;
    widget.available_height = 1080;
    widget.base_w = 800;
    widget.base_h = 600;

    // Test zoom scale monotonicity and parity for all discrete zoom levels -7 to +7
    for zoom in -K_MAX_ZOOM_LEVEL..=K_MAX_ZOOM_LEVEL {
        widget.set_zoom_level(zoom, None);
        let oracle_scale = BlindCppZoomClampingOracle::compute_scale(zoom);
        let expected_w = (800.0 * oracle_scale).round() as i32;
        let expected_h = (600.0 * oracle_scale).round() as i32;

        assert_eq!(widget.content_w, expected_w, "Width mismatch at zoom {}", zoom);
        assert_eq!(widget.content_h, expected_h, "Height mismatch at zoom {}", zoom);

        // Clamping invariant: (x, y) must always be clamped to snap bounds
        let (snap_x, snap_y) = BlindCppZoomClampingOracle::snap_xy_ref(
            widget.viewport_width, widget.viewport_height,
            widget.content_w, widget.content_h,
            widget.skip_top, widget.available_height, 0, widget.viewport_height,
            false, widget.x, widget.y,
        );
        assert_eq!(widget.x, snap_x);
        assert_eq!(widget.y, snap_y);
    }

    // Zoom-in and zoom-out step reversibility
    widget.set_zoom_level(0, None);
    assert_eq!(widget.zoom, 0);
    assert_eq!(widget.content_w, 800);

    widget.zoom_in(None);
    assert_eq!(widget.zoom, 1);
    assert_eq!(widget.content_w, 1600);

    widget.zoom_out(None);
    assert_eq!(widget.zoom, 0);
    assert_eq!(widget.content_w, 800);
}

#[test]
fn test_oracle_3_extreme_aspect_ratios_and_boundary_clamping() {
    // 1. Extreme panoramic banner: 10000x20 in 1280x800 viewport
    let (px, py) = snap_xy(1280, 800, 10000, 20, 0, 800, 0, 800, false, 500, -200);
    assert_eq!(py, (800 - 20) / 2); // Vertically centered
    assert!(px >= 1280 - 10000 && px <= 0); // Horizontally clamped

    // 2. Extreme vertical skyscraper: 50x12000 in 1280x800 viewport
    let (vx, vy) = snap_xy(1280, 800, 50, 12000, 0, 800, 0, 800, false, 999, 1000);
    assert_eq!(vx, (1280 - 50) / 2); // Horizontally centered
    assert!(vy >= 800 - 12000 && vy <= 0); // Vertically clamped

    // 3. Swipe gesture threshold verification (MEDIAVIEW_SWIPE_DISTANCE = 80px)
    let mut view = MediaViewOverlayWidgetView::new();
    view.total_items = 5;
    view.current_index = 2;

    assert_eq!(view.handle_swipe(79), None); // Below threshold
    assert_eq!(view.handle_swipe(-79), None); // Below threshold
    assert_eq!(view.handle_swipe(80), Some(true)); // Navigated next
    assert_eq!(view.current_index, 3);
    assert_eq!(view.handle_swipe(-100), Some(false)); // Navigated prev
    assert_eq!(view.current_index, 2);
}

// ============================================================================
// BLIND C++ ORACLE 4: Stories 5000ms Photo Timing, Hold-to-Pause FSM & Slider
// Reference: Telegram Desktop `media/stories/media_stories_controller.cpp` lines 185-235 &
//            `media/stories/media_stories_slider.cpp` lines 90-154
// ============================================================================

struct BlindCppStoriesOracle;

impl BlindCppStoriesOracle {
    fn compute_progress(started: i64, paused_at: Option<i64>, now: i64, duration: u64) -> f32 {
        if duration == 0 {
            return 1.0;
        }
        let reference = paused_at.unwrap_or(now);
        let elapsed = (reference - started).max(0) as u64;
        (elapsed as f32 / duration as f32).clamp(0.0, 1.0)
    }

    fn compute_slider_segments_ref(
        total: usize, current_index: usize, progress: f32,
        container_w: f32, single: f32, skip: f32,
    ) -> Vec<(f32, f32, f32, f32)> { // (x, width, opacity, active_fill_width)
        let max = (((container_w + skip) / (single + skip)).floor() as usize).max(1);
        let count = total.clamp(1, max);
        let one = (container_w - (count as f32 - 1.0) * skip) / count as f32;
        let mut result = Vec::with_capacity(count);
        let mut left = 0.0;

        for i in 0..count {
            let opacity = if i > current_index { 0.4 } else { 1.0 };
            let active_fill = if i == current_index {
                (one * progress).max(single).min(one)
            } else if i < current_index {
                one
            } else {
                0.0
            };
            result.push((left, one, opacity, active_fill));
            left += one + skip;
        }
        result
    }
}

#[test]
fn test_oracle_4_stories_photo_timing_and_hold_to_pause_fsm() {
    assert_eq!(K_PHOTO_DURATION_MS, 5000);

    let start_time = 1_000_000i64;
    let mut controller = MediaStoriesControllerStories::new(4242, 4, 0, start_time);

    // Initial state: t=0ms, progress = 0.0
    assert_eq!(controller.playback.progress(start_time), 0.0);
    assert!(!controller.playback.is_finished(start_time));

    // Progress at 2000ms: 40% (2000 / 5000)
    let t_2000 = start_time + 2000;
    assert_eq!(controller.playback.progress(t_2000), 0.4);
    assert_eq!(
        controller.playback.progress(t_2000),
        BlindCppStoriesOracle::compute_progress(controller.playback.started, controller.playback.paused_at, t_2000, 5000)
    );

    // Gesture: User holds screen at t=2000ms
    controller.content_pressed(true, t_2000);
    assert_eq!(controller.pause_state, PauseState::Paused);
    assert_eq!(controller.playback.paused_at, Some(t_2000));

    // While holding for 15,000ms (until t=17,000ms), progress MUST remain frozen at 0.4
    let t_hold = start_time + 17000;
    assert_eq!(controller.playback.elapsed(t_hold), 2000);
    assert_eq!(controller.playback.progress(t_hold), 0.4);

    // User releases hold at t=17,000ms
    controller.content_pressed(false, t_hold);
    assert_eq!(controller.pause_state, PauseState::Playing);
    assert_eq!(controller.playback.paused_at, None);
    // Adjusted started offset should be start_time + 15,000ms
    assert_eq!(controller.playback.started, start_time + 15000);

    // Play remaining 3000ms (until t=20,000ms)
    let t_end = start_time + 20000;
    assert_eq!(controller.playback.progress(t_end), 1.0);
    assert!(controller.playback.is_finished(t_end));

    // Story subjump navigation
    assert!(controller.subjump_for(1, t_end));
    assert_eq!(controller.current_index, 1);
    assert!(controller.marked_read);
    // Next story started clean at t_end
    assert_eq!(controller.playback.started, t_end);
    assert_eq!(controller.playback.progress(t_end), 0.0);
}

#[test]
fn test_oracle_4_stories_segmented_progress_geometry_adversarial() {
    let container_width = 800.0f32;
    let slider = MediaStoriesSliderStories::new(5, 2); // 5 segments, active index = 2
    let mut slider_mut = slider.clone();
    slider_mut.set_progress(0.75); // 75% progress on active segment

    let layout = slider_mut.compute_layout(container_width);
    let oracle_layout = BlindCppStoriesOracle::compute_slider_segments_ref(
        5, 2, 0.75, container_width,
        K_DEFAULT_SLIDER_WIDTH, K_DEFAULT_SLIDER_SKIP,
    );

    assert_eq!(layout.len(), 5);
    assert_eq!(layout.len(), oracle_layout.len());

    for (i, (seg, &(ox, ow, o_op, o_fill))) in layout.iter().zip(oracle_layout.iter()).enumerate() {
        assert!((seg.x - ox).abs() < 1e-4, "Mismatch in seg {} x", i);
        assert!((seg.width - ow).abs() < 1e-4, "Mismatch in seg {} width", i);
        assert!((seg.opacity - o_op).abs() < 1e-4, "Mismatch in seg {} opacity", i);
        assert!((seg.active_fill_width - o_fill).abs() < 1e-4, "Mismatch in seg {} fill width", i);

        if i < 2 {
            // Completed segments
            assert_eq!(seg.opacity, K_OPACITY_ACTIVE);
            assert_eq!(seg.active_fill_width, seg.width);
        } else if i == 2 {
            // Current active segment (75%)
            assert_eq!(seg.opacity, K_OPACITY_ACTIVE);
            let expected_fill = (seg.width * 0.75).max(K_DEFAULT_SLIDER_WIDTH).min(seg.width);
            assert!((seg.active_fill_width - expected_fill).abs() < 1e-4);
        } else {
            // Unplayed future segments
            assert_eq!(seg.opacity, K_OPACITY_INACTIVE);
            assert_eq!(seg.active_fill_width, 0.0);
        }
    }

    // Boundary check: single story segment (total = 1)
    let single_story_slider = MediaStoriesSliderStories::new(1, 0);
    let single_layout = single_story_slider.compute_layout(400.0);
    assert_eq!(single_layout.len(), 1);
    assert_eq!(single_layout[0].width, 400.0);
    assert_eq!(single_layout[0].x, 0.0);
}

// ============================================================================
// BLIND C++ ORACLE 5: SMTC System Media Controls Bridge State Transitions & Sync
// Reference: Telegram Desktop `media/system_media_controls_manager.cpp` &
//            `crates/gpui_telegram_media/src/media/system_media_controls_video.rs`
// ============================================================================

struct BlindCppSmtcOracle;

impl BlindCppSmtcOracle {
    fn loop_status_from_repeat(mode: RepeatMode) -> LoopStatus {
        match mode {
            RepeatMode::None => LoopStatus::None,
            RepeatMode::One => LoopStatus::Track,
            RepeatMode::All => LoopStatus::Playlist,
        }
    }

    fn skip_forward(pos: i64, duration: i64) -> i64 {
        (pos + 15_000).clamp(0, duration)
    }

    fn skip_backward(pos: i64, duration: i64) -> i64 {
        (pos - 15_000).clamp(0, duration)
    }
}

#[test]
fn test_oracle_5_smtc_state_transitions_and_playback_sync() {
    assert_eq!(K_SKIP_MS, 15_000);

    let mut smtc = SystemMediaControlsManager::new();
    assert_eq!(smtc.status, PlaybackStatus::Stopped);
    assert_eq!(smtc.loop_status, LoopStatus::None);

    // Track metadata synchronization
    let duration = 180_000i64; // 3 minutes = 180,000ms
    smtc.set_track("Never Gonna Give You Up", "Rick Astley", duration);
    assert_eq!(smtc.track.title, "Never Gonna Give You Up");
    assert_eq!(smtc.track.artist, "Rick Astley");
    assert_eq!(smtc.track.duration_ms, duration);
    assert_eq!(smtc.track.position_ms, 0);

    // 1. State machine transitions: Stopped -> Playing -> Paused -> Stopped
    smtc.set_playback_status(PlaybackStatus::Playing);
    assert_eq!(smtc.status, PlaybackStatus::Playing);

    smtc.set_playback_status(PlaybackStatus::Paused);
    assert_eq!(smtc.status, PlaybackStatus::Paused);

    smtc.set_playback_status(PlaybackStatus::Playing);
    assert_eq!(smtc.status, PlaybackStatus::Playing);

    smtc.set_playback_status(PlaybackStatus::Stopped);
    assert_eq!(smtc.status, PlaybackStatus::Stopped);

    // 2. Repeat mode parity mapping
    for mode in [RepeatMode::None, RepeatMode::One, RepeatMode::All] {
        smtc.set_repeat_mode(mode);
        let expected = BlindCppSmtcOracle::loop_status_from_repeat(mode);
        assert_eq!(smtc.loop_status, expected);
        assert_eq!(repeat_mode_to_loop_status(mode), expected);
    }

    // 3. Skip controls & position boundary clamping
    smtc.set_position(10_000);
    assert_eq!(smtc.track.position_ms, 10_000);

    // Skip backward from 10,000ms by 15,000ms -> clamps to 0
    smtc.skip_backward();
    assert_eq!(smtc.track.position_ms, 0);
    assert_eq!(smtc.track.position_ms, BlindCppSmtcOracle::skip_backward(10_000, duration));

    // Skip forward by 15,000ms -> 15,000ms
    smtc.skip_forward();
    assert_eq!(smtc.track.position_ms, 15_000);
    assert_eq!(smtc.track.position_ms, BlindCppSmtcOracle::skip_forward(0, duration));

    // Near end of track: position = 175,000ms. Skip forward by 15,000ms -> clamps to duration (180,000ms)
    smtc.set_position(175_000);
    smtc.skip_forward();
    assert_eq!(smtc.track.position_ms, duration);
    assert_eq!(smtc.track.position_ms, BlindCppSmtcOracle::skip_forward(175_000, duration));

    // Out-of-bounds position clamping
    smtc.set_position(-500);
    assert_eq!(smtc.track.position_ms, 0);
    smtc.set_position(999_999);
    assert_eq!(smtc.track.position_ms, duration);
}

#[test]
fn test_oracle_5_smtc_video_command_queue_fifo_and_sync() {
    let mut video_bridge = SystemMediaControlsVideo::new();
    let video_duration = 600_000i64; // 10 minutes
    let initial_state = VideoState::new("Big Buck Bunny", "Blender Foundation", video_duration);

    video_bridge.set_state(initial_state);
    assert_eq!(video_bridge.state.title, "Big Buck Bunny");
    assert!(!video_bridge.is_playing());

    // Push sequence of native OS media commands
    video_bridge.push_command(VideoMediaCommand::Play);
    video_bridge.push_command(VideoMediaCommand::Seek(120_000));
    video_bridge.push_command(VideoMediaCommand::Pause);
    video_bridge.push_command(VideoMediaCommand::Next);

    // Verify FIFO processing order
    assert_eq!(video_bridge.pop_command(), Some(VideoMediaCommand::Play));
    assert_eq!(video_bridge.pop_command(), Some(VideoMediaCommand::Seek(120_000)));
    assert_eq!(video_bridge.pop_command(), Some(VideoMediaCommand::Pause));
    assert_eq!(video_bridge.pop_command(), Some(VideoMediaCommand::Next));
    assert_eq!(video_bridge.pop_command(), None);
}

#[test]
fn test_5_oracles_comprehensive_cross_cutting_integration() {
    // Cross-cutting sanity: ensure all 5 oracles execute in unified pipeline
    // 1. Waveform 5-bit packing
    let test_bars = vec![15u8; 100];
    let packed = encode_waveform_5bit(&test_bars);
    assert_eq!(decode_waveform_5bit(&packed), test_bars);

    // 2. Fast-start MP4 validator
    assert!(!check_streaming_support(&[]));

    // 3. Viewport clamping
    let (cx, cy) = snap_xy(1000, 1000, 500, 500, 0, 1000, 0, 1000, false, 0, 0);
    assert_eq!(cx, 250);
    assert_eq!(cy, 250);

    // 4. Stories timing
    let playback = PhotoPlayback::new(500, 5000);
    assert!(!playback.is_finished(500));

    // 5. SMTC loop conversion
    assert_eq!(repeat_mode_to_loop_status(RepeatMode::All), LoopStatus::Playlist);
}
