use serde_json::Value;
use std::fmt;

pub const LONG_RUNNING_TOOL_NAMES: [&str; 3] = ["task", "mcp_task", "subagent"];
pub const EXTRA_SHORT_TOOL_TIMEOUT_MS: f64 = 5.0 * 60.0 * 1_000.0;
pub const SHORT_TOOL_TIMEOUT_MS: f64 = 15.0 * 60.0 * 1_000.0;
pub const MEDIUM_TOOL_TIMEOUT_MS: f64 = 30.0 * 60.0 * 1_000.0;
pub const LONG_TOOL_TIMEOUT_MS: f64 = 60.0 * 60.0 * 1_000.0;
pub const EXTRA_LONG_TOOL_TIMEOUT_MS: f64 = 2.0 * 60.0 * 60.0 * 1_000.0;
pub const BACKGROUND_SHELL_DEFAULT_BLOCK_UNTIL_MS: f64 = 10.0 * 60.0 * 1_000.0;
const TIMEOUT_BUFFER_MS: f64 = 60.0 * 1_000.0;
const TOOL_CALL_GUARD_HEADROOM_MS: f64 = 60.0 * 1_000.0;
const TOOL_CALL_GUARD_BLOCK_GRACE_MS: f64 = 30.0 * 1_000.0;
pub const TOOL_CALL_TIMEOUT_TIERS_MS: [f64; 5] = [
    EXTRA_SHORT_TOOL_TIMEOUT_MS,
    SHORT_TOOL_TIMEOUT_MS,
    MEDIUM_TOOL_TIMEOUT_MS,
    LONG_TOOL_TIMEOUT_MS,
    EXTRA_LONG_TOOL_TIMEOUT_MS,
];

#[derive(Debug, Clone, PartialEq)]
pub struct FusedStepGuardTimeoutError {
    pub fuse_guard_ms: f64,
}
impl fmt::Display for FusedStepGuardTimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fused model/tool step exceeded guard timeout after {} seconds", (self.fuse_guard_ms / 1_000.0).round())
    }
}
impl std::error::Error for FusedStepGuardTimeoutError {}

pub fn is_fused_step_guard_timeout_reason(error: &(dyn std::error::Error + 'static)) -> bool {
    error.downcast_ref::<FusedStepGuardTimeoutError>().is_some()
}

pub enum TimeoutArgs<'a> {
    Missing,
    JsonText(&'a str),
    Value(&'a Value),
}

fn numeric_block_until(value: &Value) -> Option<f64> {
    let raw = value.as_object()?.get("block_until_ms")?.as_f64()?;
    (raw.is_finite() && raw >= 0.0).then_some(raw)
}

pub fn parse_block_until_ms(args: TimeoutArgs<'_>) -> Option<f64> {
    match args {
        TimeoutArgs::Missing => None,
        TimeoutArgs::Value(value) => numeric_block_until(value),
        TimeoutArgs::JsonText(text) => serde_json::from_str::<Value>(text).ok().as_ref().and_then(numeric_block_until),
    }
}

pub fn is_subagent_tool_name(tool_name: &str) -> bool {
    let lowered = tool_name.to_ascii_lowercase();
    LONG_RUNNING_TOOL_NAMES.contains(&lowered.as_str())
}

pub fn suggested_tool_timeout_ms(tool_name: &str, block_ms: Option<f64>) -> f64 {
    if is_subagent_tool_name(tool_name) {
        return LONG_TOOL_TIMEOUT_MS;
    }
    block_ms.map_or(SHORT_TOOL_TIMEOUT_MS, |value| (value + TIMEOUT_BUFFER_MS).max(0.0))
}

pub fn pick_tool_call_timeout_tier_ms(suggested_ms: f64) -> f64 {
    TOOL_CALL_TIMEOUT_TIERS_MS
        .iter()
        .copied()
        .find(|tier| *tier >= suggested_ms)
        .unwrap_or(EXTRA_LONG_TOOL_TIMEOUT_MS)
}

pub fn tool_call_execution_guard_ms(tool_name: &str, block_ms: Option<f64>) -> f64 {
    let tier_ms = pick_tool_call_timeout_tier_ms(suggested_tool_timeout_ms(tool_name, block_ms));
    let tier_headroom_ms = tier_ms - TOOL_CALL_GUARD_HEADROOM_MS;
    let Some(block_ms) = block_ms else { return tier_headroom_ms; };
    let requested_ms = tier_headroom_ms.max(block_ms + TOOL_CALL_GUARD_BLOCK_GRACE_MS);
    if requested_ms >= tier_ms { tier_headroom_ms } else { requested_ms }
}

pub fn build_tool_call_execution_timed_out_message(tool_name: &str, execution_timeout_ms: f64) -> String {
    let shell_hint = if tool_name.eq_ignore_ascii_case("shell") {
        " For long-running commands, re-run with block_until_ms set to a small value (or 0) so the command runs in the background, then poll its output instead of blocking on it."
    } else {
        ""
    };
    if execution_timeout_ms == 0.0 {
        format!("The {tool_name} tool call could not start because activity setup exceeded the per-call time limit. The execution environment may be slow or overloaded.{shell_hint}")
    } else {
        format!("The {tool_name} tool call timed out after {} seconds and was terminated. The execution environment may be unresponsive, or the operation needs longer than the per-call time limit.{shell_hint}", (execution_timeout_ms / 1_000.0).round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_block_until_and_selects_reference_tiers() {
        assert_eq!(parse_block_until_ms(TimeoutArgs::Missing), None);
        assert_eq!(parse_block_until_ms(TimeoutArgs::JsonText(r#"{"block_until_ms":120000}"#)), Some(120_000.0));
        assert_eq!(parse_block_until_ms(TimeoutArgs::Value(&json!({"block_until_ms":-1}))), None);
        assert_eq!(suggested_tool_timeout_ms("Task", Some(1.0)), LONG_TOOL_TIMEOUT_MS);
        assert_eq!(suggested_tool_timeout_ms("shell", None), SHORT_TOOL_TIMEOUT_MS);
        assert_eq!(pick_tool_call_timeout_tier_ms(16.0 * 60.0 * 1_000.0), MEDIUM_TOOL_TIMEOUT_MS);
    }

    #[test]
    fn guard_and_messages_match_reference_behavior() {
        assert_eq!(tool_call_execution_guard_ms("shell", None), SHORT_TOOL_TIMEOUT_MS - 60_000.0);
        assert!(build_tool_call_execution_timed_out_message("shell", 0.0).contains("runs in the background"));
        assert!(build_tool_call_execution_timed_out_message("read", 90_000.0).contains("90 seconds"));
        let error = FusedStepGuardTimeoutError { fuse_guard_ms: 90_000.0 };
        assert!(is_fused_step_guard_timeout_reason(&error));
    }
}
