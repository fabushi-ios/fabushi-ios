pub const SNAPSHOT_TTL_MS: u64 = 5 * 60 * 1_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_ttl_matches_grok_reference() {
        assert_eq!(SNAPSHOT_TTL_MS, 300_000);
    }
}
