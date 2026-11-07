//! Subsystem Integration & Unit Test Suite for Module 09 Slice 2:
//! Media Streaming Instance, Chunk Cache, Demuxers & Video Clip Readers

use gpui_telegram_media::media::clip::*;
use gpui_telegram_media::media::streaming::*;

#[test]
fn test_priority_queue_ordering_and_tiebreaking() {
    let mut pq = PriorityQueue::new();
    assert!(pq.is_empty());

    // Add elements with default priority (0)
    pq.add(100);
    pq.add(50);
    pq.add(200);

    // Tiebreaker: lower offset value first
    assert_eq!(pq.take(), Some(50));
    assert_eq!(pq.take(), Some(100));

    // Increase priority and add new element
    pq.reset_priorities();
    pq.add(300); // priority 1
    pq.add(25);  // priority 1

    // Higher priority (1) should come before priority 0 (200)
    assert_eq!(pq.take(), Some(25));
    assert_eq!(pq.take(), Some(300));
    assert_eq!(pq.take(), Some(200));
    assert!(pq.is_empty());
}

#[test]
fn test_priority_queue_range_extraction() {
    let mut pq = PriorityQueue::new();
    pq.add(0);
    pq.add(128 * 1024);
    pq.add(256 * 1024);
    pq.add(384 * 1024);
    pq.add(512 * 1024);

    let extracted = pq.take_in_range(128 * 1024, 384 * 1024);
    assert_eq!(extracted, vec![128 * 1024, 256 * 1024]);
    assert_eq!(pq.len(), 3);
    assert_eq!(pq.front(), Some(0));
}

#[test]
fn test_loaded_part_validation() {
    let file_size = 300 * 1024; // 300 KB total file size

    // Full 128 KB part is valid
    let full_part = LoadedPart {
        offset: 0,
        bytes: vec![0u8; K_PART_SIZE],
    };
    assert!(full_part.is_valid(file_size));

    // Tail part reaching exact file_size (300KB - 256KB = 44KB) is valid
    let tail_part = LoadedPart {
        offset: 256 * 1024,
        bytes: vec![0u8; 44 * 1024],
    };
    assert!(tail_part.is_valid(file_size));

    // Incomplete middle part is invalid
    let incomplete_part = LoadedPart {
        offset: 0,
        bytes: vec![0u8; 64 * 1024],
    };
    assert!(!incomplete_part.is_valid(file_size));

    // Failed part is invalid
    let failed = LoadedPart::failed();
    assert!(!failed.is_valid(file_size));
}

#[test]
fn test_local_loader_progressive_read() {
    let sample_data = vec![42u8; 250 * 1024]; // 250 KB
    let mut loader = MediaStreamingLoaderLocalStreaming::from_bytes(sample_data);

    assert_eq!(loader.size(), 250 * 1024);

    // Read first 128 KB chunk
    let part1 = loader.read_part(0);
    assert!(part1.is_valid(250 * 1024));
    assert_eq!(part1.bytes.len(), K_PART_SIZE);
    assert_eq!(part1.bytes[0], 42);

    // Read second chunk (tail of 122 KB)
    let part2 = loader.read_part(K_PART_SIZE as i64);
    assert!(part2.is_valid(250 * 1024));
    assert_eq!(part2.bytes.len(), (250 - 128) * 1024);

    // Out of bounds read returns failed part
    let oob = loader.read_part(300 * 1024);
    assert!(!oob.is_valid(250 * 1024));
}

#[test]
fn test_mtproto_loader_speed_estimation() {
    let now = 10000;
    let stats = vec![
        StatsEntry {
            start: 8000,
            end: Some(8500), // 500ms, 128KB
            offset: 0,
        },
        StatsEntry {
            start: 8500,
            end: Some(9000), // 500ms, 128KB
            offset: 128 * 1024,
        },
        StatsEntry {
            start: 9000,
            end: Some(9500), // 500ms, 128KB
            offset: 256 * 1024,
        },
    ];

    // Total received = 3 * 128KB = 384KB over 1500ms
    // Speed = 384 * 1024 * 1000 / 1500 = 262,144 B/s (256 KB/s)
    let speed = compute_speed_estimate(&stats, now);
    assert_eq!(speed.bytes_per_second, 262144);
    assert!(!speed.unreliable); // 3 parts received -> reliable!
}

#[test]
fn test_streaming_reader_slice_math_and_lru_cache() {
    let total_size = 20 * 1024 * 1024; // 20 MB file
    assert_eq!(slices_count(total_size), 3); // 8MB + 8MB + 4MB = 3 slices
    assert_eq!(slice_index_for_offset(0), 0);
    assert_eq!(slice_index_for_offset(8 * 1024 * 1024), 1);
    assert_eq!(slice_index_for_offset(16 * 1024 * 1024), 2);

    let mut reader = MediaStreamingReaderStreaming::new(total_size);

    // Insert parts across slices
    reader.insert_part(0, vec![1u8; K_PART_SIZE]);
    reader.insert_part(K_IN_SLICE, vec![2u8; K_PART_SIZE]);
    assert_eq!(reader.active_slices.len(), 2);

    // Insert into a third slice -> causes eviction of slice 0
    reader.insert_part(K_IN_SLICE * 2, vec![3u8; K_PART_SIZE]);
    assert_eq!(reader.active_slices.len(), 2);
    assert!(!reader.active_slices.contains(&0)); // Slice 0 evicted
    assert!(reader.active_slices.contains(&1));
    assert!(reader.active_slices.contains(&2));
}

