use super::agent_db::{CompactionCommit, CompactionCommitOutcome};
use super::agent_db_serde::SpendGuardState;
use super::pending_card_sweeps::{
    expire_pending_auto_review_approval_entries,
    expire_pending_local_tool_permission_ask_entries,
};
use super::production::{ProductionSessionWorkers, hide_session_work_receipts};
use super::typed_replay::{TypedReplayEntry, project_typed_replay_entries};

impl ProductionSessionWorkers {
    pub fn append_agent_transcript_entries(
        &self,
        agent_id: &str,
        entries: &[serde_json::Value],
    ) -> Result<usize, String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_transcript_write_admission_locked(agent_id)?;
            self.open_agent_db_owner_locked(agent_id)?
                .append_transcript_entries(entries)
                .map_err(|error| error.to_string())
        })
    }

    pub fn append_agent_transcript_entries_if_session_admitted(
        &self,
        agent_id: &str,
        session_key: &str,
        entries: &[serde_json::Value],
    ) -> Result<usize, String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_transcript_write_admission_locked(agent_id)?;
            self.open_agent_db_owner_locked(agent_id)?
                .append_transcript_entries_if_session_admitted(session_key, entries)
                .map_err(|error| error.to_string())
        })
    }

    /// Append room-owned transcript state while the caller already holds the
    /// per-agent filesystem/admission boundary. This keeps external producers
    /// from reacquiring the non-reentrant mutation lock.
    pub(crate) fn append_agent_transcript_entries_locked(
        &self,
        agent_id: &str,
        session_key: Option<&str>,
        entries: &[serde_json::Value],
    ) -> Result<usize, String> {
        self.assert_agent_transcript_write_admission_locked(agent_id)?;
        let owner = self.open_agent_db_owner_locked(agent_id)?;
        match session_key {
            Some(session_key) => owner
                .append_transcript_entries_if_session_admitted(session_key, entries)
                .map_err(|error| error.to_string()),
            None => owner
                .append_transcript_entries(entries)
                .map_err(|error| error.to_string()),
        }
    }

    pub fn append_agent_spend_guard_card_if_session_admitted(
        &self,
        agent_id: &str,
        session_key: &str,
        entry: &serde_json::Value,
        state: &SpendGuardState,
        activity_at: f64,
    ) -> Result<bool, String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_transcript_write_admission_locked(agent_id)?;
            self.open_agent_db_owner_locked(agent_id)?
                .append_spend_guard_card_if_session_admitted(
                    session_key,
                    entry,
                    state,
                    activity_at,
                )
                .map_err(|error| error.to_string())
        })
    }

    pub fn append_agent_transcript_entries_and_release_media(
        &self,
        agent_id: &str,
        session_key: &str,
        entries: &[serde_json::Value],
        expected_lifecycle_revision: &str,
        media_work_id: &str,
    ) -> Result<usize, String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_transcript_write_admission_locked(agent_id)?;
            self.open_agent_db_owner_locked(agent_id)?
                .append_transcript_entries_and_release_media(
                    entries,
                    session_key,
                    expected_lifecycle_revision,
                    media_work_id,
                )
                .map_err(|error| error.to_string())
        })
    }

    pub fn commit_agent_compaction(
        &self,
        agent_id: &str,
        commit: &CompactionCommit,
    ) -> Result<CompactionCommitOutcome, String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_transcript_write_admission_locked(agent_id)?;
            self.open_agent_db_owner_locked(agent_id)?
                .commit_compaction(commit)
                .map_err(|error| error.to_string())
        })
    }

    pub fn read_agent_compacted_transcript_entries(
        &self,
        agent_id: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let mut entries = self
            .open_agent_db_owner(agent_id)?
            .get_compacted_transcript_entries()
            .map_err(|error| error.to_string())?;
        hide_session_work_receipts(&mut entries);
        Ok(entries)
    }

    /// Read the canonical compacted replay through the strict typed transcript
    /// boundary. Unsupported rows are rejected before any inference owner can
    /// serialize them into provider text.
    pub fn read_agent_typed_compacted_transcript_entries(
        &self,
        agent_id: &str,
    ) -> Result<Vec<TypedReplayEntry>, String> {
        let entries = self.read_agent_compacted_transcript_entries(agent_id)?;
        if !entries.iter().any(|entry| {
            entry.get("kind").and_then(serde_json::Value::as_str) == Some("compaction")
        }) {
            return Ok(Vec::new());
        }
        project_typed_replay_entries(&entries).map_err(|error| error.to_string())
    }

    pub fn update_agent_transcript_entry(
        &self,
        agent_id: &str,
        entry_id: &str,
        next: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_transcript_write_admission_locked(agent_id)?;
            self.open_agent_db_owner_locked(agent_id)?
                .update_transcript_entry(entry_id, next)
                .map_err(|error| error.to_string())
        })
    }

    pub fn update_agent_transcript_entry_if_session_admitted(
        &self,
        agent_id: &str,
        session_key: &str,
        entry_id: &str,
        next: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_work_admission_locked(agent_id)?;
            self.open_agent_db_owner_locked(agent_id)?
                .update_transcript_entry_if_session_admitted(session_key, entry_id, next)
                .map_err(|error| error.to_string())
        })
    }

    pub fn delete_agent_transcript_entry(
        &self,
        agent_id: &str,
        entry_id: &str,
    ) -> Result<bool, String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_transcript_write_admission_locked(agent_id)?;
            self.open_agent_db_owner_locked(agent_id)?
                .delete_transcript_entry(entry_id)
                .map_err(|error| error.to_string())
        })
    }

    pub fn clear_agent_conversation(&self, agent_id: &str) -> Result<bool, String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_transcript_write_admission_locked(agent_id)?;
            let store = self.create_agent_blob_store(agent_id)?;
            self.existing_session_db_path(agent_id)?;
            futures::executor::block_on(self.pool.clear_blobs(
                agent_id,
                &store.blob_db_path,
                store.legacy_blob_db_path.as_deref(),
            ))
            .map_err(|error| error.to_string())?;
            self.open_agent_db_owner_locked(agent_id)?
                .clear_conversation()
                .map_err(|error| error.to_string())
        })
    }

    pub fn get_agent_profile_prompt_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        self.open_agent_db_owner(agent_id)?
            .get_agent_profile_prompt_snapshot()
            .map_err(|error| error.to_string())
    }

    pub fn set_agent_profile_prompt_snapshot(
        &self,
        agent_id: &str,
        snapshot: &serde_json::Value,
    ) -> Result<bool, String> {
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            self.open_agent_db_owner_locked(agent_id)?
                .set_agent_profile_prompt_snapshot(snapshot)
                .map_err(|error| error.to_string())
        })
    }

    pub fn clear_agent_profile_prompt_snapshot(&self, agent_id: &str) -> Result<bool, String> {
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            self.open_agent_db_owner_locked(agent_id)?
                .clear_agent_profile_prompt_snapshot()
                .map_err(|error| error.to_string())
        })
    }

    pub fn clear_agent_transient_state(&self, agent_id: &str) -> Result<bool, String> {
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            self.open_agent_db_owner_locked(agent_id)?
                .clear_transient_state()
                .map_err(|error| error.to_string())
        })
    }

    pub fn expire_pending_auto_review_approvals(
        &self,
        agent_id: &str,
        only_request_id: Option<&str>,
    ) -> Result<Vec<String>, String> {
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            let owner = self.open_agent_db_owner_locked(agent_id)?;
            expire_pending_auto_review_approval_entries(
                owner.as_ref(),
                only_request_id,
            )
            .map_err(|error| error.to_string())
        })
    }

    pub fn expire_pending_local_tool_permission_asks(
        &self,
        agent_id: &str,
        only_request_id: Option<&str>,
        if_pending_before_ms: Option<f64>,
    ) -> Result<Vec<String>, String> {
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            let owner = self.open_agent_db_owner_locked(agent_id)?;
            expire_pending_local_tool_permission_ask_entries(
                owner.as_ref(),
                only_request_id,
                if_pending_before_ms,
            )
            .map_err(|error| error.to_string())
        })
    }
}
