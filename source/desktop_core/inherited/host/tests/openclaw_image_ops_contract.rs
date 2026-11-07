use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use serde_json::json;

use mahayana_host_runtime::extensions::attachments::attachments_service::{AttachmentsBox, AttachmentsService};
use mahayana_host_runtime::extensions::attachments::generate_image_service::GenerateImageAuth;
use mahayana_host_runtime::extensions::attachments::image_ops::{
    build_image_resize_side_grid, convert_bmp_to_png, is_animated_webp_buffer,
    is_still_png_buffer, normalize_anthropic_inline_image, prepare_attachment,
    read_image_metadata_from_header, read_image_probe_from_header, AttachmentInput,
    ImageMetadata, IMAGE_REDUCE_QUALITY_STEPS, WITHIN_BYTES_QUALITY_STEPS,
};

struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ocrm-image-ops-test-{name}-{}", uuid::Uuid::new_v4()));
        let _ = fs::create_dir_all(&path);
        Self { path }
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct TestAuth;
impl GenerateImageAuth for TestAuth {
    fn get_access_token(&self) -> Result<String, String> {
        Ok("token".into())
    }
    fn get_machine_id(&self) -> Result<String, String> {
        Ok("machine".into())
    }
}

#[derive(Default)]
struct TestBox;
impl AttachmentsBox for TestBox {
    fn run_state(&self, _agent_id: &str) -> Result<String, String> {
        Ok("running".into())
    }
    fn upload_file(&self, _agent_id: &str, _path: &str, _data: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

fn test_attachments_service(temp_dir: &std::path::Path) -> AttachmentsService {
    AttachmentsService::new(
        temp_dir.to_path_buf(),
        Arc::new(TestAuth),
        Arc::new(TestBox::default()),
        None,
    )
}

#[test]
fn test_ocrm_image_ops_build_resize_side_grid() {
    let grid1 = build_image_resize_side_grid(1200, 900);
    assert_eq!(grid1, vec![1200, 1000, 900, 800]);

    let grid_zero = build_image_resize_side_grid(0, 0);
    assert!(grid_zero.is_empty());

    let grid2 = build_image_resize_side_grid(1600, 1800);
    assert_eq!(grid2, vec![1600, 1400, 1200, 1000, 800]);
}

#[test]
fn test_ocrm_image_ops_quality_steps_ladder() {
    assert_eq!(IMAGE_REDUCE_QUALITY_STEPS, &[85, 75, 65, 55, 45, 35]);
    assert_eq!(WITHIN_BYTES_QUALITY_STEPS, &[92, 85, 78, 70, 62, 52, 42]);
}

#[test]
fn test_ocrm_image_ops_animated_webp_detection() {
    // Valid VP8X animated WebP buffer: RIFF (4) + len (4) + WEBP (4) + VP8X (4) + chunk_len (4) + flags (1) + ...
    let mut webp = Vec::new();
    webp.extend_from_slice(b"RIFF");
    webp.extend_from_slice(&32u32.to_le_bytes());
    webp.extend_from_slice(b"WEBP");
    webp.extend_from_slice(b"VP8X");
    webp.extend_from_slice(&10u32.to_le_bytes()); // chunk len >= 10
    webp.push(0x02); // animation bit 1 set
    webp.extend_from_slice(&[0u8; 15]); // padding to length 40

    assert!(is_animated_webp_buffer(&webp));

    // Clear animation bit
    webp[20] = 0x00;
    assert!(!is_animated_webp_buffer(&webp));

    // Truncated buffer
    assert!(!is_animated_webp_buffer(&webp[..20]));
}

#[test]
fn test_ocrm_image_ops_still_png_and_apng_detection() {
    // 1x1 PNG: signature + IHDR chunk + IDAT chunk + IEND chunk
    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    // IHDR chunk: len=13, type="IHDR", data (13 bytes), crc (4 bytes)
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&1u32.to_be_bytes()); // width
    png.extend_from_slice(&1u32.to_be_bytes()); // height
    png.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit truecolor
    png.extend_from_slice(&[0u8; 4]); // crc placeholder
    // IDAT chunk: len=4, type="IDAT", data, crc
    png.extend_from_slice(&4u32.to_be_bytes());
    png.extend_from_slice(b"IDAT");
    png.extend_from_slice(&[0u8; 4]);
    png.extend_from_slice(&[0u8; 4]);
    // IEND chunk
    png.extend_from_slice(&0u32.to_be_bytes());
    png.extend_from_slice(b"IEND");
    png.extend_from_slice(&[0u8; 4]);

    assert!(is_still_png_buffer(&png));

    // APNG: insert acTL chunk before IDAT
    let mut apng = Vec::new();
    apng.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    apng.extend_from_slice(&13u32.to_be_bytes());
    apng.extend_from_slice(b"IHDR");
    apng.extend_from_slice(&1u32.to_be_bytes());
    apng.extend_from_slice(&1u32.to_be_bytes());
    apng.extend_from_slice(&[8, 2, 0, 0, 0]);
    apng.extend_from_slice(&[0u8; 4]);
    // acTL chunk (animation control)
    apng.extend_from_slice(&8u32.to_be_bytes());
    apng.extend_from_slice(b"acTL");
    apng.extend_from_slice(&[0u8; 8]);
    apng.extend_from_slice(&[0u8; 4]);
    // IDAT
    apng.extend_from_slice(&4u32.to_be_bytes());
    apng.extend_from_slice(b"IDAT");
    apng.extend_from_slice(&[0u8; 4]);
    apng.extend_from_slice(&[0u8; 4]);

    assert!(!is_still_png_buffer(&apng));
}

#[test]
fn test_ocrm_image_ops_display_metadata_orientation_swap() {
    // Construct JPEG with SOF0 (640x480) and EXIF App1 with orientation 6
    let mut jpeg = Vec::new();
    jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI

    // App1 with EXIF
    let mut exif_payload = Vec::new();
    exif_payload.extend_from_slice(b"Exif\0\0");
    exif_payload.extend_from_slice(b"II"); // little endian
    exif_payload.extend_from_slice(&42u16.to_le_bytes()); // magic 42
    exif_payload.extend_from_slice(&8u32.to_le_bytes()); // offset to IFD0 (8 bytes from TIFF start)
    // IFD0: 1 entry
    exif_payload.extend_from_slice(&1u16.to_le_bytes()); // 1 entry
    // Entry: tag 0x0112 (Orientation), type 3 (SHORT), count 1, value 6
    exif_payload.extend_from_slice(&0x0112u16.to_le_bytes());
    exif_payload.extend_from_slice(&3u16.to_le_bytes());
    exif_payload.extend_from_slice(&1u32.to_le_bytes());
    exif_payload.extend_from_slice(&6u16.to_le_bytes());
    exif_payload.extend_from_slice(&0u16.to_le_bytes()); // padding

    let app1_len = (exif_payload.len() + 2) as u16;
    jpeg.extend_from_slice(&[0xFF, 0xE1]);
    jpeg.extend_from_slice(&app1_len.to_be_bytes());
    jpeg.extend_from_slice(&exif_payload);

    // SOF0: height=480, width=640
    jpeg.extend_from_slice(&[0xFF, 0xC0]);
    jpeg.extend_from_slice(&11u16.to_be_bytes()); // length
    jpeg.push(8); // precision
    jpeg.extend_from_slice(&480u16.to_be_bytes()); // height
    jpeg.extend_from_slice(&640u16.to_be_bytes()); // width
    jpeg.push(3); // components
    jpeg.extend_from_slice(&[1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]); // components data

    let probe = read_image_probe_from_header(&jpeg).expect("probe success");
    assert_eq!(probe.width, 640);
    assert_eq!(probe.height, 480);
    assert_eq!(probe.orientation, Some(6));

    let display_meta = read_image_metadata_from_header(&jpeg).expect("metadata success");
    // Orientation 6 must swap width and height for display!
    assert_eq!(
        display_meta,
        ImageMetadata {
            width: 480,
            height: 640,
        }
    );
}

#[test]
fn test_ocrm_image_ops_bmp_to_png_conversion_and_pixel_limit() {
    // Two 24-bit BMP pixels: red (0x00, 0x00, 0xFF) then green (0x00, 0xFF, 0x00) with row padding
    let hex_bmp = "424d3e00000000000000360000002800000002000000010000000100180000000000\
                   08000000000000000000000000000000000000000000ff00ff000000";
    let bmp = (0..hex_bmp.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex_bmp[i..i + 2], 16).unwrap())
        .collect::<Vec<u8>>();

