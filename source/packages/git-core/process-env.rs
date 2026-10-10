use std::collections::{BTreeMap, HashSet};

const NULL_DEVICE: &str = "/dev/null";
const PINNED_GIT_CONFIG_ENTRIES: [(&str, &str); 4] = [
    ("safe.bareRepository", "explicit"),
    ("core.fsmonitor", "false"),
    ("core.hooksPath", NULL_DEVICE),
    ("core.attributesFile", NULL_DEVICE),
];

#[derive(Debug, Default, Clone)]
pub struct GitProcessEnvOptions {
    pub spawner_env: Option<BTreeMap<String, String>>,
    pub options_env: Option<BTreeMap<String, String>>,
    pub command: Option<String>,
}

fn parse_js_decimal_prefix(raw: &str) -> Option<i64> {
    let trimmed = raw.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut end = 0usize;
    if matches!(bytes.first().copied(), Some(b'+') | Some(b'-')) {
        end = 1;
    }

    let digits_start = end;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == digits_start {
        return None;
    }

    trimmed[..end].parse::<i64>().ok()
}

fn read_git_config_count(env: &BTreeMap<String, String>) -> usize {
    let Some(raw) = env.get("GIT_CONFIG_COUNT") else {
        return 0;
    };
    if raw.is_empty() {
        return 0;
    }
    parse_js_decimal_prefix(raw)
        .filter(|value| *value >= 0)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0)
}

fn apply_pinned_git_config(
    env: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let count = read_git_config_count(&env);
    let mut result = env;
    let mut found_keys = HashSet::new();

    for index in 0..count {
        let existing_key = result.get(&format!("GIT_CONFIG_KEY_{index}")).cloned();
        if let Some(existing_key) = existing_key {
            for (pinned_key, pinned_value) in PINNED_GIT_CONFIG_ENTRIES {
                if existing_key == pinned_key {
                    result.insert(
                        format!("GIT_CONFIG_VALUE_{index}"),
                        pinned_value.to_owned(),
                    );
                    found_keys.insert(pinned_key);
                }
            }
        }
    }

    let mut next_index = count;
    for (pinned_key, pinned_value) in PINNED_GIT_CONFIG_ENTRIES {
        if found_keys.contains(pinned_key) {
            continue;
        }
        result.insert(
            format!("GIT_CONFIG_KEY_{next_index}"),
            pinned_key.to_owned(),
        );
        result.insert(
            format!("GIT_CONFIG_VALUE_{next_index}"),
            pinned_value.to_owned(),
        );
        next_index += 1;
    }

    if next_index != count {
        result.insert("GIT_CONFIG_COUNT".to_owned(), next_index.to_string());
    }

    result
}

pub fn create_git_process_env_with_base(
    mut base_env: BTreeMap<String, String>,
    options: GitProcessEnvOptions,
) -> BTreeMap<String, String> {
    if let Some(spawner_env) = options.spawner_env {
        base_env.extend(spawner_env);
    }

    base_env.insert("LC_ALL".to_owned(), "en_US.UTF-8".to_owned());
    base_env.insert("LANG".to_owned(), "en_US.UTF-8".to_owned());
    base_env.insert("GIT_PAGER".to_owned(), "cat".to_owned());

    if let Some(options_env) = options.options_env {
        base_env.extend(options_env);
    }

    if let Some(command) = options.command {
        base_env.insert("VSCODE_GIT_COMMAND".to_owned(), command);
    } else {
        base_env.remove("VSCODE_GIT_COMMAND");
    }

    apply_pinned_git_config(base_env)
}

pub fn create_git_process_env(options: GitProcessEnvOptions) -> BTreeMap<String, String> {
    create_git_process_env_with_base(std::env::vars().collect(), options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn merges_in_pinned_order_and_overrides_protected_git_config() {
        let base = map(&[
            ("BASE", "1"),
            ("VSCODE_GIT_COMMAND", "stale"),
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "core.fsmonitor"),
            ("GIT_CONFIG_VALUE_0", "true"),
        ]);
        let env = create_git_process_env_with_base(
            base,
            GitProcessEnvOptions {
                spawner_env: Some(map(&[("BASE", "2"), ("LANG", "spawner")])),
                options_env: Some(map(&[("LANG", "caller"), ("EXTRA", "yes")])),
                command: Some("status".to_owned()),
            },
        );

        assert_eq!(env.get("BASE").map(String::as_str), Some("2"));
        assert_eq!(env.get("LC_ALL").map(String::as_str), Some("en_US.UTF-8"));
        assert_eq!(env.get("LANG").map(String::as_str), Some("caller"));
        assert_eq!(env.get("GIT_PAGER").map(String::as_str), Some("cat"));
        assert_eq!(env.get("VSCODE_GIT_COMMAND").map(String::as_str), Some("status"));
        assert_eq!(env.get("GIT_CONFIG_VALUE_0").map(String::as_str), Some("false"));
        assert_eq!(env.get("GIT_CONFIG_COUNT").map(String::as_str), Some("4"));
        assert_eq!(
            env.get("GIT_CONFIG_KEY_1").map(String::as_str),
            Some("safe.bareRepository")
        );
        assert_eq!(
            env.get("GIT_CONFIG_VALUE_2").map(String::as_str),
            Some(NULL_DEVICE)
        );
    }

    #[test]
    fn parses_multi_digit_js_style_decimal_prefixes_without_overrun() {
        assert_eq!(parse_js_decimal_prefix("123junk"), Some(123));
        assert_eq!(parse_js_decimal_prefix(" +42.9"), Some(42));
        assert_eq!(parse_js_decimal_prefix("-1"), Some(-1));
        assert_eq!(parse_js_decimal_prefix("junk"), None);
    }

    #[test]
    fn follows_parse_int_prefix_and_removes_stale_command() {
        let env = create_git_process_env_with_base(
            map(&[
                ("GIT_CONFIG_COUNT", " 2junk"),
                ("GIT_CONFIG_KEY_0", "safe.bareRepository"),
                ("GIT_CONFIG_VALUE_0", "all"),
                ("GIT_CONFIG_KEY_1", "core.fsmonitor"),
                ("GIT_CONFIG_VALUE_1", "true"),
                ("VSCODE_GIT_COMMAND", "old"),
            ]),
            GitProcessEnvOptions::default(),
        );

        assert!(!env.contains_key("VSCODE_GIT_COMMAND"));
        assert_eq!(env.get("GIT_CONFIG_COUNT").map(String::as_str), Some("4"));
        assert_eq!(env.get("GIT_CONFIG_VALUE_0").map(String::as_str), Some("explicit"));
        assert_eq!(env.get("GIT_CONFIG_VALUE_1").map(String::as_str), Some("false"));
    }
}
