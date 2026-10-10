pub const PI_DEFAULT_MAX_BYTES: usize = 50 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_default_max_bytes_matches_grok_reference() {
        assert_eq!(PI_DEFAULT_MAX_BYTES, 51_200);
    }
}
