use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::cursor_backend::SandPrivacyMode;
use mahayana_host_runtime::extensions::forever_box::{
    DiskPressureLevel, DiskPressureReminderEpisodes,
};
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError, RoutedProvider,
    RoutedProviderCheckpoint, RoutedToolDefinition,
};
use mahayana_host_runtime::host_request_context::HostRequestContext;
use mahayana_host_runtime::runner::production_turn_agent_owner::{
    ProductionTurnAgentBuildBindings, ProductionTurnAgentLifecycleBindings,
    ProductionTurnAgentOwner, ProductionTurnAgentStaticConfig,
    ProductionTurnPrivacyModeResolver, ProductionTurnProfileAnnouncementCommit,
    ProductionTurnSummarizationPrompt,
};
use mahayana_host_runtime::runner::production_turn_run_shell_adapter::RoutedProviderCheckpointStore;
use mahayana_host_runtime::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedToolBridge, RunnerRequestContextSnapshot,
};
use mahayana_host_runtime::runner::sand_agent_runner::SandAgentRunner;
use mahayana_host_runtime::runner::send_message_reminder_middleware::DISK_PRESSURE_REMINDER_MESSAGE;
use mahayana_host_runtime::runner::subagent_runtime::{
    ControlResult, RunOutcome as SubagentRunOutcome, SubagentRuntime, SubagentStatus,
};
use mahayana_host_runtime::runner::turn_agent_composition::TurnAgentComposition;
use mahayana_host_runtime::runner::tools::box_help_tool::WAITING_USER_CANCELLATION_PREFIX;
use mahayana_host_runtime::runner::{TerminalOutcome, TurnRunOptions};
use serde_json::Value;

struct EmptyBridge;

impl RoutedToolBridge for EmptyBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool(
            "contract bridge has no tools".into(),
        ))
    }
}

struct MemoryCheckpointStore;

impl RoutedProviderCheckpointStore for MemoryCheckpointStore {
    fn persist(
        &self,
        _checkpoint: &RoutedProviderCheckpoint,
    ) -> Result<String, ProviderSessionError> {
        Ok("memory-checkpoint".into())
    }
}

fn composition() -> TurnAgentComposition {
    TurnAgentComposition::new(
        RoutedProvider::OpenRouter,
        Arc::new(EmptyBridge),
        RunnerRequestContextSnapshot {
            context: HostRequestContext {
                os_version: "test".into(),
                shell: None,
                time_zone: Some("UTC".into()),
                transcripts_folder: "/tmp/transcripts".into(),
                user_full_name: None,
            },
            rules: None,
        },
        RoutedProviderCancellation::default(),
        Arc::new(MemoryCheckpointStore),
    )
}

fn runner() -> SandAgentRunner {
    SandAgentRunner::new(ProductionTurnAgentOwner::new(composition()))
}

fn user_messages() -> Vec<ProviderMessage> {
    vec![ProviderMessage {
        role: "user".into(),
        content: "hello".into(),
    }]
}

fn build_bindings(
    conversation_id: &str,
    summarization: ProductionTurnSummarizationPrompt,
) -> ProductionTurnAgentBuildBindings {
    let privacy_mode_resolver: ProductionTurnPrivacyModeResolver =
        Arc::new(|| Some(SandPrivacyMode::NoTraining));
    ProductionTurnAgentBuildBindings::new(
        ProductionTurnAgentStaticConfig {
            model_id: "grok-contract".into(),
            agent_token_limit: 200_000,
            conversation_id: conversation_id.into(),
            is_box_scoped_subagent: false,
            is_subagent_runner: false,
            is_shared_room_runner: false,
            sand_send_message_delivery_owed: true,
            transcripts_folder_available: true,
        },
        privacy_mode_resolver,
        summarization,
    )
}

