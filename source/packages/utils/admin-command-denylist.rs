pub const ADMIN_COMMAND_DENYLIST_MAX_RULE_LENGTH: usize = 512;

fn is_admin_command_denylist_separator(character: char) -> bool {
    matches!(
        character,
        ' ' | '\t' | '\n' | '\r' | '\u{00a0}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}'
    )
}

fn js_trim_whitespace(character: char) -> bool {
    character.is_whitespace() || character == '\u{feff}'
}

pub fn normalize_admin_command_denylist_text(value: &str) -> String {
    let mut result = String::new();
    let mut pending_separator = false;
    for character in value.chars() {
        if is_admin_command_denylist_separator(character) {
            pending_separator = true;
            continue;
        }
        if pending_separator && !result.is_empty() {
            result.push(' ');
        }
        pending_separator = false;
        result.push(character);
    }
    result
}

fn is_only_wildcards(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|character| character == '*')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCommandDenylistColonRule {
    pub executable_pattern: String,
    pub args_pattern: String,
}

pub fn parse_admin_command_denylist_colon_rule(rule: &str) -> Option<AdminCommandDenylistColonRule> {
    let index = rule.find(':')?;
    if index == 0 {
        return None;
    }
    let executable_pattern = rule[..index].trim().to_owned();
    if executable_pattern.chars().any(js_trim_whitespace) {
        return None;
    }
    Some(AdminCommandDenylistColonRule {
        executable_pattern,
        args_pattern: rule[index + 1..].trim().to_owned(),
    })
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

pub fn get_admin_command_denylist_rule_error(rule: &str) -> Option<String> {
    let normalized = normalize_admin_command_denylist_text(rule);
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return Some("Rule cannot be empty".to_owned());
    }
    if utf16_len(trimmed) > ADMIN_COMMAND_DENYLIST_MAX_RULE_LENGTH {
        return Some(format!(
            "Rule cannot exceed {ADMIN_COMMAND_DENYLIST_MAX_RULE_LENGTH} characters"
        ));
    }
    if is_only_wildcards(trimmed) {
        return Some("Rule cannot match every command; be more specific than wildcards alone".to_owned());
    }
    if trimmed.starts_with(':') {
        return Some("Colon rules need an executable before `:` (e.g. `aws:*s3 rm*`)".to_owned());
    }
    if let Some(colon_rule) = parse_admin_command_denylist_colon_rule(trimmed) {
        if is_only_wildcards(&colon_rule.executable_pattern)
            && (colon_rule.args_pattern.is_empty() || is_only_wildcards(&colon_rule.args_pattern))
        {
            return Some("Rule cannot match every command; narrow the executable or argument pattern".to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_reference_separator_set_to_single_spaces() {
        assert_eq!(
            normalize_admin_command_denylist_text("  aws\t\n\u{00a0}s3\u{200b}rm  "),
            "aws s3 rm"
        );
    }

    #[test]
    fn parses_colon_rules_and_rejects_whitespace_in_executable() {
        assert_eq!(
            parse_admin_command_denylist_colon_rule("aws:*s3 rm*"),
            Some(AdminCommandDenylistColonRule {
                executable_pattern: "aws".into(),
                args_pattern: "*s3 rm*".into(),
            })
        );
        assert_eq!(parse_admin_command_denylist_colon_rule("aws cli:*"), None);
        assert_eq!(parse_admin_command_denylist_colon_rule(":*"), None);
    }

    #[test]
    fn reports_reference_validation_errors() {
        assert_eq!(get_admin_command_denylist_rule_error("  "), Some("Rule cannot be empty".into()));
        assert_eq!(
            get_admin_command_denylist_rule_error("***"),
            Some("Rule cannot match every command; be more specific than wildcards alone".into())
        );
        assert_eq!(
            get_admin_command_denylist_rule_error(":*"),
            Some("Colon rules need an executable before `:` (e.g. `aws:*s3 rm*`)".into())
        );
        assert_eq!(
            get_admin_command_denylist_rule_error("***:***"),
            Some("Rule cannot match every command; narrow the executable or argument pattern".into())
        );
        assert_eq!(get_admin_command_denylist_rule_error("aws:*s3 rm*"), None);
    }

    #[test]
    fn uses_javascript_utf16_length_for_the_reference_limit() {
        assert!(get_admin_command_denylist_rule_error(&"a".repeat(512)).is_none());
        let too_long = "😀".repeat(257);
        assert!(get_admin_command_denylist_rule_error(&too_long).unwrap().contains("512"));
    }
}
