use mahayana_app_host::{AppHostFeatureMode, HostResponse, default_app_data_dir};
use mahayana_unified_app_host::{UnifiedAppHost, dispatch_json as dispatch_unified_json};
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use sha2::{Digest, Sha256};
use uuid::Uuid;
#[path = "../../../../internal/host-extensions.rs"]
mod host_extensions;
#[path = "../../../../internal/scheduling.rs"]
mod scheduling;

#[path = "../../../utils/workspace-paths.rs"]
mod package_utils_workspace_paths;
#[path = "../../../utils/repo-url.rs"]
mod package_utils_repo_url;
#[path = "../../../utils/web-search-year-guidance.rs"]
mod package_utils_web_search_year_guidance;
#[path = "../../../utils/command-glob.rs"]
mod package_utils_command_glob;
#[path = "../../../utils/model-utils.rs"]
mod package_utils_model_utils;
#[path = "../../../utils/disposable.rs"]
mod package_utils_disposable;
#[path = "../../../utils/attempt.rs"]
mod package_utils_attempt;
#[path = "../../../utils/canvas-path.rs"]
mod package_utils_canvas_path;
#[path = "../../../utils/ttl-cache.rs"]
mod package_utils_ttl_cache;
#[path = "../../../utils/admin-command-denylist.rs"]
mod package_utils_admin_command_denylist;
#[path = "../../../utils/path-matchers.rs"]
mod package_utils_path_matchers;
#[path = "../../../utils/path-utils.rs"]
mod package_utils_path_utils;
#[path = "../../../utils/git-provider-url.rs"]
mod package_utils_git_provider_url;
#[path = "../../../utils/local-pr-creation-forge.rs"]
mod package_utils_local_pr_creation_forge;
#[path = "../../../utils/encoding-browser.rs"]
mod package_utils_encoding_browser;
#[path = "../../../constants/composer.rs"]
mod package_constants_composer;
#[path = "../../../constants/structured-log.rs"]
mod package_constants_structured_log;
#[path = "../../../constants/project-send-message.rs"]
mod package_constants_project_send_message;
#[path = "../../../constants/smart-mode-classifier.rs"]
mod package_constants_smart_mode_classifier;
#[path = "../../../constants/system-notification.rs"]
mod package_constants_system_notification;
#[path = "../../../constants/environment-setup.rs"]
mod package_constants_environment_setup;
#[path = "../../../constants/project-conversation.rs"]
mod package_constants_project_conversation;
#[path = "../../../constants/cloud-agent.rs"]
mod package_constants_cloud_agent;
#[path = "../../../constants/auto-spillover-ui.rs"]
mod package_constants_auto_spillover_ui;
#[path = "../../../constants/git-diff.rs"]
mod package_constants_git_diff;
#[path = "../../../constants/sand-box-archive.rs"]
mod package_constants_sand_box_archive;
#[path = "../../../constants/ask-question.rs"]
mod package_constants_ask_question;
#[path = "../../../constants/mcp.rs"]
mod package_constants_mcp;
#[path = "../../../constants/permissions.rs"]
mod package_constants_permissions;
#[path = "../../../constants/sand-box.rs"]
mod package_constants_sand_box;
#[path = "../../../constants/sand-supervisor.rs"]
mod package_constants_sand_supervisor;
#[path = "../../../constants/agent-store-ids.rs"]
mod package_constants_agent_store_ids;
#[path = "../../../constants/repo-label.rs"]
mod package_constants_repo_label;
#[path = "../../../agent-core/conversation-actions/context-injection.rs"]
mod package_agent_core_context_injection;
#[path = "../../../agent-core/conversation-actions/controlled.rs"]
mod package_agent_core_conversation_actions_controlled;
#[path = "../../../agent-core/conversation-actions/receiver-contract.rs"]
mod package_agent_core_conversation_actions_receiver_contract;
#[path = "../../../agent-core/conversation-actions/remote.rs"]
mod package_agent_core_conversation_actions_remote;
#[path = "../../../agent-core/conversation-actions/steer-outbox.rs"]
mod package_agent_core_conversation_actions_steer_outbox;
#[path = "../../../agent-core/goal-pursuit-guidelines.rs"]
mod package_agent_core_goal_pursuit_guidelines;
#[path = "../../../agent-core/domain-utils.rs"]
mod package_agent_core_domain_utils;
#[path = "../../../agent-core/goal-continuation.rs"]
mod package_agent_core_goal_continuation;
#[path = "../../../agent-core/mcp-auth-flow.rs"]
mod package_agent_core_mcp_auth_flow;
#[path = "../../../analytics-client/deferred-buffer.rs"]
mod package_analytics_client_deferred_buffer;
#[path = "../../../agent-exec/await-outcome.rs"]
mod package_agent_exec_await_outcome;
#[path = "../../../agent-exec/agent-skill-metadata.rs"]
mod package_agent_exec_agent_skill_metadata;
#[path = "../../../agent-exec/background-completion-dispatch.rs"]
mod package_agent_exec_background_completion_dispatch;
#[path = "../../../agent-exec/subagent-lifecycle-state-machine.rs"]
mod package_agent_exec_subagent_lifecycle_state_machine;
#[path = "../../../agent-exec/subagent-queue.rs"]
mod package_agent_exec_subagent_queue;
#[path = "../../../agent-exec/subagent-lifecycle-store.rs"]
mod package_agent_exec_subagent_lifecycle_store;
#[path = "../../../agent-store-sync/etag.rs"]
mod package_agent_store_sync_etag;
#[path = "../../../agent-store-sync/conflict-notice-claim-state.rs"]
mod package_agent_store_sync_conflict_notice_claim_state;
#[path = "../../../agent-store-sync/validation.rs"]
mod package_agent_store_sync_validation;
#[path = "../../../agent-exec/background-work-metadata.rs"]
mod package_agent_exec_background_work_metadata;
#[path = "../../../agent-exec/common.rs"]
mod package_agent_exec_common;
#[path = "../../../agent-exec/exec-error.rs"]
mod package_agent_exec_exec_error;
#[path = "../../../agent-exec/execution-timing.rs"]
mod package_agent_exec_execution_timing;
#[path = "../../../agent-exec/request-context-parts.rs"]
mod package_agent_exec_request_context_parts;
#[path = "../../../agent-transcript/context-stripping.rs"]
mod package_agent_transcript_context_stripping;
#[path = "../../../agent-transcript/paths.rs"]
mod package_agent_transcript_paths;
#[path = "../../../agent-kv/serde.rs"]
mod package_agent_kv_serde;
#[path = "../../../agent-kv/blob-not-found-error.rs"]
mod package_agent_kv_blob_not_found_error;
#[path = "../../../agent-kv/blob-store.rs"]
mod package_agent_kv_blob_store;
#[path = "../../../redaction/privacy-mode.rs"]
mod package_redaction_privacy_mode;
#[path = "../../../redaction/classification.rs"]
mod package_redaction_classification;
#[path = "../../../redaction/privacy-context.rs"]
mod package_redaction_privacy_context;
#[path = "../../../redaction/shouldRedact.rs"]
mod package_redaction_should_redact;
#[path = "../../../agent-analytics/commit-scoring/git-repo-utils.rs"]
mod package_agent_analytics_git_repo_utils;
#[path = "../../../mcp-agent-exec/mcp.rs"]
mod package_mcp_agent_exec_mcp;
#[path = "../../../git-core/process-env.rs"]
mod package_git_core_process_env;
#[path = "../../../git-core/redaction.rs"]
mod package_git_core_redaction;
#[path = "../../../git-core/diagnostics.rs"]
mod package_git_core_diagnostics;

