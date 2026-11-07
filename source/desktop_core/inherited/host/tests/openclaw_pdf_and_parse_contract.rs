//! Contract tests for Unit 28: PDF Document Content Extraction, Metadata Validation, and Media Output Parsing.

use std::path::PathBuf;
use std::sync::Arc;
use serde_json::json;
use mahayana_host_runtime::extensions::attachments::attachments_service::{
    AttachmentsBox, AttachmentsService,
};
use mahayana_host_runtime::extensions::attachments::generate_image_service::GenerateImageAuth;
use mahayana_host_runtime::extensions::attachments::pdf_extract::{
    DocumentExtractionMetadata, DocumentExtractionPages, DocumentExtractor, DocumentPageSelection,
    PdfExtractedImage, PdfExtractionRequest, PdfExtractionResult, extract_pdf_content,
    validate_extraction_metadata,
};
use mahayana_host_runtime::extensions::attachments::parse_output::{
    ParsedMediaOutputSegment, SplitMediaFromOutputOptions, SplitMediaOutputOptions,
    is_allowed_remote_media_url, is_blocked_remote_media_hostname, is_valid_media,
    split_media_from_output, split_media_output,
};

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

fn create_test_service() -> AttachmentsService {
    AttachmentsService::new(
        PathBuf::from("/tmp/pdf-and-parse-tests"),
        Arc::new(MockAuth),
        Arc::new(MockBox),
        None,
    )
}

struct MockPdfExtractor {
    result: Result<PdfExtractionResult, String>,
}

impl DocumentExtractor for MockPdfExtractor {
    fn id(&self) -> &str {
        "mock-pdf"
    }

    fn plugin_id(&self) -> &str {
        "document-extract"
    }

    fn mime_types(&self) -> &[String] {
        static MIMES: std::sync::LazyLock<Vec<String>> =
            std::sync::LazyLock::new(|| vec!["application/pdf".to_string()]);
        &MIMES
    }

    fn extract(&self, _request: &PdfExtractionRequest) -> Result<PdfExtractionResult, String> {
        self.result.clone()
    }
}

#[test]
fn test_pdf_extract_disabled_when_no_extractor() {
    let req = PdfExtractionRequest {
        buffer: b"%PDF-1.4".to_vec(),
        mime_type: "application/pdf".to_string(),
        max_pages: 5,
        max_pixels: None,
        min_text_chars: None,
        password: None,
        page_numbers: None,
    };
    let err = extract_pdf_content(&req, &[]).unwrap_err();
    assert!(err.contains("PDF extraction disabled or unavailable"));
}

#[test]
fn test_pdf_extract_metadata_validation() {
    let req_auto = PdfExtractionRequest {
        buffer: Vec::new(),
        mime_type: "application/pdf".to_string(),
        max_pages: 3,
        max_pixels: None,
        min_text_chars: None,
        password: None,
        page_numbers: None,
    };

    // Valid automatic selection with 2 pages processed out of 10 (truncated = true)
    let meta_valid = DocumentExtractionMetadata {
        pages: Some(DocumentExtractionPages {
            processed: vec![1, 2],
            total: 10,
            selection: DocumentPageSelection::Automatic,
            truncated: true,
        }),
        text_truncated: false,
        images_truncated: false,
    };
    assert!(validate_extraction_metadata(&meta_valid, &req_auto).is_some());

    // Invalid: selection mismatch (explicit when request is automatic)
    let meta_mismatch = DocumentExtractionMetadata {
        pages: Some(DocumentExtractionPages {
            processed: vec![1, 2],
            total: 10,
            selection: DocumentPageSelection::Explicit,
            truncated: true,
        }),
        text_truncated: false,
        images_truncated: false,
    };
    assert!(validate_extraction_metadata(&meta_mismatch, &req_auto).is_none());

    // Invalid: page number 0 (must be positive)
    let meta_zero_page = DocumentExtractionMetadata {
        pages: Some(DocumentExtractionPages {
            processed: vec![0, 1],
            total: 10,
            selection: DocumentPageSelection::Automatic,
            truncated: true,
        }),
        text_truncated: false,
        images_truncated: false,
    };
    assert!(validate_extraction_metadata(&meta_zero_page, &req_auto).is_none());

    // Invalid: exceeds max_pages (max_pages=3, processed=4)
    let meta_over_budget = DocumentExtractionMetadata {
        pages: Some(DocumentExtractionPages {
            processed: vec![1, 2, 3, 4],
            total: 10,
            selection: DocumentPageSelection::Automatic,
            truncated: true,
        }),
        text_truncated: false,
        images_truncated: false,
    };
    assert!(validate_extraction_metadata(&meta_over_budget, &req_auto).is_none());

    // Explicit selection request
    let req_explicit = PdfExtractionRequest {
        buffer: Vec::new(),
        mime_type: "application/pdf".to_string(),
        max_pages: 5,
        max_pixels: None,
        min_text_chars: None,
        password: None,
        page_numbers: Some(vec![2, 4]),
    };
    let meta_explicit_valid = DocumentExtractionMetadata {
        pages: Some(DocumentExtractionPages {
            processed: vec![2, 4],
            total: 10,
            selection: DocumentPageSelection::Explicit,
            truncated: false,
        }),
        text_truncated: false,
        images_truncated: false,
    };
    assert!(validate_extraction_metadata(&meta_explicit_valid, &req_explicit).is_some());

    // Unrequested page in explicit mode (page 3 not in [2, 4])
    let meta_unrequested = DocumentExtractionMetadata {
        pages: Some(DocumentExtractionPages {
            processed: vec![2, 3],
            total: 10,
            selection: DocumentPageSelection::Explicit,
            truncated: false,
        }),
        text_truncated: false,
        images_truncated: false,
    };
    assert!(validate_extraction_metadata(&meta_unrequested, &req_explicit).is_none());
}

