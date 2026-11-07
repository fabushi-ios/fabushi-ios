use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{params, Connection};
use serde_json::Value;

use crate::gateway_command_error::GatewayCommandError;

#[path = "device_pairing_protocol.rs"]
pub mod device_pairing_protocol;
#[path = "device_pairing_tokens.rs"]
pub mod device_pairing_tokens;
#[path = "device_pairing_storage.rs"]
pub mod device_pairing_storage;

pub use self::device_pairing_protocol::*;

pub struct DevicePairingOwner {
    pub(crate) conn: Mutex<Connection>,
    pub(crate) db_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTokenAuthorization {
    pub device_id: String,
    pub role: String,
    pub scopes: Vec<String>,
}

pub(crate) const PENDING_PAIRING_TTL_MS: i64 = 900_000;

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(crate) fn new_device_token() -> Result<String, GatewayCommandError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| GatewayCommandError::Internal("failed to generate device token".into()))?;
    Ok(format!(
        "sand-device-tok-{}",
        URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub(crate) fn scopes_for_role(
    scopes_json: Option<&str>,
    role: &str,
) -> Result<Value, GatewayCommandError> {
    let scopes = scopes_json
        .unwrap_or("[]")
        .parse::<Value>()
        .map_err(|error| {
            GatewayCommandError::BadRequest(format!("invalid approved device scopes: {error}"))
        })?;
    let Some(scopes) = scopes.as_array() else {
        return Err(GatewayCommandError::BadRequest(
            "approved device scopes must be an array".into(),
        ));
    };
    let mut normalized = Vec::new();
    for scope in scopes.iter().filter_map(Value::as_str).map(str::trim) {
        if scope.is_empty()
            || (role == "operator" && !scope.starts_with("operator."))
            || (role != "operator" && scope.starts_with("operator."))
        {
            continue;
        }
        if !normalized.iter().any(|known| known == scope) {
            normalized.push(scope.to_string());
        }
    }
    Ok(Value::Array(
        normalized.into_iter().map(Value::String).collect(),
    ))
}

pub(crate) fn parse_string_set(raw: &str) -> Option<BTreeSet<String>> {
    let values = serde_json::from_str::<Value>(raw).ok()?.as_array()?.clone();
    let mut result = BTreeSet::new();
    for value in values {
        let value = value.as_str()?.trim();
        if value.is_empty() {
            return None;
        }
        result.insert(value.to_string());
    }
    Some(result)
}

pub(crate) fn pending_request_is_compatible(
    existing_public_key: &str,
    existing_browser_origin: Option<&str>,
    existing_role: Option<&str>,
    existing_roles_json: Option<&str>,
    existing_scopes_json: Option<&str>,
    public_key: &str,
    browser_origin: Option<&str>,
    role: &str,
    roles_json: &str,
    scopes_json: &str,
) -> bool {
    if existing_public_key != public_key
        || existing_browser_origin != browser_origin
        || existing_role != Some(role)
    {
        return false;
    }
    let (Some(existing_roles), Some(existing_scopes), Some(requested_roles), Some(requested_scopes)) = (
        existing_roles_json.and_then(parse_string_set),
        existing_scopes_json.and_then(parse_string_set),
        parse_string_set(roles_json),
        parse_string_set(scopes_json),
    ) else {
        return false;
    };
    requested_roles.is_subset(&existing_roles) && requested_scopes.is_subset(&existing_scopes)
}

pub(crate) fn bump_revision(conn: &Connection) -> Result<u64, rusqlite::Error> {
    let now = now_ms();
    let current_val: Option<String> = conn
        .query_row(
            "SELECT value_json FROM config_machine_state WHERE state_key = 'devicePairing.revision'",
            [],
            |row| row.get(0),
        )
        .ok();
    let current_rev: u64 = current_val
        .and_then(|v| v.trim_matches('"').parse().ok())
        .unwrap_or(0);
    let next_rev = current_rev + 1;
    conn.execute(
        "INSERT INTO config_machine_state (state_key, value_json, updated_at_ms)
         VALUES ('devicePairing.revision', ?1, ?2)
         ON CONFLICT(state_key) DO UPDATE SET value_json = ?1, updated_at_ms = ?2",
        params![format!("\"{next_rev}\""), now],
    )?;
    Ok(next_rev)
}

pub(crate) fn purge_expired_pending_requests(
    conn: &Connection,
    now: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM device_pairing_pending
         WHERE COALESCE(refreshed_at_ms, ts) <= ?1 - ?2",
        params![now, PENDING_PAIRING_TTL_MS],
    )?;
    Ok(())
}

impl DevicePairingOwner {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let conn = Connection::open(&path).map_err(|e| format!("Failed to open pairing db: {e}"))?;
        let owner = Self {
            conn: Mutex::new(conn),
            db_path: Some(path),
        };
        owner
            .init_schema()
            .map_err(|e| format!("Failed to init schema: {e}"))?;
        Ok(owner)
    }

    pub fn in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory()
            .map_err(|e| format!("Failed to open in-memory pairing db: {e}"))?;
        let owner = Self {
            conn: Mutex::new(conn),
            db_path: None,
        };
        owner
            .init_schema()
            .map_err(|e| format!("Failed to init schema: {e}"))?;
        Ok(owner)
    }

    fn init_schema(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS device_pairing_pending (
                request_id TEXT PRIMARY KEY NOT NULL,
                device_id TEXT NOT NULL,
                public_key TEXT NOT NULL,
                display_name TEXT,
                platform TEXT,
                device_family TEXT,
                client_id TEXT,
                client_mode TEXT,
                browser_origin TEXT,
                role TEXT,
                roles_json TEXT,
                scopes_json TEXT,
                remote_ip TEXT,
                silent INTEGER,
                is_repair INTEGER,
                ts INTEGER NOT NULL,
                refreshed_at_ms INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_device_pairing_pending_device_id ON device_pairing_pending(device_id);

            CREATE TABLE IF NOT EXISTS device_pairing_paired (
                device_id TEXT PRIMARY KEY NOT NULL,
                public_key TEXT NOT NULL,
                display_name TEXT,
                operator_label TEXT,
                platform TEXT,
                device_family TEXT,
                client_id TEXT,
                client_mode TEXT,
                browser_origin TEXT,
                role TEXT,
                roles_json TEXT,
                scopes_json TEXT,
                approved_scopes_json TEXT,
                remote_ip TEXT,
                tokens_json TEXT NOT NULL,
                approved_via TEXT,
                node_surface_json TEXT,
                pending_node_surface_json TEXT,
                created_at_ms INTEGER NOT NULL,
                approved_at_ms INTEGER NOT NULL,
                last_seen_at_ms INTEGER,
                last_seen_reason TEXT
            );

            CREATE TABLE IF NOT EXISTS device_bootstrap_tokens (
                token_key TEXT PRIMARY KEY NOT NULL,
                token TEXT NOT NULL,
                setup_id TEXT,
                ts INTEGER NOT NULL,
                device_id TEXT,
                public_key TEXT,
                profile_json TEXT,
                redeemed_profile_json TEXT,
                pending_profile_json TEXT,
                issued_at_ms INTEGER NOT NULL,
                expires_at_ms INTEGER NOT NULL DEFAULT 0,
                last_used_at_ms INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_device_bootstrap_tokens_setup_id ON device_bootstrap_tokens(setup_id);

            CREATE TABLE IF NOT EXISTS device_pairing_join_codes (
                shortcode TEXT PRIMARY KEY NOT NULL,
                payload_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                expires_at_ms INTEGER NOT NULL,
                redeemed_at_ms INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_device_pairing_join_codes_expiry
                ON device_pairing_join_codes(expires_at_ms);

            CREATE TABLE IF NOT EXISTS device_pair_setup_completions (
                setup_id TEXT PRIMARY KEY NOT NULL,
                device_id TEXT NOT NULL,
                device_name TEXT,
                access TEXT NOT NULL,
                completed_at_ms INTEGER NOT NULL,
                delivery_state TEXT NOT NULL,
                retain_until_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS config_machine_state (
                state_key TEXT PRIMARY KEY NOT NULL,
                value_json TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );
            "#,
        )?;
        let has_bootstrap_expiry: bool = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('device_bootstrap_tokens')
                WHERE name = 'expires_at_ms'
            )",
            [],
            |row| row.get(0),
        )?;
        if !has_bootstrap_expiry {
            conn.execute_batch(
                "ALTER TABLE device_bootstrap_tokens
                   ADD COLUMN expires_at_ms INTEGER NOT NULL DEFAULT 0;
                 UPDATE device_bootstrap_tokens
                    SET expires_at_ms = issued_at_ms + 900000
                  WHERE expires_at_ms = 0;",
            )?;
        } else {
            conn.execute(
                "UPDATE device_bootstrap_tokens
                    SET expires_at_ms = issued_at_ms + ?1
                  WHERE expires_at_ms = 0",
                params![PENDING_PAIRING_TTL_MS],
            )?;
        }
        Ok(())
    }

    pub fn current_revision(&self) -> u64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value_json FROM config_machine_state WHERE state_key = 'devicePairing.revision'",
            [],
            |row| row.get(0),
        )
        .ok()
        .and_then(|v: String| v.trim_matches('"').parse().ok())
        .unwrap_or(0)
    }
}

