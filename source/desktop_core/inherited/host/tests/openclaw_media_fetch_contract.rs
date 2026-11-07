//! Contract test suite for OpenClaw Media Fetch Pipeline (`OCRM-UNIT-MEDIA-012`).
//!
//! Validates:
//! - Content-Disposition RFC 5987 / 6266 extended filenames (UTF-8, ISO-8859-1).
//! - Unquoted apostrophes, Windows paths, and malformed extended fallbacks.
//! - URL path basename extraction with malformed percent escape tolerance and slash substitution.
//! - Content-Length header parsing and maxBytes overflow rejection.
//! - MIME container hint resolution (video/mp4 vs audio/mp4) and HTTP charset retention.
//! - Telegram bot token redaction in URLs and error messages.
//! - Empty body / 204 rejection without disk pollution.
//! - Gateway dispatch integration for `readRemoteMediaBuffer` and `saveRemoteMedia`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use mahayana_host_runtime::extensions::attachments::attachments_service::{
    AttachmentsBox, AttachmentsService,
};
use mahayana_host_runtime::extensions::attachments::generate_image_service::GenerateImageAuth;
use mahayana_host_runtime::extensions::attachments::media_fetch::{
    check_ssrf_allowed, is_generic_response_content_type,
    parse_content_disposition_file_name, parse_media_content_length,
    redact_sensitive_text, resolve_remote_file_name,
    resolve_response_content_type, retain_charset_parameter,
    save_response_media, MediaFetchErrorCode, SaveResponseMediaOptions,
};

const MEDIA_FETCH_SOURCE: &str = include_str!("../src/extensions/attachments/media_fetch.rs");

#[test]
fn remote_fetch_entries_use_bounded_response_reads() {
    assert!(MEDIA_FETCH_SOURCE.contains("fn read_bounded_response("));
    assert!(MEDIA_FETCH_SOURCE.contains("take(max_bytes.saturating_add(1) as u64)"));
    assert!(!MEDIA_FETCH_SOURCE.contains("res.bytes()"));
}

struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ocrm-fetch-test-{name}-{}", uuid::Uuid::new_v4()));
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