#[test]
fn test_streaming_reader_preload_window() {
    let total_size = 5 * 1024 * 1024; // 5 MB
    let mut reader = MediaStreamingReaderStreaming::new(total_size);

    let needed = reader.preload_offsets_ahead(0);
    assert_eq!(needed.len(), K_PRELOAD_PARTS_AHEAD);
    assert_eq!(needed[0], K_PART_SIZE);
    assert_eq!(needed[1], K_PART_SIZE * 2);

    // Insert part 1
    reader.insert_part(K_PART_SIZE, vec![0u8; K_PART_SIZE]);
    let needed_after = reader.preload_offsets_ahead(0);
    assert!(!needed_after.contains(&K_PART_SIZE)); // Part 1 already cached
}

#[test]
fn test_mp4_fast_start_moov_atom_validation() {
    // Construct fake MP4 with ftyp then moov within 128KB
    let mut data = Vec::new();
    // ftyp atom (size 24)
    data.extend_from_slice(&24u32.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    data.extend_from_slice(&[0u8; 16]);

    // moov atom (size 100)
    data.extend_from_slice(&100u32.to_be_bytes());
    data.extend_from_slice(b"moov");
    data.extend_from_slice(&[0u8; 92]);

    assert!(check_streaming_support(&data));

    // If moov is placed after 128 KB
    let mut far_data = Vec::new();
    far_data.extend_from_slice(&((130 * 1024) as u32).to_be_bytes());
    far_data.extend_from_slice(b"mdat");
    far_data.resize(130 * 1024, 0);
    far_data.extend_from_slice(&100u32.to_be_bytes());
    far_data.extend_from_slice(b"moov");

    assert!(!check_streaming_support(&far_data));
}

#[test]
fn test_video_frame_resize_and_expand_decisions() {
    let outer = (1920, 1080);
    let original = (1280, 720); // Exact 16:9 aspect ratio

    let decision = decide_video_frame_resize(outer, original);
    assert_eq!(decision.result, (1920, 1080));
    assert!(decision.expanding);

    // Portrait video in landscape outer container
    let portrait = (720, 1280);
    let portrait_dec = decide_video_frame_resize(outer, portrait);
    assert_eq!(portrait_dec.result.1, 1080);
    assert!(!portrait_dec.expanding); // Needs letterboxing
}

#[test]
fn test_clip_reader_triple_buffer_fsm() {
    let mut clip = MediaClipReaderClip::new(320, 240, 5000, 0);
    assert!(clip.is_started());

    clip.start();
    assert_eq!(clip.step, 0);

    // Show initial frame
    assert_eq!(show_frame_index(clip.step), 0);
    assert_eq!(write_frame_index(clip.step), 1);

    // Background thread writes next frame to slot 1
    clip.move_to_next_write(40, 320, 240);
    assert_eq!(clip.frames[1].position_ms, 40);

    // UI advances to display next frame
    assert!(clip.move_to_next_show());
    assert_eq!(clip.step, 1);
    assert_eq!(show_frame_index(clip.step), 1);
    assert_eq!(write_frame_index(clip.step), 2);
}

#[test]
fn test_adaptive_bitrate_switching_logic() {
    let mut doc = MediaStreamingDocumentStreaming::new(1001, 5 * 1024 * 1024); // 5 MB (720p)
    doc.information.video.state.duration = 60_000; // 60s
    doc.information.video.state.position = 10_000; // 10s
    doc.information.video.state.received_till = 20_000; // 20s (ahead by 10s > 4s threshold)

    doc.other_qualities = vec![
        QualityDescriptor {
            size_in_bytes: 2 * 1024 * 1024,
            height: 480,
        },
        QualityDescriptor {
            size_in_bytes: 15 * 1024 * 1024, // 15 MB (1080p)
            height: 1080,
        },
    ];

    // High download speed (1 MB/s) allows upscaling to 1080p
    doc.last_speed_estimate = SpeedEstimate {
        bytes_per_second: 1024 * 1024,
        unreliable: false,
    };
    assert!(doc.check_switch_to_higher_quality());
    assert_eq!(doc.requested_quality_height, Some(1080));

    // Buffer underrun triggers downscale to 480p
    doc.set_waiting(true);
    assert_eq!(doc.requested_quality_height, Some(480));
}

#[test]
fn test_playback_options_and_buffering_thresholds() {
    let options = PlaybackOptions {
        speed: 3.5,   // Over max -> clamp to 2.0
        volume: -0.2, // Under min -> clamp to 0.0
        ..Default::default()
    };
    let clean = options.sanitized();
    assert_eq!(clean.speed, 2.0);
    assert_eq!(clean.volume, 0.0);

    let remote_player = MediaStreamingPlayerStreaming::new(true);
    assert_eq!(remote_player.load_in_advance_for(), 32000);

    let local_player = MediaStreamingPlayerStreaming::new(false);
    assert_eq!(local_player.load_in_advance_for(), 5000);
}
