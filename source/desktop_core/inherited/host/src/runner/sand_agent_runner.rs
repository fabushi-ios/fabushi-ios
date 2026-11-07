use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError,
};
use crate::extensions::transcript::async_task_union::AsyncTask;

use super::background_work::{
    BackgroundShellWatchOptions, CloudAgentWatchOptions,
    RunnerBackgroundShellWatches, RunnerCloudAgentWatches,
};
use super::computer_use::{ComputerUseCoordination, ComputerUseUsageSnapshot};
use super::production_turn_agent_owner::ProductionTurnAgentOwner;
use super::production_turn_input_projection::ProductionTurnInputProjection;
use super::routed_provider_runtime::RoutedProviderCancellation;
use super::subagent_runtime::{
    ControlResult, PendingWake, RunOutcome, RunningSubagentInfo, SettleResult,
    SubagentLineage, SubagentRecord, SubagentRuntime, SubagentSessionSnapshot,
};
use super::turn_observation::TurnObservationHandle;
use super::{TerminalOutcome, TurnRunFinished, TurnRunOptions};

/// Runner-owned shipping facade for provider-backed turns.
///
/// This is intentionally separate from Host supervision. The Host owns the
/// process/thread boundary; this type owns the turn Agent owner and therefore
/// the provider execution plus terminal settlement for the turn.
pub struct SandAgentRunner {
    owner: ProductionTurnAgentOwner,
    generated_agent_runtime: Option<Arc<Mutex<SubagentRuntime>>>,
    observation: Option<TurnObservationHandle>,
    cloud_agent_watches: Option<Arc<RunnerCloudAgentWatches>>,
    background_shell_watches: Option<Arc<RunnerBackgroundShellWatches>>,
    computer_use: Option<Arc<Mutex<ComputerUseCoordination>>>,
}

impl SandAgentRunner {
    pub fn new(owner: ProductionTurnAgentOwner) -> Self {
        Self {
            owner,
            generated_agent_runtime: None,
            observation: None,
            cloud_agent_watches: None,
            background_shell_watches: None,
            computer_use: None,
        }
    }

    pub fn with_generated_agent_runtime(
        mut self,
        runtime: Arc<Mutex<SubagentRuntime>>,
    ) -> Self {
        self.generated_agent_runtime = Some(runtime);
        self
    }

    pub fn generated_agent_runtime(&self) -> Option<Arc<Mutex<SubagentRuntime>>> {
        self.generated_agent_runtime.as_ref().map(Arc::clone)
    }

    pub fn with_runtime_services(
        mut self,
        observation: Option<TurnObservationHandle>,
        cloud_agent_watches: Option<Arc<RunnerCloudAgentWatches>>,
        background_shell_watches: Option<Arc<RunnerBackgroundShellWatches>>,
        computer_use: Option<Arc<Mutex<ComputerUseCoordination>>>,
    ) -> Self {
        self.observation = observation;
        self.cloud_agent_watches = cloud_agent_watches;
        self.background_shell_watches = background_shell_watches;
        self.computer_use = computer_use;
        self
    }

    pub fn list_async_tasks(&self) -> Result<Vec<AsyncTask>, String> {
        let Some(observation) = self.observation.as_ref() else {
            return Ok(Vec::new());
        };
        observation
            .lock()
            .map_err(|_| "turn observation lock poisoned".to_string())
            .map(|observation| observation.list_async_tasks())
    }

    pub fn get_activity_snapshot(&self) -> Result<Vec<String>, String> {
        let Some(observation) = self.observation.as_ref() else {
            return Ok(Vec::new());
        };
        observation
            .lock()
            .map_err(|_| "turn observation lock poisoned".to_string())
            .map(|observation| observation.recent_activity())
    }

    pub fn get_observed_tool_call_count(&self) -> Result<u64, String> {
        let Some(observation) = self.observation.as_ref() else {
            return Ok(0);
        };
        observation
            .lock()
            .map_err(|_| "turn observation lock poisoned".to_string())
            .map(|observation| observation.observed_tool_call_count())
    }

