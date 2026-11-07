use std::collections::HashMap;

use mahayana_host_runtime::host_request_context::HostRequestContext;
use mahayana_host_runtime::runner::system_prompt::{
    DEFAULT_SAND_SYSTEM_PROMPT, SAND_CLOUD_AGENTS_DISABLED_PROMPT_SECTION,
    SAND_MCP_MULTI_ACCOUNT_PROMPT_SECTION, SAND_SYSTEM_PROMPT_CLOUD_AGENTS_DISABLED,
    USER_MESSAGE_REPLY_REMINDER, ReplyContext, append_user_reply_reminder_with_disabled,
    build_attached_files_note, build_reply_context_note, build_sand_base_system_prompt,
    build_sand_subagent_system_prompt, build_user_message_address_note,
    format_attached_file_size, is_media_review_subagent_type,
};
use mahayana_host_runtime::runner::system_prompt_assembly::{
    AgentProfileForPrompt, AgentSkillPromptItem, ComputerPromptState, RemoteBoxPromptState,
    RunnerPromptRole,
    append_agent_profile_system_prompt, append_computer_system_prompt,
    append_mcp_system_prompt_sections, append_remote_box_system_prompt,
    append_workflows_system_prompt, render_agent_profile_section,
    render_computer_system_prompt, render_remote_box_system_prompt,
    render_request_context_system_prompt,
    render_request_context_system_prompt_with_capabilities,
    render_budgeted_agent_skills_prompt, render_workflows_system_prompt,
};
use mahayana_host_runtime::extensions::inference::provider_session::ProviderMessage;
use mahayana_host_runtime::sand_multitask::SAND_MULTITASK_PROMPT_SECTION;
use serde_json::json;

#[test]
fn frozen_base_prompt_variants_preserve_grok_send_message_and_cloud_agent_contracts() {
    assert!(DEFAULT_SAND_SYSTEM_PROMPT.starts_with(
        "You are Fabushi, a warm, concise desktop assistant.\n\n## How a turn works"
    ));
    assert!(DEFAULT_SAND_SYSTEM_PROMPT.contains(
        "ALWAYS hand it to a Cursor cloud agent with the CloudAgent tool"
    ));
    assert!(!SAND_SYSTEM_PROMPT_CLOUD_AGENTS_DISABLED.contains(
        "ALWAYS hand it to a Cursor cloud agent with the CloudAgent tool"
    ));
    assert!(SAND_SYSTEM_PROMPT_CLOUD_AGENTS_DISABLED.contains(
        "Cursor cloud agents are disabled by your team's admin"
    ));
    assert_eq!(
        build_sand_base_system_prompt(true),
        DEFAULT_SAND_SYSTEM_PROMPT
    );
    assert_eq!(
        build_sand_base_system_prompt(false),
        SAND_SYSTEM_PROMPT_CLOUD_AGENTS_DISABLED
    );
    assert!(SAND_CLOUD_AGENTS_DISABLED_PROMPT_SECTION.contains("CloudAgent tool is not available"));
    assert!(SAND_MCP_MULTI_ACCOUNT_PROMPT_SECTION.contains("account_label"));
}

#[test]
fn frozen_reply_and_attachment_helpers_match_grok_shapes() {
    assert_eq!(format_attached_file_size(-1.0), "");
    assert_eq!(format_attached_file_size(1_023.0), "1023 B");
    assert_eq!(format_attached_file_size(1_024.0), "1.0 KB");
    assert_eq!(format_attached_file_size(10_240.0), "10 KB");
    assert_eq!(format_attached_file_size(1_048_576.0), "1.0 MB");
    assert_eq!(format_attached_file_size(1_073_741_824.0), "1.0 GB");

    let files = vec![" /Users/me/report.pdf ".to_string(), "".to_string()];
    let box_paths = HashMap::from([(
        "/Users/me/report.pdf".to_string(),
        "/workspace/uploads/report.pdf".to_string(),
    )]);
    let sizes = HashMap::from([("/Users/me/report.pdf".to_string(), 2_048.0)]);
    let note = build_attached_files_note(&files, &box_paths, &sizes);
    assert!(note.starts_with("The user attached a file."));
    assert!(note.contains("/Users/me/report.pdf (2.0 KB)"));
    assert!(note.contains("also copied into your box at /workspace/uploads/report.pdf"));

    assert_eq!(
        build_reply_context_note(Some(&ReplyContext {
            target_id: " t3u ".into(),
            quote: " hello ".into(),
        })),
        "[In reply to t3u: \"hello\"]"
    );
    assert_eq!(build_user_message_address_note(Some(" t3u ")), "[t3u]");
}

