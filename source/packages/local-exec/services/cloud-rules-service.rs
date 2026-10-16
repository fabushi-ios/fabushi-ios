pub const CLOUD_SPECIFIC_INSTRUCTIONS_HEADER: &str = "## Cursor Cloud specific instructions";
pub const CLOUD_SPECIFIC_INSTRUCTIONS_SEPARATOR: &str =
    "\n\n## Cursor Cloud specific instructions\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_instruction_markers_match_grok_reference() {
        assert_eq!(
            CLOUD_SPECIFIC_INSTRUCTIONS_SEPARATOR,
            format!("\n\n{CLOUD_SPECIFIC_INSTRUCTIONS_HEADER}\n")
        );
    }
}
