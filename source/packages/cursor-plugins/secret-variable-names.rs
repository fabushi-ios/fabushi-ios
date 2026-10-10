const SECRET_WORDS: &[&str] = &[
    "CREDENTIAL",
    "CREDENTIALS",
    "KEY",
    "PASSPHRASE",
    "PASSWORD",
    "SECRET",
    "TOKEN",
];

fn to_segments(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut segments = Vec::new();
    let mut current = String::new();

    for (index, character) in chars.iter().copied().enumerate() {
        if !character.is_ascii_alphanumeric() {
            if !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            continue;
        }

        let previous = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
        let next = chars.get(index + 1).copied();
        let starts_new_segment = !current.is_empty()
            && (previous
                .map(|value| {
                    (value.is_ascii_lowercase() || value.is_ascii_digit())
                        && character.is_ascii_uppercase()
                })
                .unwrap_or(false)
                || previous
                    .map(|value| {
                        value.is_ascii_uppercase()
                            && character.is_ascii_uppercase()
                            && next.map(|value| value.is_ascii_lowercase()).unwrap_or(false)
                    })
                    .unwrap_or(false));

        if starts_new_segment {
            segments.push(std::mem::take(&mut current));
        }
        current.push(character.to_ascii_uppercase());
    }

    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

pub fn is_secret_plugin_variable_name(name: &str) -> bool {
    to_segments(name)
        .iter()
        .any(|segment| SECRET_WORDS.contains(&segment.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_secret_words_as_identifier_segments_not_substrings() {
        for name in [
            "API_TOKEN",
            "apiToken",
            "accessKeyId",
            "clientPassword",
            "db-passphrase",
            "serviceCredentials",
            "HTTPSecretValue",
        ] {
            assert!(is_secret_plugin_variable_name(name), "{name}");
        }

        for name in ["monkey", "tokenizer", "keyboard", "secretary", "publicValue"] {
            assert!(!is_secret_plugin_variable_name(name), "{name}");
        }
    }

    #[test]
    fn segmenter_matches_grok_camelcase_and_acronym_boundaries() {
        assert_eq!(to_segments("APIKey"), vec!["API", "KEY"]);
        assert_eq!(to_segments("OAuthToken"), vec!["O", "AUTH", "TOKEN"]);
        assert_eq!(to_segments("foo2Bar-baz"), vec!["FOO2", "BAR", "BAZ"]);
    }
}
