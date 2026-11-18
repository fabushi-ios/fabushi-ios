use crate::db::schema::{ChatMemberRecord, ChatRecord, UserRecord};
use crate::routes::{check_active_membership, err_resp, map_telegram_err, parse_json_object};
use crate::server::{extract_message_content_text, is_welcome_bot_chat, AppState, AuthBearer};
use axum::{body::Bytes, extract::{Path, State}, http::StatusCode, response::{IntoResponse, Json, Response}};
use chrono::Utc;
use gpui_telegram_core::types::{ChatId, MsgId, TopicId, UserId};
use serde_json::{json, Map, Value};
use std::sync::Arc;

fn chat_kind_str(chat_type: u8, is_bot: bool) -> &'static str {
    if is_bot || chat_type == 4 { "bot" } else {
        match chat_type { 0 => "direct", 2 => "supergroup", 3 => "channel", 5 => "saved", _ => "group" }
    }
}

fn exposed_chat_username(chat: &ChatRecord) -> Option<String> {
    if is_welcome_bot_chat(chat.chat_id) {
        Some("telegram_official_bot".to_string())
    } else {
        chat.username.clone()
    }
}

fn format_peer_display_name(user: &UserRecord) -> String {
    match &user.last_name {
        Some(ln) if !ln.trim().is_empty() => format!("{} {}", user.first_name, ln.trim()),
        _ => user.first_name.clone(),
    }
}

fn resolve_direct_peer_and_bot(
    state: &AppState,
    viewer_id: UserId,
    chat: &ChatRecord,
    members: &[UserId],
) -> (Option<UserRecord>, bool) {
    let is_direct_or_bot = matches!(chat.chat_type, 0 | 4) || is_welcome_bot_chat(chat.chat_id);
    if !is_direct_or_bot {
        return (None, false);
    }
    let mut candidates = Vec::new();
    for &member_uid in members {
        if member_uid != viewer_id {
            if let Ok(Some(u)) = state.db.get_user(member_uid) {
                candidates.push(u);
            }
        }
    }
    if candidates.is_empty() && is_welcome_bot_chat(chat.chat_id) {
        if let Ok(Some(u)) = state.db.get_user(UserId(777_000)) {
            candidates.push(u);
        }
    }
    let peer = candidates
        .iter()
        .find(|u| u.is_bot)
        .cloned()
        .or_else(|| candidates.into_iter().next());

    let peer_is_bot = peer.as_ref().is_some_and(|u| u.is_bot);
    let peer_username_bot = peer
        .as_ref()
        .and_then(|u| u.username.as_deref())
        .is_some_and(|un| un.to_ascii_lowercase().ends_with("bot"));
    let chat_username_bot = chat
        .username
        .as_deref()
        .is_some_and(|un| un.to_ascii_lowercase().ends_with("bot"));
    let is_bot = is_welcome_bot_chat(chat.chat_id)
        || chat.chat_type == 4
        || peer_is_bot
        || peer_username_bot
        || chat_username_bot;
    (peer, is_bot)
}