#[test]
fn frozen_reply_reminder_and_subagent_helpers_are_source_closed() {
    assert_eq!(
        append_user_reply_reminder_with_disabled("hello", true),
        "hello"
    );
    assert_eq!(
        append_user_reply_reminder_with_disabled("", false),
        USER_MESSAGE_REPLY_REMINDER
    );
    assert!(
        append_user_reply_reminder_with_disabled("hello", false)
            .ends_with(USER_MESSAGE_REPLY_REMINDER)
    );
    assert!(is_media_review_subagent_type(Some("watch-video")));
    assert!(is_media_review_subagent_type(Some("VideoReview")));
    assert!(!is_media_review_subagent_type(Some("generalPurpose")));

    let prompt = build_sand_subagent_system_prompt(Some("browserUse"), true);
    assert!(prompt.starts_with("You are Fabushi running as the browserUse subagent."));
    assert!(prompt.contains("Operate in readonly mode: do not modify anything."));
    assert!(prompt.contains("## Staying safe while you work"));
}

#[test]
fn shipping_system_prompt_assembly_uses_the_frozen_supported_variant() {
    let context = HostRequestContext {
        os_version: "test".into(),
        shell: None,
        time_zone: Some("America/Los_Angeles".into()),
        transcripts_folder: "/tmp/transcripts".into(),
        user_full_name: Some("Ada Lovelace".into()),
    };
    let prompt = render_request_context_system_prompt(
        &context,
        Some(&[json!({
            "name": "security",
            "content": "Never disclose credentials.",
            "isRequired": true
        })]),
    );
    assert!(prompt.starts_with(SAND_SYSTEM_PROMPT_CLOUD_AGENTS_DISABLED));
    assert!(prompt.contains(SAND_CLOUD_AGENTS_DISABLED_PROMPT_SECTION));
    assert!(prompt.contains("the user lives in America/Los_Angeles"));
    assert!(prompt.contains("Your user is Ada Lovelace"));
    assert!(prompt.contains("### security (required)\nNever disclose credentials."));
}


#[test]
fn shipping_system_prompt_projects_live_cloud_agent_and_multitask_capabilities() {
    let context = HostRequestContext {
        os_version: "test".into(),
        shell: None,
        time_zone: Some("UTC".into()),
        transcripts_folder: "/tmp/transcripts".into(),
        user_full_name: None,
    };
    let prompt = render_request_context_system_prompt_with_capabilities(
        &context,
        None,
        true,
        true,
    );
    assert!(prompt.starts_with(DEFAULT_SAND_SYSTEM_PROMPT));
    assert!(!prompt.contains(SAND_CLOUD_AGENTS_DISABLED_PROMPT_SECTION));
    assert!(prompt.contains(SAND_MULTITASK_PROMPT_SECTION));

    let disabled = render_request_context_system_prompt_with_capabilities(
        &context,
        None,
        false,
        false,
    );
    assert!(disabled.starts_with(SAND_SYSTEM_PROMPT_CLOUD_AGENTS_DISABLED));
    assert!(disabled.contains(SAND_CLOUD_AGENTS_DISABLED_PROMPT_SECTION));
    assert!(!disabled.contains(SAND_MULTITASK_PROMPT_SECTION));
}


