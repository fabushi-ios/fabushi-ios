use crate::db::{ServerDatabase, SqliteResultExt};
use chrono::{DateTime, TimeZone, Utc};
use gpui_telegram_agent::acp::{SubagentLifecycleStatus, SubagentNode};
use gpui_telegram_agent::hermes::security::PendingApproval;
use gpui_telegram_agent::hermes::tools_agentic::{
    DesktopPanelState, IntegrationConnection, KanbanBoard, KanbanCard, KanbanChecklist,
    KanbanChecklistItem, KanbanComment, LocalAgenticStore, LocalCronJob, LocalSkillEntry,
};
use gpui_telegram_agent::hermes::toolsets::{AgenticStateBackend, BoxFuture, HermesUiAction};
use gpui_telegram_agent::tasks::CronRoutine;
use gpui_telegram_core::error::Result;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

fn dt_from_ms(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_else(Utc::now)
}

fn mask_secret_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() <= 8 {
        "***".to_string()
    } else {
        format!("{}***{}", &trimmed[..4], &trimmed[trimmed.len() - 4..])
    }
}

/// Iterative JSON value parser supporting arbitrary nesting depth (>128 levels)
/// without hitting `serde_json`'s default recursion limit.
fn parse_json_value_deep(raw: &str) -> std::result::Result<Value, String> {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        return Ok(v);
    }

    enum Frame {
        Array(Vec<Value>),
        Object(serde_json::Map<String, Value>, Option<String>),
    }

    fn attach(stack: &mut [Frame], val: Value, root: &mut Option<Value>) -> std::result::Result<(), String> {
        match stack.last_mut() {
            Some(Frame::Array(arr)) => {
                arr.push(val);
                Ok(())
            }
            Some(Frame::Object(map, pending_key)) => {
                let key = pending_key
                    .take()
                    .ok_or_else(|| "Unexpected value without key in JSON object".to_string())?;
                map.insert(key, val);
                Ok(())
            }
            None => {
                if root.is_some() {
                    return Err("Trailing JSON content".to_string());
                }
                *root = Some(val);
                Ok(())
            }
        }
    }

    let bytes = raw.as_bytes();
    let mut i = 0;
    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<Value> = None;

    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        match bytes[i] {
            b',' | b':' => {
                i += 1;
            }
            b'{' => {
                stack.push(Frame::Object(serde_json::Map::new(), None));
                i += 1;
            }
            b'[' => {
                stack.push(Frame::Array(Vec::new()));
                i += 1;
            }
            b'}' => {
                let Frame::Object(map, pending_key) = stack
                    .pop()
                    .ok_or_else(|| "Unmatched '}' in JSON".to_string())?
                else {
                    return Err("Mismatched '}' inside JSON array".to_string());
                };
                if pending_key.is_some() {
                    return Err("Dangling key before '}' in JSON object".to_string());
                }
                attach(&mut stack, Value::Object(map), &mut root)?;
                i += 1;
            }
            b']' => {
                let Frame::Array(arr) = stack
                    .pop()
                    .ok_or_else(|| "Unmatched ']' in JSON".to_string())?
                else {
                    return Err("Mismatched ']' inside JSON object".to_string());
                };
                attach(&mut stack, Value::Array(arr), &mut root)?;
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                let mut escaped = false;
                while i < bytes.len() {
                    if escaped {
                        escaped = false;
                        i += 1;
                    } else if bytes[i] == b'\\' {
                        escaped = true;
                        i += 1;
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
                let s: String = serde_json::from_str(&raw[start..i]).map_err(|e| e.to_string())?;
                let is_object_key = matches!(stack.last(), Some(Frame::Object(_, None)));
                if is_object_key {
                    if let Some(Frame::Object(_, pending_key)) = stack.last_mut() {
                        *pending_key = Some(s);
                    }
                } else {
                    attach(&mut stack, Value::String(s), &mut root)?;
                }
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b',' | b'}' | b']' | b':')
                {
                    i += 1;
                }
                let v: Value = serde_json::from_str(&raw[start..i]).map_err(|e| e.to_string())?;
                attach(&mut stack, v, &mut root)?;
            }
        }
    }

    if !stack.is_empty() {
        return Err("Unclosed container in JSON".to_string());
    }
    root.ok_or_else(|| "Empty JSON input".to_string())
}

fn prune_scope_rows(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    id_col: &str,
    scope_key: &str,
    keep_ids: impl Iterator<Item = impl AsRef<str>>,
) -> Result<()> {
    let keep: std::collections::HashSet<String> =
        keep_ids.map(|s| s.as_ref().to_string()).collect();
    if keep.is_empty() {
        tx.execute(
            &format!("DELETE FROM {table} WHERE scope_key = ?1"),
            params![scope_key],
        )
        .map_db()?;
        return Ok(());
    }
    let mut stmt = tx
        .prepare(&format!("SELECT {id_col} FROM {table} WHERE scope_key = ?1"))
        .map_db()?;
    let existing: Vec<String> = stmt
        .query_map(params![scope_key], |r| r.get(0))
        .map_db()?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_db()?;
    drop(stmt);
    for id in existing {
        if !keep.contains(&id) {
            tx.execute(
                &format!("DELETE FROM {table} WHERE scope_key = ?1 AND {id_col} = ?2"),
                params![scope_key, id],
            )
            .map_db()?;
        }
    }
    Ok(())
}

