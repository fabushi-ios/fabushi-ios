//! Native PDF document content extraction and metadata validation.
//!
//! Maps upstream OpenClaw modules:
//! - `src/media/pdf-extract.ts`
//! - `src/media/pdf-extract.test.ts`

use std::collections::HashSet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfExtractedImage {
    pub data: String,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentPageSelection {
    Automatic,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentExtractionPages {
    pub processed: Vec<u32>,
    pub total: u32,
    pub selection: DocumentPageSelection,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentExtractionMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<DocumentExtractionPages>,
    pub text_truncated: bool,
    pub images_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfExtractionRequest {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub buffer: Vec<u8>,
    #[serde(default = "default_pdf_mime")]
    pub mime_type: String,
    pub max_pages: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_pixels: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_text_chars: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_numbers: Option<Vec<u32>>,
}

fn default_pdf_mime() -> String {
    "application/pdf".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfExtractionResult {
    pub text: String,
    pub images: Vec<PdfExtractedImage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<DocumentExtractionMetadata>,
}

pub trait DocumentExtractor: Send + Sync {
    fn id(&self) -> &str;
    fn plugin_id(&self) -> &str;
    fn mime_types(&self) -> &[String];
    fn extract(&self, request: &PdfExtractionRequest) -> Result<PdfExtractionResult, String>;
}

/// Validates extraction metadata against caller request bounds and invariants.
/// Untrusted or inconsistent metadata is rejected (stripped) before rendering into prompt text.
pub fn validate_extraction_metadata(
    metadata: &DocumentExtractionMetadata,
    request: &PdfExtractionRequest,
) -> Option<DocumentExtractionMetadata> {
    if let Some(pages) = &metadata.pages {
        let expected_selection = if request.page_numbers.is_some() {
            DocumentPageSelection::Explicit
        } else {
            DocumentPageSelection::Automatic
        };
        if pages.selection != expected_selection {
            return None;
        }

        if pages.processed.len() > request.max_pages {
            return None;
        }

        // Each page must be positive non-zero (> 0) and <= total
        let mut seen = HashSet::new();
        for &page in &pages.processed {
            if page == 0 || page > pages.total {
                return None;
            }
            if !seen.insert(page) {
                return None; // Duplicate page
            }
            if let Some(req_pages) = &request.page_numbers {
                if !req_pages.contains(&page) {
                    return None; // Unrequested page
                }
            }
        }

        let expected_count = if let Some(req_pages) = &request.page_numbers {
            req_pages.len()
        } else {
            pages.total as usize
        };
        let has_omitted_pages = pages.processed.len() < expected_count;

        let consistent_truncation = pages.truncated == has_omitted_pages
            || (has_omitted_pages && metadata.text_truncated);
        if !consistent_truncation {
            return None;
        }
    }

    Some(metadata.clone())
}

/// Executes PDF extraction through available document extractor plugins.
pub fn extract_pdf_content(
    request: &PdfExtractionRequest,
    extractors: &[Box<dyn DocumentExtractor>],
) -> Result<PdfExtractionResult, String> {
    let pdf_mime = "application/pdf";
    let matching: Vec<&Box<dyn DocumentExtractor>> = extractors
        .iter()
        .filter(|e| e.mime_types().iter().any(|m| m.trim().eq_ignore_ascii_case(pdf_mime)))
        .collect();

    if matching.is_empty() {
        return Err(
            "PDF extraction disabled or unavailable: enable the document-extract plugin to process application/pdf files.".to_string(),
        );
    }

    let mut errors = Vec::new();
    for extractor in matching {
        match extractor.extract(request) {
            Ok(result) => {
                let trusted_metadata = result.metadata.as_ref().and_then(|meta| {
                    validate_extraction_metadata(meta, request)
                });
                return Ok(PdfExtractionResult {
                    text: result.text,
                    images: result.images,
                    metadata: trusted_metadata,
                });
            }
            Err(err) => {
                errors.push(err);
            }
        }
    }

    if errors.len() == 1 {
        Err(format!("Document extraction failed for application/pdf: {}", errors[0]))
    } else {
        Err(format!(
            "Document extraction failed for application/pdf: Multiple errors: {}",
            errors.join("; ")
        ))
    }
}
