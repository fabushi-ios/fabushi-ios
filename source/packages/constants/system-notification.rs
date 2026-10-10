pub const SYSTEM_NOTIFICATION_TAG: &str = "system_notification";
pub const SYSTEM_NOTIFICATION_OPEN_TAG: &str = "<system_notification>";
pub const SYSTEM_NOTIFICATION_CLOSE_TAG: &str = "</system_notification>";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_notification_wrappers() {
        assert_eq!(SYSTEM_NOTIFICATION_OPEN_TAG, format!("<{SYSTEM_NOTIFICATION_TAG}>"));
        assert_eq!(SYSTEM_NOTIFICATION_CLOSE_TAG, format!("</{SYSTEM_NOTIFICATION_TAG}>"));
    }
}