pub async fn chats_list_clean(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    crate::routes::enforce_ttl_expiry(&state).await;
    let chats = match state.db.list_user_chats(auth.user_id) {
        Ok(c) => c,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", &e.to_string()),
    };
    let summaries: Vec<Value> = chats.into_iter().map(|c| {
        let members = state.db.get_chat_members(c.chat_id).unwrap_or_default();
        let (peer, is_bot_chat) = resolve_direct_peer_and_bot(&state, auth.user_id, &c, &members);
        let (is_archived, is_pinned, is_muted, mute_until_ms) = state.db.get_user_chat_setting(auth.user_id, c.chat_id).unwrap_or((false, false, false, None));
        let rs = state.db.get_read_state(c.chat_id, auth.user_id, TopicId(0)).ok().flatten();
        let draft = state.db.get_draft(auth.user_id, c.chat_id, 0).ok().flatten();
        let latest_msg = if c.last_message_id.0 > 0 {
            state.db.list_messages_with_topic(c.chat_id, None, None, 1).ok().and_then(|mut l| l.pop()).and_then(|rec| {
                crate::messaging::pipeline::MessagePipeline::record_to_item(&rec).ok().map(|item| extract_message_content_text(&item.content))
            }).filter(|s| !s.is_empty())
        } else { None };
        let peer_user_id = peer.as_ref().map(|u| u.user_id.0);
        let peer_username = peer.as_ref().and_then(|u| u.username.clone());
        let peer_display_name = peer.as_ref().map(format_peer_display_name);
        let peer_phone = peer.as_ref().and_then(|u| u.phone.clone());
        let peer_bio = peer.as_ref().and_then(|u| u.bio.clone());
        let is_blocked = peer
            .as_ref()
            .and_then(|u| state.db.is_user_blocked(auth.user_id, u.user_id).ok())
            .unwrap_or(false);
        let ttl_seconds = state.db.get_chat_ttl(c.chat_id).unwrap_or(0);
        let mut obj = Map::new();
        obj.insert("chat_id".into(), json!(c.chat_id.0));
        obj.insert("id".into(), json!(c.chat_id.0));
        obj.insert("title".into(), json!(c.title));
        obj.insert("about".into(), json!(c.about));
        obj.insert("username".into(), json!(exposed_chat_username(&c)));
        obj.insert("chat_type".into(), json!(c.chat_type));
        obj.insert("kind".into(), json!(chat_kind_str(c.chat_type, is_bot_chat)));
        obj.insert("creator_id".into(), json!(c.creator_id.0));
        obj.insert("is_group".into(), json!(matches!(c.chat_type, 1 | 2)));
        obj.insert("is_channel".into(), json!(c.chat_type == 3));
        obj.insert("is_bot".into(), json!(is_bot_chat));
        obj.insert("peer_user_id".into(), json!(peer_user_id));
        obj.insert("peer_username".into(), json!(peer_username));
        obj.insert("peer_display_name".into(), json!(peer_display_name));
        obj.insert("peer_phone".into(), json!(peer_phone));
        obj.insert("peer_bio".into(), json!(peer_bio));
        obj.insert("is_blocked".into(), json!(is_blocked));
        obj.insert("message_ttl_seconds".into(), json!(ttl_seconds));
        obj.insert("is_forum".into(), json!(c.is_forum));
        obj.insert("is_archived".into(), json!(is_archived));
        obj.insert("is_pinned".into(), json!(is_pinned));
        obj.insert("is_muted".into(), json!(is_muted));
        obj.insert("mute_until_ms".into(), json!(mute_until_ms));
        obj.insert("pts".into(), json!(c.pts));
        obj.insert("last_message_id".into(), json!(c.last_message_id.0));
        obj.insert("last_message_date_ms".into(), json!(c.last_message_date_ms));
        obj.insert("unread_count".into(), json!(rs.as_ref().map(|r| r.unread_count).unwrap_or(0)));
        obj.insert("read_inbox_max_id".into(), json!(rs.as_ref().map(|r| r.inbox_max_id.0).unwrap_or(0)));
        obj.insert("read_outbox_max_id".into(), json!(rs.as_ref().map(|r| r.outbox_max_id.0).unwrap_or(0)));
        obj.insert("member_count".into(), json!(members.len()));
        obj.insert("pinned_message_id".into(), json!(c.pinned_message_id.map(|m| m.0)));
        if let Some(txt) = latest_msg { obj.insert("last_message".into(), json!(txt)); }
        if let Some(d) = draft { obj.insert("draft".into(), serde_json::to_value(d).unwrap_or(Value::Null)); }
        Value::Object(obj)
    }).collect();
    let total = summaries.len();
    (StatusCode::OK, Json(json!({ "chats": summaries, "total": total }))).into_response()
}

