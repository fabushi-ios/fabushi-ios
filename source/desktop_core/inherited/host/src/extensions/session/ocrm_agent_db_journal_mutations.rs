use rusqlite::params;
use rusqlite::OptionalExtension;

use super::agent_db::{
    session_work_write_is_admitted, AgentDbProjectionError, PendingMemoryMutation,
    PendingProfileMutation, SandAgentDb, SessionWorkAdmissionRecord, KV_MEMORY_SNAPSHOT,
    KV_PENDING_MEMORY_MUTATION, KV_PENDING_PROFILE_MUTATION, KV_PROFILE,
    KV_SESSION_WORK_ADMISSION,
};
use super::agent_db_schema::*;
use super::agent_db_serde::parse_profile;

impl SandAgentDb {
    pub fn read_pending_profile_mutation(
        &self,
    ) -> Result<Option<PendingProfileMutation>, AgentDbProjectionError> {
        self.read_kv(KV_PENDING_PROFILE_MUTATION)?
            .map(|raw| serde_json::from_str(&raw).map_err(AgentDbProjectionError::from))
            .transpose()
    }

    pub fn prepare_pending_profile_mutation(
        &self,
        mutation: &PendingProfileMutation,
    ) -> Result<bool, AgentDbProjectionError> {
        if mutation.mutation_id.trim().is_empty()
            || mutation.session_key.trim().is_empty()
            || mutation.session_id.trim().is_empty()
            || mutation.lifecycle_revision.trim().is_empty()
            || mutation.before_files.is_empty()
            || !matches!(
                mutation.operation.as_str(),
                "profile-update" | "avatar-replace" | "avatar-remove"
            )
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "pending profile mutation identity or file state is incomplete".into(),
            ));
        }
        let encoded = serde_json::to_string(mutation)?;
        self.run_write("preparePendingProfileMutation", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let raw_admission = db
                    .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?;
                let Some(raw_admission) = raw_admission else {
                    return Ok(false);
                };
                let admission: SessionWorkAdmissionRecord = serde_json::from_str(&raw_admission)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                if admission.session_key != mutation.session_key
                    || admission.session_id != mutation.session_id
                    || admission.lifecycle_revision != mutation.lifecycle_revision
                {
                    return Ok(false);
                }
                if db
                    .query_row(GET_KV_SQL, params![KV_PENDING_PROFILE_MUTATION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                    .is_some()
                {
                    return Ok(false);
                }
                Ok(db
                    .execute(SET_KV_SQL, params![KV_PENDING_PROFILE_MUTATION, &encoded])?
                    > 0)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn seal_pending_profile_mutation(
        &self,
        mutation: &PendingProfileMutation,
    ) -> Result<bool, AgentDbProjectionError> {
        if mutation.after_files.is_empty() {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "pending profile mutation cannot be sealed without after-state".into(),
            ));
        }
        let expected = serde_json::to_string(mutation)?;
        self.run_write("sealPendingProfileMutation", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                let Some(raw_pending) = db
                    .query_row(GET_KV_SQL, params![KV_PENDING_PROFILE_MUTATION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                else {
                    return Ok(false);
                };
                let mut unsealed = mutation.clone();
                unsealed.after_files.clear();
                let unsealed = serde_json::to_string(&unsealed)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                if raw_pending != unsealed || !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                Ok(db.execute(
                    COMPARE_AND_SET_KV_SQL,
                    params![expected, KV_PENDING_PROFILE_MUTATION, raw_pending],
                )? == 1)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn commit_pending_profile_mutation(
        &self,
        mutation: &PendingProfileMutation,
    ) -> Result<bool, AgentDbProjectionError> {
        if mutation.after_files.is_empty() {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "pending profile mutation cannot commit without after-state".into(),
            ));
        }
        let expected = serde_json::to_string(mutation)?;
        self.run_write("commitPendingProfileMutation", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let raw_admission = db
                    .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?;
                let Some(raw_admission) = raw_admission else {
                    return Ok(false);
                };
                let admission: SessionWorkAdmissionRecord = serde_json::from_str(&raw_admission)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                if admission.session_key != mutation.session_key
                    || admission.session_id != mutation.session_id
                    || admission.lifecycle_revision != mutation.lifecycle_revision
                {
                    return Ok(false);
                }
                let Some(raw_pending) = db
                    .query_row(GET_KV_SQL, params![KV_PENDING_PROFILE_MUTATION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                else {
                    return Ok(false);
                };
                if raw_pending != expected {
                    return Ok(false);
                }
                let current = db
                    .query_row(GET_KV_SQL, params![KV_PROFILE], |row| row.get::<_, String>(0))
                    .optional()?;
                let current_profile = parse_profile(current.as_deref());
                if current_profile != mutation.previous_profile {
                    return Ok(false);
                }
                if let Some(next) = mutation.next_profile.as_ref() {
                    let encoded = serde_json::json!({
                        "description": next.description.clone(),
                        "avatarPath": next.avatar_path.clone(),
                    })
                    .to_string();
                    db.execute(SET_KV_SQL, params![KV_PROFILE, encoded])?;
                }
                Ok(db.execute(DELETE_KV_SQL, params![KV_PENDING_PROFILE_MUTATION])? > 0)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn clear_pending_profile_mutation(
        &self,
        mutation: &PendingProfileMutation,
    ) -> Result<bool, AgentDbProjectionError> {
        let expected = serde_json::to_string(mutation)?;
        self.run_write("clearPendingProfileMutation", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let Some(raw_admission) = db
                    .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                else {
                    return Ok(false);
                };
                let admission: SessionWorkAdmissionRecord = serde_json::from_str(&raw_admission)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                if admission.session_key != mutation.session_key
                    || admission.session_id != mutation.session_id
                    || admission.lifecycle_revision != mutation.lifecycle_revision
                {
                    return Ok(false);
                }
                let Some(raw_pending) = db
                    .query_row(GET_KV_SQL, params![KV_PENDING_PROFILE_MUTATION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                else {
                    return Ok(false);
                };
                if raw_pending != expected {
                    return Ok(false);
                }
                Ok(db.execute(DELETE_KV_SQL, params![KV_PENDING_PROFILE_MUTATION])? > 0)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn read_pending_memory_mutation(
        &self,
    ) -> Result<Option<PendingMemoryMutation>, AgentDbProjectionError> {
        self.read_kv(KV_PENDING_MEMORY_MUTATION)?
            .map(|raw| serde_json::from_str(&raw).map_err(AgentDbProjectionError::from))
            .transpose()
    }

    pub fn prepare_pending_memory_mutation(
        &self,
        mutation: &PendingMemoryMutation,
    ) -> Result<bool, AgentDbProjectionError> {
        if mutation.mutation_id.trim().is_empty()
            || mutation.session_key.trim().is_empty()
            || mutation.session_id.trim().is_empty()
            || mutation.lifecycle_revision.trim().is_empty()
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "pending memory mutation identity is incomplete".into(),
            ));
        }
        let encoded = serde_json::to_string(mutation)?;
        self.run_write("preparePendingMemoryMutation", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let raw_admission = db
                    .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?;
                let Some(raw_admission) = raw_admission else {
                    return Ok(false);
                };
                let admission: SessionWorkAdmissionRecord = serde_json::from_str(&raw_admission)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                if admission.session_key != mutation.session_key
                    || admission.session_id != mutation.session_id
                    || admission.lifecycle_revision != mutation.lifecycle_revision
                {
                    return Ok(false);
                }
                if db
                    .query_row(GET_KV_SQL, params![KV_PENDING_MEMORY_MUTATION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                    .is_some()
                {
                    return Ok(false);
                }
                Ok(db
                    .execute(SET_KV_SQL, params![KV_PENDING_MEMORY_MUTATION, &encoded])?
                    > 0)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn commit_pending_memory_mutation(
        &self,
        mutation: &PendingMemoryMutation,
    ) -> Result<bool, AgentDbProjectionError> {
        if mutation.mutation_id.trim().is_empty()
            || mutation.session_key.trim().is_empty()
            || mutation.session_id.trim().is_empty()
            || mutation.lifecycle_revision.trim().is_empty()
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "pending memory mutation identity is incomplete".into(),
            ));
        }
        let expected = serde_json::to_string(mutation)?;
        self.run_write("commitPendingMemoryMutation", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let raw_admission = db
                    .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?;
                let Some(raw_admission) = raw_admission else {
                    return Ok(false);
                };
                let admission: SessionWorkAdmissionRecord = serde_json::from_str(&raw_admission)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                if admission.session_key != mutation.session_key
                    || admission.session_id != mutation.session_id
                    || admission.lifecycle_revision != mutation.lifecycle_revision
                {
                    return Ok(false);
                }
                let Some(raw_pending) = db
                    .query_row(GET_KV_SQL, params![KV_PENDING_MEMORY_MUTATION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                else {
                    return Ok(false);
                };
                if raw_pending != expected {
                    return Ok(false);
                }
                let changed_snapshot = db.execute(DELETE_KV_SQL, params![KV_MEMORY_SNAPSHOT])? > 0;
                let changed_journal =
                    db.execute(DELETE_KV_SQL, params![KV_PENDING_MEMORY_MUTATION])? > 0;
                Ok(changed_snapshot || changed_journal)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn clear_pending_memory_mutation(
        &self,
        mutation: &PendingMemoryMutation,
    ) -> Result<bool, AgentDbProjectionError> {
        if mutation.mutation_id.trim().is_empty()
            || mutation.session_key.trim().is_empty()
            || mutation.session_id.trim().is_empty()
            || mutation.lifecycle_revision.trim().is_empty()
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "pending memory mutation identity is incomplete".into(),
            ));
        }
        let expected = serde_json::to_string(mutation)?;
        self.run_write("clearPendingMemoryMutation", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let Some(raw_admission) = db
                    .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                else {
                    return Ok(false);
                };
                let admission: SessionWorkAdmissionRecord = serde_json::from_str(&raw_admission)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                if admission.session_key != mutation.session_key
                    || admission.session_id != mutation.session_id
                    || admission.lifecycle_revision != mutation.lifecycle_revision
                {
                    return Ok(false);
                }
                let Some(raw_pending) = db
                    .query_row(GET_KV_SQL, params![KV_PENDING_MEMORY_MUTATION], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                else {
                    return Ok(false);
                };
                if raw_pending != expected {
                    return Ok(false);
                }
                Ok(db.execute(DELETE_KV_SQL, params![KV_PENDING_MEMORY_MUTATION])? > 0)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }
}
