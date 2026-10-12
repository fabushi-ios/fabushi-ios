pub const META_PARENT_COMPLETION_TAG: &str = "agent_notification";
pub const META_PARENT_COMPLETION_OPEN_TAG: &str = "<agent_notification>";
pub const META_PARENT_COMPLETION_CLOSE_TAG: &str = "</agent_notification>";
pub const META_PARENT_COMPLETION_SYSTEM_REMINDER: &str = r#"<system_reminder>
Do not reiterate or repeat the contents of this agent notification to the user unless asked to do so.

Follow your instructions for Handling subagent notifications.
</system_reminder>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_pinned_completion_protocol() {
        assert_eq!(META_PARENT_COMPLETION_TAG, "agent_notification");
        assert_eq!(
            META_PARENT_COMPLETION_OPEN_TAG,
            format!("<{}>", META_PARENT_COMPLETION_TAG)
        );
        assert_eq!(
            META_PARENT_COMPLETION_CLOSE_TAG,
            format!("</{}>", META_PARENT_COMPLETION_TAG)
        );
        assert!(META_PARENT_COMPLETION_SYSTEM_REMINDER.contains(
            "Do not reiterate or repeat the contents of this agent notification"
        ));
        assert!(META_PARENT_COMPLETION_SYSTEM_REMINDER.ends_with("</system_reminder>"));
    }
}
