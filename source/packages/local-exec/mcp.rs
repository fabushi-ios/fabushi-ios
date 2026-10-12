pub fn sanitize_server_name(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for character in name.chars() {
        if character.is_whitespace() {
            sanitized.push('_');
        } else if character.is_ascii_alphanumeric()
            || matches!(character, '_' | '.' | '-')
        {
            sanitized.push(character);
        }
    }

    let mut normalized = String::with_capacity(sanitized.len());
    let mut previous_dot = false;
    for character in sanitized.chars() {
        if character == '.' {
            if !previous_dot {
                normalized.push('.');
            }
            previous_dot = true;
        } else {
            normalized.push(character);
            previous_dot = false;
        }
    }

    if normalized == "." || normalized == ".." {
        return normalized.replace('.', "_");
    }
    if normalized.is_empty() {
        return "_".to_owned();
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::sanitize_server_name;

    #[test]
    fn sanitizes_server_names_without_changing_transport_semantics() {
        assert_eq!(sanitize_server_name("alpha beta"), "alpha_beta");
        assert_eq!(sanitize_server_name("a...b"), "a.b");
        assert_eq!(sanitize_server_name(".."), "_");
        assert_eq!(sanitize_server_name("🔐"), "_");
        assert_eq!(sanitize_server_name("mcp@server/path"), "mcpserverpath");
        assert_eq!(sanitize_server_name("a-b_c.1"), "a-b_c.1");
    }
}