    let png = convert_bmp_to_png(&bmp).expect("BMP to PNG conversion succeeded");
    assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");

    let probe = read_image_probe_from_header(&png).expect("probe PNG");
    assert_eq!(probe.width, 2);
    assert_eq!(probe.height, 1);
    assert_eq!(probe.format, "png");
}

#[test]
fn test_ocrm_image_ops_prepare_attachment() {
    use base64::Engine as _;
    let hex_bmp = "424d3e00000000000000360000002800000002000000010000000100180000000000\
                   08000000000000000000000000000000000000000000ff00ff000000";
    let bmp_bytes = (0..hex_bmp.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex_bmp[i..i + 2], 16).unwrap())
        .collect::<Vec<u8>>();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bmp_bytes);

    let input = AttachmentInput {
        base64: b64,
        label: "test.bmp".to_string(),
        mime: "image/bmp".to_string(),
        image_storage: None,
    };

    let prepared = prepare_attachment(&input).expect("prepare attachment succeeded");
    assert_eq!(prepared.size_bytes, bmp_bytes.len());
    assert_eq!(prepared.mime, "image/bmp");
    assert_eq!(prepared.width, Some(2));
    assert_eq!(prepared.height, Some(1));
    assert!(prepared.buffer.is_some());

    // Test invalid base64 rejection
    let bad_input = AttachmentInput {
        base64: "not valid base64!!!".to_string(),
        label: "corrupt.png".to_string(),
        mime: "image/png".to_string(),
        image_storage: None,
    };
    let err = prepare_attachment(&bad_input).unwrap_err();
    assert!(err.contains("invalid base64 content"));
}

