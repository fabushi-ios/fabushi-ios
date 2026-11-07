use std::fs;
use std::path::{Path, PathBuf};
use serde_json::json;
use mahayana_host_runtime::extensions::attachments::media_reference::{
    build_inbound_media_uri_from_path, classify_media_reference_source,
    normalize_media_reference_source, parse_inbound_media_uri,
    read_outbound_media_file_bounded, resolve_inbound_media_reference,
    resolve_media_reference_local_path_info, resolve_media_reference_sandbox_path,
    MediaReferenceError, MediaReferenceLocalPathInfo,
};
use mahayana_host_runtime::extensions::attachments::media_probe::{
    get_playback_policy, parse_ffprobe_media_metadata,
    resolve_native_playback_codec_compatibility, resolve_playback_input_format,
    resolve_playback_mode, to_media_probe_result, PlaybackMediaKind, PlaybackMediaProbeResult,
    PlaybackMode,
};
use mahayana_host_runtime::extensions::attachments::media_facts::{
    canonicalize_persisted_user_message_media, has_meaningful_retired_media_carrier,
    has_staged_media_facts, is_image_media_fact, is_meaningful_media_fact,
    is_video_media_fact, project_media_facts, read_persisted_media_facts,
    resolve_media_fact_kind, resolve_media_facts, strip_legacy_media_context_fields,
    MediaFact,
};

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ocrm_media_ref_test_{}_{}", label, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_ocrm_media_ref_01_inbound_uri_parsing_and_validation() {
    // 1. Valid URI
    let parsed = parse_inbound_media_uri("media://inbound/recording-01.m4a").unwrap().unwrap();
    assert_eq!(parsed.id, "recording-01.m4a");
    assert_eq!(parsed.normalized_source, "media://inbound/recording-01.m4a");

    // 2. Strips legacy MEDIA: prefix
    let parsed_legacy = parse_inbound_media_uri("MEDIA: media://inbound/snapshot.png").unwrap().unwrap();
    assert_eq!(parsed_legacy.id, "snapshot.png");

    // 3. Rejects unsupported hostname
    let err_host = parse_inbound_media_uri("media://outbound/snapshot.png").unwrap_err();
    match err_host {
        MediaReferenceError::PathNotAllowed(msg) => {
            assert!(msg.contains("outbound"));
        }
        _ => panic!("expected PathNotAllowed"),
    }

    // 4. Rejects search query and fragments
    let err_query = parse_inbound_media_uri("media://inbound/snapshot.png?token=123").unwrap_err();
    assert!(matches!(err_query, MediaReferenceError::InvalidPath(_)));

    // 5. Rejects traversal and nested directories
    let err_traversal = parse_inbound_media_uri("media://inbound/../secret.txt").unwrap_err();
    assert!(matches!(err_traversal, MediaReferenceError::InvalidPath(_)));

    let err_nested = parse_inbound_media_uri("media://inbound/nested/dir.png").unwrap_err();
    assert!(matches!(err_nested, MediaReferenceError::InvalidPath(_)));
}

#[test]
fn test_ocrm_media_ref_02_classify_media_reference_source() {
    let win = classify_media_reference_source(r"C:\Users\media\clip.mp4", true);
    assert!(win.looks_like_windows_drive_path);
    assert!(!win.has_unsupported_scheme);

    let http = classify_media_reference_source("https://example.com/asset.png", true);
    assert!(http.is_http_url);
    assert!(!http.has_unsupported_scheme);

    let data = classify_media_reference_source("data:image/png;base64,AAAA", true);
    assert!(data.is_data_url);
    assert!(!data.has_unsupported_scheme);

    let disallowed_data = classify_media_reference_source("data:image/png;base64,AAAA", false);
    assert!(disallowed_data.is_data_url);
    assert!(disallowed_data.has_unsupported_scheme);

    let media_store = classify_media_reference_source("media://inbound/doc.pdf", true);
    assert!(media_store.is_media_store_url);
    assert!(!media_store.has_unsupported_scheme);

    let unsupported = classify_media_reference_source("ftp://server/file.txt", true);
    assert!(unsupported.has_unsupported_scheme);
}

