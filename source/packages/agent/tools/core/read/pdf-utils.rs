const PDF_HEADER: &[u8; 4] = b"%PDF";

pub fn has_pdf_magic_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(PDF_HEADER)
}

fn has_pdf_extension(file_path: &str) -> bool {
    let bytes = file_path.as_bytes();
    bytes.len() >= 4 && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".pdf")
}

pub fn is_pdf_binary(bytes: &[u8], file_path: Option<&str>) -> bool {
    has_pdf_magic_bytes(bytes)
        || file_path.map(has_pdf_extension).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_magic_bytes_or_case_insensitive_pdf_extension() {
        assert!(has_pdf_magic_bytes(b"%PDF-1.7"));
        assert!(!has_pdf_magic_bytes(b"%PD"));
        assert!(is_pdf_binary(b"not-pdf", Some("/tmp/report.PDF")));
        assert!(is_pdf_binary(b"%PDF-data", Some("/tmp/report.txt")));
        assert!(!is_pdf_binary(b"plain", Some("/tmp/report.pdf.txt")));
        assert!(!is_pdf_binary(b"plain", None));
    }
}
