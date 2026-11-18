pub mod acp;
pub mod actions;
pub mod analytics_wallet;
pub mod auth_advanced;
pub mod auth_factors;
pub mod bots_miniapps;
pub mod chat_extras;
pub mod chat_lifecycle;
pub mod computers;
pub mod devices;
pub mod e2ee;
pub mod fabushi_miniapp_api;
pub mod hermes;
pub mod magpie_api;
pub mod media;
pub mod messages;
pub mod social;
pub mod sync_polls;
pub mod tailnet;
pub mod telegram_bot_api;
pub mod users;

use crate::db::schema::{ChatMemberRecord, ChatRecord, UserRecord};
use crate::server::AppState;
use axum::{
    body::Bytes,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::{ChatId, PeerId, UserId};
use gpui_telegram_proto::contracts::UserProfileDto;
use gpui_telegram_proto::message::DeleteTombstone;
use gpui_telegram_proto::sync::{PtsUpdate, UpdateEvent};
use gpui_telegram_proto::ws::WsServerFrame;
use serde_json::{json, Value};

pub fn err_resp(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(json!({ "error": code, "message": msg }))).into_response()
}

pub fn parse_json_object(bytes: &Bytes) -> Result<Value, Response> {
    if bytes.is_empty() {
        return Err(err_resp(StatusCode::BAD_REQUEST, "empty_body", "Request body cannot be empty"));
    }
    let val: Value = serde_json::from_slice(bytes)
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, "invalid_json", &format!("Malformed JSON: {e}")))?;
    if !val.is_object() {
        return Err(err_resp(StatusCode::BAD_REQUEST, "invalid_payload", "Expected JSON object"));
    }
    Ok(val)
}

