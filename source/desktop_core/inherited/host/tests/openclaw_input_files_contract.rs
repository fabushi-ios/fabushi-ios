//! Contract test suite for OpenClaw Input Files & Staged Inputs (`OCRM-UNIT-MEDIA-014`).
//!
//! Validates:
//! - Staged input directory prefix, ownership markers, sha256 verification, and path specs.
//! - Staged input alias extraction, duplicate disambiguation, and media path mapping.
//! - Text decoding across UTF-8, Latin-1, Windows-1251, Windows-1252, and UTF-16LE.
//! - BOM-detected UTF-16 precedence over declared charsets.
//! - UTF-16 surrogate-safe truncation and text truncation metadata flags.
//! - Rejection of disallowed URL sources and oversized files/images.
//! - Video byte classification and image allowlist rejection (AVI -> video/x-msvideo).
//! - HEIC / HEIF image normalization to JPEG.
//! - Gateway dispatch integration for input file and staged input methods.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use mahayana_host_runtime::extensions::attachments::attachments_service::{
    AttachmentsBox, AttachmentsService,
};
use mahayana_host_runtime::extensions::attachments::generate_image_service::GenerateImageAuth;
use mahayana_host_runtime::extensions::attachments::input_files::{
    classify_attachment_bytes, extract_file_content_from_buffer,
    extract_file_content_from_source, extract_image_content_from_source,
    normalize_input_image_buffer, resolve_input_file_limits, slice_utf16_safe,
    truncate_utf16_safe, AttachmentClass, InputFileLimitsConfig, InputFileSource,
    InputImageLimits, InputImageSource, DEFAULT_INPUT_IMAGE_MIMES,
};
use mahayana_host_runtime::extensions::attachments::media_facts::MediaFact;
use mahayana_host_runtime::extensions::attachments::staged_inputs::{
    ensure_staged_input_directory, is_staged_input_path, resolve_staged_input_media_paths,
    staged_input_directories_from_entries, staged_input_directory, staged_input_path_directory,
    StagedManifestEntry, STAGED_INPUT_DIRECTORY_PREFIX, STAGED_INPUT_GITIGNORE,
    STAGED_INPUT_GITIGNORE_SHA256,
};
use serde_json::Value;

struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ocrm-input-files-test-{name}-{}", uuid::Uuid::new_v4()));
        let _ = fs::create_dir_all(&path);
        Self { path }
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct MockAuth;
impl GenerateImageAuth for MockAuth {
    fn get_access_token(&self) -> Result<String, String> {
        Ok("token".into())
    }
    fn get_machine_id(&self) -> Result<String, String> {
        Ok("machine".into())
    }
}

