use std::path::PathBuf;
use std::time::SystemTime;
use base64::Engine;

use mahayana_host_runtime::extensions::attachments::media_qr_png::{
    convert_bmp_to_png, encode_png_rgb, encode_png_rgba, fill_pixel,
    render_qr_png_base64, render_qr_png_buffer, render_qr_png_data_url,
    render_qr_terminal, resolve_qr_png_integer_option, resolve_qr_temp_path_segment,
    write_qr_png_temp_file, QrPngRenderOptions, QrPngTempFileOptions,
    QR_PNG_DATA_URL_PREFIX, TERMINAL_BLACK_ON_WHITE, TERMINAL_RESET,
};
use mahayana_host_runtime::extensions::attachments::media_store_shared::{
    format_media_size, SaveMediaSourceError, SaveMediaSourceErrorCode, MEDIA_FILE_MODE,
};

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("ocrm_qr_png_{label}_{nonce}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_ocrm_qr_terminal_compact() {
    let payload = "https://example.com/pairing";
    let terminal_out = render_qr_terminal(payload, true).expect("compact terminal render");

    // Must start with black-on-white ANSI escape and end with reset
    assert!(terminal_out.starts_with(TERMINAL_BLACK_ON_WHITE));
    assert!(terminal_out.ends_with(TERMINAL_RESET));

    // Must contain compact half-block characters
    assert!(
        terminal_out.contains('█')
            || terminal_out.contains('▀')
            || terminal_out.contains('▄')
            || terminal_out.contains(' ')
    );

    let lines: Vec<&str> = terminal_out.lines().collect();
    assert!(!lines.is_empty());
    for line in &lines {
        assert!(line.starts_with(TERMINAL_BLACK_ON_WHITE));
        assert!(line.ends_with(TERMINAL_RESET));
    }
}

#[test]
fn test_ocrm_qr_terminal_standard() {
    let payload = "openclaw:standard_terminal";
    let terminal_out = render_qr_terminal(payload, false).expect("standard terminal render");

    let lines: Vec<&str> = terminal_out.lines().collect();
    assert!(!lines.is_empty());
    // In standard mode, lines contain double block characters
    assert!(terminal_out.contains("██") || terminal_out.contains("  "));
}

#[test]
fn test_ocrm_qr_image_options_validation() {
    // resolve_qr_png_integer_option
    assert_eq!(
        resolve_qr_png_integer_option("scale", 6.8, 1, 12).unwrap(),
        6
    );
    assert_eq!(
        resolve_qr_png_integer_option("scale", 1.0, 1, 12).unwrap(),
        1
    );
    assert_eq!(
        resolve_qr_png_integer_option("scale", 12.0, 1, 12).unwrap(),
        12
    );

    // Bounded range checks
    assert!(resolve_qr_png_integer_option("scale", 0.9, 1, 12).is_err());
    assert!(resolve_qr_png_integer_option("scale", 13.0, 1, 12).is_err());
    assert!(resolve_qr_png_integer_option("scale", f64::NAN, 1, 12).is_err());
    assert!(resolve_qr_png_integer_option("scale", f64::INFINITY, 1, 12).is_err());

    // marginModules
    assert_eq!(
        resolve_qr_png_integer_option("marginModules", 4.0, 0, 16).unwrap(),
        4
    );
    assert_eq!(
        resolve_qr_png_integer_option("marginModules", 0.0, 0, 16).unwrap(),
        0
    );
    assert_eq!(
        resolve_qr_png_integer_option("marginModules", 16.0, 0, 16).unwrap(),
        16
    );
    assert!(resolve_qr_png_integer_option("marginModules", -1.0, 0, 16).is_err());
    assert!(resolve_qr_png_integer_option("marginModules", 17.0, 0, 16).is_err());

    // resolve_qr_temp_path_segment
    assert_eq!(
        resolve_qr_temp_path_segment("dirPrefix", "valid_prefix").unwrap(),
        "valid_prefix"
    );
    assert_eq!(
        resolve_qr_temp_path_segment("fileName", "custom-qr.png").unwrap(),
        "custom-qr.png"
    );

    assert!(resolve_qr_temp_path_segment("dirPrefix", "").is_err());
    assert!(resolve_qr_temp_path_segment("dirPrefix", ".").is_err());
    assert!(resolve_qr_temp_path_segment("dirPrefix", "..").is_err());
    assert!(resolve_qr_temp_path_segment("dirPrefix", "dir/sub").is_err());
    assert!(resolve_qr_temp_path_segment("dirPrefix", "../traversal").is_err());
}

