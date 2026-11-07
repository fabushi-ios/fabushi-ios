use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};
use rusqlite::params;
use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::storage::store_db::{live_db_handle_count, release_live_db_handle};

use super::agent_db::{
    parse_transcript_entry, AgentDbProjectionError, SandAgentDb, SandAgentDbOptions,
    SessionWorkResultReceipt, KV_ORIGIN, KV_PARTNERS, KV_PROFILE_SNAPSHOT, KV_PURPOSE,
};
use super::agent_db_recovery::{open_configured_db, DbRecoveryOptions};
use super::agent_db_schema::{GET_KV_SQL, LIST_TRANSCRIPT_ENTRIES_SQL, NEWEST_DIVIDER_ANCHOR_TIMESTAMP_SQL};
use super::agent_db_transcript_pages::{
    read_transcript_page, read_transcript_tail, read_transcript_window, TranscriptPage,
    TranscriptPageQuery, TranscriptWindow, TranscriptWindowQuery,
};
use super::ocrm_agent_db_persisted_metadata::valid_agent_purpose;
use super::ocrm_agent_db_persisted_ops::{
    branch_reply_counts_from_entries, main_transcript_entries_from_entries,
    read_branched_entries_from_db, read_transcript_entries_from_db,
    read_transcript_entry_from_db, thread_descendants_from_entries,
};