#[test]
fn test_pdf_extract_mock_extractor_success_and_error() {
    let req = PdfExtractionRequest {
        buffer: b"%PDF-1.4".to_vec(),
        mime_type: "application/pdf".to_string(),
        max_pages: 5,
        max_pixels: None,
        min_text_chars: None,
        password: None,
        page_numbers: None,
    };

    let success_extractor = Box::new(MockPdfExtractor {
        result: Ok(PdfExtractionResult {
            text: "Sample extracted PDF text".to_string(),
            images: vec![PdfExtractedImage {
                data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==".to_string(),
                mime_type: "image/png".to_string(),
                page: Some(1),
                width: Some(1),
                height: Some(1),
            }],
            metadata: Some(DocumentExtractionMetadata {
                pages: Some(DocumentExtractionPages {
                    processed: vec![1],
                    total: 1,
                    selection: DocumentPageSelection::Automatic,
                    truncated: false,
                }),
                text_truncated: false,
                images_truncated: false,
            }),
        }),
    });

    let res = extract_pdf_content(&req, &[success_extractor]).expect("extraction should succeed");
    assert_eq!(res.text, "Sample extracted PDF text");
    assert_eq!(res.images.len(), 1);
    assert!(res.metadata.is_some());

    let fail_extractor = Box::new(MockPdfExtractor {
        result: Err("password required".to_string()),
    });
    let err = extract_pdf_content(&req, &[fail_extractor]).unwrap_err();
    assert!(err.contains("Document extraction failed for application/pdf: password required"));
}

#[test]
fn test_parse_output_media_directives() {
    let text = "Here is the result:\nMEDIA: /path/to/diagram.png\nAll completed.";
    let opts = SplitMediaOutputOptions {
        extract_media_directives: true,
        extract_audio_directives: true,
        preserve_trailing_whitespace: false,
    };
    let parsed = split_media_output(text, &opts);
    assert_eq!(parsed.media_urls, vec!["/path/to/diagram.png"]);
    assert_eq!(parsed.text, "Here is the result:\nAll completed.");
    assert_eq!(parsed.rejected_media_count, 0);

    // Multiple bracketed media items
    let bracketed = "MEDIA: [/path/to/one.png, /path/to/two.jpg]";
    let parsed_bracketed = split_media_output(bracketed, &opts);
    assert_eq!(parsed_bracketed.media_urls, vec!["/path/to/one.png", "/path/to/two.jpg"]);
    assert_eq!(parsed_bracketed.text, "");
}

