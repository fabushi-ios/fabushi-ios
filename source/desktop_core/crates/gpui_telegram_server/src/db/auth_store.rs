use crate::db::{ServerDatabase, SqliteResultExt};
use crate::db::schema::UserRecord;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::contracts::{QrPairStatusResponse, SessionDto, TwoFactorStatusResponse};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneChallengeRecord {
    pub challenge_id: String,
    pub phone: String,
    pub phone_code_hash: String,
    pub code_hash: String,
    pub attempts_left: i32,
    pub cooldown_until_ms: i64,
    pub expires_at_ms: i64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyCredentialRecord {
    pub credential_id: String,
    pub user_id: UserId,
    pub public_key: String,
    pub sign_count: i64,
    pub device_name: Option<String>,
    pub created_at_ms: i64,
    pub last_used_at_ms: i64,
}

impl ServerDatabase {
    pub fn save_phone_challenge(&self, c: &PhoneChallengeRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT OR REPLACE INTO account_phone_challenges
                (challenge_id, phone, phone_code_hash, code_hash, attempts_left, cooldown_until_ms, expires_at_ms, created_at_ms)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
                params![c.challenge_id, c.phone, c.phone_code_hash, c.code_hash, c.attempts_left, c.cooldown_until_ms, c.expires_at_ms, c.created_at_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn get_latest_phone_challenge(&self, phone: &str) -> Result<Option<PhoneChallengeRecord>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                r#"SELECT challenge_id, phone, phone_code_hash, code_hash, attempts_left, cooldown_until_ms, expires_at_ms, created_at_ms
                   FROM account_phone_challenges WHERE phone = ?1 ORDER BY created_at_ms DESC LIMIT 1"#,
                params![phone],
                |r| Ok(PhoneChallengeRecord {
                    challenge_id: r.get(0)?, phone: r.get(1)?, phone_code_hash: r.get(2)?,
                    code_hash: r.get(3)?, attempts_left: r.get(4)?, cooldown_until_ms: r.get(5)?,
                    expires_at_ms: r.get(6)?, created_at_ms: r.get(7)?,
                }),
            ).optional().map_db()
        })
    }

    pub fn decrement_phone_challenge_attempts(&self, challenge_id: &str) -> Result<i32> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE account_phone_challenges SET attempts_left = MAX(attempts_left - 1, 0) WHERE challenge_id = ?1",
                params![challenge_id],
            ).map_db()?;
            conn.query_row(
                "SELECT attempts_left FROM account_phone_challenges WHERE challenge_id = ?1",
                params![challenge_id],
                |r| r.get(0),
            ).optional().map_db().map(|opt| opt.unwrap_or(0))
        })
    }

    pub fn delete_phone_challenge(&self, challenge_id: &str) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute("DELETE FROM account_phone_challenges WHERE challenge_id = ?1", params![challenge_id]).map_db()?;
            Ok(())
        })
    }

    pub fn get_user_by_phone(&self, phone: &str) -> Result<Option<UserRecord>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                r#"SELECT user_id, username, phone, first_name, last_name, bio, password_hash,
                          is_bot, is_verified, is_support, is_scam, is_fake, profile_photo_file_id,
                          account_generation, created_at_ms, updated_at_ms
                   FROM users WHERE phone = ?1 LIMIT 1"#,
                params![phone],
                |r| Ok(UserRecord {
                    user_id: UserId(r.get::<_, i64>(0)? as u64),
                    username: r.get(1)?, phone: r.get(2)?, first_name: r.get(3)?,
                    last_name: r.get(4)?, bio: r.get(5)?, password_hash: r.get(6)?,
                    is_bot: r.get::<_, i64>(7)? != 0, is_verified: r.get::<_, i64>(8)? != 0,
                    is_support: r.get::<_, i64>(9)? != 0, is_scam: r.get::<_, i64>(10)? != 0,
                    is_fake: r.get::<_, i64>(11)? != 0, profile_photo_file_id: r.get(12)?,
                    account_generation: r.get::<_, i64>(13)? as u64,
                    created_at_ms: r.get(14)?, updated_at_ms: r.get(15)?,
                }),
            ).optional().map_db()
        })
    }

    pub fn update_user_emoji_and_phone(&self, user_id: UserId, emoji_status: Option<&str>, phone: Option<&str>) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            if let Some(emoji) = emoji_status {
                conn.execute("UPDATE users SET emoji_status = ?1, updated_at_ms = ?2 WHERE user_id = ?3",
                    params![emoji, now_ms, user_id.0 as i64]).map_db()?;
            }
            if let Some(ph) = phone {
                conn.execute("UPDATE users SET phone = ?1, updated_at_ms = ?2 WHERE user_id = ?3",
                    params![ph, now_ms, user_id.0 as i64]).map_db()?;
            }
            Ok(())
        })
    }

    pub fn get_user_emoji_status(&self, user_id: UserId) -> Result<Option<String>> {
        self.pool.with_reader(|conn| {
            conn.query_row("SELECT emoji_status FROM users WHERE user_id = ?1", params![user_id.0 as i64], |r| r.get(0))
                .optional().map_db().map(|o| o.flatten())
        })
    }

    pub fn get_two_factor_status(&self, user_id: UserId) -> Result<TwoFactorStatusResponse> {
        self.pool.with_reader(|conn| {
            let row: Option<(Option<String>, Option<String>)> = conn.query_row(
                "SELECT hint, recovery_email FROM two_factor_auth WHERE user_id = ?1",
                params![user_id.0 as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional().map_db()?;
            Ok(match row {
                Some((hint, rec)) => TwoFactorStatusResponse { has_password: true, hint, recovery_email_pattern: rec },
                None => TwoFactorStatusResponse { has_password: false, hint: None, recovery_email_pattern: None },
            })
        })
    }

    pub fn get_two_factor_hash(&self, user_id: UserId) -> Result<Option<String>> {
        self.pool.with_reader(|conn| {
            conn.query_row("SELECT password_hash FROM two_factor_auth WHERE user_id = ?1", params![user_id.0 as i64], |r| r.get(0))
                .optional().map_db()
        })
    }

    pub fn set_two_factor_auth(&self, user_id: UserId, password_hash: &str, hint: Option<&str>, recovery_email: Option<&str>) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT OR REPLACE INTO two_factor_auth (user_id, password_hash, hint, recovery_email, updated_at_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5)"#,
                params![user_id.0 as i64, password_hash, hint, recovery_email, now_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn delete_two_factor_auth(&self, user_id: UserId) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute("DELETE FROM two_factor_auth WHERE user_id = ?1", params![user_id.0 as i64]).map_db()?;
            Ok(())
        })
    }

    pub fn create_two_factor_challenge(&self, token: &str, user_id: UserId, expires_at_ms: i64) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT INTO two_factor_challenges (challenge_token, user_id, attempts_left, expires_at_ms, created_at_ms) VALUES (?1, ?2, 5, ?3, ?4)",
                params![token, user_id.0 as i64, expires_at_ms, now_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn verify_two_factor_challenge(&self, token: &str) -> Result<Option<(UserId, i32, i64)>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT user_id, attempts_left, expires_at_ms FROM two_factor_challenges WHERE challenge_token = ?1",
                params![token],
                |r| Ok((UserId(r.get::<_, i64>(0)? as u64), r.get(1)?, r.get(2)?)),
            ).optional().map_db()
        })
    }

    pub fn decrement_or_delete_two_factor_challenge(&self, token: &str, delete: bool) -> Result<()> {
        self.pool.with_conn(|conn| {
            if delete {
                conn.execute("DELETE FROM two_factor_challenges WHERE challenge_token = ?1", params![token]).map_db()?;
            } else {
                conn.execute("UPDATE two_factor_challenges SET attempts_left = MAX(attempts_left - 1, 0) WHERE challenge_token = ?1", params![token]).map_db()?;
            }
            Ok(())
        })
    }

    pub fn save_passkey_challenge(&self, challenge_id: &str, user_id: Option<UserId>, kind: &str, expires_at_ms: i64) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO passkey_challenges (challenge_id, user_id, kind, challenge_b64, expires_at_ms, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![challenge_id, user_id.map(|u| u.0 as i64), kind, challenge_id, expires_at_ms, now_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn consume_passkey_challenge(&self, challenge_id: &str) -> Result<Option<(Option<UserId>, String, i64)>> {
        self.pool.with_transaction(|tx| {
            let row: Option<(Option<i64>, String, i64)> = tx.query_row(
                "SELECT user_id, kind, expires_at_ms FROM passkey_challenges WHERE challenge_id = ?1",
                params![challenge_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).optional().map_err(|e| TelegramError::Database(e.to_string()))?;
            if row.is_some() {
                tx.execute("DELETE FROM passkey_challenges WHERE challenge_id = ?1", params![challenge_id])
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            Ok(row.map(|(uid, k, exp)| (uid.map(|v| UserId(v as u64)), k, exp)))
        })
    }

    pub fn save_passkey_credential(&self, cred: &PasskeyCredentialRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT OR REPLACE INTO passkey_credentials
                (credential_id, user_id, public_key, sign_count, device_name, created_at_ms, last_used_at_ms)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
                params![cred.credential_id, cred.user_id.0 as i64, cred.public_key, cred.sign_count, cred.device_name, cred.created_at_ms, cred.last_used_at_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn get_passkey_credential(&self, credential_id: &str) -> Result<Option<PasskeyCredentialRecord>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT credential_id, user_id, public_key, sign_count, device_name, created_at_ms, last_used_at_ms FROM passkey_credentials WHERE credential_id = ?1",
                params![credential_id],
                |r| Ok(PasskeyCredentialRecord {
                    credential_id: r.get(0)?, user_id: UserId(r.get::<_, i64>(1)? as u64),
                    public_key: r.get(2)?, sign_count: r.get(3)?, device_name: r.get(4)?,
                    created_at_ms: r.get(5)?, last_used_at_ms: r.get(6)?,
                }),
            ).optional().map_db()
        })
    }

    pub fn list_user_passkeys(&self, user_id: UserId) -> Result<Vec<PasskeyCredentialRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT credential_id, user_id, public_key, sign_count, device_name, created_at_ms, last_used_at_ms FROM passkey_credentials WHERE user_id = ?1 ORDER BY created_at_ms DESC"
            ).map_db()?;
            let rows = stmt.query_map(params![user_id.0 as i64], |r| Ok(PasskeyCredentialRecord {
                credential_id: r.get(0)?, user_id: UserId(r.get::<_, i64>(1)? as u64),
                public_key: r.get(2)?, sign_count: r.get(3)?, device_name: r.get(4)?,
                created_at_ms: r.get(5)?, last_used_at_ms: r.get(6)?,
            })).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn create_qr_session(&self, pair_token: &str, expires_at_ms: i64) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT INTO qr_login_sessions (pair_token, status, user_id, session_json, expires_at_ms, created_at_ms) VALUES (?1, 'pending', NULL, NULL, ?2, ?3)",
                params![pair_token, expires_at_ms, now_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn accept_qr_session(&self, pair_token: &str, user_id: UserId, session_json: &str) -> Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            let changed = conn.execute(
                "UPDATE qr_login_sessions SET status = 'confirmed', user_id = ?1, session_json = ?2 WHERE pair_token = ?3 AND expires_at_ms > ?4 AND status = 'pending'",
                params![user_id.0 as i64, session_json, pair_token, now_ms],
            ).map_db()?;
            Ok(changed > 0)
        })
    }

    pub fn get_qr_session(&self, pair_token: &str) -> Result<Option<QrPairStatusResponse>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_reader(|conn| {
            let row: Option<(String, Option<String>, i64)> = conn.query_row(
                "SELECT status, session_json, expires_at_ms FROM qr_login_sessions WHERE pair_token = ?1",
                params![pair_token],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).optional().map_db()?;
            Ok(row.map(|(status, sess_json, exp)| {
                let effective = if status == "pending" && exp <= now_ms { "expired".to_string() } else { status };
                let auth: Option<gpui_telegram_proto::contracts::AuthResponse> = sess_json.and_then(|s| serde_json::from_str(&s).ok());
                let token = auth.as_ref().map(|a| a.token.clone());
                QrPairStatusResponse { status: effective, pair_token: Some(pair_token.to_string()), session: auth, token, user: None }
            }))
        })
    }

    pub fn list_user_sessions(&self, user_id: UserId, current_session_id: &str) -> Result<Vec<SessionDto>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT session_id, device_id, client_type, ip_address, created_at_ms, last_active_ms FROM sessions WHERE user_id = ?1 AND revoked_at_ms IS NULL AND expires_at_ms > ?2 ORDER BY last_active_ms DESC"
            ).map_db()?;
            let rows = stmt.query_map(params![user_id.0 as i64, now_ms], |r| {
                let sid: String = r.get(0)?;
                Ok(SessionDto {
                    is_current: sid == current_session_id,
                    session_id: sid,
                    device_name: r.get(1)?,
                    platform: r.get(2)?,
                    ip_address: r.get(3)?,
                    created_at_ms: r.get(4)?,
                    last_active_ms: r.get(5)?,
                })
            }).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn revoke_user_session(&self, user_id: UserId, session_id: &str) -> Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            let n = conn.execute(
                "UPDATE sessions SET revoked_at_ms = ?1 WHERE session_id = ?2 AND user_id = ?3 AND revoked_at_ms IS NULL",
                params![now_ms, session_id, user_id.0 as i64],
            ).map_db()?;
            Ok(n > 0)
        })
    }

    pub fn revoke_other_sessions(&self, user_id: UserId, keep_session_id: &str) -> Result<u64> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            let n = conn.execute(
                "UPDATE sessions SET revoked_at_ms = ?1 WHERE user_id = ?2 AND session_id != ?3 AND revoked_at_ms IS NULL",
                params![now_ms, user_id.0 as i64, keep_session_id],
            ).map_db()?;
            Ok(n as u64)
        })
    }
}