#[path = "../../../cursor-plugins/snapshot-state.rs"]
mod package_cursor_plugins_snapshot_state;
#[path = "../../../local-exec/pi/truncate.rs"]
mod package_local_exec_pi_truncate;
#[path = "../../../hooks-carriers/limits.rs"]
mod package_hooks_carriers_limits;
#[path = "../../../hooks-carriers/errors.rs"]
mod package_hooks_carriers_errors;
#[path = "../../../hooks-exec/hook-error-handling.rs"]
mod package_hooks_exec_hook_error_handling;
#[path = "../../../mcp-core/config/mcp-focus-retry-cooldown.rs"]
mod package_mcp_core_focus_retry_cooldown;
#[path = "../../../mcp-core/config/mcp-fsm-timing-config.rs"]
mod package_mcp_core_fsm_timing_config;
#[path = "../../../mcp-core/config/mcp-inline-reconnect-cooldown.rs"]
mod package_mcp_core_inline_reconnect_cooldown;
#[path = "../../../local-exec/constants.rs"]
mod package_local_exec_constants;
#[path = "../../../agent/utils/token-estimate.rs"]
mod package_agent_utils_token_estimate;
#[path = "../../../agent/utils/prompt-xml-escape.rs"]
mod package_agent_utils_prompt_xml_escape;
#[path = "../../../agent/utils/request-path.rs"]
mod package_agent_utils_request_path;
#[path = "../../../agent/tools/lenient-boolean.rs"]
mod package_agent_tools_lenient_boolean;
#[path = "../../../cursor-plugins/identifiers.rs"]
mod package_cursor_plugins_identifiers;
#[path = "../../../cursor-plugins/secret-variable-names.rs"]
mod package_cursor_plugins_secret_variable_names;
#[path = "../../../agent/tools/lenient-enum.rs"]
mod package_agent_tools_lenient_enum;
#[path = "../../../agent/context-processing-skill-id.rs"]
mod package_agent_context_processing_skill_id;
#[path = "../../../agent/utils/meta-parent-completion-protocol.rs"]
mod package_agent_utils_meta_parent_completion_protocol;
#[path = "../../../agent/utils/mcp-auth-instruction.rs"]
mod package_agent_utils_mcp_auth_instruction;
#[path = "../../../agent/prompts/anti-ask-question-copy.rs"]
mod package_agent_prompts_anti_ask_question_copy;
#[path = "../../../agent/prompts/claude-helpers.rs"]
mod package_agent_prompts_claude_helpers;
#[path = "../../../agent/prompts/cloud/no-repository-access.rs"]
mod package_agent_prompts_cloud_no_repository_access;
#[path = "../../../agent/utils/slack-sender-line.rs"]
mod package_agent_utils_slack_sender_line;
#[path = "../../../local-exec/shell-timeout.rs"]
mod package_local_exec_shell_timeout;
#[path = "../../../agent/prompts/user-info-sanitization.rs"]
mod package_agent_prompts_user_info_sanitization;
#[path = "../../../agent/context-processing-uploaded-documents.rs"]
mod package_agent_context_processing_uploaded_documents;
#[path = "../../../agent/utils/agent-mode-guidance.rs"]
mod package_agent_utils_agent_mode_guidance;
#[path = "../../../agent/tools/core/read/pdf-utils.rs"]
mod package_agent_tools_core_read_pdf_utils;
#[path = "../../../cursor-plugins/cloud-manifest.rs"]
mod package_cursor_plugins_cloud_manifest;
#[path = "../../../agent/context-processing-invocation.rs"]
mod package_agent_context_processing_invocation;
#[path = "../../../hooks-carriers/hook-additional-context-render.rs"]
mod package_hooks_carriers_hook_additional_context_render;
#[path = "../../../agent/context-processing-cursor-commands.rs"]
mod package_agent_context_processing_cursor_commands;
#[path = "../../../agent/context-processing-documentation.rs"]
mod package_agent_context_processing_documentation;

#[path = "../../../local-exec/pending-decision-provider.rs"]
mod package_local_exec_pending_decision_provider;
#[path = "../../../mcp-core/config/mcp-tool-call-timeout.rs"]
mod package_mcp_core_tool_call_timeout;
#[path = "../../../local-exec/agent-data-cleanup.rs"]
mod package_local_exec_agent_data_cleanup;
#[path = "../../../agent/prompts/user-info.rs"]
mod package_agent_prompts_user_info;
#[path = "../../../local-exec/services/cloud-rules-service.rs"]
mod package_local_exec_cloud_rules_service;
#[path = "../../../local-exec/ignore-rules.rs"]
mod package_local_exec_ignore_rules;
#[path = "../../../cursor-plugins/plugin-variables.rs"]
mod package_cursor_plugins_plugin_variables;
#[path = "../../../cursor-plugins/capabilities.rs"]
mod package_cursor_plugins_capabilities;
#[path = "../../../hooks-carriers/collect.rs"]
mod package_hooks_carriers_collect;
#[path = "../../../agent/tools/core/worktree-paths.rs"]
mod package_agent_tools_core_worktree_paths;
#[path = "../../../local-exec/team-repo-filters.rs"]
mod package_local_exec_team_repo_filters;
#[path = "../../../agent/tools/subagent-model-force-policy.rs"]
mod package_agent_tools_subagent_model_force_policy;
#[path = "../../../agent/tools/subagent-composer-model-ids.rs"]
mod package_agent_tools_subagent_composer_model_ids;
#[path = "../../../agent/self-summary/constants.rs"]
mod package_agent_self_summary_constants;
#[path = "../../../agent/tools/task-tool-name.rs"]
mod package_agent_tools_task_tool_name;
#[path = "../../../cursor-plugins/schema-version.rs"]
mod package_cursor_plugins_schema_version;
#[path = "../../../cursor-plugins/validate-subpath.rs"]
mod package_cursor_plugins_validate_subpath;
#[path = "../../../cursor-plugins/environment-filter.rs"]
mod package_cursor_plugins_environment_filter;
#[path = "../../../agent/tools/tool-execution-timeout.rs"]
mod package_agent_tools_tool_execution_timeout;
#[path = "../../../agent/tools/core/read/common.rs"]
mod package_agent_tools_core_read_common;
#[path = "../../../local-exec/services/team-settings-service.rs"]
mod package_local_exec_services_team_settings_service;
#[path = "../../../agent/actions/background-shell-action-handler.rs"]
mod package_agent_actions_background_shell_action_handler;
#[path = "../../../agent/actions/background-subagent-action-handler.rs"]
mod package_agent_actions_background_subagent_action_handler;
#[path = "../../../agent/actions/cancel-action-handler.rs"]
mod package_agent_actions_cancel_action_handler;
#[path = "../../../agent/state-utils.rs"]
mod package_agent_state_utils;
#[path = "../../../agent/constants.rs"]
mod package_agent_constants;
#[path = "../../../agent/state-agent-type.rs"]
mod package_agent_state_agent_type;
#[path = "../../../agent/common.rs"]
mod package_agent_common;
#[path = "../../../context/browser-bridge.rs"]
mod package_context_browser_bridge;
#[path = "../../../agent-store-sync/sync-client-config.rs"]
mod package_agent_store_sync_client_config;
#[path = "../../../hooks/sanitize-system-reminder.rs"]
mod package_hooks_sanitize_system_reminder;
#[path = "../../../local-exec/int32.rs"]
mod package_local_exec_int32;
#[path = "../../../local-exec/mcp.rs"]
mod package_local_exec_mcp;
#[path = "../../../local-exec/mcp-tool-annotations.rs"]
mod package_local_exec_mcp_tool_annotations;
#[path = "../../../hooks/hook-step.rs"]
mod package_hooks_hook_step;
#[path = "../../../hooks/validators/base.rs"]
mod package_hooks_validators_base;
#[path = "../../../hooks/validators/baseHookResponse.rs"]
mod package_hooks_validators_base_hook_response;
#[path = "../../../hooks/validators/afterAgentResponseResponse.rs"]
mod package_hooks_validators_after_agent_response_response;
#[path = "../../../hooks/validators/afterAgentThoughtResponse.rs"]
mod package_hooks_validators_after_agent_thought_response;
#[path = "../../../hooks/validators/afterEditFileResponse.rs"]
mod package_hooks_validators_after_edit_file_response;
#[path = "../../../hooks/validators/afterMCPExecutionResponse.rs"]
mod package_hooks_validators_after_m_c_p_execution_response;
#[path = "../../../hooks/validators/afterShellExecutionResponse.rs"]
mod package_hooks_validators_after_shell_execution_response;
#[path = "../../../hooks/validators/afterTabFileEditResponse.rs"]
mod package_hooks_validators_after_tab_file_edit_response;
#[path = "../../../hooks/validators/sessionEndResponse.rs"]
mod package_hooks_validators_session_end_response;
#[path = "../../../hooks/validators/postToolUseFailureResponse.rs"]
mod package_hooks_validators_post_tool_use_failure_response;
#[path = "../../../hooks/validators/postToolUseResponse.rs"]
mod package_hooks_validators_post_tool_use_response;
#[path = "../../../hooks/validators/stopResponse.rs"]
mod package_hooks_validators_stop_response;
#[path = "../../../hooks/validators/subagentStopResponse.rs"]
mod package_hooks_validators_subagent_stop_response;
#[path = "../../../hooks/validators/preCompactResponse.rs"]
mod package_hooks_validators_pre_compact_response;
#[path = "../../../hooks/validators/subagentStartResponse.rs"]
mod package_hooks_validators_subagent_start_response;
#[path = "../../../hooks/validators/workspaceOpenResponse.rs"]
mod package_hooks_validators_workspace_open_response;
#[path = "../../../hooks/validators/beforeReadFileResponse.rs"]
mod package_hooks_validators_before_read_file_response;
#[path = "../../../hooks/validators/beforePromptSubmitResponse.rs"]
mod package_hooks_validators_before_prompt_submit_response;
#[path = "../../../hooks/validators/beforeTabFileReadResponse.rs"]
mod package_hooks_validators_before_tab_file_read_response;
#[path = "../../../hooks/validators/beforeCommandExecutionHookResponse.rs"]
mod package_hooks_validators_before_command_execution_hook_response;
#[path = "../../../hooks/validators/sessionStartResponse.rs"]
mod package_hooks_validators_session_start_response;
#[path = "../../../hooks/validators/preToolUseResponse.rs"]
mod package_hooks_validators_pre_tool_use_response;

