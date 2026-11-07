use mahayana_host_runtime::host_request_context::create_host_request_context;
use mahayana_host_runtime::runner::routed_provider_runtime::RunnerRequestContextSnapshot;
use mahayana_host_runtime::runner::system_prompt_assembly::render_request_context_system_prompt;
use serde_json::json;

#[test]
fn runner_request_context_projects_live_timezone_identity_and_rules() {
    let provider = create_host_request_context(
        "/tmp/fabushi/transcripts",
        || Some("Asia/Shanghai".to_string()),
        || Some(vec![
            json!({
                "fullPath": "security",
                "content": "Never publish credentials.",
                "isRequired": true
            }),
            json!({
                "fullPath": "style",
                "content": "Keep release notes concise.",
                "isRequired": false
            }),
        ]),
        || Some("  Gloria Chan  ".to_string()),
    );
    let snapshot = RunnerRequestContextSnapshot {
        context: provider.resolve(),
        rules: provider.resolve_rules(),
    };

    assert_eq!(
        snapshot.context.transcripts_folder,
        "/tmp/fabushi/transcripts"
    );
    assert_eq!(
        snapshot.context.time_zone.as_deref(),
        Some("Asia/Shanghai")
    );
    assert_eq!(
        snapshot.context.user_full_name.as_deref(),
        Some("Gloria Chan")
    );

    let prompt = render_request_context_system_prompt(
        &snapshot.context,
        snapshot.rules.as_deref(),
    );
    assert!(prompt.contains("Asia/Shanghai"));
    assert!(prompt.contains("Your user is Gloria Chan"));
    assert!(prompt.contains("Never publish credentials."));
    assert!(prompt.contains("security (required)"));
}

#[test]
fn unresolved_team_rules_are_not_fabricated_into_the_prompt() {
    let provider = create_host_request_context(
        "/tmp/fabushi/transcripts",
        || Some("UTC".to_string()),
        || None::<Vec<serde_json::Value>>,
        || None,
    );
    let snapshot = RunnerRequestContextSnapshot {
        context: provider.resolve(),
        rules: provider.resolve_rules(),
    };
    assert!(snapshot.rules.is_none());

    let prompt = render_request_context_system_prompt(
        &snapshot.context,
        snapshot.rules.as_deref(),
    );
    assert!(!prompt.contains("## Team rules"));
}