#[test]
fn test_ocrm_media_fetch_content_disposition_rfc5987_and_fallbacks() {
    // 1. Unquoted filename retains apostrophes
    assert_eq!(
        parse_content_disposition_file_name(Some("attachment; filename='report.csv")),
        Some("'report.csv".to_string())
    );
    assert_eq!(
        parse_content_disposition_file_name(Some("attachment; filename=report.csv'")),
        Some("report.csv'".to_string())
    );
    assert_eq!(
        parse_content_disposition_file_name(Some("attachment; filename='report.csv'")),
        Some("'report.csv'".to_string())
    );

    // 2. Unrelated quoted parameter containing filename is ignored
    assert_eq!(
        parse_content_disposition_file_name(Some(
            r#"attachment; note="x; filename=spoof.csv; y"; filename=safe.csv"#
        )),
        Some("safe.csv".to_string())
    );

    // 3. Mixed Windows path separators preserve final basename
    assert_eq!(
        parse_content_disposition_file_name(Some(r#"attachment; filename="C:/tmp/reports\Q1.csv""#)),
        Some("Q1.csv".to_string())
    );

    // 4. Legacy relative Windows path reduced to basename
    assert_eq!(
        parse_content_disposition_file_name(Some(r#"attachment; filename="reports\Q1.csv""#)),
        Some("Q1.csv".to_string())
    );

    // 5. ISO-8859-1 extended filename
    assert_eq!(
        parse_content_disposition_file_name(Some("attachment; filename*=ISO-8859-1''caf%E9.csv")),
        Some("café.csv".to_string())
    );

    // 6. Malformed extended filename falls back to plain filename
    assert_eq!(
        parse_content_disposition_file_name(Some(
            "attachment; filename=fallback.csv; filename*=UTF-8''%ZZbad.csv"
        )),
        Some("fallback.csv".to_string())
    );

    // 7. Unsupported extended charset (UTF-16) falls back to plain filename
    assert_eq!(
        parse_content_disposition_file_name(Some(
            "attachment; filename*=UTF-16''bad.csv; filename=fallback.csv"
        )),
        Some("fallback.csv".to_string())
    );

    // 8. Extended UTF-8 with decoded separators converted to underscores
    assert_eq!(
        parse_content_disposition_file_name(Some(
            "attachment; filename*=UTF-8''reports%2F%2FQ1.pdf"
        )),
        Some("reports__Q1.pdf".to_string())
    );
}

#[test]
fn test_ocrm_media_fetch_url_basename_and_malformed_percent() {
    // 1. Malformed percent escapes keep raw URL path basename
    let url1 = "https://example.com/files/bad%E0%A4%A.pdf";
    let name1 = resolve_remote_file_name(None, url1, None);
    assert_eq!(name1, Some("bad%E0%A4%A.pdf".to_string()));

    // 2. Decoded URL fallback separators (%5C) replaced with underscores inside basename
    let url2 = "https://example.com/files/reports%5CQ1.pdf";
    let name2 = resolve_remote_file_name(None, url2, None);
    assert_eq!(name2, Some("reports_Q1.pdf".to_string()));

    // 3. File path hint takes precedence over URL
    let url3 = "https://example.com/download?id=123";
    let name3 = resolve_remote_file_name(None, url3, Some("/path/to/custom_file.bin"));
    assert_eq!(name3, Some("custom_file.bin".to_string()));
}

#[test]
fn test_ocrm_media_fetch_content_length_and_max_bytes() {
    // 1. Valid content length
    assert_eq!(parse_media_content_length(Some("1024")), Ok(Some(1024)));
    assert_eq!(parse_media_content_length(None), Ok(None));
    assert_eq!(parse_media_content_length(Some("")), Ok(None));

    // 2. Malformed content length rejects before reads
    let err = parse_media_content_length(Some("1e9")).unwrap_err();
    assert!(err.contains("invalid content-length header: 1e9"));

    // 3. Save response media checks content-length against maxBytes
    let mut headers = BTreeMap::new();
    headers.insert("content-length".to_string(), "5".to_string());
    headers.insert("content-type".to_string(), "application/octet-stream".to_string());

    let opts = SaveResponseMediaOptions {
        source_url: Some("https://example.com/file.bin".to_string()),
        max_bytes: Some(4),
        ..Default::default()
    };
    let res = save_response_media(&[1, 2, 3, 4, 5], &headers, 200, &opts);
    assert!(res.is_err());
    let fetch_err = res.unwrap_err();
    assert_eq!(fetch_err.code, MediaFetchErrorCode::MaxBytes);
    assert!(fetch_err.message.contains("content length 5 exceeds maxBytes 4"));

    // 4. Streamed payload exceeds maxBytes
    let mut headers2 = BTreeMap::new();
    headers2.insert("content-type".to_string(), "application/octet-stream".to_string());
    let opts2 = SaveResponseMediaOptions {
        source_url: Some("https://example.com/large.bin".to_string()),
        max_bytes: Some(4),
        ..Default::default()
    };
    let res2 = save_response_media(&[1, 2, 3, 4, 5], &headers2, 200, &opts2);
    assert!(res2.is_err());
    let fetch_err2 = res2.unwrap_err();
    assert_eq!(fetch_err2.code, MediaFetchErrorCode::MaxBytes);
    assert!(fetch_err2.message.contains("payload exceeds maxBytes 4"));
}

#[test]
fn test_ocrm_media_fetch_content_type_container_hint_and_charset() {
    // 1. Generic content types
    assert!(is_generic_response_content_type(Some("application/octet-stream")));
    assert!(is_generic_response_content_type(Some("binary/octet-stream")));
    assert!(is_generic_response_content_type(Some("application/zip")));
    assert!(is_generic_response_content_type(None));
    assert!(!is_generic_response_content_type(Some("image/png")));

    // 2. Fallback used for generic type
    let resolved = resolve_response_content_type(
        Some("application/octet-stream"),
        Some("text/csv"),
    );
    assert_eq!(resolved, Some("text/csv".to_string()));

    // 3. Audio fallback preserved when header reports matching video container
    let container_resolved = resolve_response_content_type(
        Some("video/mp4"),
        Some("audio/mp4"),
    );
    assert_eq!(container_resolved, Some("audio/mp4".to_string()));

    // 4. HTTP charset retention
    let stored = "application/x-openclaw-charset-proof";
    let header = r#"application/x-openclaw-charset-proof; charset="cp1251""#;
    let retained = retain_charset_parameter(stored, header);
    assert_eq!(retained, "application/x-openclaw-charset-proof;charset=cp1251");

    let stored_utf8 = "text/plain";
    let header_utf8 = "text/plain; charset=gbk";
    assert_eq!(retain_charset_parameter(stored_utf8, header_utf8), "text/plain;charset=gbk");
}

#[test]
fn test_ocrm_media_fetch_ssrf_and_redaction() {
    // 1. Redact bot token from URL and errors
    let bot_token = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZabcd";
    let raw_url = format!("https://files.example.test/file/bot{bot_token}/photos/1.jpg");
    let redacted = redact_sensitive_text(&raw_url);
    assert!(!redacted.contains(bot_token));
    assert_eq!(
        redacted,
        "https://files.example.test/file/bot123456…abcd/photos/1.jpg"
    );

    // 2. SSRF check blocks 127.0.0.1 by default
    let blocked = check_ssrf_allowed("http://127.0.0.1/sensitive.bin", None, None, false);
    assert!(blocked.is_err());
    let err = blocked.unwrap_err();
    assert_eq!(err.code, MediaFetchErrorCode::FetchFailed);
    assert!(err.message.contains("Blocked hostname or private/internal/special-use IP address"));

    // 3. SSRF check allows explicit origin
    let allowed = check_ssrf_allowed(
        "http://127.0.0.1:8080/media",
        Some(&["http://127.0.0.1:8080".to_string()]),
        None,
        false,
    );
    assert!(allowed.is_ok());
}

#[test]
fn test_ocrm_media_fetch_empty_response_and_original_filename() {
    let temp = TempTestDir::new("empty-and-orig");

    // 1. Empty body or 204 rejected with HTTP 204; empty response body
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/octet-stream".to_string());
    let opts_204 = SaveResponseMediaOptions {
        source_url: Some("https://example.com/empty".to_string()),
        base_dir: Some(temp.path.clone()),
        ..Default::default()
    };
    let res_204 = save_response_media(&[], &headers, 204, &opts_204);
    assert!(res_204.is_err());
    let err_204 = res_204.unwrap_err();
    assert_eq!(err_204.code, MediaFetchErrorCode::HttpError);
    assert_eq!(err_204.status, Some(204));
    assert!(err_204.message.contains("empty response body"));

    // 2. Preserves explicit original filename when saving stream
    let bytes = b"openclaw document test contents";
    let mut headers_docx = BTreeMap::new();
    headers_docx.insert(
        "content-type".to_string(),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
    );
    let opts_docx = SaveResponseMediaOptions {
        source_url: Some("https://example.com/views/original".to_string()),
        original_filename: Some("document.docx".to_string()),
        base_dir: Some(temp.path.clone()),
        ..Default::default()
    };
    let saved = save_response_media(bytes, &headers_docx, 200, &opts_docx).unwrap();
    assert_eq!(saved.file_name, Some("document.docx".to_string()));
    assert!(saved.path.contains("document---"));
    assert!(saved.path.ends_with(".docx"));
    assert_eq!(fs::read(&saved.path).unwrap(), bytes);
}

#[test]
fn test_ocrm_media_fetch_gateway_dispatch() {
    let temp = TempTestDir::new("gateway-dispatch");
    let service = AttachmentsService::new(
        &temp.path,
        Arc::new(MockAuth),
        Arc::new(MockBox),
        None,
    );

    // 1. Dispatch saveRemoteMedia with blocked localhost url
    let args = serde_json::json!({
        "url": "http://127.0.0.1/malicious.bin",
        "maxBytes": 1024
    });
    let result = service.dispatch_gateway("saveRemoteMedia", &args);
    assert!(result.is_some());
    let res = result.unwrap();
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("Blocked hostname or private/internal/special-use IP address"));

    // 2. Dispatch readRemoteMediaBuffer with blocked localhost url
    let result2 = service.dispatch_gateway("readRemoteMediaBuffer", &args);
    assert!(result2.is_some());
    let res2 = result2.unwrap();
    assert!(res2.is_err());
    assert!(res2.unwrap_err().contains("Blocked hostname or private/internal/special-use IP address"));
}
