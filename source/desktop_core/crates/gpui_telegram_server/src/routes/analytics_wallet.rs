use crate::routes::{check_active_membership, map_telegram_err, parse_json_object};
use crate::search::schema::SearchQueryParams;
use crate::server::{extract_message_content_text, is_welcome_bot_chat, AppState, AuthBearer};
use axum::{body::Bytes, extract::{Path, Query, State}, http::StatusCode, response::{IntoResponse, Json, Response}};
use gpui_telegram_core::types::{ChatId, MsgId, UserId};
use gpui_telegram_proto::contracts::{SearchHitDto, StarGiftRequest};
use gpui_telegram_proto::message::MessageContent;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub async fn search_unified(
    auth: AuthBearer, State(state): State<Arc<AppState>>, Query(params): Query<HashMap<String, String>>,
) -> Response {
    crate::routes::enforce_ttl_expiry(&state).await;
    let q = params.get("q").map(|s| s.trim().to_string()).unwrap_or_default();
    let category = params.get("category").or_else(|| params.get("filter")).map(|s| s.trim().to_lowercase()).unwrap_or_else(|| "chats".into());
    let limit = params.get("limit").and_then(|s| s.parse::<usize>().ok()).unwrap_or(50).clamp(1, 100);
    let scoped_chat = match params.get("chat_id").and_then(|s| s.parse::<i64>().ok()).filter(|&c| c > 0) {
        Some(cid) => {
            if let Err(r) = check_active_membership(&state, ChatId(cid), auth.user_id) { return r; }
            Some(ChatId(cid))
        }
        None => None,
    };

    let member_chats: Vec<ChatId> = match scoped_chat {
        Some(cid) => vec![cid],
        None => state.db.list_user_member_chat_ids(auth.user_id).unwrap_or_default(),
    };
    let allowed_set: HashSet<i64> = member_chats.iter().map(|c| c.0).collect();
    let allowed_vec: Vec<i64> = member_chats.iter().map(|c| c.0).collect();
    let mode = params.get("mode").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
    let entity_hint = params.get("entity").map(|s| s.trim()).filter(|s| !s.is_empty());
    let mut hits: Vec<SearchHitDto> = Vec::new();
    let mut results: Vec<Value> = Vec::new();
    let mut bot_chat_map: HashMap<i64, i64> = HashMap::new();
    let q_lower = q.to_lowercase();
    let q_clean = q_lower.trim_start_matches('@');

    // 1. Scoped Hybrid Memory + Tantivy full-text search over message & file bodies
    if !q.is_empty() && matches!(category.as_str(), "all" | "chats" | "messages" | "hybrid" | "") {
        if mode == "hybrid" || category == "hybrid" {
            let hybrid_hits = state.hybrid_memory.search_with_limit(&q, None, &allowed_vec, entity_hint, limit);
            for h in hybrid_hits {
                if !allowed_set.contains(&h.chat_id) { continue; }
                let mid = if h.message_id.starts_with("file:") {
                    0
                } else {
                    h.message_id
                        .rsplit(':')
                        .next()
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0)
                };
                let (sender_id, date_ms, media_type) = if mid > 0 {
                    state.db.get_message(ChatId(h.chat_id), MsgId(mid)).ok().flatten()
                        .map(|r| (r.sender_id.0, r.date_ms, r.media_type))
                        .unwrap_or((0, 0, None))
                } else {
                    (0, 0, Some("file".to_string()))
                };
                hits.push(SearchHitDto {
                    chat_id: h.chat_id,
                    message_id: mid,
                    sender_id,
                    date_ms,
                    text: h.content.clone(),
                    snippet_html: h.content,
                    media_type,
                });
            }
        }
        let mut sq = SearchQueryParams::new(&q).with_limit(limit * 3);
        if let Some(cid) = scoped_chat { sq = sq.with_chat_id(cid.0.to_string()); }
        if hits.len() < limit {
            if let Ok(t_hits) = state.search_indexer.search_hits_for_chats(&q, &allowed_vec, limit * 2) {
                for h in t_hits {
                    if let Ok(cid_num) = h.chat_id.parse::<i64>() {
                        if !allowed_set.contains(&cid_num) { continue; }
                        let mid = if h.message_id.starts_with("file:") {
                    0
                } else {
                    h.message_id
                        .rsplit(':')
                        .next()
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0)
                };
                        if hits.iter().any(|existing| existing.chat_id == cid_num && existing.message_id == mid) {
                            continue;
                        }
                        let sid = h.sender_id.parse::<u64>().unwrap_or(0);
                        let Some(rec) = state
                            .db
                            .get_message(ChatId(cid_num), MsgId(mid))
                            .ok()
                            .flatten()
                            .filter(|r| r.deleted_at_ms.is_none())
                        else {
                            continue;
                        };
                        let text = crate::messaging::pipeline::MessagePipeline::record_to_item(&rec)
                            .ok()
                            .map(|i| extract_message_content_text(&i.content))
                            .unwrap_or_else(|| h.snippet_html.clone());
                        let media_type = rec.media_type;
                        hits.push(SearchHitDto {
                            chat_id: cid_num, message_id: mid, sender_id: sid, date_ms: h.date,
                            text, snippet_html: h.snippet_html, media_type,
                        });
                        if hits.len() >= limit { break; }
                    }
                }
            }
        }
        let _ = sq;
    }

    // 2. Categorized chat & media/link/file filter scan over authorized chats
    if let Ok(chats) = state.db.list_user_chats(auth.user_id) {
        for c in chats {
            if !allowed_set.contains(&c.chat_id.0) { continue; }
            let members = state.db.get_chat_members(c.chat_id).unwrap_or_default();
            let is_bot = is_welcome_bot_chat(c.chat_id) || c.chat_type == 4
                || c.username.as_deref().map(|u| u.to_lowercase().ends_with("bot")).unwrap_or(false)
                || (c.chat_type == 0 && members.iter().any(|&m| m != auth.user_id && state.db.get_user(m).ok().flatten().map(|u| u.is_bot).unwrap_or(false)));
            let ctype = if is_bot { "bot" } else if c.chat_type == 0 { "private" } else if c.chat_type == 3 { "channel" } else { "group" };
            let msgs = state.db.list_messages_with_topic(c.chat_id, None, None, 40).unwrap_or_default();
            let preview = msgs.last().and_then(|r| crate::messaging::pipeline::MessagePipeline::record_to_item(r).ok())
                .map(|i| extract_message_content_text(&i.content))
                .unwrap_or_else(|| c.about.clone().unwrap_or_default());

            if matches!(category.as_str(), "chats" | "channels" | "apps" | "bots" | "all" | "") {
                if is_bot {
                    for &m in &members {
                        if m != auth.user_id {
                            bot_chat_map.entry(m.0 as i64).or_insert(c.chat_id.0);
                        }
                    }
                }
                let cat_ok = if category == "apps" || category == "bots" { is_bot } else if category == "channels" { matches!(ctype, "channel" | "group") } else { true };
                let q_ok = q_lower.is_empty()
                    || c.title.to_lowercase().contains(&q_lower)
                    || preview.to_lowercase().contains(&q_lower)
                    || c.username
                        .as_deref()
                        .is_some_and(|u| u.to_lowercase().contains(q_clean));
                if cat_ok && q_ok {
                    results.push(json!({
                        "chat_id": c.chat_id.0, "title": c.title, "subtitle": preview,
                        "chat_type": ctype, "is_bot": is_bot, "kind": "chat",
                    }));
                }
            } else {
                for r in &msgs {
                    if let Ok(item) = crate::messaging::pipeline::MessagePipeline::record_to_item(r) {
                        let txt = extract_message_content_text(&item.content);
                        let matched = match category.as_str() {
                            "media" | "photos" | "videos" => matches!(item.content, MessageContent::Media(_) | MessageContent::Photo { .. } | MessageContent::Video { .. }),
                            "files" | "documents" => matches!(item.content, MessageContent::Document { .. } | MessageContent::Media(_)),
                            "music" | "audio" => matches!(item.content, MessageContent::Audio { .. }),
                            "voice" => matches!(item.content, MessageContent::Voice(_) | MessageContent::VideoNote(_)),
                            "links" => txt.contains("http://") || txt.contains("https://"),
                            _ => false,
                        };
                        if matched && (q_lower.is_empty() || txt.to_lowercase().contains(&q_lower)) {
                            hits.push(SearchHitDto { chat_id: c.chat_id.0, message_id: r.message_id.0, sender_id: r.sender_id.0, date_ms: r.date_ms, text: txt.clone(), snippet_html: txt.clone(), media_type: r.media_type.clone() });
                            results.push(json!({ "chat_id": c.chat_id.0, "message_id": r.message_id.0, "title": c.title, "subtitle": txt, "kind": category }));
                        }
                    }
                }
            }
        }
    }

    // 3. Global directory search for @BotFather, user-created bots, and published Mini Apps
    let mut global_apps: Vec<gpui_telegram_proto::miniapp_contracts::MiniAppEntry> = Vec::new();
    let mut global_bots: Vec<gpui_telegram_proto::miniapp_contracts::BotRegistryEntry> = Vec::new();
    let include_global_catalog = matches!(category.as_str(), "apps" | "bots" | "miniapps" | "all")
        || (!q_lower.is_empty() && matches!(category.as_str(), "chats" | ""));
    if scoped_chat.is_none() && include_global_catalog {
        if !matches!(category.as_str(), "miniapps") {
            if let Ok(bots) = state.db.list_all_bots() {
                for bot in bots {
                    let is_botfather = bot.bot_user_id == crate::db::miniapp_store::BOTFATHER_USER_ID as i64;
                    let q_ok = q_lower.is_empty()
                        || bot.username.to_lowercase().contains(q_clean)
                        || bot.display_name.to_lowercase().contains(&q_lower)
                        || bot.description.to_lowercase().contains(&q_lower)
                        || (is_botfather
                            && ("机器人之父".contains(&q_lower)
                                || q_lower.contains("机器人")
                                || q_lower.contains("botfather")));
                    if q_ok {
                        global_bots.push(
                            crate::routes::bots_miniapps::bot_record_to_entry_for_viewer(
                                &state,
                                &bot,
                                auth.user_id.0 as i64,
                            ),
                        );
                        if bot_chat_map.contains_key(&bot.bot_user_id) && q_lower.is_empty() {
                            // Already represented as an active bot chat in `results` unless specifically queried
                            continue;
                        }
                        let existing_cid = bot_chat_map.get(&bot.bot_user_id).copied();
                        results.push(json!({
                            "kind": "bot",
                            "bot_user_id": bot.bot_user_id,
                            "username": bot.username,
                            "title": bot.display_name,
                            "subtitle": format!("@{} · {}", bot.username, bot.description),
                            "description": bot.description,
                            "chat_id": existing_cid,
                            "chat_type": "bot",
                            "is_bot": true,
                            "is_official": bot.is_official,
                            "is_inline": bot.is_inline,
                            "allow_attach_menu": bot.allow_attach_menu,
                            "menu_button_text": bot.menu_button_text,
                            "menu_button_url": bot.menu_button_url,
                        }));
                    }
                }
            }
        }

        if !matches!(category.as_str(), "bots") {
            let installed_map: HashMap<String, (i64, bool)> = state
                .db
                .list_user_installed_mini_apps(auth.user_id.0 as i64)
                .unwrap_or_default()
                .into_iter()
                .map(|(app, install, _)| (app.app_id, (install.companion_chat_id, install.allow_write_access)))
                .collect();
            if let Ok(apps) = state.db.search_published_mini_apps(q_clean, None) {
                for app in apps {
                    let companion_cid = installed_map
                        .get(&app.app_id)
                        .map(|(cid, _)| *cid)
                        .or_else(|| bot_chat_map.get(&app.bot_user_id).copied());
                    global_apps.push(crate::routes::bots_miniapps::miniapp_record_to_entry(
                        &state,
                        &app,
                        auth.user_id.0 as i64,
                        Some(&installed_map),
                    ));
                    results.push(json!({
                        "kind": "mini_app",
                        "app_id": app.app_id,
                        "bot_user_id": app.bot_user_id,
                        "bot_username": app.bot_username,
                        "short_name": app.short_name,
                        "title": app.title,
                        "subtitle": format!("@{} /{} · {}", app.bot_username, app.short_name, app.description),
                        "description": app.description,
                        "category": app.category,
                        "icon_emoji": app.icon_emoji,
                        "web_url": app.web_url,
                        "chat_id": companion_cid,
                        "companion_chat_id": companion_cid,
                        "is_installed": installed_map.contains_key(&app.app_id),
                        "is_bot": false,
                        "install_count": app.install_count,
                    }));
                }
            }
        }
    }

    if results.is_empty() && !hits.is_empty() {
        for h in &hits {
            results.push(json!({ "chat_id": h.chat_id, "message_id": h.message_id, "sender_id": h.sender_id, "text": h.text, "snippet_html": h.snippet_html, "date_ms": h.date_ms, "kind": "message" }));
        }
    }
    let total = hits.len().max(results.len());
    (
        StatusCode::OK,
        Json(json!({
            "query": q,
            "category": category,
            "total": total,
            "hits": hits,
            "results": results,
            "apps": global_apps,
            "bots": global_bots,
        })),
    )
        .into_response()
}

