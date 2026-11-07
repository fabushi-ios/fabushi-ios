use mahayana_host_runtime::extensions::attachments::media_generation_service::*;

#[test]
fn test_ocrm_media_gen_o01_aspect_ratio_normalization_and_derivation() {
    // 1. Derive aspect ratio from size using GCD
    assert_eq!(
        derive_aspect_ratio_from_size("1920x1080"),
        Some("16:9".to_string())
    );
    assert_eq!(
        derive_aspect_ratio_from_size("1024x1024"),
        Some("1:1".to_string())
    );
    assert_eq!(
        derive_aspect_ratio_from_size("800X600"),
        Some("4:3".to_string())
    );
    assert_eq!(derive_aspect_ratio_from_size("invalid"), None);

    // 2. Exact match in supported aspect ratios
    let supported = vec!["1:1".to_string(), "16:9".to_string(), "9:16".to_string()];
    assert_eq!(
        resolve_closest_aspect_ratio(Some("16:9"), None, &supported),
        Some("16:9".to_string())
    );

    // 3. Derived from requested size when aspect ratio is omitted
    assert_eq!(
        resolve_closest_aspect_ratio(None, Some("1280x720"), &supported),
        Some("16:9".to_string())
    );

    // 4. Closest aspect ratio by log distance
    // 4:3 is ~1.333; 1:1 is 1.0 (log diff ~0.287), 16:9 is 1.777 (log diff ~0.287)
    let candidates = vec!["1:1".to_string(), "16:9".to_string()];
    let closest = resolve_closest_aspect_ratio(Some("4:3"), None, &candidates);
    assert!(closest.is_some());
}

#[test]
fn test_ocrm_media_gen_o02_size_and_resolution_normalization() {
    let supported_sizes = vec![
        "512x512".to_string(),
        "1024x1024".to_string(),
        "1280x720".to_string(),
    ];

    // 1. Exact match
    assert_eq!(
        resolve_closest_size(Some("1024x1024"), None, &supported_sizes),
        Some("1024x1024".to_string())
    );

    // 2. Closest by aspect ratio when given aspect ratio alone
    assert_eq!(
        resolve_closest_size(None, Some("1:1"), &supported_sizes),
        Some("512x512".to_string())
    );

    // 3. Resolution parsing and closest match within unit
    assert_eq!(parse_resolution_rank("4K"), Some((4000.0, "K")));
    assert_eq!(parse_resolution_rank("1080P"), Some((1080.0, "P")));
    assert_eq!(parse_resolution_rank("invalid"), None);

    let supported_res = vec!["1K".to_string(), "2K".to_string(), "4K".to_string()];
    // On tie between 2K and 4K for 3K, upstream selects the higher resolution 4K (secondary score 0 < 1)
    assert_eq!(
        resolve_closest_resolution(Some("3K"), &supported_res),
        Some("4K".to_string())
    );
    assert_eq!(
        resolve_closest_resolution(Some("4K"), &supported_res),
        Some("4K".to_string())
    );
    // Unit mismatch returns None
    assert_eq!(
        resolve_closest_resolution(Some("1080P"), &supported_res),
        None
    );
}

#[test]
fn test_ocrm_media_gen_o03_duration_and_timeout_clamping() {
    // 1. Normalize duration
    assert_eq!(normalize_duration_to_closest_max(Some(14.6), None), Some(15));
    assert_eq!(normalize_duration_to_closest_max(Some(14.2), None), Some(14));
    assert_eq!(
        normalize_duration_to_closest_max(Some(15.0), Some(10.0)),
        Some(10)
    );
    assert_eq!(normalize_duration_to_closest_max(None, Some(10.0)), None);
    assert_eq!(normalize_duration_to_closest_max(Some(-5.0), None), None);

    // 2. Resolve request timeout ms
    assert_eq!(
        resolve_media_provider_request_timeout_ms(Some(45_000), Some(30_000)),
        Some(45_000)
    );
    assert_eq!(
        resolve_media_provider_request_timeout_ms(None, Some(30_000)),
        Some(30_000)
    );
    assert_eq!(
        resolve_media_provider_request_timeout_ms(Some(u64::MAX), None),
        Some(2_147_483_647)
    );
    assert_eq!(
        resolve_media_provider_request_timeout_ms(Some(200), None),
        Some(1_000)
    );
    assert_eq!(resolve_media_provider_request_timeout_ms(None, None), None);
}

#[test]
fn test_ocrm_media_gen_o04_music_generation_capabilities_and_reference_images() {
    let mut caps = MusicGenerationProviderCapabilities::default();
    caps.base.max_tracks = Some(2);
    caps.base.supports_lyrics = true;

    // 1. Only generate mode supported initially
    assert_eq!(list_supported_music_generation_modes(&caps), vec!["generate"]);

    // 2. Edit mode enabled
    caps.edit = Some(MusicGenerationEditCapabilities {
        enabled: true,
        max_input_images: Some(3),
        mode_capabilities: MusicGenerationModeCapabilities {
            supports_format: true,
            supported_formats: vec![MusicGenerationOutputFormat::Mp3],
            ..Default::default()
        },
    });
    assert_eq!(
        list_supported_music_generation_modes(&caps),
        vec!["generate", "edit"]
    );

    // 3. Resolve reference image capability errors
    assert_eq!(
        resolve_reference_image_capability_error("prov/model", 0, caps.edit.as_ref()),
        None
    );
    assert_eq!(
        resolve_reference_image_capability_error("prov/model", 2, caps.edit.as_ref()),
        None
    );
    let err_overflow =
        resolve_reference_image_capability_error("prov/model", 5, caps.edit.as_ref());
    assert_eq!(
        err_overflow,
        Some("prov/model supports at most 3 reference images, 5 requested".to_string())
    );

    let err_disabled = resolve_reference_image_capability_error(
        "prov/model",
        1,
        Some(&MusicGenerationEditCapabilities {
            enabled: false,
            max_input_images: None,
            mode_capabilities: Default::default(),
        }),
    );
    assert_eq!(
        err_disabled,
        Some("prov/model does not support reference-image edit inputs".to_string())
    );
}

#[test]
fn test_ocrm_media_gen_o05_video_asset_reading_and_overflow() {
    // 1. Within byte limit
    let small_buffer = vec![0u8; 1024];
    let asset = read_generated_video_asset(
        small_buffer,
        Some("video/mp4"),
        Some(0),
        Some(10_000),
        None,
    )
    .unwrap();
    assert_eq!(asset.file_name, "video-1.mp4");
    assert_eq!(asset.mime_type, "video/mp4");
    assert!(asset.buffer.is_some());
    assert!(asset.url.is_none());

    // 2. Overflow with remote fallback URL
    let large_buffer = vec![0u8; 20_000];
    let overflow_asset = read_generated_video_asset(
        large_buffer,
        Some("video/webm"),
        Some(2),
        Some(10_000),
        Some("https://cdn.example/video.webm"),
    )
    .unwrap();
    assert_eq!(overflow_asset.file_name, "video-3.webm");
    assert_eq!(overflow_asset.mime_type, "video/webm");
    assert!(overflow_asset.buffer.is_none());
    assert_eq!(
        overflow_asset.url,
        Some("https://cdn.example/video.webm".to_string())
    );

    // 3. Overflow without fallback URL returns Err
    let err = read_generated_video_asset(
        vec![0u8; 20_000],
        Some("video/mp4"),
        None,
        Some(10_000),
        None,
    )
    .unwrap_err();
    assert!(err.contains("exceeds byte limit"));
}
