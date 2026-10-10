fn trimmed_non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub fn format_slack_sender_line(
    sender_name: Option<&str>,
    sender_id: Option<&str>,
    sender_type: Option<&str>,
) -> Option<String> {
    let name = trimmed_non_empty(sender_name);
    let id = trimmed_non_empty(sender_id);
    if name.is_none() && id.is_none() {
        return None;
    }

    let type_suffix = trimmed_non_empty(sender_type)
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();

    match (name, id) {
        (Some(name), Some(id)) => Some(format!(
            "The current message is being sent by {name} ({id}){type_suffix}"
        )),
        (Some(name), None) => Some(format!(
            "The current message is being sent by {name}{type_suffix}"
        )),
        (None, Some(id)) => Some(format!(
            "The current message is being sent by {id}{type_suffix}"
        )),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_name_id_and_optional_type_like_grok() {
        assert_eq!(format_slack_sender_line(None, None, Some("bot")), None);
        assert_eq!(
            format_slack_sender_line(Some("  Alice "), Some(" U123 "), Some(" human ")),
            Some("The current message is being sent by Alice (U123) (human)".to_owned())
        );
        assert_eq!(
            format_slack_sender_line(Some("Alice"), None, None),
            Some("The current message is being sent by Alice".to_owned())
        );
        assert_eq!(
            format_slack_sender_line(None, Some("U123"), Some("bot")),
            Some("The current message is being sent by U123 (bot)".to_owned())
        );
    }
}
