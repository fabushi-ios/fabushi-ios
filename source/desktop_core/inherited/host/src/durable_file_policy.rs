pub const SAND_UPGRADE_RESUME_FILE_NAME: &str = "host-upgrade-resume.json";
pub const SAND_ACK_OBLIGATIONS_FILE_NAME: &str = "ack-obligations.json";
pub const SAND_PENDING_WAKE_FILE_NAME: &str = "host-pending-wakes.json";
pub const SAND_XUSER_TURN_DEDUPE_FILE_NAME: &str = "host-xuser-turn-nonces.json";
pub const SAND_DISK_PRESSURE_REMINDERS_FILE_NAME: &str = "host-disk-pressure-reminders.json";
pub const SAND_LOCAL_EXEC_OPERATION_JOURNAL_FILE_NAME: &str = "host-local-exec-operations.json";
pub const BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES: [&str; 6] = [
    SAND_UPGRADE_RESUME_FILE_NAME,
    SAND_ACK_OBLIGATIONS_FILE_NAME,
    SAND_PENDING_WAKE_FILE_NAME,
    SAND_XUSER_TURN_DEDUPE_FILE_NAME,
    SAND_DISK_PRESSURE_REMINDERS_FILE_NAME,
    SAND_LOCAL_EXEC_OPERATION_JOURNAL_FILE_NAME,
];