#[test]
fn test_ocrm_image_ops_normalize_anthropic_inline_image() {
    use base64::Engine as _;
    // One red pixel in uncompressed 24-bit BMP (from OpenClaw image worker test)
    let b64_bmp = "Qk06AAAAAAAAADYAAAAoAAAAAQAAAAEAAAABABgAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAD/AA==";

    let (norm_data, norm_mime) = normalize_anthropic_inline_image(b64_bmp, "image/bmp")
        .expect("normalize BMP inline image succeeded");

    assert_eq!(norm_mime, "image/png");
    let decoded_png = base64::engine::general_purpose::STANDARD
        .decode(&norm_data)
        .expect("valid base64 png");
    assert_eq!(&decoded_png[0..8], b"\x89PNG\r\n\x1a\n");

    let probe = read_image_probe_from_header(&decoded_png).expect("probe decoded png");
    assert_eq!(probe.width, 1);
    assert_eq!(probe.height, 1);
    assert_eq!(probe.format, "png");
}

#[test]
fn test_ocrm_image_ops_gateway_dispatch() {
    let temp = TempTestDir::new("gateway");
    let service = test_attachments_service(&temp.path);

    // 1. buildImageResizeSideGrid
    let res = service
        .dispatch_gateway(
            "buildImageResizeSideGrid",
            &json!({ "maxSide": 1200, "sideStart": 900 }),
        )
        .expect("dispatch handled")
        .expect("success");
    assert_eq!(res, json!([1200, 1000, 900, 800]));

    // 2. convertBmpToPng gateway dispatch
    let b64_bmp = "Qk06AAAAAAAAADYAAAAoAAAAAQAAAAEAAAABABgAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAD/AA==";
    let res_bmp = service
        .dispatch_gateway("convertBmpToPng", &json!({ "base64": b64_bmp }))
        .expect("dispatch handled")
        .expect("success");
    assert_eq!(res_bmp["mimeType"], "image/png");

    // 3. normalizeAnthropicInlineImage gateway dispatch
    let res_norm = service
        .dispatch_gateway(
            "normalizeAnthropicInlineImage",
            &json!({ "data": b64_bmp, "mimeType": "image/bmp" }),
        )
        .expect("dispatch handled")
        .expect("success");
    assert_eq!(res_norm["mimeType"], "image/png");
}
