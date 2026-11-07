use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use serde_json::json;

use mahayana_host_runtime::extensions::attachments::anthropic_inline_images::{
    normalize_anthropic_inline_content_blocks, AnthropicContentBlock,
};
use mahayana_host_runtime::extensions::attachments::attachments_service::{
    AttachmentsBox, AttachmentsService,
};
use mahayana_host_runtime::extensions::attachments::generate_image_service::GenerateImageAuth;
use mahayana_host_runtime::extensions::attachments::remote_workspace_file::{
    read_bounded_remote_workspace_file, RemoteWorkspaceFileError, RemoteWorkspaceFileParams,
};
use mahayana_host_runtime::extensions::attachments::web_media::{
    assert_no_windows_network_path, file_url_to_path, load_web_media,
    optimize_image_buffer_for_web_media, strip_legacy_media_directive_prefix,
    WebMediaOptions,
};

const WEB_MEDIA_SOURCE: &str = include_str!("../src/extensions/attachments/web_media.rs");

struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ocrm-web-media-test-{name}-{}", uuid::Uuid::new_v4()));
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
fn test_ocrm_anthropic_inline_content_blocks_normalization() {
    // 1. Text blocks preserved
    let blocks = vec![
        AnthropicContentBlock::Text {
            text: "Hello Anthropic".to_string(),
        },
        AnthropicContentBlock::Image {
            // One red pixel uncompressed 24-bit BMP
            data: "Qk06AAAAAAAAADYAAAAoAAAAAQAAAAEAAAABABgAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAD/AA==".to_string(),
            mime_type: "image/bmp".to_string(),
        },
    ];

    let normalized = normalize_anthropic_inline_content_blocks(&blocks)
        .expect("normalize content blocks succeeded");

    assert_eq!(normalized.len(), 2);
    match &normalized[0] {
        AnthropicContentBlock::Text { text } => assert_eq!(text, "Hello Anthropic"),
        _ => panic!("Expected text block"),
    }
    match &normalized[1] {
        AnthropicContentBlock::Image { data, mime_type } => {
            assert_eq!(mime_type, "image/png");
            use base64::Engine as _;
            let png_bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .expect("valid png base64");
            assert_eq!(&png_bytes[0..8], b"\x89PNG\r\n\x1a\n");
        }
        _ => panic!("Expected image block"),
    }

    // 2. Reject oversized input (> 10MB) before decode
    let huge_data = "A".repeat(14 * 1024 * 1024);
    let oversized_blocks = vec![AnthropicContentBlock::Image {
        data: huge_data,
        mime_type: "image/jpeg".to_string(),
    }];
    let err = normalize_anthropic_inline_content_blocks(&oversized_blocks).unwrap_err();
    assert!(err.contains("10 MB decoded safety limit"));
}

#[test]
fn test_ocrm_web_media_animated_webp_preservation() {
    // Valid animated WebP buffer with VP8X animation flag bit set
    let mut webp = Vec::new();
    webp.extend_from_slice(b"RIFF");
    webp.extend_from_slice(&32u32.to_le_bytes());
    webp.extend_from_slice(b"WEBP");
    webp.extend_from_slice(b"VP8X");
    webp.extend_from_slice(&10u32.to_le_bytes());
    webp.push(0x02); // animation bit 1 set
    webp.extend_from_slice(&[0u8; 15]);

    let res = optimize_image_buffer_for_web_media(
        webp.clone(),
        Some("image/webp"),
        Some("animated.webp"),
        Some(1024),
        None,
    )
    .expect("animated WebP preserved");

    assert_eq!(res.content_type.as_deref(), Some("image/webp"));
    assert_eq!(res.kind.as_deref(), Some("image"));
    assert_eq!(res.buffer, webp);

    // Over-budget animated WebP rejection
    let over_budget_err = optimize_image_buffer_for_web_media(
        webp.clone(),
        Some("image/webp"),
        Some("animated.webp"),
        Some(10), // cap = 10 bytes, smaller than buffer length
        None,
    )
    .unwrap_err();

    match over_budget_err {
        mahayana_host_runtime::extensions::attachments::web_media::WebMediaError::OptimizationLimit(e) => {
            assert!(e.message.contains("Animated WebP"));
            assert_eq!(e.max_bytes, 10);
        }
        other => panic!("Expected OptimizationLimit error, got: {other:?}"),
    }
}

#[test]
fn test_ocrm_web_media_file_url_and_traversal_guards() {
    // 1. strip_legacy_media_directive_prefix
    assert_eq!(
        strip_legacy_media_directive_prefix("MEDIA: file:///path/to/img.png"),
        "file:///path/to/img.png"
    );
    assert_eq!(
        strip_legacy_media_directive_prefix("media://inbound/asset-123.jpg"),
        "media://inbound/asset-123.jpg"
    );

    // 2. assert_no_windows_network_path
    assert!(assert_no_windows_network_path(r"\\attacker\share\evil.png", "test").is_err());
    assert!(assert_no_windows_network_path("//attacker/share/evil.png", "test").is_err());
    assert!(assert_no_windows_network_path("/safe/path/img.png", "test").is_ok());

    // 3. file_url_to_path
    let decoded_path = file_url_to_path("FILE:///var/tmp/caf%C3%A9%20image.png")
        .expect("decode file URL");
    assert!(decoded_path.to_string_lossy().contains("café image.png"));

    // Traversal rejection in file URL
    assert!(file_url_to_path("FILE:///C:/safe/folder%2Fsecret.png").is_err());
    assert!(file_url_to_path("FILE:///C:/safe/folder%5Csecret.png").is_err());
    assert!(file_url_to_path("FILE:////attacker/share/evil.png").is_err());
}

