#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentStoreSyncClientConfig {
    pub sync_debounce_ms: u64,
    pub sync_backoff_base_ms: u64,
    pub sync_backoff_max_ms: u64,
    pub passive_retry_interval_ms: u64,
    pub passive_index_poll_interval_ms: u64,
    pub max_file_size_bytes: u64,
    pub token_refresh_buffer_ms: u64,
    pub rpc_retry_max_attempts: u32,
    pub rpc_retry_base_delay_ms: u64,
    pub rpc_retry_max_delay_ms: u64,
    pub rpc_retry_multiplier: f64,
    pub rpc_timeout_ms: u64,
    pub blob_idle_timeout_ms: u64,
    pub sync_round_timeout_ms: u64,
    pub sync_round_unwind_timeout_ms: u64,
    pub lock_release_failure_threshold: u32,
    pub resume_gap_threshold_ms: u64,
    pub dirty_passive_stalled_threshold_ms: u64,
    pub s3_concurrency: u32,
    pub list_concurrency: u32,
    pub hash_concurrency: u32,
    pub presign_concurrency: u32,
    pub multipart_upload_threshold_bytes: u64,
    pub multipart_part_size_bytes: u64,
    pub multipart_presign_window_size: u32,
    pub pull_presign_window_size: u32,
    pub multipart_complete_max_attempts: u32,
    pub multipart_max_restarts: u32,
    pub multipart_max_conflict_renames: u32,
    pub multipart_max_expiry_refreshes: u32,
    pub write_barrier_timeout_ms: u64,
    pub scoped_reserved_slots: u32,
    pub path_sync_request_poll_ms: u64,
    pub path_sync_request_wait_poll_ms: u64,
}

pub const AGENT_STORE_SYNC_CLIENT_CONFIG_DEFAULTS: AgentStoreSyncClientConfig =
    AgentStoreSyncClientConfig {
        sync_debounce_ms: 5_000,
        sync_backoff_base_ms: 5_000,
        sync_backoff_max_ms: 60_000,
        passive_retry_interval_ms: 5_000,
        passive_index_poll_interval_ms: 2_000,
        max_file_size_bytes: 100 * 1024 * 1024,
        token_refresh_buffer_ms: 60_000,
        rpc_retry_max_attempts: 3,
        rpc_retry_base_delay_ms: 250,
        rpc_retry_max_delay_ms: 5_000,
        rpc_retry_multiplier: 2.0,
        rpc_timeout_ms: 60_000,
        blob_idle_timeout_ms: 60_000,
        sync_round_timeout_ms: 300_000,
        sync_round_unwind_timeout_ms: 30_000,
        lock_release_failure_threshold: 3,
        resume_gap_threshold_ms: 120_000,
        dirty_passive_stalled_threshold_ms: 120_000,
        s3_concurrency: 8,
        list_concurrency: 4,
        hash_concurrency: 4,
        presign_concurrency: 4,
        multipart_upload_threshold_bytes: 64 * 1024 * 1024,
        multipart_part_size_bytes: 16 * 1024 * 1024,
        multipart_presign_window_size: 8,
        pull_presign_window_size: 500,
        multipart_complete_max_attempts: 3,
        multipart_max_restarts: 1,
        multipart_max_conflict_renames: 1,
        multipart_max_expiry_refreshes: 1,
        write_barrier_timeout_ms: 2_000,
        scoped_reserved_slots: 1,
        path_sync_request_poll_ms: 250,
        path_sync_request_wait_poll_ms: 50,
    };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_pinned_sync_client_contract() {
        let config = AGENT_STORE_SYNC_CLIENT_CONFIG_DEFAULTS;
        assert_eq!(config.sync_debounce_ms, 5_000);
        assert_eq!(config.sync_backoff_max_ms, 60_000);
        assert_eq!(config.max_file_size_bytes, 100 * 1024 * 1024);
        assert_eq!(config.rpc_retry_max_attempts, 3);
        assert_eq!(config.rpc_retry_multiplier, 2.0);
        assert_eq!(config.sync_round_timeout_ms, 300_000);
        assert_eq!(config.multipart_upload_threshold_bytes, 64 * 1024 * 1024);
        assert_eq!(config.multipart_part_size_bytes, 16 * 1024 * 1024);
        assert_eq!(config.pull_presign_window_size, 500);
        assert_eq!(config.path_sync_request_wait_poll_ms, 50);
    }
}
