//! OCRM Session Work Admission Submodule (OCRM-MODULAR-01)
//!
//! Decomposed from `production.rs` to satisfy single responsibility
//! and cohesive file size budget. Implements `ProductionSessionWorkers` work admission,
//! media custody, session work archiving, and result commitment fences.

use std::path::Path;

use super::agent_db::{SessionWorkAdmissionRecord, SessionWorkResultCommitOutcome};
use super::production::ProductionSessionWorkers;

impl ProductionSessionWorkers {
    /// Reject new session-owned writes while the existing deletion owner has
    /// custody of the agent directory. Deletion itself uses its own file and
    /// store owner path and does not call these transcript mutation wrappers.
    pub fn assert_agent_work_admission(&self, agent_id: &str) -> Result<(), String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_work_admission_locked(agent_id)
        })
    }

    /// Admission check for callers that already hold the per-agent
    /// filesystem/deletion boundary. This must not call the public admission
    /// facade, because that facade intentionally acquires the same mutex.
    pub(crate) fn assert_agent_work_admission_locked(&self, agent_id: &str) -> Result<(), String> {
        if self.is_agent_being_deleted(agent_id) {
            return Err(format!(
                "agent {agent_id} deletion cleanup is pending; new session work is not admitted"
            ));
        }
        self.existing_session_db_path(agent_id)?;
        let Some(record) = self
            .open_agent_db_owner_locked(agent_id)?
            .read_session_work_admission()
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let expected_session_key = format!("agent:{agent_id}:main");
        if record.session_id != agent_id
            || record.session_key != expected_session_key
            || record.lifecycle_revision.trim().is_empty()
        {
            return Err(format!(
                "agent {agent_id} session admission identity is malformed"
            ));
        }
        if record.deletion_pending {
            return Err(format!(
                "agent {agent_id} durable deletion cleanup is pending; new session work is not admitted"
            ));
        }
        Ok(())
    }

    pub(crate) fn assert_agent_transcript_write_admission_locked(
        &self,
        agent_id: &str,
    ) -> Result<(), String> {
        self.assert_agent_work_admission_locked(agent_id)?;
        if self
            .open_agent_db_owner_locked(agent_id)?
            .read_session_work_admission()
            .map_err(|error| error.to_string())?
            .is_some_and(|record| record.archived_at_ms.is_some())
        {
            return Err(format!(
                "agent {agent_id} session is archived; agent-only transcript writes are not admitted"
            ));
        }
        Ok(())
    }

    fn assert_agent_transcript_write_admission(&self, agent_id: &str) -> Result<(), String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_transcript_write_admission_locked(agent_id)
        })
    }

    pub fn ensure_main_session_work_admission(&self, agent_id: &str) -> Result<(), String> {
        self.with_agent_filesystem_mutation(agent_id, || {
            self.ensure_main_session_work_admission_locked(agent_id)
        })
    }

    fn ensure_main_session_work_admission_locked(&self, agent_id: &str) -> Result<(), String> {
        let db = self.open_agent_db_owner_locked(agent_id)?;
        let expected_session_key = format!("agent:{agent_id}:main");
        if let Some(record) = db
            .read_session_work_admission()
            .map_err(|error| error.to_string())?
        {
            if record.session_id != agent_id
                || record.session_key != expected_session_key
                || record.lifecycle_revision.trim().is_empty()
            {
                return Err(format!(
                    "agent {agent_id} session admission identity is malformed"
                ));
            }
            return Ok(());
        }
        let lifecycle_revision = db
            .ensure_lifecycle_revision()
            .map_err(|error| error.to_string())?;
        let record = SessionWorkAdmissionRecord {
            session_key: format!("agent:{agent_id}:main"),
            session_id: agent_id.to_string(),
            lifecycle_revision,
            deletion_pending: false,
            archived_at_ms: None,
            archived_by: None,
            archive_reason: None,
            admitted_work_id: None,
            admitted_media_work_id: None,
            admitted_media_paths: Vec::new(),
            terminal_result_idempotency_key: None,
            provider_review_id: None,
            provider_review_phase: None,
            provider_review_continuation_run_id: None,
            provider_review: None,
        };
        let changed = db
            .compare_and_set_session_work_admission(None, &record)
            .map_err(|error| error.to_string())?;
        if changed {
            return Ok(());
        }
        let current = db
            .read_session_work_admission()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!(
                    "agent {agent_id} session admission creation lost its race without a record"
                )
            })?;
        if current.session_id != agent_id
            || current.session_key != expected_session_key
            || current.lifecycle_revision.trim().is_empty()
        {
            return Err(format!(
                "agent {agent_id} session admission identity is malformed after create race"
            ));
        }
        Ok(())
    }

    pub fn assert_session_work_admission(
        &self,
        agent_id: &str,
        session_key: Option<&str>,
    ) -> Result<(), String> {
        self.assert_agent_work_admission(agent_id)?;
        let Some(session_key) = session_key.map(str::trim).filter(|value| !value.is_empty())
        else {
            return Ok(());
        };
        let Some(record) = self.read_session_work_admission(agent_id)? else {
            return Ok(());
        };
        if record.session_key != session_key {
            return Err(format!(
                "session work admission key mismatch: expected {}, got {}",
                record.session_key, session_key
            ));
        }
        if record.archived_at_ms.is_some() {
            return Err(format!(
                "session {session_key} is archived and does not admit new work"
            ));
        }
        Ok(())
    }

    /// Capture the generation that a background producer is admitting work
    /// against. The caller must carry this value to its terminal edge and
    /// recheck it before treating a detached result as committed.
    pub fn capture_session_work_admission(
        &self,
        agent_id: &str,
        session_key: &str,
        work_id: &str,
    ) -> Result<String, String> {
        let session_key = session_key.trim();
        let work_id = work_id.trim();
        if session_key.is_empty() || work_id.is_empty() {
            return Err("session work admission requires a session key and work id".into());
        }
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            self.ensure_main_session_work_admission_locked(agent_id)?;
            let owner = self.open_agent_db_owner_locked(agent_id)?;
            let record = owner
                .read_session_work_admission()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "session work admission record is unavailable".to_string())?;
            if record.session_key != session_key {
                return Err(format!(
                    "session work admission key mismatch: expected {}, got {}",
                    record.session_key, session_key
                ));
            }
            if record.archived_at_ms.is_some() {
                return Err(format!(
                    "session {session_key} is archived and does not admit new work"
                ));
            }
            if let Some(admitted_work_id) = record.admitted_work_id.as_deref() {
                if admitted_work_id != work_id {
                    return Err(format!(
                        "session {session_key} already has admitted background work"
                    ));
                }
                return Ok(record.lifecycle_revision);
            }
            let lifecycle_revision = record.lifecycle_revision.clone();
            let mut next = record;
            next.admitted_work_id = Some(work_id.to_string());
            if !owner
                .compare_and_set_session_work_admission(Some(&lifecycle_revision), &next)
                .map_err(|error| error.to_string())?
            {
                return Err(format!(
                    "session {session_key} generation changed while admitting work"
                ));
            }
            Ok(lifecycle_revision)
        })
    }

    /// Recheck the generation immediately before a background terminal result
    /// is exposed to its consumer. A missing record, rebound session, archive,
    /// deletion fence, or changed lifecycle is fail-closed.
    pub fn assert_session_work_admission_generation(
        &self,
        agent_id: &str,
        session_key: &str,
        expected_lifecycle_revision: &str,
    ) -> Result<(), String> {
        self.assert_agent_work_admission(agent_id)?;
        let record = self
            .read_session_work_admission(agent_id)?
            .ok_or_else(|| "session work admission record disappeared".to_string())?;
        if record.session_key != session_key
            || record.lifecycle_revision != expected_lifecycle_revision
            || record.archived_at_ms.is_some()
        {
            return Err(format!(
                "session work admission generation changed for {session_key}"
            ));
        }
        Ok(())
    }

    /// Capture one durable media-custody identity before an owned attachment
    /// becomes visible to the transcript/channel path.
    pub fn capture_session_media_admission(
        &self,
        agent_id: &str,
        session_key: &str,
        work_id: &str,
        media_path: Option<&Path>,
    ) -> Result<String, String> {
        let session_key = session_key.trim();
        let work_id = work_id.trim();
        if session_key.is_empty() || work_id.is_empty() {
            return Err("media custody requires a session key and work id".into());
        }
        let media_path = media_path
            .map(|path| {
                path.to_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| "media custody path must be valid UTF-8".to_string())
            })
            .transpose()?;
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            let owner = self.open_agent_db_owner_locked(agent_id)?;
            let record = owner
                .read_session_work_admission()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "session work admission record is unavailable".to_string())?;
            if record.session_key != session_key {
                return Err(format!(
                    "media custody session key mismatch: expected {}, got {}",
                    record.session_key, session_key
                ));
            }
            if record.archived_at_ms.is_some() {
                return Err(format!(
                    "session {session_key} is archived and does not admit media custody"
                ));
            }
            if record.admitted_media_work_id.is_none() && !record.admitted_media_paths.is_empty() {
                return Err(format!(
                    "session {session_key} has media paths without a pending media identity"
                ));
            }
            if let Some(admitted_media_work_id) = record.admitted_media_work_id.as_deref() {
                if admitted_media_work_id != work_id {
                    return Err(format!(
                        "session {session_key} already has pending outgoing media"
                    ));
                }
                let lifecycle_revision = record.lifecycle_revision.clone();
                let mut next = record;
                if let Some(path) = media_path.as_deref() {
                    if !next.admitted_media_paths.iter().any(|known| known == path) {
                        next.admitted_media_paths.push(path.to_string());
                        if !owner
                            .compare_and_set_session_work_admission(
                                Some(&lifecycle_revision),
                                &next,
                            )
                            .map_err(|error| error.to_string())?
                        {
                            return Err(format!(
                                "session {session_key} generation changed while recording media custody"
                            ));
                        }
                    }
                }
                return Ok(lifecycle_revision);
            }
            let lifecycle_revision = record.lifecycle_revision.clone();
            if media_path.is_none() {
                return Err(
                    "media custody requires a materialized path for new media work".into(),
                );
            }
            let mut next = record;
            next.admitted_media_work_id = Some(work_id.to_string());
            next.admitted_media_paths = media_path.into_iter().collect();
            if !owner
                .compare_and_set_session_work_admission(Some(&lifecycle_revision), &next)
                .map_err(|error| error.to_string())?
            {
                return Err(format!(
                    "session {session_key} generation changed while admitting media custody"
                ));
            }
            Ok(lifecycle_revision)
        })
    }

    /// Release media custody only for the captured session generation and
    /// identity. A failed release intentionally leaves the durable fence in
    /// place so archive/restore remains fail-closed.
    pub fn release_session_media_admission(
        &self,
        agent_id: &str,
        expected_lifecycle_revision: &str,
        work_id: &str,
    ) -> Result<bool, String> {
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            let owner = self.open_agent_db_owner_locked(agent_id)?;
            let Some(mut record) = owner
                .read_session_work_admission()
                .map_err(|error| error.to_string())?
            else {
                return Ok(false);
            };
            if record.lifecycle_revision != expected_lifecycle_revision
                || record.admitted_media_work_id.as_deref() != Some(work_id.trim())
            {
                return Ok(false);
            }
            record.admitted_media_work_id = None;
            record.admitted_media_paths.clear();
            owner
                .compare_and_set_session_work_admission(
                    Some(expected_lifecycle_revision),
                    &record,
                )
                .map_err(|error| error.to_string())
        })
    }

    pub fn read_session_work_admission(
        &self,
        agent_id: &str,
    ) -> Result<Option<SessionWorkAdmissionRecord>, String> {
        if agent_id.trim().is_empty() {
            return Err("session admission read requires a non-empty agent identity".into());
        }
        let record = self
            .open_agent_db_owner(agent_id)?
            .read_session_work_admission()
            .map_err(|error| error.to_string())?;
        let Some(record) = record else {
            return Ok(None);
        };
        let expected_session_key = format!("agent:{agent_id}:main");
        if record.session_id != agent_id
            || record.session_key != expected_session_key
            || record.lifecycle_revision.trim().is_empty()
        {
            return Err(format!(
                "agent {agent_id} session admission identity is malformed"
            ));
        }
        Ok(Some(record))
    }


    pub fn compare_and_set_session_work_admission(
        &self,
        agent_id: &str,
        expected_lifecycle_revision: Option<&str>,
        next: &SessionWorkAdmissionRecord,
    ) -> Result<bool, String> {
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            self.open_agent_db_owner_locked(agent_id)?
                .compare_and_set_session_work_admission(expected_lifecycle_revision, next)
                .map_err(|error| error.to_string())
        })
    }


    pub fn transition_session_work_archive(
        &self,
        agent_id: &str,
        session_key: &str,
        expected_session_id: &str,
        expected_lifecycle_revision: Option<&str>,
        archived: bool,
        archived_by: Option<&str>,
        archive_reason: Option<&str>,
        archived_at_ms: f64,
    ) -> Result<SessionWorkAdmissionRecord, String> {
        let expected_key = format!("agent:{agent_id}:main");
        if session_key.trim() != expected_key {
            return Err("session archive transition requires the canonical main session key".into());
        }
        self.with_agent_filesystem_mutation(agent_id, || {
            self.assert_agent_work_admission_locked(agent_id)?;
            self.ensure_main_session_work_admission_locked(agent_id)?;
            self.open_agent_db_owner_locked(agent_id)?
                .transition_session_work_archive(
                    session_key,
                    expected_session_id,
                    expected_lifecycle_revision,
                    archived,
                    archived_by,
                    archive_reason,
                    archived_at_ms,
                )
                .map_err(|error| error.to_string())
        })
    }

    pub fn commit_session_work_result(
        &self,
        agent_id: &str,
        session_key: &str,
        expected_lifecycle_revision: &str,
        work_id: &str,
        idempotency_key: &str,
        content: &str,
        sent_message_count: usize,
    ) -> Result<SessionWorkResultCommitOutcome, String> {
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            self.open_agent_db_owner_locked(agent_id)?
                .commit_session_work_result(
                    session_key,
                    expected_lifecycle_revision,
                    work_id,
                    idempotency_key,
                    content,
                    sent_message_count,
                )
                .map_err(|error| error.to_string())
        })
    }

    pub fn release_session_work_admission(
        &self,
        agent_id: &str,
        expected_lifecycle_revision: &str,
        work_id: &str,
    ) -> Result<bool, String> {
        self.with_agent_admitted_filesystem_mutation(agent_id, || {
            let owner = self.open_agent_db_owner_locked(agent_id)?;
            let Some(mut record) = owner
                .read_session_work_admission()
                .map_err(|error| error.to_string())?
            else {
                return Ok(false);
            };
            if record.lifecycle_revision != expected_lifecycle_revision
                || record.admitted_work_id.as_deref() != Some(work_id)
            {
                return Ok(false);
            }
            record.admitted_work_id = None;
            owner
                .compare_and_set_session_work_admission(
                    Some(expected_lifecycle_revision),
                    &record,
                )
                .map_err(|error| error.to_string())
        })
    }

}
