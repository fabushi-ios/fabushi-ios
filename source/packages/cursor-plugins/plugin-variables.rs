pub fn expand_plugin_variables(value: &str, plugin_path: &str) -> String {
    value
        .replace("${CLAUDE_PLUGIN_ROOT}", plugin_path)
        .replace("${CURSOR_PLUGIN_ROOT}", plugin_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_both_plugin_root_variables_globally() {
        assert_eq!(
            expand_plugin_variables(
                "${CLAUDE_PLUGIN_ROOT}/a:${CURSOR_PLUGIN_ROOT}/b:${CLAUDE_PLUGIN_ROOT}",
                "/plugin"
            ),
            "/plugin/a:/plugin/b:/plugin"
        );
    }
}
