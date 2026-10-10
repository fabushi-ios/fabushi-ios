pub const FOCUS_RETRY_MINIMUM_COOLDOWN_MS: u64 = 5 * 60_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_retry_minimum_cooldown_matches_grok_reference() {
        assert_eq!(FOCUS_RETRY_MINIMUM_COOLDOWN_MS, 300_000);
    }
}
