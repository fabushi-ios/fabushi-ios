pub const READ_CHAR_HARD_LIMIT: usize = 100_000;
pub const LARGE_TEXT_BLOB_THRESHOLD: usize = 10_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_limits_match_grok_reference() {
        assert_eq!(READ_CHAR_HARD_LIMIT, 100_000);
        assert_eq!(LARGE_TEXT_BLOB_THRESHOLD, 10_000);
    }
}
