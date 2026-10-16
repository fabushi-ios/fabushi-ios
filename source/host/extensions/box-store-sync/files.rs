pub const SAND_FILES_MAX_BYTES: usize = 16 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_limit_matches_grok_reference() {
        assert_eq!(SAND_FILES_MAX_BYTES, 16_777_216);
    }
}