#[test]
fn production_owner_freezes_build_input_and_privacy_once() {
    let resolve_count = Arc::new(AtomicUsize::new(0));
    let resolver_count = Arc::clone(&resolve_count);
    let privacy_mode_resolver: ProductionTurnPrivacyModeResolver = Arc::new(move || {
        resolver_count.fetch_add(1, AtomicOrdering::SeqCst);
        Some(SandPrivacyMode::NoStorage)
    });
    let summarization: ProductionTurnSummarizationPrompt =
        Arc::new(|system, user, _| Ok(format!("{system}:{user}")));
    let owner = ProductionTurnAgentOwner::new(composition())
        .with_build_bindings(ProductionTurnAgentBuildBindings::new(
            ProductionTurnAgentStaticConfig {
                model_id: "grok-contract".into(),
                agent_token_limit: 200_000,
                conversation_id: "agent-build".into(),
                is_box_scoped_subagent: true,
                is_subagent_runner: true,
                is_shared_room_runner: false,
                sand_send_message_delivery_owed: true,
                transcripts_folder_available: true,
            },
            privacy_mode_resolver,
            summarization,
        ))
        .with_lifecycle_bindings(ProductionTurnAgentLifecycleBindings::new(
            "agent-build",
            "request-build",
        ));

    assert_eq!(resolve_count.load(AtomicOrdering::SeqCst), 1);
    let build = owner.build_input().expect("frozen build input");
    assert_eq!(build.static_config.model_id, "grok-contract");
    assert_eq!(build.static_config.conversation_id, "agent-build");
    assert!(build.static_config.is_box_scoped_subagent);
    assert!(build.static_config.is_subagent_runner);
    assert!(build.static_config.sand_send_message_delivery_owed);
    assert!(build.static_config.transcripts_folder_available);
    assert_eq!(build.static_projection.max_steps, 5_000);
    assert_eq!(build.static_projection.agent_token_limit, 200_000);
    assert_eq!(
        build.static_projection.background_summarization_start_unused_tokens,
        10_000
    );
    assert_eq!(
        build.static_projection.background_summarization_persist_unused_tokens,
        5_000
    );
    assert!(build.static_projection.enable_watch_video_in_ide_subagent);
    assert!(build.static_projection.user_message_timestamps);
    assert!(build.static_projection.rerender_user_info_on_request_context_recovery);
    assert!(build.static_projection.rerender_user_info_on_summarization);
    assert!(build.static_projection.skip_pre_turn_state_snapshot);
    assert_eq!(build.static_projection.agent_type, "IDE");
    assert_eq!(build.static_projection.conversation_group_id, "agent-build");
    assert!(build.static_projection.disable_user_info);
    assert!(build.static_projection.display_cursor_rules);
    assert!(!build.static_projection.display_skills);
    assert!(build.static_projection.exclude_agent_transcripts);
    assert!(!build.static_projection.enable_terminal_files);
    assert!(build.static_projection.enable_transcript_in_summary);
    assert_eq!(build.privacy_mode, Some(SandPrivacyMode::NoStorage));
    assert_eq!(
        owner
            .run_summarization_prompt("system", "user", &|| false)
            .expect("turn-owned summarization"),
        "system:user"
    );
    assert_eq!(
        resolve_count.load(AtomicOrdering::SeqCst),
        1,
        "privacy mode must be frozen once for the turn owner"
    );
}

