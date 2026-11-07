use std::path::PathBuf;

use mahayana_host_runtime::automations::automation::{
    AUTOMATION_PROMPT_GUIDANCE_VERSION, AUTOMATION_STATUS_PROMPT_MARKER,
    AutomationRecord, describe_trigger, render_automations_system_prompt,
};
use mahayana_host_runtime::extensions::inference::provider_session::ProviderMessage;
use mahayana_host_runtime::runner::system_prompt_assembly::append_automations_system_prompt;
use serde_json::{Value, json};

fn record(
    id: &str,
    name: &str,
    enabled: bool,
    trigger: Value,
    schedule: &str,
) -> AutomationRecord {
    let trigger_description = describe_trigger(&trigger);
    AutomationRecord {
        id: id.into(),
        name: name.into(),
        prompt: "do the saved work".into(),
        trigger,
        is_enabled: enabled,
        created_at: 1_700_000_000_000.0,
        last_run_at: None,
        raised_notices: Vec::new(),
        schedule: schedule.into(),
        trigger_description,
        next_run_at: None,
        runs: Vec::new(),
        file_path: PathBuf::from(format!("/tmp/automations/{id}/automation.json")),
    }
}

#[test]
fn routines_capability_prompt_covers_schedule_listener_lifecycle_and_current_state() {
    assert_eq!(AUTOMATION_PROMPT_GUIDANCE_VERSION, "backend_triggers_v4");
    assert_eq!(AUTOMATION_STATUS_PROMPT_MARKER, "<automation_status>");
    assert_eq!(
        render_automations_system_prompt(&[], None, Some("America/Los_Angeles")),
        ""
    );

    let rows = vec![
        record(
            "morning-digest",
            "Morning Digest",
            true,
            json!({"type":"cron","schedule":"15 8 * * 1-5"}),
            "15 8 * * 1-5",
        ),
        record(
            "incident-watch",
            "Incident Watch",
            false,
            json!({"type":"github","repo":"org/repo","events":["ci-failed"],"ciBranch":"main"}),
            "",
        ),
    ];
    let prompt = render_automations_system_prompt(
        &rows,
        Some("/box/agents/a/automations"),
        Some("America/Los_Angeles"),
    );

    for expected in [
        "## Routines",
        "/box/agents/a/automations",
        "update_state",
        "target \"routine\"",
        "America/Los_Angeles",
        "CRON_TZ=<IANA zone>",
        "@every",
        "weekday daytime",
        "Slack",
        "GitHub",
        "Microsoft Teams",
        "Linear",
        "Sentry",
        "PagerDuty",
        "AuthenticateMcpServer",
        "self-expiring",
        "[routine]",
        "Morning Digest [enabled]",
        "15 8 * * 1-5",
        "folder morning-digest",
        "Incident Watch [paused]",
        "folder incident-watch",
    ] {
        assert!(prompt.contains(expected), "missing {expected:?} in {prompt}");
    }
}

#[test]
fn shipping_prompt_assembly_injects_routines_once_into_the_system_message() {
    let rows = vec![record(
        "daily-check",
        "Daily Check",
        true,
        json!({"type":"cron","schedule":"32 9 * * 1-5"}),
        "32 9 * * 1-5",
    )];
    let mut messages = vec![
        ProviderMessage {
            role: "system".into(),
            content: "base-system".into(),
        },
        ProviderMessage {
            role: "user".into(),
            content: "hello".into(),
        },
    ];

    append_automations_system_prompt(
        &mut messages,
        &rows,
        Some("/box/automations"),
        Some("UTC"),
    );
    append_automations_system_prompt(
        &mut messages,
        &rows,
        Some("/box/automations"),
        Some("UTC"),
    );

    assert_eq!(messages.len(), 2);
    assert!(messages[0].content.starts_with("base-system\n\n## Routines\n"));
    assert_eq!(messages[0].content.matches("## Routines").count(), 1);
    assert!(messages[0].content.contains("Daily Check [enabled]"));
}
