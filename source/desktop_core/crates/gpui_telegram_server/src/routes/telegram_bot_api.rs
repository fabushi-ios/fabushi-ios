use crate::db::miniapp_store::{
    percent_decode_component, BotRegistryRecord, MiniAppRecord, BOTFATHER_USER_ID,
};
use crate::db::schema::{ChatRecord, UserRecord};
use crate::messaging::pipeline::MessagePipeline;
use crate::routes::bots_miniapps::{bot_record_to_entry, miniapp_record_to_entry};
use crate::routes::broadcast_chat_pts;
use crate::server::{extract_message_content_text, AppState};
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, UserId};
use gpui_telegram_proto::message::{
    InlineKeyboardButton, MessageContent, MessageEntity, MessageEntityType, MessageItem,
    ReplyHeader,
};
use gpui_telegram_proto::miniapp_contracts::{validate_bot_command, BotCommandSpec};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// Standard Telegram Bot API Response Helpers
// ============================================================================

fn bot_ok(result: Value) -> Response {
    (StatusCode::OK, Json(json!({ "ok": true, "result": result }))).into_response()
}

fn bot_err(status: StatusCode, error_code: i64, description: &str) -> Response {
    (
        status,
        Json(json!({
            "ok": false,
            "error_code": error_code,
            "description": description
        })),
    )
        .into_response()
}

// ============================================================================
// JSON Serializers (1:1 Official Telegram Bot API Shapes)
// ============================================================================

pub(crate) fn tg_user_json(user: &UserRecord) -> Value {
    let mut obj = Map::new();
    obj.insert("id".to_string(), json!(user.user_id.0));
    obj.insert("is_bot".to_string(), json!(user.is_bot));
    obj.insert("first_name".to_string(), json!(user.first_name));
    if let Some(last) = user.last_name.as_deref().filter(|s| !s.trim().is_empty()) {
        obj.insert("last_name".to_string(), json!(last));
    }
    if let Some(un) = user.username.as_deref().filter(|s| !s.trim().is_empty()) {
        obj.insert("username".to_string(), json!(un));
    }
    if !user.is_bot {
        obj.insert("language_code".to_string(), json!("zh-hans"));
    }
    Value::Object(obj)
}

fn tg_bot_user_json(bot: &BotRegistryRecord) -> Value {
    json!({
        "id": bot.bot_user_id,
        "is_bot": true,
        "first_name": bot.display_name,
        "username": bot.username,
        "can_join_groups": bot.can_join_groups,
        "can_read_all_group_messages": false,
        "supports_inline_queries": bot.is_inline,
        "has_main_web_app": bot.has_main_app,
    })
}

pub(crate) fn tg_private_chat_json(user: &UserRecord) -> Value {
    let mut obj = Map::new();
    obj.insert("id".to_string(), json!(user.user_id.0));
    obj.insert("type".to_string(), json!("private"));
    obj.insert("first_name".to_string(), json!(user.first_name));
    if let Some(last) = user.last_name.as_deref().filter(|s| !s.trim().is_empty()) {
        obj.insert("last_name".to_string(), json!(last));
    }
    if let Some(un) = user.username.as_deref().filter(|s| !s.trim().is_empty()) {
        obj.insert("username".to_string(), json!(un));
    }
    Value::Object(obj)
}

fn tg_group_chat_json(chat: &ChatRecord) -> Value {
    let chat_type = match chat.chat_type {
        1 => "group",
        2 => "supergroup",
        3 => "channel",
        _ => "group",
    };
    let mut obj = Map::new();
    obj.insert("id".to_string(), json!(chat.chat_id.0));
    obj.insert("type".to_string(), json!(chat_type));
    obj.insert("title".to_string(), json!(chat.title));
    if let Some(un) = chat.username.as_deref().filter(|s| !s.trim().is_empty()) {
        obj.insert("username".to_string(), json!(un));
    }
    Value::Object(obj)
}

fn tg_inline_keyboard_reply_markup(kb: &[Vec<InlineKeyboardButton>]) -> Value {
    let rows: Vec<Value> = kb
        .iter()
        .map(|row| {
            Value::Array(
                row.iter()
                    .map(|b| {
                        let mut m = Map::new();
                        m.insert("text".to_string(), json!(b.text));
                        if let Some(url) = b.url.as_deref() {
                            m.insert("url".to_string(), json!(url));
                        }
                        if let Some(wurl) = b.web_app_url.as_deref() {
                            m.insert("web_app".to_string(), json!({ "url": wurl }));
                        }
                        if let Some(cb) = b.callback_data.as_deref() {
                            if let Some(sq) = cb.strip_prefix("switch_inline:") {
                                m.insert("switch_inline_query".to_string(), json!(sq));
                            } else {
                                m.insert("callback_data".to_string(), json!(cb));
                            }
                        }
                        Value::Object(m)
                    })
                    .collect(),
            )
        })
        .collect();
    json!({ "inline_keyboard": rows })
}

fn build_tg_message_json(
    msg_id: MsgId,
    from_val: Value,
    chat_val: Value,
    date_secs: i64,
    text: &str,
    kb_opt: Option<&Vec<Vec<InlineKeyboardButton>>>,
    entities: &[MessageEntity],
) -> Value {
    let mut obj = Map::new();
    obj.insert("message_id".to_string(), json!(msg_id.0));
    obj.insert("from".to_string(), from_val);
    obj.insert("chat".to_string(), chat_val);
    obj.insert("date".to_string(), json!(date_secs));
    obj.insert("text".to_string(), json!(text));
    if !entities.is_empty() {
        let ents: Vec<Value> = entities
            .iter()
            .filter_map(|e| {
                let kind = match &e.entity_type {
                    MessageEntityType::Bold => Some("bold"),
                    MessageEntityType::Italic => Some("italic"),
                    MessageEntityType::Underline => Some("underline"),
                    MessageEntityType::Strikethrough => Some("strikethrough"),
                    MessageEntityType::Spoiler => Some("spoiler"),
                    MessageEntityType::Code => Some("code"),
                    MessageEntityType::Pre { .. } => Some("pre"),
                    MessageEntityType::Url => Some("url"),
                    MessageEntityType::TextUrl { .. } => Some("text_link"),
                    MessageEntityType::Mention => Some("mention"),
                    MessageEntityType::Hashtag => Some("hashtag"),
                    MessageEntityType::BotCommand => Some("bot_command"),
                    MessageEntityType::CustomEmoji { .. } => Some("custom_emoji"),
                }?;
                let mut em = Map::new();
                em.insert("offset".to_string(), json!(e.offset));
                em.insert("length".to_string(), json!(e.length));
                em.insert("type".to_string(), json!(kind));
                if let MessageEntityType::TextUrl { url } = &e.entity_type {
                    em.insert("url".to_string(), json!(url));
                }
                Some(Value::Object(em))
            })
            .collect();
        if !ents.is_empty() {
            obj.insert("entities".to_string(), Value::Array(ents));
        }
    }
    if let Some(kb) = kb_opt.filter(|k| !k.is_empty()) {
        obj.insert("reply_markup".to_string(), tg_inline_keyboard_reply_markup(kb));
    }
    Value::Object(obj)
}

fn extract_bot_command_entities(text: &str) -> Vec<MessageEntity> {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('/') {
        return Vec::new();
    }
    let token_end = trimmed
        .find(|c: char| c.is_whitespace())
        .unwrap_or(trimmed.len());
    let cmd_tok = &trimmed[..token_end];
    if cmd_tok.len() <= 1 {
        return Vec::new();
    }
    let leading_ws = text.len() - trimmed.len();
    let offset = text[..leading_ws].encode_utf16().count() as u32;
    let length = cmd_tok.encode_utf16().count() as u32;
    vec![MessageEntity::new(offset, length, MessageEntityType::BotCommand)]
}

// ============================================================================
// Update Enqueueing + Webhook Delivery + Cross-Route Hooks
// ============================================================================

