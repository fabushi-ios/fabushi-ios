use crate::db::schema::MessageRecord;
use crate::messaging::pipeline::MessagePipeline;
use crate::routes::{broadcast_chat_pts, check_active_membership, err_resp, map_telegram_err, parse_json_object};
use crate::search::schema::SearchDocument;
use crate::server::{
    detect_target_bots_in_chat, extract_message_content_text,
    maybe_trigger_bot_reply_with_reply_to, AppState, AuthBearer,
};
use axum::{body::Bytes, extract::{Path, Query, State}, http::StatusCode, response::{IntoResponse, Json, Response}};
use chrono::{DateTime, Utc};
use gpui_telegram_core::types::{ChatId, MessageFlags, MsgId, PeerId, TopicId, UserId};
use gpui_telegram_proto::message::{ForwardInfo, InlineKeyboardButton, MediaItem, MessageContent, MessageEntity, MessageItem, ReplyHeader, VoiceItem};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

fn compute_sender_identity(state: &AppState, sender_id: UserId) -> (String, String, bool) {
    if let Ok(Some(u)) = state.db.get_user(sender_id) {
        let name = match &u.last_name {
            Some(ln) if !ln.trim().is_empty() => format!("{} {}", u.first_name.trim(), ln.trim()),
            _ => u.first_name.trim().to_string(),
        };
        let display = if name.is_empty() {
            u.username
                .clone()
                .unwrap_or_else(|| format!("User {}", sender_id.0))
        } else {
            name
        };
        let mut initials = String::new();
        for part in display.split_whitespace().take(2) {
            if let Some(ch) = part.chars().next() {
                initials.push(ch.to_ascii_uppercase());
            }
        }
        if initials.is_empty() {
            initials.push('U');
        }
        (display, initials, u.is_bot)
    } else {
        (format!("User {}", sender_id.0), "U".to_string(), false)
    }
}

pub fn format_message_json(state: &AppState, rec: &MessageRecord, viewer_id: UserId) -> Value {
    let item = MessagePipeline::record_to_item(rec).ok();
    let mut text = item.as_ref().map(|i| extract_message_content_text(&i.content)).unwrap_or_default();
    let mut content_val = item.as_ref().and_then(|i| serde_json::to_value(&i.content).ok()).unwrap_or(Value::Null);
    if let Some(MessageContent::Poll(p)) = item.as_ref().map(|i| &i.content) {
        if let Ok(Some((_, live_poll))) = state.db.get_poll_for_user(p.poll_id as i64, viewer_id) {
            text = format!("[Poll: {}]", live_poll.question);
            content_val = serde_json::to_value(MessageContent::Poll(live_poll)).unwrap_or(content_val);
        }
    }
    let (sender_name, sender_initials, is_bot_sender) = compute_sender_identity(state, rec.sender_id);
    let reactions = state.db.list_message_reactions(rec.chat_id, rec.message_id, viewer_id).unwrap_or_default();
    let (ikb_raw, sched_ms, views) = state.db.get_message_extras(rec.chat_id, rec.message_id).unwrap_or((None, None, 0));
    let ikb: Option<Value> = ikb_raw.and_then(|s| serde_json::from_str(&s).ok());
    let ttl_seconds = state.db.get_chat_ttl(rec.chat_id).unwrap_or(0);
    let (ttl_opt, expires_at_ms) = if ttl_seconds > 0 {
        (Some(ttl_seconds), Some(rec.date_ms + (ttl_seconds as i64 * 1000)))
    } else {
        (None, None)
    };
    let date_iso = DateTime::from_timestamp_millis(rec.date_ms).unwrap_or_else(Utc::now).to_rfc3339();
    json!({
        "message_id": rec.message_id.0, "id": rec.message_id.0, "chat_id": rec.chat_id.0,
        "sender_id": rec.sender_id.0, "sender_name": sender_name, "sender_initials": sender_initials,
        "is_bot_sender": is_bot_sender,
        "topic_id": rec.topic_id.map(|t| t.0), "reply_to": rec.reply_to_id.map(|m| m.0),
        "pts": rec.pts, "date_ms": rec.date_ms, "date": date_iso, "text": text, "content": content_val,
        "is_outgoing": rec.sender_id == viewer_id, "is_edited": rec.edit_date_ms.is_some(), "edit_date_ms": rec.edit_date_ms,
        "reactions": reactions, "reply_to_info": item.as_ref().and_then(|i| i.reply_to.as_ref()),
        "forward_info": item.as_ref().and_then(|i| i.forward_info.as_ref()),
        "inline_keyboard": ikb, "bot_keyboard": ikb, "scheduled_at_ms": sched_ms, "views": views,
        "is_silent": (rec.flags_bitmask & 0x20) != 0,
        "ttl_seconds": ttl_opt,
        "expires_at_ms": expires_at_ms,
    })
}