pub async fn chat_statistics(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(chat_id): Path<i64>) -> Response {
    if let Err(r) = check_active_membership(&state, ChatId(chat_id), auth.user_id) { return r; }
    match state.db.compute_chat_statistics(ChatId(chat_id)) {
        Ok(stats) => (StatusCode::OK, Json(stats)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn chat_export(auth: AuthBearer, State(state): State<Arc<AppState>>, Path(chat_id): Path<i64>, Query(q): Query<HashMap<String, String>>) -> Response {
    if let Err(r) = check_active_membership(&state, ChatId(chat_id), auth.user_id) { return r; }
    crate::routes::enforce_ttl_expiry(&state).await;
    let limit = q.get("limit").and_then(|s| s.parse::<u32>().ok()).unwrap_or(500).clamp(1, 5000);
    match state.db.export_chat_payload(ChatId(chat_id), limit) {
        Ok(payload) => (StatusCode::OK, Json(payload)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn wallet_balance(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    match state.db.get_wallet_balance(auth.user_id) {
        Ok(b) => (StatusCode::OK, Json(b)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn wallet_transactions(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    match state.db.list_wallet_transactions(auth.user_id) {
        Ok(txs) => (StatusCode::OK, Json(json!({ "transactions": txs }))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn wallet_topup(auth: AuthBearer, State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match parse_json_object(&body) { Ok(v) => v, Err(r) => return r };
    let amount = payload.get("amount_stars").or_else(|| payload.get("amount")).and_then(|v| v.as_i64()).unwrap_or(0);
    let desc = payload.get("description").and_then(|v| v.as_str()).unwrap_or("Stars Top-up");
    match state.db.topup_wallet_stars(auth.user_id, amount, desc) {
        Ok((balance, tx)) => (
            StatusCode::OK,
            Json(json!({
                "user_id": balance.user_id,
                "stars_balance": balance.stars_balance,
                "ton_nano": balance.ton_nano,
                "updated_at_ms": balance.updated_at_ms,
                "balance": balance,
                "transaction": tx
            })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn wallet_send_stars(auth: AuthBearer, State(state): State<Arc<AppState>>, Json(req): Json<StarGiftRequest>) -> Response {
    let desc = req.message.as_deref().unwrap_or(req.gift_id.as_deref().unwrap_or("Telegram Stars Gift"));
    match state.db.transfer_wallet_stars(auth.user_id, UserId(req.to_user_id), req.amount_stars, desc) {
        Ok((balance, tx)) => (
            StatusCode::OK,
            Json(json!({
                "user_id": balance.user_id,
                "stars_balance": balance.stars_balance,
                "ton_nano": balance.ton_nano,
                "updated_at_ms": balance.updated_at_ms,
                "balance": balance,
                "transaction": tx
            })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}
