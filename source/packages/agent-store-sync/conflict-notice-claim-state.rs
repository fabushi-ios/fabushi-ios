pub const MAX_ANNOUNCED_EVENT_IDS: usize = 65_536;
pub const ANNOUNCED_EVICTION_BATCH: usize = MAX_ANNOUNCED_EVENT_IDS / 8;

#[cfg(test)]
mod tests {
    use super::{ANNOUNCED_EVICTION_BATCH, MAX_ANNOUNCED_EVENT_IDS};

    #[test]
    fn preserves_pinned_claim_state_bounds() {
        assert_eq!(MAX_ANNOUNCED_EVENT_IDS, 65_536);
        assert_eq!(ANNOUNCED_EVICTION_BATCH, 8_192);
        assert!(ANNOUNCED_EVICTION_BATCH >= 1);
    }
}