#[path = "../../../../host/host-paths.rs"]
mod host_paths;
#[path = "../../../../host/process-crash-guard.rs"]
mod process_crash_guard;
#[path = "../../../../host/notify-drain-gate.rs"]
mod notify_drain_gate;
#[path = "../../../../host/mcp-auth/mcp-auth-wait-registry.rs"]
mod mcp_auth_wait_registry;
#[path = "../../../../host/box/box-env.rs"]
mod box_env;
#[path = "../../../../host/box/box-capabilities.rs"]
mod box_capabilities;
#[path = "../../../../host/box/box-shell-command.rs"]
mod box_shell_command;
#[path = "../../../../host/host-request-context.rs"]
mod host_request_context;
#[path = "../../../../host/agent-isolation/conversation-blob-db.rs"]
mod conversation_blob_db;
#[path = "../../../../host/agent-isolation/conversation-blob-gc.rs"]
mod conversation_blob_gc;
#[path = "../../../../host/agent-isolation/legacy-blob-retirement.rs"]
mod legacy_blob_retirement;
#[path = "../../../../host/agent-isolation/conversation-blob-store.rs"]
mod conversation_blob_store;
#[path = "../../../../host/agent-isolation/agent-store-worker.rs"]
mod agent_store_worker;
#[path = "../../../../host/agent-isolation/agent-worker-pool.rs"]
mod agent_worker_pool;
#[path = "../../../../host/agent-isolation/worker-blob-store.rs"]
mod worker_blob_store;
#[path = "../../../../host/agent-isolation/transcript-mirror-offload.rs"]
mod transcript_mirror_offload;
#[path = "../../../../host/agent-isolation/transcript-mirror-worker.rs"]
mod transcript_mirror_worker;
#[path = "../../../../host/transcript-mirror/conversation-state-binary.rs"]
mod conversation_state_binary;
#[path = "../../../../host/transcript-mirror/generated-occurrence-codec.rs"]
mod generated_occurrence_codec;
#[path = "../../../../host/transcript-mirror/transcript-journal-codec.rs"]
mod transcript_journal_codec;
#[path = "../../../../host/transcript-mirror/transcript-mirror-router.rs"]
mod transcript_mirror_router;
#[path = "../../../../host/transcript-mirror/transcript-occurrence-deriver.rs"]
mod transcript_occurrence_deriver;
#[path = "../../../../host/transcript-mirror/transcript-mirror.rs"]
mod transcript_mirror;
#[path = "../../../../host/transcript-mirror/legacy-transcript-mirror.rs"]
mod legacy_transcript_mirror;
#[path = "../../../../host/transcript-mirror/production-provider.rs"]
mod production_provider;
#[path = "../../../../host/extensions/box-store-sync/files.rs"]
mod box_store_sync_files;
#[path = "../../../../host/extensions/local-exec/local-exec-failure-classifier.rs"]
mod local_exec_failure_classifier;
#[path = "../../../../host/extensions/transcript/sand-automation-failure.rs"]
mod sand_automation_failure;
#[path = "../../../../host/runner/tools/mcp-server-resolution.rs"]
mod mcp_server_resolution;
#[path = "../../../../host/extensions/box-store-sync/object-store-port.rs"]
mod box_store_object_store_port;
#[path = "../../../../host/extensions/box-lifecycle/box-lifecycle-service.rs"]
mod box_lifecycle_service;
#[path = "../../../../host/runner/site-visit-tracking.rs"]
mod site_visit_tracking;
#[path = "../../../../host/host-event-bus.rs"]
mod host_event_bus;
#[path = "../../../../host/extensions/turn-execution/turn-execution-service.rs"]
mod turn_execution_service;
#[path = "../../../../host/extensions/turn-execution/extension.rs"]
mod turn_execution_extension;
#[path = "../../../../host/box/box-monitor-layout.rs"]
mod box_monitor_layout;
#[path = "../../../../host/box/box-store-backend-policy.rs"]
mod box_store_backend_policy;
#[path = "../../../../host/box/protected-path-guard.rs"]
mod protected_path_guard;
#[path = "../../../../host/box/box-mcp.rs"]
mod box_mcp;
#[path = "../../../../host/box/box-factory.rs"]
mod box_factory;
#[path = "../../../../host/box/box-windows.rs"]
mod box_windows;
#[path = "../../../../host/box/box-file-transfer.rs"]
mod box_file_transfer;
#[path = "../../../../host/box/box-transfer.rs"]
mod box_transfer;
#[path = "../../../../host/box/exec-daemon-process.rs"]
mod exec_daemon_process;
#[path = "../../../../host/box/box-remote-accessor.rs"]
mod box_remote_accessor;
#[path = "../../../../host/box/generated-production.rs"]
mod generated_production;
#[path = "../../../../host/box/loopback-sand-box.rs"]
mod loopback_sand_box;
#[path = "../../../../host/box/shared-desktop-sand-box.rs"]
mod shared_desktop_sand_box;
#[path = "../../../../host/box/production.rs"]
mod production;


#[path = "../../../../host/extensions/local-exec/local-exec-error.rs"]
mod local_exec_error;
#[path = "../../../../host/extensions/cloud-agents/cloud-agent-launch-error.rs"]
mod cloud_agent_launch_error;
#[path = "../../../../host/runner/tools/tool-input-error.rs"]
mod tool_input_error;
#[path = "../../../../host/runner/sand-prompt-markers.rs"]
mod sand_prompt_markers;
#[path = "../../../../host/extensions/box-store-sync/box-store-sync-error.rs"]
mod box_store_sync_error;
#[path = "../../../../host/sha256.rs"]
mod sha256;
#[path = "../../../../host/storage/folder-id.rs"]
mod folder_id;
#[path = "../../../../host/extensions/transcript/channel-delivery-unregistered-error.rs"]
mod channel_delivery_unregistered_error;
#[path = "../../../../host/extensions/transcript/send-not-persisted-error.rs"]
mod send_not_persisted_error;
#[path = "../../../../host/runner/agent-state.rs"]
mod agent_state;
#[path = "../../../../host/extensions/session/conversation-blobs-path.rs"]
mod conversation_blobs_path;
#[path = "../../../../host/extensions/session/call-session.rs"]
mod call_session;
#[path = "../../../../host/automations/automation-id.rs"]
mod automation_id;
#[path = "../../../../host/attachment-paths.rs"]
mod attachment_paths;
#[path = "../../../../host/durable-file-policy.rs"]
mod durable_file_policy;


#[path = "../../../../host/ports/product-analytics.rs"]
mod product_analytics;
#[path = "../../../../host/ports/sand-analytics-types.rs"]
mod sand_analytics_types;
#[path = "../../../../host/host-diagnostics.rs"]
mod host_diagnostics;
#[path = "../../../../host/extensions/session/session-diagnostics.rs"]
mod session_diagnostics;
#[path = "../../../../host/transcript-mutation-events.rs"]
mod transcript_mutation_events;
#[path = "../../../../host/workflows/stat-keyed-parse-cache.rs"]
mod stat_keyed_parse_cache;
#[path = "../../../../host/ports/user-computer.rs"]
mod user_computer;
#[path = "../../../../host/ports/transport.rs"]
mod transport;
#[path = "../../../../host/sand-user-identity.rs"]
mod sand_user_identity;
#[path = "../../../../host/runner/clock-skew-guard.rs"]
mod clock_skew_guard;
#[path = "../../../../host/selected-image-inputs.rs"]
mod selected_image_inputs;
#[path = "../../../../host/extensions/extension-ids.generated.rs"]
mod extension_ids_generated;
#[path = "../../../../host/runner/video-container.rs"]
mod video_container;
#[path = "../../../../host/extensions/telemetry/send-trace-sampler.rs"]
mod send_trace_sampler;


