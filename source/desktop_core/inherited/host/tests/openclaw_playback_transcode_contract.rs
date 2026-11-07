use std::path::{Path, PathBuf};
use mahayana_host_runtime::extensions::attachments::media_probe::PlaybackMediaKind;
use mahayana_host_runtime::extensions::attachments::video_playback_rendition::{
    build_ffmpeg_playback_args, create_playback_transcode_cache_key,
    playback_cache_relative_path, playback_durations_match, replace_playback_file_extension,
    PlaybackTranscodeResolution, PLAYBACK_TRANSCODE_CACHE_VERSION, PLAYBACK_TRANSCODE_SUBDIR,
};

#[test]
fn test_playback_transcode_deterministic_cache_and_policy() {
    let key1 = create_playback_transcode_cache_key("/path/to/media.mp4", 1024, 1000, 2000, 1, 42);
    let key2 = create_playback_transcode_cache_key("/path/to/media.mp4", 1024, 1000, 2000, 1, 42);
    let key3 = create_playback_transcode_cache_key("/path/to/media.mp4", 1025, 1000, 2000, 1, 42);
    assert_eq!(key1, key2);
    assert_ne!(key1, key3);

    let rel_path = playback_cache_relative_path(&key1, ".mp4");
    assert_eq!(
        rel_path,
        format!("{}/{}-{}.mp4", PLAYBACK_TRANSCODE_SUBDIR, PLAYBACK_TRANSCODE_CACHE_VERSION, key1)
    );
}

#[test]
fn test_playback_file_extension_replacement() {
    assert_eq!(replace_playback_file_extension("audio.wav", ".m4a"), "audio.m4a");
    assert_eq!(replace_playback_file_extension("video.mkv", ".mp4"), "video.mp4");
    assert_eq!(replace_playback_file_extension("noext", ".mp4"), "noext.mp4");
    assert_eq!(replace_playback_file_extension("", ".mp4"), "media.mp4");
    assert_eq!(replace_playback_file_extension(".hidden", ".mp4"), ".hidden.mp4");
}

#[test]
fn test_playback_duration_matching_tolerance() {
    // 100 seconds (100,000 ms) -> 2% is 2,000 ms (clamped to max 2,000 ms)
    assert!(playback_durations_match(100_000, 100_000));
    assert!(playback_durations_match(100_000, 102_000));
    assert!(playback_durations_match(100_000, 98_000));
    assert!(!playback_durations_match(100_000, 102_001));
    assert!(!playback_durations_match(100_000, 97_999));

    // Exceeds max 20 minutes (1,200,000 ms)
    assert!(!playback_durations_match(100_000, 1_200_001));

    // Small duration: 10 seconds (10,000 ms) -> 2% is 200 ms, clamped to min 1,000 ms
    assert!(playback_durations_match(10_000, 11_000));
    assert!(playback_durations_match(10_000, 9_000));
    assert!(!playback_durations_match(10_000, 11_001));
}

#[test]
fn test_build_ffmpeg_playback_args_structure() {
    let input = Path::new("/tmp/input.mov");
    let output = Path::new("/tmp/output.mp4");
    let args = build_ffmpeg_playback_args(
        input,
        output,
        PlaybackMediaKind::Video,
        "mov,mp4,m4a,3gp,3g2,mj2",
        0,
        Some(1),
        50_000_000,
    );

    assert!(args.contains(&"-protocol_whitelist".to_string()));
    assert!(args.contains(&"file".to_string()));
    assert!(args.contains(&"-f".to_string()));
    assert!(args.contains(&"mov,mp4,m4a,3gp,3g2,mj2".to_string()));
    assert!(args.contains(&"-c:v".to_string()));
    assert!(args.contains(&"libx264".to_string()));
    assert!(args.contains(&"-c:a".to_string()));
    assert!(args.contains(&"aac".to_string()));
    assert!(args.contains(&"+faststart".to_string()));
    assert!(args.contains(&"50000001".to_string())); // max_bytes + 1

    // Audio kind
    let audio_output = Path::new("/tmp/output.m4a");
    let audio_args = build_ffmpeg_playback_args(
        input,
        audio_output,
        PlaybackMediaKind::Audio,
        "wav",
        0,
        None,
        10_000_000,
    );
    assert!(audio_args.contains(&"-vn".to_string()));
    assert!(audio_args.contains(&"ipod".to_string()));
    assert!(!audio_args.contains(&"-c:v".to_string()));
}

#[test]
fn test_playback_transcode_resolution_variants() {
    let p = PlaybackTranscodeResolution::Passthrough;
    let prep = PlaybackTranscodeResolution::Preparing;
    let fb = PlaybackTranscodeResolution::Fallback;
    let t = PlaybackTranscodeResolution::Transcoded {
        path: PathBuf::from("/store/cached.mp4"),
        content_type: "video/mp4".into(),
        extension: ".mp4".into(),
    };
    assert_ne!(p, prep);
    assert_ne!(fb, t);
}
