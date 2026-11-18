use gpui_telegram_media::thumbnails::{
    fit_dimensions_downscale_only, parse_stripped_dimensions, shrink_to_keep_aspect,
    ThumbnailGenerator, ThumbnailPreset,
};

#[test]
fn test_aspect_preservation_wide() {
    let (w, h) = shrink_to_keep_aspect(1920, 1080, 320, 320);
    assert_eq!((w, h), (320, 180));
}

#[test]
fn test_aspect_preservation_tall() {
    let (w, h) = shrink_to_keep_aspect(1080, 1920, 320, 320);
    assert_eq!((w, h), (180, 320));
}

#[test]
fn test_aspect_preservation_square() {
    let (w, h) = shrink_to_keep_aspect(1000, 1000, 320, 320);
    assert_eq!((w, h), (320, 320));
}

#[test]
fn test_downscale_only() {
    let (w, h) = fit_dimensions_downscale_only(200, 200, 320, 320);
    assert_eq!((w, h), (200, 200));

    let (w, h) = fit_dimensions_downscale_only(500, 250, 320, 320);
    assert_eq!((w, h), (320, 160));
}

#[test]
fn test_stripped_thumbnail_envelope() {
    let stripped = vec![0x01, 32, 18, 0xFF, 0xD8];
    let (w, h) = parse_stripped_dimensions(&stripped).unwrap();
    assert_eq!((w, h), (32, 18));
}

#[test]
fn test_thumbnail_generation_with_image_crate() {
    let img = image::RgbImage::new(60, 40);
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();

    let thumb = ThumbnailGenerator::generate_thumbnail(&buf, ThumbnailPreset::Micro).unwrap();
    assert!(!thumb.is_empty());
    let decoded = image::load_from_memory(&thumb).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (32, 21));

    let meta = ThumbnailGenerator::probe_metadata(&buf).unwrap();
    assert_eq!((meta.width, meta.height, meta.format.as_str()), (60, 40, "png"));

    let rgba = ThumbnailGenerator::decode_to_rgba8(&buf).unwrap();
    assert_eq!(rgba.rgba.len(), 60 * 40 * 4);

    let stripped = ThumbnailGenerator::generate_stripped_preview(&buf).unwrap();
    let decoded_stripped = ThumbnailGenerator::decode_stripped_preview(&stripped).unwrap();
    assert_eq!((decoded_stripped.width, decoded_stripped.height), (32, 21));

    let placeholder = ThumbnailGenerator::extract_thumbnail_placeholder(&buf).unwrap();
    assert!(ThumbnailGenerator::probe_metadata(&placeholder).is_ok());
}

#[test]
fn test_wav_pcm_and_voice_recorder_waveform_pipeline() {
    use gpui_telegram_media::{decode_wav_pcm, encode_wav_i16, VoiceRecorder};

    let mut recorder = VoiceRecorder::new(48000);
    let sine_chunk: Vec<i16> = (0..4800).map(|i| ((i % 48) as i16 - 24) * 900).collect();
    recorder.feed_external_pcm_i16(&sine_chunk);
    assert!(recorder.is_recording());
    assert!(recorder.current_level() > 0);

    let note = recorder.stop().unwrap();
    assert!(!recorder.is_recording());
    assert_eq!(note.duration_ms, 100);
    assert_eq!(note.waveform_bars.len(), 100);
    assert_eq!(note.waveform_5bit.len(), 63);

    let decoded = decode_wav_pcm(&note.wav_bytes).unwrap();
    assert_eq!(decoded.sample_rate, 48000);
    assert_eq!(decoded.channels, 1);
    assert_eq!(decoded.samples, sine_chunk);

    let reencoded = encode_wav_i16(&decoded.samples, 48000, 1).unwrap();
    assert_eq!(reencoded, note.wav_bytes);
}