pub(crate) fn enqueue_update_and_maybe_webhook(
    state: &Arc<AppState>,
    bot: &BotRegistryRecord,
    partial_update: Value,
) {
    let payload_str = match serde_json::to_string(&partial_update) {
        Ok(s) => s,
        Err(_) => return,
    };
    let Ok(saved) = state.db.enqueue_bot_api_update(bot.bot_user_id, &payload_str) else {
        return;
    };
    let webhook_url = bot.webhook_url.trim().to_string();
    if webhook_url.is_empty() {
        return;
    }
    let secret_token = bot.webhook_secret_token.clone();
    let mut full_update = match partial_update {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    full_update.insert("update_id".to_string(), json!(saved.update_id));
    let delivery_body = Value::Object(full_update);
    tokio::spawn(async move {
        let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        else {
            return;
        };
        let mut req = client.post(&webhook_url).json(&delivery_body);
        if !secret_token.trim().is_empty() {
            req = req.header("X-Telegram-Bot-Api-Secret-Token", secret_token.trim());
        }
        let _ = req.send().await;
    });
}

/// Enqueues a `message` update when a human user sends a message in a bot DM or mentions/commands a bot in a group.
/// Returns `true` if at least one recipient bot in the chat is a standard developer bot (`is_standard_bot`).
pub fn record_incoming_user_message_for_bot(
    state: &Arc<AppState>,
    chat_id: ChatId,
    sender_id: UserId,
    msg_id: MsgId,
    text: &str,
    date_ms: i64,
) {
    let Ok(Some(chat)) = state.db.get_chat(chat_id) else {
        return;
    };
    let Ok(Some(sender)) = state.db.get_user(sender_id) else {
        return;
    };
    if sender.is_bot {
        return;
    }
    let members = state.db.get_chat_members(chat_id).unwrap_or_default();
    let is_dm = matches!(chat.chat_type, 0 | 4);
    let entities = extract_bot_command_entities(text);
    let entities_val = if !entities.is_empty() {
        Some(
            entities
                .iter()
                .map(|e| {
                    json!({
                        "offset": e.offset,
                        "length": e.length,
                        "type": "bot_command"
                    })
                })
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };

    for member_id in members {
        if member_id == sender_id || member_id.0 == BOTFATHER_USER_ID {
            continue;
        }
        let Ok(Some(bot)) = state.db.get_bot_by_user_id(member_id.0 as i64) else {
            continue;
        };

        if is_dm {
            let _ = state
                .db
                .mark_user_started_bot_chat(sender_id.0 as i64, bot.bot_user_id);
        } else {
            // In group/supergroup/channel chats, deliver slash commands or explicit @bot mentions
            let lower = text.to_ascii_lowercase();
            let mention = format!("@{}", bot.username.to_ascii_lowercase());
            let is_cmd = text.trim_start().starts_with('/');
            if !is_cmd && !lower.contains(&mention) {
                continue;
            }
        }

        let chat_val = if is_dm {
            tg_private_chat_json(&sender)
        } else {
            tg_group_chat_json(&chat)
        };
        let mut msg_obj = Map::new();
        msg_obj.insert("message_id".to_string(), json!(msg_id.0));
        msg_obj.insert("from".to_string(), tg_user_json(&sender));
        msg_obj.insert("chat".to_string(), chat_val);
        msg_obj.insert("date".to_string(), json!(date_ms / 1000));
        msg_obj.insert("text".to_string(), json!(text));
        if let Some(ref ents) = entities_val {
            msg_obj.insert("entities".to_string(), json!(ents));
        }
        enqueue_update_and_maybe_webhook(state, &bot, json!({ "message": Value::Object(msg_obj) }));
    }
}

/// Enqueues a `callback_query` update for a bot when a user clicks an inline keyboard button.
pub fn record_incoming_callback_query_for_bot(
    state: &Arc<AppState>,
    chat_id: ChatId,
    sender_id: UserId,
    bot_user_id: UserId,
    msg_id: MsgId,
    callback_data: &str,
) -> Option<String> {
    if bot_user_id.0 == BOTFATHER_USER_ID {
        return None;
    }
    let bot = state.db.get_bot_by_user_id(bot_user_id.0 as i64).ok().flatten()?;
    let sender = state.db.get_user(sender_id).ok().flatten()?;
    let chat = state.db.get_chat(chat_id).ok().flatten()?;
    let is_dm = matches!(chat.chat_type, 0 | 4);
    if is_dm {
        let _ = state
            .db
            .mark_user_started_bot_chat(sender_id.0 as i64, bot.bot_user_id);
    }
    let qid = format!("cbq_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    let chat_val = if is_dm {
        tg_private_chat_json(&sender)
    } else {
        tg_group_chat_json(&chat)
    };
    let existing_msg = state.db.get_message(chat_id, msg_id).ok().flatten();
    let msg_text = existing_msg
        .as_ref()
        .and_then(|r| MessagePipeline::record_to_item(r).ok())
        .map(|item| extract_message_content_text(&item.content))
        .unwrap_or_default();
    let msg_date = existing_msg
        .as_ref()
        .map(|r| r.date_ms / 1000)
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    let msg_val = json!({
        "message_id": msg_id.0,
        "from": tg_bot_user_json(&bot),
        "chat": chat_val,
        "date": msg_date,
        "text": msg_text,
    });
    let update = json!({
        "callback_query": {
            "id": qid,
            "from": tg_user_json(&sender),
            "message": msg_val,
            "chat_instance": chat_id.0.to_string(),
            "data": callback_data,
        }
    });
    enqueue_update_and_maybe_webhook(state, &bot, update);
    Some(qid)
}

/// Enqueues a `message.write_access_allowed` Bot API update when a user grants DM write permission.
pub fn enqueue_write_access_allowed_update(
    state: &Arc<AppState>,
    bot: &BotRegistryRecord,
    user_id: UserId,
    msg_id: Option<MsgId>,
    web_app_name: Option<&str>,
    from_attachment_menu: bool,
    from_request: bool,
) {
    let Some(sender) = state.db.get_user(user_id).ok().flatten() else {
        return;
    };
    let mut waa = Map::new();
    waa.insert("from_request".to_string(), json!(from_request));
    waa.insert(
        "from_attachment_menu".to_string(),
        json!(from_attachment_menu),
    );
    if let Some(name) = web_app_name.filter(|s| !s.trim().is_empty()) {
        waa.insert("web_app_name".to_string(), json!(name.trim()));
    }
    let update = json!({
        "message": {
            "message_id": msg_id.map(|m| m.0).unwrap_or(0),
            "from": tg_user_json(&sender),
            "chat": tg_private_chat_json(&sender),
            "date": chrono::Utc::now().timestamp(),
            "write_access_allowed": Value::Object(waa),
        }
    });
    enqueue_update_and_maybe_webhook(state, bot, update);
}

/// Enqueues a `message.web_app_data` Bot API update when a Mini App invokes `Telegram.WebApp.sendData`.
pub fn enqueue_web_app_data_update(
    state: &Arc<AppState>,
    bot: &BotRegistryRecord,
    app: &MiniAppRecord,
    user_id: UserId,
    msg_id: MsgId,
    data: &str,
) {
    let Some(sender) = state.db.get_user(user_id).ok().flatten() else {
        return;
    };
    let update = json!({
        "message": {
            "message_id": msg_id.0,
            "from": tg_user_json(&sender),
            "chat": tg_private_chat_json(&sender),
            "date": chrono::Utc::now().timestamp(),
            "web_app_data": {
                "data": data,
                "button_text": app.title,
            }
        }
    });
    enqueue_update_and_maybe_webhook(state, bot, update);
}

// ============================================================================
// Request Parameter & Keyboard Parsers
// ============================================================================

fn parse_bot_api_params(
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: &[u8],
) -> Map<String, Value> {
    let mut map = Map::new();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if !body.is_empty() {
        let trimmed = std::str::from_utf8(body).unwrap_or("").trim();
        let looks_json = content_type.contains("application/json") || trimmed.starts_with('{');
        if looks_json {
            if let Ok(Value::Object(obj)) = serde_json::from_slice::<Value>(body) {
                for (k, v) in obj {
                    map.insert(k, v);
                }
            }
        } else if !trimmed.is_empty() {
            for pair in trimmed.split('&') {
                if pair.is_empty() {
                    continue;
                }
                let (rk, rv) = pair.split_once('=').unwrap_or((pair, ""));
                let k = percent_decode_component(&rk.replace('+', " "));
                let v_str = percent_decode_component(&rv.replace('+', " "));
                if !k.is_empty() {
                    map.insert(k, parse_scalar_or_json(&v_str));
                }
            }
        }
    }

    for (k, v) in query {
        map.entry(k.clone())
            .or_insert_with(|| parse_scalar_or_json(v));
    }

    map
}

fn parse_scalar_or_json(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed == "true" {
        return Value::Bool(true);
    }
    if trimmed == "false" {
        return Value::Bool(false);
    }
    if trimmed == "null" {
        return Value::Null;
    }
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
            return parsed;
        }
    }
    if let Ok(n) = trimmed.parse::<i64>() {
        return Value::Number(n.into());
    }
    Value::String(raw.to_string())
}

fn param_str<'a>(params: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

fn param_i64(params: &Map<String, Value>, key: &str) -> Option<i64> {
    let v = params.get(key)?;
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    if let Some(s) = v.as_str() {
        return s.trim().parse::<i64>().ok();
    }
    None
}

fn param_bool(params: &Map<String, Value>, key: &str) -> Option<bool> {
    let v = params.get(key)?;
    if let Some(b) = v.as_bool() {
        return Some(b);
    }
    if let Some(n) = v.as_i64() {
        return Some(n != 0);
    }
    if let Some(s) = v.as_str() {
        match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => return Some(true),
            "false" | "0" | "no" => return Some(false),
            _ => {}
        }
    }
    None
}

fn parse_inline_keyboard_markup(val: Option<&Value>) -> Vec<Vec<InlineKeyboardButton>> {
    let Some(markup) = val else {
        return Vec::new();
    };
    let parsed_val: Value;
    let obj = if let Some(s) = markup.as_str() {
        match serde_json::from_str::<Value>(s) {
            Ok(v) => {
                parsed_val = v;
                &parsed_val
            }
            Err(_) => return Vec::new(),
        }
    } else {
        markup
    };
    let Some(rows) = obj.get("inline_keyboard").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| row.as_array())
        .map(|row| {
            row.iter()
                .filter_map(|btn| {
                    let text = btn.get("text").and_then(|v| v.as_str())?.trim().to_string();
                    if text.is_empty() {
                        return None;
                    }
                    let url = btn
                        .get("url")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    let web_app_url = btn
                        .get("web_app")
                        .and_then(|w| w.get("url"))
                        .and_then(|v| v.as_str())
                        .or_else(|| btn.get("web_app_url").and_then(|v| v.as_str()))
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    let callback_data = btn
                        .get("callback_data")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            btn.get("switch_inline_query")
                                .and_then(|v| v.as_str())
                                .map(|q| format!("switch_inline:{}", q))
                        });
                    Some(InlineKeyboardButton {
                        text,
                        callback_data,
                        url,
                        web_app_url,
                    })
                })
                .collect::<Vec<_>>()
        })
        .filter(|r| !r.is_empty())
        .collect()
}