    pub fn get_pending_cloud_agent_watch_ids(&self, parent_agent_id: &str) -> Vec<String> {
        self.cloud_agent_watches
            .as_ref()
            .map(|watches| watches.pending_cloud_agent_watch_ids(parent_agent_id))
            .unwrap_or_default()
    }

    pub fn watch_cloud_agent(
        &self,
        parent_agent_id: &str,
        id: &str,
        quiet_origin: Option<Value>,
        after_followup: bool,
    ) -> bool {
        self.cloud_agent_watches
            .as_ref()
            .is_some_and(|watches| {
                watches.watch_cloud_agent(
                    parent_agent_id,
                    id,
                    CloudAgentWatchOptions::new(quiet_origin, after_followup),
                )
            })
    }

    pub fn watch_background_shell(
        &self,
        parent_agent_id: &str,
        id: &str,
        title: Option<String>,
        quiet_origin: Option<Value>,
    ) -> bool {
        self.background_shell_watches
            .as_ref()
            .is_some_and(|watches| {
                watches.watch_background_shell(
                    parent_agent_id,
                    id,
                    BackgroundShellWatchOptions::new(title, quiet_origin),
                )
            })
    }

    pub fn has_running_background_shell_work(&self) -> bool {
        self.background_shell_watches
            .as_ref()
            .is_some_and(|watches| watches.has_running_background_shell_work())
    }

    pub fn get_pending_shell_rewatch_ids(&self, parent_agent_id: &str) -> Vec<String> {
        self.background_shell_watches
            .as_ref()
            .map(|watches| watches.pending_shell_rewatch_ids(parent_agent_id))
            .unwrap_or_default()
    }

    pub fn cancel_background_shell_rewatches(&self, parent_agent_id: &str) -> usize {
        self.background_shell_watches
            .as_ref()
            .map(|watches| watches.dispose_parent(parent_agent_id))
            .unwrap_or_default()
    }

    pub fn get_computer_use_usage_snapshot(&self) -> Option<ComputerUseUsageSnapshot> {
        self.computer_use.as_ref().map(|owner| {
            owner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .usage_snapshot()
        })
    }

    pub fn get_computer_use_audit_action_counts(&self) -> HashMap<String, u64> {
        self.computer_use
            .as_ref()
            .map(|owner| {
                owner
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .audit_action_counts()
                    .clone()
            })
            .unwrap_or_default()
    }

    pub fn request_quiesce_for_upgrade(&self) {
        self.owner.request_quiesce_for_upgrade();
        if let Some(conversation_id) = self
            .owner
            .build_input()
            .map(|input| input.static_config.conversation_id.as_str())
        {
            let _ = self.cancel_background_shell_rewatches(conversation_id);
            if let Some(watches) = self.cloud_agent_watches.as_ref() {
                let _ = watches.dispose_parent(conversation_id);
            }
        }
    }

    pub fn cancel_quiesce_for_upgrade(&self) {
        self.owner.cancel_quiesce_for_upgrade();
    }

    pub fn is_quiescing_for_upgrade(&self) -> bool {
        self.owner.is_quiescing_for_upgrade()
    }

    pub fn is_awaiting_user_selection(&self) -> bool {
        matches!(
            self.owner.last_finished().map(|finished| &finished.outcome),
            Some(TerminalOutcome::WaitingUser)
        )
    }

    pub fn begin_generated_subagent(
        &self,
        parent_agent_id: &str,
        box_id: &str,
        subagent_agent_id: &str,
        subagent_type: &str,
        tool_call_id: &str,
        prompt: &str,
        lineage: Option<SubagentLineage>,
        now_ms: u64,
    ) -> Result<Option<PendingWake>, String> {
        self.begin_generated_subagent_with_lifecycle(
            parent_agent_id,
            box_id,
            subagent_agent_id,
            subagent_type,
            tool_call_id,
            prompt,
            lineage,
            "",
            now_ms,
        )
    }

