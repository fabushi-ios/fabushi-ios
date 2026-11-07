use mahayana_host_runtime::runner::sand_auto_review_summaries::{
    CloudLifecycleAction, SandBrowserSummaryArgs, describe_sand_mcp_auto_review_action,
    describe_sand_shell_auto_review_action, fallback_sand_mcp_auto_review_summary,
    redact_mcp_value, safe_mcp_destination_hint, summarize_sand_automation_write_action,
    summarize_sand_browser_auto_review_action, summarize_sand_cloud_agent_action,
    summarize_sand_cloud_agent_lifecycle_action, summarize_sand_computer_typed_text,
    summarize_sand_mcp_auto_review_action, summarize_sand_subagent_action,
    summarize_typed_text,
};
use serde_json::json;

#[test]
fn shell_and_mcp_summaries_preserve_frozen_user_visible_shape() {
    assert_eq!(
        describe_sand_shell_auto_review_action(
            "host_shell",
            Some("List release artifacts."),
            Some("/tmp/build"),
        ),
        "List release artifacts on your local computer from /tmp/build"
    );
    assert_eq!(
        describe_sand_shell_auto_review_action("box_shell", None, None),
        "Run a command on Fabushi's computer"
    );

    let args = json!({
        "channel_id": "release-room",
        "api_key": "abcdefghijklmnopqrstuvwxyz012345",
        "nested": {"authorization": "Bearer abcdefghijklmnopqrstuvwxyz012345"}
    });
    assert_eq!(
        safe_mcp_destination_hint(Some(&args)).as_deref(),
        Some("release-room")
    );
    let redacted = redact_mcp_value(&args, None, 0);
    assert_eq!(redacted["api_key"], "…");
    assert_eq!(redacted["nested"]["authorization"], "…");

    assert_eq!(
        fallback_sand_mcp_auto_review_summary(
            "Slack",
            "sendMessage",
            Some(&args),
        ),
        "Use Slack to send a message to release-room"
    );
    assert_eq!(
        describe_sand_mcp_auto_review_action(
            Some("Post the release notice."),
            "Slack",
            "sendMessage",
            Some(&args),
        ),
        "Post the release notice with Slack"
    );
    let detailed = summarize_sand_mcp_auto_review_action(
        "Slack",
        "sendMessage",
        Some(&args),
    );
    assert!(detailed.starts_with("Use Slack tool sendMessage with "));
    assert!(!detailed.contains("abcdefghijklmnopqrstuvwxyz012345"));
    assert!(!detailed.contains("Bearer abcdefghijklmnopqrstuvwxyz012345"));
}

#[test]
fn automation_cloud_and_subagent_summaries_match_frozen_wording() {
    assert_eq!(
        summarize_sand_automation_write_action(
            "create",
            "Morning brief",
            "Every weekday at 8 AM",
            "Summarize inbox token=abcdefghijklmnopqrstuvwxyz012345",
            Some(false),
            &[],
        ),
        "Save the routine “Morning brief” (paused) to run every weekday at 8 am: “Summarize inbox token=…”"
    );
    assert_eq!(
        summarize_sand_automation_write_action(
            "workflow_body",
            "Research",
            "",
            "Read sources",
            None,
            &["Daily".into(), "Weekly".into()],
        ),
        "Change workflow “Research” used by Daily, Weekly: “Read sources”"
    );
    assert_eq!(
        summarize_sand_automation_write_action(
            "create",
            "Secrets",
            "Every day",
            "token=abcdefghijklmnopqrstuvwxyz012345 token=zyxwvutsrqponmlkjihgfedcba987654",
            None,
            &[],
        ),
        "Save the routine “Secrets” to run every day: “token=… token=…”"
    );

    assert_eq!(
        summarize_sand_cloud_agent_action(
            "reply",
            "Check the failing job",
            Some("agent-42"),
            2,
            true,
            None,
            None,
        ),
        "Send a follow-up to cloud agent agent-42 (interrupt) with 2 attached images it can see: “Check the failing job”"
    );
    assert_eq!(
        summarize_sand_cloud_agent_lifecycle_action(
            CloudLifecycleAction::Delete,
            "agent-42",
            None,
        ),
        "Permanently delete cloud agent agent-42"
    );
    assert_eq!(
        summarize_sand_subagent_action("steer", "Inspect the trace"),
        "Send a follow-up to a running task: “Inspect the trace”"
    );
}

#[test]
fn browser_and_computer_text_summaries_never_echo_secret_like_values() {
    let secret = "abcdefghijklmnopqrstuvwxyz012345";
    assert_eq!(
        summarize_typed_text(secret),
        format!("Type {} characters", secret.chars().count())
    );
    assert_eq!(
        summarize_sand_computer_typed_text(secret),
        format!(
            "Type {} characters on Fabushi's computer",
            secret.chars().count()
        )
    );

    let browser = SandBrowserSummaryArgs {
        op: "type".into(),
        element: Some("API key field".into()),
        target_page_url: Some("https://example.test/settings".into()),
        text: Some(secret.into()),
        ..Default::default()
    };
    let summary = summarize_sand_browser_auto_review_action(&browser);
    assert!(summary.starts_with(&format!("Type {} characters", secret.chars().count())));
    assert!(summary.contains("on https://example.test/settings in the box browser"));
    assert!(!summary.contains(secret));
}
