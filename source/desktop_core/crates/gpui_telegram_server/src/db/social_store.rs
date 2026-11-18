use crate::db::{ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_proto::contracts::{CallRecordDto, GifResultDto, StickerItemDto, StickerSetDto, StoryDto};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallSignalOutcome {
    Applied(CallRecordDto),
    NotFound,
    Forbidden(&'static str),
    Conflict {
        code: &'static str,
        message: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaMetadataDto {
    pub media_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_seconds: Option<u32>,
    pub waveform: Option<Vec<u8>>,
    pub thumbnail_data_url: Option<String>,
    pub is_voice: bool,
    pub is_video_note: bool,
    pub created_at_ms: i64,
}

impl ServerDatabase {
    pub fn save_media_file(&self, m: &MediaMetadataDto, uploader_id: UserId, chat_id: Option<ChatId>) -> Result<()> {
        self.pool.with_conn(|conn| {
            let wf = m.waveform.as_ref().map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()));
            conn.execute(
                r#"INSERT OR REPLACE INTO media_files
                (media_id, uploader_id, chat_id, filename, mime_type, size_bytes, sha256, width, height, duration_seconds, waveform_json, thumbnail_data_url, is_voice, is_video_note, created_at_ms)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
                params![
                    m.media_id, uploader_id.0 as i64, chat_id.map(|c| c.0), m.filename, m.mime_type,
                    m.size_bytes as i64, m.sha256, m.width.map(|v| v as i64), m.height.map(|v| v as i64),
                    m.duration_seconds.map(|v| v as i64), wf, m.thumbnail_data_url,
                    m.is_voice as i64, m.is_video_note as i64, m.created_at_ms,
                ],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn get_media_file(&self, media_id: &str) -> Result<Option<(MediaMetadataDto, UserId, Option<ChatId>)>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT media_id, uploader_id, chat_id, filename, mime_type, size_bytes, sha256, width, height, duration_seconds, waveform_json, thumbnail_data_url, is_voice, is_video_note, created_at_ms FROM media_files WHERE media_id = ?1",
                params![media_id],
                |r| {
                    let wf: Option<String> = r.get(10)?;
                    Ok((MediaMetadataDto {
                        media_id: r.get(0)?, filename: r.get(3)?, mime_type: r.get(4)?,
                        size_bytes: r.get::<_, i64>(5)? as u64, sha256: r.get(6)?,
                        width: r.get::<_, Option<i64>>(7)?.map(|v| v as u32),
                        height: r.get::<_, Option<i64>>(8)?.map(|v| v as u32),
                        duration_seconds: r.get::<_, Option<i64>>(9)?.map(|v| v as u32),
                        waveform: wf.and_then(|s| serde_json::from_str(&s).ok()),
                        thumbnail_data_url: r.get(11)?,
                        is_voice: r.get::<_, i64>(12)? != 0,
                        is_video_note: r.get::<_, i64>(13)? != 0,
                        created_at_ms: r.get(14)?,
                    }, UserId(r.get::<_, i64>(1)? as u64), r.get::<_, Option<i64>>(2)?.map(ChatId)))
                },
            ).optional().map_db()
        })
    }

    pub fn user_can_access_media(&self, user_id: UserId, media_id: &str) -> Result<bool> {
        let Some((_, uploader, chat_opt)) = self.get_media_file(media_id)? else { return Ok(true); };
        if uploader == user_id { return Ok(true); }
        if let Some(cid) = chat_opt { return self.is_user_in_chat(cid, user_id); }
        Ok(false)
    }

    pub fn seed_default_catalog_if_empty(&self) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let cnt: i64 = tx.query_row("SELECT COUNT(*) FROM sticker_sets", [], |r| r.get(0)).map_err(|e| TelegramError::Database(e.to_string()))?;
            if cnt == 0 {
                tx.execute("INSERT INTO sticker_sets (set_id, short_name, title, is_animated, is_video, is_official, created_at_ms) VALUES ('pack_durov', 'DurovAnimals', 'Durov Originals', 0, 0, 1, ?1)", params![now_ms]).map_err(|e| TelegramError::Database(e.to_string()))?;
                for (idx, (sid, emoji)) in [("stk_thumbs", "👍"), ("stk_rocket", "🚀"), ("stk_heart", "❤️"), ("stk_fire", "🔥")].iter().enumerate() {
                    tx.execute("INSERT INTO stickers (sticker_id, set_id, emoji, mime_type, width, height, file_id, lottie_json, sort_order) VALUES (?1, 'pack_durov', ?2, 'image/webp', 512, 512, ?3, NULL, ?4)",
                        params![sid, emoji, format!("builtin_{}", sid), idx as i64]).map_err(|e| TelegramError::Database(e.to_string()))?;
                }
            }
            let gif_cnt: i64 = tx.query_row("SELECT COUNT(*) FROM gifs_catalog", [], |r| r.get(0)).map_err(|e| TelegramError::Database(e.to_string()))?;
            if gif_cnt == 0 {
                for (id, title, tags) in [
                    ("gif_celebrate", "Party Celebration", "party happy celebration confetti dance"),
                    ("gif_rocket", "Rocket Launch", "rocket moon fast ship launch"),
                    ("gif_thumbsup", "Approval Thumbs Up", "ok yes approve thumbs good"),
                    ("gif_coding", "Fast Coding Cat", "code cat typing rust dev"),
                ] {
                    tx.execute("INSERT INTO gifs_catalog (gif_id, title, tags, width, height, duration_secs, mime_type, url, preview_url, created_at_ms) VALUES (?1, ?2, ?3, 320, 240, 3, 'image/gif', ?4, ?5, ?6)",
                        params![id, title, tags, format!("/api/v1/media/builtin_{}", id), format!("/api/v1/media/builtin_{}_thumb", id), now_ms]).map_err(|e| TelegramError::Database(e.to_string()))?;
                }
            }
            Ok(())
        })
    }

    pub fn list_sticker_sets(&self, user_id: UserId) -> Result<Vec<StickerSetDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT s.set_id, s.short_name, s.title, s.is_animated, s.is_video, EXISTS(SELECT 1 FROM user_sticker_sets u WHERE u.user_id = ?1 AND u.set_id = s.set_id) FROM sticker_sets s ORDER BY s.created_at_ms ASC"
            ).map_db()?;
            let raw_sets: Vec<(StickerSetDto, bool, bool)> = stmt.query_map(params![user_id.0 as i64], |r| Ok((StickerSetDto {
                set_id: r.get(0)?, short_name: r.get(1)?, title: r.get(2)?,
                count: 0, is_installed: r.get::<_, i64>(5)? != 0, stickers: Vec::new(),
            }, r.get::<_, i64>(3)? != 0, r.get::<_, i64>(4)? != 0))).map_db()?.collect::<std::result::Result<Vec<_>, _>>().map_db()?;
            let mut out = Vec::with_capacity(raw_sets.len());
            for (mut s, anim, vid) in raw_sets {
                let mut st = conn.prepare("SELECT sticker_id, set_id, emoji, mime_type, width, height, file_id FROM stickers WHERE set_id = ?1 ORDER BY sort_order ASC").map_db()?;
                s.stickers = st.query_map(params![s.set_id], |r| Ok(StickerItemDto {
                    id: r.get(0)?, set_id: r.get(1)?, emoji: r.get(2)?, mime_type: r.get(3)?,
                    width: r.get::<_, i64>(4)? as u32, height: r.get::<_, i64>(5)? as u32,
                    file_id: r.get(6)?, is_animated: anim, is_video: vid,
                })).map_db()?.collect::<std::result::Result<Vec<_>, _>>().map_db()?;
                s.count = s.stickers.len() as u32;
                out.push(s);
            }
            Ok(out)
        })
    }

    pub fn install_sticker_set(&self, user_id: UserId, set_id: &str, install: bool) -> Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sticker_sets WHERE set_id = ?1)", params![set_id], |r| r.get(0)).map_db()?;
            if !exists { return Ok(false); }
            if install {
                conn.execute("INSERT OR IGNORE INTO user_sticker_sets (user_id, set_id, installed_at_ms) VALUES (?1, ?2, ?3)", params![user_id.0 as i64, set_id, now_ms]).map_db()?;
            } else {
                conn.execute("DELETE FROM user_sticker_sets WHERE user_id = ?1 AND set_id = ?2", params![user_id.0 as i64, set_id]).map_db()?;
            }
            Ok(true)
        })
    }

    pub fn search_gifs(&self, query: &str, limit: usize) -> Result<Vec<GifResultDto>> {
        let q = format!("%{}%", query.trim().to_lowercase());
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare("SELECT gif_id, title, width, height, mime_type, url, preview_url FROM gifs_catalog WHERE lower(title) LIKE ?1 OR lower(tags) LIKE ?1 LIMIT ?2").map_db()?;
            let rows = stmt.query_map(params![q, limit as i64], |r| {
                let id: String = r.get(0)?;
                Ok(GifResultDto {
                    file_id: format!("builtin_{}", id), id, title: r.get(1)?,
                    width: r.get::<_, i64>(2)? as u32, height: r.get::<_, i64>(3)? as u32,
                    mime_type: r.get(4)?, url: r.get(5)?, preview_url: r.get(6)?,
                })
            }).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn create_story(&self, user_id: UserId, media_id: Option<&str>, media_url: Option<&str>, media_type: &str, caption: Option<&str>, privacy: &str, ttl_secs: i64) -> Result<StoryDto> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let expires_at_ms = now_ms + ttl_secs.clamp(60, 172_800) * 1000;
        let story_id = uuid::Uuid::new_v4().to_string();
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT INTO stories (story_id, user_id, media_id, media_url, media_type, caption, privacy, view_count, created_at_ms, expires_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9)",
                params![story_id, user_id.0 as i64, media_id, media_url, media_type, caption, privacy, now_ms, expires_at_ms],
            ).map_db()?;
            let (first_name, username): (String, Option<String>) = conn.query_row("SELECT first_name, username FROM users WHERE user_id = ?1", params![user_id.0 as i64], |r| Ok((r.get(0)?, r.get(1)?))).unwrap_or_else(|_| ("User".into(), None));
            Ok(StoryDto {
                story_id, user_id: user_id.0, username, first_name, media_id: media_id.map(str::to_string),
                media_type: media_type.to_string(), caption: caption.map(str::to_string), privacy: privacy.to_string(),
                views_count: 0, is_viewed: false, created_at_ms: now_ms, expires_at_ms,
            })
        })
    }

    pub fn list_active_stories(&self, viewer: UserId) -> Result<Vec<StoryDto>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT s.story_id, s.user_id, u.username, u.first_name, s.media_id, s.media_type, s.caption, s.privacy, s.view_count,
                          EXISTS(SELECT 1 FROM story_views sv WHERE sv.story_id = s.story_id AND sv.viewer_user_id = ?1), s.created_at_ms, s.expires_at_ms
                   FROM stories s JOIN users u ON u.user_id = s.user_id WHERE s.expires_at_ms > ?2 ORDER BY s.created_at_ms DESC"#
            ).map_db()?;
            let rows = stmt.query_map(params![viewer.0 as i64, now_ms], |r| Ok(StoryDto {
                story_id: r.get(0)?, user_id: r.get::<_, i64>(1)? as u64, username: r.get(2)?,
                first_name: r.get(3)?, media_id: r.get(4)?, media_type: r.get(5)?, caption: r.get(6)?,
                privacy: r.get(7)?, views_count: r.get::<_, i64>(8)? as u32,
                is_viewed: r.get::<_, i64>(9)? != 0, created_at_ms: r.get(10)?, expires_at_ms: r.get(11)?,
            })).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn view_story(&self, story_id: &str, viewer: UserId, reaction: Option<&str>) -> Result<u32> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM stories WHERE story_id = ?1 AND expires_at_ms > ?2)", params![story_id, now_ms], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            if !exists { return Err(TelegramError::NotFound("Story not found or expired".into())); }
            tx.execute("INSERT OR REPLACE INTO story_views (story_id, viewer_user_id, reaction, viewed_at_ms) VALUES (?1, ?2, ?3, ?4)",
                params![story_id, viewer.0 as i64, reaction, now_ms]).map_err(|e| TelegramError::Database(e.to_string()))?;
            let cnt: i64 = tx.query_row("SELECT COUNT(*) FROM story_views WHERE story_id = ?1", params![story_id], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute("UPDATE stories SET view_count = ?1 WHERE story_id = ?2", params![cnt, story_id])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok(cnt as u32)
        })
    }

    pub fn delete_story(&self, story_id: &str, owner: UserId) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let n = conn.execute("DELETE FROM stories WHERE story_id = ?1 AND user_id = ?2", params![story_id, owner.0 as i64]).map_db()?;
            Ok(n > 0)
        })
    }

    pub fn save_call(&self, c: &CallRecordDto, chat_id: Option<ChatId>) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO calls (call_id, chat_id, caller_id, callee_id, is_video, state, duration_secs, rating, feedback, started_at_ms, ended_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10)",
                params![c.call_id, chat_id.map(|v| v.0), c.caller_id as i64, c.callee_id as i64, c.is_video as i64, c.state, c.duration_secs as i64, c.rating.map(|v| v as i64), c.started_at_ms, c.ended_at_ms],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn get_call(&self, call_id: &str) -> Result<Option<CallRecordDto>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT call_id, caller_id, callee_id, is_video, state, duration_secs, rating, started_at_ms, ended_at_ms FROM calls WHERE call_id = ?1",
                params![call_id],
                |r| Ok(CallRecordDto {
                    call_id: r.get(0)?, caller_id: r.get::<_, i64>(1)? as u64, callee_id: r.get::<_, i64>(2)? as u64,
                    is_video: r.get::<_, i64>(3)? != 0, state: r.get(4)?, duration_secs: r.get::<_, i64>(5)? as u32,
                    rating: r.get::<_, Option<i64>>(6)?.map(|v| v as u8), started_at_ms: r.get(7)?, ended_at_ms: r.get(8)?,
                }),
            ).optional().map_db()
        })
    }

    pub fn update_call_state_and_rating(&self, call_id: &str, state: Option<&str>, ended_at_ms: Option<i64>, rating: Option<u8>, feedback: Option<&str>) -> Result<Option<CallRecordDto>> {
        self.pool.with_transaction(|tx| {
            if let Some(st) = state {
                tx.execute("UPDATE calls SET state = ?1 WHERE call_id = ?2", params![st, call_id]).map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            if let Some(end) = ended_at_ms {
                tx.execute("UPDATE calls SET ended_at_ms = ?1, duration_secs = MAX((?1 - started_at_ms) / 1000, 0) WHERE call_id = ?2", params![end, call_id])
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            if let Some(r) = rating {
                tx.execute("UPDATE calls SET rating = ?1, feedback = COALESCE(?2, feedback) WHERE call_id = ?3", params![r as i64, feedback, call_id])
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            tx.query_row(
                "SELECT call_id, caller_id, callee_id, is_video, state, duration_secs, rating, started_at_ms, ended_at_ms FROM calls WHERE call_id = ?1",
                params![call_id],
                |r| Ok(CallRecordDto {
                    call_id: r.get(0)?, caller_id: r.get::<_, i64>(1)? as u64, callee_id: r.get::<_, i64>(2)? as u64,
                    is_video: r.get::<_, i64>(3)? != 0, state: r.get(4)?, duration_secs: r.get::<_, i64>(5)? as u32,
                    rating: r.get::<_, Option<i64>>(6)?.map(|v| v as u8), started_at_ms: r.get(7)?, ended_at_ms: r.get(8)?,
                }),
            ).optional().map_err(|e| TelegramError::Database(e.to_string()))
        })
    }

    pub fn apply_call_signal(&self, call_id: &str, actor_user_id: u64, signal_type: &str, ended_at_ms: i64) -> Result<CallSignalOutcome> {
        self.pool.with_transaction(|tx| {
            let row: Option<CallRecordDto> = tx.query_row(
                "SELECT call_id, caller_id, callee_id, is_video, state, duration_secs, rating, started_at_ms, ended_at_ms FROM calls WHERE call_id = ?1",
                params![call_id],
                |r| Ok(CallRecordDto {
                    call_id: r.get(0)?,
                    caller_id: r.get::<_, i64>(1)? as u64,
                    callee_id: r.get::<_, i64>(2)? as u64,
                    is_video: r.get::<_, i64>(3)? != 0,
                    state: r.get(4)?,
                    duration_secs: r.get::<_, i64>(5)? as u32,
                    rating: r.get::<_, Option<i64>>(6)?.map(|v| v as u8),
                    started_at_ms: r.get(7)?,
                    ended_at_ms: r.get(8)?,
                }),
            ).optional().map_err(|e| TelegramError::Database(e.to_string()))?;

            let Some(rec) = row else {
                return Ok(CallSignalOutcome::NotFound);
            };

            if rec.caller_id != actor_user_id && rec.callee_id != actor_user_id {
                return Ok(CallSignalOutcome::Forbidden("Caller did not participate in this call"));
            }

            if rec.state == "ended" {
                return Ok(CallSignalOutcome::Conflict {
                    code: "call_ended",
                    message: "Cannot signal an ended call",
                });
            }

            if matches!(signal_type, "ringing" | "accept") && rec.callee_id != actor_user_id {
                return Ok(CallSignalOutcome::Forbidden("Only callee can acknowledge ringing or accept call"));
            }

            let (new_state, ended_at) = match signal_type {
                "ringing" => {
                    if matches!(rec.state.as_str(), "active" | "connected") {
                        return Ok(CallSignalOutcome::Conflict {
                            code: "invalid_state_transition",
                            message: "Cannot revert an active call to ringing",
                        });
                    }
                    (Some("ringing"), None)
                }
                "accept" | "answer" => (Some("active"), None),
                "hangup" | "end" | "decline" => (Some("ended"), Some(ended_at_ms)),
                _ => (None, None),
            };

            if let Some(st) = new_state {
                tx.execute("UPDATE calls SET state = ?1 WHERE call_id = ?2", params![st, call_id])
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            if let Some(end) = ended_at {
                tx.execute("UPDATE calls SET ended_at_ms = ?1, duration_secs = MAX((?1 - started_at_ms) / 1000, 0) WHERE call_id = ?2", params![end, call_id])
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
            }

            let updated = tx.query_row(
                "SELECT call_id, caller_id, callee_id, is_video, state, duration_secs, rating, started_at_ms, ended_at_ms FROM calls WHERE call_id = ?1",
                params![call_id],
                |r| Ok(CallRecordDto {
                    call_id: r.get(0)?,
                    caller_id: r.get::<_, i64>(1)? as u64,
                    callee_id: r.get::<_, i64>(2)? as u64,
                    is_video: r.get::<_, i64>(3)? != 0,
                    state: r.get(4)?,
                    duration_secs: r.get::<_, i64>(5)? as u32,
                    rating: r.get::<_, Option<i64>>(6)?.map(|v| v as u8),
                    started_at_ms: r.get(7)?,
                    ended_at_ms: r.get(8)?,
                }),
            ).map_err(|e| TelegramError::Database(e.to_string()))?;

            Ok(CallSignalOutcome::Applied(updated))
        })
    }

    pub fn list_calls_for_user(&self, user_id: UserId) -> Result<Vec<CallRecordDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT call_id, caller_id, callee_id, is_video, state, duration_secs, rating, started_at_ms, ended_at_ms FROM calls WHERE caller_id = ?1 OR callee_id = ?1 ORDER BY started_at_ms DESC LIMIT 100"
            ).map_db()?;
            let rows = stmt.query_map(params![user_id.0 as i64], |r| Ok(CallRecordDto {
                call_id: r.get(0)?, caller_id: r.get::<_, i64>(1)? as u64, callee_id: r.get::<_, i64>(2)? as u64,
                is_video: r.get::<_, i64>(3)? != 0, state: r.get(4)?, duration_secs: r.get::<_, i64>(5)? as u32,
                rating: r.get::<_, Option<i64>>(6)?.map(|v| v as u8), started_at_ms: r.get(7)?, ended_at_ms: r.get(8)?,
            })).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }
}
