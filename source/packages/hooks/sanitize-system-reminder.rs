const OPEN_TAG: &[u8] = b"<system_reminder>";
const CLOSE_TAG: &[u8] = b"</system_reminder>";

fn matches_ascii_case_insensitive(bytes: &[u8], index: usize, pattern: &[u8]) -> bool {
    bytes
        .get(index..index.saturating_add(pattern.len()))
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(pattern))
}

pub fn sanitize_system_reminder_content(content: &str) -> String {
    let bytes = content.as_bytes();
    let mut output = String::with_capacity(content.len() + 8);
    let mut last = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        if matches_ascii_case_insensitive(bytes, index, CLOSE_TAG) {
            output.push_str(&content[last..index]);
            output.push_str("</system_reminder_>");
            index += CLOSE_TAG.len();
            last = index;
            continue;
        }
        if matches_ascii_case_insensitive(bytes, index, OPEN_TAG) {
            output.push_str(&content[last..index]);
            output.push_str("<system_reminder_>");
            index += OPEN_TAG.len();
            last = index;
            continue;
        }
        index += content[index..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
    }

    output.push_str(&content[last..]);
    output
}

#[cfg(test)]
mod tests {
    use super::sanitize_system_reminder_content;

    #[test]
    fn neutralizes_open_and_close_tags_case_insensitively() {
        assert_eq!(
            sanitize_system_reminder_content(
                "a<SYSTEM_REMINDER>x</System_Reminder>b<system_reminder>y"
            ),
            "a<system_reminder_>x</system_reminder_>b<system_reminder_>y"
        );
    }

    #[test]
    fn unicode_prefixes_never_create_invalid_utf8_slice_boundaries() {
        assert_eq!(
            sanitize_system_reminder_content(
                "前置🙂abcdefghijklmno<SYSTEM_REMINDER>内容</SYSTEM_REMINDER>"
            ),
            "前置🙂abcdefghijklmno<system_reminder_>内容</system_reminder_>"
        );
        assert_eq!(sanitize_system_reminder_content("系统 reminder"), "系统 reminder");
    }
}
