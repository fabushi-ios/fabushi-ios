pub const CONTEXT_TAGS_TO_STRIP: &[&str] = &[
    "user_info",
    "project_layout",
    "rules",
    "always_applied_workspace_rules",
    "agent_requestable_workspace_rules",
    "user_rules",
    "agent_skills",
    "available_skills",
    "cloud_instructions",
    "cloud_task_instructions",
    "open_and_recently_viewed_files",
    "system_reminder",
    "system-reminder",
    "mcp_instructions",
    "mcp_file_system",
    "mcp_file_system_servers",
    "git_status",
    "agent_transcripts",
    "cursor_rules_context",
    "attached_files",
    "system_notification",
    "task_notification",
    "agent_notification",
];

fn remove_tag_blocks(mut text: String, tag: &str) -> String {
    let open_prefix = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut search_from = 0usize;

    loop {
        let lower = text.to_lowercase();
        if search_from >= lower.len() {
            break;
        }
        let Some(relative_start) = lower[search_from..].find(&open_prefix) else {
            break;
        };
        let start = search_from + relative_start;
        let after_name = start + open_prefix.len();
        let valid_open = lower
            .as_bytes()
            .get(after_name)
            .is_some_and(|byte| *byte == b'>' || byte.is_ascii_whitespace());
        if !valid_open {
            search_from = after_name;
            continue;
        }
        let Some(open_end_rel) = lower[after_name..].find('>') else {
            break;
        };
        let content_start = after_name + open_end_rel + 1;
        let Some(close_rel) = lower[content_start..].find(&close) else {
            break;
        };
        let end = content_start + close_rel + close.len();
        text.replace_range(start..end, "");
        search_from = start;
    }

    text
}

fn normalize_newlines(mut text: String) -> String {
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.trim().to_owned()
}

pub fn strip_tags(text: &str, tags: &[&str]) -> String {
    let mut result = text.to_owned();
    for tag in tags {
        result = remove_tag_blocks(result, tag);
    }
    normalize_newlines(result)
}

pub fn strip_context_tags(text: &str) -> String {
    strip_tags(text, CONTEXT_TAGS_TO_STRIP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_reference_context_tags_case_insensitively_with_attributes() {
        let text = "keep\n\n\n<USER_INFO data-x=\"1\">secret</user_info>\n<rules>hidden</rules>\nvisible";
        assert_eq!(strip_context_tags(text), "keep\n\n\nvisible".replace("\n\n\n", "\n\n"));
    }

    #[test]
    fn preserves_similar_names_and_unclosed_tags() {
        let text = "a <user_information>keep</user_information> b <user_info>unclosed";
        let stripped = strip_context_tags(text);
        assert!(stripped.contains("<user_information>keep</user_information>"));
        assert!(stripped.contains("<user_info>unclosed"));
    }

    #[test]
    fn generic_stripper_supports_hidden_thinking_contract() {
        assert_eq!(
            strip_tags("before<think>x</think>after<THINKING y=\"1\">z</thinking>", &["think", "thinking"]),
            "beforeafter"
        );
    }

    #[test]
    fn preserves_complete_pinned_tag_inventory() {
        assert_eq!(CONTEXT_TAGS_TO_STRIP.len(), 23);
        assert!(CONTEXT_TAGS_TO_STRIP.contains(&"system-reminder"));
        assert!(CONTEXT_TAGS_TO_STRIP.contains(&"agent_notification"));
    }
}
