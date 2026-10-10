fn is_js_regex_dot_match(character: char) -> bool {
    !matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

pub fn matches_command_glob(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim().chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let mut pattern_index = 0usize;
    let mut value_index = 0usize;
    let mut star_index = None;
    let mut star_value_index = 0usize;

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && pattern[pattern_index] != '*'
            && pattern[pattern_index] == value[value_index]
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            if star_value_index >= value.len() || !is_js_regex_dot_match(value[star_value_index]) {
                return false;
            }
            star_value_index += 1;
            value_index = star_value_index;
            pattern_index = star + 1;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_trimmed_anchored_star_globs() {
        assert!(matches_command_glob("  git * status  ", "git diff status"));
        assert!(matches_command_glob("*", "anything"));
        assert!(matches_command_glob("literal[1]", "literal[1]"));
        assert!(!matches_command_glob("git *", "x git status"));
        assert!(!matches_command_glob("git *", "git a\nb"));
    }
}
