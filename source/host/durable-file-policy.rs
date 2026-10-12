pub const SAND_UPGRADE_RESUME_FILE_NAME: &str = "host-upgrade-resume.json";
pub const SAND_ACK_OBLIGATIONS_FILE_NAME: &str = "ack-obligations.json";
pub const SAND_PENDING_WAKE_FILE_NAME: &str = "host-pending-wakes.json";
pub const SAND_XUSER_TURN_DEDUPE_FILE_NAME: &str = "host-xuser-turn-nonces.json";
pub const SAND_DISK_PRESSURE_REMINDERS_FILE_NAME: &str = "host-disk-pressure-reminders.json";

pub const BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES: [&str; 5] = [
    SAND_UPGRADE_RESUME_FILE_NAME,
    SAND_ACK_OBLIGATIONS_FILE_NAME,
    SAND_PENDING_WAKE_FILE_NAME,
    SAND_XUSER_TURN_DEDUPE_FILE_NAME,
    SAND_DISK_PRESSURE_REMINDERS_FILE_NAME,
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_every_host_owned_durable_file_from_box_store_sync() {
        assert_eq!(BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES.len(), 5);
        assert!(BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES.contains(&SAND_UPGRADE_RESUME_FILE_NAME));
        assert!(BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES.contains(&SAND_ACK_OBLIGATIONS_FILE_NAME));
        assert!(BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES.contains(&SAND_PENDING_WAKE_FILE_NAME));
        assert!(BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES.contains(&SAND_XUSER_TURN_DEDUPE_FILE_NAME));
        assert!(BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES.contains(&SAND_DISK_PRESSURE_REMINDERS_FILE_NAME));
    }
}
