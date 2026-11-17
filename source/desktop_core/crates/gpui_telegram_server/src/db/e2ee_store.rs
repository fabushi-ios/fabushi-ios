use crate::db::{ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::e2ee::{
    DevicePreKeyBundle, OneTimePreKeyDto, PublishPreKeysRequest, SignedPreKeyDto,
};
use rusqlite::{params, OptionalExtension};

impl ServerDatabase {
    pub fn upsert_device_prekeys(
        &self,
        user_id: UserId,
        device_id: &str,
        req: &PublishPreKeysRequest,
        now_ms: i64,
    ) -> Result<usize> {
        let uid = user_id.0 as i64;
        self.pool.with_transaction(|tx| {
            tx.execute(
                r#"INSERT INTO e2ee_devices (
                       user_id, device_id, device_name, identity_key, identity_signing_key,
                       signed_prekey_id, signed_prekey_pub, signed_prekey_sig, signed_prekey_ts_ms,
                       is_active, created_at_ms, updated_at_ms
                   ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?10)
                   ON CONFLICT(user_id, device_id) DO UPDATE SET
                       device_name = COALESCE(excluded.device_name, e2ee_devices.device_name),
                       identity_key = excluded.identity_key,
                       identity_signing_key = excluded.identity_signing_key,
                       signed_prekey_id = excluded.signed_prekey_id,
                       signed_prekey_pub = excluded.signed_prekey_pub,
                       signed_prekey_sig = excluded.signed_prekey_sig,
                       signed_prekey_ts_ms = excluded.signed_prekey_ts_ms,
                       is_active = 1,
                       updated_at_ms = excluded.updated_at_ms"#,
                params![
                    uid,
                    device_id,
                    req.device_name.as_deref(),
                    &req.identity_key,
                    &req.identity_signing_key,
                    req.signed_prekey.key_id as i64,
                    &req.signed_prekey.public_key,
                    &req.signed_prekey.signature,
                    req.signed_prekey.created_at_ms,
                    now_ms,
                ],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;

            for otpk in &req.one_time_prekeys {
                tx.execute(
                    r#"INSERT OR IGNORE INTO e2ee_one_time_prekeys (
                           user_id, device_id, key_id, public_key, created_at_ms
                       ) VALUES (?1, ?2, ?3, ?4, ?5)"#,
                    params![uid, device_id, otpk.key_id as i64, &otpk.public_key, now_ms],
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            }

            let count: i64 = tx
                .query_row(
                    r#"SELECT COUNT(*) FROM e2ee_one_time_prekeys
                       WHERE user_id = ?1 AND device_id = ?2 AND consumed_at_ms IS NULL"#,
                    params![uid, device_id],
                    |r| r.get(0),
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok(count.max(0) as usize)
        })
    }

    pub fn consume_user_prekey_bundles(
        &self,
        target_user_id: UserId,
        optional_device_id: Option<&str>,
        now_ms: i64,
    ) -> Result<Vec<DevicePreKeyBundle>> {
        let uid = target_user_id.0 as i64;
        self.pool.with_transaction(|tx| {
            let device_rows: Vec<(String, Option<String>, String, String, u32, String, String, i64)> =
                if let Some(dev_id) = optional_device_id {
                    let mut stmt = tx
                        .prepare(
                            r#"SELECT device_id, device_name, identity_key, identity_signing_key,
                                      signed_prekey_id, signed_prekey_pub, signed_prekey_sig, signed_prekey_ts_ms
                               FROM e2ee_devices
                               WHERE user_id = ?1 AND device_id = ?2 AND is_active = 1
                               ORDER BY updated_at_ms DESC"#,
                        )
                        .map_err(|e| TelegramError::Database(e.to_string()))?;
                    let rows = stmt
                        .query_map(params![uid, dev_id], |r| {
                            Ok((
                                r.get(0)?,
                                r.get(1)?,
                                r.get(2)?,
                                r.get(3)?,
                                r.get::<_, i64>(4)? as u32,
                                r.get(5)?,
                                r.get(6)?,
                                r.get(7)?,
                            ))
                        })
                        .map_err(|e| TelegramError::Database(e.to_string()))?;
                    rows.collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|e| TelegramError::Database(e.to_string()))?
                } else {
                    let mut stmt = tx
                        .prepare(
                            r#"SELECT device_id, device_name, identity_key, identity_signing_key,
                                      signed_prekey_id, signed_prekey_pub, signed_prekey_sig, signed_prekey_ts_ms
                               FROM e2ee_devices
                               WHERE user_id = ?1 AND is_active = 1
                               ORDER BY updated_at_ms DESC"#,
                        )
                        .map_err(|e| TelegramError::Database(e.to_string()))?;
                    let rows = stmt
                        .query_map(params![uid], |r| {
                            Ok((
                                r.get(0)?,
                                r.get(1)?,
                                r.get(2)?,
                                r.get(3)?,
                                r.get::<_, i64>(4)? as u32,
                                r.get(5)?,
                                r.get(6)?,
                                r.get(7)?,
                            ))
                        })
                        .map_err(|e| TelegramError::Database(e.to_string()))?;
                    rows.collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|e| TelegramError::Database(e.to_string()))?
                };

            let mut bundles = Vec::with_capacity(device_rows.len());
            for (
                device_id,
                device_name,
                identity_key,
                identity_signing_key,
                spk_id,
                spk_pub,
                spk_sig,
                spk_ts_ms,
            ) in device_rows
            {
                let otpk_row: Option<(i64, u32, String)> = tx
                    .query_row(
                        r#"SELECT id, key_id, public_key FROM e2ee_one_time_prekeys
                           WHERE user_id = ?1 AND device_id = ?2 AND consumed_at_ms IS NULL
                           ORDER BY id ASC LIMIT 1"#,
                        params![uid, &device_id],
                        |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u32, r.get(2)?)),
                    )
                    .optional()
                    .map_err(|e| TelegramError::Database(e.to_string()))?;

                let one_time_prekey = if let Some((row_id, key_id, public_key)) = otpk_row {
                    let updated = tx
                        .execute(
                            r#"UPDATE e2ee_one_time_prekeys
                               SET consumed_at_ms = ?1
                               WHERE id = ?2 AND consumed_at_ms IS NULL"#,
                            params![now_ms, row_id],
                        )
                        .map_err(|e| TelegramError::Database(e.to_string()))?;
                    if updated == 1 {
                        Some(OneTimePreKeyDto { key_id, public_key })
                    } else {
                        None
                    }
                } else {
                    None
                };

                bundles.push(DevicePreKeyBundle {
                    user_id: target_user_id.0,
                    device_id,
                    device_name,
                    identity_key,
                    identity_signing_key,
                    signed_prekey: SignedPreKeyDto {
                        key_id: spk_id,
                        public_key: spk_pub,
                        signature: spk_sig,
                        created_at_ms: spk_ts_ms,
                    },
                    one_time_prekey,
                });
            }

            Ok(bundles)
        })
    }

    pub fn count_device_one_time_prekeys(
        &self,
        user_id: UserId,
        device_id: &str,
    ) -> Result<Option<(usize, bool)>> {
        let uid = user_id.0 as i64;
        self.pool.with_reader(|conn| {
            let has_device: Option<i64> = conn
                .query_row(
                    "SELECT is_active FROM e2ee_devices WHERE user_id = ?1 AND device_id = ?2",
                    params![uid, device_id],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?;
            let Some(is_active) = has_device else {
                return Ok(None);
            };
            let count: i64 = conn
                .query_row(
                    r#"SELECT COUNT(*) FROM e2ee_one_time_prekeys
                       WHERE user_id = ?1 AND device_id = ?2 AND consumed_at_ms IS NULL"#,
                    params![uid, device_id],
                    |r| r.get(0),
                )
                .map_db()?;
            Ok(Some((count.max(0) as usize, is_active != 0)))
        })
    }

    pub fn deactivate_e2ee_device(
        &self,
        user_id: UserId,
        device_id: &str,
        now_ms: i64,
    ) -> Result<bool> {
        let uid = user_id.0 as i64;
        self.pool.with_conn(|conn| {
            let n = conn
                .execute(
                    r#"UPDATE e2ee_devices
                       SET is_active = 0, updated_at_ms = ?1
                       WHERE user_id = ?2 AND device_id = ?3 AND is_active = 1"#,
                    params![now_ms, uid, device_id],
                )
                .map_db()?;
            Ok(n > 0)
        })
    }
}
