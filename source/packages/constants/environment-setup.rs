pub const DEFAULT_ENVIRONMENT_SETUP_MAX_RESUME_AGE_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
pub const MIN_ENVIRONMENT_SETUP_MAX_RESUME_AGE_MS: u64 = 60 * 60 * 1_000;
pub const MAX_ENVIRONMENT_SETUP_MAX_RESUME_AGE_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_resume_age_bounds() {
        assert_eq!(MIN_ENVIRONMENT_SETUP_MAX_RESUME_AGE_MS, 3_600_000);
        assert_eq!(DEFAULT_ENVIRONMENT_SETUP_MAX_RESUME_AGE_MS, 604_800_000);
        assert_eq!(MAX_ENVIRONMENT_SETUP_MAX_RESUME_AGE_MS, 2_592_000_000);
    }
}
