use rusqlite::params;
use rusqlite::OptionalExtension;

use super::agent_db::{
    newest_divider_anchor_timestamp_ms, notify_agent_db_listeners, session_work_write_is_admitted,
    subscribe_agent_db_listener, AgentDbListener, AgentDbListenerChannel, AgentDbProjectionError,
    AgentDbSubscription, SandAgentDb, KV_AWAITING, KV_EPISODE, KV_HIDDEN_REPAIR, KV_INTRODUCTION,
    KV_LEGACY_BLOB_RETIREMENT, KV_LATEST_REQUEST_ID, KV_MEMORY_SNAPSHOT, KV_PROFILE,
    KV_REQUEST_IDS, KV_SPEND_GUARD, KV_SPEND_GUARD_LEGACY, KV_STALE_ROOT_CLEANUP, KV_UNREAD,
};
use super::agent_db_schema::*;
use super::agent_db_serde::{
    parse_awaiting_state, parse_memory_prompt_snapshot, parse_pending_episode_turns, parse_profile,
    parse_request_records, parse_unread_state, resolve_spend_guard_state,
    serialize_spend_guard_state, AwaitingUserResponse, EpisodeTurn, MemoryPromptSnapshot,
    RequestRecord, SandProfile, SpendGuardState, UnreadState, EPISODE_PENDING_MAX,
    EPISODE_TURN_TEXT_CAP, REQUEST_ID_HISTORY_MAX, REQUEST_ID_PROMPT_MAX,
};
use super::ocrm_agent_db_persisted_ops::json_number;

impl SandAgentDb {
    pub fn subscribe_metadata(
        &self,
        key: impl Into<String>,
        listener: AgentDbListener,
    ) -> AgentDbSubscription {
        subscribe_agent_db_listener(
            &self.db_path,
            AgentDbListenerChannel::Metadata(key.into()),
            listener,
        )
    }

    pub fn subscribe_sand_profile(
        &self,
        listener: AgentDbListener,
    ) -> AgentDbSubscription {
        subscribe_agent_db_listener(
            &self.db_path,
            AgentDbListenerChannel::Profile,
            listener,
        )
    }

    pub fn subscribe_awaiting_user_response(
        &self,
        listener: AgentDbListener,
    ) -> AgentDbSubscription {
        subscribe_agent_db_listener(
            &self.db_path,
            AgentDbListenerChannel::Awaiting,
            listener,
        )
    }

    pub fn get_sand_profile(&self) -> Result<SandProfile, AgentDbProjectionError> {
        Ok(parse_profile(self.read_kv(KV_PROFILE)?.as_deref()))
    }

    pub fn set_sand_profile(
        &self,
        profile: &SandProfile,
    ) -> Result<bool, AgentDbProjectionError> {
        if self.get_sand_profile()? == *profile {
            return Ok(true);
        }
        let raw = serde_json::json!({
            "description": profile.description.clone(),
            "avatarPath": profile.avatar_path.clone(),
        })
        .to_string();
        let wrote = self.write_kv(KV_PROFILE, &raw)?;
        if wrote {
            notify_agent_db_listeners(&self.db_path, AgentDbListenerChannel::Profile);
        }
        Ok(wrote)
    }

    pub fn get_awaiting_user_response(
        &self,
    ) -> Result<Option<AwaitingUserResponse>, AgentDbProjectionError> {
        Ok(parse_awaiting_state(self.read_kv(KV_AWAITING)?.as_deref()))
    }

    pub fn set_awaiting_user_response(
        &self,
        state: Option<&AwaitingUserResponse>,
    ) -> Result<bool, AgentDbProjectionError> {
        if self.get_awaiting_user_response()?.as_ref() == state {
            return Ok(false);
        }
        let wrote = if let Some(state) = state {
            let raw = serde_json::json!({
                "tabId": state.tab_id.clone(),
                "reason": state.reason.clone(),
                "since": state.since,
            })
            .to_string();
            self.write_kv(KV_AWAITING, &raw)?
        } else {
            self.delete_kv(KV_AWAITING)?
        };
        if wrote {
            notify_agent_db_listeners(&self.db_path, AgentDbListenerChannel::Awaiting);
        }
        Ok(wrote)
    }

