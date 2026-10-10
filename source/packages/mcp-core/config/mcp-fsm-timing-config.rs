pub const DEFAULT_KEEPALIVE_PROBE_DELAY_MS: u64 = 5 * 60_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_probe_delay_matches_grok_reference() {
        assert_eq!(DEFAULT_KEEPALIVE_PROBE_DELAY_MS, 300_000);
    }
}
