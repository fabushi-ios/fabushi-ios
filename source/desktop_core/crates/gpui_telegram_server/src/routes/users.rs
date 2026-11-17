use crate::auth::AuthBearer;
use crate::db::schema::{ChatMemberRecord, ChatRecord, UserRecord};
use crate::routes::{err_resp, map_telegram_err, user_record_to_profile};
use crate::server::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use chrono::Utc;
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, UserId};
use gpui_telegram_proto::contracts::{UpdateProfileRequest, UserPrivacySettingsDto};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn get_user_profile_response(state: &AppState, user_id: UserId) -> Response {
    match state.db.get_user(user_id) {
        Ok(Some(u)) => (StatusCode::OK, Json(user_record_to_profile(state, &u))).into_response(),
        Ok(None) => {
            if let Some(u) = crate::server::sync_mem_user_to_db(state, user_id) {
                (StatusCode::OK, Json(user_record_to_profile(state, &u))).into_response()
            } else {
                err_resp(StatusCode::NOT_FOUND, "not_found", "User not found")
            }
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn get_me_profile(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let user_opt = match state.db.get_user(auth.user_id) {
        Ok(Some(u)) => Some(u),
        Ok(None) => crate::server::sync_mem_user_to_db(&state, auth.user_id),
        Err(e) => return map_telegram_err(e),
    };
    let Some(u) = user_opt else {
        return err_resp(StatusCode::NOT_FOUND, "not_found", "User not found");
    };
    let fence_gen = state.session_manager.get_or_create_fence(auth.user_id).current();
    let mut val = serde_json::to_value(user_record_to_profile(&state, &u)).unwrap_or_default();
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "account_generation".into(),
            serde_json::json!(u.account_generation.max(fence_gen)),
        );
        obj.insert("session_id".into(), serde_json::json!(auth.session.session_id));
    }
    (StatusCode::OK, Json(val)).into_response()
}

pub async fn get_user_by_id(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<u64>,
) -> Response {
    get_user_profile_response(&state, UserId(user_id))
}

pub async fn update_me_profile(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateProfileRequest>,
) -> Response {
    if let Some(ref fname) = req.first_name {
        let trimmed = fname.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 64 {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_first_name",
                "first_name must be 1-64 non-whitespace characters",
            );
        }
    }
    if let Some(ref uname) = req.username {
        let clean = uname.trim().trim_start_matches('@');
        if clean.is_empty() || clean.chars().count() > 64 {
            return err_resp(StatusCode::BAD_REQUEST, "invalid_username", "Username must be 1-64 chars");
        }
        if let Ok(Some(existing)) = state.db.get_user_by_username(clean) {
            if existing.user_id != auth.user_id {
                return err_resp(StatusCode::CONFLICT, "username_taken", "Username is already taken");
            }
        }
    }
    if let Some(ref phone_raw) = req.phone {
        let clean_phone = phone_raw.trim();
        if !clean_phone.is_empty() {
            if let Ok(Some(existing)) = state.db.get_user_by_phone(clean_phone) {
                if existing.user_id != auth.user_id {
                    return err_resp(StatusCode::CONFLICT, "phone_taken", "Phone number is already registered");
                }
            }
        }
    }
    let uname_clean = req.username.as_deref().map(|s| s.trim().trim_start_matches('@'));
    let phone_clean = req
        .phone
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match state.db.update_user_profile(
        auth.user_id,
        req.first_name.as_deref(),
        req.last_name.as_deref(),
        req.bio.as_deref(),
        uname_clean,
        req.avatar_media_id.as_deref(),
    ) {
        Ok(mut user) => {
            if req.emoji_status.is_some() || phone_clean.is_some() {
                let _ = state.db.update_user_emoji_and_phone(
                    auth.user_id,
                    req.emoji_status.as_deref(),
                    phone_clean,
                );
                user = state.db.get_user(auth.user_id).ok().flatten().unwrap_or(user);
            }
            if let Some(mut mem_u) = state.session_manager.get_user(auth.user_id) {
                mem_u.first_name = user.first_name.clone();
                mem_u.last_name = user.last_name.clone();
                if let Some(ref un) = user.username { mem_u.username = un.clone(); }
                state.session_manager.insert_user(mem_u);
            }
            let profile = user_record_to_profile(&state, &user);
            (StatusCode::OK, Json(profile)).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn resolve_peer(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if let Some(phone) = params.get("phone").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if let Ok(Some(u)) = state.db.get_user_by_phone(phone) {
            return (StatusCode::OK, Json(json!({ "kind": "user", "user": user_record_to_profile(&state, &u) }))).into_response();
        }
    }
    let raw_uname = params.get("username").map(|s| s.trim().trim_start_matches('@')).unwrap_or("");
    if raw_uname.is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_query", "username or phone query param required");
    }
    let uname = if raw_uname == "机器人之父" || raw_uname.eq_ignore_ascii_case("botfather") {
        "BotFather"
    } else {
        raw_uname
    };
    if let Some((bot_part, short_part)) = uname.split_once('/') {
        if let Ok(Some(app)) = state.db.get_mini_app(bot_part, short_part) {
            let bot_rec = state.db.get_bot_by_user_id(app.bot_user_id).ok().flatten();
            return (
                StatusCode::OK,
                Json(json!({
                    "kind": "mini_app",
                    "mini_app": app,
                    "bot": bot_rec.map(|b| json!({
                        "bot_user_id": b.bot_user_id,
                        "username": b.username,
                        "display_name": b.display_name,
                        "description": b.description,
                        "is_official": b.is_official,
                    })),
                })),
            )
                .into_response();
        }
    }
    if let Ok(Some(u)) = state.db.get_user_by_username(uname) {
        return (StatusCode::OK, Json(json!({ "kind": "user", "user": user_record_to_profile(&state, &u) }))).into_response();
    }
    if let Ok(Some(bot)) = state.db.get_bot_by_username(uname) {
        if let Ok(Some(u)) = state.db.get_user(UserId(bot.bot_user_id as u64)) {
            return (StatusCode::OK, Json(json!({ "kind": "user", "user": user_record_to_profile(&state, &u) }))).into_response();
        }
    }
    if let Ok(Some(c)) = state.db.get_chat_by_username(uname) {
        return (StatusCode::OK, Json(json!({ "kind": "chat", "chat_id": c.chat_id.0, "title": c.title, "username": c.username }))).into_response();
    }
    err_resp(StatusCode::NOT_FOUND, "peer_not_found", "No user or chat matches that identifier")
}

pub async fn contacts_list_all(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    let mut contacts = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(list) = state.db.list_contacts(auth.user_id) {
        for c in list {
            seen.insert(UserId(c.user_id));
            contacts.push(serde_json::to_value(&c).unwrap_or(json!({})));
        }
    }
    if let Ok(chats) = state.db.list_user_chats(auth.user_id) {
        for ch in chats {
            if matches!(ch.chat_type, 0 | 4) {
                if let Ok(members) = state.db.get_chat_members(ch.chat_id) {
                    for m in members {
                        if m != auth.user_id && seen.insert(m) {
                            if let Ok(Some(u)) = state.db.get_user(m) {
                                contacts.push(json!({
                                    "user_id": u.user_id.0, "chat_id": ch.chat_id.0, "first_name": u.first_name,
                                    "last_name": u.last_name, "username": u.username, "phone": u.phone,
                                    "bio": u.bio, "is_bot": u.is_bot, "is_mutual": false,
                                }));
                            }
                        }
                    }
                }
            }
        }
    }
    (StatusCode::OK, Json(json!({ "contacts": contacts, "total": contacts.len() }))).into_response()
}

pub async fn contacts_add_or_upsert(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let raw_first_name = payload
        .get("first_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let last_name = payload
        .get("last_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let explicit_is_bot = payload.get("is_bot").and_then(|v| v.as_bool()).unwrap_or(false);
    let phone = payload
        .get("phone")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let bot_prompt = payload
        .get("bot_system_prompt")
        .or_else(|| payload.get("system_prompt"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let persona = payload
        .get("persona")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let model = payload
        .get("model")
        .or_else(|| payload.get("bot_model"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let welcome_message = payload
        .get("welcome_message")
        .or_else(|| payload.get("greeting"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let avatar_shape = payload
        .get("avatar_shape")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let avatar_color = payload
        .get("avatar_color")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut raw_un = payload
        .get("username")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().trim_start_matches('@').to_string())
        .filter(|s| !s.is_empty());

    let is_botfather_alias = raw_un
        .as_deref()
        .is_some_and(|u| u.eq_ignore_ascii_case("botfather") || u == "机器人之父")
        || raw_first_name.is_some_and(|f| f.eq_ignore_ascii_case("botfather") || f == "机器人之父");
    if is_botfather_alias {
        raw_un = Some("BotFather".to_string());
    }

    let has_bot_config =
        bot_prompt.is_some() || persona.is_some() || model.is_some() || welcome_message.is_some();
    let username_looks_bot = is_botfather_alias
        || raw_un
            .as_deref()
            .is_some_and(|u| u.to_ascii_lowercase().ends_with("bot"));

    let now_ms = Utc::now().timestamp_millis();
    let pre_is_bot = explicit_is_bot || has_bot_config || username_looks_bot;
    let slug = raw_un.clone().unwrap_or_else(|| {
        let seed_name = raw_first_name.unwrap_or("New Contact");
        let base: String = seed_name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<String>()
            .to_lowercase();
        let s = if base.is_empty() {
            format!("peer_{}", now_ms.unsigned_abs() % 100_000)
        } else {
            base
        };
        if pre_is_bot && !s.ends_with("bot") {
            format!("{}_bot", s)
        } else {
            s
        }
    });

    let target_uid = if let Some(uid_num) = payload.get("user_id").and_then(|v| v.as_u64()) {
        UserId(uid_num)
    } else if is_botfather_alias {
        UserId(crate::db::miniapp_store::BOTFATHER_USER_ID)
    } else if let Ok(Some(bot)) = state.db.get_bot_by_username(&slug) {
        UserId(bot.bot_user_id as u64)
    } else if let Ok(Some(ex)) = state.db.get_user_by_username(&slug) {
        if ex.user_id != auth.user_id {
            ex.user_id
        } else {
            UserId(500_000 + (Utc::now().timestamp_micros().unsigned_abs() % 400_000))
        }
    } else {
        UserId(500_000 + (Utc::now().timestamp_micros().unsigned_abs() % 400_000))
    };

    let existing_user = state.db.get_user(target_uid).ok().flatten();
    let first_name: String = raw_first_name
        .map(ToString::to_string)
        .or_else(|| existing_user.as_ref().map(|u| u.first_name.clone()))
        .unwrap_or_else(|| "New Contact".to_string());
    let is_bot = existing_user.as_ref().is_some_and(|u| u.is_bot)
        || explicit_is_bot
        || has_bot_config
        || (existing_user.is_none() && username_looks_bot);

    let db_un = if state
        .db
        .get_user_by_username(&slug)
        .ok()
        .flatten()
        .is_some_and(|u| u.user_id != target_uid)
    {
        format!("{}_{}", slug, target_uid.0)
    } else {
        slug.clone()
    };

    let full_title = match &last_name {
        Some(ln) => format!("{} {}", first_name, ln),
        None => first_name.clone(),
    };
    let effective_bio = if is_bot {
        Some(match (&persona, &bot_prompt) {
            (Some(p), Some(sp)) if p != sp => format!("{}\n{}", p, sp),
            (Some(p), _) => p.clone(),
            (_, Some(sp)) => sp.clone(),
            (None, None) => format!("{} — Hermes AI assistant (@{})", full_title, db_un),
        })
    } else {
        persona.clone().or_else(|| bot_prompt.clone())
    };

    if existing_user.is_none() {
        let mut nu = UserRecord {
            user_id: target_uid,
            username: Some(db_un.clone()),
            phone: phone.clone(),
            first_name: first_name.clone(),
            last_name: last_name.clone(),
            bio: effective_bio.clone(),
            password_hash: None,
            is_bot,
            is_verified: is_bot,
            is_support: false,
            is_scam: false,
            is_fake: false,
            profile_photo_file_id: None,
            account_generation: 1,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        if state.db.create_user(&nu).is_err() {
            nu.phone = None;
            nu.username = Some(format!("{}_{}", slug, target_uid.0));
            let _ = state.db.create_user(&nu);
        }
    }
    let _ = state.db.upsert_contact(
        auth.user_id,
        target_uid,
        &first_name,
        last_name.as_deref(),
        phone.as_deref(),
    );

    let creator_global_ttl = state
        .db
        .get_user_privacy_settings(auth.user_id)
        .map(|s| s.global_ttl_seconds)
        .unwrap_or(0);

    let chat_type: u8 = if is_bot { 4 } else { 0 };
    let direct_cid = if is_bot {
        let (bot_cid, created_bot_chat) = state
            .db
            .ensure_companion_bot_chat_record(auth.user_id, target_uid)
            .unwrap_or_else(|_| {
                (
                    ChatId(
                        600_000_000
                            + ((auth.user_id.0 as i64 % 9_999) * 10_000)
                            + ((target_uid.0 as i64 % 9_999) + 1),
                    ),
                    false,
                )
            });
        if creator_global_ttl > 0
            && (created_bot_chat || state.db.get_chat_ttl(bot_cid).unwrap_or(0) == 0)
        {
            let _ = state.db.set_chat_ttl(bot_cid, creator_global_ttl);
        }
        state.gateway_hub.subscribe_chat(bot_cid.0, auth.user_id);
        state.gateway_hub.subscribe_chat(bot_cid.0, target_uid);

        let existing_msgs = state
            .db
            .list_messages_with_topic(bot_cid, None, None, 1)
            .unwrap_or_default();
        if existing_msgs.is_empty() {
            if target_uid.0 == crate::db::miniapp_store::BOTFATHER_USER_ID {
                let text = crate::routes::bots_miniapps::build_botfather_welcome_text();
                let kb = crate::routes::bots_miniapps::build_botfather_welcome_keyboard();
                let _ = crate::routes::bots_miniapps::send_bot_chat_reply(
                    &state,
                    bot_cid,
                    target_uid,
                    &text,
                    kb,
                );
            } else {
                let bot_apps: Vec<_> = state
                    .db
                    .list_mini_apps_for_bot(target_uid.0 as i64)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|a| a.is_published && a.category != "__bot_inline_webapp__")
                    .collect();
                if welcome_message.is_none() && !bot_apps.is_empty() {
                    let (text, kb) =
                        crate::routes::bots_miniapps::build_companion_bot_welcome_text_and_keyboard(
                            &full_title,
                            &db_un,
                            effective_bio.as_deref().unwrap_or(""),
                            &bot_apps,
                        );
                    let _ = crate::routes::bots_miniapps::send_bot_chat_reply(
                        &state,
                        bot_cid,
                        target_uid,
                        &text,
                        kb,
                    );
                } else {
                    let initial_text = welcome_message.clone().unwrap_or_else(|| {
                        let focus = persona
                            .as_deref()
                            .or(bot_prompt.as_deref())
                            .map(|s| format!(" Focus: {}.", s.chars().take(120).collect::<String>()))
                            .unwrap_or_default();
                        let model_note = model
                            .as_deref()
                            .map(|m| format!(" (model: {})", m))
                            .unwrap_or_default();
                        format!(
                            "Hello! I'm {} (@{}){}, your Hermes AI assistant.{} Ask me anything, run /help for slash commands, or use the quick actions below.",
                            full_title, db_un, model_note, focus
                        )
                    });
                    let mut msg = MessageItem::new(
                        MsgId(0),
                        bot_cid,
                        target_uid,
                        PeerId::Chat(bot_cid),
                        MessageContent::plain_text(initial_text),
                    );
                    msg.flags.is_out = false;
                    if let Ok(pts_update) = state.message_pipeline.process_incoming_message(msg) {
                        if let gpui_telegram_proto::sync::UpdateEvent::NewMessage(saved) =
                            &pts_update.event
                        {
                            let kb_json = json!([
                                [
                                    { "text": "✨ /help", "callback_data": "/help" },
                                    { "text": "🎙️ Talk Mode", "callback_data": "talk:start" }
                                ],
                                [
                                    { "text": "🧠 Memory", "callback_data": "/memory" },
                                    { "text": "🛠️ Tools", "callback_data": "/tools" }
                                ]
                            ])
                            .to_string();
                            let _ = state
                                .db
                                .save_message_extras(bot_cid, saved.id, Some(&kb_json), None);
                        }
                        crate::routes::broadcast_chat_pts(&state, bot_cid, pts_update);
                    }
                }
            }
        }
        bot_cid
    } else {
        let human_cid = ChatId(
            750_000_000 + ((auth.user_id.0 as i64 % 100_000) * 1_000_000) + (target_uid.0 as i64 % 1_000_000),
        );
        if state.db.get_chat(human_cid).ok().flatten().is_none() {
            let ch = ChatRecord {
                chat_id: human_cid,
                chat_type,
                peer_type: 1,
                title: full_title.clone(),
                username: Some(format!("dm_{}_{}", auth.user_id.0, target_uid.0)),
                about: effective_bio.clone(),
                creator_id: auth.user_id,
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
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            };
            let _ = state.db.create_chat(&ch);
            if creator_global_ttl > 0 {
                let _ = state.db.set_chat_ttl(human_cid, creator_global_ttl);
            }
            let members: Vec<(UserId, &str)> = if auth.user_id == target_uid {
                vec![(auth.user_id, "owner")]
            } else {
                vec![(auth.user_id, "owner"), (target_uid, "member")]
            };
            for (uid, role) in members {
                let _ = state.db.add_chat_member(&ChatMemberRecord {
                    chat_id: human_cid,
                    user_id: uid,
                    role: role.into(),
                    permissions_bitmask: 0xFFFF,
                    admin_title: None,
                    admin_rights: 0,
                    banned_rights: 0,
                    until_date_ms: None,
                    joined_at_ms: now_ms,
                    invited_by: Some(auth.user_id),
                    updated_at_ms: now_ms,
                });
            }
            let initial_text = format!("Added {} (@{})", full_title, db_un);
            let mut msg = MessageItem::new(
                MsgId(0),
                human_cid,
                target_uid,
                PeerId::Chat(human_cid),
                MessageContent::plain_text(initial_text),
            );
            msg.flags.is_out = false;
            let _ = state.message_pipeline.process_incoming_message(msg);
        }
        state.gateway_hub.subscribe_chat(human_cid.0, auth.user_id);
        state.gateway_hub.subscribe_chat(human_cid.0, target_uid);
        human_cid
    };
    let mut resolved_avatar_shape: Option<String> = None;
    let mut resolved_avatar_color_id: Option<String> = None;
    if is_bot && target_uid.0 != crate::db::miniapp_store::BOTFATHER_USER_ID {
        crate::routes::hermes::save_bot_persona_kv(
            &state,
            target_uid,
            Some(direct_cid),
            &full_title,
            persona.as_deref(),
            bot_prompt.as_deref(),
            model.as_deref(),
            welcome_message.as_deref(),
        );
        let default_persona = bot_prompt
            .as_deref()
            .or(persona.as_deref())
            .or(effective_bio.as_deref())
            .unwrap_or("");
        if let Ok(Some(mut rec)) = state.db.ensure_bot_registry_for_existing_bot_user(
            target_uid.0 as i64,
            auth.user_id.0 as i64,
            default_persona,
        ) {
            let model_opt = model.as_deref().map(str::trim).filter(|s| !s.is_empty());
            if model_opt.is_some() || avatar_shape.is_some() || avatar_color.is_some() {
                if let Ok(updated) = state.db.update_bot_fabushi_fields(
                    rec.bot_user_id,
                    None,
                    None,
                    None,
                    model_opt.map(|_| default_persona),
                    None,
                    avatar_shape.as_deref(),
                    avatar_color.as_deref(),
                    model_opt,
                    None,
                    None,
                    None,
                ) {
                    rec = updated;
                }
            }
            if let Some(inst) = crate::routes::bots_miniapps::provision_fabushi_bot_with_host(
                &state.db,
                &state.fabushi_bots,
                &rec,
            ) {
                let prof = inst.profile();
                resolved_avatar_shape = Some(prof.avatar_shape);
                resolved_avatar_color_id = Some(prof.avatar_color);
            } else {
                resolved_avatar_shape =
                    (!rec.avatar_shape.trim().is_empty()).then(|| rec.avatar_shape.clone());
                resolved_avatar_color_id =
                    (!rec.avatar_color.trim().is_empty()).then(|| rec.avatar_color.clone());
            }
        }
    }
    (
        StatusCode::CREATED,
        Json(json!({
            "user_id": target_uid.0,
            "chat_id": direct_cid.0,
            "chat_type": chat_type,
            "title": full_title,
            "first_name": first_name,
            "last_name": last_name,
            "username": db_un,
            "phone": phone,
            "is_bot": is_bot,
            "bio": effective_bio,
            "persona": persona,
            "system_prompt": bot_prompt,
            "model": model,
            "welcome_message": welcome_message,
            "message_ttl_seconds": state.db.get_chat_ttl(direct_cid).unwrap_or(0),
            "avatar_color": if is_bot { 0x2AABEE } else { 0x27AE60 },
            "avatar_shape": resolved_avatar_shape,
            "avatar_color_id": resolved_avatar_color_id,
        })),
    )
        .into_response()
}

pub async fn contacts_delete_user(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<u64>,
) -> Response {
    match state.db.delete_contact(auth.user_id, UserId(user_id)) {
        Ok(deleted) => (StatusCode::OK, Json(json!({ "deleted": deleted, "user_id": user_id }))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn blocked_users_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state.db.list_blocked_users(auth.user_id) {
        Ok(blocked) => (
            StatusCode::OK,
            Json(json!({ "blocked": blocked, "total": blocked.len() })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn block_user_handler(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<u64>,
) -> Response {
    match state.db.block_user(auth.user_id, UserId(user_id)) {
        Ok(item) => (StatusCode::OK, Json(item)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn unblock_user_handler(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<u64>,
) -> Response {
    match state.db.unblock_user(auth.user_id, UserId(user_id)) {
        Ok(unblocked) => (
            StatusCode::OK,
            Json(json!({ "unblocked": unblocked, "user_id": user_id })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn get_my_settings(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state.db.get_user_privacy_settings(auth.user_id) {
        Ok(s) => (StatusCode::OK, Json(s)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn put_my_settings(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<UserPrivacySettingsDto>,
) -> Response {
    match state.db.save_user_privacy_settings(auth.user_id, &req) {
        Ok(saved) => (StatusCode::OK, Json(saved)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}