    pub fn set_awaiting_user_response_for_tab(
        &self,
        tab_id: &str,
        state: Option<&AwaitingUserResponse>,
        if_since_before: Option<f64>,
    ) -> Result<bool, AgentDbProjectionError> {
        let current = self.get_awaiting_user_response()?;
        if current.as_ref().is_some_and(|current| current.tab_id != tab_id) {
            return Ok(false);
        }
        if state.is_none()
            && (current.is_none()
                || if_since_before.is_some_and(|before| {
                    current.as_ref().is_some_and(|current| current.since >= before)
                }))
        {
            return Ok(false);
        }
        if state.is_some_and(|state| state.tab_id != tab_id) {
            return Ok(false);
        }
        self.set_awaiting_user_response(state)
    }

    pub fn get_unread_state(&self) -> Result<UnreadState, AgentDbProjectionError> {
        Ok(parse_unread_state(self.read_kv(KV_UNREAD)?.as_deref()))
    }

    pub(crate) fn write_unread_state_owner(
        &self,
        state: &UnreadState,
    ) -> Result<bool, AgentDbProjectionError> {
        let raw = serde_json::json!({
            "lastActivityAt": state.last_activity_at,
            "lastViewedAt": state.last_viewed_at,
            "isManuallyUnread": state.is_manually_unread,
            "unreadCount": state.unread_count,
        })
        .to_string();
        self.write_kv(KV_UNREAD, &raw)
    }

    pub fn set_unread_state(
        &self,
        state: &UnreadState,
    ) -> Result<bool, AgentDbProjectionError> {
        self.write_unread_state_owner(state)
    }

    pub fn mark_activity(&self, at: f64) -> Result<bool, AgentDbProjectionError> {
        let current = self.get_unread_state()?;
        if current.last_activity_at >= at {
            return Ok(false);
        }
        self.write_unread_state_owner(&UnreadState {
            last_activity_at: at,
            unread_count: current.unread_count + 1.0,
            ..current
        })
    }

    pub fn mark_viewed(
        &self,
        at: f64,
        preserve_manual_unread: bool,
    ) -> Result<bool, AgentDbProjectionError> {
        let current = self.get_unread_state()?;
        if preserve_manual_unread && current.is_manually_unread {
            return Ok(false);
        }
        if !current.is_manually_unread && current.last_viewed_at >= at {
            return Ok(false);
        }
        self.write_unread_state_owner(&UnreadState {
            last_viewed_at: at,
            is_manually_unread: false,
            unread_count: 0.0,
            ..current
        })
    }