fn parse_pos_opt(params: &HashMap<String, String>, key: &str) -> Result<Option<i64>, Response> {
    match params.get(key) {
        None => Ok(None),
        Some(s) => match s.parse::<i64>() {
            Ok(v) if v > 0 => Ok(Some(v)),
            _ => Err(err_resp(StatusCode::BAD_REQUEST, "invalid_param", &format!("{key} must be a positive integer"))),
        },
    }
}

pub async fn history_for_chat(state: &AppState, viewer_id: UserId, chat_id: i64, params: HashMap<String, String>) -> Response {
    if chat_id <= 0 { return err_resp(StatusCode::BAD_REQUEST, "invalid_chat_id", "chat_id must be a positive integer"); }
    let limit = match params.get("limit") {
        None => 50u32,
        Some(s) => match s.parse::<u32>() {
            Ok(v) if (1..=100).contains(&v) => v,
            _ => return err_resp(StatusCode::BAD_REQUEST, "invalid_limit", "limit must be an integer between 1 and 100"),
        },
    };
    let before_id = match parse_pos_opt(&params, "before_id") { Ok(v) => v, Err(r) => return r };
    let offset_id = match parse_pos_opt(&params, "offset_id") { Ok(v) => v, Err(r) => return r };
    let after_id = match parse_pos_opt(&params, "after_id") { Ok(v) => v, Err(r) => return r };
    let topic_id = match parse_pos_opt(&params, "topic_id") { Ok(v) => v.map(TopicId), Err(r) => return r };
    if let (Some(b), Some(o)) = (before_id, offset_id) {
        if b != o { return err_resp(StatusCode::BAD_REQUEST, "conflicting_pagination", "Conflicting before_id and offset_id"); }
    }
    let before_eff = before_id.or(offset_id);
    if before_eff.is_some() && after_id.is_some() {
        return err_resp(StatusCode::BAD_REQUEST, "conflicting_pagination", "Cannot specify both before_id and after_id");
    }
    let (chat, _, _) = match check_active_membership(state, ChatId(chat_id), viewer_id) { Ok(c) => c, Err(r) => return r };
    crate::routes::enforce_ttl_expiry(state).await;
    match state.db.list_messages_page(ChatId(chat_id), topic_id, before_eff, after_id, limit) {
        Ok((rows, has_more)) => {
            let messages: Vec<Value> = rows.iter().map(|r| format_message_json(state, r, viewer_id)).collect();
            (StatusCode::OK, Json(json!({ "chat_id": chat_id, "messages": messages, "has_more": has_more, "pts": chat.pts, "current_pts": chat.pts }))).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn messages_history_clean(auth: AuthBearer, State(state): State<Arc<AppState>>, Query(params): Query<HashMap<String, String>>) -> Response {
    let chat_id = match params.get("chat_id").and_then(|s| s.parse::<i64>().ok()) {
        Some(id) if id > 0 => id,
        _ => return err_resp(StatusCode::BAD_REQUEST, "invalid_chat_id", "Positive chat_id query parameter is required"),
    };
    history_for_chat(&state, auth.user_id, chat_id, params).await
}

pub async fn chats_messages_history(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(chat_id): Path<i64>, Query(params): Query<HashMap<String, String>>) -> Response {
    history_for_chat(&state, auth.user_id, chat_id, params).await
}

fn validate_msg_content(c: &MessageContent) -> Result<(), Response> {
    match c {
        MessageContent::Text { text, entities } => {
            if text.trim().is_empty() || text.chars().count() > 4096 {
                return Err(err_resp(StatusCode::BAD_REQUEST, "invalid_text", "Message text must be 1..=4096 characters"));
            }
            let u16_len = text.encode_utf16().count();
            for e in entities {
                if (e.offset as usize).saturating_add(e.length as usize) > u16_len {
                    return Err(err_resp(StatusCode::BAD_REQUEST, "invalid_entities", "Message entity offset/length out of UTF-16 bounds"));
                }
            }
            Ok(())
        }
        MessageContent::Photo { file_id, .. } | MessageContent::Video { file_id, .. } | MessageContent::Audio { file_id, .. } | MessageContent::Document { file_id, .. } => {
            if file_id.trim().is_empty() { Err(err_resp(StatusCode::BAD_REQUEST, "invalid_media", "file_id cannot be empty")) } else { Ok(()) }
        }
        MessageContent::Media(m) => if m.file_id.trim().is_empty() { Err(err_resp(StatusCode::BAD_REQUEST, "invalid_media", "file_id cannot be empty")) } else { Ok(()) },
        MessageContent::Voice(v) => if v.file_id.trim().is_empty() { Err(err_resp(StatusCode::BAD_REQUEST, "invalid_media", "file_id cannot be empty")) } else { Ok(()) },
        MessageContent::VideoNote(v) => if v.file_id.trim().is_empty() { Err(err_resp(StatusCode::BAD_REQUEST, "invalid_media", "file_id cannot be empty")) } else { Ok(()) },
        MessageContent::Poll(p) => if p.question.trim().is_empty() || p.options.len() < 2 { Err(err_resp(StatusCode::BAD_REQUEST, "invalid_poll", "Poll requires question and >= 2 options")) } else { Ok(()) },
        MessageContent::Todo(t) => if t.title.trim().is_empty() { Err(err_resp(StatusCode::BAD_REQUEST, "invalid_todo", "Todo title cannot be empty")) } else { Ok(()) },
        MessageContent::Invoice(i) => if i.title.trim().is_empty() { Err(err_resp(StatusCode::BAD_REQUEST, "invalid_invoice", "Invoice title cannot be empty")) } else { Ok(()) },
        MessageContent::GenerativeCard(g) => if g.card_id.trim().is_empty() || g.title.trim().is_empty() { Err(err_resp(StatusCode::BAD_REQUEST, "invalid_generative_card", "GenerativeCard card_id and title cannot be empty")) } else { Ok(()) },
        MessageContent::Encrypted(env) => {
            if env.version != 1 {
                return Err(err_resp(StatusCode::BAD_REQUEST, "invalid_e2ee_version", "Unsupported EncryptedEnvelope version"));
            }
            if env.sender_device_id.trim().is_empty()
                || env.sender_identity_key.trim().is_empty()
                || env.sender_signing_key.trim().is_empty()
            {
                return Err(err_resp(StatusCode::BAD_REQUEST, "invalid_e2ee_sender", "EncryptedEnvelope requires non-empty sender device and key fields"));
            }
            match &env.kind {
                gpui_telegram_proto::e2ee::EncryptedPayloadKind::PairwiseFanout { recipients }
                | gpui_telegram_proto::e2ee::EncryptedPayloadKind::SenderKeyDistribution { recipients } => {
                    if recipients.is_empty()
                        || recipients.iter().any(|r| r.recipient_device_id.trim().is_empty() || r.ciphertext.trim().is_empty())
                    {
                        return Err(err_resp(StatusCode::BAD_REQUEST, "invalid_e2ee_payload", "EncryptedEnvelope recipients must be non-empty with valid ciphertexts"));
                    }
                }
                gpui_telegram_proto::e2ee::EncryptedPayloadKind::GroupSenderKey(g) => {
                    if g.ciphertext.trim().is_empty() || g.signature.trim().is_empty() {
                        return Err(err_resp(StatusCode::BAD_REQUEST, "invalid_e2ee_payload", "GroupSenderKey payload requires ciphertext and signature"));
                    }
                }
            }
            Ok(())
        }
    }
}

pub async fn send_for_chat(state: &Arc<AppState>, auth_user: UserId, chat_id_raw: i64, payload: Value, success_status: StatusCode) -> Response {
    if chat_id_raw <= 0 { return err_resp(StatusCode::BAD_REQUEST, "invalid_chat_id", "chat_id must be a positive integer"); }
    let chat_id = ChatId(chat_id_raw);
    let client_nonce = match payload.get("client_nonce") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => match n.as_u64() {
            Some(v) if v >= 1 && v <= i64::MAX as u64 => Some(v),
            _ => return err_resp(StatusCode::BAD_REQUEST, "invalid_nonce", "client_nonce must be between 1 and i64::MAX"),
        },
        Some(_) => return err_resp(StatusCode::BAD_REQUEST, "invalid_nonce", "client_nonce must be a positive integer"),
    };
    let reply_a = match payload.get("reply_to") { None | Some(Value::Null) => None, Some(v) => match v.as_i64() { Some(id) if id > 0 => Some(id), _ => return err_resp(StatusCode::BAD_REQUEST, "invalid_reply_to", "reply_to must be > 0") } };
    let reply_b = match payload.get("reply_to_id") { None | Some(Value::Null) => None, Some(v) => match v.as_i64() { Some(id) if id > 0 => Some(id), _ => return err_resp(StatusCode::BAD_REQUEST, "invalid_reply_to", "reply_to_id must be > 0") } };
    if let (Some(a), Some(b)) = (reply_a, reply_b) { if a != b { return err_resp(StatusCode::BAD_REQUEST, "conflicting_reply", "Conflicting reply_to and reply_to_id"); } }
    let reply_to = reply_a.or(reply_b).map(MsgId);
    let topic_id = match payload.get("topic_id") { None | Some(Value::Null) => None, Some(v) => match v.as_i64() { Some(t) if t > 0 => Some(TopicId(t)), _ => return err_resp(StatusCode::BAD_REQUEST, "invalid_topic_id", "topic_id must be > 0") } };

    let (chat, member_opt, _) = match check_active_membership(state, chat_id, auth_user) { Ok(c) => c, Err(r) => return r };
    if let Some(m) = member_opt {
        if m.role.trim().eq_ignore_ascii_case("restricted") && (m.banned_rights & 1) != 0 {
            return err_resp(StatusCode::FORBIDDEN, "forbidden", "User is restricted from sending messages");
        }
    }
    if matches!(chat.chat_type, 0 | 4) {
        if let Ok(members) = state.db.get_chat_members(chat_id) {
            for peer_uid in members {
                if peer_uid != auth_user
                    && (state.db.is_user_blocked(peer_uid, auth_user).unwrap_or(false)
                        || state.db.is_user_blocked(auth_user, peer_uid).unwrap_or(false))
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
    let send_as_peer_id = match payload.get("send_as_peer_id") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_i64() {
            Some(pid) if pid > 0 => Some(pid as u64),
            _ => {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "invalid_send_as",
                    "Caller is not allowed to send as the requested peer",
                );
            }
        },
    };
    if let Some(peer_id) = send_as_peer_id {
        if peer_id != auth_user.0 {
            let allowed = check_active_membership(state, ChatId(peer_id as i64), auth_user)
                .map(|(c, _, is_adm)| {
                    matches!(c.chat_type, 2 | 3) && (c.creator_id == auth_user || is_adm)
                })
                .unwrap_or(false);
            if !allowed {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "invalid_send_as",
                    "Caller is not allowed to send as the requested peer",
                );
            }
        }
    }
    let mut forward_info: Option<ForwardInfo> = None;
    let mut forwarded_content: Option<MessageContent> = None;
    if let (Some(fc), Some(fm)) = (payload.get("forward_from_chat_id").and_then(|v| v.as_i64()), payload.get("forward_from_msg_id").and_then(|v| v.as_i64())) {
        if check_active_membership(state, ChatId(fc), auth_user).is_err() {
            return err_resp(StatusCode::FORBIDDEN, "forbidden", "Cannot forward from inaccessible chat");
        }
        if let Ok(Some(orig)) = state.db.get_message(ChatId(fc), MsgId(fm)) {
            let from_name = state.db.get_user(orig.sender_id).ok().flatten().map(|u| u.first_name);
            forward_info = Some(ForwardInfo { from_id: PeerId::User(orig.sender_id), from_name, date: orig.date_ms / 1000, original_message_id: Some(MsgId(fm)) });
            forwarded_content = MessagePipeline::record_to_item(&orig).ok().map(|i| i.content);
        }
    }
    let content = if let Some(cval) = payload.get("content").filter(|v| !v.is_null()) {
        match serde_json::from_value::<MessageContent>(cval.clone()) { Ok(c) => c, Err(e) => return err_resp(StatusCode::BAD_REQUEST, "invalid_content", &format!("Invalid content: {e}")) }
    } else if let Some(mid) = payload.get("media_id").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
        let caption = payload.get("text").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        match state.db.get_media_file(mid) {
            Ok(Some((m, _, _))) if m.is_voice => MessageContent::Voice(VoiceItem { file_id: m.media_id, duration_seconds: m.duration_seconds.unwrap_or(1), waveform: m.waveform.unwrap_or_default(), mime_type: m.mime_type, size_bytes: m.size_bytes }),
            Ok(Some((m, _, _))) => MessageContent::Media(MediaItem { file_id: m.media_id, file_name: Some(m.filename), mime_type: m.mime_type, size_bytes: m.size_bytes, width: m.width, height: m.height, duration_seconds: m.duration_seconds, thumbnail: None, caption }),
            _ => return err_resp(StatusCode::BAD_REQUEST, "media_not_found", "Referenced media_id not found"),
        }
    } else if let Some(txt) = payload.get("text").and_then(|v| v.as_str()) {
        let entities: Vec<MessageEntity> = payload.get("entities").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        MessageContent::Text { text: txt.trim().to_string(), entities }
    } else if let Some(fc) = forwarded_content { fc } else {
        return err_resp(StatusCode::BAD_REQUEST, "missing_content", "text, content, media_id, or forward source is required");
    };
    if let Err(r) = validate_msg_content(&content) { return r; }
    if let MessageContent::Encrypted(ref env) = content {
        if env.sender_user_id != auth_user.0 {
            return err_resp(
                StatusCode::FORBIDDEN,
                "e2ee_sender_mismatch",
                "EncryptedEnvelope sender_user_id must match authenticated user",
            );
        }
    }

    let to_peer = if chat.chat_type == 0 { PeerId::User(auth_user) } else { PeerId::Chat(chat_id) };
    let mut item = MessageItem::new(MsgId(0), chat_id, auth_user, to_peer, content.clone());
    item.topic_id = topic_id;
    item.client_nonce = client_nonce;
    item.forward_info = forward_info;
    item.flags = MessageFlags { is_out: true, is_silent: payload.get("silent").and_then(|v| v.as_bool()).unwrap_or(false), ..Default::default() };
    if let Some(r_id) = reply_to {
        let mut rh = ReplyHeader::new(r_id);
        rh.quote_text = payload.get("quote_text").and_then(|v| v.as_str()).map(String::from);
        item.reply_to = Some(rh);
    }
    let pts_update = match state.message_pipeline.process_incoming_message_strict(item.clone()) {
        Ok(u) => u,
        Err(e) => return map_telegram_err(e),
    };
    let msg_id = match &pts_update.event {
        gpui_telegram_proto::sync::UpdateEvent::NewMessage(m)
        | gpui_telegram_proto::sync::UpdateEvent::EditMessage(m) => m.id,
        _ => MsgId(0),
    };
    let text_preview = extract_message_content_text(&content);
    let parsed_ikb = payload
        .get("inline_keyboard")
        .or_else(|| payload.get("bot_keyboard"))
        .and_then(|v| serde_json::from_value::<Vec<Vec<InlineKeyboardButton>>>(v.clone()).ok());
    let (ikb_json, sched_ms) = (
        parsed_ikb.as_ref().and_then(|k| serde_json::to_string(k).ok()),
        payload.get("scheduled_at_ms").and_then(|v| v.as_i64()),
    );
    if ikb_json.is_some() || sched_ms.is_some() {
        let _ = state.db.save_message_extras(chat_id, msg_id, ikb_json.as_deref(), sched_ms);
    }
    let is_encrypted = matches!(content, MessageContent::Encrypted(_));
    let date_ms = state.db.get_message(chat_id, msg_id).ok().flatten().map(|r| r.date_ms).unwrap_or_else(|| Utc::now().timestamp_millis());
    let date_iso = DateTime::from_timestamp_millis(date_ms).unwrap_or_else(Utc::now).to_rfc3339();
    let ttl_seconds = state.db.get_chat_ttl(chat_id).unwrap_or(0);
    let (ttl_opt, expires_at_ms) = if ttl_seconds > 0 {
        (Some(ttl_seconds), Some(date_ms + (ttl_seconds as i64 * 1000)))
    } else {
        (None, None)
    };
    if pts_update.pts_count > 0 {
        if !is_encrypted {
            state.search_indexer.index_message_nonblocking(SearchDocument::new(msg_id.0.to_string(), chat_id.0.to_string(), auth_user.0.to_string(), date_ms, text_preview.clone(), None));
            state.search_indexer.commit_nonblocking();
            let hm_doc_id = format!("{}:{}", chat_id.0, msg_id.0);
            state.hybrid_memory.index_message_or_file(&hm_doc_id, chat_id.0, "message", &text_preview, vec![]);
        }
        broadcast_chat_pts(state, chat_id, pts_update.clone());
        if !is_encrypted {
            crate::routes::telegram_bot_api::record_incoming_user_message_for_bot(
                state,
                chat_id,
                auth_user,
                msg_id,
                &text_preview,
                date_ms,
            );
            let targets = detect_target_bots_in_chat(state, chat_id, auth_user, &text_preview, reply_to);
            let has_ai_target = targets.iter().any(|buid| {
                !state
                    .db
                    .get_bot_by_user_id(buid.0 as i64)
                    .ok()
                    .flatten()
                    .is_some_and(|b| b.is_standard_bot)
            });
            if has_ai_target {
                let weak_state = Arc::downgrade(state);
                let tp = text_preview.clone();
                tokio::spawn(async move {
                    if let Some(state_bg) = weak_state.upgrade() {
                        maybe_trigger_bot_reply_with_reply_to(
                            &state_bg,
                            chat_id,
                            auth_user,
                            &tp,
                            reply_to,
                        )
                        .await;
                    }
                });
            }
        }
    }
    let (sender_name, sender_initials, _) = compute_sender_identity(state, auth_user);
    (success_status, Json(json!({
        "message_id": msg_id.0, "id": msg_id.0, "pts": pts_update.pts, "pts_count": pts_update.pts_count,
        "duplicate": pts_update.pts_count == 0, "date_ms": date_ms, "date": date_iso,
        "sender_id": auth_user.0, "sender_name": sender_name, "sender_initials": sender_initials,
        "chat_id": chat_id.0, "reply_to": reply_to.map(|m| m.0),
        "inline_keyboard": parsed_ikb, "bot_keyboard": parsed_ikb,
        "client_nonce": client_nonce, "text": text_preview, "status": "sent",
        "send_as_peer_id": send_as_peer_id,
        "ttl_seconds": ttl_opt,
        "expires_at_ms": expires_at_ms,
    }))).into_response()
}

pub async fn messages_send_clean(auth: AuthBearer, State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let chat_id = match payload.get("chat_id").and_then(|v| v.as_i64()) {
        Some(id) if id > 0 => id,
        _ => return err_resp(StatusCode::BAD_REQUEST, "invalid_chat_id", "Positive integer chat_id is required"),
    };
    send_for_chat(&state, auth.user_id, chat_id, payload, StatusCode::OK).await
}

pub async fn chats_messages_send(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(chat_id): Path<i64>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    send_for_chat(&state, auth.user_id, chat_id, payload, StatusCode::CREATED).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ChatMemberRecord, ChatRecord, UserRecord};
    use crate::ServerConfig;
    use axum::body::to_bytes;

    async fn resp_status_and_json(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, val)
    }

    #[tokio::test]
    async fn test_send_as_peer_id_strict_validation_pre_commit() {
        let state = Arc::new(AppState::new(ServerConfig::default()));
        let (user_a, _, _) = state
            .session_manager
            .register("alice_send_as", "Pass123456!", "Alice", None, "CLI")
            .unwrap();
        let (user_b, _, _) = state
            .session_manager
            .register("bob_send_as", "Pass123456!", "Bob", None, "CLI")
            .unwrap();

        let chat1 = ChatId(910_001);
        let chat2 = ChatId(910_002);
        let now = Utc::now().timestamp_millis();
        for (uid, uname, fname) in [
            (user_a.user_id, "alice_send_as", "Alice"),
            (user_b.user_id, "bob_send_as", "Bob"),
        ] {
            state
                .db
                .create_user(&UserRecord {
                    user_id: uid,
                    username: Some(uname.to_string()),
                    phone: None,
                    first_name: fname.to_string(),
                    last_name: None,
                    bio: None,
                    password_hash: None,
                    is_bot: false,
                    is_verified: false,
                    is_support: false,
                    is_scam: false,
                    is_fake: false,
                    profile_photo_file_id: None,
                    account_generation: 1,
                    created_at_ms: now,
                    updated_at_ms: now,
                })
                .unwrap();
        }
        state
            .db
            .create_chat(&ChatRecord {
                chat_id: chat1,
                chat_type: 2,
                peer_type: 2,
                title: "Channel A".to_string(),
                username: None,
                about: None,
                creator_id: user_a.user_id,
                is_forum: false,
                flags: 0,
                photo_file_id: None,
                slowmode_seconds: 0,
                linked_chat_id: None,
                pinned_message_id: None,
                pts: 0,
                last_message_id: MsgId(0),
                last_message_date_ms: 0,
                unread_count: 0,
                account_generation: 1,
                created_at_ms: now,
                updated_at_ms: now,
            })
            .unwrap();
        state
            .db
            .add_chat_member(&ChatMemberRecord {
                chat_id: chat1,
                user_id: user_a.user_id,
                role: "creator".to_string(),
                permissions_bitmask: 0,
                admin_title: None,
                admin_rights: 0,
                banned_rights: 0,
                until_date_ms: None,
                joined_at_ms: now,
                invited_by: None,
                updated_at_ms: now,
            })
            .unwrap();
        state
            .db
            .add_chat_member(&ChatMemberRecord {
                chat_id: chat1,
                user_id: user_b.user_id,
                role: "member".to_string(),
                permissions_bitmask: 0,
                admin_title: None,
                admin_rights: 0,
                banned_rights: 0,
                until_date_ms: None,
                joined_at_ms: now,
                invited_by: None,
                updated_at_ms: now,
            })
            .unwrap();
        state
            .db
            .create_chat(&ChatRecord {
                chat_id: chat2,
                chat_type: 2,
                peer_type: 2,
                title: "Channel B".to_string(),
                username: None,
                about: None,
                creator_id: user_b.user_id,
                is_forum: false,
                flags: 0,
                photo_file_id: None,
                slowmode_seconds: 0,
                linked_chat_id: None,
                pinned_message_id: None,
                pts: 0,
                last_message_id: MsgId(0),
                last_message_date_ms: 0,
                unread_count: 0,
                account_generation: 1,
                created_at_ms: now,
                updated_at_ms: now,
            })
            .unwrap();
        state
            .db
            .add_chat_member(&ChatMemberRecord {
                chat_id: chat2,
                user_id: user_b.user_id,
                role: "creator".to_string(),
                permissions_bitmask: 0,
                admin_title: None,
                admin_rights: 0,
                banned_rights: 0,
                until_date_ms: None,
                joined_at_ms: now,
                invited_by: None,
                updated_at_ms: now,
            })
            .unwrap();

        let pts_before = state.db.get_current_pts(chat1).unwrap();
        let last_msg_before = state.db.get_chat(chat1).unwrap().unwrap().last_message_id;

        let invalid_values: Vec<Value> = vec![
            json!(-1),
            json!(0),
            json!(1.5),
            json!("999"),
            json!(true),
            json!({"id": 1}),
            json!(u64::MAX),
            json!((i64::MAX as u64) + 1),
            json!(chat2.0),
        ];

        for (idx, invalid_val) in invalid_values.into_iter().enumerate() {
            let nonce = 88_000 + idx as u64;
            let resp = send_for_chat(
                &state,
                user_a.user_id,
                chat1.0,
                json!({
                    "text": format!("REJECTED_{idx}"),
                    "send_as_peer_id": invalid_val,
                    "client_nonce": nonce,
                }),
                StatusCode::CREATED,
            )
            .await;
            let (st, body) = resp_status_and_json(resp).await;
            assert_eq!(st, StatusCode::FORBIDDEN, "expected 403 for invalid send_as_peer_id idx={idx}");
            assert_eq!(body["error"].as_str(), Some("invalid_send_as"));
            assert!(state.db.find_dedup_message(user_a.user_id.0, nonce).unwrap().is_none());
        }

        // Non-admin member cannot send_as chat1
        let resp_member = send_for_chat(
            &state,
            user_b.user_id,
            chat1.0,
            json!({
                "text": "REJECTED_MEMBER",
                "send_as_peer_id": chat1.0,
                "client_nonce": 88_500,
            }),
            StatusCode::CREATED,
        )
        .await;
        let (st_member, body_member) = resp_status_and_json(resp_member).await;
        assert_eq!(st_member, StatusCode::FORBIDDEN);
        assert_eq!(body_member["error"].as_str(), Some("invalid_send_as"));

        assert_eq!(state.db.get_current_pts(chat1).unwrap(), pts_before);
        assert_eq!(
            state.db.get_chat(chat1).unwrap().unwrap().last_message_id,
            last_msg_before
        );

        // First rejected nonce (88_000) must NOT have been committed, so reusing it succeeds!
        let (st_null, body_null) = resp_status_and_json(
            send_for_chat(
                &state,
                user_a.user_id,
                chat1.0,
                json!({
                    "text": "OK_WITH_NULL_AND_REUSED_NONCE",
                    "send_as_peer_id": null,
                    "client_nonce": 88_000,
                }),
                StatusCode::CREATED,
            )
            .await,
        )
        .await;
        assert_eq!(st_null, StatusCode::CREATED);
        assert_eq!(body_null["duplicate"], json!(false));
        assert_eq!(body_null["send_as_peer_id"], Value::Null);

        // Self send_as and creator-channel send_as succeed
        let (st_self, body_self) = resp_status_and_json(
            send_for_chat(
                &state,
                user_a.user_id,
                chat1.0,
                json!({
                    "text": "OK_SELF",
                    "send_as_peer_id": user_a.user_id.0,
                    "client_nonce": 88_001,
                }),
                StatusCode::OK,
            )
            .await,
        )
        .await;
        assert_eq!(st_self, StatusCode::OK);
        assert_eq!(body_self["send_as_peer_id"].as_u64(), Some(user_a.user_id.0));

        let (st_chan, body_chan) = resp_status_and_json(
            send_for_chat(
                &state,
                user_a.user_id,
                chat1.0,
                json!({
                    "text": "OK_CHANNEL",
                    "send_as_peer_id": chat1.0,
                    "client_nonce": 88_002,
                }),
                StatusCode::OK,
            )
            .await,
        )
        .await;
        assert_eq!(st_chan, StatusCode::OK);
        assert_eq!(body_chan["send_as_peer_id"].as_i64(), Some(chat1.0));

        // Caller-owned DM chat (chat_type == 4) must NOT be accepted as send_as_peer_id
        let dm_chat = ChatId(91_003);
        state
            .db
            .create_chat(&ChatRecord {
                chat_id: dm_chat,
                chat_type: 4,
                peer_type: 1,
                title: "DM Peer".to_string(),
                username: None,
                about: None,
                creator_id: user_a.user_id,
                is_forum: false,
                flags: 0,
                photo_file_id: None,
                slowmode_seconds: 0,
                linked_chat_id: None,
                pinned_message_id: None,
                pts: 0,
                last_message_id: MsgId(0),
                last_message_date_ms: 0,
                unread_count: 0,
                account_generation: 1,
                created_at_ms: now,
                updated_at_ms: now,
            })
            .unwrap();
        state
            .db
            .add_chat_member(&ChatMemberRecord {
                chat_id: dm_chat,
                user_id: user_a.user_id,
                role: "creator".to_string(),
                permissions_bitmask: 0,
                admin_title: None,
                admin_rights: 0,
                banned_rights: 0,
                until_date_ms: None,
                joined_at_ms: now,
                invited_by: None,
                updated_at_ms: now,
            })
            .unwrap();
        let (st_dm, body_dm) = resp_status_and_json(
            send_for_chat(
                &state,
                user_a.user_id,
                chat1.0,
                json!({
                    "text": "REJECTED_DM_SEND_AS",
                    "send_as_peer_id": dm_chat.0,
                    "client_nonce": 88_003,
                }),
                StatusCode::OK,
            )
            .await,
        )
        .await;
        assert_eq!(st_dm, StatusCode::FORBIDDEN);
        assert_eq!(body_dm["error"].as_str(), Some("invalid_send_as"));
    }
}
