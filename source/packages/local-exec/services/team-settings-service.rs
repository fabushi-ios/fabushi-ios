const CACHE_EXPIRY_MS: u64 = 5 * 60 * 1_000;
const BIGINT_ZERO: u64 = 0;
const BIGINT_EIGHT: u64 = 8;
const BIGINT_SIXTEEN: u64 = 16;
const IPV4_LOW_HEXTET_MASK: u64 = 65_535;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_team_settings_constants_match_reference() {
        assert_eq!(CACHE_EXPIRY_MS, 300_000);
        assert_eq!(BIGINT_ZERO, 0);
        assert_eq!(BIGINT_EIGHT, 8);
        assert_eq!(BIGINT_SIXTEEN, 16);
        assert_eq!(IPV4_LOW_HEXTET_MASK, 65_535);
    }
}