// ============================================================================
// Chat Target Resolution for Bot API (`user_id`, `@username`, or `chat_id`)
// ============================================================================

enum BotTargetChat {
    PrivateUser {
        user: UserRecord,
        existing_chat: Option<ChatRecord>,
    },
    GroupChat {
        chat: ChatRecord,
    },
}

fn resolve_bot_target_chat(
    state: &AppState,
    bot: &BotRegistryRecord,
    chat_id_val: Option<&Value>,
) -> std::result::Result<BotTargetChat, Response> {
    let Some(raw) = chat_id_val else {
        return Err(bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: chat_id is empty",
        ));
    };

    let numeric_id = raw
        .as_i64()
        .or_else(|| raw.as_str().and_then(|s| s.trim().parse::<i64>().ok()));

    if let Some(id) = numeric_id {
        if id <= 0 {
            return Err(bot_err(
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: chat not found",
            ));
        }
        // 1. User ID (Telegram Bot API passes human user_id as private chat_id)
        if let Ok(Some(user)) = state.db.get_user(UserId(id as u64)) {
            if user.is_bot {
                return Err(bot_err(
                    StatusCode::FORBIDDEN,
                    403,
                    "Forbidden: bots can't send messages to bots",
                ));
            }
            let existing_cid = state
                .db
                .find_companion_chat_id_for_user(bot.bot_user_id, user.user_id.0 as i64)
                .ok()
                .flatten();
            let existing_chat = existing_cid.and_then(|cid| state.db.get_chat(cid).ok().flatten());
            return Ok(BotTargetChat::PrivateUser {
                user,
                existing_chat,
            });
        }

        // 2. Internal ChatId (companion DM or group/supergroup/channel)
        if let Ok(Some(chat)) = state.db.get_chat(ChatId(id)) {
            if matches!(chat.chat_type, 0 | 4) {
                let members = state.db.get_chat_members(chat.chat_id).unwrap_or_default();
                let bot_uid = UserId(bot.bot_user_id as u64);
                if !members.contains(&bot_uid) {
                    return Err(bot_err(
                        StatusCode::BAD_REQUEST,
                        400,
                        "Bad Request: chat not found",
                    ));
                }
                let peer_user = members
                    .into_iter()
                    .find(|uid| *uid != bot_uid)
                    .and_then(|uid| state.db.get_user(uid).ok().flatten())
                    .filter(|u| !u.is_bot);
                if let Some(user) = peer_user {
                    return Ok(BotTargetChat::PrivateUser {
                        user,
                        existing_chat: Some(chat),
                    });
                }
            } else {
                let is_member = state
                    .db
                    .is_user_in_chat(chat.chat_id, UserId(bot.bot_user_id as u64))
                    .unwrap_or(false);
                if !is_member {
                    return Err(bot_err(
                        StatusCode::FORBIDDEN,
                        403,
                        "Forbidden: bot is not a member of the chat",
                    ));
                }
                return Ok(BotTargetChat::GroupChat { chat });
            }
        }

        return Err(bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: chat not found",
        ));
    }

    if let Some(username_raw) = raw.as_str() {
        let clean = username_raw.trim().trim_start_matches('@');
        if !clean.is_empty() {
            if let Ok(Some(user)) = state.db.get_user_by_username(clean) {
                if user.is_bot {
                    return Err(bot_err(
                        StatusCode::FORBIDDEN,
                        403,
                        "Forbidden: bots can't send messages to bots",
                    ));
                }
                let existing_cid = state
                    .db
                    .find_companion_chat_id_for_user(bot.bot_user_id, user.user_id.0 as i64)
                    .ok()
                    .flatten();
                let existing_chat =
                    existing_cid.and_then(|cid| state.db.get_chat(cid).ok().flatten());
                return Ok(BotTargetChat::PrivateUser {
                    user,
                    existing_chat,
                });
            }
            if let Ok(Some(chat)) = state.db.get_chat_by_username(clean) {
                let is_member = state
                    .db
                    .is_user_in_chat(chat.chat_id, UserId(bot.bot_user_id as u64))
                    .unwrap_or(false);
                if !is_member {
                    return Err(bot_err(
                        StatusCode::FORBIDDEN,
                        403,
                        "Forbidden: bot is not a member of the chat",
                    ));
                }
                return Ok(BotTargetChat::GroupChat { chat });
            }
        }
    }

    Err(bot_err(
        StatusCode::BAD_REQUEST,
        400,
        "Bad Request: chat not found",
    ))
}

// ============================================================================
// HTTP Dispatchers
// ============================================================================