impl ServerDatabase {
    /// Converges an in-memory `LocalAgenticStore` (all 12 domains) into persistent SQLite tables.
    pub fn converge_agentic_store(
        &self,
        scope_key: &str,
        local: &LocalAgenticStore,
    ) -> Result<()> {
        let now_ms = Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            // 1. Kanban boards & cards
            prune_scope_rows(tx, "hermes_kanban_boards", "id", scope_key, local.boards.keys())?;
            for board in local.boards.values() {
                let cols_json = serde_json::to_string(&board.columns).unwrap_or_else(|_| "[]".into());
                let created_ms = board.created_at.timestamp_millis();
                tx.execute(
                    r#"INSERT INTO hermes_kanban_boards (id, scope_key, name, description, columns_json, created_at, updated_at)
                       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                       ON CONFLICT(scope_key, id) DO UPDATE SET
                           name = excluded.name,
                           description = excluded.description,
                           columns_json = excluded.columns_json,
                           updated_at = excluded.updated_at"#,
                    params![board.id, scope_key, board.name, board.description, cols_json, created_ms, now_ms],
                )
                .map_db()?;
            }

            prune_scope_rows(tx, "hermes_kanban_cards", "id", scope_key, local.cards.keys())?;
            for card in local.cards.values() {
                let labels_json = serde_json::to_string(&card.labels).unwrap_or_else(|_| "[]".into());
                let comments_json = serde_json::to_string(&card.comments).unwrap_or_else(|_| "[]".into());
                let checklist_json = serde_json::to_string(&card.checklists).unwrap_or_else(|_| "[]".into());
                tx.execute(
                    r#"INSERT INTO hermes_kanban_cards (
                           id, scope_key, board_id, column_name, title, description,
                           priority, assignee, labels_json, comments_json, checklist_json,
                           position, created_at, updated_at
                       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                       ON CONFLICT(scope_key, id) DO UPDATE SET
                           board_id = excluded.board_id,
                           column_name = excluded.column_name,
                           title = excluded.title,
                           description = excluded.description,
                           priority = excluded.priority,
                           assignee = excluded.assignee,
                           labels_json = excluded.labels_json,
                           comments_json = excluded.comments_json,
                           checklist_json = excluded.checklist_json,
                           position = excluded.position,
                           updated_at = excluded.updated_at"#,
                    params![
                        card.id,
                        scope_key,
                        card.board_id,
                        card.column,
                        card.title,
                        card.description,
                        card.priority,
                        card.assignee,
                        labels_json,
                        comments_json,
                        checklist_json,
                        card.position as i64,
                        card.created_at.timestamp_millis(),
                        card.updated_at.timestamp_millis(),
                    ],
                )
                .map_db()?;
            }

            // 2. Cron jobs
            prune_scope_rows(tx, "hermes_cron_jobs", "id", scope_key, local.cron_jobs.keys())?;
            for job in local.cron_jobs.values() {
                let next_ms = job.routine.next_run.map(|d| d.timestamp_millis());
                let last_ms = job.routine.last_run.map(|d| d.timestamp_millis());
                tx.execute(
                    r#"INSERT INTO hermes_cron_jobs (
                           id, scope_key, name, schedule, timezone, target_chat_id,
                           prompt, model, enabled, deliver_to, origin_chat_id,
                           next_run_at, last_run_at, created_at
                       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                       ON CONFLICT(scope_key, id) DO UPDATE SET
                           name = excluded.name,
                           schedule = excluded.schedule,
                           timezone = excluded.timezone,
                           target_chat_id = excluded.target_chat_id,
                           prompt = excluded.prompt,
                           model = excluded.model,
                           enabled = excluded.enabled,
                           next_run_at = excluded.next_run_at,
                           last_run_at = excluded.last_run_at"#,
                    params![
                        job.routine.routine_id,
                        scope_key,
                        job.routine.name,
                        job.routine.expression,
                        job.routine.time_zone,
                        job.target_chat_id,
                        job.prompt,
                        job.model,
                        job.routine.enabled as i64,
                        Option::<String>::None,
                        job.target_chat_id,
                        next_ms,
                        last_ms,
                        now_ms,
                    ],
                )
                .map_db()?;
            }

            // 3. Integration connections
            prune_scope_rows(tx, "agentic_connections", "name", scope_key, local.connections.keys())?;
            for conn_item in local.connections.values() {
                tx.execute(
                    r#"INSERT INTO agentic_connections (
                           scope_key, name, provider, endpoint, masked_secret,
                           connected, updated_at_ms, last_tested_at_ms
                       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                       ON CONFLICT(scope_key, name) DO UPDATE SET
                           provider = excluded.provider,
                           endpoint = excluded.endpoint,
                           masked_secret = excluded.masked_secret,
                           connected = excluded.connected,
                           updated_at_ms = excluded.updated_at_ms,
                           last_tested_at_ms = excluded.last_tested_at_ms"#,
                    params![
                        scope_key,
                        conn_item.name,
                        conn_item.provider,
                        conn_item.endpoint,
                        conn_item.masked_secret,
                        conn_item.connected as i64,
                        conn_item.updated_at.timestamp_millis(),
                        conn_item.last_tested_at.map(|d| d.timestamp_millis()),
                    ],
                )
                .map_db()?;
            }

            // 4. Desktop panel state
            let panels_json =
                serde_json::to_string(&local.desktop_state.open_panels).unwrap_or_else(|_| "[]".into());
            tx.execute(
                r#"INSERT INTO agentic_desktop_state (scope_key, active_destination, open_panels_json, last_action, updated_at_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5)
                   ON CONFLICT(scope_key) DO UPDATE SET
                       active_destination = excluded.active_destination,
                       open_panels_json = excluded.open_panels_json,
                       last_action = excluded.last_action,
                       updated_at_ms = excluded.updated_at_ms"#,
                params![
                    scope_key,
                    local.desktop_state.active_destination,
                    panels_json,
                    local.desktop_state.last_action,
                    now_ms,
                ],
            )
            .map_db()?;

            // 5. UI action log
            tx.execute(
                "DELETE FROM agentic_ui_action_log WHERE scope_key = ?1",
                params![scope_key],
            )
            .map_db()?;
            for (idx, act) in local.ui_action_log.iter().enumerate() {
                let id = format!("{scope_key}_{idx}_{}", Uuid::new_v4().simple());
                let payload_json = serde_json::to_string(&act.payload).unwrap_or_else(|_| "{}".into());
                tx.execute(
                    r#"INSERT INTO agentic_ui_action_log (id, scope_key, action_type, payload_json, created_at_ms)
                       VALUES (?1, ?2, ?3, ?4, ?5)"#,
                    params![id, scope_key, act.action_type, payload_json, now_ms + idx as i64],
                )
                .map_db()?;
            }

            // 6. Subagent lineage
            prune_scope_rows(
                tx,
                "agentic_subagent_lineage",
                "task_id",
                scope_key,
                local.lineage.nodes.keys(),
            )?;
            for node in local.lineage.nodes.values() {
                let node_json = serde_json::to_string(node).ok();
                let status_str = format!("{:?}", node.status).to_ascii_lowercase();
                tx.execute(
                    r#"INSERT INTO agentic_subagent_lineage (
                           task_id, scope_key, parent_session_id, role, status, node_json, created_at_ms, updated_at_ms
                       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                       ON CONFLICT(scope_key, task_id) DO UPDATE SET
                           parent_session_id = excluded.parent_session_id,
                           role = excluded.role,
                           status = excluded.status,
                           node_json = excluded.node_json,
                           updated_at_ms = excluded.updated_at_ms"#,
                    params![
                        node.agent_id,
                        scope_key,
                        node.parent_id,
                        node.role,
                        status_str,
                        node_json,
                        node.started_at_ms as i64,
                        now_ms,
                    ],
                )
                .map_db()?;
            }

            // 7. Memory, User entries, Skills, Honcho observations, Session notes via hermes_kv_state
            let kv_ns = format!("agentic_store:{scope_key}");
            let kv_items: [(&str, String); 5] = [
                (
                    "memory_entries",
                    serde_json::to_string(&local.memory_entries).unwrap_or_else(|_| "[]".into()),
                ),
                (
                    "user_entries",
                    serde_json::to_string(&local.user_entries).unwrap_or_else(|_| "[]".into()),
                ),
                (
                    "skills",
                    serde_json::to_string(&local.skills).unwrap_or_else(|_| "{}".into()),
                ),
                (
                    "honcho_observations",
                    serde_json::to_string(&local.honcho_observations)
                        .unwrap_or_else(|_| "{}".into()),
                ),
                (
                    "session_notes",
                    serde_json::to_string(&local.session_notes).unwrap_or_else(|_| "[]".into()),
                ),
            ];
            for (k, v) in kv_items {
                tx.execute(
                    r#"INSERT INTO hermes_kv_state (namespace, key, value, updated_at)
                       VALUES (?1, ?2, ?3, ?4)
                       ON CONFLICT(namespace, key) DO UPDATE SET
                           value = excluded.value,
                           updated_at = excluded.updated_at"#,
                    params![kv_ns, k, v, now_ms],
                )
                .map_db()?;
            }

            Ok(())
        })
    }

    /// Loads a `LocalAgenticStore` hydrated from persistent SQLite tables.
    pub fn load_agentic_store(&self, scope_key: &str) -> Result<LocalAgenticStore> {
        self.pool.with_reader(|conn| {
            let mut store = LocalAgenticStore::new();

            // 1. Boards
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, name, description, columns_json, created_at FROM hermes_kanban_boards WHERE scope_key = ?1",
                    )
                    .map_db()?;
                let mut rows = stmt.query(params![scope_key]).map_db()?;
                while let Some(row) = rows.next().map_db()? {
                    let id: String = row.get(0).map_db()?;
                    let cols_raw: String = row.get(3).map_db()?;
                    let columns: Vec<String> = serde_json::from_str(&cols_raw).unwrap_or_default();
                    let created_ms: i64 = row.get(4).map_db()?;
                    store.boards.insert(
                        id.clone(),
                        KanbanBoard {
                            id,
                            name: row.get(1).map_db()?,
                            description: row.get(2).map_db()?,
                            columns,
                            created_at: dt_from_ms(created_ms),
                        },
                    );
                }
            }

            // 2. Cards
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, board_id, column_name, title, description, priority, assignee, labels_json, comments_json, checklist_json, position, created_at, updated_at FROM hermes_kanban_cards WHERE scope_key = ?1",
                    )
                    .map_db()?;
                let mut rows = stmt.query(params![scope_key]).map_db()?;
                while let Some(row) = rows.next().map_db()? {
                    let id: String = row.get(0).map_db()?;
                    let labels: Vec<String> =
                        serde_json::from_str(&row.get::<_, String>(7).map_db()?).unwrap_or_default();
                    let comments: Vec<KanbanComment> =
                        serde_json::from_str(&row.get::<_, String>(8).map_db()?).unwrap_or_default();
                    let checklists: Vec<KanbanChecklist> =
                        serde_json::from_str(&row.get::<_, String>(9).map_db()?).unwrap_or_default();
                    store.cards.insert(
                        id.clone(),
                        KanbanCard {
                            id,
                            board_id: row.get(1).map_db()?,
                            column: row.get(2).map_db()?,
                            title: row.get(3).map_db()?,
                            description: row.get(4).map_db()?,
                            priority: row.get(5).map_db()?,
                            assignee: row.get(6).map_db()?,
                            labels,
                            comments,
                            checklists,
                            position: row.get::<_, i64>(10).map_db()? as usize,
                            created_at: dt_from_ms(row.get(11).map_db()?),
                            updated_at: dt_from_ms(row.get(12).map_db()?),
                        },
                    );
                }
            }

            // 3. Cron jobs
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, name, schedule, timezone, target_chat_id, prompt, model, enabled, next_run_at, last_run_at FROM hermes_cron_jobs WHERE scope_key = ?1",
                    )
                    .map_db()?;
                let mut rows = stmt.query(params![scope_key]).map_db()?;
                while let Some(row) = rows.next().map_db()? {
                    let id: String = row.get(0).map_db()?;
                    let mut routine = CronRoutine::new(id.clone(), row.get(2).map_db()?)
                        .with_name(row.get::<_, String>(1).map_db()?)
                        .with_time_zone(row.get(3).map_db()?);
                    routine.enabled = row.get::<_, i64>(7).map_db()? != 0;
                    routine.next_run = row.get::<_, Option<i64>>(8).map_db()?.map(dt_from_ms);
                    routine.last_run = row.get::<_, Option<i64>>(9).map_db()?.map(dt_from_ms);
                    store.cron_jobs.insert(
                        id,
                        LocalCronJob {
                            routine,
                            prompt: row.get(5).map_db()?,
                            target_chat_id: row.get(4).map_db()?,
                            model: row.get(6).map_db()?,
                        },
                    );
                }
            }

            // 4. Connections
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT name, provider, endpoint, masked_secret, connected, updated_at_ms, last_tested_at_ms FROM agentic_connections WHERE scope_key = ?1",
                    )
                    .map_db()?;
                let mut rows = stmt.query(params![scope_key]).map_db()?;
                while let Some(row) = rows.next().map_db()? {
                    let name: String = row.get(0).map_db()?;
                    store.connections.insert(
                        name.clone(),
                        IntegrationConnection {
                            name,
                            provider: row.get(1).map_db()?,
                            endpoint: row.get(2).map_db()?,
                            masked_secret: row.get(3).map_db()?,
                            connected: row.get::<_, i64>(4).map_db()? != 0,
                            updated_at: dt_from_ms(row.get(5).map_db()?),
                            last_tested_at: row.get::<_, Option<i64>>(6).map_db()?.map(dt_from_ms),
                        },
                    );
                }
            }

            // 5. Desktop state
            if let Some(row) = conn
                .query_row(
                    "SELECT active_destination, open_panels_json, last_action FROM agentic_desktop_state WHERE scope_key = ?1",
                    params![scope_key],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()
                .map_db()?
            {
                let open_panels: Vec<String> = serde_json::from_str(&row.1).unwrap_or_default();
                store.desktop_state = DesktopPanelState {
                    active_destination: row.0,
                    open_panels,
                    last_action: row.2,
                };
            }

            // 6. UI action log
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT action_type, payload_json FROM agentic_ui_action_log WHERE scope_key = ?1 ORDER BY created_at_ms ASC",
                    )
                    .map_db()?;
                let mut rows = stmt.query(params![scope_key]).map_db()?;
                while let Some(row) = rows.next().map_db()? {
                    let action_type: String = row.get(0).map_db()?;
                    let payload_raw: String = row.get(1).map_db()?;
                    let payload: Value = parse_json_value_deep(&payload_raw).unwrap_or_else(|_| json!({}));
                    store.ui_action_log.push(HermesUiAction {
                        action_type,
                        payload,
                    });
                }
            }

            // 7. Subagent lineage
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT task_id, parent_session_id, role, status, node_json, created_at_ms FROM agentic_subagent_lineage WHERE scope_key = ?1",
                    )
                    .map_db()?;
                let mut rows = stmt.query(params![scope_key]).map_db()?;
                while let Some(row) = rows.next().map_db()? {
                    let task_id: String = row.get(0).map_db()?;
                    let parent_id: Option<String> = row.get(1).map_db()?;
                    let role: String = row.get(2).map_db()?;
                    let status_raw: String = row.get(3).map_db()?;
                    let node_json: Option<String> = row.get(4).map_db()?;
                    let created_ms: i64 = row.get(5).map_db()?;
                    if let Some(node) = node_json.and_then(|j| serde_json::from_str::<SubagentNode>(&j).ok()) {
                        store.lineage.nodes.insert(task_id, node);
                    } else {
                        let status = match status_raw.as_str() {
                            "done" => SubagentLifecycleStatus::Done,
                            "error" => SubagentLifecycleStatus::Error,
                            "aborted" => SubagentLifecycleStatus::Aborted,
                            "pending" => SubagentLifecycleStatus::Pending,
                            _ => SubagentLifecycleStatus::Running,
                        };
                        store.lineage.nodes.insert(
                            task_id.clone(),
                            SubagentNode {
                                agent_id: task_id,
                                parent_id,
                                root_parent_id: None,
                                tool_call_id: None,
                                role,
                                status,
                                started_at_ms: created_ms.max(0) as u64,
                                completed_at_ms: None,
                                tool_call_count: 0,
                                artifacts: Vec::new(),
                            },
                        );
                    }
                }
            }

            // 8. KV fields (memory, user, skills, honcho, session_notes)
            let kv_ns = format!("agentic_store:{scope_key}");
            {
                let mut stmt = conn
                    .prepare("SELECT key, value FROM hermes_kv_state WHERE namespace = ?1")
                    .map_db()?;
                let mut rows = stmt.query(params![kv_ns]).map_db()?;
                while let Some(row) = rows.next().map_db()? {
                    let k: String = row.get(0).map_db()?;
                    let v: String = row.get(1).map_db()?;
                    match k.as_str() {
                        "memory_entries" => {
                            if let Ok(parsed) = serde_json::from_str(&v) {
                                store.memory_entries = parsed;
                            }
                        }
                        "user_entries" => {
                            if let Ok(parsed) = serde_json::from_str(&v) {
                                store.user_entries = parsed;
                            }
                        }
                        "skills" => {
                            if let Ok(parsed) = serde_json::from_str::<HashMap<String, LocalSkillEntry>>(&v) {
                                store.skills = parsed;
                            }
                        }
                        "honcho_observations" => {
                            if let Ok(parsed) = serde_json::from_str(&v) {
                                store.honcho_observations = parsed;
                            }
                        }
                        "session_notes" => {
                            if let Ok(parsed) = serde_json::from_str(&v) {
                                store.session_notes = parsed;
                            }
                        }
                        _ => {}
                    }
                }
            }

            Ok(store)
        })
    }

    pub fn hermes_kv_set(&self, namespace: &str, key: &str, value: &str) -> Result<()> {
        let now_ms = Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO hermes_kv_state (namespace, key, value, updated_at)
                   VALUES (?1, ?2, ?3, ?4)
                   ON CONFLICT(namespace, key) DO UPDATE SET
                       value = excluded.value,
                       updated_at = excluded.updated_at"#,
                params![namespace, key, value, now_ms],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn hermes_kv_get(&self, namespace: &str, key: &str) -> Result<Option<String>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT value FROM hermes_kv_state WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
                |r| r.get(0),
            )
            .optional()
            .map_db()
        })
    }
}

