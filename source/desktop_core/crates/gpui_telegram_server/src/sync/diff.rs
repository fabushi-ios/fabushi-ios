use crate::db::ServerDatabase;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_proto::message::MessageItem;
use gpui_telegram_proto::sync::{PtsUpdate, SyncGetDifferenceRequest, SyncGetDifferenceResponse, UpdateEvent};
use rusqlite::params;
use std::sync::Arc;

/// Service responsible for calculating catch-up updates (SyncGetDifference).
#[derive(Debug, Clone)]
pub struct SyncDifferenceService {
    db: Arc<ServerDatabase>,
}

impl SyncDifferenceService {
    pub fn new(db: Arc<ServerDatabase>) -> Self {
        Self { db }
    }

    /// Evaluates difference between client's requested PTS and current server journal state.
    pub fn get_difference(&self, req: &SyncGetDifferenceRequest) -> Result<SyncGetDifferenceResponse> {
        let current_pts = self.db.get_current_pts(req.chat_id)?;

        // Case 1: Client is fully caught up or ahead
        if req.pts >= current_pts {
            return Ok(SyncGetDifferenceResponse {
                chat_id: req.chat_id,
                new_messages: Vec::new(),
                other_updates: Vec::new(),
                pts: current_pts,
                current_pts,
                updates: Vec::new(),
                has_more: false,
                snapshot_required: false,
            });
        }

        // Case 2: Client's requested PTS is behind the journal floor (updates pruned)
        let floor_pts = self.db.get_journal_floor_pts(req.chat_id)?;
        if req.pts + 1 < floor_pts {
            return Ok(SyncGetDifferenceResponse {
                chat_id: req.chat_id,
                new_messages: Vec::new(),
                other_updates: Vec::new(),
                pts: current_pts,
                current_pts,
                updates: Vec::new(),
                has_more: true,
                snapshot_required: true,
            });
        }

        // Case 3: Fetch updates window
        let limit = if req.limit == 0 {
            req.pts_total_limit.unwrap_or(100).clamp(1, 100)
        } else {
            req.limit.clamp(1, 100)
        };

        let raw_rows = self.db.pool().with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT pts, pts_count, date_ms, event_type, payload_json
                    FROM pts_updates
                    WHERE chat_id = ?1 AND pts > ?2
                    ORDER BY pts ASC
                    LIMIT ?3
                    "#,
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;

            let rows = stmt
                .query_map(params![req.chat_id.0, req.pts as i64, limit as i64], |row| {
                    let pts: i64 = row.get(0)?;
                    let pts_count: i64 = row.get(1)?;
                    let date_ms: i64 = row.get(2)?;
                    let _event_type: String = row.get(3)?;
                    let payload_json: String = row.get(4)?;
                    Ok((pts as u64, pts_count as u32, date_ms, payload_json))
                })
                .map_err(|e| TelegramError::Database(e.to_string()))?;

            let mut list = Vec::new();
            for r in rows {
                list.push(r.map_err(|e| TelegramError::Database(e.to_string()))?);
            }
            Ok(list)
        })?;

        let mut new_messages: Vec<MessageItem> = Vec::new();
        let mut other_updates: Vec<UpdateEvent> = Vec::new();
        let mut pts_updates: Vec<PtsUpdate> = Vec::new();

        for (pts, pts_count, date_ms, payload_json) in raw_rows {
            let event: UpdateEvent = serde_json::from_str(&payload_json)
                .map_err(|e| TelegramError::Serialization(e.to_string()))?;

            match &event {
                UpdateEvent::NewMessage(msg) => {
                    new_messages.push(msg.clone());
                }
                other => {
                    other_updates.push(other.clone());
                }
            }

            pts_updates.push(PtsUpdate {
                chat_id: req.chat_id,
                pts,
                pts_count,
                event,
                timestamp_ms: date_ms,
            });
        }

        let new_pts = pts_updates.last().map(|u| u.pts).unwrap_or(req.pts);
        let has_more = new_pts < current_pts;

        Ok(SyncGetDifferenceResponse {
            chat_id: req.chat_id,
            new_messages,
            other_updates,
            pts: new_pts,
            current_pts,
            updates: pts_updates,
            has_more,
            snapshot_required: false,
        })
    }
}
