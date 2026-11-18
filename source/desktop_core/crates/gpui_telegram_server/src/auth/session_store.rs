use crate::bus::{CrossNodeEventBus, EventPayload, ServerEvent};
use crate::db::SqlitePool;
use gpui_telegram_core::error::{Result, TelegramError};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;

const LAST_ACTIVE_DEBOUNCE_MS: i64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub user_id: u64,
    pub device_id: String,
    pub client_type: String,
    pub app_version: String,
    pub ip_address: Option<String>,
    pub location_desc: Option<String>,
    pub account_generation: u64,
    pub created_at_ms: i64,
    pub last_active_ms: i64,
    pub expires_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub enum TokenVerifyOutcome {
    Valid(SessionRecord),
    Rejected,
    NotFound,
}

#[derive(Clone)]
pub struct PersistentSessionStore {
    pool: SqlitePool,
    event_bus: Arc<RwLock<Option<Arc<dyn CrossNodeEventBus>>>>,
}

impl PersistentSessionStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            event_bus: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_event_bus(&self, bus: Arc<dyn CrossNodeEventBus>) {
        *self.event_bus.write() = Some(bus);
    }

    pub fn hash_token(secret: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(secret.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn compute_expires_at_ms(now_ms: i64, ttl_seconds: u64) -> i64 {
        let ttl_ms = i64::try_from(ttl_seconds)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000);
        now_ms.saturating_add(ttl_ms)
    }

    pub fn issue_session(
        &self,
        user_id: u64,
        device_id: &str,
        client_type: &str,
        app_version: &str,
        account_generation: u64,
        ttl_seconds: u64,
    ) -> Result<(SessionRecord, String)> {
        let session_id = format!("sess_{}", uuid::Uuid::new_v4().simple());
        let raw_secret = format!(
            "{}_{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let raw_token = format!("{session_id}.{raw_secret}");
        let token_hash = Self::hash_token(&raw_secret);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let expires_at_ms = Self::compute_expires_at_ms(now_ms, ttl_seconds);

        let record = SessionRecord {
            session_id: session_id.clone(),
            user_id,
            device_id: device_id.to_string(),
            client_type: client_type.to_string(),
            app_version: app_version.to_string(),
            ip_address: None,
            location_desc: None,
            account_generation: account_generation.max(1),
            created_at_ms: now_ms,
            last_active_ms: now_ms,
            expires_at_ms,
            revoked_at_ms: None,
        };

        self.insert_session_record(&record, &token_hash)?;
        Ok((record, raw_token))
    }

    pub fn create_session(
        &self,
        user_id: u64,
        token: &str,
        device_id: &str,
        client_type: &str,
        app_version: &str,
        ttl_seconds: u64,
    ) -> Result<SessionRecord> {
        let (session_id, token_hash) = match token.trim().split_once('.') {
            Some((sid, secret)) if !sid.is_empty() && !secret.is_empty() && !secret.contains('.') => {
                (sid.to_string(), Self::hash_token(secret))
            }
            _ => (
                format!("sess_{}", uuid::Uuid::new_v4().simple()),
                Self::hash_token(token),
            ),
        };
        let now_ms = chrono::Utc::now().timestamp_millis();
        let expires_at_ms = Self::compute_expires_at_ms(now_ms, ttl_seconds);

        let record = SessionRecord {
            session_id,
            user_id,
            device_id: device_id.to_string(),
            client_type: client_type.to_string(),
            app_version: app_version.to_string(),
            ip_address: None,
            location_desc: None,
            account_generation: 1,
            created_at_ms: now_ms,
            last_active_ms: now_ms,
            expires_at_ms,
            revoked_at_ms: None,
        };

        self.insert_session_record(&record, &token_hash)?;
        Ok(record)
    }

    fn insert_session_record(&self, record: &SessionRecord, token_hash: &str) -> Result<()> {
        self.pool.with_writer(|conn| {
            conn.execute(
                r#"
                INSERT INTO sessions (
                    session_id, user_id, device_id, client_type, app_version,
                    ip_address, location_desc, account_generation, token_hash,
                    created_at_ms, last_active_ms, expires_at_ms, revoked_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                "#,
                rusqlite::params![
                    record.session_id,
                    record.user_id as i64,
                    record.device_id,
                    record.client_type,
                    record.app_version,
                    record.ip_address,
                    record.location_desc,
                    record.account_generation as i64,
                    token_hash,
                    record.created_at_ms,
                    record.last_active_ms,
                    record.expires_at_ms,
                    record.revoked_at_ms,
                ],
            )
            .map_err(|e| TelegramError::Database(format!("Failed to insert session: {e}")))?;
            Ok(())
        })
    }

    /// Verifies a `"{session_id}.{raw_secret}"` token by primary-key lookup and constant-time hash check.
    pub fn verify_token_with_status(&self, token: &str) -> Result<TokenVerifyOutcome> {
        let trimmed = token.trim();
        let Some((session_id, raw_secret)) = trimmed.split_once('.') else {
            return Ok(TokenVerifyOutcome::NotFound);
        };
        if session_id.is_empty() || raw_secret.is_empty() || raw_secret.contains('.') {
            return Ok(TokenVerifyOutcome::NotFound);
        }

        let input_hash = Self::hash_token(raw_secret);
        let now_ms = chrono::Utc::now().timestamp_millis();

        let row_opt = self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT
                        s.session_id, s.user_id, s.device_id, s.client_type, s.app_version,
                        s.ip_address, s.location_desc, s.account_generation, s.token_hash,
                        s.created_at_ms, s.last_active_ms, s.expires_at_ms, s.revoked_at_ms,
                        u.account_generation
                    FROM sessions s
                    LEFT JOIN users u ON u.user_id = s.user_id
                    WHERE s.session_id = ?1
                    LIMIT 1
                    "#,
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;

            let mut rows = stmt
                .query(rusqlite::params![session_id])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            if let Some(row) = rows
                .next()
                .map_err(|e| TelegramError::Database(e.to_string()))?
            {
                let session_id: String = row.get(0).map_err(|e| TelegramError::Database(e.to_string()))?;
                let user_id: i64 = row.get(1).map_err(|e| TelegramError::Database(e.to_string()))?;
                let device_id: String = row.get(2).map_err(|e| TelegramError::Database(e.to_string()))?;
                let client_type: String = row.get(3).map_err(|e| TelegramError::Database(e.to_string()))?;
                let app_version: String = row.get(4).map_err(|e| TelegramError::Database(e.to_string()))?;
                let ip_address: Option<String> = row.get(5).map_err(|e| TelegramError::Database(e.to_string()))?;
                let location_desc: Option<String> = row.get(6).map_err(|e| TelegramError::Database(e.to_string()))?;
                let account_generation: i64 = row.get(7).map_err(|e| TelegramError::Database(e.to_string()))?;
                let stored_hash: String = row.get(8).map_err(|e| TelegramError::Database(e.to_string()))?;
                let created_at_ms: i64 = row.get(9).map_err(|e| TelegramError::Database(e.to_string()))?;
                let last_active_ms: i64 = row.get(10).map_err(|e| TelegramError::Database(e.to_string()))?;
                let expires_at_ms: i64 = row.get(11).map_err(|e| TelegramError::Database(e.to_string()))?;
                let revoked_at_ms: Option<i64> = row.get(12).map_err(|e| TelegramError::Database(e.to_string()))?;
                let user_gen: Option<i64> = row.get(13).map_err(|e| TelegramError::Database(e.to_string()))?;

                Ok(Some((
                    SessionRecord {
                        session_id,
                        user_id: user_id as u64,
                        device_id,
                        client_type,
                        app_version,
                        ip_address,
                        location_desc,
                        account_generation: account_generation.max(1) as u64,
                        created_at_ms,
                        last_active_ms,
                        expires_at_ms,
                        revoked_at_ms,
                    },
                    stored_hash,
                    user_gen.map(|g| g.max(1) as u64),
                )))
            } else {
                Ok(None)
            }
        })?;

        let (record, stored_hash, user_generation) = match row_opt {
            Some(triple) => triple,
            None => return Ok(TokenVerifyOutcome::NotFound),
        };

        // Constant-time token hash equality check
        let hash_valid = input_hash.len() == stored_hash.len()
            && bool::from(input_hash.as_bytes().ct_eq(stored_hash.as_bytes()));
        let fenced = user_generation.is_some_and(|ug| record.account_generation < ug);

        if !hash_valid
            || record.revoked_at_ms.is_some()
            || now_ms >= record.expires_at_ms
            || fenced
        {
            return Ok(TokenVerifyOutcome::Rejected);
        }

        // Debounced asynchronous last_active_ms update (never blocks the hot read path on writer lock)
        if now_ms.saturating_sub(record.last_active_ms) >= LAST_ACTIVE_DEBOUNCE_MS {
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                let pool = self.pool.clone();
                let sid = record.session_id.clone();
                rt.spawn_blocking(move || {
                    let _ = pool.with_writer(|conn| {
                        let _ = conn.execute(
                            "UPDATE sessions SET last_active_ms = ?1 WHERE session_id = ?2",
                            rusqlite::params![now_ms, sid],
                        );
                        Ok(())
                    });
                });
            }
        }

        Ok(TokenVerifyOutcome::Valid(record))
    }

    /// Verifies the token using constant-time hash comparison and checks expiration/revocation.
    pub fn verify_token(&self, token: &str) -> Result<Option<SessionRecord>> {
        match self.verify_token_with_status(token)? {
            TokenVerifyOutcome::Valid(rec) => Ok(Some(rec)),
            TokenVerifyOutcome::Rejected | TokenVerifyOutcome::NotFound => Ok(None),
        }
    }

    pub fn revoke_session(&self, session_id: &str, reason: &str) -> Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut affected_user_id = None;

        let rows_affected = self.pool.with_writer(|conn| {
            if let Ok(uid) = conn.query_row(
                "SELECT user_id FROM sessions WHERE session_id = ?1",
                rusqlite::params![session_id],
                |row| row.get::<_, i64>(0),
            ) {
                affected_user_id = Some(uid as u64);
            }

            conn.execute(
                "UPDATE sessions SET revoked_at_ms = ?1 WHERE session_id = ?2 AND revoked_at_ms IS NULL",
                rusqlite::params![now_ms, session_id],
            )
            .map_err(|e| TelegramError::Database(format!("Failed to revoke session: {e}")))
        })?;

        if rows_affected > 0 {
            if let Some(user_id) = affected_user_id {
                self.emit_session_revoked_background(user_id, session_id, reason);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn revoke_user_session(
        &self,
        user_id: u64,
        session_id: &str,
        reason: &str,
    ) -> Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let rows_affected = self.pool.with_writer(|conn| {
            conn.execute(
                "UPDATE sessions SET revoked_at_ms = ?1 WHERE session_id = ?2 AND user_id = ?3 AND revoked_at_ms IS NULL",
                rusqlite::params![now_ms, session_id, user_id as i64],
            )
            .map_err(|e| TelegramError::Database(format!("Failed to revoke user session: {e}")))
        })?;

        if rows_affected > 0 {
            self.emit_session_revoked_background(user_id, session_id, reason);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn revoke_user_session_async(
        &self,
        user_id: u64,
        session_id: &str,
        reason: &str,
    ) -> Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let rows_affected = self.pool.with_writer(|conn| {
            conn.execute(
                "UPDATE sessions SET revoked_at_ms = ?1 WHERE session_id = ?2 AND user_id = ?3 AND revoked_at_ms IS NULL",
                rusqlite::params![now_ms, session_id, user_id as i64],
            )
            .map_err(|e| TelegramError::Database(format!("Failed to revoke user session: {e}")))
        })?;

        if rows_affected > 0 {
            self.emit_session_revoked_async(user_id, session_id, reason)
                .await;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn revoke_other_sessions_async(
        &self,
        user_id: u64,
        keep_session_id: &str,
        reason: &str,
    ) -> Result<Vec<String>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let revoked_ids = self.pool.with_transaction(|tx| {
            let mut stmt = tx
                .prepare(
                    "SELECT session_id FROM sessions WHERE user_id = ?1 AND session_id != ?2 AND revoked_at_ms IS NULL",
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(rusqlite::params![user_id as i64, keep_session_id], |r| {
                    r.get::<_, String>(0)
                })
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(|e| TelegramError::Database(e.to_string()))?);
            }
            drop(stmt);

            tx.execute(
                "UPDATE sessions SET revoked_at_ms = ?1 WHERE user_id = ?2 AND session_id != ?3 AND revoked_at_ms IS NULL",
                rusqlite::params![now_ms, user_id as i64, keep_session_id],
            )
            .map_err(|e| TelegramError::Database(format!("Failed to revoke other sessions: {e}")))?;

            Ok(ids)
        })?;

        for sid in &revoked_ids {
            self.emit_session_revoked_async(user_id, sid, reason).await;
        }
        Ok(revoked_ids)
    }

    fn emit_session_revoked_background(&self, user_id: u64, session_id: &str, reason: &str) {
        if let Some(bus) = self.event_bus.read().clone() {
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                let channel = format!("user:{user_id}");
                let event = ServerEvent::new(
                    &channel,
                    bus.node_id(),
                    EventPayload::SessionRevoked {
                        user_id,
                        session_id: session_id.to_string(),
                        reason: reason.to_string(),
                    },
                );
                rt.spawn(async move {
                    let _ = bus.publish(&channel, &event).await;
                });
            }
        }
    }

    async fn emit_session_revoked_async(&self, user_id: u64, session_id: &str, reason: &str) {
        let bus = self.event_bus.read().clone();
        if let Some(bus) = bus {
            let channel = format!("user:{user_id}");
            let event = ServerEvent::new(
                &channel,
                bus.node_id(),
                EventPayload::SessionRevoked {
                    user_id,
                    session_id: session_id.to_string(),
                    reason: reason.to_string(),
                },
            );
            let _ = bus.publish(&channel, &event).await;
        }
    }

    pub fn list_active_sessions_for_user(&self, user_id: u64) -> Result<Vec<SessionRecord>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT
                        session_id, user_id, device_id, client_type, app_version,
                        ip_address, location_desc, account_generation,
                        created_at_ms, last_active_ms, expires_at_ms, revoked_at_ms
                    FROM sessions
                    WHERE user_id = ?1 AND revoked_at_ms IS NULL AND expires_at_ms > ?2
                    ORDER BY last_active_ms DESC
                    "#,
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;

            let rows = stmt
                .query_map(rusqlite::params![user_id as i64, now_ms], |row| {
                    Ok(SessionRecord {
                        session_id: row.get(0)?,
                        user_id: row.get::<_, i64>(1)? as u64,
                        device_id: row.get(2)?,
                        client_type: row.get(3)?,
                        app_version: row.get(4)?,
                        ip_address: row.get(5)?,
                        location_desc: row.get(6)?,
                        account_generation: row.get::<_, i64>(7)? as u64,
                        created_at_ms: row.get(8)?,
                        last_active_ms: row.get(9)?,
                        expires_at_ms: row.get(10)?,
                        revoked_at_ms: row.get(11)?,
                    })
                })
                .map_err(|e| TelegramError::Database(e.to_string()))?;

            let mut list = Vec::new();
            for r in rows {
                list.push(r.map_err(|e| TelegramError::Database(e.to_string()))?);
            }
            Ok(list)
        })
    }
}