#[path = "../../../../host/extensions/telemetry/telemetry-record.rs"]
mod telemetry_record;
#[path = "../../../../host/runner/tools/sand-permission-request.rs"]
mod sand_permission_request;
#[path = "../../../../host/runner/tools/sand-secret-request.rs"]
mod sand_secret_request;
#[path = "../../../../host/extensions/box-store-sync/box-store-diagnostics.rs"]
mod box_store_diagnostics;
#[path = "../../../../host/extensions/telemetry/host-event-bus-telemetry.rs"]
mod host_event_bus_telemetry;
#[path = "../../../../host/extensions/telemetry/host-diagnostic-telemetry.rs"]
mod host_diagnostic_telemetry;
#[path = "../../../../host/extensions/telemetry/search-index-health-telemetry.rs"]
mod search_index_health_telemetry;
#[path = "../../../../host/extensions/telemetry/auto-review-approval-telemetry.rs"]
mod auto_review_approval_telemetry;
#[path = "../../../../host/extensions/telemetry/disk-pressure-telemetry.rs"]
mod disk_pressure_telemetry;
#[path = "../../../../host/extensions/telemetry/automation-shadow-prune-telemetry.rs"]
mod automation_shadow_prune_telemetry;
#[path = "../../../../host/extensions/telemetry/turn-empty-delivery-telemetry.rs"]
mod turn_empty_delivery_telemetry;
#[path = "../../../../host/extensions/telemetry/box-log-ship-telemetry.rs"]
mod box_log_ship_telemetry;
#[path = "../../../../host/extensions/telemetry/experiments-diagnostic-telemetry.rs"]
mod experiments_diagnostic_telemetry;
#[path = "../../../../host/runner/tool-call-identity.rs"]
mod tool_call_identity;

enum MobileHumanCallTransportCommand {
    Identity,
    IceServers,
    List {
        limit: usize,
    },
    Create {
        call_id: String,
        peer_human_id: String,
    },
    Get {
        call_id: String,
        after_seq: u64,
        limit: usize,
    },
    AppendEvent {
        call_id: String,
        client_event_id: String,
        generation: u64,
        kind: String,
        payload: serde_json::Value,
    },
}

enum MobileHostCommand {
    Dispatch {
        input: String,
        reply: mpsc::SyncSender<String>,
    },
    HumanCall {
        command: MobileHumanCallTransportCommand,
        reply: mpsc::SyncSender<Result<serde_json::Value, String>>,
    },
    Shutdown {
        reply: mpsc::SyncSender<Result<(), String>>,
    },
}

#[derive(Clone)]
struct MobileHostBridge {
    commands: mpsc::SyncSender<MobileHostCommand>,
}

impl MobileHostBridge {
    fn spawn<Factory>(factory: Factory) -> Result<(Self, JoinHandle<()>), String>
    where
        Factory: FnOnce() -> Result<UnifiedAppHost, String> + Send + 'static,
    {
        let (commands, receiver) = mpsc::sync_channel::<MobileHostCommand>(64);
        let (ready, ready_receiver) = mpsc::sync_channel::<Result<(), String>>(1);
        let host_thread = thread::Builder::new()
            .name("fabushi-mobile-host".to_owned())
            .spawn(move || {
                let host = match factory() {
                    Ok(host) => {
                        let _ = ready.send(Ok(()));
                        host
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };

                while let Ok(command) = receiver.recv() {
                    match command {
                        MobileHostCommand::Dispatch { input, reply } => {
                            let output = match process_crash_guard::catch_host_fault(
                                "mahayana-app-host-thread",
                                || dispatch_unified_json(&host, &input),
                            ) {
                                Ok(output) => output,
                                Err(fault) => host_fault_response(fault),
                            };
                            let _ = reply.send(output);
                        }
                        MobileHostCommand::HumanCall { command, reply } => {
                            let output = match process_crash_guard::catch_host_fault(
                                "mahayana-app-host-human-call",
                                || match command {
                                    MobileHumanCallTransportCommand::Identity => {
                                        host.human_call_transport_identity()
                                    }
                                    MobileHumanCallTransportCommand::IceServers => {
                                        host.human_call_ice_servers()
                                    }
                                    MobileHumanCallTransportCommand::List { limit } => {
                                        host.human_call_remote_list(limit)
                                    }
                                    MobileHumanCallTransportCommand::Create {
                                        call_id,
                                        peer_human_id,
                                    } => host.human_call_remote_create(
                                        &call_id,
                                        &peer_human_id,
                                    ),
                                    MobileHumanCallTransportCommand::Get {
                                        call_id,
                                        after_seq,
                                        limit,
                                    } => host.human_call_remote_get(
                                        &call_id,
                                        after_seq,
                                        limit,
                                    ),
                                    MobileHumanCallTransportCommand::AppendEvent {
                                        call_id,
                                        client_event_id,
                                        generation,
                                        kind,
                                        payload,
                                    } => host.human_call_remote_append_event(
                                        &call_id,
                                        &client_event_id,
                                        generation,
                                        &kind,
                                        payload,
                                    ),
                                }
                                .map_err(|error| error.to_string()),
                            ) {
                                Ok(output) => output,
                                Err(fault) => Err(format!(
                                    "host_fault[{}]: {}",
                                    fault.scope, fault.message
                                )),
                            };
                            let _ = reply.send(output);
                        }
                        MobileHostCommand::Shutdown { reply } => {
                            let result = host.close().map_err(|error| error.to_string());
                            let _ = reply.send(result);
                            break;
                        }
                    }
                }
            })
            .map_err(|error| format!("spawn mobile Host thread: {error}"))?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok((Self { commands }, host_thread)),
            Ok(Err(error)) => {
                let _ = host_thread.join();
                Err(error)
            }
            Err(error) => {
                let _ = host_thread.join();
                Err(format!("mobile Host thread exited before ready: {error}"))
            }
        }
    }

    fn dispatch_json(&self, input: &str) -> String {
        let (reply, receiver) = mpsc::sync_channel(1);
        if self
            .commands
            .send(MobileHostCommand::Dispatch {
                input: input.to_owned(),
                reply,
            })
            .is_err()
        {
            return "{\"ok\":false,\"error\":\"mobile Host thread unavailable\"}".to_owned();
        }
        receiver.recv().unwrap_or_else(|_| {
            "{\"ok\":false,\"error\":\"mobile Host reply unavailable\"}".to_owned()
        })
    }

    fn human_call(
        &self,
        command: MobileHumanCallTransportCommand,
    ) -> Result<serde_json::Value, String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.commands
            .send(MobileHostCommand::HumanCall { command, reply })
            .map_err(|_| "mobile Host thread unavailable during Human call transport".to_string())?;
        receiver
            .recv()
            .map_err(|_| "mobile Host Human call transport reply unavailable".to_string())?
    }

    fn request_ok(&self, method: &str) -> bool {
        let input = serde_json::json!({
            "method": method,
            "params": {}
        })
        .to_string();
        serde_json::from_str::<serde_json::Value>(&self.dispatch_json(&input))
            .ok()
            .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
            .unwrap_or(false)
    }

    fn shutdown(&self) -> Result<(), String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.commands
            .send(MobileHostCommand::Shutdown { reply })
            .map_err(|_| "mobile Host thread unavailable during shutdown".to_string())?;
        receiver
            .recv()
            .map_err(|_| "mobile Host shutdown settlement reply unavailable".to_string())?
    }
}

struct MobileTurnRunner {
    host: MobileHostBridge,
    _session: turn_execution_service::TurnExecutionValue,
    _hooks: turn_execution_service::TurnExecutionValue,
    overrides: Option<turn_execution_service::TurnExecutionValue>,
}

impl MobileTurnRunner {
    fn host_is_live(&self) -> bool {
        self.host.request_ok("host.platform")
    }

    fn is_group_member(&self) -> bool {
        self.overrides.is_some()
    }
}

#[derive(Clone)]
struct MobileTurnExecutor {
    host: MobileHostBridge,
}

impl turn_execution_service::TurnExecutor for MobileTurnExecutor {
    fn is_inference_ready(
        &self,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        let ready = self.host.request_ok("feature.info");
        Box::pin(async move { ready })
    }

    fn create_runner(
        &self,
        session: turn_execution_service::TurnExecutionValue,
        hooks: turn_execution_service::TurnExecutionValue,
    ) -> turn_execution_service::TurnExecutionValue {
        Arc::new(MobileTurnRunner {
            host: self.host.clone(),
            _session: session,
            _hooks: hooks,
            overrides: None,
        })
    }

