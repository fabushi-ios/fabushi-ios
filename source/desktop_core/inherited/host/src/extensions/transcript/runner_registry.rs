use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::extensions::inference::provider_session::ProviderSessionError;
use crate::extensions::turn_execution::turn_execution_service::TurnExecutionError;
use crate::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedProviderTaskRegistry,
};

use super::transcript_manager::TranscriptTurnExecutionPort;

pub const RUN_WATCHDOG_INTERRUPT_REASON: &str =
    "run-queue watchdog: releasing a wedged predecessor";
pub const RUN_DIRECT_USER_INTERRUPT_REASON: &str = "superseded by a new user message";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RoutedSupersedeState {
    dispatched: bool,
    recovery_shaped: bool,
}

#[derive(Clone, Default)]
pub struct TranscriptRunnerRegistry {
    routed_provider_tasks: Arc<RoutedProviderTaskRegistry>,
    group_member_tasks: Arc<RoutedProviderTaskRegistry>,
    current_routed_streams: Arc<Mutex<HashMap<String, String>>>,
    current_group_member_streams: Arc<Mutex<HashMap<String, String>>>,
    routed_supersede_state: Arc<Mutex<HashMap<String, RoutedSupersedeState>>>,
    dm_preempted_group_members: Arc<Mutex<HashSet<String>>>,
    turn_execution: Arc<Mutex<Option<TranscriptTurnExecutionPort>>>,
    quiescing_for_upgrade: Arc<AtomicBool>,
}