    pub fn begin_generated_subagent_with_lifecycle(
        &self,
        parent_agent_id: &str,
        box_id: &str,
        subagent_agent_id: &str,
        subagent_type: &str,
        tool_call_id: &str,
        prompt: &str,
        lineage: Option<SubagentLineage>,
        lifecycle_revision: &str,
        now_ms: u64,
    ) -> Result<Option<PendingWake>, String> {
        if lifecycle_revision.trim().is_empty() {
            return Err("generated subagent lifecycle revision is required".into());
        }
        let Some(runtime) = self.generated_agent_runtime.as_ref() else {
            return Ok(None);
        };
        let mut runtime = runtime
            .lock()
            .map_err(|_| "generated Agent runtime lock poisoned".to_string())?;
        runtime.register_session(subagent_agent_id, SubagentSessionSnapshot::default());
        Ok(runtime.dispatch_background_subagent_with_lifecycle(
            parent_agent_id,
            box_id,
            subagent_agent_id,
            subagent_type,
            tool_call_id,
            prompt,
            lineage,
            None,
            lifecycle_revision,
            now_ms,
        ))
    }

    pub fn settle_generated_subagent(
        &self,
        subagent_agent_id: &str,
        outcome: RunOutcome,
        now_ms: u64,
    ) -> Result<SettleResult, String> {
        let Some(runtime) = self.generated_agent_runtime.as_ref() else {
            return Ok(SettleResult::default());
        };
        runtime
            .lock()
            .map_err(|_| "generated Agent runtime lock poisoned".to_string())
            .map(|mut runtime| {
                runtime.settle_background_subagent_turn(subagent_agent_id, outcome, now_ms)
            })
    }

    pub fn list_subagents(&self) -> Result<Vec<(String, SubagentRecord)>, String> {
        let Some(runtime) = self.generated_agent_runtime.as_ref() else {
            return Ok(Vec::new());
        };
        runtime
            .lock()
            .map_err(|_| "generated Agent runtime lock poisoned".to_string())
            .map(|runtime| runtime.list_subagents())
    }

    pub fn list_running_subagents(&self, now_ms: u64) -> Result<Vec<RunningSubagentInfo>, String> {
        let Some(runtime) = self.generated_agent_runtime.as_ref() else {
            return Ok(Vec::new());
        };
        runtime
            .lock()
            .map_err(|_| "generated Agent runtime lock poisoned".to_string())
            .map(|runtime| runtime.list_running_subagents(now_ms))
    }

    pub fn get_running_subagent(
        &self,
        subagent_agent_id: &str,
        now_ms: u64,
    ) -> Result<Option<RunningSubagentInfo>, String> {
        let Some(runtime) = self.generated_agent_runtime.as_ref() else {
            return Ok(None);
        };
        runtime
            .lock()
            .map_err(|_| "generated Agent runtime lock poisoned".to_string())
            .map(|runtime| runtime.get_running_subagent(subagent_agent_id, now_ms))
    }

    pub fn get_subagent_outline(&self, subagent_agent_id: &str) -> Result<Vec<Value>, String> {
        let Some(runtime) = self.generated_agent_runtime.as_ref() else {
            return Ok(Vec::new());
        };
        runtime
            .lock()
            .map_err(|_| "generated Agent runtime lock poisoned".to_string())
            .map(|runtime| runtime.get_subagent_outline(subagent_agent_id))
    }

    pub fn has_subagent(&self, subagent_agent_id: &str) -> Result<bool, String> {
        let Some(runtime) = self.generated_agent_runtime.as_ref() else {
            return Ok(false);
        };
        runtime
            .lock()
            .map_err(|_| "generated Agent runtime lock poisoned".to_string())
            .map(|runtime| runtime.has_subagent(subagent_agent_id))
    }