#[test]
fn test_ocrm_media_ref_03_inbound_path_resolution_and_sandbox_rewrite() {
    let root = temp_dir("store");
    let inbound_dir = root.join("inbound");
    fs::create_dir_all(&inbound_dir).unwrap();

    let file_path = inbound_dir.join("photo.jpg");
    fs::write(&file_path, b"test-photo").unwrap();

    // build URI from path
    let uri = build_inbound_media_uri_from_path(&file_path, &root);
    assert_eq!(uri.as_deref(), Some("media://inbound/photo.jpg"));

    // Outside path cannot be built
    let outside = root.join("outside.jpg");
    fs::write(&outside, b"outside").unwrap();
    assert_eq!(build_inbound_media_uri_from_path(&outside, &root), None);

    // Sandbox rewrite
    let rewrite = resolve_media_reference_sandbox_path("media://inbound/photo.jpg", None);
    assert_eq!(rewrite.resolved, "media/inbound/photo.jpg");
    assert_eq!(rewrite.rewritten_from.as_deref(), Some("media://inbound/photo.jpg"));

    // Local path info resolution
    let info = resolve_media_reference_local_path_info("media://inbound/photo.jpg", &root).unwrap();
    match info {
        MediaReferenceLocalPathInfo::Inbound { id, path } => {
            assert_eq!(id, "photo.jpg");
            assert_eq!(path, file_path);
        }
        _ => panic!("expected Inbound reference"),
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_ocrm_media_ref_04_bounded_read_file() {
    let root = temp_dir("bounded");
    let test_file = root.join("sample.bin");
    fs::write(&test_file, b"0123456789").unwrap(); // 10 bytes

    // Read within limits
    let ok_data = read_outbound_media_file_bounded(&test_file, 10).unwrap();
    assert_eq!(ok_data, b"0123456789");

    // Read with tighter limit rejects oversized
    let err = read_outbound_media_file_bounded(&test_file, 5).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_ocrm_media_ref_05_playback_codec_policy_resolution() {
    let video_policy = get_playback_policy(PlaybackMediaKind::Video);
    assert_eq!(resolve_playback_mode("video/mp4", video_policy), Some(PlaybackMode::Native));
    assert_eq!(resolve_playback_mode("video/quicktime", video_policy), Some(PlaybackMode::Transcode));
    assert_eq!(resolve_playback_mode("video/webm", video_policy), Some(PlaybackMode::Transcode));
    assert_eq!(resolve_playback_input_format(video_policy, "video/quicktime"), Some("mov"));

    let audio_policy = get_playback_policy(PlaybackMediaKind::Audio);
    assert_eq!(resolve_playback_mode("audio/mp3", audio_policy), Some(PlaybackMode::Native));
    assert_eq!(resolve_playback_mode("audio/wav", audio_policy), Some(PlaybackMode::Native));
    assert_eq!(resolve_playback_mode("audio/flac", audio_policy), Some(PlaybackMode::Transcode));
    assert_eq!(resolve_playback_input_format(audio_policy, "audio/flac"), Some("flac"));
}

#[test]
fn test_ocrm_media_ref_06_native_codec_compatibility_rules() {
    // 1. Audio WAV requires PCM s16le or u8
    let wav_probe_ok = PlaybackMediaProbeResult {
        audio_codec: Some("pcm_s16le".into()),
        audio_stream_index: Some(0),
        ..Default::default()
    };
    assert_eq!(
        resolve_native_playback_codec_compatibility(PlaybackMediaKind::Audio, "audio/wav", &wav_probe_ok),
        Some(true)
    );

    let wav_probe_bad = PlaybackMediaProbeResult {
        audio_codec: Some("mp3".into()),
        audio_stream_index: Some(0),
        ..Default::default()
    };
    assert_eq!(
        resolve_native_playback_codec_compatibility(PlaybackMediaKind::Audio, "audio/wav", &wav_probe_bad),
        Some(false)
    );

    // 2. Video requires H.264 baseline/main/high with yuv420p and AAC/MP3 audio
    let video_ok = PlaybackMediaProbeResult {
        video_codec: Some("h264".into()),
        video_profile: Some("high".into()),
        video_pixel_format: Some("yuv420p".into()),
        video_stream_index: Some(0),
        audio_codec: Some("aac".into()),
        audio_stream_index: Some(1),
        ..Default::default()
    };
    assert_eq!(
        resolve_native_playback_codec_compatibility(PlaybackMediaKind::Video, "video/mp4", &video_ok),
        Some(true)
    );

    // HEVC is incompatible
    let video_hevc = PlaybackMediaProbeResult {
        video_codec: Some("hevc".into()),
        video_profile: Some("main".into()),
        video_pixel_format: Some("yuv420p".into()),
        video_stream_index: Some(0),
        ..Default::default()
    };
    assert_eq!(
        resolve_native_playback_codec_compatibility(PlaybackMediaKind::Video, "video/mp4", &video_hevc),
        Some(false)
    );
}

#[test]
fn test_ocrm_media_ref_07_probe_metadata_aspect_ratio_and_rotation() {
    let ffprobe_json = r#"{
        "format": { "duration": "12.5" },
        "streams": [
            {
                "codec_type": "video",
                "codec_name": "h264",
                "profile": "high",
                "pix_fmt": "yuv420p",
                "width": 1920,
                "height": 1080,
                "sample_aspect_ratio": "1:1",
                "duration": "12.48",
                "index": 0,
                "disposition": { "default": 1, "attached_pic": 0 },
                "side_data_list": [ { "rotation": 90 } ]
            },
            {
                "codec_type": "audio",
                "codec_name": "aac",
                "duration": "12.5",
                "index": 1,
                "disposition": { "default": 1 }
            }
        ]
    }"#;

    let probe = parse_ffprobe_media_metadata(ffprobe_json, PlaybackMediaKind::Video).unwrap();
    assert_eq!(probe.duration_ms, Some(12500));
    assert_eq!(probe.video_codec.as_deref(), Some("h264"));
    assert_eq!(probe.audio_codec.as_deref(), Some("aac"));
    assert_eq!(probe.video_rotation, Some(90));

    // to_media_probe_result swaps width and height on 90 degree rotation
    let display = to_media_probe_result(Some(&probe));
    assert_eq!(display.width, Some(1080));
    assert_eq!(display.height, Some(1920));
}

#[test]
fn test_ocrm_media_ref_08_canonicalize_persisted_user_message_media() {
    let mut msg = json!({
        "role": "user",
        "content": "check this photo",
        "MediaPath": "/tmp/captured.jpg",
        "MediaType": "image/jpeg",
        "MediaWorkspaceDir": "/workspace/agent-1"
    });

    assert!(has_meaningful_retired_media_carrier(&msg));

    let changed = canonicalize_persisted_user_message_media(&mut msg).unwrap();
    assert!(changed);

    // Root legacy keys must be removed
    assert!(msg.get("MediaPath").is_none());
    assert!(msg.get("MediaType").is_none());
    assert!(msg.get("MediaWorkspaceDir").is_none());

    // __openclaw.media must be populated
    let facts = read_persisted_media_facts(&msg).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].path.as_deref(), Some("/tmp/captured.jpg"));
    assert_eq!(facts[0].content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(facts[0].workspace_dir.as_deref(), Some("/workspace/agent-1"));
    assert!(is_image_media_fact(&facts[0]));

    // Projection back to channel formats
    let projected = project_media_facts(&facts, "channel");
    assert_eq!(projected.get("MediaPath").and_then(|v| v.as_str()), Some("/tmp/captured.jpg"));
    assert_eq!(projected.get("MediaType").and_then(|v| v.as_str()), Some("image/jpeg"));
}