    fn create_group_member_runner(
        &self,
        session: turn_execution_service::TurnExecutionValue,
        hooks: turn_execution_service::TurnExecutionValue,
        overrides: turn_execution_service::TurnExecutionValue,
    ) -> turn_execution_service::TurnExecutionValue {
        Arc::new(MobileTurnRunner {
            host: self.host.clone(),
            _session: session,
            _hooks: hooks,
            overrides: Some(overrides),
        })
    }
}

fn json_identity_text(value: &serde_json::Value) -> Result<String, String> {
    match value {
        serde_json::Value::String(value) => {
            let value = value.trim();
            if value.is_empty() {
                Err("Human identity is empty".into())
            } else {
                Ok(value.to_string())
            }
        }
        serde_json::Value::Number(value) => Ok(value.to_string()),
        _ => Err("Human identity must be a string or number".into()),
    }
}

fn required_remote_call(
    response: &serde_json::Value,
) -> Result<&serde_json::Map<String, serde_json::Value>, String> {
    response
        .get("call")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "remote Human call response omitted call".to_string())
}

fn validate_remote_call_identity(
    remote: &serde_json::Map<String, serde_json::Value>,
    call_id: &str,
    local_human_id: &str,
    peer_human_id: &str,
    expected_creator_id: &str,
    expected_state: &str,
    expected_generation: u64,
) -> Result<(), String> {
    let remote_call_id = remote
        .get("callId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "remote Human call omitted callId".to_string())?;
    if remote_call_id != call_id {
        return Err("remote Human call id does not match canonical CallSession owner".into());
    }
    let creator = json_identity_text(
        remote
            .get("creatorUserId")
            .ok_or_else(|| "remote Human call omitted creatorUserId".to_string())?,
    )?;
    let peer = json_identity_text(
        remote
            .get("peerUserId")
            .ok_or_else(|| "remote Human call omitted peerUserId".to_string())?,
    )?;
    if creator != expected_creator_id {
        return Err("remote Human call creator does not match canonical CallSession owner".into());
    }
    let mut remote_participants = vec![creator, peer];
    remote_participants.sort();
    remote_participants.dedup();
    let mut expected_participants =
        vec![local_human_id.to_string(), peer_human_id.to_string()];
    expected_participants.sort();
    expected_participants.dedup();
    if remote_participants != expected_participants {
        return Err("remote Human call participants do not match canonical CallSession owner".into());
    }
    let state = remote
        .get("state")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "remote Human call omitted state".to_string())?;
    if state != expected_state {
        return Err("remote Human call state does not match canonical CallSession owner".into());
    }
    let generation = remote
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "remote Human call omitted generation".to_string())?;
    if generation != expected_generation {
        return Err("remote Human call generation does not match canonical CallSession owner".into());
    }
    Ok(())
}