pub async fn chats_create_strict(auth: AuthBearer, State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let peer_user_id = payload
        .get("peer_user_id")
        .or_else(|| payload.get("user_id"))
        .and_then(|v| v.as_u64())
        .map(UserId);
    let mut chat_type: u8 = match payload.get("chat_type").or_else(|| payload.get("kind")) {
        None | Some(Value::Null) => if let Some(p) = peer_user_id { if p == auth.user_id { 5 } else { 0 } } else { 1 },
        Some(Value::Number(n)) => match n.as_u64() { Some(v @ 0..=5) => v as u8, _ => return err_resp(StatusCode::BAD_REQUEST, "invalid_chat_type", "chat_type must be 0..=5") },
        Some(Value::String(s)) => match s.to_ascii_lowercase().as_str() {
            "direct" | "private" | "dm" => 0, "group" => 1, "supergroup" => 2, "channel" => 3, "bot" => 4, "saved" | "saved_messages" => 5,
            _ => return err_resp(StatusCode::BAD_REQUEST, "invalid_chat_type", "Unsupported chat_type"),
        },
        Some(_) => return err_resp(StatusCode::BAD_REQUEST, "invalid_chat_type", "chat_type must be a string or integer"),
    };
    let title = match payload.get("title") {
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() || t.chars().count() > 255 {
                return err_resp(StatusCode::BAD_REQUEST, "invalid_title", "title must be 1..=255 characters");
            }
            t.to_string()
        }
        Some(Value::Null) | None => {
            if chat_type == 5 { "Saved Messages".to_string() }
            else if let Some(peer) = peer_user_id {
                state.db.get_user(peer).ok().flatten().map(|u| u.first_name).unwrap_or_else(|| format!("User {}", peer.0))
            } else {
                return err_resp(StatusCode::BAD_REQUEST, "invalid_title", "title is required and must be a non-empty string");
            }
        }
        Some(_) => return err_resp(StatusCode::BAD_REQUEST, "invalid_title", "title must be a string"),
    };
    let about = payload.get("about").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let username = payload.get("username").and_then(|v| v.as_str()).map(|s| s.trim().trim_start_matches('@').to_string()).filter(|s| !s.is_empty());
    let is_forum = payload.get("is_forum").and_then(|v| v.as_bool()).unwrap_or(false);
    if is_forum && chat_type == 1 { chat_type = 2; }

    let mut member_ids: Vec<UserId> = Vec::new();
    if let Some(peer) = peer_user_id {
        if peer != auth.user_id {
            match state.db.get_user(peer) {
                Ok(Some(u)) => { if u.is_bot && chat_type == 0 { chat_type = 4; } member_ids.push(peer); }
                Ok(None) => return err_resp(StatusCode::NOT_FOUND, "peer_not_found", "Target peer user not found"),
                Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", &e.to_string()),
            }
        }
    }
    if let Some(arr) = payload.get("member_ids").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(id) = v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).map(UserId) {
                if id != auth.user_id && !member_ids.contains(&id) { member_ids.push(id); }
            }
        }
    }
    if let Some(arr) = payload.get("member_usernames").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(un) = v.as_str().map(|s| s.trim().trim_start_matches('@')).filter(|s| !s.is_empty()) {
                if let Ok(Some(u)) = state.db.get_user_by_username(un) {
                    if u.user_id != auth.user_id && !member_ids.contains(&u.user_id) { member_ids.push(u.user_id); }
                }
            }
        }
    }
    // Reuse existing DM if direct/saved chat with same peer already exists
    if matches!(chat_type, 0 | 4 | 5) && member_ids.len() <= 1 {
        if let Ok(existing) = state.db.list_user_chats(auth.user_id) {
            for ch in existing {
                if ch.chat_type == chat_type {
                    let ms = state.db.get_chat_members(ch.chat_id).unwrap_or_default();
                    let matches = match (chat_type, member_ids.first()) {
                        (5, None) => ms.len() == 1 && ms.contains(&auth.user_id),
                        (_, Some(p)) => ms.len() == 2 && ms.contains(&auth.user_id) && ms.contains(p),
                        _ => false,
                    };
                    if matches {
                        return (StatusCode::CREATED, Json(chat_detail_json(&state, auth.user_id, &ch))).into_response();
                    }
                }
            }
        }
    }
    let now_ms = Utc::now().timestamp_millis();
    let chat_id = match state.db.next_chat_id() {
        Ok(id) => id,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", &e.to_string()),
    };
    let peer_type = match chat_type { 0 | 4 | 5 => 1, 2 | 3 => 3, _ => 2 };
    let rec = ChatRecord {
        chat_id, chat_type, peer_type, title, username, about: about.clone(), creator_id: auth.user_id,
        is_forum, flags: 0, photo_file_id: None, slowmode_seconds: 0, linked_chat_id: None, pinned_message_id: None,
        pts: 0, last_message_id: MsgId(0), last_message_date_ms: 0, unread_count: 0, account_generation: 1,
        created_at_ms: now_ms, updated_at_ms: now_ms,
    };
    let mut members_to_insert = Vec::with_capacity(1 + member_ids.len());
    members_to_insert.push(ChatMemberRecord {
        chat_id, user_id: auth.user_id, role: "owner".into(), permissions_bitmask: 0xFFFF,
        admin_title: None, admin_rights: 0xFFFF, banned_rights: 0, until_date_ms: None, joined_at_ms: now_ms, invited_by: None, updated_at_ms: now_ms,
    });
    for &m in &member_ids {
        members_to_insert.push(ChatMemberRecord {
            chat_id, user_id: m, role: "member".into(), permissions_bitmask: 0xFFFF,
            admin_title: None, admin_rights: 0, banned_rights: 0, until_date_ms: None, joined_at_ms: now_ms, invited_by: Some(auth.user_id), updated_at_ms: now_ms,
        });
    }
    if let Err(e) = state.db.create_chat_with_members(&rec, &members_to_insert) {
        return map_telegram_err(e);
    }
    if chat_type != 5 {
        if let Ok(priv_settings) = state.db.get_user_privacy_settings(auth.user_id) {
            if priv_settings.global_ttl_seconds > 0 {
                let _ = state.db.set_chat_ttl(chat_id, priv_settings.global_ttl_seconds);
            }
        }
    }
    state.gateway_hub.subscribe_chat(chat_id.0, auth.user_id);
    for &m in &member_ids {
        state.gateway_hub.subscribe_chat(chat_id.0, m);
    }
    (StatusCode::CREATED, Json(chat_detail_json(&state, auth.user_id, &rec))).into_response()
}

