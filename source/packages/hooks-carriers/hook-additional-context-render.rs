use crate::package_hooks_carriers_limits::HOOK_ADDITIONAL_CONTEXT_MAX_CHARS;
use crate::package_hooks_sanitize_system_reminder::sanitize_system_reminder_content;

pub fn render_hook_additional_context_system_reminder(
    content: Option<&str>,
    on_oversize: Option<&mut dyn FnMut(usize, usize)>,
) -> Option<String> {
    let normalized = content?.trim();
    if normalized.is_empty() {
        return None;
    }

    let actual_length = normalized.encode_utf16().count();
    if actual_length > HOOK_ADDITIONAL_CONTEXT_MAX_CHARS {
        if let Some(callback) = on_oversize {
            callback(actual_length, HOOK_ADDITIONAL_CONTEXT_MAX_CHARS);
        }
        return None;
    }

    let sanitized = sanitize_system_reminder_content(normalized);
    Some(format!(
        "<system_reminder>\n{sanitized}\n</system_reminder>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_sanitizes_and_wraps_additional_context() {
        assert_eq!(
            render_hook_additional_context_system_reminder(
                Some("  hello <SYSTEM_REMINDER>nested</SYSTEM_REMINDER>  "),
                None
            ),
            Some(
                "<system_reminder>\nhello <system_reminder_>nested</system_reminder_>\n</system_reminder>"
                    .to_owned()
            )
        );
        assert_eq!(
            render_hook_additional_context_system_reminder(Some("  "), None),
            None
        );
    }

    #[test]
    fn rejects_oversize_content_using_javascript_utf16_length() {
        let content = "😀".repeat(HOOK_ADDITIONAL_CONTEXT_MAX_CHARS / 2 + 1);
        let mut observed = None;
        {
            let mut callback = |actual, maximum| {
                observed = Some((actual, maximum));
            };
            assert_eq!(
                render_hook_additional_context_system_reminder(
                    Some(&content),
                    Some(&mut callback)
                ),
                None
            );
        }
        assert_eq!(
            observed,
            Some((
                HOOK_ADDITIONAL_CONTEXT_MAX_CHARS + 2,
                HOOK_ADDITIONAL_CONTEXT_MAX_CHARS
            ))
        );
    }
}