/// Handler for `/{bot_token_seg}/{method}` where `bot_token_seg` must start with `bot`.
pub async fn telegram_bot_api_dispatch(
    State(state): State<Arc<AppState>>,
    Path((bot_token_seg, method)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(token) = bot_token_seg.strip_prefix("bot") else {
        return bot_err(StatusCode::NOT_FOUND, 404, "Not Found");
    };
    dispatch_bot_api_call(&state, token, &method, &query, &headers, &body).await
}

/// Alternate split handler for `/bot/{token}/{method}`.
pub async fn telegram_bot_api_dispatch_split(
    State(state): State<Arc<AppState>>,
    Path((token, method)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    dispatch_bot_api_call(&state, &token, &method, &query, &headers, &body).await
}

async fn dispatch_bot_api_call(
    state: &Arc<AppState>,
    raw_token: &str,
    method: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: &Bytes,
) -> Response {
    let token = raw_token.trim();
    if token.is_empty() {
        return bot_err(StatusCode::UNAUTHORIZED, 401, "Unauthorized");
    }
    let Some(bot) = state.db.get_bot_by_token(token).ok().flatten() else {
        return bot_err(StatusCode::UNAUTHORIZED, 401, "Unauthorized");
    };

    let params = parse_bot_api_params(query, headers, body);
    let method_lc = method.trim().to_ascii_lowercase();

    match method_lc.as_str() {
        "getme" => handle_get_me(&bot),
        "getupdates" => handle_get_updates(state, &bot, &params),
        "setwebhook" => handle_set_webhook(state, &bot, &params),
        "deletewebhook" => handle_delete_webhook(state, &bot, &params),
        "getwebhookinfo" => handle_get_webhook_info(state, &bot),
        "sendmessage" => handle_send_message(state, &bot, &params),
        "editmessagetext" => handle_edit_message_text(state, &bot, &params),
        "deletemessage" => handle_delete_message(state, &bot, &params),
        "pinchatmessage" => handle_pin_chat_message(state, &bot, &params),
        "unpinchatmessage" => handle_unpin_chat_message(state, &bot, &params),
        "unpinallchatmessages" => handle_unpin_all_chat_messages(state, &bot, &params),
        "answercallbackquery" => handle_answer_callback_query(&params),
        "answerwebappquery" => handle_answer_web_app_query(state, &bot, &params),
        "setmycommands" => handle_set_my_commands(state, &bot, &params),
        "getmycommands" => handle_get_my_commands(&bot),
        "deletemycommands" => handle_delete_my_commands(state, &bot),
        "setchatmenubutton" => handle_set_chat_menu_button(state, &bot, &params),
        "getchatmenubutton" => handle_get_chat_menu_button(&bot),
        "setmydescription" => handle_set_my_description(state, &bot, &params),
        "getmydescription" => handle_get_my_description(&bot),
        "setmyshortdescription" => handle_set_my_short_description(state, &bot, &params),
        "getmyshortdescription" => handle_get_my_short_description(&bot),
        "setmyname" => handle_set_my_name(state, &bot, &params),
        "getmyname" => handle_get_my_name(&bot),
        "setmyprivacypolicy" => handle_set_my_privacy_policy(state, &bot, &params),
        "getmyprivacypolicy" => handle_get_my_privacy_policy(&bot),
        "publishminiapp" => handle_publish_mini_app(state, &bot, &params),
        _ => bot_err(
            StatusCode::NOT_FOUND,
            404,
            &format!("Not Found: unknown method '{}'", method),
        ),
    }
}

// ============================================================================
// Method Implementations
// ============================================================================

fn handle_get_me(bot: &BotRegistryRecord) -> Response {
    bot_ok(tg_bot_user_json(bot))
}

fn handle_get_updates(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let offset = param_i64(params, "offset");
    let limit = param_i64(params, "limit").unwrap_or(100).clamp(1, 100) as usize;
    match state.db.get_bot_api_updates(bot.bot_user_id, offset, limit) {
        Ok(records) => {
            let updates: Vec<Value> = records
                .into_iter()
                .map(|r| {
                    let mut map = match serde_json::from_str::<Value>(&r.payload_json) {
                        Ok(Value::Object(m)) => m,
                        _ => Map::new(),
                    };
                    map.insert("update_id".to_string(), json!(r.update_id));
                    Value::Object(map)
                })
                .collect();
            bot_ok(Value::Array(updates))
        }
        Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
    }
}

fn handle_set_webhook(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let url = param_str(params, "url").unwrap_or("").trim();
    if !url.is_empty() && !url.starts_with("http://") && !url.starts_with("https://") {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: bad webhook: HTTPS/HTTP URL expected",
        );
    }
    let secret = param_str(params, "secret_token").unwrap_or("");
    if param_bool(params, "drop_pending_updates").unwrap_or(false) {
        let _ = state.db.clear_bot_api_updates(bot.bot_user_id);
    }
    match state.db.update_bot_webhook(bot.bot_user_id, url, secret) {
        Ok(_) => bot_ok(json!(true)),
        Err(e) => bot_err(StatusCode::BAD_REQUEST, 400, &e.to_string()),
    }
}

fn handle_delete_webhook(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    if param_bool(params, "drop_pending_updates").unwrap_or(false) {
        let _ = state.db.clear_bot_api_updates(bot.bot_user_id);
    }
    match state.db.update_bot_webhook(bot.bot_user_id, "", "") {
        Ok(_) => bot_ok(json!(true)),
        Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
    }
}

fn handle_get_webhook_info(state: &AppState, bot: &BotRegistryRecord) -> Response {
    let pending = state
        .db
        .count_bot_api_updates(bot.bot_user_id)
        .unwrap_or(0);
    bot_ok(json!({
        "url": bot.webhook_url,
        "has_custom_certificate": false,
        "pending_update_count": pending,
        "max_connections": 40,
    }))
}

fn handle_send_message(
    state: &Arc<AppState>,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let text_raw = param_str(params, "text").unwrap_or("").trim();
    if text_raw.is_empty() {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: message text is empty",
        );
    }

    let target = match resolve_bot_target_chat(state, bot, params.get("chat_id")) {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    let (target_cid, response_chat_val) = match target {
        BotTargetChat::PrivateUser { user, existing_chat } => {
            let blocked = state
                .db
                .is_user_blocked(user.user_id, UserId(bot.bot_user_id as u64))
                .unwrap_or(false);
            if blocked {
                return bot_err(
                    StatusCode::FORBIDDEN,
                    403,
                    "Forbidden: bot was blocked by the user",
                );
            }
            let perm = state
                .db
                .get_user_bot_permission(user.user_id.0 as i64, bot.bot_user_id)
                .ok();
            let allowed = perm
                .as_ref()
                .is_some_and(|p| p.allows_write_to_pm || p.has_started_chat);
            if !allowed {
                return bot_err(
                    StatusCode::FORBIDDEN,
                    403,
                    "Forbidden: bot can't initiate conversation with a user",
                );
            }
            let cid = match existing_chat {
                Some(c) => c.chat_id,
                None => match state
                    .db
                    .ensure_companion_bot_chat_record(user.user_id, UserId(bot.bot_user_id as u64))
                {
                    Ok((cid, _)) => cid,
                    Err(e) => return bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
                },
            };
            state.gateway_hub.subscribe_chat(cid.0, user.user_id);
            state
                .gateway_hub
                .subscribe_chat(cid.0, UserId(bot.bot_user_id as u64));
            (cid, tg_private_chat_json(&user))
        }
        BotTargetChat::GroupChat { chat } => (chat.chat_id, tg_group_chat_json(&chat)),
    };

    let reply_to_id = param_i64(params, "reply_to_message_id").or_else(|| {
        params
            .get("reply_parameters")
            .and_then(|rp| rp.get("message_id"))
            .and_then(|v| v.as_i64())
    });
    let kb_rows = parse_inline_keyboard_markup(params.get("reply_markup"));
    let entities = parse_message_entities_from_params(params, text_raw);

    let mut msg = MessageItem::new(
        MsgId(0),
        target_cid,
        UserId(bot.bot_user_id as u64),
        PeerId::Chat(target_cid),
        MessageContent::text_with_entities(text_raw, entities.clone()),
    );
    msg.flags.is_out = false;
    msg.flags.is_silent = param_bool(params, "disable_notification").unwrap_or(false);
    if let Some(rid) = reply_to_id.filter(|&id| id > 0) {
        msg.reply_to = Some(ReplyHeader::new(MsgId(rid)));
    }
    if !kb_rows.is_empty() {
        msg.inline_keyboard = Some(kb_rows.clone());
    }

    let pts_update = match state.message_pipeline.process_incoming_message(msg) {
        Ok(u) => u,
        Err(e) => return bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
    };
    let (saved_id, date_secs) = match &pts_update.event {
        gpui_telegram_proto::sync::UpdateEvent::NewMessage(saved) => {
            (saved.id, saved.date.timestamp())
        }
        _ => (MsgId(0), chrono::Utc::now().timestamp()),
    };
    if !kb_rows.is_empty() && saved_id.0 > 0 {
        if let Ok(kb_json) = serde_json::to_string(&kb_rows) {
            let _ = state
                .db
                .save_message_extras(target_cid, saved_id, Some(&kb_json), None);
        }
    }
    broadcast_chat_pts(state, target_cid, pts_update);

    let msg_json = build_tg_message_json(
        saved_id,
        tg_bot_user_json(bot),
        response_chat_val,
        date_secs,
        text_raw,
        Some(&kb_rows),
        &entities,
    );
    bot_ok(msg_json)
}

fn parse_message_entities_from_params(params: &Map<String, Value>, _text: &str) -> Vec<MessageEntity> {
    let Some(arr) = params.get("entities").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| {
            let offset = v.get("offset").and_then(|o| o.as_u64())? as u32;
            let length = v.get("length").and_then(|l| l.as_u64())? as u32;
            let kind = v.get("type").and_then(|t| t.as_str())?;
            let entity_type = match kind {
                "bold" => MessageEntityType::Bold,
                "italic" => MessageEntityType::Italic,
                "underline" => MessageEntityType::Underline,
                "strikethrough" => MessageEntityType::Strikethrough,
                "spoiler" => MessageEntityType::Spoiler,
                "code" => MessageEntityType::Code,
                "pre" => MessageEntityType::Pre {
                    language: v
                        .get("language")
                        .and_then(|l| l.as_str())
                        .map(String::from),
                },
                "url" => MessageEntityType::Url,
                "text_link" => MessageEntityType::TextUrl {
                    url: v
                        .get("url")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string(),
                },
                "mention" => MessageEntityType::Mention,
                "hashtag" => MessageEntityType::Hashtag,
                "bot_command" => MessageEntityType::BotCommand,
                _ => return None,
            };
            Some(MessageEntity::new(offset, length, entity_type))
        })
        .collect()
}

fn handle_edit_message_text(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let text_raw = param_str(params, "text").unwrap_or("").trim();
    if text_raw.is_empty() {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: message text is empty",
        );
    }
    let Some(msg_id_raw) = param_i64(params, "message_id").filter(|&id| id > 0) else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: message_id is required",
        );
    };

    let target = match resolve_bot_target_chat(state, bot, params.get("chat_id")) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let (cid, response_chat_val) = match target {
        BotTargetChat::PrivateUser {
            user,
            existing_chat: Some(chat),
        } => (chat.chat_id, tg_private_chat_json(&user)),
        BotTargetChat::PrivateUser {
            existing_chat: None,
            ..
        } => {
            return bot_err(
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: message to edit not found",
            )
        }
        BotTargetChat::GroupChat { chat } => (chat.chat_id, tg_group_chat_json(&chat)),
    };

    let msg_id = MsgId(msg_id_raw);
    let Ok(Some(existing)) = state.db.get_message(cid, msg_id) else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: message to edit not found",
        );
    };
    if existing.sender_id.0 != bot.bot_user_id as u64 {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: message can't be edited",
        );
    }

    let entities = parse_message_entities_from_params(params, text_raw);
    let mut pts_update = match state.message_pipeline.edit_message(
        cid,
        msg_id,
        MessageContent::text_with_entities(text_raw, entities.clone()),
    ) {
        Ok(u) => u,
        Err(e) => return bot_err(StatusCode::BAD_REQUEST, 400, &e.to_string()),
    };

    let kb_rows = if params.contains_key("reply_markup") {
        let rows = parse_inline_keyboard_markup(params.get("reply_markup"));
        let json_str = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string());
        let _ = state.db.save_message_extras(cid, msg_id, Some(&json_str), None);
        Some(rows)
    } else {
        state
            .db
            .get_message_extras(cid, msg_id)
            .ok()
            .and_then(|(ikb, _, _)| ikb)
            .and_then(|s| serde_json::from_str::<Vec<Vec<InlineKeyboardButton>>>(&s).ok())
    };
    if let gpui_telegram_proto::sync::UpdateEvent::EditMessage(ref mut edited_msg) = pts_update.event
    {
        edited_msg.inline_keyboard = kb_rows.clone();
    }
    broadcast_chat_pts(state, cid, pts_update);

    let msg_json = build_tg_message_json(
        msg_id,
        tg_bot_user_json(bot),
        response_chat_val,
        existing.date_ms / 1000,
        text_raw,
        kb_rows.as_ref(),
        &entities,
    );
    bot_ok(msg_json)
}