#[test]
fn production_owner_binds_disk_pressure_summarization_and_profile_lifecycle() {
    let reminders = Arc::new(DiskPressureReminderEpisodes::new(
        None,
        Some(Arc::new(|| "episode-success".to_string())),
        None,
    ));
    reminders.observe_pressure(DiskPressureLevel::Soft, true);
    let profile_commits = Arc::new(AtomicUsize::new(0));
    let profile_counter = Arc::clone(&profile_commits);
    let profile_commit: ProductionTurnProfileAnnouncementCommit =
        Arc::new(move || {
            profile_counter.fetch_add(1, AtomicOrdering::SeqCst);
        });
    let summarization: ProductionTurnSummarizationPrompt = Arc::new(
        |system_prompt, user_prompt, should_cancel| {
            if should_cancel() {
                return Err(ProviderSessionError::Cancelled(
                    "summary cancelled".into(),
                ));
            }
            Ok(format!("{system_prompt}::{user_prompt}"))
        },
    );
    let mut owner = ProductionTurnAgentOwner::new(composition())
        .with_build_bindings(build_bindings("agent-success", summarization))
        .with_lifecycle_bindings(
        ProductionTurnAgentLifecycleBindings::new(
            "agent-success",
            "request-success",
        )
        .with_disk_pressure_reminders(Some(Arc::clone(&reminders)))
        .with_profile_announcement_commit(Some(profile_commit)),
    );

    assert_eq!(owner.disk_pressure_episode_id(), Some("episode-success"));
    let projected = owner.project_provider_messages_for_turn(&user_messages());
    assert!(projected.iter().any(|message| {
        message.role == "user"
            && message.content.contains(DISK_PRESSURE_REMINDER_MESSAGE)
    }));
    assert_eq!(
        owner
            .run_summarization_prompt("system", "user", &|| false)
            .expect("summarization surface"),
        "system::user"
    );
    owner
        .run_with(&user_messages(), || Ok("done".into()))
        .expect("successful turn");

    assert_eq!(profile_commits.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(
        reminders.claim("agent-success", "request-after-success"),
        None,
        "successful owner must commit the active disk-pressure episode"
    );
}

#[test]
fn production_owner_releases_uncommitted_disk_pressure_on_dispose() {
    let reminders = Arc::new(DiskPressureReminderEpisodes::new(
        None,
        Some(Arc::new(|| "episode-release".to_string())),
        None,
    ));
    reminders.observe_pressure(DiskPressureLevel::Hard, true);
    let profile_commits = Arc::new(AtomicUsize::new(0));
    let profile_counter = Arc::clone(&profile_commits);
    let profile_commit: ProductionTurnProfileAnnouncementCommit =
        Arc::new(move || {
            profile_counter.fetch_add(1, AtomicOrdering::SeqCst);
        });
    let summarization: ProductionTurnSummarizationPrompt =
        Arc::new(|_, _, _| Ok("summary".into()));
    let mut owner = ProductionTurnAgentOwner::new(composition())
        .with_build_bindings(build_bindings("agent-release", summarization))
        .with_lifecycle_bindings(
        ProductionTurnAgentLifecycleBindings::new(
            "agent-release",
            "request-release",
        )
        .with_disk_pressure_reminders(Some(Arc::clone(&reminders)))
        .with_profile_announcement_commit(Some(profile_commit)),
    );

    let error = owner
        .run_with(&user_messages(), || {
            Err(ProviderSessionError::Transport("upstream reset".into()))
        })
        .expect_err("failed turn");
    assert!(error.to_string().contains("upstream reset"));
    assert_eq!(profile_commits.load(AtomicOrdering::SeqCst), 0);
    owner.dispose();
    assert_eq!(
        reminders.claim("agent-release", "request-retry"),
        Some("episode-release".to_string()),
        "failed owner must release rather than commit the episode"
    );
}

#[test]
fn shipping_sand_agent_owner_settles_completed_turn_once() {
    let mut runner = runner();
    let result = runner
        .run_with(&user_messages(), || Ok("done".into()))
        .expect("completed turn");
    assert_eq!(result, "done");
    assert!(matches!(
        runner.last_finished().map(|finished| &finished.outcome),
        Some(TerminalOutcome::Completed)
    ));
}

#[test]
fn shipping_sand_agent_owner_preserves_retryable_transport_failure() {
    let mut runner = runner();
    let error = runner
        .run_with(&user_messages(), || {
            Err(ProviderSessionError::Transport(
                "upstream reset".into(),
            ))
        })
        .expect_err("transport failure");
    assert!(error.to_string().contains("upstream reset"));
    assert!(matches!(
        runner.last_finished().map(|finished| &finished.outcome),
        Some(TerminalOutcome::Failed {
            retryable: true,
            message,
        }) if message.contains("upstream reset")
    ));
}

#[test]
fn shipping_sand_agent_owner_settles_provider_cancellation() {
    let mut runner = runner();
    let error = runner
        .run_with(&user_messages(), || {
            Err(ProviderSessionError::Cancelled(
                "user cancelled".into(),
            ))
        })
        .expect_err("cancelled turn");
    assert!(matches!(error, ProviderSessionError::Cancelled(_)));
    assert!(matches!(
        runner.last_finished().map(|finished| &finished.outcome),
        Some(TerminalOutcome::Cancelled)
    ));
}

#[test]
fn shipping_sand_agent_owner_settles_box_help_as_waiting_user() {
    let mut runner = runner();
    let error = runner
        .run_with(&user_messages(), || {
            Err(ProviderSessionError::Cancelled(format!(
                "{WAITING_USER_CANCELLATION_PREFIX}request-1"
            )))
        })
        .expect_err("waiting user turn");
    assert!(matches!(error, ProviderSessionError::Cancelled(_)));
    assert!(matches!(
        runner.last_finished().map(|finished| &finished.outcome),
        Some(TerminalOutcome::WaitingUser)
    ));
}

#[test]
fn shipping_sand_agent_owner_fails_closed_without_user_prompt() {
    let mut runner = runner();
    let messages = vec![ProviderMessage {
        role: "system".into(),
        content: "system only".into(),
    }];
    let mut executed = false;
    let error = runner
        .run_with(&messages, || {
            executed = true;
            Ok("unexpected".into())
        })
        .expect_err("missing user prompt");
    assert!(matches!(error, ProviderSessionError::Configuration(_)));
    assert!(!executed);
    assert!(runner.last_finished().is_none());
}


#[test]
fn shipping_sand_agent_owner_preserves_projected_request_identity_through_terminal_settlement() {
    let mut runner = runner();
    let result = runner
        .run_with_options(
            &user_messages(),
            TurnRunOptions {
                inference_request_id: Some("stream-request-123".into()),
                ..TurnRunOptions::default()
            },
            || Ok("done".into()),
        )
        .expect("completed projected turn");
    assert_eq!(result, "done");
    let finished = runner.last_finished().expect("terminal settlement");
    assert_eq!(finished.owner.request_id, "stream-request-123");
    assert_eq!(finished.owner.generation, 1);
    assert!(matches!(finished.outcome, TerminalOutcome::Completed));
}


#[test]
fn shipping_sand_agent_runner_owns_and_settles_generated_subagent_runtime() {
    let shared = Arc::new(Mutex::new(SubagentRuntime::default()));
    let runner = runner().with_generated_agent_runtime(Arc::clone(&shared));
    assert!(runner.generated_agent_runtime().is_some());

    let pending = runner
        .begin_generated_subagent_with_lifecycle(
            "parent-agent",
            "box-a",
            "child-agent",
            "general-purpose",
            "tool-call-1",
            "research this",
            None,
            "child-lifecycle-1",
            100,
        )
        .expect("generated subagent begin")
        .expect("new subagent pending wake metadata");
    assert_eq!(pending.parent_agent_id, "parent-agent");
    assert_eq!(pending.work_id, "child-agent");
    {
        let runtime = shared.lock().expect("shared runtime");
        assert_eq!(runtime.list_running_subagents(150).len(), 1);
        assert_eq!(runtime.list_running_subagents(150)[0].subagent_id, "child-agent");
    }

    let settled = runner
        .settle_generated_subagent(
            "child-agent",
            SubagentRunOutcome::Completed("done".into()),
            200,
        )
        .expect("generated subagent settle");
    let completion = settled.completion.expect("generated subagent completion");
    assert_eq!(completion.lifecycle_revision, "child-lifecycle-1");
    let runtime = shared.lock().expect("shared runtime after settle");
    assert!(runtime.list_running_subagents(250).is_empty());
    assert!(runtime
        .list_subagents()
        .iter()
        .any(|(id, record)| id == "child-agent" && record.status == SubagentStatus::Done));
}

#[test]
fn shipping_sand_agent_runner_exposes_generated_subagent_control_surface() {
    let shared = Arc::new(Mutex::new(SubagentRuntime::default()));
    let runner = runner().with_generated_agent_runtime(Arc::clone(&shared));

    runner
        .begin_generated_subagent_with_lifecycle(
            "parent-agent",
            "box-a",
            "child-agent",
            "general-purpose",
            "tool-call-2",
            "inspect lifecycle",
            None,
            "child-lifecycle-2",
            100,
        )
        .expect("begin generated subagent")
        .expect("pending wake");

    assert!(runner.has_subagent("child-agent").expect("has subagent"));
    assert!(runner.has_running_subagents().expect("has running subagents"));
    assert_eq!(runner.list_subagents().expect("list subagents").len(), 1);
    assert_eq!(
        runner
            .get_running_subagent("child-agent", 150)
            .expect("get running subagent")
            .expect("running subagent")
            .subagent_id,
        "child-agent"
    );
    assert!(matches!(
        runner
            .steer_subagent("child-agent", "focus on lifecycle")
            .expect("steer subagent"),
        ControlResult::Ok { .. }
    ));
    assert!(matches!(
        runner.abort_subagent("child-agent").expect("abort subagent"),
        ControlResult::Ok { .. }
    ));

    runner.reset().expect("reset generated subagent state");
    assert!(!runner.has_subagent("child-agent").expect("subagent cleared"));
    assert!(!runner.has_running_subagents().expect("running cleared"));
    assert!(runner.get_subagent_outline("child-agent").expect("outline").is_empty());
}

#[test]
fn shipping_sand_agent_owner_marks_upgrade_quiesce_in_terminal_settlement() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let signal = Arc::new(AtomicBool::new(false));
    let composition = TurnAgentComposition::new(
        RoutedProvider::OpenRouter,
        Arc::new(EmptyBridge),
        RunnerRequestContextSnapshot {
            context: HostRequestContext {
                os_version: "test".into(),
                shell: None,
                time_zone: Some("UTC".into()),
                transcripts_folder: "/tmp/transcripts".into(),
                user_full_name: None,
            },
            rules: None,
        },
        RoutedProviderCancellation::default(),
        Arc::new(MemoryCheckpointStore),
    );
    let mut runner = SandAgentRunner::new(
        ProductionTurnAgentOwner::new(composition)
            .with_upgrade_quiesce_signal(Arc::clone(&signal)),
    );
    let signal_for_run = Arc::clone(&signal);
    let error = runner
        .run_with(&user_messages(), move || {
            signal_for_run.store(true, Ordering::Release);
            Err(ProviderSessionError::Cancelled(
                "quiescing for forced host upgrade".into(),
            ))
        })
        .expect_err("upgrade quiesce cancels the active turn");
    assert!(matches!(error, ProviderSessionError::Cancelled(_)));
    let finished = runner.last_finished().expect("terminal settlement");
    assert!(finished.quiesced_for_upgrade);
    assert!(matches!(finished.outcome, TerminalOutcome::Cancelled));
}