#[test]
fn frozen_agent_profile_section_is_provider_bound_once() {
    let profile = AgentProfileForPrompt {
        name: " Researcher ".into(),
        description: " Investigates deeply. ".into(),
        file_path: "/home/oai/share/agents/a/profile.json".into(),
        settings_file_path: "/home/oai/share/agents/a/settings.json".into(),
    };
    let section = render_agent_profile_section(&profile, false).expect("profile section");
    assert!(section.starts_with("Agent profile:\nTitle: Researcher"));
    assert!(section.contains("Your agent name is \"Researcher\"."));
    assert!(section.contains("Description: Investigates deeply."));
    assert!(section.contains("/home/oai/share/agents/a/profile.json"));
    assert!(section.contains("update_state tool (target \"profile\", action \"set\")"));
    assert!(section.contains("ExternalShell"));
    assert!(section.contains("/home/oai/share/agents/a/settings.json"));
    assert!(section.contains("\"hidden_from_sidebar\""));

    let mut messages = vec![
        ProviderMessage { role: "system".into(), content: "base".into() },
        ProviderMessage { role: "user".into(), content: "hello".into() },
    ];
    append_agent_profile_system_prompt(&mut messages, &section);
    append_agent_profile_system_prompt(&mut messages, &section);
    assert_eq!(
        messages[0].content.matches("Agent profile:\n").count(),
        1
    );

    let shared = render_agent_profile_section(&profile, true).expect("shared profile");
    assert!(shared.contains("Title: Researcher"));
    assert!(!shared.contains("Your agent name is"));
    assert!(!shared.contains("profile.json"));
    assert!(!shared.contains("settings.json"));
}


#[test]
fn frozen_workflows_section_is_rendered_once_for_the_shipping_provider_prompt() {
    let prompt = render_workflows_system_prompt(Some("/home/oai/share/workflows"));
    assert_eq!(
        prompt,
        "Workflows are a GLOBAL, shared library across all of the user's assistants. User-created skills live as files at /home/oai/share/workflows: one subfolder per workflow, each holding a SKILL.md. Prefer the update_state tool (target \"workflow\") to save, rewrite, and delete them. Reference workflows as [name](sand-workflow:<id>)."
    );
    assert!(render_workflows_system_prompt(None).is_empty());

    let mut messages = vec![
        ProviderMessage { role: "system".into(), content: "base".into() },
        ProviderMessage { role: "user".into(), content: "hello".into() },
    ];
    append_workflows_system_prompt(&mut messages, Some("/home/oai/share/workflows"));
    append_workflows_system_prompt(&mut messages, Some("/home/oai/share/workflows"));
    assert_eq!(
        messages[0]
            .content
            .matches("Workflows are a GLOBAL, shared library")
            .count(),
        1
    );
}


#[test]
fn frozen_agent_skill_catalog_uses_the_turn_token_limit_and_preserves_protected_skills() {
    let under_budget = render_budgeted_agent_skills_prompt(
        &[AgentSkillPromptItem {
            full_path: "/home/oai/share/workflows/research/SKILL.md".into(),
            description: Some("Research workflow".into()),
        }],
        200_000,
    );
    assert_eq!(under_budget.strategy, "under_budget");
    assert_eq!(under_budget.retained_count, 1);
    assert_eq!(under_budget.omitted_count, 0);
    assert!(under_budget.prompt.contains("research/SKILL.md"));
    assert!(under_budget.prompt.contains("Research workflow"));

    let mut crowded = vec![AgentSkillPromptItem {
        full_path: "/home/oai/share/workflows/canvas/SKILL.md".into(),
        description: Some("protected canvas skill".into()),
    }];
    crowded.extend((0..30).map(|index| AgentSkillPromptItem {
        full_path: format!("/home/oai/share/workflows/skill-{index}/SKILL.md"),
        description: Some("x".repeat(800)),
    }));
    let capped = render_budgeted_agent_skills_prompt(&crowded, 2_000);
    assert!(matches!(
        capped.strategy,
        "shortened_descriptions" | "dropped_descriptions" | "omitted_skills"
    ));
    assert!(capped.prompt.contains("/canvas/SKILL.md"));
    if capped.omitted_count > 0 {
        assert!(capped.prompt.contains("Additional skills omitted"));
    }
}