impl SandAgentDb {
    pub fn get_agent_profile_prompt_snapshot(
        &self,
    ) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
        let Some(raw) = self.read_kv(KV_PROFILE_SNAPSHOT)? else {
            return Ok(None);
        };
        Ok(serde_json::from_str::<serde_json::Value>(&raw).ok())
    }

    pub fn set_agent_profile_prompt_snapshot(
        &self,
        snapshot: &serde_json::Value,
    ) -> Result<bool, AgentDbProjectionError> {
        self.write_kv(KV_PROFILE_SNAPSHOT, &snapshot.to_string())
    }

    pub fn clear_agent_profile_prompt_snapshot(&self) -> Result<bool, AgentDbProjectionError> {
        self.delete_kv(KV_PROFILE_SNAPSHOT)
    }

    pub fn get_agent_origin(&self) -> Result<String, AgentDbProjectionError> {
        Ok(if self.read_kv(KV_ORIGIN)?.as_deref() == Some("dev") {
            "dev".to_string()
        } else {
            "user".to_string()
        })
    }

    pub fn set_agent_origin(&self, origin: &str) -> Result<bool, AgentDbProjectionError> {
        self.write_kv(KV_ORIGIN, if origin == "dev" { "dev" } else { "user" })
    }

    pub fn get_agent_purpose(&self) -> Result<Option<String>, AgentDbProjectionError> {
        Ok(self
            .read_kv(KV_PURPOSE)?
            .filter(|value| valid_agent_purpose(value)))
    }

    pub fn set_agent_purpose(&self, purpose: &str) -> Result<bool, AgentDbProjectionError> {
        self.write_kv(KV_PURPOSE, purpose)
    }

    pub fn clear_agent_purpose(&self) -> Result<bool, AgentDbProjectionError> {
        self.delete_kv(KV_PURPOSE)
    }

    pub fn get_conversation_partner_ids(
        &self,
    ) -> Result<Vec<String>, AgentDbProjectionError> {
        let Some(raw) = self.read_kv(KV_PARTNERS)? else {
            return Ok(Vec::new());
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Ok(Vec::new());
        };
        Ok(value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn add_conversation_partner(
        &self,
        partner_id: &str,
    ) -> Result<bool, AgentDbProjectionError> {
        let id = partner_id.trim();
        if id.is_empty() {
            return Ok(false);
        }
        let own_id = self
            .read_kv("metadata")?
            .and_then(|raw| decode_hex(&raw).ok())
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|metadata| {
                metadata
                    .get("agentId")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_default();
        if id == own_id {
            return Ok(false);
        }
        let mut values = self
            .get_conversation_partner_ids()?
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        if !values.insert(id.to_string()) {
            return Ok(false);
        }
        let raw = serde_json::Value::Array(
            values
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        )
        .to_string();
        self.write_kv(KV_PARTNERS, &raw)
    }

    pub fn get_transcript_entries(
        &self,
    ) -> Result<Vec<serde_json::Value>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(Vec::new());
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        Ok(read_transcript_entries_from_db(db)?)
    }

    pub fn get_transcript_page(
        &self,
        query: TranscriptPageQuery,
    ) -> Result<TranscriptPage, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(TranscriptPage {
                entries: Vec::new(),
                next_before_seq: None,
            });
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(TranscriptPage {
                entries: Vec::new(),
                next_before_seq: None,
            });
        };
        Ok(read_transcript_page(db, query)?)
    }

    pub fn get_transcript_window(
        &self,
        query: TranscriptWindowQuery,
    ) -> Result<TranscriptWindow<BTreeMap<String, usize>>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(TranscriptWindow {
                entries: Vec::new(),
                next_before_seq: None,
                thread_counts: BTreeMap::new(),
            });
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(TranscriptWindow {
                entries: Vec::new(),
                next_before_seq: None,
                thread_counts: BTreeMap::new(),
            });
        };
        let branched = read_branched_entries_from_db(db)?;
        let counts = branch_reply_counts_from_entries(&branched);
        Ok(read_transcript_window(db, query, |entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let id = entry.get("id").and_then(serde_json::Value::as_str)?;
                    counts.get(id).copied().map(|count| (id.to_string(), count))
                })
                .collect()
        })?)
    }

    pub fn get_transcript_tail(
        &self,
        query: TranscriptWindowQuery,
    ) -> Result<TranscriptPage, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(TranscriptPage {
                entries: Vec::new(),
                next_before_seq: None,
            });
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(TranscriptPage {
                entries: Vec::new(),
                next_before_seq: None,
            });
        };
        Ok(read_transcript_tail(db, query)?)
    }

    pub fn get_transcript_entry_by_id(
        &self,
        entry_id: &str,
    ) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(None);
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(None);
        };
        Ok(read_transcript_entry_from_db(db, entry_id)?)
    }

    pub fn get_branched_entries(
        &self,
    ) -> Result<Vec<serde_json::Value>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(Vec::new());
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        Ok(read_branched_entries_from_db(db)?)
    }

    pub fn get_thread_entries(
        &self,
        root_id: &str,
    ) -> Result<Vec<serde_json::Value>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(Vec::new());
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        let Some(root) = read_transcript_entry_from_db(db, root_id)? else {
            return Ok(Vec::new());
        };
        let branched = read_branched_entries_from_db(db)?;
        let mut entries = vec![root];
        entries.extend(thread_descendants_from_entries(root_id, &branched));
        Ok(entries)
    }

    pub fn get_main_transcript_entries(
        &self,
    ) -> Result<Vec<serde_json::Value>, AgentDbProjectionError> {
        let entries = self.get_transcript_entries()?;
        Ok(main_transcript_entries_from_entries(&entries))
    }

    pub fn close(&self, checkpoint: bool) {
        if self
            .handle_registered
            .swap(false, Ordering::AcqRel)
        {
            release_live_db_handle(&self.db_path);
        }
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(mut guard) = self.connection.lock() else {
            return;
        };
        let Some(db) = guard.take() else {
            return;
        };
        if checkpoint {
            let _ = db.query_row(
                "PRAGMA wal_checkpoint(TRUNCATE)",
                [],
                |row| Ok((row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
            );
        }
        drop(db);
    }

    pub(crate) fn seed_default_metadata_if_missing(&self) -> Result<(), AgentDbProjectionError> {
        if self.read_kv("metadata")?.is_some() {
            return Ok(());
        }
        let metadata = default_agent_metadata(&self.agent_dir_name);
        let raw = encode_hex(&serde_json::to_vec(&metadata)?);
        let _ = self.write_kv("metadata", &raw)?;
        Ok(())
    }
}

impl Drop for SandAgentDb {
    fn drop(&mut self) {
        self.close(false);
    }
}

pub(crate) fn session_work_result_receipt_entry(
    receipt: &SessionWorkResultReceipt,
    entry_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": entry_id,
        "kind": "background-result-receipt",
        "sessionKey": receipt.session_key,
        "lifecycleRevision": receipt.lifecycle_revision,
        "workId": receipt.work_id,
        "idempotencyKey": receipt.idempotency_key,
        "content": receipt.content,
        "sentMessageCount": receipt.sent_message_count,
    })
}

pub(crate) fn media_settlement_receipt_entry(
    entry_id: &str,
    session_key: &str,
    media_work_id: &str,
    lifecycle_revision: &str,
    media_paths: &[String],
    transcript_entries: &[serde_json::Value],
) -> serde_json::Value {
    let transcript_entry_ids = transcript_entries
        .iter()
        .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    serde_json::json!({
        "id": entry_id,
        "kind": "outgoing-media-settlement",
        "sessionKey": session_key,
        "mediaWorkId": media_work_id,
        "lifecycleRevision": lifecycle_revision,
        "mediaPaths": media_paths,
        "transcriptEntryIds": transcript_entry_ids,
    })
}

