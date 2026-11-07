use std::path::Path;
use uuid::Uuid;

use crate::agents::agent_profile::SandAgentProfile;
use crate::extensions::memory::agent_state::{MemoryTier, StateWriteResult, MEMORY_NOTE_PREFIX};
use crate::extensions::memory::memory_service::{FileMemoryStore, MemoryKind, MemoryRecoveryRecord};

use super::agent_db::{
    PendingMemoryMutation, PendingProfileMutation, ProfileFileSnapshot, SandAgentDb,
};
use super::agent_db_serde::{MemoryPromptSnapshot, SandProfile};
use super::production::ProductionSessionWorkers;
use super::session_mutations::set_agent_avatar_bytes as mutate_agent_avatar_bytes;
use super::session_mutations::{
    capture_profile_mutation_files, profile_mutation_files_match,
    restore_profile_mutation_files,
};
use super::session_profile_files::{
    AgentAvatarResponse, AgentProfileUpdate, get_agent_avatar as get_profile_avatar,
    get_agent_avatar_png as get_profile_avatar_png, write_agent_profile_update,
};
use super::session_summaries::AgentSummary;

impl ProductionSessionWorkers {
    pub(crate) fn restore_memory_records(
        store: &FileMemoryStore,
        records: &[MemoryRecoveryRecord],
    ) -> Result<(), String> {
        store
            .restore_records_exact(records)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn prepare_memory_mutation(
        owner: &SandAgentDb,
        operation: &str,
        records: Vec<MemoryRecoveryRecord>,
        prior_snapshot: Option<MemoryPromptSnapshot>,
    ) -> Result<PendingMemoryMutation, String> {
        let admission = owner
            .read_session_work_admission()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "memory mutation requires a session admission".to_string())?;
        let mutation = PendingMemoryMutation {
            mutation_id: format!("memory-{operation}:{}", Uuid::new_v4()),
            operation: operation.to_string(),
            session_key: admission.session_key,
            session_id: admission.session_id,
            lifecycle_revision: admission.lifecycle_revision,
            records,
            prior_snapshot,
        };
        if !owner
            .prepare_pending_memory_mutation(&mutation)
            .map_err(|error| error.to_string())?
        {
            return Err("memory mutation could not establish its durable journal".into());
        }
        Ok(mutation)
    }

    pub(crate) fn rollback_memory_mutation(
        owner: &SandAgentDb,
        store: &FileMemoryStore,
        mutation: &PendingMemoryMutation,
    ) -> Result<(), String> {
        Self::restore_memory_records(store, &mutation.records)?;
        if !owner
            .clear_pending_memory_mutation(mutation)
            .map_err(|error| error.to_string())?
        {
            return Err("memory mutation journal could not be cleared after rollback".into());
        }
        Ok(())
    }

