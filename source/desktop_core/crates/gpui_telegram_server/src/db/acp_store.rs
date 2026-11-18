use crate::db::{ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::Result;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

impl ServerDatabase {
    pub fn upsert_acp_task(&self, task_id: &str, payload: &Value, status: &str) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let json_str = serde_json::to_string(payload).unwrap_or_else(|_| "{}".into());
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO acp_tasks (task_id, payload_json, status, updated_at_ms) VALUES (?1, ?2, ?3, ?4)",
                params![task_id, json_str, status, now_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn get_acp_task(&self, task_id: &str) -> Result<Option<Value>> {
        self.pool.with_reader(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT payload_json FROM acp_tasks WHERE task_id = ?1",
                    params![task_id],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?;
            Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
        })
    }

    pub fn load_acp_tasks(&self) -> Result<Vec<(String, Value)>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare("SELECT task_id, payload_json FROM acp_tasks ORDER BY updated_at_ms ASC").map_db()?;
            let rows = stmt.query_map([], |r| {
                let id: String = r.get(0)?;
                let raw: String = r.get(1)?;
                Ok((id, serde_json::from_str(&raw).unwrap_or(Value::Null)))
            }).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn upsert_approval_gate(&self, gate_id: &str, task_id: &str, decision: Option<&str>, payload: &Value) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let json_str = serde_json::to_string(payload).unwrap_or_else(|_| "{}".into());
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO acp_approval_gates (gate_id, task_id, decision, payload_json, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![gate_id, task_id, decision, json_str, now_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn load_approval_gates(&self) -> Result<Vec<(String, Value)>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare("SELECT gate_id, payload_json FROM acp_approval_gates ORDER BY updated_at_ms ASC").map_db()?;
            let rows = stmt.query_map([], |r| {
                let id: String = r.get(0)?;
                let raw: String = r.get(1)?;
                Ok((id, serde_json::from_str(&raw).unwrap_or(Value::Null)))
            }).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn get_approval_gate(
        &self,
        gate_id: &str,
    ) -> Result<Option<gpui_telegram_agent::tasks::ApprovalGate>> {
        self.pool.with_reader(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT payload_json FROM acp_approval_gates WHERE gate_id = ?1",
                    params![gate_id],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?;
            match raw {
                Some(s) => {
                    let gate = serde_json::from_str::<gpui_telegram_agent::tasks::ApprovalGate>(&s)?;
                    Ok(Some(gate))
                }
                None => Ok(None),
            }
        })
    }

    pub fn try_decide_approval_gate(
        &self,
        gate_id: &str,
        decision_label: &str,
        new_payload: &serde_json::Value,
    ) -> gpui_telegram_core::error::Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let json_str = serde_json::to_string(new_payload).unwrap_or_else(|_| "{}".into());
        self.pool.with_writer(|conn| {
            conn.execute(
                "UPDATE acp_approval_gates SET decision = ?2, payload_json = ?3, updated_at_ms = ?4 WHERE gate_id = ?1 AND decision IS NULL",
                params![gate_id, decision_label, json_str, now_ms],
            )
            .map_db()?;
            Ok(conn.changes() == 1)
        })
    }

    pub fn try_consume_approval_gate(&self, gate_id: &str, new_payload: &Value) -> Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let json_str = serde_json::to_string(new_payload).unwrap_or_else(|_| "{}".into());
        self.pool.with_writer(|conn| {
            conn.execute(
                "UPDATE acp_approval_gates SET decision = 'consumed', payload_json = ?2, updated_at_ms = ?3 WHERE gate_id = ?1 AND decision IN ('approve_once', 'approve')",
                params![gate_id, json_str, now_ms],
            )
            .map_db()?;
            Ok(conn.changes() == 1)
        })
    }

    pub fn upsert_mcp_server(&self, server_id: &str, name: &str, endpoint: &str, status: &str, tools: &Value) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let tools_json = serde_json::to_string(tools).unwrap_or_else(|_| "[]".into());
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO mcp_servers_db (server_id, name, endpoint, status, tools_json, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![server_id, name, endpoint, status, tools_json, now_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn load_mcp_servers(&self) -> Result<Vec<(String, String, String, String, Value)>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare("SELECT server_id, name, endpoint, status, tools_json FROM mcp_servers_db ORDER BY updated_at_ms ASC").map_db()?;
            let rows = stmt.query_map([], |r| {
                let tools_raw: String = r.get(4)?;
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, serde_json::from_str(&tools_raw).unwrap_or(Value::Array(vec![]))))
            }).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn delete_mcp_server(&self, server_id: &str) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let n = conn.execute("DELETE FROM mcp_servers_db WHERE server_id = ?1", params![server_id]).map_db()?;
            Ok(n > 0)
        })
    }

    pub fn upsert_computer_lease(&self, lease_id: &str, owner_user_id: u64, session_id: Option<&str>, status: &str, actions: &Value, created_at_ms: i64) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let actions_json = serde_json::to_string(actions).unwrap_or_else(|_| "[]".into());
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO computer_leases_db (lease_id, owner_user_id, session_id, status, actions_json, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![lease_id, owner_user_id as i64, session_id, status, actions_json, created_at_ms, now_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn load_computer_leases(&self) -> Result<Vec<(String, u64, Option<String>, String, Value, i64, i64)>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare("SELECT lease_id, owner_user_id, session_id, status, actions_json, created_at_ms, updated_at_ms FROM computer_leases_db ORDER BY created_at_ms ASC").map_db()?;
            let rows = stmt.query_map([], |r| {
                let raw: String = r.get(4)?;
                Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?, r.get(3)?, serde_json::from_str(&raw).unwrap_or(Value::Array(vec![])), r.get(5)?, r.get(6)?))
            }).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn upsert_talk_session(&self, session_id: &str, user_id: u64, status: &str, transcript: &Value, created_at_ms: i64) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let transcript_json = serde_json::to_string(transcript).unwrap_or_else(|_| "[]".into());
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO talk_sessions_db (session_id, user_id, status, transcript_json, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![session_id, user_id as i64, status, transcript_json, created_at_ms, now_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn load_talk_sessions(&self) -> Result<Vec<(String, u64, String, Value, i64)>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare("SELECT session_id, user_id, status, transcript_json, created_at_ms FROM talk_sessions_db ORDER BY created_at_ms ASC").map_db()?;
            let rows = stmt.query_map([], |r| {
                let raw: String = r.get(3)?;
                Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?, serde_json::from_str(&raw).unwrap_or(Value::Array(vec![])), r.get(4)?))
            }).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn get_talk_session_owner(&self, session_id: &str) -> Result<Option<u64>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT user_id FROM talk_sessions_db WHERE session_id = ?1",
                params![session_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|opt| opt.map(|uid| uid.max(0) as u64))
            .map_db()
        })
    }
}