pub(crate) fn parse_session_work_result_receipt(
    entry: &serde_json::Value,
) -> Option<SessionWorkResultReceipt> {
    if entry.get("kind").and_then(serde_json::Value::as_str)
        != Some("background-result-receipt")
    {
        return None;
    }
    Some(SessionWorkResultReceipt {
        session_key: entry.get("sessionKey")?.as_str()?.to_string(),
        lifecycle_revision: entry.get("lifecycleRevision")?.as_str()?.to_string(),
        work_id: entry.get("workId")?.as_str()?.to_string(),
        idempotency_key: entry.get("idempotencyKey")?.as_str()?.to_string(),
        content: entry.get("content")?.as_str()?.to_string(),
        sent_message_count: usize::try_from(
            entry.get("sentMessageCount")?.as_u64()?,
        )
        .ok()?,
    })
}

pub(crate) fn default_agent_metadata(agent_dir_name: &str) -> serde_json::Value {
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let blob_encryption_key = first
        .as_bytes()
        .iter()
        .chain(second.as_bytes().iter())
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    serde_json::json!({
        "agentId": agent_dir_name,
        "latestRootBlobId": "",
        "name": "New Agent",
        "mode": "default",
        "isRunEverything": false,
        "createdAt": created_at,
        "lifecycleRevision": Uuid::new_v4().to_string(),
        "blobEncryptionKey": blob_encryption_key,
    })
}

pub(crate) fn open_projection_db(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<rusqlite::Connection, AgentDbProjectionError> {
    let agent_dir_name = db_path
        .parent()
        .and_then(Path::file_name)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let options = DbRecoveryOptions {
        busy_timeout_ms,
        ..DbRecoveryOptions::default()
    };
    Ok(open_configured_db(
        db_path,
        &agent_dir_name,
        &options,
        live_db_handle_count(db_path) > 0,
    )?)
}

pub(crate) fn read_metadata_json(
    db: &rusqlite::Connection,
) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
    let Some(raw) = read_kv(db, "metadata")? else {
        return Ok(None);
    };
    let metadata_bytes = decode_hex(&raw).map_err(AgentDbProjectionError::MetadataHex)?;
    let metadata_json = String::from_utf8(metadata_bytes)?;
    Ok(Some(serde_json::from_str(&metadata_json)?))
}

pub(crate) fn newest_divider_anchor_timestamp_ms(
    db: &rusqlite::Connection,
) -> Result<f64, rusqlite::Error> {
    Ok(db
        .query_row(NEWEST_DIVIDER_ANCHOR_TIMESTAMP_SQL, [], |row| {
            row.get::<_, Option<f64>>(0)
        })
        .optional()?
        .flatten()
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or_default())
}

pub(crate) fn read_kv(
    db: &rusqlite::Connection,
    key: &str,
) -> Result<Option<String>, rusqlite::Error> {
    db.query_row(GET_KV_SQL, params![key], |row| row.get::<_, String>(0))
        .optional()
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn decode_hex(raw: &str) -> Result<Vec<u8>, String> {
    let clean = raw.trim();
    if clean.len() % 2 != 0 {
        return Err("odd-length hex string".into());
    }
    let mut output = Vec::with_capacity(clean.len() / 2);
    for index in (0..clean.len()).step_by(2) {
        let pair = &clean[index..index + 2];
        let byte = u8::from_str_radix(pair, 16)
            .map_err(|_| format!("invalid hex pair at offset {index}: {pair}"))?;
        output.push(byte);
    }
    Ok(output)
}

pub fn read_persisted_transcript_entries(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<Vec<serde_json::Value>, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let mut statement = db.prepare(LIST_TRANSCRIPT_ENTRIES_SQL)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut entries = Vec::new();
    for row in rows {
        let raw = row?;
        if let Some(entry) = parse_transcript_entry(&raw) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

pub fn append_persisted_transcript_entries(
    db_path: &Path,
    busy_timeout_ms: u64,
    entries: &[serde_json::Value],
) -> Result<usize, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.append_transcript_entries(entries)
}

pub fn delete_persisted_transcript_entry(
    db_path: &Path,
    busy_timeout_ms: u64,
    entry_id: &str,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.delete_transcript_entry(entry_id)
}