    pub fn write_agent_memory(
        &self,
        agent_id: &str,
        content: &str,
        tier: MemoryTier,
    ) -> StateWriteResult {
        let result = self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_work_admission_locked(agent_id)?;
            let store = self.memory_service.store_for_agent(agent_id);
            let payload = match tier {
                MemoryTier::Note => format!("{MEMORY_NOTE_PREFIX}{}", content.trim()),
                MemoryTier::Profile | MemoryTier::Log => content.trim().to_string(),
            };
            let kind = match tier {
                MemoryTier::Profile => MemoryKind::Profile,
                MemoryTier::Note | MemoryTier::Log => MemoryKind::Log,
            };
            store
                .add_memory(&payload, chrono::Utc::now().timestamp_millis(), kind)
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(Some(record)) => StateWriteResult::success(format!(
                "Remembered in agent memory ({}): {}",
                match tier {
                    MemoryTier::Profile => "profile",
                    MemoryTier::Note => "note",
                    MemoryTier::Log => "log",
                },
                record.content
            )),
            Ok(None) => StateWriteResult::failure(
                "nothing was saved to agent memory - the fact was empty or already recorded.",
            ),
            Err(error) => StateWriteResult::failure(format!("could not update agent memory: {error}")),
        }
    }

    pub fn remove_agent_memory(&self, agent_id: &str, memory_id: &str) -> Result<bool, String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_work_admission_locked(agent_id)?;
            let owner = self.open_agent_db_owner_locked(agent_id)?;
            let prior_snapshot = owner
                .get_memory_prompt_snapshot()
                .map_err(|error| error.to_string())?;
            let store = self.memory_service.store_for_agent(agent_id);
            let previous = store
                .list_memories(usize::MAX)
                .into_iter()
                .find(|record| record.id == memory_id);
            let Some(previous) = previous else {
                return Ok(false);
            };
            let mutation = Self::prepare_memory_mutation(
                &owner,
                "remove",
                store.snapshot_for_recovery(&[previous.clone()]),
                prior_snapshot,
            )?;
            let removed = match store.remove_memory(memory_id) {
                Ok(removed) => removed,
                Err(error) => {
                    Self::rollback_memory_mutation(&owner, &store, &mutation)
                        .map_err(|rollback| format!("memory remove failed: {error}; {rollback}"))?;
                    return Err(error.to_string());
                }
            };
            if !removed {
                Self::rollback_memory_mutation(&owner, &store, &mutation)?;
                return Ok(false);
            }
            match owner.commit_pending_memory_mutation(&mutation) {
                Ok(true) => Ok(true),
                Ok(false) => {
                    Self::rollback_memory_mutation(&owner, &store, &mutation)
                        .map_err(|rollback| format!("memory settlement was rejected; {rollback}"))?;
                    Err("memory settlement was rejected by the session owner".into())
                }
                Err(error) => Err(format!(
                    "memory settlement outcome unknown; journal retained for recovery: {error}"
                )),
            }
        })
    }

    pub(crate) fn prepare_profile_mutation(
        owner: &SandAgentDb,
        operation: &str,
        before_files: Vec<ProfileFileSnapshot>,
        next_profile: Option<SandProfile>,
    ) -> Result<PendingProfileMutation, String> {
        let admission = owner
            .read_session_work_admission()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "profile mutation requires a session admission".to_string())?;
        let previous_profile = owner
            .get_sand_profile()
            .map_err(|error| error.to_string())?;
        let mutation = PendingProfileMutation {
            mutation_id: format!("profile-{}:{}", operation, Uuid::new_v4()),
            operation: operation.to_string(),
            session_key: admission.session_key,
            session_id: admission.session_id,
            lifecycle_revision: admission.lifecycle_revision,
            before_files,
            after_files: Vec::new(),
            previous_profile,
            next_profile,
        };
        if !owner
            .prepare_pending_profile_mutation(&mutation)
            .map_err(|error| error.to_string())?
        {
            return Err("profile mutation could not establish its durable journal".into());
        }
        Ok(mutation)
    }

    pub(crate) fn rollback_profile_mutation(
        owner: &SandAgentDb,
        agent_dir: &Path,
        mutation: &PendingProfileMutation,
    ) -> Result<(), String> {
        if !profile_mutation_files_match(agent_dir, &mutation.before_files) {
            if !profile_mutation_files_match(agent_dir, &mutation.after_files) {
                return Err("profile mutation rollback found an unknown file state".into());
            }
            restore_profile_mutation_files(agent_dir, &mutation.before_files)?;
        }
        if !owner
            .clear_pending_profile_mutation(mutation)
            .map_err(|error| error.to_string())?
        {
            return Err("profile mutation journal could not be cleared after rollback".into());
        }
        Ok(())
    }

    pub fn update_agent_profile(
        &self,
        agent_id: &str,
        update: &AgentProfileUpdate,
        active_agent_id: Option<&str>,
    ) -> Result<Option<AgentSummary>, String> {
        let db_path = self.existing_session_db_path(agent_id)?;
        let agent_dir = db_path
            .parent()
            .ok_or_else(|| "agent database has no parent directory".to_string())?;
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_work_admission_locked(agent_id)?;
            let owner = self.open_agent_db_owner_locked(agent_id)?;
            let before_files = capture_profile_mutation_files(agent_dir, true)?;
            let mutation = Self::prepare_profile_mutation(
                &owner,
                "profile-update",
                before_files,
                None,
            )?;
            if let Err(error) = write_agent_profile_update(agent_dir, update) {
                Self::rollback_profile_mutation(&owner, agent_dir, &mutation)
                    .map_err(|rollback| format!("profile update failed: {error}; {rollback}"))?;
                return Err(error.to_string());
            }
            let mut sealed = mutation.clone();
            sealed.after_files = capture_profile_mutation_files(agent_dir, true)?;
            match owner.seal_pending_profile_mutation(&sealed) {
                Ok(true) => {}
                Ok(false) => {
                    Self::rollback_profile_mutation(&owner, agent_dir, &mutation)
                        .map_err(|rollback| format!("profile mutation seal rejected; {rollback}"))?;
                    return Err("profile mutation seal was rejected by the session owner".into());
                }
                Err(error) => {
                    return Err(format!(
                        "profile mutation seal outcome unknown; journal retained for recovery: {error}"
                    ));
                }
            }
            match owner.commit_pending_profile_mutation(&sealed) {
                Ok(true) => Ok(()),
                Ok(false) => {
                    Self::rollback_profile_mutation(&owner, agent_dir, &sealed)
                        .map_err(|rollback| format!("profile settlement was rejected; {rollback}"))?;
                    Err("profile settlement was rejected by the session owner".into())
                }
                Err(error) => Err(format!(
                    "profile settlement outcome unknown; journal retained for recovery: {error}"
                )),
            }
        })?;
        self.summarize_agent_by_id(agent_id, active_agent_id)
    }
}