struct MockBox;
impl AttachmentsBox for MockBox {
    fn run_state(&self, _agent_id: &str) -> Result<String, String> {
        Ok("running".into())
    }
    fn upload_file(&self, _agent_id: &str, _path: &str, _data: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

fn create_test_service(root: PathBuf) -> AttachmentsService {
    AttachmentsService::new(
        root,
        Arc::new(MockAuth),
        Arc::new(MockBox),
        None,
    )
}

#[test]
fn test_staged_input_directory_and_ownership() {
    let hex_id = "c".repeat(64);
    let uuid_id = "11111111-1111-4111-8111-111111111111";

    assert_eq!(
        staged_input_directory(&hex_id),
        format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex_id}")
    );

    let hex_dir = staged_input_path_directory(&format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex_id}/input-secret.txt"));
    assert_eq!(hex_dir, Some(format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex_id}")));

    let uuid_dir = staged_input_path_directory(&format!("{STAGED_INPUT_DIRECTORY_PREFIX}{uuid_id}/file.pyc"));
    assert_eq!(uuid_dir, Some(format!("{STAGED_INPUT_DIRECTORY_PREFIX}{uuid_id}")));

    // Rejection of invalid candidate identities
    assert_eq!(staged_input_path_directory("media/inbound/other-dir/file.txt"), None);
    assert_eq!(staged_input_path_directory(&format!("{STAGED_INPUT_DIRECTORY_PREFIX}short/file.txt")), None);
    assert_eq!(staged_input_path_directory(&format!("{STAGED_INPUT_DIRECTORY_PREFIX}{}/file.txt", "C".repeat(64))), None);

    let mut dirs = HashSet::new();
    dirs.insert(format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex_id}"));
    assert!(is_staged_input_path(&format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex_id}/data.bin"), &dirs));
    assert!(!is_staged_input_path(&format!("{STAGED_INPUT_DIRECTORY_PREFIX}{uuid_id}/data.bin"), &dirs));

    // Staged manifest entries verification
    let entries = vec![
        StagedManifestEntry {
            path: format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex_id}/.gitignore"),
            r#type: "file".into(),
            size: Some(STAGED_INPUT_GITIGNORE.len()),
            sha256: Some(STAGED_INPUT_GITIGNORE_SHA256.into()),
        },
        StagedManifestEntry {
            path: format!("{STAGED_INPUT_DIRECTORY_PREFIX}{uuid_id}/.gitignore"),
            r#type: "file".into(),
            size: Some(10), // Wrong size
            sha256: Some(STAGED_INPUT_GITIGNORE_SHA256.into()),
        },
    ];
    let matched_dirs = staged_input_directories_from_entries(&entries);
    assert_eq!(matched_dirs.len(), 1);
    assert!(matched_dirs.contains(&format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex_id}")));

    // Ensure directory creation and ownership enforcement
    let tmp = TempTestDir::new("staged-dir-ownership");
    let target_dir = format!("{STAGED_INPUT_DIRECTORY_PREFIX}{uuid_id}");
    ensure_staged_input_directory(&tmp.path, &target_dir).expect("Directory should be created");

    let ignore_path = tmp.path.join(&target_dir).join(".gitignore");
    assert!(ignore_path.exists());
    assert_eq!(fs::read_to_string(&ignore_path).unwrap(), STAGED_INPUT_GITIGNORE);

    // Idempotent re-entry
    ensure_staged_input_directory(&tmp.path, &target_dir).expect("Idempotent check should succeed");

    // Tampering with .gitignore must fail ownership check
    fs::write(&ignore_path, "tampered content").unwrap();
    let err = ensure_staged_input_directory(&tmp.path, &target_dir).unwrap_err();
    assert!(err.contains("not owned by OpenClaw"));
}

#[test]
fn test_resolve_staged_input_media_paths() {
    let hex_id = "e".repeat(64);
    let dir = staged_input_directory(&hex_id);

    let p1 = format!("{dir}/input-file_analysis.csv");
    let p2 = format!("{dir}/input-file_dup.txt");
    let p3 = format!("{dir}/input-file_dup.json"); // Duplicate alias "file_dup"

    let facts = vec![
        MediaFact {
            path: Some(p1.clone()),
            staged: Some(true),
            ..Default::default()
        },
        MediaFact {
            path: Some(p2.clone()),
            staged: Some(true),
            ..Default::default()
        },
        MediaFact {
            path: Some(p3.clone()),
            staged: Some(true),
            ..Default::default()
        },
        // Not staged
        MediaFact {
            path: Some(format!("{dir}/input-file_not_staged.txt")),
            staged: Some(false),
            ..Default::default()
        },
        // Does not match file_ pattern
        MediaFact {
            path: Some(format!("{dir}/input-custom_name.txt")),
            staged: Some(true),
            ..Default::default()
        },
    ];

    let resolved = resolve_staged_input_media_paths(Some(&facts));
    assert_eq!(resolved.get("file_analysis.csv"), Some(&p1));
    assert_eq!(resolved.get("file_analysis"), Some(&p1));
    // Exact filenames with extensions remain unambiguous
    assert_eq!(resolved.get("file_dup.txt"), Some(&p2));
    assert_eq!(resolved.get("file_dup.json"), Some(&p3));
    // The shared stem "file_dup" is ambiguous and must be removed
    assert_eq!(resolved.get("file_dup"), None);
}

#[test]
fn test_named_attachment_decoding_and_charsets() {
    // UTF-8 decoding
    let utf8_text = "Café notes: résumé et météo pour demain.";
    let limits = resolve_input_file_limits(None);
    let res = extract_file_content_from_buffer(
        utf8_text.as_bytes(),
        Some("notes.txt"),
        Some("text/plain"),
        None,
        &limits,
        None,
    ).expect("UTF-8 text extraction should succeed");
    assert_eq!(res.text.as_deref(), Some(utf8_text));
    assert_eq!(res.metadata.as_ref().and_then(|m| m.text_truncated), None);

    // Windows-1252 decoding ("café €": 0x63, 0x61, 0x66, 0xe9, 0x20, 0x80)
    let w1252_bytes = vec![0x63, 0x61, 0x66, 0xe9, 0x20, 0x80];
    let res_w1252 = extract_file_content_from_buffer(
        &w1252_bytes,
        Some("data.txt"),
        Some("text/plain; charset=\"windows-1252\""),
        None,
        &limits,
        None,
    ).expect("Windows-1252 decode should succeed");
    assert_eq!(res_w1252.text.as_deref(), Some("café €"));

    // Recovery from unclosed quote in charset: text/plain; charset="windows-1252
    let res_unclosed = extract_file_content_from_buffer(
        &w1252_bytes,
        Some("data.txt"),
        Some("text/plain; charset=\"windows-1252"),
        None,
        &limits,
        None,
    ).expect("Unclosed quote recovery should succeed");
    assert_eq!(res_unclosed.text.as_deref(), Some("café €"));

    // Byte-detected UTF-16 with BOM overrides declared Windows-1252
    let mut utf16_bom = vec![0xff, 0xfe]; // UTF-16LE BOM
    for u in "café €".encode_utf16() {
        utf16_bom.extend_from_slice(&u.to_le_bytes());
    }
    let res_utf16 = extract_file_content_from_buffer(
        &utf16_bom,
        Some("utf16.txt"),
        Some("text/plain; charset=\"windows-1252\""),
        None,
        &limits,
        None,
    ).expect("UTF-16LE BOM decode should succeed");
    assert_eq!(res_utf16.text.as_deref(), Some("café €"));
}

#[test]
fn test_input_file_limits_and_truncation() {
    let limits_cfg = InputFileLimitsConfig {
        max_chars: Some(14),
        allowed_mimes: Some(vec!["text/plain".into()]),
        ..Default::default()
    };
    let limits = resolve_input_file_limits(Some(&limits_cfg));
    assert_eq!(limits.max_chars, 14);

    let content = "VISIBLE_PREFIX|INVISIBLE_TAIL";
    let res = extract_file_content_from_buffer(
        content.as_bytes(),
        Some("long.txt"),
        Some("text/plain"),
        None,
        &limits,
        None,
    ).expect("Extraction should succeed with truncation");

    assert_eq!(res.text.as_deref(), Some("VISIBLE_PREFIX"));
    let meta = res.metadata.expect("Metadata should be present when truncated");
    assert_eq!(meta.text_truncated, Some(true));
    assert_eq!(meta.images_truncated, Some(false));

    // UTF-16 safe surrogate slicing
    let emoji_text = "Hello 😀 world";
    assert_eq!(slice_utf16_safe(emoji_text, 0, Some(7)), "Hello "); // Cut before surrogate
    assert_eq!(slice_utf16_safe(emoji_text, 0, Some(8)), "Hello 😀");
    assert_eq!(truncate_utf16_safe(emoji_text, 7), "Hello ");
    assert_eq!(truncate_utf16_safe(emoji_text, 8), "Hello 😀");
}

#[test]
fn test_input_source_and_image_validation() {
    // Disabled URL sources
    let limits = resolve_input_file_limits(Some(&InputFileLimitsConfig {
        allow_url: Some(false),
        ..Default::default()
    }));

    let url_source = InputFileSource::Url {
        url: "https://example.com/file.txt".into(),
        media_type: Some("text/plain".into()),
        filename: Some("file.txt".into()),
    };
    let err = extract_file_content_from_source(&url_source, &limits).unwrap_err();
    assert!(err.contains("URL sources are disabled by config"));

    // File too large rejection
    let big_data = BASE64_STANDARD.encode(vec![0u8; 2048]);
    let big_source = InputFileSource::Base64 {
        data: big_data,
        media_type: Some("text/plain".into()),
        filename: Some("big.txt".into()),
    };
    let tiny_limits = resolve_input_file_limits(Some(&InputFileLimitsConfig {
        max_bytes: Some(1024),
        ..Default::default()
    }));
    let big_err = extract_file_content_from_source(&big_source, &tiny_limits).unwrap_err();
    assert!(big_err.contains("too large"));

    // AVI byte detection and rejection from image allowlist
    let mut avi_bytes = Vec::from(&b"RIFF\x38\x00\x00\x00AVI "[..]);
    avi_bytes.resize(64, 0);

    let classification = classify_attachment_bytes(&avi_bytes, None, None, Some("clip.avi"));
    assert_eq!(classification.class, AttachmentClass::Video);
    assert_eq!(classification.mime.as_deref(), Some("video/x-msvideo"));

    let image_limits = InputImageLimits {
        allow_url: false,
        url_allowlist: None,
        allowed_mimes: DEFAULT_INPUT_IMAGE_MIMES.iter().map(|s| s.to_string()).collect(),
        max_bytes: 1024 * 1024,
        max_redirects: 0,
        timeout_ms: 1000,
    };

    let avi_img_source = InputImageSource::Base64 {
        data: BASE64_STANDARD.encode(&avi_bytes),
        media_type: Some("image/jpeg".into()),
    };
    let avi_err = extract_image_content_from_source(&avi_img_source, &image_limits).unwrap_err();
    assert!(avi_err.contains("Unsupported image MIME type: video/x-msvideo"));

    // HEIC image normalization
    let heic_dummy = vec![0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c'];
    let normalized = normalize_input_image_buffer(&heic_dummy, Some("image/heic"), &image_limits)
        .expect("HEIC normalization should succeed");
    assert_eq!(normalized.mime_type, "image/jpeg");
    assert!(normalized.buffer.starts_with(b"\xff\xd8\xff"));
}

#[test]
fn test_gateway_dispatch_input_files_and_staged_inputs() {
    let tmp = TempTestDir::new("gateway-dispatch-input-files");
    let service = create_test_service(tmp.path.clone());

    // 1. resolveInputFileLimits
    let limits_res = service.dispatch_gateway(
        "resolveInputFileLimits",
        &serde_json::json!({ "maxChars": 5000, "allowedMimes": ["text/plain", "application/json"] }),
    ).expect("resolveInputFileLimits dispatch");
    let limits_val = limits_res.expect("Value returned");
    assert_eq!(limits_val.get("maxChars").and_then(Value::as_u64), Some(5000));

    // 2. extractFileContentFromBuffer
    let text = "Hello OpenClaw Unit 27 Staged Inputs";
    let b64 = BASE64_STANDARD.encode(text.as_bytes());
    let extract_res = service.dispatch_gateway(
        "extractFileContentFromBuffer",
        &serde_json::json!({
            "bufferBase64": b64,
            "filename": "hello.txt",
            "mimeType": "text/plain",
            "limits": limits_val
        }),
    ).expect("extractFileContentFromBuffer dispatch").expect("extract result");
    assert_eq!(extract_res.get("text").and_then(Value::as_str), Some(text));

    // 3. stagedInputPathDirectory
    let hex = "f".repeat(64);
    let path_res = service.dispatch_gateway(
        "stagedInputPathDirectory",
        &serde_json::json!({ "relativePath": format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex}/item.txt") }),
    ).expect("stagedInputPathDirectory dispatch").expect("directory result");
    assert_eq!(path_res.as_str(), Some(format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex}").as_str()));

    // 4. isStagedInputPath
    let dirs_json = serde_json::json!([format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex}")]);
    let match_res = service.dispatch_gateway(
        "isStagedInputPath",
        &serde_json::json!({
            "relativePath": format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex}/item.txt"),
            "directories": dirs_json
        }),
    ).expect("isStagedInputPath dispatch").expect("bool result");
    assert_eq!(match_res.as_bool(), Some(true));

    // 5. ensureStagedInputDirectory
    let dir_name = format!("{STAGED_INPUT_DIRECTORY_PREFIX}{hex}");
    let ensure_res = service.dispatch_gateway(
        "ensureStagedInputDirectory",
        &serde_json::json!({
            "rootDir": tmp.path.to_str().unwrap(),
            "directory": dir_name
        }),
    ).expect("ensureStagedInputDirectory dispatch").expect("success result");
    assert_eq!(ensure_res.get("success").and_then(Value::as_bool), Some(true));
}
