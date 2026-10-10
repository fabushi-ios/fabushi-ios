pub fn parse_ignore_rules(raw: &str) -> Vec<String> {
    raw.split('\n')
        .filter_map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line).trim_start();
            (!line.is_empty() && !line.starts_with('#')).then(|| line.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rules_with_grok_whitespace_and_comment_semantics() {
        assert_eq!(
            parse_ignore_rules("  # comment\r\n  build/\r\n\n\ttmp/**\nfoo # literal"),
            vec!["build/", "tmp/**", "foo # literal"]
        );
    }
}
