//! Grok-aligned Mahayana Host process entrypoint.
//!
//! The shipping desktop Host process is owned by `source/host/app`. During the
//! migration, `mahayana-unified-app-host` remains an internal compatibility
//! backend so existing product commands keep working while Grok Host/Runner
//! modules are moved behind this process boundary. Electron must never launch
//! the legacy third_party desktop Host binary directly.

mod ocrm_handlers;
pub(crate) use ocrm_handlers::*;
pub(crate) use mahayana_host_runtime::extensions::transcript::turn_runtime::classify_agent_error;
pub(crate) use mahayana_host_runtime::extensions::transcript::send_group_fanout::{
    collect_new_member_send_messages, GROUP_MEMBER_DM_PREEMPTED_ERROR,
};
pub(crate) use mahayana_host_runtime::extensions::transcript::group_chat_glue::should_redrive_group_member_after_preemption;
pub(crate) use mahayana_host_runtime::extensions::inference::provider_session::configured_routed_provider;

use mahayana_host_runtime::extensions::action_audit::action_audit_service::{AuditAction, AuditRecord};
use mahayana_host_runtime::extensions::box_store_sync::box_copy_in::{
    BOX_COPY_IN_ARG, execute_production_box_copy_in_from_env,
};
use mahayana_host_runtime::extensions::box_store_sync::production::ProductionBoxStoreSyncApi;
use mahayana_host_runtime::extensions::local_exec::native_file_sandbox::{
    LOCAL_EXEC_FILE_SANDBOX_ARG, execute_production_native_file_request_from_env,
};
use mahayana_host_runtime::extensions::action_audit::extension::ActionAuditExtension;
use mahayana_host_runtime::extensions::attachments::attachments_service::AttachmentsService;
use mahayana_host_runtime::extensions::attachments::generate_image_service::{
    GenerateImageAuth, PersistGeneratedImage, PersistedImage,
};
use mahayana_host_runtime::extensions::attachments::attachments_service::persist_image_bytes;
use mahayana_host_runtime::extensions::auto_review::extension::HostAutoReviewExtension;
use mahayana_host_runtime::extensions::auto_review::sand_backend_smart_mode_classifier_exec::{
    create_sand_backend_smart_mode_classifier_executor_with_cancellation,
    pin_sand_auto_review_classifier_measurement_reporter,
};
use mahayana_host_runtime::extensions::auth::extension::HostAuthExtension;
use mahayana_host_runtime::extensions::telemetry::auto_review_approval_telemetry::{
    AutoReviewApprovalReport,
};
use mahayana_host_runtime::runner::auto_review_gate::{
    AutoReviewGate, AutoReviewGateDependencies, AutoReviewInstructions, ShellApprovalSurface,
};
use mahayana_host_runtime::runner::sand_auto_review::{
    SandAutoReviewApprovalStatus, SandAutoReviewController, SandAutoReviewDecision,
    SandAutoReviewEvent, SandAutoReviewExpiryCause, SandAutoReviewMode, SandAutoReviewModes,
    SandAutoReviewResolution, fingerprint_sand_auto_review_target,
    sand_auto_review_approval_expiry_policy,
};
use mahayana_host_runtime::runner::sand_auto_review_classifier_run::{
    AutoReviewClassifierDecision, SmartModeClassifierMeasurementKind,
    run_sand_auto_review_classifier,
};
use mahayana_host_runtime::runner::sand_auto_review_tool_escalations::{
    McpApprovalRequest, ShellApprovalRequest, ShellApprovalTarget,
    request_sand_mcp_approval, request_sand_shell_approval,
};
use mahayana_host_runtime::runner::sand_automation_auto_review::{
    AutomationReviewOutcome, SAND_AUTOMATION_WRITE_CLASSIFIER_ERROR_REASON,
    review_sand_automation_write,
};
use mahayana_host_runtime::runner::sand_auto_review_summaries::CloudLifecycleAction;
use mahayana_host_runtime::runner::sand_browser_auto_review::run_sand_browser_auto_review_preflight;
use mahayana_host_runtime::runner::sand_computer_auto_review::{
    BoxIdentity, InstructionPermissions, SAND_COMPUTER_AUTO_REVIEW_CLASSIFIER_ERROR_REASON,
    run_sand_computer_auto_review_preflight,
};
use mahayana_host_runtime::runner::sand_cloud_agent_auto_review::{
    CloudAgentReviewImage, CloudAgentReviewOutcome,
    SAND_CLOUD_AGENT_CLASSIFIER_ERROR_REASON,
    build_sand_cloud_agent_lifecycle_review_target, build_sand_cloud_agent_review_target,
    review_sand_cloud_agent_action, review_sand_cloud_agent_lifecycle_action,
};
use mahayana_host_runtime::runner::sand_subagent_auto_review::{
    SubagentReviewOutcome, SAND_SUBAGENT_CLASSIFIER_ERROR_REASON,
    build_sand_subagent_steer_review_target,
    review_sand_subagent_action,
};
use mahayana_host_runtime::extensions::cloud_agents::cloud_agents_service::SandCloudAgentManager;
use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::memory::agent_state::AvatarBoxFileReader;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::session::session_paths::{
    is_root_main_session_key, resolve_internal_send_route, resolve_public_session_route,
    InternalSessionRouteError, PublicSessionRoute, PublicSessionRouteError, SessionKind,
};
use mahayana_host_runtime::extensions::session::session_profile_files::AgentProfileUpdate;
use mahayana_host_runtime::agents::agent_messaging::{
    AgentAddress, AgentGroupAddress, AgentMessageImage, build_admin_broadcast_wake_prompt,
    clamp_agent_message,
};
use mahayana_host_runtime::connectors::channel_delivery::HostChannelDelivery;
use mahayana_host_runtime::agents::agent_profile::{SandAgentProfile, get_sand_profile_path};
use mahayana_host_runtime::agents::settings_file::get_sand_settings_path;
use mahayana_host_runtime::extensions::transcript::agent_to_agent_messaging::{
    AgentWakeRequest, ProductionAgentToAgentMessaging, agent_inbound_failure_report,
    agent_inbound_failure_tray, persist_agent_inbound_message, should_interrupt_priority_peer,
};
use mahayana_host_runtime::extensions::telemetry::HostTelemetryProjection;
use mahayana_host_runtime::extensions::local_exec::extension::HostLocalExecExtension;
use mahayana_host_runtime::extensions::local_exec::gateway_local_exec_sand_box::GatewayLocalToolScope;
use mahayana_host_runtime::extensions::local_tool_permission::extension::{
    HostLocalToolPermissionExtension,
};
use mahayana_host_runtime::extensions::local_tool_permission::local_tool_permission_resolution::{
    LocalToolPermissionResolutionArgs, SandLocalToolPermissionResolutionError,
};
use mahayana_host_runtime::host_runner_composition::{
    HostRunnerComposition, ProductionTurnCompositionHooks,
};
use mahayana_host_runtime::extensions::session::box_handoff_service::{
    BoxHandoffDeps, BoxHandoffService, HandoffDecision, HandoffRequest, HandoffStartResult,
    HandoffTelemetry, HandoffTrigger, PendingHandoff, ScreenshotPayload,
};
use mahayana_host_runtime::extensions::settings::settings_service::SettingsService;
use mahayana_host_runtime::extensions::secrets::extension::{
    HostSecretsExtension, SecretsGatewayError, dispatch_secrets_gateway_call,
};
use mahayana_host_runtime::extensions::notifications::extension::notification_agent_from_value;
use mahayana_host_runtime::extensions::session::gateway::{
    SessionGatewayError, persist_accepted_send_prompt_context,
};
use mahayana_host_runtime::extensions::transcript::production_runtime::{
    ProductionSendError, ProductionTranscriptRuntime,
};
use mahayana_host_runtime::extensions::transcript::background_wakes::{
    BackgroundWakes, build_channel_delivery_failure_wake_prompt,
    build_channel_inbound_wake_prompt, build_channel_outbound_message,
    build_timeline_event_wake_prompt, distinct_inbound_channel_addresses,
    format_channel_address_value, humanize_channel_delivery_failure,
    redrivable_inbound_envelopes,
};
use mahayana_host_runtime::extensions::transcript::completion_revivals::{
    CompletionRevivalRuntimePort, CompletionRevivals, RevivalExecution, RevivalReport,
    ShellCompletion, SubagentCompletion,
};
use mahayana_host_runtime::extensions::transcript::sand_pending_wake_store::{
    DurablePendingWakeMarker, PendingWakeKind, QuietWakeOrigin, coerce_quiet_origin,
};
use mahayana_host_runtime::extensions::transcript::pending_wake_rearm::{
    LostSubagentWake, PendingWakeReport, PendingWakeRuntimePort,
};
use mahayana_host_runtime::extensions::transcript::group_chat_glue::{
    GroupMemberPreview, GroupRoomEntryObserver, group_member_reaction_target,
};
use mahayana_host_runtime::extensions::transcript::group_chat_orchestrator::GroupMemberTurnRequest;
use mahayana_host_runtime::extensions::cross_user_sharing::production::{
    CrossUserGatewayError, ProductionCrossUserRuntime, RemoteRequestedTurnRunner,
    SharedRoomTurnRunner,
};
use mahayana_host_runtime::groups::group_chat::{GroupDescription, GroupMember};
use mahayana_host_runtime::extensions::transcript::send_group_fanout::{
    GroupMemberTurnExecutor, LocalGroupFanoutDisposition,
};
use mahayana_host_runtime::extensions::transcript::send_acceptance::emit_accepted_send_echoes;
use mahayana_host_runtime::extensions::transcript::send_pipeline::PersistedSendContext;
use mahayana_host_runtime::extensions::transcript::send_turn_dispatch::prepare_direct_turn_runner_args;
use mahayana_host_runtime::extensions::transcript::roster_emit::ProductionRosterEmit;
use mahayana_host_runtime::extensions::transcript::transcript_manager::TranscriptManager;
use mahayana_host_runtime::extensions::transcript::transcript_entry_ids::{
    TranscriptEntryIdKind, next_entry_id,
};
use mahayana_host_runtime::extensions::transcript::send_message_shaping::{
    collect_inbound_images, shape_send_prompt_media_args,
};
use mahayana_host_runtime::extensions::transcript::box_handoff_resume::{
    BOX_HANDOFF_RESUME_TITLE, LISTENER_CONNECT_RESUME_TITLE, MCP_AUTH_RESUME_TITLE,
    box_handoff_resume_prompt, format_mcp_account_display_name, listener_connect_resume_prompt,
    mcp_auth_resume_prompt, should_resume_hidden_handoff,
};
use mahayana_host_runtime::extensions::transcript::sand_upgrade_resume_store::{
    upgrade_resume_marker_identity_is_well_formed, UpgradeResumeMarker,
};
pub(crate) use mahayana_host_runtime::extensions::transcript::upgrade_recreate_resume::build_upgrade_resume_prompt;
use mahayana_host_runtime::extensions::transcript::box_request_entries::resolve_box_request_entry;
use mahayana_host_runtime::extensions::transcript::workflow_commands::{
    WorkflowCommandError, WorkflowRunNowPlan,
};
use mahayana_host_runtime::extensions::transcript::automation_run_path::{
    AutomationExecutionResult, AutomationRunFenceSnapshot, FireAutomationOutcome,
};
use mahayana_host_runtime::extensions::transcript::automation_runtime::{
    AutomationCommandError, dispatch_automation_command,
};
use mahayana_host_runtime::automations::automation_status_reminder::create_automation_status_reminder;
use mahayana_host_runtime::automations::automation_id::stable_automation_id;
use mahayana_host_runtime::automations::automation_trigger::{trigger_matches_event, trigger_members};
use mahayana_host_runtime::extensions::automations::listener_integrations::count_listener_platforms;
use mahayana_host_runtime::extensions::automations::fire_delivery::{
    PreparedBackendFire, prepare_backend_fire,
};
use mahayana_host_runtime::extensions::automations::production_lifecycle::ProductionAutomationsLifecycle;
use mahayana_host_runtime::extensions::automations::sand_automation_cloud_sync::{
    ScheduledCloudAutomation, is_server_schedulable, sand_cloud_definition,
};
use mahayana_host_runtime::extensions::automations::sand_automation_fire_consumer::{
    BackendAutomationFire, FireCompletion,
};
use mahayana_host_runtime::extensions::transcript::async_task_union::AsyncTask;
use mahayana_host_runtime::extensions::transcript::ack_obligations::{
    AckObligations, AckRedrivePreparation, build_ack_redrive_empty_delivery_report,
    build_ack_redrive_send_args,
};
use mahayana_host_runtime::extensions::transcript::runner_registry::{
    RUN_DIRECT_USER_INTERRUPT_REASON, RUN_WATCHDOG_INTERRUPT_REASON, TranscriptRunnerRegistry,
};
use mahayana_host_runtime::extensions::transcript::run_scheduler::WatchdogStage;
use mahayana_host_runtime::extensions::transcript::agent_lifecycle::{
    AgentDeletionRuntimeDeps, AgentKickstartHook, AgentLifecycleGatewayError,
    CreatedAgentKickstartRuntimePort, INTRODUCTION_FAILED_TRAY_TITLE,
    KickstartRunError, KickstartTurnOutcome, introduction_failed_tray_key,
    request_disk_saver_audit, run_created_agent_kickstart,
};
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError, RoutedProvider, RoutedProviderOptions,
    RoutedMcpMetaToolDefinition, RoutedToolDefinition,
    run_routed_provider_text,
};
use mahayana_host_runtime::extensions::session::agent_db::{
    CompactionCommit, CompactionCommitOutcome, ProviderReviewContinuationOutcome,
    SessionProviderReview,
};
use mahayana_host_runtime::extensions::inference::inference_service::{
    InferenceUsage, authorize_routed_provider_request,
};
use mahayana_host_runtime::extensions::inference::production::ProductionInferenceExtension;
use mahayana_host_runtime::extensions::inference::cursor_session::{
    RequestLineage, SandSessionOptions,
};
use mahayana_host_runtime::extensions::inference::generated_inference_codec::InferenceReason;
use mahayana_host_runtime::extensions::webauthn_proxy::extension::HostWebAuthnProxyExtension;
use mahayana_host_runtime::extensions::telemetry::analytics_service::TelemetryService;
use mahayana_host_runtime::extensions::telemetry::automation_fire_telemetry::{
    AutomationFireDroppedReport, AutomationRunReport,
};
use mahayana_host_runtime::extensions::telemetry::agent_open_telemetry::AgentOpenReport;
use mahayana_host_runtime::extensions::telemetry::queue_telemetry_mappers::{
    PendingWakeReport as TelemetryPendingWakeReport, QueueAcceptedReport, QueueDequeuedReport,
    QueueWatchdogReport, SendDispatchReport,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    ConnectorAuthGatewayError, HostBundleIdentity, HostProductAnalytics, HostStructuredLogTelemetry,
    HostTelemetryApi, MessageSentReport, McpDiscoveryFailedGatewayError, dispatch_connector_auth_gateway,
    dispatch_mcp_discovery_failed_gateway,
};
use mahayana_host_runtime::extensions::telemetry::mcp_discovery_telemetry::McpDiscoveryFailedReport;
use mahayana_host_runtime::extensions::telemetry::host_lifecycle_progress::{
    HostLifecycleCompletion, production_host_lifecycle_watchdog,
};
use mahayana_host_runtime::extensions::telemetry::revival_telemetry_mappers::{
    ShellRevivalReport, SubagentRevivalReport,
};
use mahayana_host_runtime::extensions::experiments::HostExperimentsExtension;
use mahayana_host_runtime::extensions::content_search::extension::ProductionContentSearchExtension;
use mahayana_host_runtime::extensions::trays::extension::HostTraysExtension;
use mahayana_host_runtime::extensions::host_upgrade::production::{
    ProductionHostUpgradeExtension, ProductionHostUpgradePeers,
};
use mahayana_host_runtime::extensions::host_upgrade::host_bundle_upgrade::{
    SAND_BOX_HOST_VERSION_PATH, read_local_host_version,
};
use mahayana_host_runtime::extensions::node_host::session_registry::{
    NodeHostSessionLease, NodeHostSessionMetadata,
};
use mahayana_host_runtime::extensions::node_host::invoke_protocol::{
    NodeInvokeRequest, NodeInvokeResult, MAX_PARAMS_JSON_BYTES,
};
use mahayana_host_runtime::extensions::node_host::invocation_runtime::NodeHostInvocationRuntime;
use mahayana_host_runtime::extensions::node_host::runtime_state::NodeHostRuntimeState;
use mahayana_host_runtime::extensions::node_host::runtime_manifest::{
    NodeHostManifest, NODE_SYSTEM_RUN_COMMAND,
};
use mahayana_host_runtime::host_production_extensions::{
    CURRENT_SHIPPING_PRODUCTION_EXTENSION_IDS, start_production_host_box_extensions,
    start_production_host_extensions,
};
use mahayana_host_runtime::host_roster_bookkeeping::{
    HostRosterBookkeeping, production_host_roster_bookkeeping,
};
use mahayana_host_runtime::network_enrollment_owner::{
    DEFAULT_FABUSHI_API_BASE_URL, FABUSHI_API_BASE_URL_ENV, NetworkEnrollmentOwner,
    NetworkEnrollmentSupervisor,
};
use mahayana_host_runtime::remote_computer_target::{
    FileRemoteComputerDeviceSecretProvider, RemoteComputerDeviceSecretProvider,
    production_remote_computer_target_service,
};
use mahayana_host_runtime::extensions::auth::auth_service::FabushiCoreCredentialProvider;
use mahayana_host_runtime::host_invariant::{install_invariant_reporter, invariant_failure};
#[cfg(test)]
use mahayana_host_runtime::host_production_extensions::{
    ProductionBrowserUaLog, ProductionHostExtensions,
};
use mahayana_host_runtime::extensions::telemetry::turn_telemetry_mappers::{
    ClosingSendNudgeFields, ComputerUseUsageFields, TokenUsage as TelemetryTokenUsage,
    TtftFields, TurnAwaitFields, TurnInterruptFields, TurnRetryFields, TurnUsageFields,
    UserMessageReceivedFields,
};
use mahayana_host_runtime::extensions::telemetry::agent_error_telemetry::AgentErrorReport;
use mahayana_host_runtime::extensions::telemetry::journal_outcome_telemetry::JournalOutcomeReport;
use mahayana_host_runtime::extensions::telemetry::sand_error_tags::SandErrorValue;
pub(crate) use mahayana_host_runtime::extensions::transcript::agent_run_error::provider_failure_tray;
use mahayana_host_runtime::extensions::transcript::turn_runtime::{
    REPLY_NUDGE_PROMPT, TurnTerminalKind, build_turn_empty_delivery_report,
    is_delivery_owed, project_turn_terminal,
    shape_closing_send_nudge_turn_input, shape_reply_nudge_turn_input, should_attempt_reply_nudge,
};
pub(crate) use mahayana_host_runtime::ports::telemetry::sand_error_detail;
use mahayana_host_runtime::r#box::box_transfer::TransferBox;
use mahayana_host_runtime::extensions::forever_box::{
    ForeverBoxRemoteResourceLifecycle, ForeverBoxRunnerResourcePort, BoxStatus, ForeverBoxService,
};
use mahayana_host_runtime::extensions::teach_recording::teach_recording_service::{
    TeachRecordingApi, TeachStatus,
};
use mahayana_host_runtime::runner_context_production_provider::ProductionRunnerRequestContextSource;
use mahayana_host_runtime::send_trace_host::{
    BeginTurnTraceOptions, HostTrace, TurnTraceTypeOptions, add_turn_trace_event, begin_send_trace,
    begin_turn_trace, mark_turn_trace_error, record_completed_trace_span, resolve_turn_trace_type,
    set_turn_trace_attributes,
};
use mahayana_host_runtime::sand_activity::ActivityUpdate;
use mahayana_host_runtime::runner::production_turn_run_shell_adapter::{
    ProviderRetryEvent, ProviderRetryOutcome, ProviderRetryReport,
};
use mahayana_host_runtime::runner::production_turn_input_projection::create_production_turn_input_projection;
use mahayana_host_runtime::runner::is_recovery_shaped_turn;
use mahayana_host_runtime::runner::{
    should_run_completed_turn_side_effects, should_run_turn_memory,
};
use mahayana_host_runtime::runner::prompt_collector_glue::{
    apply_staged_attachment_paths_for_turn, resolve_profile_update_for_turn,
    selected_media_host_paths_for_turn, unanswered_questions_user_message_for_turn,
};
use mahayana_host_runtime::runner::runner_prompt_glue::{
    RunnerPromptAutomationState, RunnerPromptGlueOwner, RunnerPromptMcpState,
    RunnerPromptProfileState, RunnerPromptRemoteState,
};
use mahayana_host_runtime::runner::sand_memory::{
    FrozenMemorySnapshot, MEMORY_PROJECT_INJECTED_CAP, MEMORY_PROJECT_PROFILE_PROMPT_LIMIT,
    MEMORY_PROJECT_RECENT_PROMPT_LIMIT, MEMORY_RECENT_PROMPT_LIMIT,
    MEMORY_USER_PROFILE_PROMPT_LIMIT, MEMORY_USER_RECENT_PROMPT_LIMIT,
    is_memorable_exchange, is_memory_freeze_enabled,
};
use mahayana_host_runtime::runner::turn_memory::{
    ProductionEpisodeProgress, TurnExchange, TurnMemoryMode, build_turn_memory_exchange,
    run_turn_memory_with,
};
use mahayana_host_runtime::runner::tools::sand_state_tool::{
    RoutineAutoReviewCallback, RoutinePostWriteCallback,
};
use mahayana_host_runtime::runner::tools::listener_connect_cards::{
    surface_listener_connect_cards,
};
use mahayana_host_runtime::runner::tools::sand_browser_tools::{
    BrowserAutoReviewCallback, BrowserPersistImageCallback, BrowserPossibleNavigationCallback,
    BrowserShellSafetyCallback, BrowserToolExecutor, ProductionBrowserToolExecutor,
    capture_browser_review_state,
    to_browser_review_action,
};
use mahayana_host_runtime::runner::host_computer_tool_dependencies::ProductionComputerToolExecutor;
use mahayana_host_runtime::runner::host_file_transfer_dependencies::ProductionFileTransferExecutor;
use mahayana_host_runtime::runner::host_external_machine_dependencies::ProductionExternalMachineExecutor;
use mahayana_host_runtime::runner::host_web_dependencies::ProductionWebToolExecutor;
use mahayana_host_runtime::runner::host_generate_image_dependencies::ProductionGenerateImageToolExecutor;
use mahayana_host_runtime::runner::tools::sand_file_transfer_tools::FileTransferExecutor;
use mahayana_host_runtime::runner::tools::sand_external_machine_tools::{
    ExternalMachineExecutor, ExternalMachineShellArgs, ExternalShellAutoReviewCallback,
};
use mahayana_host_runtime::runner::tools::sand_web_tools::WebToolExecutor;
use mahayana_host_runtime::runner::tools::sand_generate_image_tool::GenerateImageToolExecutor;
use mahayana_host_runtime::runner::tools::sand_computer_tool::{
    ComputerAutoReviewCallback, ComputerPersistImageCallback, ComputerToolExecutor,
    ComputerToolExposure, ReportedComputerAction,
    to_exact_action_value, validate_computer_action,
};
use mahayana_host_runtime::runner::system_prompt_assembly::{
    AgentProfileForPrompt, AgentSkillPromptItem, append_agent_directory_system_prompt,
    append_automations_system_prompt, append_channels_system_prompt,
    append_combined_memory_system_prompt, append_budgeted_workflows_system_prompt,
    ComputerPromptState, RemoteBoxPromptState, RunnerPromptRole,
    render_agent_profile_section, resolve_combined_memory_system_prompt,
};
use mahayana_host_runtime::runner::sand_agent_profile_prompt::{
    AgentProfileIdentity, AgentProfilePromptSnapshot, normalize_agent_profile_identity,
    persist_announced_agent_profile_snapshot, resolve_agent_profile_prompt_snapshot,
};
use mahayana_host_runtime::runner::production_turn_agent_owner::{
    ProductionTurnAgentBuildBindings, ProductionTurnAgentLifecycleBindings,
    ProductionTurnAgentStaticConfig, ProductionTurnPrivacyModeResolver,
    ProductionTurnProfileAnnouncementCommit, ProductionTurnSummarizationPrompt,
};
use mahayana_host_runtime::runner_production_bridge::{
    ProductionActionAuditInput, ProductionRunnerCompositionInput,
};
use mahayana_host_runtime::runner::sand_action_audit::{
    ActionAuditRecord, ActionAuditSink, navigation_probe_command, normalize_navigation_url,
    with_transport_resolving_sink,
};
use mahayana_host_runtime::runner::bot_block_detection::with_bot_block_detection_sink;
use mahayana_host_runtime::extensions::telemetry::bot_block_telemetry::BotBlockReport;
use mahayana_host_runtime::runner::turn_agent_composition::{
    SAND_AGENT_TOKEN_LIMIT, TurnSubagentLaunchReviewBindings, build_turn_subagent_types,
    create_turn_subagent_task_review,
};
use mahayana_host_runtime::runner::compaction_planning::{
    CompactionSettings, build_compaction_details_with_quality, build_compaction_summary_prompt,
    build_turn_prefix_summary_prompt, compaction_without_summary,
    fit_compaction_summary_with_suffix, format_compaction_details_suffix,
    prepare_compaction_plan,
};
use mahayana_host_runtime::runner::turn_observation::{
    ToolCallTelemetryEvent, TurnObservation, TurnObservationHandle,
    async_tasks_changed_event,
};
use mahayana_host_runtime::runner::routed_provider_runtime::{
    ProductionRoutedProviderCheckpointStore, RoutedProviderCancellation, RoutedToolBridge,
    RunnerRequestContextSource,
};
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderTokenUsage, merge_provider_token_usage,
};
use mahayana_host_runtime::runner::box_tool_access::{
    BoxShellAutoReviewCallback, RunnerBoxResourcePort, RunnerBoxShellRequest,
};
use mahayana_host_runtime::cloud_agents::cloud_agent_tool::{CloudAgentReviewHook, CloudAgentToolDependencies};
use mahayana_host_runtime::runner::background_work::{
    BackgroundShellWatchOptions, BackgroundShellWatchOutcome,
    CloudAgentWatchOptions, RunnerBackgroundShellWatches, RunnerCloudAgentWatches,
    shell_rewatch_poll_ms,
};
use mahayana_host_runtime::runner::shell_terminal_watch::{
    ShellWatchStatus, poll_shell_terminal_file,
};
use mahayana_host_runtime::runner::coordinator_tool_relay::{
    CoordinatorToolRelay, ROUTED_TOOL_EXECUTE_METHOD, ROUTED_TOOL_LIST_METHOD,
    RUNNER_RESOLVE_ROUTED_TOOL_GATEWAY_METHOD,
};
use mahayana_host_runtime::extensions::mcp::coordinator_relay::{
    BoxServerStatusLoader, CoordinatorMcpLifecycleRelay, CoordinatorMcpManagerBackend,
    MCP_RESOLVE_LIFECYCLE_GATEWAY_METHOD,
};
use mahayana_host_runtime::extensions::mcp::mcp_service::McpHostService;
use mahayana_host_runtime::extensions::mcp::production_box_state::{
    MCP_STATE_EXEC_FIELD_NUMBER, MCP_TOOL_EXEC_FIELD_NUMBER, ProductionBoxMcpStateLoader,
    execute_box_mcp_raw,
};
use mahayana_host_runtime::runner::tools::mcp_host_service_management_sink::McpHostServiceManagementSink;
use mahayana_host_runtime::runner::tools::sand_mcp_management_tools::ConnectorCardVariant;
use mahayana_host_runtime::runner::sand_agent_runner::SandAgentRunner;
use mahayana_host_runtime::runner::subagent_runtime::{
    RunOutcome as GeneratedSubagentRunOutcome, SubagentLineage as GeneratedSubagentLineage,
    SubagentRuntime,
};
use mahayana_host_runtime::attachment_paths::{
    AgentMediaKind, OutgoingMediaCustody, file_url_for_path, persist_agent_media_bytes,
};
use mahayana_host_runtime::runner::tools::send_message_encoding::{
    image_mime_from_path, resolve_box_media_attachment,
};
use mahayana_host_runtime::runner::tools::box_help_tool::{BoxHelpOutcome, BoxHelpRequest};
use mahayana_host_runtime::runner::tools::send_message_tool::{
    CountingSendMessageSink, ResolvedAttachmentSource, SendMessageDeliveryCounter,
    SendMessageSink, file_path_from_file_url,
};
use mahayana_host_runtime::runner::tools::turn_toolset::{
    TurnLocalToolPermissionBinding, TurnToolsetRole,
};
use mahayana_host_runtime::selected_image_inputs::read_image_file_dimensions;
use mahayana_host_runtime::runner::tools::sand_reaction_tool::{
    CountingReactionSink, ReactionDeliveryCounter, ReactionSink,
};
use mahayana_host_runtime::runner::tools::sand_agent_management_tools::{
    AgentManagementRecord, AgentManagementSink,
};
use mahayana_host_runtime::runner::tools::sand_task_subagent_tool::{
    SubagentLaunchRecord, SubagentTaskReviewCallback, SubagentTaskSink,
};
use mahayana_host_runtime::runner::tools::sand_subagent_management_tools::{
    SteerReview, SubagentSteerReviewCallback,
};
use mahayana_host_runtime::gateway_config::{gateway_scheme, resolve_gateway_server_config};
use mahayana_host_runtime::device_pairing_owner::DevicePairingOwner;
use mahayana_host_runtime::gateway_protocol::is_openclaw_gateway_command;
use mahayana_host_runtime::gateway_server::{
    GatewayApi, GatewayCommandContext, GatewayCommandError, GatewayCommandReport,
    GatewayEventHub, GatewayHealth, GatewayServerDeps, start_gateway_server,
};
use mahayana_host_runtime::host_gateway_api::{
    CreateAgentNonceLedger, HOST_CAPABILITIES, sanitize_create_agent_args,
};
use mahayana_host_runtime::extensions::cloud_workers::extension::{
    CloudWorkerProvisioningExtension, CloudWorkerStatusSnapshot,
};
use mahayana_host_runtime::extensions::cloud_workers::CloudWorkerDoctorRequest;
use mahayana_host_runtime::extensions::plugins::capability_catalog::{
    PluginCapabilityCatalog, CAPABILITY_FAMILY_WORKER_PROVIDERS,
};
use mahayana_host_runtime::sand_host::{
    BOX_READY_REPORT_ATTEMPTS, BOX_READY_REPORT_RETRY_MS, BOX_READY_STAGE_MARKER_PATH,
    box_ready_duration_ms, compute_host_health, should_report_box_ready,
};
use mahayana_host_runtime::host_discovery::{
    GatewayDiscoveryInfo, clear_gateway_discovery, write_gateway_discovery,
};
use mahayana_host_runtime::host_lock::acquire_host_lock;
use mahayana_host_runtime::host_initial_transcript_load::{
    InitialTranscriptDegradedReason, ensure_initial_transcript_loaded,
    load_initial_transcript_resiliently,
};
use mahayana_host_runtime::host_paths::{
    get_gateway_discovery_path, get_host_lock_path, to_model_visible_path,
};
use mahayana_host_runtime::r#box::box_env::BoxEnvironmentUpdate;
use mahayana_host_runtime::r#box::exec_daemon_process::start_managed_box_exec_daemon_from_process_env;
use mahayana_host_runtime::r#box::production::{
    BOX_APPLY_ENVIRONMENT_GATEWAY_METHOD, ProductionBoxEnvironment,
};
use mahayana_unified_app_host::{
    PlatformRequestHost, UnifiedAppHost, default_unified_app_data_dir, dispatch_json,
    is_platform_request_json,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock, Weak, mpsc,
    atomic::{AtomicBool, Ordering},
};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SHUTDOWN_WATCHDOG_MS: u64 = 5_000;

fn teach_recording_status_value(status: TeachStatus) -> serde_json::Value {
    serde_json::json!({
        "state": status.state,
        "agentId": status.agent_id,
        "startedAtMs": status.started_at_ms,
        "maxDurationMs": status.max_duration_ms,
    })
}


fn ensure_managed_runtime_layout(app_data_dir: &Path) -> io::Result<()> {
    // The desktop product owns this fallback workspace.  It must exist before
    // the native engine canonicalizes the path while opening the first Agent
    // session.  User-selected workspace paths are validated elsewhere and are
    // never created implicitly.
    fs::create_dir_all(app_data_dir.join("feature-host/runtime/workspace"))
}

fn install_shutdown_signal_worker(
    host_tx: mpsc::Sender<HostLaneRequest>,
    shutdown_complete: Arc<AtomicBool>,
) -> io::Result<thread::JoinHandle<()>> {
    let mut signals = Signals::new([SIGTERM, SIGINT])?;
    Ok(thread::spawn(move || {
        let Some(signal) = signals.forever().next() else {
            return;
        };
        let label = if signal == SIGTERM { "SIGTERM" } else { "SIGINT" };
        eprintln!("[sand-host] received {label}, shutting down");
        let _ = host_tx.send(HostLaneRequest::StdinClosed);

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(SHUTDOWN_WATCHDOG_MS));
            if !shutdown_complete.load(Ordering::Acquire) {
                eprintln!("[sand-host] shutdown watchdog expired");
                std::process::exit(1);
            }
        });
    }))
}


pub(crate) enum HostLaneRequest {
    Stdin(String),
    Gateway {
        method: String,
        args: serde_json::Value,
        reply: mpsc::SyncSender<Result<serde_json::Value, GatewayCommandError>>,
    },
    StdinClosed,
}

const RUNNER_ACCEPT_ROUTED_PROMPT_GATEWAY_METHOD: &str = "runner.acceptRoutedPrompt";
pub(crate) const RUNNER_START_ROUTED_PROVIDER_GATEWAY_METHOD: &str = "runner.startRoutedProvider";
const RUNNER_CANCEL_ROUTED_PROVIDER_GATEWAY_METHOD: &str = "runner.cancelRoutedProvider";
pub(crate) const RUNNER_INFERENCE_EVENT_CHANNEL: &str = "runner-inference";

pub(crate) fn group_room_entry_observer(events: GatewayEventHub) -> GroupRoomEntryObserver {
    Arc::new(move |room_id, entry| {
        events.publish(serde_json::json!({
            "channel": "transcript",
            "payload": {
                "type": "appended",
                "agentId": room_id,
                "entry": entry,
            }
        }));
    })
}


fn persist_runner_media_bytes(
    sessions: &ProductionSessionWorkers,
    agent_id: &str,
    source_name: &str,
    bytes: &[u8],
    kind: AgentMediaKind,
) -> Option<String> {
    sessions
        .with_agent_admitted_filesystem_mutation(agent_id, || {
            let db_path = sessions.session_db_path(agent_id)?;
            let agent_dir = db_path
                .parent()
                .ok_or_else(|| "agent database has no parent directory".to_string())?;
            let path = persist_agent_media_bytes(agent_dir, source_name, bytes, kind)
                .map_err(|error| error.to_string())?;
            file_url_for_path(path)
                .ok_or_else(|| "persisted runner media path is not a valid file URL".to_string())
        })
        .ok()
}

fn resolve_production_attachment_source(
    sessions: &ProductionSessionWorkers,
    forever_box: &ForeverBoxService,
    agent_id: &str,
    source_url: &str,
) -> (ResolvedAttachmentSource, Option<std::path::PathBuf>) {
    let Some(source_path) = file_path_from_file_url(source_url) else {
        return (ResolvedAttachmentSource {
            url: source_url.to_string(),
            file_name: None,
        }, None);
    };
    let file_name = source_path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty());
    let source_path_text = source_path.to_string_lossy().into_owned();
    if let Ok(bytes) = fs::read(&source_path) {
        let kind = if image_mime_from_path(&source_path_text).is_some() {
            AgentMediaKind::Image
        } else {
            AgentMediaKind::Attachment
        };
        if let Some(url) = persist_runner_media_bytes(
            sessions,
            agent_id,
            &source_path_text,
            &bytes,
            kind,
        ) {
            return (
                ResolvedAttachmentSource { url: url.clone(), file_name },
                file_path_from_file_url(&url),
            );
        }
    }
    let status = forever_box.get_status(agent_id);
    let remote_box_has_desktop = status.vnc_url.is_some()
        || status.windows.as_ref().is_some_and(|windows| !windows.is_empty());
    let resolved = resolve_box_media_attachment(
        &source_path_text,
        remote_box_has_desktop,
        |path| forever_box.box_().download_file(agent_id, path).ok(),
        |bytes, _mime| persist_runner_media_bytes(
            sessions,
            agent_id,
            &source_path_text,
            bytes,
            AgentMediaKind::Image,
        ),
        |name, bytes| persist_runner_media_bytes(
            sessions,
            agent_id,
            name,
            bytes,
            AgentMediaKind::Attachment,
        ),
    )
    .unwrap_or_else(|| source_url.to_string());
    let owned_path = (resolved != source_url)
        .then(|| file_path_from_file_url(&resolved))
        .flatten();
    (ResolvedAttachmentSource { url: resolved, file_name }, owned_path)
}

struct ProductionSendMessageSink {
    host_tx: mpsc::Sender<HostLaneRequest>,
    cloud_agents: Arc<SandCloudAgentManager>,
    sessions: Arc<ProductionSessionWorkers>,
    forever_box: Arc<ForeverBoxService>,
    transcript_manager: Arc<TranscriptManager>,
    events: GatewayEventHub,
    ack_obligations: Arc<AckObligations>,
    transcript_runtime: Arc<ProductionTranscriptRuntime>,
    ack_token: Option<String>,
    agent_id: String,
    session_key: Option<String>,
    reply_thread_target: Option<String>,
    is_fork: bool,
    pending_media: Mutex<HashMap<String, PendingOutgoingMedia>>,
}

struct PendingOutgoingMedia {
    custody: OutgoingMediaCustody,
    session_key: Option<String>,
    lifecycle_revision: Option<String>,
    media_work_id: Option<String>,
}

impl ProductionSendMessageSink {

    fn fulfill_ack_obligation(&self) {
        if let Some(ack_token) = self.ack_token.as_deref() {
            if let Err(error) = self
                .ack_obligations
                .fulfill_ack_obligation(&self.agent_id, ack_token)
            {
                eprintln!(
                    "mahayana-host-ack fulfill_failed agent={} error={error}",
                    self.agent_id
                );
            }
        }
    }

    fn assert_pending_media_current(&self, tool_call_id: &str) -> Result<(), ProviderSessionError> {
        let fence = self
            .pending_media
            .lock()
            .map_err(|_| ProviderSessionError::Tool("outgoing media custody lock poisoned".into()))?
            .get(tool_call_id)
            .and_then(|pending| {
                Some((
                    pending.session_key.as_deref()?,
                    pending.lifecycle_revision.as_deref()?,
                ))
            })
            .map(|(session_key, lifecycle_revision)| {
                (session_key.to_string(), lifecycle_revision.to_string())
            });
        let Some((session_key, lifecycle_revision)) = fence else {
            return Ok(());
        };
        self.sessions
            .assert_session_work_admission_generation(
                &self.agent_id,
                &session_key,
                &lifecycle_revision,
            )
            .map_err(ProviderSessionError::Tool)
    }
}

impl SendMessageSink for ProductionSendMessageSink {
    fn is_awaiting_user_selection(&self) -> bool {
        self.sessions
            .get_agent_awaiting_user_response(&self.agent_id)
            .ok()
            .flatten()
            .is_some()
    }

    fn resolve_cloud_agent_title(&self, bc_id: &str) -> Option<String> {
        self.cloud_agents
            .get(bc_id)
            .ok()
            .flatten()
            .map(|detail| detail.summary.name.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn request_box_help(
        &self,
        request: BoxHelpRequest,
        timestamp_ms: u64,
        tool_call_id: &str,
    ) -> Result<BoxHelpOutcome, ProviderSessionError> {
        let outcome = self
            .transcript_manager
            .start_box_handoff(HandoffRequest {
                agent_id: self.agent_id.clone(),
                instruction: request.instruction.clone(),
                telemetry: HandoffTelemetry {
                    reason: request.reason,
                    domain: request.domain,
                    idp_domain: request.idp_domain,
                },
            })
            .map_err(ProviderSessionError::Tool)?;
        match outcome {
            HandoffStartResult::AlreadyPending { request_id, instruction } => {
                Ok(BoxHelpOutcome::AlreadyPending { request_id, instruction })
            }
            HandoffStartResult::Started { request_id } => {
                let mut entry = serde_json::json!({
                    "id": format!("runner-box-help:{tool_call_id}"),
                    "kind": "send-message",
                    "message": {
                        "type": "text",
                        "content": request.instruction.clone(),
                    },
                    "timestampMs": timestamp_ms,
                    "boxRequestId": request_id.clone(),
                    "boxInstruction": request.instruction,
                });
                if let Some(session_key) = self.session_key.as_deref() {
                    entry["sessionKey"] = serde_json::Value::String(session_key.to_string());
                }
                let tracking = self
                    .transcript_runtime
                    .track_box_request_entry(&self.agent_id, &entry);
                if let Some(superseded_request_id) = tracking.superseded_request_id.as_deref() {
                    if let Err(error) = resolve_box_request_entry(
                        &self.sessions,
                        &self.agent_id,
                        superseded_request_id,
                        "dismissed",
                    ) {
                        eprintln!(
                            "mahayana-host box_request_supersede_failed agent={} request={} error={error}",
                            self.agent_id,
                            superseded_request_id,
                        );
                    }
                }
                let append_result = if let Some(session_key) = self.session_key.as_deref() {
                    self.sessions.append_agent_transcript_entries_if_session_admitted(
                        &self.agent_id,
                        session_key,
                        &[entry],
                    )
                } else {
                    self.sessions
                        .append_agent_transcript_entries(&self.agent_id, &[entry])
                };
                if let Err(error) = append_result {
                    let _ = self
                        .transcript_runtime
                        .resolve_box_request_tracking(&request_id);
                    let _ = self
                        .transcript_manager
                        .forget_box_handoff(&self.agent_id);
                    return Err(ProviderSessionError::Tool(format!(
                        "could not persist request_box_help for {}: {error}",
                        self.agent_id
                    )));
                }
                self.fulfill_ack_obligation();
                self.transcript_runtime.track_runner_activity_update(
                    &self.agent_id,
                    &ActivityUpdate::SendMessage,
                    started_at_ms(),
                );
                Ok(BoxHelpOutcome::Started { request_id })
            }
        }
    }

    fn resolve_attachment_source(
        &self,
        source_url: &str,
        tool_call_id: &str,
    ) -> Result<ResolvedAttachmentSource, ProviderSessionError> {
        let admission = if let Some(session_key) = self.session_key.as_deref() {
            self.sessions
                .assert_session_work_admission(&self.agent_id, Some(session_key))
                .map_err(ProviderSessionError::Tool)?;
            self.sessions
                .read_session_work_admission(&self.agent_id)
                .map_err(ProviderSessionError::Tool)?
                .filter(|record| record.session_key == session_key)
                .map(|record| (record.session_key, record.lifecycle_revision))
        } else {
            None
        };
        let admission_session_key = admission.as_ref().map(|(key, _)| key.clone());
        let admission_lifecycle_revision = admission.map(|(_, revision)| revision);
        let (resolved, owned_path) = resolve_production_attachment_source(
            self.sessions.as_ref(),
            self.forever_box.as_ref(),
            &self.agent_id,
            source_url,
        );
        if let Some(path) = owned_path {
            let mut custody = OutgoingMediaCustody::default();
            custody.track(path.clone());
            let media_work_id = format!("send-media:{tool_call_id}");
            let media_lifecycle_revision = if let Some(session_key) = admission_session_key.as_deref() {
                Some(self
                    .sessions
                    .capture_session_media_admission(
                        &self.agent_id,
                        session_key,
                        &media_work_id,
                        Some(&path),
                    )
                    .map_err(|error| {
                        for cleanup_error in custody.rollback() {
                            eprintln!(
                                "mahayana-host outgoing_media_cleanup_failed agent={} tool={} error={cleanup_error}",
                                self.agent_id, tool_call_id
                            );
                        }
                        ProviderSessionError::Tool(error)
                    })?)
            } else {
                None
            };
            let mut pending = match self.pending_media.lock() {
                Ok(pending) => pending,
                Err(_) => {
                    for error in custody.rollback() {
                        eprintln!(
                            "mahayana-host outgoing_media_cleanup_failed agent={} tool={} error={error}",
                            self.agent_id, tool_call_id
                        );
                    }
                    if let Some(lifecycle_revision) = media_lifecycle_revision.as_deref() {
                        let _ = self.sessions.release_session_media_admission(
                            &self.agent_id,
                            lifecycle_revision,
                            &media_work_id,
                        );
                    }
                    return Err(ProviderSessionError::Tool(
                        "outgoing media custody lock poisoned".into(),
                    ));
                }
            };
            let pending_media = pending
                .entry(tool_call_id.to_string())
                .or_insert_with(|| PendingOutgoingMedia {
                    custody: OutgoingMediaCustody::default(),
                    session_key: admission_session_key.clone(),
                    lifecycle_revision: media_lifecycle_revision
                        .clone()
                        .or(admission_lifecycle_revision.clone()),
                    media_work_id: media_lifecycle_revision
                        .as_ref()
                        .map(|_| media_work_id.clone()),
                });
            pending_media.custody.absorb(custody);
        }
        Ok(resolved)
    }

    fn abort_attachment_custody(&self, tool_call_id: &str) {
        let mut pending = match self.pending_media.lock() {
            Ok(pending) => pending,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(mut custody) = pending.remove(tool_call_id) else { return };
        for error in custody.custody.rollback() {
            eprintln!(
                "mahayana-host outgoing_media_cleanup_failed agent={} tool_call={} error={error}",
                self.agent_id, tool_call_id
            );
        }
        if let (Some(revision), Some(work_id)) = (
            custody.lifecycle_revision.as_deref(),
            custody.media_work_id.as_deref(),
        ) {
            if let Err(error) = self.sessions.release_session_media_admission(
                &self.agent_id,
                revision,
                work_id,
            ) {
                eprintln!(
                    "mahayana-host outgoing_media_admission_release_failed agent={} tool_call={} error={error}",
                    self.agent_id, tool_call_id
                );
            }
        }
    }

    fn read_media_dimensions(&self, resolved_url: &str) -> Option<(u32, u32)> {
        let path = file_path_from_file_url(resolved_url)?;
        let path = path.to_str()?;
        let dimensions = read_image_file_dimensions(path)?;
        Some((dimensions.width, dimensions.height))
    }

    fn send_message(
        &self,
        message: serde_json::Value,
        timestamp_ms: u64,
        tool_call_id: &str,
    ) -> Result<Option<String>, ProviderSessionError> {
        let channel_delivery = message
            .get("channel")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);

        if let Err(error) = self.assert_pending_media_current(tool_call_id) {
            self.abort_attachment_custody(tool_call_id);
            return Err(error);
        }
        let media_fence = self
            .pending_media
            .lock()
            .map_err(|_| ProviderSessionError::Tool("outgoing media custody lock poisoned".into()))?
            .get(tool_call_id)
            .and_then(|pending| {
                Some((
                    pending.lifecycle_revision.as_ref()?.clone(),
                    pending.media_work_id.as_ref()?.clone(),
                ))
            });
        let entry = match (media_fence, self.session_key.as_deref()) {
            (Some((revision, work_id)), Some(session_key)) => self
                .transcript_runtime
                .append_generated_send_message_entry_with_media_custody(
                    self.sessions.as_ref(),
                    &self.agent_id,
                    session_key,
                    &message,
                    timestamp_ms,
                    self.reply_thread_target.as_deref(),
                    self.is_fork,
                    &revision,
                    &work_id,
                ),
            (None, Some(session_key)) => self
                .transcript_runtime
                .append_generated_send_message_entry(
                    self.sessions.as_ref(),
                    &self.agent_id,
                    session_key,
                    &message,
                    timestamp_ms,
                    self.reply_thread_target.as_deref(),
                    self.is_fork,
                ),
            (None, None) => self
                .transcript_runtime
                .append_generated_send_message_entry_compatibility(
                    self.sessions.as_ref(),
                    &self.agent_id,
                    &message,
                    timestamp_ms,
                    self.reply_thread_target.as_deref(),
                    self.is_fork,
                ),
            (Some((_revision, _work_id)), None) => Err(
                "outgoing media fence is missing its admitted session identity".into(),
            ),
        }
            .map_err(|error| {
                self.abort_attachment_custody(tool_call_id);
                ProviderSessionError::Tool(format!(
                    "could not persist SendMessage for {}: {error}",
                    self.agent_id
                ))
            })?;
        let entry_id = entry
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                self.abort_attachment_custody(tool_call_id);
                ProviderSessionError::Tool(
                    "persisted SendMessage entry did not contain an id".to_string(),
                )
            })?;
        let mut pending = match self.pending_media.lock() {
            Ok(pending) => pending,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(mut custody) = pending.remove(tool_call_id) {
            custody.custody.commit();
        }
        if let Some(address_token) = channel_delivery {
            let host_tx = self.host_tx.clone();
            let agent_id = self.agent_id.clone();
            let session_key = self.session_key.clone();
            let mut outbound_message = message.clone();
            if let Some(object) = outbound_message.as_object_mut() {
                object.insert(
                    "deliveryOperationId".to_string(),
                    serde_json::Value::String(entry_id.clone()),
                );
            }
            if let Err(error) = thread::Builder::new()
                .name(format!("mahayana-channel-send-{agent_id}"))
                .spawn(move || {
                    if let Err(error) = call_host_lane(
                        &host_tx,
                        "deliverToChannel",
                        serde_json::json!({
                            "agentId": agent_id,
                            "addressToken": address_token,
                            "sessionKey": session_key,
                            "message": outbound_message,
                        }),
                    ) {
                        eprintln!("mahayana-host channel_delivery_dispatch_failed error={error}");
                    }
                })
            {
                eprintln!(
                    "mahayana-host channel_delivery_dispatch_spawn_failed agent={} error={error}",
                    self.agent_id
                );
            }
        }
        self.events.publish(serde_json::json!({
            "channel": "transcript",
            "payload": {
                "type": "appended",
                "agentId": self.agent_id,
                "entry": entry,
            }
        }));
        self.fulfill_ack_obligation();
        self.transcript_runtime.track_runner_activity_update(
            &self.agent_id,
            &ActivityUpdate::SendMessage,
            started_at_ms(),
        );
        Ok(Some(entry_id))
    }
}

fn reaction_gateway_args(
    agent_id: &str,
    message_address: &str,
    emoji: &str,
) -> serde_json::Value {
    serde_json::json!({
        "agentId": agent_id,
        "entryId": message_address,
        "emoji": emoji,
    })
}

struct ProductionAgentManagementSink {
    sessions: Arc<ProductionSessionWorkers>,
    forever_box: Arc<ForeverBoxService>,
    messaging: Arc<ProductionAgentToAgentMessaging>,
    agent_messages: Arc<Mutex<Vec<String>>>,
    agent_id: String,
    session_key: Option<String>,
    reply_to_id: Option<String>,
}

impl AgentManagementSink for ProductionAgentManagementSink {
    fn self_agent_id(&self) -> &str {
        &self.agent_id
    }

    fn resolve_image_source(
        &self,
        image: &AgentMessageImage,
        _tool_call_id: &str,
    ) -> Result<AgentMessageImage, ProviderSessionError> {
        let (resolved, _) = resolve_production_attachment_source(
            self.sessions.as_ref(),
            self.forever_box.as_ref(),
            &self.agent_id,
            &image.url,
        );
        Ok(AgentMessageImage {
            url: resolved.url,
            alt: image.alt.clone(),
        })
    }

    fn send_to_agent(
        &self,
        target_id: &str,
        message: &str,
        images: &[AgentMessageImage],
        priority: bool,
    ) -> Result<String, ProviderSessionError> {
        let entry_id = self
            .messaging
            .send_to_agent_with_session(
                &self.agent_id,
                self.session_key.as_deref(),
                target_id,
                message,
                images,
                priority,
            )
            .map_err(ProviderSessionError::Tool)?;
        if !message.trim().is_empty() {
            if let Ok(mut agent_messages) = self.agent_messages.lock() {
                agent_messages.push(message.to_string());
            }
        }
        Ok(entry_id)
    }

    fn send_to_agent_with_reply(
        &self,
        target_id: &str,
        message: &str,
        images: &[AgentMessageImage],
        priority: bool,
        reply_to_id: Option<&str>,
    ) -> Result<String, ProviderSessionError> {
        let entry_id = self
            .messaging
            .send_to_agent_with_session_and_reply(
                &self.agent_id,
                self.session_key.as_deref(),
                target_id,
                message,
                images,
                priority,
                reply_to_id,
            )
            .map_err(ProviderSessionError::Tool)?;
        if !message.trim().is_empty() {
            if let Ok(mut agent_messages) = self.agent_messages.lock() {
                agent_messages.push(message.to_string());
            }
        }
        Ok(entry_id)
    }

    fn current_reply_to_id(&self) -> Option<&str> {
        self.reply_to_id.as_deref()
    }

    fn create_agent(
        &self,
        name: &str,
        description: &str,
    ) -> Result<AgentManagementRecord, ProviderSessionError> {
        let profile = SandAgentProfile {
            name: name.trim().to_string(),
            description: description.trim().to_string(),
            title: String::new(),
            avatar_shape: String::new(),
            avatar_color: String::new(),
        };
        let record = self
            .sessions
            .materialize_new_session(Some(&profile), "user", None)
            .map_err(ProviderSessionError::Tool)?;
        Ok(AgentManagementRecord {
            id: record.id,
            name: record.profile.name,
        })
    }

    fn update_agent(
        &self,
        agent_id: &str,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<Option<AgentManagementRecord>, ProviderSessionError> {
        let Some(current) = self
            .sessions
            .get_agent_profile_text(agent_id)
            .map_err(ProviderSessionError::Tool)?
        else {
            return Ok(None);
        };
        let update = AgentProfileUpdate {
            name: name.unwrap_or(&current.name).to_string(),
            description: description.unwrap_or(&current.description).to_string(),
            title: None,
            avatar_shape: None,
            avatar_color: None,
        };
        let updated = self
            .sessions
            .update_agent_profile(agent_id, &update, None)
            .map_err(ProviderSessionError::Tool)?;
        Ok(updated.map(|summary| AgentManagementRecord {
            id: summary.id,
            name: summary.name,
        }))
    }
}

struct ProductionReactionSink {
    host_tx: mpsc::Sender<HostLaneRequest>,
    agent_id: String,
}

impl ReactionSink for ProductionReactionSink {
    fn react(
        &self,
        message_address: &str,
        emoji: &str,
    ) -> Result<(), ProviderSessionError> {
        call_host_lane(
            &self.host_tx,
            "reactToMessage",
            reaction_gateway_args(&self.agent_id, message_address, emoji),
        )
        .map_err(|error| {
            ProviderSessionError::Tool(format!(
                "could not react to {message_address}: {error}"
            ))
        })?;
        Ok(())
    }
}

struct RoutedTurnLeaseGuard {
    runtime: Arc<ProductionTranscriptRuntime>,
    agent_id: String,
    stream_id: String,
    events: GatewayEventHub,
    settled: bool,
}

impl RoutedTurnLeaseGuard {
    fn new(
        runtime: Arc<ProductionTranscriptRuntime>,
        agent_id: String,
        stream_id: String,
        events: GatewayEventHub,
    ) -> Self {
        Self {
            runtime,
            agent_id,
            stream_id,
            events,
            settled: false,
        }
    }

    fn settle(&mut self) {
        if self.settled {
            return;
        }
        self.settled = true;
        if let Ok(Some(settlement)) = self.runtime.settle_routed_turn(
            &self.agent_id,
            &self.stream_id,
            started_at_ms(),
        ) {
            if let Some(watchdog) = settlement.watchdog {
                self.events.publish(serde_json::json!({
                    "channel": "run-queue-watchdog",
                    "payload": {
                        "agentId": watchdog.agent_id,
                        "stage": watchdog.stage.as_str(),
                        "activeLane": watchdog.active_lane.as_str(),
                        "activeSource": watchdog.active_source,
                        "activeRuntimeMs": watchdog.active_runtime_ms,
                        "waitingUserAgeMs": watchdog.waiting_user_age_ms,
                        "ackToken": watchdog.ack_token,
                        "interrupted": false
                    }
                }));
            }
            if let Some(next) = settlement.next {
                self.events.publish(serde_json::json!({
                    "channel": "run-queue-dequeued",
                    "payload": {
                        "agentId": next.agent_id,
                        "lane": next.lane.as_str(),
                        "source": next.source,
                        "queueWaitMs": next.queue_wait_ms,
                        "acceptedToRunMs": next.accepted_to_run_ms,
                        "jumpedBackground": next.jumped_background,
                        "depthUser": next.depth_user,
                        "depthAgent": next.depth_agent,
                        "depthBackground": next.depth_background
                    }
                }));
            }
        }
    }
}

impl Drop for RoutedTurnLeaseGuard {
    fn drop(&mut self) {
        self.settle();
    }
}

pub(crate) struct UnifiedGatewayApi {
    pub(crate) host_tx: mpsc::Sender<HostLaneRequest>,
    pub(crate) auth: Arc<HostAuthExtension>,
    pub(crate) experiments: Arc<HostExperimentsExtension>,
    pub(crate) settings: Arc<SettingsService>,
    pub(crate) inference: Arc<ProductionInferenceExtension>,
    pub(crate) content_search: Arc<ProductionContentSearchExtension>,
    pub(crate) events: GatewayEventHub,
    pub(crate) routed_tool_relay: Arc<CoordinatorToolRelay>,
    pub(crate) mcp_lifecycle_relay: Arc<CoordinatorMcpLifecycleRelay>,
    pub(crate) mcp_service: Arc<McpHostService>,
    pub(crate) data_dir: PathBuf,
    pub(crate) request_context: Arc<dyn RunnerRequestContextSource>,
    pub(crate) session_workers: Arc<ProductionSessionWorkers>,
    pub(crate) runner_registry: Arc<TranscriptRunnerRegistry>,
    pub(crate) ack_obligations: Arc<AckObligations>,
    pub(crate) agent_deletion_runtime: AgentDeletionRuntimeDeps,
    pub(crate) forever_box: Arc<ForeverBoxService>,
    pub(crate) teach_recording: Arc<Mutex<Option<TeachRecordingApi>>>,
    pub(crate) local_exec: Arc<HostLocalExecExtension>,
    pub(crate) session_handoff: BoxHandoffService,
    pub(crate) webauthn_proxy: Arc<HostWebAuthnProxyExtension>,
    pub(crate) trays: Arc<HostTraysExtension>,
    pub(crate) transcript_runtime: Arc<ProductionTranscriptRuntime>,
    pub(crate) generated_agent_runtime: Arc<Mutex<SubagentRuntime>>,
    pub(crate) completion_revivals: Arc<CompletionRevivals>,
    pub(crate) transcript_manager: Arc<TranscriptManager>,
    pub(crate) roster_emit: Arc<ProductionRosterEmit>,
    pub(crate) background_wakes: Arc<Mutex<BackgroundWakes<serde_json::Value>>>,
    pub(crate) channel_delivery: HostChannelDelivery,
    pub(crate) telemetry_logs: HostStructuredLogTelemetry,
    pub(crate) product_analytics: HostProductAnalytics,
    pub(crate) telemetry_api: HostTelemetryApi,
    pub(crate) production_action_auditor: ActionAuditExtension,
    pub(crate) cloud_agents: Arc<SandCloudAgentManager>,
    pub(crate) cloud_workers: CloudWorkerProvisioningExtension,
    pub(crate) plugin_capability_catalog: Arc<PluginCapabilityCatalog>,
    pub(crate) cloud_agent_watches: Arc<RunnerCloudAgentWatches>,
    pub(crate) background_shell_watches: Arc<RunnerBackgroundShellWatches>,
    pub(crate) secrets: Arc<HostSecretsExtension>,
    pub(crate) local_tool_permission: Arc<HostLocalToolPermissionExtension>,
    pub(crate) auto_review: Arc<HostAutoReviewExtension>,
    pub(crate) host_runner_composition: Arc<HostRunnerComposition>,
    pub(crate) box_store_sync: ProductionBoxStoreSyncApi,
    pub(crate) roster_bookkeeping: Arc<Mutex<HostRosterBookkeeping>>,
    pub(crate) cross_user: Arc<ProductionCrossUserRuntime>,
    pub(crate) host_upgrade: Arc<ProductionHostUpgradeExtension>,
    pub(crate) automations_lifecycle: Arc<Mutex<Weak<ProductionAutomationsLifecycle>>>,
    pub(crate) create_agent_nonces: Mutex<CreateAgentNonceLedger<serde_json::Value>>,
    pub(crate) last_busy_at_ms: Mutex<u64>,
    pub(crate) device_pairing: Arc<DevicePairingOwner>,
    pub(crate) node_host_runtime: Arc<NodeHostRuntimeState>,
    pub(crate) node_host_local_session: NodeHostSessionLease,
    pub(crate) generated_subagent_cleanup: GeneratedSubagentCleanupSlot,
}

fn publish_cloud_worker_status(
    events: &GatewayEventHub,
    projection: CloudWorkerStatusSnapshot,
) {
    events.publish(serde_json::json!({
        "channel": "cloud-workers.status",
        "payload": {
            "kind": "status-changed",
            "projection": projection
        }
    }));
}

pub(crate) type GeneratedSubagentCleanup = Arc<
    dyn Fn(&str, &str) -> Result<(), String> + Send + Sync + 'static,
>;
pub(crate) type GeneratedSubagentCleanupSlot = Arc<Mutex<Option<GeneratedSubagentCleanup>>>;

#[derive(Clone)]
pub(crate) struct LocalRoutedRunnerDeps {
    pub(crate) routed_tool_relay: Arc<CoordinatorToolRelay>,
    pub(crate) mcp_service: Arc<McpHostService>,
    pub(crate) auth: Arc<HostAuthExtension>,
    pub(crate) auto_review: Arc<HostAutoReviewExtension>,
    pub(crate) events: GatewayEventHub,
    pub(crate) host_tx: mpsc::Sender<HostLaneRequest>,
    pub(crate) data_dir: PathBuf,
    pub(crate) request_context: Arc<dyn RunnerRequestContextSource>,
    pub(crate) experiments: Arc<HostExperimentsExtension>,
    pub(crate) settings: Arc<SettingsService>,
    pub(crate) inference: Arc<ProductionInferenceExtension>,
    pub(crate) session_workers: Arc<ProductionSessionWorkers>,
    pub(crate) runner_registry: Arc<TranscriptRunnerRegistry>,
    pub(crate) ack_obligations: Arc<AckObligations>,
    pub(crate) transcript_runtime: Arc<ProductionTranscriptRuntime>,
    pub(crate) transcript_manager: Arc<TranscriptManager>,
    pub(crate) generated_agent_runtime: Arc<Mutex<SubagentRuntime>>,
    pub(crate) completion_revivals: Arc<CompletionRevivals>,
    pub(crate) forever_box: Arc<ForeverBoxService>,
    pub(crate) local_exec: Arc<HostLocalExecExtension>,
    pub(crate) local_tool_permission: Arc<HostLocalToolPermissionExtension>,
    pub(crate) session_handoff: BoxHandoffService,
    pub(crate) trays: Arc<HostTraysExtension>,
    pub(crate) telemetry_logs: HostStructuredLogTelemetry,
    pub(crate) product_analytics: HostProductAnalytics,
    pub(crate) production_action_auditor: ActionAuditExtension,
    pub(crate) cloud_agents: Arc<SandCloudAgentManager>,
    pub(crate) cloud_agent_watches: Arc<RunnerCloudAgentWatches>,
    pub(crate) background_shell_watches: Arc<RunnerBackgroundShellWatches>,
    pub(crate) host_runner_composition: Arc<HostRunnerComposition>,
    pub(crate) box_store_sync: ProductionBoxStoreSyncApi,
    pub(crate) automations_lifecycle: Arc<Mutex<Weak<ProductionAutomationsLifecycle>>>,
    pub(crate) generated_subagent_cleanup: GeneratedSubagentCleanupSlot,
}

#[derive(Clone)]
struct ProductionCreatedAgentKickstartRuntime {
    deps: LocalRoutedRunnerDeps,
    roster: Arc<ProductionRosterEmit>,
}

impl CreatedAgentKickstartRuntimePort for ProductionCreatedAgentKickstartRuntime {
    fn is_run_ready(&self) -> bool {
        !self.deps.transcript_manager.is_quiescing_for_upgrade()
            && configured_routed_provider(&self.deps.data_dir.join("settings.json")).is_some()
    }

    fn can_execute(&self) -> bool {
        self.is_run_ready()
    }

    fn is_disallowed_session(&self, agent_id: &str) -> bool {
        self.deps
            .session_workers
            .summarize_agent_by_id(agent_id, None)
            .map(|summary| summary.is_none_or(|summary| summary.is_group))
            .unwrap_or(true)
    }

    fn is_run_in_flight(&self, agent_id: &str) -> bool {
        self.deps.transcript_runtime.is_agent_running(agent_id)
            || !self
                .deps
                .runner_registry
                .active_stream_ids_for_agent(agent_id)
                .is_empty()
    }

    fn run_hidden(
        &self,
        agent_id: &str,
        prompt: &str,
        source: &str,
    ) -> Result<KickstartTurnOutcome, KickstartRunError> {
        let Some(provider) =
            configured_routed_provider(&self.deps.data_dir.join("settings.json"))
        else {
            return Err(KickstartRunError {
                request_id: None,
                message: "no routed provider configured for created-agent kickstart".into(),
            });
        };
        run_local_kickstart_turn(self.deps.clone(), provider, agent_id, source, prompt)
    }

    fn mark_resume_pending(&self, agent_id: &str, source: &str) -> Result<(), String> {
        self.deps
            .transcript_manager
            .mark_upgrade_resume_pending(UpgradeResumeMarker {
                agent_id: agent_id.to_string(),
                marked_at_ms: started_at_ms() as f64,
                source: Some(source.to_string()),
                automation_id: None,
                automation_run_id: None,
            });
        Ok(())
    }

    fn report_failure(&self, agent_id: &str, error: &KickstartRunError) {
        let classified_error = ProviderSessionError::Tool(error.message.clone());
        let report = AgentErrorReport {
            source: "onboarding_kickstart".into(),
            conversation_id: agent_id.to_string(),
            request_id: error.request_id.clone(),
            error: classify_agent_error(&classified_error),
            detail: Some(sand_error_detail(&error.message)),
        };
        if let Err(telemetry_error) = self.deps.telemetry_logs.report_agent_error(&report) {
            eprintln!(
                "mahayana-host kickstart_agent_error_telemetry_failed agent={agent_id} error={telemetry_error}"
            );
        }
    }

    fn report_disk_saver_audit_failure(&self, agent_id: &str, error: &KickstartRunError) {
        let classified_error = ProviderSessionError::Tool(error.message.clone());
        let report = AgentErrorReport {
            source: "disk_saver_reaudit".into(),
            conversation_id: agent_id.to_string(),
            request_id: error.request_id.clone(),
            error: classify_agent_error(&classified_error),
            detail: Some(sand_error_detail(&error.message)),
        };
        if let Err(telemetry_error) = self.deps.telemetry_logs.report_agent_error(&report) {
            eprintln!(
                "mahayana-host disk_saver_reaudit_telemetry_failed agent={agent_id} error={telemetry_error}"
            );
        }
    }

    fn push_introduction_failure(&self, agent_id: &str, error: &KickstartRunError) {
        let mut tray =
            provider_failure_tray(agent_id, &error.message, started_at_ms() as i64);
        tray.title = INTRODUCTION_FAILED_TRAY_TITLE.into();
        tray.dedupe_key = Some(introduction_failed_tray_key(agent_id));
        self.deps.trays.push_error(tray);
    }

    fn emit_agent_update(&self, agent_id: &str) -> Result<(), String> {
        self.roster.emit_agent_update(agent_id)
    }
}

#[derive(Clone)]
struct ProductionSubagentTaskSink {
    deps: LocalRoutedRunnerDeps,
    parent_agent_id: String,
    provider: RoutedProvider,
    parent_stream_id: String,
    root_parent_request_id: String,
}

impl SubagentTaskSink for ProductionSubagentTaskSink {
    fn launch_subagent(
        &self,
        prompt: &str,
        subagent_type: &str,
        tool_call_id: &str,
    ) -> Result<SubagentLaunchRecord, ProviderSessionError> {
        let profile = SandAgentProfile {
            name: format!("{} subagent", subagent_type.trim()),
            description: prompt.trim().to_string(),
            title: String::new(),
            avatar_shape: String::new(),
            avatar_color: String::new(),
        };
        let child = self
            .deps
            .session_workers
            .materialize_new_session(Some(&profile), "subagent", Some(subagent_type))
            .map_err(ProviderSessionError::Tool)?;
        let child_id = child.id.clone();
        let child_lifecycle_revision = self
            .deps
            .session_workers
            .summarize_agent_by_id(&child_id, None)
            .map_err(ProviderSessionError::Tool)?
            .and_then(|summary| {
                (!summary.lifecycle_revision.trim().is_empty())
                    .then_some(summary.lifecycle_revision)
            })
            .ok_or_else(|| {
                ProviderSessionError::Tool(
                    "subagent session lifecycle revision unavailable after materialization".into(),
                )
            })?;
        let child_session_key = self
            .deps
            .session_workers
            .read_session_work_admission(&child_id)
            .map_err(ProviderSessionError::Tool)?
            .map(|record| record.session_key)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ProviderSessionError::Tool(
                    "subagent session admission identity unavailable after materialization".into(),
                )
            })?;
        let child_stream_id = uuid::Uuid::new_v4().to_string();
        let normalized_type = subagent_type
            .chars()
            .filter(|ch| !matches!(ch, '-' | '_' | ' '))
            .collect::<String>();
        let args = serde_json::json!({
            "provider": self.provider.as_str(),
            "agentId": child_id,
            "sessionKey": child_session_key,
            "spawnedBy": self.parent_agent_id,
            "lifecycleRevision": child_lifecycle_revision,
            "streamId": child_stream_id,
            "messages": [{"role":"user","content":prompt}],
            "requestSource": "subagent",
            "parentAgentId": self.parent_agent_id,
            "subagentType": subagent_type,
            "parentAgentToolCallId": tool_call_id,
            "lineage": {
                "parentRequestId": self.parent_stream_id,
                "rootParentRequestId": self.root_parent_request_id,
                "parentAgentToolCallId": tool_call_id,
            },
            "skipLabeling": true,
            "isComputerUseSubagent": normalized_type.eq_ignore_ascii_case("computeruse"),
            "isBrowserUseSubagent": normalized_type.eq_ignore_ascii_case("browseruse"),
        });
        start_routed_provider_task(
            Arc::clone(&self.deps.routed_tool_relay),
            Arc::clone(&self.deps.mcp_service),
            self.deps.events.clone(),
            self.deps.host_tx.clone(),
            self.deps.data_dir.clone(),
            Arc::clone(&self.deps.request_context),
            Arc::clone(&self.deps.auth),
            Arc::clone(&self.deps.auto_review),
            Arc::clone(&self.deps.experiments),
            Arc::clone(&self.deps.settings),
            Arc::clone(&self.deps.inference),
            Arc::clone(&self.deps.session_workers),
            Arc::clone(&self.deps.runner_registry),
            Arc::clone(&self.deps.ack_obligations),
            Arc::clone(&self.deps.transcript_runtime),
            Arc::clone(&self.deps.transcript_manager),
            Arc::clone(&self.deps.generated_agent_runtime),
            Arc::clone(&self.deps.completion_revivals),
            Arc::clone(&self.deps.forever_box),
            Arc::clone(&self.deps.local_exec),
            Arc::clone(&self.deps.local_tool_permission),
            self.deps.session_handoff.clone(),
            Arc::clone(&self.deps.trays),
            self.deps.telemetry_logs.clone(),
            self.deps.product_analytics.clone(),
            self.deps.production_action_auditor.clone(),
            Arc::clone(&self.deps.cloud_agents),
            Arc::clone(&self.deps.cloud_agent_watches),
            Arc::clone(&self.deps.background_shell_watches),
            Arc::clone(&self.deps.host_runner_composition),
            self.deps.box_store_sync.clone(),
            Arc::clone(&self.deps.automations_lifecycle),
            Arc::clone(&self.deps.generated_subagent_cleanup),
            None,
            args,
        )
        .map_err(|error| ProviderSessionError::Tool(format!("subagent launch failed: {error:?}")))?;
        Ok(SubagentLaunchRecord {
            id: child.id,
            subagent_type: subagent_type.to_string(),
        })
    }
}

fn publish_generated_subagents(
    events: &GatewayEventHub,
    runtime: &Arc<Mutex<SubagentRuntime>>,
    parent_agent_id: &str,
) {
    let subagents = runtime
        .lock()
        .map(|runtime| runtime.list_subagents())
        .unwrap_or_default()
        .into_iter()
        .map(|(id, record)| {
            let status = match record.status {
                mahayana_host_runtime::runner::subagent_runtime::SubagentStatus::Pending => "pending",
                mahayana_host_runtime::runner::subagent_runtime::SubagentStatus::Running => "running",
                mahayana_host_runtime::runner::subagent_runtime::SubagentStatus::Done => "done",
                mahayana_host_runtime::runner::subagent_runtime::SubagentStatus::Error => "error",
                mahayana_host_runtime::runner::subagent_runtime::SubagentStatus::Aborted => "aborted",
            };
            serde_json::json!({
                "subagentId": id,
                "subagentType": record.subagent_type,
                "title": record.title,
                "status": status,
            })
        })
        .collect::<Vec<_>>();
    events.publish(serde_json::json!({
        "channel": "subagents",
        "payload": {
            "parentAgentId": parent_agent_id,
            "subagents": subagents,
        }
    }));
}

#[derive(Clone)]
struct ProductionPendingWakeRuntime {
    gateway: Arc<Mutex<Weak<UnifiedGatewayApi>>>,
    cloud_agent_watches: Arc<RunnerCloudAgentWatches>,
    background_shell_watches: Arc<RunnerBackgroundShellWatches>,
    completion_revivals: Arc<CompletionRevivals>,
}

impl ProductionPendingWakeRuntime {
    fn gateway(&self) -> Option<Arc<UnifiedGatewayApi>> {
        self.gateway
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .upgrade()
    }
}

impl PendingWakeRuntimePort for ProductionPendingWakeRuntime {
    fn can_execute(&self) -> bool {
        self.gateway().is_some()
    }

    fn is_agent_gone(&self, agent_id: &str) -> bool {
        self.gateway().is_none_or(|gateway| {
            gateway
                .transcript_runtime
                .session_runtime()
                .is_agent_gone(&gateway.session_workers, agent_id)
        })
    }

    fn is_group_session(&self, agent_id: &str) -> Result<bool, String> {
        let gateway = self
            .gateway()
            .ok_or_else(|| "Host gateway is not ready for pending-wake rearm".to_string())?;
        Ok(gateway
            .session_workers
            .summarize_agent_by_id(agent_id, None)?
            .is_some_and(|summary| summary.is_group))
    }

    fn cloud_watch_is_armed(&self, agent_id: &str, work_id: &str) -> bool {
        self.cloud_agent_watches
            .is_cloud_watch_armed(agent_id, work_id)
    }

    fn watch_cloud_agent(
        &self,
        agent_id: &str,
        work_id: &str,
        quiet_origin: Option<&QuietWakeOrigin>,
    ) -> Result<(), String> {
        let quiet_origin = quiet_origin
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| format!("could not encode quiet wake origin: {error}"))?;
        let _ = self.cloud_agent_watches.watch_cloud_agent(
            agent_id,
            work_id,
            CloudAgentWatchOptions::new(quiet_origin, false),
        );
        Ok(())
    }

    fn watch_background_shell(
        &self,
        agent_id: &str,
        work_id: &str,
        title: Option<&str>,
        quiet_origin: Option<&QuietWakeOrigin>,
    ) -> Result<(), String> {
        let quiet_origin = quiet_origin
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| format!("could not encode quiet wake origin: {error}"))?;
        let _ = self.background_shell_watches.watch_background_shell(
            agent_id,
            work_id,
            BackgroundShellWatchOptions::new(title.map(str::to_string), quiet_origin),
        );
        Ok(())
    }

    fn deliver_recreate_interrupted_shell_notice(
        &self,
        marker: &DurablePendingWakeMarker,
    ) -> Result<(), String> {
        self.completion_revivals
            .handle_background_shell_completion(ShellCompletion {
                agent_id: marker.agent_id.clone(),
                shell_id: marker.work_id.clone(),
                title: marker
                    .title
                    .clone()
                    .unwrap_or_else(|| "Background command".into()),
                status: "error".into(),
                detail: Some(
                    "A host restart interrupted observation of this background command before its terminal result could be delivered; the process state did not survive the recreate boundary, so its final state is unknown."
                        .into(),
                ),
                output_path: None,
                quiet_origin: marker.quiet_origin.clone(),
                pending_wake_marker: Some(marker.clone()),
            });
        Ok(())
    }

    fn revive_lost_subagent(&self, wake: LostSubagentWake) -> Result<(), String> {
        self.completion_revivals
            .handle_background_subagent_completion(SubagentCompletion {
                parent_agent_id: wake.parent_agent_id,
                subagent_agent_id: wake.subagent_agent_id,
                title: wake.title,
                subagent_type: wake.subagent_type,
                status: "error".into(),
                result: wake.result,
                quiet_origin: wake.quiet_origin,
                pending_wake_marker: Some(wake.pending_wake_marker),
            });
        Ok(())
    }

    fn revive_channel_delivery_failure(
        &self,
        marker: &DurablePendingWakeMarker,
    ) -> Result<(), String> {
        let gateway = self
            .gateway()
            .ok_or_else(|| "Host gateway is not ready for channel failure rearm".to_string())?;
        gateway.rearm_channel_delivery_failure(marker)
    }

    fn emit_async_tasks_for_agent(&self, agent_id: &str) {
        if let Some(gateway) = self.gateway() {
            gateway.emit_async_tasks_for_agent(agent_id);
        }
    }

    fn report_pending_wake(&self, report: PendingWakeReport) {
        let Some(gateway) = self.gateway() else {
            return;
        };
        let telemetry = TelemetryPendingWakeReport {
            conversation_id: report.conversation_id.clone(),
            outcome: report.outcome.clone(),
            kind: Some(match report.kind {
                PendingWakeKind::CloudAgent => "cloud-agent",
                PendingWakeKind::Subagent => "subagent",
                PendingWakeKind::Shell => "shell",
                PendingWakeKind::ChannelDelivery => "channel-delivery",
            }.to_string()),
            work_id: Some(report.work_id.clone()),
            age_ms: report.age_ms,
            reason: report.reason.clone(),
            is_quiet_origin: Some(report.is_quiet_origin),
        };
        gateway.events.publish(serde_json::json!({
            "channel": "pending-wake",
            "payload": {
                "agentId": report.conversation_id,
                "kind": report.kind,
                "workId": report.work_id,
                "outcome": report.outcome,
                "ageMs": report.age_ms,
                "reason": report.reason,
                "isQuietOrigin": report.is_quiet_origin,
            }
        }));
        if let Err(error) = gateway.telemetry_logs.report_pending_wake(&telemetry) {
            eprintln!("mahayana-host pending_wake_telemetry_failed error={error}");
        }
    }
}

#[derive(Clone)]
struct ProductionCompletionRevivalRuntime {
    gateway: Arc<Mutex<Weak<UnifiedGatewayApi>>>,
}

fn collect_live_async_tasks(
    generated_agent_runtime: &Mutex<SubagentRuntime>,
    cloud_agent_watches: &RunnerCloudAgentWatches,
    background_shell_watches: &RunnerBackgroundShellWatches,
    agent_id: &str,
) -> Vec<AsyncTask> {
    let mut live_tasks = generated_agent_runtime
        .lock()
        .map(|runtime| {
            runtime
                .running_subagent_records_for_parent(agent_id)
                .into_iter()
                .map(|(id, record)| AsyncTask {
                    kind: "subagent".into(),
                    id,
                    label: record.title,
                    status: "running".into(),
                    started_at_ms: record.started_at_ms as f64,
                    detail: Some(record.subagent_type.clone()),
                    subagent_type: Some(record.subagent_type),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    live_tasks.extend(
        background_shell_watches
            .async_task_snapshots(agent_id)
            .into_iter()
            .map(|task| AsyncTask {
                kind: "shell".into(),
                id: task.id,
                label: task.label,
                status: "running".into(),
                started_at_ms: task.started_at_ms as f64,
                detail: None,
                subagent_type: None,
            }),
    );
    live_tasks.extend(
        cloud_agent_watches
            .async_task_snapshots(agent_id)
            .into_iter()
            .map(|task| AsyncTask {
                kind: "cloud-agent".into(),
                id: task.id,
                label: task.label,
                status: "running".into(),
                started_at_ms: task.started_at_ms as f64,
                detail: None,
                subagent_type: None,
            }),
    );
    live_tasks
}

fn publish_async_tasks_changed(
    events: &GatewayEventHub,
    runtime: &ProductionTranscriptRuntime,
    generated_agent_runtime: &Mutex<SubagentRuntime>,
    cloud_agent_watches: &RunnerCloudAgentWatches,
    background_shell_watches: &RunnerBackgroundShellWatches,
    agent_id: &str,
) {
    let live_tasks = collect_live_async_tasks(
        generated_agent_runtime,
        cloud_agent_watches,
        background_shell_watches,
        agent_id,
    );
    let tasks = runtime.get_async_tasks(agent_id, &live_tasks);
    match async_tasks_changed_event(agent_id, &tasks) {
        Ok(payload) => events.publish(serde_json::json!({
            "channel": "async-tasks",
            "payload": payload,
        })),
        Err(error) => eprintln!(
            "mahayana-host async_tasks_projection_failed agent={} error={}",
            agent_id, error
        ),
    }
}

fn publish_async_tasks_changed_from_slots(
    events: &GatewayEventHub,
    runtime: &ProductionTranscriptRuntime,
    generated_agent_runtime: &Arc<Mutex<SubagentRuntime>>,
    cloud_agent_watches: &Arc<Mutex<Weak<RunnerCloudAgentWatches>>>,
    background_shell_watches: &Arc<Mutex<Weak<RunnerBackgroundShellWatches>>>,
    agent_id: &str,
) {
    let cloud_agent_watches = cloud_agent_watches
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .upgrade();
    let background_shell_watches = background_shell_watches
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .upgrade();
    if let (Some(cloud_agent_watches), Some(background_shell_watches)) =
        (cloud_agent_watches, background_shell_watches)
    {
        publish_async_tasks_changed(
            events,
            runtime,
            generated_agent_runtime.as_ref(),
            cloud_agent_watches.as_ref(),
            background_shell_watches.as_ref(),
            agent_id,
        );
        return;
    }

    let tasks = runtime.get_async_tasks(agent_id, &[]);
    if let Ok(payload) = async_tasks_changed_event(agent_id, &tasks) {
        events.publish(serde_json::json!({
            "channel": "async-tasks",
            "payload": payload,
        }));
    }
}

impl ProductionCompletionRevivalRuntime {
    fn gateway(&self) -> Option<Arc<UnifiedGatewayApi>> {
        self.gateway
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .upgrade()
    }
}

impl CompletionRevivalRuntimePort for ProductionCompletionRevivalRuntime {
    fn can_execute(&self) -> bool {
        let Some(gateway) = self.gateway() else {
            return false;
        };
        configured_routed_provider(&gateway.data_dir.join("settings.json")).is_some()
    }

    fn is_agent_deleted(&self, agent_id: &str) -> bool {
        self.gateway().is_none_or(|gateway| {
            gateway
                .transcript_runtime
                .session_runtime()
                .is_agent_gone(&gateway.session_workers, agent_id)
        })
    }

    fn is_agent_gone(&self, agent_id: &str) -> bool {
        self.is_agent_deleted(agent_id)
    }

    fn clear_pending_wake(&self, marker: DurablePendingWakeMarker) {
        let Some(gateway) = self.gateway() else {
            return;
        };
        if let Some(store) = gateway.transcript_runtime.pending_wake_store() {
            let _ = store.clear_if_matches(&marker);
        }
        gateway.emit_async_tasks_for_agent(&marker.agent_id);
    }

    fn clear_pending_wake_by_key(&self, agent_id: &str, kind: PendingWakeKind, work_id: &str) {
        let Some(gateway) = self.gateway() else {
            return;
        };
        if let Some(store) = gateway.transcript_runtime.pending_wake_store() {
            let _ = store.clear_one(agent_id, kind, work_id);
        }
        gateway.emit_async_tasks_for_agent(agent_id);
    }

    fn run_background_revival(
        &self,
        agent_id: &str,
        source: &str,
        prompt: &str,
        is_silence_allowed: bool,
        auto_review_epoch: &str,
    ) -> Result<RevivalExecution, String> {
        let gateway = self
            .gateway()
            .ok_or_else(|| "Host gateway is not ready for background revival".to_string())?;
        let provider = configured_routed_provider(&gateway.data_dir.join("settings.json"))
            .ok_or_else(|| "no routed provider configured for background revival".to_string())?;
        run_local_background_revival_turn(
            gateway.local_routed_runner_deps(),
            provider,
            agent_id,
            source,
            prompt,
            is_silence_allowed,
            auto_review_epoch,
        )
    }

    fn mark_resume_pending_for_quiesced_revival(&self, agent_id: &str) {
        let Some(gateway) = self.gateway() else {
            return;
        };
        gateway
            .transcript_manager
            .mark_upgrade_resume_pending(UpgradeResumeMarker {
                agent_id: agent_id.to_string(),
                marked_at_ms: started_at_ms() as f64,
                source: Some("background-revival".into()),
                automation_id: None,
                automation_run_id: None,
            });
    }

    fn report_revival(&self, report: RevivalReport) {
        let Some(gateway) = self.gateway() else {
            return;
        };
        let completion_count = i64::try_from(report.completion_count).unwrap_or(i64::MAX);
        let sent_message_count = report
            .sent_message_count
            .map(|count| i64::try_from(count).unwrap_or(i64::MAX));
        let result = if report.kind == "shell" {
            gateway.telemetry_logs.report_shell_revival(&ShellRevivalReport {
                conversation_id: report.agent_id,
                outcome: report.outcome,
                completion_count,
                sent_message_count,
                is_quiet_origin: Some(report.is_quiet_origin),
                reason: report.reason,
            })
        } else {
            gateway.telemetry_logs.report_subagent_revival(&SubagentRevivalReport {
                parent_agent_id: report.agent_id,
                outcome: report.outcome,
                completion_count,
                subagent_type: report.subagent_type,
                subagent_agent_id: report.subagent_agent_id,
                reason: report.reason,
                sent_message_count,
                is_quiet_origin: Some(report.is_quiet_origin),
            })
        };
        if let Err(error) = result {
            eprintln!("mahayana-host background_revival_telemetry_failed error={error}");
        }
    }

    fn report_revival_error(&self, agent_id: &str, title: &str, error: &str) {
        eprintln!(
            "mahayana-host background_revival_failed agent={agent_id} title={title} error={error}"
        );
    }
}

pub(crate) fn report_agent_roster_security_findings(config: &serde_json::Value) {
    for finding in mahayana_host_runtime::agents::agent_roster::collect_agent_roster_findings(config)
    {
        mahayana_host_runtime::host_diagnostics::report_diagnostic_event(
            "security.agent_roster.finding",
            serde_json::json!({
                "checkId": finding.check_id,
                "severity": finding.severity,
                "title": finding.title,
                "detail": finding.detail,
                "remediation": finding.remediation,
            }),
        );
    }
}

pub(crate) fn session_kind_label(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Cron => "cron",
        SessionKind::Direct => "direct",
        SessionKind::Group => "group",
        SessionKind::Global => "global",
        SessionKind::SpawnChild => "spawn-child",
        SessionKind::Unknown => "unknown",
    }
}

pub(crate) fn resolve_gateway_session_route(
    args: &serde_json::Value,
    raw_key: &str,
) -> Result<PublicSessionRoute, GatewayCommandError> {
    resolve_public_session_route(
        raw_key,
        args.get("chatType").and_then(serde_json::Value::as_str),
        args.get("spawnedBy").and_then(serde_json::Value::as_str),
    )
    .map_err(|error| match error {
        PublicSessionRouteError::EmptySelector => {
            GatewayCommandError::BadRequest("session selector must be non-empty".into())
        }
        PublicSessionRouteError::HarnessNamespaceReserved => GatewayCommandError::BadRequest(
            "Session key namespace is reserved for agent harness-owned sessions.".into(),
        ),
        PublicSessionRouteError::EmptyAgentIdentity => GatewayCommandError::BadRequest(
            "session selector contains an empty agent identity".into(),
        ),
    })
}

pub(crate) fn reject_agent_wide_session_route(
    route: &PublicSessionRoute,
    operation: &str,
) -> Result<(), GatewayCommandError> {
    if matches!(
        route.kind,
        SessionKind::Cron
            | SessionKind::Group
            | SessionKind::SpawnChild
            | SessionKind::Unknown
    ) {
        return Err(GatewayCommandError::BadRequest(format!(
            "{operation} does not support {kind} session routes until per-session ownership is available",
            kind = session_kind_label(route.kind),
        )));
    }
    Ok(())
}

fn validate_internal_send_route(args: &serde_json::Value) -> Result<(), ProductionSendError> {
    let Some(agent_id) = args
        .get("agentId")
        .or_else(|| args.get("id"))
        .and_then(serde_json::Value::as_str)
    else {
        return Err(ProductionSendError::BadRequest(
            "sendPrompt requires agentId".into(),
        ));
    };
    let request_source = args
        .get("requestSource")
        .and_then(serde_json::Value::as_str);
    let requires_selector = matches!(
        request_source,
        Some("automation" | "group-member" | "upgrade-resume" | "ack-redrive" | "subagent")
    );
    let session_key = args
        .get("sessionKey")
        .or_else(|| args.get("sessionId"))
        .and_then(serde_json::Value::as_str);
    let explicit_direct_fallback = args
        .get("directMainFallback")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !requires_selector && session_key.is_none() && !explicit_direct_fallback {
        return Ok(());
    }
    resolve_internal_send_route(
        agent_id,
        session_key,
        args.get("chatType").and_then(serde_json::Value::as_str),
        args.get("spawnedBy").and_then(serde_json::Value::as_str),
        explicit_direct_fallback,
    )
    .map(|_| ())
    .map_err(|error| {
        let detail = match error {
            InternalSessionRouteError::MissingSelector =>
                "trusted send producer requires sessionKey/sessionId".to_string(),
            InternalSessionRouteError::EmptyAgentIdentity =>
                "trusted send producer requires a non-empty agentId".to_string(),
            InternalSessionRouteError::SelectorAgentMismatch =>
                "session selector does not belong to agentId".to_string(),
            InternalSessionRouteError::Public(PublicSessionRouteError::EmptySelector) =>
                "session selector must be non-empty".to_string(),
            InternalSessionRouteError::Public(PublicSessionRouteError::HarnessNamespaceReserved) =>
                "Session key namespace is reserved for agent harness-owned sessions.".to_string(),
            InternalSessionRouteError::Public(PublicSessionRouteError::EmptyAgentIdentity) =>
                "session selector contains an empty agent identity".to_string(),
        };
        ProductionSendError::BadRequest(detail)
    })
}

impl UnifiedGatewayApi {

    fn rearm_channel_delivery_failure(
        &self,
        marker: &DurablePendingWakeMarker,
    ) -> Result<(), String> {
        let address_token = marker
            .address_token
            .clone()
            .ok_or_else(|| "channel failure marker has no address token".to_string())?;
        let reason = marker
            .failure_reason
            .clone()
            .ok_or_else(|| "channel failure marker has no failure reason".to_string())?;
        queue_channel_delivery_failure(
            self.local_routed_runner_deps(),
            Arc::clone(&self.roster_emit),
            Arc::clone(&self.background_wakes),
            marker.agent_id.clone(),
            address_token,
            marker.delivery_operation_id.clone(),
            marker.session_key.clone(),
            reason,
        );
        Ok(())
    }

    fn call_accept_routed_prompt(
        &self,
        args: serde_json::Value,
        gateway_context: Option<&GatewayCommandContext>,
    ) -> Result<serde_json::Value, GatewayCommandError> {

            let _ = self.preempt_group_member_runs_for_direct_send(&args);
            let send_trace = begin_send_trace(
                gateway_context.and_then(|context| context.traceparent.as_deref()),
            );
            let host_receipt_elapsed_ms = gateway_context
                .map(|context| context.dispatch_started.elapsed().as_secs_f64() * 1_000.0)
                .unwrap_or_default();
            let host_receipt_epoch_ms =
                started_at_ms().saturating_sub(host_receipt_elapsed_ms as u64) as f64;
            let durable_append_timing = Mutex::new(None::<(f64, f64)>);
            let durable_args = args.clone();
            let acceptance_agent_id = durable_args
                .get("agentId")
                .or_else(|| durable_args.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let acceptance_was_on_screen = acceptance_agent_id
                .as_deref()
                .is_some_and(|agent_id| self.addressed_chat_on_screen(agent_id));
            if durable_args
                .get("requestSource")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                == Some("human-handoff")
            {
                let conversation_id = durable_args
                    .get("humanHandoffConversationId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        GatewayCommandError::BadRequest(
                            "human-handoff requires humanHandoffConversationId".into(),
                        )
                    })?;
                self.session_workers
                    .read_human_conversation_transcript(conversation_id)
                    .map_err(GatewayCommandError::BadRequest)?;
            }
            let mut acceptance = self
                .transcript_runtime
                .accept_routed_send(&durable_args, |accepted| {
                    let durable_append_start_epoch_ms = started_at_ms() as f64;
                    let durable_append_started = Instant::now();
                    let mut persisted = persist_accepted_send_prompt_context(
                        &self.session_workers,
                        &durable_args,
                        accepted,
                    )
                    .map_err(map_session_send_error)?;
                    *durable_append_timing
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((
                        durable_append_start_epoch_ms,
                        durable_append_started.elapsed().as_secs_f64() * 1_000.0,
                    ));
                    persisted.mark_accepted_echoes_on_active_transcript(
                        acceptance_was_on_screen,
                    );
                    Ok(persisted)
                })
                .map_err(map_production_send_error)?;
            if !acceptance.duplicate {
                if let Some(agent_id) = acceptance_agent_id.as_deref() {
                    self.emit_persisted_send_acceptance(
                        agent_id,
                        &acceptance.context,
                        true,
                    )
                    .map_err(map_production_send_error)?;
                    let ack_emit_host_ms = gateway_context
                        .map(|context| context.dispatch_started.elapsed().as_secs_f64() * 1_000.0)
                        .unwrap_or_default();
                    record_send_acceptance_tracing(
                        send_trace.as_ref(),
                        agent_id,
                        durable_args
                            .get("clientNonce")
                            .and_then(serde_json::Value::as_str),
                        &acceptance.context,
                        *durable_append_timing
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()),
                        host_receipt_epoch_ms,
                        ack_emit_host_ms,
                    );
                }
            }

            if !acceptance.duplicate
                && durable_args
                    .get("skipAckObligation")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
            {
                let agent_id = durable_args
                    .get("agentId")
                    .or_else(|| durable_args.get("id"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if let Some(agent_id) = agent_id {
                    let direct_local = self
                        .session_workers
                        .summarize_agent_by_id(agent_id, None)
                        .map_err(GatewayCommandError::Internal)?
                        .is_some_and(|summary| !summary.is_group);
                    if direct_local {
                        let accepted_at_ms = started_at_ms() as f64;
                        let _ack_guard = self.ack_obligations.arm_send_guard(
                            agent_id,
                            accepted_at_ms,
                            true,
                        );
                        self.ack_obligations
                            .record_send(agent_id, accepted_at_ms)
                            .map_err(|error| GatewayCommandError::Internal(format!(
                                "could not record durable ack obligation for {agent_id}: {error}"
                            )))?;
                    }
                }
            }

            let context = acceptance.context;
            let recent_user_messages = context
                .recent_user_messages
                .into_iter()
                .map(|message| {
                    let mut value = serde_json::json!({
                        "id": message.id,
                        "text": message.text,
                    });
                    if let Some(confirmed) = message.confirmed {
                        value["confirmed"] = serde_json::Value::Bool(confirmed);
                    }
                    value
                })
                .collect::<Vec<_>>();
            let result = Ok(serde_json::json!({
                "accepted": true,
                "duplicate": acceptance.duplicate,
                "echoEntryId": context.echo_entry_id,
                "userMessageId": context.user_message_id,
                "recentUserMessages": recent_user_messages,
            }));
            if let Some(trace) = send_trace.as_ref() {
                trace.span.end();
            }
            return result;
        
    }

    fn call_send_prompt(
        &self,
        args: serde_json::Value,
        gateway_context: Option<&GatewayCommandContext>,
    ) -> Result<serde_json::Value, GatewayCommandError> {

            validate_internal_send_route(&args).map_err(map_production_send_error)?;
            if let Some(agent_id) = args
                .get("agentId")
                .or_else(|| args.get("id"))
                .and_then(serde_json::Value::as_str)
            {
                self.session_workers
                    .assert_session_work_admission(
                        agent_id,
                        args.get("sessionKey")
                            .or_else(|| args.get("sessionId"))
                            .and_then(serde_json::Value::as_str),
                    )
                    .map_err(ProductionSendError::BadRequest)
                    .map_err(map_production_send_error)?;
            }

            let send_trace = begin_send_trace(
                gateway_context.and_then(|context| context.traceparent.as_deref()),
            );
            let host_receipt_elapsed_ms = gateway_context
                .map(|context| context.dispatch_started.elapsed().as_secs_f64() * 1_000.0)
                .unwrap_or_default();
            let host_receipt_epoch_ms =
                started_at_ms().saturating_sub(host_receipt_elapsed_ms as u64) as f64;
            let durable_append_timing = Mutex::new(None::<(f64, f64)>);
            let durable_args = args.clone();
            let runner_args = shape_send_prompt_media_args(&args);
            let watchdog_registry = Arc::clone(&self.runner_registry);
            let watchdog_ack_obligations = Arc::clone(&self.ack_obligations);
            let supersede_registry = Arc::clone(&self.runner_registry);
            let supersede_ack_obligations = Arc::clone(&self.ack_obligations);
            let supersede_logs = self.telemetry_logs.clone();
            let watchdog_transcript_runtime = Arc::clone(&self.transcript_runtime);
            let watchdog_events = self.events.clone();
            let watchdog_logs = self.telemetry_logs.clone();
            let user_message_logs = self.telemetry_logs.clone();
            let accepted_logs = self.telemetry_logs.clone();
            let dequeued_logs = self.telemetry_logs.clone();
            let send_agent_id = durable_args
                .get("agentId")
                .or_else(|| durable_args.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let send_was_in_flight = send_agent_id
                .as_deref()
                .is_some_and(|agent_id| self.transcript_runtime.is_agent_running(agent_id));
            let send_is_direct_local = match send_agent_id.as_deref() {
                Some(agent_id) => self
                    .session_workers
                    .summarize_agent_by_id(agent_id, None)
                    .map_err(GatewayCommandError::Internal)?
                    .is_some_and(|summary| !summary.is_group),
                None => false,
            };
            let send_addressed_on_screen = send_agent_id
                .as_deref()
                .is_some_and(|agent_id| self.addressed_chat_on_screen(agent_id));
            let send_ack_guard = Mutex::new(None);
            let persisted_send_context = Mutex::new(None::<PersistedSendContext>);
            let send_result = self
                .transcript_runtime
                .execute_send_with_acceptance_observer(
                    &durable_args,
                    || {
                        let persisted = persisted_send_context
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone()
                            .ok_or_else(|| ProductionSendError::Internal(
                                "send dispatch started before durable persistence".into(),
                            ))?;
                        if let Some(group_result) =
                            self.dispatch_mirror_or_group_send_if_supported(
                                &durable_args,
                                &runner_args,
                                &persisted,
                            )?
                        {
                            return Ok(group_result);
                        }
                        let agent_id = send_agent_id.as_deref().ok_or_else(|| {
                            ProductionSendError::BadRequest(
                                "sendPrompt requires agentId for direct Runner dispatch".into(),
                            )
                        })?;
                        let direct_runner_args = prepare_direct_turn_runner_args(
                            Arc::clone(&self.session_workers),
                            agent_id,
                            &runner_args,
                            &persisted,
                        )
                        .map_err(ProductionSendError::Internal)?;
                        call_host_lane(&self.host_tx, "sendPrompt", direct_runner_args)
                            .map_err(map_gateway_send_error)
                    },
                    |accepted| {
                        let durable_append_start_epoch_ms = started_at_ms() as f64;
                        let durable_append_started = Instant::now();
                        let mut persisted = persist_accepted_send_prompt_context(
                            &self.session_workers,
                            &durable_args,
                            accepted,
                        )
                        .map_err(map_session_send_error)?;
                        *durable_append_timing
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((
                            durable_append_start_epoch_ms,
                            durable_append_started.elapsed().as_secs_f64() * 1_000.0,
                        ));
                        persisted.mark_accepted_echoes_on_active_transcript(
                            send_addressed_on_screen,
                        );
                        *persisted_send_context
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            Some(persisted.clone());
                        if accepted.get("accepted").and_then(serde_json::Value::as_bool)
                            == Some(true)
                        {
                            if accepted.get("duplicate").and_then(serde_json::Value::as_bool)
                                != Some(true)
                            {
                                if let Some(agent_id) = send_agent_id.as_deref() {
                                    let attachment_paths = durable_args
                                        .get("attachmentPaths")
                                        .and_then(serde_json::Value::as_array)
                                        .map(|values| {
                                            values
                                                .iter()
                                                .filter_map(serde_json::Value::as_str)
                                                .map(str::to_string)
                                                .collect::<Vec<_>>()
                                        })
                                        .unwrap_or_default();
                                    let is_group_room = self
                                        .session_workers
                                        .summarize_agent_by_id(agent_id, None)
                                        .map_err(ProductionSendError::Internal)?
                                        .is_some_and(|summary| summary.is_group);
                                    self.telemetry_api.report_message_sent(MessageSentReport {
                                        agent_id: agent_id.to_string(),
                                        prompt: durable_args
                                            .get("prompt")
                                            .or_else(|| durable_args.get("text"))
                                            .and_then(serde_json::Value::as_str)
                                            .map(str::to_string),
                                        attachment_paths,
                                        rich_text: durable_args
                                            .get("richText")
                                            .and_then(serde_json::Value::as_str)
                                            .map(str::to_string),
                                        is_fork: durable_args
                                            .get("isFork")
                                            .and_then(serde_json::Value::as_bool)
                                            .unwrap_or(false),
                                        source: durable_args
                                            .get("source")
                                            .and_then(serde_json::Value::as_str)
                                            .map(str::to_string),
                                        is_group_room,
                                    });
                                }
                            }
                            if let Some(agent_id) = send_agent_id.as_deref() {
                                let direct_local = self
                                    .session_workers
                                    .summarize_agent_by_id(agent_id, None)
                                    .map_err(ProductionSendError::Internal)?
                                    .is_some_and(|summary| !summary.is_group);
                                if direct_local {
                                    let fields = UserMessageReceivedFields {
                                        conversation_id: agent_id.to_string(),
                                        was_in_flight: send_was_in_flight,
                                    };
                                    if let Err(error) = user_message_logs.report_user_message_received(&fields) {
                                        eprintln!(
                                            "mahayana-host user_message_received_telemetry_failed agent={agent_id} error={error}"
                                        );
                                    }
                                }
                                if direct_local
                                    && durable_args
                                        .get("skipAckObligation")
                                        .and_then(serde_json::Value::as_bool)
                                        != Some(true)
                                {
                                    let accepted_at_ms = started_at_ms() as f64;
                                    let guard = self.ack_obligations.arm_send_guard(
                                        agent_id,
                                        accepted_at_ms,
                                        true,
                                    );
                                    self.ack_obligations
                                        .record_send(agent_id, accepted_at_ms)
                                        .map_err(|error| ProductionSendError::Internal(
                                            format!(
                                                "could not record durable ack obligation for {agent_id}: {error}"
                                            )
                                        ))?;
                                    *send_ack_guard
                                        .lock()
                                        .map_err(|_| ProductionSendError::Internal(
                                            "send ack guard slot poisoned".into()
                                        ))? = Some(guard);
                                }
                            }
                        }
                        Ok(persisted)
                    },
                    |persisted| {
                        if let Some(agent_id) = send_agent_id.as_deref() {
                            if send_is_direct_local {
                                let interrupted_group =
                                    self.preempt_group_member_runs_for_direct_send(&durable_args);
                                let carries_recovery = persisted
                                    .user_message_id
                                    .as_deref()
                                    .is_some_and(|value| !value.trim().is_empty())
                                    && durable_args
                                        .get("isFork")
                                        .and_then(serde_json::Value::as_bool)
                                        != Some(true);
                                let interrupted_one_to_one = supersede_registry
                                    .preempt_routed_agent_for_supersede(
                                        agent_id,
                                        RUN_DIRECT_USER_INTERRUPT_REASON,
                                        Some(carries_recovery),
                                    )
                                    > 0;
                                let had_active_run =
                                    interrupted_group || interrupted_one_to_one;
                                if had_active_run {
                                    if let Err(error) = supersede_ack_obligations
                                        .record_interrupt(agent_id, started_at_ms() as f64)
                                    {
                                        eprintln!(
                                            "mahayana-host supersede_ack_interrupt_failed agent={agent_id} error={error}"
                                        );
                                    }
                                }
                                let fields = TurnInterruptFields {
                                    conversation_id: agent_id.to_string(),
                                    reason: "superseded".into(),
                                    had_active_run,
                                    was_in_flight: send_was_in_flight,
                                };
                                if let Err(error) = supersede_logs.report_turn_interrupt(&fields) {
                                    eprintln!(
                                        "mahayana-host turn_interrupt_telemetry_failed agent={agent_id} error={error}"
                                    );
                                }
                            }
                            if let Err(error) =
                                self.emit_persisted_send_acceptance(agent_id, persisted, true)
                            {
                                eprintln!(
                                    "mahayana-host send_acceptance_projection_failed agent={agent_id} error={error}"
                                );
                            }
                            let ack_emit_host_ms = gateway_context
                                .map(|context| context.dispatch_started.elapsed().as_secs_f64() * 1_000.0)
                                .unwrap_or_default();
                            record_send_acceptance_tracing(
                                send_trace.as_ref(),
                                agent_id,
                                durable_args
                                    .get("clientNonce")
                                    .and_then(serde_json::Value::as_str),
                                persisted,
                                *durable_append_timing
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
                                host_receipt_epoch_ms,
                                ack_emit_host_ms,
                            );
                        }
                    },
                    move |event| {
                        let was_in_flight = event.stage == WatchdogStage::Trip
                            && watchdog_transcript_runtime.is_agent_running(&event.agent_id);
                        let interrupted = if event.stage == WatchdogStage::Trip {
                            let interrupted = watchdog_registry
                                .interrupt_wedged_run_for_watchdog(&event.agent_id);
                            if interrupted {
                                let _ = watchdog_ack_obligations.record_interrupt(
                                    &event.agent_id,
                                    started_at_ms() as f64,
                                );
                            }
                            let fields = TurnInterruptFields {
                                conversation_id: event.agent_id.clone(),
                                reason: "watchdog".into(),
                                had_active_run: interrupted,
                                was_in_flight,
                            };
                            if let Err(error) = watchdog_logs.report_turn_interrupt(&fields) {
                                eprintln!(
                                    "mahayana-host turn_interrupt_telemetry_failed agent={} error={error}",
                                    event.agent_id
                                );
                            }
                            interrupted
                        } else {
                            false
                        };
                        if event.stage == WatchdogStage::Escape {
                            let _ = watchdog_ack_obligations.retire_ack_run_token(
                                &event.agent_id,
                                event.ack_token.as_deref(),
                            );
                        }
                        let report = QueueWatchdogReport {
                            conversation_id: event.agent_id.clone(),
                            stage: event.stage.as_str().to_string(),
                            active_lane: Some(event.active_lane.as_str().to_string()),
                            active_source: Some(event.active_source.clone()),
                            active_runtime_ms: event.active_runtime_ms as f64,
                            waiting_user_age_ms: event.waiting_user_age_ms.map(|value| value as f64),
                            interrupted: (event.stage == WatchdogStage::Trip).then_some(interrupted),
                        };
                        let _ = watchdog_logs.report_queue_watchdog(&report);
                        watchdog_events.publish(serde_json::json!({
                            "channel": "run-queue-watchdog",
                            "payload": {
                                "agentId": event.agent_id,
                                "stage": event.stage.as_str(),
                                "activeLane": event.active_lane.as_str(),
                                "activeSource": event.active_source,
                                "activeRuntimeMs": event.active_runtime_ms,
                                "waitingUserAgeMs": event.waiting_user_age_ms,
                                "ackToken": event.ack_token,
                                "interrupted": interrupted
                            }
                        }));
                        interrupted
                    },
                    move |event| {
                        let report = QueueAcceptedReport {
                            conversation_id: event.agent_id.clone(),
                            lane: event.lane.as_str().to_string(),
                            source: event.source.clone(),
                            position: i64::try_from(event.position).unwrap_or(i64::MAX),
                            depth_user: i64::try_from(event.depth_user).unwrap_or(i64::MAX),
                            depth_agent: i64::try_from(event.depth_agent).unwrap_or(i64::MAX),
                            depth_background: i64::try_from(event.depth_background).unwrap_or(i64::MAX),
                            has_active: event.has_active,
                        };
                        let _ = accepted_logs.report_queue_accepted(&report);
                    },
                    move |event| {
                        let report = QueueDequeuedReport {
                            conversation_id: event.agent_id.clone(),
                            lane: event.lane.as_str().to_string(),
                            source: event.source.clone(),
                            queue_wait_ms: event.queue_wait_ms as f64,
                            accepted_to_run_ms: event.accepted_to_run_ms.map(|value| value as f64),
                            jumped_background: i64::try_from(event.jumped_background).unwrap_or(i64::MAX),
                            depth_user: i64::try_from(event.depth_user).unwrap_or(i64::MAX),
                            depth_agent: i64::try_from(event.depth_agent).unwrap_or(i64::MAX),
                            depth_background: i64::try_from(event.depth_background).unwrap_or(i64::MAX),
                        };
                        let _ = dequeued_logs.report_queue_dequeued(&report);
                    },
                );
            // Dropping the guard after the complete dispatch scope mirrors
            // Grok's Symbol.dispose send guard: it only recreates a missing
            // direct-local obligation and never double-coalesces an existing one.
            drop(send_ack_guard);
            if let Some(trace) = send_trace.as_ref() {
                trace.span.end();
            }
            return send_result.map_err(map_production_send_error);
        
    }

    fn addressed_chat_on_screen(&self, agent_id: &str) -> bool {
        SandAgentSessionStore::new(Arc::clone(&self.session_workers))
            .read_active_agent_id()
            .as_deref()
            == Some(agent_id)
    }

    fn emit_persisted_send_acceptance(
        &self,
        agent_id: &str,
        context: &PersistedSendContext,
        direct_addressed_acceptance: bool,
    ) -> Result<(), ProductionSendError> {
        if !context.acceptance_effects_applied {
            return Ok(());
        }
        self.trays.clear_for_agent(agent_id);
        let active_agent_id =
            SandAgentSessionStore::new(Arc::clone(&self.session_workers)).read_active_agent_id();
        emit_accepted_send_echoes(
            &self.events,
            &self.roster_emit,
            active_agent_id.as_deref(),
            agent_id,
            direct_addressed_acceptance,
            context,
        )
        .map(|_| ())
        .map_err(ProductionSendError::Internal)
    }

    fn created_agent_kickstart_hook(&self) -> AgentKickstartHook {
        let sessions = Arc::clone(&self.session_workers);
        let runtime = ProductionCreatedAgentKickstartRuntime {
            deps: self.local_routed_runner_deps(),
            roster: Arc::clone(&self.roster_emit),
        };
        Arc::new(move |agent_id| {
            let sessions = Arc::clone(&sessions);
            let runtime = runtime.clone();
            let agent_id = agent_id.to_string();
            let thread_agent_id = agent_id.clone();
            if let Err(error) = thread::Builder::new()
                .name(format!("mahayana-agent-kickstart-{agent_id}"))
                .spawn(move || {
                    if let Err(error) =
                        run_created_agent_kickstart(sessions.as_ref(), &runtime, &thread_agent_id)
                    {
                        eprintln!(
                            "mahayana-host created_agent_kickstart_failed agent={} error={error}",
                            thread_agent_id
                        );
                    }
                })
            {
                eprintln!(
                    "mahayana-host created_agent_kickstart_spawn_failed agent={agent_id} error={error}"
                );
            }
        })
    }

    fn emit_session_activation_roster_updates(
        &self,
        agent_id: &str,
        previous_agent_id: Option<&str>,
    ) {
        let _ = self.roster_emit.emit_agent_update(agent_id);
        if let Some(previous_agent_id) = previous_agent_id
            && previous_agent_id != agent_id
        {
            let _ = self.roster_emit.emit_agent_update(previous_agent_id);
        }
    }

    fn schedule_windowed_session_activation(
        &self,
        agent_id: String,
        shipped_through_id: Option<String>,
    ) {
        let generation = self
            .transcript_runtime
            .session_runtime()
            .schedule_deferred_activation(&agent_id, shipped_through_id.as_deref());
        let manager = Arc::clone(&self.transcript_manager);
        let roster = Arc::clone(&self.roster_emit);
        let spawn_agent_id = agent_id.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("mahayana-windowed-activation-{agent_id}"))
            .spawn(move || {
                // Frozen Grok waits for the task boundary before activating a
                // cold bounded read. Keep this off the request lane so the
                // bounded payload can settle before the full Session becomes active.
                thread::sleep(Duration::from_millis(1));
                if manager.is_disposed() {
                    return;
                }
                let runtime = manager.transcript_runtime();
                let Some(claim) = runtime
                    .session_runtime()
                    .claim_deferred_activation(generation, &agent_id)
                else {
                    return;
                };
                let previous = manager.active_agent_id();
                match manager.switch_agent(&agent_id, started_at_ms() as f64) {
                    Ok(entries) => {
                        for entry in runtime
                            .session_runtime()
                            .windowed_catch_up(claim.shipped_through_id.as_deref(), &entries)
                        {
                            roster.publish_transcript_appended(&agent_id, &entry);
                        }
                        let _ = roster.emit_agent_update(&agent_id);
                        if let Some(previous) = previous.as_deref()
                            && previous != agent_id
                        {
                            let _ = roster.emit_agent_update(previous);
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "[sand] windowed background activation failed for {agent_id}: {error}"
                        );
                    }
                }
            })
        {
            eprintln!(
                "mahayana-host windowed_activation_worker_spawn_failed agent={spawn_agent_id} error={error}"
            );
        }
    }

    fn local_routed_runner_deps(&self) -> LocalRoutedRunnerDeps {
        LocalRoutedRunnerDeps {
            routed_tool_relay: Arc::clone(&self.routed_tool_relay),
            mcp_service: Arc::clone(&self.mcp_service),
            auth: Arc::clone(&self.auth),
            auto_review: Arc::clone(&self.auto_review),
            events: self.events.clone(),
            host_tx: self.host_tx.clone(),
            data_dir: self.data_dir.clone(),
            request_context: Arc::clone(&self.request_context),
            experiments: Arc::clone(&self.experiments),
            settings: Arc::clone(&self.settings),
            inference: Arc::clone(&self.inference),
            session_workers: Arc::clone(&self.session_workers),
            runner_registry: Arc::clone(&self.runner_registry),
            ack_obligations: Arc::clone(&self.ack_obligations),
            transcript_runtime: Arc::clone(&self.transcript_runtime),
            transcript_manager: Arc::clone(&self.transcript_manager),
            generated_agent_runtime: Arc::clone(&self.generated_agent_runtime),
            completion_revivals: Arc::clone(&self.completion_revivals),
            forever_box: Arc::clone(&self.forever_box),
            local_exec: Arc::clone(&self.local_exec),
            local_tool_permission: Arc::clone(&self.local_tool_permission),
            session_handoff: self.session_handoff.clone(),
            trays: Arc::clone(&self.trays),
            telemetry_logs: self.telemetry_logs.clone(),
            product_analytics: self.product_analytics.clone(),
            production_action_auditor: self.production_action_auditor.clone(),
            cloud_agents: Arc::clone(&self.cloud_agents),
            cloud_agent_watches: Arc::clone(&self.cloud_agent_watches),
            background_shell_watches: Arc::clone(&self.background_shell_watches),
            host_runner_composition: Arc::clone(&self.host_runner_composition),
            box_store_sync: self.box_store_sync.clone(),
            automations_lifecycle: Arc::clone(&self.automations_lifecycle),
            generated_subagent_cleanup: Arc::clone(&self.generated_subagent_cleanup),
        }
    }

    fn emit_async_tasks_for_agent(&self, agent_id: &str) {
        publish_async_tasks_changed(
            &self.events,
            self.transcript_runtime.as_ref(),
            self.generated_agent_runtime.as_ref(),
            self.cloud_agent_watches.as_ref(),
            self.background_shell_watches.as_ref(),
            agent_id,
        );
    }

    fn refresh_production_automations(&self) {
        let lifecycle = self
            .automations_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .upgrade();
        if let Some(lifecycle) = lifecycle {
            // Reconcile both backend relay state and the cloud scheduling
            // shadow. The listener reconnect watcher remains an interactive
            // handoff owner and is never armed merely because a routine exists.
            lifecycle.request_reconcile();
        }
    }

    fn delete_production_automation_schedules(&self, agent_id: &str) {
        let lifecycle = self
            .automations_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .upgrade();
        if let Some(lifecycle) = lifecycle {
            lifecycle.delete_agent_schedules(agent_id);
        }
    }

    fn resume_with_hidden_handoff(
        &self,
        agent_id: &str,
        prompt: String,
        error_title: &str,
    ) -> Result<(), String> {
        let provider = configured_routed_provider(&self.data_dir.join("settings.json"));
        let can_execute = !self.transcript_runtime.is_quiescing_for_upgrade() && provider.is_some();
        if !can_execute {
            return Ok(());
        }
        let is_group = self
            .session_workers
            .summarize_agent_by_id(agent_id, None)
            .ok()
            .flatten()
            .map(|summary| summary.is_group);
        if !should_resume_hidden_handoff(true, is_group) {
            return Ok(());
        }
        let Some(provider) = provider else {
            return Ok(());
        };

        let ack_token = match self.ack_obligations.mint_ack_run_token(agent_id) {
            Ok(token) => token,
            Err(error) => {
                let error = error.to_string();
                report_handoff_resume_error(
                    &self.telemetry_logs,
                    self.trays.as_ref(),
                    agent_id,
                    None,
                    error_title,
                    &error,
                );
                return Err(error);
            }
        };
        let stream_id = format!("handoff-resume-{}", uuid::Uuid::new_v4());
        let worker_agent_id = agent_id.to_string();
        let worker_stream_id = stream_id.clone();
        let worker_prompt = prompt;
        let worker_error_title = error_title.to_string();
        let worker_ack_token = ack_token.clone();
        let worker_ack_obligations = Arc::clone(&self.ack_obligations);
        let worker_roster = Arc::clone(&self.roster_emit);
        let worker_logs = self.telemetry_logs.clone();
        let worker_trays = Arc::clone(&self.trays);
        let deps = self.local_routed_runner_deps();

        match thread::Builder::new()
            .name("mahayana-handoff-resume".into())
            .spawn(move || {
                let result = run_local_background_revival_turn_with_context(
                    deps,
                    provider,
                    &worker_agent_id,
                    "handoff-resume",
                    &worker_prompt,
                    false,
                    "handoff-resume",
                    BackgroundRevivalContext {
                        stream_id: Some(worker_stream_id.clone()),
                        ack_token: worker_ack_token.clone(),
                        ..BackgroundRevivalContext::default()
                    },
                );
                let _ = worker_roster.emit_agent_update(&worker_agent_id);
                if let Err(error) = result {
                    report_handoff_resume_error(
                        &worker_logs,
                        worker_trays.as_ref(),
                        &worker_agent_id,
                        Some(&worker_stream_id),
                        &worker_error_title,
                        &error,
                    );
                }
                worker_ack_obligations
                    .retire_ack_run_token(&worker_agent_id, worker_ack_token.as_deref());
            })
        {
            Ok(_) => Ok(()),
            Err(error) => {
                self.ack_obligations
                    .retire_ack_run_token(agent_id, ack_token.as_deref());
                let error = format!("could not start handoff resume worker: {error}");
                report_handoff_resume_error(
                    &self.telemetry_logs,
                    self.trays.as_ref(),
                    agent_id,
                    Some(&stream_id),
                    error_title,
                    &error,
                );
                Err(error)
            }
        }
    }

    fn resume_after_listener_connect(&self, agent_id: &str, platform: &str) -> Result<(), String> {
        self.resume_with_hidden_handoff(
            agent_id,
            listener_connect_resume_prompt(platform),
            LISTENER_CONNECT_RESUME_TITLE,
        )
    }

    fn resume_after_mcp_auth(
        &self,
        agent_id: &str,
        server_name: &str,
        account_key: &str,
    ) -> Result<(), String> {
        let display_name = format_mcp_account_display_name(server_name, account_key);
        self.resume_with_hidden_handoff(
            agent_id,
            mcp_auth_resume_prompt(&display_name),
            MCP_AUTH_RESUME_TITLE,
        )
    }

    fn dispatch_production_listener_event(&self, event: serde_json::Value) -> bool {
        let Some(provider) = configured_routed_provider(&self.data_dir.join("settings.json")) else {
            return false;
        };
        if provider == RoutedProvider::Cursor {
            // Fail closed: do not ACK backend relay delivery until the Cursor
            // background Runner path is owned by the Rust Host.
            return false;
        }

        let runtime = self.transcript_manager.automation_runtime();
        let Ok(entries) = runtime.list_all_automation_definitions() else {
            return false;
        };
        let lifecycle = self
            .automations_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .upgrade();
        let matches = entries
            .into_iter()
            .filter(|entry| entry.automation.is_enabled)
            .filter(|entry| {
                let cloud = ScheduledCloudAutomation {
                    id: entry.automation.id.clone(),
                    name: entry.automation.name.clone(),
                    prompt: entry.automation.prompt.clone(),
                    is_enabled: entry.automation.is_enabled,
                    trigger: entry.automation.trigger.clone(),
                };
                lifecycle
                    .as_ref()
                    .map(|owner| owner.should_schedule_locally(&entry.agent_id, &cloud))
                    .unwrap_or_else(|| !is_server_schedulable(&entry.automation.trigger))
            })
            .filter(|entry| trigger_matches_event(&entry.automation.trigger, &event, true, false))
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return false;
        }

        let mut accepted = 0usize;
        for entry in matches {
            let runtime = Arc::clone(&runtime);
            let deps = self.local_routed_runner_deps();
            let event = event.clone();
            let agent_id = entry.agent_id;
            let automation_id = entry.automation.id;
            let automation_name = entry.automation.name;
            let spawn = thread::Builder::new()
                .name("mahayana-automation-listener-fire".into())
                .spawn(move || {
                    let execute_deps = deps.clone();
                    let execute_agent_id = agent_id.clone();
                    let execute_automation_id = automation_id.clone();
                    let execute_automation_name = automation_name.clone();
                    let executor = Arc::new(move |_agent_id: &str, _automation_id: &str, _automation_name: &str, prompt: &str| {
                        run_local_automation_turn(
                            execute_deps.clone(),
                            provider,
                            &execute_agent_id,
                            &execute_automation_id,
                            &execute_automation_name,
                            prompt,
                        )
                    });
                    if let Err(error) = runtime.enqueue_event_automation_fire_with(
                        &agent_id,
                        &automation_id,
                        event,
                        None,
                        executor,
                    ) {
                        eprintln!(
                            "[sand:automations] listener fire failed agent={agent_id} automation={automation_id}: {error}"
                        );
                    }
                });
            if spawn.is_ok() {
                accepted += 1;
            }
        }
        accepted > 0
    }

    fn report_production_backend_fire_dropped(
        &self,
        fire: &BackendAutomationFire,
        reason: &str,
        error_type: Option<&str>,
        error_code: Option<&str>,
    ) {
        let now_ms = started_at_ms();
        let scheduled_for_ms = fire.scheduled_for_ms.map(|value| value as f64);
        let lateness_ms = fire
            .scheduled_for_ms
            .map(|value| now_ms.saturating_sub(value) as f64);
        let report = AutomationFireDroppedReport {
            conversation_id: fire.sand_agent_id.clone(),
            trigger: if fire.event.is_some() { "event".into() } else { "schedule".into() },
            reason: reason.to_owned(),
            scheduled_for_ms,
            lateness_ms,
            error_type: error_type.map(str::to_owned),
            error_code: error_code.map(str::to_owned),
            run_uuid: Some(fire.id.clone()),
            fire_age_ms: Some(now_ms.saturating_sub(fire.timestamp_ms) as f64),
            has_definition_revision: Some(fire.definition_revision.is_some()),
            box_uptime_ms: None,
        };
        let _ = self.telemetry_logs.report_automation_fire_dropped(&report);
    }

    fn dispatch_production_backend_fire(
        &self,
        fire: BackendAutomationFire,
        completion: Arc<dyn Fn(Option<FireCompletion>) + Send + Sync>,
    ) -> bool {
        let runtime = self.transcript_manager.automation_runtime();
        let Ok(entries) = runtime.list_all_automations() else {
            return false;
        };
        let definition_time_zone = self.session_workers.resolve_user_time_zone();
        match prepare_backend_fire(&entries, &fire, |entry| {
            let cloud = ScheduledCloudAutomation {
                id: entry.automation.id.clone(),
                name: entry.automation.name.clone(),
                prompt: entry.automation.prompt.clone(),
                is_enabled: entry.automation.is_enabled,
                trigger: entry.automation.trigger.clone(),
            };
            sand_cloud_definition(
                &entry.agent_id,
                &cloud,
                definition_time_zone.as_deref(),
            )
            .map(|definition| definition.hash)
        }) {
            PreparedBackendFire::Complete {
                completion: terminal,
                reason,
            } => {
                if reason != "existing_run" {
                    self.report_production_backend_fire_dropped(&fire, reason, None, None);
                }
                completion(Some(terminal));
                true
            }
            PreparedBackendFire::Abandon { .. } => false,
            PreparedBackendFire::Schedule {
                agent_id,
                automation_id,
                run_uuid,
                scheduled_for_ms,
            } => {
                let Some(provider) =
                    configured_routed_provider(&self.data_dir.join("settings.json"))
                else {
                    return false;
                };
                if provider == RoutedProvider::Cursor {
                    return false;
                }
                let Some(automation) = entries
                    .iter()
                    .find(|entry| {
                        entry.agent_id == agent_id && entry.automation.id == automation_id
                    })
                    .map(|entry| entry.automation.clone())
                else {
                    return false;
                };
                let deps = self.local_routed_runner_deps();
                thread::Builder::new()
                    .name("mahayana-automation-cloud-fire".into())
                    .spawn(move || {
                        let result = runtime.run_server_scheduled_automation_with(
                            &agent_id,
                            &automation_id,
                            run_uuid,
                            scheduled_for_ms.map(|value| value as f64),
                            |prompt| {
                                run_local_automation_turn(
                                    deps,
                                    provider,
                                    &agent_id,
                                    &automation_id,
                                    &automation.name,
                                    prompt,
                                )
                            },
                        );
                        completion(automation_fire_completion(result));
                    })
                    .is_ok()
            }
            PreparedBackendFire::Event {
                agent_id,
                automation_id,
                run_uuid,
                event,
            } => {
                let Some(provider) =
                    configured_routed_provider(&self.data_dir.join("settings.json"))
                else {
                    return false;
                };
                if provider == RoutedProvider::Cursor {
                    return false;
                }
                let Some(automation) = entries
                    .iter()
                    .find(|entry| {
                        entry.agent_id == agent_id && entry.automation.id == automation_id
                    })
                    .map(|entry| entry.automation.clone())
                else {
                    return false;
                };
                let deps = self.local_routed_runner_deps();
                thread::Builder::new()
                    .name("mahayana-automation-cloud-event".into())
                    .spawn(move || {
                        let execute_deps = deps.clone();
                        let execute_agent_id = agent_id.clone();
                        let execute_automation_id = automation_id.clone();
                        let execute_automation_name = automation.name.clone();
                        let executor = Arc::new(move |_agent_id: &str, _automation_id: &str, _automation_name: &str, prompt: &str| {
                            run_local_automation_turn(
                                execute_deps.clone(),
                                provider,
                                &execute_agent_id,
                                &execute_automation_id,
                                &execute_automation_name,
                                prompt,
                            )
                        });
                        let result = runtime.enqueue_event_automation_fire_with(
                            &agent_id,
                            &automation_id,
                            event,
                            Some(run_uuid),
                            executor,
                        );
                        completion(automation_fire_completion(result));
                    })
                    .is_ok()
            }
        }
    }

    fn resume_interrupted_upgrade_turns(&self) -> Result<(), String> {
        if !self.transcript_manager.can_execute() {
            return Ok(());
        }
        let Some(store) = self.transcript_runtime.upgrade_resume_store() else {
            return Ok(());
        };
        let pending = store.list_pending();
        if pending.is_empty() {
            return Ok(());
        }

        for marker in pending {
            if !upgrade_resume_marker_identity_is_well_formed(&marker) {
                eprintln!(
                    "mahayana-host upgrade_resume_identity_invalid agent={}",
                    marker.agent_id
                );
                continue;
            }
            let Some(summary) = self
                .session_workers
                .summarize_agent_by_id(&marker.agent_id, None)?
            else {
                let _ = store.clear_if_matches(&marker);
                continue;
            };
            if summary.is_group {
                let _ = store.clear_if_matches(&marker);
                continue;
            }

            let resumed_source = match marker.source.as_deref() {
                Some("notification") => "background-revival",
                Some(source) if !source.trim().is_empty() => source,
                _ => "handoff-resume",
            };
            let automation_wake = if resumed_source == "automation" {
                marker.automation_id.as_deref().and_then(|automation_id| {
                    self.transcript_manager
                        .automation_runtime()
                        .get_agent_automations(&marker.agent_id)
                        .ok()?
                        .into_iter()
                        .find(|automation| automation.id == automation_id)
                        .map(|automation| serde_json::json!({
                            "id": automation.id,
                            "name": automation.name,
                        }))
                })
            } else {
                None
            };

            let provider = configured_routed_provider(&self.data_dir.join("settings.json"));
            let result = match provider {
                Some(provider) if provider != RoutedProvider::Cursor => {
                    start_local_upgrade_resume_turn(
                        self.local_routed_runner_deps(),
                        provider,
                        &marker,
                        automation_wake,
                    )
                }
                _ => {
                    let session_key = if let Some(automation_id) = marker.automation_id.as_deref() {
                        resolve_admitted_automation_session_key(
                            &self.session_workers,
                            &marker.agent_id,
                            automation_id,
                        )?
                    } else {
                        self.session_workers
                            .read_session_work_admission(&marker.agent_id)?
                            .map(|record| record.session_key)
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| {
                                format!(
                                    "upgrade resume requires an existing admitted session identity for {}",
                                    marker.agent_id
                                )
                            })?
                    };
                    let mut args = serde_json::json!({
                        "agentId": marker.agent_id,
                        "sessionKey": session_key,
                        "prompt": build_upgrade_resume_prompt(resumed_source),
                        "clientNonce": format!(
                            "upgrade-resume:{}:{}",
                            marker.agent_id,
                            uuid::Uuid::new_v4()
                        ),
                        "appendUserMessage": false,
                        "hidden": true,
                        "requestSource": resumed_source,
                        "upgradeResume": true,
                        "skipAckObligation": true,
                        "awaitTurn": false,
                    });
                    if let Some(automation_wake) = automation_wake {
                        args["automationWake"] = automation_wake;
                    }
                    self.call("sendPrompt", args)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                }
            };

            if let Err(error) = result {
                let classified_error = ProviderSessionError::Tool(error.clone());
                let report = AgentErrorReport {
                    source: "resume".into(),
                    conversation_id: marker.agent_id.clone(),
                    request_id: None,
                    error: classify_agent_error(&classified_error),
                    detail: Some(sand_error_detail(&error)),
                };
                if let Err(telemetry_error) = self.telemetry_logs.report_agent_error(&report) {
                    eprintln!(
                        "mahayana-host upgrade_resume_telemetry_failed agent={} error={telemetry_error}",
                        marker.agent_id
                    );
                }
                let mut tray =
                    provider_failure_tray(&marker.agent_id, &error, started_at_ms() as i64);
                tray.title = "Agent failed to resume after host update".into();
                self.trays.push_error(tray);
                eprintln!(
                    "mahayana-host upgrade_resume_failed agent={} error={error}",
                    marker.agent_id
                );
            } else {
                let _ = store.clear_if_matches(&marker);
            }
        }
        Ok(())
    }

    fn preempt_group_member_runs_for_direct_send(&self, args: &serde_json::Value) -> bool {
        let request_source = args
            .get("requestSource")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let is_direct = matches!(request_source, None | Some("turn"))
            && args.get("automationWake").is_none_or(serde_json::Value::is_null)
            && args.get("groupContext").is_none_or(serde_json::Value::is_null);
        if !is_direct {
            return false;
        }
        let Some(agent_id) = args
            .get("agentId")
            .or_else(|| args.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return false;
        };
        self.runner_registry.preempt_group_member_agent(
            agent_id,
            "direct user message preempted group member turn",
        ) > 0
    }

    fn persisted_user_entry(
        &self,
        agent_id: &str,
        context: &PersistedSendContext,
    ) -> Result<Option<serde_json::Value>, ProductionSendError> {
        let Some(user_message_id) = context.user_message_id.as_deref() else {
            return Ok(None);
        };
        Ok(self
            .session_workers
            .read_agent_transcript_entries(agent_id)
            .map_err(ProductionSendError::Internal)?
            .into_iter()
            .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(user_message_id)))
    }

    fn dispatch_mirror_or_group_send_if_supported(
        &self,
        args: &serde_json::Value,
        shaped_args: &serde_json::Value,
        context: &PersistedSendContext,
    ) -> Result<Option<serde_json::Value>, ProductionSendError> {
        let Some(agent_id) = args.get("agentId").or_else(|| args.get("id"))
            .and_then(serde_json::Value::as_str).map(str::trim).filter(|value| !value.is_empty())
        else { return Ok(None); };
        let agent_dir = self.session_workers.agents_root().join(agent_id);

        if let Some(remote_room) =
            mahayana_host_runtime::groups::remote_room_store::read_sand_remote_room_config(&agent_dir)
        {
            if remote_room.is_revoked == Some(true) {
                return Err(ProductionSendError::Rejected("This shared room is no longer active.".into()));
            }
            let has_files = shaped_args.get("attachmentPaths").and_then(serde_json::Value::as_array)
                .is_some_and(|values| !values.is_empty());
            let has_videos = shaped_args.get("selectedVideos").and_then(serde_json::Value::as_array)
                .is_some_and(|values| !values.is_empty());
            if has_files || has_videos {
                return Err(ProductionSendError::BadRequest(
                    "Shared mirror rooms only support image attachments.".into(),
                ));
            }
            let entry = self.persisted_user_entry(agent_id, context)?.ok_or_else(|| {
                ProductionSendError::Internal("mirror-room send is missing its durable user entry".into())
            })?;
            self.cross_user.publish_room_entry_and_wait(&remote_room.room_id, &entry)
                .map_err(ProductionSendError::Internal)?;
            return Ok(Some(serde_json::json!({"accepted": true, "mirrorRoom": true})));
        }

        let Some(group_config) =
            mahayana_host_runtime::groups::group_store::read_sand_group_config(&agent_dir)
        else { return Ok(None); };
        if let Some(shared_room_id) = group_config.shared_room_id.as_deref() {
            if let Some(entry) = self.persisted_user_entry(agent_id, context)? {
                self.cross_user.publish_room_entry_and_wait(shared_room_id, &entry)
                    .map_err(ProductionSendError::Internal)?;
            }
        }
        let Some(provider) = configured_routed_provider(&self.data_dir.join("settings.json")) else {
            return Ok(None);
        };
        let deps = self.local_routed_runner_deps();
        let member_room_id = agent_id.to_string();
        let executor: GroupMemberTurnExecutor = Arc::new(move |request| {
            run_local_group_member_turn(deps.clone(), provider, Some(&member_room_id), request)
        });
        let epoch = self.transcript_runtime.current_turn_epoch(agent_id);
        match self.transcript_manager.group_chat().run_group_turn(
            Arc::clone(&self.transcript_runtime),
            agent_id,
            epoch,
            executor,
            self.cross_user.remote_executor(),
            Some(group_room_entry_observer(self.events.clone())),
        ).map_err(ProductionSendError::Internal)? {
            LocalGroupFanoutDisposition::NotGroup | LocalGroupFanoutDisposition::DeferredRemote { .. } => Ok(None),
            LocalGroupFanoutDisposition::Completed { posted_messages, member_failures } => Ok(Some(serde_json::json!({
                "accepted": true,
                "groupFanout": true,
                "postedMessages": posted_messages,
                "memberFailureCount": member_failures.len(),
            }))),
        }
    }

    fn call_openclaw_gateway_command(
        &self,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, GatewayCommandError> {
        match method {
            "device.pair.request"
            | "device.pair.status"
            | "device.pair.approve"
            | "device.pair.reject"
            | "device.pair.list"
            | "device.pair.revoke"
            | "device.pair.rotateToken"
            | "device.pair.setupCode" => self.device_pairing.handle_pairing_command(method, &args),
            "config.get" => call_openclaw_config_get(self, &args),
            "config.set" => call_openclaw_config_set(self, &args),
            "system.info" => call_openclaw_system_info(self, &args),
            "sessions.list" => call_openclaw_sessions_list(self, &args),
            "sessions.get" => call_openclaw_sessions_get(self, &args),
            "sessions.create" => call_openclaw_sessions_create(self, &args),
            "sessions.patch" => Err(GatewayCommandError::Internal(
                "sessions.patch requires the authenticated Gateway request context".into(),
            )),
            "sessions.providerReview.continue" => Err(GatewayCommandError::Internal(
                "sessions.providerReview.continue requires the authenticated Gateway request context".into(),
            )),
            "sessions.delete" => call_openclaw_sessions_delete(self, &args),
            "chat.send" => call_openclaw_chat_send(self, args),
            "chat.abort" => call_openclaw_chat_abort(self, &args),
            "cron.list" => call_openclaw_cron_list(self, &args),
            "cron.trigger" => call_openclaw_cron_trigger(self, &args),
            "node.register" => call_openclaw_node_register(self, &args),
            "node.list" => call_openclaw_node_list(self, &args),
            "node.status" => call_openclaw_node_status(self, &args),
            "node.invoke" => call_openclaw_node_invoke(self, &args),
            _ => Err(GatewayCommandError::UnknownMethod(method.to_string())),
        }
    }

    pub(crate) fn delete_generated_subagent(
        &self,
        agent_id: &str,
        lifecycle_revision: &str,
    ) -> Result<(), String> {
        delete_generated_subagent(self, agent_id, lifecycle_revision)
    }

}

#[derive(Debug, Default)]
struct BackgroundRevivalContext {
    selected_images: Vec<serde_json::Value>,
    skipped_question_prompts: Vec<String>,
    dismissed_question_prompts: Vec<String>,
    stream_id: Option<String>,
    ack_token: Option<String>,
}

fn run_local_background_revival_turn(
    deps: LocalRoutedRunnerDeps,
    provider: RoutedProvider,
    agent_id: &str,
    source: &str,
    prompt: &str,
    is_silence_allowed: bool,
    auto_review_epoch: &str,
) -> Result<RevivalExecution, String> {
    run_local_background_revival_turn_with_context(
        deps,
        provider,
        agent_id,
        source,
        prompt,
        is_silence_allowed,
        auto_review_epoch,
        BackgroundRevivalContext::default(),
    )
}

fn run_local_background_revival_turn_with_context(
    deps: LocalRoutedRunnerDeps,
    provider: RoutedProvider,
    agent_id: &str,
    source: &str,
    prompt: &str,
    is_silence_allowed: bool,
    auto_review_epoch: &str,
    context: BackgroundRevivalContext,
) -> Result<RevivalExecution, String> {
    let BackgroundRevivalContext {
        selected_images,
        skipped_question_prompts,
        dismissed_question_prompts,
        stream_id,
        ack_token,
    } = context;
    let stream_id =
        stream_id.unwrap_or_else(|| format!("background-revival-{}", uuid::Uuid::new_v4()));
    let client_nonce = format!(
        "background-revival:{}:{}",
        agent_id,
        uuid::Uuid::new_v4()
    );
    let before = deps
        .session_workers
        .read_agent_transcript_entries(agent_id)
        .unwrap_or_default();
    let session_key = deps
        .session_workers
        .read_session_work_admission(agent_id)?
        .map(|record| record.session_key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "background revival requires an existing admitted session identity for {agent_id}"
            )
        })?;
    let admitted_lifecycle_revision = deps
        .session_workers
        .capture_session_work_admission(agent_id, &session_key, &stream_id)?;
    let mut admission_args = serde_json::json!({
        "agentId": agent_id,
        "sessionKey": session_key,
        "prompt": prompt,
        "clientNonce": client_nonce,
        "streamId": stream_id,
        "appendUserMessage": false,
        "hidden": true,
        "requestSource": source,
        "skipAckObligation": true,
    });
    if let Some(ack_token) = ack_token.as_deref() {
        admission_args["ackToken"] = serde_json::Value::String(ack_token.to_string());
    }
    if let Err(error) = deps.session_workers.assert_session_work_admission(
        agent_id,
        admission_args.get("sessionKey").and_then(serde_json::Value::as_str),
    ) {
        let _ = deps.session_workers.release_session_work_admission(
            agent_id,
            &admitted_lifecycle_revision,
            &stream_id,
        );
        return Err(error);
    }
    if let Err(error) = deps.transcript_runtime.accept_routed_send(&admission_args, |_| {
        Ok::<_, ProductionSendError>(PersistedSendContext::default())
    }) {
        let _ = deps.session_workers.release_session_work_admission(
            agent_id,
            &admitted_lifecycle_revision,
            &stream_id,
        );
        return Err(error.to_string());
    }

    let receiver = deps.events.subscribe();
    let mut runner_args = serde_json::json!({
        "provider": provider.as_str(),
        "agentId": agent_id,
        "sessionKey": admission_args.get("sessionKey").cloned().unwrap_or_default(),
        "streamId": stream_id,
        "requestSource": source,
        "hidden": true,
        "isSilenceAllowed": is_silence_allowed,
        "autoReviewEpoch": auto_review_epoch,
        "messages": [{
            "role": "user",
            "content": prompt,
        }],
    });
    if let Some(ack_token) = ack_token {
        runner_args["ackToken"] = serde_json::Value::String(ack_token);
    }
    if !selected_images.is_empty() {
        runner_args["selectedImages"] = serde_json::Value::Array(selected_images);
    }
    if !skipped_question_prompts.is_empty() {
        runner_args["skippedQuestionPrompts"] = serde_json::Value::Array(
            skipped_question_prompts
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        );
    }
    if !dismissed_question_prompts.is_empty() {
        runner_args["dismissedQuestionPrompts"] = serde_json::Value::Array(
            dismissed_question_prompts
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        );
    }
    if let Err(error) = start_routed_provider_task(
        deps.routed_tool_relay,
        deps.mcp_service,
        deps.events.clone(),
        deps.host_tx,
        deps.data_dir,
        deps.request_context,
        deps.auth,
        deps.auto_review,
        deps.experiments,
        deps.settings,
        deps.inference,
        Arc::clone(&deps.session_workers),
        Arc::clone(&deps.runner_registry),
        deps.ack_obligations,
        deps.transcript_runtime,
        deps.transcript_manager,
        deps.generated_agent_runtime,
        deps.completion_revivals,
        deps.forever_box,
        deps.local_exec,
        deps.local_tool_permission,
        deps.session_handoff,
        deps.trays,
        deps.telemetry_logs,
        deps.product_analytics,
        deps.production_action_auditor,
        deps.cloud_agents,
        deps.cloud_agent_watches,
        deps.background_shell_watches,
        deps.host_runner_composition,
        deps.box_store_sync,
        Arc::clone(&deps.automations_lifecycle),
        Arc::clone(&deps.generated_subagent_cleanup),
        None,
        runner_args,
    )
    .map_err(|error| error.to_string()) {
        let _ = deps.session_workers.release_session_work_admission(
            agent_id,
            &admitted_lifecycle_revision,
            &stream_id,
        );
        return Err(error);
    }

    let deadline = Instant::now() + Duration::from_secs(30 * 60);
    loop {
        let now = Instant::now();
        if now >= deadline {
            let _ = deps
                .runner_registry
                .cancel_stream(&stream_id, "background revival timed out");
            let _ = deps.session_workers.release_session_work_admission(
                agent_id,
                &admitted_lifecycle_revision,
                &stream_id,
            );
            return Ok(RevivalExecution {
                aborted: true,
                quiesced_for_upgrade: false,
                sent_message_count: 0,
            });
        }
        let wait = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(250));
        match receiver.recv_timeout(wait) {
            Ok(event) => {
                let Some(result) = automation_terminal_from_event(&event, &stream_id) else {
                    continue;
                };
                match result {
                    Ok(AutomationExecutionResult::Completed)
                    | Ok(AutomationExecutionResult::CompletedWithFences { .. }) => {
                        deps.session_workers
                            .assert_session_work_admission_generation(
                                agent_id,
                                admission_args
                                    .get("sessionKey")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or_default(),
                                &admitted_lifecycle_revision,
                            )?;
                        let terminal_content = event
                            .get("payload")
                            .and_then(serde_json::Value::as_object)
                            .and_then(|payload| payload.get("content"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        let after = deps
                            .session_workers
                            .read_agent_transcript_entries(agent_id)
                            .unwrap_or_default();
                        let sent_message_count = collect_new_member_send_messages(&before, &after)
                            .len();
                        let receipt = deps.session_workers.commit_session_work_result(
                            agent_id,
                            admission_args
                                .get("sessionKey")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default(),
                            &admitted_lifecycle_revision,
                            &stream_id,
                            &client_nonce,
                            terminal_content,
                            sent_message_count,
                        )?;
                        let sent_message_count = match receipt {
                            mahayana_host_runtime::extensions::session::agent_db::SessionWorkResultCommitOutcome::Committed(
                                receipt,
                            )
                            | mahayana_host_runtime::extensions::session::agent_db::SessionWorkResultCommitOutcome::AlreadyCommitted(
                                receipt,
                            ) => receipt.sent_message_count,
                            mahayana_host_runtime::extensions::session::agent_db::SessionWorkResultCommitOutcome::RejectedStale => {
                                return Err("background result generation became stale before receipt commit".into());
                            }
                            mahayana_host_runtime::extensions::session::agent_db::SessionWorkResultCommitOutcome::ConflictingResult => {
                                return Err("background result idempotency key conflicts with a prior receipt".into());
                            }
                        };
                        return Ok(RevivalExecution {
                            aborted: false,
                            quiesced_for_upgrade: false,
                            sent_message_count,
                        });
                    }
                    Ok(AutomationExecutionResult::Interrupted { .. })
                    | Ok(AutomationExecutionResult::InterruptedWithFences { .. }) => {
                        let _ = deps.session_workers.release_session_work_admission(
                            agent_id,
                            &admitted_lifecycle_revision,
                            &stream_id,
                        );
                        return Ok(RevivalExecution {
                            aborted: true,
                            quiesced_for_upgrade: false,
                            sent_message_count: 0,
                        });
                    }
                    Ok(AutomationExecutionResult::Failed { detail })
                    | Ok(AutomationExecutionResult::FailedWithFences { detail, .. }) => {
                        let _ = deps.session_workers.release_session_work_admission(
                            agent_id,
                            &admitted_lifecycle_revision,
                            &stream_id,
                        );
                        return Err(detail);
                    }
                    Err(error) => {
                        let _ = deps.session_workers.release_session_work_admission(
                            agent_id,
                            &admitted_lifecycle_revision,
                            &stream_id,
                        );
                        return Err(error);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = deps.session_workers.release_session_work_admission(
                    agent_id,
                    &admitted_lifecycle_revision,
                    &stream_id,
                );
                return Err("Host event bus disconnected during background revival".into());
            }
        }
    }
}

fn append_channel_inbound_entries(
    deps: &LocalRoutedRunnerDeps,
    roster: &ProductionRosterEmit,
    agent_id: &str,
    envelopes: &[serde_json::Value],
) -> Result<(), String> {
    let session_key = deps
        .session_workers
        .read_session_work_admission(agent_id)?
        .map(|record| record.session_key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "channel inbound requires an existing admitted session identity for {agent_id}"
            )
        })?;
    deps.session_workers
        .assert_session_work_admission(agent_id, Some(&session_key))?;
    let mut transcript = deps
        .session_workers
        .read_agent_transcript_entries(agent_id)
        .unwrap_or_default();
    let mut entries = Vec::new();
    for envelope in envelopes.iter().filter(|envelope| {
        envelope.get("isDisplayed").and_then(serde_json::Value::as_bool) != Some(true)
    }) {
        let channel = envelope
            .get("address")
            .and_then(format_channel_address_value)
            .unwrap_or_default();
        let entry = serde_json::json!({
            "kind": "message",
            "id": next_entry_id(&transcript, TranscriptEntryIdKind::UserMessage),
            "role": "user",
            "content": envelope.get("text").and_then(serde_json::Value::as_str).unwrap_or_default(),
            "isStreaming": false,
            "timestampMs": envelope.get("timestampMs").cloned().unwrap_or_else(|| serde_json::Value::Number(started_at_ms().into())),
            "channel": channel,
            "channelSender": envelope.get("sender").cloned().unwrap_or(serde_json::Value::Null),
            "sessionKey": session_key.clone(),
        });
        transcript.push(entry.clone());
        entries.push(entry);
    }
    if entries.is_empty() {
        return Ok(());
    }
    deps.session_workers
        .append_agent_transcript_entries_if_session_admitted(agent_id, &session_key, &entries)?;
    let _ = deps
        .session_workers
        .mark_agent_activity(agent_id, started_at_ms() as f64);
    let _ = roster.emit_agent_update(agent_id);
    Ok(())
}

fn requeue_channel_inbound_envelopes(
    wakes: &Arc<Mutex<BackgroundWakes<serde_json::Value>>>,
    agent_id: &str,
    envelopes: Vec<serde_json::Value>,
) {
    let mut state = wakes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let pending = state.pending_inbound.remove(agent_id).unwrap_or_default();
    state.pending_inbound.insert(
        agent_id.to_string(),
        envelopes.into_iter().chain(pending).collect(),
    );
}

fn publish_channel_activity(
    events: &GatewayEventHub,
    agent_id: &str,
    addresses: &[String],
    is_active: bool,
) {
    for address in addresses {
        events.publish(serde_json::json!({
            "channel": "channel-activity",
            "payload": {
                "agentId": agent_id,
                "address": address,
                "isActive": is_active,
            }
        }));
    }
}

fn run_channel_inbound_revival_worker(
    deps: LocalRoutedRunnerDeps,
    roster: Arc<ProductionRosterEmit>,
    wakes: Arc<Mutex<BackgroundWakes<serde_json::Value>>>,
    agent_id: String,
) {
    loop {
        let envelopes = {
            let mut wakes = wakes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            BackgroundWakes::take_pending(&mut wakes.pending_inbound, &agent_id)
        };
        if envelopes.is_empty() {
            break;
        }

        let summary = match deps.session_workers.summarize_agent_by_id(&agent_id, None) {
            Ok(Some(summary)) => summary,
            Ok(None) => break,
            Err(error) => {
                eprintln!(
                    "mahayana-host channel_inbound_summary_failed agent={agent_id} error={error}"
                );
                requeue_channel_inbound_envelopes(&wakes, &agent_id, envelopes);
                break;
            }
        };
        if summary.is_group || summary.remote_room.is_some() {
            continue;
        }
        let Some(provider) = configured_routed_provider(&deps.data_dir.join("settings.json")) else {
            let mut state = wakes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let pending = state.pending_inbound.remove(&agent_id).unwrap_or_default();
            state.pending_inbound.insert(
                agent_id.clone(),
                envelopes.into_iter().chain(pending).collect(),
            );
            break;
        };
        if deps.transcript_runtime.is_quiescing_for_upgrade() {
            let mut state = wakes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let pending = state.pending_inbound.remove(&agent_id).unwrap_or_default();
            state.pending_inbound.insert(
                agent_id.clone(),
                envelopes.into_iter().chain(pending).collect(),
            );
            break;
        }

        if let Err(error) = append_channel_inbound_entries(&deps, roster.as_ref(), &agent_id, &envelopes) {
            eprintln!("mahayana-host channel_inbound_persist_failed agent={agent_id} error={error}");
            continue;
        }

        let addresses = distinct_inbound_channel_addresses(&envelopes);
        publish_channel_activity(&deps.events, &agent_id, &addresses, true);
        let prompt = build_channel_inbound_wake_prompt(&envelopes);
        let widget_prompts = deps
            .transcript_manager
            .widget_responses()
            .collect_unanswered_question_prompts(&agent_id)
            .unwrap_or_default();
        let selected_images = collect_inbound_images(&envelopes)
            .into_iter()
            .map(|image| {
                serde_json::json!({
                    "data": image.data,
                    "mimeType": image.mime_type,
                })
            })
            .collect();
        let result = run_local_background_revival_turn_with_context(
            deps.clone(),
            provider,
            &agent_id,
            "connector",
            &prompt,
            false,
            "continue",
            BackgroundRevivalContext {
                selected_images,
                skipped_question_prompts: widget_prompts.skipped_question_prompts,
                dismissed_question_prompts: widget_prompts.dismissed_question_prompts,
                ..BackgroundRevivalContext::default()
            },
        );
        publish_channel_activity(&deps.events, &agent_id, &addresses, false);

        match result {
            Ok(execution) if execution.aborted => {
                let preempted = {
                    let mut state = wakes
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.dm_preempted_wake_agent_ids.remove(&agent_id)
                };
                if preempted {
                    let redrivable = redrivable_inbound_envelopes(&envelopes);
                    if !redrivable.is_empty() {
                        let mut state = wakes
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let pending = state.pending_inbound.remove(&agent_id).unwrap_or_default();
                        state.pending_inbound.insert(
                            agent_id.clone(),
                            redrivable.into_iter().chain(pending).collect(),
                        );
                    }
                }
            }
            Ok(execution) => {
                if execution.sent_message_count == 0 {
                    let _ = run_local_background_revival_turn(
                        deps.clone(),
                        provider,
                        &agent_id,
                        "connector",
                        REPLY_NUDGE_PROMPT,
                        false,
                        "continue",
                    );
                }
                let _ = roster.emit_agent_update(&agent_id);
            }
            Err(error) => {
                let classified_error = ProviderSessionError::Tool(error.clone());
                let report = AgentErrorReport {
                    source: "connector".into(),
                    conversation_id: agent_id.clone(),
                    request_id: None,
                    error: classify_agent_error(&classified_error),
                    detail: Some(sand_error_detail(&error)),
                };
                if let Err(telemetry_error) = deps.telemetry_logs.report_agent_error(&report) {
                    eprintln!(
                        "mahayana-host channel_inbound_telemetry_failed agent={} error={telemetry_error}",
                        agent_id
                    );
                }
                let mut tray = provider_failure_tray(&agent_id, &error, started_at_ms() as i64);
                tray.title = "Channel message follow-up failed".into();
                deps.trays.push_error(tray);
            }
        }
    }
    let mut state = wakes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    BackgroundWakes::<serde_json::Value>::end_revival(
        &mut state.reviving_inbound_agent_ids,
        &agent_id,
    );
}

fn requeue_channel_failures(
    wakes: &Arc<Mutex<BackgroundWakes<serde_json::Value>>>,
    agent_id: &str,
    failures: Vec<serde_json::Value>,
) {
    let mut state = wakes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let pending = state.pending_channel_failures.remove(agent_id).unwrap_or_default();
    state.pending_channel_failures.insert(
        agent_id.to_string(),
        failures.into_iter().chain(pending).collect(),
    );
}

fn clear_durable_channel_failure_markers(
    deps: &LocalRoutedRunnerDeps,
    agent_id: &str,
    failures: &[serde_json::Value],
) {
    let Some(store) = deps.transcript_runtime.pending_wake_store() else {
        return;
    };
    for failure in failures {
        let Some(marker) = channel_failure_marker_from_failure(agent_id, failure) else {
            eprintln!(
                "mahayana-host channel_failure_marker_identity_incomplete agent={agent_id}"
            );
            continue;
        };
        if !store.clear_if_matches(&marker) {
            eprintln!(
                "mahayana-host channel_failure_marker_settlement_missed agent={} operation={}",
                agent_id, marker.work_id
            );
        }
    }
}

fn channel_failure_marker_from_failure(
    agent_id: &str,
    failure: &serde_json::Value,
) -> Option<DurablePendingWakeMarker> {
    let work_id = failure
        .get("deliveryOperationId")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())?;
    let marked_at_ms = failure
        .get("markedAtMs")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite())?;
    let address_token = failure
        .get("addressToken")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())?;
    let failure_reason = failure
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())?;
    let session_key = failure
        .get("sessionKey")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned);
    Some(DurablePendingWakeMarker {
        agent_id: agent_id.to_string(),
        kind: PendingWakeKind::ChannelDelivery,
        work_id: work_id.to_string(),
        marked_at_ms,
        quiet_origin: None,
        title: Some("Channel delivery failure".into()),
        subagent_type: None,
        interrupted_by_recreate: false,
        delivery_operation_id: Some(work_id.to_string()),
        address_token: Some(address_token.to_string()),
        failure_reason: Some(failure_reason.to_string()),
        session_key,
    })
}

fn filter_current_channel_failure_provenance(
    deps: &LocalRoutedRunnerDeps,
    agent_id: &str,
    failures: &mut Vec<serde_json::Value>,
) -> Result<(), String> {
    let current_session_key = deps
        .session_workers
        .read_session_work_admission(agent_id)?
        .map(|record| record.session_key)
        .filter(|value| !value.trim().is_empty());
    if current_session_key.is_none()
        && failures.iter().any(|failure| {
            failure
                .get("sessionKey")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        })
    {
        return Err("channel failure session provenance is unavailable".into());
    }
    let mut stale = Vec::new();
    failures.retain(|failure| {
        let Some(session_key) = failure
            .get("sessionKey")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return true;
        };
        match current_session_key.as_deref() {
            Some(current) if current == session_key => true,
            Some(_) => {
                stale.push(failure.clone());
                false
            }
            None => true,
        }
    });
    clear_durable_channel_failure_markers(deps, agent_id, &stale);
    Ok(())
}

fn run_channel_failure_revival_worker(
    deps: LocalRoutedRunnerDeps,
    roster: Arc<ProductionRosterEmit>,
    wakes: Arc<Mutex<BackgroundWakes<serde_json::Value>>>,
    agent_id: String,
) {
    loop {
        let mut failures = {
            let mut state = wakes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            BackgroundWakes::take_pending(&mut state.pending_channel_failures, &agent_id)
        };
        if failures.is_empty() {
            break;
        }

        let summary = match deps.session_workers.summarize_agent_by_id(&agent_id, None) {
            Ok(Some(summary)) => summary,
            Ok(None) => {
                clear_durable_channel_failure_markers(&deps, &agent_id, &failures);
                continue;
            }
            Err(error) => {
                eprintln!(
                    "mahayana-host channel_failure_resolve_failed agent={} error={error}",
                    agent_id
                );
                requeue_channel_failures(&wakes, &agent_id, failures);
                break;
            }
        };
        if summary.is_group {
            clear_durable_channel_failure_markers(&deps, &agent_id, &failures);
            continue;
        }
        if let Err(error) = filter_current_channel_failure_provenance(
            &deps,
            &agent_id,
            &mut failures,
        ) {
            eprintln!(
                "mahayana-host channel_failure_provenance_resolve_failed agent={} error={error}",
                agent_id
            );
            requeue_channel_failures(&wakes, &agent_id, failures);
            break;
        }
        if failures.is_empty() {
            continue;
        }
        let Some(provider) = configured_routed_provider(&deps.data_dir.join("settings.json")) else {
            requeue_channel_failures(&wakes, &agent_id, failures);
            break;
        };
        if deps.transcript_runtime.is_quiescing_for_upgrade() {
            requeue_channel_failures(&wakes, &agent_id, failures);
            break;
        }

        let prompt = build_channel_delivery_failure_wake_prompt(&failures);
        let widget_prompts = deps
            .transcript_manager
            .widget_responses()
            .collect_unanswered_question_prompts(&agent_id)
            .unwrap_or_default();
        match run_local_background_revival_turn_with_context(
            deps.clone(),
            provider,
            &agent_id,
            "connector",
            &prompt,
            false,
            "continue",
            BackgroundRevivalContext {
                selected_images: Vec::new(),
                skipped_question_prompts: widget_prompts.skipped_question_prompts,
                dismissed_question_prompts: widget_prompts.dismissed_question_prompts,
                ..BackgroundRevivalContext::default()
            },
        ) {
            Ok(execution)
                if !execution.aborted
                    && !execution.quiesced_for_upgrade
                    && execution.sent_message_count > 0 =>
            {
                clear_durable_channel_failure_markers(&deps, &agent_id, &failures);
                let _ = roster.emit_agent_update(&agent_id);
            }
            Ok(_) => {
                requeue_channel_failures(&wakes, &agent_id, failures);
                break;
            }
            Err(error) => {
                let classified_error = ProviderSessionError::Tool(error.clone());
                let report = AgentErrorReport {
                    source: "channel_failure".into(),
                    conversation_id: agent_id.clone(),
                    request_id: None,
                    error: classify_agent_error(&classified_error),
                    detail: Some(sand_error_detail(&error)),
                };
                if let Err(telemetry_error) = deps.telemetry_logs.report_agent_error(&report) {
                    eprintln!(
                        "mahayana-host channel_failure_telemetry_failed agent={} error={telemetry_error}",
                        agent_id
                    );
                }
                let mut tray = provider_failure_tray(&agent_id, &error, started_at_ms() as i64);
                tray.title = "Delivery-failure follow-up failed".into();
                deps.trays.push_error(tray);
                requeue_channel_failures(&wakes, &agent_id, failures);
                break;
            }
        }
    }
    let mut state = wakes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    BackgroundWakes::<serde_json::Value>::end_revival(
        &mut state.reviving_channel_failure_agent_ids,
        &agent_id,
    );
}

fn queue_channel_delivery_failure(
    deps: LocalRoutedRunnerDeps,
    roster: Arc<ProductionRosterEmit>,
    wakes: Arc<Mutex<BackgroundWakes<serde_json::Value>>>,
    agent_id: String,
    address_token: String,
    delivery_operation_id: Option<String>,
    session_key: Option<String>,
    reason: String,
) {
    let marked_at_ms = started_at_ms() as f64;
    if let Some(operation_id) = delivery_operation_id.as_deref() {
        if let Some(store) = deps.transcript_runtime.pending_wake_store() {
            let written = store.mark_pending(DurablePendingWakeMarker {
                agent_id: agent_id.clone(),
                kind: PendingWakeKind::ChannelDelivery,
                work_id: operation_id.to_string(),
                marked_at_ms,
                quiet_origin: None,
                title: Some("Channel delivery failure".into()),
                subagent_type: None,
                interrupted_by_recreate: false,
                delivery_operation_id: Some(operation_id.to_string()),
                address_token: Some(address_token.clone()),
                failure_reason: Some(reason.clone()),
                session_key: session_key.clone(),
            });
            if !written {
                eprintln!(
                    "mahayana-host channel_failure_persist_failed agent={} operation={}",
                    agent_id, operation_id
                );
            }
        }
    }
    let should_spawn = {
        let mut state = wakes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        BackgroundWakes::enqueue(
            &mut state.pending_channel_failures,
            agent_id.clone(),
            serde_json::json!({
                "addressToken": address_token,
                "deliveryOperationId": delivery_operation_id,
                "markedAtMs": marked_at_ms,
                "sessionKey": session_key,
                "reason": reason,
            }),
        );
        BackgroundWakes::<serde_json::Value>::begin_revival(
            &mut state.reviving_channel_failure_agent_ids,
            &agent_id,
        )
    };
    if !should_spawn {
        return;
    }
    let worker_agent_id = agent_id.clone();
    let worker_wakes = Arc::clone(&wakes);
    if let Err(error) = thread::Builder::new()
        .name(format!("mahayana-channel-failure-{agent_id}"))
        .spawn(move || {
            run_channel_failure_revival_worker(deps, roster, worker_wakes, worker_agent_id);
        })
    {
        let mut state = wakes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        BackgroundWakes::<serde_json::Value>::end_revival(
            &mut state.reviving_channel_failure_agent_ids,
            &agent_id,
        );
        eprintln!(
            "mahayana-host channel_failure_worker_spawn_failed agent={} error={error}",
            agent_id
        );
    }
}

fn start_ack_redrive_worker(
    api: Arc<UnifiedGatewayApi>,
    ack_obligations: Arc<AckObligations>,
    session_workers: Arc<ProductionSessionWorkers>,
    runner_registry: Arc<TranscriptRunnerRegistry>,
    events: GatewayEventHub,
    stop: Arc<AtomicBool>,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("mahayana-ack-redrive".into())
        .spawn(move || {
            let _ = ack_obligations.arm_boot_redrives(started_at_ms());
            while !stop.load(Ordering::Acquire) {
                if api.transcript_runtime.is_quiescing_for_upgrade() {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }

                let now_ms = started_at_ms();
                let obligations = ack_obligations.pending_obligations();
                for obligation in &obligations {
                    let agent_id = obligation.agent_id.as_str();
                    let active = !runner_registry.active_stream_ids_for_agent(agent_id).is_empty()
                        || api.transcript_runtime.is_agent_running(agent_id);
                    if !active && ack_obligations.redrive_schedule(agent_id).is_none() {
                        let _ = ack_obligations
                            .schedule_ack_redrive_after_idle(agent_id, now_ms);
                    }
                }

                for obligation in obligations {
                    if stop.load(Ordering::Acquire)
                        || api.transcript_runtime.is_quiescing_for_upgrade()
                    {
                        break;
                    }
                    let agent_id = obligation.agent_id.clone();
                    if !runner_registry.active_stream_ids_for_agent(&agent_id).is_empty()
                        || api.transcript_runtime.is_agent_running(&agent_id)
                    {
                        continue;
                    }
                    let Some(trigger) =
                        ack_obligations.take_due_redrive(&agent_id, now_ms)
                    else {
                        continue;
                    };
                    let agent_exists = session_workers
                        .session_db_path(&agent_id)
                        .is_ok_and(|path| path.is_file());
                    match ack_obligations.prepare_redrive_with_telemetry(
                        &agent_id, agent_exists, trigger, now_ms as f64,
                    ) {
                        Ok(AckRedrivePreparation::Missing) => {}
                        Ok(AckRedrivePreparation::LostAgentDeleted(lost)) => {
                            events.publish(serde_json::json!({
                                "channel": "ack-obligation",
                                "payload": {
                                    "agentId": agent_id,
                                    "outcome": "lost",
                                    "reason": "agent_deleted",
                                    "ageMs": started_at_ms() as f64 - lost.created_at_ms,
                                    "coalescedCount": lost.coalesced_count,
                                    "redriveAttempts": lost.redrive_attempts,
                                }
                            }));
                        }
                        Ok(AckRedrivePreparation::LostMaxRedrives(lost)) => {
                            events.publish(serde_json::json!({
                                "channel": "ack-obligation",
                                "payload": {
                                    "agentId": agent_id,
                                    "outcome": "lost",
                                    "reason": "max_redrives",
                                    "ageMs": started_at_ms() as f64 - lost.created_at_ms,
                                    "coalescedCount": lost.coalesced_count,
                                    "redriveAttempts": lost.redrive_attempts,
                                }
                            }));
                        }
                        Ok(AckRedrivePreparation::Ready(bumped)) => {
                            let session_key = match session_workers
                                .read_session_work_admission(&agent_id)
                            {
                                Ok(Some(record)) if !record.session_key.trim().is_empty() => {
                                    record.session_key
                                }
                                Ok(_) => {
                                    eprintln!(
                                        "mahayana-host-ack redrive_missing_session_identity agent={agent_id}"
                                    );
                                    continue;
                                }
                                Err(error) => {
                                    eprintln!(
                                        "mahayana-host-ack redrive_session_identity_read_failed agent={} error={error}",
                                        agent_id
                                    );
                                    continue;
                                }
                            };
                            let send_now_ms = started_at_ms();
                            events.publish(serde_json::json!({
                                "channel": "ack-obligation",
                                "payload": {
                                    "agentId": agent_id,
                                    "outcome": "redrive",
                                    "reason": trigger.as_str(),
                                    "ageMs": send_now_ms as f64 - bumped.created_at_ms,
                                    "coalescedCount": bumped.coalesced_count,
                                    "redriveAttempts": bumped.redrive_attempts,
                                }
                            }));
                            let args = build_ack_redrive_send_args(
                                &agent_id,
                                &session_key,
                                &bumped,
                                trigger,
                                send_now_ms,
                            );
                            if let Err(error) = api.call("sendPrompt", args) {
                                eprintln!(
                                    "mahayana-host-ack redrive_failed agent={agent_id} error={error}"
                                );
                                events.publish(serde_json::json!({
                                    "channel": "ack-obligation",
                                    "payload": {
                                        "agentId": agent_id,
                                        "outcome": "redrive_error",
                                        "reason": trigger.as_str(),
                                        "redriveAttempts": bumped.redrive_attempts,
                                        "message": error.to_string(),
                                    }
                                }));
                            }
                        }
                        Err(error) => {
                            eprintln!(
                                "mahayana-host-ack redrive_prepare_failed agent={agent_id} error={error}"
                            );
                        }
                    }
                }

                let mut slept = 0u64;
                while slept < 100 && !stop.load(Ordering::Acquire) {
                    let slice = (100 - slept).min(25);
                    thread::sleep(Duration::from_millis(slice));
                    slept = slept.saturating_add(slice);
                }
            }
        })
}

fn project_forever_box_status(
    status: &BoxStatus,
    handoff: Option<&PendingHandoff>,
) -> serde_json::Value {
    let windows = status.windows.as_ref().map(|windows| {
        windows
            .iter()
            .map(|window| {
                serde_json::json!({
                    "windowIndex": window.window_index,
                    "vncUrl": window.vnc_url,
                })
            })
            .collect::<Vec<_>>()
    });
    let handoff = handoff.map(|pending| {
        serde_json::json!({
            "requestId": pending.request_id,
            "instruction": pending.instruction,
            "snapshotDataUrl": pending.snapshot_data_url,
        })
    });
    serde_json::json!({
        "agentId": status.agent_id,
        "state": status.state,
        "vncUrl": status.vnc_url,
        "windows": windows,
        "imageUpdateAvailable": status.image_update_available,
        "pull": status.pull_percent.map(|percent| serde_json::json!({ "percent": percent })),
        "handoff": handoff,
    })
}

fn required_box_agent_id<'a>(
    method: &str,
    args: &'a serde_json::Value,
) -> Result<&'a str, GatewayCommandError> {
    args.get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(format!("{method} requires id")))
}

fn call_host_lane(
    host_tx: &mpsc::Sender<HostLaneRequest>,
    method: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let (reply, result) = mpsc::sync_channel(1);
    host_tx
        .send(HostLaneRequest::Gateway {
            method: method.to_string(),
            args,
            reply,
        })
        .map_err(|_| GatewayCommandError::Internal("Mahayana Host lane is closed".into()))?;
    result
        .recv_timeout(Duration::from_secs(120))
        .map_err(|_| GatewayCommandError::Internal("Mahayana Host gateway request timed out".into()))?
}

struct ProductionAutoReviewGateDeps {
    auto_review: Arc<HostAutoReviewExtension>,
    controller: Arc<SandAutoReviewController>,
    box_id: String,
}

impl AutoReviewGateDependencies for ProductionAutoReviewGateDeps {
    fn base_modes(&self) -> SandAutoReviewModes {
        self.auto_review.current_modes()
    }

    fn current_modes(&self) -> Option<SandAutoReviewModes> {
        Some(self.auto_review.current_modes())
    }

    fn controller(&self) -> Option<Arc<SandAutoReviewController>> {
        Some(Arc::clone(&self.controller))
    }

    fn resolve_box_id(&self) -> String {
        self.box_id.clone()
    }

    fn instructions(&self) -> Option<AutoReviewInstructions> {
        Some(self.auto_review.instructions())
    }
}

type RoutedMcpAutoReviewCallback = Arc<
    dyn Fn(&RoutedToolDefinition, &serde_json::Value, &str)
            -> Result<Option<String>, ProviderSessionError>
        + Send
        + Sync
        + 'static,
>;

#[derive(Clone)]
struct CoordinatorRoutedToolBridge {
    relay: Arc<CoordinatorToolRelay>,
    transcript_runtime: Arc<ProductionTranscriptRuntime>,
    telemetry_logs: HostStructuredLogTelemetry,
    agent_id: String,
    mcp_review: Option<RoutedMcpAutoReviewCallback>,
}

impl RoutedToolBridge for CoordinatorRoutedToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let started = Instant::now();
        let result = self
            .relay
            .request(ROUTED_TOOL_LIST_METHOD, serde_json::json!({}))
            .map_err(|error| ProviderSessionError::Tool(error.to_string()))
            .and_then(decode_routed_tools);
        if let Err(error) = result.as_ref() {
            let report = McpDiscoveryFailedReport {
                error_class: "provider_session_error".into(),
                elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
                served_stale: false,
            };
            if let Err(telemetry_error) = self.telemetry_logs.report_mcp_discovery_failed(&report) {
                eprintln!(
                    "mahayana-host mcp_discovery_failed_telemetry_failed agent={} discovery_error={} telemetry_error={}",
                    self.agent_id, error, telemetry_error
                );
            }
        }
        result
    }

    fn list_mcp_meta_tools(
        &self,
    ) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
        let started = Instant::now();
        let result = self
            .relay
            .request(ROUTED_TOOL_LIST_METHOD, serde_json::json!({}))
            .map_err(|error| ProviderSessionError::Tool(error.to_string()))
            .and_then(decode_routed_mcp_meta_tools);
        if let Err(error) = result.as_ref() {
            let report = McpDiscoveryFailedReport {
                error_class: "provider_session_error".into(),
                elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
                served_stale: false,
            };
            if let Err(telemetry_error) = self.telemetry_logs.report_mcp_discovery_failed(&report) {
                eprintln!(
                    "mahayana-host mcp_meta_discovery_failed_telemetry_failed agent={} discovery_error={} telemetry_error={}",
                    self.agent_id, error, telemetry_error
                );
            }
        }
        result
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: serde_json::Value,
        tool_call_id: &str,
    ) -> Result<serde_json::Value, ProviderSessionError> {
        if let Some(review) = self.mcp_review.as_ref() {
            if let Some(reason) = review(tool, &args, tool_call_id)? {
                return Ok(serde_json::Value::String(reason));
            }
        }
        let activity_args = serde_json::json!({
            "providerIdentifier": tool.provider_identifier,
            "serverIdentifier": tool.provider_identifier,
        })
        .to_string();
        self.transcript_runtime.track_runner_activity_update(
            &self.agent_id,
            &ActivityUpdate::ToolCall {
                id: tool_call_id.to_string(),
                name: "mcpToolCall".into(),
                status: "pending".into(),
                args: Some(activity_args.clone()),
                summary: tool.description.clone(),
            },
            started_at_ms(),
        );
        let result = self.relay
            .request(
                ROUTED_TOOL_EXECUTE_METHOD,
                serde_json::json!({
                    "providerIdentifier": tool.provider_identifier,
                    "name": tool.name,
                    "toolName": tool.tool_name,
                    "args": args,
                    "toolCallId": tool_call_id,
                    "agentId": self.agent_id,
                }),
            )
            .map_err(|error| ProviderSessionError::Tool(error.to_string()));
        self.transcript_runtime.track_runner_activity_update(
            &self.agent_id,
            &ActivityUpdate::ToolCall {
                id: tool_call_id.to_string(),
                name: "mcpToolCall".into(),
                status: if result.is_ok() { "completed" } else { "failed" }.into(),
                args: Some(activity_args),
                summary: tool.description.clone(),
            },
            started_at_ms(),
        );
        result
    }
}

fn decode_routed_tools(
    value: serde_json::Value,
) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
    let rows = value.as_array().ok_or_else(|| {
        ProviderSessionError::Protocol("listRoutedMcpTools did not return an array".into())
    })?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(RoutedToolDefinition {
                name: row.get("name")?.as_str()?.to_string(),
                provider_identifier: row.get("providerIdentifier")?.as_str()?.to_string(),
                tool_name: row.get("toolName")?.as_str()?.to_string(),
                description: row.get("description").and_then(serde_json::Value::as_str).map(str::to_string),
                input_schema: row.get("inputSchema").cloned().unwrap_or_else(|| {
                    serde_json::json!({"type":"object","additionalProperties":true})
                }),
            })
        })
        .collect())
}

fn decode_routed_mcp_meta_tools(
    value: serde_json::Value,
) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
    let rows = value.as_array().ok_or_else(|| {
        ProviderSessionError::Protocol("listRoutedMcpTools did not return an array".into())
    })?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(RoutedMcpMetaToolDefinition {
                tool: RoutedToolDefinition {
                    name: row.get("name")?.as_str()?.to_string(),
                    provider_identifier: row.get("providerIdentifier")?.as_str()?.to_string(),
                    tool_name: row.get("toolName")?.as_str()?.to_string(),
                    description: row
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    input_schema: row.get("inputSchema").cloned().unwrap_or_else(|| {
                        serde_json::json!({"type":"object","additionalProperties":true})
                    }),
                },
                plugin: row.get("plugin").cloned().filter(|value| !value.is_null()),
                marketplace: row
                    .get("marketplace")
                    .cloned()
                    .filter(|value| !value.is_null()),
                plugin_id: row
                    .get("pluginId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                marketplace_id: row
                    .get("marketplaceId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect())
}

fn decode_provider_messages(
    args: &serde_json::Value,
) -> Result<Vec<ProviderMessage>, GatewayCommandError> {
    let rows = args
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| GatewayCommandError::Internal(
            "runner.startRoutedProvider requires messages".into()
        ))?;
    let messages = rows
        .iter()
        .filter_map(|row| {
            Some(ProviderMessage {
                role: row.get("role")?.as_str()?.to_string(),
                content: row.get("content")?.as_str()?.to_string(),
            })
        })
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return Err(GatewayCommandError::Internal(
            "runner.startRoutedProvider requires at least one message".into()
        ));
    }
    Ok(messages)
}

fn build_cloud_agent_auto_review_hook(
    auth: Arc<HostAuthExtension>,
    auto_review: Arc<HostAutoReviewExtension>,
    controller: Arc<mahayana_host_runtime::runner::sand_auto_review::SandAutoReviewController>,
    agent_id: String,
    request_source: String,
    cancellation: RoutedProviderCancellation,
    conversation_context: Vec<ProviderMessage>,
) -> CloudAgentReviewHook {
    Arc::new(move |args, images, tool_call_id| {
        let Some(object) = args.as_object() else {
            return Ok(None);
        };
        let action = object
            .get("action")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if action.is_empty() {
            return Ok(None);
        }

        let mode = auto_review.current_modes().cloud_agent;
        let cancelled = || cancellation.is_cancelled();
        if matches!(action, "launch" | "reply") {
            let prompt = object
                .get("prompt")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let review_images = images
                .iter()
                .map(|image| CloudAgentReviewImage {
                    name: Path::new(&image.path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(&image.path)
                        .to_string(),
                    data: image.data.clone(),
                })
                .collect::<Vec<_>>();
            let Some(target) = build_sand_cloud_agent_review_target(
                action,
                prompt,
                object.get("agent_id").and_then(serde_json::Value::as_str),
                &review_images,
                object
                    .get("interrupt")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                object.get("repo_url").and_then(serde_json::Value::as_str),
                object.get("title").and_then(serde_json::Value::as_str),
            ) else {
                return Ok(None);
            };
            let classifier_cancellation = cancellation.clone();
            let mut classifier =
                create_sand_backend_smart_mode_classifier_executor_with_cancellation(
                    Arc::clone(&auth),
                    Arc::new(move || classifier_cancellation.is_cancelled()),
                )
                .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
            let classifier_context = conversation_context
                .iter()
                .map(|message| {
                    serde_json::json!({
                        "role": message.role,
                        "content": message.content,
                    })
                })
                .collect::<Vec<_>>();
            let outcome = review_sand_cloud_agent_action(
                mode,
                &target,
                Some(controller.as_ref()),
                &request_source,
                cancelled,
                |risk_target, classifier_mode| {
                    run_sand_auto_review_classifier(
                        &mut classifier,
                        tool_call_id,
                        &agent_id,
                        classifier_mode,
                        || risk_target.clone(),
                        || Ok(classifier_context.clone()),
                        &[],
                        SAND_CLOUD_AGENT_CLASSIFIER_ERROR_REASON,
                    )
                },
            )
            .map_err(|error| ProviderSessionError::Tool(format!("{error:?}")))?;
            return Ok(match outcome {
                CloudAgentReviewOutcome::Allowed => None,
                CloudAgentReviewOutcome::Blocked(reason) => Some(reason),
                CloudAgentReviewOutcome::Cancelled => {
                    Some("The cloud agent action was cancelled.".into())
                }
            });
        }

        let lifecycle = match action {
            "rename" => Some(CloudLifecycleAction::Rename),
            "cancel" => Some(CloudLifecycleAction::Cancel),
            "archive" => Some(CloudLifecycleAction::Archive),
            "unarchive" => Some(CloudLifecycleAction::Unarchive),
            "delete" => Some(CloudLifecycleAction::Delete),
            _ => None,
        };
        let Some(lifecycle) = lifecycle else {
            return Ok(None);
        };
        let agent = object
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let Some(target) = build_sand_cloud_agent_lifecycle_review_target(
            action,
            agent,
            object.get("title").and_then(serde_json::Value::as_str),
        ) else {
            return Ok(None);
        };
        let outcome = review_sand_cloud_agent_lifecycle_action(
            mode,
            lifecycle,
            &target,
            Some(controller.as_ref()),
            &request_source,
            cancelled,
        )
        .map_err(|error| ProviderSessionError::Tool(format!("{error:?}")))?;
        Ok(match outcome {
            CloudAgentReviewOutcome::Allowed => None,
            CloudAgentReviewOutcome::Blocked(reason) => Some(reason),
            CloudAgentReviewOutcome::Cancelled => {
                Some("The cloud agent action was cancelled.".into())
            }
        })
    })
}

fn starts_new_local_tool_direction(
    request_source: Option<&str>,
    is_ack_redrive: bool,
    is_upgrade_resume: bool,
    turn_hidden: bool,
    is_group_member_turn: bool,
    has_generated_parent: bool,
) -> bool {
    matches!(request_source, None | Some("turn"))
        && !is_ack_redrive
        && !is_upgrade_resume
        && !turn_hidden
        && !is_group_member_turn
        && !has_generated_parent
}

pub(crate) fn start_routed_provider_task(
    routed_tool_relay: Arc<CoordinatorToolRelay>,
    mcp_service: Arc<McpHostService>,
    events: GatewayEventHub,
    host_tx: mpsc::Sender<HostLaneRequest>,
    data_dir: PathBuf,
    request_context: Arc<dyn RunnerRequestContextSource>,
    auth: Arc<HostAuthExtension>,
    auto_review: Arc<HostAutoReviewExtension>,
    experiments: Arc<HostExperimentsExtension>,
    settings: Arc<SettingsService>,
    inference: Arc<ProductionInferenceExtension>,
    session_workers: Arc<ProductionSessionWorkers>,
    runner_registry: Arc<TranscriptRunnerRegistry>,
    ack_obligations: Arc<AckObligations>,
    transcript_runtime: Arc<ProductionTranscriptRuntime>,
    transcript_manager: Arc<TranscriptManager>,
    generated_agent_runtime: Arc<Mutex<SubagentRuntime>>,
    completion_revivals: Arc<CompletionRevivals>,
    forever_box: Arc<ForeverBoxService>,
    local_exec: Arc<HostLocalExecExtension>,
    local_tool_permission: Arc<HostLocalToolPermissionExtension>,
    session_handoff: BoxHandoffService,
    trays: Arc<HostTraysExtension>,
    telemetry_logs: HostStructuredLogTelemetry,
    product_analytics: HostProductAnalytics,
    production_action_auditor: ActionAuditExtension,
    cloud_agents: Arc<SandCloudAgentManager>,
    cloud_agent_watches: Arc<RunnerCloudAgentWatches>,
    background_shell_watches: Arc<RunnerBackgroundShellWatches>,
    host_runner_composition: Arc<HostRunnerComposition>,
    box_store_sync: ProductionBoxStoreSyncApi,
    automations_lifecycle: Arc<Mutex<Weak<ProductionAutomationsLifecycle>>>,
    generated_subagent_cleanup: GeneratedSubagentCleanupSlot,
    gateway_context: Option<GatewayCommandContext>,
    mut args: serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let agent_id = args.get("agentId").and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| GatewayCommandError::Internal(
            "runner.startRoutedProvider requires agentId".into()
        ))?
        .to_string();
    let requested_worker_session_key = args
        .get("sessionKey")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let worker_session_key = match requested_worker_session_key {
        Some(session_key) => session_key,
        None => session_workers
            .read_session_work_admission(&agent_id)
            .map_err(GatewayCommandError::Internal)?
            .map(|record| record.session_key)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| GatewayCommandError::Internal(
                format!("runner.startRoutedProvider requires an existing admitted session identity for {agent_id}"),
            ))?,
    };
    let worker_session_admitted_key = session_workers
        .read_session_work_admission(&agent_id)
        .map_err(GatewayCommandError::Internal)?
        .and_then(|record| {
            (record.session_key == worker_session_key).then(|| worker_session_key.clone())
        });
    let persisted_session_model = SandAgentSessionStore::new(Arc::clone(&session_workers))
        .read_session_model_entry(&agent_id)
        .map_err(GatewayCommandError::Internal)?;
    let provider_name = args
        .get("provider")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let requested_provider = RoutedProvider::parse(provider_name)
        .ok_or_else(|| GatewayCommandError::Internal(format!(
            "unsupported routed provider: {provider_name}"
        )))?;
    let provider = authorize_routed_provider_request(
        inference.route(),
        requested_provider,
    )
    .map_err(GatewayCommandError::Internal)?;
    let stream_id = args.get("streamId").and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| GatewayCommandError::Internal(
            "runner.startRoutedProvider requires streamId".into()
        ))?
        .to_string();
    let is_group_member_turn = args
        .get("groupMemberTurn")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let request_source = args
        .get("requestSource")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if worker_session_admitted_key.is_none()
        && !(is_group_member_turn && request_source.as_deref() == Some("group-member"))
    {
        return Err(GatewayCommandError::Internal(format!(
            "runner.startRoutedProvider requires an owner-admitted session identity for {agent_id}"
        )));
    }
    let generated_parent_agent_id = args
        .get("parentAgentId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let generated_subagent_type = args
        .get("subagentType")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("general-purpose")
        .to_string();
    let generated_lifecycle_revision = args
        .get("lifecycleRevision")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_string();
    let provider_review_lifecycle_revision = args
        .get("providerReviewLifecycleRevision")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if request_source.as_deref() == Some("provider-review")
        && provider_review_lifecycle_revision.is_none()
    {
        return Err(GatewayCommandError::Internal(
            "provider-review Runner dispatch requires the captured lifecycle revision".into(),
        ));
    }
    let generated_tool_call_id = args
        .get("parentAgentToolCallId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&stream_id)
        .to_string();
    let is_ack_redrive = args
        .get("ackRedrive")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let is_upgrade_resume = args
        .get("upgradeResume")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let turn_hidden = args
        .get("hidden")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    transcript_runtime
        .require_routed_turn_lease(&agent_id, &stream_id)
        .map_err(map_production_send_error)?;
    let human_handoff_conversation_id =
        transcript_runtime.routed_human_handoff_conversation_id(&agent_id, &stream_id);
    let turn_local_tool_permission = local_tool_permission.controller();
    let starts_new_user_direction = starts_new_local_tool_direction(
        request_source.as_deref(),
        is_ack_redrive,
        is_upgrade_resume,
        turn_hidden,
        is_group_member_turn,
        generated_parent_agent_id.is_some(),
    );
    if starts_new_user_direction {
        turn_local_tool_permission.begin_turn(&agent_id);
    }
    let turn_local_tool_direction_epoch =
        turn_local_tool_permission.direction_epoch(&agent_id);
    let routed_turn_lease_guard = RoutedTurnLeaseGuard::new(
        Arc::clone(&transcript_runtime),
        agent_id.clone(),
        stream_id.clone(),
        events.clone(),
    );
    let mut attachment_paths = args
        .get("attachmentPaths")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    for media_path in selected_media_host_paths_for_turn(&args) {
        if !attachment_paths.iter().any(|known| known == &media_path) {
            attachment_paths.push(media_path);
        }
    }
    if !attachment_paths.is_empty() {
        if let Some(services) = transcript_manager.production_services() {
            let staged = services.attachments.stage_into_box(&agent_id, &attachment_paths);
            apply_staged_attachment_paths_for_turn(&mut args, &staged);
        }
    }

    let lifecycle_messages = decode_provider_messages(&args)?;
    let mut turn_input = create_production_turn_input_projection(
        &args,
        &stream_id,
        &lifecycle_messages,
    )
    .map_err(|error| GatewayCommandError::Internal(error.to_string()))?;
    let typed_replay = session_workers
        .read_agent_typed_compacted_transcript_entries(&agent_id)
        .map_err(|error| GatewayCommandError::Internal(format!(
            "could not read typed compacted replay for {agent_id}: {error}"
        )))?;
    if !typed_replay.is_empty() {
        turn_input.typed_replay = Some(typed_replay);
    }

    let prompt_compaction_epoch = host_runner_composition
        .prompt_compaction_epoch(&session_workers, &agent_id)
        .map_err(GatewayCommandError::Internal)?;
    let profile_compaction_epoch = i64::try_from(prompt_compaction_epoch).map_err(|_| {
        GatewayCommandError::Internal(format!(
            "production conversation compaction epoch exceeds profile snapshot range for {agent_id}"
        ))
    })?;
    let prompt_role = if generated_parent_agent_id.is_none() {
        RunnerPromptRole::Main
    } else if generated_subagent_type.eq_ignore_ascii_case("computeruse") {
        RunnerPromptRole::ComputerUseSubagent
    } else if generated_subagent_type.eq_ignore_ascii_case("browseruse") {
        RunnerPromptRole::BrowserUseSubagent
    } else {
        RunnerPromptRole::OtherSubagent
    };
    let prompt_remote_lease_state = Arc::new(Mutex::new((true, false, None::<u32>)));

    let prompt_watermark_sessions = Arc::clone(&session_workers);
    let prompt_watermark_owner = Arc::clone(&host_runner_composition);
    let prompt_watermark_agent_id = agent_id.clone();
    let confirmed_user_watermark_for_turn = Arc::new(move || {
        prompt_watermark_owner.confirmed_user_turn_watermark(
            &prompt_watermark_sessions,
            &prompt_watermark_agent_id,
        )
    });

    let prompt_profile_sessions = Arc::clone(&session_workers);
    let prompt_profile_agent_id = agent_id.clone();
    let profile_for_turn = Arc::new(move || -> Result<RunnerPromptProfileState, String> {
        let Some(profile) = prompt_profile_sessions
            .get_agent_profile_text(&prompt_profile_agent_id)
            .map_err(|error| {
                format!(
                    "could not read production Agent profile for {}: {error}",
                    prompt_profile_agent_id
                )
            })?
        else {
            return Ok(RunnerPromptProfileState::default());
        };
        let agent_dir = prompt_profile_sessions
            .session_db_path(&prompt_profile_agent_id)?
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                format!(
                    "agent database path has no parent for {}",
                    prompt_profile_agent_id
                )
            })?;
        let profile_path = to_model_visible_path(&get_sand_profile_path(&agent_dir))
            .to_string_lossy()
            .into_owned();
        let settings_path = to_model_visible_path(&get_sand_settings_path(&agent_dir))
            .to_string_lossy()
            .into_owned();
        let profile_for_prompt = AgentProfileForPrompt {
            name: profile.name.clone(),
            description: profile.description.clone(),
            file_path: profile_path,
            settings_file_path: settings_path,
        };
        let Some(live_section) = render_agent_profile_section(&profile_for_prompt, false) else {
            return Ok(RunnerPromptProfileState::default());
        };
        let persisted_profile_snapshot = prompt_profile_sessions
            .get_agent_profile_prompt_snapshot(&prompt_profile_agent_id)?
            .and_then(|value| serde_json::from_value::<AgentProfilePromptSnapshot>(value).ok());
        let identity = normalize_agent_profile_identity(AgentProfileIdentity {
            name: profile.name,
            description: profile.description,
        });
        let resolved_profile_snapshot = resolve_agent_profile_prompt_snapshot(
            persisted_profile_snapshot.as_ref(),
            profile_compaction_epoch,
            live_section,
            identity.clone(),
        );
        if persisted_profile_snapshot.as_ref() != Some(&resolved_profile_snapshot) {
            let value = serde_json::to_value(&resolved_profile_snapshot).map_err(|error| {
                format!(
                    "could not serialize production profile prompt snapshot for {}: {error}",
                    prompt_profile_agent_id
                )
            })?;
            prompt_profile_sessions
                .set_agent_profile_prompt_snapshot(&prompt_profile_agent_id, &value)
                .map_err(|error| {
                    format!(
                        "could not persist production profile prompt snapshot for {}: {error}",
                        prompt_profile_agent_id
                    )
                })?;
        }
        let profile_update =
            resolve_profile_update_for_turn(&identity, &resolved_profile_snapshot.announced_identity);
        let announcement = profile_update
            .as_ref()
            .map(|_| (resolved_profile_snapshot.clone(), identity));
        Ok(RunnerPromptProfileState {
            system_section: Some(resolved_profile_snapshot.profile_section),
            profile_update,
            announcement,
        })
    });

    let prompt_mcp_service = Arc::clone(&mcp_service);
    let prompt_mcp_agent_id = agent_id.clone();
    let mcp_for_turn = Arc::new(move || -> Result<RunnerPromptMcpState, String> {
        Ok(match prompt_mcp_service.list_installed() {
            Ok(installed_servers) => RunnerPromptMcpState {
                installed_servers,
                discovery_unavailable: false,
            },
            Err(error) => {
                eprintln!(
                    "[sand:mcp] connector discovery unavailable for provider turn agent={}: {error}",
                    prompt_mcp_agent_id
                );
                RunnerPromptMcpState {
                    installed_servers: Vec::new(),
                    discovery_unavailable: true,
                }
            }
        })
    });

    let prompt_remote_forever_box = Arc::clone(&forever_box);
    let prompt_remote_handoff = session_handoff.clone();
    let prompt_remote_agent_id = agent_id.clone();
    let prompt_remote_lease = Arc::clone(&prompt_remote_lease_state);
    let remote_for_turn = Arc::new(move || -> Result<RunnerPromptRemoteState, String> {
        let desktop_capable = prompt_remote_forever_box
            .box_()
            .inner()
            .shared_desktop()
            .is_some();
        let status = prompt_remote_forever_box.get_status(&prompt_remote_agent_id);
        let box_available = prompt_remote_forever_box.box_().is_available();
        let lease = prompt_remote_lease
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let desktop_ready = desktop_capable
            && lease.0
            && (status.vnc_url.is_some()
                || status
                    .windows
                    .as_ref()
                    .is_some_and(|windows| !windows.is_empty()));
        Ok(RunnerPromptRemoteState {
            remote_box: RemoteBoxPromptState {
                role: prompt_role,
                available: box_available,
                runtime_state: status.state.clone(),
                desktop_capable,
                desktop_ready,
            },
            computer: ComputerPromptState {
                role: prompt_role,
                box_available,
                desktop_capable,
                desktop_ready,
                control_lease_active: lease.1,
                human_takeover_pending: prompt_remote_handoff
                    .get(&prompt_remote_agent_id)
                    .is_some(),
                browser_use_offered: false,
                window_index: lease.2,
            },
        })
    });

    let prompt_automation_sessions = Arc::clone(&session_workers);
    let prompt_automation_owner = Arc::clone(&host_runner_composition);
    let prompt_automation_agent_id = agent_id.clone();
    let prompt_firing_automation_id = args
        .get("automationWake")
        .and_then(serde_json::Value::as_object)
        .and_then(|wake| wake.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let prompt_is_silence_allowed = args
        .get("isSilenceAllowed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let automation_for_turn = Arc::new(move || -> Result<RunnerPromptAutomationState, String> {
        let automation_store =
            prompt_automation_sessions.open_automation_store(&prompt_automation_agent_id)?;
        let rendered = create_automation_status_reminder(
            &automation_store,
            prompt_firing_automation_id.as_deref(),
        );
        Ok(RunnerPromptAutomationState {
            status_reminder: prompt_automation_owner.automation_status_reminder_for_turn(
                &prompt_automation_agent_id,
                rendered.as_deref(),
                prompt_compaction_epoch,
            ),
            compaction_epoch: prompt_compaction_epoch,
            is_silence_allowed: prompt_is_silence_allowed,
        })
    });
    let prompt_note_owner = Arc::clone(&host_runner_composition);
    let prompt_note_agent_id = agent_id.clone();
    let note_automation_status = Arc::new(move |reminder: Option<String>, epoch: u64| {
        prompt_note_owner.note_automation_status_reminder(
            &prompt_note_agent_id,
            reminder.as_deref(),
            epoch,
        );
    });
    let prompt_experiments = Arc::clone(&experiments);
    let prompt_owner = RunnerPromptGlueOwner {
        is_subagent_runner: generated_parent_agent_id.is_some(),
        confirmed_user_watermark_for_turn,
        profile_for_turn,
        mcp_for_turn,
        remote_for_turn,
        automation_for_turn,
        note_automation_status,
        spotlight_enabled_for_turn: Arc::new(move || {
            prompt_experiments.check_feature_gate("sand_spotlight")
        }),
    };

    let mut provider_messages =
        prompt_owner.project_provider_messages(&args, &lifecycle_messages).messages;
    if let Some(prepared_session) = session_workers
        .prepare_existing_agent(&agent_id)
        .map_err(|error| {
            GatewayCommandError::Internal(format!(
                "could not prepare production session worker state for {agent_id}: {error}"
            ))
        })?
    {
        session_workers
            .ensure_capacity_for_turn(&prepared_session)
            .map_err(|error| {
                GatewayCommandError::Internal(format!(
                    "conversation capacity gate rejected {agent_id}: {error}"
                ))
            })?;
    }

    let mut prepend_user_messages = prompt_owner
        .prepend_unconfirmed_user_messages(&args, &mut provider_messages)
        .map_err(GatewayCommandError::Internal)?;
    if let Some(unanswered) = unanswered_questions_user_message_for_turn(&args) {
        prepend_user_messages.push(unanswered);
    }
    turn_input.prompt_action.prepend_user_messages = prepend_user_messages;
    let prompt_profile_state = prompt_owner
        .append_profile(&mut provider_messages)
        .map_err(GatewayCommandError::Internal)?;
    let profile_update_for_turn = prompt_profile_state.profile_update;
    let pending_profile_announcement = prompt_profile_state.announcement;

    let memory_service = session_workers.memory_service();
    let agent_summaries = Arc::new(
        session_workers
            .list_agent_summaries(Some(&agent_id))
            .map_err(|error| GatewayCommandError::Internal(format!(
                "could not read production agent directory for {agent_id}: {error}"
            )))?,
    );
    let memory_store = if is_group_member_turn {
        None
    } else {
        let resolve_agent_summaries = Arc::clone(&agent_summaries);
        let resolve_agent_name: Arc<dyn Fn(&str) -> String + Send + Sync> =
            Arc::new(move |candidate: &str| {
                resolve_agent_summaries
                    .iter()
                    .find(|summary| summary.id == candidate)
                    .map(|summary| summary.name.trim())
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| candidate.to_string())
            });
        let user_memory_store = memory_service.user_memory_store(
            agent_id.clone(),
            Arc::clone(&resolve_agent_name),
        );
        let user_memory_recall = user_memory_store.recall(
            MEMORY_USER_PROFILE_PROMPT_LIMIT,
            MEMORY_USER_RECENT_PROMPT_LIMIT,
        );
        let user_memory_location = to_model_visible_path(&user_memory_store.get_location())
            .to_string_lossy()
            .into_owned();
        let user_memory_own_shard_location =
            to_model_visible_path(&user_memory_store.get_own_shard_location())
                .to_string_lossy()
                .into_owned();

        let project_memory_store = memory_service.project_memory_store(
            agent_id.clone(),
            memory_service.project_membership_for_agent(&agent_id),
            Arc::clone(&resolve_agent_name),
        );
        let mut project_memory_recall = project_memory_store.recall_for_prompt(
            MEMORY_PROJECT_PROFILE_PROMPT_LIMIT,
            MEMORY_PROJECT_RECENT_PROMPT_LIMIT,
            MEMORY_PROJECT_INJECTED_CAP,
        );
        for block in &mut project_memory_recall.injected {
            block.own_shard_dir = to_model_visible_path(&block.own_shard_dir);
        }
        let projects_root_location = to_model_visible_path(&project_memory_store.get_location())
            .to_string_lossy()
            .into_owned();

        let memory_store = memory_service.store_for_agent(&agent_id);
        let memory_recall = memory_store.recall(MEMORY_RECENT_PROMPT_LIMIT);
        let memory_location = to_model_visible_path(&memory_store.get_location())
            .to_string_lossy()
            .into_owned();

        const FROZEN_MEMORY_COMPACTION_EPOCH: u64 = 0;
        let frozen_memory_snapshot = session_workers
            .get_agent_memory_prompt_snapshot(&agent_id)
            .map_err(|error| {
                GatewayCommandError::Internal(format!(
                    "could not read production memory prompt snapshot for {agent_id}: {error}"
                ))
            })?
            .and_then(|snapshot| {
                let epoch = snapshot.compaction_epoch;
                (epoch.is_finite()
                    && epoch >= 0.0
                    && epoch.fract() == 0.0
                    && epoch <= u64::MAX as f64)
                    .then(|| FrozenMemorySnapshot {
                        render: snapshot.render,
                        compaction_epoch: epoch as u64,
                    })
            });
        let disable_memory_freeze = std::env::var("SAND_DISABLE_MEMORY_FREEZE").ok();
        let resolved_memory = resolve_combined_memory_system_prompt(
            &user_memory_recall,
            Some(&user_memory_location),
            Some(&user_memory_own_shard_location),
            &project_memory_recall,
            Some(&projects_root_location),
            &memory_recall,
            Some(&memory_location),
            frozen_memory_snapshot.as_ref(),
            FROZEN_MEMORY_COMPACTION_EPOCH,
            is_memory_freeze_enabled(disable_memory_freeze.as_deref()),
        );
        if let Some(snapshot) = resolved_memory.snapshot_to_persist.as_ref() {
            session_workers
                .set_agent_memory_prompt_snapshot(
                    &agent_id,
                    &serde_json::json!({
                        "render": snapshot.render,
                        "compactionEpoch": snapshot.compaction_epoch,
                    }),
                )
                .map_err(|error| {
                    GatewayCommandError::Internal(format!(
                        "could not persist production memory prompt snapshot for {agent_id}: {error}"
                    ))
                })?;
        }
        append_combined_memory_system_prompt(
            &mut provider_messages,
            &resolved_memory.render,
        );
        Some(memory_store)
    };
    let automation_store = session_workers
        .open_automation_store(&agent_id)
        .map_err(|error| GatewayCommandError::Internal(format!(
            "could not open production automation store for {agent_id}: {error}"
        )))?;
    let automation_location = automation_store.get_location().to_string_lossy().into_owned();
    let automation_time_zone = automation_store.resolved_user_time_zone();
    let automation_definitions = automation_store.list_definitions();
    append_automations_system_prompt(
        &mut provider_messages,
        &automation_definitions,
        Some(&automation_location),
        automation_time_zone.as_deref(),
    );
    let workflow_store = session_workers
        .open_workflow_store(&agent_id)
        .map_err(|error| GatewayCommandError::Internal(format!(
            "could not open production workflow store for {agent_id}: {error}"
        )))?;
    let workflow_location = workflow_store.get_location().to_string_lossy().into_owned();
    let available_skill_prompts = if generated_parent_agent_id.is_none() && !is_group_member_turn {
        workflow_store
            .list()
            .into_iter()
            .filter(|workflow| workflow.trigger.is_none() && !workflow.disable_model_invocation)
            .map(|workflow| AgentSkillPromptItem {
                full_path: workflow.file_path.to_string_lossy().into_owned(),
                description: (!workflow.description.trim().is_empty())
                    .then_some(workflow.description),
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    append_budgeted_workflows_system_prompt(
        &mut provider_messages,
        Some(&workflow_location),
        &available_skill_prompts,
        SAND_AGENT_TOKEN_LIMIT,
    );

    let channel_store = session_workers
        .open_channel_store(&agent_id)
        .map_err(|error| GatewayCommandError::Internal(format!(
            "could not open production channel store for {agent_id}: {error}"
        )))?;
    let channel_location = to_model_visible_path(channel_store.get_location())
        .to_string_lossy()
        .into_owned();
    let channel_connections = session_workers
        .list_agent_channels(&agent_id)
        .map_err(|error| GatewayCommandError::Internal(format!(
            "could not read production channel state for {agent_id}: {error}"
        )))?;
    append_channels_system_prompt(
        &mut provider_messages,
        &channel_connections,
        Some(&channel_location),
    );

    let other_agents = agent_summaries
        .iter()
        .filter(|summary| summary.id != agent_id && !summary.is_group)
        .map(|summary| AgentAddress {
            id: summary.id.clone(),
            name: summary.name.clone(),
            description: (!summary.description.trim().is_empty())
                .then(|| summary.description.clone()),
            is_group: false,
        })
        .collect::<Vec<_>>();
    let agent_groups = agent_summaries
        .iter()
        .filter(|summary| {
            summary.is_group
                && summary.member_ids.iter().any(|member_id| member_id == &agent_id)
        })
        .map(|summary| {
            let members = summary
                .member_ids
                .iter()
                .filter(|member_id| member_id.as_str() != agent_id)
                .filter_map(|member_id| {
                    agent_summaries
                        .iter()
                        .find(|candidate| candidate.id == *member_id && !candidate.is_group)
                })
                .map(|member| AgentAddress {
                    id: member.id.clone(),
                    name: member.name.clone(),
                    description: (!member.description.trim().is_empty())
                        .then(|| member.description.clone()),
                    is_group: false,
                })
                .collect::<Vec<_>>();
            AgentGroupAddress {
                address: AgentAddress {
                    id: summary.id.clone(),
                    name: summary.name.clone(),
                    description: (!summary.description.trim().is_empty())
                        .then(|| summary.description.clone()),
                    is_group: true,
                },
                members,
            }
        })
        .collect::<Vec<_>>();
    let agents_root_location = to_model_visible_path(session_workers.agents_root())
        .to_string_lossy()
        .into_owned();
    append_agent_directory_system_prompt(
        &mut provider_messages,
        &other_agents,
        &agent_groups,
        Some(&agents_root_location),
        generated_parent_agent_id.is_none(),
    );

    let shipping_desktop_capable = forever_box.box_().inner().shared_desktop().is_some();
    let shipping_box_resources = Arc::new(
        ForeverBoxRunnerResourcePort::new(
            Arc::clone(&forever_box),
            agent_id.clone(),
        )
        .with_background_shell_watches(Arc::clone(&background_shell_watches)),
    );
    let mut computer_control_lease = None;
    let mut computer_use_window_granted = true;
    if prompt_role == RunnerPromptRole::ComputerUseSubagent {
        computer_control_lease =
            host_runner_composition.begin_computer_use_preparation(&agent_id);
        computer_use_window_granted = computer_control_lease.is_some();
        if computer_use_window_granted {
            match forever_box.box_().ensure_ready(&agent_id) {
                Err(error) => {
                    host_runner_composition.mark_computer_use_preparation_failed(
                        &agent_id,
                        mahayana_host_runtime::runner::computer_use::ComputerUsePrewarmStage::Box,
                        error.to_string(),
                    );
                }
                Ok(_) if shipping_desktop_capable => {
                    let prewarm = shipping_box_resources.execute_shell(RunnerBoxShellRequest {
                        command: "box-chrome --sand-prepare".into(),
                        working_directory: "/workspace".into(),
                        tool_call_id: format!("sand-cua-browser-prepare-{agent_id}"),
                        is_background: false,
                        block_until_ms: None,
                    });
                    match prewarm {
                        Ok(_) => {
                            host_runner_composition.mark_computer_use_preparation_ready(&agent_id);
                        }
                        Err(error) => {
                            host_runner_composition.mark_computer_use_preparation_failed(
                                &agent_id,
                                mahayana_host_runtime::runner::computer_use::ComputerUsePrewarmStage::Browser,
                                error.to_string(),
                            );
                        }
                    }
                }
                Ok(_) => {
                    host_runner_composition.mark_computer_use_preparation_ready(&agent_id);
                }
            }
        }
    }
    let computer_control_lease_active = computer_control_lease
        .as_ref()
        .is_some_and(|lease| host_runner_composition.owns_computer_control_lease(lease));
    let shipping_window_index = computer_control_lease
        .as_ref()
        .map(|lease| lease.window_index())
        .or_else(|| forever_box.box_().get_agent_window_index(&agent_id));
    {
        let mut lease_state = prompt_remote_lease_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *lease_state = (
            computer_use_window_granted,
            computer_control_lease_active,
            shipping_window_index,
        );
    }
    prompt_owner
        .append_live_runtime_sections(
            &mut provider_messages,
            profile_update_for_turn.as_deref(),
        )
        .map_err(GatewayCommandError::Internal)?;
    let spotlight_enabled = prompt_owner.spotlight_enabled();
    let dynamic_tools_enabled = experiments.is_dynamic_tools_enabled();
    let shared_room_box_tools_enabled = experiments.is_shared_room_box_tools_enabled();
    let browser_use_subagent_enabled = experiments.is_browser_use_subagent_enabled();
    let multitask_enabled = !args
        .get("groupMemberTurn")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        && experiments.is_multitask_enabled();

    let journal_logs = telemetry_logs.clone();
    let journal_experiments = Arc::clone(&experiments);
    let agent_state_checkpoint_sink = host_runner_composition
        .compose_production_checkpoint_sink(
            &session_workers,
            &data_dir,
            &agent_id,
            is_group_member_turn,
            Arc::new(move |outcome| {
                let cause = (outcome.outcome == "failed").then(|| {
                    SandErrorValue::new(match outcome.op.as_str() {
                        "append" => "SAND-E0720",
                        "checkpoint" => "SAND-E0721",
                        "replay" => "SAND-E0723",
                        _ => "SAND-E0001",
                    })
                });
                let report = JournalOutcomeReport {
                    outcome: outcome.outcome.clone(),
                    op: outcome.op.clone(),
                    conversation_id: outcome.conversation_id.clone(),
                    entry_count: outcome
                        .entry_count
                        .map(|value| value.min(i64::MAX as usize) as i64),
                    bytes: outcome
                        .bytes
                        .map(|value| value.min(i64::MAX as u64) as i64),
                    duration_ms: outcome.duration_ms,
                    cause,
                };
                let _ = journal_logs.report_journal_outcome(&report);
            }),
            Arc::new(move || {
                Ok(journal_experiments.check_feature_gate("sand_new_transcript_journal"))
            }),
        )
        .map_err(|error| GatewayCommandError::Internal(format!(
            "could not initialize production Runner checkpoint composition for {agent_id}: {error}"
        )))?;

    let group_room_id = is_group_member_turn
        .then(|| {
            args.get("groupRoomId")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .flatten();
    let group_member_name = is_group_member_turn
        .then(|| {
            args.get("groupMemberName")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .flatten();
    transcript_runtime.begin_provider_run_with_kind(&agent_id, is_group_member_turn);
    let supplied_ack_token = args
        .get("ackToken")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let ack_token = if let Some(ack_token) = supplied_ack_token {
        if !ack_obligations
            .is_ack_run_token_for_agent(&agent_id, ack_token)
            .map_err(|error| GatewayCommandError::Internal(error.to_string()))?
        {
            transcript_runtime.end_provider_run_with_kind(&agent_id, is_group_member_turn);
            let _ = transcript_runtime.retire_idle_live_session(
                &session_workers,
                &agent_id,
            );
            return Err(GatewayCommandError::BadRequest(
                "runner ackToken is not owned by this agent".into(),
            ));
        }
        Some(ack_token.to_string())
    } else {
        ack_obligations
            .mint_ack_run_token(&agent_id)
            .map_err(|error| {
                transcript_runtime.end_provider_run_with_kind(&agent_id, is_group_member_turn);
                let _ = transcript_runtime.retire_idle_live_session(
                    &session_workers,
                    &agent_id,
                );
                GatewayCommandError::Internal(format!(
                    "could not mint ack run token for {agent_id}: {error}"
                ))
            })?
    };
    turn_input.ack_token = ack_token.clone();
    let recovery_shaped = lifecycle_messages
        .iter()
        .rev()
        .find(|message| message.role == "user" && !message.content.trim().is_empty())
        .is_some_and(|message| is_recovery_shaped_turn(&message.content, &turn_input.options));
    let cancellation = (if is_group_member_turn {
        runner_registry.register_group_member(&agent_id, &stream_id)
    } else {
        runner_registry.register_routed_provider_with_recovery_shape(
            &agent_id,
            &stream_id,
            recovery_shaped,
        )
    })
        .map_err(|error| {
            ack_obligations.retire_ack_run_token(&agent_id, ack_token.as_deref());
            transcript_runtime.end_provider_run_with_kind(&agent_id, is_group_member_turn);
            let _ = transcript_runtime.retire_idle_live_session(
                &session_workers,
                &agent_id,
            );
            GatewayCommandError::Internal(error.to_string())
        })?;
    if !is_group_member_turn {
        host_runner_composition.bind_local_permission_surface(&agent_id);
    }
    let approvals_resolvable = !matches!(
        request_source.as_deref(),
        Some("group-member" | "automation")
    );
    let auto_review_controller = auto_review.bind_runner_with_session(
        &agent_id,
        worker_session_admitted_key.as_deref(),
        approvals_resolvable,
    );
    let auto_review_cancellation = cancellation.clone();
    auto_review_controller.set_cancellation_probe(Arc::new(move || {
        auto_review_cancellation.is_cancelled()
    }));
    let auto_review_service = auto_review.service();
    let auto_review_request_source = request_source
        .clone()
        .unwrap_or_else(|| "turn".to_string());
    let auto_review_context = lifecycle_messages.clone();
    let checkpoint_store = Arc::new(
        ProductionRoutedProviderCheckpointStore::new(
            &data_dir,
            &agent_id,
            &stream_id,
        ),
    );
    let resolved_request_context = request_context.resolve();
    let worker_events = events.clone();
    let accepted_stream_id = stream_id.clone();
    let worker_stream_id = stream_id.clone();
    let worker_registry = Arc::clone(&runner_registry);
    let worker_mcp_service = Arc::clone(&mcp_service);
    let worker_host_runner_composition = Arc::clone(&host_runner_composition);
    let worker_cancellation = cancellation.clone();
    let worker_sessions = Arc::clone(&session_workers);
    let worker_human_handoff_sessions = Arc::clone(&session_workers);
    let worker_human_handoff_session_key = worker_session_admitted_key.clone();
    let worker_retire_sessions = Arc::clone(&session_workers);
    let worker_ack_obligations = Arc::clone(&ack_obligations);
    let worker_transcript_runtime = Arc::clone(&transcript_runtime);
    let worker_transcript_manager = Arc::clone(&transcript_manager);
    let worker_ack_token = ack_token.clone();
    let worker_trays = Arc::clone(&trays);
    let worker_telemetry_logs = telemetry_logs.clone();
    let worker_request_source = request_source.clone();
    let worker_provider_review_lifecycle_revision = provider_review_lifecycle_revision.clone();
    let worker_human_handoff_conversation_id = human_handoff_conversation_id.clone();
    let worker_group_room_id = group_room_id.clone();
    let worker_group_member_name = group_member_name.clone();
    let worker_is_handoff_resume = worker_request_source.as_deref() == Some("handoff-resume");
    let persisted_model_override = persisted_session_model
        .model_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let worker_session_options = SandSessionOptions {
        model_id: persisted_model_override
            .clone()
            .or_else(|| args
            .get("modelId")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)),
        session_model_override: persisted_model_override.is_some(),
        thinking_level: persisted_session_model.thinking_level.clone(),
        is_summarization_session: args
            .get("isSummarizationSession")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        is_computer_use_subagent: args
            .get("isComputerUseSubagent")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        is_browser_use_subagent: args
            .get("isBrowserUseSubagent")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        request_source: request_source.clone(),
        conversation_id: Some(agent_id.clone()),
        inference_reason: match args
            .get("inferenceReason")
            .and_then(serde_json::Value::as_i64)
        {
            Some(1) => Some(InferenceReason::GeminiVideoSubagent),
            _ => None,
        },
        lineage: args
            .get("lineage")
            .and_then(serde_json::Value::as_object)
            .and_then(|lineage| {
                let parent_request_id = lineage
                    .get("parentRequestId")?
                    .as_str()?
                    .trim()
                    .to_string();
                let root_parent_request_id = lineage
                    .get("rootParentRequestId")?
                    .as_str()?
                    .trim()
                    .to_string();
                if parent_request_id.is_empty() || root_parent_request_id.is_empty() {
                    return None;
                }
                Some(RequestLineage {
                    parent_request_id,
                    root_parent_request_id,
                    parent_agent_tool_call_id: lineage
                        .get("parentAgentToolCallId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                })
            }),
        skip_labeling: args
            .get("skipLabeling")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    };
    let worker_group_post_deps = LocalRoutedRunnerDeps {
        routed_tool_relay: Arc::clone(&routed_tool_relay),
        mcp_service: Arc::clone(&mcp_service),
        auth: Arc::clone(&auth),
        auto_review: Arc::clone(&auto_review),
        events: events.clone(),
        host_tx: host_tx.clone(),
        data_dir: data_dir.clone(),
        request_context: Arc::clone(&request_context),
        experiments: Arc::clone(&experiments),
        settings: Arc::clone(&settings),
        inference: Arc::clone(&inference),
        session_workers: Arc::clone(&session_workers),
        runner_registry: Arc::clone(&runner_registry),
        ack_obligations: Arc::clone(&ack_obligations),
        transcript_runtime: Arc::clone(&transcript_runtime),
        transcript_manager: Arc::clone(&transcript_manager),
        generated_agent_runtime: Arc::clone(&generated_agent_runtime),
        completion_revivals: Arc::clone(&completion_revivals),
        forever_box: Arc::clone(&forever_box),
        local_exec: Arc::clone(&local_exec),
        local_tool_permission: Arc::clone(&local_tool_permission),
        session_handoff: session_handoff.clone(),
        trays: Arc::clone(&trays),
        telemetry_logs: telemetry_logs.clone(),
        product_analytics: product_analytics.clone(),
        production_action_auditor: production_action_auditor.clone(),
        cloud_agents: Arc::clone(&cloud_agents),
        cloud_agent_watches: Arc::clone(&cloud_agent_watches),
        background_shell_watches: Arc::clone(&background_shell_watches),
        host_runner_composition: Arc::clone(&host_runner_composition),
        box_store_sync: box_store_sync.clone(),
        automations_lifecycle: Arc::clone(&automations_lifecycle),
        generated_subagent_cleanup: Arc::clone(&generated_subagent_cleanup),
    };
    let worker_generated_parent_agent_id = generated_parent_agent_id.clone();
    let worker_generated_subagent_type = generated_subagent_type.clone();
    let worker_generated_tool_call_id = generated_tool_call_id.clone();
    let worker_generated_lineage = worker_session_options.lineage.as_ref().map(|lineage| {
        GeneratedSubagentLineage {
            parent_request_id: Some(lineage.parent_request_id.clone()),
            root_parent_request_id: Some(lineage.root_parent_request_id.clone()),
            parent_agent_tool_call_id: lineage.parent_agent_tool_call_id.clone(),
        }
    });
    let worker_subagent_task_sink: Option<Arc<dyn SubagentTaskSink>> =
        if generated_parent_agent_id.is_none() {
            let root_parent_request_id = worker_session_options
                .lineage
                .as_ref()
                .map(|lineage| lineage.root_parent_request_id.clone())
                .unwrap_or_else(|| stream_id.clone());
            Some(Arc::new(ProductionSubagentTaskSink {
                deps: LocalRoutedRunnerDeps {
                    routed_tool_relay: Arc::clone(&routed_tool_relay),
                    mcp_service: Arc::clone(&mcp_service),
                    auth: Arc::clone(&auth),
                    auto_review: Arc::clone(&auto_review),
                    events: events.clone(),
                    host_tx: host_tx.clone(),
                    data_dir: data_dir.clone(),
                    request_context: Arc::clone(&request_context),
                    experiments: Arc::clone(&experiments),
                    settings: Arc::clone(&settings),
                    inference: Arc::clone(&inference),
                    session_workers: Arc::clone(&session_workers),
                    runner_registry: Arc::clone(&runner_registry),
                    ack_obligations: Arc::clone(&ack_obligations),
                    transcript_runtime: Arc::clone(&transcript_runtime),
                    transcript_manager: Arc::clone(&transcript_manager),
                    generated_agent_runtime: Arc::clone(&generated_agent_runtime),
                    completion_revivals: Arc::clone(&completion_revivals),
                    forever_box: Arc::clone(&forever_box),
                    local_exec: Arc::clone(&local_exec),
                    local_tool_permission: Arc::clone(&local_tool_permission),
                    session_handoff: session_handoff.clone(),
                    trays: Arc::clone(&trays),
                    telemetry_logs: telemetry_logs.clone(),
                    product_analytics: product_analytics.clone(),
                    production_action_auditor: production_action_auditor.clone(),
                    cloud_agents: Arc::clone(&cloud_agents),
                    cloud_agent_watches: Arc::clone(&cloud_agent_watches),
                    background_shell_watches: Arc::clone(&background_shell_watches),
                    host_runner_composition: Arc::clone(&host_runner_composition),
                    box_store_sync: box_store_sync.clone(),
                    automations_lifecycle: Arc::clone(&automations_lifecycle),
                    generated_subagent_cleanup: Arc::clone(&generated_subagent_cleanup),
                },
                parent_agent_id: agent_id.clone(),
                provider,
                parent_stream_id: stream_id.clone(),
                root_parent_request_id,
            }))
        } else {
            None
        };
    let worker_subagent_management_runtime = generated_parent_agent_id
        .is_none()
        .then(|| Arc::clone(&generated_agent_runtime));
    let worker_is_ack_redrive = is_ack_redrive;
    let worker_is_upgrade_resume = is_upgrade_resume;
    let worker_is_kickstart = request_source.as_deref() == Some("kickstart");
    let worker_profile_announcement = pending_profile_announcement;
    let worker_memory_store = memory_store.clone();
    let worker_memory_service = session_workers.memory_service();
    let worker_generated_cleanup_slot = Arc::clone(&generated_subagent_cleanup);
    let worker_turn_hidden = turn_hidden;
    let worker_turn_trace_type = resolve_turn_trace_type(TurnTraceTypeOptions {
        automation_wake: args.get("automationWake"),
        request_source: request_source.as_deref(),
        hidden: turn_hidden,
    });
    let worker_gateway_context = gateway_context;
    let worker_enter_epoch_ms = args
        .get("enterEpochMs")
        .and_then(serde_json::Value::as_f64);
    let worker_reply_thread_target = args
        .get("replyContext")
        .and_then(|context| context.get("targetId"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let worker_send_is_fork = turn_input.options.is_fork;
    let avatar_box_file_reader: AvatarBoxFileReader = {
        let forever_box = Arc::clone(&forever_box);
        let avatar_agent_id = agent_id.clone();
        Arc::new(move |path: &std::path::Path| {
            let path = path
                .to_str()
                .ok_or_else(|| "avatar box path must be valid UTF-8".to_string())?;
            forever_box
                .download_file(&(), &avatar_agent_id, path)
                .map_err(|error| error.to_string())
        })
    };
    let turn_state_surfaces = host_runner_composition
        .compose_turn_state_surfaces(
            Arc::clone(&session_workers),
            &agent_id,
            is_group_member_turn,
            multitask_enabled,
            Some(avatar_box_file_reader),
        )
        .map_err(|error| GatewayCommandError::Internal(format!(
            "could not compose production turn state surfaces for {agent_id}: {error}"
        )))?;
    let state_writer = turn_state_surfaces.state_writer;
    let multitask_todo_state = turn_state_surfaces.multitask_todo_state;
    let cloud_agent_dir = session_workers
        .session_db_path(&agent_id)
        .map_err(GatewayCommandError::Internal)?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| GatewayCommandError::Internal(
            format!("agent database path has no parent for {agent_id}")
        ))?;
    let worker_cloud_agents = Arc::clone(&cloud_agents);
    let worker_cloud_agent_watches = Arc::clone(&cloud_agent_watches);
    let worker_async_cloud_agent_watches = Arc::clone(&cloud_agent_watches);
    let worker_async_background_shell_watches = Arc::clone(&background_shell_watches);
    let worker_box_store_sync = box_store_sync.clone();
    let worker_generated_agent_runtime = Arc::clone(&generated_agent_runtime);
    let worker_async_generated_agent_runtime = Arc::clone(&generated_agent_runtime);
    let worker_completion_revivals = Arc::clone(&completion_revivals);
    let cloud_agent_quiet_origin = args
        .get("quietOrigin")
        .filter(|origin| origin.is_object())
        .cloned()
        .or_else(|| {
            args.get("automationWake")
                .and_then(serde_json::Value::as_object)
                .and_then(|wake| {
                    let id = wake.get("id").and_then(serde_json::Value::as_str)?;
                    let name = wake.get("name").and_then(serde_json::Value::as_str)?;
                    Some(serde_json::json!({
                        "automation": {
                            "id": id,
                            "name": name,
                        }
                    }))
                })
        })
        .or_else(|| {
            args.get("isSilenceAllowed")
                .and_then(serde_json::Value::as_bool)
                .filter(|allowed| *allowed)
                .map(|_| serde_json::json!({}))
        });
    let worker_auth = Arc::clone(&auth);
    let worker_auto_review = Arc::clone(&auto_review);
    let worker_auto_review_controller = Arc::clone(&auto_review_controller);
    let spawn_error_auto_review = Arc::clone(&auto_review_service);
    let spawn_error_agent_id = agent_id.clone();
    let spawn = thread::Builder::new()
        .name(format!("mahayana-runner-provider-{agent_id}"))
        .spawn(move || {
            let mut worker_routed_turn_lease = routed_turn_lease_guard;
            let runner_started_at_ms = started_at_ms();
            let turn_trace = begin_turn_trace(BeginTurnTraceOptions {
                conversation_id: agent_id.clone(),
                turn_type: worker_turn_trace_type.clone(),
                traceparent: worker_gateway_context
                    .as_ref()
                    .and_then(|context| context.traceparent.clone()),
                parent_ctx: None,
                start_time: Some(runner_started_at_ms as f64),
                sample_ratio: None,
                attributes: {
                    let mut attributes = BTreeMap::from([(
                        "sand.is_fork".into(),
                        serde_json::json!(worker_send_is_fork),
                    )]);
                    if let Some(model_id) = worker_session_options.model_id.clone() {
                        attributes.insert("sand.model_id".into(), serde_json::json!(model_id));
                    }
                    attributes
                },
            });
            let turn_type = worker_request_source
                .clone()
                .unwrap_or_else(|| "turn".to_string());
            let structured_turn = Arc::new(Mutex::new(worker_telemetry_logs.start_turn(
                agent_id.clone(),
                turn_type,
                worker_session_options.model_id.clone(),
            )));
            if let Ok(mut turn) = structured_turn.lock() {
                turn.set_request_id(worker_stream_id.clone());
            }
            let observation_events = worker_events.clone();
            let observation: TurnObservationHandle = TurnObservation::shared(
                agent_id.clone(),
                Some(Arc::new(move |event| {
                    observation_events.publish(serde_json::json!({
                        "channel": "runner-turn-observation",
                        "payload": event,
                    }));
                })),
            );
            let send_dispatch_logs = worker_telemetry_logs.clone();
            let send_dispatch_conversation_id = agent_id.clone();
            let send_dispatch_trace_id = turn_trace
                .as_ref()
                .and_then(|trace| trace.trace_id.clone());
            let send_dispatch_span_id = turn_trace
                .as_ref()
                .and_then(|trace| trace.span_id.clone());
            let send_dispatch_turn_trace = turn_trace.clone();
            let await_telemetry_logs = worker_telemetry_logs.clone();
            let await_conversation_id = agent_id.clone();
            let ttft_telemetry_logs = worker_telemetry_logs.clone();
            let ttft_conversation_id = agent_id.clone();
            let ttft_trace_id = turn_trace
                .as_ref()
                .and_then(|trace| trace.trace_id.clone())
                .unwrap_or_default();
            let ttft_span_id = turn_trace
                .as_ref()
                .and_then(|trace| trace.span_id.clone())
                .unwrap_or_default();
            let ttft_turn_trace = turn_trace.clone();
            if let Ok(mut observation) = observation.lock() {
                observation.set_request_id(Some(worker_stream_id.clone()));
                let async_tasks_runtime = Arc::clone(&worker_transcript_runtime);
                let async_tasks_subagents = Arc::clone(&worker_async_generated_agent_runtime);
                let async_tasks_cloud = Arc::clone(&worker_async_cloud_agent_watches);
                let async_tasks_shell = Arc::clone(&worker_async_background_shell_watches);
                observation.set_async_tasks_provider(Arc::new(move |owner_agent_id| {
                    let live_tasks = collect_live_async_tasks(
                        async_tasks_subagents.as_ref(),
                        async_tasks_cloud.as_ref(),
                        async_tasks_shell.as_ref(),
                        owner_agent_id,
                    );
                    async_tasks_runtime.get_async_tasks(owner_agent_id, &live_tasks)
                }));
                let async_tasks_events = worker_events.clone();
                observation.set_async_tasks_event_handler(Arc::new(move |payload| {
                    async_tasks_events.publish(serde_json::json!({
                        "channel": "async-tasks",
                        "payload": payload,
                    }));
                }));
                let tool_call_logs = worker_telemetry_logs.clone();
                let client_side_tool_events = worker_events.clone();
                let client_side_tool_agent_id = agent_id.clone();
                let client_side_tool_manager = Arc::clone(&worker_transcript_manager);
                observation.set_client_side_tool_v2_handler(Arc::new(move |projected| {
                    if let Some(event) = client_side_tool_manager.publish_client_side_tool_v2(
                        &client_side_tool_agent_id,
                        projected.into_produced_value(),
                    )
                    {
                        client_side_tool_events.publish(serde_json::json!({
                            "channel": mahayana_host_runtime::extensions::transcript::client_side_tool_v2_producer::CLIENT_SIDE_TOOL_V2_FAMILY,
                            "payload": event,
                        }));
                    }
                }));
                observation.set_tool_call_telemetry_handler(Arc::new(move |event| {
                    let result = match event {
                        ToolCallTelemetryEvent::Started {
                            conversation_id,
                            request_id,
                            tool_name,
                            tool_call_id,
                            surface,
                        } => tool_call_logs.report_tool_call_started(
                            &conversation_id,
                            request_id.as_deref(),
                            &tool_name,
                            &tool_call_id,
                            &surface,
                        ),
                        ToolCallTelemetryEvent::Error {
                            conversation_id,
                            request_id,
                            tool_name,
                            tool_call_id,
                            error_class,
                            duration_ms,
                            connector,
                        } => tool_call_logs.report_tool_call_error(
                            &conversation_id,
                            request_id.as_deref(),
                            &tool_name,
                            &tool_call_id,
                            &error_class,
                            duration_ms,
                            &connector,
                        ),
                        ToolCallTelemetryEvent::Stalled {
                            conversation_id,
                            request_id,
                            tool_name,
                            tool_call_id,
                            connector,
                            elapsed_ms,
                        } => tool_call_logs.report_tool_call_stalled(
                            &conversation_id,
                            request_id.as_deref(),
                            &tool_name,
                            &tool_call_id,
                            &connector,
                            elapsed_ms,
                        ),
                    };
                    if let Err(error) = result {
                        eprintln!("mahayana-host tool_call_telemetry_failed error={error}");
                    }
                }));
                observation.set_send_dispatch_handler(Arc::new(move |event| {
                    let mut trace_attributes = BTreeMap::new();
                    if let Some(dispatch_ms) = event
                        .get("dispatchMs")
                        .and_then(serde_json::Value::as_f64)
                    {
                        trace_attributes.insert(
                            "sand.send_dispatch_ms".into(),
                            serde_json::json!(dispatch_ms),
                        );
                    } else if let Some(reason) = event
                        .get("skewReason")
                        .and_then(serde_json::Value::as_str)
                    {
                        trace_attributes.insert(
                            "sand.send_dispatch_skew_reason".into(),
                            serde_json::json!(reason),
                        );
                    }
                    if let Some(host_dispatch_ms) = event
                        .get("hostDispatchMs")
                        .and_then(serde_json::Value::as_f64)
                    {
                        trace_attributes.insert(
                            "sand.send_dispatch_host_ms".into(),
                            serde_json::json!(host_dispatch_ms),
                        );
                    }
                    set_turn_trace_attributes(
                        send_dispatch_turn_trace.as_ref(),
                        &trace_attributes,
                    );
                    let report = SendDispatchReport {
                        conversation_id: send_dispatch_conversation_id.clone(),
                        dispatch_ms: event.get("dispatchMs").and_then(serde_json::Value::as_f64),
                        host_dispatch_ms: event.get("hostDispatchMs").and_then(serde_json::Value::as_f64).unwrap_or_default(),
                        skew: event.get("skew").and_then(serde_json::Value::as_bool).unwrap_or(false).to_string(),
                        skew_reason: event.get("skewReason").and_then(serde_json::Value::as_str).map(str::to_string),
                        skew_bucket: event.get("skewBucket").and_then(serde_json::Value::as_str).map(str::to_string),
                        is_fork: event.get("isFork").and_then(serde_json::Value::as_bool).unwrap_or(false),
                        model_id: event.get("modelId").and_then(serde_json::Value::as_str).map(str::to_string),
                        trace_id: send_dispatch_trace_id.clone(),
                        span_id: send_dispatch_span_id.clone(),
                    };
                    if let Err(error) = send_dispatch_logs.report_send_dispatch(&report) {
                        eprintln!("mahayana-host send_dispatch_telemetry_failed agent={} error={error}", send_dispatch_conversation_id);
                    }
                }));
                observation.set_first_token_handler(Arc::new(move |event| {
                    let Some(chunk_type) = event
                        .get("chunkType")
                        .and_then(serde_json::Value::as_str)
                    else {
                        return;
                    };
                    let mut trace_attributes = BTreeMap::new();
                    if let Some(ttft_ms) = event
                        .get("ttftMs")
                        .and_then(serde_json::Value::as_f64)
                    {
                        trace_attributes.insert("sand.ttft_ms".into(), serde_json::json!(ttft_ms));
                    } else if let Some(reason) = event
                        .get("skewReason")
                        .and_then(serde_json::Value::as_str)
                    {
                        trace_attributes.insert(
                            "sand.ttft_skew_reason".into(),
                            serde_json::json!(reason),
                        );
                    }
                    set_turn_trace_attributes(ttft_turn_trace.as_ref(), &trace_attributes);
                    add_turn_trace_event(
                        ttft_turn_trace.as_ref(),
                        "first_token",
                        &BTreeMap::from([(
                            "chunk_type".into(),
                            serde_json::json!(chunk_type),
                        )]),
                    );
                    let fields = TtftFields {
                        conversation_id: ttft_conversation_id.clone(),
                        ttft_ms: event.get("ttftMs").and_then(serde_json::Value::as_f64),
                        skew: event
                            .get("skew")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        skew_reason: event
                            .get("skewReason")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        chunk_type: chunk_type.to_string(),
                        is_fork: event
                            .get("isFork")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        model_id: event
                            .get("modelId")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        trace_id: ttft_trace_id.clone(),
                        span_id: ttft_span_id.clone(),
                    };
                    if let Err(error) = ttft_telemetry_logs.report_ttft(&fields) {
                        eprintln!(
                            "mahayana-host ttft_telemetry_failed agent={} error={error}",
                            ttft_conversation_id
                        );
                    }
                }));
                observation.set_turn_await_handler(Arc::new(move |event| {
                    let Some(await_index) = event
                        .get("awaitIndex")
                        .and_then(serde_json::Value::as_u64)
                    else {
                        return;
                    };
                    let Some(block_until_ms) = event
                        .get("blockUntilMs")
                        .and_then(serde_json::Value::as_u64)
                    else {
                        return;
                    };
                    let Some(outcome) = event
                        .get("outcome")
                        .and_then(serde_json::Value::as_str)
                    else {
                        return;
                    };
                    let fields = TurnAwaitFields {
                        conversation_id: await_conversation_id.clone(),
                        block_until_ms,
                        outcome: outcome.to_string(),
                        await_index,
                    };
                    if let Err(error) = await_telemetry_logs.report_turn_await(&fields) {
                        eprintln!(
                            "mahayana-host turn_await_telemetry_failed agent={} error={error}",
                            await_conversation_id
                        );
                    }
                }));
                observation.turn_started(runner_started_at_ms);
            }
            let auto_review_gate = Arc::new(AutoReviewGate::new(Arc::new(
                ProductionAutoReviewGateDeps {
                    auto_review: Arc::clone(&worker_auto_review),
                    controller: Arc::clone(&worker_auto_review_controller),
                    box_id: agent_id.clone(),
                },
            )));
            let _ = auto_review_gate.current_modes();

            let mcp_review_gate = Arc::clone(&auto_review_gate);
            let mcp_review_auth = Arc::clone(&worker_auth);
            let mcp_review_controller = Arc::clone(&worker_auto_review_controller);
            let mcp_review_cancellation = worker_cancellation.clone();
            let mcp_review_agent_id = agent_id.clone();
            let mcp_review_request_source = auto_review_request_source.clone();
            let mcp_review_context = auto_review_context
                .iter()
                .map(|message| {
                    serde_json::json!({
                        "role": message.role,
                        "content": message.content,
                    })
                })
                .collect::<Vec<_>>();
            let mcp_review: RoutedMcpAutoReviewCallback = Arc::new(
                move |tool: &RoutedToolDefinition,
                      args: &serde_json::Value,
                      tool_call_id: &str| {
                    mcp_review_gate.assert_no_pending_approval().map_err(|error| {
                        ProviderSessionError::Tool(error.to_string())
                    })?;
                    let mode = mcp_review_gate.current_modes().mcp;
                    if mode == SandAutoReviewMode::Off {
                        return Ok(None);
                    }
                    let instructions = mcp_review_gate.user_instructions();
                    let risk_target = serde_json::json!({
                        "action": "mcp",
                        "arguments": {
                            "server_display_name": tool.provider_identifier,
                            "tool_name": if tool.tool_name.trim().is_empty() { &tool.name } else { &tool.tool_name },
                            "mcp_arguments": args,
                            "allow_instructions": instructions.as_ref().map(|value| &value.allow_instructions),
                            "block_instructions": instructions.as_ref().map(|value| &value.block_instructions),
                        }
                    });
                    let classifier_cancellation = mcp_review_cancellation.clone();
                    let mut classifier =
                        create_sand_backend_smart_mode_classifier_executor_with_cancellation(
                            Arc::clone(&mcp_review_auth),
                            Arc::new(move || classifier_cancellation.is_cancelled()),
                        )
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                    let classifier_mode = if mode == SandAutoReviewMode::Shadow {
                        "shadow"
                    } else {
                        "enforce"
                    };
                    let decision = run_sand_auto_review_classifier(
                        &mut classifier,
                        tool_call_id,
                        &mcp_review_agent_id,
                        classifier_mode,
                        || risk_target.clone(),
                        || Ok(mcp_review_context.clone()),
                        &[],
                        "Auto-review could not classify this MCP action.",
                    )
                    .map_err(|error| ProviderSessionError::Tool(format!("{error:?}")))?;
                    if mode == SandAutoReviewMode::Shadow {
                        return Ok(None);
                    }
                    let (reason, proposed_rule) = match decision {
                        AutoReviewClassifierDecision::Allow => return Ok(None),
                        AutoReviewClassifierDecision::Reject { reason } => {
                            return Ok(Some(reason));
                        }
                        AutoReviewClassifierDecision::Block {
                            reason,
                            proposed_rule,
                        } => (reason, proposed_rule),
                    };
                    let decision = request_sand_mcp_approval(
                        mcp_review_controller.as_ref(),
                        &McpApprovalRequest {
                            fingerprint: fingerprint_sand_auto_review_target(&risk_target),
                            reason: reason.clone(),
                            server_display_name: tool.provider_identifier.clone(),
                            tool_name: if tool.tool_name.trim().is_empty() {
                                tool.name.clone()
                            } else {
                                tool.tool_name.clone()
                            },
                            mcp_arguments: Some(args.clone()),
                            description: tool.description.clone(),
                            proposed_rule,
                            expiry_policy: sand_auto_review_approval_expiry_policy(
                                &mcp_review_request_source,
                            ),
                        },
                    )
                    .map_err(ProviderSessionError::Tool)?;
                    Ok(match decision {
                        SandAutoReviewDecision::Approved => None,
                        SandAutoReviewDecision::Denied { reason: denied } => Some(
                            if denied.trim().is_empty() { reason } else { denied },
                        ),
                    })
                },
            );
            let bridge: Arc<dyn RoutedToolBridge> = Arc::new(CoordinatorRoutedToolBridge {
                relay: routed_tool_relay,
                transcript_runtime: Arc::clone(&worker_transcript_runtime),
                telemetry_logs: worker_telemetry_logs.clone(),
                agent_id: agent_id.clone(),
                mcp_review: Some(mcp_review),
            });
            let box_resources: Arc<dyn RunnerBoxResourcePort> =
                shipping_box_resources.clone();
            let box_shell_gate = Arc::clone(&auto_review_gate);
            let box_shell_auth = Arc::clone(&worker_auth);
            let box_shell_controller = Arc::clone(&worker_auto_review_controller);
            let box_shell_cancellation = worker_cancellation.clone();
            let box_shell_agent_id = agent_id.clone();
            let box_shell_request_source = auto_review_request_source.clone();
            let box_shell_context = auto_review_context
                .iter()
                .map(|message| {
                    serde_json::json!({
                        "role": message.role,
                        "content": message.content,
                    })
                })
                .collect::<Vec<_>>();
            let box_shell_review: BoxShellAutoReviewCallback = Arc::new(
                move |request: &RunnerBoxShellRequest| {
                    box_shell_gate.assert_no_pending_approval().map_err(|error| {
                        ProviderSessionError::Tool(error.to_string())
                    })?;
                    let mode = box_shell_gate.current_modes().box_shell;
                    let approval_identity =
                        box_shell_gate.shell_approval_identity(ShellApprovalSurface::BoxShell);
                    if mode == SandAutoReviewMode::Off {
                        box_shell_gate.mark_shell_side_effect_start(ShellApprovalSurface::BoxShell);
                        return Ok(None);
                    }
                    let instructions = box_shell_gate.user_instructions();
                    let risk_target = serde_json::json!({
                        "action": "shell",
                        "arguments": {
                            "command": request.command,
                            "working_directory": request.working_directory,
                            "surface": "isolated_box",
                            "approval_identity": approval_identity,
                            "allow_instructions": instructions.as_ref().map(|value| &value.allow_instructions),
                            "block_instructions": instructions.as_ref().map(|value| &value.block_instructions),
                        }
                    });
                    let classifier_cancellation = box_shell_cancellation.clone();
                    let mut classifier =
                        create_sand_backend_smart_mode_classifier_executor_with_cancellation(
                            Arc::clone(&box_shell_auth),
                            Arc::new(move || classifier_cancellation.is_cancelled()),
                        )
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                    let classifier_mode = if mode == SandAutoReviewMode::Shadow {
                        "shadow"
                    } else {
                        "enforce"
                    };
                    let decision = run_sand_auto_review_classifier(
                        &mut classifier,
                        &request.tool_call_id,
                        &box_shell_agent_id,
                        classifier_mode,
                        || risk_target.clone(),
                        || Ok(box_shell_context.clone()),
                        &[],
                        "Auto-review could not classify this shell command.",
                    )
                    .map_err(|error| ProviderSessionError::Tool(format!("{error:?}")))?;
                    if mode == SandAutoReviewMode::Shadow {
                        box_shell_gate.mark_shell_side_effect_start(ShellApprovalSurface::BoxShell);
                        return Ok(None);
                    }
                    let (reason, proposed_rule) = match decision {
                        AutoReviewClassifierDecision::Allow => {
                            box_shell_gate.mark_shell_side_effect_start(
                                ShellApprovalSurface::BoxShell,
                            );
                            return Ok(None);
                        }
                        AutoReviewClassifierDecision::Reject { reason } => {
                            return Ok(Some(reason));
                        }
                        AutoReviewClassifierDecision::Block {
                            reason,
                            proposed_rule,
                        } => (reason, proposed_rule),
                    };
                    let identity_before_approval = approval_identity.clone();
                    let recheck_gate = Arc::clone(&box_shell_gate);
                    let decision = request_sand_shell_approval(
                        box_shell_controller.as_ref(),
                        &ShellApprovalRequest {
                            target: ShellApprovalTarget {
                                surface: mahayana_host_runtime::runner::sand_auto_review::SandAutoReviewSurface::BoxShell,
                                description: Some("Run a shell command in the isolated box".into()),
                                working_directory: Some(request.working_directory.clone()),
                            },
                            fingerprint: fingerprint_sand_auto_review_target(&risk_target),
                            reason: reason.clone(),
                            command: request.command.clone(),
                            proposed_rule,
                            expiry_policy: sand_auto_review_approval_expiry_policy(
                                &box_shell_request_source,
                            ),
                        },
                        |_| Ok(()),
                        move |_| {
                            let current = recheck_gate
                                .shell_approval_identity(ShellApprovalSurface::BoxShell);
                            if current == identity_before_approval {
                                Ok(())
                            } else {
                                Err("Shell approval context changed while awaiting review".into())
                            }
                        },
                    )
                    .map_err(ProviderSessionError::Tool)?;
                    Ok(match decision {
                        SandAutoReviewDecision::Approved => {
                            box_shell_gate.mark_shell_side_effect_start(
                                ShellApprovalSurface::BoxShell,
                            );
                            None
                        }
                        SandAutoReviewDecision::Denied { reason: denied } => Some(
                            if denied.trim().is_empty() { reason } else { denied },
                        ),
                    })
                },
            );

            let external_shell_gate = Arc::clone(&auto_review_gate);
            let external_shell_auth = Arc::clone(&worker_auth);
            let external_shell_controller = Arc::clone(&worker_auto_review_controller);
            let external_shell_cancellation = worker_cancellation.clone();
            let external_shell_agent_id = agent_id.clone();
            let external_shell_request_source = auto_review_request_source.clone();
            let external_shell_context = auto_review_context
                .iter()
                .map(|message| serde_json::json!({"role": message.role, "content": message.content}))
                .collect::<Vec<_>>();
            let external_shell_review: ExternalShellAutoReviewCallback = Arc::new(
                move |request: &ExternalMachineShellArgs| {
                    external_shell_gate.assert_no_pending_approval()
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                    let mode = external_shell_gate.current_modes().host_shell;
                    let approval_identity =
                        external_shell_gate.shell_approval_identity(ShellApprovalSurface::HostShell);
                    if mode == SandAutoReviewMode::Off {
                        external_shell_gate.mark_shell_side_effect_start(ShellApprovalSurface::HostShell);
                        return Ok(None);
                    }
                    let instructions = external_shell_gate.user_instructions();
                    let risk_target = serde_json::json!({
                        "action": "shell",
                        "arguments": {
                            "command": request.command,
                            "working_directory": request.working_directory,
                            "surface": "host_machine",
                            "approval_identity": approval_identity,
                            "allow_instructions": instructions.as_ref().map(|value| &value.allow_instructions),
                            "block_instructions": instructions.as_ref().map(|value| &value.block_instructions),
                        }
                    });
                    let classifier_cancellation = external_shell_cancellation.clone();
                    let mut classifier =
                        create_sand_backend_smart_mode_classifier_executor_with_cancellation(
                            Arc::clone(&external_shell_auth),
                            Arc::new(move || classifier_cancellation.is_cancelled()),
                        )
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                    let classifier_mode = if mode == SandAutoReviewMode::Shadow { "shadow" } else { "enforce" };
                    let decision = run_sand_auto_review_classifier(
                        &mut classifier,
                        &request.tool_call_id,
                        &external_shell_agent_id,
                        classifier_mode,
                        || risk_target.clone(),
                        || Ok(external_shell_context.clone()),
                        &[],
                        "Auto-review could not classify this shell command.",
                    )
                    .map_err(|error| ProviderSessionError::Tool(format!("{error:?}")))?;
                    if mode == SandAutoReviewMode::Shadow {
                        external_shell_gate.mark_shell_side_effect_start(ShellApprovalSurface::HostShell);
                        return Ok(None);
                    }
                    let (reason, proposed_rule) = match decision {
                        AutoReviewClassifierDecision::Allow => {
                            external_shell_gate.mark_shell_side_effect_start(ShellApprovalSurface::HostShell);
                            return Ok(None);
                        }
                        AutoReviewClassifierDecision::Reject { reason } => return Ok(Some(reason)),
                        AutoReviewClassifierDecision::Block { reason, proposed_rule } => (reason, proposed_rule),
                    };
                    let identity_before_approval = approval_identity.clone();
                    let recheck_gate = Arc::clone(&external_shell_gate);
                    let decision = request_sand_shell_approval(
                        external_shell_controller.as_ref(),
                        &ShellApprovalRequest {
                            target: ShellApprovalTarget {
                                surface: mahayana_host_runtime::runner::sand_auto_review::SandAutoReviewSurface::HostShell,
                                description: Some("Run a shell command on the user's computer".into()),
                                working_directory: Some(request.working_directory.clone()),
                            },
                            fingerprint: fingerprint_sand_auto_review_target(&risk_target),
                            reason: reason.clone(),
                            command: request.command.clone(),
                            proposed_rule,
                            expiry_policy: sand_auto_review_approval_expiry_policy(&external_shell_request_source),
                        },
                        |_| Ok(()),
                        move |_| {
                            let current = recheck_gate.shell_approval_identity(ShellApprovalSurface::HostShell);
                            if current == identity_before_approval {
                                Ok(())
                            } else {
                                Err("Shell approval context changed while awaiting review".into())
                            }
                        },
                    ).map_err(ProviderSessionError::Tool)?;
                    Ok(match decision {
                        SandAutoReviewDecision::Approved => {
                            external_shell_gate.mark_shell_side_effect_start(ShellApprovalSurface::HostShell);
                            None
                        }
                        SandAutoReviewDecision::Denied { reason: denied } => {
                            Some(if denied.trim().is_empty() { reason } else { denied })
                        }
                    })
                },
            );
            let browser_media_sessions = Arc::clone(&worker_sessions);
            let computer_media_sessions = Arc::clone(&worker_sessions);
            let computer_review_box = Arc::clone(&box_resources);
            let computer_review_auth = Arc::clone(&worker_auth);
            let computer_review_gate = Arc::clone(&auto_review_gate);
            let computer_review_controller = Arc::clone(&worker_auto_review_controller);
            let computer_review_cancellation = worker_cancellation.clone();
            let computer_review_agent_id = agent_id.clone();
            let computer_review_request_source = auto_review_request_source.clone();
            let computer_review_context = auto_review_context
                .iter()
                .map(|message| {
                    serde_json::json!({
                        "role": message.role,
                        "content": message.content,
                    })
                })
                .collect::<Vec<_>>();
            let computer_review_workspace_paths = vec!["/workspace".to_string()];
            let computer_auto_review: ComputerAutoReviewCallback =
                Arc::new(move |args, tool_call_id| {
                    let mode = computer_review_gate.current_modes().computer;
                    let computer_review_instructions =
                        computer_review_gate.user_instructions().map(|instructions| {
                            InstructionPermissions {
                                allow_instructions: instructions.allow_instructions,
                                block_instructions: instructions.block_instructions,
                            }
                        });
                    validate_computer_action(args, Some(mode))
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                    let initial_display = computer_review_box.browser_window_index()?;
                    let box_identity = BoxIdentity {
                        box_id: std::env::var("SAND_BOX_HOST")
                            .ok()
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| "127.0.0.1".into()),
                        window_generation: format!(
                            "{}:{}",
                            computer_review_agent_id,
                            initial_display
                        ),
                    };
                    let exact_action = to_exact_action_value(args);
                    let classifier_cancellation = computer_review_cancellation.clone();
                    let mut classifier =
                        create_sand_backend_smart_mode_classifier_executor_with_cancellation(
                            Arc::clone(&computer_review_auth),
                            Arc::new(move || classifier_cancellation.is_cancelled()),
                        )
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                    run_sand_computer_auto_review_preflight(
                        mode,
                        &exact_action,
                        args.description.as_deref(),
                        &box_identity,
                        &computer_review_agent_id,
                        &computer_review_request_source,
                        Some(computer_review_controller.as_ref()),
                        computer_review_instructions.as_ref(),
                        || {
                            let live_display = computer_review_box
                                .browser_window_index()
                                .map_err(|error| error.to_string())?;
                            capture_browser_review_state(
                                computer_review_box.as_ref(),
                                live_display,
                                None,
                                tool_call_id,
                            )
                            .map(|state| state.display_state_identity)
                            .map_err(|error| error.to_string())
                        },
                        |risk_target, classifier_mode| {
                            run_sand_auto_review_classifier(
                                &mut classifier,
                                tool_call_id,
                                &computer_review_agent_id,
                                classifier_mode,
                                || risk_target.clone(),
                                || Ok(computer_review_context.clone()),
                                &computer_review_workspace_paths,
                                SAND_COMPUTER_AUTO_REVIEW_CLASSIFIER_ERROR_REASON,
                            )
                        },
                    )
                    .map_err(|error| ProviderSessionError::Tool(error.to_string()))
                });
            let browser_review_box = Arc::clone(&box_resources);
            let browser_review_auth = Arc::clone(&worker_auth);
            let browser_review_auto_review = Arc::clone(&worker_auto_review);
            let browser_review_controller = Arc::clone(&worker_auto_review_controller);
            let browser_review_cancellation = worker_cancellation.clone();
            let browser_review_agent_id = agent_id.clone();
            let browser_review_request_source = auto_review_request_source.clone();
            let browser_review_context = auto_review_context
                .iter()
                .map(|message| {
                    serde_json::json!({
                        "role": message.role,
                        "content": message.content,
                    })
                })
                .collect::<Vec<_>>();
            let browser_auto_review: BrowserAutoReviewCallback =
                Arc::new(move |spec, args, tool_call_id| {
                    let mode = browser_review_auto_review.current_modes().computer;
                    let initial_display = browser_review_box.browser_window_index()?;
                    let exact_action = serde_json::to_value(to_browser_review_action(
                        spec.op,
                        args,
                        &browser_review_agent_id,
                    ))
                    .map_err(|error| {
                        ProviderSessionError::Tool(format!(
                            "Browser Auto-review action projection failed: {error}"
                        ))
                    })?;
                    let view_id = exact_action
                        .get("viewId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    let box_identity = BoxIdentity {
                        box_id: std::env::var("SAND_BOX_HOST")
                            .ok()
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| "127.0.0.1".into()),
                        window_generation: format!(
                            "{}:{}",
                            browser_review_agent_id,
                            initial_display
                        ),
                    };
                    let classifier_cancellation = browser_review_cancellation.clone();
                    let mut classifier =
                        create_sand_backend_smart_mode_classifier_executor_with_cancellation(
                            Arc::clone(&browser_review_auth),
                            Arc::new(move || classifier_cancellation.is_cancelled()),
                        )
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                    run_sand_browser_auto_review_preflight(
                        mode,
                        &exact_action,
                        &box_identity,
                        &browser_review_agent_id,
                        &browser_review_request_source,
                        Some(browser_review_controller.as_ref()),
                        || {
                            let display = browser_review_box
                                .browser_window_index()
                                .map_err(|error| error.to_string())?;
                            capture_browser_review_state(
                                browser_review_box.as_ref(),
                                display,
                                view_id.as_deref(),
                                tool_call_id,
                            )
                            .map_err(|error| error.to_string())
                        },
                        |risk_target, classifier_mode| {
                            run_sand_auto_review_classifier(
                                &mut classifier,
                                tool_call_id,
                                &browser_review_agent_id,
                                classifier_mode,
                                || risk_target.clone(),
                                || Ok(browser_review_context.clone()),
                                &[],
                                SAND_COMPUTER_AUTO_REVIEW_CLASSIFIER_ERROR_REASON,
                            )
                        },
                    )
                    .map_err(|error| ProviderSessionError::Tool(error.to_string()))
                });
            let state_approval_gate = Arc::clone(&auto_review_gate);
            let state_approval_barrier =
                Arc::new(move || {
                    state_approval_gate
                        .assert_no_pending_approval()
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))
                }) as mahayana_host_runtime::runner::tools::sand_state_tool::StateApprovalBarrier;
            let routine_review_auth = Arc::clone(&worker_auth);
            let routine_review_auto_review = Arc::clone(&worker_auto_review);
            let routine_review_controller = Arc::clone(&worker_auto_review_controller);
            let routine_review_cancellation = worker_cancellation.clone();
            let routine_review_agent_id = agent_id.clone();
            let routine_review_request_source = auto_review_request_source.clone();
            let routine_review_context = auto_review_context
                .iter()
                .map(|message| {
                    serde_json::json!({
                        "role": message.role,
                        "content": message.content,
                    })
                })
                .collect::<Vec<_>>();
            let routine_auto_review: RoutineAutoReviewCallback =
                Arc::new(move |target, tool_call_id| {
                    let mode = routine_review_auto_review.current_modes().automation_write;
                    let classifier_cancellation = routine_review_cancellation.clone();
                    let mut classifier =
                        create_sand_backend_smart_mode_classifier_executor_with_cancellation(
                            Arc::clone(&routine_review_auth),
                            Arc::new(move || classifier_cancellation.is_cancelled()),
                        )
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                    let outcome = review_sand_automation_write(
                        mode,
                        target,
                        &routine_review_agent_id,
                        Some(routine_review_controller.as_ref()),
                        &routine_review_request_source,
                        |risk_target, classifier_mode| {
                            run_sand_auto_review_classifier(
                                &mut classifier,
                                tool_call_id,
                                &routine_review_agent_id,
                                classifier_mode,
                                || risk_target.clone(),
                                || Ok(routine_review_context.clone()),
                                &[],
                                SAND_AUTOMATION_WRITE_CLASSIFIER_ERROR_REASON,
                            )
                        },
                    )
                    .map_err(|error| {
                        ProviderSessionError::Tool(match error {
                            mahayana_host_runtime::runner::sand_auto_review_classifier_run::AutoReviewClassifierError::Aborted(reason)
                            | mahayana_host_runtime::runner::sand_auto_review_classifier_run::AutoReviewClassifierError::Timeout(reason)
                            | mahayana_host_runtime::runner::sand_auto_review_classifier_run::AutoReviewClassifierError::Failed(reason) => reason,
                        })
                    })?;
                    match outcome {
                        AutomationReviewOutcome::Allowed => Ok(None),
                        AutomationReviewOutcome::Blocked(reason) => Ok(Some(reason)),
                    }
                });
            let cloud_agent_review = build_cloud_agent_auto_review_hook(
                Arc::clone(&worker_auth),
                Arc::clone(&worker_auto_review),
                Arc::clone(&worker_auto_review_controller),
                agent_id.clone(),
                auto_review_request_source.clone(),
                worker_cancellation.clone(),
                auto_review_context.clone(),
            );
            let cloud_watch_parent_agent_id = agent_id.clone();
            let cloud_watch_owner = Arc::clone(&worker_cloud_agent_watches);
            let cloud_agent_watch = Arc::new(
                move |bc_id: &str,
                      after_followup: bool,
                      quiet_origin: Option<serde_json::Value>| {
                    let _ = cloud_watch_owner.watch_cloud_agent(
                        &cloud_watch_parent_agent_id,
                        bc_id,
                        CloudAgentWatchOptions::new(quiet_origin, after_followup),
                    );
                },
            );
            let cloud_agent_tool = CloudAgentToolDependencies {
                manager: Arc::clone(&worker_cloud_agents),
                agent_dir: cloud_agent_dir,
                box_resources: Arc::clone(&box_resources),
                cancellation: worker_cancellation.clone(),
                review: Some(cloud_agent_review),
                quiet_origin: cloud_agent_quiet_origin,
                watch: Some(cloud_agent_watch),
            };
            let subagent_task_review: Option<SubagentTaskReviewCallback> =
                worker_subagent_management_runtime.as_ref().map(|_| {
                    let mode_source = Arc::clone(&worker_auto_review);
                    let launch_review_gate = Arc::clone(&auto_review_gate);
                    let review_auth = Arc::clone(&worker_auth);
                    let review_cancellation = worker_cancellation.clone();
                    let cancellation_for_classifier = worker_cancellation.clone();
                    let review_agent_id = agent_id.clone();
                    let review_context = auto_review_context
                        .iter()
                        .map(|message| {
                            serde_json::json!({
                                "role": message.role,
                                "content": message.content,
                            })
                        })
                        .collect::<Vec<_>>();
                    create_turn_subagent_task_review(TurnSubagentLaunchReviewBindings {
                        mode: Arc::new(move || mode_source.current_modes().subagent_launch),
                        assert_no_pending_approval: Arc::new(move || {
                            launch_review_gate
                                .assert_no_pending_approval()
                                .map_err(|error| ProviderSessionError::Tool(error.to_string()))
                        }),
                        controller: Arc::clone(&worker_auto_review_controller),
                        request_source: auto_review_request_source.clone(),
                        should_cancel: Arc::new(move || review_cancellation.is_cancelled()),
                        classify: Arc::new(move |risk_target, tool_call_id, classifier_mode| {
                            let classifier_cancellation = cancellation_for_classifier.clone();
                            let mut classifier =
                                create_sand_backend_smart_mode_classifier_executor_with_cancellation(
                                    Arc::clone(&review_auth),
                                    Arc::new(move || classifier_cancellation.is_cancelled()),
                                )
                                .map_err(|error| {
                                    mahayana_host_runtime::runner::sand_auto_review_classifier_run::AutoReviewClassifierError::Failed(
                                        error.to_string(),
                                    )
                                })?;
                            run_sand_auto_review_classifier(
                                &mut classifier,
                                tool_call_id,
                                &review_agent_id,
                                classifier_mode,
                                || risk_target.clone(),
                                || Ok(review_context.clone()),
                                &[],
                                SAND_SUBAGENT_CLASSIFIER_ERROR_REASON,
                            )
                        }),
                    })
                });
            let subagent_steer_review: Option<SubagentSteerReviewCallback> =
                worker_subagent_management_runtime.as_ref().map(|_| {
                    let review_auth = Arc::clone(&worker_auth);
                    let review_auto_review = Arc::clone(&worker_auto_review);
                    let review_controller = Arc::clone(&worker_auto_review_controller);
                    let review_cancellation = worker_cancellation.clone();
                    let review_agent_id = agent_id.clone();
                    let review_request_source = auto_review_request_source.clone();
                    let review_context = auto_review_context
                        .iter()
                        .map(|message| {
                            serde_json::json!({
                                "role": message.role,
                                "content": message.content,
                            })
                        })
                        .collect::<Vec<_>>();
                    Arc::new(move |subagent_id: &str, message: &str, tool_call_id: &str| {
                        let Some(target) = build_sand_subagent_steer_review_target(
                            subagent_id,
                            message,
                        ) else {
                            return Ok(SteerReview {
                                allowed: false,
                                reason: "MessageSubagent requires a subagent id and message.".into(),
                            });
                        };
                        let mode = review_auto_review.current_modes().subagent_launch;
                        let classifier_cancellation = review_cancellation.clone();
                        let mut classifier =
                            create_sand_backend_smart_mode_classifier_executor_with_cancellation(
                                Arc::clone(&review_auth),
                                Arc::new(move || classifier_cancellation.is_cancelled()),
                            )
                            .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                        let outcome = review_sand_subagent_action(
                            mode,
                            &target,
                            Some(review_controller.as_ref()),
                            &review_request_source,
                            || review_cancellation.is_cancelled(),
                            |risk_target, classifier_mode| {
                                run_sand_auto_review_classifier(
                                    &mut classifier,
                                    tool_call_id,
                                    &review_agent_id,
                                    classifier_mode,
                                    || risk_target.clone(),
                                    || Ok(review_context.clone()),
                                    &[],
                                    SAND_SUBAGENT_CLASSIFIER_ERROR_REASON,
                                )
                            },
                        )
                        .map_err(|error| ProviderSessionError::Tool(format!("{error:?}")))?;
                        Ok(match outcome {
                            SubagentReviewOutcome::Allowed => SteerReview {
                                allowed: true,
                                reason: String::new(),
                            },
                            SubagentReviewOutcome::Blocked(reason) => SteerReview {
                                allowed: false,
                                reason,
                            },
                            SubagentReviewOutcome::Cancelled => SteerReview {
                                allowed: false,
                                reason: "The subagent steering action was cancelled.".into(),
                            },
                        })
                    }) as SubagentSteerReviewCallback
                });
            let send_message_delivery_counter = SendMessageDeliveryCounter::default();
            let group_preview = worker_group_room_id.as_deref().map(|room_id| {
                Arc::new(Mutex::new(GroupMemberPreview::new(
                    room_id,
                    &agent_id,
                    worker_group_member_name.as_deref().unwrap_or(&agent_id),
                    &stream_id,
                )))
            });
            let delta_group_preview = group_preview.clone();
            let delta_events = worker_events.clone();
            let delta_stream_id = stream_id.clone();
            let delta_runtime = Arc::clone(&worker_transcript_runtime);
            let delta_agent_id = agent_id.clone();
            let delta_observation = Arc::clone(&observation);
            let mut on_text_delta = move |delta: &str, accumulated: &str| {
                if !delta.is_empty() {
                    if let Ok(mut observation) = delta_observation.lock() {
                        let _ = observation.observe_stream_output("text");
                    }
                }
                delta_runtime.track_runner_activity_update(
                    &delta_agent_id,
                    &ActivityUpdate::TextDelta {
                        text: delta.to_string(),
                    },
                    started_at_ms(),
                );
                if let Some(preview) = delta_group_preview.as_ref()
                    && let Ok(mut preview) = preview.lock()
                    && let Some(payload) = preview.on_text_delta(accumulated)
                {
                    delta_events.publish(serde_json::json!({
                        "channel": "transcript",
                        "payload": payload,
                    }));
                }
                delta_events.publish(serde_json::json!({
                    "channel": RUNNER_INFERENCE_EVENT_CHANNEL,
                    "payload": {
                        "streamId": delta_stream_id,
                        "agentId": delta_agent_id.clone(),
                        "type": "delta",
                        "content": accumulated
                    }
                }));
            };
            let agent_wake_events = worker_events.clone();
            let priority_registry = Arc::clone(&worker_registry);
            let priority_runtime = Arc::clone(&worker_transcript_runtime);
            let priority_telemetry = telemetry_logs.clone();
            let agent_message_analytics = product_analytics.clone();
            let agent_messaging = Arc::new(ProductionAgentToAgentMessaging::new(
                Arc::clone(&worker_sessions),
                Arc::new(move |request: &AgentWakeRequest| {
                    agent_wake_events.publish(serde_json::json!({
                        "channel": "agent-inbound-wake-request",
                        "payload": request,
                    }));
                }),
                Some(Arc::new(move |target_agent_id: &str, reason: &str| {
                    if !should_interrupt_priority_peer(
                        priority_runtime.active_turn_lane(target_agent_id),
                    ) {
                        return 0;
                    }
                    let was_in_flight = priority_runtime.is_agent_running(target_agent_id);
                    let direct = priority_registry.cancel_agent(target_agent_id, reason);
                    let group_member =
                        priority_registry.preempt_group_member_agent(target_agent_id, reason);
                    let cancelled = direct.saturating_add(group_member);
                    if cancelled > 0 || was_in_flight {
                        let _ = priority_telemetry.report_turn_interrupt(&TurnInterruptFields {
                            conversation_id: target_agent_id.to_string(),
                            reason: "agent_steer".into(),
                            had_active_run: cancelled > 0,
                            was_in_flight,
                        });
                    }
                    cancelled
                })),
            ).with_analytics(Arc::new(move |from_agent_id, to_agent_id, is_group_target, is_priority| {
                let _ = agent_message_analytics.track_event(
                    "sand.agent_message.sent",
                    &serde_json::json!({
                        "from_agent_id": from_agent_id,
                        "to_agent_id": to_agent_id,
                        "is_group_target": is_group_target,
                        "is_priority": is_priority,
                    }),
                );
            })).with_group_post({
                let group_deps = worker_group_post_deps.clone();
                let group_session_key = worker_session_admitted_key.clone();
                Arc::new(move |from_agent_id, group_id, message, is_priority| {
                    post_agent_message_to_group_with_session(
                        group_deps.clone(),
                        group_session_key.as_deref(),
                        from_agent_id,
                        group_id,
                        message,
                        is_priority,
                    )
                })
            }));
            let turn_agent_messages = Arc::new(Mutex::new(Vec::<String>::new()));
            let agent_management_sink: Arc<dyn AgentManagementSink> = Arc::new(
                ProductionAgentManagementSink {
                    sessions: Arc::clone(&worker_sessions),
                    forever_box: Arc::clone(&forever_box),
                    messaging: agent_messaging,
                    agent_messages: Arc::clone(&turn_agent_messages),
                    agent_id: agent_id.clone(),
                    session_key: worker_session_admitted_key.clone(),
                    reply_to_id: (request_source.as_deref() == Some("agent-inbound"))
                        .then(|| worker_reply_thread_target.clone())
                        .flatten(),
                },
            );
            let base_send_message_sink: Arc<dyn SendMessageSink> = Arc::new(
                ProductionSendMessageSink {
                    host_tx: host_tx.clone(),
                    cloud_agents: Arc::clone(&worker_cloud_agents),
                    sessions: Arc::clone(&worker_sessions),
                    forever_box: Arc::clone(&forever_box),
                    transcript_manager: Arc::clone(&worker_transcript_manager),
                    events: worker_events.clone(),
                    ack_obligations: Arc::clone(&worker_ack_obligations),
                    transcript_runtime: Arc::clone(&worker_transcript_runtime),
                    ack_token: worker_ack_token.clone(),
                    agent_id: agent_id.clone(),
                    session_key: worker_session_admitted_key.clone(),
                    reply_thread_target: worker_reply_thread_target.clone(),
                    is_fork: worker_send_is_fork,
                    pending_media: Mutex::new(HashMap::new()),
                },
            );
            let send_message_sink: Arc<dyn SendMessageSink> = Arc::new(
                CountingSendMessageSink::new(
                    base_send_message_sink,
                    send_message_delivery_counter.clone(),
                ),
            );
            let mcp_awaiting_sink = Arc::clone(&send_message_sink);
            let mcp_connector_sink = Arc::clone(&send_message_sink);
            let mcp_management_sink = Arc::new(
                McpHostServiceManagementSink::new(
                    Arc::clone(&worker_mcp_service),
                    Some(agent_id.clone()),
                    true,
                )
                .with_interaction_callbacks(
                    Arc::new(move || mcp_awaiting_sink.is_awaiting_user_selection()),
                    Arc::new(move |card| {
                        let variant = match card.variant {
                            ConnectorCardVariant::Connect => "connect",
                            ConnectorCardVariant::Connected => "connected",
                        };
                        let server_id = card.server_id.clone();
                        mcp_connector_sink
                            .send_message(
                                serde_json::json!({
                                    "type": "connector",
                                    "connector": card.connector,
                                    "serverId": card.server_id,
                                    "variant": variant,
                                }),
                                started_at_ms(),
                                &format!("mcp-management:{server_id}:{variant}"),
                            )
                            .map(|_| ())
                    }),
                ),
            );
            let routine_post_write: Option<RoutinePostWriteCallback> =
                automations_lifecycle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .upgrade()
                    .map(|lifecycle| {
                    let sink = Arc::clone(&send_message_sink);
                    let callback_agent_id = agent_id.clone();
                    let listener_connect_manager = Arc::clone(&worker_transcript_manager);
                    Arc::new(move |target: &mahayana_host_runtime::runner::sand_automation_auto_review::AutomationWriteTarget, tool_call_id: &str| {
                        if !matches!(target.operation.as_str(), "create" | "update")
                            || !target.spec.is_enabled
                        {
                            return Ok(());
                        }
                        let mut platforms = trigger_members(&target.spec.trigger)
                            .into_iter()
                            .filter_map(|listener| {
                                listener
                                    .get("type")
                                    .and_then(serde_json::Value::as_str)
                                    .filter(|platform| matches!(*platform, "slack" | "github"))
                                    .map(ToOwned::to_owned)
                            })
                            .collect::<Vec<_>>();
                        platforms.sort();
                        platforms.dedup();
                        let surfaced = surface_listener_connect_cards(
                            &platforms,
                            Some(|platform: &str| lifecycle.is_platform_connected(platform)),
                            |platform| match platform {
                                "slack" => Some("Slack".to_string()),
                                "github" => Some("GitHub".to_string()),
                                _ => None,
                            },
                        );
                        for card in surfaced.cards {
                            let platform = card.platform.clone();
                            let card_tool_call_id =
                                format!("{tool_call_id}:listener:{platform}");
                            sink.send_message(
                                serde_json::json!({
                                    "type": card.message_type,
                                    "platform": platform,
                                    "reason": card.reason,
                                }),
                                started_at_ms(),
                                &card_tool_call_id,
                            )?;
                            listener_connect_manager
                                .emit_listener_connect_card(&callback_agent_id, &platform);
                            lifecycle.watch_listener_connection(
                                callback_agent_id.clone(),
                                platform,
                            );
                        }
                        Ok(())
                    }) as RoutinePostWriteCallback
                });
            let reaction_delivery_counter = ReactionDeliveryCounter::default();
            let base_reaction_sink: Arc<dyn ReactionSink> = Arc::new(
                ProductionReactionSink {
                    host_tx,
                    agent_id: group_member_reaction_target(
                        worker_group_room_id.as_deref(),
                        &agent_id,
                    ),
                },
            );
            let reaction_sink: Arc<dyn ReactionSink> = Arc::new(CountingReactionSink::new(
                base_reaction_sink,
                reaction_delivery_counter.clone(),
            ));
            let retry_runtime = Arc::clone(&worker_transcript_runtime);
            let retry_agent_id = agent_id.clone();
            let retry_sink: Arc<dyn Fn(&ProviderRetryEvent) + Send + Sync> =
                Arc::new(move |_event: &ProviderRetryEvent| {
                    retry_runtime.track_runner_activity_update(
                        &retry_agent_id,
                        &ActivityUpdate::Retrying,
                        started_at_ms(),
                    );
                });
            let retry_observation = Arc::clone(&observation);
            let retry_structured_turn = Arc::clone(&structured_turn);
            let retry_telemetry_logs = worker_telemetry_logs.clone();
            let retry_conversation_id = agent_id.clone();
            let retry_report_sink: Arc<dyn Fn(&ProviderRetryReport) + Send + Sync> =
                Arc::new(move |report: &ProviderRetryReport| {
                    let outcome = match report.outcome {
                        ProviderRetryOutcome::Retried => "retried",
                        ProviderRetryOutcome::Exhausted => "exhausted",
                        ProviderRetryOutcome::GaveUpIneligible => "gave_up_ineligible",
                    };
                    if report.outcome == ProviderRetryOutcome::Retried
                        && let Ok(mut turn) = retry_structured_turn.lock()
                    {
                        turn.note_retry(report.delay_ms, Some(report.cause.as_str()));
                    }
                    if let Ok(observation) = retry_observation.lock() {
                        observation.report_turn_retry(serde_json::json!({
                            "outcome": outcome,
                            "attempt": report.attempt,
                            "maxAttempts": report.max_attempts,
                            "delayMs": report.delay_ms,
                            "serverPaced": report.server_paced,
                            "resumeFromCheckpoint": report.resume_from_checkpoint,
                            "watchdogExpired": report.watchdog_expired,
                            "error": report.error,
                        }));
                    }
                    let fields = TurnRetryFields {
                        conversation_id: retry_conversation_id.clone(),
                        outcome: outcome.into(),
                        attempt: u64::from(report.attempt),
                        max_attempts: u64::from(report.max_attempts),
                        error_type: report.error_type.clone(),
                        error_code: report.error_code.clone(),
                        cause: report.cause.clone(),
                        delay_ms: report.delay_ms.map(|value| value as f64),
                        server_paced: Some(report.server_paced),
                    };
                    if let Err(error) = retry_telemetry_logs.report_turn_retry(&fields) {
                        eprintln!(
                            "mahayana-host turn_retry_telemetry_failed agent={} error={error}",
                            retry_conversation_id
                        );
                    }
                });
            let audit_events = worker_events.clone();
            let host_action_auditor = Arc::clone(production_action_auditor.service());
            let base_action_audit_sink: Arc<dyn ActionAuditSink> = Arc::new(
                move |record: ActionAuditRecord| {
                    audit_events.publish(serde_json::json!({
                        "channel": "runner-action-audit",
                        "payload": record,
                    }));
                    match serde_json::from_value::<AuditAction>(record.action.clone()) {
                        Ok(action) => host_action_auditor.record(AuditRecord {
                            occurred_at_ms: record.occurred_at_ms,
                            agent_id: record.agent_id,
                            turn_id: record.turn_id,
                            box_id: None,
                            action,
                        }),
                        Err(error) => eprintln!(
                            "mahayana-host-action-audit invalid runner action: {error}"
                        ),
                    }
                },
            );
            let bot_block_logs = telemetry_logs.clone();
            let action_audit_sink: Arc<dyn ActionAuditSink> = with_bot_block_detection_sink(
                base_action_audit_sink,
                Arc::new(move |hit, record| {
                    let report = BotBlockReport {
                        conversation_id: record.agent_id.clone(),
                        family: hit.family.to_string(),
                        confidence: hit.confidence.as_str().to_string(),
                        blocked_host: hit.blocked_host.clone(),
                        blocked_url: hit.blocked_url.clone(),
                    };
                    if let Err(error) = bot_block_logs.report_bot_block(&report) {
                        eprintln!(
                            "mahayana-host bot_block_telemetry_failed agent={} error={error}",
                            record.agent_id
                        );
                    }
                }),
            );
            let audit_mcp_service = Arc::clone(&worker_mcp_service);
            let action_audit_sink = with_transport_resolving_sink(
                action_audit_sink,
                Arc::new(move |server_identifier| {
                    audit_mcp_service
                        .transport_for_server_identifier(server_identifier)
                        .unwrap_or_else(|_| "unknown".to_string())
                }),
            );
            let remote_resource_gate = Arc::clone(&auto_review_gate);
            let remote_shell_audit = Arc::clone(&action_audit_sink);
            let remote_shell_agent_id = agent_id.clone();
            let remote_shell_turn_id = stream_id.clone();
            let remote_navigation_baseline_owner =
                Arc::clone(&worker_host_runner_composition);
            let remote_navigation_probe_owner =
                Arc::clone(&worker_host_runner_composition);
            let remote_navigation_audit = Arc::clone(&action_audit_sink);
            let remote_navigation_agent_id = agent_id.clone();
            let remote_navigation_turn_id = stream_id.clone();
            let remote_computer_audit_owner =
                Arc::clone(&worker_host_runner_composition);
            shipping_box_resources.bind_remote_resource_lifecycle(
                ForeverBoxRemoteResourceLifecycle {
                    turn_id: Some(stream_id.clone()),
                    box_id: agent_id.clone(),
                    assert_no_pending_approval: Arc::new(move || {
                        remote_resource_gate
                            .assert_no_pending_approval()
                            .map_err(|error| ProviderSessionError::Tool(error.to_string()))
                    }),
                    audit_shell: Arc::new(move |kind, command, turn_id, _box_id| {
                        let shell_kind = match kind {
                            mahayana_host_runtime::runner::remote_box_resources::RemoteShellKind::Foreground => "foreground",
                            mahayana_host_runtime::runner::remote_box_resources::RemoteShellKind::Background => "background",
                        };
                        remote_shell_audit.record(ActionAuditRecord {
                            occurred_at_ms: started_at_ms(),
                            agent_id: remote_shell_agent_id.clone(),
                            turn_id: turn_id
                                .map(str::to_string)
                                .or_else(|| Some(remote_shell_turn_id.clone())),
                            action: serde_json::json!({
                                "kind": "shellCommand",
                                "command": command,
                                "shellKind": shell_kind,
                                "target": "isolated_box",
                            }),
                        });
                    }),
                    capture_navigation_baseline: Arc::new(move |stdout| {
                        remote_navigation_baseline_owner
                            .capture_computer_navigation_baseline(stdout);
                    }),
                    probe_navigation: Arc::new(move |stdout| {
                        let Some(generation) =
                            remote_navigation_probe_owner.request_computer_navigation_probe()
                        else {
                            return;
                        };
                        for record in remote_navigation_probe_owner
                            .complete_computer_navigation_probe(
                                generation,
                                stdout,
                                &remote_navigation_agent_id,
                                Some(&remote_navigation_turn_id),
                                started_at_ms(),
                            )
                        {
                            remote_navigation_audit.record(record);
                        }
                    }),
                    record_computer_audit_intent: Arc::new(move |action_case| {
                        let Some(action_case) = action_case else {
                            return;
                        };
                        remote_computer_audit_owner
                            .record_computer_audit_intent(action_case);
                    }),
                },
            );

            let browser_media_agent_id = agent_id.clone();
            let browser_persist_image: BrowserPersistImageCallback = Arc::new(
                move |bytes, mime| {
                    if mime != "image/png" {
                        return;
                    }
                    if persist_runner_media_bytes(
                        &browser_media_sessions,
                        &browser_media_agent_id,
                        "browser-screenshot.png",
                        bytes,
                        AgentMediaKind::Image,
                    )
                    .is_none()
                    {
                        eprintln!(
                            "mahayana-host browser_screenshot_persist_failed agent={} error=admission-or-persist-failed",
                            browser_media_agent_id
                        );
                    }
                },
            );
            let browser_navigation_urls =
                Arc::new(Mutex::new(BTreeMap::<String, String>::new()));
            let browser_navigation_urls_sink = Arc::clone(&browser_navigation_urls);
            let browser_navigation_audit = Arc::clone(&action_audit_sink);
            let browser_navigation_agent_id = agent_id.clone();
            let browser_navigation_turn_id = stream_id.clone();
            let browser_possible_navigation: BrowserPossibleNavigationCallback =
                Arc::new(move |response| {
                    let Some(url) = response
                        .url
                        .as_deref()
                        .and_then(normalize_navigation_url)
                    else {
                        return;
                    };
                    let view_id = response
                        .view_id
                        .as_deref()
                        .unwrap_or("default")
                        .to_string();
                    let should_record =
                        if let Ok(mut urls) = browser_navigation_urls_sink.lock() {
                            if urls.get(&view_id).is_some_and(|prior| prior == &url) {
                                false
                            } else {
                                urls.insert(view_id, url.clone());
                                true
                            }
                        } else {
                            false
                        };
                    if !should_record {
                        return;
                    }
                    browser_navigation_audit.record(ActionAuditRecord {
                        occurred_at_ms: started_at_ms(),
                        agent_id: browser_navigation_agent_id.clone(),
                        turn_id: Some(browser_navigation_turn_id.clone()),
                        action: serde_json::json!({
                            "kind": "browserNavigation",
                            "url": url,
                            "pageTitle": response.title.as_deref().unwrap_or_default(),
                        }),
                    });
                });
            let browser_shell_gate = Arc::clone(&auto_review_gate);
            let browser_shell_audit = Arc::clone(&action_audit_sink);
            let browser_shell_agent_id = agent_id.clone();
            let browser_shell_turn_id = stream_id.clone();
            let browser_shell_safety: BrowserShellSafetyCallback = Arc::new(
                move |request: &RunnerBoxShellRequest| {
                    browser_shell_gate
                        .assert_no_pending_approval()
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                    browser_shell_audit.record(ActionAuditRecord {
                        occurred_at_ms: started_at_ms(),
                        agent_id: browser_shell_agent_id.clone(),
                        turn_id: Some(browser_shell_turn_id.clone()),
                        action: serde_json::json!({
                            "kind": "shellCommand",
                            "command": request.command.clone(),
                            "shellKind": if request.is_background { "background" } else { "foreground" },
                            "target": "isolated_box",
                        }),
                    });
                    Ok(())
                },
            );
            let mut browser_executor_owner = ProductionBrowserToolExecutor::new(
                Arc::clone(&box_resources),
                agent_id.clone(),
            )
            .with_auto_review_callback(browser_auto_review)
            .with_possible_navigation_callback(browser_possible_navigation)
            .with_shell_safety_callback(browser_shell_safety);
            if !is_group_member_turn {
                browser_executor_owner =
                    browser_executor_owner.with_persist_image_callback(browser_persist_image);
            }
            let browser_executor: Arc<dyn BrowserToolExecutor> =
                Arc::new(browser_executor_owner);
            let computer_media_agent_id = agent_id.clone();
            let computer_persist_image: ComputerPersistImageCallback = Arc::new(
                move |bytes, mime| {
                    if mime != "image/webp" {
                        return None;
                    }
                    persist_runner_media_bytes(
                        &computer_media_sessions,
                        &computer_media_agent_id,
                        "computer-screenshot.webp",
                        bytes,
                        AgentMediaKind::Image,
                    )
                },
            );
            let computer_navigation_baseline_box = Arc::clone(&box_resources);
            if let Ok(display_number) = computer_navigation_baseline_box.browser_window_index() {
                if let Ok(display_number) = u16::try_from(display_number) {
                    if let Ok(result) = computer_navigation_baseline_box.execute_shell(RunnerBoxShellRequest {
                        command: navigation_probe_command(display_number),
                        working_directory: "/workspace".into(),
                        tool_call_id: "sand-navigation-probe-baseline".into(),
                        is_background: false,
                        block_until_ms: None,
                    }) {
                        if result.get("exitCode").and_then(serde_json::Value::as_i64).unwrap_or(-1) == 0 {
                            if let Some(stdout) = result.get("stdout").and_then(serde_json::Value::as_str) {
                                worker_host_runner_composition.capture_computer_navigation_baseline(stdout);
                            }
                        }
                    }
                }
            }
            let computer_control_handoff = session_handoff.clone();
            let computer_control_agent_id = agent_id.clone();
            let computer_control_lease_for_check = computer_control_lease.clone();
            let computer_control_owner_for_check =
                Arc::clone(&worker_host_runner_composition);
            let computer_control_box = Arc::clone(&forever_box);
            let computer_action_audit = Arc::clone(&action_audit_sink);
            let computer_action_agent_id = agent_id.clone();
            let computer_action_turn_id = stream_id.clone();
            let computer_navigation_box = Arc::clone(&box_resources);
            let computer_navigation_owner = Arc::clone(&worker_host_runner_composition);
            let computer_navigation_audit = Arc::clone(&action_audit_sink);
            let computer_navigation_agent_id = agent_id.clone();
            let computer_navigation_turn_id = stream_id.clone();
            let mut computer_executor_owner =
                ProductionComputerToolExecutor::new(Arc::clone(&box_resources))
                    .with_auto_review_callback(computer_auto_review)
                    .with_action_report_callback(Arc::new(move |reported, _tool_call_id| {
                        let action = match reported {
                            ReportedComputerAction::Drag { x, y } => serde_json::json!({
                                "kind": "computerAction",
                                "type": "drag",
                                "x": x,
                                "y": y,
                            }),
                            ReportedComputerAction::Move { x, y } => serde_json::json!({
                                "kind": "computerAction",
                                "type": "move",
                                "x": x,
                                "y": y,
                            }),
                            ReportedComputerAction::Scroll { x, y } => serde_json::json!({
                                "kind": "computerAction",
                                "type": "scroll",
                                "x": x,
                                "y": y,
                            }),
                            ReportedComputerAction::Click { x, y, button, count } => serde_json::json!({
                                "kind": "computerAction",
                                "type": "click",
                                "x": x,
                                "y": y,
                                "button": button,
                                "count": count,
                            }),
                        };
                        computer_action_audit.record(ActionAuditRecord {
                            occurred_at_ms: started_at_ms(),
                            agent_id: computer_action_agent_id.clone(),
                            turn_id: Some(computer_action_turn_id.clone()),
                            action,
                        });
                    }))
                    .with_post_action_callback(Arc::new(move |tool_call_id| {
                        let Some(generation) = computer_navigation_owner.request_computer_navigation_probe() else {
                            return;
                        };
                        let navigation_box = Arc::clone(&computer_navigation_box);
                        let navigation_owner = Arc::clone(&computer_navigation_owner);
                        let navigation_audit = Arc::clone(&computer_navigation_audit);
                        let navigation_agent_id = computer_navigation_agent_id.clone();
                        let navigation_turn_id = computer_navigation_turn_id.clone();
                        let tool_call_id = tool_call_id.to_string();
                        let _ = thread::Builder::new()
                            .name(format!("mahayana-navigation-probe-{generation}"))
                            .spawn(move || {
                                loop {
                                    let now = started_at_ms();
                                    let Some(wait_ms) = navigation_owner.computer_navigation_probe_wait_ms(generation, now) else {
                                        return;
                                    };
                                    if wait_ms == 0 {
                                        break;
                                    }
                                    thread::sleep(Duration::from_millis(wait_ms));
                                }
                                let Ok(display_number) = navigation_box.browser_window_index() else {
                                    return;
                                };
                                let Ok(display_number) = u16::try_from(display_number) else {
                                    return;
                                };
                                let Ok(result) = navigation_box.execute_shell(RunnerBoxShellRequest {
                                    command: navigation_probe_command(display_number),
                                    working_directory: "/workspace".into(),
                                    tool_call_id: format!("{tool_call_id}:navigation-probe"),
                                    is_background: false,
                                    block_until_ms: None,
                                }) else {
                                    return;
                                };
                                if result.get("exitCode").and_then(serde_json::Value::as_i64).unwrap_or(-1) != 0 {
                                    return;
                                }
                                let Some(stdout) = result.get("stdout").and_then(serde_json::Value::as_str) else {
                                    return;
                                };
                                let occurred_at_ms = started_at_ms();
                                for record in navigation_owner.complete_computer_navigation_probe(
                                    generation,
                                    stdout,
                                    &navigation_agent_id,
                                    Some(&navigation_turn_id),
                                    occurred_at_ms,
                                ) {
                                    navigation_audit.record(record);
                                }
                            });
                    }))
                    .with_availability_check(Arc::new(move |args| {
                        if !computer_control_box.box_().is_available() {
                            return Err(ProviderSessionError::Tool("Computer is unavailable because the shipping box runtime is not available.".into()));
                        }
                        if computer_control_box.box_().inner().shared_desktop().is_none() {
                            return Err(ProviderSessionError::Tool("Computer is unavailable because the shipping box has no desktop monitor capability.".into()));
                        }
                        if args.action != mahayana_host_runtime::runner::tools::sand_computer_tool::ComputerActionName::Screenshot {
                            let lease = computer_control_lease_for_check.as_ref().ok_or_else(|| ProviderSessionError::Tool(
                                "Computer input is unavailable because this Runner does not hold a ComputerControlLease.".into()
                            ))?;
                            if !computer_control_owner_for_check.owns_computer_control_lease(lease) {
                                return Err(ProviderSessionError::Tool(
                                    "Computer input is unavailable because this Runner's ComputerControlLease is no longer active.".into(),
                                ));
                            }
                            if computer_control_box.box_().get_agent_window_index(&computer_control_agent_id)
                                .is_some_and(|window| window != lease.window_index())
                            {
                                return Err(ProviderSessionError::Tool(
                                    "Computer input is unavailable because the live desktop assignment no longer matches this Runner's ComputerControlLease.".into(),
                                ));
                            }
                            if computer_control_handoff.get(&computer_control_agent_id).is_some() {
                                return Err(ProviderSessionError::Tool(
                                    "Computer control is currently handed to the user. Wait for the user to hand the box back before sending desktop input.".into(),
                                ));
                            }
                        }
                        Ok(())
                    }));
            if !is_group_member_turn {
                computer_executor_owner =
                    computer_executor_owner.with_persist_image_callback(computer_persist_image);
            }
            let computer_executor: Arc<dyn ComputerToolExecutor> =
                Arc::new(computer_executor_owner);
            let file_transfer_executor: Arc<dyn FileTransferExecutor> = Arc::new(
                ProductionFileTransferExecutor::for_turn(
                    Arc::clone(&forever_box),
                    Arc::clone(&local_exec),
                    Arc::clone(&local_tool_permission),
                    agent_id.clone(),
                    turn_local_tool_direction_epoch,
                ),
            );
            let external_machine_executor: Arc<dyn ExternalMachineExecutor> = Arc::new(
                ProductionExternalMachineExecutor::for_turn(
                    Arc::clone(&local_exec),
                    Arc::clone(&local_tool_permission),
                    agent_id.clone(),
                    turn_local_tool_direction_epoch,
                ),
            );
            let worker_provider_usage = Arc::new(Mutex::new(None::<ProviderTokenUsage>));
            let usage_store = Arc::clone(&worker_provider_usage);
            let usage_inference = Arc::clone(&inference);
            let usage_provider = provider;
            let usage_sink: Arc<dyn Fn(ProviderTokenUsage) + Send + Sync> =
                Arc::new(move |usage| {
                    usage_inference.record_usage(
                        usage_provider,
                        InferenceUsage {
                            input_tokens: Some(usage.input_tokens),
                            output_tokens: Some(usage.output_tokens),
                            cache_read_tokens: Some(usage.cache_read_tokens),
                            cache_write_tokens: Some(usage.cache_write_tokens),
                        },
                    );
                    if let Ok(mut stored) = usage_store.lock() {
                        *stored = Some(merge_provider_token_usage(stored.take(), usage));
                    }
                });
            let cursor_auth = inference.cursor_auth_for_session(Some(&worker_session_options));
            let settle_labeling_auth = Arc::clone(&cursor_auth);
            let turn_model_id = cursor_auth.requested_model().model_id;
            let web_executor: Arc<dyn WebToolExecutor> = Arc::new(
                ProductionWebToolExecutor::new(Arc::clone(&inference), turn_model_id.clone()),
            );
            let generated_image_sessions = Arc::clone(&worker_sessions);
            let generated_image_agent_id = agent_id.clone();
            let persist_generated_image: PersistGeneratedImage = Arc::new(move |bytes, mime_type| {
                generated_image_sessions.with_agent_admitted_filesystem_mutation(
                    &generated_image_agent_id,
                    || {
                        let db_path = generated_image_sessions
                            .session_db_path(&generated_image_agent_id)?;
                        let agent_dir = db_path
                            .parent()
                            .ok_or_else(|| "agent database has no parent directory".to_string())?;
                        let generated_image_dir = agent_dir.join("generated-images");
                        let persisted = persist_image_bytes(&generated_image_dir, bytes, mime_type)
                            .map_err(|error| error.to_string())?;
                        Ok(Some(PersistedImage {
                            absolute_path: persisted.absolute_path.to_string_lossy().into_owned(),
                        }))
                    },
                )
            });
            let generate_image_auth: Arc<dyn GenerateImageAuth> = auth.clone();
            let generate_image_executor: Arc<dyn GenerateImageToolExecutor> = Arc::new(
                ProductionGenerateImageToolExecutor::new(
                    generate_image_auth,
                    persist_generated_image,
                ),
            );
            let turn_conversation_id = cursor_auth
                .conversation_id()
                .unwrap_or_else(|| agent_id.clone());
            let privacy_auth = Arc::clone(&cursor_auth);
            let privacy_mode_resolver: ProductionTurnPrivacyModeResolver =
                Arc::new(move || {
                    let backend_url = privacy_auth.backend_url().ok()?;
                    let access_token = privacy_auth.access_token().ok()?;
                    let machine_id = privacy_auth.machine_id().ok()?;
                    mahayana_host_runtime::cursor_backend::resolve_sand_privacy_mode(
                        &backend_url,
                        &access_token,
                        &machine_id,
                    )
                });
            let transcripts_folder_available =
                !resolved_request_context.context.transcripts_folder.trim().is_empty();
            let computer_exposure = if worker_generated_parent_agent_id.is_some() {
                if worker_generated_subagent_type.eq_ignore_ascii_case("computeruse")
                    && computer_use_window_granted
                    && computer_control_lease_active
                {
                    ComputerToolExposure::Full
                } else {
                    ComputerToolExposure::Disabled
                }
            } else {
                ComputerToolExposure::ScreenshotOnly
            };
            let remote_box_available = forever_box.box_().is_available();
            let remote_box_has_desktop = forever_box.box_().inner().shared_desktop().is_some();
            let subagent_task_allowed_types = build_turn_subagent_types(
                worker_generated_parent_agent_id.is_some(),
                multitask_enabled,
                remote_box_available,
                remote_box_has_desktop,
                browser_use_subagent_enabled,
            )
            .map(Arc::new);
            let cloud_agent_tool_for_turn = if worker_cloud_agents.is_disabled_by_team_admin() {
                None
            } else {
                Some(cloud_agent_tool)
            };
            let composition = worker_host_runner_composition.compose_production_turn(
                ProductionRunnerCompositionInput {
                    provider,
                    bridge,
                    cursor_auth: Some(cursor_auth),
                    request_context: resolved_request_context,
                    cancellation,
                    checkpoint_store,
                    retry_sink: Some(retry_sink),
                    retry_report_sink: Some(retry_report_sink),
                    usage_sink: Some(usage_sink),
                    spotlight_enabled,
                    toolset_role: TurnToolsetRole {
                        is_subagent_runner: worker_generated_parent_agent_id.is_some(),
                        is_shared_room_runner: worker_group_room_id.is_some(),
                        is_box_scoped_subagent: worker_generated_parent_agent_id.is_some(),
                        is_computer_use_subagent: worker_generated_parent_agent_id.is_some()
                            && worker_generated_subagent_type.eq_ignore_ascii_case("computeruse"),
                        is_browser_use_subagent: worker_generated_parent_agent_id.is_some()
                            && worker_generated_subagent_type.eq_ignore_ascii_case("browseruse"),
                        subagent_configs_present: worker_generated_parent_agent_id.is_none(),
                        shared_room_box_tools_enabled,
                        remote_box_available,
                        remote_box_has_desktop,
                        dynamic_tools_enabled,
                    },
                    local_tool_permission: worker_session_admitted_key.clone().map(|session_key| {
                        TurnLocalToolPermissionBinding {
                            controller: Arc::clone(&turn_local_tool_permission),
                            agent_id: agent_id.clone(),
                            session_key,
                            direction_epoch: turn_local_tool_direction_epoch,
                        }
                    }),
                    box_resources: Some(box_resources),
                    browser_executor: Some(browser_executor),
                    web_executor: Some(web_executor),
                    generate_image_executor: Some(generate_image_executor),
                    computer_executor: Some(computer_executor),
                    computer_exposure,
                    file_transfer_executor: Some(file_transfer_executor),
                    external_machine_executor: Some(external_machine_executor),
                    external_shell_review: Some(external_shell_review),
                    mcp_management_sink: Some(mcp_management_sink),
                    send_message_sink: Some(send_message_sink),
                    reaction_sink: Some(reaction_sink),
                    cloud_agent_tool: cloud_agent_tool_for_turn,
                    multitask_enabled,
                    action_audit: Some(ProductionActionAuditInput {
                        agent_id: agent_id.clone(),
                        turn_id: Some(stream_id.clone()),
                        sink: action_audit_sink,
                    }),
                    observation: Some(Arc::clone(&observation)),
                },
                ProductionTurnCompositionHooks {
                    agent_management_sink,
                    state_writer,
                    routine_auto_review,
                    state_approval_barrier,
                    box_shell_review,
                    subagent_task_sink: worker_subagent_task_sink,
                    subagent_task_allowed_types,
                    subagent_task_review,
                    subagent_management_runtime: worker_subagent_management_runtime,
                    subagent_steer_review,
                    routine_post_write,
                    multitask_todo_state,
                },
            )
            .with_silence_allowed(prompt_is_silence_allowed);
            let summarization_inference = Arc::clone(&inference);
            let summarization_data_dir = data_dir.clone();
            let summarization_prompt: ProductionTurnSummarizationPrompt =
                Arc::new(move |system_prompt, user_prompt, should_cancel| {
                    summarization_inference.run_summarization_prompt(
                        &summarization_data_dir,
                        system_prompt,
                        user_prompt,
                        should_cancel,
                    )
                });
            let turn_static_config = ProductionTurnAgentStaticConfig {
                model_id: turn_model_id.clone(),
                agent_token_limit: SAND_AGENT_TOKEN_LIMIT,
                conversation_id: turn_conversation_id,
                is_box_scoped_subagent: worker_generated_parent_agent_id.is_some(),
                is_subagent_runner: worker_generated_parent_agent_id.is_some(),
                is_shared_room_runner: worker_group_room_id.is_some(),
                sand_send_message_delivery_owed: is_delivery_owed(
                    send_message_delivery_counter.count(),
                    reaction_delivery_counter.count() > 0,
                ),
                transcripts_folder_available,
            };
            let build_bindings = ProductionTurnAgentBuildBindings::new(
                turn_static_config,
                privacy_mode_resolver,
                Arc::clone(&summarization_prompt),
            );
            let profile_announcement_commit = worker_profile_announcement
                .clone()
                .map(|(turn_snapshot, identity)| {
                    let profile_sessions = Arc::clone(&worker_retire_sessions);
                    let profile_agent_id = agent_id.clone();
                    Arc::new(move || {
                        if let Ok(current_value) =
                            profile_sessions.get_agent_profile_prompt_snapshot(&profile_agent_id)
                        {
                            let current = current_value.and_then(|value| {
                                serde_json::from_value::<AgentProfilePromptSnapshot>(value).ok()
                            });
                            if let Some(next) = persist_announced_agent_profile_snapshot(
                                current.as_ref(),
                                &turn_snapshot,
                                &identity,
                            ) {
                                if let Ok(value) = serde_json::to_value(&next) {
                                    let _ = profile_sessions
                                        .set_agent_profile_prompt_snapshot(&profile_agent_id, &value);
                                }
                            }
                        }
                    }) as ProductionTurnProfileAnnouncementCommit
                });
            let lifecycle_bindings = ProductionTurnAgentLifecycleBindings::new(
                agent_id.clone(),
                worker_stream_id.clone(),
            )
            .with_disk_pressure_reminders(forever_box.disk_pressure_reminder_episodes())
            .with_profile_announcement_commit(profile_announcement_commit);
            let mut runner = worker_host_runner_composition.compose_production_runner(
                composition,
                build_bindings,
                lifecycle_bindings,
                agent_state_checkpoint_sink,
                worker_registry.upgrade_quiesce_signal(),
                Arc::clone(&worker_generated_agent_runtime),
                Some(Arc::clone(&observation)),
                Some(Arc::clone(&worker_cloud_agent_watches)),
                Some(Arc::clone(&background_shell_watches)),
                Some(worker_host_runner_composition.computer_use_coordination()),
            );
            let generated_prompt = lifecycle_messages
                .iter()
                .rev()
                .find(|message| message.role == "user" && !message.content.trim().is_empty())
                .map(|message| message.content.trim().to_string())
                .unwrap_or_default();
            let generated_parent = worker_generated_parent_agent_id.clone();
            if let Some(parent_agent_id) = generated_parent.as_deref() {
                if let Ok(Some(pending)) = runner.begin_generated_subagent_with_lifecycle(
                    parent_agent_id,
                    "shipping-runner",
                    &agent_id,
                    &worker_generated_subagent_type,
                    &worker_generated_tool_call_id,
                    &generated_prompt,
                    worker_generated_lineage.clone(),
                    &generated_lifecycle_revision,
                    started_at_ms(),
                ) {
                    if let Some(store) = worker_transcript_runtime.pending_wake_store() {
                        let quiet_origin = pending
                            .quiet_origin
                            .as_ref()
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                            .as_ref()
                            .and_then(coerce_quiet_origin);
                        let written = store.mark_pending(DurablePendingWakeMarker {
                            agent_id: pending.parent_agent_id.clone(),
                            kind: PendingWakeKind::Subagent,
                            work_id: pending.work_id.clone(),
                            marked_at_ms: pending.marked_at_ms as f64,
                            quiet_origin,
                            title: Some(pending.title.clone()),
                            subagent_type: Some(pending.subagent_type.clone()),
                            interrupted_by_recreate: false,
                            delivery_operation_id: None,
                            address_token: None,
                            failure_reason: None,
                            session_key: None,
                        });
                        if !written {
                            eprintln!(
                                "mahayana-host pending_subagent_wake_persist_failed agent={} work={}",
                                pending.parent_agent_id, pending.work_id
                            );
                        }
                        worker_events.publish(serde_json::json!({
                            "channel": "pending-wake",
                            "payload": {
                                "agentId": pending.parent_agent_id,
                                "kind": "subagent",
                                "workId": pending.work_id,
                                "outcome": if written { "persisted" } else { "persist_failed" },
                            }
                        }));
                        publish_async_tasks_changed(
                            &worker_events,
                            worker_transcript_runtime.as_ref(),
                            worker_generated_agent_runtime.as_ref(),
                            worker_cloud_agent_watches.as_ref(),
                            background_shell_watches.as_ref(),
                            parent_agent_id,
                        );
                    }
                }
                worker_transcript_runtime.begin_live_subagent(parent_agent_id);
                publish_generated_subagents(
                    &worker_events,
                    &worker_generated_agent_runtime,
                    parent_agent_id,
                );
            }
            if let Ok(mut observation) = observation.lock() {
                let dispatch_perf_ms = worker_gateway_context
                    .as_ref()
                    .map(|context| context.dispatch_started.elapsed().as_secs_f64() * 1_000.0)
                    .unwrap_or_default();
                observation.observe_send_dispatch(
                    0.0,
                    dispatch_perf_ms,
                    worker_enter_epoch_ms,
                    started_at_ms() as f64,
                    turn_input.options.is_fork,
                    provider.as_str(),
                );
            }
            let _ = worker_registry.mark_routed_provider_dispatched(&worker_stream_id);
            let turn_epoch = worker_transcript_runtime.current_turn_epoch(&agent_id);
            turn_input.options.turn_started_at_ms = Some(runner_started_at_ms);
            let mut result = runner.run_routed_provider_with_projected_turn_input(
                &data_dir,
                &lifecycle_messages,
                &provider_messages,
                turn_input.clone(),
                &mut on_text_delta,
            );
            if provider == RoutedProvider::Codex
                && let Err(ProviderSessionError::ProviderRefusal(refusal)) = result.as_ref()
            {
                let explanation = refusal
                    .explanation
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                match (
                    explanation,
                    worker_sessions.read_session_work_admission(&agent_id),
                ) {
                    (Some(explanation), Ok(Some(admission))) => {
                        let review = SessionProviderReview {
                            id: uuid::Uuid::new_v4().to_string(),
                            session_id: admission.session_id,
                            run_id: worker_stream_id.clone(),
                            provider: "openai".into(),
                            model: turn_model_id.clone(),
                            runtime_id: "codex".into(),
                            api: Some("openai-chatgpt-responses".into()),
                            native_thread_id: None,
                            native_turn_id: None,
                            explanation,
                            continuation_message: refusal.continuation_message.clone(),
                            error_type: refusal.error_type.clone(),
                        };
                        if let Err(error) = worker_sessions.record_session_provider_review(
                            &agent_id,
                            &admission.lifecycle_revision,
                            &review,
                        ) {
                            eprintln!(
                                "mahayana-host provider_review_record_failed agent={} run={} error={error}",
                                agent_id, worker_stream_id
                            );
                        }
                    }
                    (None, _) => eprintln!(
                        "mahayana-host provider_review_rejected_missing_explanation agent={} run={}",
                        agent_id, worker_stream_id
                    ),
                    (_, Ok(None)) => eprintln!(
                        "mahayana-host provider_review_rejected_missing_admission agent={} run={}",
                        agent_id, worker_stream_id
                    ),
                    (_, Err(error)) => eprintln!(
                        "mahayana-host provider_review_admission_read_failed agent={} run={} error={error}",
                        agent_id, worker_stream_id
                    ),
                }
            }
            let mut waiting_user = matches!(
                runner.last_finished().map(|finished| &finished.outcome),
                Some(mahayana_host_runtime::runner::TerminalOutcome::WaitingUser)
            );
            let mut reply_nudge_attempts = 0usize;
            let mut stream_output_produced = result
                .as_ref()
                .ok()
                .is_some_and(|content| !content.is_empty());
            let is_user_turn = !worker_turn_hidden
                && worker_request_source
                    .as_deref()
                    .is_none_or(|source| source == "turn");
            if is_user_turn {
                while should_attempt_reply_nudge(
                    send_message_delivery_counter.count(),
                    reaction_delivery_counter.count() > 0,
                    reply_nudge_attempts,
                    turn_epoch,
                    worker_transcript_runtime.current_turn_epoch(&agent_id),
                    worker_cancellation.is_cancelled(),
                    waiting_user,
                    result.is_ok(),
                ) {
                    reply_nudge_attempts = reply_nudge_attempts.saturating_add(1);
                    let nudge_input = shape_reply_nudge_turn_input(
                        &lifecycle_messages,
                        &provider_messages,
                        &turn_input.options,
                    );
                    let mut suppress_hidden_nudge_delta = |_delta: &str, _accumulated: &str| {};
                    result = runner.run_routed_provider_with_projected_messages(
                        &data_dir,
                        &nudge_input.lifecycle_messages,
                        &nudge_input.provider_messages,
                        nudge_input.options,
                        &mut suppress_hidden_nudge_delta,
                    );
                    stream_output_produced |= result
                        .as_ref()
                        .ok()
                        .is_some_and(|content| !content.is_empty());
                    waiting_user = matches!(
                        runner.last_finished().map(|finished| &finished.outcome),
                        Some(mahayana_host_runtime::runner::TerminalOutcome::WaitingUser)
                    );
                }
            }

            let should_attempt_closing_send_nudge = is_user_turn
                && result.is_ok()
                && !worker_cancellation.is_cancelled()
                && !waiting_user
                && turn_epoch == worker_transcript_runtime.current_turn_epoch(&agent_id)
                && runner
                    .last_finished()
                    .is_some_and(|finished| finished.ended_on_silent_tool_calls);
            if should_attempt_closing_send_nudge {
                let nudge_input = shape_closing_send_nudge_turn_input(
                    &lifecycle_messages,
                    &provider_messages,
                    &turn_input.options,
                );
                let mut suppress_hidden_nudge_delta = |_delta: &str, _accumulated: &str| {};
                result = runner.run_routed_provider_with_projected_messages(
                    &data_dir,
                    &nudge_input.lifecycle_messages,
                    &nudge_input.provider_messages,
                    nudge_input.options,
                    &mut suppress_hidden_nudge_delta,
                );
                stream_output_produced |= result
                    .as_ref()
                    .ok()
                    .is_some_and(|content| !content.is_empty());
                waiting_user = matches!(
                    runner.last_finished().map(|finished| &finished.outcome),
                    Some(mahayana_host_runtime::runner::TerminalOutcome::WaitingUser)
                );
                let closing_send_fields = ClosingSendNudgeFields {
                    conversation_id: agent_id.clone(),
                    delivered: !is_delivery_owed(
                        send_message_delivery_counter.count(),
                        reaction_delivery_counter.count() > 0,
                    ),
                    sent_message_count: send_message_delivery_counter.count(),
                    aborted: worker_cancellation.is_cancelled(),
                };
                if let Err(error) =
                    worker_telemetry_logs.report_closing_send_nudge(&closing_send_fields)
                {
                    eprintln!(
                        "mahayana-host closing_send_nudge_telemetry_failed agent={} error={error}",
                        agent_id
                    );
                }
            }

            // Context compaction is admitted only after the real provider turn
            // has settled its final response. Planning is pure; summarization
            // uses the existing Runner inference binding; the session owner is
            // the only writer. Any missing usage, admission, unsupported
            // metadata, cancellation, or summarizer failure leaves the prior
            // transcript authoritative.
            if is_user_turn
                && result.is_ok()
                && !waiting_user
                && !worker_cancellation.is_cancelled()
            {
                let observed_usage = worker_provider_usage
                    .lock()
                    .ok()
                    .and_then(|usage| *usage);
                let context_tokens = observed_usage
                    .map(|usage| usage.input_tokens)
                    .unwrap_or_default();
                if context_tokens > 0 {
                    let compaction_entries = match worker_sessions
                        .read_agent_transcript_entries(&agent_id)
                    {
                        Ok(entries) => entries,
                        Err(error) => {
                            eprintln!(
                                "mahayana-host compaction_transcript_read_failed agent={} run={} error={error}",
                                agent_id, worker_stream_id
                            );
                            Vec::new()
                        }
                    };
                    let settings = CompactionSettings {
                        enabled: true,
                        reserve_tokens: 16_000,
                        keep_recent_tokens: 8_000,
                    };
                    if let Some(plan) = prepare_compaction_plan(
                        &compaction_entries,
                        context_tokens,
                        SAND_AGENT_TOKEN_LIMIT as u64,
                        settings,
                        None,
                    ) {
                        match worker_sessions.read_session_work_admission(&agent_id) {
                                Ok(Some(admission)) => {
                                    match build_compaction_summary_prompt(&plan) {
                                        Ok(summary_prompt) => {
                                            let commit_compaction = |summary: String,
                                                                      quality_degraded: bool| {
                                                let commit = CompactionCommit {
                                                    entry_id: format!(
                                                        "compaction:{}:{}",
                                                        agent_id, worker_stream_id
                                                    ),
                                                    expected_transcript_tail_id: plan
                                                        .observed_transcript_tail_id
                                                        .clone(),
                                                    first_kept_entry_id: Some(
                                                        plan.first_kept_entry_id.clone(),
                                                    ),
                                                    summary,
                                                    tokens_before: plan.tokens_before,
                                                    details: build_compaction_details_with_quality(
                                                        &plan,
                                                        quality_degraded,
                                                    ),
                                                    source_turn_id: worker_stream_id.clone(),
                                                    expected_lifecycle_revision: admission
                                                        .lifecycle_revision
                                                        .clone(),
                                                };
                                                match worker_sessions
                                                    .commit_agent_compaction(&agent_id, &commit)
                                                {
                                                    Ok(CompactionCommitOutcome::Committed(_))
                                                    | Ok(CompactionCommitOutcome::AlreadyCommitted(_)) => {}
                                                    Ok(CompactionCommitOutcome::RejectedStale) => {
                                                        eprintln!(
                                                            "mahayana-host compaction_rejected_stale agent={} run={}",
                                                            agent_id, worker_stream_id
                                                        );
                                                    }
                                                    Err(error) => eprintln!(
                                                        "mahayana-host compaction_commit_failed agent={} run={} error={error}",
                                                        agent_id, worker_stream_id
                                                    ),
                                                }
                                            };
                                            let should_cancel = || worker_cancellation.is_cancelled();
                                            match summarization_prompt(
                                                "You are the Fabushi conversation-compaction summarizer. Return only the durable summary text.",
                                                &summary_prompt,
                                                &should_cancel,
                                            ) {
                                                Ok(summary) if !should_cancel() => {
                                                    let summary = if plan.is_split_turn
                                                        && !plan.turn_prefix_entries.is_empty()
                                                    {
                                                        match build_turn_prefix_summary_prompt(&plan) {
                                                            Ok(prefix_prompt) => match summarization_prompt(
                                                                "You are the Fabushi split-turn prefix summarizer. Return only the concise prefix context.",
                                                                    &prefix_prompt,
                                                                    &should_cancel,
                                                                ) {
                                                                    Ok(prefix) if !should_cancel() => Some((format!(
                                                                        "{summary}\n\n---\n\n**Turn Context (split turn):**\n\n{prefix}"
                                                                    ), false)),
                                                                    Ok(_) => None,
                                                                    Err(_) => Some((compaction_without_summary(&plan), true)),
                                                                },
                                                            Err(_) => Some((compaction_without_summary(&plan), true)),
                                                        }
                                                    } else {
                                                        Some((summary, false))
                                                    };
                                                    if let Some((summary, mut quality_degraded)) = summary {
                                                        let details =
                                                            build_compaction_details_with_quality(
                                                                &plan,
                                                                quality_degraded,
                                                            );
                                                        let suffix =
                                                            format_compaction_details_suffix(details.as_ref());
                                                        let summary = match fit_compaction_summary_with_suffix(
                                                            &summary,
                                                            plan.summary_token_budget,
                                                            &suffix,
                                                        ) {
                                                            Ok(summary) => summary,
                                                            Err(error) => {
                                                                quality_degraded = true;
                                                                eprintln!(
                                                                    "mahayana-host compaction_summary_invalid agent={} run={} error={error}",
                                                                    agent_id, worker_stream_id
                                                                );
                                                                let fallback =
                                                                    compaction_without_summary(&plan);
                                                                let fallback_details =
                                                                    build_compaction_details_with_quality(
                                                                        &plan,
                                                                        true,
                                                                    );
                                                                let fallback_suffix =
                                                                    format_compaction_details_suffix(
                                                                        fallback_details.as_ref(),
                                                                    );
                                                                fit_compaction_summary_with_suffix(
                                                                    &fallback,
                                                                    plan.summary_token_budget,
                                                                    &fallback_suffix,
                                                                )
                                                                .unwrap_or(fallback)
                                                            }
                                                        };
                                                        commit_compaction(summary, quality_degraded);
                                                    } else {
                                                        eprintln!(
                                                            "mahayana-host compaction_cancelled_during_split_prefix agent={} run={}",
                                                            agent_id, worker_stream_id
                                                        );
                                                    }
                                                }
                                                Ok(_) => eprintln!(
                                                    "mahayana-host compaction_cancelled_after_summary agent={} run={}",
                                                    agent_id, worker_stream_id
                                                ),
                                                Err(error) => {
                                                    eprintln!(
                                                        "mahayana-host compaction_summary_failed agent={} run={} error={error}",
                                                        agent_id, worker_stream_id
                                                    );
                                                    if !should_cancel() {
                                                        let fallback = compaction_without_summary(&plan);
                                                        let fallback_details =
                                                            build_compaction_details_with_quality(
                                                                &plan,
                                                                true,
                                                            );
                                                        let fallback_suffix =
                                                            format_compaction_details_suffix(
                                                                fallback_details.as_ref(),
                                                            );
                                                        commit_compaction(
                                                            fit_compaction_summary_with_suffix(
                                                                &fallback,
                                                                plan.summary_token_budget,
                                                                &fallback_suffix,
                                                            )
                                                            .unwrap_or(fallback),
                                                            true,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        Err(error) => eprintln!(
                                            "mahayana-host compaction_prompt_build_failed agent={} run={} error={error}",
                                            agent_id, worker_stream_id
                                        ),
                                    }
                                }
                                Ok(None) => eprintln!(
                                    "mahayana-host compaction_missing_admission agent={} run={}",
                                    agent_id, worker_stream_id
                                ),
                                Err(error) => eprintln!(
                                    "mahayana-host compaction_admission_read_failed agent={} run={} error={error}",
                                    agent_id, worker_stream_id
                                ),
                        }
                    }
                }
            }

            if should_run_completed_turn_side_effects(
                worker_generated_parent_agent_id.is_some(),
                worker_turn_hidden,
                waiting_user,
                worker_cancellation.is_cancelled(),
                result.is_ok(),
            ) && turn_epoch == worker_transcript_runtime.current_turn_epoch(&agent_id)
            {
                if let Ok(content) = result.as_ref() {
                    let mut labeled_messages = lifecycle_messages.clone();
                    if !content.is_empty() {
                        labeled_messages.push(ProviderMessage {
                            role: "assistant".into(),
                            content: content.clone(),
                        });
                    }
                    settle_labeling_auth.record_post_turn_labeling(&labeled_messages);
                }
            }

            let turn_interrupted = worker_cancellation.reason().is_some_and(|reason| {
                matches!(
                    reason.as_str(),
                    RUN_DIRECT_USER_INTERRUPT_REASON | RUN_WATCHDOG_INTERRUPT_REASON
                )
            });
            if let Ok(mut observation) = observation.lock() {
                observation.flush_pending_awaits_on_unwind(turn_interrupted);
            }

            if is_user_turn {
                let observed_tool_call_count = observation
                    .lock()
                    .map(|observation| observation.observed_tool_call_count())
                    .unwrap_or_default();
                if let Some(report) = build_turn_empty_delivery_report(
                    &agent_id,
                    Some(&worker_stream_id),
                    worker_request_source.as_deref().or(Some("turn")),
                    reply_nudge_attempts,
                    observed_tool_call_count,
                    stream_output_produced,
                    started_at_ms().saturating_sub(runner_started_at_ms),
                    worker_ack_obligations.store().get(&agent_id).is_some(),
                    send_message_delivery_counter.count(),
                    reaction_delivery_counter.count() > 0,
                    waiting_user,
                    worker_cancellation.is_cancelled(),
                    result.is_ok(),
                    turn_epoch,
                    worker_transcript_runtime.current_turn_epoch(&agent_id),
                ) {
                    if let Err(error) = worker_telemetry_logs.report_turn_empty_delivery(&report) {
                        eprintln!(
                            "mahayana-host empty_delivery_telemetry_failed agent={agent_id} error={error}"
                        );
                    }
                }
            }

            if let (Ok(content), Some(worker_memory_store)) =
                (result.as_ref(), worker_memory_store.as_ref())
            {
                if let Some(user_prompt) = lifecycle_messages
                    .iter()
                    .rev()
                    .find(|message| {
                        message.role == "user" && !message.content.trim().is_empty()
                    })
                    .map(|message| message.content.trim().to_string())
                {
                    let memory_evidence_enabled = worker_memory_service.synthesis_enabled();
                    if should_run_turn_memory(
                        turn_epoch == worker_transcript_runtime.current_turn_epoch(&agent_id),
                        worker_turn_hidden,
                        waiting_user,
                        result.is_ok(),
                        memory_evidence_enabled,
                        is_memorable_exchange(&user_prompt),
                    ) {
                        {
                            let episode_progress = ProductionEpisodeProgress::new(
                                Arc::clone(&worker_retire_sessions),
                                &agent_id,
                            );
                            let memory_cancellation = worker_cancellation.clone();
                            let mut execute_memory_prompt =
                                |system_prompt: &str, user_prompt: &str| -> Result<String, String> {
                                    let messages = vec![
                                        ProviderMessage {
                                            role: "system".into(),
                                            content: system_prompt.to_string(),
                                        },
                                        ProviderMessage {
                                            role: "user".into(),
                                            content: user_prompt.to_string(),
                                        },
                                    ];
                                    let mut reject_tool = |
                                        _tool: &RoutedToolDefinition,
                                        _args: serde_json::Value,
                                        _tool_call_id: &str,
                                    | -> Result<serde_json::Value, ProviderSessionError> {
                                        Err(ProviderSessionError::Tool(
                                            "turn-memory inference exposes no tools".into(),
                                        ))
                                    };
                                    let mut ignore_delta = |_delta: &str, _accumulated: &str| {};
                                    let should_cancel = || memory_cancellation.is_cancelled();
                                    let mut options = RoutedProviderOptions {
                                        data_dir: &data_dir,
                                        typed_replay: None,
                                        cursor_auth: Some(auth.clone()),
                                        tools: &[],
                                        mcp_server_url: None,
                                        execute_tool: &mut reject_tool,
                                        on_text_delta: &mut ignore_delta,
                                        on_partial_tool_call: None,
                                        should_cancel: &should_cancel,
                                    };
                                    run_routed_provider_text(provider, &messages, &mut options)
                                        .map_err(|error| error.to_string())
                                };
                            let remembered_agent_messages = turn_agent_messages
                                .lock()
                                .map(|messages| messages.clone())
                                .unwrap_or_default();
                            let exchange = build_turn_memory_exchange(
                                user_prompt,
                                remembered_agent_messages,
                                content.clone(),
                            );
                            let mut record_memory_evidence = |exchange: &TurnExchange| {
                                let _ = worker_memory_service.record_memory_evidence(
                                    &agent_id,
                                    None,
                                    &exchange.user,
                                    &exchange.agent,
                                    runner_started_at_ms as i64,
                                );
                            };
                            let memory_mode = if memory_evidence_enabled {
                                TurnMemoryMode::RecordEvidence(&mut record_memory_evidence)
                            } else {
                                TurnMemoryMode::Extract
                            };
                            let _ = run_turn_memory_with(
                                worker_memory_store,
                                Some(&episode_progress),
                                runner_started_at_ms as i64,
                                exchange,
                                memory_mode,
                                &mut execute_memory_prompt,
                            );
                        }
                    }
                }
            }

            if worker_is_ack_redrive
                && !waiting_user
                && !worker_cancellation.is_cancelled()
                && result.is_ok()
            {
                let observed_tool_call_count = observation
                    .lock()
                    .map(|observation| observation.observed_tool_call_count())
                    .unwrap_or_default();
                let stream_output_produced = result
                    .as_ref()
                    .ok()
                    .is_some_and(|content| !content.is_empty());
                let obligation = worker_ack_obligations.store().get(&agent_id);
                if let Some(report) = build_ack_redrive_empty_delivery_report(
                    obligation.as_ref(),
                    &agent_id,
                    Some(&worker_stream_id),
                    worker_request_source.as_deref(),
                    observed_tool_call_count,
                    stream_output_produced,
                    started_at_ms().saturating_sub(runner_started_at_ms),
                ) {
                    if let Err(error) = worker_telemetry_logs.report_turn_empty_delivery(&report) {
                        eprintln!(
                            "mahayana-host-ack empty_delivery_telemetry_failed agent={agent_id} error={error}"
                        );
                    }
                }
            }

            if let Some(parent_agent_id) = generated_parent.as_deref() {
                let generated_outcome = match result.as_ref() {
                    Ok(content) => GeneratedSubagentRunOutcome::Completed(content.clone()),
                    Err(_) if worker_cancellation.is_cancelled() => GeneratedSubagentRunOutcome::Aborted,
                    Err(error) => GeneratedSubagentRunOutcome::Error(error.to_string()),
                };
                if let Ok(settled) = runner.settle_generated_subagent(
                    &agent_id,
                    generated_outcome,
                    started_at_ms(),
                ) {
                    if let Some(usage) = settled.computer_use_usage {
                        let fields = ComputerUseUsageFields {
                            parent_agent_id: usage.parent_agent_id,
                            subagent_agent_id: usage.subagent_agent_id,
                            subagent_type: usage.subagent_type,
                            subagent_request_id: usage.subagent_request_id,
                            model_id: usage.model_id.unwrap_or_default(),
                            outcome: usage.outcome,
                            duration_ms: usage.duration_ms as f64,
                            tool_call_count: u64::try_from(usage.tool_call_count).unwrap_or(u64::MAX),
                            turn_ended_count: usage.turn_ended_count,
                            usage: usage.usage.map(|usage| TelemetryTokenUsage {
                                input_tokens: usage.input_tokens,
                                output_tokens: usage.output_tokens,
                                cache_read_tokens: usage.cache_read_tokens,
                                cache_write_tokens: usage.cache_write_tokens,
                                reasoning_tokens: usage.reasoning_tokens,
                            }),
                        };
                        if let Err(error) = worker_telemetry_logs.report_computer_use_usage(&fields) {
                            eprintln!(
                                "mahayana-host computer_use_usage_telemetry_failed agent={agent_id} error={error}"
                            );
                        }
                    }
                    if let Some(completion) = settled.completion {
                        let cleanup_agent_id = completion.subagent_agent_id.clone();
                        let cleanup_revision = completion.lifecycle_revision.clone();
                        let quiet_origin = completion
                            .quiet_origin
                            .as_ref()
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                            .as_ref()
                            .and_then(coerce_quiet_origin);
                        let pending_wake_marker = DurablePendingWakeMarker {
                            agent_id: completion.parent_agent_id.clone(),
                            kind: PendingWakeKind::Subagent,
                            work_id: completion.subagent_agent_id.clone(),
                            marked_at_ms: completion.marked_at_ms as f64,
                            quiet_origin: quiet_origin.clone(),
                            title: Some(completion.title.clone()),
                            subagent_type: Some(completion.subagent_type.clone()),
                            interrupted_by_recreate: false,
                            delivery_operation_id: None,
                            address_token: None,
                            failure_reason: None,
                            session_key: None,
                        };
                        publish_async_tasks_changed(
                            &worker_events,
                            worker_transcript_runtime.as_ref(),
                            worker_generated_agent_runtime.as_ref(),
                            worker_cloud_agent_watches.as_ref(),
                            background_shell_watches.as_ref(),
                            &completion.parent_agent_id,
                        );
                        worker_completion_revivals.handle_background_subagent_completion(
                            SubagentCompletion {
                                parent_agent_id: completion.parent_agent_id,
                                subagent_agent_id: completion.subagent_agent_id,
                                title: completion.title,
                                subagent_type: completion.subagent_type,
                                status: completion.status,
                                result: completion.result,
                                quiet_origin,
                                pending_wake_marker: Some(pending_wake_marker),
                            },
                        );
                        let cleanup = worker_generated_cleanup_slot
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone();
                        if let Some(cleanup) = cleanup {
                            if let Err(error) = cleanup(&cleanup_agent_id, &cleanup_revision) {
                                eprintln!(
                                    "mahayana-host generated_subagent_cleanup_failed agent={} error={}",
                                    cleanup_agent_id, error
                                );
                            }
                        }
                    }
                }
                worker_transcript_runtime.end_live_subagent(parent_agent_id);
                publish_generated_subagents(
                    &worker_events,
                    &worker_generated_agent_runtime,
                    parent_agent_id,
                );
            }

            // A terminal inference event is the renderer-visible completion
            // boundary. Do not publish it until the Host has actually settled
            // the run: otherwise the UI can render the final assistant turn
            // while the registry/live session still owns the provider, and an
            // immediate app quit races that cleanup path.
            let usage_report = worker_transcript_runtime.settle_turn_usage(
                &agent_id,
                worker_request_source.as_deref().unwrap_or("turn"),
            );
            let usage_fields = TurnUsageFields {
                conversation_id: usage_report.agent_id.clone(),
                source: usage_report.source,
                request_id: usage_report.request_id,
                request_id_count: u64::try_from(usage_report.request_id_count)
                    .unwrap_or(u64::MAX),
                turn_ended_seq: usage_report.turn_ended_seq,
                usage: worker_provider_usage
                    .lock()
                    .ok()
                    .and_then(|usage| *usage)
                    .map(|usage| TelemetryTokenUsage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_read_tokens: usage.cache_read_tokens,
                        cache_write_tokens: usage.cache_write_tokens,
                        reasoning_tokens: usage.reasoning_tokens,
                    }),
            };
            if let Err(error) = worker_telemetry_logs.report_turn_usage(&usage_fields) {
                eprintln!(
                    "mahayana-host turn_usage_telemetry_failed agent={} error={error}",
                    agent_id
                );
            }
            if !waiting_user && !worker_cancellation.is_cancelled() {
                if let (Some(conversation_id), Some(content)) = (
                    worker_human_handoff_conversation_id.as_deref(),
                    result.as_ref().ok().cloned(),
                ) {
                    match worker_human_handoff_sessions.append_human_agent_message(
                        conversation_id,
                        &agent_id,
                        &worker_stream_id,
                        worker_human_handoff_session_key.as_deref(),
                        &content,
                        started_at_ms() as f64,
                    ) {
                        Ok(entry) => worker_events.publish(serde_json::json!({
                            "channel": "transcript",
                            "payload": {
                                "type": "appended",
                                "agentId": conversation_id,
                                "entry": entry,
                            }
                        })),
                        Err(error) => {
                            result = Err(ProviderSessionError::Tool(format!(
                                "could not persist Human handoff Agent result: {error}"
                            )));
                        }
                    }
                }
            }

            let sent_message_count = send_message_delivery_counter.count();
            let reacted = reaction_delivery_counter.count() > 0;
            if worker_request_source.as_deref() == Some("provider-review") {
                let outcome = if worker_cancellation.is_cancelled() {
                    ProviderReviewContinuationOutcome::Cancelled
                } else if waiting_user {
                    ProviderReviewContinuationOutcome::OutcomeUnknown
                } else if result.is_ok() {
                    ProviderReviewContinuationOutcome::Completed
                } else {
                    ProviderReviewContinuationOutcome::Failed
                };
                if let Some(expected_revision) =
                    worker_provider_review_lifecycle_revision.as_deref()
                {
                    if let Err(error) = worker_sessions.settle_provider_review_continuation(
                        &agent_id,
                        expected_revision,
                        &worker_stream_id,
                        outcome,
                    ) {
                        eprintln!(
                            "mahayana-host provider_review_settlement_failed agent={} run={} error={error}",
                            agent_id, worker_stream_id
                        );
                    }
                } else {
                    eprintln!(
                        "mahayana-host provider_review_settlement_missing_generation agent={} run={}",
                        agent_id, worker_stream_id
                    );
                }
            }
            let terminal_projection = project_turn_terminal(
                waiting_user,
                worker_cancellation.is_cancelled(),
                result.is_ok(),
                sent_message_count,
                reacted,
            );
            worker_transcript_runtime.track_runner_activity_update(
                &agent_id,
                &ActivityUpdate::TurnEnded,
                started_at_ms(),
            );
            if let Some(preview) = group_preview.as_ref()
                && let Ok(mut preview) = preview.lock()
                && let Some(payload) = preview.finish()
            {
                worker_events.publish(serde_json::json!({
                    "channel": "transcript",
                    "payload": payload,
                }));
            }
            if prompt_role == RunnerPromptRole::ComputerUseSubagent {
                let usage = worker_provider_usage
                    .lock()
                    .ok()
                    .and_then(|usage| *usage)
                    .map(|usage| mahayana_host_runtime::runner::TurnUsage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_read_tokens: usage.cache_read_tokens,
                        cache_write_tokens: usage.cache_write_tokens,
                        reasoning_tokens: usage.reasoning_tokens,
                    });
                worker_host_runner_composition.finish_computer_use_turn(
                    &agent_id,
                    computer_control_lease.as_ref(),
                    worker_session_options.model_id.as_deref(),
                    usage,
                );
            }
            worker_transcript_runtime.end_provider_run_with_kind(&agent_id, is_group_member_turn);
            worker_registry.finish_routed_provider(&worker_stream_id);
            auto_review_service.unbind_runner(
                &agent_id,
                SandAutoReviewExpiryCause::SessionEnd,
            );
            if !is_group_member_turn {
                worker_host_runner_composition.unbind_local_permission_surface(&agent_id);
            }
            worker_ack_obligations.retire_ack_run_token(
                &agent_id,
                worker_ack_token.as_deref(),
            );
            if result.is_ok() {
                if worker_is_upgrade_resume {
                    let _ = worker_transcript_manager.emit_agent_update(&agent_id);
                }
                let _ = worker_transcript_manager.emit_automations(&agent_id);
            }
            worker_routed_turn_lease.settle();
            let _ = worker_transcript_runtime
                .retire_idle_live_session(&worker_retire_sessions, &agent_id);
            let _ = worker_box_store_sync.schedule_store_db_snapshot(&agent_id);

            if let Ok(mut turn) = structured_turn.lock() {
                match terminal_projection.kind {
                    TurnTerminalKind::WaitingUser
                    | TurnTerminalKind::Cancelled
                    | TurnTerminalKind::Completed => {
                        turn.finalize(terminal_projection.kind.as_str(), None, None);
                    }
                    TurnTerminalKind::Failed => {
                        if let Err(error) = result.as_ref() {
                            let classified = classify_agent_error(error);
                            let detail = sand_error_detail(error);
                            turn.finalize(
                                terminal_projection.kind.as_str(),
                                Some(&classified),
                                Some((&detail.message, detail.stack.as_deref())),
                            );
                        } else {
                            turn.finalize(terminal_projection.kind.as_str(), None, None);
                        }
                    }
                }
            }

            if worker_cancellation.is_cancelled() {
                set_turn_trace_attributes(
                    turn_trace.as_ref(),
                    &BTreeMap::from([(
                        "sand.outcome".into(),
                        serde_json::json!("aborted"),
                    )]),
                );
            } else if waiting_user {
                set_turn_trace_attributes(
                    turn_trace.as_ref(),
                    &BTreeMap::from([(
                        "sand.outcome".into(),
                        serde_json::json!("awaiting_user"),
                    )]),
                );
            } else if let Some(error) = result.as_ref().err() {
                mark_turn_trace_error(turn_trace.as_ref(), error);
            } else {
                set_turn_trace_attributes(
                    turn_trace.as_ref(),
                    &BTreeMap::from([(
                        "sand.outcome".into(),
                        serde_json::json!("success"),
                    )]),
                );
            }
            if let Some(trace) = turn_trace.as_ref() {
                trace.span.end();
            }

            if waiting_user {
                worker_events.publish(serde_json::json!({
                    "channel": RUNNER_INFERENCE_EVENT_CHANNEL,
                    "payload": {
                        "streamId": stream_id,
                        "type": "completed",
                        "content": "",
                        "waitingUser": true,
                        "sentMessageCount": sent_message_count,
                        "reacted": reacted
                    }
                }));
            } else if worker_cancellation.is_cancelled() {
                worker_events.publish(serde_json::json!({
                    "channel": RUNNER_INFERENCE_EVENT_CHANNEL,
                    "payload": {
                        "streamId": stream_id,
                        "type": "cancelled",
                        "message": "Runner provider request cancelled",
                        "sentMessageCount": sent_message_count,
                        "reacted": reacted
                    }
                }));
            } else {
                match result {
                    Ok(content) => worker_events.publish(serde_json::json!({
                        "channel": RUNNER_INFERENCE_EVENT_CHANNEL,
                        "payload": {
                            "streamId": stream_id,
                            "type": "completed",
                            "content": content,
                            "sentMessageCount": sent_message_count,
                            "reacted": reacted
                        }
                    })),
                    Err(error) => {
                        let message = error.to_string();
                        if worker_is_ack_redrive || worker_is_upgrade_resume {
                            let report = AgentErrorReport {
                                source: if worker_is_upgrade_resume {
                                    "resume".into()
                                } else {
                                    "ack_redrive".into()
                                },
                                conversation_id: agent_id.clone(),
                                request_id: Some(worker_stream_id.clone()),
                                error: classify_agent_error(&error),
                                detail: Some(sand_error_detail(&error)),
                            };
                            if let Err(telemetry_error) =
                                worker_telemetry_logs.report_agent_error(&report)
                            {
                                eprintln!(
                                    "mahayana-host-ack agent_error_telemetry_failed agent={agent_id} error={telemetry_error}"
                                );
                            }
                        }
                        if !worker_is_kickstart && !worker_is_handoff_resume {
                            let mut tray = provider_failure_tray(
                                &agent_id,
                                &message,
                                started_at_ms() as i64,
                            );
                            if worker_is_upgrade_resume {
                                tray.title = "Agent failed to resume after host update".into();
                            }
                            worker_trays.push_error(tray);
                        }
                        worker_events.publish(serde_json::json!({
                            "channel": RUNNER_INFERENCE_EVENT_CHANNEL,
                            "payload": {
                                "streamId": stream_id,
                                "type": "failed",
                                "message": message,
                                "sentMessageCount": sent_message_count,
                                "reacted": reacted
                            }
                        }))
                    },
                }
            }
        });
    if let Err(error) = spawn {
        transcript_runtime.end_provider_run_with_kind(
            &spawn_error_agent_id,
            is_group_member_turn,
        );
        runner_registry.finish_routed_provider(&accepted_stream_id);
        spawn_error_auto_review.unbind_runner(
            &spawn_error_agent_id,
            SandAutoReviewExpiryCause::SessionEnd,
        );
        if !is_group_member_turn {
            host_runner_composition.unbind_local_permission_surface(&spawn_error_agent_id);
        }
        ack_obligations.retire_ack_run_token(
            &spawn_error_agent_id,
            ack_token.as_deref(),
        );
        let _ = transcript_runtime
            .retire_idle_live_session(&session_workers, &spawn_error_agent_id);
        return Err(GatewayCommandError::Internal(format!(
            "could not start routed provider Runner: {error}"
        )));
    }
    Ok(serde_json::json!({
        "accepted": true,
        "streamId": accepted_stream_id,
        "provider": provider.as_str(),
    }))
}

impl GatewayApi for UnifiedGatewayApi {
    fn authenticate_device_token(
        &self,
        token: &str,
    ) -> Result<Option<mahayana_host_runtime::device_pairing_owner::DeviceTokenAuthorization>, GatewayCommandError> {
        self.device_pairing.authenticate_device_token(token)
    }

    fn authorize_device_token_request(
        &self,
        authorization: &mahayana_host_runtime::device_pairing_owner::DeviceTokenAuthorization,
        method: &str,
        args: &serde_json::Value,
    ) -> Result<(), GatewayCommandError> {
        if authorization.role != "node" {
            return Ok(());
        }
        let target = match method {
            "node.invoke" => args
                .get("nodeId")
                .and_then(serde_json::Value::as_str),
            "node.register" => args
                .get("nodeId")
                .or_else(|| args.get("deviceId"))
                .or_else(|| args.get("client").and_then(|value| value.get("id")))
                .or_else(|| args.get("device").and_then(|value| value.get("id")))
                .and_then(serde_json::Value::as_str),
            "device.pair.request" | "device.pair.status" => {
                args.get("deviceId").and_then(serde_json::Value::as_str)
            }
            _ => return Ok(()),
        }
        .map(str::trim)
        .filter(|value| !value.is_empty());

        let Some(target) = target else {
            return Err(GatewayCommandError::Forbidden(
                "DEVICE_SCOPE_REQUIRED: node device token requires a target device identity".into(),
            ));
        };
        if target != authorization.device_id {
            return Err(GatewayCommandError::Forbidden(
                "DEVICE_SCOPE_MISMATCH: node device token cannot address another device".into(),
            ));
        }
        Ok(())
    }

    fn redeem_device_pairing_join_code(
        &self,
        shortcode: &str,
    ) -> Result<Option<serde_json::Value>, GatewayCommandError> {
        self.device_pairing.redeem_device_pairing_join_code(shortcode)
    }

    fn call(
        &self,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, GatewayCommandError> {
        self.transcript_runtime
            .session_runtime()
            .note_desktop_contact(started_at_ms() as f64);
        if is_openclaw_gateway_command(method) {
            return self.call_openclaw_gateway_command(method, args);
        }
        if method == "deliverToChannel" {
            let agent_id = args
                .get("agentId")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "deliverToChannel requires agentId".into(),
                ))?
                .to_string();
            let address_token = args
                .get("addressToken")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "deliverToChannel requires addressToken".into(),
                ))?
                .to_string();
            let message = args
                .get("message")
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "deliverToChannel requires message".into(),
                ))?;
            let delivery_session_key = args
                .get("sessionKey")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let Some(outbound) = build_channel_outbound_message(message) else {
                return Ok(serde_json::json!({"queued": false}));
            };
            let delivery = self.channel_delivery.clone();
            let deps = self.local_routed_runner_deps();
            let roster = Arc::clone(&self.roster_emit);
            let wakes = Arc::clone(&self.background_wakes);
            let worker_agent_id = agent_id.clone();
            let worker_address = address_token.clone();
            let delivery_operation_id = outbound.operation_id().map(ToOwned::to_owned);
            let worker_session_key = delivery_session_key.clone();
            if let Err(error) = thread::Builder::new()
                .name(format!("mahayana-channel-delivery-{agent_id}"))
                .spawn(move || {
                    let admission_result = match worker_session_key.as_deref() {
                        Some(session_key) => match deps
                            .session_workers
                            .read_session_work_admission(&worker_agent_id)
                        {
                            Ok(Some(record)) if record.session_key == session_key => deps
                                .session_workers
                                .assert_session_work_admission(
                                    &worker_agent_id,
                                    Some(session_key),
                                ),
                            Ok(Some(_)) => Err(
                                "channel delivery session identity changed before dispatch".into(),
                            ),
                            Ok(None) => {
                                Err("channel delivery session admission unavailable".into())
                            }
                            Err(error) => Err(error),
                        },
                        None => Ok(()),
                    };
                    if let Err(error) = admission_result
                        .and_then(|_| delivery.deliver(&worker_agent_id, &worker_address, &outbound))
                    {
                        let reason =
                            humanize_channel_delivery_failure(&worker_address, &error);
                        let mut tray = provider_failure_tray(
                            &worker_agent_id,
                            &reason,
                            started_at_ms() as i64,
                        );
                        tray.title = "Message not delivered".into();
                        deps.trays.push_error(tray);
                        queue_channel_delivery_failure(
                            deps,
                            roster,
                            wakes,
                            worker_agent_id,
                            worker_address,
                            delivery_operation_id,
                            worker_session_key,
                            reason,
                        );
                    }
                })
            {
                return Err(GatewayCommandError::Internal(format!(
                    "could not start channel delivery worker: {error}"
                )));
            }
            return Ok(serde_json::json!({"queued": true}));
        }

        if method == "resumeAfterRecreate" {
            let local_ids = self.transcript_runtime.upgrade_resume_agent_ids();
            let mut resume_ids = local_ids.clone();
            if let Some(agent_ids) = args.get("agentIds").and_then(serde_json::Value::as_array) {
                for agent_id in agent_ids.iter().filter_map(serde_json::Value::as_str) {
                    let agent_id = agent_id.trim();
                    if agent_id.is_empty() || resume_ids.iter().any(|known| known == agent_id) {
                        continue;
                    }
                    self.transcript_manager.mark_upgrade_resume_pending(UpgradeResumeMarker {
                        agent_id: agent_id.to_string(),
                        marked_at_ms: started_at_ms() as f64,
                        source: None,
                        automation_id: None,
                        automation_run_id: None,
                    });
                    resume_ids.push(agent_id.to_string());
                }
            }
            self.transcript_manager.resume_after_recreate();
            let carried = args
                .get("resumePendingWakes")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let restored_pending_wakes = self
                .transcript_manager
                .restore_recreate_pending_wakes(carried)
                .map_err(GatewayCommandError::Internal)?;
            self.resume_interrupted_upgrade_turns()
                .map_err(GatewayCommandError::Internal)?;
            return Ok(serde_json::json!({
                "resumed": resume_ids.len(),
                "restoredPendingWakes": restored_pending_wakes,
            }));
        }

        if method == "broadcastToAgents" {
            let message = args
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let text = clamp_agent_message(message);
            if text.is_empty()
                || self.transcript_runtime.is_quiescing_for_upgrade()
                || configured_routed_provider(&self.data_dir.join("settings.json")).is_none()
            {
                return Ok(serde_json::json!({"total": 0, "scheduled": 0}));
            }
            let ids = match args.get("targets") {
                Some(serde_json::Value::String(targets)) if targets == "all" => self
                    .session_workers
                    .list_agent_summaries(None)
                    .map_err(GatewayCommandError::Internal)?
                    .into_iter()
                    .map(|summary| summary.id)
                    .collect::<Vec<_>>(),
                Some(serde_json::Value::Array(targets)) => {
                    let mut ids = Vec::<String>::new();
                    for id in targets.iter().filter_map(serde_json::Value::as_str) {
                        let id = id.trim();
                        if !id.is_empty() && !ids.iter().any(|known| known == id) {
                            ids.push(id.to_string());
                        }
                    }
                    ids
                }
                _ => {
                    return Err(GatewayCommandError::BadRequest(
                        "broadcastToAgents requires targets = \"all\" or an array".into(),
                    ));
                }
            };
            let total = ids.len();
            let provider = configured_routed_provider(&self.data_dir.join("settings.json"))
                .ok_or_else(|| GatewayCommandError::Internal(
                    "no routed provider configured for broadcast".into(),
                ))?;
            let mut scheduled = 0usize;
            for agent_id in ids {
                let Ok(Some(summary)) =
                    self.session_workers.summarize_agent_by_id(&agent_id, None)
                else {
                    continue;
                };
                if summary.is_group || summary.remote_room.is_some() {
                    continue;
                }
                let deps = self.local_routed_runner_deps();
                let roster = Arc::clone(&self.roster_emit);
                let prompt = build_admin_broadcast_wake_prompt(&text);
                let worker_agent_id = agent_id.clone();
                let worker_provider = provider;
                if thread::Builder::new()
                    .name(format!("mahayana-broadcast-{agent_id}"))
                    .spawn(move || {
                        match run_local_background_revival_turn(
                            deps.clone(),
                            worker_provider,
                            &worker_agent_id,
                            "broadcast",
                            &prompt,
                            false,
                            "continue",
                        ) {
                            Ok(execution) => {
                                if !execution.aborted && execution.sent_message_count == 0 {
                                    let _ = run_local_background_revival_turn(
                                        deps.clone(),
                                        worker_provider,
                                        &worker_agent_id,
                                        "broadcast",
                                        REPLY_NUDGE_PROMPT,
                                        false,
                                        "continue",
                                    );
                                }
                                let _ = roster.emit_agent_update(&worker_agent_id);
                            }
                            Err(error) => {
                                let classified_error = ProviderSessionError::Tool(error.clone());
                                let report = AgentErrorReport {
                                    source: "broadcast".into(),
                                    conversation_id: worker_agent_id.clone(),
                                    request_id: None,
                                    error: classify_agent_error(&classified_error),
                                    detail: Some(sand_error_detail(&error)),
                                };
                                let _ = deps.telemetry_logs.report_agent_error(&report);
                                let mut tray = provider_failure_tray(
                                    &worker_agent_id,
                                    &error,
                                    started_at_ms() as i64,
                                );
                                tray.title = "Broadcast message failed".into();
                                deps.trays.push_error(tray);
                            }
                        }
                    })
                    .is_ok()
                {
                    scheduled += 1;
                }
            }
            return Ok(serde_json::json!({"total": total, "scheduled": scheduled}));
        }

        if method == "wakeForInbound" {
            let agent_id = args
                .get("agentId")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "wakeForInbound requires agentId".into(),
                ))?
                .to_string();
            let envelope = args
                .get("envelope")
                .cloned()
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "wakeForInbound requires envelope".into(),
                ))?;
            let should_spawn = {
                let mut wakes = self
                    .background_wakes
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                BackgroundWakes::enqueue(&mut wakes.pending_inbound, agent_id.clone(), envelope);
                BackgroundWakes::<serde_json::Value>::begin_revival(
                    &mut wakes.reviving_inbound_agent_ids,
                    &agent_id,
                )
            };
            if should_spawn {
                let deps = self.local_routed_runner_deps();
                let roster = Arc::clone(&self.roster_emit);
                let wakes = Arc::clone(&self.background_wakes);
                let worker_agent_id = agent_id.clone();
                if let Err(error) = thread::Builder::new()
                    .name(format!("mahayana-channel-inbound-{agent_id}"))
                    .spawn(move || {
                        run_channel_inbound_revival_worker(
                            deps,
                            roster,
                            wakes,
                            worker_agent_id,
                        );
                    })
                {
                    let mut wakes = self
                        .background_wakes
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    BackgroundWakes::<serde_json::Value>::end_revival(
                        &mut wakes.reviving_inbound_agent_ids,
                        &agent_id,
                    );
                    return Err(GatewayCommandError::Internal(format!(
                        "could not start channel inbound worker: {error}"
                    )));
                }
            }
            return Ok(serde_json::json!({
                "queued": true,
                "revivalStarted": should_spawn,
            }));
        }
        if let Some(result) =
            dispatch_connector_auth_gateway(&self.telemetry_logs, method, &args)
        {
            return result.map_err(|error| match error {
                ConnectorAuthGatewayError::BadRequest(message) => {
                    GatewayCommandError::BadRequest(message)
                }
                ConnectorAuthGatewayError::Internal(message) => {
                    GatewayCommandError::Internal(message)
                }
            });
        }
        if let Some(result) =
            dispatch_mcp_discovery_failed_gateway(&self.telemetry_logs, method, &args)
        {
            return result.map_err(|error| match error {
                McpDiscoveryFailedGatewayError::BadRequest(message) => {
                    GatewayCommandError::BadRequest(message)
                }
                McpDiscoveryFailedGatewayError::Internal(message) => {
                    GatewayCommandError::Internal(message)
                }
            });
        }
        if method == "loadBoxMcpServers" {
            let config_json = args
                .get("configJson")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "loadBoxMcpServers requires configJson".into(),
                ))?;
            let server_identifiers = self
                .forever_box
                .load_mcp_servers(config_json)
                .map_err(|error| GatewayCommandError::Internal(error.to_string()))?;
            return Ok(serde_json::json!({ "serverIdentifiers": server_identifiers }));
        }
        if matches!(method, "listBoxMcpToolsRaw" | "executeBoxMcpToolRaw") {
            let payload_hex = args
                .get("payloadHex")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    format!("{method} requires payloadHex"),
                ))?;
            let field_number = if method == "listBoxMcpToolsRaw" {
                MCP_STATE_EXEC_FIELD_NUMBER
            } else {
                MCP_TOOL_EXEC_FIELD_NUMBER
            };
            let mut accessor = self
                .forever_box
                .mcp_resource_accessor()
                .map_err(|error| GatewayCommandError::Internal(error.to_string()))?;
            let response_hex = execute_box_mcp_raw(&mut accessor, field_number, payload_hex)
                .map_err(GatewayCommandError::Internal)?;
            return Ok(serde_json::json!({ "payloadHex": response_hex }));
        }

        if matches!(
            method,
            "getTeachRecordingStatus" | "startTeachRecording" | "stopTeachRecording"
        ) {
            let api = self
                .teach_recording
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .ok_or_else(|| GatewayCommandError::Internal(
                    "teach-recording production extension is not initialized".into(),
                ))?;
            let status = match method {
                "getTeachRecordingStatus" => api.get_status(),
                "startTeachRecording" => {
                    let agent_id = args.get("agentId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| GatewayCommandError::BadRequest(
                            "startTeachRecording requires agentId".into(),
                        ))?;
                    let entry_point = args.get("entryPoint")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    api.start(agent_id, entry_point)
                        .map_err(|error| GatewayCommandError::BadRequest(error.to_string()))?
                }
                "stopTeachRecording" => {
                    let agent_id = args.get("agentId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| GatewayCommandError::BadRequest(
                            "stopTeachRecording requires agentId".into(),
                        ))?;
                    let save = args.get("save")
                        .and_then(serde_json::Value::as_bool)
                        .ok_or_else(|| GatewayCommandError::BadRequest(
                            "stopTeachRecording requires boolean save".into(),
                        ))?;
                    api.stop(agent_id, save)
                        .map_err(|error| GatewayCommandError::BadRequest(error.to_string()))?
                }
                _ => invariant_failure(),
            };
            return Ok(teach_recording_status_value(status));
        }

        if method == "getListenerIntegrations" {
            let lifecycle = self
                .automations_lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .upgrade()
                .ok_or_else(|| GatewayCommandError::Internal(
                    "production Automations lifecycle is not initialized".into(),
                ))?;
            let definitions = self
                .transcript_manager
                .automation_runtime()
                .list_all_automation_definitions()
                .map_err(GatewayCommandError::Internal)?;
            let counts = count_listener_platforms(
                definitions
                    .iter()
                    .map(|entry| (entry.automation.is_enabled, &entry.automation.trigger)),
            );
            let mut integrations = Vec::with_capacity(2);
            for platform in ["slack", "github"] {
                let is_connected = listener_connection_state_or_disconnected(
                    platform,
                    lifecycle.is_platform_connected(platform),
                );
                let status = lifecycle.listener_source_status(platform);
                let mut integration = serde_json::Map::new();
                integration.insert("platform".into(), serde_json::Value::String(platform.into()));
                integration.insert("isConnected".into(), serde_json::Value::Bool(is_connected));
                integration.insert(
                    "state".into(),
                    serde_json::Value::String(
                        status
                            .as_ref()
                            .map(|status| status.state.as_str())
                            .unwrap_or("idle")
                            .to_string(),
                    ),
                );
                integration.insert(
                    "neededByCount".into(),
                    serde_json::Value::Number(
                        counts.get(platform).copied().unwrap_or_default().into(),
                    ),
                );
                if let Some(detail) = status.as_ref().and_then(|status| status.detail.as_ref()) {
                    integration.insert("detail".into(), serde_json::Value::String(detail.clone()));
                }
                if let Some(status) = status.as_ref().filter(|status| !status.scope_issues.is_empty()) {
                    integration.insert(
                        "scopeIssues".into(),
                        serde_json::to_value(&status.scope_issues)
                            .map_err(|error| GatewayCommandError::Internal(error.to_string()))?,
                    );
                }
                integrations.push(serde_json::Value::Object(integration));
            }
            return Ok(serde_json::json!({ "integrations": integrations }));
        }
        if method == "getListenerConnectUrl" {
            let platform = args
                .get("platform")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|platform| matches!(*platform, "github" | "slack"))
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "getListenerConnectUrl requires platform github or slack".into(),
                ))?;
            let lifecycle = self
                .automations_lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .upgrade()
                .ok_or_else(|| GatewayCommandError::Internal(
                    "production Automations lifecycle is not initialized".into(),
                ))?;
            return Ok(serde_json::json!({
                "url": lifecycle.get_connect_url(platform),
            }));
        }

        if method == "kickstartAgent" || method == "requestDiskSaverAudit" {
            let agent_id = args
                .get("id")
                .or_else(|| args.get("agentId"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(format!(
                    "{method} requires id"
                )))?;
            let runtime = ProductionCreatedAgentKickstartRuntime {
                deps: self.local_routed_runner_deps(),
                roster: Arc::clone(&self.roster_emit),
            };
            let in_flight = if method == "kickstartAgent" {
                run_created_agent_kickstart(
                    self.session_workers.as_ref(),
                    &runtime,
                    agent_id,
                )
            } else {
                request_disk_saver_audit(
                    self.session_workers.as_ref(),
                    &runtime,
                    agent_id,
                )
            }
            .map_err(GatewayCommandError::Internal)?;
            return Ok(if method == "kickstartAgent" {
                serde_json::json!({ "isIntroductionInFlight": in_flight })
            } else {
                serde_json::json!({ "isAuditInFlight": in_flight })
            });
        }

        if method == "createAgent" {
            let projected = sanitize_create_agent_args(&args);

            let nonce = projected
                .get("clientNonce")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            if let Some(nonce) = nonce {
                let mut ledger = self
                    .create_agent_nonces
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(cached) = ledger.get(&nonce).cloned() {
                    return Ok(cached);
                }
                let minted = self
                    .transcript_manager
                    .dispatch_agent_lifecycle_gateway_call(
                        &self.agent_deletion_runtime,
                        Some(self.created_agent_kickstart_hook()),
                        method,
                        &projected,
                    )
                .ok_or_else(|| GatewayCommandError::UnknownMethod(method.to_string()))?
                .map_err(|error| match error {
                    AgentLifecycleGatewayError::BadRequest(message) => {
                        GatewayCommandError::BadRequest(message)
                    }
                    AgentLifecycleGatewayError::Internal(message) => {
                        GatewayCommandError::Internal(message)
                    }
                })?;
                ledger.insert(nonce, minted.clone());
                return Ok(minted);
            }
            return self
                .transcript_manager
                .dispatch_agent_lifecycle_gateway_call(
                    &self.agent_deletion_runtime,
                    Some(self.created_agent_kickstart_hook()),
                    method,
                    &projected,
                )
            .ok_or_else(|| GatewayCommandError::UnknownMethod(method.to_string()))?
            .map_err(|error| match error {
                AgentLifecycleGatewayError::BadRequest(message) => {
                    GatewayCommandError::BadRequest(message)
                }
                AgentLifecycleGatewayError::Internal(message) => {
                    GatewayCommandError::Internal(message)
                }
            });
        }

        if let Some(result) = dispatch_secrets_gateway_call(&self.secrets, method, &args) {
            return result.map_err(map_secrets_gateway_error);
        }
        if let Some(result) = self.cross_user.call_gateway(method, &args) {
            return result.map_err(|error| match error {
                CrossUserGatewayError::BadRequest(message) => {
                    GatewayCommandError::BadRequest(message)
                }
                CrossUserGatewayError::Internal(message) => {
                    GatewayCommandError::Internal(message)
                }
            });
        }
        if method == "isAgentNetworkEnabled" {
            return Ok(serde_json::Value::Bool(
                self.experiments.is_agent_network_enabled(),
            ));
        }
        if method == "runAgentAutomationNow" {
            let agent_id = args
                .get("id")
                .or_else(|| args.get("agentId"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "runAgentAutomationNow requires id".into(),
                ))?
                .to_string();
            let automation_id = args
                .get("automationId")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "runAgentAutomationNow requires automationId".into(),
                ))?
                .to_string();
            let runtime = self.transcript_manager.automation_runtime();
            let automation = runtime
                .get_agent_automations(&agent_id)
                .map_err(GatewayCommandError::Internal)?
                .into_iter()
                .find(|automation| automation.id == automation_id);
            let Some(automation) = automation else {
                return Ok(serde_json::Value::Null);
            };
            let Some(provider) =
                configured_routed_provider(&self.data_dir.join("settings.json"))
            else {
                return call_host_lane(&self.host_tx, method, args);
            };
            if provider == RoutedProvider::Cursor {
                return call_host_lane(&self.host_tx, method, args);
            }
            let deps = self.local_routed_runner_deps();
            runtime
                .run_agent_automation_now_with(
                    &agent_id,
                    &automation_id,
                    |prompt| {
                        run_local_automation_turn(
                            deps,
                            provider,
                            &agent_id,
                            &automation_id,
                            &automation.name,
                            prompt,
                        )
                    },
                )
                .map_err(GatewayCommandError::Internal)?;
            return Ok(serde_json::Value::Null);
        }
        if method == "runAgentWorkflowNow" {
            let plan = self
                .transcript_manager
                .workflow_commands()
                .prepare_run_now(&args)
                .map_err(map_workflow_command_error)?;
            match plan {
                None => return Ok(serde_json::Value::Null),
                Some(WorkflowRunNowPlan::Automation {
                    agent_id,
                    automation_id,
                    automation_name,
                }) => {
                    let Some(provider) =
                        configured_routed_provider(&self.data_dir.join("settings.json"))
                    else {
                        return call_host_lane(&self.host_tx, method, args);
                    };
                    if provider == RoutedProvider::Cursor {
                        return call_host_lane(&self.host_tx, method, args);
                    }
                    let runtime = self.transcript_manager.automation_runtime();
                    let deps = self.local_routed_runner_deps();
                    runtime
                        .run_agent_automation_now_with(
                            &agent_id,
                            &automation_id,
                            |prompt| {
                                run_local_automation_turn(
                                    deps,
                                    provider,
                                    &agent_id,
                                    &automation_id,
                                    &automation_name,
                                    prompt,
                                )
                            },
                        )
                        .map_err(GatewayCommandError::Internal)?;
                    return Ok(serde_json::Value::Null);
                }
                Some(WorkflowRunNowPlan::Reference {
                    agent_id,
                    visible_prompt,
                    rich_text,
                    ..
                }) => {
                    return self.call_send_prompt(
                        serde_json::json!({
                            "agentId": agent_id,
                            "prompt": visible_prompt,
                            "richText": rich_text,
                            "awaitTurn": true,
                            "source": "workflow-reference"
                        }),
                        None,
                    );
                }
            }
        }
        let automation_runtime = self.transcript_manager.automation_runtime();
        if let Some(result) = dispatch_automation_command(
            automation_runtime.as_ref(),
            method,
            &args,
        ) {
            let result = result.map_err(map_automation_command_error);
            if result.is_ok() && method != "getAgentAutomations" {
                self.refresh_production_automations();
            }
            return result;
        }
        if let Some(result) = self
            .transcript_manager
            .workflow_commands()
            .dispatch(method, &args)
        {
            return result.map_err(map_workflow_command_error);
        }
        if method == "promptAcceptanceStatus" {
            return self
                .transcript_manager
                .prompt_acceptance_status(&args)
                .map_err(map_production_send_error);
        }
        if method == "setWindowFocused" {
            let is_focused = args
                .get("isFocused")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "setWindowFocused requires isFocused".into()
                ))?;
            self.transcript_manager
                .set_window_focused(is_focused, started_at_ms() as f64)
                .map_err(GatewayCommandError::Internal)?;
            return Ok(serde_json::Value::Null);
        }
        if matches!(method, "openAgent" | "openAgentWindowed" | "openAgentTail") {
            let agent_id = args
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(format!(
                    "{method} requires id"
                )))?;
            let previous_active_agent_id = self.transcript_manager.active_agent_id();
            let was_active = previous_active_agent_id.as_deref() == Some(agent_id);
            let operation_started = std::time::Instant::now();
            let now_ms = started_at_ms() as f64;
            let (response, entry_count) = match method {
                "openAgent" => {
                    let entries = self
                        .transcript_manager
                        .switch_agent(agent_id, now_ms)
                        .map_err(GatewayCommandError::Internal)?;
                    let entry_count = entries.len();
                    (serde_json::Value::Array(entries), entry_count)
                }
                "openAgentWindowed" => {
                    let query = mahayana_host_runtime::extensions::session::agent_db_transcript_pages::TranscriptWindowQuery {
                        before_seq: args.get("beforeSeq").and_then(serde_json::Value::as_i64),
                        limit: args.get("limit").and_then(serde_json::Value::as_i64).unwrap_or(500),
                    };
                    let window = self
                        .transcript_manager
                        .open_agent_windowed(agent_id, query, now_ms)
                        .map_err(GatewayCommandError::Internal)?;
                    let entry_count = window.entries.len();
                    (
                        serde_json::json!({
                            "entries": window.entries,
                            "nextBeforeSeq": window.next_before_seq,
                            "threadCounts": window.thread_counts,
                        }),
                        entry_count,
                    )
                }
                "openAgentTail" => {
                    let query = mahayana_host_runtime::extensions::session::agent_db_transcript_pages::TranscriptWindowQuery {
                        before_seq: args.get("beforeSeq").and_then(serde_json::Value::as_i64),
                        limit: args.get("limit").and_then(serde_json::Value::as_i64).unwrap_or(500),
                    };
                    let page = self
                        .transcript_manager
                        .open_agent_tail(agent_id, query, now_ms)
                        .map_err(GatewayCommandError::Internal)?;
                    let entry_count = page.entries.len();
                    (
                        serde_json::json!({
                            "entries": page.entries,
                            "nextBeforeSeq": page.next_before_seq,
                        }),
                        entry_count,
                    )
                }
                _ => invariant_failure(),
            };
            let active_after = self.transcript_manager.active_agent_id();
            if !was_active && active_after.as_deref() == Some(agent_id) {
                if method == "openAgent"
                    && let Some(entries) = response.as_array()
                {
                    self.roster_emit
                        .publish_transcript_snapshot(agent_id, entries);
                }
                self.emit_session_activation_roster_updates(
                    agent_id,
                    previous_active_agent_id.as_deref(),
                );
            } else if !was_active && matches!(method, "openAgentWindowed" | "openAgentTail") {
                let shipped_through_id = response
                    .get("entries")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|entries| entries.last())
                    .and_then(|entry| entry.get("id"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned);
                self.schedule_windowed_session_activation(
                    agent_id.to_string(),
                    shipped_through_id,
                );
            }
            if let Err(error) = self.telemetry_logs.report_agent_open(&AgentOpenReport {
                conversation_id: agent_id.to_string(),
                duration_ms: operation_started.elapsed().as_millis() as u64,
                entry_count,
                was_active,
            }) {
                eprintln!("mahayana-host agent_open_telemetry_failed error={error}");
            }
            return Ok(response);
        }
        if method == "getAsyncTasks" {
            let agent_id = args
                .get("id")
                .or_else(|| args.get("agentId"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "getAsyncTasks requires id".into()
                ))?;
            return serde_json::to_value(
                self.transcript_manager.get_async_tasks(agent_id, &[])
            )
            .map_err(|error| GatewayCommandError::Internal(error.to_string()));
        }
        if let Some(result) = self
            .transcript_manager
            .dispatch_memory_gateway_call(method, &args)
        {
            return result.map_err(GatewayCommandError::Internal);
        }
        if let Some(result) = self
            .transcript_manager
            .dispatch_agent_lifecycle_gateway_call(
                &self.agent_deletion_runtime,
                None,
                method,
                &args,
            )
        {
            let result = result.map_err(|error| match error {
                AgentLifecycleGatewayError::BadRequest(message) => GatewayCommandError::BadRequest(message),
                AgentLifecycleGatewayError::Internal(message) => GatewayCommandError::Internal(message),
            });
            if result.is_ok() {
                match method {
                    "deleteAgent" => {
                        if let Some(agent_id) = args.get("id").and_then(serde_json::Value::as_str) {
                            self.host_runner_composition.forget_local_tool_permission(agent_id);
                            self.delete_production_automation_schedules(agent_id);
                        }
                    }
                    "deleteAgents" => {
                        if let Some(ids) = args.get("ids").and_then(serde_json::Value::as_array) {
                            for agent_id in ids.iter().filter_map(serde_json::Value::as_str) {
                                self.host_runner_composition.forget_local_tool_permission(agent_id);
                                self.delete_production_automation_schedules(agent_id);
                            }
                        }
                    }
                    _ => {}
                }
                if matches!(method, "deleteAgent" | "deleteAgents") {
                    self.refresh_production_automations();
                }
            }
            return result;
        }
        let channel_timeline_context =
            if matches!(method, "connectChannel" | "disconnectChannel") {
                args.get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .zip(
                        args.get("platform")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty()),
                    )
                    .map(|(agent_id, platform)| {
                        let previous_label = self
                            .session_workers
                            .list_agent_channels(agent_id)
                            .ok()
                            .and_then(|channels| {
                                channels
                                    .into_iter()
                                    .find(|connection| connection.platform == platform)
                                    .map(|connection| connection.label)
                            });
                        (agent_id.to_string(), platform.to_string(), previous_label)
                    })
            } else {
                None
            };
        if let Some(result) = self
            .transcript_manager
            .dispatch_session_gateway_call(method, &args)
        {
            let mut value = result.map_err(|error| match error {
                SessionGatewayError::BadRequest(message) => GatewayCommandError::BadRequest(message),
                SessionGatewayError::Internal(message) => GatewayCommandError::Internal(message),
            })?;
            if method == "listAgents" {
                self.transcript_manager.decorate_agent_summaries(&mut value);
            }
            if let Some((agent_id, platform, previous_label)) = channel_timeline_context {
                let current_label = value.as_array().and_then(|connections| {
                    connections.iter().find_map(|connection| {
                        (connection.get("platform").and_then(serde_json::Value::as_str)
                            == Some(platform.as_str()))
                        .then(|| {
                            connection
                                .get("label")
                                .and_then(serde_json::Value::as_str)
                                .map(ToOwned::to_owned)
                        })
                        .flatten()
                    })
                });
                let label = current_label
                    .or(previous_label)
                    .unwrap_or_else(|| platform.clone());
                let event_type = if method == "connectChannel" {
                    "channel-connected"
                } else {
                    "channel-disconnected"
                };
                self.roster_emit.publish_timeline_event(
                    &agent_id,
                    serde_json::json!({
                        "type": event_type,
                        "label": label,
                    }),
                );
            }
            return Ok(value);
        }
        if method == "interruptAgent" {
            let agent_id = args
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "interruptAgent requires id".into()
                ))?;
            let was_in_flight = self.transcript_runtime.is_agent_running(agent_id);
            let reason = "cancelled by user";
            let direct = self.runner_registry.cancel_agent(agent_id, reason);
            let group_member = self.runner_registry.cancel_group_member_agent(agent_id, reason);
            let cancelled = direct.saturating_add(group_member);
            if cancelled > 0 {
                if let Err(error) = self.ack_obligations.record_interrupt(agent_id, started_at_ms() as f64) {
                    eprintln!(
                        "mahayana-host user_interrupt_ack_failed agent={agent_id} error={error}"
                    );
                }
            }
            let fields = TurnInterruptFields {
                conversation_id: agent_id.to_string(),
                reason: "user-cancel".into(),
                had_active_run: cancelled > 0,
                was_in_flight,
            };
            if let Err(error) = self.telemetry_logs.report_turn_interrupt(&fields) {
                eprintln!(
                    "mahayana-host user_interrupt_telemetry_failed agent={agent_id} error={error}"
                );
            }
            return Ok(serde_json::json!({
                "agentId": agent_id,
                "interrupted": cancelled > 0,
            }));
        }

        if method == "getForeverBoxStatus" {
            let agent_id = required_box_agent_id(method, &args)?;
            let status = self.forever_box.get_status(agent_id);
            let handoff = self.transcript_manager.box_handoff(agent_id);
            return Ok(project_forever_box_status(&status, handoff.as_ref()));
        }
        if method == "cloudWorkers.status" {
            let projection = self.cloud_workers.status_snapshot();
            publish_cloud_worker_status(&self.events, projection.clone());
            return serde_json::to_value(projection)
                .map_err(|error| GatewayCommandError::Internal(format!(
                    "cloud worker status projection failed: {error}"
                )));
        }
        if method == "cloudWorkers.capabilities" {
            let mut providers = self
                .plugin_capability_catalog
                .list_capabilities_for_family(CAPABILITY_FAMILY_WORKER_PROVIDERS);
            providers.sort_by(|left, right| {
                left.provider_id
                    .cmp(&right.provider_id)
                    .then_with(|| left.plugin_id.cmp(&right.plugin_id))
            });
            return Ok(serde_json::json!({
                "owner": "cloud-workers",
                "metadataOnly": true,
                "providers": providers.into_iter().map(|provider| {
                    let mut projected = serde_json::json!({
                        "providerId": provider.provider_id,
                        "pluginId": provider.plugin_id,
                    });
                    if let Some(label) = provider.label {
                        projected["label"] = serde_json::Value::String(label);
                    }
                    projected
                }).collect::<Vec<_>>(),
            }));
        }
        if method == "cloudWorkers.doctor" {
            let request: CloudWorkerDoctorRequest = serde_json::from_value(args)
                .map_err(|error| GatewayCommandError::BadRequest(format!(
                    "cloud worker doctor request is invalid: {error}"
                )))?;
            let result = self
                .cloud_workers
                .inspect_doctor(request)
                .map_err(|error| GatewayCommandError::BadRequest(error.to_string()))?;
            publish_cloud_worker_status(&self.events, self.cloud_workers.status_snapshot());
            return serde_json::to_value(result)
                .map_err(|error| GatewayCommandError::Internal(format!(
                    "cloud worker doctor projection failed: {error}"
                )));
        }
        if method == "ensureForeverBox" {
            let agent_id = required_box_agent_id(method, &args)?;
            let status = self
                .forever_box
                .ensure(agent_id)
                .map_err(|error| GatewayCommandError::Internal(error.to_string()))?;
            let handoff = self.transcript_manager.box_handoff(agent_id);
            return Ok(project_forever_box_status(&status, handoff.as_ref()));
        }
        if method == "handBackForeverBox" {
            let agent_id = required_box_agent_id(method, &args)?;
            let trigger = args
                .get("trigger")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("button");
            let decision = self
                .transcript_manager
                .hand_back_forever_box(
                    agent_id,
                    HandoffTrigger::Name(trigger.to_string()),
                )
                .map_err(GatewayCommandError::Internal)?;
            if let HandoffDecision::End(decision) = decision {
                if let Err(error) = self.resume_with_hidden_handoff(
                    agent_id,
                    box_handoff_resume_prompt(&decision.trigger).to_string(),
                    BOX_HANDOFF_RESUME_TITLE,
                ) {
                    eprintln!(
                        "mahayana-host box_handoff_resume_failed agent={agent_id} error={error}"
                    );
                }
            }
            let status = self.forever_box.get_status(agent_id);
            let handoff = self.transcript_manager.box_handoff(agent_id);
            return Ok(project_forever_box_status(&status, handoff.as_ref()));
        }
        if method == RUNNER_ACCEPT_ROUTED_PROMPT_GATEWAY_METHOD {
            return self.call_accept_routed_prompt(args, None);
        }
        if method == RUNNER_RESOLVE_ROUTED_TOOL_GATEWAY_METHOD {
            return self
                .routed_tool_relay
                .resolve(&args)
                .map_err(|error| GatewayCommandError::BadRequest(error.to_string()));
        }
        if method == MCP_RESOLVE_LIFECYCLE_GATEWAY_METHOD {
            return self
                .mcp_lifecycle_relay
                .resolve(&args)
                .map_err(|error| GatewayCommandError::BadRequest(error.to_string()));
        }
        if method == RUNNER_START_ROUTED_PROVIDER_GATEWAY_METHOD {
            return start_routed_provider_task(
                Arc::clone(&self.routed_tool_relay),
                Arc::clone(&self.mcp_service),
                self.events.clone(),
                self.host_tx.clone(),
                self.data_dir.clone(),
                Arc::clone(&self.request_context),
                Arc::clone(&self.auth),
                Arc::clone(&self.auto_review),
                Arc::clone(&self.experiments),
                Arc::clone(&self.settings),
                Arc::clone(&self.inference),
                Arc::clone(&self.session_workers),
                Arc::clone(&self.runner_registry),
                Arc::clone(&self.ack_obligations),
                Arc::clone(&self.transcript_runtime),
                Arc::clone(&self.transcript_manager),
                Arc::clone(&self.generated_agent_runtime),
                Arc::clone(&self.completion_revivals),
                Arc::clone(&self.forever_box),
                Arc::clone(&self.local_exec),
                Arc::clone(&self.local_tool_permission),
                self.session_handoff.clone(),
                Arc::clone(&self.trays),
                self.telemetry_logs.clone(),
                self.product_analytics.clone(),
                self.production_action_auditor.clone(),
                Arc::clone(&self.cloud_agents),
                Arc::clone(&self.cloud_agent_watches),
                Arc::clone(&self.background_shell_watches),
                Arc::clone(&self.host_runner_composition),
                self.box_store_sync.clone(),
                Arc::clone(&self.automations_lifecycle),
                Arc::clone(&self.generated_subagent_cleanup),
                None,
                args,
            );
        }
        if method == "getTrays" {
            return serde_json::to_value(self.trays.list())
                .map_err(|error| GatewayCommandError::Internal(error.to_string()));
        }
        if method == "dismissTray" {
            let id = args
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "dismissTray requires id".into()
                ))?;
            return Ok(serde_json::Value::Bool(self.trays.dismiss(id)));
        }
        if method == "clearTrays" {
            self.trays.clear_all();
            return Ok(serde_json::Value::Null);
        }
        if method == "requestWebAuthnCeremony" {
            return self
                .webauthn_proxy
                .request_ceremony(args)
                .map_err(|error| GatewayCommandError::Internal(error.to_string()));
        }
        if method == RUNNER_CANCEL_ROUTED_PROVIDER_GATEWAY_METHOD {
            let stream_id = args
                .get("streamId")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| GatewayCommandError::Internal(
                    "runner.cancelRoutedProvider requires streamId".into()
                ))?;
            let reason = args
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("Runner provider request cancelled");
            let cancelled_agent_id = self.runner_registry.agent_id_for_stream(stream_id);
            let was_in_flight = cancelled_agent_id
                .as_deref()
                .is_some_and(|agent_id| self.transcript_runtime.is_agent_running(agent_id));
            if reason.to_ascii_lowercase().contains("superseded") {
                if let Some(agent_id) = cancelled_agent_id.as_deref() {
                    if self.transcript_runtime.active_turn_source(agent_id).as_deref()
                        == Some("connector")
                    {
                        self.background_wakes
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .dm_preempted_wake_agent_ids
                            .insert(agent_id.to_string());
                    }
                }
            }
            let cancelled = self.runner_registry.cancel_stream(stream_id, reason);
            if reason.to_ascii_lowercase().contains("superseded") {
                if let Some(agent_id) = cancelled_agent_id.as_deref() {
                    let fields = TurnInterruptFields {
                        conversation_id: agent_id.to_string(),
                        reason: "superseded".into(),
                        had_active_run: cancelled,
                        was_in_flight,
                    };
                    if let Err(error) = self.telemetry_logs.report_turn_interrupt(&fields) {
                        eprintln!(
                            "mahayana-host superseded_turn_interrupt_telemetry_failed agent={} error={error}",
                            agent_id
                        );
                    }
                }
            }
            if cancelled {
                self.events.publish(serde_json::json!({
                    "channel": RUNNER_INFERENCE_EVENT_CHANNEL,
                    "payload": {
                        "streamId": stream_id,
                        "type": "cancelled",
                        "message": reason
                    }
                }));
            }
            return Ok(serde_json::json!({
                "streamId": stream_id,
                "cancelled": cancelled,
            }));
        }
        if method == "acceptAgentInboundMessage" {
            let agent_id = args
                .get("agentId")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "acceptAgentInboundMessage requires agentId".into()
                ))?;
            let inbound = args
                .get("inbound")
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "acceptAgentInboundMessage requires inbound".into()
                ))?;
            let accepted = persist_agent_inbound_message(
                &self.session_workers,
                agent_id,
                inbound,
            )
            .map_err(GatewayCommandError::Internal)?;
            if accepted {
                self.roster_emit
                    .emit_agent_update(agent_id)
                    .map_err(GatewayCommandError::Internal)?;
            }
            return Ok(serde_json::json!({ "accepted": accepted }));
        }
        if method == "reportAgentInboundFailure" {
            let agent_id = args
                .get("agentId")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "reportAgentInboundFailure requires agentId".into()
                ))?;
            let error_code = args
                .get("errorCode")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("INFERENCE_PROVIDER_FAILED");
            let detail = args
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Agent inbound wake failed");
            let request_id = args
                .get("requestId")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Err(error) = self.telemetry_logs.report_agent_error(&agent_inbound_failure_report(
                agent_id,
                request_id,
                error_code,
                detail,
            )) {
                eprintln!(
                    "mahayana-host agent_inbound_failure_telemetry_failed agent={} error={error}",
                    agent_id
                );
            }
            let tray = self.trays.push_error(agent_inbound_failure_tray(
                agent_id,
                request_id,
                error_code,
                detail,
            ));
            return serde_json::to_value(tray).map_err(|error| {
                GatewayCommandError::Internal(format!(
                    "could not encode agent inbound failure tray: {error}"
                ))
            });
        }
        if method == "respondToWidget" {
            let agent_id = required_string_arg(&args, "agentId", method)?;
            let entry_id = required_string_arg(&args, "entryId", method)?;
            let value = required_string_arg(&args, "value", method)?;
            let accepted = self
                .transcript_manager
                .respond_to_widget_with(
                    entry_id,
                    value,
                    agent_id,
                    started_at_ms() as f64,
                    |send_args| {
                        self.call_send_prompt(send_args, None)
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    },
                )
                .map_err(GatewayCommandError::Internal)?;
            return Ok(serde_json::json!({ "accepted": accepted }));
        }
        if method == "dismissWidget" {
            let agent_id = required_string_arg(&args, "agentId", method)?;
            let entry_id = required_string_arg(&args, "entryId", method)?;
            let accepted = self
                .transcript_manager
                .dismiss_widget(entry_id, agent_id, started_at_ms() as f64)
                .map_err(GatewayCommandError::Internal)?;
            return Ok(serde_json::json!({ "accepted": accepted }));
        }
        if method == "submitSecret" {
            let agent_id = required_string_arg(&args, "agentId", method)?;
            let entry_id = required_string_arg(&args, "entryId", method)?;
            let value = required_string_arg(&args, "value", method)?;
            let accepted = self
                .transcript_manager
                .submit_secret_with(
                    entry_id,
                    value,
                    agent_id,
                    started_at_ms() as f64,
                    |prompt| {
                        self.resume_with_hidden_handoff(
                            agent_id,
                            prompt,
                            "Agent failed to resume after secret submission",
                        )
                    },
                )
                .map_err(GatewayCommandError::Internal)?;
            return Ok(serde_json::json!({ "accepted": accepted }));
        }
        if method == "reactToMessage" {
            let agent_id = required_string_arg(&args, "agentId", method)?;
            let entry_id = required_string_arg(&args, "entryId", method)?;
            let emoji = required_string_arg(&args, "emoji", method)?;
            let accepted = self
                .transcript_manager
                .react_to_message_with(
                    entry_id,
                    emoji,
                    agent_id,
                    started_at_ms() as f64,
                    |prompt| {
                        self.resume_with_hidden_handoff(
                            agent_id,
                            prompt,
                            "Agent failed to resume after reaction",
                        )
                    },
                )
                .map_err(GatewayCommandError::Internal)?;
            return Ok(serde_json::json!({ "accepted": accepted }));
        }

        // UnifiedAppHost owns a QuickJS runtime and is intentionally !Send.
        // Product calls remain on its owner lane while the Runner provider
        // worker above streams through the Host event hub.
        if method == "sendPrompt" {
            return self.call_send_prompt(args, None);
        }
        if method == "resolveAutoReviewApproval" {
            let agent_id = args
                .get("agentId")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "resolveAutoReviewApproval requires agentId".into(),
                ))?;
            let entry_id = args
                .get("entryId")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "resolveAutoReviewApproval requires entryId".into(),
                ))?;
            let request_id = args
                .get("requestId")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "resolveAutoReviewApproval requires requestId".into(),
                ))?;
            let resolution = match args
                .get("resolution")
                .and_then(serde_json::Value::as_str)
            {
                Some("approved") => SandAutoReviewResolution::Approved,
                Some("denied") => SandAutoReviewResolution::Denied,
                _ => {
                    return Err(GatewayCommandError::BadRequest(
                        "resolveAutoReviewApproval requires approved or denied resolution".into(),
                    ));
                }
            };
            self.auto_review
                .service()
                .resolve_approval_for_entry(
                    request_id,
                    resolution,
                    agent_id,
                    entry_id,
                )
                .map_err(|error| {
                    if error.contains("SAND_AUTO_REVIEW_STALE") {
                        GatewayCommandError::BadRequest(error)
                    } else {
                        GatewayCommandError::Internal(error)
                    }
                })?;
            return Ok(serde_json::Value::Null);
        }
        if method == "resolveLocalToolPermission" {
            let agent_id = args
                .get("agentId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "resolveLocalToolPermission requires agentId".into(),
                ))?;
            let entry_id = args
                .get("entryId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "resolveLocalToolPermission requires entryId".into(),
                ))?;
            let request_id = args
                .get("requestId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "resolveLocalToolPermission requires requestId".into(),
                ))?;
            let resolution = args
                .get("resolution")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| GatewayCommandError::BadRequest(
                    "resolveLocalToolPermission requires resolution".into(),
                ))?;
            self.local_tool_permission
                .resolve_ask(&LocalToolPermissionResolutionArgs {
                    agent_id: agent_id.to_string(),
                    entry_id: entry_id.to_string(),
                    request_id: request_id.to_string(),
                    resolution: resolution.to_string(),
                })
                .map_err(map_local_tool_permission_resolution_error)?;
            return Ok(serde_json::Value::Null);
        }
        if method == "getHostSettings" {
            let settings = self.settings.get_host_settings();
            return Ok(settings);
        }
        if method == "setHostSettings" {
            let permission_changed = args.get("localToolPermission").is_some();
            let result = self.settings.set_host_settings(&args)
                .map_err(|error| GatewayCommandError::Internal(format!("Failed to persist settings: {error}")))?;
            report_agent_roster_security_findings(&result);
            self.roster_bookkeeping
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .apply_config(&result);
            if permission_changed {
                self.local_tool_permission.note_permission_changed();
            }
            return Ok(result);
        }
        call_host_lane(&self.host_tx, method, args)
    }

    fn health(&self) -> GatewayHealth {
        let running_agent_ids = self.transcript_manager.live_running_agent_ids();
        let awaiting_approval_agent_ids =
            self.auto_review.service().agent_ids_with_pending_approvals();
        let active_agent_id = self.transcript_manager.active_agent_id();
        let has_running_subagents = self
            .generated_agent_runtime
            .lock()
            .map(|runtime| runtime.has_running_subagents())
            .unwrap_or(true);
        let has_mid_drain_revival = self.completion_revivals.has_mid_drain_revival();
        let has_running_background_shell =
            self.background_shell_watches.has_running_background_shell_work();
        let has_other_background_work =
            self.transcript_manager.has_carryable_pending_wake()
                || has_running_subagents
                || has_running_background_shell
                || has_mid_drain_revival;
        let mut last_busy_at_ms = self
            .last_busy_at_ms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let health = compute_host_health(
            running_agent_ids,
            awaiting_approval_agent_ids,
            has_other_background_work,
            active_agent_id,
            started_at_ms(),
            *last_busy_at_ms,
        );
        *last_busy_at_ms = health.last_busy_at_ms;
        GatewayHealth {
            is_busy: health.is_busy,
            busy_only_awaiting_approval: Some(health.busy_only_awaiting_approval),
            active_agent_id: health.active_agent_id,
            last_busy_at_ms: Some(health.last_busy_at_ms),
        }
    }

    fn prepare_for_upgrade(&self) -> Result<serde_json::Value, GatewayCommandError> {
        self.host_upgrade
            .service()
            .prepare_for_upgrade()
            .map_err(GatewayCommandError::Internal)?;
        let mut running_agent_ids = self
            .transcript_manager
            .live_running_agent_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        running_agent_ids.extend(self.completion_revivals.mid_drain_revival_agent_ids());
        let running_turns = running_agent_ids.len()
            + usize::from(self.background_shell_watches.has_running_background_shell_work());
        Ok(serde_json::json!({
            "quiescing": self.transcript_manager.is_quiescing_for_upgrade(),
            "runningTurns": running_turns,
            "resumeAgentIds": self.transcript_runtime.upgrade_resume_agent_ids(),
            "resumePendingWakes": self.transcript_manager.recreate_carry_pending_wakes(),
        }))
    }

    fn call_with_context(
        &self,
        method: &str,
        args: serde_json::Value,
        context: &GatewayCommandContext,
    ) -> Result<serde_json::Value, GatewayCommandError> {
        if method == "sessions.providerReview.continue" {
            return call_openclaw_sessions_provider_review_continue(self, &args, context);
        }
        if method == "sessions.patch" {
            return call_openclaw_sessions_patch(self, &args, context);
        }
        if method == RUNNER_ACCEPT_ROUTED_PROMPT_GATEWAY_METHOD {
            return self.call_accept_routed_prompt(args, Some(context));
        }
        if method == "sendPrompt" {
            return self.call_send_prompt(args, Some(context));
        }
        if method == RUNNER_START_ROUTED_PROVIDER_GATEWAY_METHOD {
            return start_routed_provider_task(
                Arc::clone(&self.routed_tool_relay),
                Arc::clone(&self.mcp_service),
                self.events.clone(),
                self.host_tx.clone(),
                self.data_dir.clone(),
                Arc::clone(&self.request_context),
                Arc::clone(&self.auth),
                Arc::clone(&self.auto_review),
                Arc::clone(&self.experiments),
                Arc::clone(&self.settings),
                Arc::clone(&self.inference),
                Arc::clone(&self.session_workers),
                Arc::clone(&self.runner_registry),
                Arc::clone(&self.ack_obligations),
                Arc::clone(&self.transcript_runtime),
                Arc::clone(&self.transcript_manager),
                Arc::clone(&self.generated_agent_runtime),
                Arc::clone(&self.completion_revivals),
                Arc::clone(&self.forever_box),
                Arc::clone(&self.local_exec),
                Arc::clone(&self.local_tool_permission),
                self.session_handoff.clone(),
                Arc::clone(&self.trays),
                self.telemetry_logs.clone(),
                self.product_analytics.clone(),
                self.production_action_auditor.clone(),
                Arc::clone(&self.cloud_agents),
                Arc::clone(&self.cloud_agent_watches),
                Arc::clone(&self.background_shell_watches),
                Arc::clone(&self.host_runner_composition),
                self.box_store_sync.clone(),
                Arc::clone(&self.automations_lifecycle),
                Arc::clone(&self.generated_subagent_cleanup),
                Some(context.clone()),
                args,
            );
        }
        self.call(method, args)
    }

    fn on_command_complete(&self, report: GatewayCommandReport) {
        log_gateway_command_report("complete", &report);
        if let Err(error) = self.telemetry_logs.report_gateway_command_timing(&report) {
            eprintln!(
                "mahayana-host gateway_command_timing_telemetry_failed method={} error={error}",
                report.method
            );
        }
    }

    fn on_command_error(&self, report: GatewayCommandReport) {
        log_gateway_command_report("error", &report);
        if let Err(error) = self.telemetry_logs.report_gateway_command_error(&report) {
            eprintln!(
                "mahayana-host gateway_command_error_telemetry_failed method={} error={error}",
                report.method
            );
        }
    }
}

fn record_send_acceptance_tracing(
    send_trace: Option<&HostTrace>,
    agent_id: &str,
    client_nonce: Option<&str>,
    persisted: &PersistedSendContext,
    durable_timing: Option<(f64, f64)>,
    host_receipt_epoch_ms: f64,
    ack_emit_host_ms: f64,
) {
    if !persisted.acceptance_effects_applied {
        return;
    }
    let nonce_attributes = client_nonce
        .filter(|value| !value.is_empty())
        .map(|value| ("sand.client_nonce".to_string(), serde_json::Value::String(value.to_string())));

    if let Some((start_epoch_ms, duration_ms)) = durable_timing {
        let mut attributes = BTreeMap::from([
            (
                "sand.durable_append_ms".to_string(),
                serde_json::json!(duration_ms.max(0.0).round()),
            ),
            (
                "sand.durable".to_string(),
                serde_json::Value::Bool(persisted.accepted_durably),
            ),
            (
                "sand.conversation_id".to_string(),
                serde_json::Value::String(agent_id.to_string()),
            ),
        ]);
        if let Some((key, value)) = nonce_attributes.clone() {
            attributes.insert(key, value);
        }
        record_completed_trace_span(
            send_trace,
            "durable-append",
            start_epoch_ms,
            start_epoch_ms + duration_ms.max(0.0),
            &attributes,
        );
    }

    let mut ack_attributes = BTreeMap::from([
        (
            "sand.ack_emit_host_ms".to_string(),
            serde_json::json!(ack_emit_host_ms.max(0.0).round()),
        ),
        (
            "sand.conversation_id".to_string(),
            serde_json::Value::String(agent_id.to_string()),
        ),
    ]);
    if let Some((key, value)) = nonce_attributes {
        ack_attributes.insert(key, value);
    }
    record_completed_trace_span(
        send_trace,
        "send-ack-emit",
        host_receipt_epoch_ms,
        host_receipt_epoch_ms + ack_emit_host_ms.max(0.0),
        &ack_attributes,
    );
    if let Some(send_trace) = send_trace {
        send_trace.span.set_attribute(
            "sand.ack_emit_host_ms",
            serde_json::json!(ack_emit_host_ms.max(0.0).round()),
        );
    }
}

fn map_secrets_gateway_error(error: SecretsGatewayError) -> GatewayCommandError {
    match error {
        SecretsGatewayError::BadRequest(message) => GatewayCommandError::BadRequest(message),
        SecretsGatewayError::Internal(message) => GatewayCommandError::Internal(message),
    }
}

fn map_automation_command_error(error: AutomationCommandError) -> GatewayCommandError {
    match error {
        AutomationCommandError::BadRequest(message) => GatewayCommandError::BadRequest(message),
        AutomationCommandError::Internal(message) => GatewayCommandError::Internal(message),
    }
}

fn map_local_tool_permission_resolution_error(
    error: SandLocalToolPermissionResolutionError,
) -> GatewayCommandError {
    match error {
        SandLocalToolPermissionResolutionError::UnknownResolution
        | SandLocalToolPermissionResolutionError::Stale => {
            GatewayCommandError::BadRequest(error.to_string())
        }
        SandLocalToolPermissionResolutionError::Transcript(message) => {
            GatewayCommandError::Internal(message)
        }
    }
}

fn map_workflow_command_error(error: WorkflowCommandError) -> GatewayCommandError {
    match error {
        WorkflowCommandError::BadRequest(message) => GatewayCommandError::BadRequest(message),
        WorkflowCommandError::Internal(message) => GatewayCommandError::Internal(message),
    }
}

fn map_gateway_send_error(error: GatewayCommandError) -> ProductionSendError {
    match error {
        GatewayCommandError::BadRequest(message) => ProductionSendError::BadRequest(message),
        GatewayCommandError::Unauthorized(message) | GatewayCommandError::Forbidden(message) => {
            ProductionSendError::Rejected(message)
        }
        GatewayCommandError::Conflict(message) => ProductionSendError::Conflict(message),
        GatewayCommandError::UnknownMethod(message) | GatewayCommandError::Internal(message) => {
            ProductionSendError::Internal(message)
        }
    }
}

pub(crate) fn map_session_send_error(error: SessionGatewayError) -> ProductionSendError {
    match error {
        SessionGatewayError::BadRequest(message) => ProductionSendError::BadRequest(message),
        SessionGatewayError::Internal(message) => ProductionSendError::Internal(message),
    }
}

pub(crate) fn map_production_send_error(error: ProductionSendError) -> GatewayCommandError {
    match error {
        ProductionSendError::BadRequest(message) => GatewayCommandError::BadRequest(message),
        ProductionSendError::Conflict(message) | ProductionSendError::Rejected(message) => {
            GatewayCommandError::Conflict(message)
        }
        ProductionSendError::Internal(message) => GatewayCommandError::Internal(message),
    }
}

fn log_gateway_command_report(kind: &str, report: &GatewayCommandReport) {
    let mut value = serde_json::json!({
        "kind": kind,
        "method": report.method,
        "durationMs": report.duration_ms,
        "status": report.status,
    });
    if let Some(request_id) = report.request_id.as_deref() {
        value["requestId"] = serde_json::Value::String(request_id.to_string());
    }
    if let Some(traceparent) = report.traceparent.as_deref() {
        value["traceparent"] = serde_json::Value::String(traceparent.to_string());
    }
    if let Some(error) = report.error.as_deref() {
        value["error"] = serde_json::Value::String(error.to_string());
    }
    if let Some(reason) = report.reason.as_deref() {
        value["reason"] = serde_json::Value::String(reason.to_string());
    }
    if let Some(error_class) = report.error_class.as_deref() {
        value["errorClass"] = serde_json::Value::String(error_class.to_string());
    }
    if let Some(errno) = report.errno.as_deref() {
        value["errno"] = serde_json::Value::String(errno.to_string());
    }
    eprintln!("mahayana-host-gateway-command {value}");
}

fn decode_box_environment_update(
    args: &serde_json::Value,
) -> Result<BoxEnvironmentUpdate, GatewayCommandError> {
    let object = args.as_object().ok_or_else(|| {
        GatewayCommandError::Internal("box.applyEnvironment params must be an object".into())
    })?;
    let raw_env = object.get("env").and_then(serde_json::Value::as_object).ok_or_else(|| {
        GatewayCommandError::Internal("box.applyEnvironment params.env must be an object".into())
    })?;
    let replace = object
        .get("replace")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            GatewayCommandError::Internal("box.applyEnvironment params.replace must be a boolean".into())
        })?;
    let mut env = BTreeMap::new();
    for (name, value) in raw_env {
        let value = value.as_str().ok_or_else(|| {
            GatewayCommandError::Internal(format!(
                "box.applyEnvironment env value for {name} must be a string"
            ))
        })?;
        env.insert(name.clone(), value.to_string());
    }
    Ok(BoxEnvironmentUpdate { env, replace })
}

fn dispatch_box_environment_call<Apply>(
    method: &str,
    args: &serde_json::Value,
    mut apply: Apply,
) -> Option<Result<serde_json::Value, GatewayCommandError>>
where
    Apply: FnMut(&BoxEnvironmentUpdate) -> Result<(), String>,
{
    if method != BOX_APPLY_ENVIRONMENT_GATEWAY_METHOD {
        return None;
    }
    Some(
        decode_box_environment_update(args).and_then(|update| {
            apply(&update)
                .map_err(GatewayCommandError::Internal)
                .map(|()| serde_json::json!({ "applied": true }))
        }),
    )
}

fn dispatch_gateway_call(
    host: &UnifiedAppHost,
    forever_box: &ForeverBoxService,
    attachments: &AttachmentsService,
    method: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    if let Some(result) = dispatch_mini_app_management(host, method, &args) {
        return result;
    }
    if let Some(result) = attachments.dispatch_gateway(method, &args) {
        return result.map_err(GatewayCommandError::Internal);
    }

    match host.grok_gateway_call(method, args.clone()) {
        Ok(Some(result)) => return Ok(result),
        Ok(None) => {}
        Err(error) => {
            return Err(GatewayCommandError::Internal(format!(
                "Mahayana Host Grok compatibility dispatch failed for {method}: {error}"
            )));
        }
    }

    if let Some(result) = dispatch_box_environment_call(method, &args, |update| {
        forever_box
            .apply_environment(update)
            .map_err(|error| error.to_string())
    }) {
        return result;
    }

    let request = serde_json::json!({
        "id": "gateway",
        "method": method,
        "params": args,
    });
    let encoded = serde_json::to_string(&request)
        .map_err(|error| GatewayCommandError::Internal(error.to_string()))?;
    let response = dispatch_json(host, &encoded);
    let parsed: serde_json::Value = serde_json::from_str(&response)
        .map_err(|error| GatewayCommandError::Internal(error.to_string()))?;
    if parsed.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(parsed.get("result").cloned().unwrap_or(serde_json::Value::Null));
    }
    let message = parsed
        .get("error")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Mahayana Host gateway request failed")
        .to_string();
    if message.contains("unknown") {
        Err(GatewayCommandError::UnknownMethod(method.to_string()))
    } else {
        Err(GatewayCommandError::Internal(message))
    }
}

/// OMA-001's named MainData-only package management intents. The MainData
/// carrier is not permission to accept policy or package bytes from callers.
fn dispatch_mini_app_management(
    host: &UnifiedAppHost,
    method: &str,
    args: &serde_json::Value,
) -> Option<Result<serde_json::Value, GatewayCommandError>> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use mahayana_host_runtime::extensions::mcp::installable_package::{
        ledger, policy, verify_catalog_bundle, NativeMiniAppEntry,
    };

    let allowed_fields: &[&str] = match method {
        "listApprovedMiniApps" | "listInstalledMiniApps" => &[],
        "installApprovedMiniApp" | "uninstallMiniApp" => &["appId", "operationId"],
        "getInstalledMiniAppResources" => &["appId", "operationId"],
        _ => return None,
    };
    let respond = || -> Result<serde_json::Value, GatewayCommandError> {
        let object = args.as_object().ok_or_else(|| {
            GatewayCommandError::BadRequest("mini-app intent must be an object".into())
        })?;
        if object.keys().any(|key| !allowed_fields.contains(&key.as_str())) {
            return Err(GatewayCommandError::BadRequest(
                "mini-app intent contains unsupported fields".into(),
            ));
        }
        match method {
            "listApprovedMiniApps" => list_approved_mini_apps(host),
            "installApprovedMiniApp" => {
                mini_app_identifier_arg(object, "appId")?;
                let operation_id = mini_app_identifier_arg(object, "operationId")?;
                if operation_id.len() < 16 {
                    return Err(GatewayCommandError::BadRequest(
                        "mini-app operationId must be a native-generated random identifier".into(),
                    ));
                }
                install_approved_mini_app(host, mini_app_identifier_arg(object, "appId")?, operation_id)
            }
            "getInstalledMiniAppResources" => {
                let app_id = mini_app_identifier_arg(object, "appId")?;
                let operation_id = mini_app_identifier_arg(object, "operationId")?;
                if operation_id.len() < 16 {
                    return Err(GatewayCommandError::BadRequest(
                        "mini-app operationId must be a native-generated random identifier".into(),
                    ));
                }
                let (install_scope, claim) = current_native_mini_app_scope(host)?;
                let store = host.mini_app_store_port().map_err(|_| {
                    GatewayCommandError::Internal("not_configured: RuntimeStore SQLite connection is unavailable".into())
                })?;
                let client = policy::CatalogClient::production_from_environment().map_err(proof_gateway_error)?;
                // Commit a valid signed feed before looking up the exact release.
                // This ordering preserves a key revocation even if proof lookup is 404.
                let latest_trust = refresh_mini_app_trust(&client, &store, true)?;
                let loaded = store.with_connection(|connection| {
                    ledger::initialize(connection)?;
                    ledger::load_active_authorized_archive(connection, &install_scope, app_id)
                }).map_err(|_| GatewayCommandError::Internal("storage: installed mini-app could not be loaded".into()))?;
                let Some((archive, proof_snapshot, cached_trust)) = loaded else {
                    return Err(GatewayCommandError::Internal("resource_not_found: mini-app is not installed".into()));
                };
                let persisted_quarantine = store.with_connection(|connection| ledger::quarantine_reason(connection, &install_scope, app_id))
                    .map_err(|_| GatewayCommandError::Internal("storage: mini-app quarantine state could not be read".into()))?;
                if persisted_quarantine.as_deref() == Some("release_revoked") {
                    return Err(GatewayCommandError::Internal("release_revoked".into()));
                }
                let latest_trust = latest_trust.or(cached_trust);
                let effective_snapshot = policy::overlay_cached_trust(&proof_snapshot, latest_trust.as_deref()).map_err(proof_gateway_error)?;
                let verified = match policy::verify_install_proof(&effective_snapshot, app_id, policy::now_ms(), true) {
                    Ok(proof) => proof,
                    Err(error) => {
                        let _ = store.with_connection(|connection| ledger::set_quarantined(connection, &install_scope, app_id, Some(&format!("{error:?}"))));
                        return Err(proof_gateway_error(error));
                    }
                };
                if let Some(trust) = latest_trust.as_deref() {
                    let descriptor = policy::proof_descriptor(&proof_snapshot).map_err(proof_gateway_error)?;
                    match client.fetch_install_proof(&descriptor) {
                        Ok(raw) => {
                            let current_proof = policy::overlay_trust(&raw, trust).map_err(proof_gateway_error)?;
                            let current_authorization = match policy::verify_install_proof(&current_proof, app_id, policy::now_ms(), true) {
                                Ok(proof) => proof,
                                Err(error) => {
                                    let reason = format!("{error:?}");
                                    let _ = store.with_connection(|connection| ledger::set_quarantined(connection, &install_scope, app_id, Some(&reason)));
                                    return Err(proof_gateway_error(error));
                                }
                            };
                            if current_authorization.authorization.version != verified.authorization.version
                                || current_authorization.authorization.package_sha256 != verified.authorization.package_sha256 {
                                let _ = store.with_connection(|connection| ledger::set_quarantined(connection, &install_scope, app_id, Some("release_binding_changed")));
                                return Err(GatewayCommandError::Internal("release_revoked".into()));
                            }
                        }
                        Err(policy::ProofError::NotFound) => {
                            let _ = store.with_connection(|connection| ledger::set_quarantined(connection, &install_scope, app_id, Some("release_revoked")));
                            return Err(GatewayCommandError::Internal("release_revoked".into()));
                        }
                        Err(policy::ProofError::Network) => { /* verified last-good feed + archive below */ }
                        Err(error) => return Err(proof_gateway_error(error)),
                    }
                }
                let package = match verify_catalog_bundle(&archive, &verified.authorization) {
                    Ok(package) => package,
                    Err(_) => {
                        let _ = store.with_connection(|connection| ledger::set_quarantined(connection, &install_scope, app_id, Some("package_verification")));
                        return Err(GatewayCommandError::Internal("digest_mismatch: installed package failed verification".into()));
                    }
                };
                if !native_mini_app_claim_is_current(&claim) {
                    return Err(GatewayCommandError::Internal("stale_account: account changed during resource verification".into()));
                }
                let resources = package.resource_entries().map(|(path, bytes, mime)| serde_json::json!({
                    "path": path, "mime": mime, "bytesBase64": STANDARD.encode(bytes)
                })).collect::<Vec<_>>();
                let _ = operation_id;
                Ok(serde_json::json!({"sameScopeHash": install_scope.account_scope,
                    "descriptor": {
                    "appId": package.app_id(), "version": package.version(),
                    "digest": package.digest(), "entrypoint": package.entrypoint(),
                    "nativeRuntime": package.native_runtime()
                }, "resources": resources}))
            }
            "listInstalledMiniApps" | "uninstallMiniApp" => {
                let app_id = if method == "uninstallMiniApp" {
                    Some(mini_app_identifier_arg(object, "appId")?)
                } else {
                    None
                };
                let operation_id = if method == "uninstallMiniApp" {
                    let value = mini_app_identifier_arg(object, "operationId")?;
                    Some(value)
                } else {
                    None
                };
                let (install_scope, claim) = current_native_mini_app_scope(host)?;
                let store = host.mini_app_store_port().map_err(|_| {
                    GatewayCommandError::Internal(
                        "not_configured: RuntimeStore SQLite connection is unavailable".into(),
                    )
                })?;
                if let Some(app_id) = app_id {
                    let operation_id = operation_id.expect("validated uninstall operation ID");
                    store
                        .with_connection(|connection| {
                            ledger::initialize(connection)
                                .and_then(|()| ledger::remove_fenced(connection, &install_scope, &install_scope, app_id, operation_id, || {
                                    native_mini_app_claim_is_current(&claim)
                                }))
                        })
                        .map(|removed| serde_json::json!({"appId": app_id, "removed": removed}))
                        .map_err(|_| GatewayCommandError::Internal(
                            "storage: mini-app code removal failed".into(),
                        ))
                } else {
                    let items = store
                        .with_connection(|connection| {
                            ledger::initialize(connection)
                                .and_then(|()| ledger::list_installed(connection, &install_scope))
                        })
                        .map_err(|_| GatewayCommandError::Internal(
                            "storage: mini-app install listing failed".into(),
                        ))?;
                    let entries = items.into_iter().map(|item| {
                        let validation = validate_installed_listing(
                            host, &install_scope, &item.app_id, &item.digest,
                            &item.version, &item.publisher_id, item.size_bytes,
                        );
                        NativeMiniAppEntry {
                            app_id: item.app_id,
                            display_name: validation.display_name,
                            publisher_id: item.publisher_id,
                            version: item.version,
                            package_digest: item.digest,
                            size_bytes: validation.size_bytes,
                            offline_core: true,
                            installed: true,
                            supported: validation.supported,
                            state: validation.state,
                        }
                    }).collect::<Vec<_>>();
                    let response = serde_json::json!({"items": entries});
                    if !native_mini_app_claim_is_current(&claim) {
                        return Err(GatewayCommandError::Internal("stale_account: account changed during install listing".into()));
                    }
                    Ok(response)
                }
            }
            _ => unreachable!("recognized mini-app method"),
        }
    };
    Some(respond())
}

fn current_native_mini_app_scope_from_store(
    store: &mahayana_unified_app_host::MiniAppRuntimeStorePort,
) -> Result<(
    mahayana_host_runtime::extensions::mcp::installable_package::ledger::HostInstallScope,
    mahayana_host_runtime::extensions::session::native_messaging::FabushiNativeCredentialClaim,
), GatewayCommandError> {
    use mahayana_host_runtime::extensions::{
        mcp::installable_package::ledger,
        session::native_messaging::{FabushiIdentityVerificationError, FabushiNativeMessagingClient},
    };
    let client = FabushiNativeMessagingClient::from_env().map_err(|_| {
        GatewayCommandError::Internal("not_authenticated: Native account credential is unavailable".into())
    })?;
    let claim = client.credential_claim().map_err(|_| {
        GatewayCommandError::Internal("not_authenticated: Native account credential is invalid or expired".into())
    })?;
    let scope_hash = claim.account_scope_hash();
    let verified = client.verify_account_identity();
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64).unwrap_or(0);
    let scope = match verified {
        Ok(verified_claim) if verified_claim == claim => store.with_connection(|connection| {
            ledger::initialize(connection)?;
            ledger::verified_native_account(connection, &scope_hash, &claim.session_binding_hash, claim.expires_at_ms, now_ms)
        }).map_err(|_| GatewayCommandError::Internal("storage: verified Native account scope could not be persisted".into()))?,
        Ok(_) | Err(FabushiIdentityVerificationError::Unauthenticated)
        | Err(FabushiIdentityVerificationError::Rejected)
        | Err(FabushiIdentityVerificationError::IdentityMismatch) => {
            let _ = store.with_connection(|connection| {
                ledger::initialize(connection)?;
                ledger::clear_cached_native_account(connection)
            });
            return Err(GatewayCommandError::Internal("not_authenticated: Backend did not confirm the Native account identity".into()));
        }
        Err(FabushiIdentityVerificationError::Network) => store.with_connection(|connection| {
            ledger::initialize(connection)?;
            ledger::cached_native_account(connection, &scope_hash, &claim.session_binding_hash, now_ms)
        }).map_err(|_| GatewayCommandError::Internal("storage: cached Native account scope is unavailable".into()))?
            .ok_or_else(|| GatewayCommandError::Internal("not_authenticated: no unexpired verified Native account snapshot is available offline".into()))?,
    };
    Ok((scope, claim))
}

fn current_native_mini_app_scope(
    host: &UnifiedAppHost,
) -> Result<(
    mahayana_host_runtime::extensions::mcp::installable_package::ledger::HostInstallScope,
    mahayana_host_runtime::extensions::session::native_messaging::FabushiNativeCredentialClaim,
), GatewayCommandError> {
    let store = host.mini_app_store_port().map_err(|_| {
        GatewayCommandError::Internal("not_configured: RuntimeStore SQLite connection is unavailable".into())
    })?;
    current_native_mini_app_scope_from_store(&store)
}

fn native_mini_app_claim_is_current(
    expected: &mahayana_host_runtime::extensions::session::native_messaging::FabushiNativeCredentialClaim,
) -> bool {
    use mahayana_host_runtime::extensions::session::native_messaging::FabushiNativeMessagingClient;
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64).unwrap_or(0);
    FabushiNativeMessagingClient::from_env().ok()
        .and_then(|client| client.credential_claim().ok())
        .is_some_and(|current| current == *expected && current.expires_at_ms > now_ms)
}

fn proof_gateway_error(error: mahayana_host_runtime::extensions::mcp::installable_package::policy::ProofError) -> GatewayCommandError {
    use mahayana_host_runtime::extensions::mcp::installable_package::policy::ProofError;
    let code = match error {
        ProofError::NotFound => "release_not_approved",
        ProofError::FeedFreshness => "trust_feed_expired",
        ProofError::TrustRollback => "trust_feed_rollback",
        ProofError::Revoked => "release_revoked",
        ProofError::Signature => "release_signature_invalid",
        ProofError::DescriptorBinding | ProofError::Authorization | ProofError::Malformed => "trust_feed_invalid",
        ProofError::Network => "marketplace_unavailable",
        ProofError::Rejected => "marketplace_rejected",
        ProofError::ArtifactSize => "digest_mismatch",
    };
    GatewayCommandError::Internal(code.into())
}

fn refresh_mini_app_trust(
    client: &mahayana_host_runtime::extensions::mcp::installable_package::policy::CatalogClient,
    store: &mahayana_unified_app_host::MiniAppRuntimeStorePort,
    allow_transport_fallback: bool,
) -> Result<Option<String>, GatewayCommandError> {
    use mahayana_host_runtime::extensions::mcp::installable_package::{ledger, policy};
    let cached = store.with_connection(|connection| {
        ledger::initialize(connection)?;
        ledger::current_trust_envelope(connection)
    }).map_err(|_| GatewayCommandError::Internal("storage: mini-app trust state could not be read".into()))?;
    let fetched = match client.fetch_install_trust() {
        Ok(raw) => raw,
        Err(error) if allow_transport_fallback && policy::is_transport_failure(&error) => {
            let cached = cached.ok_or_else(|| GatewayCommandError::Internal("marketplace_unavailable".into()))?;
            policy::verify_trust_feed(&cached, policy::now_ms(), true).map_err(proof_gateway_error)?;
            return Ok(Some(cached));
        }
        Err(error) => return Err(proof_gateway_error(error)),
    };
    let verified = policy::verify_trust_feed(&fetched, policy::now_ms(), false).map_err(proof_gateway_error)?;
    policy::trust_feed_not_rolled_back(&verified, cached.as_deref()).map_err(proof_gateway_error)?;
    store.with_connection(|connection| {
        ledger::cache_verified_trust(connection, verified.epoch, verified.issued_at_ms,
            &verified.signed_payload, &verified.envelope_json)
    }).map_err(|_| GatewayCommandError::Internal("storage: verified trust feed could not be persisted".into()))?;
    Ok(Some(verified.envelope_json))
}

fn list_approved_mini_apps(host: &UnifiedAppHost) -> Result<serde_json::Value, GatewayCommandError> {
    use mahayana_host_runtime::extensions::mcp::installable_package::{ledger, policy::{self, CatalogClient}};
    let (install_scope, claim) = current_native_mini_app_scope(host)?;
    let store = host.mini_app_store_port().map_err(|_| GatewayCommandError::Internal("not_configured: RuntimeStore SQLite connection is unavailable".into()))?;
    let (installed, cached_trust) = store.with_connection(|connection| {
        ledger::initialize(connection)?;
        Ok::<_, ledger::InstallError>((
            ledger::list_installed(connection, &install_scope)?,
            ledger::current_trust_envelope(connection)?,
        ))
    }).map_err(|_| GatewayCommandError::Internal("storage: mini-app install listing failed".into()))?;
    let client = CatalogClient::production_from_environment().map_err(proof_gateway_error)?;
    let latest_trust = refresh_mini_app_trust(&client, &store, true)?.or(cached_trust);
    let descriptors = client.list_descriptors().map_err(proof_gateway_error)?;
    let mut items = Vec::new();
    for entry in descriptors {
        let descriptor = entry.get("descriptor").unwrap_or(&entry);
        let Some(app_id) = policy::descriptor_app_id(descriptor) else { continue };
        let Some(version) = policy::descriptor_version(descriptor) else { continue };
        if stable_version_tuple(version).is_none() { continue; }
        let Some(publisher_id) = policy::descriptor_publisher(descriptor) else { continue };
        let raw = match client.fetch_install_proof(descriptor) {
            Ok(raw) => raw,
            Err(policy::ProofError::NotFound) => continue,
            Err(error) => return Err(proof_gateway_error(error)),
        };
        let proof_raw = latest_trust.as_deref().map_or(Ok(raw.clone()), |trust| policy::overlay_trust(&raw, trust))
            .map_err(proof_gateway_error)?;
        let proof = policy::verify_install_proof(&proof_raw, app_id, policy::now_ms(), false)
            .map_err(proof_gateway_error)?;
        if !policy::descriptor_supports_desktop(&proof.descriptor) { continue; }
        let is_installed = installed.iter().any(|row| row.app_id == app_id);
        items.push(serde_json::json!({
            "appId": app_id,
            "displayName": policy::descriptor_display_name(&proof.descriptor).unwrap_or(app_id),
            "publisherId": publisher_id,
            "version": version,
            "packageDigest": proof.artifact.digest,
            "sizeBytes": proof.artifact.size_bytes,
            "offlineCore": true,
            "installed": is_installed,
            "supported": true
        }));
    }
    if !native_mini_app_claim_is_current(&claim) {
        return Err(GatewayCommandError::Internal("stale_account: account changed during catalog request".into()));
    }
    Ok(serde_json::json!({"items": items}))
}

fn install_approved_mini_app(host: &UnifiedAppHost, app_id: &str, operation_id: &str) -> Result<serde_json::Value, GatewayCommandError> {
    use mahayana_host_runtime::extensions::mcp::installable_package::{ledger, policy::{self, CatalogClient}, verify_catalog_bundle};
    let (install_scope, claim) = current_native_mini_app_scope(host)?;
    let store = host.mini_app_store_port().map_err(|_| GatewayCommandError::Internal("not_configured: RuntimeStore SQLite connection is unavailable".into()))?;
    let cached_trust = store.with_connection(|connection| {
        ledger::initialize(connection)?;
        ledger::current_trust_envelope(connection)
    }).map_err(|_| GatewayCommandError::Internal("storage: mini-app trust state could not be read".into()))?;
    let client = CatalogClient::production_from_environment().map_err(proof_gateway_error)?;
    let latest_trust = refresh_mini_app_trust(&client, &store, false)?.or(cached_trust);
    let descriptors = client.list_descriptors().map_err(proof_gateway_error)?;
    let mut approved = Vec::new();
    for entry in descriptors {
        let descriptor = entry.get("descriptor").unwrap_or(&entry);
        if policy::descriptor_app_id(descriptor) != Some(app_id) { continue; }
        if policy::descriptor_version(descriptor).and_then(stable_version_tuple).is_none() { continue; }
        let raw = match client.fetch_install_proof(descriptor) {
            Ok(raw) => raw,
            Err(policy::ProofError::NotFound) => continue,
            Err(error) => return Err(proof_gateway_error(error)),
        };
        let proof_raw = latest_trust.as_deref().map_or(Ok(raw.clone()), |trust| policy::overlay_trust(&raw, trust)).map_err(proof_gateway_error)?;
        let proof = policy::verify_install_proof(&proof_raw, app_id, policy::now_ms(), false).map_err(proof_gateway_error)?;
        if !policy::descriptor_supports_desktop(&proof.descriptor) { continue; }
        approved.push((proof, proof_raw));
    }
    approved.sort_by(|a, b| stable_version_tuple(policy::descriptor_version(&a.0.descriptor).unwrap_or_default()).cmp(&stable_version_tuple(policy::descriptor_version(&b.0.descriptor).unwrap_or_default())));
    let Some((proof, raw)) = approved.pop() else {
        return Err(GatewayCommandError::Internal("release_not_approved".into()));
    };
    let archive = client
        .download_artifact(
            &proof.artifact.digest,
            proof.artifact.size_bytes,
            proof.artifact.url.as_deref(),
        )
        .map_err(proof_gateway_error)?;
    let package = verify_catalog_bundle(&archive, &proof.authorization).map_err(|_| GatewayCommandError::Internal("digest_mismatch: downloaded mini-app package failed verification".into()))?;
    if !native_mini_app_claim_is_current(&claim) {
        return Err(GatewayCommandError::Internal("stale_account: account changed during install".into()));
    }
    let record = store.with_connection(|connection| {
        ledger::initialize(connection)?;
        let old = ledger::installed(connection, &install_scope, app_id)?;
        let changed = old.as_ref().is_none_or(|row| row.digest != package.digest());
        let record = ledger::activate_authorized_fenced(connection, &install_scope, &install_scope, operation_id, package, &raw, policy::now_ms(), || {
            native_mini_app_claim_is_current(&claim)
        })?;
        Ok::<_, ledger::InstallError>((record, changed))
    }).map_err(|_| GatewayCommandError::Internal("storage: mini-app activation failed".into()))?;
    Ok(serde_json::json!({"appId": record.0.app_id, "version": record.0.version,
        "digest": record.0.digest, "publisherId": proof.authorization.publisher_id,
        "offlineCore": true, "trustEpoch": proof.authorization.trust_epoch,
        "state": "ready", "changed": record.1}))
}

fn stable_version_tuple(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.').map(|part| part.parse::<u64>().ok());
    match (parts.next().flatten(), parts.next().flatten(), parts.next().flatten(), parts.next()) {
        (Some(major), Some(minor), Some(patch), None) => Some((major, minor, patch)),
        _ => None,
    }
}

struct InstalledMiniAppValidation {
    display_name: String,
    size_bytes: u64,
    supported: bool,
    state: mahayana_host_runtime::extensions::mcp::installable_package::NativeMiniAppInstallState,
}

fn quarantined_mini_app_listing(
    host: &UnifiedAppHost,
    scope: &mahayana_host_runtime::extensions::mcp::installable_package::ledger::HostInstallScope,
    app_id: &str,
    fallback_size_bytes: u64,
    reason: &str,
) -> InstalledMiniAppValidation {
    use mahayana_host_runtime::extensions::mcp::installable_package::ledger;
    if let Ok(store) = host.mini_app_store_port() {
        let _ = store.with_connection(|connection| {
            ledger::set_quarantined(connection, scope, app_id, Some(reason))
        });
    }
    InstalledMiniAppValidation {
        display_name: app_id.to_string(),
        size_bytes: fallback_size_bytes,
        supported: false,
        state: mahayana_host_runtime::extensions::mcp::installable_package::NativeMiniAppInstallState::Quarantined,
    }
}

fn validate_installed_listing(
    host: &UnifiedAppHost,
    scope: &mahayana_host_runtime::extensions::mcp::installable_package::ledger::HostInstallScope,
    app_id: &str,
    digest: &str,
    expected_version: &str,
    expected_publisher_id: &str,
    fallback_size_bytes: u64,
) -> InstalledMiniAppValidation {
    use mahayana_host_runtime::extensions::mcp::installable_package::{ledger, policy, verify_catalog_bundle};
    let Ok(store) = host.mini_app_store_port() else {
        return InstalledMiniAppValidation { display_name: app_id.into(), size_bytes: fallback_size_bytes, supported: false, state: mahayana_host_runtime::extensions::mcp::installable_package::NativeMiniAppInstallState::Quarantined };
    };
    let hard_revoked = store.with_connection(|connection| ledger::quarantine_reason(connection, scope, app_id))
        .ok().flatten().as_deref() == Some("release_revoked");
    if hard_revoked {
        return InstalledMiniAppValidation {
            display_name: app_id.to_string(), size_bytes: fallback_size_bytes, supported: false,
            state: mahayana_host_runtime::extensions::mcp::installable_package::NativeMiniAppInstallState::Quarantined,
        };
    }
    let verified = store.with_connection(|connection| {
        ledger::load_active_authorized_archive(connection, scope, app_id)
    });
    let Ok(Some((archive, snapshot, latest_trust))) = verified else {
        return quarantined_mini_app_listing(host, scope, app_id, fallback_size_bytes, "authorization_missing");
    };
    let Ok(snapshot) = policy::overlay_cached_trust(&snapshot, latest_trust.as_deref()) else {
        return quarantined_mini_app_listing(host, scope, app_id, fallback_size_bytes, "cached_policy");
    };
    let Ok(proof) = policy::verify_install_proof(&snapshot, app_id, policy::now_ms(), true) else {
        return quarantined_mini_app_listing(host, scope, app_id, fallback_size_bytes, "cached_policy");
    };
    if proof.authorization.version != expected_version
        || proof.authorization.publisher_id != expected_publisher_id
        || proof.artifact.size_bytes != archive.len() as u64
        || proof.artifact.size_bytes > 100 * 1024 * 1024
    {
        return quarantined_mini_app_listing(host, scope, app_id, fallback_size_bytes, "artifact_size");
    }
    let Ok(package) = verify_catalog_bundle(&archive, &proof.authorization) else {
        return quarantined_mini_app_listing(host, scope, app_id, fallback_size_bytes, "package_verification");
    };
    if package.digest() != digest || package.version() != expected_version
        || package.publisher_id() != expected_publisher_id || package.app_id() != app_id
    {
        return quarantined_mini_app_listing(host, scope, app_id, fallback_size_bytes, "active_digest");
    }
    let _ = store.with_connection(|connection| ledger::set_quarantined(connection, scope, app_id, None));
    let display_name = policy::descriptor_display_name(&proof.descriptor)
        .filter(|name| !name.trim().is_empty() && name.len() <= 160)
        .unwrap_or(app_id)
        .to_string();
    InstalledMiniAppValidation {
        display_name,
        size_bytes: proof.artifact.size_bytes,
        supported: true,
        state: mahayana_host_runtime::extensions::mcp::installable_package::NativeMiniAppInstallState::Ready,
    }
}

fn mini_app_identifier_arg<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<&'a str, GatewayCommandError> {
    let value = object.get(field).and_then(serde_json::Value::as_str).ok_or_else(|| {
        GatewayCommandError::BadRequest(format!("mini-app intent requires {field}"))
    })?;
    let valid = !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));
    if valid {
        Ok(value)
    } else {
        Err(GatewayCommandError::BadRequest(format!(
            "mini-app intent has invalid {field}"
        )))
    }
}

fn required_string_arg<'a>(
    args: &'a serde_json::Value,
    key: &str,
    method: &str,
) -> Result<&'a str, GatewayCommandError> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(format!("{method} requires {key}")))
}

pub(crate) fn started_at_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

fn process_start_instant() -> &'static Instant {
    static PROCESS_START: OnceLock<Instant> = OnceLock::new();
    PROCESS_START.get_or_init(Instant::now)
}

pub(crate) fn process_uptime_ms() -> u64 {
    process_start_instant()
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn write_response(stdout: &Mutex<io::Stdout>, response: &str) -> io::Result<()> {
    let mut stdout = stdout
        .lock()
        .map_err(|_| io::Error::other("desktop host stdout lock poisoned"))?;
    writeln!(stdout, "{response}")?;
    stdout.flush()
}

fn write_runtime_event(
    stdout: &Mutex<io::Stdout>,
    gateway_events: &GatewayEventHub,
    event: serde_json::Value,
) -> io::Result<()> {
    gateway_events.publish(event.clone());
    let frame = serde_json::json!({ "event": event });
    let encoded = serde_json::to_string(&frame)
        .map_err(|error| io::Error::other(format!("event serialization failed: {error}")))?;
    write_response(stdout, &encoded)
}

fn drain_ready_runtime_events(
    host: &UnifiedAppHost,
    stdout: &Mutex<io::Stdout>,
    gateway_events: &GatewayEventHub,
) -> io::Result<()> {
    // Product-local events enqueued by a just-completed command must cross the
    // exact same Grok projection boundary as events delivered by the background
    // PUSH worker. Writing this synchronous drain raw creates a second event
    // protocol and drops compatibility metadata such as attachment contexts.
    let event_source = host.feature_event_source();
    loop {
        match event_source.receive(Duration::ZERO) {
            Ok(Some(event)) => {
                let event = event_source.project_grok_gateway_event(&event);
                write_runtime_event(stdout, gateway_events, event)?;
            }
            Ok(None) => return Ok(()),
            Err(error) => {
                eprintln!("failed to drain Mahayana runtime event: {error}");
                return Ok(());
            }
        }
    }
}

fn main() {
    if std::env::args().any(|arg| arg == LOCAL_EXEC_FILE_SANDBOX_ARG) {
        std::process::exit(execute_production_native_file_request_from_env());
    }
    if std::env::args().any(|arg| arg == BOX_COPY_IN_ARG) {
        let environment = std::env::vars().collect::<BTreeMap<_, _>>();
        let exit_code = execute_production_box_copy_in_from_env(&environment, Path::new("/"));
        std::process::exit(exit_code);
    }

    let _ = process_start_instant();
    let host_started_at_ms = started_at_ms();

    let _process_crash_guard =
        mahayana_host_runtime::process_crash_guard::install_process_crash_guards(
            "sand-host",
            None,
        );
    let app_data_dir = default_unified_app_data_dir();
    if let Err(error) = ensure_managed_runtime_layout(&app_data_dir) {
        eprintln!(
            "failed to initialize managed Mahayana runtime layout at {}: {error}",
            app_data_dir.display()
        );
        std::process::exit(1);
    }

    let host = match UnifiedAppHost::new(app_data_dir.clone()) {
        Ok(host) => host,
        Err(error) => {
            eprintln!("failed to initialize unified Mahayana app host: {error}");
            std::process::exit(1);
        }
    };
    let production_box = ProductionBoxEnvironment::from_process_env();
    let (host_tx, host_rx) = mpsc::channel::<HostLaneRequest>();
    // Platform/account HTTP may legitimately take tens of seconds. Keep it on
    // a dedicated Rust product lane so feature.receive and Agent commands keep
    // flowing through the primary serial Host lane without starvation. The
    // child protocol already correlates responses by id, so out-of-order
    // platform replies are safe.
    let platform_host = match PlatformRequestHost::new(app_data_dir.clone()) {
        Ok(host) => host,
        Err(error) => {
            eprintln!("failed to initialize Mahayana platform request lane: {error}");
            std::process::exit(1);
        }
    };
    let stdout = Arc::new(Mutex::new(io::stdout()));

    let host_lock_path = get_host_lock_path();
    let host_lock = match acquire_host_lock(&host_lock_path, std::process::id()) {
        Ok(acquisition) => acquisition.lock,
        Err(error) => {
            eprintln!(
                "failed to acquire Mahayana Host lock at {}: {error}",
                host_lock_path.display()
            );
            return;
        }
    };

    let mut box_exec_daemon =
        match start_managed_box_exec_daemon_from_process_env(&app_data_dir) {
            Ok(daemon) => daemon,
            Err(error) => {
                eprintln!("failed to start managed Grok box exec-daemon: {error}");
                return;
            }
        };

    let gateway_events = GatewayEventHub::default();
    let production_extensions = Arc::new(match start_production_host_extensions(
        &app_data_dir,
        gateway_events.clone(),
    ) {
        Ok(extensions) => extensions,
        Err(error) => {
            eprintln!("failed to start production Host extensions: {error}");
            return;
        }
    });
    publish_cloud_worker_status(
        &gateway_events,
        production_extensions.cloud_workers.status_snapshot(),
    );
    let mut network_enrollment_supervisor: Option<NetworkEnrollmentSupervisor> = {
        match FileRemoteComputerDeviceSecretProvider::from_environment() {
            Ok(device_provider) => {
                let credential_provider: Arc<dyn FabushiCoreCredentialProvider> =
                    production_extensions.auth.service().clone();
                let device_secret_provider: Arc<dyn RemoteComputerDeviceSecretProvider> =
                    Arc::new(device_provider);
                let api_base_url = std::env::var(FABUSHI_API_BASE_URL_ENV)
                    .unwrap_or_else(|_| DEFAULT_FABUSHI_API_BASE_URL.to_string());
                let target_service = production_remote_computer_target_service(
                    &api_base_url,
                    Arc::clone(&credential_provider),
                    Arc::clone(&device_secret_provider),
                )
                .map(Some)
                .unwrap_or_else(|error| {
                    eprintln!("[fabushi-fdn] remote target service unavailable: {error}");
                    None
                });
                match NetworkEnrollmentOwner::from_environment(
                    credential_provider,
                    device_secret_provider,
                    app_data_dir.join("fdn-network"),
                ) {
                    Ok(owner) => match NetworkEnrollmentSupervisor::start_with_service(owner, target_service) {
                        Ok(supervisor) => Some(supervisor),
                        Err(error) => {
                            eprintln!("[fabushi-fdn] network enrollment supervisor unavailable: {error}");
                            None
                        }
                    },
                    Err(error) => {
                        eprintln!("[fabushi-fdn] network enrollment owner unavailable: {error}");
                        None
                    }
                }
            }
            Err(_) => None,
        }
    };
    let host_telemetry = production_extensions.telemetry.clone();
    let classifier_logs = host_telemetry.logs.clone();
    let classifier_analytics = host_telemetry.analytics.clone();
    pin_sand_auto_review_classifier_measurement_reporter(Some(Arc::new(move |measurement| {
        let mut metadata = BTreeMap::from([
            ("mode".into(), measurement.mode.clone()),
            ("actionKind".into(), measurement.action_kind.clone()),
        ]);
        if let Some(surface_label) = measurement.surface_label.as_ref() {
            metadata.insert("surfaceLabel".into(), surface_label.clone());
        }
        if let Some(timeout_ms) = measurement.timeout_ms {
            metadata.insert("timeoutMs".into(), timeout_ms.to_string());
        }
        if let Some(has_target) = measurement.has_target {
            metadata.insert("hasTarget".into(), has_target.to_string());
        }
        if let Some(has_target_arguments) = measurement.has_target_arguments {
            metadata.insert("hasTargetArguments".into(), has_target_arguments.to_string());
        }
        if let Some(outcome) = measurement.outcome.as_ref() {
            metadata.insert("outcome".into(), outcome.clone());
        }
        if let Some(decision) = measurement.decision.as_ref() {
            metadata.insert("decision".into(), decision.clone());
        }
        if let Some(has_reason) = measurement.has_reason {
            metadata.insert("hasReason".into(), has_reason.to_string());
        }
        if let Some(latency_ms) = measurement.latency_ms {
            metadata.insert("latencyMs".into(), latency_ms.round().max(0.0).to_string());
        }
        if let Some(retry_count) = measurement.retry_count {
            metadata.insert("retryCount".into(), retry_count.to_string());
        }
        if let Some(failure_reason) = measurement.failure_reason.as_ref() {
            metadata.insert("failureReason".into(), failure_reason.clone());
        }
        if let Some(retryable) = measurement.retryable {
            metadata.insert("retryable".into(), retryable.to_string());
        }
        let started = matches!(
            measurement.kind,
            SmartModeClassifierMeasurementKind::Started
        );
        let event = if started {
            "smart_mode.classifier_call.started"
        } else {
            "smart_mode.classifier_call"
        };
        if let Err(error) = classifier_logs.report_projection(&HostTelemetryProjection {
            level: Some("info"),
            event: Some(event),
            metadata: metadata.clone(),
        }) {
            eprintln!("mahayana-host classifier_measurement_telemetry_failed error={error}");
        }
        if !started {
            let properties = serde_json::Value::Object(
                metadata
                    .into_iter()
                    .map(|(key, value)| (key, serde_json::Value::String(value)))
                    .collect(),
            );
            let _ = classifier_analytics.track_event("smart_mode.classifier_call", &properties);
        }
    })));
    let fatal_telemetry = host_telemetry.clone();
    let fatal_logs = host_telemetry.logs.clone();
    _process_crash_guard.set_reporter(Some(Arc::new(move |_message, kind| {
        let _ = fatal_logs.report_host_crash(kind.as_str());
        fatal_telemetry.flush_for_fatal_exit();
    })));
    let invariant_logs = host_telemetry.logs.clone();
    let _invariant_reporter = install_invariant_reporter(Arc::new(move |report| {
        if let Err(error) = invariant_logs.report_invariant_violation(report) {
            eprintln!("mahayana-host invariant_violation_telemetry_failed error={error}");
        }
    }));
    let mut box_extensions = match
        start_production_host_box_extensions(&production_extensions, production_box)
    {
        Ok(extensions) => extensions,
        Err(error) => {
            eprintln!("failed to start production Box extensions: {error}");
            return;
        }
    };
    let host_lifecycle_api = host_telemetry.api();
    let lifecycle_now_ms = host_lifecycle_api.monotonic_now_ms();
    let lifecycle_started_at_ms = lifecycle_now_ms
        - started_at_ms().saturating_sub(host_started_at_ms) as f64;
    let mut host_lifecycle = host_lifecycle_api.create_host_lifecycle_progress(
        lifecycle_started_at_ms,
        production_host_lifecycle_watchdog(),
    );
    if let Err(error) = host_lifecycle.complete(HostLifecycleCompletion {
        phase: "plugin_graph".into(),
        plugin_count: Some(CURRENT_SHIPPING_PRODUCTION_EXTENSION_IDS.len() as u64),
        entry_count: None,
    }) {
        host_lifecycle.fail();
        eprintln!("failed to settle Mahayana Host plugin_graph lifecycle phase: {error}");
        return;
    }
    let forever_box = Arc::clone(&box_extensions.forever_box);
    let attachments_service = box_extensions.attachments.service();
    let secrets_extension = Arc::clone(&box_extensions.secrets);
    let runner_request_context: Arc<dyn RunnerRequestContextSource> =
        Arc::new(ProductionRunnerRequestContextSource::new(
            Arc::clone(&production_extensions.auth),
            Arc::clone(production_extensions.managed_setup.team_rules()),
            app_data_dir.join("transcripts"),
        ));
    let settings_extension = Arc::clone(&production_extensions.settings);
    let local_tool_permission_extension =
        Arc::clone(&production_extensions.local_tool_permission);
    let settings_for_session = Arc::clone(&settings_extension);

    let local_human_id = std::env::var("FABUSHI_LOCAL_HUMAN_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let session_workers = Arc::new(
        ProductionSessionWorkers::production_with_identity_and_dependencies(
            local_human_id,
            Arc::new(move || settings_for_session.get_user_time_zone()),
            production_extensions.memory.service(),
        ),
    );
    session_workers.bind_memory_synthesis_gate();
    if let Ok(store) = host.mini_app_store_port() {
        let store_for_scope = store.clone();
        let store_for_verifier = store;
        let verifier: mahayana_host_runtime::extensions::session::production::MiniAppPackageVerifier = Arc::new(move |app_id: &str, version: Option<&str>, digest: Option<&str>| {
            use mahayana_host_runtime::extensions::mcp::installable_package::ledger;
            let (scope, _) = current_native_mini_app_scope_from_store(&store_for_scope).map_err(|e| {
                mahayana_host_runtime::extensions::session::production::MiniAppPostError::StaleScope(e.to_string())
            })?;
            store_for_verifier.with_connection(|conn| {
                ledger::initialize(conn).map_err(|e| mahayana_host_runtime::extensions::session::production::MiniAppPostError::PersistenceFailed(format!("{e:?}")))?;
                let installed = ledger::installed(conn, &scope, app_id).map_err(|e| mahayana_host_runtime::extensions::session::production::MiniAppPostError::PersistenceFailed(format!("{e:?}")))?;
                let Some(record) = installed else {
                    return Err(mahayana_host_runtime::extensions::session::production::MiniAppPostError::AppNotInstalled(app_id.to_string()));
                };
                if let Some(reason) = ledger::quarantine_reason(conn, &scope, app_id).map_err(|e| mahayana_host_runtime::extensions::session::production::MiniAppPostError::PersistenceFailed(format!("{e:?}")))? {
                    return Err(mahayana_host_runtime::extensions::session::production::MiniAppPostError::AppNotInstalled(format!("{app_id} (quarantined: {reason})")));
                }
                if let Some(expected_ver) = version {
                    if record.version != expected_ver {
                        return Err(mahayana_host_runtime::extensions::session::production::MiniAppPostError::PackageDigestMismatch(format!(
                            "version mismatch: expected {expected_ver}, found {}", record.version
                        )));
                    }
                }
                if let Some(expected_digest) = digest {
                    if record.digest != expected_digest {
                        return Err(mahayana_host_runtime::extensions::session::production::MiniAppPostError::PackageDigestMismatch(format!(
                            "digest mismatch: expected {expected_digest}, found {}", record.digest
                        )));
                    }
                }
                Ok(())
            }).map_err(|e| {
                mahayana_host_runtime::extensions::session::production::MiniAppPostError::PersistenceFailed(format!("{e:?}"))
            })
        });
        session_workers.set_mini_app_package_verifier(verifier);
    }
    let handoff_prepare_box = Arc::clone(&forever_box);
    let handoff_screenshot_box = Arc::clone(&forever_box);
    let handoff_status_box = Arc::clone(&forever_box);
    let handoff_status_events = gateway_events.clone();
    let handoff_started_events = gateway_events.clone();
    let handoff_ended_events = gateway_events.clone();
    let handoff_logs = host_telemetry.logs.clone();
    let handoff_analytics = host_telemetry.analytics.clone();
    let handoff_deps = BoxHandoffDeps {
        prepare: Some(Arc::new(move |request| {
            handoff_prepare_box
                .ensure(&request.agent_id)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })),
        grab_screenshot: Some(Arc::new(move |agent_id| {
            handoff_screenshot_box
                .capture_screenshot(agent_id)
                .map(|bytes| bytes.map(ScreenshotPayload::Bytes))
                .map_err(|error| error.to_string())
        })),
        on_status_changed: Some(Arc::new(move |agent_id, pending| {
            let status = handoff_status_box.get_status(agent_id);
            handoff_status_events.publish(serde_json::json!({
                "channel": "forever-box",
                "payload": project_forever_box_status(&status, pending.as_ref()),
            }));
        })),
        on_started: Some(Arc::new(move |event| {
            handoff_started_events.publish(serde_json::json!({
                "channel": "session.box-handoff-started",
                "payload": {
                    "agentId": event.agent_id,
                    "instruction": event.instruction,
                },
            }));
        })),
        on_ended: Some(Arc::new(move |event| {
            handoff_ended_events.publish(serde_json::json!({
                "channel": "session.box-handoff-ended",
                "payload": {
                    "agentId": event.agent_id,
                    "requestId": event.request_id,
                    "resolution": event.resolution,
                    "trigger": event.trigger,
                },
            }));
            Ok(())
        })),
        report_box_help: Some(Arc::new(move |event| {
            let _ = handoff_logs.report_box_help(&event);
        })),
        track_event: Some(Arc::new(move |name, properties| {
            let _ = handoff_analytics.track_event(name, &properties);
        })),
        ..BoxHandoffDeps::default()
    };
    let (session_workers, session_handoff) = match production_extensions.start_session(
        Arc::clone(&session_workers),
        handoff_deps,
    ) {
        Ok(session) => session,
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to start production Session extension: {error}");
            return;
        }
    };
    let auto_review_update_events = gateway_events.clone();
    let auto_review_update_sessions = Arc::clone(&session_workers);
    let auto_review_update_sink = Arc::new(move |agent_id: &str, update: serde_json::Value| {
        let request_id = update
            .get("requestId")
            .or_else(|| update.pointer("/message/approval/requestId"))
            .and_then(serde_json::Value::as_str);
        let Some(request_id) = request_id else {
            return;
        };
        let Ok(entries) = auto_review_update_sessions.read_agent_transcript_entries(agent_id) else {
            return;
        };
        let Some(entry) = entries.into_iter().find(|entry| {
            entry.get("id").and_then(serde_json::Value::as_str) == Some(request_id)
        }) else {
            return;
        };
        let event_type = if update.get("type").and_then(serde_json::Value::as_str)
            == Some("send-message")
        {
            "appended"
        } else {
            "updated"
        };
        auto_review_update_events.publish(serde_json::json!({
            "channel": "transcript",
            "payload": {
                "type": event_type,
                "agentId": agent_id,
                "entry": entry,
            }
        }));
    });
    let auto_review_logs = host_telemetry.logs.clone();
    let auto_review_telemetry_sink = Arc::new(move |event: &SandAutoReviewEvent| {
        let (event_type, approval, cause) = match event {
            SandAutoReviewEvent::Created(approval) => ("created", approval, None),
            SandAutoReviewEvent::Resolved(approval) => ("resolved", approval, None),
            SandAutoReviewEvent::Expired { approval, cause } => (
                "expired",
                approval,
                Some(match cause {
                    SandAutoReviewExpiryCause::Ttl => "ttl",
                    SandAutoReviewExpiryCause::Cancelled => "cancelled",
                    SandAutoReviewExpiryCause::UserRedirect => "user_redirect",
                    SandAutoReviewExpiryCause::SettingsChange => "settings_change",
                    SandAutoReviewExpiryCause::SessionEnd => "session_end",
                    SandAutoReviewExpiryCause::Quiesce => "quiesce",
                    SandAutoReviewExpiryCause::Other(value) => value.as_str(),
                }.to_string()),
            ),
        };
        let status = match approval.status {
            SandAutoReviewApprovalStatus::Pending => "pending",
            SandAutoReviewApprovalStatus::Approved => "approved",
            SandAutoReviewApprovalStatus::Denied => "denied",
            SandAutoReviewApprovalStatus::Expired => "expired",
        };
        let now_ms = started_at_ms();
        let report = AutoReviewApprovalReport {
            event_type: event_type.to_string(),
            conversation_id: approval.agent_id.clone(),
            approval_id: approval.id.clone(),
            surface: approval.surface.key().to_string(),
            status: status.to_string(),
            age_ms: now_ms.saturating_sub(approval.created_at_ms) as f64,
            ttl_ms: approval
                .expires_at_ms
                .map(|expires_at_ms| expires_at_ms.saturating_sub(approval.created_at_ms) as f64),
            cause,
        };
        let _ = auto_review_logs.report_auto_review_approval(&report);
    });
    let auto_review_expire_sweep_logs = host_telemetry.logs.clone();
    let auto_review_expire_sweep_sink = Arc::new(move |stage: &str, error_class: &str| {
        let _ = auto_review_expire_sweep_logs
            .report_auto_review_expire_sweep_failed(stage, error_class);
    });
    let auto_review_extension = match production_extensions.start_auto_review(
        Arc::clone(&session_workers),
        format!("host-{}", uuid::Uuid::new_v4()),
        auto_review_update_sink,
        auto_review_telemetry_sink,
        Some(auto_review_expire_sweep_sink),
    ) {
        Ok(extension) => extension,
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to start production AutoReview extension: {error}");
            return;
        }
    };
    let auto_review_display_recheck_logs = host_telemetry.logs.clone();
    auto_review_extension
        .service()
        .set_display_recheck_failed_sink(Arc::new(move |agent_id| {
            let _ = auto_review_display_recheck_logs
                .report_auto_review_display_recheck_failed(agent_id);
        }));
    let (transcript_manager, roster_emit, transcript_events, transcript_profile_watch_error) =
        match production_extensions.start_transcript(
            &app_data_dir,
            Arc::clone(&session_workers),
            Arc::clone(&attachments_service),
            gateway_events.clone(),
        ) {
            Ok(transcript) => transcript,
            Err(error) => {
                host_lifecycle.fail();
                eprintln!("failed to start production Transcript extension: {error}");
                return;
            }
        };
    if let Some(error) = transcript_profile_watch_error {
        eprintln!(
            "[sand-host] profile watcher unavailable; roster RPC remains authoritative: {error}"
        );
    }
    transcript_manager
        .set_handoff_service(session_handoff.clone())
        .expect("production Transcript manager handoff service must be configured exactly once");
    transcript_manager
        .widget_responses()
        .bind_auto_review(auto_review_extension.service())
        .expect("Transcript WidgetResponses AutoReview owner must be configured exactly once");
    let content_search_extension = Arc::clone(&production_extensions.content_search);
    let permission_widget_responses = transcript_manager.widget_responses();
    let stranded_permission_logs = host_telemetry.logs.clone();
    local_tool_permission_extension.bind_transcript(
        permission_widget_responses,
        started_at_ms(),
        Some(Arc::new(move || {
            let _ = stranded_permission_logs.report_local_tool_permission_stranded_retirement();
        })),
        Some(Arc::new(|message| eprintln!("{message}"))),
    );
    let retired_permission_events = gateway_events.clone();
    let local_exec_retirement_owner = Arc::clone(&production_extensions.local_exec);
    local_tool_permission_extension.bind_approval_retired_sink(Some(Arc::new(move |approval_id| {
        local_exec_retirement_owner.retire_approval(approval_id);
        retired_permission_events.publish(serde_json::json!({
            "channel": "local-tool-permission.approval-retired",
            "payload": { "approvalId": approval_id },
        }));
    })));

    let notification_baseline = session_workers
        .list_agent_summaries(None)
        .ok()
        .map(|summaries| {
            summaries
                .into_iter()
                .filter_map(|summary| serde_json::to_value(summary).ok())
                .filter_map(|value| notification_agent_from_value(&value))
                .collect::<Vec<_>>()
        });
    let notification_transcript = Arc::clone(&transcript_manager);
    if let Err(error) = production_extensions.start_notifications(
        gateway_events.clone(),
        notification_baseline,
        Arc::new(move || {
            notification_transcript
                .window_focused_at_ms()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(|value| value as u64)
        }),
    ) {
        host_lifecycle.fail();
        eprintln!("failed to start production Notifications extension: {error}");
        return;
    }
    {
        let dropped_logs = host_telemetry.logs.clone();
        transcript_manager
            .automation_runtime()
            .set_dropped_fire_reporter(Some(Arc::new(move |dropped| {
                let lateness_ms = dropped.scheduled_for_ms.map(|scheduled_for_ms| {
                    (started_at_ms() as f64 - scheduled_for_ms).max(0.0)
                });
                let report = AutomationFireDroppedReport {
                    conversation_id: dropped.agent_id,
                    trigger: dropped.trigger.as_str().to_string(),
                    reason: dropped.reason,
                    scheduled_for_ms: dropped.scheduled_for_ms,
                    lateness_ms,
                    error_type: None,
                    error_code: None,
                    run_uuid: dropped.run_uuid,
                    fire_age_ms: None,
                    has_definition_revision: None,
                    box_uptime_ms: None,
                };
                let _ = dropped_logs.report_automation_fire_dropped(&report);
            })));
    }
    {
        let lifecycle_logs = host_telemetry.logs.clone();
        let lifecycle_roster = Arc::clone(&roster_emit);
        let lifecycle_transcript_manager = Arc::clone(&transcript_manager);
        transcript_manager
            .automation_runtime()
            .set_lifecycle_reporter(Some(Arc::new(move |event| {
                lifecycle_transcript_manager.emit_automation_config_changed();
                lifecycle_roster.publish_timeline_event(
                    &event.agent_id,
                    serde_json::json!({
                        "type": "automation-changed",
                        "action": event.action.as_str(),
                        "automationId": event.automation_id.clone(),
                        "automationName": event.automation_name.clone(),
                    }),
                );
                let _ = lifecycle_logs.report_automation_lifecycle(event);
            })));
    }
    {
        let run_logs = host_telemetry.logs.clone();
        let run_transcript_manager = Arc::clone(&transcript_manager);
        transcript_manager
            .automation_runtime()
            .run_path()
            .set_run_reporter(Some(Arc::new(move |observed| {
                if observed.quiesced_for_upgrade {
                    run_transcript_manager.mark_upgrade_resume_pending(UpgradeResumeMarker {
                        agent_id: observed.agent_id.clone(),
                        marked_at_ms: started_at_ms() as f64,
                        source: Some("automation".into()),
                        automation_id: Some(observed.automation_id.clone()),
                        automation_run_id: observed.automation_run_id.clone(),
                    });
                }
                let Some(is_group) = observed.is_group else {
                    return;
                };
                let outcome = match observed.outcome {
                    FireAutomationOutcome::Ok => "ok",
                    FireAutomationOutcome::Error => "error",
                    FireAutomationOutcome::Interrupted => "interrupted",
                };
                let report = AutomationRunReport {
                    conversation_id: observed.agent_id.clone(),
                    automation_id: stable_automation_id(
                        &observed.agent_id,
                        &observed.automation_id,
                    ),
                    trigger: observed.trigger.as_str().to_string(),
                    outcome: outcome.into(),
                    is_group,
                    duration_ms: observed.duration_ms,
                    lateness_ms: observed.lateness_ms,
                    scheduled_for_ms: observed.scheduled_for_ms,
                    sent_message_count: observed.sent_message_count,
                    event_batch_size: observed.event_batch_size,
                };
                let _ = run_logs.report_automation_run(&report);
            })));
    }
    let runner_registry = transcript_manager.runner_registry();
    let transcript_runtime = transcript_manager.transcript_runtime();
    let generated_agent_runtime = Arc::new(Mutex::new(SubagentRuntime::default()));
    let host_runner_composition = Arc::new(HostRunnerComposition::production(
        local_tool_permission_extension.controller(),
        Arc::clone(&session_workers),
    ));
    let permission_surface_owner = Arc::clone(&host_runner_composition);
    local_tool_permission_extension.bind_ask_surfaces(Arc::new(move |agent_id| {
        permission_surface_owner.can_ask_local_tool_permission(agent_id)
    }));
    let ack_obligations = transcript_manager.ack_obligations();
    {
        let ack_logs = host_telemetry.logs.clone();
        ack_obligations.set_telemetry_reporter(Some(Arc::new(move |report| {
            let _ = ack_logs.report_ack_obligation(report);
        })));
    }
    let cross_user_deletion_slot =
        Arc::new(Mutex::new(Weak::<ProductionCrossUserRuntime>::new()));
    let cloud_agent_deletion_watches =
        Arc::new(Mutex::new(Weak::<RunnerCloudAgentWatches>::new()));
    let background_shell_deletion_watches =
        Arc::new(Mutex::new(Weak::<RunnerBackgroundShellWatches>::new()));
    let completion_revivals_deletion_slot =
        Arc::new(Mutex::new(Weak::<CompletionRevivals>::new()));
    let box_store_sync_deletion_slot = Arc::new(Mutex::new(None::<ProductionBoxStoreSyncApi>));
    let generated_subagent_cleanup_slot: GeneratedSubagentCleanupSlot =
        Arc::new(Mutex::new(None));
    let agent_deletion_runtime = AgentDeletionRuntimeDeps {
        mark_deleting: Some({
            let transcript_runtime = Arc::clone(&transcript_runtime);
            Arc::new(move |agent_id| {
                transcript_runtime.session_runtime().mark_agent_deleted(agent_id);
                Ok(())
            })
        }),
        clear_deleting: Some({
            let transcript_runtime = Arc::clone(&transcript_runtime);
            Arc::new(move |agent_id| {
                transcript_runtime.session_runtime().clear_agent_deleted(agent_id);
                Ok(())
            })
        }),
        cancel_runner: Some({
            let runner_registry = Arc::clone(&runner_registry);
            let transcript_runtime = Arc::clone(&transcript_runtime);
            let telemetry_logs = host_telemetry.logs.clone();
            Arc::new(move |agent_id| {
                let was_in_flight = transcript_runtime.is_agent_running(agent_id);
                runner_registry.clear_group_member_preempted(agent_id);
                let had_group_run =
                    runner_registry.cancel_group_member_agent(agent_id, "agent deleted") > 0;
                let had_active_run =
                    runner_registry.cancel_agent(agent_id, "agent deleted") > 0 || had_group_run;
                let fields = TurnInterruptFields {
                    conversation_id: agent_id.to_string(),
                    reason: "agent_deleted".into(),
                    had_active_run,
                    was_in_flight,
                };
                telemetry_logs
                    .report_turn_interrupt(&fields)
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
        }),
        forget_ack: Some({
            let ack_obligations = Arc::clone(&ack_obligations);
            Arc::new(move |agent_id| {
                let _ = ack_obligations
                    .forget_agent(agent_id)
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
        }),
        sharing_departure: Some({
            let slot = Arc::clone(&cross_user_deletion_slot);
            Arc::new(move |agent_id| {
                let runtime = slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .upgrade();
                if let Some(runtime) = runtime {
                    runtime.note_agent_deleted(agent_id)?;
                }
                Ok(())
            })
        }),
        clear_trays: Some({
            let trays = Arc::clone(&production_extensions.trays);
            Arc::new(move |agent_id| {
                trays.clear_for_agent(agent_id);
                Ok(())
            })
        }),
        dispose_background_work: Some({
            let cloud_agent_watches = Arc::clone(&cloud_agent_deletion_watches);
            let background_shell_watches = Arc::clone(&background_shell_deletion_watches);
            let completion_revivals = Arc::clone(&completion_revivals_deletion_slot);
            Arc::new(move |agent_id| {
                if let Some(completion_revivals) = completion_revivals
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .upgrade()
                {
                    completion_revivals.clear_agent_pending_completions(agent_id);
                }
                if let Some(watches) = cloud_agent_watches
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .upgrade()
                {
                    watches.dispose_parent(agent_id);
                }
                if let Some(watches) = background_shell_watches
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .upgrade()
                {
                    watches.dispose_parent(agent_id);
                }
                Ok(())
            })
        }),
        drain_runner: Some({
            let runner_registry = Arc::clone(&runner_registry);
            let transcript_runtime = Arc::clone(&transcript_runtime);
            let generated_agent_runtime = Arc::clone(&generated_agent_runtime);
            Arc::new(move |agent_id| {
                let child_ids = generated_agent_runtime
                    .lock()
                    .map(|mut runtime| runtime.abort_running_subagents_for_parent(agent_id))
                    .unwrap_or_default();
                for child_id in &child_ids {
                    let _ = runner_registry.cancel_agent(child_id, "parent agent deleted");
                }

                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    let parent_streams_drained =
                        runner_registry.active_stream_ids_for_agent(agent_id).is_empty();
                    let child_streams_drained = child_ids.iter().all(|child_id| {
                        runner_registry.active_stream_ids_for_agent(child_id).is_empty()
                    });
                    let subagents_drained = generated_agent_runtime
                        .lock()
                        .map(|runtime| {
                            runtime.running_subagent_ids_for_parent(agent_id).is_empty()
                        })
                        .unwrap_or(false);
                    let lifecycle_drained = !transcript_runtime.is_agent_running(agent_id);
                    let dispatch_drained = transcript_runtime.is_turn_dispatch_idle(agent_id);

                    if parent_streams_drained
                        && child_streams_drained
                        && subagents_drained
                        && lifecycle_drained
                        && dispatch_drained
                    {
                        return Ok(());
                    }
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "timed out draining Agent {agent_id} before delete"
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            })
        }),
        release_box: Some({
            let forever_box = Arc::clone(&forever_box);
            Arc::new(move |agent_id| {
                forever_box.release_agent(agent_id);
                Ok(())
            })
        }),
        forget_agent_state: Some({
            let box_store_sync_slot = Arc::clone(&box_store_sync_deletion_slot);
            let forever_box = Arc::clone(&forever_box);
            Arc::new(move |agent_id| {
                if let Some(api) = box_store_sync_slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
                {
                    api.forget_agent(agent_id)?;
                }
                forever_box.forget_disk_pressure_agent(agent_id);
                Ok(())
            })
        }),
        forget_handoff: Some({
            let handoff = session_handoff.clone();
            Arc::new(move |agent_id| {
                handoff.forget(agent_id);
                Ok(())
            })
        }),
        clear_pending_wakes: Some({
            let transcript_runtime = transcript_manager.transcript_runtime();
            Arc::new(move |agent_id| {
                transcript_runtime.clear_agent_durable_recovery(agent_id);
                Ok(())
            })
        }),
    };

    let gateway_config = match resolve_gateway_server_config() {
        Ok(config) => config,
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to resolve Mahayana gateway configuration: {error}");
            return;
        }
    };
    let gateway_started_at = started_at_ms();
    let local_exec_extension = Arc::clone(&production_extensions.local_exec);
    let local_exec_ask_owner = Arc::clone(&local_exec_extension);
    local_tool_permission_extension.bind_live_computer_check(Arc::new(move |agent_id| {
        local_exec_ask_owner.check_live_computer_for_ask(Some(agent_id))
    }));
    let routed_tool_relay = Arc::new(CoordinatorToolRelay::new(gateway_events.clone()));
    let mcp_lifecycle_relay = Arc::new(CoordinatorMcpLifecycleRelay::new(gateway_events.clone()));
    let box_mcp_owner = Arc::clone(&forever_box);
    let box_status_loader: BoxServerStatusLoader = Arc::new(move |ids, kick_only| {
        let accessor = box_mcp_owner
            .mcp_resource_accessor()
            .map_err(|error| error.to_string())?;
        ProductionBoxMcpStateLoader::new(accessor).list_servers(ids, kick_only)
    });
    let mcp_manager_backend = Arc::new(CoordinatorMcpManagerBackend::new(
        Arc::clone(&mcp_lifecycle_relay),
        box_status_loader,
    ));
    let box_store_idle_runtime = Arc::clone(&transcript_manager);
    let mcp_service = match production_extensions.start_mcp(
        &app_data_dir,
        Arc::clone(&mcp_lifecycle_relay),
        mcp_manager_backend,
    ) {
        Ok(service) => service,
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to start production MCP extension: {error}");
            return;
        }
    };
    if let Err(error) = production_extensions.start_box_store_sync(Arc::new(move || {
        box_store_idle_runtime.live_running_agent_ids().is_empty()
            && !box_store_idle_runtime.has_carryable_pending_wake()
    })) {
        host_lifecycle.fail();
        eprintln!("failed to start production BoxStoreSync extension: {error}");
        return;
    }
    let box_store_sync_api = match production_extensions.box_store_sync_api() {
        Ok(Some(api)) => api,
        Ok(None) => {
            host_lifecycle.fail();
            eprintln!("production BoxStoreSync started without an API");
            return;
        }
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to access production BoxStoreSync API: {error}");
            return;
        }
    };
    *box_store_sync_deletion_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(box_store_sync_api.clone());
    let roster_bookkeeping = Arc::new(Mutex::new(production_host_roster_bookkeeping(
        Arc::clone(&attachments_service),
        Arc::clone(&transcript_manager),
        Arc::clone(&forever_box),
        Arc::clone(&production_extensions),
        box_store_sync_api.clone(),
        Arc::clone(&production_extensions.source_map),
    )));
    roster_bookkeeping
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .apply_config(&settings_extension.get_host_settings());
    let lifecycle_host_bundle_version = read_local_host_version(SAND_BOX_HOST_VERSION_PATH)
        .or_else(|| {
            std::env::var("SAND_HOST_BUNDLE_VERSION")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .or_else(|| Some(env!("CARGO_PKG_VERSION").to_string()));
    let lifecycle_box_store_id = match box_store_sync_api.get_store_id() {
        Ok(store_id) => Some(store_id),
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to resolve production BoxStore identity: {error}");
            return;
        }
    };
    if let Err(error) = host_lifecycle_api.set_host_bundle_identity(HostBundleIdentity {
        host_bundle_version: lifecycle_host_bundle_version,
        box_store_id: lifecycle_box_store_id,
    }) {
        host_lifecycle.fail();
        eprintln!("failed to set Mahayana Host telemetry identity: {error}");
        return;
    }
    if let Err(error) = host_lifecycle.complete(HostLifecycleCompletion {
        phase: "identity".into(),
        plugin_count: None,
        entry_count: None,
    }) {
        host_lifecycle.fail();
        eprintln!("failed to settle Mahayana Host identity lifecycle phase: {error}");
        return;
    }
    if let Err(error) = host_lifecycle.complete(HostLifecycleCompletion {
        phase: "log_catchup".into(),
        plugin_count: None,
        entry_count: None,
    }) {
        host_lifecycle.fail();
        eprintln!("failed to settle Mahayana Host log_catchup lifecycle phase: {error}");
        return;
    }
    let initial_transcript_entry_count = match load_initial_transcript_resiliently(|| {
        ensure_initial_transcript_loaded(&session_workers, transcript_runtime.session_runtime())
    }) {
        Ok(outcome) => {
            let entry_count = outcome.entry_count;
            match outcome.degraded {
                Some(InitialTranscriptDegradedReason::SqliteBusy(error)) => {
                    eprintln!(
                        "[sand-host] initial transcript load still locked after retries (kept alive): {error}"
                    );
                }
                Some(InitialTranscriptDegradedReason::AgentLimit) => {
                    eprintln!(
                        "[sand-host] no session at the agent cap; starting without one so the roster and delete stay reachable"
                    );
                }
                None => {
                    eprintln!("[sand-host] initial transcript loaded entries={entry_count}");
                }
            }
            entry_count
        }
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("[sand-host] initial transcript load failed: {error}");
            return;
        }
    };
    if let Err(error) = host_lifecycle.complete(HostLifecycleCompletion {
        phase: "transcript_read".into(),
        plugin_count: None,
        entry_count: Some(initial_transcript_entry_count as u64),
    }) {
        host_lifecycle.fail();
        eprintln!("failed to settle Mahayana Host transcript_read lifecycle phase: {error}");
        return;
    }
    let automations_lifecycle_slot =
        Arc::new(Mutex::new(Weak::<ProductionAutomationsLifecycle>::new()));
    let cloud_agent_completion_gateway_slot =
        Arc::new(Mutex::new(Weak::<UnifiedGatewayApi>::new()));
    let completion_revivals = Arc::new(CompletionRevivals::new(Arc::new(
        ProductionCompletionRevivalRuntime {
            gateway: Arc::clone(&cloud_agent_completion_gateway_slot),
        },
    )));
    *completion_revivals_deletion_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Arc::downgrade(&completion_revivals);
    let cloud_watch_manager = production_extensions.cloud_agents.service();
    let cloud_watch_pending_store = transcript_runtime.pending_wake_store().cloned();
    let cloud_watch_pending_events = gateway_events.clone();
    let cloud_watch_settled = Arc::clone(&completion_revivals);
    let cloud_watch_async_runtime = Arc::clone(&transcript_runtime);
    let cloud_watch_async_events = gateway_events.clone();
    let cloud_watch_async_generated = Arc::clone(&generated_agent_runtime);
    let cloud_watch_async_cloud_slot = Arc::clone(&cloud_agent_deletion_watches);
    let cloud_watch_async_shell_slot = Arc::clone(&background_shell_deletion_watches);
    let cloud_agent_watches = Arc::new(RunnerCloudAgentWatches::new(
        Arc::new(move |bc_id, wait_for_restart| {
            let result = cloud_watch_manager.await_completion(bc_id, wait_for_restart);
            mahayana_host_runtime::runner::background_work::CloudAgentWatchOutcome {
                status: result.status.to_string(),
                text: result.text,
            }
        }),
        cloud_watch_pending_store.map(|store| {
            Arc::new(move |pending: &mahayana_host_runtime::runner::background_work::CloudAgentPendingWatch| {
                let quiet_origin = pending
                    .quiet_origin
                    .as_ref()
                    .and_then(coerce_quiet_origin);
                let written = store.mark_pending(DurablePendingWakeMarker {
                    agent_id: pending.parent_agent_id.clone(),
                    kind: PendingWakeKind::CloudAgent,
                    work_id: pending.work_id.clone(),
                    marked_at_ms: pending.marked_at_ms as f64,
                    quiet_origin,
                    title: Some(pending.title.clone()),
                    subagent_type: Some("cursor-agent".into()),
                    interrupted_by_recreate: false,
                    delivery_operation_id: None,
                    address_token: None,
                    failure_reason: None,
                    session_key: None,
                });
                if !written {
                    eprintln!(
                        "mahayana-host pending_cloud_agent_wake_persist_failed agent={} work={}",
                        pending.parent_agent_id, pending.work_id
                    );
                }
                cloud_watch_pending_events.publish(serde_json::json!({
                    "channel": "pending-wake",
                    "payload": {
                        "agentId": pending.parent_agent_id,
                        "kind": "cloud-agent",
                        "workId": pending.work_id,
                        "outcome": if written { "persisted" } else { "persist_failed" },
                    }
                }));
            }) as mahayana_host_runtime::runner::background_work::CloudAgentPendingCallback
        }),
        Some(Arc::new(move |completion| {
            let quiet_origin = completion
                .quiet_origin
                .as_ref()
                .and_then(coerce_quiet_origin);
            let pending_wake_marker = DurablePendingWakeMarker {
                agent_id: completion.parent_agent_id.clone(),
                kind: PendingWakeKind::CloudAgent,
                work_id: completion.work_id.clone(),
                marked_at_ms: completion.marked_at_ms,
                quiet_origin: quiet_origin.clone(),
                title: Some(completion.title.clone()),
                subagent_type: Some("cursor-agent".into()),
                interrupted_by_recreate: false,
                delivery_operation_id: None,
                address_token: None,
                failure_reason: None,
                session_key: None,
            };
            let completion_revivals = Arc::clone(&cloud_watch_settled);
            let thread_name = format!("mahayana-cloud-agent-revival-{}", completion.work_id);
            let _ = thread::Builder::new().name(thread_name).spawn(move || {
                completion_revivals.handle_background_subagent_completion(SubagentCompletion {
                    parent_agent_id: completion.parent_agent_id,
                    subagent_agent_id: completion.work_id,
                    title: completion.title,
                    subagent_type: "cursor-agent".into(),
                    status: completion.status,
                    result: completion.result,
                    quiet_origin,
                    pending_wake_marker: Some(pending_wake_marker),
                });
            });
        })),
        Some(Arc::new(move |agent_id| {
            publish_async_tasks_changed_from_slots(
                &cloud_watch_async_events,
                cloud_watch_async_runtime.as_ref(),
                &cloud_watch_async_generated,
                &cloud_watch_async_cloud_slot,
                &cloud_watch_async_shell_slot,
                agent_id,
            );
        })),
    ));
    let shell_watch_box = Arc::clone(&forever_box);
    let shell_watch_local_tool_permission = local_tool_permission_extension.controller();
    let shell_watch_pending_store = transcript_runtime.pending_wake_store().cloned();
    let shell_watch_pending_events = gateway_events.clone();
    let shell_watch_settled = Arc::clone(&completion_revivals);
    let shell_watch_async_runtime = Arc::clone(&transcript_runtime);
    let shell_watch_async_events = gateway_events.clone();
    let shell_watch_async_generated = Arc::clone(&generated_agent_runtime);
    let shell_watch_async_cloud_slot = Arc::clone(&cloud_agent_deletion_watches);
    let shell_watch_async_shell_slot = Arc::clone(&background_shell_deletion_watches);
    *cloud_agent_deletion_watches
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Arc::downgrade(&cloud_agent_watches);
    let background_shell_watches = Arc::new(RunnerBackgroundShellWatches::new(
        Arc::new(move |agent_id, shell_id, cancelled| {
            let resources = ForeverBoxRunnerResourcePort::new(
                Arc::clone(&shell_watch_box),
                agent_id.to_string(),
            )
            .with_background_read_permission(Arc::clone(&shell_watch_local_tool_permission));
            let poll_ms = shell_rewatch_poll_ms(
                std::env::var("SAND_SHELL_REWATCH_POLL_MS")
                    .ok()
                    .as_deref(),
            );
            poll_shell_terminal_file(
                started_at_ms(),
                poll_ms,
                || resources.read_background_shell_terminal(shell_id),
                started_at_ms,
                |milliseconds| thread::sleep(Duration::from_millis(milliseconds)),
                || cancelled.load(Ordering::Acquire),
            )
            .map(|settlement| BackgroundShellWatchOutcome {
                status: match settlement.status {
                    ShellWatchStatus::Success => "success".into(),
                    ShellWatchStatus::Error => "error".into(),
                },
                detail: settlement.detail,
                output_path: settlement.output_path,
            })
        }),
        shell_watch_pending_store.map(|store| {
            Arc::new(move |pending: &mahayana_host_runtime::runner::background_work::BackgroundShellPendingWatch| {
                let quiet_origin = pending
                    .quiet_origin
                    .as_ref()
                    .and_then(coerce_quiet_origin);
                let written = store.mark_pending(DurablePendingWakeMarker {
                    agent_id: pending.parent_agent_id.clone(),
                    kind: PendingWakeKind::Shell,
                    work_id: pending.work_id.clone(),
                    marked_at_ms: pending.marked_at_ms as f64,
                    quiet_origin,
                    title: Some(pending.title.clone()),
                    subagent_type: None,
                    interrupted_by_recreate: false,
                    delivery_operation_id: None,
                    address_token: None,
                    failure_reason: None,
                    session_key: None,
                });
                if !written {
                    eprintln!(
                        "mahayana-host pending_shell_wake_persist_failed agent={} work={}",
                        pending.parent_agent_id, pending.work_id
                    );
                }
                shell_watch_pending_events.publish(serde_json::json!({
                    "channel": "pending-wake",
                    "payload": {
                        "agentId": pending.parent_agent_id,
                        "kind": "shell",
                        "workId": pending.work_id,
                        "outcome": if written { "persisted" } else { "persist_failed" },
                    }
                }));
            }) as mahayana_host_runtime::runner::background_work::BackgroundShellPendingCallback
        }),
        Some(Arc::new(move |completion| {
            let quiet_origin = completion
                .quiet_origin
                .as_ref()
                .and_then(coerce_quiet_origin);
            let pending_wake_marker = DurablePendingWakeMarker {
                agent_id: completion.parent_agent_id.clone(),
                kind: PendingWakeKind::Shell,
                work_id: completion.work_id.clone(),
                marked_at_ms: completion.marked_at_ms,
                quiet_origin: quiet_origin.clone(),
                title: Some(completion.title.clone()),
                subagent_type: None,
                interrupted_by_recreate: false,
                delivery_operation_id: None,
                address_token: None,
                failure_reason: None,
                session_key: None,
            };
            let completion_revivals = Arc::clone(&shell_watch_settled);
            let thread_name = format!("mahayana-shell-revival-{}", completion.work_id);
            let _ = thread::Builder::new().name(thread_name).spawn(move || {
                completion_revivals.handle_background_shell_completion(ShellCompletion {
                    agent_id: completion.parent_agent_id,
                    shell_id: completion.work_id,
                    title: completion.title,
                    status: completion.status,
                    detail: completion.detail,
                    output_path: completion.output_path,
                    quiet_origin,
                    pending_wake_marker: Some(pending_wake_marker),
                });
            });
        })),
        Some(Arc::new(move |agent_id| {
            publish_async_tasks_changed_from_slots(
                &shell_watch_async_events,
                shell_watch_async_runtime.as_ref(),
                &shell_watch_async_generated,
                &shell_watch_async_cloud_slot,
                &shell_watch_async_shell_slot,
                agent_id,
            );
        })),
    ));
    *background_shell_deletion_watches
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Arc::downgrade(&background_shell_watches);

    if let Some(pending_store) = transcript_runtime.pending_wake_store().cloned() {
        let abort_async_runtime = Arc::clone(&transcript_runtime);
        let abort_async_events = gateway_events.clone();
        let abort_async_generated = Arc::clone(&generated_agent_runtime);
        let abort_async_cloud_slot = Arc::clone(&cloud_agent_deletion_watches);
        let abort_async_shell_slot = Arc::clone(&background_shell_deletion_watches);
        generated_agent_runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_abort_observer(Some(Arc::new(move |parent_agent_id, subagent_id| {
                if let Some(marker) = pending_store
                    .list_pending()
                    .into_iter()
                    .find(|entry| {
                        entry.agent_id == parent_agent_id
                            && entry.kind == PendingWakeKind::Subagent
                            && entry.work_id == subagent_id
                    })
                {
                    let _ = pending_store.clear_if_matches(&marker);
                }
                publish_async_tasks_changed_from_slots(
                    &abort_async_events,
                    abort_async_runtime.as_ref(),
                    &abort_async_generated,
                    &abort_async_cloud_slot,
                    &abort_async_shell_slot,
                    parent_agent_id,
                );
            })));
    }

    let cross_user_runner_deps = LocalRoutedRunnerDeps {
        routed_tool_relay: Arc::clone(&routed_tool_relay),
        mcp_service: Arc::clone(&mcp_service),
        auth: Arc::clone(&production_extensions.auth),
        auto_review: Arc::clone(&auto_review_extension),
        events: gateway_events.clone(),
        host_tx: host_tx.clone(),
        data_dir: app_data_dir.clone(),
        request_context: Arc::clone(&runner_request_context),
        experiments: Arc::clone(&production_extensions.experiments),
        settings: Arc::clone(&settings_extension),
        inference: Arc::clone(&production_extensions.inference),
        session_workers: Arc::clone(&session_workers),
        runner_registry: Arc::clone(&runner_registry),
        ack_obligations: Arc::clone(&ack_obligations),
        transcript_runtime: Arc::clone(&transcript_runtime),
        transcript_manager: Arc::clone(&transcript_manager),
        generated_agent_runtime: Arc::clone(&generated_agent_runtime),
        completion_revivals: Arc::clone(&completion_revivals),
        forever_box: Arc::clone(&forever_box),
        local_exec: Arc::clone(&local_exec_extension),
        local_tool_permission: Arc::clone(&local_tool_permission_extension),
        session_handoff: session_handoff.clone(),
        trays: Arc::clone(&production_extensions.trays),
        telemetry_logs: host_telemetry.logs.clone(),
        product_analytics: host_telemetry.analytics.clone(),
        production_action_auditor: production_extensions.action_audit.clone(),
        cloud_agents: production_extensions.cloud_agents.service(),
        cloud_agent_watches: Arc::clone(&cloud_agent_watches),
        background_shell_watches: Arc::clone(&background_shell_watches),
        host_runner_composition: Arc::clone(&host_runner_composition),
        box_store_sync: box_store_sync_api.clone(),
        automations_lifecycle: Arc::clone(&automations_lifecycle_slot),
        generated_subagent_cleanup: Arc::clone(&generated_subagent_cleanup_slot),
    };
    let timeline_event_runner_deps = cross_user_runner_deps.clone();
    let timeline_event_roster = Arc::clone(&roster_emit);
    let timeline_event_wakes = transcript_manager.background_wakes();
    let timeline_event_wakes_for_sink = Arc::clone(&timeline_event_wakes);
    roster_emit.bind_timeline_wake_sink(Some(Arc::new(move |agent_id, event| {
        let agent_id = agent_id.trim().to_string();
        if agent_id.is_empty() {
            return;
        }
        let Ok(Some(summary)) = timeline_event_runner_deps
            .session_workers
            .summarize_agent_by_id(&agent_id, None)
        else {
            return;
        };
        if summary.is_group {
            return;
        }

        let entry = serde_json::json!({
            "kind": "event",
            "id": format!("event-{}", uuid::Uuid::new_v4()),
            "event": event.clone(),
            "timestampMs": started_at_ms(),
        });
        let admission = match timeline_event_runner_deps
            .session_workers
            .read_session_work_admission(&agent_id)
        {
            Ok(admission) => admission,
            Err(error) => {
                eprintln!(
                    "mahayana-host timeline_event_admission_read_failed agent={} error={error}",
                    agent_id
                );
                return;
            }
        };
        let append_result = if let Some(admission) = admission {
            let mut entry = entry;
            entry["sessionKey"] = serde_json::Value::String(admission.session_key.clone());
            timeline_event_runner_deps
                .session_workers
                .append_agent_transcript_entries_if_session_admitted(
                    &agent_id,
                    &admission.session_key,
                    &[entry],
                )
        } else {
            timeline_event_runner_deps
                .session_workers
                .append_agent_transcript_entries(&agent_id, &[entry])
        };
        if let Err(error) = append_result {
            eprintln!(
                "mahayana-host timeline_event_persist_failed agent={} error={error}",
                agent_id
            );
            return;
        }

        let already_running = timeline_event_runner_deps
            .transcript_runtime
            .is_agent_running(&agent_id)
            || !timeline_event_runner_deps
                .runner_registry
                .active_stream_ids_for_agent(&agent_id)
                .is_empty();
        if already_running {
            return;
        }
        if configured_routed_provider(&timeline_event_runner_deps.data_dir.join("settings.json"))
            .is_none()
        {
            return;
        }

        let should_spawn = {
            let mut wakes = timeline_event_wakes_for_sink
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            BackgroundWakes::enqueue(&mut wakes.pending_event_wakes, agent_id.clone(), event);
            BackgroundWakes::<serde_json::Value>::begin_revival(
                &mut wakes.reviving_event_agent_ids,
                &agent_id,
            )
        };
        if !should_spawn {
            return;
        }

        let deps = timeline_event_runner_deps.clone();
        let wakes = Arc::clone(&timeline_event_wakes_for_sink);
        let roster = Arc::clone(&timeline_event_roster);
        let worker_agent_id = agent_id.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("mahayana-timeline-event-{agent_id}"))
            .spawn(move || {
                loop {
                    let events = {
                        let mut wakes = wakes
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        BackgroundWakes::take_pending(
                            &mut wakes.pending_event_wakes,
                            &worker_agent_id,
                        )
                    };
                    if events.is_empty() {
                        break;
                    }
                    let Some(provider) =
                        configured_routed_provider(&deps.data_dir.join("settings.json"))
                    else {
                        break;
                    };
                    let prompt = build_timeline_event_wake_prompt(&events);
                    match run_local_background_revival_turn(
                        deps.clone(),
                        provider,
                        &worker_agent_id,
                        "event",
                        &prompt,
                        true,
                        "continue",
                    ) {
                        Ok(_) => {
                            let _ = roster.emit_agent_update(&worker_agent_id);
                        }
                        Err(error) => {
                            let classified_error = ProviderSessionError::Tool(error.clone());
                            let report = AgentErrorReport {
                                source: "event".into(),
                                conversation_id: worker_agent_id.clone(),
                                request_id: None,
                                error: classify_agent_error(&classified_error),
                                detail: Some(sand_error_detail(&error)),
                            };
                            if let Err(telemetry_error) = deps.telemetry_logs.report_agent_error(&report) {
                                eprintln!(
                                    "mahayana-host timeline_event_telemetry_failed agent={} error={telemetry_error}",
                                    worker_agent_id
                                );
                            }
                            let mut tray = provider_failure_tray(
                                &worker_agent_id,
                                &error,
                                started_at_ms() as i64,
                            );
                            tray.title = "Timeline event follow-up failed".into();
                            deps.trays.push_error(tray);
                        }
                    }
                }
                let mut wakes = wakes
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                BackgroundWakes::<serde_json::Value>::end_revival(
                    &mut wakes.reviving_event_agent_ids,
                    &worker_agent_id,
                );
            })
        {
            let mut wakes = timeline_event_wakes_for_sink
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            BackgroundWakes::<serde_json::Value>::end_revival(
                &mut wakes.reviving_event_agent_ids,
                &agent_id,
            );
            eprintln!(
                "mahayana-host timeline_event_worker_spawn_failed agent={} error={error}",
                agent_id
            );
        }
    })));

    let cross_user_settings_path = app_data_dir.join("settings.json");
    let remote_requested_runner_deps = cross_user_runner_deps.clone();
    let remote_requested_settings_path = cross_user_settings_path.clone();
    let run_remote_requested_turn: RemoteRequestedTurnRunner = Arc::new(
        move |agent_id, system_prompt, prompt| {
            let provider = configured_routed_provider(&remote_requested_settings_path)
                .ok_or_else(|| "no routed provider configured for shared-room turn".to_string())?;
            run_local_group_member_turn(
                remote_requested_runner_deps.clone(),
                provider,
                None,
                GroupMemberTurnRequest {
                    member: GroupMember {
                        id: agent_id.to_string(),
                        name: agent_id.to_string(),
                        description: String::new(),
                    },
                    system_prompt: system_prompt.to_string(),
                    prompt: prompt.to_string(),
                    group: GroupDescription {
                        name: "Shared room".into(),
                        description: String::new(),
                    },
                    peers: Vec::new(),
                    new_messages: Vec::new(),
                    shared_room_id: None,
                },
            )
        },
    );
    let shared_room_runner_deps = cross_user_runner_deps;
    let shared_room_settings_path = cross_user_settings_path;
    let shared_room_runtime = Arc::clone(&transcript_runtime);
    let shared_room_events = shared_room_runner_deps.events.clone();
    let shared_room_group_chat = transcript_manager.group_chat();
    let run_shared_room_turn: SharedRoomTurnRunner = Arc::new(
        move |room_agent_id, remote_executor| {
            let provider = configured_routed_provider(&shared_room_settings_path)
                .ok_or_else(|| "no routed provider configured for shared-room fanout".to_string())?;
            let deps = shared_room_runner_deps.clone();
            let member_room_id = room_agent_id.to_string();
            let executor: GroupMemberTurnExecutor = Arc::new(move |request| {
                run_local_group_member_turn(
                    deps.clone(),
                    provider,
                    Some(&member_room_id),
                    request,
                )
            });
            let epoch = shared_room_runtime.next_turn_epoch(room_agent_id);
            match shared_room_group_chat.run_group_turn(
                Arc::clone(&shared_room_runtime),
                room_agent_id,
                epoch,
                executor,
                remote_executor,
                Some(group_room_entry_observer(shared_room_events.clone())),
            )? {
                LocalGroupFanoutDisposition::NotGroup => Err(format!(
                    "shared-room relay target is not a group: {room_agent_id}"
                )),
                LocalGroupFanoutDisposition::DeferredRemote { .. } => Err(
                    "shared-room relay fanout is still missing a remote executor".to_string(),
                ),
                LocalGroupFanoutDisposition::Completed { .. } => Ok(()),
            }
        },
    );
    let cross_user = match production_extensions.start_cross_user(
        Arc::clone(&attachments_service),
        transcript_manager.shared_rooms(),
        run_remote_requested_turn,
        run_shared_room_turn,
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to initialize production CrossUserSharing: {error}");
            return;
        }
    };

    *cross_user_deletion_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Arc::downgrade(&cross_user);
    transcript_runtime.bind_shared_group_remote_executor(cross_user.remote_executor());
    transcript_runtime.bind_shared_group_room_entry_publisher(Some({
        let publisher = Arc::clone(&cross_user);
        Arc::new(move |room_id, entry| publisher.publish_room_entry_and_wait(room_id, entry))
    }));

    let host_upgrade_gateway_slot =
        Arc::new(Mutex::new(Weak::<UnifiedGatewayApi>::new()));
    let host_upgrade_automation = transcript_manager.automation_runtime();
    let host_upgrade_production_automations = Arc::clone(&automations_lifecycle_slot);
    let host_upgrade_sharing = Arc::clone(&cross_user);
    let host_upgrade_transcript = Arc::clone(&transcript_manager);
    let host_upgrade_sessions = Arc::clone(&session_workers);
    let host_upgrade_resume_gateway = Arc::clone(&host_upgrade_gateway_slot);
    let host_upgrade = match production_extensions.start_host_upgrade(
        ProductionHostUpgradePeers {
            suspend_automation_wakes: Arc::new(move || {
                host_upgrade_automation.suspend_wakes();
                if let Some(runtime) = host_upgrade_production_automations
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .upgrade()
                {
                    runtime.suspend_wakes();
                }
                Ok(())
            }),
            prepare_sharing_for_upgrade: Arc::new(move || {
                host_upgrade_sharing.prepare_for_upgrade();
            }),
            quiesce_transcript_for_upgrade: Arc::new(move || {
                let kickstart_agent_ids = host_upgrade_transcript
                    .live_running_agent_ids()
                    .into_iter()
                    .filter(|agent_id| {
                        host_upgrade_transcript.active_turn_source(agent_id).as_deref()
                            == Some("kickstart")
                    })
                    .collect::<Vec<_>>();
                host_upgrade_transcript.quiesce_for_upgrade();
                for agent_id in kickstart_agent_ids {
                    let _ = host_upgrade_sessions
                        .set_agent_introduction_pending(&agent_id, false);
                    host_upgrade_transcript.mark_upgrade_resume_pending(UpgradeResumeMarker {
                        agent_id,
                        marked_at_ms: started_at_ms() as f64,
                        source: Some("turn".into()),
                        automation_id: None,
                        automation_run_id: None,
                    });
                }
                Ok(())
            }),
            resume_interrupted_upgrade_turns: Arc::new(move || {
                let gateway = host_upgrade_resume_gateway
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .upgrade()
                    .ok_or_else(|| "Host gateway is not ready for upgrade resume".to_string())?;
                gateway.resume_interrupted_upgrade_turns()
            }),
        },
    ) {
        Ok(extension) => extension,
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to start production HostUpgrade extension: {error}");
            return;
        }
    };

    let teach_recording_slot = Arc::new(Mutex::new(None::<TeachRecordingApi>));
    let device_pairing_db_path = app_data_dir.join("device-pairing.db");
    let device_pairing = Arc::new(
        DevicePairingOwner::new(&device_pairing_db_path)
            .unwrap_or_else(|_| DevicePairingOwner::in_memory().expect("in-memory device pairing db"))
    );
    let node_host_runtime = Arc::new(NodeHostRuntimeState::new());
    let local_available_commands = [NODE_SYSTEM_RUN_COMMAND.to_string()]
        .into_iter()
        .collect::<HashSet<_>>();
    let local_manifest = NodeHostManifest::build(
        &local_available_commands,
        false,
        false,
        None,
    );
    let mut local_capabilities = HOST_CAPABILITIES
        .iter()
        .map(|capability| (*capability).to_string())
        .collect::<Vec<_>>();
    local_capabilities.extend(local_manifest.capabilities.iter().cloned());
    let local_session = node_host_runtime.connect(NodeHostSessionMetadata {
        node_id: "sand-local-node".into(),
        display_name: "Local Host Node".into(),
        platform: std::env::consts::OS.into(),
        device_family: std::env::consts::OS.into(),
        // Advertise only the command with a real production invoke branch.
        // Canvas/Computer command names remain unavailable until their
        // existing owners are wired through this node boundary.
        commands: local_manifest.advertised_commands,
        capabilities: local_capabilities,
        paired: true,
        gateway_local: true,
    });
    if !node_host_runtime.set_active(&local_session, true) {
        host_lifecycle.fail();
        eprintln!("failed to activate local NodeHost session during startup");
        return;
    }

    let gateway_api = Arc::new(UnifiedGatewayApi {
            host_tx: host_tx.clone(),
            auth: Arc::clone(&production_extensions.auth),
            experiments: Arc::clone(&production_extensions.experiments),
            settings: Arc::clone(&settings_extension),
            inference: Arc::clone(&production_extensions.inference),
            content_search: Arc::clone(&content_search_extension),
            events: gateway_events.clone(),
            routed_tool_relay: Arc::clone(&routed_tool_relay),
            mcp_lifecycle_relay: Arc::clone(&mcp_lifecycle_relay),
            mcp_service: Arc::clone(&mcp_service),
            data_dir: app_data_dir.clone(),
            request_context: Arc::clone(&runner_request_context),
            session_workers: Arc::clone(&session_workers),
            runner_registry: Arc::clone(&runner_registry),
            ack_obligations: Arc::clone(&ack_obligations),
            agent_deletion_runtime,
            forever_box: Arc::clone(&forever_box),
            teach_recording: Arc::clone(&teach_recording_slot),
            local_exec: Arc::clone(&local_exec_extension),
            session_handoff: session_handoff.clone(),
            webauthn_proxy: Arc::clone(&production_extensions.webauthn_proxy),
            trays: Arc::clone(&production_extensions.trays),
            transcript_runtime: Arc::clone(&transcript_runtime),
            generated_agent_runtime: Arc::clone(&generated_agent_runtime),
            completion_revivals: Arc::clone(&completion_revivals),
            transcript_manager: Arc::clone(&transcript_manager),
            roster_emit: Arc::clone(&roster_emit),
            background_wakes: Arc::clone(&timeline_event_wakes),
            channel_delivery: production_extensions.channel_delivery.clone(),
            telemetry_logs: host_telemetry.logs.clone(),
            product_analytics: host_telemetry.analytics.clone(),
            telemetry_api: host_telemetry.api(),
            production_action_auditor: production_extensions.action_audit.clone(),
            cloud_agents: production_extensions.cloud_agents.service(),
            cloud_workers: production_extensions.cloud_workers.clone(),
            plugin_capability_catalog: Arc::clone(&production_extensions.plugin_capability_catalog),
            cloud_agent_watches: Arc::clone(&cloud_agent_watches),
            background_shell_watches: Arc::clone(&background_shell_watches),
            secrets: Arc::clone(&secrets_extension),
            local_tool_permission: Arc::clone(&local_tool_permission_extension),
            auto_review: Arc::clone(&auto_review_extension),
            host_runner_composition: Arc::clone(&host_runner_composition),
            box_store_sync: box_store_sync_api.clone(),
            roster_bookkeeping: Arc::clone(&roster_bookkeeping),
            cross_user: Arc::clone(&cross_user),
            host_upgrade: Arc::clone(&host_upgrade),
            automations_lifecycle: Arc::clone(&automations_lifecycle_slot),
            create_agent_nonces: Mutex::new(CreateAgentNonceLedger::default()),
            last_busy_at_ms: Mutex::new(gateway_started_at),
            device_pairing,
            node_host_runtime,
            node_host_local_session: local_session,
            generated_subagent_cleanup: Arc::clone(&generated_subagent_cleanup_slot),
        });
    let generated_subagent_cleanup_gateway = Arc::downgrade(&gateway_api);
    *generated_subagent_cleanup_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(move |agent_id, revision| {
            let gateway = generated_subagent_cleanup_gateway
                .upgrade()
                .ok_or_else(|| "Host Gateway is no longer current".to_string())?;
            gateway.delete_generated_subagent(agent_id, revision)
        }));
    *host_upgrade_gateway_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::downgrade(&gateway_api);
    *cloud_agent_completion_gateway_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::downgrade(&gateway_api);

    let mcp_auth_gateway = Arc::downgrade(&gateway_api);
    let _mcp_auth_unsubscribe = mcp_service.subscribe_to_auth_completion(Arc::new(move |event| {
        if event
            .get("outcome")
            .or_else(|| event.get("status"))
            .and_then(serde_json::Value::as_str)
            == Some("cancelled")
        {
            return;
        }
        let requesting_agent_id = event
            .get("requestingAgentId")
            .or_else(|| event.get("requesting_agent_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let server_name = event
            .get("serverName")
            .or_else(|| event.get("server_name"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let account_key = event
            .get("accountKey")
            .or_else(|| event.get("account_key"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("default");
        let server_id = event
            .get("serverId")
            .or_else(|| event.get("server_id"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !server_id.is_empty() {
            if let Some(gateway) = mcp_auth_gateway.upgrade() {
                gateway
                    .mcp_service
                    .note_auth_completed_elsewhere(server_id, account_key);
            }
        }
        let (Some(agent_id), Some(server_name)) = (requesting_agent_id, server_name) else {
            return;
        };
        let Some(gateway) = mcp_auth_gateway.upgrade() else {
            return;
        };
        if let Err(error) = gateway.resume_after_mcp_auth(agent_id, server_name, account_key) {
            eprintln!(
                "mahayana-host mcp_auth_resume_failed agent={agent_id} error={error}"
            );
        }
    }));

    // Frozen TranscriptManager owns pending-wake rearm. Shipping main only
    // supplies the process adapter after Gateway/Runner resources are live.
    transcript_manager
        .bind_pending_wake_runtime(Arc::new(ProductionPendingWakeRuntime {
            gateway: Arc::clone(&cloud_agent_completion_gateway_slot),
            cloud_agent_watches: Arc::clone(&cloud_agent_watches),
            background_shell_watches: Arc::clone(&background_shell_watches),
            completion_revivals: Arc::clone(&completion_revivals),
        }))
        .expect("TranscriptManager pending-wake runtime must be configured exactly once");
    if let Err(error) = transcript_manager.rearm_pending_wakes() {
        host_lifecycle.fail();
        eprintln!("failed to rearm Transcript pending wakes: {error}");
        return;
    }

    let listener_runtime = transcript_manager.automation_runtime();
    let listener_lifecycle_slot = Arc::clone(&automations_lifecycle_slot);
    let production_listeners = Arc::new(move || {
        let Ok(entries) = listener_runtime.list_all_automation_definitions() else {
            return (Vec::new(), Vec::new());
        };
        let lifecycle = listener_lifecycle_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .upgrade();
        let mut slack = Vec::new();
        let mut github = Vec::new();
        for entry in entries.into_iter().filter(|entry| entry.automation.is_enabled) {
            let cloud = ScheduledCloudAutomation {
                id: entry.automation.id.clone(),
                name: entry.automation.name.clone(),
                prompt: entry.automation.prompt.clone(),
                is_enabled: entry.automation.is_enabled,
                trigger: entry.automation.trigger.clone(),
            };
            let should_schedule_locally = lifecycle
                .as_ref()
                .map(|owner| owner.should_schedule_locally(&entry.agent_id, &cloud))
                .unwrap_or_else(|| !is_server_schedulable(&entry.automation.trigger));
            if !should_schedule_locally {
                continue;
            }
            for listener in trigger_members(&entry.automation.trigger) {
                match listener.get("type").and_then(serde_json::Value::as_str) {
                    Some("slack") => slack.push(listener),
                    Some("github") => github.push(listener),
                    _ => {}
                }
            }
        }
        (slack, github)
    });
    let listener_gateway = Arc::clone(&gateway_api);
    let relay_sink = Arc::new(move |event: serde_json::Value| {
        listener_gateway.dispatch_production_listener_event(event)
    });
    let fire_gateway = Arc::clone(&gateway_api);
    let fire_dispatch = Arc::new(move |fire, completion| {
        fire_gateway.dispatch_production_backend_fire(fire, completion)
    });
    let resume_gateway = Arc::clone(&gateway_api);
    let on_listener_connected = Arc::new(move |agent_id: &str, platform: &str| {
        if let Err(error) = resume_gateway.resume_after_listener_connect(agent_id, platform) {
            eprintln!(
                "[sand:automations] listener reconnect resume failed agent={agent_id} platform={platform}: {error}"
            );
        }
    });
    let listener_session_workers = Arc::clone(&session_workers);
    let listener_agent_channels = Arc::new(move |agent_id: &str| {
        listener_session_workers
            .list_agent_channels(agent_id)?
            .into_iter()
            .map(|connection| serde_json::to_value(connection).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()
    });
    let cloud_runtime = transcript_manager.automation_runtime();
    let cloud_definitions = Arc::new(move || {
        cloud_runtime
            .list_all_automation_definitions()
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| {
                        (
                            entry.agent_id,
                            ScheduledCloudAutomation {
                                id: entry.automation.id,
                                name: entry.automation.name,
                                prompt: entry.automation.prompt,
                                is_enabled: entry.automation.is_enabled,
                                trigger: entry.automation.trigger,
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            })
    });
    let cloud_agent_sessions = Arc::clone(&session_workers);
    let cloud_agent_ids = Arc::new(move || cloud_agent_sessions.list_agent_record_ids());
    let cloud_time_zone_sessions = Arc::clone(&session_workers);
    let cloud_time_zone = Arc::new(move || cloud_time_zone_sessions.resolve_user_time_zone());

    let automations_lifecycle = match production_extensions.start_automations(
        production_listeners,
        relay_sink,
        fire_dispatch,
        on_listener_connected,
        listener_agent_channels,
        cloud_definitions,
        cloud_agent_ids,
        cloud_time_zone,
        Arc::new(|message| eprintln!("{message}")),
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to start production Automations extension: {error}");
            return;
        }
    };
    *gateway_api
        .automations_lifecycle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Arc::downgrade(&automations_lifecycle);
    gateway_api.refresh_production_automations();

    let teach_gateway = Arc::downgrade(&gateway_api);
    let teach_recording_api = match production_extensions.start_teach_recording(
        Arc::clone(&forever_box),
        Arc::clone(&session_workers),
        Arc::new(move |agent_id, content, client_nonce, rich_text| {
            let gateway = teach_gateway
                .upgrade()
                .ok_or_else(|| "teach-recording gateway is unavailable".to_string())?;
            let mut args = serde_json::json!({
                "agentId": agent_id,
                "prompt": content,
                "clientNonce": client_nonce,
                "directAddressedAcceptance": true,
                "awaitTurn": false,
            });
            if let Some(rich_text) = rich_text {
                args["richText"] = serde_json::Value::String(rich_text.to_string());
            }
            gateway
                .call("sendPrompt", args)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }),
    ) {
        Ok(api) => api,
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to start production TeachRecording extension: {error}");
            return;
        }
    };
    *teach_recording_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(teach_recording_api);

    let gateway_server = match start_gateway_server(GatewayServerDeps {
        api: gateway_api.clone(),
        events: gateway_events.clone(),
        local_exec: Some(local_exec_extension.gateway_bridge()),
        webauthn: Some(production_extensions.webauthn_proxy.gateway_bridge()),
        config: gateway_config.clone(),
        started_at: gateway_started_at,
    }) {
        Ok(server) => server,
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to start Mahayana Host gateway: {error}");
            return;
        }
    };

    let ack_redrive_stop = Arc::new(AtomicBool::new(false));

    let gateway_discovery_path = get_gateway_discovery_path();
    let gateway_discovery = GatewayDiscoveryInfo {
        port: gateway_server.port(),
        pid: std::process::id(),
        started_at: gateway_started_at,
        scheme: Some(gateway_scheme(&gateway_config).to_string()),
        host: Some(gateway_config.host.clone()),
        token: gateway_config.auth_token.clone(),
    };
    if let Err(error) = write_gateway_discovery(&gateway_discovery, &gateway_discovery_path) {
        host_lifecycle.fail();
        eprintln!(
            "failed to publish Mahayana Host gateway discovery at {}: {error}",
            gateway_discovery_path.display()
        );
        return;
    }

    let ready_boot_id = std::env::var("SAND_BOX_BOOT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let ready_boot_started_at_ms = std::env::var("SAND_BOX_BOOT_STARTED_AT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    let ready_marker = fs::read_to_string(BOX_READY_STAGE_MARKER_PATH).ok();
    if should_report_box_ready(
        ready_boot_id.as_deref(),
        ready_boot_started_at_ms,
        ready_marker.as_deref(),
    ) {
        if let (Some(boot_id), Some(boot_started_at_ms)) =
            (ready_boot_id, ready_boot_started_at_ms)
        {
            let ready_logs = host_telemetry.logs.clone();
            let _box_ready_worker = thread::spawn(move || {
                let duration_ms = box_ready_duration_ms(started_at_ms(), boot_started_at_ms);
                for attempt in 1..=BOX_READY_REPORT_ATTEMPTS {
                    if ready_logs.report_box_boot_stage_confirmed("ready", duration_ms) {
                        let _ = fs::write(BOX_READY_STAGE_MARKER_PATH, &boot_id);
                        return;
                    }
                    if attempt < BOX_READY_REPORT_ATTEMPTS {
                        thread::sleep(Duration::from_millis(BOX_READY_REPORT_RETRY_MS));
                    }
                }
            });
        }
    }

    let shutdown_complete = Arc::new(AtomicBool::new(false));
    let _shutdown_signal_worker = match install_shutdown_signal_worker(
        host_tx.clone(),
        Arc::clone(&shutdown_complete),
    ) {
        Ok(worker) => worker,
        Err(error) => {
            host_lifecycle.fail();
            eprintln!("failed to install Mahayana Host shutdown signal handlers: {error}");
            return;
        }
    };

    production_extensions.notify_bus.mark_background_work_ready();
    if let Err(error) = production_extensions.start_cross_user_background_work() {
        eprintln!("[sand-host] CrossUserSharing background work failed: {error}");
    }
    let _ = local_tool_permission_extension.background_work_ready();

    if let Err(error) = host_upgrade.service().resume_interrupted_upgrade_turns() {
        eprintln!("mahayana-host upgrade_resume_boot_failed error={error}");
    }
    let _ack_redrive_worker = match start_ack_redrive_worker(
        Arc::clone(&gateway_api),
        Arc::clone(&ack_obligations),
        Arc::clone(&session_workers),
        Arc::clone(&runner_registry),
        gateway_events.clone(),
        Arc::clone(&ack_redrive_stop),
    ) {
        Ok(worker) => Some(worker),
        Err(error) => {
            eprintln!("failed to start Mahayana ack-redrive worker: {error}");
            None
        }
    };

    let host_bundle_version = host_upgrade
        .service()
        .resolve_host_bundle_identity_version(Some(env!("CARGO_PKG_VERSION")));
    let _ = host_telemetry.logs.report_host_startup(BTreeMap::from([
        (
            "auto_update".into(),
            forever_box.is_auto_update_enabled().to_string(),
        ),
        (
            "duration_ms".into(),
            started_at_ms().saturating_sub(host_started_at_ms).to_string(),
        ),
        ("host_bundle_version".into(), host_bundle_version),
    ]));

    if let Err(error) = host_lifecycle.complete(HostLifecycleCompletion {
        phase: "ready".into(),
        plugin_count: None,
        entry_count: None,
    }) {
        host_lifecycle.fail();
        eprintln!("failed to settle Mahayana Host ready lifecycle phase: {error}");
        return;
    }

    // Runtime events travel as unsolicited JSON frames. The event worker blocks
    // in Rust instead of issuing 500 ms JSON-RPC receive requests from Electron.
    // Test mode has a non-blocking deterministic backend, so a small sleep keeps
    // that lane from spinning while CI is idle.
    let event_source = host.feature_event_source();
    let event_stdout = Arc::clone(&stdout);
    let event_gateway = gateway_events.clone();
    let _event_worker = thread::spawn(move || loop {
        match event_source.receive(Duration::from_secs(30)) {
            Ok(Some(event)) => {
                let event = event_source.project_grok_gateway_event(&event);
                if write_runtime_event(&event_stdout, &event_gateway, event).is_err() {
                    break;
                }
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                eprintln!("Mahayana runtime event stream failed: {error}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    });

    let platform_stdout = Arc::clone(&stdout);
    let (platform_tx, platform_rx) = mpsc::channel::<String>();
    let _platform_worker = thread::spawn(move || {
        while let Ok(line) = platform_rx.recv() {
            let response = platform_host.dispatch_json(&line);
            if write_response(&platform_stdout, &response).is_err() {
                break;
            }
        }
    });

    // Read stdin on a lightweight transport thread. The UnifiedAppHost itself
    // stays on this owner thread so QuickJS and the rest of the Host runtime
    // never cross a Send/Sync boundary. Gateway requests join the same serial
    // lane through HostLaneRequest.
    let stdin_tx = host_tx.clone();
    let _stdin_worker = thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    eprintln!("failed to read host request: {error}");
                    break;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            if stdin_tx.send(HostLaneRequest::Stdin(line)).is_err() {
                return;
            }
        }
        let _ = stdin_tx.send(HostLaneRequest::StdinClosed);
    });
    drop(host_tx);

    while let Ok(request) = host_rx.recv() {
        match request {
            HostLaneRequest::Gateway { method, args, reply } => {
                let _ = reply.send(dispatch_gateway_call(&host, &forever_box, &attachments_service, &method, args));
            }
            HostLaneRequest::StdinClosed => break,
            HostLaneRequest::Stdin(line) => {
                if is_platform_request_json(&line) {
                    if platform_tx.send(line).is_err() {
                        break;
                    }
                    continue;
                }
                let response = dispatch_json(&host, &line);
                if write_response(&stdout, &response).is_err() {
                    break;
                }
                // Commands may enqueue product-local events that are not backed
                // by the model runtime receiver. Drain those immediately so
                // they are pushed in the same turn.
                if drain_ready_runtime_events(&host, &stdout, &gateway_events).is_err() {
                    break;
                }
            }
        }
    }
    drop(platform_tx);
    if let Some(supervisor) = network_enrollment_supervisor.as_mut() {
        supervisor.stop();
    }
    ack_redrive_stop.store(true, Ordering::Release);
    drop(gateway_server);
    runner_registry.cancel_all("Mahayana Host shutting down");
    host_runner_composition.dispose();
    routed_tool_relay.cancel_all("Mahayana Host shutting down");
    mcp_lifecycle_relay.cancel_all("Mahayana Host shutting down");
    if let Err(error) = production_extensions.stop_box_store_sync() {
        eprintln!("failed to stop production BoxStoreSync extension cleanly: {error}");
    }
    if let Err(error) = production_extensions.stop_mcp() {
        eprintln!("failed to stop production MCP extension cleanly: {error}");
    }
    if let Err(error) = production_extensions.stop_automations() {
        eprintln!("failed to stop production Automations extension cleanly: {error}");
    }
    if let Err(error) = production_extensions.stop_teach_recording() {
        eprintln!("failed to stop production TeachRecording extension cleanly: {error}");
    }
    if let Err(error) = production_extensions.stop_host_upgrade() {
        eprintln!("failed to stop production HostUpgrade extension cleanly: {error}");
    }
    if let Err(error) = production_extensions.stop_cross_user() {
        eprintln!("failed to stop production CrossUserSharing extension cleanly: {error}");
    }
    if let Err(error) = production_extensions.stop_notifications() {
        eprintln!("failed to stop production Notifications extension cleanly: {error}");
    }
    production_extensions.notify_bus.stop();
    if let Err(error) = production_extensions.stop_auto_review() {
        eprintln!("failed to stop production AutoReview extension cleanly: {error}");
    }
    if let Err(error) = production_extensions.stop_transcript() {
        eprintln!("failed to stop production Transcript extension cleanly: {error}");
    }
    if let Err(error) = production_extensions.shutdown_session() {
        eprintln!("failed to shut down production Session extension cleanly: {error}");
    }
    box_extensions.stop();
    // CloudAgents and Telemetry both depend on earlier production extensions.
    // Settle them while Auth / Experiments / Inference are still live.
    production_extensions.stop_cloud_agents();
    production_extensions.stop_structured_log_domain_reporters();
    production_extensions.telemetry.dispose();
    if let Some(daemon) = box_exec_daemon.as_mut() {
        if let Err(error) = daemon.close() {
            eprintln!("failed to stop managed Grok box exec-daemon cleanly: {error}");
        }
    }
    if let Err(error) = clear_gateway_discovery(&gateway_discovery_path) {
        eprintln!(
            "failed to clear Mahayana Host gateway discovery at {}: {error}",
            gateway_discovery_path.display()
        );
    }
    if let Err(error) = host_lock.release() {
        eprintln!(
            "failed to release Mahayana Host lock at {}: {error}",
            host_lock_path.display()
        );
    }
    shutdown_complete.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::{
        BOX_APPLY_ENVIRONMENT_GATEWAY_METHOD, ProductionBrowserUaLog,
        ProductionHostExtensions, ProductionRunnerRequestContextSource, UnifiedGatewayApi,
        decode_provider_messages,
        dispatch_box_environment_call, ensure_managed_runtime_layout, is_platform_request_json,
        AutomationExecutionResult, automation_fire_completion, automation_terminal_from_event,
        listener_connect_resume_args, starts_new_local_tool_direction,
        project_forever_box_status, reaction_gateway_args,
    };
    use mahayana_host_runtime::extensions::forever_box::BoxStatus;
    use mahayana_host_runtime::extensions::session::box_handoff_service::PendingHandoff;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn local_tool_direction_advances_only_for_new_direct_user_turns() {
        assert!(starts_new_local_tool_direction(
            None, false, false, false, false, false
        ));
        assert!(starts_new_local_tool_direction(
            Some("turn"), false, false, false, false, false
        ));
        for source in ["automation", "background-revival", "handoff-resume", "subagent"] {
            assert!(!starts_new_local_tool_direction(
                Some(source), false, false, false, false, false
            ));
        }
        assert!(!starts_new_local_tool_direction(
            Some("turn"), true, false, false, false, false
        ));
        assert!(!starts_new_local_tool_direction(
            Some("turn"), false, true, false, false, false
        ));
        assert!(!starts_new_local_tool_direction(
            Some("turn"), false, false, true, false, false
        ));
        assert!(!starts_new_local_tool_direction(
            Some("turn"), false, false, false, true, false
        ));
        assert!(!starts_new_local_tool_direction(
            Some("turn"), false, false, false, false, true
        ));
    }

    #[test]
    fn production_browser_ua_log_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProductionBrowserUaLog>();
    }

    #[test]
    fn shipping_production_extension_graph_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProductionHostExtensions>();
        assert_send_sync::<ProductionSessionWorkers>();
        assert_send_sync::<TranscriptRunnerRegistry>();
    }

    #[test]
    fn gateway_proxy_is_send_sync_without_moving_the_unified_host() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<UnifiedGatewayApi>();
        assert_send_sync::<ProductionRunnerRequestContextSource>();
    }

    #[test]
    fn routes_platform_http_away_from_the_feature_event_lane() {
        assert!(is_platform_request_json(
            r#"{"id":1,"method":"platform.request","params":{}}"#,
        ));
        assert!(!is_platform_request_json(
            r#"{"id":2,"method":"feature.receive","params":{"timeoutMs":500}}"#,
        ));
        assert!(!is_platform_request_json(
            r#"{"id":3,"method":"feature.execute","params":{}}"#,
        ));
    }



    #[test]
    fn listener_integration_status_errors_degrade_to_disconnected() {
        assert!(listener_connection_state_or_disconnected(
            "github",
            Ok(true)
        ));
        assert!(!listener_connection_state_or_disconnected(
            "slack",
            Err("temporary auth failure".into())
        ));
    }

    #[test]
    fn listener_gateway_methods_are_owned_by_production_automations_lifecycle() {
        let source = include_str!("main.rs");
        assert!(source.contains("if method == \"getListenerIntegrations\""));
        assert!(source.contains("if method == \"getListenerConnectUrl\""));
        assert!(source.contains("lifecycle.is_platform_connected(platform)"));
        assert!(source.contains("lifecycle.get_connect_url(platform)"));
        assert!(
            !source.contains("watch_listener_connection(entry.agent_id.clone(), platform)"),
            "existing routines must not implicitly arm the interactive connect watcher"
        );
    }

    #[test]
    fn listener_connect_resume_is_hidden_handoff_with_stable_nonce() {
        let args = listener_connect_resume_args("agent-1", "slack", "nonce-123");
        assert_eq!(args["agentId"], "agent-1");
        assert_eq!(args["clientNonce"], "nonce-123");
        assert_eq!(args["requestSource"], "handoff-resume");
        assert_eq!(args["appendUserMessage"], false);
        assert_eq!(args["hidden"], true);
        assert!(
            args["prompt"]
                .as_str()
                .expect("resume prompt")
                .contains("bot is invited")
        );
    }

    #[test]
    fn backend_fire_completion_preserves_retryable_none_and_terminal_outcomes() {
        assert_eq!(automation_fire_completion(Ok(None)), None);
        assert_eq!(
            automation_fire_completion(Ok(Some(
                mahayana_host_runtime::extensions::transcript::automation_run_path::FireAutomationOutcome::Ok
            )))
            .expect("success completion")
            .status,
            "succeeded"
        );
        assert_eq!(
            automation_fire_completion(Err("runner failed".into()))
                .expect("failed completion")
                .status,
            "failed"
        );
    }

    #[test]
    fn runner_provider_gateway_rejects_empty_message_batches() {
        assert!(decode_provider_messages(&serde_json::json!({"messages": []})).is_err());
        let messages = decode_provider_messages(&serde_json::json!({
            "messages": [{"role":"user","content":"hello"}]
        }))
        .expect("provider messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
    }

    #[test]
    fn automation_terminal_event_maps_runner_settlement_without_guessing() {
        let completed = serde_json::json!({
            "channel": RUNNER_INFERENCE_EVENT_CHANNEL,
            "payload": {"streamId":"automation-1","type":"completed","content":""}
        });
        assert_eq!(
            automation_terminal_from_event(&completed, "automation-1"),
            Some(Ok(AutomationExecutionResult::Completed))
        );

        let cancelled = serde_json::json!({
            "channel": RUNNER_INFERENCE_EVENT_CHANNEL,
            "payload": {"streamId":"automation-1","type":"cancelled","message":"stop"}
        });
        assert_eq!(
            automation_terminal_from_event(&cancelled, "automation-1"),
            Some(Ok(AutomationExecutionResult::Interrupted {
                detail: "Interrupted before it finished.".into(),
                quiesced_for_upgrade: false,
            }))
        );

        let failed = serde_json::json!({
            "channel": RUNNER_INFERENCE_EVENT_CHANNEL,
            "payload": {"streamId":"automation-1","type":"failed","message":"provider failed"}
        });
        assert_eq!(
            automation_terminal_from_event(&failed, "automation-1"),
            Some(Ok(AutomationExecutionResult::Failed {
                detail: "provider failed".into(),
            }))
        );

        let other = serde_json::json!({
            "channel": RUNNER_INFERENCE_EVENT_CHANNEL,
            "payload": {"streamId":"automation-2","type":"completed"}
        });
        assert_eq!(automation_terminal_from_event(&other, "automation-1"), None);
    }

    #[test]
    fn reaction_sink_uses_authoritative_host_gateway_payload() {
        assert_eq!(
            reaction_gateway_args("agent-a", "t3u", "👍"),
            serde_json::json!({
                "agentId": "agent-a",
                "entryId": "t3u",
                "emoji": "👍"
            })
        );
    }

    #[test]
    fn forever_box_status_projection_carries_pending_handoff_for_renderer() {
        let status = BoxStatus {
            agent_id: "agent-a".into(),
            state: "running".into(),
            vnc_url: Some("http://127.0.0.1/vnc.html".into()),
            windows: None,
            image_update_available: Some(true),
            pull_percent: None,
        };
        let handoff = PendingHandoff {
            request_id: "request-a".into(),
            instruction: "Sign in".into(),
            snapshot_data_url: Some("data:image/webp;base64,YWJj".into()),
        };
        let projected = project_forever_box_status(&status, Some(&handoff));
        assert_eq!(projected["agentId"], "agent-a");
        assert_eq!(projected["handoff"]["requestId"], "request-a");
        assert_eq!(
            projected["handoff"]["snapshotDataUrl"],
            "data:image/webp;base64,YWJj"
        );
    }

    #[test]
    fn shipping_gateway_routes_box_environment_to_production_box_owner() {
        let args = serde_json::json!({
            "env": { "FABUSHI_AGENT": "enabled", "SHELL": "/bin/zsh" },
            "replace": true
        });
        let mut observed = None;
        let result = dispatch_box_environment_call(
            BOX_APPLY_ENVIRONMENT_GATEWAY_METHOD,
            &args,
            |update| {
                observed = Some(update.clone());
                Ok(())
            },
        )
        .expect("box route should be owned by production Host")
        .expect("box route should succeed");

        assert_eq!(result, serde_json::json!({ "applied": true }));
        let observed = observed.expect("production box update");
        assert_eq!(observed.env["FABUSHI_AGENT"], "enabled");
        assert_eq!(observed.env["SHELL"], "/bin/zsh");
        assert!(observed.replace);
    }

    #[test]
    fn creates_product_owned_fallback_workspace_before_host_startup() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "fabushi-mahayana-desktop-layout-{}-{suffix}",
            std::process::id()
        ));

        ensure_managed_runtime_layout(&root).expect("initialize managed runtime layout");
        assert!(root.join("feature-host/runtime/workspace").is_dir());

        fs::remove_dir_all(root).expect("remove test layout");
    }
}
