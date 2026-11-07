use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::agent_db_schema::{
    DELETE_KV_SQL, GET_KV_SQL, LIST_TRANSCRIPT_PAGE_SQL, LIST_TRANSCRIPT_TAIL_SQL,
    LIST_TRANSCRIPT_WINDOW_SQL, SET_KV_SQL,
};
use super::agent_db_serde::parse_transcript_entry;

pub const DEFAULT_TRANSCRIPT_WINDOW_LIMIT: i64 = 500;
pub const MAX_TRANSCRIPT_WINDOW_LIMIT: i64 = 5_000;

const HUMAN_SNAPSHOT_REGISTRY_KEY: &str = "humanTranscriptSnapshots:v1";
const HUMAN_SNAPSHOT_KEY_PREFIX: &str = "humanTranscriptSnapshot:v1:";
const HUMAN_SNAPSHOT_LIMIT: usize = 16;

// Human history can arrive after newer rows have already received physical
// sequence numbers. Keep those numbers immutable (Agent pagination also uses
// them), and derive one chronological key for both Human read paths instead.
// Non-remote entries remain in the same transcript, after remote messages with
// the same timestamp; seq makes every key unique without rewriting user data.
const HUMAN_TRANSCRIPT_ORDER_CTE: &str = r#"
WITH human_ordered AS (
  SELECT seq, entry,
    CASE
      WHEN json_type(entry, '$.timestampMs') IN ('integer', 'real')
        AND CAST(json_extract(entry, '$.timestampMs') AS REAL)
          BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308
      THEN CAST(json_extract(entry, '$.timestampMs') AS REAL)
      ELSE 0.0
    END AS timestamp_ms,
    CASE
      WHEN json_type(entry, '$.remoteMessageId') IN ('integer', 'text')
        AND CAST(json_extract(entry, '$.remoteMessageId') AS TEXT) != ''
        AND CAST(json_extract(entry, '$.remoteMessageId') AS TEXT) NOT GLOB '*[^0-9]*'
        AND CAST(json_extract(entry, '$.remoteMessageId') AS INTEGER)
          BETWEEN 1 AND 9007199254740991
      THEN CAST(json_extract(entry, '$.remoteMessageId') AS INTEGER)
      ELSE 9007199254740992
    END AS remote_id
  FROM transcript_entries
  WHERE (?1 IS NULL AND json_extract(entry, '$.topicId') IS NULL)
     OR (json_extract(entry, '$.topicId') = ?1)
)
"#;

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptPage {
    pub entries: Vec<Value>,
    pub next_before_seq: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HumanTranscriptPage {
    pub entries: Vec<Value>,
    pub next_before_seq: Option<i64>,
    pub next_before_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanTranscriptPageQuery {
    pub before_seq: Option<i64>,
    pub before_cursor: Option<String>,
    pub limit: i64,
    pub topic_id: Option<String>,
}

impl From<TranscriptWindowQuery> for HumanTranscriptPageQuery {
    fn from(query: TranscriptWindowQuery) -> Self {
        Self {
            before_seq: query.before_seq,
            before_cursor: None,
            limit: query.limit,
            topic_id: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HumanSnapshotRegistry {
    version: u32,
    scope: String,
    snapshots: Vec<String>,
}

struct HumanSnapshotCursor {
    snapshot_id: String,
    position: i64,
    anchor_seq: i64,
}

struct HumanSnapshotRow {
    position: i64,
    seq: i64,
    entry: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptWindow<T> {
    pub entries: Vec<Value>,
    pub next_before_seq: Option<i64>,
    pub thread_counts: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptPageQuery {
    pub before_seq: Option<i64>,
    pub since_ms: Option<i64>,
    pub until_ms: i64,
    pub limit: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptWindowQuery {
    pub before_seq: Option<i64>,
    pub limit: i64,
}

#[derive(Debug)]
struct TranscriptRow {
    seq: i64,
    entry: String,
}

fn page(rows: Vec<TranscriptRow>, limit: usize) -> TranscriptPage {
    let has_more = rows.len() > limit;
    let selected = rows.into_iter().take(limit).collect::<Vec<_>>();
    let oldest_seq = selected.last().map(|row| row.seq);
    let entries = selected
        .iter()
        .rev()
        .filter_map(|row| parse_transcript_entry(&row.entry))
        .collect();
    TranscriptPage {
        entries,
        next_before_seq: if has_more { oldest_seq } else { None },
    }
}

fn sanitize_limit(limit: i64) -> usize {
    if limit > 0 {
        limit.min(MAX_TRANSCRIPT_WINDOW_LIMIT) as usize
    } else {
        DEFAULT_TRANSCRIPT_WINDOW_LIMIT as usize
    }
}

pub fn read_transcript_page(
    db: &Connection,
    query: TranscriptPageQuery,
) -> Result<TranscriptPage, rusqlite::Error> {
    let query_limit = query.limit.saturating_add(1);
    let mut statement = db.prepare(LIST_TRANSCRIPT_PAGE_SQL)?;
    let rows = statement
        .query_map(
            params![
                query.before_seq,
                query.before_seq,
                query.since_ms,
                query.since_ms,
                query.until_ms,
                query_limit,
            ],
            |row| {
                Ok(TranscriptRow {
                    seq: row.get(0)?,
                    entry: row.get(1)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let limit = if query.limit > 0 {
        query.limit as usize
    } else {
        0
    };
    Ok(page(rows, limit))
}

pub fn read_transcript_window<T, F>(
    db: &Connection,
    query: TranscriptWindowQuery,
    thread_counts_for: F,
) -> Result<TranscriptWindow<T>, rusqlite::Error>
where
    F: FnOnce(&[Value]) -> T,
{
    let limit = sanitize_limit(query.limit);
    let mut statement = db.prepare(LIST_TRANSCRIPT_WINDOW_SQL)?;
    let rows = statement
        .query_map(
            params![
                query.before_seq,
                query.before_seq,
                (limit + 1) as i64,
            ],
            |row| {
                Ok(TranscriptRow {
                    seq: row.get(0)?,
                    entry: row.get(1)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let result = page(rows, limit);
    let thread_counts = thread_counts_for(&result.entries);
    Ok(TranscriptWindow {
        entries: result.entries,
        next_before_seq: result.next_before_seq,
        thread_counts,
    })
}

pub fn read_transcript_tail(
    db: &Connection,
    query: TranscriptWindowQuery,
) -> Result<TranscriptPage, rusqlite::Error> {
    let limit = sanitize_limit(query.limit);
    let mut statement = db.prepare(LIST_TRANSCRIPT_TAIL_SQL)?;
    let rows = statement
        .query_map(
            params![
                query.before_seq,
                query.before_seq,
                (limit + 1) as i64,
            ],
            |row| {
                Ok(TranscriptRow {
                    seq: row.get(0)?,
                    entry: row.get(1)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(page(rows, limit))
}

pub fn read_human_transcript_entries_for_topic(
    db: &Connection,
    topic_id: Option<&str>,
) -> Result<Vec<Value>, rusqlite::Error> {
    if let Some(tid) = topic_id {
        if tid.trim().is_empty() {
            return Err(human_cursor_error(
                "Human transcript cursor topicId must not be empty",
            ));
        }
    }
    let sql = format!(
        "{HUMAN_TRANSCRIPT_ORDER_CTE}
         SELECT entry FROM human_ordered
         ORDER BY timestamp_ms ASC, remote_id ASC, seq ASC"
    );
    let mut statement = db.prepare(&sql)?;
    let rows = statement.query_map(params![topic_id], |row| row.get::<_, String>(0))?;
    let mut entries = Vec::new();
    for row in rows {
        if let Some(entry) = parse_transcript_entry(&row?) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

pub fn read_human_transcript_entries(
    db: &Connection,
) -> Result<Vec<Value>, rusqlite::Error> {
    read_human_transcript_entries_for_topic(db, None)
}

pub fn read_human_transcript_tail(
    db: &Connection,
    query: &HumanTranscriptPageQuery,
    metadata: &Value,
) -> Result<(HumanTranscriptPage, bool), rusqlite::Error> {
    if let Some(tid) = query.topic_id.as_deref() {
        if tid.trim().is_empty() {
            return Err(human_cursor_error(
                "Human transcript cursor topicId must not be empty",
            ));
        }
    }
    let scope = conversation_snapshot_scope(metadata, query.topic_id.as_deref())?;
    let registry_key = conversation_snapshot_registry_key(metadata, query.topic_id.as_deref());
    let cursor = query.before_cursor.as_deref().map(parse_human_cursor).transpose()?;
    if let Some(cursor) = cursor.as_ref() {
        if query.before_seq.is_some_and(|seq| seq != cursor.anchor_seq) {
            return Err(human_cursor_error("Human transcript cursor does not match beforeSeq"));
        }
    }
    // Each first page freezes only a sequence index, never message payloads or
    // canonical ordering fields. Continuations read current rows in that fixed
    // order, so a pending settlement or timestamp refresh cannot move a boundary.
    // One SQLite transaction also keeps eviction/validation/page selection atomic.
    db.execute_batch(if cursor.is_some() { "BEGIN" } else { "BEGIN IMMEDIATE" })?;
    let result = (|| {
        let mut registry = read_human_snapshot_registry(db, &registry_key, &scope)?;
        let (snapshot_id, position, anchor_seq, changed) = if let Some(cursor) = cursor {
            if !registry.snapshots.contains(&cursor.snapshot_id) {
                return Err(human_cursor_error("Human transcript cursor is unknown or expired"));
            }
            (cursor.snapshot_id, cursor.position, Some(cursor.anchor_seq), false)
        } else {
            // Legacy callers may still send a physical beforeSeq. That request
            // keeps its current-anchor semantics and starts a new stable walk of
            // the remaining rows. New callers continue with beforeCursor.
            if let Some(before_seq) = query.before_seq {
                let exists: bool = db.query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM transcript_entries
                       WHERE seq = ?1
                         AND ((?2 IS NULL AND json_extract(entry, '$.topicId') IS NULL) OR (json_extract(entry, '$.topicId') = ?2))
                     )",
                    params![before_seq, query.topic_id.as_deref()],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Ok((empty_human_page(), false));
                }
            }
            let snapshot_id = Uuid::new_v4().simple().to_string();
            let snapshot_key = format!("{HUMAN_SNAPSHOT_KEY_PREFIX}{snapshot_id}");
            let sql = format!(
                "{HUMAN_TRANSCRIPT_ORDER_CTE}
                 INSERT INTO kv (key, value)
                 SELECT ?2, json_object('version', 1, 'scope', ?3,
                                        'seqs', json_group_array(seq))
                 FROM (
                   SELECT seq FROM human_ordered
                   WHERE ?4 IS NULL OR (timestamp_ms, remote_id, seq) < (
                     SELECT timestamp_ms, remote_id, seq FROM human_ordered WHERE seq = ?4
                   )
                   ORDER BY timestamp_ms DESC, remote_id DESC, seq DESC
                 )"
            );
            if db.execute(
                &sql,
                params![query.topic_id.as_deref(), snapshot_key, scope, query.before_seq],
            )? != 1
            {
                return Err(human_cursor_error("Human transcript cursor index was not persisted"));
            }
            registry.snapshots.push(snapshot_id.clone());
            while registry.snapshots.len() > HUMAN_SNAPSHOT_LIMIT {
                let expired = registry.snapshots.remove(0);
                db.execute(DELETE_KV_SQL, params![format!("{HUMAN_SNAPSHOT_KEY_PREFIX}{expired}")])?;
            }
            let raw = serde_json::to_string(&registry)
                .map_err(|_| human_cursor_error("Human transcript cursor registry could not be encoded"))?;
            if db.execute(SET_KV_SQL, params![registry_key, raw])? != 1 {
                return Err(human_cursor_error("Human transcript cursor registry was not persisted"));
            }
            (snapshot_id, -1, None, true)
        };
        read_human_snapshot_page(
            db, &snapshot_id, position, anchor_seq, &scope, sanitize_limit(query.limit),
        ).map(|page| (page, changed))
    })();
    match result {
        Ok(result) => {
            if let Err(error) = db.execute_batch("COMMIT") {
                let _ = db.execute_batch("ROLLBACK");
                return Err(error);
            }
            Ok(result)
        }
        Err(error) => {
            let _ = db.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn human_cursor_error(message: &str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.to_owned(),
    )))
}

fn empty_human_page() -> HumanTranscriptPage {
    HumanTranscriptPage {
        entries: Vec::new(),
        next_before_seq: None,
        next_before_cursor: None,
    }
}

pub fn conversation_snapshot_scope(
    metadata: &Value,
    topic_id: Option<&str>,
) -> Result<String, rusqlite::Error> {
    if let Some(tid) = topic_id {
        if tid.trim().is_empty() {
            return Err(human_cursor_error(
                "Human transcript cursor topicId must not be empty",
            ));
        }
    }
    let conversation_id = metadata
        .get("conversationId")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| human_cursor_error("Human transcript cursor requires conversation metadata"))?;
    let kind = metadata.get("conversationKind").and_then(Value::as_str);

    match kind {
        Some("human") | Some("direct") => {
            let participants = metadata
                .get("participantIds")
                .and_then(Value::as_array)
                .filter(|ids| ids.len() == 2)
                .ok_or_else(|| {
                    human_cursor_error("Human transcript cursor requires a direct participant pair")
                })?;
            let local = participants[0].as_str().filter(|id| !id.trim().is_empty());
            let peer = participants[1].as_str().filter(|id| !id.trim().is_empty());
            if local.is_none() || peer.is_none() || local == peer {
                return Err(human_cursor_error(
                    "Human transcript cursor has invalid conversation scope",
                ));
            }
            let local = local.unwrap();
            let peer = peer.unwrap();
            if topic_id.is_none() && kind == Some("human") {
                serde_json::to_string(&(conversation_id, local, peer))
                    .map_err(|_| human_cursor_error("Human transcript cursor scope could not be encoded"))
            } else {
                serde_json::to_string(&("direct", conversation_id, local, peer, topic_id))
                    .map_err(|_| human_cursor_error("Human transcript cursor scope could not be encoded"))
            }
        }
        Some("group") => {
            let local = metadata
                .get("localHumanId")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .or_else(|| {
                    metadata
                        .get("participantIds")
                        .and_then(Value::as_array)
                        .and_then(|ids| ids.first())
                        .and_then(Value::as_str)
                        .filter(|id| !id.trim().is_empty())
                })
                .ok_or_else(|| {
                    human_cursor_error("Human transcript cursor has invalid conversation scope")
                })?;
            serde_json::to_string(&("group", conversation_id, local, topic_id))
                .map_err(|_| human_cursor_error("Human transcript cursor scope could not be encoded"))
        }
        Some("channel") => {
            serde_json::to_string(&("channel", conversation_id, topic_id))
                .map_err(|_| human_cursor_error("Human transcript cursor scope could not be encoded"))
        }
        _ => Err(human_cursor_error(
            "Human transcript cursor has invalid conversation scope",
        )),
    }
}

pub fn human_snapshot_scope(metadata: &Value) -> Result<String, rusqlite::Error> {
    conversation_snapshot_scope(metadata, None)
}

fn conversation_snapshot_registry_key(metadata: &Value, topic_id: Option<&str>) -> String {
    let kind = metadata.get("conversationKind").and_then(Value::as_str);
    if topic_id.is_none() && kind == Some("human") {
        HUMAN_SNAPSHOT_REGISTRY_KEY.to_string()
    } else if let Some(tid) = topic_id {
        format!("{HUMAN_SNAPSHOT_REGISTRY_KEY}:topic:{tid}")
    } else {
        format!("{HUMAN_SNAPSHOT_REGISTRY_KEY}:main")
    }
}

fn valid_human_snapshot_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_human_cursor(raw: &str) -> Result<HumanSnapshotCursor, rusqlite::Error> {
    if raw.len() > 128 {
        return Err(human_cursor_error("Human transcript cursor is invalid"));
    }
    let fields = raw.split(':').collect::<Vec<_>>();
    if fields.len() != 4 || fields[0] != "htp1" || !valid_human_snapshot_id(fields[1]) {
        return Err(human_cursor_error("Human transcript cursor is invalid"));
    }
    let position = fields[2].parse::<i64>()
        .map_err(|_| human_cursor_error("Human transcript cursor position is invalid"))?;
    let anchor_seq = fields[3].parse::<i64>()
        .map_err(|_| human_cursor_error("Human transcript cursor anchor is invalid"))?;
    if position < 0 {
        return Err(human_cursor_error("Human transcript cursor position is invalid"));
    }
    Ok(HumanSnapshotCursor { snapshot_id: fields[1].to_owned(), position, anchor_seq })
}

fn read_human_snapshot_registry(
    db: &Connection,
    registry_key: &str,
    scope: &str,
) -> Result<HumanSnapshotRegistry, rusqlite::Error> {
    let raw = db.query_row(GET_KV_SQL, params![registry_key], |row| {
        row.get::<_, String>(0)
    }).optional()?;
    let Some(raw) = raw else {
        return Ok(HumanSnapshotRegistry { version: 1, scope: scope.to_owned(), snapshots: Vec::new() });
    };
    let registry: HumanSnapshotRegistry = serde_json::from_str(&raw)
        .map_err(|_| human_cursor_error("Human transcript cursor registry is invalid"))?;
    let distinct = registry.snapshots.iter().collect::<std::collections::BTreeSet<_>>();
    if registry.version != 1 || registry.scope != scope
        || registry.snapshots.len() > HUMAN_SNAPSHOT_LIMIT
        || distinct.len() != registry.snapshots.len()
        || !registry.snapshots.iter().all(|id| valid_human_snapshot_id(id))
    {
        return Err(human_cursor_error("Human transcript cursor registry has invalid version or scope"));
    }
    Ok(registry)
}

fn read_human_snapshot_page(
    db: &Connection,
    snapshot_id: &str,
    position: i64,
    anchor_seq: Option<i64>,
    scope: &str,
    limit: usize,
) -> Result<HumanTranscriptPage, rusqlite::Error> {
    let snapshot_key = format!("{HUMAN_SNAPSHOT_KEY_PREFIX}{snapshot_id}");
    let anchor_path = format!("$.seqs[{}]", position.max(0));
    let metadata = db.query_row(
        "SELECT json_extract(value, '$.version'), json_extract(value, '$.scope'),
                json_type(value, '$.seqs'), json_array_length(value, '$.seqs'),
                json_extract(value, ?2)
         FROM kv WHERE key = ?1",
        params![snapshot_key, anchor_path],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?,
                  row.get::<_, String>(2)?, row.get::<_, i64>(3)?, row.get::<_, Option<i64>>(4)?)),
    ).optional()?.ok_or_else(|| human_cursor_error("Human transcript cursor is unknown or expired"))?;
    if metadata.0 != 1 || metadata.1 != scope || metadata.2 != "array" || metadata.3 < 0 {
        return Err(human_cursor_error("Human transcript cursor index has invalid version or scope"));
    }
    // A corrupted index must not let INNER JOIN silently discard invalid or
    // deleted identities, nor return the same physical row twice. JSON type
    // and SQLite storage type are both checked: numeric strings, floats and
    // integers outside SQLite's signed domain cannot alias an existing seq.
    let integrity = db.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN cursor_index.type = 'integer'
                                      AND typeof(cursor_index.value) = 'integer'
                                      AND cursor_index.value > 0
                                  THEN 0 ELSE 1 END), 0),
                COUNT(DISTINCT cursor_index.value), COUNT(transcript.seq)
         FROM kv AS snapshot, json_each(snapshot.value, '$.seqs') AS cursor_index
         LEFT JOIN transcript_entries AS transcript ON transcript.seq = cursor_index.value
         WHERE snapshot.key = ?1",
        params![snapshot_key],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?,
                  row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
    )?;
    if integrity.0 != metadata.3 || integrity.1 != 0 || integrity.2 != integrity.0 {
        return Err(human_cursor_error("Human transcript cursor index contains invalid or duplicate sequence identities"));
    }
    if integrity.3 != integrity.0 {
        return Err(human_cursor_error("Human transcript cursor index references a deleted row"));
    }
    if let Some(anchor_seq) = anchor_seq {
        if position >= metadata.3 || metadata.4 != Some(anchor_seq) {
            return Err(human_cursor_error("Human transcript cursor position or anchor is invalid"));
        }
    }
    // SQLite walks the persisted index directly. Only this page's payloads
    // leave SQLite; continuations neither copy nor rewrite the full index.
    let mut statement = db.prepare(
        "SELECT cursor_index.key, transcript.seq, transcript.entry
         FROM kv AS snapshot, json_each(snapshot.value, '$.seqs') AS cursor_index
         JOIN transcript_entries AS transcript ON transcript.seq = cursor_index.value
         WHERE snapshot.key = ?1 AND cursor_index.key > ?2
         ORDER BY cursor_index.key ASC LIMIT ?3"
    )?;
    let rows = statement.query_map(params![snapshot_key, position, (limit + 1) as i64], |row| {
        Ok(HumanSnapshotRow { position: row.get(0)?, seq: row.get(1)?, entry: row.get(2)? })
    })?.collect::<Result<Vec<_>, _>>()?;
    let has_more = rows.len() > limit;
    let selected = rows.into_iter().take(limit).collect::<Vec<_>>();
    let oldest = selected.last();
    let next_before_seq = if has_more { oldest.map(|row| row.seq) } else { None };
    let next_before_cursor = if has_more {
        oldest.map(|row| format!("htp1:{snapshot_id}:{}:{}", row.position, row.seq))
    } else { None };
    Ok(HumanTranscriptPage {
        entries: selected.iter().rev().filter_map(|row| parse_transcript_entry(&row.entry)).collect(),
        next_before_seq,
        next_before_cursor,
    })
}

