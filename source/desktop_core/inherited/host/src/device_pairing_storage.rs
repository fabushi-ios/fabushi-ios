use rusqlite::params;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    bump_revision, new_device_token, now_ms, pending_request_is_compatible,
    purge_expired_pending_requests, scopes_for_role, DevicePairingOwner, PENDING_PAIRING_TTL_MS,
};
use crate::gateway_command_error::GatewayCommandError;

impl DevicePairingOwner {
    pub fn handle_pair_request(&self, args: &Value) -> Result<Value, GatewayCommandError> {
        let device_id = args
            .get("deviceId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                GatewayCommandError::BadRequest("device.pair.request requires deviceId".into())
            })?;
        let public_key = args
            .get("publicKey")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                GatewayCommandError::BadRequest("device.pair.request requires publicKey".into())
            })?;

        let display_name = args
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or(device_id);
        let platform = args
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let device_family = args.get("deviceFamily").and_then(Value::as_str);
        let client_id = args.get("clientId").and_then(Value::as_str);
        let client_mode = args.get("clientMode").and_then(Value::as_str);
        let browser_origin = args.get("browserOrigin").and_then(Value::as_str);
        let role = args
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("operator");
        let roles_json = args
            .get("roles")
            .map(|v| v.to_string())
            .unwrap_or_else(|| json!([role]).to_string());
        let scopes_json = args
            .get("scopes")
            .map(|v| v.to_string())
            .unwrap_or_else(|| json!(["operator.read", "operator.write"]).to_string());
        let silent = if args.get("silent").and_then(Value::as_bool) == Some(true) {
            1
        } else {
            0
        };

        let conn = self.conn.lock().unwrap();
        let now = now_ms();
        purge_expired_pending_requests(&conn, now).map_err(|error| {
            GatewayCommandError::Internal(format!("Failed to expire pending pairings: {error}"))
        })?;

        // Check if device is already paired (is_repair)
        let is_repair: bool = conn
            .query_row(
                "SELECT COUNT(1) FROM device_pairing_paired WHERE device_id = ?1",
                params![device_id],
                |row| row.get(0),
            )
            .map(|c: i64| c > 0)
            .unwrap_or(false);

        let pending_candidates = {
            let mut statement = conn
                .prepare(
                    "SELECT request_id, public_key, browser_origin, role, roles_json, scopes_json
                     FROM device_pairing_pending
                     WHERE device_id = ?1
                     ORDER BY ts DESC",
                )
                .map_err(|error| {
                    GatewayCommandError::Internal(format!(
                        "Failed to inspect pending pairing requests: {error}"
                    ))
                })?;
            let rows = statement
                .query_map(params![device_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                })
                .map_err(|error| {
                    GatewayCommandError::Internal(format!(
                        "Failed to enumerate pending pairing requests: {error}"
                    ))
                })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
                GatewayCommandError::Internal(format!(
                    "Failed to read pending pairing request: {error}"
                ))
            })?
        };

        if let Some((request_id, ..)) =
            pending_candidates.iter().find(|(_, existing_public_key, existing_browser_origin, existing_role, existing_roles, existing_scopes)| {
                pending_request_is_compatible(
                    existing_public_key,
                    existing_browser_origin.as_deref(),
                    existing_role.as_deref(),
                    existing_roles.as_deref(),
                    existing_scopes.as_deref(),
                    public_key,
                    browser_origin,
                    role,
                    &roles_json,
                    &scopes_json,
                )
            })
        {
            conn.execute(
                "UPDATE device_pairing_pending
                 SET display_name = ?1, platform = ?2, device_family = ?3,
                     client_id = ?4, client_mode = ?5, browser_origin = ?6,
                     silent = ?7, refreshed_at_ms = ?8
                 WHERE request_id = ?9",
                params![
                    display_name,
                    platform,
                    device_family,
                    client_id,
                    client_mode,
                    browser_origin,
                    silent,
                    now,
                    request_id,
                ],
            )
            .map_err(|error| {
                GatewayCommandError::Internal(format!(
                    "Failed to refresh pending pairing request: {error}"
                ))
            })?;
            return Ok(json!({
                "status": "pending",
                "requestId": request_id,
                "deviceId": device_id,
                "created": false,
                "refreshedAtMs": now,
                "isRepair": is_repair,
                "expiresAtMs": now + PENDING_PAIRING_TTL_MS
            }));
        }

        if !pending_candidates.is_empty() {
            conn.execute(
                "DELETE FROM device_pairing_pending WHERE device_id = ?1",
                params![device_id],
            )
            .map_err(|error| {
                GatewayCommandError::Internal(format!(
                    "Failed to supersede pending pairing requests: {error}"
                ))
            })?;
        }

        let request_id = format!("pair-{}", Uuid::new_v4());
        let is_repair_int = if is_repair { 1 } else { 0 };

        conn.execute(
            r#"INSERT INTO device_pairing_pending (
                request_id, device_id, public_key, display_name, platform, device_family,
                client_id, client_mode, browser_origin, role, roles_json, scopes_json,
                silent, is_repair, ts, refreshed_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)"#,
            params![
                request_id, device_id, public_key, display_name, platform, device_family,
                client_id, client_mode, browser_origin, role, roles_json, scopes_json,
                silent, is_repair_int, now
            ],
        ).map_err(|e| GatewayCommandError::Internal(format!("Failed to store pending pairing: {e}")))?;

        Ok(json!({
            "status": "pending",
            "requestId": request_id,
            "deviceId": device_id,
            "created": true,
            "isRepair": is_repair,
            "expiresAtMs": now + PENDING_PAIRING_TTL_MS
        }))
    }

    pub fn handle_pair_status(&self, args: &Value) -> Result<Value, GatewayCommandError> {
        let request_id = args.get("requestId").and_then(Value::as_str).map(str::trim);
        let device_id = args.get("deviceId").and_then(Value::as_str).map(str::trim);

        let conn = self.conn.lock().unwrap();
        purge_expired_pending_requests(&conn, now_ms()).map_err(|error| {
            GatewayCommandError::Internal(format!("Failed to expire pending pairings: {error}"))
        })?;

        if let Some(req_id) = request_id {
            let pending_exists: bool = conn
                .query_row(
                    "SELECT COUNT(1) FROM device_pairing_pending WHERE request_id = ?1",
                    params![req_id],
                    |row| row.get(0),
                )
                .map(|c: i64| c > 0)
                .unwrap_or(false);

            if pending_exists {
                return Ok(json!({
                    "status": "pending",
                    "requestId": req_id
                }));
            }
        }

        if let Some(dev_id) = device_id {
            let paired_row: Option<(String, Option<String>, Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT device_id, display_name, platform, device_family FROM device_pairing_paired WHERE device_id = ?1",
                    params![dev_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .ok();

            if let Some((d_id, name, platform, device_family)) = paired_row {
                return Ok(json!({
                    "status": "approved",
                    "device": {
                        "deviceId": d_id,
                        "displayName": name.unwrap_or_else(|| d_id.clone()),
                        "platform": platform,
                        "deviceFamily": device_family
                    }
                }));
            }
        }

        Ok(json!({ "status": "rejected" }))
    }

    pub fn handle_pair_approve(&self, args: &Value) -> Result<Value, GatewayCommandError> {
        let request_id = args
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                GatewayCommandError::BadRequest("device.pair.approve requires requestId".into())
            })?;

        let mut conn = self.conn.lock().unwrap();
        purge_expired_pending_requests(&conn, now_ms()).map_err(|error| {
            GatewayCommandError::Internal(format!("Failed to expire pending pairings: {error}"))
        })?;

        // 1. Fetch pending record
        let pending = conn
            .query_row(
                r#"SELECT device_id, public_key, display_name, platform, device_family,
                      client_id, client_mode, browser_origin, role, roles_json, scopes_json, ts
               FROM device_pairing_pending WHERE request_id = ?1"#,
                params![request_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, i64>(11)?,
                    ))
                },
            )
            .map_err(|_| {
                GatewayCommandError::BadRequest(format!("unknown requestId: {request_id}"))
            })?;

        let (
            device_id,
            public_key,
            display_name,
            platform,
            device_family,
            client_id,
            client_mode,
            browser_origin,
            role,
            roles_json,
            scopes_json,
            created_at,
        ) = pending;

        let now = now_ms();
        let requested_roles = roles_json
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()
            .map_err(|error| {
                GatewayCommandError::BadRequest(format!("invalid requested device roles: {error}"))
            })?
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        let mut token_entries = serde_json::Map::new();
        for role in requested_roles
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|role| !role.is_empty())
        {
            if token_entries.contains_key(role) {
                continue;
            }
            token_entries.insert(
                role.to_string(),
                json!({
                    "token": new_device_token()?,
                    "role": role,
                    "scopes": scopes_for_role(scopes_json.as_deref(), role)?,
                    "createdAtMs": now,
                    "rotatedAtMs": now
                }),
            );
        }
        if token_entries.is_empty() {
            return Err(GatewayCommandError::BadRequest(
                "device pairing approval requires at least one non-empty role".into(),
            ));
        }
        let tokens_json = Value::Object(token_entries).to_string();

        // Atomic transaction: delete from pending, upsert into paired, bump revision.
        let tx = conn.transaction().map_err(|error| {
            GatewayCommandError::Internal(format!("Failed to begin pairing approval: {error}"))
        })?;
        tx.execute(
            "DELETE FROM device_pairing_pending WHERE request_id = ?1",
            params![request_id],
        )
        .map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to delete pending request: {e}"))
        })?;

        tx.execute(
            r#"INSERT INTO device_pairing_paired (
                device_id, public_key, display_name, platform, device_family,
                client_id, client_mode, browser_origin, role, roles_json, scopes_json, approved_scopes_json,
                tokens_json, approved_via, created_at_ms, approved_at_ms, last_seen_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, 'owner', ?13, ?14, ?14)
            ON CONFLICT(device_id) DO UPDATE SET
                public_key = ?2, display_name = ?3, platform = ?4, device_family = ?5,
                client_id = ?6, client_mode = ?7, browser_origin = ?8, role = ?9,
                roles_json = ?10, scopes_json = ?11, approved_scopes_json = ?11,
                tokens_json = ?12, node_surface_json = NULL, pending_node_surface_json = NULL,
                approved_at_ms = ?14, last_seen_at_ms = ?14"#,
            params![
                device_id, public_key, display_name, platform, device_family,
                client_id, client_mode, browser_origin, role, roles_json, scopes_json,
                tokens_json, created_at, now
            ],
        ).map_err(|e| GatewayCommandError::Internal(format!("Failed to persist paired device: {e}")))?;

        let new_rev = bump_revision(&tx).map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to bump pairing revision: {e}"))
        })?;
        tx.commit().map_err(|error| {
            GatewayCommandError::Internal(format!("Failed to commit pairing approval: {error}"))
        })?;

        Ok(json!({
            "requestId": request_id,
            "deviceId": device_id,
            "decision": "approved",
            "revision": new_rev,
            "device": {
                "deviceId": device_id,
                "displayName": display_name,
                "platform": platform,
                "approvedAtMs": now
            }
        }))
    }

    pub fn handle_pair_reject(&self, args: &Value) -> Result<Value, GatewayCommandError> {
        let request_id = args
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                GatewayCommandError::BadRequest("device.pair.reject requires requestId".into())
            })?;

        let conn = self.conn.lock().unwrap();
        purge_expired_pending_requests(&conn, now_ms()).map_err(|error| {
            GatewayCommandError::Internal(format!("Failed to expire pending pairings: {error}"))
        })?;

        let device_id: String = conn
            .query_row(
                "SELECT device_id FROM device_pairing_pending WHERE request_id = ?1",
                params![request_id],
                |row| row.get(0),
            )
            .map_err(|_| {
                GatewayCommandError::BadRequest(format!("unknown requestId: {request_id}"))
            })?;

        conn.execute(
            "DELETE FROM device_pairing_pending WHERE request_id = ?1",
            params![request_id],
        )
        .map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to delete pending request: {e}"))
        })?;

        Ok(json!({
            "requestId": request_id,
            "deviceId": device_id,
            "decision": "rejected"
        }))
    }

    pub fn handle_pair_list(&self, _args: &Value) -> Result<Value, GatewayCommandError> {
        let conn = self.conn.lock().unwrap();
        purge_expired_pending_requests(&conn, now_ms()).map_err(|error| {
            GatewayCommandError::Internal(format!("Failed to expire pending pairings: {error}"))
        })?;

        let mut pending_stmt = conn.prepare(
            "SELECT request_id, device_id, display_name, platform, role, ts FROM device_pairing_pending"
        ).map_err(|e| GatewayCommandError::Internal(e.to_string()))?;

        let pending_rows = pending_stmt
            .query_map([], |row| {
                Ok(json!({
                    "requestId": row.get::<_, String>(0)?,
                    "deviceId": row.get::<_, String>(1)?,
                    "displayName": row.get::<_, Option<String>>(2)?,
                    "platform": row.get::<_, Option<String>>(3)?,
                    "role": row.get::<_, Option<String>>(4)?,
                    "ts": row.get::<_, i64>(5)?
                }))
            })
            .map_err(|e| GatewayCommandError::Internal(e.to_string()))?;

        let pending: Vec<Value> = pending_rows.filter_map(Result::ok).collect();

        let mut paired_stmt = conn.prepare(
            "SELECT device_id, display_name, platform, role, approved_at_ms, last_seen_at_ms FROM device_pairing_paired"
        ).map_err(|e| GatewayCommandError::Internal(e.to_string()))?;

        let paired_rows = paired_stmt
            .query_map([], |row| {
                Ok(json!({
                    "deviceId": row.get::<_, String>(0)?,
                    "displayName": row.get::<_, Option<String>>(1)?,
                    "platform": row.get::<_, Option<String>>(2)?,
                    "role": row.get::<_, Option<String>>(3)?,
                    "approvedAtMs": row.get::<_, i64>(4)?,
                    "lastSeenAtMs": row.get::<_, Option<i64>>(5)?
                }))
            })
            .map_err(|e| GatewayCommandError::Internal(e.to_string()))?;

        let paired: Vec<Value> = paired_rows.filter_map(Result::ok).collect();

        Ok(json!({
            "pending": pending,
            "paired": paired
        }))
    }

    pub fn handle_pair_revoke(&self, args: &Value) -> Result<Value, GatewayCommandError> {
        let device_id = args
            .get("deviceId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                GatewayCommandError::BadRequest("device.pair.revoke requires deviceId".into())
            })?;

        let role = args.get("role").and_then(Value::as_str);

        let mut conn = self.conn.lock().unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(1) FROM device_pairing_paired WHERE device_id = ?1",
                params![device_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if count == 0 {
            return Err(GatewayCommandError::BadRequest(format!(
                "unknown deviceId: {device_id}"
            )));
        }

        let tx = conn.transaction().map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to begin pairing revoke transaction: {e}"))
        })?;

        tx.execute(
            "DELETE FROM device_pairing_paired WHERE device_id = ?1",
            params![device_id],
        )
        .map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to delete paired device: {e}"))
        })?;

        let new_rev = bump_revision(&tx).map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to bump pairing revision: {e}"))
        })?;
        tx.commit().map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to commit pairing revoke transaction: {e}"))
        })?;

        Ok(json!({
            "deviceId": device_id,
            "role": role.unwrap_or("all"),
            "revokedAtMs": now_ms(),
            "revision": new_rev
        }))
    }

    pub fn handle_pairing_command(
        &self,
        method: &str,
        args: &Value,
    ) -> Result<Value, GatewayCommandError> {
        match method {
            "device.pair.request" => self.handle_pair_request(args),
            "device.pair.status" => self.handle_pair_status(args),
            "device.pair.approve" => self.handle_pair_approve(args),
            "device.pair.reject" => self.handle_pair_reject(args),
            "device.pair.list" => self.handle_pair_list(args),
            "device.pair.revoke" => self.handle_pair_revoke(args),
            "device.pair.rotateToken" => self.handle_pair_rotate_token(args),
            "device.pair.setupCode" => self.handle_pair_setup_code(args),
            _ => Err(GatewayCommandError::UnknownMethod(format!(
                "Unknown device pairing method: {method}"
            ))),
        }
    }
}
