// The pinned Grok 0.18 module retains only creation of the default manager
// logger. No manager declarations survived tree shaking, so native parity keeps
// the retained logger identity private without inventing a callable manager.
#[allow(dead_code)]
const DEFAULT_MANAGER_LOGGER_NAME: &str = "ControlledConversationActionManager";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_retained_default_manager_logger_identity() {
        assert_eq!(
            DEFAULT_MANAGER_LOGGER_NAME,
            "ControlledConversationActionManager"
        );
    }
}
