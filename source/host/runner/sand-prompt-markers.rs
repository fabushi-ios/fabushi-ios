pub const SAND_HIDDEN_PROMPT_MARKER: &str = "[SAND_HIDDEN_PROMPT]";
pub const SAND_TRUSTED_AUTOMATION_PROMPT_MARKER: &str = "[SAND_TRUSTED_AUTOMATION_PROMPT]";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prompt_markers_match_reference_wire_values() {
        assert_eq!(SAND_HIDDEN_PROMPT_MARKER, "[SAND_HIDDEN_PROMPT]");
        assert_eq!(SAND_TRUSTED_AUTOMATION_PROMPT_MARKER, "[SAND_TRUSTED_AUTOMATION_PROMPT]");
    }
}
