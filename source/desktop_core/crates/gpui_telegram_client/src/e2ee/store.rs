use crate::cache::sqlite_cache::{db_err, SqliteClientCache};
use crate::e2ee::double_ratchet::DoubleRatchetSession;
use crate::e2ee::keys::{
    b64_encode, DeviceIdentityKeys, E2eeError, LocalOneTimePreKey, LocalSignedPreKey,
};
use crate::e2ee::sender_keys::SenderKeyState;
use gpui_telegram_proto::e2ee::{OneTimePreKeyDto, PublishPreKeysRequest, SignedPreKeyDto};
use parking_lot::RwLock;
use rusqlite::{params, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

#[derive(Default)]
struct MemoryE2eeState {
    identity: Option<DeviceIdentityKeys>,
    next_spk_id: u32,
    next_otpk_id: u32,
    signed_prekeys: HashMap<u32, LocalSignedPreKey>,
    one_time_prekeys: HashMap<u32, LocalOneTimePreKey>,
    sessions: HashMap<(u64, String), DoubleRatchetSession>,
    sender_keys: HashMap<(i64, u64, String), SenderKeyState>,
    sender_key_history: HashMap<(i64, u64, String, u32), (SenderKeyState, u64)>,
    sender_key_seq: u64,
    distributed_keys: HashSet<(i64, u32, u64, String)>,
}

#[derive(Clone)]
pub struct E2eeStore {
    cache: Option<Arc<SqliteClientCache>>,
    mem: Arc<RwLock<MemoryE2eeState>>,
}

impl std::fmt::Debug for E2eeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2eeStore")
            .field("persistent", &self.cache.is_some())
            .finish()
    }
}

impl E2eeStore {
    pub fn in_memory() -> Self {
        Self {
            cache: None,
            mem: Arc::new(RwLock::new(MemoryE2eeState {
                next_spk_id: 1,
                next_otpk_id: 1,
                ..Default::default()
            })),
        }
    }

    pub fn open(base_dir: impl AsRef<Path>) -> Result<Self, E2eeError> {
        let cache = Arc::new(
            SqliteClientCache::open(base_dir.as_ref())
                .map_err(|e| E2eeError::Storage(e.to_string()))?,
        );
        Ok(Self::from_cache(cache))
    }

    pub(crate) fn from_cache(cache: Arc<SqliteClientCache>) -> Self {
        Self {
            cache: Some(cache),
            mem: Arc::new(RwLock::new(MemoryE2eeState {
                next_spk_id: 1,
                next_otpk_id: 1,
                ..Default::default()
            })),
        }
    }

