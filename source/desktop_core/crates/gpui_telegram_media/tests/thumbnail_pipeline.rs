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
}
