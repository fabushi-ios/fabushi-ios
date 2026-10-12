pub const DEFAULT_SHELL_FOREGROUND_TIMEOUT_MS: i64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutBehavior {
    Background,
    Other,
}

/// Policy-only counterpart of Grok's shell timeout resolver.
///
/// This module does not create an iOS process or shell. Any unsupported process
/// execution remains owned by safe local capabilities or Remote Runner.
pub fn resolve_shell_timeout_ms(
    timeout: i64,
    is_background: bool,
    timeout_behavior: Option<TimeoutBehavior>,
    hard_timeout: Option<i64>,
) -> i64 {
    if timeout != 0 {
        return timeout;
    }

    if is_background
        || timeout_behavior == Some(TimeoutBehavior::Background)
        || hard_timeout.map(|value| value > 0).unwrap_or(false)
    {
        return 0;
    }

    DEFAULT_SHELL_FOREGROUND_TIMEOUT_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_explicit_timeout_and_background_semantics() {
        assert_eq!(resolve_shell_timeout_ms(12_345, false, None, None), 12_345);
        assert_eq!(resolve_shell_timeout_ms(0, true, None, None), 0);
        assert_eq!(
            resolve_shell_timeout_ms(0, false, Some(TimeoutBehavior::Background), None),
            0
        );
        assert_eq!(resolve_shell_timeout_ms(0, false, None, Some(1)), 0);
        assert_eq!(
            resolve_shell_timeout_ms(0, false, Some(TimeoutBehavior::Other), Some(0)),
            DEFAULT_SHELL_FOREGROUND_TIMEOUT_MS
        );
    }
}
