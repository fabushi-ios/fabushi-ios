use std::path::PathBuf;
use mahayana_host_runtime::extensions::attachments::media_audio_transcode::{
    build_ffmpeg_opus_args, has_prompt_image_input, is_voice_message_compatible_audio,
    normalize_audio_extension, normalize_media_reference_for_comparison,
    normalize_output_file_name, normalize_temp_prefix, parse_ffprobe_codec_and_sample_rate,
    OpusTranscodeArgs, PromptImageOrderEntry, DEFAULT_OPUS_SAMPLE_RATE_HZ,
};
use mahayana_host_runtime::extensions::attachments::media_facts::MediaFact;

#[test]
fn test_ocrm_audio_o01_voice_message_compatible_audio() {
    // 1. By extension
    assert!(is_voice_message_compatible_audio(None, Some("memo.ogg")));
    assert!(is_voice_message_compatible_audio(None, Some("memo.oga")));
    assert!(is_voice_message_compatible_audio(None, Some("memo.opus")));
    assert!(is_voice_message_compatible_audio(None, Some("voice.mp3")));
    assert!(is_voice_message_compatible_audio(None, Some("voice.m4a")));
    assert!(!is_voice_message_compatible_audio(None, Some("song.flac")));
    assert!(!is_voice_message_compatible_audio(None, Some("audio.wav")));

    // 2. By MIME type
    assert!(is_voice_message_compatible_audio(Some("audio/ogg"), None));
    assert!(is_voice_message_compatible_audio(Some("audio/opus"), None));
    assert!(is_voice_message_compatible_audio(Some("audio/mpeg"), None));
    assert!(is_voice_message_compatible_audio(Some("audio/x-m4a"), None));
    assert!(!is_voice_message_compatible_audio(Some("audio/wav"), None));
    assert!(!is_voice_message_compatible_audio(Some("audio/flac"), None));
    assert!(!is_voice_message_compatible_audio(Some("video/mp4"), None));
}

#[test]
fn test_ocrm_audio_o02_ffprobe_codec_and_sample_rate_parsing() {
    let (c1, r1) = parse_ffprobe_codec_and_sample_rate("opus,48000\n");
    assert_eq!(c1.as_deref(), Some("opus"));
    assert_eq!(r1, Some(48000));

    let (c2, r2) = parse_ffprobe_codec_and_sample_rate("AAC\r\n44100");
    assert_eq!(c2.as_deref(), Some("aac"));
    assert_eq!(r2, Some(44100));

    let (c3, r3) = parse_ffprobe_codec_and_sample_rate("mp3,not-a-number");
    assert_eq!(c3.as_deref(), Some("mp3"));
    assert_eq!(r3, None);
}

#[test]
fn test_ocrm_audio_o03_media_reference_comparison_normalization() {
    // 1. Absolute clean path
    let norm1 = normalize_media_reference_for_comparison("/tmp/audio/voice.opus");
    assert_eq!(norm1, "/tmp/audio/voice.opus");

    // 2. Traversal path retained literally to avoid collision
    let norm2 = normalize_media_reference_for_comparison("/tmp/audio/../secret.opus");
    assert_eq!(norm2, "/tmp/audio/../secret.opus");

    // 3. file:// URL decoded
    let norm3 = normalize_media_reference_for_comparison("file:///tmp/voice.opus");
    assert_eq!(norm3, "/tmp/voice.opus");

    // 4. Encoded separator %2f preserved
    let norm4 = normalize_media_reference_for_comparison("file:///tmp%2fvoice.opus");
    assert_eq!(norm4, "file:///tmp%2fvoice.opus");
}

#[test]
fn test_ocrm_audio_o04_prompt_image_input_detection() {
    // 1. Explicit images count
    assert!(has_prompt_image_input(1, &[], &[], None));

    // 2. Explicit image order
    assert!(has_prompt_image_input(0, &[PromptImageOrderEntry::Inline], &[], None));

    // 3. Media fact with image kind
    let image_fact = MediaFact {
        kind: Some("image".into()),
        path: Some("/tmp/img.png".into()),
        ..Default::default()
    };
    assert!(has_prompt_image_input(0, &[], &[image_fact.clone()], None));

    // 4. Suppressed index ignored
    assert!(!has_prompt_image_input(0, &[], &[image_fact.clone()], Some(&[0])));

    // 5. Hydration suppressed fact ignored
    let suppressed_fact = MediaFact {
        kind: Some("image".into()),
        hydration_suppressed: Some(true),
        path: Some("/tmp/img.png".into()),
        ..Default::default()
    };
    assert!(!has_prompt_image_input(0, &[], &[suppressed_fact], None));
}

#[test]
fn test_ocrm_audio_o05_audio_extension_and_temp_normalization() {
    assert_eq!(normalize_audio_extension(Some("mp3"), None), ".mp3");
    assert_eq!(normalize_audio_extension(Some(".m4a"), None), ".m4a");
    assert_eq!(normalize_audio_extension(None, Some("clip.wav")), ".wav");
    assert_eq!(normalize_audio_extension(None, None), ".audio");
    assert_eq!(normalize_audio_extension(Some("waytoolongextensionname"), None), ".audio");

    assert_eq!(normalize_temp_prefix(Some("voice")), "voice-");
    assert_eq!(normalize_temp_prefix(Some("voice-")), "voice-");
    assert_eq!(normalize_temp_prefix(Some(".")), "audio-opus-");
    assert_eq!(normalize_temp_prefix(None), "audio-opus-");

    assert_eq!(normalize_output_file_name(Some("custom.opus")), "custom.opus");
    assert_eq!(normalize_output_file_name(Some("/path/to/nested.opus")), "nested.opus");
    assert_eq!(normalize_output_file_name(Some("..")), "voice.opus");
    assert_eq!(normalize_output_file_name(None), "voice.opus");
}

#[test]
fn test_ocrm_audio_o06_build_ffmpeg_opus_args() {
    let args = OpusTranscodeArgs {
        input_path: PathBuf::from("/tmp/in.wav"),
        output_path: PathBuf::from("/tmp/out.opus"),
        bitrate: "64k".into(),
        sample_rate_hz: DEFAULT_OPUS_SAMPLE_RATE_HZ,
        channels: 1,
        max_duration_seconds: Some(120),
    };

    let cmd = build_ffmpeg_opus_args(&args);
    assert!(cmd.contains(&"-c:a".to_string()));
    assert!(cmd.contains(&"libopus".to_string()));
    assert!(cmd.contains(&"-b:a".to_string()));
    assert!(cmd.contains(&"64k".to_string()));
    assert!(cmd.contains(&"-ar".to_string()));
    assert!(cmd.contains(&"48000".to_string()));
    assert!(cmd.contains(&"-ac".to_string()));
    assert!(cmd.contains(&"1".to_string()));
    assert!(cmd.contains(&"-t".to_string()));
    assert!(cmd.contains(&"120".to_string()));
    assert!(cmd.contains(&"-f".to_string()));
    assert!(cmd.contains(&"opus".to_string()));
    assert_eq!(cmd.last().unwrap(), "/tmp/out.opus");
}