fn required_json_string(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, String> {
    params
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{key} is required"))
}

fn required_json_u64(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<u64, String> {
    params
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("{key} must be an unsigned integer"))
}

struct MobileAppHost {
    host: MobileHostBridge,
    host_thread: Option<JoinHandle<()>>,
    extension_runtime: tokio::runtime::Runtime,
    extensions: Option<host_extensions::StartedHostExtensions>,
    call_sessions: call_session::CallSessionStore,
}

impl MobileAppHost {
    fn new(app_data_dir: impl Into<PathBuf>) -> Result<Self, String> {
        let path = app_data_dir.into();
        let host_path = path.clone();
        Self::from_factory(path, move || {
            UnifiedAppHost::new(host_path).map_err(|error| error.to_string())
        })
    }

    fn new_with_feature_mode(
        app_data_dir: impl Into<PathBuf>,
        feature_mode: AppHostFeatureMode,
    ) -> Result<Self, String> {
        let path = app_data_dir.into();
        let host_path = path.clone();
        Self::from_factory(path, move || {
            UnifiedAppHost::new_with_feature_mode(host_path, feature_mode)
                .map_err(|error| error.to_string())
        })
    }

    fn new_with_feature_mode_and_storage_passphrase(
        app_data_dir: impl Into<PathBuf>,
        feature_mode: AppHostFeatureMode,
        storage_passphrase: String,
    ) -> Result<Self, String> {
        let path = app_data_dir.into();
        let host_path = path.clone();
        Self::from_factory(path, move || {
            UnifiedAppHost::new_with_feature_mode_and_storage_passphrase(
                host_path,
                feature_mode,
                storage_passphrase,
            )
            .map_err(|error| error.to_string())
        })
    }

    fn from_factory<Factory>(app_data_dir: PathBuf, factory: Factory) -> Result<Self, String>
    where
        Factory: FnOnce() -> Result<UnifiedAppHost, String> + Send + 'static,
    {
        let call_sessions = call_session::CallSessionStore::open(&app_data_dir, 5_000)?;
        let (host, host_thread) = MobileHostBridge::spawn(factory)?;
        let extension_runtime = match tokio::runtime::Builder::new_current_thread().build() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = host.shutdown();
                let _ = host_thread.join();
                return Err(format!("create mobile Host extension runtime: {error}"));
            }
        };
        let executor: Arc<dyn turn_execution_service::TurnExecutor> =
            Arc::new(MobileTurnExecutor { host: host.clone() });
        let extension =
            turn_execution_extension::bound_turn_execution_extension::<MobileHostBridge>(executor);
        let extensions = match extension_runtime.block_on(host_extensions::start_host_extensions(
            &[extension],
            Arc::new(host.clone()),
            |_extension_id, _error| {},
        )) {
            Ok(extensions) => extensions,
            Err(error) => {
                let _ = host.shutdown();
                let _ = host_thread.join();
                return Err(format!("start mobile Host extensions: {error}"));
            }
        };
        Ok(Self {
            host,
            host_thread: Some(host_thread),
            extension_runtime,
            extensions: Some(extensions),
            call_sessions,
        })
    }

    fn dispatch_json(&self, input: &str) -> String {
        if let Some(output) = self.dispatch_call_session_json(input) {
            return output;
        }
        self.host.dispatch_json(input)
    }

    fn call_transport_identity(&self) -> Result<(String, String), String> {
        let identity = self
            .host
            .human_call(MobileHumanCallTransportCommand::Identity)?;
        let user_id = json_identity_text(
            identity
                .get("userId")
                .ok_or_else(|| "Human call transport identity omitted userId".to_string())?,
        )?;
        let device_id = identity
            .get("deviceId")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Human call transport identity omitted deviceId".to_string())?
            .to_string();
        Ok((user_id, device_id))
    }

    fn local_call_peer(
        &self,
        participant_ids: &[String],
        local_human_id: &str,
    ) -> Result<String, String> {
        let mut peers = participant_ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty() && *value != local_human_id)
            .collect::<Vec<_>>();
        peers.sort_unstable();
        peers.dedup();
        if peers.len() != 1 {
            return Err("shipping Human call transport currently requires exactly one peer".into());
        }
        Ok(peers[0].to_string())
    }

    fn authorized_call_session(
        &self,
        call_id: &str,
        local_human_id: &str,
    ) -> Result<call_session::CallSession, String> {
        let call = self
            .call_sessions
            .get(call_id)?
            .ok_or_else(|| "call session not found".to_string())?;
        if !call
            .participant_ids
            .iter()
            .any(|participant| participant == local_human_id)
        {
            return Err("call session does not belong to the authenticated Human identity".into());
        }
        Ok(call)
    }

    fn create_shipping_call_session(
        &self,
        scope_id: &str,
        participant_ids: &[String],
    ) -> Result<call_session::CallSession, String> {
        let (local_human_id, _) = self.call_transport_identity()?;
        if !participant_ids.iter().any(|value| value == &local_human_id) {
            return Err("authenticated Human identity must be a call participant".into());
        }
        let peer = self.local_call_peer(participant_ids, &local_human_id)?;
        let call_id = Uuid::new_v4().to_string();
        let response = self.host.human_call(MobileHumanCallTransportCommand::Create {
            call_id: call_id.clone(),
            peer_human_id: peer.clone(),
        })?;
        let remote = required_remote_call(&response)?;
        validate_remote_call_identity(
            remote,
            &call_id,
            &local_human_id,
            &peer,
            &local_human_id,
            "invited",
            0,
        )?;
        let event_seq = remote
            .get("eventSeq")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "remote Human call omitted eventSeq".to_string())?;
        if event_seq != 0 {
            return Err("new remote Human call must start at eventSeq 0".into());
        }
        let call = self.call_sessions.create_with_id(
            &call_id,
            scope_id,
            &local_human_id,
            participant_ids,
        )?;
        self.call_sessions.set_remote_event_seq(&call_id, event_seq)?;
        Ok(call)
    }

    fn sync_call_session_from_remote(
        &self,
        call_id: &str,
    ) -> Result<call_session::CallSession, String> {
        let (local_human_id, _) = self.call_transport_identity()?;
        let local = self.authorized_call_session(call_id, &local_human_id)?;
        let peer = self.local_call_peer(&local.participant_ids, &local_human_id)?;
        let mut cursor = self.call_sessions.remote_event_seq(call_id)?;

        for _ in 0..8 {
            let response = self.host.human_call(MobileHumanCallTransportCommand::Get {
                call_id: call_id.to_string(),
                after_seq: cursor,
                limit: 200,
            })?;
            let remote = required_remote_call(&response)?;
            validate_remote_call_identity(
                remote,
                call_id,
                &local_human_id,
                &peer,
                &local.creator_id,
                remote
                    .get("state")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                remote
                    .get("generation")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(local.generation),
            )?;
            let remote_head = remote
                .get("eventSeq")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| "remote Human call omitted eventSeq".to_string())?;
            if remote_head < cursor {
                return Err("remote Human call event cursor regressed".into());
            }

            let events = response
                .get("events")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "remote Human call sync omitted events".to_string())?;
            for event in events {
                let seq = event
                    .get("seq")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| "remote Human call event omitted seq".to_string())?;
                if seq <= cursor {
                    continue;
                }
                if seq != cursor + 1 {
                    return Err(format!(
                        "remote Human call event sequence gap: expected {}, received {}",
                        cursor + 1,
                        seq
                    ));
                }
                self.apply_remote_call_event(call_id, event)?;
                self.call_sessions.set_remote_event_seq(call_id, seq)?;
                cursor = seq;
            }

            if cursor >= remote_head {
                let synced = self.authorized_call_session(call_id, &local_human_id)?;
                let remote_state = remote
                    .get("state")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "remote Human call omitted state".to_string())?;
                let remote_generation = remote
                    .get("generation")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| "remote Human call omitted generation".to_string())?;
                if synced.state != remote_state || synced.generation != remote_generation {
                    return Err("remote Human call head disagrees with canonical local state".into());
                }
                return Ok(synced);
            }

            let next_after_seq = response
                .get("nextAfterSeq")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(cursor);
            if next_after_seq <= cursor {
                return Err("remote Human call sync made no progress".into());
            }
        }
        Err("remote Human call sync exceeded bounded replay window".into())
    }

    fn apply_remote_call_event(
        &self,
        call_id: &str,
        event: &serde_json::Value,
    ) -> Result<(), String> {
        let event_call_id = event
            .get("callId")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "remote Human call event omitted callId".to_string())?;
        if event_call_id != call_id {
            return Err("remote Human call event callId mismatch".into());
        }
        let generation = event
            .get("generation")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "remote Human call event omitted generation".to_string())?;
        let kind = event
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "remote Human call event omitted kind".to_string())?;
        let payload = event
            .get("payload")
            .filter(|value| value.is_object())
            .ok_or_else(|| "remote Human call event omitted payload".to_string())?;
        match kind {
            "transition" => {
                let action = payload
                    .get("action")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "remote call transition omitted action".to_string())?;
                let target_state = payload
                    .get("state")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "remote call transition omitted state".to_string())?;
                let current = self
                    .call_sessions
                    .get(call_id)?
                    .ok_or_else(|| "call session not found".to_string())?;
                if current.state == target_state && current.generation == generation {
                    return Ok(());
                }
                let terminal_reason = payload
                    .get("terminalReason")
                    .and_then(serde_json::Value::as_str);
                let (projected_state, projected_generation, _, _) =
                    call_session::transition_target(&current, action, terminal_reason)?;
                if projected_state != target_state || projected_generation != generation {
                    return Err("remote transition disagrees with canonical CallSession state machine".into());
                }
                self.call_sessions
                    .transition(call_id, current.generation, action, terminal_reason)?;
            }
            "media" => {
                let current = self
                    .call_sessions
                    .get(call_id)?
                    .ok_or_else(|| "call session not found".to_string())?;
                if current.generation != generation {
                    return Err("remote call media event has stale generation".into());
                }
                let media = payload
                    .get("mediaCapabilities")
                    .ok_or_else(|| "remote call media event omitted mediaCapabilities".to_string())?;
                let devices = payload
                    .get("deviceSelection")
                    .ok_or_else(|| "remote call media event omitted deviceSelection".to_string())?;
                if current.media_capabilities != *media || current.device_selection != *devices {
                    self.call_sessions
                        .update_media(call_id, generation, Some(media), Some(devices))?;
                }
            }
            "signal" => {
                let seq = event
                    .get("seq")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| "remote Human call signal omitted seq".to_string())?;
                let sender_device_id = payload
                    .get("senderDeviceId")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "remote Human call signal omitted senderDeviceId".to_string())?;
                let signal_kind = payload
                    .get("signalKind")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "remote Human call signal omitted signalKind".to_string())?;
                let signal = payload
                    .get("signal")
                    .filter(|value| value.is_object())
                    .ok_or_else(|| "remote Human call signal omitted signal payload".to_string())?;
                self.call_sessions.append_remote_signal(
                    call_id,
                    generation,
                    seq,
                    sender_device_id,
                    signal_kind,
                    signal,
                )?;
            }
            other => {
                return Err(format!("unsupported remote Human call event kind {other}"));
            }
        }
        Ok(())
    }

    fn append_shipping_call_event(
        &self,
        call_id: &str,
        generation: u64,
        kind: &str,
        payload: serde_json::Value,
        client_event_id: String,
    ) -> Result<serde_json::Value, String> {
        let response = self.host.human_call(MobileHumanCallTransportCommand::AppendEvent {
            call_id: call_id.to_string(),
            client_event_id: client_event_id.clone(),
            generation,
            kind: kind.to_string(),
            payload: payload.clone(),
        })?;
        let event = response
            .get("event")
            .filter(|value| value.is_object())
            .ok_or_else(|| "remote Human call append omitted event".to_string())?;
        if event
            .get("callId")
            .and_then(serde_json::Value::as_str)
            != Some(call_id)
            || event
                .get("clientEventId")
                .and_then(serde_json::Value::as_str)
                != Some(client_event_id.as_str())
            || event
                .get("generation")
                .and_then(serde_json::Value::as_u64)
                != Some(generation)
            || event.get("kind").and_then(serde_json::Value::as_str) != Some(kind)
            || event.get("payload") != Some(&payload)
        {
            return Err("remote Human call backend returned a mismatched canonical event".into());
        }
        Ok(response)
    }

    fn stable_call_event_id(
        prefix: &str,
        generation: u64,
        payload: &serde_json::Value,
    ) -> Result<String, String> {
        let bytes = serde_json::to_vec(payload).map_err(|error| error.to_string())?;
        let digest = Sha256::digest(bytes);
        let suffix = digest
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(format!("{prefix}:{generation}:{suffix}"))
    }

    fn dispatch_call_session_json(&self, input: &str) -> Option<String> {
        let request = match serde_json::from_str::<serde_json::Value>(input) {
            Ok(value) => value,
            Err(_) => return None,
        };
        let method = request.get("method")?.as_str()?;
        if !matches!(
            method,
            "getCallTransportIdentity"
                | "getCallIceServers"
                | "createCallSession"
                | "getCallSession"
                | "listCallSessions"
                | "transitionCallSession"
                | "updateCallMedia"
                | "sendCallSignal"
                | "listCallSignals"
        ) {
            return None;
        }
        let id = request.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let params = request
            .get("params")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        let result: Result<serde_json::Value, String> = (|| match method {
            "getCallTransportIdentity" => self
                .host
                .human_call(MobileHumanCallTransportCommand::Identity),
            "getCallIceServers" => self
                .host
                .human_call(MobileHumanCallTransportCommand::IceServers),
            "createCallSession" => {
                let scope_id = required_json_string(&params, "scopeId")?;
                let creator_id = match self
                    .host
                    .human_call(MobileHumanCallTransportCommand::Identity)
                {
                    Ok(identity) => identity
                        .get("userId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "Human call transport identity omitted userId".to_string())?
                        .to_string(),
                    Err(identity_error) => {
                        #[cfg(test)]
                        {
                            params
                                .get("creatorId")
                                .and_then(serde_json::Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .map(str::to_string)
                                .ok_or(identity_error)?
                        }
                        #[cfg(not(test))]
                        {
                            return Err(identity_error);
                        }
                    }
                };
                if let Some(requested_creator) = params
                    .get("creatorId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    if requested_creator != creator_id {
                        return Err(
                            "creatorId must match the authenticated Human call identity".into()
                        );
                    }
                }
                let participant_ids = params
                    .get("participantIds")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| "participantIds must be an array".to_string())?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .ok_or_else(|| "participantIds must contain strings".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                serde_json::to_value(
                    self.call_sessions
                        .create(&scope_id, &creator_id, &participant_ids)?,
                )
                .map_err(|error| error.to_string())
            }
            "getCallSession" => {
                let call_id = required_json_string(&params, "callId")?;
                serde_json::to_value(self.call_sessions.get(&call_id)?)
                    .map_err(|error| error.to_string())
            }
            "listCallSessions" => {
                let scope_id = required_json_string(&params, "scopeId")?;
                let limit = params
                    .get("limit")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(20) as usize;
                serde_json::to_value(self.call_sessions.list_for_scope(&scope_id, limit)?)
                    .map_err(|error| error.to_string())
            }
            "transitionCallSession" => {
                let call_id = required_json_string(&params, "callId")?;
                let generation = required_json_u64(&params, "generation")?;
                let action = required_json_string(&params, "action")?;
                let terminal_reason = params.get("terminalReason").and_then(serde_json::Value::as_str);
                serde_json::to_value(
                    self.call_sessions
                        .transition(&call_id, generation, &action, terminal_reason)?,
                )
                .map_err(|error| error.to_string())
            }
            "updateCallMedia" => {
                let call_id = required_json_string(&params, "callId")?;
                let generation = required_json_u64(&params, "generation")?;
                let media_capabilities = params.get("mediaCapabilities");
                let device_selection = params.get("deviceSelection");
                serde_json::to_value(self.call_sessions.update_media(
                    &call_id,
                    generation,
                    media_capabilities,
                    device_selection,
                )?)
                .map_err(|error| error.to_string())
            }
            "sendCallSignal" => {
                let call_id = required_json_string(&params, "callId")?;
                let generation = required_json_u64(&params, "generation")?;
                let seq = required_json_u64(&params, "seq")?;
                let sender_device_id = required_json_string(&params, "senderDeviceId")?;
                let kind = required_json_string(&params, "kind")?;
                let payload = params
                    .get("payload")
                    .ok_or_else(|| "payload is required".to_string())?;
                serde_json::to_value(self.call_sessions.append_signal(
                    &call_id,
                    generation,
                    seq,
                    &sender_device_id,
                    &kind,
                    payload,
                )?)
                .map_err(|error| error.to_string())
            }
            "listCallSignals" => {
                let call_id = required_json_string(&params, "callId")?;
                let generation = required_json_u64(&params, "generation")?;
                let after_seq = params
                    .get("afterSeq")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let limit = params
                    .get("limit")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(100) as usize;
                serde_json::to_value(
                    self.call_sessions
                        .list_signals(&call_id, generation, after_seq, limit)?,
                )
                .map_err(|error| error.to_string())
            }
            _ => unreachable!(),
        })();

        let response = match result {
            Ok(value) => serde_json::json!({
                "id": id,
                "ok": true,
                "result": value,
                "error": serde_json::Value::Null,
            }),
            Err(error) => serde_json::json!({
                "id": id,
                "ok": false,
                "result": serde_json::Value::Null,
                "error": error,
            }),
        };
        Some(response.to_string())
    }

    #[cfg(test)]
    fn turn_execution_registry(&self) -> Arc<turn_execution_service::TurnExecutionRegistry> {
        self.extensions
            .as_ref()
            .and_then(|extensions| {
                extensions.api::<turn_execution_service::TurnExecutionRegistry>(
                    extension_ids_generated::TURN_EXECUTION,
                )
            })
            .expect("production turn-execution extension")
    }
}

