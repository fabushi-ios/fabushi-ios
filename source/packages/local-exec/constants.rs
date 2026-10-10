pub const MAX_BUFFER_SIZE: usize = 1_048_576;
pub const MAX_OFS_FILE_SIZE: u64 = 50 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_exec_limits_match_grok_reference() {
        assert_eq!(MAX_BUFFER_SIZE, 1_048_576);
        assert_eq!(MAX_OFS_FILE_SIZE, 52_428_800);
    }
}