impl TranscriptRunnerRegistry {
    pub fn new(routed_provider_tasks: Arc<RoutedProviderTaskRegistry>) -> Self {
        Self {
            routed_provider_tasks,
            group_member_tasks: Arc::new(RoutedProviderTaskRegistry::default()),
            current_routed_streams: Arc::new(Mutex::new(HashMap::new())),
            current_group_member_streams: Arc::new(Mutex::new(HashMap::new())),
            routed_supersede_state: Arc::new(Mutex::new(HashMap::new())),
            dm_preempted_group_members: Arc::new(Mutex::new(HashSet::new())),
            turn_execution: Arc::new(Mutex::new(None)),
            quiescing_for_upgrade: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_turn_execution(&self, execution: TranscriptTurnExecutionPort) {
        *self
            .turn_execution
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(execution);
    }

    pub fn turn_execution(&self) -> Option<TranscriptTurnExecutionPort> {
        self.turn_execution
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn can_execute(&self) -> bool {
        self.turn_execution()
            .is_some_and(|execution| execution.can_execute())
    }

    pub fn can_execute_group_member(&self) -> bool {
        self.turn_execution()
            .is_some_and(|execution| execution.can_execute_group_member())
    }

    pub async fn is_run_ready(&self) -> bool {
        let Some(execution) = self.turn_execution() else {
            return true;
        };
        execution.is_run_ready().await
    }

    pub fn create_runner(&self, session: Value, hooks: Value) -> Result<Value, TurnExecutionError> {
        self.turn_execution()
            .ok_or(TurnExecutionError::Unbound)?
            .create_runner(session, hooks)
    }

    pub fn create_group_member_runner(
        &self,
        session: Value,
        hooks: Value,
        overrides: Value,
    ) -> Result<Value, TurnExecutionError> {
        self.turn_execution()
            .ok_or(TurnExecutionError::Unbound)?
            .create_group_member_runner(session, hooks, overrides)
    }

    pub fn register_routed_provider(
        &self,
        agent_id: &str,
        stream_id: &str,
    ) -> Result<RoutedProviderCancellation, ProviderSessionError> {
        self.register_routed_provider_with_recovery_shape(agent_id, stream_id, false)
    }

    pub fn register_routed_provider_with_recovery_shape(
        &self,
        agent_id: &str,
        stream_id: &str,
        recovery_shaped: bool,
    ) -> Result<RoutedProviderCancellation, ProviderSessionError> {
        let cancellation = self
            .routed_provider_tasks
            .register_for_agent(agent_id, stream_id)?;
        self.routed_supersede_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                stream_id.to_string(),
                RoutedSupersedeState {
                    dispatched: false,
                    recovery_shaped,
                },
            );
        self.note_current_stream(&self.current_routed_streams, agent_id, stream_id);
        Ok(cancellation)
    }

    pub fn mark_routed_provider_dispatched(&self, stream_id: &str) -> bool {
        let mut states = self
            .routed_supersede_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(state) = states.get_mut(stream_id) else {
            return false;
        };
        state.dispatched = true;
        true
    }

    pub fn register_group_member(
        &self,
        agent_id: &str,
        stream_id: &str,
    ) -> Result<RoutedProviderCancellation, ProviderSessionError> {
        let cancellation = self
            .group_member_tasks
            .register_for_agent(agent_id, stream_id)?;
        self.note_current_stream(&self.current_group_member_streams, agent_id, stream_id);
        Ok(cancellation)
    }

    pub fn finish_routed_provider(&self, stream_id: &str) {
        self.routed_provider_tasks.finish(stream_id);
        self.group_member_tasks.finish(stream_id);
        self.clear_current_stream_if_matches(&self.current_routed_streams, stream_id);
        self.clear_current_stream_if_matches(&self.current_group_member_streams, stream_id);
        self.routed_supersede_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(stream_id);
    }

    pub fn cancel_stream(&self, stream_id: &str, reason: impl Into<String>) -> bool {
        let reason = reason.into();
        self.routed_provider_tasks.cancel(stream_id, reason.clone())
            || self.group_member_tasks.cancel(stream_id, reason)
    }

    pub fn agent_id_for_stream(&self, stream_id: &str) -> Option<String> {
        self.routed_provider_tasks
            .agent_id_for_stream(stream_id)
            .or_else(|| self.group_member_tasks.agent_id_for_stream(stream_id))
    }

    pub fn current_routed_stream_id_for_agent(&self, agent_id: &str) -> Option<String> {
        self.current_stream_for_agent(&self.current_routed_streams, agent_id)
    }

    pub fn current_group_member_stream_id_for_agent(&self, agent_id: &str) -> Option<String> {
        self.current_stream_for_agent(&self.current_group_member_streams, agent_id)
    }

    pub fn interrupt_wedged_run_for_watchdog(&self, agent_id: &str) -> bool {
        let routed = self
            .current_routed_stream_id_for_agent(agent_id)
            .is_some_and(|stream_id| {
                self.routed_provider_tasks
                    .cancel(&stream_id, RUN_WATCHDOG_INTERRUPT_REASON)
            });
        let group_member = self
            .current_group_member_stream_id_for_agent(agent_id)
            .is_some_and(|stream_id| {
                self.group_member_tasks
                    .cancel(&stream_id, RUN_WATCHDOG_INTERRUPT_REASON)
            });
        routed || group_member
    }

    pub fn cancel_agent(&self, agent_id: &str, reason: impl Into<String>) -> usize {
        self.routed_provider_tasks.cancel_agent(agent_id, reason)
    }

    pub fn cancel_group_member_agent(&self, agent_id: &str, reason: impl Into<String>) -> usize {
        self.group_member_tasks.cancel_agent(agent_id, reason)
    }

    pub fn preempt_routed_agent(&self, agent_id: &str, reason: impl Into<String>) -> usize {
        self.preempt_routed_agent_for_supersede(agent_id, reason, None)
    }

    pub fn preempt_routed_agent_for_supersede(
        &self,
        agent_id: &str,
        reason: impl Into<String>,
        carries_recovery: Option<bool>,
    ) -> usize {
        let Some(stream_id) = self.current_routed_stream_id_for_agent(agent_id) else {
            return 0;
        };
        if let Some(carries_recovery) = carries_recovery {
            let states = self
                .routed_supersede_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(state) = states.get(&stream_id) {
                if !state.dispatched && (!carries_recovery || !state.recovery_shaped) {
                    return 0;
                }
            }
        }
        usize::from(self.routed_provider_tasks.cancel(&stream_id, reason.into()))
    }

    pub fn preempt_group_member_agent(&self, agent_id: &str, reason: impl Into<String>) -> usize {
        let Some(stream_id) = self.current_group_member_stream_id_for_agent(agent_id) else {
            return 0;
        };
        if !self.group_member_tasks.cancel(&stream_id, reason.into()) {
            return 0;
        }
        self.dm_preempted_group_members
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(agent_id.to_string());
        1
    }

    pub fn take_group_member_preempted(&self, agent_id: &str) -> bool {
        self.dm_preempted_group_members
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(agent_id)
    }

    pub fn clear_group_member_preempted(&self, agent_id: &str) {
        self.dm_preempted_group_members
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(agent_id);
    }

    pub fn active_stream_ids_for_agent(&self, agent_id: &str) -> Vec<String> {
        let mut streams = self
            .routed_provider_tasks
            .active_stream_ids_for_agent(agent_id);
        streams.extend(
            self.group_member_tasks
                .active_stream_ids_for_agent(agent_id),
        );
        streams.sort();
        streams
    }

    pub fn active_group_member_stream_ids_for_agent(&self, agent_id: &str) -> Vec<String> {
        self.group_member_tasks
            .active_stream_ids_for_agent(agent_id)
    }

    pub fn active_count(&self) -> usize {
        self.routed_provider_tasks.active_count() + self.group_member_tasks.active_count()
    }

    pub fn upgrade_quiesce_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.quiescing_for_upgrade)
    }

    pub fn request_quiesce_for_upgrade(&self) {
        self.quiescing_for_upgrade.store(true, Ordering::Release);
        self.routed_provider_tasks
            .cancel_all("quiescing for forced host upgrade");
        self.group_member_tasks
            .cancel_all("quiescing for forced host upgrade");
    }

    pub fn cancel_quiesce_for_upgrade(&self) {
        self.quiescing_for_upgrade.store(false, Ordering::Release);
    }

    pub fn is_quiescing_for_upgrade(&self) -> bool {
        self.quiescing_for_upgrade.load(Ordering::Acquire)
    }

    pub fn cancel_all(&self, reason: &str) {
        self.routed_provider_tasks.cancel_all(reason);
        self.group_member_tasks.cancel_all(reason);
    }

    fn note_current_stream(
        &self,
        current: &Mutex<HashMap<String, String>>,
        agent_id: &str,
        stream_id: &str,
    ) {
        current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(agent_id.to_string(), stream_id.to_string());
    }

    fn current_stream_for_agent(
        &self,
        current: &Mutex<HashMap<String, String>>,
        agent_id: &str,
    ) -> Option<String> {
        current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(agent_id)
            .cloned()
    }

    fn clear_current_stream_if_matches(
        &self,
        current: &Mutex<HashMap<String, String>>,
        stream_id: &str,
    ) {
        current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, current_stream_id| current_stream_id != stream_id);
    }
}