impl AgenticStateBackend for ServerDatabase {
    fn connections_op<'a>(
        &'a self,
        scope_key: &'a str,
        args: &'a Value,
    ) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("list")
                .to_ascii_lowercase();
            let now_ms = Utc::now().timestamp_millis();

            match action.as_str() {
                "list" => self
                    .pool
                    .with_reader(|conn| {
                        let mut stmt = conn
                            .prepare(
                                "SELECT name, provider, endpoint, masked_secret, connected, updated_at_ms, last_tested_at_ms FROM agentic_connections WHERE scope_key = ?1 ORDER BY name ASC",
                            )
                            .map_db()?;
                        let mut rows = stmt.query(params![scope_key]).map_db()?;
                        let mut list = Vec::new();
                        while let Some(row) = rows.next().map_db()? {
                            list.push(IntegrationConnection {
                                name: row.get(0).map_db()?,
                                provider: row.get(1).map_db()?,
                                endpoint: row.get(2).map_db()?,
                                masked_secret: row.get(3).map_db()?,
                                connected: row.get::<_, i64>(4).map_db()? != 0,
                                updated_at: dt_from_ms(row.get(5).map_db()?),
                                last_tested_at: row.get::<_, Option<i64>>(6).map_db()?.map(dt_from_ms),
                            });
                        }
                        Ok(json!({ "connections": list }))
                    })
                    .map_err(|e| e.to_string()),
                "connect" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'name'".to_string())?;
                    let provider = args.get("provider").and_then(|v| v.as_str()).unwrap_or(name);
                    let endpoint = args.get("endpoint").and_then(|v| v.as_str());
                    let masked_secret = args
                        .get("api_key")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(mask_secret_value);
                    self.pool
                        .with_conn(|conn| {
                            conn.execute(
                                r#"INSERT INTO agentic_connections (scope_key, name, provider, endpoint, masked_secret, connected, updated_at_ms, last_tested_at_ms)
                                   VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, NULL)
                                   ON CONFLICT(scope_key, name) DO UPDATE SET
                                       provider = excluded.provider,
                                       endpoint = excluded.endpoint,
                                       masked_secret = excluded.masked_secret,
                                       connected = 1,
                                       updated_at_ms = excluded.updated_at_ms"#,
                                params![scope_key, name, provider, endpoint, masked_secret, now_ms],
                            )
                            .map_db()
                        })
                        .map_err(|e| e.to_string())?;
                    let entry = IntegrationConnection {
                        name: name.to_string(),
                        provider: provider.to_string(),
                        endpoint: endpoint.map(ToString::to_string),
                        masked_secret,
                        connected: true,
                        updated_at: dt_from_ms(now_ms),
                        last_tested_at: None,
                    };
                    serde_json::to_value(&entry).map_err(|e| e.to_string())
                }
                "disconnect" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'name'".to_string())?;
                    let updated = self
                        .pool
                        .with_conn(|conn| {
                            conn.execute(
                                "UPDATE agentic_connections SET connected = 0, masked_secret = NULL, updated_at_ms = ?1 WHERE scope_key = ?2 AND name = ?3",
                                params![now_ms, scope_key, name],
                            )
                            .map_db()
                        })
                        .map_err(|e| e.to_string())?;
                    if updated == 0 {
                        return Err(format!("Connection '{name}' not found"));
                    }
                    Ok(json!({ "name": name, "connected": false }))
                }
                "test" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'name'".to_string())?;
                    let connected: Option<i64> = self
                        .pool
                        .with_conn(|conn| {
                            conn.execute(
                                "UPDATE agentic_connections SET last_tested_at_ms = ?1 WHERE scope_key = ?2 AND name = ?3",
                                params![now_ms, scope_key, name],
                            )
                            .map_db()?;
                            conn.query_row(
                                "SELECT connected FROM agentic_connections WHERE scope_key = ?1 AND name = ?2",
                                params![scope_key, name],
                                |r| r.get(0),
                            )
                            .optional()
                            .map_db()
                        })
                        .map_err(|e| e.to_string())?;
                    let Some(c) = connected else {
                        return Err(format!("Connection '{name}' not found"));
                    };
                    Ok(json!({ "name": name, "healthy": c != 0 }))
                }
                "get" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'name'".to_string())?;
                    let entry = self
                        .pool
                        .with_reader(|conn| {
                            conn.query_row(
                                "SELECT name, provider, endpoint, masked_secret, connected, updated_at_ms, last_tested_at_ms FROM agentic_connections WHERE scope_key = ?1 AND name = ?2",
                                params![scope_key, name],
                                |row| {
                                    Ok(IntegrationConnection {
                                        name: row.get(0)?,
                                        provider: row.get(1)?,
                                        endpoint: row.get(2)?,
                                        masked_secret: row.get(3)?,
                                        connected: row.get::<_, i64>(4)? != 0,
                                        updated_at: dt_from_ms(row.get(5)?),
                                        last_tested_at: row.get::<_, Option<i64>>(6)?.map(dt_from_ms),
                                    })
                                },
                            )
                            .optional()
                            .map_db()
                        })
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Connection '{name}' not found"))?;
                    serde_json::to_value(entry).map_err(|e| e.to_string())
                }
                other => Err(format!("Unsupported action '{other}'")),
            }
        })
    }

    fn desktop_ui_inspect<'a>(
        &'a self,
        scope_key: &'a str,
    ) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            self.pool
                .with_reader(|conn| {
                    let desktop_state = conn
                        .query_row(
                            "SELECT active_destination, open_panels_json, last_action FROM agentic_desktop_state WHERE scope_key = ?1",
                            params![scope_key],
                            |r| {
                                let dest: String = r.get(0)?;
                                let panels_raw: String = r.get(1)?;
                                let last_action: Option<String> = r.get(2)?;
                                Ok(DesktopPanelState {
                                    active_destination: dest,
                                    open_panels: serde_json::from_str(&panels_raw).unwrap_or_default(),
                                    last_action,
                                })
                            },
                        )
                        .optional()
                        .map_db()?
                        .unwrap_or_default();

                    let mut stmt = conn
                        .prepare(
                            "SELECT action_type, payload_json FROM agentic_ui_action_log WHERE scope_key = ?1 ORDER BY created_at_ms DESC LIMIT 10",
                        )
                        .map_db()?;
                    let mut rows = stmt.query(params![scope_key]).map_db()?;
                    let mut recent_ui_actions = Vec::new();
                    while let Some(row) = rows.next().map_db()? {
                        let action_type: String = row.get(0).map_db()?;
                        let payload_raw: String = row.get(1).map_db()?;
                        recent_ui_actions.push(HermesUiAction {
                            action_type,
                            payload: parse_json_value_deep(&payload_raw).unwrap_or_else(|_| json!({})),
                        });
                    }
                    Ok(json!({
                        "desktop_state": desktop_state,
                        "recent_ui_actions": recent_ui_actions,
                    }))
                })
                .map_err(|e| e.to_string())
        })
    }

    fn desktop_ui_act<'a>(
        &'a self,
        scope_key: &'a str,
        action: HermesUiAction,
    ) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            let now_ms = Utc::now().timestamp_millis();
            self.pool
                .with_transaction(|tx| {
                    let mut state = tx
                        .query_row(
                            "SELECT active_destination, open_panels_json, last_action FROM agentic_desktop_state WHERE scope_key = ?1",
                            params![scope_key],
                            |r| {
                                let dest: String = r.get(0)?;
                                let panels_raw: String = r.get(1)?;
                                let last_action: Option<String> = r.get(2)?;
                                Ok(DesktopPanelState {
                                    active_destination: dest,
                                    open_panels: serde_json::from_str(&panels_raw).unwrap_or_default(),
                                    last_action,
                                })
                            },
                        )
                        .optional()
                        .map_db()?
                        .unwrap_or_default();

                    state.last_action = Some(action.action_type.clone());
                    if let Some(dest) = action.payload.get("destination").and_then(|v| v.as_str()) {
                        state.active_destination = dest.to_string();
                    }
                    if let Some(panel) = action.payload.get("panel").and_then(|v| v.as_str()) {
                        if !state.open_panels.iter().any(|p| p == panel) {
                            state.open_panels.push(panel.to_string());
                        }
                    }

                    let panels_json = serde_json::to_string(&state.open_panels).unwrap_or_else(|_| "[]".into());
                    tx.execute(
                        r#"INSERT INTO agentic_desktop_state (scope_key, active_destination, open_panels_json, last_action, updated_at_ms)
                           VALUES (?1, ?2, ?3, ?4, ?5)
                           ON CONFLICT(scope_key) DO UPDATE SET
                               active_destination = excluded.active_destination,
                               open_panels_json = excluded.open_panels_json,
                               last_action = excluded.last_action,
                               updated_at_ms = excluded.updated_at_ms"#,
                        params![scope_key, state.active_destination, panels_json, state.last_action, now_ms],
                    )
                    .map_db()?;

                    let log_id = format!("ui_{}", Uuid::new_v4().simple());
                    let payload_json = serde_json::to_string(&action.payload).unwrap_or_else(|_| "{}".into());
                    tx.execute(
                        "INSERT INTO agentic_ui_action_log (id, scope_key, action_type, payload_json, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![log_id, scope_key, action.action_type, payload_json, now_ms],
                    )
                    .map_db()?;

                    Ok(json!({ "ok": true, "action_type": action.action_type, "desktop_state": state }))
                })
                .map_err(|e| e.to_string())
        })
    }

    fn record_lineage<'a>(
        &'a self,
        scope_key: &'a str,
        task_id: &'a str,
        parent_session_id: Option<&'a str>,
        role: &'a str,
        status: &'a str,
    ) -> BoxFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            let now_ms = Utc::now().timestamp_millis();
            self.pool
                .with_conn(|conn| {
                    conn.execute(
                        r#"INSERT INTO agentic_subagent_lineage (task_id, scope_key, parent_session_id, role, status, node_json, created_at_ms, updated_at_ms)
                           VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?6)
                           ON CONFLICT(scope_key, task_id) DO UPDATE SET
                               status = excluded.status,
                               updated_at_ms = excluded.updated_at_ms"#,
                        params![task_id, scope_key, parent_session_id, role, status, now_ms],
                    )
                    .map_db()?;
                    Ok(())
                })
                .map_err(|e| e.to_string())
        })
    }

    fn cron_op<'a>(
        &'a self,
        scope_key: &'a str,
        args: &'a Value,
    ) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("list")
                .to_ascii_lowercase();
            let now_ms = Utc::now().timestamp_millis();
            match action.as_str() {
                "list" => self
                    .pool
                    .with_reader(|conn| {
                        let mut stmt = conn
                            .prepare(
                                "SELECT id, name, schedule, timezone, target_chat_id, prompt, model, enabled, next_run_at, last_run_at, created_at FROM hermes_cron_jobs WHERE scope_key = ?1 ORDER BY created_at DESC",
                            )
                            .map_db()?;
                        let mut rows = stmt.query(params![scope_key]).map_db()?;
                        let mut jobs = Vec::new();
                        while let Some(r) = rows.next().map_db()? {
                            jobs.push(json!({
                                "id": r.get::<_, String>(0).map_db()?,
                                "name": r.get::<_, String>(1).map_db()?,
                                "schedule": r.get::<_, String>(2).map_db()?,
                                "timezone": r.get::<_, Option<String>>(3).map_db()?,
                                "target_chat_id": r.get::<_, Option<i64>>(4).map_db()?,
                                "prompt": r.get::<_, String>(5).map_db()?,
                                "model": r.get::<_, Option<String>>(6).map_db()?,
                                "enabled": r.get::<_, i64>(7).map_db()? != 0,
                                "next_run_at": r.get::<_, Option<i64>>(8).map_db()?,
                                "last_run_at": r.get::<_, Option<i64>>(9).map_db()?,
                                "created_at": r.get::<_, i64>(10).map_db()?,
                            }));
                        }
                        Ok(json!({ "jobs": jobs }))
                    })
                    .map_err(|e| e.to_string()),
                "create" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string)
                        .unwrap_or_else(|| format!("cron_{}", Uuid::new_v4().simple()));
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Hermes Scheduled Task");
                    let schedule = args
                        .get("schedule")
                        .and_then(|v| v.as_str())
                        .unwrap_or("@every 1h");
                    let tz = args.get("timezone").and_then(|v| v.as_str());
                    let mut routine = CronRoutine::new(id.clone(), schedule.to_string())
                        .with_time_zone(tz.map(ToString::to_string));
                    if !routine.validate_expression() {
                        return Err(format!("Invalid cron or @every schedule expression: '{schedule}'"));
                    }
                    let next_run_at = routine.compute_next_run(Utc::now()).map(|d| d.timestamp_millis());
                    let prompt = args.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
                    let target_chat_id = args.get("target_chat_id").and_then(|v| v.as_i64());
                    let model = args.get("model").and_then(|v| v.as_str());
                    self.pool
                        .with_conn(|conn| {
                            conn.execute(
                                r#"INSERT INTO hermes_cron_jobs (id, scope_key, name, schedule, timezone, target_chat_id, prompt, model, enabled, next_run_at, created_at)
                                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10)"#,
                                params![id, scope_key, name, schedule, tz, target_chat_id, prompt, model, next_run_at, now_ms],
                            )
                            .map_db()?;
                            Ok(json!({
                                "id": id,
                                "name": name,
                                "schedule": schedule,
                                "timezone": tz,
                                "target_chat_id": target_chat_id,
                                "prompt": prompt,
                                "enabled": true,
                                "next_run_at": next_run_at,
                                "created_at": now_ms,
                            }))
                        })
                        .map_err(|e| e.to_string())
                }
                "get" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'id'".to_string())?;
                    self.pool
                        .with_reader(|conn| {
                            conn.query_row(
                                "SELECT id, name, schedule, timezone, target_chat_id, prompt, model, enabled, next_run_at, last_run_at, created_at FROM hermes_cron_jobs WHERE id = ?1 AND scope_key = ?2",
                                params![id, scope_key],
                                |r| {
                                    Ok(json!({
                                        "id": r.get::<_, String>(0)?,
                                        "name": r.get::<_, String>(1)?,
                                        "schedule": r.get::<_, String>(2)?,
                                        "timezone": r.get::<_, Option<String>>(3)?,
                                        "target_chat_id": r.get::<_, Option<i64>>(4)?,
                                        "prompt": r.get::<_, String>(5)?,
                                        "model": r.get::<_, Option<String>>(6)?,
                                        "enabled": r.get::<_, i64>(7)? != 0,
                                        "next_run_at": r.get::<_, Option<i64>>(8)?,
                                        "last_run_at": r.get::<_, Option<i64>>(9)?,
                                        "created_at": r.get::<_, i64>(10)?,
                                    }))
                                },
                            )
                            .optional()
                            .map_db()?
                            .ok_or_else(|| gpui_telegram_core::error::TelegramError::NotFound(format!("Cron job '{id}' not found")))
                        })
                        .map_err(|e| e.to_string())
                }
                "preview" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'id'".to_string())?;
                    let (schedule, tz): (String, Option<String>) = self
                        .pool
                        .with_reader(|conn| {
                            conn.query_row(
                                "SELECT schedule, timezone FROM hermes_cron_jobs WHERE id = ?1 AND scope_key = ?2",
                                params![id, scope_key],
                                |r| Ok((r.get(0)?, r.get(1)?)),
                            )
                            .optional()
                            .map_db()?
                            .ok_or_else(|| gpui_telegram_core::error::TelegramError::NotFound(format!("Cron job '{id}' not found")))
                        })
                        .map_err(|e| e.to_string())?;
                    let routine = CronRoutine::new(id.to_string(), schedule.clone())
                        .with_time_zone(tz);
                    let summary = routine.preview_next_7_days(Utc::now());
                    Ok(json!({
                        "id": id,
                        "schedule": schedule,
                        "scheduled_fires_next_7_days": summary.scheduled_fires_next_7_days,
                        "fires_on_weekend": summary.fires_on_weekend,
                        "fires_overnight": summary.fires_overnight,
                    }))
                }
                "history" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'id'".to_string())?;
                    self.pool
                        .with_reader(|conn| {
                            let exists: bool = conn
                                .query_row(
                                    "SELECT EXISTS(SELECT 1 FROM hermes_cron_jobs WHERE id = ?1 AND scope_key = ?2)",
                                    params![id, scope_key],
                                    |r| r.get(0),
                                )
                                .map_db()?;
                            if !exists {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound(format!("Cron job '{id}' not found")));
                            }
                            let mut stmt = conn
                                .prepare(
                                    "SELECT id, job_id, run_at, status, output_preview, error, duration_ms FROM hermes_cron_history WHERE job_id = ?1 AND scope_key = ?2 ORDER BY run_at DESC LIMIT 20",
                                )
                                .map_db()?;
                            let mut rows = stmt.query(params![id, scope_key]).map_db()?;
                            let mut history = Vec::new();
                            while let Some(r) = rows.next().map_db()? {
                                history.push(json!({
                                    "id": r.get::<_, String>(0).map_db()?,
                                    "job_id": r.get::<_, String>(1).map_db()?,
                                    "run_at": r.get::<_, i64>(2).map_db()?,
                                    "status": r.get::<_, String>(3).map_db()?,
                                    "output_preview": r.get::<_, Option<String>>(4).map_db()?,
                                    "error": r.get::<_, Option<String>>(5).map_db()?,
                                    "duration_ms": r.get::<_, i64>(6).map_db()?,
                                }));
                            }
                            Ok(json!({ "id": id, "history": history }))
                        })
                        .map_err(|e| e.to_string())
                }
                "run_now" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'id'".to_string())?;
                    self.pool
                        .with_transaction(|tx| {
                            let row: Option<(String, Option<String>)> = tx
                                .query_row(
                                    "SELECT schedule, timezone FROM hermes_cron_jobs WHERE id = ?1 AND scope_key = ?2",
                                    params![id, scope_key],
                                    |r| Ok((r.get(0)?, r.get(1)?)),
                                )
                                .optional()
                                .map_db()?;
                            let Some((schedule, tz)) = row else {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound(format!("Cron job '{id}' not found")));
                            };
                            let mut routine = CronRoutine::new(id.to_string(), schedule).with_time_zone(tz);
                            let next_run_at = routine.compute_next_run(Utc::now()).map(|d| d.timestamp_millis());
                            let run_id = format!("crun_{}", Uuid::new_v4().simple());
                            let output_preview = "Triggered cron job";
                            tx.execute(
                                "INSERT INTO hermes_cron_history (id, scope_key, job_id, run_at, status, output_preview, error, duration_ms) VALUES (?1, ?2, ?3, ?4, 'completed', ?5, NULL, 0)",
                                params![run_id, scope_key, id, now_ms, output_preview],
                            )
                            .map_db()?;
                            tx.execute(
                                "UPDATE hermes_cron_jobs SET last_run_at = ?1, next_run_at = COALESCE(?2, next_run_at) WHERE id = ?3 AND scope_key = ?4",
                                params![now_ms, next_run_at, id, scope_key],
                            )
                            .map_db()?;
                            Ok(json!({
                                "id": run_id,
                                "job_id": id,
                                "run_at": now_ms,
                                "status": "completed",
                                "output_preview": output_preview,
                                "error": Value::Null,
                                "duration_ms": 0,
                            }))
                        })
                        .map_err(|e| e.to_string())
                }
                "pause" | "resume" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'id'".to_string())?;
                    let enabled = if action == "resume" { 1i64 } else { 0i64 };
                    self.pool
                        .with_conn(|conn| {
                            let n = conn
                                .execute(
                                    "UPDATE hermes_cron_jobs SET enabled = ?1 WHERE id = ?2 AND scope_key = ?3",
                                    params![enabled, id, scope_key],
                                )
                                .map_db()?;
                            if n == 0 {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound(format!("Cron job '{id}' not found")));
                            }
                            Ok(json!({ "id": id, "enabled": enabled != 0 }))
                        })
                        .map_err(|e| e.to_string())
                }
                "delete" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'id'".to_string())?;
                    let deleted = self
                        .pool
                        .with_transaction(|tx| {
                            tx.execute(
                                "DELETE FROM hermes_cron_history WHERE job_id = ?1 AND scope_key = ?2",
                                params![id, scope_key],
                            )
                            .map_db()?;
                            tx.execute(
                                "DELETE FROM hermes_cron_jobs WHERE id = ?1 AND scope_key = ?2",
                                params![id, scope_key],
                            )
                            .map_db()
                        })
                        .map_err(|e| e.to_string())?;
                    Ok(json!({ "id": id, "deleted": deleted > 0 }))
                }
                _ => Err(format!("Unsupported cron action '{action}'")),
            }
        })
    }

    fn kanban_op<'a>(
        &'a self,
        scope_key: &'a str,
        action: &'a str,
        args: &'a Value,
    ) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            let now_ms = Utc::now().timestamp_millis();
            match action {
                "kanban_list_boards" => self
                    .pool
                    .with_reader(|conn| {
                        let mut stmt = conn
                            .prepare("SELECT id, name, description, columns_json, created_at, updated_at FROM hermes_kanban_boards WHERE scope_key = ?1 ORDER BY created_at DESC")
                            .map_db()?;
                        let mut rows = stmt.query(params![scope_key]).map_db()?;
                        let mut boards = Vec::new();
                        while let Some(r) = rows.next().map_db()? {
                            let cols_raw: String = r.get(3).map_db()?;
                            let cols: Vec<String> = serde_json::from_str(&cols_raw).unwrap_or_default();
                            boards.push(json!({
                                "id": r.get::<_, String>(0).map_db()?,
                                "name": r.get::<_, String>(1).map_db()?,
                                "description": r.get::<_, String>(2).map_db()?,
                                "columns": cols,
                                "created_at": r.get::<_, i64>(4).map_db()?,
                                "updated_at": r.get::<_, i64>(5).map_db()?,
                            }));
                        }
                        Ok(json!({ "boards": boards }))
                    })
                    .map_err(|e| e.to_string()),
                "kanban_create_board" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'name'".to_string())?;
                    let desc = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
                    let cols: Vec<String> = args
                        .get("columns")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(ToString::to_string)).collect())
                        .filter(|v: &Vec<String>| !v.is_empty())
                        .unwrap_or_else(|| vec!["Backlog".into(), "In Progress".into(), "Review".into(), "Done".into()]);
                    let id = format!("board_{}", Uuid::new_v4().simple());
                    let cols_json = serde_json::to_string(&cols).unwrap_or_else(|_| "[]".into());
                    self.pool
                        .with_conn(|conn| {
                            conn.execute(
                                "INSERT INTO hermes_kanban_boards (id, scope_key, name, description, columns_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                                params![id, scope_key, name, desc, cols_json, now_ms],
                            )
                            .map_db()?;
                            Ok(json!({
                                "id": id,
                                "name": name,
                                "description": desc,
                                "columns": cols,
                                "created_at": now_ms,
                                "updated_at": now_ms,
                            }))
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_create_card" => {
                    let board_id = args
                        .get("board_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'board_id'".to_string())?;
                    let title = args
                        .get("title")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'title'".to_string())?;
                    let column = args
                        .get("column")
                        .or_else(|| args.get("column_name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Backlog");
                    let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
                    let priority = args.get("priority").and_then(|v| v.as_str()).unwrap_or("medium");
                    let assignee = args.get("assignee").and_then(|v| v.as_str());
                    let id = format!("card_{}", Uuid::new_v4().simple());
                    self.pool
                        .with_conn(|conn| {
                            let exists: bool = conn
                                .query_row(
                                    "SELECT EXISTS(SELECT 1 FROM hermes_kanban_boards WHERE id = ?1 AND scope_key = ?2)",
                                    params![board_id, scope_key],
                                    |r| r.get(0),
                                )
                                .map_db()?;
                            if !exists {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound(format!("Board '{board_id}' not found")));
                            }
                            conn.execute(
                                "INSERT INTO hermes_kanban_cards (id, scope_key, board_id, column_name, title, description, priority, assignee, labels_json, comments_json, checklist_json, position, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '[]', '[]', '[]', 0, ?9, ?9)",
                                params![id, scope_key, board_id, column, title, description, priority, assignee, now_ms],
                            )
                            .map_db()?;
                            Ok(json!({
                                "id": id,
                                "board_id": board_id,
                                "column": column,
                                "title": title,
                                "description": description,
                                "priority": priority,
                                "assignee": assignee,
                                "created_at": now_ms,
                            }))
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_move_card" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let col = args
                        .get("column")
                        .or_else(|| args.get("target_column"))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'column'".to_string())?;
                    self.pool
                        .with_conn(|conn| {
                            let n = conn
                                .execute(
                                    "UPDATE hermes_kanban_cards SET column_name = ?1, updated_at = ?2 WHERE id = ?3 AND scope_key = ?4",
                                    params![col, now_ms, card_id, scope_key],
                                )
                                .map_db()?;
                            if n == 0 {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound(format!("Card '{card_id}' not found")));
                            }
                            Ok(json!({ "id": card_id, "column": col, "updated_at": now_ms }))
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_get_board" => {
                    let board_id = args
                        .get("board_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'board_id'".to_string())?;
                    self.pool
                        .with_reader(|conn| {
                            let board = conn
                                .query_row(
                                    "SELECT id, name, description, columns_json, created_at, updated_at FROM hermes_kanban_boards WHERE id = ?1 AND scope_key = ?2",
                                    params![board_id, scope_key],
                                    |r| {
                                        let cols_raw: String = r.get(3)?;
                                        let cols: Vec<String> = serde_json::from_str(&cols_raw).unwrap_or_default();
                                        Ok(json!({
                                            "id": r.get::<_, String>(0)?,
                                            "name": r.get::<_, String>(1)?,
                                            "description": r.get::<_, String>(2)?,
                                            "columns": cols,
                                            "created_at": r.get::<_, i64>(4)?,
                                            "updated_at": r.get::<_, i64>(5)?,
                                        }))
                                    },
                                )
                                .optional()
                                .map_db()?
                                .ok_or_else(|| gpui_telegram_core::error::TelegramError::NotFound(format!("Board '{board_id}' not found")))?;
                            let mut stmt = conn
                                .prepare("SELECT id, board_id, column_name, title, description, priority, assignee, created_at, updated_at FROM hermes_kanban_cards WHERE board_id = ?1 AND scope_key = ?2 ORDER BY position ASC")
                                .map_db()?;
                            let mut rows = stmt.query(params![board_id, scope_key]).map_db()?;
                            let mut cards = Vec::new();
                            while let Some(r) = rows.next().map_db()? {
                                cards.push(json!({
                                    "id": r.get::<_, String>(0).map_db()?,
                                    "board_id": r.get::<_, String>(1).map_db()?,
                                    "column": r.get::<_, String>(2).map_db()?,
                                    "title": r.get::<_, String>(3).map_db()?,
                                    "description": r.get::<_, String>(4).map_db()?,
                                    "priority": r.get::<_, String>(5).map_db()?,
                                    "assignee": r.get::<_, Option<String>>(6).map_db()?,
                                    "created_at": r.get::<_, i64>(7).map_db()?,
                                    "updated_at": r.get::<_, i64>(8).map_db()?,
                                }));
                            }
                            Ok(json!({ "board": board, "cards": cards }))
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_get_card" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    self.pool
                        .with_reader(|conn| {
                            conn.query_row(
                                "SELECT id, board_id, column_name, title, description, priority, assignee, labels_json, comments_json, checklist_json, position, created_at, updated_at FROM hermes_kanban_cards WHERE id = ?1 AND scope_key = ?2",
                                params![card_id, scope_key],
                                |r| {
                                    let labels: Vec<String> = serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default();
                                    let comments: Vec<KanbanComment> = serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default();
                                    let checklists: Vec<KanbanChecklist> = serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default();
                                    let col: String = r.get(2)?;
                                    Ok(json!({
                                        "id": r.get::<_, String>(0)?,
                                        "board_id": r.get::<_, String>(1)?,
                                        "column": col,
                                        "column_name": col,
                                        "title": r.get::<_, String>(3)?,
                                        "description": r.get::<_, String>(4)?,
                                        "priority": r.get::<_, String>(5)?,
                                        "assignee": r.get::<_, Option<String>>(6)?,
                                        "labels": labels,
                                        "comments": comments,
                                        "checklists": checklists,
                                        "position": r.get::<_, i64>(10)?,
                                        "created_at": r.get::<_, i64>(11)?,
                                        "updated_at": r.get::<_, i64>(12)?,
                                    }))
                                },
                            )
                            .optional()
                            .map_db()?
                            .ok_or_else(|| gpui_telegram_core::error::TelegramError::NotFound(format!("Card '{card_id}' not found")))
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_update_card" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    self.pool
                        .with_transaction(|tx| {
                            let row = tx
                                .query_row(
                                    "SELECT board_id, column_name, title, description, priority, assignee, labels_json, comments_json, checklist_json, position, created_at FROM hermes_kanban_cards WHERE id = ?1 AND scope_key = ?2",
                                    params![card_id, scope_key],
                                    |r| {
                                        Ok((
                                            r.get::<_, String>(0)?,
                                            r.get::<_, String>(1)?,
                                            r.get::<_, String>(2)?,
                                            r.get::<_, String>(3)?,
                                            r.get::<_, String>(4)?,
                                            r.get::<_, Option<String>>(5)?,
                                            r.get::<_, String>(6)?,
                                            r.get::<_, String>(7)?,
                                            r.get::<_, String>(8)?,
                                            r.get::<_, i64>(9)?,
                                            r.get::<_, i64>(10)?,
                                        ))
                                    },
                                )
                                .optional()
                                .map_db()?
                                .ok_or_else(|| gpui_telegram_core::error::TelegramError::NotFound(format!("Card '{card_id}' not found")))?;

                            let (board_id, col, mut title, mut desc, mut priority, mut assignee, mut labels_json, comments_json, checklist_json, position, created_at) = row;
                            if let Some(t) = args.get("title").and_then(|v| v.as_str()) {
                                title = t.to_string();
                            }
                            if let Some(d) = args.get("description").and_then(|v| v.as_str()) {
                                desc = d.to_string();
                            }
                            if let Some(p) = args.get("priority").and_then(|v| v.as_str()) {
                                priority = p.to_string();
                            }
                            if let Some(a) = args.get("assignee") {
                                assignee = a.as_str().map(ToString::to_string);
                            }
                            if let Some(lbls) = args.get("labels").and_then(|v| v.as_array()) {
                                let vec_lbls: Vec<String> = lbls.iter().filter_map(|x| x.as_str().map(ToString::to_string)).collect();
                                labels_json = serde_json::to_string(&vec_lbls).unwrap_or_else(|_| "[]".into());
                            }
                            tx.execute(
                                "UPDATE hermes_kanban_cards SET title = ?1, description = ?2, priority = ?3, assignee = ?4, labels_json = ?5, updated_at = ?6 WHERE id = ?7 AND scope_key = ?8",
                                params![title, desc, priority, assignee, labels_json, now_ms, card_id, scope_key],
                            )
                            .map_db()?;
                            tx.execute(
                                "UPDATE hermes_kanban_boards SET updated_at = ?1 WHERE id = ?2 AND scope_key = ?3",
                                params![now_ms, board_id, scope_key],
                            )
                            .map_db()?;
                            let labels: Vec<String> = serde_json::from_str(&labels_json).unwrap_or_default();
                            let comments: Vec<KanbanComment> = serde_json::from_str(&comments_json).unwrap_or_default();
                            let checklists: Vec<KanbanChecklist> = serde_json::from_str(&checklist_json).unwrap_or_default();
                            Ok(json!({
                                "id": card_id,
                                "board_id": board_id,
                                "column": col,
                                "column_name": col,
                                "title": title,
                                "description": desc,
                                "priority": priority,
                                "assignee": assignee,
                                "labels": labels,
                                "comments": comments,
                                "checklists": checklists,
                                "position": position,
                                "created_at": created_at,
                                "updated_at": now_ms,
                            }))
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_assign_card" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let assignee = args.get("assignee").and_then(|v| v.as_str()).map(ToString::to_string);
                    self.pool
                        .with_transaction(|tx| {
                            let n = tx
                                .execute(
                                    "UPDATE hermes_kanban_cards SET assignee = ?1, updated_at = ?2 WHERE id = ?3 AND scope_key = ?4",
                                    params![assignee, now_ms, card_id, scope_key],
                                )
                                .map_db()?;
                            if n == 0 {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound(format!("Card '{card_id}' not found")));
                            }
                            tx.query_row(
                                "SELECT id, board_id, column_name, title, description, priority, assignee, created_at, updated_at FROM hermes_kanban_cards WHERE id = ?1 AND scope_key = ?2",
                                params![card_id, scope_key],
                                |r| {
                                    Ok(json!({
                                        "id": r.get::<_, String>(0)?,
                                        "board_id": r.get::<_, String>(1)?,
                                        "column": r.get::<_, String>(2)?,
                                        "title": r.get::<_, String>(3)?,
                                        "description": r.get::<_, String>(4)?,
                                        "priority": r.get::<_, String>(5)?,
                                        "assignee": r.get::<_, Option<String>>(6)?,
                                        "created_at": r.get::<_, i64>(7)?,
                                        "updated_at": r.get::<_, i64>(8)?,
                                    }))
                                },
                            )
                            .map_db()
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_add_comment" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let body = args
                        .get("body")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'body'".to_string())?;
                    let author = args.get("author").and_then(|v| v.as_str()).unwrap_or("hermes");
                    self.pool
                        .with_transaction(|tx| {
                            let comments_raw: Option<String> = tx
                                .query_row(
                                    "SELECT comments_json FROM hermes_kanban_cards WHERE id = ?1 AND scope_key = ?2",
                                    params![card_id, scope_key],
                                    |r| r.get(0),
                                )
                                .optional()
                                .map_db()?;
                            let Some(raw) = comments_raw else {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound(format!("Card '{card_id}' not found")));
                            };
                            let mut comments: Vec<KanbanComment> = serde_json::from_str(&raw).unwrap_or_default();
                            comments.push(KanbanComment {
                                id: format!("kcmt_{}", Uuid::new_v4().simple()),
                                author: author.to_string(),
                                body: body.to_string(),
                                created_at: dt_from_ms(now_ms),
                            });
                            let next_raw = serde_json::to_string(&comments).unwrap_or_else(|_| "[]".into());
                            tx.execute(
                                "UPDATE hermes_kanban_cards SET comments_json = ?1, updated_at = ?2 WHERE id = ?3 AND scope_key = ?4",
                                params![next_raw, now_ms, card_id, scope_key],
                            )
                            .map_db()?;
                            Ok(json!({ "id": card_id, "card_id": card_id, "comments": comments, "updated_at": now_ms }))
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_list_comments" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    self.pool
                        .with_reader(|conn| {
                            let raw: Option<String> = conn
                                .query_row(
                                    "SELECT comments_json FROM hermes_kanban_cards WHERE id = ?1 AND scope_key = ?2",
                                    params![card_id, scope_key],
                                    |r| r.get(0),
                                )
                                .optional()
                                .map_db()?;
                            let Some(comments_raw) = raw else {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound(format!("Card '{card_id}' not found")));
                            };
                            let comments: Vec<KanbanComment> = serde_json::from_str(&comments_raw).unwrap_or_default();
                            Ok(json!({ "card_id": card_id, "comments": comments }))
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_add_checklist" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let title = args
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Checklist")
                        .to_string();
                    let items: Vec<KanbanChecklistItem> = args
                        .get("items")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str())
                                .map(|t| KanbanChecklistItem {
                                    text: t.to_string(),
                                    checked: false,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    self.pool
                        .with_transaction(|tx| {
                            let raw: Option<String> = tx
                                .query_row(
                                    "SELECT checklist_json FROM hermes_kanban_cards WHERE id = ?1 AND scope_key = ?2",
                                    params![card_id, scope_key],
                                    |r| r.get(0),
                                )
                                .optional()
                                .map_db()?;
                            let Some(chk_raw) = raw else {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound(format!("Card '{card_id}' not found")));
                            };
                            let mut checklists: Vec<KanbanChecklist> = serde_json::from_str(&chk_raw).unwrap_or_default();
                            let checklist = KanbanChecklist {
                                id: format!("chk_{}", Uuid::new_v4().simple()),
                                title,
                                items,
                            };
                            checklists.push(checklist.clone());
                            let next_raw = serde_json::to_string(&checklists).unwrap_or_else(|_| "[]".into());
                            tx.execute(
                                "UPDATE hermes_kanban_cards SET checklist_json = ?1, updated_at = ?2 WHERE id = ?3 AND scope_key = ?4",
                                params![next_raw, now_ms, card_id, scope_key],
                            )
                            .map_db()?;
                            Ok(json!({ "id": checklist.id, "card_id": card_id, "checklist": checklist, "checklists": checklists }))
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_toggle_checklist" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let checklist_id = args.get("checklist_id").and_then(|v| v.as_str());
                    let item_index = args.get("item_index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let explicit_checked = args.get("checked").and_then(|v| v.as_bool());
                    self.pool
                        .with_transaction(|tx| {
                            let raw: Option<String> = tx
                                .query_row(
                                    "SELECT checklist_json FROM hermes_kanban_cards WHERE id = ?1 AND scope_key = ?2",
                                    params![card_id, scope_key],
                                    |r| r.get(0),
                                )
                                .optional()
                                .map_db()?;
                            let Some(chk_raw) = raw else {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound(format!("Card '{card_id}' not found")));
                            };
                            let mut checklists: Vec<KanbanChecklist> = serde_json::from_str(&chk_raw).unwrap_or_default();
                            let chk_opt = if let Some(cid) = checklist_id {
                                checklists.iter_mut().find(|c| c.id == cid)
                            } else {
                                checklists.first_mut()
                            };
                            let Some(chk) = chk_opt else {
                                return Err(gpui_telegram_core::error::TelegramError::NotFound("Checklist not found on card".into()));
                            };
                            let Some(item) = chk.items.get_mut(item_index) else {
                                return Err(gpui_telegram_core::error::TelegramError::Validation(format!("Checklist item index {item_index} out of range")));
                            };
                            item.checked = explicit_checked.unwrap_or(!item.checked);
                            let updated_chk = chk.clone();
                            let next_raw = serde_json::to_string(&checklists).unwrap_or_else(|_| "[]".into());
                            tx.execute(
                                "UPDATE hermes_kanban_cards SET checklist_json = ?1, updated_at = ?2 WHERE id = ?3 AND scope_key = ?4",
                                params![next_raw, now_ms, card_id, scope_key],
                            )
                            .map_db()?;
                            Ok(json!({ "card_id": card_id, "checklist": updated_chk, "checklists": checklists }))
                        })
                        .map_err(|e| e.to_string())
                }
                "kanban_delete_card" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let deleted = self
                        .pool
                        .with_conn(|conn| {
                            conn.execute(
                                "DELETE FROM hermes_kanban_cards WHERE id = ?1 AND scope_key = ?2",
                                params![card_id, scope_key],
                            )
                            .map_db()
                        })
                        .map_err(|e| e.to_string())?;
                    Ok(json!({ "card_id": card_id, "deleted": deleted > 0 }))
                }
                "kanban_search_cards" => {
                    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                    let board_id = args.get("board_id").and_then(|v| v.as_str());
                    self.pool
                        .with_reader(|conn| {
                            let mut stmt = conn
                                .prepare("SELECT id, board_id, column_name, title, description, priority, assignee FROM hermes_kanban_cards WHERE scope_key = ?1")
                                .map_db()?;
                            let mut rows = stmt.query(params![scope_key]).map_db()?;
                            let mut cards = Vec::new();
                            while let Some(r) = rows.next().map_db()? {
                                let bid: String = r.get(1).map_db()?;
                                if let Some(filter_bid) = board_id {
                                    if bid != filter_bid {
                                        continue;
                                    }
                                }
                                let title: String = r.get(3).map_db()?;
                                let desc: String = r.get(4).map_db()?;
                                if query.is_empty()
                                    || title.to_lowercase().contains(&query)
                                    || desc.to_lowercase().contains(&query)
                                {
                                    cards.push(json!({
                                        "id": r.get::<_, String>(0).map_db()?,
                                        "board_id": bid,
                                        "column": r.get::<_, String>(2).map_db()?,
                                        "title": title,
                                        "description": desc,
                                        "priority": r.get::<_, String>(5).map_db()?,
                                        "assignee": r.get::<_, Option<String>>(6).map_db()?,
                                    }));
                                }
                            }
                            Ok(json!({ "count": cards.len(), "cards": cards }))
                        })
                        .map_err(|e| e.to_string())
                }
                _ => Err(format!("Unsupported kanban action '{action}'")),
            }
        })
    }

    fn kv_set<'a>(
        &'a self,
        namespace: &'a str,
        key: &'a str,
        value: &'a str,
    ) -> BoxFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            self.hermes_kv_set(namespace, key, value)
                .map_err(|e| e.to_string())
        })
    }

    fn kv_get<'a>(
        &'a self,
        namespace: &'a str,
        key: &'a str,
    ) -> BoxFuture<'a, std::result::Result<Option<String>, String>> {
        Box::pin(async move {
            self.hermes_kv_get(namespace, key)
                .map_err(|e| e.to_string())
        })
    }

    fn record_checkpoint<'a>(
        &'a self,
        session_id: &'a str,
        turn_index: u64,
        file_path: &'a str,
        original_content: Option<&'a [u8]>,
    ) -> BoxFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            let id = format!("chk_{}", Uuid::new_v4().simple());
            let now_ms = Utc::now().timestamp_millis();
            let content_str = original_content.map(|b| String::from_utf8_lossy(b).into_owned());
            self.pool
                .with_conn(|conn| {
                    conn.execute(
                        r#"INSERT INTO hermes_checkpoints (id, session_id, turn_index, file_path, original_content, created_at)
                           VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                           ON CONFLICT(session_id, turn_index, file_path) DO UPDATE SET
                               original_content = excluded.original_content"#,
                        params![id, session_id, turn_index as i64, file_path, content_str, now_ms],
                    )
                    .map_db()?;
                    Ok(())
                })
                .map_err(|e| e.to_string())
        })
    }

    fn record_approval<'a>(
        &'a self,
        approval: PendingApproval,
    ) -> BoxFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            let now_ms = Utc::now().timestamp_millis();
            let pattern = approval.rule_id.unwrap_or(approval.tool_name);
            self.pool
                .with_conn(|conn| {
                    conn.execute(
                        r#"INSERT INTO hermes_approvals (id, session_id, command, risk_level, pattern_name, status, created_at)
                           VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)
                           ON CONFLICT(id) DO NOTHING"#,
                        params![approval.id, approval.session_id, approval.command, approval.risk_level, pattern, now_ms],
                    )
                    .map_db()?;
                    Ok(())
                })
                .map_err(|e| e.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_telegram_agent::hermes::toolsets::AgenticStateBackend;
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_converge_load_and_agentic_backend_persist_across_reopen() {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join("agentic_converge.db");
        let scope = "chat_42:default";

        {
            let db = ServerDatabase::open(&db_path).expect("open db 1");
            let mut local = LocalAgenticStore::new();
            let now = Utc::now();
            local.boards.insert(
                "board_m4".into(),
                KanbanBoard {
                    id: "board_m4".into(),
                    name: "Sprint M4".into(),
                    description: "Core integration".into(),
                    columns: vec!["Todo".into(), "Done".into()],
                    created_at: now,
                },
            );
            local.cards.insert(
                "card_m4_1".into(),
                KanbanCard {
                    id: "card_m4_1".into(),
                    board_id: "board_m4".into(),
                    column: "Todo".into(),
                    title: "Wire SQLite".into(),
                    description: "Feature 17".into(),
                    priority: "high".into(),
                    assignee: Some("alice".into()),
                    labels: vec!["m4".into()],
                    comments: Vec::new(),
                    checklists: Vec::new(),
                    position: 0,
                    created_at: now,
                    updated_at: now,
                },
            );
            local.connections.insert(
                "github".into(),
                IntegrationConnection {
                    name: "github".into(),
                    provider: "github".into(),
                    endpoint: Some("https://api.github.com".into()),
                    masked_secret: Some("gh_p***cret".into()),
                    connected: true,
                    updated_at: now,
                    last_tested_at: None,
                },
            );
            local
                .lineage
                .add_agent("child_s1".into(), Some("parent_s1".into()), "reviewer".into());

            db.converge_agentic_store(scope, &local).expect("converge");

            let backend: &dyn AgenticStateBackend = &db;
            backend
                .kv_set("runtime", "last_converge", "ok_v34")
                .await
                .expect("kv_set");
            backend
                .connections_op(
                    scope,
                    &json!({
                        "action": "connect",
                        "name": "linear",
                        "provider": "linear",
                        "api_key": "lin_tok_secret_value",
                        "endpoint": "https://api.linear.app",
                    }),
                )
                .await
                .expect("connections_op connect");
            backend
                .desktop_ui_act(
                    scope,
                    HermesUiAction {
                        action_type: "select_destination".into(),
                        payload: json!({"destination": "tasks", "panel": "inspector"}),
                    },
                )
                .await
                .expect("desktop_ui_act");
            let inspected = backend
                .desktop_ui_inspect(scope)
                .await
                .expect("desktop_ui_inspect");
            assert_eq!(inspected["desktop_state"]["active_destination"], "tasks");
        }

        // Re-open database from disk and verify full convergence survival
        let db2 = ServerDatabase::open(&db_path).expect("open db 2");
        let loaded = db2.load_agentic_store(scope).expect("load_agentic_store");
        assert_eq!(loaded.boards.len(), 1);
        assert_eq!(loaded.cards.len(), 1);
        assert!(loaded.connections.contains_key("github"));
        assert!(loaded.connections.contains_key("linear"));
        assert_eq!(loaded.lineage.nodes.len(), 1);
        assert_eq!(loaded.desktop_state.active_destination, "tasks");
        assert_eq!(
            db2.hermes_kv_get("runtime", "last_converge").unwrap().as_deref(),
            Some("ok_v34")
        );
    }

    #[tokio::test]
    async fn test_agentic_store_scope_isolation_pruning_deep_json_and_full_ops() {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join("agentic_remediation.db");
        let db = ServerDatabase::open(&db_path).expect("open db");
        let now = Utc::now();

        // 1. Scope isolation on duplicate board/card/cron/lineage IDs
        let mut local_a = LocalAgenticStore::new();
        local_a.boards.insert(
            "board_1".into(),
            KanbanBoard {
                id: "board_1".into(),
                name: "Scope A Board".into(),
                description: "A".into(),
                columns: vec!["Todo".into()],
                created_at: now,
            },
        );
        local_a.cards.insert(
            "card_1".into(),
            KanbanCard {
                id: "card_1".into(),
                board_id: "board_1".into(),
                column: "Todo".into(),
                title: "Card A".into(),
                description: "".into(),
                priority: "high".into(),
                assignee: None,
                labels: vec![],
                comments: vec![],
                checklists: vec![],
                position: 0,
                created_at: now,
                updated_at: now,
            },
        );
        local_a.cron_jobs.insert(
            "cron_1".into(),
            LocalCronJob {
                routine: CronRoutine::new("cron_1".to_string(), "@every 1h".to_string()).with_name("Cron A"),
                prompt: "A".into(),
                target_chat_id: None,
                model: None,
            },
        );
        local_a.lineage.add_agent("agent_1".into(), None, "worker".into());
        db.converge_agentic_store("scope_A", &local_a).unwrap();

        let mut local_b = LocalAgenticStore::new();
        local_b.boards.insert(
            "board_1".into(),
            KanbanBoard {
                id: "board_1".into(),
                name: "Scope B Board".into(),
                description: "B".into(),
                columns: vec!["Done".into()],
                created_at: now,
            },
        );
        local_b.cards.insert(
            "card_1".into(),
            KanbanCard {
                id: "card_1".into(),
                board_id: "board_1".into(),
                column: "Done".into(),
                title: "Card B".into(),
                description: "".into(),
                priority: "low".into(),
                assignee: None,
                labels: vec![],
                comments: vec![],
                checklists: vec![],
                position: 0,
                created_at: now,
                updated_at: now,
            },
        );
        local_b.cron_jobs.insert(
            "cron_1".into(),
            LocalCronJob {
                routine: CronRoutine::new("cron_1".to_string(), "@every 2h".to_string()).with_name("Cron B"),
                prompt: "B".into(),
                target_chat_id: None,
                model: None,
            },
        );
        local_b.lineage.add_agent("agent_1".into(), None, "reviewer".into());
        db.converge_agentic_store("scope_B", &local_b).unwrap();

        let loaded_a = db.load_agentic_store("scope_A").unwrap();
        assert_eq!(loaded_a.boards.get("board_1").unwrap().name, "Scope A Board");
        assert_eq!(loaded_a.cards.get("card_1").unwrap().title, "Card A");
        assert_eq!(loaded_a.cron_jobs.get("cron_1").unwrap().prompt, "A");
        assert_eq!(loaded_a.lineage.nodes.get("agent_1").unwrap().role, "worker");

        // 2. Pruning deleted items on convergence for scope_A without affecting scope_B
        let empty = LocalAgenticStore::new();
        db.converge_agentic_store("scope_A", &empty).unwrap();
        let pruned_a = db.load_agentic_store("scope_A").unwrap();
        assert!(pruned_a.boards.is_empty());
        assert!(pruned_a.cards.is_empty());
        assert!(pruned_a.cron_jobs.is_empty());
        assert!(pruned_a.lineage.nodes.is_empty());

        let loaded_b = db.load_agentic_store("scope_B").unwrap();
        assert_eq!(loaded_b.boards.len(), 1);
        assert_eq!(loaded_b.cards.len(), 1);

        // 3. Deeply nested JSON (>128 levels) survives converge -> reopen -> load
        let mut deep = json!({ "leaf": "must_survive_reopen" });
        for _ in 0..150 {
            deep = json!({ "nested": deep });
        }
        let mut deep_store = LocalAgenticStore::new();
        deep_store.ui_action_log.push(HermesUiAction {
            action_type: "deep".into(),
            payload: deep.clone(),
        });
        db.converge_agentic_store("scope_deep", &deep_store).unwrap();
        drop(db);

        let db2 = ServerDatabase::open(&db_path).expect("reopen");
        let loaded_deep = db2.load_agentic_store("scope_deep").unwrap();
        assert_ne!(loaded_deep.ui_action_log[0].payload, json!({}));

        // 4. Full cron_op and kanban_op SQLite coverage (no "Delegate..." errors)
        let backend: &dyn AgenticStateBackend = &db2;
        let created_cron = backend
            .cron_op(
                "scope_ops",
                &json!({ "action": "create", "id": "c1", "name": "C1", "schedule": "0 * * * *", "prompt": "hi" }),
            )
            .await
            .unwrap();
        assert_eq!(created_cron["id"], "c1");
        assert_eq!(
            backend.cron_op("scope_ops", &json!({ "action": "get", "id": "c1" })).await.unwrap()["id"],
            "c1"
        );
        assert!(backend
            .cron_op("scope_ops", &json!({ "action": "preview", "id": "c1" }))
            .await
            .unwrap()["scheduled_fires_next_7_days"]
            .is_number());
        assert_eq!(
            backend.cron_op("scope_ops", &json!({ "action": "run_now", "id": "c1" })).await.unwrap()["status"],
            "completed"
        );
        assert_eq!(
            backend.cron_op("scope_ops", &json!({ "action": "history", "id": "c1" })).await.unwrap()["history"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let b = backend
            .kanban_op("scope_ops", "kanban_create_board", &json!({ "name": "Ops" }))
            .await
            .unwrap();
        let bid = b["id"].as_str().unwrap();
        let c = backend
            .kanban_op(
                "scope_ops",
                "kanban_create_card",
                &json!({ "board_id": bid, "title": "Initial" }),
            )
            .await
            .unwrap();
        let cid = c["id"].as_str().unwrap();
        let updated = backend
            .kanban_op(
                "scope_ops",
                "kanban_update_card",
                &json!({ "card_id": cid, "title": "Updated", "priority": "critical" }),
            )
            .await
            .unwrap();
        assert_eq!(updated["title"], "Updated");
        assert_eq!(updated["priority"], "critical");
        let assigned = backend
            .kanban_op(
                "scope_ops",
                "kanban_assign_card",
                &json!({ "card_id": cid, "assignee": "bob" }),
            )
            .await
            .unwrap();
        assert_eq!(assigned["assignee"], "bob");
        backend
            .kanban_op(
                "scope_ops",
                "kanban_add_comment",
                &json!({ "card_id": cid, "author": "alice", "body": "LGTM" }),
            )
            .await
            .unwrap();
        let comments = backend
            .kanban_op("scope_ops", "kanban_list_comments", &json!({ "card_id": cid }))
            .await
            .unwrap();
        assert_eq!(comments["comments"].as_array().unwrap().len(), 1);
        let chk = backend
            .kanban_op(
                "scope_ops",
                "kanban_add_checklist",
                &json!({ "card_id": cid, "title": "QA", "items": ["Check SQLite"] }),
            )
            .await
            .unwrap();
        let chk_id = chk["id"].as_str().unwrap();
        let toggled = backend
            .kanban_op(
                "scope_ops",
                "kanban_toggle_checklist",
                &json!({ "card_id": cid, "checklist_id": chk_id, "item_index": 0 }),
            )
            .await
            .unwrap();
        assert_eq!(toggled["checklist"]["items"][0]["checked"], true);
    }
}