fn handle_delete_message(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let Some(msg_id_raw) = param_i64(params, "message_id").filter(|&id| id > 0) else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: message_id is required",
        );
    };
    let target = match resolve_bot_target_chat(state, bot, params.get("chat_id")) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let cid = match target {
        BotTargetChat::PrivateUser {
            existing_chat: Some(c),
            ..
        } => c.chat_id,
        BotTargetChat::PrivateUser {
            existing_chat: None,
            ..
        } => {
            return bot_err(
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: message to delete not found",
            )
        }
        BotTargetChat::GroupChat { chat } => chat.chat_id,
    };
    let msg_id = MsgId(msg_id_raw);
    if state
        .db
        .get_message(cid, msg_id)
        .ok()
        .flatten()
        .is_none()
    {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: message to delete not found",
        );
    }
    let prev_pins = state.db.list_pinned_messages(cid).unwrap_or_default();
    let removed_pinned = prev_pins.contains(&msg_id.0);
    match state
        .message_pipeline
        .delete_messages(cid, vec![msg_id], true)
    {
        Ok(pts) => {
            broadcast_chat_pts(state, cid, pts);
            if removed_pinned {
                let pinned_ids = state.db.list_pinned_messages(cid).unwrap_or_default();
                crate::routes::broadcast_chat_event(
                    state,
                    cid,
                    gpui_telegram_proto::sync::UpdateEvent::PinnedMessagesChanged {
                        chat_id: cid,
                        pinned_ids: pinned_ids.into_iter().map(MsgId).collect(),
                    },
                );
            }
            bot_ok(json!(true))
        }
        Err(e) => bot_err(StatusCode::BAD_REQUEST, 400, &e.to_string()),
    }
}

fn resolve_bot_pin_target_chat(
    state: &AppState,
    bot: &BotRegistryRecord,
    chat_id_val: Option<&Value>,
    require_existing_dm: bool,
) -> std::result::Result<Option<ChatId>, Response> {
    let target = resolve_bot_target_chat(state, bot, chat_id_val)?;
    match target {
        BotTargetChat::PrivateUser { user, existing_chat } => {
            if state
                .db
                .is_user_blocked(user.user_id, UserId(bot.bot_user_id as u64))
                .unwrap_or(false)
            {
                return Err(bot_err(
                    StatusCode::FORBIDDEN,
                    403,
                    "Forbidden: bot was blocked by the user",
                ));
            }
            match existing_chat {
                Some(c) => Ok(Some(c.chat_id)),
                None if require_existing_dm => Err(bot_err(
                    StatusCode::BAD_REQUEST,
                    400,
                    "Bad Request: message to pin not found",
                )),
                None => Ok(None),
            }
        }
        BotTargetChat::GroupChat { chat } => {
            let bot_uid = UserId(bot.bot_user_id as u64);
            let allowed = if chat.creator_id == bot_uid {
                true
            } else if let Ok(Some(member)) = state.db.get_chat_member(chat.chat_id, bot_uid) {
                let role = member.role.trim().to_ascii_lowercase();
                matches!(role.as_str(), "owner" | "creator")
                    || (matches!(role.as_str(), "admin" | "administrator")
                        && (member.admin_rights == 0
                            || (member.admin_rights & crate::server::ADMIN_RIGHTS_PIN_MESSAGES)
                                != 0))
            } else {
                false
            };
            if !allowed {
                return Err(bot_err(
                    StatusCode::BAD_REQUEST,
                    400,
                    "Bad Request: not enough rights to manage pinned messages in the chat",
                ));
            }
            Ok(Some(chat.chat_id))
        }
    }
}

fn emit_bot_pinned_messages_changed(state: &AppState, cid: ChatId, ids: &[i64]) {
    if let Ok((_, pts_update)) = state.pts_engine.advance_pts(
        cid,
        1,
        gpui_telegram_proto::sync::UpdateEvent::PinnedMessagesChanged {
            chat_id: cid,
            pinned_ids: ids.iter().copied().map(MsgId).collect(),
        },
    ) {
        broadcast_chat_pts(state, cid, pts_update);
    }
}

fn handle_pin_chat_message(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let Some(message_id) = param_i64(params, "message_id").filter(|&id| id > 0) else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: message_id is required",
        );
    };
    let Some(cid) = (match resolve_bot_pin_target_chat(state, bot, params.get("chat_id"), true) {
        Ok(c) => c,
        Err(resp) => return resp,
    }) else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: message to pin not found",
        );
    };
    if !state
        .db
        .get_message(cid, MsgId(message_id))
        .ok()
        .flatten()
        .is_some_and(|m| m.deleted_at_ms.is_none())
    {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: message to pin not found",
        );
    }
    match state
        .db
        .pin_message(cid, MsgId(message_id), UserId(bot.bot_user_id as u64))
    {
        Ok(ids) => {
            emit_bot_pinned_messages_changed(state, cid, &ids);
            bot_ok(json!(true))
        }
        Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
    }
}

fn handle_unpin_chat_message(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let msg_id_opt = param_i64(params, "message_id").filter(|&id| id > 0);
    let Some(cid) = (match resolve_bot_pin_target_chat(
        state,
        bot,
        params.get("chat_id"),
        msg_id_opt.is_some(),
    ) {
        Ok(c) => c,
        Err(resp) => return resp,
    }) else {
        return bot_ok(json!(true));
    };

    let target_mid = if let Some(mid) = msg_id_opt {
        if state
            .db
            .get_message(cid, MsgId(mid))
            .ok()
            .flatten()
            .is_none()
        {
            return bot_err(
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: message to unpin not found",
            );
        }
        Some(MsgId(mid))
    } else {
        match state.db.list_pinned_messages(cid) {
            Ok(list) => match list.first().copied() {
                Some(latest) => Some(MsgId(latest)),
                None => return bot_ok(json!(true)),
            },
            Err(e) => return bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
        }
    };

    match state.db.unpin_message(cid, target_mid) {
        Ok(ids) => {
            emit_bot_pinned_messages_changed(state, cid, &ids);
            bot_ok(json!(true))
        }
        Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
    }
}

fn handle_unpin_all_chat_messages(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let Some(cid) = (match resolve_bot_pin_target_chat(state, bot, params.get("chat_id"), false) {
        Ok(c) => c,
        Err(resp) => return resp,
    }) else {
        return bot_ok(json!(true));
    };
    match state.db.unpin_message(cid, None) {
        Ok(_) => {
            emit_bot_pinned_messages_changed(state, cid, &[]);
            bot_ok(json!(true))
        }
        Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
    }
}

fn handle_answer_callback_query(params: &Map<String, Value>) -> Response {
    let qid = param_str(params, "callback_query_id").unwrap_or("").trim();
    if qid.is_empty() {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: callback_query_id is empty",
        );
    }
    bot_ok(json!(true))
}

fn handle_answer_web_app_query(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let qid = param_str(params, "web_app_query_id").unwrap_or("").trim();
    if qid.is_empty() {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: web_app_query_id is empty",
        );
    }
    let Some(sess) = state.db.get_mini_app_session(qid).ok().flatten() else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: QUERY_ID_INVALID",
        );
    };
    if sess.bot_user_id != bot.bot_user_id {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: QUERY_ID_INVALID",
        );
    }

    let result_obj = match params.get("result") {
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok(),
        Some(v) if v.is_object() => Some(v.clone()),
        _ => None,
    };
    let Some(res_val) = result_obj else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: result must be a JSON InlineQueryResult object",
        );
    };

    let msg_text = res_val
        .get("input_message_content")
        .and_then(|imc| imc.get("message_text"))
        .and_then(|v| v.as_str())
        .or_else(|| res_val.get("description").and_then(|v| v.as_str()))
        .or_else(|| res_val.get("title").and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim();
    if msg_text.is_empty() {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: result message_text cannot be empty",
        );
    }

    let kb_rows = parse_inline_keyboard_markup(res_val.get("reply_markup"));
    let target_cid = if sess.chat_id > 0 {
        ChatId(sess.chat_id)
    } else {
        match state.db.ensure_companion_bot_chat_record(
            UserId(sess.user_id as u64),
            UserId(bot.bot_user_id as u64),
        ) {
            Ok((cid, _)) => cid,
            Err(e) => return bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
        }
    };
    state
        .gateway_hub
        .subscribe_chat(target_cid.0, UserId(sess.user_id as u64));
    state
        .gateway_hub
        .subscribe_chat(target_cid.0, UserId(bot.bot_user_id as u64));

    let mut msg = MessageItem::new(
        MsgId(0),
        target_cid,
        UserId(bot.bot_user_id as u64),
        PeerId::Chat(target_cid),
        MessageContent::plain_text(msg_text),
    );
    msg.flags.is_out = false;
    if !kb_rows.is_empty() {
        msg.inline_keyboard = Some(kb_rows.clone());
    }
    let pts_update = match state.message_pipeline.process_incoming_message(msg) {
        Ok(u) => u,
        Err(e) => return bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
    };
    let saved_id = match &pts_update.event {
        gpui_telegram_proto::sync::UpdateEvent::NewMessage(saved) => saved.id,
        _ => MsgId(0),
    };
    if !kb_rows.is_empty() && saved_id.0 > 0 {
        if let Ok(kb_json) = serde_json::to_string(&kb_rows) {
            let _ = state
                .db
                .save_message_extras(target_cid, saved_id, Some(&kb_json), None);
        }
    }
    broadcast_chat_pts(state, target_cid, pts_update);

    bot_ok(json!({
        "inline_message_id": format!("webapp:{}:{}:{}", target_cid.0, saved_id.0, qid)
    }))
}

