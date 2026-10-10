pub fn strip_s3_etag_quotes(raw: Option<&str>) -> String {
    let trimmed = raw.unwrap_or_default().trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        return trimmed[1..trimmed.len() - 1].to_owned();
    }
    trimmed.to_owned()
}

pub fn normalize_s3_etag(raw: Option<&str>) -> String {
    strip_s3_etag_quotes(raw).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{normalize_s3_etag, strip_s3_etag_quotes};

    #[test]
    fn strips_only_matching_outer_quotes_after_trim() {
        assert_eq!(strip_s3_etag_quotes(Some("  \"AbC-123\"  ")), "AbC-123");
        assert_eq!(strip_s3_etag_quotes(Some("\"AbC-123")), "\"AbC-123");
        assert_eq!(strip_s3_etag_quotes(Some("AbC-123\"")), "AbC-123\"");
        assert_eq!(strip_s3_etag_quotes(None), "");
        assert_eq!(strip_s3_etag_quotes(Some("   ")), "");
    }

    #[test]
    fn normalization_lowercases_after_quote_stripping() {
        assert_eq!(normalize_s3_etag(Some(" \"A1B2-C3\" ")), "a1b2-c3");
    }
}