pub(crate) fn chat_detail_json(state: &AppState, user_id: UserId, chat: &ChatRecord) -> Value {
    let members = state.db.get_chat_members(chat.chat_id).unwrap_or_default();
    let members_count = if members.is_empty() { 1 } else { members.len() };
    let (peer, is_bot) = resolve_direct_peer_and_bot(state, user_id, chat, &members);
    let (is_archived, is_pinned, is_muted, mute_until_ms) = state
        .db
        .get_user_chat_setting(user_id, chat.chat_id)
        .unwrap_or((false, false, false, None));
    let rs = state.db.get_read_state(chat.chat_id, user_id, TopicId(0)).ok().flatten();
    let draft = state.db.get_draft(user_id, chat.chat_id, 0).ok().flatten();
    let peer_user_id = peer.as_ref().map(|u| u.user_id.0);
    let peer_username = peer.as_ref().and_then(|u| u.username.clone());
    let peer_display_name = peer.as_ref().map(format_peer_display_name);
    let peer_phone = peer.as_ref().and_then(|u| u.phone.clone());
    let peer_bio = peer.as_ref().and_then(|u| u.bio.clone());
    let is_blocked = peer
        .as_ref()
        .and_then(|u| state.db.is_user_blocked(user_id, u.user_id).ok())
        .unwrap_or(false);
    let ttl_seconds = state.db.get_chat_ttl(chat.chat_id).unwrap_or(0);
    json!({
        "chat_id": chat.chat_id.0, "id": chat.chat_id.0, "title": chat.title, "about": chat.about, "username": exposed_chat_username(chat),
        "chat_type": chat.chat_type, "kind": chat_kind_str(chat.chat_type, is_bot), "creator_id": chat.creator_id.0,
        "is_forum": chat.is_forum, "photo_file_id": chat.photo_file_id, "members_count": members_count, "member_count": members_count,
        "is_group": matches!(chat.chat_type, 1 | 2), "is_channel": chat.chat_type == 3, "is_bot": is_bot,
        "peer_user_id": peer_user_id, "peer_username": peer_username, "peer_display_name": peer_display_name,
        "peer_phone": peer_phone, "peer_bio": peer_bio,
        "is_blocked": is_blocked, "message_ttl_seconds": ttl_seconds,
        "is_archived": is_archived, "is_pinned": is_pinned, "is_muted": is_muted, "mute_until_ms": mute_until_ms,
        "pts": chat.pts, "last_message_id": chat.last_message_id.0, "last_message_date_ms": chat.last_message_date_ms,
        "unread_count": rs.as_ref().map(|r| r.unread_count).unwrap_or(0),
        "read_inbox_max_id": rs.as_ref().map(|r| r.inbox_max_id.0).unwrap_or(0),
        "read_outbox_max_id": rs.as_ref().map(|r| r.outbox_max_id.0).unwrap_or(0),
        "pinned_message_id": chat.pinned_message_id.map(|m| m.0), "draft": draft,
        "created_at_ms": chat.created_at_ms,
    })
}

