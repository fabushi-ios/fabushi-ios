//! Mahayana Host/Runner ownership boundary matching Grok Bot 0.18.
//!
//! Coordinator supervision lives in `source/node-agent-coordinator`; this
//! crate owns accepted turn execution, stream attempts, retry/checkpoint policy,
//! cancellation and terminal settlement.

pub mod extensions;
pub mod cursor_backend;
pub mod host_paths;
pub mod host_discovery;
pub mod host_lock;
pub mod gateway_config;
pub mod gateway_command_error;
pub mod gateway_protocol;
pub mod gateway_server;
pub mod host_event_bus;
pub mod runner;
pub mod runner_production_bridge;
pub mod production_binding_providers;
pub mod host_production_extensions;
pub mod host_runner_composition;

pub use runner::{
    AttemptCheckpoint, AttemptProgress, CheckpointBoundary, CompletedAwaitOutcome,
    ContextWindowTracker, FullStreamSanitizer, HIDDEN_PROMPT_MARKER, MAX_SAFE_TOKEN_COUNT,
    ModelResolutionTicket, RecentUserMessage, ResolvedModelTracker, RetryDecision,
    SUMMARIZATION_MAX_OUTPUT_TOKENS, SUMMARIZATION_MAX_PROMPT_CHARS, SanitizedExtendedUsage,
    SanitizedUsage, StreamAttemptPolicy, StreamFailureKind, StreamSanitizerItem, StreamWatchdog,
    SummarizationPolicy, TOOL_CALL_IDENTITY_CAP, TerminalOutcome, ToolCallIdentity,
    ToolSurfaceUpdate, TransientStreamError, TurnCancellation, TurnEndedUsage, TurnOwnerToken,
    TurnRunFinished, TurnRunOptions, TurnRunShell, TurnRunShellError, TurnRunStarted,
    TurnSettlement, TurnUsage, add_token_counts, await_block_until_ms,
    build_unanswered_questions_note, classify_completed_await_outcome, merge_turn_usage,
    sanitize_usage, select_unconfirmed_user_messages, should_use_self_summary,
    summarization_policy, to_safe_token_count, to_safe_usage_count, total_input_tokens,
    turn_usage_from_turn_ended,
};

pub mod sand_activity;
pub mod sand_quiet_work_origin;
pub mod sha256;
pub mod storage;
pub mod host_diagnostics;
pub mod host_system_info;
pub mod host_invariant;
pub mod host_initial_transcript_load;
pub mod process_crash_guard;
pub mod network_worker_process;
pub mod network_enrollment_owner;
pub mod remote_computer_target;
pub mod sensitive_input_challenge;
pub mod r#box;
pub mod automations;
pub mod workflows;
pub mod watched_directory;
pub mod groups;
pub mod attachment_paths;
pub mod media_mime;
pub mod media_understanding_defaults;
pub mod ports;
pub mod durable_file_policy;
pub mod sand_user_identity;
pub mod notify_drain_gate;
pub mod agents;
pub mod transcript_mutation_events;
pub mod selected_image_inputs;
pub mod host_request_context;
pub mod runner_context_production_provider;
pub mod agent_isolation;
pub mod host_secret_store;
pub mod transcript_mirror;

pub mod mcp_auth;

pub mod host_roster_bookkeeping;
pub mod cloud_agents;

pub mod connectors;
pub mod send_trace_host;

pub mod sand_multitask;

pub mod device_pairing_owner;
pub use device_pairing_owner::DevicePairingOwner;

pub mod host_gateway_api;
pub mod sand_host;
pub mod error_diagnostics;
pub mod process_spawn_diagnostics;