#[test]
fn test_parse_output_traversal_and_ssrf_guards() {
    assert!(!is_valid_media("../../secret.env", true, true));
    assert!(!is_valid_media("~baduser/file.png", true, true));
    assert!(is_valid_media("~/valid/file.png", true, true));
    assert!(is_valid_media("/var/media/sample.png", true, true));

    // SSRF hostname checks
    assert!(is_blocked_remote_media_hostname("localhost"));
    assert!(is_blocked_remote_media_hostname("127.0.0.1"));
    assert!(is_blocked_remote_media_hostname("10.0.0.1"));
    assert!(is_blocked_remote_media_hostname("192.168.1.1"));
    assert!(is_blocked_remote_media_hostname("intranet.local"));
    assert!(is_blocked_remote_media_hostname("cluster.internal"));
    assert!(!is_blocked_remote_media_hostname("example.com"));

    assert!(is_allowed_remote_media_url("https://example.com/image.png"));
    assert!(!is_allowed_remote_media_url("http://example.com/image.png")); // insecure http rejected
    assert!(!is_allowed_remote_media_url("https://127.0.0.1/image.png"));
}

#[test]
fn test_parse_output_audio_directive_tag() {
    let text = "Spoken reply.[[audio_as_voice]]";
    let opts = SplitMediaOutputOptions {
        extract_media_directives: true,
        extract_audio_directives: true,
        preserve_trailing_whitespace: false,
    };
    let parsed = split_media_output(text, &opts);
    assert_eq!(parsed.text, "Spoken reply.");
    assert!(parsed.audio_as_voice);
}

#[test]
fn test_parse_output_code_block_isolation() {
    let text = "Here is some code:\n```bash\nMEDIA: /not/extracted.png\n```\nDone.";
    let opts = SplitMediaOutputOptions {
        extract_media_directives: true,
        extract_audio_directives: true,
        preserve_trailing_whitespace: false,
    };
    let parsed = split_media_output(text, &opts);
    assert!(parsed.media_urls.is_empty());
    assert!(parsed.text.contains("MEDIA: /not/extracted.png"));
}

#[test]
fn test_parse_output_markdown_images() {
    let text = "See diagram: ![Diagram](https://example.com/photo.png) looks great.";
    let opts = SplitMediaFromOutputOptions {
        extract_media_directives: true,
        extract_markdown_images: true,
        extract_audio_directives: true,
        preserve_trailing_whitespace: false,
        markdown_image_allowlist: Some(vec!["https://example.com/photo.png".to_string()]),
    };
    let parsed = split_media_from_output(text, &opts);
    assert_eq!(parsed.media_urls, vec!["https://example.com/photo.png"]);
    assert_eq!(parsed.text, "See diagram: looks great.");
    assert_eq!(parsed.segments.len(), 3);
    assert_eq!(parsed.segments[0], ParsedMediaOutputSegment::Text { text: "See diagram:".to_string() });
    assert_eq!(parsed.segments[1], ParsedMediaOutputSegment::Media { url: "https://example.com/photo.png".to_string() });
    assert_eq!(parsed.segments[2], ParsedMediaOutputSegment::Text { text: "looks great.".to_string() });
}

#[test]
fn test_gateway_dispatch_pdf_and_parse_output() {
    let service = create_test_service();

    // extractPdfContent gateway call
    let req_val = json!({
        "buffer": [],
        "mimeType": "application/pdf",
        "maxPages": 2
    });
    let res = service.dispatch_gateway("extractPdfContent", &req_val);
    assert!(res.is_some());
    assert!(res.unwrap().is_err()); // No plugins available

    // validatePdfExtractionMetadata gateway call
    let meta_val = json!({
        "metadata": {
            "pages": {
                "processed": [1, 2],
                "total": 5,
                "selection": "automatic",
                "truncated": true
            },
            "textTruncated": false,
            "imagesTruncated": false
        },
        "request": {
            "maxPages": 3,
            "mimeType": "application/pdf"
        }
    });
    let val_res = service.dispatch_gateway("validatePdfExtractionMetadata", &meta_val);
    assert!(val_res.is_some());
    let trusted = val_res.unwrap().unwrap();
    assert!(!trusted.is_null());

    // splitMediaOutput gateway call
    let parse_val = json!({
        "raw": "Report:\nMEDIA: /tmp/chart.png\nEnd.",
        "options": {
            "extractMediaDirectives": true,
            "extractAudioDirectives": true
        }
    });
    let split_res = service.dispatch_gateway("splitMediaOutput", &parse_val);
    assert!(split_res.is_some());
    let output = split_res.unwrap().unwrap();
    assert_eq!(output["mediaUrls"][0], "/tmp/chart.png");
    assert_eq!(output["text"], "Report:\nEnd.");
}