#[test]
fn test_ocrm_qr_png_data_url_and_buffer() {
    let payload = "fabushi://device-pair/session-12345";
    let opts = QrPngRenderOptions {
        scale: Some(4),
        margin_modules: Some(2),
    };

    let png_bytes = render_qr_png_buffer(payload, &opts).expect("render png buffer");
    assert!(png_bytes.len() > 8);
    // Magic PNG header
    assert_eq!(&png_bytes[0..8], &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

    let data_url = render_qr_png_data_url(payload, &opts).expect("render png data url");
    assert!(data_url.starts_with(QR_PNG_DATA_URL_PREFIX));

    let b64_part = &data_url[QR_PNG_DATA_URL_PREFIX.len()..];
    let decoded = base64::prelude::BASE64_STANDARD
        .decode(b64_part)
        .expect("decode base64");
    assert_eq!(decoded, png_bytes);

    let raw_b64 = render_qr_png_base64(payload, &opts).expect("render png base64");
    assert_eq!(raw_b64, b64_part);
}

#[test]
fn test_ocrm_qr_png_temp_file_lifecycle() {
    let tmp = temp_dir("temp_file_lifecycle");
    let payload = "pairing:secret_nonce_payload";

    let temp_opts = QrPngTempFileOptions {
        tmp_root: tmp.clone(),
        dir_prefix: "pairing_session".to_string(),
        file_name: Some("pair_qr.png".to_string()),
        render_options: QrPngRenderOptions::default(),
    };

    let result = write_qr_png_temp_file(payload, &temp_opts).expect("write temp qr png");
    assert!(result.file_path.ends_with("pair_qr.png"));
    assert!(result.dir_path.starts_with(tmp.to_str().unwrap()));
    assert_eq!(result.media_local_roots.len(), 1);
    assert_eq!(result.media_local_roots[0], result.dir_path);

    let file_on_disk = PathBuf::from(&result.file_path);
    assert!(file_on_disk.is_file());
    let file_bytes = std::fs::read(&file_on_disk).unwrap();
    assert_eq!(&file_bytes[0..8], &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_ocrm_png_encode_raw_rgb_and_rgba() {
    // 2x2 RGB image (12 bytes)
    // Red, Green, Blue, White
    let rgb_pixels: [u8; 12] = [
        255, 0, 0,   // (0,0) Red
        0, 255, 0,   // (1,0) Green
        0, 0, 255,   // (0,1) Blue
        255, 255, 255, // (1,1) White
    ];

    let png_rgb = encode_png_rgb(&rgb_pixels, 2, 2).expect("encode 2x2 rgb");
    assert_eq!(&png_rgb[0..8], &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

    // Check IHDR length = 13
    assert_eq!(&png_rgb[8..12], &[0, 0, 0, 13]);
    assert_eq!(&png_rgb[12..16], b"IHDR");
    // Width = 2
    assert_eq!(&png_rgb[16..20], &[0, 0, 0, 2]);
    // Height = 2
    assert_eq!(&png_rgb[20..24], &[0, 0, 0, 2]);
    // Bit depth = 8
    assert_eq!(png_rgb[24], 8);
    // Color type = 2 (RGB)
    assert_eq!(png_rgb[25], 2);

    // 2x2 RGBA image (16 bytes)
    let mut rgba_pixels = [0u8; 16];
    // Fill pixels using fill_pixel
    fill_pixel(&mut rgba_pixels, 0, 0, 2, 255, 0, 0, 255);
    fill_pixel(&mut rgba_pixels, 1, 0, 2, 0, 255, 0, 255);
    fill_pixel(&mut rgba_pixels, 0, 1, 2, 0, 0, 255, 128);
    fill_pixel(&mut rgba_pixels, 1, 1, 2, 10, 20, 30, 255);

    // Test out of bounds fill_pixel (must not panic)
    fill_pixel(&mut rgba_pixels, -1, 0, 2, 99, 99, 99, 99);
    fill_pixel(&mut rgba_pixels, 0, -1, 2, 99, 99, 99, 99);
    fill_pixel(&mut rgba_pixels, 2, 0, 2, 99, 99, 99, 99);
    fill_pixel(&mut rgba_pixels, 0, 2, 2, 99, 99, 99, 99);

    let png_rgba = encode_png_rgba(&rgba_pixels, 2, 2).expect("encode 2x2 rgba");
    assert_eq!(&png_rgba[0..8], &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    // Color type = 6 (RGBA)
    assert_eq!(png_rgba[25], 6);

    // Invalid buffer length check
    assert!(encode_png_rgb(&rgb_pixels[..10], 2, 2).is_err());
}

#[test]
fn test_ocrm_bmp_to_png_conversion() {
    // Construct a minimal 2x2 24-bit uncompressed BMP:
    // Header size: 14 (file header) + 40 (DIB header) = 54 bytes
    // Scanline stride for 2 pixels * 3 bytes = 6 bytes -> padded to 8 bytes.
    // 2 rows * 8 bytes = 16 bytes pixel data. Total = 70 bytes.
    let mut bmp = vec![0u8; 70];
    // File header
    bmp[0] = 0x42; // 'B'
    bmp[1] = 0x4D; // 'M'
    bmp[2..6].copy_from_slice(&70u32.to_le_bytes()); // File size
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes()); // Pixel offset

    // DIB header (BITMAPINFOHEADER)
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes()); // DIB header size
    bmp[18..22].copy_from_slice(&2i32.to_le_bytes()); // Width = 2
    bmp[22..26].copy_from_slice(&2i32.to_le_bytes()); // Height = 2 (bottom-up)
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes()); // Planes = 1
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes()); // bpp = 24
    bmp[30..34].copy_from_slice(&0u32.to_le_bytes()); // Compression = 0 (BI_RGB)

    // Row 0 (bottom row): 2 pixels (BGR) + 2 bytes padding
    // Px 0: Blue (255, 0, 0)
    bmp[54] = 255; bmp[55] = 0; bmp[56] = 0;
    // Px 1: Green (0, 255, 0)
    bmp[57] = 0; bmp[58] = 255; bmp[59] = 0;
    // Padding: bmp[60], bmp[61] = 0

    // Row 1 (top row): 2 pixels (BGR) + 2 bytes padding
    // Px 0: Red (0, 0, 255)
    bmp[62] = 0; bmp[63] = 0; bmp[64] = 255;
    // Px 1: White (255, 255, 255)
    bmp[65] = 255; bmp[66] = 255; bmp[67] = 255;
    // Padding: bmp[68], bmp[69] = 0

    let png_out = convert_bmp_to_png(&bmp).expect("convert 2x2 bmp to png");
    assert_eq!(&png_out[0..8], &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

    // Negative tests:
    // Truncated buffer
    assert!(convert_bmp_to_png(&bmp[..40]).is_err());
    // Bad signature
    let mut bad_sig = bmp.clone();
    bad_sig[0] = 0x58;
    assert!(convert_bmp_to_png(&bad_sig).is_err());
    // Unsupported bpp (e.g. 16 bpp)
    let mut bad_bpp = bmp.clone();
    bad_bpp[28..30].copy_from_slice(&16u16.to_le_bytes());
    assert!(convert_bmp_to_png(&bad_bpp).is_err());
    // Unsupported compression (e.g. BI_RLE8 = 1)
    let mut bad_comp = bmp.clone();
    bad_comp[30..34].copy_from_slice(&1u32.to_le_bytes());
    assert!(convert_bmp_to_png(&bad_comp).is_err());
}

#[test]
fn test_ocrm_media_store_shared_invariants() {
    // Mode invariant
    assert_eq!(MEDIA_FILE_MODE, 0o644);

    // format_media_size legacy-binary formatting with mega cap
    assert_eq!(format_media_size(0), "0B");
    assert_eq!(format_media_size(512), "512B");
    assert_eq!(format_media_size(1024), "1KB");
    assert_eq!(format_media_size(1536), "1.50KB");
    assert_eq!(format_media_size(10 * 1024 * 1024), "10MB");
    assert_eq!(
        format_media_size(25 * 1024 * 1024 + 512 * 1024),
        "25.50MB"
    );
    // Mega cap
    assert_eq!(format_media_size(1024 * 1024 * 1024), "1024MB");

    // SaveMediaSourceError
    let err = SaveMediaSourceError::too_large(25 * 1024 * 1024);
    assert_eq!(err.code, SaveMediaSourceErrorCode::TooLarge);
    assert_eq!(err.code.as_str(), "too-large");
    assert_eq!(err.message, "Media exceeds 25MB limit");
    assert_eq!(err.to_string(), "too-large: Media exceeds 25MB limit");

    // Verify all error codes
    assert_eq!(SaveMediaSourceErrorCode::InvalidPath.as_str(), "invalid-path");
    assert_eq!(SaveMediaSourceErrorCode::NotFound.as_str(), "not-found");
    assert_eq!(SaveMediaSourceErrorCode::NotFile.as_str(), "not-file");
    assert_eq!(SaveMediaSourceErrorCode::PathMismatch.as_str(), "path-mismatch");
    assert_eq!(SaveMediaSourceErrorCode::TooLarge.as_str(), "too-large");
}

struct TestAuth;
impl mahayana_host_runtime::extensions::attachments::generate_image_service::GenerateImageAuth for TestAuth {
    fn get_access_token(&self) -> Result<String, String> {
        Ok("token".into())
    }
    fn get_machine_id(&self) -> Result<String, String> {
        Ok("machine".into())
    }
}

#[derive(Default)]
struct TestBox;
impl mahayana_host_runtime::extensions::attachments::attachments_service::AttachmentsBox for TestBox {
    fn run_state(&self, _agent_id: &str) -> Result<String, String> {
        Ok("running".into())
    }
    fn upload_file(&self, _agent_id: &str, _path: &str, _data: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn test_ocrm_qr_png_gateway_dispatch() {
    use std::sync::Arc;
    use serde_json::json;
    use mahayana_host_runtime::extensions::attachments::attachments_service::AttachmentsService;

    let tmp = temp_dir("gateway_dispatch");
    let service = AttachmentsService::new(
        tmp.clone(),
        Arc::new(TestAuth),
        Arc::new(TestBox::default()),
        None,
    );

    // 1. renderQrTerminal dispatch
    let term_args = json!({
        "input": "gateway:terminal_qr_test",
        "small": true
    });
    let term_res = service
        .dispatch_gateway("renderQrTerminal", &term_args)
        .expect("handler exists")
        .expect("terminal qr render success");
    assert!(term_res.as_str().unwrap().starts_with(TERMINAL_BLACK_ON_WHITE));

    // 2. renderQrPngDataUrl dispatch
    let png_args = json!({
        "input": "gateway:png_data_url_test",
        "scale": 4,
        "marginModules": 2
    });
    let png_res = service
        .dispatch_gateway("renderQrPngDataUrl", &png_args)
        .expect("handler exists")
        .expect("png data url success");
    assert!(png_res.as_str().unwrap().starts_with(QR_PNG_DATA_URL_PREFIX));

    // 3. formatMediaSize dispatch
    let size_args = json!({ "bytes": 10 * 1024 * 1024 });
    let size_res = service
        .dispatch_gateway("formatMediaSize", &size_args)
        .expect("handler exists")
        .expect("formatMediaSize success");
    assert_eq!(size_res.as_str().unwrap(), "10MB");

    // 4. convertBmpToPng dispatch
    let mut bmp = vec![0u8; 70];
    bmp[0] = 0x42; bmp[1] = 0x4D;
    bmp[2..6].copy_from_slice(&70u32.to_le_bytes());
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&2i32.to_le_bytes());
    bmp[22..26].copy_from_slice(&2i32.to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
    bmp[30..34].copy_from_slice(&0u32.to_le_bytes());
    bmp[54] = 255; bmp[55] = 0; bmp[56] = 0;
    bmp[57] = 0; bmp[58] = 255; bmp[59] = 0;
    bmp[62] = 0; bmp[63] = 0; bmp[64] = 255;
    bmp[65] = 255; bmp[66] = 255; bmp[67] = 255;

    let b64_bmp = base64::prelude::BASE64_STANDARD.encode(&bmp);
    let bmp_args = json!({ "bytesBase64": b64_bmp });
    let bmp_res = service
        .dispatch_gateway("convertBmpToPng", &bmp_args)
        .expect("handler exists")
        .expect("convertBmpToPng success");
    let decoded_png = base64::prelude::BASE64_STANDARD
        .decode(bmp_res.as_str().unwrap())
        .expect("decode converted png");
    assert_eq!(&decoded_png[0..8], &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

    let _ = std::fs::remove_dir_all(&tmp);
}
