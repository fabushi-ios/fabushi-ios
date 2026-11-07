use std::path::Path;

use mahayana_host_runtime::runner_context_production_provider::{
    ProductionRunnerRequestContextSource, build_production_runner_request_context_snapshot,
};
use serde_json::json;

#[test]
fn production_runner_context_projects_host_identity_rules_and_transcript_path() {
    let rules = Some(vec![
        json!({"fullPath":"/team/rule-a","content":"always test"}),
        json!({"fullPath":"/team/rule-b","content":"preserve boundaries"}),
    ]);
    let snapshot = build_production_runner_request_context_snapshot(
        Path::new("/var/fabushi/transcripts"),
        rules.clone(),
        Some("  Alice   Example  ".into()),
    );

    assert_eq!(
        snapshot.context.transcripts_folder,
        "/var/fabushi/transcripts"
    );
    assert_eq!(
        snapshot.context.user_full_name.as_deref(),
        Some("Alice Example")
    );
    assert_eq!(snapshot.rules, rules);
    assert!(!snapshot.context.os_version.trim().is_empty());
}

#[test]
fn production_runner_context_source_is_send_sync_for_host_supervision() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProductionRunnerRequestContextSource>();
}
