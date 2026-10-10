pub const STRUCTURED_LOG_REPLAY_MAX_AGE_MS: u64 = 17 * 60 * 60 * 1_000;
pub const STRUCTURED_LOG_FUTURE_TIMESTAMP_MAX_SKEW_MS: u64 = 2 * 60 * 60 * 1_000;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_reference_windows() {
        assert_eq!(STRUCTURED_LOG_REPLAY_MAX_AGE_MS, 61_200_000);
        assert_eq!(STRUCTURED_LOG_FUTURE_TIMESTAMP_MAX_SKEW_MS, 7_200_000);
    }
}
