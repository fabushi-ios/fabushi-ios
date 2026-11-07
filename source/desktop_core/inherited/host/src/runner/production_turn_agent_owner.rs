use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cursor_backend::SandPrivacyMode;
use crate::extensions::forever_box::DiskPressureReminderEpisodes;
use crate::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError,
};

use super::generated_agent_turn_stream::run_production_generated_agent_stream;
use super::production_agent_checkpoint::AgentStateCheckpointSink;
use super::production_turn_input_projection::{
    ProductionTurnInputProjection, create_production_turn_agent_input_projection,
};
use super::production_turn_run_shell_adapter::run_production_turn_shell_lifecycle;
use super::send_message_reminder_middleware::DISK_PRESSURE_REMINDER_MESSAGE;
use super::routed_provider_runtime::RoutedProviderCancellation;
use super::turn_agent_composition::{
    SAND_AGENT_MAX_STEPS, SAND_AGENT_TOKEN_LIMIT, TurnAgentComposition,
};
use super::{
    TerminalOutcome, TurnRunFinished, TurnRunOptions, TurnRunShell,
};

pub type ProductionTurnSummarizationPrompt = Arc<
    dyn Fn(&str, &str, &dyn Fn() -> bool) -> Result<String, ProviderSessionError>
        + Send
        + Sync,
>;
pub type ProductionTurnProfileAnnouncementCommit = Arc<dyn Fn() + Send + Sync>;
pub type ProductionTurnPrivacyModeResolver =
    Arc<dyn Fn() -> Option<SandPrivacyMode> + Send + Sync>;

/// Frozen, per-turn static inputs owned by ProductionTurnAgentOwner.
///
/// This is deliberately a projection of existing production truths rather than
/// a second configuration system. Detailed prompt/tool configuration remains in
/// TurnAgentComposition; the owner freezes the identity and mode inputs that
/// must not drift after a turn starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionTurnAgentStaticConfig {
    pub model_id: String,
    pub agent_token_limit: usize,
    pub conversation_id: String,
    pub is_box_scoped_subagent: bool,
    pub is_subagent_runner: bool,
    pub is_shared_room_runner: bool,
    pub sand_send_message_delivery_owed: bool,
    pub transcripts_folder_available: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductionTurnAgentStaticProjection {
    pub max_steps: usize,
    pub agent_token_limit: usize,
    pub background_summarization_start_unused_tokens: usize,
    pub background_summarization_start_unused_percent: f64,
    pub background_summarization_persist_unused_tokens: usize,
    pub background_summarization_persist_unused_percent: f64,
    pub background_summarization_discard_on_error: bool,
    pub background_summarization_require_trigger_for_mid_loop_persist: bool,
    pub enable_watch_video_in_ide_subagent: bool,
    pub sand_send_message_delivery_owed: bool,
    pub user_message_timestamps: bool,
    pub rerender_user_info_on_request_context_recovery: bool,
    pub rerender_user_info_on_summarization: bool,
    pub skip_pre_turn_state_snapshot: bool,
    pub agent_type: &'static str,
    pub conversation_group_id: String,
    pub disable_user_info: bool,
    pub display_cursor_rules: bool,
    pub display_skills: bool,
    pub exclude_agent_transcripts: bool,
    pub enable_terminal_files: bool,
    pub enable_transcript_in_summary: bool,
}

impl ProductionTurnAgentStaticConfig {
    pub fn frozen_projection(&self) -> ProductionTurnAgentStaticProjection {
        ProductionTurnAgentStaticProjection {
            max_steps: SAND_AGENT_MAX_STEPS,
            agent_token_limit: self.agent_token_limit,
            background_summarization_start_unused_tokens: 10_000,
            background_summarization_start_unused_percent: 0.1,
            background_summarization_persist_unused_tokens: 5_000,
            background_summarization_persist_unused_percent: 0.05,
            background_summarization_discard_on_error: true,
            background_summarization_require_trigger_for_mid_loop_persist: true,
            enable_watch_video_in_ide_subagent: true,
            sand_send_message_delivery_owed: self.sand_send_message_delivery_owed,
            user_message_timestamps: true,
            rerender_user_info_on_request_context_recovery: true,
            rerender_user_info_on_summarization: true,
            skip_pre_turn_state_snapshot: true,
            agent_type: "IDE",
            conversation_group_id: self.conversation_id.clone(),
            disable_user_info: self.is_box_scoped_subagent,
            display_cursor_rules: true,
            display_skills: !self.is_subagent_runner && !self.is_shared_room_runner,
            exclude_agent_transcripts: self.is_subagent_runner
                || !self.transcripts_folder_available,
            enable_terminal_files: false,
            enable_transcript_in_summary: true,
        }
    }
}