    pub fn has_running_subagents(&self) -> Result<bool, String> {
        let Some(runtime) = self.generated_agent_runtime.as_ref() else {
            return Ok(false);
        };
        runtime
            .lock()
            .map_err(|_| "generated Agent runtime lock poisoned".to_string())
            .map(|runtime| runtime.has_running_subagents())
    }

    pub fn steer_subagent(
        &self,
        subagent_agent_id: &str,
        message: &str,
    ) -> Result<ControlResult, String> {
        let Some(runtime) = self.generated_agent_runtime.as_ref() else {
            return Ok(ControlResult::NotRunning);
        };
        runtime
            .lock()
            .map_err(|_| "generated Agent runtime lock poisoned".to_string())
            .map(|mut runtime| runtime.steer_subagent(subagent_agent_id, message))
    }

    pub fn abort_subagent(&self, subagent_agent_id: &str) -> Result<ControlResult, String> {
        let Some(runtime) = self.generated_agent_runtime.as_ref() else {
            return Ok(ControlResult::NotRunning);
        };
        runtime
            .lock()
            .map_err(|_| "generated Agent runtime lock poisoned".to_string())
            .map(|mut runtime| runtime.abort_subagent(subagent_agent_id))
    }

    pub fn reset(&self) -> Result<(), String> {
        if let Some(runtime) = self.generated_agent_runtime.as_ref() {
            runtime
                .lock()
                .map_err(|_| "generated Agent runtime lock poisoned".to_string())?
                .reset();
        }
        Ok(())
    }

    pub fn dispose(&mut self) {
        if let Some(conversation_id) = self
            .owner
            .build_input()
            .map(|input| input.static_config.conversation_id.as_str())
        {
            if let Some(watches) = self.background_shell_watches.as_ref() {
                let _ = watches.dispose_parent(conversation_id);
            }
            if let Some(watches) = self.cloud_agent_watches.as_ref() {
                let _ = watches.dispose_parent(conversation_id);
            }
        }
        self.owner.dispose();
    }

    pub fn cancellation(&self) -> RoutedProviderCancellation {
        self.owner.cancellation()
    }

    pub fn last_finished(&self) -> Option<&TurnRunFinished> {
        self.owner.last_finished()
    }

    pub fn run_routed_provider(
        &mut self,
        data_dir: &Path,
        messages: &[ProviderMessage],
        on_text_delta: &mut dyn FnMut(&str, &str),
    ) -> Result<String, ProviderSessionError> {
        self.owner
            .run_routed_provider(data_dir, messages, on_text_delta)
    }

    pub fn run_routed_provider_with_options(
        &mut self,
        data_dir: &Path,
        messages: &[ProviderMessage],
        options: TurnRunOptions,
        on_text_delta: &mut dyn FnMut(&str, &str),
    ) -> Result<String, ProviderSessionError> {
        self.owner
            .run_routed_provider_with_options(data_dir, messages, options, on_text_delta)
    }

    pub fn run_routed_provider_with_projected_messages(
        &mut self,
        data_dir: &Path,
        lifecycle_messages: &[ProviderMessage],
        provider_messages: &[ProviderMessage],
        options: TurnRunOptions,
        on_text_delta: &mut dyn FnMut(&str, &str),
    ) -> Result<String, ProviderSessionError> {
        self.owner.run_routed_provider_with_projected_messages(
            data_dir,
            lifecycle_messages,
            provider_messages,
            options,
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
        self.owner.run_routed_provider_with_projected_turn_input(
            data_dir,
            lifecycle_messages,
            provider_messages,
            turn_input,
            on_text_delta,
        )
    }

    pub fn run_with<Execute>(
        &mut self,
        messages: &[ProviderMessage],
        execute: Execute,
    ) -> Result<String, ProviderSessionError>
    where
        Execute: FnOnce() -> Result<String, ProviderSessionError>,
    {
        self.owner.run_with(messages, execute)
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
        self.owner.run_with_options(messages, options, execute)
    }
}
