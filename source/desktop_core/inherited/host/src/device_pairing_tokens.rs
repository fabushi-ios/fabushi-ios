use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::{
    bump_revision, new_device_token, now_ms, scopes_for_role, DevicePairingOwner,
    DeviceTokenAuthorization, PENDING_PAIRING_TTL_MS,
};
use super::device_pairing_protocol::{is_device_pairing_join_code, validate_mobile_pairing_url, DEVICE_PAIRING_JOIN_CODE_BYTES};
use crate::gateway_command_error::GatewayCommandError;

impl DevicePairingOwner {
    pub fn authenticate_device_token(
        &self,
        token: &str,
    ) -> Result<Option<DeviceTokenAuthorization>, GatewayCommandError> {
        if token.is_empty() || token.len() > 256 {
            return Ok(None);
        }

        let conn = self.conn.lock().unwrap();
        let mut statement = conn
            .prepare("SELECT device_id, tokens_json FROM device_pairing_paired")
            .map_err(|error| {
                GatewayCommandError::Internal(format!("failed to read device tokens: {error}"))
            })?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| {
                GatewayCommandError::Internal(format!("failed to enumerate device tokens: {error}"))
            })?;

        for row in rows {
            let (device_id, tokens_json) = row.map_err(|error| {
                GatewayCommandError::Internal(format!("failed to read device token row: {error}"))
            })?;
            let entries = match serde_json::from_str::<Value>(&tokens_json) {
                Ok(Value::Object(entries)) => entries,
                Ok(_) | Err(_) => continue,
            };
            for entry in entries.values() {
                let Some(candidate) = entry.get("token").and_then(Value::as_str) else {
                    continue;
                };
                if candidate.len() != token.len()
                    || !bool::from(candidate.as_bytes().ct_eq(token.as_bytes()))
                {
                    continue;
                }
                let Some(role) = entry.get("role").and_then(Value::as_str) else {
                    continue;
                };
                let scopes = entry
                    .get("scopes")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                return Ok(Some(DeviceTokenAuthorization {
                    device_id,
                    role: role.to_string(),
                    scopes,
                }));
            }
        }
        Ok(None)
    }

    pub fn handle_pair_rotate_token(&self, args: &Value) -> Result<Value, GatewayCommandError> {
        let device_id = args
            .get("deviceId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                GatewayCommandError::BadRequest("device.pair.rotateToken requires deviceId".into())
            })?;

        let role = args
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("operator");
        let mut conn = self.conn.lock().unwrap();

        let (roles_json, scopes_json, existing_tokens_json): (
            Option<String>,
            Option<String>,
            String,
        ) = conn
            .query_row(
                "SELECT roles_json, scopes_json, tokens_json
                 FROM device_pairing_paired WHERE device_id = ?1",
                params![device_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| {
                GatewayCommandError::BadRequest(format!("unknown deviceId: {device_id}"))
            })?;

        let approved_roles = roles_json
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()
            .map_err(|error| {
                GatewayCommandError::Internal(format!("invalid approved device roles: {error}"))
            })?
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        if !approved_roles
            .iter()
            .filter_map(Value::as_str)
            .any(|approved| approved == role)
        {
            return Err(GatewayCommandError::BadRequest(format!(
                "role {role} is not approved for deviceId: {device_id}"
            )));
        }
        let role_scopes = scopes_for_role(scopes_json.as_deref(), role)?;

        let now = now_ms();
        let new_token = new_device_token()?;
        let mut token_entries = serde_json::from_str::<Value>(&existing_tokens_json)
            .map_err(|error| {
                GatewayCommandError::Internal(format!("invalid stored device tokens: {error}"))
            })?
            .as_object()
            .cloned()
            .ok_or_else(|| {
                GatewayCommandError::Internal("stored device tokens are not an object".into())
            })?;
        token_entries.insert(
            role.to_string(),
            json!({
                "token": new_token,
                "role": role,
                "scopes": role_scopes,
                "createdAtMs": now,
                "rotatedAtMs": now
            }),
        );
        let tokens_json = Value::Object(token_entries).to_string();

        let tx = conn.transaction().map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to begin token rotation transaction: {e}"))
        })?;

        tx.execute(
            "UPDATE device_pairing_paired
             SET tokens_json = ?1, node_surface_json = NULL, pending_node_surface_json = NULL
             WHERE device_id = ?2",
            params![tokens_json, device_id],
        )
        .map_err(|e| GatewayCommandError::Internal(format!("Failed to update tokens: {e}")))?;

        let new_rev = bump_revision(&tx).map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to bump pairing revision: {e}"))
        })?;
        tx.commit().map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to commit token rotation transaction: {e}"))
        })?;

        let is_cross_device = args
            .get("crossDevice")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if is_cross_device {
            Ok(json!({
                "deviceId": device_id,
                "role": role,
                "rotatedAtMs": now,
                "tokenDelivery": "withheld-cross-device",
                "revision": new_rev
            }))
        } else {
            Ok(json!({
                "deviceId": device_id,
                "role": role,
                "token": new_token,
                "rotatedAtMs": now,
                "tokenDelivery": "in-band",
                "revision": new_rev
            }))
        }
    }

    pub fn handle_pair_setup_code(&self, args: &Value) -> Result<Value, GatewayCommandError> {
        let gateway_url = args
            .get("gatewayUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(url) = gateway_url {
            validate_mobile_pairing_url(url)
                .map_err(GatewayCommandError::BadRequest)?;
        }
        let setup_id = format!("setup-{}", Uuid::new_v4());
        let setup_code = format!("sand-setup-{}", Uuid::new_v4());
        let now = now_ms();
        let expires_at = now + PENDING_PAIRING_TTL_MS;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM device_bootstrap_tokens WHERE expires_at_ms <= ?1",
            params![now],
        )
        .map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to purge expired bootstrap tokens: {e}"))
        })?;
        conn.execute(
            r#"INSERT INTO device_bootstrap_tokens
               (token_key, token, setup_id, ts, issued_at_ms, expires_at_ms)
               VALUES (?1, ?2, ?3, ?4, ?4, ?5)"#,
            params![setup_code, setup_code, setup_id, now, expires_at],
        )
        .map_err(|e| {
            GatewayCommandError::Internal(format!("Failed to store bootstrap token: {e}"))
        })?;

        let qr_data_url = crate::extensions::attachments::media_qr_png::render_qr_png_data_url(
            &setup_code,
            &crate::extensions::attachments::media_qr_png::QrPngRenderOptions::default(),
        )
        .ok();
        let qr_terminal =
            crate::extensions::attachments::media_qr_png::render_qr_terminal(&setup_code, true)
                .ok();

        let mut res = json!({
            "setupId": setup_id,
            "setupCode": setup_code,
            "auth": "token",
            "urlSource": "local",
            "access": "full",
            "expiresAtMs": expires_at
        });
        if let Some(url) = gateway_url {
            res["gatewayUrl"] = Value::String(url.to_string());
        }
        if let Some(qr) = qr_data_url {
            res["qrDataUrl"] = Value::String(qr);
        }
        if let Some(term) = qr_terminal {
            res["qrTerminal"] = Value::String(term);
        }
        Ok(res)
    }

    pub fn consume_bootstrap_token(
        &self,
        token: &str,
    ) -> Result<Option<Value>, GatewayCommandError> {
        let token = token.trim();
        if token.is_empty() || token.len() > 256 {
            return Ok(None);
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|error| {
            GatewayCommandError::Internal(format!("failed to begin bootstrap-token consume: {error}"))
        })?;
        let now = now_ms();
        let row = tx
            .query_row(
                "SELECT setup_id, issued_at_ms, expires_at_ms
                   FROM device_bootstrap_tokens
                  WHERE token_key = ?1 AND token = ?1 AND expires_at_ms > ?2",
                params![token, now],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                GatewayCommandError::Internal(format!("failed to read bootstrap token: {error}"))
            })?;
        let Some((setup_id, issued_at_ms, expires_at_ms)) = row else {
            tx.execute(
                "DELETE FROM device_bootstrap_tokens WHERE token_key = ?1 AND expires_at_ms <= ?2",
                params![token, now],
            )
            .map_err(|error| {
                GatewayCommandError::Internal(format!("failed to reap expired bootstrap token: {error}"))
            })?;
            tx.commit().map_err(|error| {
                GatewayCommandError::Internal(format!("failed to commit bootstrap-token reap: {error}"))
            })?;
            return Ok(None);
        };
        let deleted = tx
            .execute(
                "DELETE FROM device_bootstrap_tokens WHERE token_key = ?1 AND token = ?1 AND expires_at_ms > ?2",
                params![token, now],
            )
            .map_err(|error| {
                GatewayCommandError::Internal(format!("failed to consume bootstrap token: {error}"))
            })?;
        if deleted != 1 {
            tx.commit().map_err(|error| {
                GatewayCommandError::Internal(format!("failed to commit bootstrap-token retry: {error}"))
            })?;
            return Ok(None);
        }
        tx.commit().map_err(|error| {
            GatewayCommandError::Internal(format!("failed to commit bootstrap-token consume: {error}"))
        })?;
        Ok(Some(json!({
            "setupId": setup_id,
            "issuedAtMs": issued_at_ms,
            "expiresAtMs": expires_at_ms,
        })))
    }

    /// Stores a short-lived pairing setup payload under a cryptographically random
    /// 128-bit base64url shortcode. The payload is only returned once by
    /// `redeem_device_pairing_join_code`.
    pub fn register_device_pairing_join_code(
        &self,
        payload: &Value,
        expires_at_ms: i64,
    ) -> Result<String, GatewayCommandError> {
        let now = now_ms();
        if expires_at_ms <= now {
            return Err(GatewayCommandError::BadRequest(
                "device pairing join code requires a future expiry".into(),
            ));
        }
        let payload_json = serde_json::to_string(payload).map_err(|error| {
            GatewayCommandError::BadRequest(format!("invalid device pairing payload: {error}"))
        })?;
        if payload_json.len() > 64 * 1024 {
            return Err(GatewayCommandError::BadRequest(
                "device pairing payload exceeds 64 KiB".into(),
            ));
        }

        let mut bytes = [0_u8; DEVICE_PAIRING_JOIN_CODE_BYTES];
        let conn = self.conn.lock().unwrap();
        for _ in 0..4 {
            getrandom::fill(&mut bytes).map_err(|_| {
                GatewayCommandError::Internal("failed to generate device pairing join code".into())
            })?;
            let shortcode = URL_SAFE_NO_PAD.encode(bytes);
            let inserted = conn
                .execute(
                    "INSERT OR IGNORE INTO device_pairing_join_codes
                     (shortcode, payload_json, created_at_ms, expires_at_ms)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![shortcode, payload_json, now, expires_at_ms],
                )
                .map_err(|error| GatewayCommandError::Internal(error.to_string()))?;
            if inserted == 1 {
                return Ok(shortcode);
            }
        }
        Err(GatewayCommandError::Internal(
            "failed to allocate a unique device pairing join code".into(),
        ))
    }

    /// Atomically burns a live shortcode and returns its validated JSON payload.
    pub fn redeem_device_pairing_join_code(
        &self,
        shortcode: &str,
    ) -> Result<Option<Value>, GatewayCommandError> {
        let shortcode = shortcode.trim();
        if !is_device_pairing_join_code(shortcode) {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
        let now = now_ms();
        let payload_json: Option<String> = conn
            .query_row(
                "SELECT payload_json FROM device_pairing_join_codes
                 WHERE shortcode = ?1 AND redeemed_at_ms IS NULL AND expires_at_ms > ?2",
                params![shortcode, now],
                |row| row.get(0),
            )
            .ok();
        let Some(payload_json) = payload_json else {
            return Ok(None);
        };
        let updated = conn
            .execute(
                "UPDATE device_pairing_join_codes SET redeemed_at_ms = ?1
                 WHERE shortcode = ?2 AND redeemed_at_ms IS NULL AND expires_at_ms > ?1",
                params![now, shortcode],
            )
            .map_err(|error| GatewayCommandError::Internal(error.to_string()))?;
        if updated != 1 {
            return Ok(None);
        }
        let payload = serde_json::from_str(&payload_json).map_err(|error| {
            GatewayCommandError::Internal(format!(
                "stored device pairing payload is invalid: {error}"
            ))
        })?;
        Ok(Some(payload))
    }
}