fn handle_set_my_commands(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let raw_cmds = match params.get("commands") {
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok(),
        Some(v) => Some(v.clone()),
        None => None,
    };
    let Some(Value::Array(arr)) = raw_cmds else {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: commands must be a JSON array",
        );
    };
    let mut specs = Vec::with_capacity(arr.len());
    for item in arr {
        let raw_cmd = item.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let clean_cmd = match validate_bot_command(raw_cmd) {
            Ok(c) => c,
            Err(e) => return bot_err(StatusCode::BAD_REQUEST, 400, &format!("Bad Request: {e}")),
        };
        let description = item
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if description.is_empty() {
            return bot_err(
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: command description cannot be empty",
            );
        }
        specs.push(BotCommandSpec {
            command: clean_cmd,
            description,
        });
    }
    let json_str = serde_json::to_string(&specs).unwrap_or_else(|_| "[]".into());
    match state.db.update_bot_commands(bot.bot_user_id, &json_str) {
        Ok(_) => bot_ok(json!(true)),
        Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
    }
}

fn handle_get_my_commands(bot: &BotRegistryRecord) -> Response {
    let cmds: Vec<BotCommandSpec> = serde_json::from_str(&bot.commands_json).unwrap_or_default();
    bot_ok(json!(cmds))
}

fn handle_delete_my_commands(state: &AppState, bot: &BotRegistryRecord) -> Response {
    match state.db.update_bot_commands(bot.bot_user_id, "[]") {
        Ok(_) => bot_ok(json!(true)),
        Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
    }
}

fn handle_set_chat_menu_button(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let mb_val = match params.get("menu_button") {
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok(),
        Some(v) if v.is_object() => Some(v.clone()),
        _ => None,
    };
    let Some(mb) = mb_val else {
        // When menu_button is omitted in Telegram Bot API, reset to default
        return match state
            .db
            .update_bot_menu_button_typed(bot.bot_user_id, "default", "", "")
        {
            Ok(_) => bot_ok(json!(true)),
            Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
        };
    };

    let mb_type = mb
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .trim()
        .to_ascii_lowercase();
    match mb_type.as_str() {
        "default" => match state
            .db
            .update_bot_menu_button_typed(bot.bot_user_id, "default", "", "")
        {
            Ok(_) => bot_ok(json!(true)),
            Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
        },
        "commands" => match state
            .db
            .update_bot_menu_button_typed(bot.bot_user_id, "commands", "", "")
        {
            Ok(_) => bot_ok(json!(true)),
            Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
        },
        "web_app" => {
            let text = mb.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
            let url = mb
                .get("web_app")
                .and_then(|w| w.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if text.is_empty() || url.is_empty() {
                return bot_err(
                    StatusCode::BAD_REQUEST,
                    400,
                    "Bad Request: web_app menu button requires text and web_app.url",
                );
            }
            match state
                .db
                .update_bot_menu_button_typed(bot.bot_user_id, "web_app", text, url)
            {
                Ok(_) => bot_ok(json!(true)),
                Err(e) => bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string()),
            }
        }
        _ => bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: invalid menu_button type",
        ),
    }
}

fn handle_get_chat_menu_button(bot: &BotRegistryRecord) -> Response {
    let mb_type = if bot.menu_button_type.trim().is_empty() {
        if !bot.menu_button_url.trim().is_empty() {
            "web_app"
        } else {
            "default"
        }
    } else {
        bot.menu_button_type.trim()
    };
    match mb_type {
        "web_app" => bot_ok(json!({
            "type": "web_app",
            "text": if bot.menu_button_text.trim().is_empty() { "Open App" } else { bot.menu_button_text.trim() },
            "web_app": { "url": bot.menu_button_url }
        })),
        "commands" => bot_ok(json!({ "type": "commands" })),
        _ => bot_ok(json!({ "type": "default" })),
    }
}

fn handle_set_my_description(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let desc = param_str(params, "description").unwrap_or("").trim();
    match state.db.update_bot_description(bot.bot_user_id, desc) {
        Ok(_) => bot_ok(json!(true)),
        Err(e) => bot_err(StatusCode::BAD_REQUEST, 400, &e.to_string()),
    }
}

fn handle_get_my_description(bot: &BotRegistryRecord) -> Response {
    bot_ok(json!({ "description": bot.description }))
}

fn handle_set_my_short_description(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let short = param_str(params, "short_description").unwrap_or("").trim();
    match state.db.update_bot_about(bot.bot_user_id, short) {
        Ok(_) => bot_ok(json!(true)),
        Err(e) => bot_err(StatusCode::BAD_REQUEST, 400, &e.to_string()),
    }
}

fn handle_get_my_short_description(bot: &BotRegistryRecord) -> Response {
    bot_ok(json!({ "short_description": bot.about }))
}

fn handle_set_my_name(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let name = param_str(params, "name").unwrap_or("").trim();
    if name.is_empty() {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: name cannot be empty",
        );
    }
    match state.db.update_bot_name(bot.bot_user_id, name) {
        Ok(_) => bot_ok(json!(true)),
        Err(e) => bot_err(StatusCode::BAD_REQUEST, 400, &e.to_string()),
    }
}

fn handle_get_my_name(bot: &BotRegistryRecord) -> Response {
    bot_ok(json!({ "name": bot.display_name }))
}

fn handle_set_my_privacy_policy(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let url = param_str(params, "privacy_policy_url")
        .or_else(|| param_str(params, "url"))
        .unwrap_or("")
        .trim();
    match state.db.update_bot_privacy_policy(bot.bot_user_id, url) {
        Ok(_) => bot_ok(json!(true)),
        Err(e) => bot_err(StatusCode::BAD_REQUEST, 400, &e.to_string()),
    }
}

fn handle_get_my_privacy_policy(bot: &BotRegistryRecord) -> Response {
    bot_ok(json!({ "privacy_policy_url": bot.privacy_policy_url }))
}

fn handle_publish_mini_app(
    state: &AppState,
    bot: &BotRegistryRecord,
    params: &Map<String, Value>,
) -> Response {
    let short_name = param_str(params, "short_name").unwrap_or("").trim();
    let title = param_str(params, "title").unwrap_or("").trim();
    if short_name.len() < 3 || title.is_empty() {
        return bot_err(
            StatusCode::BAD_REQUEST,
            400,
            "Bad Request: short_name (>=3 chars) and title are required",
        );
    }
    let description = param_str(params, "description").unwrap_or("").trim();
    let category = param_str(params, "category").unwrap_or("developer").trim();
    let icon_emoji = param_str(params, "icon_emoji").unwrap_or("🧩").trim();
    let html_bundle = param_str(params, "html_bundle");
    let raw_web_url = param_str(params, "web_url").unwrap_or("").trim();
    let is_main_app = param_bool(params, "is_main_app");
    let header_color = param_str(params, "header_color").unwrap_or("").trim();
    let background_color = param_str(params, "background_color").unwrap_or("").trim();
    let ui_schema_json = params
        .get("ui_schema")
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".into()))
        .unwrap_or_else(|| "{}".to_string());

    let app = match state.db.create_or_upsert_mini_app_with_options(
        bot.bot_user_id,
        &bot.username,
        short_name,
        title,
        description,
        category,
        icon_emoji,
        if raw_web_url.is_empty() {
            None
        } else {
            Some(raw_web_url)
        },
        &ui_schema_json,
        true,
        param_bool(params, "is_attach_enabled").unwrap_or(false) || bot.allow_attach_menu,
        false,
        header_color,
        background_color,
        is_main_app,
        false,
    ) {
        Ok(a) => a,
        Err(e) => return bot_err(StatusCode::BAD_REQUEST, 400, &e.to_string()),
    };

    if let Some(html) = html_bundle.filter(|h| !h.trim().is_empty()) {
        if let Err(e) = state.db.upsert_mini_app_hosted_bundle(&app.app_id, html) {
            return bot_err(StatusCode::INTERNAL_SERVER_ERROR, 500, &e.to_string());
        }
    }

    let entry = miniapp_record_to_entry(state, &app, bot.owner_user_id, None);
    let _ = bot_record_to_entry(bot);
    bot_ok(serde_json::to_value(&entry).unwrap_or_else(|_| json!({ "app_id": app.app_id })))
}

// ============================================================================
// Official-Compatible `/js/telegram-web-app.js` Bridge Runtime
// ============================================================================

/// GET /js/telegram-web-app.js
pub async fn serve_telegram_web_app_js() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        TELEGRAM_WEB_APP_JS,
    )
        .into_response()
}

