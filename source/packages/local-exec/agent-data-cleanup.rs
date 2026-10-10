pub const DEFAULT_AGENT_DATA_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
pub const DEFAULT_AGENT_DATA_CLEANUP_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_intervals_match_grok_reference() {
        assert_eq!(DEFAULT_AGENT_DATA_TTL_MS, 2_592_000_000);
        assert_eq!(DEFAULT_AGENT_DATA_CLEANUP_INTERVAL_MS, 86_400_000);
    }
}