#[test]
fn shipping_remote_box_and_computer_sections_follow_frozen_order_and_live_gates() {
    let mut messages = vec![
        ProviderMessage { role: "system".into(), content: "base".into() },
        ProviderMessage { role: "user".into(), content: "hello".into() },
    ];
    append_mcp_system_prompt_sections(
        &mut messages,
        &[json!({"name":"Acme","status":"connected","customInstructions":"Use raw rows."})],
        true,
        true,
    );
    append_remote_box_system_prompt(
        &mut messages,
        &RemoteBoxPromptState {
            role: RunnerPromptRole::Main,
            available: true,
            runtime_state: "running".into(),
            desktop_capable: true,
            desktop_ready: true,
        },
    );
    append_computer_system_prompt(
        &mut messages,
        &ComputerPromptState {
            role: RunnerPromptRole::Main,
            box_available: true,
            desktop_capable: true,
            desktop_ready: true,
            control_lease_active: true,
            human_takeover_pending: false,
            browser_use_offered: false,
            window_index: Some(2),
        },
    );

    let content = &messages[0].content;
    let mcp_custom = content.find("## Connector custom instructions").expect("mcp custom");
    let mcp_status = content.find("<mcp_status>").expect("mcp status");
    let remote_box = content.find("## Your box").expect("remote box");
    let computer = content.find("## The box desktop").expect("computer");
    assert!(mcp_custom < mcp_status && mcp_status < remote_box && remote_box < computer);
    assert_eq!(content.matches("## Your box").count(), 1);
    assert_eq!(content.matches("## The box desktop").count(), 1);

    let unavailable = render_remote_box_system_prompt(&RemoteBoxPromptState {
        role: RunnerPromptRole::Main,
        available: false,
        runtime_state: "absent".into(),
        desktop_capable: false,
        desktop_ready: false,
    });
    assert!(unavailable.contains("shipping box runtime is unavailable"));
    assert!(unavailable.contains("runtime state: absent"));

    let takeover = render_computer_system_prompt(&ComputerPromptState {
        role: RunnerPromptRole::ComputerUseSubagent,
        box_available: true,
        desktop_capable: true,
        desktop_ready: true,
        control_lease_active: true,
        human_takeover_pending: true,
        browser_use_offered: false,
        window_index: Some(1),
    });
    assert!(takeover.contains("user currently has control"));
    assert!(takeover.contains("read-only Screenshot"));
    assert!(takeover.contains("do not send clicks"));

    let computer_use = render_computer_system_prompt(&ComputerPromptState {
        role: RunnerPromptRole::ComputerUseSubagent,
        box_available: true,
        desktop_capable: true,
        desktop_ready: true,
        control_lease_active: true,
        human_takeover_pending: false,
        browser_use_offered: false,
        window_index: Some(4),
    });
    assert!(computer_use.starts_with("## Computer"));
    assert!(computer_use.contains("window index is 4"));

    let no_lease = render_computer_system_prompt(&ComputerPromptState {
        role: RunnerPromptRole::ComputerUseSubagent,
        box_available: true,
        desktop_capable: true,
        desktop_ready: true,
        control_lease_active: false,
        human_takeover_pending: false,
        browser_use_offered: false,
        window_index: Some(4),
    });
    assert!(no_lease.contains("does not hold the live ComputerControlLease"));

    let other = render_computer_system_prompt(&ComputerPromptState {
        role: RunnerPromptRole::OtherSubagent,
        box_available: true,
        desktop_capable: true,
        desktop_ready: true,
        control_lease_active: false,
        human_takeover_pending: false,
        browser_use_offered: false,
        window_index: None,
    });
    assert!(other.is_empty());
}