pub async fn chats_get_scoped(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(chat_id): Path<i64>) -> Response {
    if chat_id <= 0 { return err_resp(StatusCode::BAD_REQUEST, "invalid_chat_id", "chat_id must be a positive integer"); }
    let chat = match state.db.get_chat(ChatId(chat_id)) {
        Ok(Some(c)) => c,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "not_found", "Chat not found"),
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", &e.to_string()),
    };
    let is_member = chat.creator_id == auth.user_id || state.db.get_chat_member(ChatId(chat_id), auth.user_id).ok().flatten()
        .map(|m| !crate::db::ServerDatabase::is_inactive_member_role(&m.role)).unwrap_or(false);
    if !is_member { return err_resp(StatusCode::NOT_FOUND, "not_found", "Chat not found"); }
    (StatusCode::OK, Json(chat_detail_json(&state, auth.user_id, &chat))).into_response()
}

pub async fn chats_delete_chat(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(chat_id): Path<i64>) -> Response {
    let (chat, _member, is_admin) = match check_active_membership(&state, ChatId(chat_id), auth.user_id) {
        Ok(ctx) => ctx, Err(r) => return r,
    };
    if !matches!(chat.chat_type, 0 | 4 | 5) && chat.creator_id != auth.user_id && !is_admin {
        return err_resp(StatusCode::FORBIDDEN, "forbidden", "Only chat owner or admin can delete this chat");
    }
    match state.db.delete_chat(ChatId(chat_id)) {
        Ok(true) => {
            state.gateway_hub.unsubscribe_chat(chat_id, auth.user_id);
            for uid in state.gateway_hub.get_chat_subscribers(chat_id) {
                state.gateway_hub.unsubscribe_chat(chat_id, uid);
            }
            (StatusCode::OK, Json(json!({ "deleted": true, "chat_id": chat_id }))).into_response()
        }
        Ok(false) => err_resp(StatusCode::NOT_FOUND, "not_found", "Chat not found"),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, "database_error", &e.to_string()),
    }
}
