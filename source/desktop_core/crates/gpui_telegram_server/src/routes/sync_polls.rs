use crate::routes::{broadcast_chat_pts, check_active_membership, err_resp, map_telegram_err, parse_json_object};
use crate::server::{AppState, AuthBearer};
use axum::{body::Bytes, extract::{Path, Query, State}, http::StatusCode, response::{IntoResponse, Json, Response}};
use gpui_telegram_core::types::{ChatId, MsgId};
use gpui_telegram_proto::contracts::{CreatePollHttpRequest, PollHttpResponse, ReactionHttpRequest, ReactionHttpResponse, ReadChatHttpResponse, VotePollHttpRequest};
use gpui_telegram_proto::message::{MessageContent, MessageEntity};
use gpui_telegram_proto::sync::{PtsUpdate, ReadHistoryEvent, SyncGetDifferenceRequest, UpdateEvent};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

async fn edit_impl(state: &AppState, auth: &AuthBearer, chat_id: i64, msg_id: i64, text: &str, entities: Vec<MessageEntity>) -> Response {
    if chat_id <= 0 || msg_id <= 0 { return err_resp(StatusCode::BAD_REQUEST, "invalid_id", "Positive chat_id and message_id required"); }
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 4096 { return err_resp(StatusCode::BAD_REQUEST, "invalid_text", "Edited text must be 1..=4096 chars"); }
    let (_chat, _m, is_admin) = match check_active_membership(state, ChatId(chat_id), auth.user_id) { Ok(c) => c, Err(r) => return r };
    let msg = match state.db.get_message(ChatId(chat_id), MsgId(msg_id)) {
        Ok(Some(m)) => m,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "not_found", "Message not found"),
        Err(e) => return map_telegram_err(e),
    };
    if msg.sender_id != auth.user_id && !is_admin { return err_resp(StatusCode::FORBIDDEN, "forbidden", "Cannot edit another user's message"); }
    let content = MessageContent::Text { text: trimmed.to_string(), entities };
    match state.message_pipeline.edit_message(ChatId(chat_id), MsgId(msg_id), content) {
        Ok(pts_update) => {
            state.search_indexer.index_message_nonblocking(crate::search::schema::SearchDocument::new(msg_id.to_string(), chat_id.to_string(), msg.sender_id.0.to_string(), msg.date_ms, trimmed.to_string(), None));
            state.search_indexer.commit_nonblocking();
            broadcast_chat_pts(state, ChatId(chat_id), pts_update.clone());
            let edited_at_ms = chrono::Utc::now().timestamp_millis();
            (StatusCode::OK, Json(json!({ "success": true, "chat_id": chat_id, "message_id": msg_id, "pts": pts_update.pts, "edited_at_ms": edited_at_ms, "text": trimmed }))).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn messages_edit_path(auth: AuthBearer, State(state): State<Arc<AppState>>, Path((chat_id, msg_id)): Path<(i64, i64)>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let entities: Vec<MessageEntity> = payload.get("entities").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    edit_impl(&state, &auth, chat_id, msg_id, text, entities).await
}

pub async fn messages_edit_legacy(auth: AuthBearer, State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let msg_id = payload.get("message_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let entities: Vec<MessageEntity> = payload.get("entities").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    edit_impl(&state, &auth, chat_id, msg_id, text, entities).await
}

async fn delete_impl(state: &AppState, auth: &AuthBearer, chat_id: i64, ids: Vec<i64>, revoke: bool) -> Response {
    if chat_id <= 0 || ids.is_empty() { return err_resp(StatusCode::BAD_REQUEST, "invalid_params", "Positive chat_id and message_ids required"); }
    let (_chat, _m, is_admin) = match check_active_membership(state, ChatId(chat_id), auth.user_id) { Ok(c) => c, Err(r) => return r };
    for &id in &ids {
        if let Ok(Some(msg)) = state.db.get_message(ChatId(chat_id), MsgId(id)) {
            if msg.sender_id != auth.user_id && !is_admin { return err_resp(StatusCode::FORBIDDEN, "forbidden", "Cannot delete another user's message"); }
        }
    }
    let msg_ids: Vec<MsgId> = ids.iter().copied().map(MsgId).collect();
    match state.message_pipeline.delete_messages(ChatId(chat_id), msg_ids, revoke) {
        Ok(pts_update) => {
            for &id in &ids { state.search_indexer.delete_message_nonblocking(id.to_string()); }
            state.search_indexer.commit_nonblocking();
            broadcast_chat_pts(state, ChatId(chat_id), pts_update.clone());
            (StatusCode::OK, Json(json!({ "success": true, "chat_id": chat_id, "deleted_ids": ids, "revoke": revoke, "pts": pts_update.pts }))).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn messages_delete_path(auth: AuthBearer, State(state): State<Arc<AppState>>, Path((chat_id, msg_id)): Path<(i64, i64)>, Query(q): Query<HashMap<String, String>>) -> Response {
    let revoke = q.get("revoke").map(|s| s != "false" && s != "0").unwrap_or(true);
    delete_impl(&state, &auth, chat_id, vec![msg_id], revoke).await
}

pub async fn messages_delete_legacy(auth: AuthBearer, State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let mut ids: Vec<i64> = payload.get("message_ids").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_i64()).collect()).unwrap_or_default();
    if ids.is_empty() { if let Some(single) = payload.get("message_id").and_then(|v| v.as_i64()) { ids.push(single); } }
    let revoke = payload.get("revoke").and_then(|v| v.as_bool()).unwrap_or(true);
    delete_impl(&state, &auth, chat_id, ids, revoke).await
}

async fn reaction_impl(state: &AppState, auth: &AuthBearer, chat_id: i64, msg_id: i64, emoji: &str, action: &str) -> Response {
    if emoji.trim().is_empty() { return err_resp(StatusCode::BAD_REQUEST, "invalid_emoji", "emoji is required"); }
    if let Err(r) = check_active_membership(state, ChatId(chat_id), auth.user_id) { return r; }
    match state.db.toggle_reaction_atomic(ChatId(chat_id), MsgId(msg_id), auth.user_id, emoji.trim(), action) {
        Ok((pts, details, counts)) => {
            let ev = UpdateEvent::ReactionUpdated(gpui_telegram_proto::sync::ReactionEvent::new(ChatId(chat_id), MsgId(msg_id), auth.user_id, counts.clone()));
            broadcast_chat_pts(state, ChatId(chat_id), PtsUpdate::new(
                ChatId(chat_id), pts, 1, ev, chrono::Utc::now().timestamp_millis(),
            ));
            (StatusCode::OK, Json(ReactionHttpResponse { success: true, chat_id, message_id: msg_id, pts, reactions: counts, details, status: None, reactions_authoritative: true })).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn messages_reaction_path(auth: AuthBearer, State(state): State<Arc<AppState>>, Path((chat_id, msg_id)): Path<(i64, i64)>, Json(req): Json<ReactionHttpRequest>) -> Response {
    reaction_impl(&state, &auth, chat_id, msg_id, &req.emoji, req.action.as_deref().unwrap_or("toggle")).await
}

pub async fn messages_reaction_legacy(auth: AuthBearer, State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let msg_id = payload.get("message_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let emoji = payload.get("emoji").and_then(|v| v.as_str()).unwrap_or("");
    let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or("toggle");
    reaction_impl(&state, &auth, chat_id, msg_id, emoji, action).await
}

pub async fn polls_create(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(chat_id): Path<i64>, Json(req): Json<CreatePollHttpRequest>) -> Response {
    let (c, m_opt, _) = match check_active_membership(&state, ChatId(chat_id), auth.user_id) { Ok(c) => c, Err(r) => return r };
    if let Some(m) = m_opt {
        if m.role.trim().eq_ignore_ascii_case("restricted") && (m.banned_rights & 1) != 0 {
            return err_resp(StatusCode::FORBIDDEN, "forbidden", "User is restricted from sending messages");
        }
    }
    if matches!(c.chat_type, 0 | 4) {
        if let Ok(members) = state.db.get_chat_members(ChatId(chat_id)) {
            for peer_uid in members {
                if peer_uid != auth.user_id
                    && (state.db.is_user_blocked(peer_uid, auth.user_id).unwrap_or(false)
                        || state.db.is_user_blocked(auth.user_id, peer_uid).unwrap_or(false))
                {
                    return err_resp(
                        StatusCode::FORBIDDEN,
                        "user_is_blocked",
                        "Cannot send messages in a blocked direct chat",
                    );
                }
            }
        }
    }
    match state.db.create_poll_atomic(ChatId(chat_id), auth.user_id, &req.question, &req.options, req.is_anonymous, req.multiple_choice, req.is_quiz, req.correct_option_id, req.solution.as_deref(), req.topic_id) {
        Ok((msg_id, pts, poll, item)) => {
            state.search_indexer.index_message_nonblocking(crate::search::schema::SearchDocument::new(
                msg_id.0.to_string(), chat_id.to_string(), auth.user_id.0.to_string(), item.date.timestamp_millis(), format!("[Poll: {}]", req.question), Some("poll".into()),
            ));
            state.search_indexer.commit_nonblocking();
            broadcast_chat_pts(&state, ChatId(chat_id), PtsUpdate::new(
                ChatId(chat_id), pts, 1, UpdateEvent::NewMessage(item), chrono::Utc::now().timestamp_millis(),
            ));
            (StatusCode::CREATED, Json(json!({ "success": true, "chat_id": chat_id, "message_id": msg_id.0, "pts": pts, "poll": poll }))).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn polls_get(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(poll_id): Path<i64>) -> Response {
    match state.db.get_poll_for_user(poll_id, auth.user_id) {
        Ok(Some((cid, poll))) => {
            if let Err(r) = check_active_membership(&state, cid, auth.user_id) { return r; }
            (StatusCode::OK, Json(json!({ "chat_id": cid.0, "poll": poll }))).into_response()
        }
        Ok(None) => err_resp(StatusCode::NOT_FOUND, "not_found", "Poll not found"),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn polls_vote(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(poll_id): Path<i64>, Json(req): Json<VotePollHttpRequest>) -> Response {
    match state.db.get_poll_for_user(poll_id, auth.user_id) {
        Ok(Some((cid, _))) => {
            if let Err(r) = check_active_membership(&state, cid, auth.user_id) { return r; }
        }
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "not_found", "Poll not found"),
        Err(e) => return map_telegram_err(e),
    }
    match state.db.vote_poll_atomic(poll_id, auth.user_id, &req.option_ids) {
        Ok((cid, mid, pts, poll)) => {
            broadcast_chat_pts(&state, cid, PtsUpdate::new(
                cid, pts, 1, UpdateEvent::PollUpdated { chat_id: cid, message_id: mid, poll: poll.clone() }, chrono::Utc::now().timestamp_millis(),
            ));
            (StatusCode::OK, Json(PollHttpResponse { success: true, pts, poll })).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn polls_close(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(poll_id): Path<i64>) -> Response {
    match state.db.get_poll_for_user(poll_id, auth.user_id) {
        Ok(Some((cid, _))) => {
            if let Err(r) = check_active_membership(&state, cid, auth.user_id) { return r; }
        }
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "not_found", "Poll not found"),
        Err(e) => return map_telegram_err(e),
    }
    match state.db.close_poll_atomic(poll_id, auth.user_id) {
        Ok((cid, mid, pts, poll)) => {
            broadcast_chat_pts(&state, cid, PtsUpdate::new(
                cid, pts, 1, UpdateEvent::PollUpdated { chat_id: cid, message_id: mid, poll: poll.clone() }, chrono::Utc::now().timestamp_millis(),
            ));
            (StatusCode::OK, Json(PollHttpResponse { success: true, pts, poll })).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

async fn read_impl(state: &AppState, auth: &AuthBearer, chat_id: i64, max_id: i64, topic_id: i64) -> Response {
    if chat_id <= 0 || max_id < 0 { return err_resp(StatusCode::BAD_REQUEST, "invalid_read_cursor", "Valid chat_id and max_id required"); }
    if let Err(r) = check_active_membership(state, ChatId(chat_id), auth.user_id) { return r; }
    match state.db.mark_chat_read_atomic(ChatId(chat_id), auth.user_id, MsgId(max_id), topic_id) {
        Ok((inbox_max, outbox_max, unread, pts, senders)) => {
            let ev_in = UpdateEvent::ReadHistory(ReadHistoryEvent::new(ChatId(chat_id), inbox_max, false, unread));
            state.gateway_hub.broadcast_to_user(auth.user_id, gpui_telegram_proto::ws::WsServerFrame::Update(ev_in));
            let ev_out = UpdateEvent::ReadHistory(ReadHistoryEvent::new(ChatId(chat_id), inbox_max, true, 0));
            for s in senders {
                state.gateway_hub.broadcast_to_user(s, gpui_telegram_proto::ws::WsServerFrame::Update(ev_out.clone()));
            }
            (StatusCode::OK, Json(ReadChatHttpResponse {
                success: true, chat_id, pts, read_inbox_max_id: inbox_max.0, read_outbox_max_id: outbox_max.0, unread_count: unread,
            })).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn chats_read_path(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(chat_id): Path<i64>, body: Bytes) -> Response {
    let payload = parse_json_object(&body).unwrap_or_else(|_| json!({}));
    let max_id = payload.get("max_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let topic_id = payload.get("topic_id").and_then(|v| v.as_i64()).unwrap_or(0);
    read_impl(&state, &auth, chat_id, max_id, topic_id).await
}

pub async fn messages_read_legacy(auth: AuthBearer, State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let max_id = payload.get("max_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let topic_id = payload.get("topic_id").and_then(|v| v.as_i64()).unwrap_or(0);
    read_impl(&state, &auth, chat_id, max_id, topic_id).await
}

pub async fn sync_get_difference_clean(auth: AuthBearer, State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = if body.is_empty() { json!({}) } else { match parse_json_object(&body) { Ok(v) => v, Err(r) => return r } };
    let pts = payload.get("pts").and_then(|v| v.as_u64()).unwrap_or(0);
    let limit = payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(100).clamp(1, 200) as u32;
    if let Some(cid) = payload.get("chat_id").and_then(|v| v.as_i64()).filter(|&c| c > 0) {
        if let Err(r) = check_active_membership(&state, ChatId(cid), auth.user_id) { return r; }
        let req = SyncGetDifferenceRequest::new(ChatId(cid), pts, limit);
        return match state.pts_engine.get_difference(&req) {
            Ok(resp) => (StatusCode::OK, Json(json!({
                "chat_id": resp.chat_id.0, "pts": resp.pts, "new_pts": resp.pts, "current_pts": resp.current_pts,
                "has_more": resp.has_more, "messages": resp.new_messages, "other_updates": resp.other_updates,
                "updates": resp.updates, "snapshot_required": resp.snapshot_required,
            }))).into_response(),
            Err(e) => map_telegram_err(e),
        };
    }
    let chat_ids = state.db.list_user_member_chat_ids(auth.user_id).unwrap_or_default();
    let per_chat_pts: HashMap<i64, u64> = payload.get("chat_pts").and_then(|v| v.as_object()).map(|m| {
        m.iter().filter_map(|(k, v)| Some((k.parse::<i64>().ok()?, v.as_u64()?))).collect()
    }).unwrap_or_default();
    let mut diffs = Vec::new();
    for cid in chat_ids {
        let from = per_chat_pts.get(&cid.0).copied().unwrap_or(pts);
        if let Ok(resp) = state.pts_engine.get_difference(&SyncGetDifferenceRequest::new(cid, from, limit)) {
            if !resp.updates.is_empty() || resp.current_pts > from || resp.snapshot_required {
                diffs.push(json!({
                    "chat_id": cid.0, "pts": resp.pts, "current_pts": resp.current_pts,
                    "has_more": resp.has_more, "messages": resp.new_messages,
                    "other_updates": resp.other_updates, "updates": resp.updates,
                    "snapshot_required": resp.snapshot_required,
                }));
            }
        }
    }
    (StatusCode::OK, Json(json!({ "chats": diffs }))).into_response()
}
