use std::fs;
use std::path::PathBuf;

fn shipping_source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read shipping source {relative}: {error}"))
}

#[test]
fn shipping_provider_turn_consumes_dynamic_sections_in_frozen_order() {
    let source = shipping_source("app/src/main.rs");
    let collector = shipping_source("src/runner/prompt_collector_glue.rs");
    let glue = shipping_source("src/runner/runner_prompt_glue.rs");
    let turn_start = source
        .find("fn start_routed_provider_task(")
        .expect("shipping routed provider turn");
    let provider_start = source[turn_start..]
        .find("let mut provider_messages =")
        .map(|offset| turn_start + offset)
        .expect("shipping provider projection");
    let turn_prefix = &source[turn_start..provider_start];
    let tail = &source[provider_start..];

    let workflows = tail
        .find("append_budgeted_workflows_system_prompt(")
        .expect("shipping workflows section");
    let channels = tail
        .find("append_channels_system_prompt(")
        .expect("shipping channels section");
    let directory = tail
        .find("append_agent_directory_system_prompt(")
        .expect("shipping agent directory section");
    let live_runtime = tail
        .find(".append_live_runtime_sections(")
        .expect("shipping Runner prompt-glue live runtime delegation");

    assert!(workflows < channels && channels < directory && directory < live_runtime);
    assert!(tail[..channels].contains("list_agent_channels(&agent_id)"));
    assert!(
        tail[..directory].contains("summary.member_ids")
            && tail[..directory].contains("summary.is_group")
    );
    assert!(turn_prefix.contains("mcp_service.list_installed()"));
    assert!(turn_prefix.contains("discovery_unavailable: true"));
    assert!(turn_prefix.contains("is_subagent_runner: generated_parent_agent_id.is_some()"));
    assert!(tail[..channels].contains("let available_skill_prompts ="));
    assert!(tail[..channels].contains("workflow_store"));
    assert!(tail[..channels].contains(".list()"));
    assert!(tail[..channels].contains("SAND_AGENT_TOKEN_LIMIT"));
    assert_eq!(source.matches("append_budgeted_workflows_system_prompt(").count(), 1);
    assert_eq!(source.matches("append_channels_system_prompt(").count(), 1);
    assert_eq!(source.matches("append_agent_directory_system_prompt(").count(), 1);
    assert_eq!(source.matches(".append_live_runtime_sections(").count(), 1);

    assert!(glue.contains("pub struct RunnerPromptGlueOwner"));
    assert!(glue.contains("pub fn append_live_runtime_sections("));
    assert!(glue.contains("let mcp = (self.mcp_for_turn)()?;"));
    assert!(glue.contains("append_mcp_runtime_sections_for_turn("));
    assert!(glue.contains("mcp.discovery_unavailable"));
    assert!(glue.contains("self.is_subagent_runner"));
    assert!(collector.contains("pub fn append_mcp_runtime_sections_for_turn("));
    assert!(collector.contains("append_mcp_system_prompt_sections("));
    assert!(collector.contains("!is_subagent_runner"));
    assert_eq!(source.matches("append_mcp_system_prompt_sections(").count(), 0);

    assert!(turn_prefix.contains("host_runner_composition"));
    assert!(turn_prefix.contains(".prompt_compaction_epoch(&session_workers, &agent_id)"));
    assert!(turn_prefix.contains("automation_status_reminder_for_turn("));
    assert!(turn_prefix.contains("note_automation_status_reminder("));
    assert!(collector.contains("pub struct PromptCollectorAutomationReminderState"));
    assert!(turn_prefix.contains("confirmed_user_turn_watermark("));
    assert!(turn_prefix.contains("&prompt_watermark_sessions"));
    assert!(turn_prefix.contains("&prompt_watermark_agent_id"));
    assert_eq!(source.matches(".prepend_unconfirmed_user_messages(").count(), 1);
    assert!(glue.contains("pub fn prepend_unconfirmed_user_messages("));
    assert!(glue.contains("prepend_unconfirmed_user_messages_with_watermark_and_collect_for_turn("));
    assert!(collector.contains("pub fn prepend_unconfirmed_user_messages_with_watermark_and_collect_for_turn("));
    assert!(collector.contains("collect_prepend_user_messages("));

    assert!(turn_prefix.contains("production_services()"));
    assert!(turn_prefix.contains("selected_media_host_paths_for_turn(&args)"));
    assert!(turn_prefix.contains("attachment_paths.push(media_path)"));
    assert!(turn_prefix.contains("services.attachments.stage_into_box(&agent_id, &attachment_paths)"));
    assert_eq!(
        source.matches("apply_staged_attachment_paths_for_turn(").count(),
        1
    );
    assert!(collector.contains("pub fn apply_staged_attachment_paths_for_turn("));
    assert!(collector.contains("pub fn collect_turn_action_for_projection("));
    assert!(collector.contains("read_subagent_video_bytes("));
    assert!(collector.contains("bytes_look_like_video_container("));
}
