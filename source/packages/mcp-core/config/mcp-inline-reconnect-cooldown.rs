pub const DEFAULT_INLINE_RECONNECT_COOLDOWN_MS: u64 = 5 * 60_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_reconnect_cooldown_matches_grok_reference() {
        assert_eq!(DEFAULT_INLINE_RECONNECT_COOLDOWN_MS, 300_000);
    }
}
