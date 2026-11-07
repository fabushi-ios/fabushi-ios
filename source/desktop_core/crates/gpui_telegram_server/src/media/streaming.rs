use crate::media::cas_store::CasStore;
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use gpui_telegram_media::range_stream::{RangeError, RangeStreamAdapter};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

/// Streams an immutable CAS blob with full RFC 7233 Range support (HTTP 206 Partial Content).
pub async fn stream_blob(
    cas: &CasStore,
    sha256_hash: &str,
    range_header: Option<&str>,
    mime_type: Option<&str>,
) -> Response {
    let file_path = match cas.resolve_cas_path(sha256_hash) {
        Some(p) => p,
        None => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from(format!("Blob {sha256_hash} not found in CAS")))
                .unwrap_or_default();
        }
    };

    stream_cas_file(&file_path, range_header, mime_type).await
}

/// Core RFC 7233 range streaming engine for any local file.
///
/// Supports:
/// - Closed ranges (`bytes=0-1023` -> 206 Partial Content)
/// - Open-ended ranges (`bytes=1024-` -> 206 Partial Content)
/// - Suffix ranges (`bytes=-500` -> 206 Partial Content)
/// - Unsatisfiable ranges (`bytes=999999-` -> 416 Range Not Satisfiable)
/// - Full content streaming when Range header is omitted -> 200 OK
pub async fn stream_cas_file(
    file_path: &Path,
    range_header: Option<&str>,
    mime_type: Option<&str>,
) -> Response {
    let meta = match tokio::fs::metadata(file_path).await {
        Ok(m) => m,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("Media file unreadable"))
                .unwrap_or_default();
        }
    };

    let total_size = meta.len();
    let content_type = mime_type.unwrap_or("application/octet-stream");

    if let Some(range_raw) = range_header {
        let trimmed = range_raw.trim();
        if !trimmed.is_empty() {
            match RangeStreamAdapter::parse_range_header(trimmed, total_size) {
                Ok(byte_range) => {
                    let mut file = match File::open(file_path).await {
                        Ok(f) => f,
                        Err(e) => {
                            return Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .body(Body::from(format!("Failed to open CAS file: {e}")))
                                .unwrap_or_default();
                        }
                    };

                    if let Err(e) = file.seek(SeekFrom::Start(byte_range.start)).await {
                        return Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(Body::from(format!("Seek failed: {e}")))
                            .unwrap_or_default();
                    }

                    let length = byte_range.length();
                    let bounded_reader = file.take(length);
                    let stream = ReaderStream::new(bounded_reader);
                    let body = Body::from_stream(stream);

                    let content_range =
                        RangeStreamAdapter::format_content_range(&byte_range, total_size);

                    return Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(header::CONTENT_TYPE, content_type)
                        .header(header::ACCEPT_RANGES, "bytes")
                        .header(header::CONTENT_RANGE, content_range)
                        .header(header::CONTENT_LENGTH, length.to_string())
                        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
                        .body(body)
                        .unwrap_or_default();
                }
                Err(RangeError::NotSatisfiable(_)) => {
                    let content_range =
                        RangeStreamAdapter::format_unsatisfiable_content_range(total_size);
                    return Response::builder()
                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                        .header(header::CONTENT_RANGE, content_range)
                        .body(Body::empty())
                        .unwrap_or_default();
                }
                Err(_) => {
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Body::from("Malformed Range header"))
                        .unwrap_or_default();
                }
            }
        }
    }

    // Full content streaming (HTTP 200)
    let file = match File::open(file_path).await {
        Ok(f) => f,
        Err(e) => {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("Failed to open CAS file: {e}")))
                .unwrap_or_default();
        }
    };

    let stream = ReaderStream::new(file);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, total_size.to_string())
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .body(Body::from_stream(stream))
        .unwrap_or_default()
}