/// Immutable build input captured once before the shipping provider path runs.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductionTurnAgentBuildInput {
    pub static_config: ProductionTurnAgentStaticConfig,
    pub static_projection: ProductionTurnAgentStaticProjection,
    pub privacy_mode: Option<SandPrivacyMode>,
}

/// Existing production surfaces that ProductionTurnAgentOwner resolves exactly
/// once into ProductionTurnAgentBuildInput.
///
/// Inference/privacy and summarization retain their canonical owners. This
/// binding only makes the Runner turn owner responsible for the per-turn
/// resolve/freeze lifecycle, matching Grok's buildAgentForRun owner boundary.
pub struct ProductionTurnAgentBuildBindings {
    static_config: ProductionTurnAgentStaticConfig,
    privacy_mode_resolver: ProductionTurnPrivacyModeResolver,
    summarization_prompt: ProductionTurnSummarizationPrompt,
}

impl ProductionTurnAgentBuildBindings {
    pub fn new(
        static_config: ProductionTurnAgentStaticConfig,
        privacy_mode_resolver: ProductionTurnPrivacyModeResolver,
        summarization_prompt: ProductionTurnSummarizationPrompt,
    ) -> Self {
        Self {
            static_config,
            privacy_mode_resolver,
            summarization_prompt,
        }
    }
}

/// Host-resolved lifecycle surfaces that are consumed by one production turn.
///
/// Concrete ForeverBox and profile stores retain their canonical owners. This
/// immutable binding lets ProductionTurnAgentOwner own their per-turn
/// claim/commit/dispose semantics without creating a second runtime.
pub struct ProductionTurnAgentLifecycleBindings {
    conversation_id: String,
    claim_id: String,
    disk_pressure_reminders: Option<Arc<DiskPressureReminderEpisodes>>,
    profile_announcement_commit: Option<ProductionTurnProfileAnnouncementCommit>,
}

impl ProductionTurnAgentLifecycleBindings {
    pub fn new(
        conversation_id: impl Into<String>,
        claim_id: impl Into<String>,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            claim_id: claim_id.into(),
            disk_pressure_reminders: None,
            profile_announcement_commit: None,
        }
    }

    pub fn with_disk_pressure_reminders(
        mut self,
        reminders: Option<Arc<DiskPressureReminderEpisodes>>,
    ) -> Self {
        self.disk_pressure_reminders = reminders;
        self
    }

    pub fn with_profile_announcement_commit(
        mut self,
        commit: Option<ProductionTurnProfileAnnouncementCommit>,
    ) -> Self {
        self.profile_announcement_commit = commit;
        self
    }
}

/// Owns one production turn's terminal lifecycle independently from the Host.
///
/// The Host may supervise the worker thread, but begin/dispatched/terminal
/// settlement belongs here so a Host failure or a provider failure cannot
/// produce two final states for the same Runner turn.
pub struct ProductionTurnAgentOwner {
    composition: TurnAgentComposition,
    shell: TurnRunShell,
    last_finished: Option<TurnRunFinished>,
    agent_state_checkpoint_sink: Option<Arc<dyn AgentStateCheckpointSink>>,
    upgrade_quiescing: Arc<AtomicBool>,
    build_input: Option<ProductionTurnAgentBuildInput>,
    summarization_prompt: Option<ProductionTurnSummarizationPrompt>,
    lifecycle_bindings: Option<ProductionTurnAgentLifecycleBindings>,
    disk_pressure_episode_id: Option<String>,
    disk_pressure_committed: bool,
    profile_announcement_committed: bool,
    disposed: bool,
}

impl ProductionTurnAgentOwner {
    pub fn new(composition: TurnAgentComposition) -> Self {
        Self {
            composition,
            shell: TurnRunShell::default(),
            last_finished: None,
            agent_state_checkpoint_sink: None,
            upgrade_quiescing: Arc::new(AtomicBool::new(false)),
            build_input: None,
            summarization_prompt: None,
            lifecycle_bindings: None,
            disk_pressure_episode_id: None,
            disk_pressure_committed: false,
            profile_announcement_committed: false,
            disposed: false,
        }
    }