// Parity ledger symbols presence anchor:
// - DevicePairingOwner
// - register_device_pairing_join_code
// - handle_pair_request

#[cfg(test)]
mod tests {
    use super::{DevicePairingOwner, PENDING_PAIRING_TTL_MS};
    use rusqlite::params;
    use serde_json::{Value, json};

    #[test]
    fn expired_pending_request_is_rejected_before_status_or_approval() {
        let owner = DevicePairingOwner::in_memory().expect("pairing owner");
        let expired_at = super::now_ms() - PENDING_PAIRING_TTL_MS - 1;
        {
            let conn = owner.conn.lock().expect("pairing database lock");
            conn.execute(
                "INSERT INTO device_pairing_pending
                 (request_id, device_id, public_key, ts, refreshed_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params!["expired-request", "expired-device", "expired-key", expired_at],
            )
            .expect("insert expired pending request");
        }

        let status = owner
            .handle_pair_status(&json!({ "requestId": "expired-request" }))
            .expect("status lookup");
        assert_eq!(status["status"], "rejected");

        let approval = owner.handle_pair_approve(&json!({
            "requestId": "expired-request"
        }));
        assert!(approval.is_err());
    }

    #[test]
    fn token_rotation_clears_retained_node_surfaces() {
        let owner = DevicePairingOwner::in_memory().expect("pairing owner");
        let request = owner
            .handle_pair_request(&json!({
                "deviceId": "rotation-surface-device",
                "publicKey": "rotation-surface-key"
            }))
            .expect("pair request");
        owner
            .handle_pair_approve(&json!({
                "requestId": request["requestId"]
            }))
            .expect("pair approval");

        {
            let conn = owner.conn.lock().expect("pairing database lock");
            conn.execute(
                "UPDATE device_pairing_paired
                 SET node_surface_json = ?1, pending_node_surface_json = ?2
                 WHERE device_id = ?3",
                params!["{\"generation\":1}", "{\"generation\":2}", "rotation-surface-device"],
            )
            .expect("seed retained node surfaces");
        }

        owner
            .handle_pair_rotate_token(&json!({
                "deviceId": "rotation-surface-device"
            }))
            .expect("token rotation");

        let conn = owner.conn.lock().expect("pairing database lock");
        let surfaces: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT node_surface_json, pending_node_surface_json
                 FROM device_pairing_paired WHERE device_id = ?1",
                params!["rotation-surface-device"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read rotated pairing");
        assert_eq!(surfaces, (None, None));
    }

    #[test]
    fn repair_request_is_pending_without_overwriting_paired_credentials_or_surface() {
        let owner = DevicePairingOwner::in_memory().expect("pairing owner");
        let initial_request = owner
            .handle_pair_request(&json!({
                "deviceId": "repair-request-device",
                "publicKey": "initial-repair-key",
                "roles": ["operator"]
            }))
            .expect("initial pair request");
        owner
            .handle_pair_approve(&json!({
                "requestId": initial_request["requestId"]
            }))
            .expect("initial pair approval");

        {
            let conn = owner.conn.lock().expect("pairing database lock");
            conn.execute(
                "UPDATE device_pairing_paired
                 SET node_surface_json = ?1, pending_node_surface_json = ?2
                 WHERE device_id = ?3",
                params![
                    "{\"generation\":7}",
                    "{\"generation\":8}",
                    "repair-request-device"
                ],
            )
            .expect("seed retained node surfaces");
        }

        let (old_public_key, old_tokens, old_surface, old_pending_surface): (
            String,
            String,
            Option<String>,
            Option<String>,
        ) = {
            let conn = owner.conn.lock().expect("pairing database lock");
            conn.query_row(
                "SELECT public_key, tokens_json, node_surface_json, pending_node_surface_json
                 FROM device_pairing_paired WHERE device_id = ?1",
                params!["repair-request-device"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read paired identity before repair request")
        };

        let repair_request = owner
            .handle_pair_request(&json!({
                "deviceId": "repair-request-device",
                "publicKey": "replacement-repair-key",
                "roles": ["operator"]
            }))
            .expect("repair pair request");
        assert_eq!(repair_request["status"], "pending");
        assert_eq!(repair_request["isRepair"], true);
        assert_eq!(repair_request["created"], true);

        let (pending_public_key, paired_public_key, paired_tokens, paired_surface, paired_pending_surface): (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = {
            let conn = owner.conn.lock().expect("pairing database lock");
            let pending_public_key = conn
                .query_row(
                    "SELECT public_key FROM device_pairing_pending WHERE request_id = ?1",
                    params![repair_request["requestId"].as_str().expect("requestId")],
                    |row| row.get(0),
                )
                .expect("read repair pending identity");
            let paired = conn
                .query_row(
                    "SELECT public_key, tokens_json, node_surface_json, pending_node_surface_json
                     FROM device_pairing_paired WHERE device_id = ?1",
                    params!["repair-request-device"],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("read paired identity after repair request");
            (pending_public_key, paired.0, paired.1, paired.2, paired.3)
        };

        assert_eq!(pending_public_key, "replacement-repair-key");
        assert_eq!(paired_public_key, old_public_key);
        assert_eq!(paired_tokens, old_tokens);
        assert_eq!(paired_surface, old_surface);
        assert_eq!(paired_pending_surface, old_pending_surface);
    }

    #[test]
    fn token_rotation_preserves_tokens_for_other_approved_roles() {
        let owner = DevicePairingOwner::in_memory().expect("pairing owner");
        let request = owner
            .handle_pair_request(&json!({
                "deviceId": "multi-role-rotation-device",
                "publicKey": "multi-role-rotation-key",
                "roles": ["operator", "node"]
            }))
            .expect("pair request");
        owner
            .handle_pair_approve(&json!({
                "requestId": request["requestId"]
            }))
            .expect("pair approval");

        let before: Value = {
            let conn = owner.conn.lock().expect("pairing database lock");
            let raw: String = conn
                .query_row(
                    "SELECT tokens_json FROM device_pairing_paired WHERE device_id = ?1",
                    params!["multi-role-rotation-device"],
                    |row| row.get(0),
                )
                .expect("read initial role tokens");
            serde_json::from_str(&raw).expect("parse initial role tokens")
        };

        owner
            .handle_pair_rotate_token(&json!({
                "deviceId": "multi-role-rotation-device",
                "role": "node"
            }))
            .expect("node token rotation");

        let after: Value = {
            let conn = owner.conn.lock().expect("pairing database lock");
            let raw: String = conn
                .query_row(
                    "SELECT tokens_json FROM device_pairing_paired WHERE device_id = ?1",
                    params!["multi-role-rotation-device"],
                    |row| row.get(0),
                )
                .expect("read rotated role tokens");
            serde_json::from_str(&raw).expect("parse rotated role tokens")
        };

        assert_eq!(before["operator"]["token"], after["operator"]["token"]);
        assert_ne!(before["node"]["token"], after["node"]["token"]);
    }
}