pub fn map_telegram_err(err: TelegramError) -> Response {
    match err {
        TelegramError::Auth(m) | TelegramError::AuthFailed(m) => {
            err_resp(StatusCode::UNAUTHORIZED, "unauthorized", &m)
        }
        TelegramError::NotFound(m) => err_resp(StatusCode::NOT_FOUND, "not_found", &m),
        TelegramError::PermissionDenied(m) => err_resp(StatusCode::FORBIDDEN, "forbidden", &m),
        TelegramError::Validation(m) => {
            if m.contains("nonce conflict") {
                err_resp(StatusCode::CONFLICT, "nonce_conflict", &m)
            } else {
                err_resp(StatusCode::BAD_REQUEST, "validation_error", &m)
            }
        }
        TelegramError::Database(_) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", "Internal storage error"),
        other => err_resp(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", &other.to_string()),
    }
}

pub fn check_active_membership(
    state: &AppState,
    chat_id: ChatId,
    user_id: UserId,
) -> Result<(ChatRecord, Option<ChatMemberRecord>, bool), Response> {
    let chat = match state.db.get_chat(chat_id) {
        Ok(Some(c)) => c,
        Ok(None) => return Err(err_resp(StatusCode::NOT_FOUND, "not_found", "Chat not found")),
        Err(_) => return Err(err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", "Failed to query chat")),
    };
    let member = match state.db.get_chat_member(chat_id, user_id) {
        Ok(m) => m,
        Err(_) => return Err(err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", "Failed to query membership")),
    };
    if chat.creator_id == user_id {
        return Ok((chat, member, true));
    }
    let Some(m) = member else {
        return Err(err_resp(StatusCode::FORBIDDEN, "forbidden", "Caller is not a member of this chat"));
    };
    if crate::db::ServerDatabase::is_inactive_member_role(&m.role) {
        return Err(err_resp(StatusCode::FORBIDDEN, "forbidden", "Caller membership is not active"));
    }
    let is_admin = crate::db::ServerDatabase::is_admin_member_role(&m.role);
    Ok((chat, Some(m), is_admin))
}

pub fn broadcast_chat_pts(state: &AppState, chat_id: ChatId, pts_update: PtsUpdate) {
    state.gateway_hub.broadcast_to_chat_and_members(chat_id, None, WsServerFrame::PtsPush(pts_update));
}

pub fn broadcast_chat_event(state: &AppState, chat_id: ChatId, ev: UpdateEvent) {
    state.gateway_hub.broadcast_to_chat_and_members(chat_id, None, WsServerFrame::Update(ev));
}

pub async fn enforce_ttl_expiry(state: &AppState) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let Ok(purged) = state.db.purge_expired_ttl_messages(now_ms) else {
        return;
    };
    if purged.is_empty() {
        return;
    }
    for (chat_id, msg_ids) in purged {
        for &mid in &msg_ids {
            state
                .search_indexer
                .delete_chat_message_nonblocking(chat_id.0.to_string(), mid.0.to_string());
        }
        let tombstone = DeleteTombstone::new(PeerId::Chat(chat_id), msg_ids, true);
        broadcast_chat_event(state, chat_id, UpdateEvent::DeleteMessages(tombstone));
    }
    let _ = state.search_indexer.commit().await;
}

pub fn user_record_to_profile(state: &AppState, u: &UserRecord) -> UserProfileDto {
    let emoji_status = state.db.get_user_emoji_status(u.user_id).ok().flatten();
    let online = state.gateway_hub.user_connection_count(u.user_id) > 0;
    let is_premium = state
        .db
        .get_user_privacy_settings(u.user_id)
        .map(|s| s.is_premium)
        .unwrap_or(false);
    UserProfileDto {
        user_id: u.user_id.raw(),
        username: u.username.clone(),
        first_name: u.first_name.clone(),
        last_name: u.last_name.clone(),
        bio: u.bio.clone(),
        phone: u.phone.clone(),
        avatar_media_id: u.profile_photo_file_id.clone(),
        photo_file_id: u.profile_photo_file_id.clone(),
        emoji_status,
        is_bot: u.is_bot,
        is_verified: u.is_verified,
        is_premium,
        status: if online { "online".into() } else { "offline".into() },
        last_seen_ms: Some(u.updated_at_ms),
    }
}

pub fn issue_persistent_session(
    state: &AppState,
    user: &UserRecord,
    device_name: &str,
) -> gpui_telegram_proto::contracts::AuthResponse {
    let created_at = chrono::DateTime::from_timestamp_millis(user.created_at_ms).unwrap_or_else(chrono::Utc::now);
    let username = user
        .username
        .clone()
        .unwrap_or_else(|| format!("user_{}", user.user_id.0));
    let account_gen = user.account_generation.max(1);
    let ttl_secs = 30 * 24 * 3600u64;

    if let Ok((sess_rec, token)) = state.session_store.issue_session(
        user.user_id.raw(),
        device_name,
        "gpui_desktop",
        env!("CARGO_PKG_VERSION"),
        account_gen,
        ttl_secs,
    ) {
        let expires_at = chrono::DateTime::from_timestamp_millis(sess_rec.expires_at_ms)
            .unwrap_or_else(chrono::Utc::now);
        return gpui_telegram_proto::contracts::AuthResponse {
            user_id: user.user_id.raw(),
            username,
            session_id: sess_rec.session_id,
            token,
            first_name: Some(user.first_name.clone()),
            last_name: user.last_name.clone(),
            account_generation: Some(sess_rec.account_generation),
            created_at: Some(created_at.to_rfc3339()),
            expires_at: Some(expires_at.to_rfc3339()),
        };
    }

    let mem_user = crate::auth::session_manager::UserRecord {
        user_id: user.user_id,
        username: username.clone(),
        first_name: user.first_name.clone(),
        last_name: user.last_name.clone(),
        password_hash: user.password_hash.clone().unwrap_or_default(),
        created_at,
    };
    state.session_manager.insert_user(mem_user);
    let (session, token) = state.session_manager.create_session(user.user_id, device_name, chrono::Duration::days(30));
    gpui_telegram_proto::contracts::AuthResponse {
        user_id: user.user_id.raw(),
        username,
        session_id: session.session_id,
        token,
        first_name: Some(user.first_name.clone()),
        last_name: user.last_name.clone(),
        account_generation: Some(account_gen),
        created_at: Some(created_at.to_rfc3339()),
        expires_at: Some(session.expires_at.to_rfc3339()),
    }
}
