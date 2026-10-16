/// Mirrors JavaScript's Math.round(value.length / 4).
///
/// JavaScript String.length counts UTF-16 code units, so Rust must not use
/// UTF-8 byte length or Unicode scalar count here.
pub fn estimate_string_token_count(value: &str) -> usize {
    value.encode_utf16().count().saturating_add(2) / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_javascript_utf16_length_and_rounding() {
        assert_eq!(estimate_string_token_count(""), 0);
        assert_eq!(estimate_string_token_count("a"), 0);
        assert_eq!(estimate_string_token_count("ab"), 1);
        assert_eq!(estimate_string_token_count("abcdef"), 2);
        assert_eq!(estimate_string_token_count("😀"), 1);
        assert_eq!(estimate_string_token_count("a😀b"), 1);
    }
}