#[test]
fn test_ocrm_web_media_local_loading_and_roots_boundary() {
    let temp = TempTestDir::new("web-media-roots");
    let allowed_dir = temp.path.join("allowed");
    let forbidden_dir = temp.path.join("forbidden");
    fs::create_dir_all(&allowed_dir).unwrap();
    fs::create_dir_all(&forbidden_dir).unwrap();

    let safe_file = allowed_dir.join("test.png");
    // Minimal 1x1 PNG bytes
    let png_bytes = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
        0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
        0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
        0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
        0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    fs::write(&safe_file, png_bytes).unwrap();

    let forbidden_file = forbidden_dir.join("secret.png");
    fs::write(&forbidden_file, png_bytes).unwrap();

    let opts = WebMediaOptions {
        local_roots: Some(vec![allowed_dir.clone()]),
        optimize_images: false,
        ..Default::default()
    };

    // Allowed read succeeds
    let res = load_web_media(&safe_file.to_string_lossy(), &opts).expect("load safe file");
    assert_eq!(res.buffer, png_bytes);

    // Forbidden read outside allowed local_roots fails with path-not-allowed
    let err = load_web_media(&forbidden_file.to_string_lossy(), &opts).unwrap_err();
    match err {
        mahayana_host_runtime::extensions::attachments::web_media::WebMediaError::LocalAccess { code, .. } => {
            assert_eq!(code, "path-not-allowed");
        }
        other => panic!("Expected path-not-allowed LocalAccess, got: {other:?}"),
    }
}

#[test]
fn web_media_local_reads_are_bounded_before_buffer_acceptance() {
    assert!(WEB_MEDIA_SOURCE.contains("read_outbound_media_file_bounded("));
    assert!(!WEB_MEDIA_SOURCE.contains("let buffer = fs::read(&file_path)"));
}

#[test]
fn test_ocrm_remote_workspace_file_transfer() {
    let temp = TempTestDir::new("remote-ws");
    let test_file = temp.path.join("data.bin");
    let data = b"Chunked remote workspace file transfer verification data bytes!";
    fs::write(&test_file, data).unwrap();

    let params = RemoteWorkspaceFileParams {
        path: test_file.clone(),
        max_bytes: 1024,
        workspace_root: Some(temp.path.clone()),
        timeout_ms: Some(5000),
    };

    // Transfer with small cap (1024) to exercise chunking
    let transferred = read_bounded_remote_workspace_file(&params, 1024, || Ok(()))
        .expect("remote transfer succeeded");
    assert_eq!(transferred, data);

    // Reject oversized file
    let small_cap_params = RemoteWorkspaceFileParams {
        path: test_file.clone(),
        max_bytes: 10,
        workspace_root: Some(temp.path.clone()),
        timeout_ms: Some(5000),
    };
    let err = read_bounded_remote_workspace_file(&small_cap_params, 1024, || Ok(())).unwrap_err();
    match err {
        RemoteWorkspaceFileError::FileTooLarge(limit) => assert_eq!(limit, 10),
        other => panic!("Expected FileTooLarge, got: {other:?}"),
    }

    // Closed owner cancels transfer
    let closed_err = read_bounded_remote_workspace_file(&params, 1024, || {
        Err(RemoteWorkspaceFileError::OwnerClosed)
    })
    .unwrap_err();
    match closed_err {
        RemoteWorkspaceFileError::OwnerClosed => {}
        other => panic!("Expected OwnerClosed, got: {other:?}"),
    }
}

#[test]
fn test_ocrm_web_media_gateway_service_dispatch() {
    let temp = TempTestDir::new("gateway-web");
    let service = test_attachments_service(&temp.path);

    // Minimal 1x1 PNG file in temp dir
    let img_path = temp.path.join("dot.png");
    let png_bytes = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
        0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
        0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
        0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
        0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    fs::write(&img_path, png_bytes).unwrap();

    let res = service
        .dispatch_gateway(
            "loadWebMedia",
            &json!({
                "url": img_path.to_string_lossy(),
                "optimizeImages": false
            }),
        )
        .expect("dispatch handled")
        .expect("result ok");

    assert_eq!(res["contentType"], "image/png");
    assert_eq!(res["fileName"], "dot.png");

    // Gateway normalizeAnthropicInlineContentBlocks
    let blocks_res = service
        .dispatch_gateway(
            "normalizeAnthropicInlineContentBlocks",
            &json!({
                "blocks": [
                    { "type": "text", "text": "ping" }
                ]
            }),
        )
        .expect("dispatch handled")
        .expect("result ok");

    assert_eq!(blocks_res, json!([{ "type": "text", "text": "ping" }]));
}