    pub fn with_agent_state_checkpoint_sink(
        mut self,
        sink: Arc<dyn AgentStateCheckpointSink>,
    ) -> Self {
        self.agent_state_checkpoint_sink = Some(sink);
        self
    }

    pub fn with_upgrade_quiesce_signal(mut self, signal: Arc<AtomicBool>) -> Self {
        self.upgrade_quiescing = signal;
        self
    }

    pub fn request_quiesce_for_upgrade(&self) {
        self.upgrade_quiescing.store(true, Ordering::Release);
    }

    pub fn cancel_quiesce_for_upgrade(&self) {
        self.upgrade_quiescing.store(false, Ordering::Release);
    }

    pub fn is_quiescing_for_upgrade(&self) -> bool {
        self.upgrade_quiescing.load(Ordering::Acquire)
    }

    pub fn with_build_bindings(
        mut self,
        bindings: ProductionTurnAgentBuildBindings,
    ) -> Self {
        let privacy_mode = (bindings.privacy_mode_resolver)();
        let static_projection = bindings.static_config.frozen_projection();
        self.build_input = Some(ProductionTurnAgentBuildInput {
            static_config: bindings.static_config,
            static_projection,
            privacy_mode,
        });
        self.summarization_prompt = Some(bindings.summarization_prompt);
        self
    }

    pub fn build_input(&self) -> Option<&ProductionTurnAgentBuildInput> {
        self.build_input.as_ref()
    }

    fn validate_build_input(&self) -> Result<(), ProviderSessionError> {
        let input = self.build_input.as_ref().ok_or_else(|| {
            ProviderSessionError::Configuration(
                "production turn build input is not bound".into(),
            )
        })?;
        if input.static_config.model_id.trim().is_empty() {
            return Err(ProviderSessionError::Configuration(
                "production turn model id is empty".into(),
            ));
        }
        if input.static_config.conversation_id.trim().is_empty() {
            return Err(ProviderSessionError::Configuration(
                "production turn conversation id is empty".into(),
            ));
        }
        if input.static_projection.max_steps != SAND_AGENT_MAX_STEPS {
            return Err(ProviderSessionError::Configuration(
                "production turn max steps drifted from frozen Grok config".into(),
            ));
        }
        if input.static_projection.agent_token_limit != SAND_AGENT_TOKEN_LIMIT {
            return Err(ProviderSessionError::Configuration(
                "production turn agent token limit drifted from frozen Grok config".into(),
            ));
        }
        if input.static_projection.conversation_group_id != input.static_config.conversation_id {
            return Err(ProviderSessionError::Configuration(
                "production turn conversation group identity drifted".into(),
            ));
        }
        if let Some(lifecycle) = self.lifecycle_bindings.as_ref()
            && lifecycle.conversation_id != input.static_config.conversation_id
        {
            return Err(ProviderSessionError::Configuration(
                "production turn build/lifecycle conversation identities differ".into(),
            ));
        }
        Ok(())
    }

    pub fn with_lifecycle_bindings(
        mut self,
        bindings: ProductionTurnAgentLifecycleBindings,
    ) -> Self {
        self.release_uncommitted_disk_pressure();
        self.disk_pressure_episode_id = bindings
            .disk_pressure_reminders
            .as_ref()
            .and_then(|reminders| {
                reminders.claim(&bindings.conversation_id, &bindings.claim_id)
            });
        self.lifecycle_bindings = Some(bindings);
        self.disk_pressure_committed = false;
        self.profile_announcement_committed = false;
        self.disposed = false;
        self
    }

    pub fn disk_pressure_episode_id(&self) -> Option<&str> {
        self.disk_pressure_episode_id.as_deref()
    }

    pub fn run_summarization_prompt(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<String, ProviderSessionError> {
        let summarization_prompt = self.summarization_prompt.as_ref().ok_or_else(|| {
            ProviderSessionError::Configuration(
                "production turn summarization surface is not bound".into(),
            )
        })?;
        (summarization_prompt)(system_prompt, user_prompt, should_cancel)
    }

    pub fn project_provider_messages_for_turn(
        &self,
        provider_messages: &[ProviderMessage],
    ) -> Vec<ProviderMessage> {
        let mut projected = provider_messages.to_vec();
        if self.disk_pressure_committed || self.disk_pressure_episode_id.is_none() {
            return projected;
        }
        if !projected.iter().any(|message| {
            message.role == "user"
                && message.content.contains(DISK_PRESSURE_REMINDER_MESSAGE)
        }) {
            projected.push(ProviderMessage {
                role: "user".into(),
                content: DISK_PRESSURE_REMINDER_MESSAGE.into(),
            });
        }
        projected
    }

    pub fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        self.release_uncommitted_disk_pressure();
    }