impl Drop for MobileAppHost {
    fn drop(&mut self) {
        if let Some(mut extensions) = self.extensions.take() {
            self.extension_runtime.block_on(extensions.stop());
        }
        let _ = self.host.shutdown();
        if let Some(host_thread) = self.host_thread.take() {
            let _ = host_thread.join();
        }
    }
}

fn host_fault_response(fault: process_crash_guard::HostFault) -> String {
    serde_json::to_string(&HostResponse {
        id: None,
        ok: false,
        result: None,
        error: Some(format!("host_fault[{}]: {}", fault.scope, fault.message)),
    })
    .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"host fault\"}".to_owned())
}

thread_local! {
    static LAST_INIT_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_last_init_error(error: Option<String>) {
    LAST_INIT_ERROR.with(|slot| {
        *slot.borrow_mut() = error.and_then(|message| {
            CString::new(message.replace('\0', "�")).ok()
        });
    });
}

fn failed_host_init(error: impl Into<String>) -> *mut MobileAppHost {
    set_last_init_error(Some(error.into()));
    std::ptr::null_mut()
}

/// Returns the most recent native app-host initialization failure on this thread.
///
/// The pointer remains valid until another create call updates the thread-local
/// error slot. Callers must copy the string immediately and must not free it.
#[unsafe(no_mangle)]
pub extern "C" fn mahayana_app_host_last_error() -> *const c_char {
    LAST_INIT_ERROR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|message| message.as_ptr())
            .unwrap_or(std::ptr::null())
    })
}

