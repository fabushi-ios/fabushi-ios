use std::path::{Path, PathBuf};

pub const PACK_DOWNLOAD_CONCURRENCY: usize = 2;
pub const PACK_RESTORE_MIN_MEMBERS: usize = 16;
pub const PACK_RESTORE_MIN_BYTE_SHARE: u64 = 8;
pub const PACK_EXTRACT_CONCURRENCY: usize = 16;
pub const PACK_BUILD_MIN_MEMBERS: usize = PACK_RESTORE_MIN_MEMBERS;
pub const PACK_BUILD_MIN_BYTES: u64 = 4 * 1024 * 1024;
pub const PACK_TMP_DIR_NAME: &str = "sand-box-store-pack";
pub const PACK_TMP_MAX_AGE_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStorePackCategory {
    pub abs_root: PathBuf,
    pub rel_prefix: String,
    pub stage_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreBlobGroup {
    pub sha: String,
    pub size: u64,
    pub rel_paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BoxStorePackCycleSummary {
    pub files_scanned: usize,
    pub files_uploaded: usize,
    pub bytes_uploaded: u64,
    pub skipped_unchanged: usize,
    pub removed: usize,
    pub failures: usize,
}

pub fn resolve_local_path_for_rel_path(
    categories: &[BoxStorePackCategory],
    rel_path: &str,
) -> Option<PathBuf> {
    let best = categories
        .iter()
        .filter(|category| {
            !category.stage_only
                && rel_path.starts_with(&format!("{}/", category.rel_prefix))
        })
        .max_by_key(|category| category.rel_prefix.len())?;
    let suffix = rel_path
        .strip_prefix(&best.rel_prefix)?
        .trim_start_matches('/');
    Some(best.abs_root.join(suffix))
}

pub fn should_restore_from_pack(
    member_count: usize,
    usable_compressed_bytes: u64,
    usable_uncompressed_bytes: u64,
    pack_bytes: u64,
) -> bool {
    if member_count < PACK_RESTORE_MIN_MEMBERS
        && usable_uncompressed_bytes < PACK_BUILD_MIN_BYTES
    {
        return false;
    }
    usable_compressed_bytes.saturating_mul(PACK_RESTORE_MIN_BYTE_SHARE) >= pack_bytes
}

pub fn should_build_pack(member_count: usize, total_bytes: u64) -> bool {
    member_count >= PACK_BUILD_MIN_MEMBERS || total_bytes >= PACK_BUILD_MIN_BYTES
}

pub fn temp_is_stale(now_ms: u64, modified_ms: u64) -> bool {
    now_ms.saturating_sub(modified_ms) > PACK_TMP_MAX_AGE_MS
}

pub fn temp_dir(base: &Path) -> PathBuf {
    base.join(PACK_TMP_DIR_NAME)
}


#[cfg(test)]
mod ios_pack_pipeline_parity_tests {
    use super::*;

    #[test]
    fn ios_pack_pipeline_constants_match_current_desktop_authority() {
        assert_eq!(PACK_DOWNLOAD_CONCURRENCY, 2);
        assert_eq!(PACK_EXTRACT_CONCURRENCY, 16);
        assert_eq!(PACK_RESTORE_MIN_MEMBERS, 16);
        assert_eq!(PACK_BUILD_MIN_MEMBERS, 16);
        assert_eq!(PACK_BUILD_MIN_BYTES, 4 * 1024 * 1024);
        assert_eq!(PACK_TMP_MAX_AGE_MS, 24 * 60 * 60 * 1_000);
    }

    #[test]
    fn ios_pack_restore_threshold_and_bandwidth_share_match_desktop() {
        assert!(!should_restore_from_pack(15, 10, 1024, 80));
        assert!(should_restore_from_pack(16, 10, 1024, 80));
        assert!(should_restore_from_pack(1, 10, 4 * 1024 * 1024, 80));
        assert!(!should_restore_from_pack(16, 9, 1024, 80));
        assert!(should_build_pack(16, 1));
        assert!(should_build_pack(1, 4 * 1024 * 1024));
        assert!(!should_build_pack(15, 4 * 1024 * 1024 - 1));
    }

    #[test]
    fn ios_pack_stale_temp_cleanup_uses_strict_twenty_four_hour_boundary() {
        assert!(!temp_is_stale(PACK_TMP_MAX_AGE_MS, 0));
        assert!(temp_is_stale(PACK_TMP_MAX_AGE_MS + 1, 0));
    }
}
