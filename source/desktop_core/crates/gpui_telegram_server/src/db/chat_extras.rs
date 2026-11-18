use crate::db::{ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MsgId, UserId};
use gpui_telegram_proto::contracts::{
    BlockedUserDto, ContactDto, DraftDto, FolderDto, InviteLinkDto, UpsertFolderRequest,
    UserPrivacySettingsDto,
};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

impl ServerDatabase {
    pub fn upsert_contact(&self, owner: UserId, contact_id: UserId, first_name: &str, last_name: Option<&str>, phone: Option<&str>) -> Result<ContactDto> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO contacts (owner_user_id, contact_user_id, first_name, last_name, phone, created_at_ms, updated_at_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                   ON CONFLICT(owner_user_id, contact_user_id) DO UPDATE SET
                       first_name = excluded.first_name, last_name = excluded.last_name,
                       phone = COALESCE(excluded.phone, contacts.phone), updated_at_ms = excluded.updated_at_ms"#,
                params![owner.0 as i64, contact_id.0 as i64, first_name, last_name, phone, now_ms],
            ).map_db()?;
            conn.query_row(
                r#"SELECT c.contact_user_id, c.first_name, c.last_name, u.username, COALESCE(c.phone, u.phone), u.is_bot,
                          EXISTS(SELECT 1 FROM contacts c2 WHERE c2.owner_user_id = c.contact_user_id AND c2.contact_user_id = c.owner_user_id) AS mutual
                   FROM contacts c JOIN users u ON u.user_id = c.contact_user_id
                   WHERE c.owner_user_id = ?1 AND c.contact_user_id = ?2"#,
                params![owner.0 as i64, contact_id.0 as i64],
                |r| Ok(ContactDto {
                    user_id: r.get::<_, i64>(0)? as u64, first_name: r.get(1)?, last_name: r.get(2)?,
                    username: r.get(3)?, phone: r.get(4)?, is_bot: r.get::<_, i64>(5)? != 0,
                    is_mutual: r.get::<_, i64>(6)? != 0, chat_id: None,
                }),
            ).map_db()
        })
    }

    pub fn list_contacts(&self, owner: UserId) -> Result<Vec<ContactDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT c.contact_user_id, c.first_name, c.last_name, u.username, COALESCE(c.phone, u.phone), u.is_bot,
                          EXISTS(SELECT 1 FROM contacts c2 WHERE c2.owner_user_id = c.contact_user_id AND c2.contact_user_id = c.owner_user_id) AS mutual
                   FROM contacts c JOIN users u ON u.user_id = c.contact_user_id
                   WHERE c.owner_user_id = ?1 ORDER BY c.first_name ASC"#
            ).map_db()?;
            let rows = stmt.query_map(params![owner.0 as i64], |r| Ok(ContactDto {
                user_id: r.get::<_, i64>(0)? as u64, first_name: r.get(1)?, last_name: r.get(2)?,
                username: r.get(3)?, phone: r.get(4)?, is_bot: r.get::<_, i64>(5)? != 0,
                is_mutual: r.get::<_, i64>(6)? != 0, chat_id: None,
            })).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn delete_contact(&self, owner: UserId, contact_id: UserId) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let n = conn.execute("DELETE FROM contacts WHERE owner_user_id = ?1 AND contact_user_id = ?2",
                params![owner.0 as i64, contact_id.0 as i64]).map_db()?;
            Ok(n > 0)
        })
    }

    pub fn update_member_role(&self, chat_id: ChatId, target_user_id: UserId, role: &str, admin_rights: u32, banned_rights: u32, admin_title: Option<&str>) -> Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            let n = conn.execute(
                "UPDATE chat_members SET role = ?1, admin_rights = ?2, banned_rights = ?3, admin_title = ?4, updated_at_ms = ?5 WHERE chat_id = ?6 AND user_id = ?7",
                params![role, admin_rights as i64, banned_rights as i64, admin_title, now_ms, chat_id.0, target_user_id.0 as i64],
            ).map_db()?;
            Ok(n > 0)
        })
    }

    pub fn delete_chat(&self, chat_id: ChatId) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let n = conn.execute("DELETE FROM chats WHERE chat_id = ?1", params![chat_id.0]).map_db()?;
            Ok(n > 0)
        })
    }

    pub fn create_chat_invite(&self, chat_id: ChatId, creator_id: UserId, title: Option<&str>, usage_limit: Option<u32>, expire_date_ms: Option<i64>, request_needed: bool) -> Result<InviteLinkDto> {
        let invite_id = uuid::Uuid::new_v4().to_string();
        let code = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let link = format!("https://t.me/+{}", code);
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO chat_invites (invite_id, chat_id, code, creator_id, title, usage_limit, usage_count, expire_date_ms, request_needed, is_revoked, created_at_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, 0, ?9)"#,
                params![invite_id, chat_id.0, code, creator_id.0 as i64, title, usage_limit.map(|v| v as i64), expire_date_ms, request_needed as i64, now_ms],
            ).map_db()?;
            Ok(InviteLinkDto {
                invite_id, chat_id: chat_id.0, creator_id: creator_id.0, link, code,
                title: title.map(str::to_string), usage_limit, usage_count: 0, expire_date_ms,
                request_needed, is_revoked: false, created_at_ms: now_ms,
            })
        })
    }

    pub fn list_chat_invites(&self, chat_id: ChatId) -> Result<Vec<InviteLinkDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT invite_id, chat_id, creator_id, code, title, usage_limit, usage_count, expire_date_ms, request_needed, is_revoked, created_at_ms FROM chat_invites WHERE chat_id = ?1 ORDER BY created_at_ms DESC"
            ).map_db()?;
            let rows = stmt.query_map(params![chat_id.0], |r| {
                let code: String = r.get(3)?;
                Ok(InviteLinkDto {
                    invite_id: r.get(0)?, chat_id: r.get(1)?, creator_id: r.get::<_, i64>(2)? as u64,
                    link: format!("https://t.me/+{}", code), code, title: r.get(4)?,
                    usage_limit: r.get::<_, Option<i64>>(5)?.map(|v| v as u32),
                    usage_count: r.get::<_, i64>(6)? as u32, expire_date_ms: r.get(7)?,
                    request_needed: r.get::<_, i64>(8)? != 0, is_revoked: r.get::<_, i64>(9)? != 0,
                    created_at_ms: r.get(10)?,
                })
            }).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn revoke_chat_invite(&self, chat_id: ChatId, invite_id: &str) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let n = conn.execute("UPDATE chat_invites SET is_revoked = 1 WHERE chat_id = ?1 AND invite_id = ?2", params![chat_id.0, invite_id]).map_db()?;
            Ok(n > 0)
        })
    }

    pub fn join_chat_by_invite_code(&self, user_id: UserId, code_or_link: &str) -> Result<ChatId> {
        let code = code_or_link.trim().trim_start_matches("https://t.me/+").trim_start_matches('+');
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let row: Option<(String, i64, Option<i64>, i64, Option<i64>, i64)> = tx.query_row(
                "SELECT invite_id, chat_id, usage_limit, usage_count, expire_date_ms, is_revoked FROM chat_invites WHERE code = ?1",
                params![code],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            ).optional().map_err(|e| TelegramError::Database(e.to_string()))?;
            let (invite_id, cid, limit, count, exp, revoked) = row.ok_or_else(|| TelegramError::NotFound("Invite link not found".into()))?;
            if revoked != 0 { return Err(TelegramError::Validation("Invite link has been revoked".into())); }
            if let Some(e) = exp { if e <= now_ms { return Err(TelegramError::Validation("Invite link expired".into())); } }
            if let Some(lim) = limit { if count >= lim { return Err(TelegramError::Validation("Invite usage limit reached".into())); } }
            let existing_role: Option<String> = tx.query_row(
                "SELECT role FROM chat_members WHERE chat_id = ?1 AND user_id = ?2",
                params![cid, user_id.0 as i64],
                |r| r.get(0),
            ).optional().map_err(|e| TelegramError::Database(e.to_string()))?;
            if let Some(ref role) = existing_role {
                let norm = role.trim().to_ascii_lowercase();
                if matches!(norm.as_str(), "kicked" | "banned") {
                    return Err(TelegramError::PermissionDenied("User is banned from this chat".into()));
                }
                if norm == "left" {
                    tx.execute(
                        "UPDATE chat_members SET role = 'member', updated_at_ms = ?3 WHERE chat_id = ?1 AND user_id = ?2",
                        params![cid, user_id.0 as i64, now_ms],
                    ).map_err(|e| TelegramError::Database(e.to_string()))?;
                }
            } else {
                tx.execute(
                    r#"INSERT INTO chat_members (chat_id, user_id, role, permissions_bitmask, admin_title, admin_rights, banned_rights, until_date_ms, joined_at_ms, invited_by, updated_at_ms)
                       VALUES (?1, ?2, 'member', 65535, NULL, 0, 0, NULL, ?3, NULL, ?3)"#,
                    params![cid, user_id.0 as i64, now_ms],
                ).map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            tx.execute("UPDATE chat_invites SET usage_count = usage_count + 1 WHERE invite_id = ?1", params![invite_id])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok(ChatId(cid))
        })
    }

    pub fn list_folders(&self, user_id: UserId) -> Result<Vec<FolderDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT folder_id, title, emoticon, pinned_chat_ids_json, included_chat_ids_json, excluded_chat_ids_json,
                          include_contacts, include_non_contacts, include_groups, include_channels, include_bots,
                          exclude_muted, exclude_read, exclude_archived, sort_order
                   FROM chat_folders WHERE user_id = ?1 ORDER BY sort_order ASC, folder_id ASC"#
            ).map_db()?;
            let rows = stmt.query_map(params![user_id.0 as i64], |r| Ok(FolderDto {
                id: r.get(0)?, title: r.get(1)?, emoticon: r.get(2)?,
                pinned_chats: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                include_chats: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                exclude_chats: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
                include_contacts: r.get::<_, i64>(6)? != 0, include_non_contacts: r.get::<_, i64>(7)? != 0,
                include_groups: r.get::<_, i64>(8)? != 0, include_channels: r.get::<_, i64>(9)? != 0,
                include_bots: r.get::<_, i64>(10)? != 0, exclude_muted: r.get::<_, i64>(11)? != 0,
                exclude_read: r.get::<_, i64>(12)? != 0, exclude_archived: r.get::<_, i64>(13)? != 0,
                sort_order: r.get(14)?,
            })).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn upsert_folder(&self, user_id: UserId, folder_id: Option<i64>, req: &UpsertFolderRequest) -> Result<FolderDto> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut fid = folder_id.unwrap_or(req.id);
        self.pool.with_conn(|conn| {
            if fid <= 0 {
                let max_id: i64 = conn.query_row(
                    "SELECT COALESCE(MAX(folder_id), 0) FROM chat_folders WHERE user_id = ?1",
                    params![user_id.0 as i64], |r| r.get(0),
                ).map_db()?;
                fid = max_id + 1;
            }
            conn.execute(
                r#"INSERT OR REPLACE INTO chat_folders
                (user_id, folder_id, title, emoticon, pinned_chat_ids_json, included_chat_ids_json, excluded_chat_ids_json,
                 include_contacts, include_non_contacts, include_groups, include_channels, include_bots,
                 exclude_muted, exclude_read, exclude_archived, sort_order, updated_at_ms)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"#,
                params![
                    user_id.0 as i64, fid, req.title.trim(), req.emoticon,
                    serde_json::to_string(&req.pinned_chats).unwrap_or_else(|_| "[]".into()),
                    serde_json::to_string(&req.include_chats).unwrap_or_else(|_| "[]".into()),
                    serde_json::to_string(&req.exclude_chats).unwrap_or_else(|_| "[]".into()),
                    req.include_contacts as i64, req.include_non_contacts as i64, req.include_groups as i64,
                    req.include_channels as i64, req.include_bots as i64, req.exclude_muted as i64,
                    req.exclude_read as i64, req.exclude_archived as i64, fid as i32, now_ms,
                ],
            ).map_db()?;
            Ok(FolderDto {
                id: fid, title: req.title.trim().to_string(), emoticon: req.emoticon.clone(),
                pinned_chats: req.pinned_chats.clone(), include_chats: req.include_chats.clone(),
                exclude_chats: req.exclude_chats.clone(), include_contacts: req.include_contacts,
                include_non_contacts: req.include_non_contacts, include_groups: req.include_groups,
                include_channels: req.include_channels, include_bots: req.include_bots,
                exclude_muted: req.exclude_muted, exclude_read: req.exclude_read,
                exclude_archived: req.exclude_archived, sort_order: fid as i32,
            })
        })
    }

    pub fn delete_folder(&self, user_id: UserId, folder_id: i64) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let n = conn.execute("DELETE FROM chat_folders WHERE user_id = ?1 AND folder_id = ?2", params![user_id.0 as i64, folder_id]).map_db()?;
            Ok(n > 0)
        })
    }

    pub fn set_chat_setting(&self, user_id: UserId, chat_id: ChatId, archived: Option<bool>, pinned: Option<bool>, muted: Option<bool>, mute_until_ms: Option<Option<i64>>) -> Result<(bool, bool, bool, Option<i64>)> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO user_chat_settings (user_id, chat_id, updated_at_ms) VALUES (?1, ?2, ?3)",
                params![user_id.0 as i64, chat_id.0, now_ms],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            if let Some(a) = archived {
                tx.execute("UPDATE user_chat_settings SET is_archived = ?1, updated_at_ms = ?2 WHERE user_id = ?3 AND chat_id = ?4",
                    params![a as i64, now_ms, user_id.0 as i64, chat_id.0]).map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            if let Some(p) = pinned {
                let order = if p { now_ms } else { 0 };
                tx.execute("UPDATE user_chat_settings SET is_pinned = ?1, pin_order = ?2, updated_at_ms = ?3 WHERE user_id = ?4 AND chat_id = ?5",
                    params![p as i64, order, now_ms, user_id.0 as i64, chat_id.0]).map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            if let Some(m) = muted {
                tx.execute("UPDATE user_chat_settings SET is_muted = ?1, updated_at_ms = ?2 WHERE user_id = ?3 AND chat_id = ?4",
                    params![m as i64, now_ms, user_id.0 as i64, chat_id.0]).map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            if let Some(until) = mute_until_ms {
                tx.execute("UPDATE user_chat_settings SET mute_until_ms = ?1, updated_at_ms = ?2 WHERE user_id = ?3 AND chat_id = ?4",
                    params![until, now_ms, user_id.0 as i64, chat_id.0]).map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            tx.query_row(
                "SELECT is_archived, is_pinned, is_muted, mute_until_ms FROM user_chat_settings WHERE user_id = ?1 AND chat_id = ?2",
                params![user_id.0 as i64, chat_id.0],
                |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0, r.get::<_, i64>(2)? != 0, r.get(3)?)),
            ).map_err(|e| TelegramError::Database(e.to_string()))
        })
    }

    pub fn get_user_chat_setting(&self, user_id: UserId, chat_id: ChatId) -> Result<(bool, bool, bool, Option<i64>)> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT is_archived, is_pinned, is_muted, mute_until_ms FROM user_chat_settings WHERE user_id = ?1 AND chat_id = ?2",
                params![user_id.0 as i64, chat_id.0],
                |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0, r.get::<_, i64>(2)? != 0, r.get(3)?)),
            ).optional().map_db().map(|o| o.unwrap_or((false, false, false, None)))
        })
    }

    pub fn pin_message(&self, chat_id: ChatId, message_id: MsgId, user_id: UserId) -> Result<Vec<i64>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO pinned_messages (chat_id, message_id, pinned_by, pinned_at_ms) VALUES (?1, ?2, ?3, ?4)",
                params![chat_id.0, message_id.0, user_id.0 as i64, now_ms],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute("UPDATE chats SET pinned_message_id = ?1, updated_at_ms = ?2 WHERE chat_id = ?3", params![message_id.0, now_ms, chat_id.0])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            Self::query_pinned_ids_tx(tx, chat_id)
        })
    }

    pub fn unpin_message(&self, chat_id: ChatId, message_id: Option<MsgId>) -> Result<Vec<i64>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            if let Some(mid) = message_id {
                tx.execute("DELETE FROM pinned_messages WHERE chat_id = ?1 AND message_id = ?2", params![chat_id.0, mid.0])
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
            } else {
                tx.execute("DELETE FROM pinned_messages WHERE chat_id = ?1", params![chat_id.0])
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            let ids = Self::query_pinned_ids_tx(tx, chat_id)?;
            let latest = ids.first().copied();
            tx.execute("UPDATE chats SET pinned_message_id = ?1, updated_at_ms = ?2 WHERE chat_id = ?3", params![latest, now_ms, chat_id.0])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok(ids)
        })
    }

    pub fn list_pinned_messages(&self, chat_id: ChatId) -> Result<Vec<i64>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT pm.message_id
                     FROM pinned_messages pm
                     INNER JOIN messages m ON m.chat_id = pm.chat_id AND m.message_id = pm.message_id
                     WHERE pm.chat_id = ?1 AND m.deleted_at_ms IS NULL
                     ORDER BY pm.pinned_at_ms DESC, pm.message_id DESC",
                )
                .map_db()?;
            let rows = stmt.query_map(params![chat_id.0], |r| r.get(0)).map_db()?;
            rows.collect::<std::result::Result<Vec<i64>, _>>().map_db()
        })
    }

    pub fn list_pinned_message_dtos(
        &self,
        chat_id: ChatId,
    ) -> Result<Vec<gpui_telegram_proto::contracts::chats::PinnedMessageDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT pm.message_id, pm.pinned_by, pm.pinned_at_ms, m.content_json
                     FROM pinned_messages pm
                     INNER JOIN messages m ON m.chat_id = pm.chat_id AND m.message_id = pm.message_id
                     WHERE pm.chat_id = ?1 AND m.deleted_at_ms IS NULL
                     ORDER BY pm.pinned_at_ms DESC, pm.message_id DESC",
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![chat_id.0], |r| {
                    let message_id: i64 = r.get(0)?;
                    let pinned_by = r.get::<_, i64>(1)?.max(0) as u64;
                    let pinned_at_ms: i64 = r.get(2)?;
                    let content_json: Option<String> = r.get(3)?;
                    let preview_text = content_json
                        .as_deref()
                        .and_then(|raw| {
                            serde_json::from_str::<gpui_telegram_proto::message::MessageContent>(raw)
                                .ok()
                        })
                        .map(|c| {
                            let full = crate::server::extract_message_content_text(&c);
                            full.lines().next().unwrap_or(&full).to_string()
                        })
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| "[Pinned message]".to_string());
                    Ok(gpui_telegram_proto::contracts::chats::PinnedMessageDto {
                        chat_id: chat_id.0,
                        message_id,
                        pinned_by,
                        pinned_at_ms,
                        preview_text,
                    })
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    fn query_pinned_ids_tx(tx: &rusqlite::Transaction<'_>, chat_id: ChatId) -> Result<Vec<i64>> {
        let mut stmt = tx
            .prepare(
                "SELECT pm.message_id
                 FROM pinned_messages pm
                 INNER JOIN messages m ON m.chat_id = pm.chat_id AND m.message_id = pm.message_id
                 WHERE pm.chat_id = ?1 AND m.deleted_at_ms IS NULL
                 ORDER BY pm.pinned_at_ms DESC, pm.message_id DESC",
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let rows = stmt.query_map(params![chat_id.0], |r| r.get(0)).map_err(|e| TelegramError::Database(e.to_string()))?;
        rows.collect::<std::result::Result<Vec<i64>, _>>().map_err(|e| TelegramError::Database(e.to_string()))
    }

    pub fn upsert_draft(&self, user_id: UserId, chat_id: ChatId, topic_id: i64, text: &str, entities: &Value, reply_to: Option<i64>, no_webpage: bool) -> Result<Option<DraftDto>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            if text.is_empty() && reply_to.is_none() {
                conn.execute("DELETE FROM drafts WHERE user_id = ?1 AND chat_id = ?2 AND topic_id = ?3",
                    params![user_id.0 as i64, chat_id.0, topic_id]).map_db()?;
                return Ok(None);
            }
            let entities_json = serde_json::to_string(entities).unwrap_or_else(|_| "[]".into());
            conn.execute(
                r#"INSERT OR REPLACE INTO drafts (user_id, chat_id, topic_id, text, entities_json, reply_to_msg_id, no_webpage, date_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
                params![user_id.0 as i64, chat_id.0, topic_id, text, entities_json, reply_to, no_webpage as i64, now_ms],
            ).map_db()?;
            Ok(Some(DraftDto {
                chat_id: chat_id.0,
                topic_id: if topic_id > 0 { Some(topic_id) } else { None },
                text: text.to_string(),
                entities: serde_json::from_value(entities.clone()).unwrap_or_default(),
                reply_to_msg_id: reply_to, date_ms: now_ms,
            }))
        })
    }

    pub fn get_draft(&self, user_id: UserId, chat_id: ChatId, topic_id: i64) -> Result<Option<DraftDto>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT text, entities_json, reply_to_msg_id, date_ms FROM drafts WHERE user_id = ?1 AND chat_id = ?2 AND topic_id = ?3",
                params![user_id.0 as i64, chat_id.0, topic_id],
                |r| Ok(DraftDto {
                    chat_id: chat_id.0, topic_id: if topic_id > 0 { Some(topic_id) } else { None },
                    text: r.get(0)?, entities: serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or_default(),
                    reply_to_msg_id: r.get(2)?, date_ms: r.get(3)?,
                }),
            ).optional().map_db()
        })
    }

    pub fn clear_chat_history(&self, chat_id: ChatId, user_id: UserId) -> Result<usize> {
        self.clear_chat_history_with_ids(chat_id, user_id)
            .map(|ids| ids.len())
    }

    pub fn clear_chat_history_with_ids(
        &self,
        chat_id: ChatId,
        user_id: UserId,
    ) -> Result<Vec<MsgId>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let ids: Vec<MsgId> = {
                let mut stmt = tx
                    .prepare("SELECT message_id FROM messages WHERE chat_id = ?1 ORDER BY message_id ASC")
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                let rows = stmt
                    .query_map(params![chat_id.0], |r| Ok(MsgId(r.get(0)?)))
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| TelegramError::Database(e.to_string()))?
            };
            tx.execute("DELETE FROM messages WHERE chat_id = ?1", params![chat_id.0])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let _ = tx.execute(
                "DELETE FROM message_reactions WHERE chat_id = ?1",
                params![chat_id.0],
            );
            let _ = tx.execute(
                "DELETE FROM pinned_messages WHERE chat_id = ?1",
                params![chat_id.0],
            );
            tx.execute(
                "UPDATE chats SET last_message_id = 0, last_message_date_ms = 0, pinned_message_id = NULL, unread_count = 0, updated_at_ms = ?1 WHERE chat_id = ?2",
                params![now_ms, chat_id.0],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
            let _ = tx.execute(
                "UPDATE read_states SET unread_count = 0, updated_at_ms = ?1 WHERE chat_id = ?2 AND user_id = ?3",
                params![now_ms, chat_id.0, user_id.0 as i64],
            );
            Ok(ids)
        })
    }

    pub fn purge_expired_ttl_messages(&self, now_ms: i64) -> Result<Vec<(ChatId, Vec<MsgId>)>> {
        let expired_pairs: Vec<(i64, i64)> = self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"SELECT m.chat_id, m.message_id
                       FROM messages m
                       JOIN chats c ON c.chat_id = m.chat_id
                       WHERE c.message_ttl_seconds > 0
                         AND (m.date_ms + (c.message_ttl_seconds * 1000)) <= ?1
                       ORDER BY m.chat_id ASC, m.message_id ASC"#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![now_ms], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })?;

        if expired_pairs.is_empty() {
            return Ok(Vec::new());
        }

        self.pool.with_transaction(|tx| {
            let mut grouped: Vec<(ChatId, Vec<MsgId>)> = Vec::new();
            for (cid, mid) in expired_pairs {
                let n = tx
                    .execute(
                        "DELETE FROM messages WHERE chat_id = ?1 AND message_id = ?2",
                        params![cid, mid],
                    )
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                if n > 0 {
                    let _ = tx.execute(
                        "DELETE FROM message_reactions WHERE chat_id = ?1 AND message_id = ?2",
                        params![cid, mid],
                    );
                    let _ = tx.execute(
                        "DELETE FROM pinned_messages WHERE chat_id = ?1 AND message_id = ?2",
                        params![cid, mid],
                    );
                    if let Some((last_cid, list)) = grouped.last_mut() {
                        if last_cid.0 == cid {
                            list.push(MsgId(mid));
                            continue;
                        }
                    }
                    grouped.push((ChatId(cid), vec![MsgId(mid)]));
                }
            }

            for (chat_id, _) in &grouped {
                let latest: Option<(i64, i64)> = tx
                    .query_row(
                        r#"SELECT message_id, date_ms
                           FROM messages
                           WHERE chat_id = ?1 AND deleted_at_ms IS NULL
                           ORDER BY message_id DESC
                           LIMIT 1"#,
                        params![chat_id.0],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                let (last_mid, last_date) = latest.unwrap_or((0, 0));
                let pinned_ids = Self::query_pinned_ids_tx(tx, *chat_id)?;
                let latest_pin = pinned_ids.first().copied();
                tx.execute(
                    "UPDATE chats SET last_message_id = ?1, last_message_date_ms = ?2, pinned_message_id = ?3, updated_at_ms = ?4 WHERE chat_id = ?5",
                    params![last_mid, last_date, latest_pin, now_ms, chat_id.0],
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            }

            Ok(grouped)
        })
    }

    pub fn leave_or_delete_chat(&self, chat_id: ChatId, user_id: UserId) -> Result<bool> {
        self.pool.with_transaction(|tx| {
            let chat_info: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT chat_type, creator_id FROM chats WHERE chat_id = ?1",
                    params![chat_id.0],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let Some((chat_type, creator_id)) = chat_info else {
                return Ok(false);
            };
            if matches!(chat_type, 0 | 4 | 5) || creator_id == user_id.0 as i64 {
                let n = tx
                    .execute("DELETE FROM chats WHERE chat_id = ?1", params![chat_id.0])
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                Ok(n > 0)
            } else {
                let n = tx
                    .execute(
                        "DELETE FROM chat_members WHERE chat_id = ?1 AND user_id = ?2",
                        params![chat_id.0, user_id.0 as i64],
                    )
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                let _ = tx.execute(
                    "DELETE FROM user_chat_settings WHERE chat_id = ?1 AND user_id = ?2",
                    params![chat_id.0, user_id.0 as i64],
                );
                Ok(n > 0)
            }
        })
    }

    pub fn set_chat_ttl(&self, chat_id: ChatId, ttl_seconds: u32) -> Result<u32> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE chats SET message_ttl_seconds = ?1, updated_at_ms = ?2 WHERE chat_id = ?3",
                params![ttl_seconds as i64, now_ms, chat_id.0],
            )
            .map_db()?;
            Ok(ttl_seconds)
        })
    }

    pub fn get_chat_ttl(&self, chat_id: ChatId) -> Result<u32> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT message_ttl_seconds FROM chats WHERE chat_id = ?1",
                params![chat_id.0],
                |r| Ok(r.get::<_, i64>(0)?.max(0) as u32),
            )
            .optional()
            .map_db()
            .map(|o| o.unwrap_or(0))
        })
    }

    pub fn block_user(&self, owner: UserId, target: UserId) -> Result<BlockedUserDto> {
        if owner == target {
            return Err(TelegramError::Validation("Cannot block yourself".into()));
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM users WHERE user_id = ?1)",
                    params![target.0 as i64],
                    |r| r.get(0),
                )
                .map_db()?;
            if !exists {
                return Err(TelegramError::NotFound("Target user not found".into()));
            }
            conn.execute(
                r#"INSERT INTO blocked_users (owner_user_id, blocked_user_id, blocked_at_ms)
                   VALUES (?1, ?2, ?3)
                   ON CONFLICT(owner_user_id, blocked_user_id) DO UPDATE SET blocked_at_ms = excluded.blocked_at_ms"#,
                params![owner.0 as i64, target.0 as i64, now_ms],
            )
            .map_db()?;
            conn.query_row(
                r#"SELECT u.user_id, u.first_name, u.last_name, u.username, u.phone, b.blocked_at_ms
                   FROM blocked_users b JOIN users u ON u.user_id = b.blocked_user_id
                   WHERE b.owner_user_id = ?1 AND b.blocked_user_id = ?2"#,
                params![owner.0 as i64, target.0 as i64],
                |r| {
                    Ok(BlockedUserDto {
                        user_id: r.get::<_, i64>(0)? as u64,
                        first_name: r.get(1)?,
                        last_name: r.get(2)?,
                        username: r.get(3)?,
                        phone: r.get(4)?,
                        blocked_at_ms: r.get(5)?,
                    })
                },
            )
            .map_db()
        })
    }

    pub fn unblock_user(&self, owner: UserId, target: UserId) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let n = conn
                .execute(
                    "DELETE FROM blocked_users WHERE owner_user_id = ?1 AND blocked_user_id = ?2",
                    params![owner.0 as i64, target.0 as i64],
                )
                .map_db()?;
            Ok(n > 0)
        })
    }

    pub fn is_user_blocked(&self, owner: UserId, target: UserId) -> Result<bool> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM blocked_users WHERE owner_user_id = ?1 AND blocked_user_id = ?2)",
                params![owner.0 as i64, target.0 as i64],
                |r| r.get(0),
            )
            .map_db()
        })
    }

    pub fn list_blocked_users(&self, owner: UserId) -> Result<Vec<BlockedUserDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"SELECT u.user_id, u.first_name, u.last_name, u.username, u.phone, b.blocked_at_ms
                       FROM blocked_users b JOIN users u ON u.user_id = b.blocked_user_id
                       WHERE b.owner_user_id = ?1
                       ORDER BY b.blocked_at_ms DESC"#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![owner.0 as i64], |r| {
                    Ok(BlockedUserDto {
                        user_id: r.get::<_, i64>(0)? as u64,
                        first_name: r.get(1)?,
                        last_name: r.get(2)?,
                        username: r.get(3)?,
                        phone: r.get(4)?,
                        blocked_at_ms: r.get(5)?,
                    })
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn get_user_privacy_settings(&self, user_id: UserId) -> Result<UserPrivacySettingsDto> {
        self.pool.with_reader(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT settings_json FROM user_privacy_settings WHERE user_id = ?1",
                    params![user_id.0 as i64],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?;
            match raw {
                Some(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
                None => Ok(UserPrivacySettingsDto::default()),
            }
        })
    }

    pub fn save_user_privacy_settings(
        &self,
        user_id: UserId,
        settings: &UserPrivacySettingsDto,
    ) -> Result<UserPrivacySettingsDto> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let raw = serde_json::to_string(settings)
            .map_err(|e| TelegramError::Validation(e.to_string()))?;
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO user_privacy_settings (user_id, settings_json, updated_at_ms)
                   VALUES (?1, ?2, ?3)
                   ON CONFLICT(user_id) DO UPDATE SET
                       settings_json = excluded.settings_json,
                       updated_at_ms = excluded.updated_at_ms"#,
                params![user_id.0 as i64, raw, now_ms],
            )
            .map_db()?;
            Ok(settings.clone())
        })
    }
}