/// Creates a native app-host handle.
///
/// # Safety
/// If `app_data_dir` is non-null, it must point to a valid NUL-terminated C string
/// for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_create(
    app_data_dir: *const c_char,
) -> *mut MobileAppHost {
    let path = if app_data_dir.is_null() {
        default_app_data_dir()
    } else {
        PathBuf::from(
            unsafe { CStr::from_ptr(app_data_dir) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    set_last_init_error(None);
    match MobileAppHost::new(path) {
        Ok(host) => Box::into_raw(Box::new(host)),
        Err(error) => failed_host_init(error),
    }
}

/// Creates a production native app-host with a stable storage passphrase supplied
/// by the platform Keychain/Keystore bridge. The passphrase is consumed in memory
/// and never written to the Rust app-data directory.
///
/// # Safety
/// Both pointers must reference valid NUL-terminated strings for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_create_with_storage_passphrase(
    app_data_dir: *const c_char,
    storage_passphrase: *const c_char,
) -> *mut MobileAppHost {
    set_last_init_error(None);
    if storage_passphrase.is_null() {
        return failed_host_init("storage passphrase pointer is null");
    }
    let path = if app_data_dir.is_null() {
        default_app_data_dir()
    } else {
        PathBuf::from(
            unsafe { CStr::from_ptr(app_data_dir) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    let passphrase = unsafe { CStr::from_ptr(storage_passphrase) }
        .to_string_lossy()
        .into_owned();
    if passphrase.is_empty() {
        return failed_host_init("storage passphrase is empty");
    }
    match MobileAppHost::new_with_feature_mode_and_storage_passphrase(
        path,
        AppHostFeatureMode::Production,
        passphrase,
    ) {
        Ok(host) => Box::into_raw(Box::new(host)),
        Err(error) => failed_host_init(error),
    }
}

/// Creates a native app-host handle backed by the deterministic FeatureHost test mode.
/// This is used only by explicit UI/instrumentation test harnesses; normal app
/// creation continues to use the production mode.
///
/// # Safety
/// `app_data_dir` must follow the same contract as `mahayana_app_host_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_create_test(
    app_data_dir: *const c_char,
) -> *mut MobileAppHost {
    let path = if app_data_dir.is_null() {
        default_app_data_dir()
    } else {
        PathBuf::from(
            unsafe { CStr::from_ptr(app_data_dir) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    set_last_init_error(None);
    match MobileAppHost::new_with_feature_mode(path, AppHostFeatureMode::Test) {
        Ok(host) => Box::into_raw(Box::new(host)),
        Err(error) => failed_host_init(error),
    }
}

/// Dispatches one JSON request through an existing native app-host handle.
///
/// # Safety
/// `host` must be a live pointer returned by `mahayana_app_host_create`, and
/// `request_json` must point to a valid NUL-terminated C string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_dispatch_with_handle(
    host: *mut MobileAppHost,
    request_json: *const c_char,
) -> *mut c_char {
    if host.is_null() || request_json.is_null() {
        return CString::new("{\"ok\":false,\"error\":\"null host or request\"}")
            .unwrap()
            .into_raw();
    }
    let input = unsafe { CStr::from_ptr(request_json) }.to_string_lossy();
    let host_ref = unsafe { &*host };
    let output = match process_crash_guard::catch_host_fault("mahayana-app-host", || {
        host_ref.dispatch_json(&input)
    }) {
        Ok(output) => output,
        Err(fault) => host_fault_response(fault),
    };
    CString::new(output)
        .unwrap_or_else(|_| CString::new("{\"ok\":false,\"error\":\"invalid response\"}").unwrap())
        .into_raw()
}

/// Destroys a native app-host handle.
///
/// # Safety
/// `host` must be null or a live pointer returned by `mahayana_app_host_create`
/// that has not previously been destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_destroy(host: *mut MobileAppHost) {
    if !host.is_null() {
        unsafe {
            drop(Box::from_raw(host));
        }
    }
}

/// Dispatches one JSON request using a temporary default app-host.
///
/// # Safety
/// `request_json` must point to a valid NUL-terminated C string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_dispatch(request_json: *const c_char) -> *mut c_char {
    if request_json.is_null() {
        return CString::new("{\"ok\":false,\"error\":\"null request\"}")
            .unwrap()
            .into_raw();
    }
    let input = unsafe { CStr::from_ptr(request_json) }.to_string_lossy();
    let output = match MobileAppHost::new(default_app_data_dir()) {
        Ok(host) => match process_crash_guard::catch_host_fault("mahayana-app-host-temporary", || {
            host.dispatch_json(&input)
        }) {
            Ok(output) => output,
            Err(fault) => host_fault_response(fault),
        },
        Err(error) => serde_json::to_string(&HostResponse {
            id: None,
            ok: false,
            result: None,
            error: Some(error.to_string()),
        })
        .unwrap(),
    };
    CString::new(output)
        .unwrap_or_else(|_| CString::new("{\"ok\":false,\"error\":\"invalid response\"}").unwrap())
        .into_raw()
}

/// Frees a response string returned by this FFI module.
///
/// # Safety
/// `pointer` must be null or a pointer returned by a Mahayana app-host dispatch
/// function that has not previously been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mahayana_app_host_free_string(pointer: *mut c_char) {
    if !pointer.is_null() {
        unsafe {
            drop(CString::from_raw(pointer));
        }
    }
}

#[cfg(test)]
mod mobile_turn_execution_composition_tests {
    use super::*;
    use std::any::Any;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-ios-mobile-turn-execution-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn value<T: Any + Send + Sync>(value: T) -> turn_execution_service::TurnExecutionValue {
        Arc::new(value)
    }

    #[test]
    fn ffi_reports_rejected_empty_storage_passphrase() {
        let root = temp_dir();
        let root = CString::new(root.to_string_lossy().as_bytes()).unwrap();
        let empty = CString::new("").unwrap();

        let host = unsafe {
            mahayana_app_host_create_with_storage_passphrase(root.as_ptr(), empty.as_ptr())
        };
        assert!(host.is_null());

        let error = mahayana_app_host_last_error();
        assert!(!error.is_null());
        let message = unsafe { CStr::from_ptr(error) }.to_string_lossy();
        assert_eq!(message, "storage passphrase is empty");
    }

    #[test]
    fn shipping_mobile_host_starts_and_binds_turn_execution() {
        let root = temp_dir();
        let host = MobileAppHost::new_with_feature_mode(&root, AppHostFeatureMode::Test).unwrap();
        let registry = host.turn_execution_registry();

        assert!(registry.can_execute());
        let probe_runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert!(probe_runtime.block_on(registry.is_run_ready()));

        let runner = registry
            .create_runner(value("session"), value("hooks"))
            .unwrap()
            .downcast::<MobileTurnRunner>()
            .unwrap();
        assert!(runner.host_is_live());
        assert!(!runner.is_group_member());

        let group_runner = registry
            .create_group_member_runner(
                value("session"),
                value("hooks"),
                value("overrides"),
            )
            .unwrap()
            .downcast::<MobileTurnRunner>()
            .unwrap();
        assert!(group_runner.host_is_live());
        assert!(group_runner.is_group_member());

        host.host.shutdown().expect("settle mobile Host");
        assert!(
            !host.host.request_ok("host.platform"),
            "shutdown must not return until the canonical Host thread has settled and stopped"
        );

        drop(host);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn shipping_mobile_host_routes_human_call_state_through_host_owner() {
        let root = temp_dir();
        let host = MobileAppHost::new_with_feature_mode(&root, AppHostFeatureMode::Test).unwrap();
        let create = serde_json::json!({
            "id": "call-create",
            "method": "createCallSession",
            "params": {
                "scopeId": "conversation-1",
                "creatorId": "alice",
                "participantIds": ["alice", "bob"]
            }
        });
        let created: serde_json::Value =
            serde_json::from_str(&host.dispatch_json(&create.to_string())).unwrap();
        assert_eq!(created["ok"], true);
        assert_eq!(created["result"]["state"], "invited");
        let call_id = created["result"]["id"].as_str().unwrap().to_string();

        let ring = serde_json::json!({
            "id": "call-ring",
            "method": "transitionCallSession",
            "params": {
                "callId": call_id,
                "generation": 0,
                "action": "ring"
            }
        });
        let ringing: serde_json::Value =
            serde_json::from_str(&host.dispatch_json(&ring.to_string())).unwrap();
        assert_eq!(ringing["ok"], true);
        assert_eq!(ringing["result"]["state"], "ringing");

        let signal = serde_json::json!({
            "id": "call-signal",
            "method": "sendCallSignal",
            "params": {
                "callId": call_id,
                "generation": 0,
                "seq": 1,
                "senderDeviceId": "device-a",
                "kind": "offer",
                "payload": {"sdp": "offer"}
            }
        });
        let signaled: serde_json::Value =
            serde_json::from_str(&host.dispatch_json(&signal.to_string())).unwrap();
        assert_eq!(signaled["ok"], true);
        assert_eq!(signaled["result"]["seq"], 1);

        let list = serde_json::json!({
            "id": "call-list-signals",
            "method": "listCallSignals",
            "params": {
                "callId": call_id,
                "generation": 0,
                "afterSeq": 0,
                "limit": 100
            }
        });
        let signals: serde_json::Value =
            serde_json::from_str(&host.dispatch_json(&list.to_string())).unwrap();
        assert_eq!(signals["ok"], true);
        assert_eq!(signals["result"].as_array().unwrap().len(), 1);

        drop(host);
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(target_os = "android")]
mod android_jni {
    use super::*;
    use jni::JNIEnv;
    use jni::objects::{JObject, JString};
    use jni::sys::{jlong, jstring};

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_ombhrum_fabushi_core_MahayanaHost_nativeCreate(
        mut env: JNIEnv,
        _object: JObject,
        app_data_dir: JString,
        storage_passphrase: JString,
    ) -> jlong {
        let path = match env.get_string(&app_data_dir) {
            Ok(value) => PathBuf::from(value.to_string_lossy().into_owned()),
            Err(_) => return 0,
        };
        let passphrase = match env.get_string(&storage_passphrase) {
            Ok(value) => value.to_string_lossy().into_owned(),
            Err(_) => return 0,
        };
        if passphrase.is_empty() {
            return 0;
        }
        match MobileAppHost::new_with_feature_mode_and_storage_passphrase(
            path,
            AppHostFeatureMode::Production,
            passphrase,
        ) {
            Ok(host) => Box::into_raw(Box::new(host)) as jlong,
            Err(_) => 0,
        }
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_ombhrum_fabushi_core_MahayanaHost_nativeCreateTest(
        mut env: JNIEnv,
        _object: JObject,
        app_data_dir: JString,
    ) -> jlong {
        let path = match env.get_string(&app_data_dir) {
            Ok(value) => PathBuf::from(value.to_string_lossy().into_owned()),
            Err(_) => return 0,
        };
        match MobileAppHost::new_with_feature_mode(path, AppHostFeatureMode::Test) {
            Ok(host) => Box::into_raw(Box::new(host)) as jlong,
            Err(_) => 0,
        }
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_ombhrum_fabushi_core_MahayanaHost_nativeDispatch(
        mut env: JNIEnv,
        _object: JObject,
        handle: jlong,
        request_json: JString,
    ) -> jstring {
        if handle == 0 {
            return env
                .new_string("{\"ok\":false,\"error\":\"native host is not initialized\"}")
                .map(|value| value.into_raw())
                .unwrap_or(std::ptr::null_mut());
        }
        let input = match env.get_string(&request_json) {
            Ok(value) => value.to_string_lossy().into_owned(),
            Err(error) => format!("{{\"ok\":false,\"error\":\"invalid request: {error}\"}}"),
        };
        let host = unsafe { &*(handle as *mut MobileAppHost) };
        env.new_string(host.dispatch_json(&input))
            .map(|value| value.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_ombhrum_fabushi_core_MahayanaHost_nativeDestroy(
        _env: JNIEnv,
        _object: JObject,
        handle: jlong,
    ) {
        if handle != 0 {
            unsafe {
                drop(Box::from_raw(handle as *mut MobileAppHost));
            }
        }
    }
}