    fn commit_successful_lifecycle(&mut self) {
        let Some(bindings) = self.lifecycle_bindings.as_ref() else {
            return;
        };
        let reminders = bindings.disk_pressure_reminders.as_ref().map(Arc::clone);
        let conversation_id = bindings.conversation_id.clone();
        let claim_id = bindings.claim_id.clone();
        let profile_commit = bindings.profile_announcement_commit.clone();

        if !self.disk_pressure_committed {
            if let (Some(reminders), Some(_episode_id)) =
                (reminders, self.disk_pressure_episode_id.as_ref())
            {
                self.disk_pressure_committed =
                    reminders.commit(&conversation_id, &claim_id);
            }
        }
        if !self.profile_announcement_committed {
            if let Some(commit) = profile_commit {
                commit();
                self.profile_announcement_committed = true;
            }
        }
    }

    fn release_uncommitted_disk_pressure(&mut self) {
        if self.disk_pressure_committed || self.disk_pressure_episode_id.is_none() {
            return;
        }
        let Some(bindings) = self.lifecycle_bindings.as_ref() else {
            return;
        };
        if let Some(reminders) = bindings.disk_pressure_reminders.as_ref() {
            reminders.release(&bindings.conversation_id, &bindings.claim_id);
        }
        self.disk_pressure_episode_id = None;
    }

    pub fn cancellation(&self) -> RoutedProviderCancellation {
        self.composition.cancellation()
    }

    pub fn last_finished(&self) -> Option<&TurnRunFinished> {
        self.last_finished.as_ref()
    }

    pub fn run_routed_provider(
        &mut self,
        data_dir: &Path,
        messages: &[ProviderMessage],
        on_text_delta: &mut dyn FnMut(&str, &str),
    ) -> Result<String, ProviderSessionError> {
        self.run_routed_provider_with_options(
            data_dir,
            messages,
            TurnRunOptions::default(),
            on_text_delta,
        )
    }

    pub fn run_routed_provider_with_options(
        &mut self,
        data_dir: &Path,
        messages: &[ProviderMessage],
        options: TurnRunOptions,
        on_text_delta: &mut dyn FnMut(&str, &str),
    ) -> Result<String, ProviderSessionError> {
        self.run_routed_provider_with_projected_turn_input(
            data_dir,
            messages,
            messages,
            ProductionTurnInputProjection {
                options,
                ack_token: None,
                prompt_action: Default::default(),
                typed_replay: None,
            },
            on_text_delta,
        )
    }

    pub fn run_routed_provider_with_projected_messages(
        &mut self,
        data_dir: &Path,
        lifecycle_messages: &[ProviderMessage],
        provider_messages: &[ProviderMessage],
        options: TurnRunOptions,
        on_text_delta: &mut dyn FnMut(&str, &str),
    ) -> Result<String, ProviderSessionError> {
        self.run_routed_provider_with_projected_turn_input(
            data_dir,
            lifecycle_messages,
            provider_messages,
            ProductionTurnInputProjection {
                options,
                ack_token: None,
                prompt_action: Default::default(),
                typed_replay: None,
            },
            on_text_delta,
        )
    }