    pub fn get_or_create_identity(&self) -> Result<DeviceIdentityKeys, E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let existing: Option<(Vec<u8>, Vec<u8>)> = conn
                .query_row(
                    "SELECT x25519_sec, ed25519_sec FROM e2ee_identity WHERE id = 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            if let Some((x_sec, e_sec)) = existing {
                if x_sec.len() == 32 && e_sec.len() == 32 {
                    let mut x = [0u8; 32];
                    let mut e = [0u8; 32];
                    x.copy_from_slice(&x_sec);
                    e.copy_from_slice(&e_sec);
                    return Ok(DeviceIdentityKeys::from_secrets(x, e));
                }
            }
            let id = DeviceIdentityKeys::generate()?;
            let now_ms = chrono::Utc::now().timestamp_millis();
            conn.execute(
                r#"INSERT OR REPLACE INTO e2ee_identity (
                       id, x25519_sec, x25519_pub, ed25519_sec, ed25519_pub,
                       next_spk_id, next_otpk_id, created_at_ms
                   ) VALUES (1, ?1, ?2, ?3, ?4, 1, 1, ?5)"#,
                params![
                    id.x25519_sec.as_slice(),
                    id.x25519_pub.as_slice(),
                    id.ed25519_sec.as_slice(),
                    id.ed25519_pub.as_slice(),
                    now_ms,
                ],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(id);
        }

        let mut mem = self.mem.write();
        if let Some(ref id) = mem.identity {
            return Ok(id.clone());
        }
        let id = DeviceIdentityKeys::generate()?;
        mem.identity = Some(id.clone());
        Ok(id)
    }

    pub fn ensure_signed_prekey(
        &self,
        identity: &DeviceIdentityKeys,
    ) -> Result<LocalSignedPreKey, E2eeError> {
        if let Some(spk) = self.latest_signed_prekey()? {
            return Ok(spk);
        }
        let key_id = self.allocate_spk_id()?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let spk = LocalSignedPreKey::generate(key_id, identity, now_ms)?;
        self.save_signed_prekey(&spk)?;
        Ok(spk)
    }

    fn allocate_spk_id(&self) -> Result<u32, E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let curr: u32 = conn
                .query_row(
                    "SELECT next_spk_id FROM e2ee_identity WHERE id = 1",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(1)
                .max(1) as u32;
            conn.execute(
                "UPDATE e2ee_identity SET next_spk_id = ?1 WHERE id = 1",
                params![(curr + 1) as i64],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(curr);
        }
        let mut mem = self.mem.write();
        let curr = mem.next_spk_id.max(1);
        mem.next_spk_id = curr + 1;
        Ok(curr)
    }

    fn allocate_otpk_ids(&self, count: usize) -> Result<u32, E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let curr: u32 = conn
                .query_row(
                    "SELECT next_otpk_id FROM e2ee_identity WHERE id = 1",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(1)
                .max(1) as u32;
            conn.execute(
                "UPDATE e2ee_identity SET next_otpk_id = ?1 WHERE id = 1",
                params![(curr + count as u32) as i64],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(curr);
        }
        let mut mem = self.mem.write();
        let curr = mem.next_otpk_id.max(1);
        mem.next_otpk_id = curr + count as u32;
        Ok(curr)
    }

    pub fn save_signed_prekey(&self, spk: &LocalSignedPreKey) -> Result<(), E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            conn.execute(
                r#"INSERT OR REPLACE INTO e2ee_signed_prekeys (
                       key_id, secret, public, signature, created_at_ms
                   ) VALUES (?1, ?2, ?3, ?4, ?5)"#,
                params![
                    spk.key_id as i64,
                    spk.secret.as_slice(),
                    spk.public.as_slice(),
                    spk.signature.as_slice(),
                    spk.created_at_ms,
                ],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(());
        }
        self.mem.write().signed_prekeys.insert(spk.key_id, spk.clone());
        Ok(())
    }

    pub fn get_signed_prekey(&self, key_id: u32) -> Result<Option<LocalSignedPreKey>, E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>, i64)> = conn
                .query_row(
                    "SELECT secret, public, signature, created_at_ms FROM e2ee_signed_prekeys WHERE key_id = ?1",
                    params![key_id as i64],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(row.and_then(|(sec, pub_b, sig_b, ts)| {
                if sec.len() == 32 && pub_b.len() == 32 && sig_b.len() == 64 {
                    let mut secret = [0u8; 32];
                    let mut public = [0u8; 32];
                    let mut signature = [0u8; 64];
                    secret.copy_from_slice(&sec);
                    public.copy_from_slice(&pub_b);
                    signature.copy_from_slice(&sig_b);
                    Some(LocalSignedPreKey {
                        key_id,
                        secret,
                        public,
                        signature,
                        created_at_ms: ts,
                    })
                } else {
                    None
                }
            }));
        }
        Ok(self.mem.read().signed_prekeys.get(&key_id).cloned())
    }

    pub fn latest_signed_prekey(&self) -> Result<Option<LocalSignedPreKey>, E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let max_id: Option<i64> = conn
                .query_row("SELECT MAX(key_id) FROM e2ee_signed_prekeys", [], |r| r.get(0))
                .optional()
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?
                .flatten();
            drop(conn);
            return match max_id {
                Some(id) if id >= 0 => self.get_signed_prekey(id as u32),
                _ => Ok(None),
            };
        }
        let mem = self.mem.read();
        Ok(mem.signed_prekeys.values().max_by_key(|k| k.key_id).cloned())
    }

    pub fn generate_one_time_prekeys(
        &self,
        count: usize,
    ) -> Result<Vec<LocalOneTimePreKey>, E2eeError> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let start_id = self.allocate_otpk_ids(count)?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut keys = Vec::with_capacity(count);
        for offset in 0..count {
            keys.push(LocalOneTimePreKey::generate(start_id + offset as u32, now_ms)?);
        }
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            for k in &keys {
                conn.execute(
                    r#"INSERT OR REPLACE INTO e2ee_one_time_prekeys (
                           key_id, secret, public, consumed, created_at_ms
                       ) VALUES (?1, ?2, ?3, 0, ?4)"#,
                    params![
                        k.key_id as i64,
                        k.secret.as_slice(),
                        k.public.as_slice(),
                        k.created_at_ms,
                    ],
                )
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            }
        } else {
            let mut mem = self.mem.write();
            for k in &keys {
                mem.one_time_prekeys.insert(k.key_id, k.clone());
            }
        }
        Ok(keys)
    }

    pub fn get_one_time_prekey(
        &self,
        key_id: u32,
    ) -> Result<Option<LocalOneTimePreKey>, E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let row: Option<(Vec<u8>, Vec<u8>, i64, i64)> = conn
                .query_row(
                    "SELECT secret, public, consumed, created_at_ms FROM e2ee_one_time_prekeys WHERE key_id = ?1",
                    params![key_id as i64],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(row.and_then(|(sec, pub_b, consumed, ts)| {
                if consumed == 0 && sec.len() == 32 && pub_b.len() == 32 {
                    let mut secret = [0u8; 32];
                    let mut public = [0u8; 32];
                    secret.copy_from_slice(&sec);
                    public.copy_from_slice(&pub_b);
                    Some(LocalOneTimePreKey {
                        key_id,
                        secret,
                        public,
                        consumed: false,
                        created_at_ms: ts,
                    })
                } else {
                    None
                }
            }));
        }
        Ok(self
            .mem
            .read()
            .one_time_prekeys
            .get(&key_id)
            .filter(|k| !k.consumed)
            .cloned())
    }

    pub fn consume_one_time_prekey(&self, key_id: u32) -> Result<Option<LocalOneTimePreKey>, E2eeError> {
        let key = self.get_one_time_prekey(key_id)?;
        if key.is_some() {
            if let Some(ref cache) = self.cache {
                let conn = cache.conn.lock();
                conn.execute(
                    "UPDATE e2ee_one_time_prekeys SET consumed = 1 WHERE key_id = ?1",
                    params![key_id as i64],
                )
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            } else if let Some(slot) = self.mem.write().one_time_prekeys.get_mut(&key_id) {
                slot.consumed = true;
            }
        }
        Ok(key)
    }

    pub fn peek_unconsumed_otpk(&self) -> Option<LocalOneTimePreKey> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let id: Option<i64> = conn
                .query_row(
                    "SELECT key_id FROM e2ee_one_time_prekeys WHERE consumed = 0 ORDER BY key_id ASC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .ok()
                .flatten();
            drop(conn);
            return id.and_then(|k| self.get_one_time_prekey(k as u32).ok().flatten());
        }
        self.mem
            .read()
            .one_time_prekeys
            .values()
            .filter(|k| !k.consumed)
            .min_by_key(|k| k.key_id)
            .cloned()
    }

    pub fn build_publish_request(
        &self,
        device_id: &str,
        device_name: Option<String>,
        otpk_batch_size: usize,
    ) -> Result<PublishPreKeysRequest, E2eeError> {
        let identity = self.get_or_create_identity()?;
        let spk = self.ensure_signed_prekey(&identity)?;
        let otpks = self.generate_one_time_prekeys(otpk_batch_size)?;
        Ok(PublishPreKeysRequest {
            device_id: device_id.to_string(),
            device_name,
            identity_key: b64_encode(&identity.x25519_pub),
            identity_signing_key: b64_encode(&identity.ed25519_pub),
            signed_prekey: SignedPreKeyDto {
                key_id: spk.key_id,
                public_key: b64_encode(&spk.public),
                signature: b64_encode(&spk.signature),
                created_at_ms: spk.created_at_ms,
            },
            one_time_prekeys: otpks
                .into_iter()
                .map(|k| OneTimePreKeyDto {
                    key_id: k.key_id,
                    public_key: b64_encode(&k.public),
                })
                .collect(),
        })
    }

    pub fn get_session(
        &self,
        peer_user_id: u64,
        peer_device_id: &str,
    ) -> Result<Option<DoubleRatchetSession>, E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let json: Option<String> = conn
                .query_row(
                    "SELECT state_json FROM e2ee_sessions WHERE peer_user_id = ?1 AND peer_device_id = ?2",
                    params![peer_user_id as i64, peer_device_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return match json {
                Some(j) => Ok(Some(DoubleRatchetSession::from_json(&j)?)),
                None => Ok(None),
            };
        }
        Ok(self
            .mem
            .read()
            .sessions
            .get(&(peer_user_id, peer_device_id.to_string()))
            .cloned())
    }

    pub fn save_session(
        &self,
        peer_user_id: u64,
        peer_device_id: &str,
        session: &DoubleRatchetSession,
    ) -> Result<(), E2eeError> {
        if let Some(ref cache) = self.cache {
            let json = session.to_json()?;
            let now_ms = chrono::Utc::now().timestamp_millis();
            let conn = cache.conn.lock();
            conn.execute(
                r#"INSERT OR REPLACE INTO e2ee_sessions (
                       peer_user_id, peer_device_id, state_json, updated_at_ms
                   ) VALUES (?1, ?2, ?3, ?4)"#,
                params![peer_user_id as i64, peer_device_id, json, now_ms],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(());
        }
        self.mem
            .write()
            .sessions
            .insert((peer_user_id, peer_device_id.to_string()), session.clone());
        Ok(())
    }

    pub fn list_peer_sessions(
        &self,
        peer_user_id: u64,
    ) -> Result<Vec<(String, DoubleRatchetSession)>, E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let mut stmt = conn
                .prepare("SELECT peer_device_id, state_json FROM e2ee_sessions WHERE peer_user_id = ?1")
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            let rows = stmt
                .query_map(params![peer_user_id as i64], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            let mut out = Vec::new();
            for row in rows {
                let (dev, json) = row.map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
                out.push((dev, DoubleRatchetSession::from_json(&json)?));
            }
            return Ok(out);
        }
        Ok(self
            .mem
            .read()
            .sessions
            .iter()
            .filter(|((uid, _), _)| *uid == peer_user_id)
            .map(|((_, dev), sess)| (dev.clone(), sess.clone()))
            .collect())
    }

    pub fn get_sender_key(
        &self,
        chat_id: i64,
        sender_user_id: u64,
        sender_device_id: &str,
    ) -> Result<Option<SenderKeyState>, E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let json: Option<String> = conn
                .query_row(
                    r#"SELECT state_json FROM e2ee_sender_keys
                       WHERE chat_id = ?1 AND sender_user_id = ?2 AND sender_device_id = ?3"#,
                    params![chat_id, sender_user_id as i64, sender_device_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return match json {
                Some(j) => Ok(Some(SenderKeyState::from_json(&j)?)),
                None => Ok(None),
            };
        }
        Ok(self
            .mem
            .read()
            .sender_keys
            .get(&(chat_id, sender_user_id, sender_device_id.to_string()))
            .cloned())
    }

    pub fn get_sender_key_for_chain(
        &self,
        chat_id: i64,
        sender_user_id: u64,
        sender_device_id: &str,
        chain_id: u32,
    ) -> Result<Option<SenderKeyState>, E2eeError> {
        if let Some(active) = self.get_sender_key(chat_id, sender_user_id, sender_device_id)? {
            if active.chain_id == chain_id {
                return Ok(Some(active));
            }
        }
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let json: Option<String> = conn
                .query_row(
                    r#"SELECT state_json FROM e2ee_sender_key_history
                       WHERE chat_id = ?1 AND sender_user_id = ?2 AND sender_device_id = ?3 AND chain_id = ?4"#,
                    params![chat_id, sender_user_id as i64, sender_device_id, chain_id as i64],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return match json {
                Some(j) => Ok(Some(SenderKeyState::from_json(&j)?)),
                None => Ok(None),
            };
        }
        Ok(self
            .mem
            .read()
            .sender_key_history
            .get(&(chat_id, sender_user_id, sender_device_id.to_string(), chain_id))
            .map(|(st, _)| st.clone()))
    }

    pub fn save_sender_key(&self, state: &SenderKeyState) -> Result<(), E2eeError> {
        const MAX_SENDER_KEY_HISTORY: usize = 4;
        if let Some(ref cache) = self.cache {
            let json = state.to_json()?;
            let now_ms = chrono::Utc::now().timestamp_millis();
            let conn = cache.conn.lock();
            conn.execute(
                r#"INSERT OR REPLACE INTO e2ee_sender_keys (
                       chat_id, sender_user_id, sender_device_id, state_json, updated_at_ms
                   ) VALUES (?1, ?2, ?3, ?4, ?5)"#,
                params![
                    state.chat_id,
                    state.sender_user_id as i64,
                    &state.sender_device_id,
                    json,
                    now_ms,
                ],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            conn.execute(
                r#"INSERT OR REPLACE INTO e2ee_sender_key_history (
                       chat_id, sender_user_id, sender_device_id, chain_id, state_json, updated_at_ms
                   ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                params![
                    state.chat_id,
                    state.sender_user_id as i64,
                    &state.sender_device_id,
                    state.chain_id as i64,
                    json,
                    now_ms,
                ],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            // Prune history beyond MAX_SENDER_KEY_HISTORY while always keeping the active chain_id.
            let _ = conn.execute(
                r#"DELETE FROM e2ee_sender_key_history
                   WHERE chat_id = ?1 AND sender_user_id = ?2 AND sender_device_id = ?3
                     AND chain_id != ?4
                     AND chain_id NOT IN (
                         SELECT chain_id FROM e2ee_sender_key_history
                         WHERE chat_id = ?1 AND sender_user_id = ?2 AND sender_device_id = ?3
                         ORDER BY updated_at_ms DESC, rowid DESC
                         LIMIT ?5
                     )"#,
                params![
                    state.chat_id,
                    state.sender_user_id as i64,
                    &state.sender_device_id,
                    state.chain_id as i64,
                    MAX_SENDER_KEY_HISTORY as i64,
                ],
            );
            return Ok(());
        }
        let mut mem = self.mem.write();
        mem.sender_keys.insert(
            (
                state.chat_id,
                state.sender_user_id,
                state.sender_device_id.clone(),
            ),
            state.clone(),
        );
        mem.sender_key_seq = mem.sender_key_seq.saturating_add(1);
        let seq = mem.sender_key_seq;
        mem.sender_key_history.insert(
            (
                state.chat_id,
                state.sender_user_id,
                state.sender_device_id.clone(),
                state.chain_id,
            ),
            (state.clone(), seq),
        );
        let mut matching: Vec<((i64, u64, String, u32), u64)> = mem
            .sender_key_history
            .iter()
            .filter(|((cid, uid, dev, ch), _)| {
                *cid == state.chat_id
                    && *uid == state.sender_user_id
                    && dev == &state.sender_device_id
                    && *ch != state.chain_id
            })
            .map(|(k, (_, s))| (k.clone(), *s))
            .collect();
        if matching.len() >= MAX_SENDER_KEY_HISTORY {
            matching.sort_by_key(|(_, s)| *s);
            let to_remove = matching.len() + 1 - MAX_SENDER_KEY_HISTORY;
            for (k, _) in matching.into_iter().take(to_remove) {
                mem.sender_key_history.remove(&k);
            }
        }
        Ok(())
    }

    pub fn update_sender_key_state(&self, state: &SenderKeyState) -> Result<(), E2eeError> {
        if let Some(active) = self.get_sender_key(state.chat_id, state.sender_user_id, &state.sender_device_id)? {
            if active.chain_id == state.chain_id {
                return self.save_sender_key(state);
            }
        } else {
            return self.save_sender_key(state);
        }
        if let Some(ref cache) = self.cache {
            let json = state.to_json()?;
            let now_ms = chrono::Utc::now().timestamp_millis();
            let conn = cache.conn.lock();
            conn.execute(
                r#"INSERT OR REPLACE INTO e2ee_sender_key_history (
                       chat_id, sender_user_id, sender_device_id, chain_id, state_json, updated_at_ms
                   ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                params![
                    state.chat_id,
                    state.sender_user_id as i64,
                    &state.sender_device_id,
                    state.chain_id as i64,
                    json,
                    now_ms,
                ],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(());
        }
        let mut mem = self.mem.write();
        mem.sender_key_seq = mem.sender_key_seq.saturating_add(1);
        let seq = mem.sender_key_seq;
        mem.sender_key_history.insert(
            (
                state.chat_id,
                state.sender_user_id,
                state.sender_device_id.clone(),
                state.chain_id,
            ),
            (state.clone(), seq),
        );
        Ok(())
    }

    pub fn delete_own_sender_key(
        &self,
        chat_id: i64,
        sender_user_id: u64,
        sender_device_id: &str,
    ) -> Result<(), E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            conn.execute(
                r#"DELETE FROM e2ee_sender_keys
                   WHERE chat_id = ?1 AND sender_user_id = ?2 AND sender_device_id = ?3"#,
                params![chat_id, sender_user_id as i64, sender_device_id],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            conn.execute(
                "DELETE FROM e2ee_distributed_keys WHERE chat_id = ?1",
                params![chat_id],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(());
        }
        let mut mem = self.mem.write();
        mem.sender_keys
            .remove(&(chat_id, sender_user_id, sender_device_id.to_string()));
        mem.distributed_keys
            .retain(|(cid, _, _, _)| *cid != chat_id);
        Ok(())
    }

    pub fn is_sender_key_distributed_to(
        &self,
        chat_id: i64,
        chain_id: u32,
        peer_user_id: u64,
        peer_device_id: &str,
    ) -> Result<bool, E2eeError> {
        if let Some(ref cache) = self.cache {
            let conn = cache.conn.lock();
            let hit: Option<i64> = conn
                .query_row(
                    r#"SELECT 1 FROM e2ee_distributed_keys
                       WHERE chat_id = ?1 AND chain_id = ?2 AND peer_user_id = ?3 AND peer_device_id = ?4"#,
                    params![chat_id, chain_id as i64, peer_user_id as i64, peer_device_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(hit.is_some());
        }
        Ok(self.mem.read().distributed_keys.contains(&(
            chat_id,
            chain_id,
            peer_user_id,
            peer_device_id.to_string(),
        )))
    }

    pub fn mark_sender_key_distributed_to(
        &self,
        chat_id: i64,
        chain_id: u32,
        peer_user_id: u64,
        peer_device_id: &str,
    ) -> Result<(), E2eeError> {
        if let Some(ref cache) = self.cache {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let conn = cache.conn.lock();
            conn.execute(
                r#"INSERT OR REPLACE INTO e2ee_distributed_keys (
                       chat_id, chain_id, peer_user_id, peer_device_id, distributed_at_ms
                   ) VALUES (?1, ?2, ?3, ?4, ?5)"#,
                params![
                    chat_id,
                    chain_id as i64,
                    peer_user_id as i64,
                    peer_device_id,
                    now_ms,
                ],
            )
            .map_err(|e| E2eeError::Storage(db_err(e).to_string()))?;
            return Ok(());
        }
        self.mem.write().distributed_keys.insert((
            chat_id,
            chain_id,
            peer_user_id,
            peer_device_id.to_string(),
        ));
        Ok(())
    }
}
