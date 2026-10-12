pub fn is_background_automation_trigger(trigger: &str) -> bool {
    matches!(trigger, "schedule" | "event")
}

fn remove_parenthesized(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'(' {
            if let Some(relative_end) = text[index + 1..].find(')') {
                out.push(' ');
                index += relative_end + 2;
                continue;
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

fn is_hex(byte: u8) -> bool {
    byte.is_ascii_hexdigit()
}

fn uuid_length_at(bytes: &[u8], start: usize) -> Option<usize> {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut index = start;
    for (group_index, length) in GROUPS.into_iter().enumerate() {
        for _ in 0..length {
            if !bytes.get(index).copied().is_some_and(is_hex) {
                return None;
            }
            index += 1;
        }
        if group_index + 1 < GROUPS.len() {
            if bytes.get(index) != Some(&b'-') {
                return None;
            }
            index += 1;
        }
    }
    Some(index - start)
}

fn remove_uuids(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if let Some(length) = uuid_length_at(bytes, index) {
            out.push(' ');
            index += length;
        } else {
            out.push(bytes[index] as char);
            index += 1;
        }
    }
    out
}

fn remove_hex_addresses(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'0')
            && bytes.get(index + 1) == Some(&b'x')
            && bytes.get(index + 2).copied().is_some_and(is_hex)
        {
            index += 2;
            while bytes.get(index).copied().is_some_and(is_hex) {
                index += 1;
            }
            out.push(' ');
            continue;
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

pub fn normalize_automation_error_kind(detail: Option<&str>) -> String {
    let lowered = detail.unwrap_or_default().trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return "unknown".into();
    }

    let without_parentheses = remove_parenthesized(&lowered);
    let without_uuids = remove_uuids(&without_parentheses);
    let without_hex = remove_hex_addresses(&without_uuids);
    let words = without_hex
        .chars()
        .map(|ch| {
            if ch.is_ascii_digit() || (!ch.is_ascii_lowercase() && ch != ' ') {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>();

    let normalized = words
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        "unknown".into()
    } else {
        normalized
    }
}

pub fn should_notify_automation_failure(occurrence: u64) -> bool {
    occurrence <= 1 || occurrence.is_power_of_two()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_only_background_triggers() {
        assert!(is_background_automation_trigger("schedule"));
        assert!(is_background_automation_trigger("event"));
        assert!(!is_background_automation_trigger("manual"));
    }

    #[test]
    fn normalizes_dynamic_error_details_like_reference() {
        assert_eq!(normalize_automation_error_kind(None), "unknown");
        assert_eq!(
            normalize_automation_error_kind(Some(
                "Request 123 failed (attempt 9) for 123e4567-e89b-12d3-a456-426614174000 at 0xDEADBEEF: Timeout!!!"
            )),
            "request failed for at timeout"
        );
        assert_eq!(
            normalize_automation_error_kind(Some("  HTTP 503 upstream / gateway #42 retry later now please extra  ")),
            "http upstream gateway retry later now"
        );
        assert_eq!(normalize_automation_error_kind(Some("123-0xabc")), "unknown");
    }

    #[test]
    fn notifies_first_and_power_of_two_occurrences() {
        for occurrence in [0, 1, 2, 4, 8, 16] {
            assert!(should_notify_automation_failure(occurrence));
        }
        for occurrence in [3, 5, 6, 7, 9, 10] {
            assert!(!should_notify_automation_failure(occurrence));
        }
    }
}