const TELEGRAM_WEB_APP_JS: &str = r#"(function () {
  if (window.Telegram && window.Telegram.WebApp && window.Telegram.WebApp._gpuiBridgeReady) {
    return;
  }

  function parseInitData(raw) {
    var result = {};
    if (!raw) return result;
    try {
      var params = new URLSearchParams(raw);
      params.forEach(function (value, key) {
        if (key === 'user' || key === 'receiver' || key === 'chat') {
          try {
            result[key] = JSON.parse(value);
          } catch (_) {
            result[key] = value;
          }
        } else {
          result[key] = value;
        }
      });
    } catch (_) {}
    return result;
  }

  function extractInitDataFromLocation() {
    if (typeof window.__TG_INIT_DATA__ === 'string' && window.__TG_INIT_DATA__) {
      return window.__TG_INIT_DATA__;
    }
    try {
      var qs = new URLSearchParams(window.location.search);
      if (qs.get('tgWebAppData')) return qs.get('tgWebAppData');
      if (qs.get('init_data')) return qs.get('init_data');
      var hash = (window.location.hash || '').replace(/^#/, '');
      if (hash) {
        var hs = new URLSearchParams(hash);
        if (hs.get('tgWebAppData')) return hs.get('tgWebAppData');
        if (hs.get('init_data')) return hs.get('init_data');
      }
    } catch (_) {}
    return '';
  }

  function postToHost(eventType, eventData) {
    var payload = eventData === undefined ? {} : eventData;
    try {
      if (window.TelegramWebviewProxy && typeof window.TelegramWebviewProxy.postEvent === 'function') {
        window.TelegramWebviewProxy.postEvent(eventType, JSON.stringify(payload));
      }
    } catch (_) {}
    try {
      if (
        window.webkit &&
        window.webkit.messageHandlers &&
        window.webkit.messageHandlers.TelegramWebviewProxy &&
        typeof window.webkit.messageHandlers.TelegramWebviewProxy.postMessage === 'function'
      ) {
        window.webkit.messageHandlers.TelegramWebviewProxy.postMessage({
          eventType: eventType,
          eventData: payload
        });
      }
    } catch (_) {}
    try {
      if (window.parent && window.parent !== window && typeof window.parent.postMessage === 'function') {
        window.parent.postMessage(
          { type: 'telegram_web_app_event', eventType: eventType, eventData: payload },
          '*'
        );
      }
    } catch (_) {}
  }

  var listeners = {};
  var pendingWriteCallbacks = [];
  var pendingPopupCallbacks = [];

  function emitEvent(eventType, eventData) {
    var list = listeners[eventType] || [];
    for (var i = 0; i < list.length; i++) {
      try {
        list[i](eventData);
      } catch (_) {}
    }
  }

  window.addEventListener('message', function (ev) {
    var data = ev && ev.data;
    if (!data || typeof data !== 'object') return;
    if (data.eventType) {
      if (data.eventType === 'main_button_pressed') {
        mainButton._fireClick();
      } else if (data.eventType === 'secondary_button_pressed') {
        secondaryButton._fireClick();
      } else if (data.eventType === 'back_button_pressed') {
        backButton._fireClick();
      } else if (data.eventType === 'settings_button_pressed') {
        settingsButton._fireClick();
      } else if (data.eventType === 'write_access_requested') {
        var allowed = !!(data.eventData && data.eventData.status === 'allowed');
        if (allowed && webApp.initDataUnsafe && webApp.initDataUnsafe.user) {
          webApp.initDataUnsafe.user.allows_write_to_pm = true;
        }
        while (pendingWriteCallbacks.length) {
          var cb = pendingWriteCallbacks.shift();
          try { cb(allowed); } catch (_) {}
        }
      } else if (data.eventType === 'popup_closed') {
        while (pendingPopupCallbacks.length) {
          var pcb = pendingPopupCallbacks.shift();
          try { pcb(data.eventData ? data.eventData.button_id : null); } catch (_) {}
        }
      }
      emitEvent(data.eventType, data.eventData);
    }
  });

  function makeBottomButton(isSecondary) {
    var clickCallbacks = [];
    var btn = {
      text: isSecondary ? 'Cancel' : 'Continue',
      color: isSecondary ? '#f3f4f6' : '#2481cc',
      textColor: isSecondary ? '#111827' : '#ffffff',
      isVisible: false,
      isActive: true,
      isProgressVisible: false,
      hasShineEffect: false,
      position: isSecondary ? 'left' : 'bottom',
      setText: function (t) {
        btn.text = String(t || '');
        btn._sync();
        return btn;
      },
      onClick: function (fn) {
        if (typeof fn === 'function') clickCallbacks.push(fn);
        return btn;
      },
      offClick: function (fn) {
        clickCallbacks = clickCallbacks.filter(function (c) { return c !== fn; });
        return btn;
      },
      show: function () {
        btn.isVisible = true;
        btn._sync();
        return btn;
      },
      hide: function () {
        btn.isVisible = false;
        btn._sync();
        return btn;
      },
      enable: function () {
        btn.isActive = true;
        btn._sync();
        return btn;
      },
      disable: function () {
        btn.isActive = false;
        btn._sync();
        return btn;
      },
      showProgress: function () {
        btn.isProgressVisible = true;
        btn._sync();
        return btn;
      },
      hideProgress: function () {
        btn.isProgressVisible = false;
        btn._sync();
        return btn;
      },
      setParams: function (p) {
        if (!p || typeof p !== 'object') return btn;
        if (p.text !== undefined) btn.text = String(p.text);
        if (p.color !== undefined) btn.color = String(p.color);
        if (p.text_color !== undefined) btn.textColor = String(p.text_color);
        if (p.is_active !== undefined) btn.isActive = !!p.is_active;
        if (p.is_visible !== undefined) btn.isVisible = !!p.is_visible;
        if (p.has_shine_effect !== undefined) btn.hasShineEffect = !!p.has_shine_effect;
        if (p.position !== undefined) btn.position = String(p.position);
        btn._sync();
        return btn;
      },
      _sync: function () {
        postToHost(
          isSecondary ? 'web_app_setup_secondary_button' : 'web_app_setup_main_button',
          {
            is_visible: btn.isVisible,
            is_active: btn.isActive,
            is_progress_visible: btn.isProgressVisible,
            text: btn.text,
            color: btn.color,
            text_color: btn.textColor,
            has_shine_effect: btn.hasShineEffect,
            position: btn.position
          }
        );
      },
      _fireClick: function () {
        clickCallbacks.slice().forEach(function (fn) {
          try { fn(); } catch (_) {}
        });
      }
    };
    return btn;
  }

  var mainButton = makeBottomButton(false);
  var secondaryButton = makeBottomButton(true);

  var backClickCallbacks = [];
  var backButton = {
    isVisible: false,
    onClick: function (fn) {
      if (typeof fn === 'function') backClickCallbacks.push(fn);
      return backButton;
    },
    offClick: function (fn) {
      backClickCallbacks = backClickCallbacks.filter(function (c) { return c !== fn; });
      return backButton;
    },
    show: function () {
      backButton.isVisible = true;
      postToHost('web_app_setup_back_button', { is_visible: true });
      return backButton;
    },
    hide: function () {
      backButton.isVisible = false;
      postToHost('web_app_setup_back_button', { is_visible: false });
      return backButton;
    },
    _fireClick: function () {
      backClickCallbacks.slice().forEach(function (fn) {
        try { fn(); } catch (_) {}
      });
    }
  };

  var settingsClickCallbacks = [];
  var settingsButton = {
    isVisible: false,
    onClick: function (fn) {
      if (typeof fn === 'function') settingsClickCallbacks.push(fn);
      return settingsButton;
    },
    offClick: function (fn) {
      settingsClickCallbacks = settingsClickCallbacks.filter(function (c) { return c !== fn; });
      return settingsButton;
    },
    show: function () {
      settingsButton.isVisible = true;
      postToHost('web_app_setup_settings_button', { is_visible: true });
      return settingsButton;
    },
    hide: function () {
      settingsButton.isVisible = false;
      postToHost('web_app_setup_settings_button', { is_visible: false });
      return settingsButton;
    },
    _fireClick: function () {
      settingsClickCallbacks.slice().forEach(function (fn) {
        try { fn(); } catch (_) {}
      });
    }
  };

  function resolveApiBase() {
    if (typeof window.__TG_API_BASE__ === 'string' && window.__TG_API_BASE__) {
      return window.__TG_API_BASE__.replace(/\/$/, '');
    }
    var m = (window.location.pathname || '').match(/^\/miniapps\/([^/]+)\/([^/]+)/);
    if (m) {
      return '/api/v1/miniapps/' + m[1] + '/' + m[2];
    }
    return '';
  }

  function storageFetch(path, opts) {
    var base = resolveApiBase();
    if (!base) return Promise.reject(new Error('No Mini App storage endpoint'));
    var hdrs = Object.assign({ 'Content-Type': 'application/json' }, (opts && opts.headers) || {});
    if (typeof window.__TG_AUTH_TOKEN__ === 'string' && window.__TG_AUTH_TOKEN__) {
      hdrs['Authorization'] = 'Bearer ' + window.__TG_AUTH_TOKEN__;
    }
    return fetch(base + path, Object.assign({}, opts, { headers: hdrs })).then(function (r) {
      if (!r.ok) throw new Error('HTTP ' + r.status);
      return r.json();
    });
  }

  function localStorageKey() {
    return 'tg_cloud_storage:' + (resolveApiBase() || 'default');
  }
  function localStorageReadMap() {
    try {
      return JSON.parse(window.localStorage.getItem(localStorageKey()) || '{}') || {};
    } catch (_) {
      return {};
    }
  }
  function localStorageWriteMap(map) {
    try {
      window.localStorage.setItem(localStorageKey(), JSON.stringify(map));
    } catch (_) {}
  }

  var CloudStorage = {
    setItem: function (key, value, cb) {
      var k = String(key);
      var v = String(value);
      return storageFetch('/storage', {
        method: 'PUT',
        body: JSON.stringify({ key: k, value: v })
      })
        .catch(function () {
          var m = localStorageReadMap();
          m[k] = v;
          localStorageWriteMap(m);
          return true;
        })
        .then(function () {
          if (cb) cb(null, true);
          return true;
        })
        .catch(function (e) {
          if (cb) cb(e, false);
          return false;
        });
    },
    getItem: function (key, cb) {
      var k = String(key);
      return storageFetch('/storage', { method: 'GET' })
        .then(function (j) {
          return (j.entries && Object.prototype.hasOwnProperty.call(j.entries, k)) ? j.entries[k] : '';
        })
        .catch(function () {
          var m = localStorageReadMap();
          return Object.prototype.hasOwnProperty.call(m, k) ? m[k] : '';
        })
        .then(function (val) {
          if (cb) cb(null, val);
          return val;
        });
    },
    getItems: function (keys, cb) {
      var list = Array.isArray(keys) ? keys : [];
      return storageFetch('/storage', { method: 'GET' })
        .then(function (j) { return j.entries || {}; })
        .catch(function () { return localStorageReadMap(); })
        .then(function (all) {
          var out = {};
          list.forEach(function (k) {
            out[k] = Object.prototype.hasOwnProperty.call(all, k) ? all[k] : '';
          });
          if (cb) cb(null, out);
          return out;
        });
    },
    removeItem: function (key, cb) {
      var k = String(key);
      return storageFetch('/storage/' + encodeURIComponent(k), { method: 'DELETE' })
        .catch(function () {
          var m = localStorageReadMap();
          delete m[k];
          localStorageWriteMap(m);
          return true;
        })
        .then(function () {
          if (cb) cb(null, true);
          return true;
        });
    },
    removeItems: function (keys, cb) {
      var list = Array.isArray(keys) ? keys : [];
      return Promise.all(list.map(function (k) { return CloudStorage.removeItem(k); }))
        .then(function () {
          if (cb) cb(null, true);
          return true;
        });
    },
    getKeys: function (cb) {
      return storageFetch('/storage', { method: 'GET' })
        .then(function (j) { return Object.keys(j.entries || {}); })
        .catch(function () { return Object.keys(localStorageReadMap()); })
        .then(function (keys) {
          if (cb) cb(null, keys);
          return keys;
        });
    }
  };

  var rawInit = extractInitDataFromLocation();
  var unsafe = parseInitData(rawInit);

  var webApp = {
    _gpuiBridgeReady: true,
    initData: rawInit,
    initDataUnsafe: unsafe,
    version: '8.0',
    platform: 'macos',
    colorScheme: 'light',
    themeParams: {
      bg_color: '#ffffff',
      secondary_bg_color: '#f4f6f9',
      text_color: '#111827',
      hint_color: '#6b7280',
      link_color: '#2481cc',
      button_color: '#2481cc',
      button_text_color: '#ffffff',
      header_bg_color: '#ffffff'
    },
    headerColor: '#ffffff',
    backgroundColor: '#ffffff',
    bottomBarColor: '#ffffff',
    isExpanded: true,
    isFullscreen: false,
    isClosingConfirmationEnabled: false,
    isVerticalSwipesEnabled: true,
    viewportHeight: window.innerHeight || 720,
    viewportStableHeight: window.innerHeight || 720,
    MainButton: mainButton,
    BottomButton: mainButton,
    SecondaryButton: secondaryButton,
    BackButton: backButton,
    SettingsButton: settingsButton,
    CloudStorage: CloudStorage,
    HapticFeedback: {
      impactOccurred: function (style) {
        postToHost('web_app_trigger_haptic_feedback', { type: 'impact', impact_style: style || 'medium' });
      },
      notificationOccurred: function (type) {
        postToHost('web_app_trigger_haptic_feedback', { type: 'notification', notification_type: type || 'success' });
      },
      selectionChanged: function () {
        postToHost('web_app_trigger_haptic_feedback', { type: 'selection_change' });
      }
    },
    ready: function () {
      postToHost('web_app_ready', {});
    },
    expand: function () {
      webApp.isExpanded = true;
      postToHost('web_app_expand', {});
    },
    requestFullscreen: function () {
      webApp.isFullscreen = true;
      postToHost('web_app_request_fullscreen', {});
      emitEvent('fullscreenChanged', { is_fullscreen: true });
    },
    exitFullscreen: function () {
      webApp.isFullscreen = false;
      postToHost('web_app_exit_fullscreen', {});
      emitEvent('fullscreenChanged', { is_fullscreen: false });
    },
    close: function () {
      postToHost('web_app_close', {});
    },
    enableClosingConfirmation: function () {
      webApp.isClosingConfirmationEnabled = true;
      postToHost('web_app_setup_closing_behavior', { need_confirmation: true });
    },
    disableClosingConfirmation: function () {
      webApp.isClosingConfirmationEnabled = false;
      postToHost('web_app_setup_closing_behavior', { need_confirmation: false });
    },
    setHeaderColor: function (color) {
      webApp.headerColor = String(color || '#ffffff');
      postToHost('web_app_set_header_color', { color: webApp.headerColor });
    },
    setBackgroundColor: function (color) {
      webApp.backgroundColor = String(color || '#ffffff');
      postToHost('web_app_set_background_color', { color: webApp.backgroundColor });
    },
    sendData: function (data) {
      var str = typeof data === 'string' ? data : JSON.stringify(data);
      postToHost('web_app_data_send', { data: str });
      var base = resolveApiBase();
      if (base && typeof window.__TG_AUTH_TOKEN__ === 'string' && window.__TG_AUTH_TOKEN__) {
        fetch(base + '/action', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'Authorization': 'Bearer ' + window.__TG_AUTH_TOKEN__
          },
          body: JSON.stringify({
            action_type: 'send_data',
            data: str,
            query_id: (webApp.initDataUnsafe && webApp.initDataUnsafe.query_id) || undefined
          })
        }).catch(function () {});
      }
    },
    requestWriteAccess: function (cb) {
      return new Promise(function (resolve) {
        var settled = false;
        function finish(allowed) {
          if (settled) return;
          settled = true;
          if (allowed && webApp.initDataUnsafe && webApp.initDataUnsafe.user) {
            webApp.initDataUnsafe.user.allows_write_to_pm = true;
          }
          emitEvent('writeAccessRequested', { status: allowed ? 'allowed' : 'cancelled' });
          if (typeof cb === 'function') {
            try { cb(allowed); } catch (_) {}
          }
          resolve(allowed);
        }

        if (webApp.initDataUnsafe && webApp.initDataUnsafe.user && webApp.initDataUnsafe.user.allows_write_to_pm === true) {
          finish(true);
          return;
        }

        var hasNativeHost =
          (window.TelegramWebviewProxy && typeof window.TelegramWebviewProxy.postEvent === 'function') ||
          (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.TelegramWebviewProxy) ||
          (window.parent && window.parent !== window);

        if (hasNativeHost) {
          pendingWriteCallbacks.push(finish);
          postToHost('web_app_request_write_access', {});
          return;
        }

        var botId = window.__TG_BOT_ID__;
        var token = window.__TG_AUTH_TOKEN__;
        if (botId && token) {
          var allowed = window.confirm('允许该机器人向您发送消息吗？');
          if (allowed) {
            fetch('/api/v1/bots/' + botId + '/allow-send-message', {
              method: 'POST',
              headers: {
                'Content-Type': 'application/json',
                'Authorization': 'Bearer ' + token
              },
              body: JSON.stringify({ from_attachment_menu: false })
            })
              .then(function (r) { finish(r.ok); })
              .catch(function () { finish(false); });
          } else {
            finish(false);
          }
          return;
        }

        finish(false);
      });
    },
    openLink: function (url, options) {
      postToHost('web_app_open_link', { url: String(url || ''), options: options || {} });
      if (!window.TelegramWebviewProxy && window.parent === window) {
        window.open(String(url || ''), '_blank', 'noopener');
      }
    },
    openTelegramLink: function (url) {
      postToHost('web_app_open_tg_link', { path_full: String(url || '') });
    },
    showPopup: function (params, cb) {
      return new Promise(function (resolve) {
        function done(id) {
          if (typeof cb === 'function') {
            try { cb(id); } catch (_) {}
          }
          resolve(id);
        }
        var hasHost =
          (window.TelegramWebviewProxy && typeof window.TelegramWebviewProxy.postEvent === 'function') ||
          (window.parent && window.parent !== window);
        if (hasHost) {
          pendingPopupCallbacks.push(done);
          postToHost('web_app_open_popup', params || {});
        } else {
          window.alert((params && params.message) || '');
          done('ok');
        }
      });
    },
    showAlert: function (message, cb) {
      return webApp.showPopup({ message: String(message || ''), buttons: [{ type: 'ok' }] }, function () {
        if (typeof cb === 'function') cb();
      });
    },
    showConfirm: function (message, cb) {
      var hasHost =
        (window.TelegramWebviewProxy && typeof window.TelegramWebviewProxy.postEvent === 'function') ||
        (window.parent && window.parent !== window);
      if (!hasHost) {
        var res = window.confirm(String(message || ''));
        if (typeof cb === 'function') cb(res);
        return Promise.resolve(res);
      }
      return webApp.showPopup(
        {
          message: String(message || ''),
          buttons: [
            { id: 'ok', type: 'ok' },
            { id: 'cancel', type: 'cancel' }
          ]
        },
        function (id) {
          var ok = id === 'ok';
          if (typeof cb === 'function') cb(ok);
        }
      );
    },
    onEvent: function (eventType, cb) {
      if (!eventType || typeof cb !== 'function') return;
      if (!listeners[eventType]) listeners[eventType] = [];
      if (listeners[eventType].indexOf(cb) === -1) listeners[eventType].push(cb);
    },
    offEvent: function (eventType, cb) {
      if (!eventType || !listeners[eventType]) return;
      listeners[eventType] = listeners[eventType].filter(function (fn) { return fn !== cb; });
    }
  };

  window.Telegram = window.Telegram || {};
  window.Telegram.WebApp = webApp;
})();
"#;