    pub fn get_newest_divider_anchor_timestamp_ms(
        &self,
    ) -> Result<f64, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(0.0);
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(0.0);
        };
        Ok(newest_divider_anchor_timestamp_ms(db)?)
    }

    pub fn mark_unread(&self, at: f64) -> Result<bool, AgentDbProjectionError> {
        let current = self.get_unread_state()?;
        let newest = self.get_newest_divider_anchor_timestamp_ms()?;
        let last_activity_at = if current.last_activity_at == 0.0 {
            at
        } else {
            current.last_activity_at
        };
        let newest_bound = if newest > 0.0 {
            newest - 1.0
        } else {
            f64::INFINITY
        };
        let last_viewed_at = current
            .last_viewed_at
            .min(last_activity_at - 1.0)
            .min(at - 1.0)
            .min(newest_bound);
        self.write_unread_state_owner(&UnreadState {
            last_activity_at,
            last_viewed_at,
            is_manually_unread: true,
            unread_count: current.unread_count.max(1.0),
        })
    }

    pub fn mark_read(&self, at: f64) -> Result<bool, AgentDbProjectionError> {
        let current = self.get_unread_state()?;
        self.write_unread_state_owner(&UnreadState {
            last_viewed_at: current.last_activity_at.max(at),
            is_manually_unread: false,
            unread_count: 0.0,
            ..current
        })
    }

    pub fn get_introduction_pending(&self) -> Result<bool, AgentDbProjectionError> {
        Ok(self.read_kv(KV_INTRODUCTION)?.as_deref() == Some("1"))
    }

    pub fn set_introduction_pending(
        &self,
        pending: bool,
    ) -> Result<bool, AgentDbProjectionError> {
        if pending {
            self.write_kv(KV_INTRODUCTION, "1")
        } else {
            self.delete_kv(KV_INTRODUCTION)
        }
    }

    fn read_version(&self, key: &str) -> Result<u64, AgentDbProjectionError> {
        Ok(self
            .read_kv(key)?
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or_default())
    }

    pub fn get_hidden_entry_repair_version(&self) -> Result<u64, AgentDbProjectionError> {
        self.read_version(KV_HIDDEN_REPAIR)
    }

    pub fn set_hidden_entry_repair_version(
        &self,
        version: u64,
    ) -> Result<bool, AgentDbProjectionError> {
        self.write_kv(KV_HIDDEN_REPAIR, &version.to_string())
    }

    pub fn get_stale_root_cleanup_version(&self) -> Result<u64, AgentDbProjectionError> {
        self.read_version(KV_STALE_ROOT_CLEANUP)
    }

    pub fn set_stale_root_cleanup_version(
        &self,
        version: u64,
    ) -> Result<bool, AgentDbProjectionError> {
        self.write_kv(KV_STALE_ROOT_CLEANUP, &version.to_string())
    }

    pub fn get_legacy_blob_retirement_version(&self) -> Result<u64, AgentDbProjectionError> {
        self.read_version(KV_LEGACY_BLOB_RETIREMENT)
    }

    pub fn has_legacy_conversation_blobs(&self) -> Result<bool, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(false);
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(false);
        };
        Ok(db
            .query_row(HAS_LEGACY_BLOB_SQL, [], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn retire_legacy_conversation_blobs(
        &self,
        version: u64,
    ) -> Result<bool, AgentDbProjectionError> {
        self.run_write("retireLegacyConversationBlobs", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                db.execute(CLEAR_BLOBS_SQL, [])?;
                db.execute(
                    SET_KV_SQL,
                    params![KV_LEGACY_BLOB_RETIREMENT, version.to_string()],
                )?;
                Ok(true)
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

    pub fn get_automation_spend_guard_state(
        &self,
    ) -> Result<SpendGuardState, AgentDbProjectionError> {
        let state = self.read_kv(KV_SPEND_GUARD)?;
        let legacy = self.read_kv(KV_SPEND_GUARD_LEGACY)?;
        Ok(resolve_spend_guard_state(state.as_deref(), legacy.as_deref()))
    }

    pub fn set_automation_spend_guard_state(
        &self,
        state: &SpendGuardState,
    ) -> Result<bool, AgentDbProjectionError> {
        let raw = serialize_spend_guard_state(state);
        self.run_write("setAutomationSpendGuardState", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let changed = if let Some(raw) = raw.as_deref() {
                    db.execute(SET_KV_SQL, params![KV_SPEND_GUARD, raw])? > 0
                } else {
                    db.execute(DELETE_KV_SQL, params![KV_SPEND_GUARD])? > 0
                };
                let legacy_changed =
                    db.execute(DELETE_KV_SQL, params![KV_SPEND_GUARD_LEGACY])? > 0;
                Ok(changed || legacy_changed)
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

    pub fn get_request_ids(&self) -> Result<Vec<RequestRecord>, AgentDbProjectionError> {
        let mut records = parse_request_records(self.read_kv(KV_REQUEST_IDS)?.as_deref());
        if records.is_empty() {
            if let Some(legacy) = self
                .read_kv(KV_LATEST_REQUEST_ID)?
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
            {
                records.push(RequestRecord {
                    id: legacy,
                    at: 0.0,
                    prompt: None,
                    source: None,
                });
            }
        }
        Ok(records)
    }

    pub fn record_request_id(
        &self,
        id: &str,
        at: f64,
        prompt: Option<&str>,
        source: Option<&str>,
    ) -> Result<bool, AgentDbProjectionError> {
        let id = id.trim();
        if id.is_empty() {
            return Ok(false);
        }
        self.run_write("recordRequestId", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let mut records = parse_request_records(
                    super::agent_db::read_kv(db, KV_REQUEST_IDS)?.as_deref(),
                );
                if records.is_empty() {
                    if let Some(legacy) = super::agent_db::read_kv(db, KV_LATEST_REQUEST_ID)?
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                    {
                        records.push(RequestRecord {
                            id: legacy,
                            at: 0.0,
                            prompt: None,
                            source: None,
                        });
                    }
                }
                if records.last().is_some_and(|record| record.id == id) {
                    return Ok(false);
                }
                let label = prompt
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.chars().take(REQUEST_ID_PROMPT_MAX).collect::<String>());
                records.push(RequestRecord {
                    id: id.to_string(),
                    at,
                    prompt: label,
                    source: source.filter(|value| !value.is_empty()).map(ToOwned::to_owned),
                });
                if records.len() > REQUEST_ID_HISTORY_MAX {
                    records.drain(0..records.len() - REQUEST_ID_HISTORY_MAX);
                }
                let raw = serde_json::Value::Array(
                    records
                        .iter()
                        .map(|record| {
                            let mut value = serde_json::Map::new();
                            value.insert("id".into(), serde_json::Value::String(record.id.clone()));
                            value.insert("at".into(), json_number(record.at));
                            if let Some(prompt) = &record.prompt {
                                value.insert("prompt".into(), serde_json::Value::String(prompt.clone()));
                            }
                            if let Some(source) = &record.source {
                                value.insert("source".into(), serde_json::Value::String(source.clone()));
                            }
                            serde_json::Value::Object(value)
                        })
                        .collect(),
                )
                .to_string();
                Ok(db.execute(SET_KV_SQL, params![KV_REQUEST_IDS, raw])? > 0)
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

    pub fn get_pending_episode_turns(
        &self,
    ) -> Result<Vec<EpisodeTurn>, AgentDbProjectionError> {
        Ok(parse_pending_episode_turns(self.read_kv(KV_EPISODE)?.as_deref()))
    }

    pub fn record_episode_turn(
        &self,
        turn: &EpisodeTurn,
    ) -> Result<bool, AgentDbProjectionError> {
        self.run_write("recordEpisodeTurn", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let mut turns = parse_pending_episode_turns(
                    super::agent_db::read_kv(db, KV_EPISODE)?.as_deref(),
                );
                turns.push(EpisodeTurn {
                    ts: turn.ts,
                    user: turn.user.chars().take(EPISODE_TURN_TEXT_CAP).collect(),
                    agent: turn.agent.chars().take(EPISODE_TURN_TEXT_CAP).collect(),
                });
                if turns.len() > EPISODE_PENDING_MAX {
                    turns.drain(0..turns.len() - EPISODE_PENDING_MAX);
                }
                let raw = serde_json::Value::Array(
                    turns
                        .iter()
                        .map(|turn| {
                            serde_json::json!({
                                "ts": turn.ts,
                                "user": turn.user.clone(),
                                "agent": turn.agent.clone(),
                            })
                        })
                        .collect(),
                )
                .to_string();
                Ok(db.execute(SET_KV_SQL, params![KV_EPISODE, raw])? > 0)
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

    pub fn clear_pending_episode_turns(&self) -> Result<bool, AgentDbProjectionError> {
        self.run_write("clearPendingEpisodeTurns", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                Ok(db.execute(DELETE_KV_SQL, params![KV_EPISODE])? > 0)
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

    pub fn get_memory_prompt_snapshot(
        &self,
    ) -> Result<Option<MemoryPromptSnapshot>, AgentDbProjectionError> {
        Ok(parse_memory_prompt_snapshot(
            self.read_kv(KV_MEMORY_SNAPSHOT)?.as_deref(),
        ))
    }

    pub fn set_memory_prompt_snapshot(
        &self,
        snapshot: &serde_json::Value,
    ) -> Result<bool, AgentDbProjectionError> {
        self.write_kv(KV_MEMORY_SNAPSHOT, &snapshot.to_string())
    }

    pub fn clear_memory_prompt_snapshot(&self) -> Result<bool, AgentDbProjectionError> {
        self.delete_kv(KV_MEMORY_SNAPSHOT)
    }
}