    pub fn run_routed_provider_with_projected_turn_input(
        &mut self,
        data_dir: &Path,
        lifecycle_messages: &[ProviderMessage],
        provider_messages: &[ProviderMessage],
        turn_input: ProductionTurnInputProjection,
        on_text_delta: &mut dyn FnMut(&str, &str),
    ) -> Result<String, ProviderSessionError> {
        self.validate_build_input()?;
        let turn_max_steps = self
            .build_input
            .as_ref()
            .expect("validated production turn build input")
            .static_projection
            .max_steps;
        let checkpoint_sink_for_projection =
            self.agent_state_checkpoint_sink.clone();
        let checkpoint_sink_for_stream =
            self.agent_state_checkpoint_sink.clone();
        let composition_for_projection = self.composition.clone();
        let composition_for_stream = self.composition.clone();
        let completion_probe = self.composition.clone();
        let projected_provider_messages =
            self.project_provider_messages_for_turn(provider_messages);
        let shell_options = turn_input.options.clone();
        let turn_quiesced = Arc::new(AtomicBool::new(false));
        let stream_upgrade_quiescing = Arc::clone(&self.upgrade_quiescing);
        let stream_turn_quiesced = Arc::clone(&turn_quiesced);
        let tool_loop_gate_for_stream = self.shell.tool_loop_gate();
        let result = run_production_turn_shell_lifecycle(
            &mut self.shell,
            &mut self.last_finished,
            lifecycle_messages,
            shell_options,
            Arc::clone(&self.upgrade_quiescing),
            Arc::clone(&turn_quiesced),
            move |_prompt, _started| {
                create_production_turn_agent_input_projection(
                    &composition_for_projection,
                    checkpoint_sink_for_projection.as_ref(),
                    lifecycle_messages,
                    &projected_provider_messages,
                    &turn_input,
                    on_text_delta,
                )
            },
            move |prepared| {
                let projection = prepared.prepared;
                let stream_composition = composition_for_stream
                    .with_projected_mcp_tools(projection.mcp_tools.clone())
                    .with_projected_mcp_meta_tools(projection.mcp_meta_tools.clone())
                    .with_tool_loop_gate(
                        tool_loop_gate_for_stream.clone(),
                        prepared.started.owner.clone(),
                    );
                run_production_generated_agent_stream(
                    stream_composition,
                    checkpoint_sink_for_stream,
                    data_dir,
                    projection,
                    turn_max_steps,
                    prepared.started.owner.generation,
                    Arc::clone(&stream_upgrade_quiescing),
                    Arc::clone(&stream_turn_quiesced),
                )
            },
            |_started| {},
        );
        if let (Ok(content), Some(finished)) = (&result, self.last_finished.as_mut()) {
            finished.ended_on_silent_tool_calls =
                completion_probe.last_run_ended_on_silent_tool_calls(content);
        }
        if result.is_ok() {
            self.commit_successful_lifecycle();
        }
        result
    }

    /// Contract seam used by independent lifecycle tests. The shipping path
    /// above supplies the real TurnAgentComposition executor.
    pub fn run_with<Execute>(
        &mut self,
        messages: &[ProviderMessage],
        execute: Execute,
    ) -> Result<String, ProviderSessionError>
    where
        Execute: FnOnce() -> Result<String, ProviderSessionError>,
    {
        self.run_with_options(messages, TurnRunOptions::default(), execute)
    }

    pub fn run_with_options<Execute>(
        &mut self,
        messages: &[ProviderMessage],
        options: TurnRunOptions,
        execute: Execute,
    ) -> Result<String, ProviderSessionError>
    where
        Execute: FnOnce() -> Result<String, ProviderSessionError>,
    {
        // This is the independent contract seam, not the shipping provider path.
        // Preserve the owner invariant that a durable Agent checkpoint must
        // succeed before Completed settlement. The shipping path above already
        // owns this transaction inside run_production_generated_agent_stream,
        // so it intentionally does not checkpoint a second time here.
        let checkpoint_sink = self.agent_state_checkpoint_sink.clone();
        let checkpoint_options = options.clone();
        let turn_quiesced = Arc::new(AtomicBool::new(false));
        let result = run_production_turn_shell_lifecycle(
            &mut self.shell,
            &mut self.last_finished,
            messages,
            options,
            Arc::clone(&self.upgrade_quiescing),
            turn_quiesced,
            |_prompt, _started| Ok(()),
            move |_prepared| {
                let result = execute();
                match (result, checkpoint_sink) {
                    (Ok(content), Some(sink)) => {
                        sink.checkpoint_text_turn(
                            messages,
                            &checkpoint_options,
                            &content,
                        )?;
                        Ok(content)
                    }
                    (result, _) => result,
                }
            },
            |_started| {},
        );
        if result.is_ok() {
            self.commit_successful_lifecycle();
        }
        result
    }
}

impl Drop for ProductionTurnAgentOwner {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[allow(dead_code)]
fn _terminal_outcome_type_anchor(outcome: &TerminalOutcome) -> bool {
    matches!(
        outcome,
        TerminalOutcome::Completed
            | TerminalOutcome::Cancelled
            | TerminalOutcome::WaitingUser
            | TerminalOutcome::Failed { .. }
    )
}
