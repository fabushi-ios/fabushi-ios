use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Error returned when parsing or validating RFC 7233 HTTP Range requests.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RangeError {
    #[error("Missing bytes= prefix")]
    MissingPrefix,

    #[error("Invalid range header format: {0}")]
    InvalidHeader(String),

    #[error("416 Range Not Satisfiable: total_size={0}")]
    NotSatisfiable(u64),
}

/// Represents an inclusive byte range [start, end] for HTTP 206 Partial Content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64, // Inclusive
}

impl ByteRange {
    /// Returns the length in bytes covered by this range.
    #[inline]
    pub fn length(&self) -> u64 {
        if self.end >= self.start {
            self.end - self.start + 1
        } else {
            0
        }
    }
}

/// HTTP 206 Partial Content range streaming adapter implementing RFC 7233.
/// Upstream reference: `Telegram/SourceFiles/media/streaming/media_streaming_file.cpp`.
#[derive(Debug, Clone)]
pub struct RangeStreamAdapter;

impl RangeStreamAdapter {
    /// Parses RFC 7233 `Range: bytes=...` headers against a known file/blob size.
    ///
    /// Supports:
    /// - Closed ranges: `bytes=0-1023` -> [0, 1023]
    /// - Open-ended ranges: `bytes=1024-` -> [1024, total_size - 1]
    /// - Suffix ranges: `bytes=-500` -> [total_size - 500, total_size - 1]
    /// - Out-of-bounds checks returning `RangeError::NotSatisfiable(416)`
    pub fn parse_range_header(header: &str, total_size: u64) -> Result<ByteRange, RangeError> {
        if total_size == 0 {
            return Err(RangeError::NotSatisfiable(0));
        }

        let trimmed = header.trim();
        let prefix = "bytes=";
        if !trimmed.starts_with(prefix) && !trimmed.to_ascii_lowercase().starts_with(prefix) {
            return Err(RangeError::MissingPrefix);
        }

        let spec = &trimmed[prefix.len()..].trim();
        let parts: Vec<&str> = spec.split('-').collect();
        if parts.len() != 2 {
            return Err(RangeError::InvalidHeader(header.to_string()));
        }

        let first = parts[0].trim();
        let second = parts[1].trim();

        if first.is_empty() && second.is_empty() {
            return Err(RangeError::InvalidHeader(header.to_string()));
        }

        if first.is_empty() {
            // Suffix range: bytes=-500 (request last 500 bytes)
            let suffix_len = second
                .parse::<u64>()
                .map_err(|_| RangeError::InvalidHeader(header.to_string()))?;
            if suffix_len == 0 {
                return Err(RangeError::NotSatisfiable(total_size));
            }
            let actual_len = suffix_len.min(total_size);
            let start = total_size - actual_len;
            let end = total_size - 1;
            Ok(ByteRange { start, end })
        } else if second.is_empty() {
            // Open-ended range: bytes=1024-
            let start = first
                .parse::<u64>()
                .map_err(|_| RangeError::InvalidHeader(header.to_string()))?;
            if start >= total_size {
                return Err(RangeError::NotSatisfiable(total_size));
            }
            let end = total_size - 1;
            Ok(ByteRange { start, end })
        } else {
            // Closed range: bytes=0-1023
            let start = first
                .parse::<u64>()
                .map_err(|_| RangeError::InvalidHeader(header.to_string()))?;
            let end = second
                .parse::<u64>()
                .map_err(|_| RangeError::InvalidHeader(header.to_string()))?;

            if start > end || start >= total_size {
                return Err(RangeError::NotSatisfiable(total_size));
            }

            // Cap the upper bound at total_size - 1 per RFC 7233
            let clamped_end = end.min(total_size - 1);
            Ok(ByteRange {
                start,
                end: clamped_end,
            })
        }
    }

    /// Formats the RFC 7233 `Content-Range` header value (e.g. `bytes 0-1023/2048`).
    pub fn format_content_range(range: &ByteRange, total_size: u64) -> String {
        format!("bytes {}-{}/{}", range.start, range.end, total_size)
    }

    /// Formats the RFC 7233 unsatisfiable `Content-Range` header (e.g. `bytes */2048`).
    pub fn format_unsatisfiable_content_range(total_size: u64) -> String {
        format!("bytes */{}", total_size)
    }

    /// Extracts a byte subslice corresponding to the specified ByteRange safely.
    pub fn extract_slice<'a>(data: &'a [u8], range: &ByteRange) -> Result<&'a [u8], RangeError> {
        let start = range.start as usize;
        let end = range.end as usize;
        if start > end || end >= data.len() {
            return Err(RangeError::NotSatisfiable(data.len() as u64));
        }
        Ok(&data[start..=end])
    }
}
