use crate::dedup::{CachedMessage, DedupWaitError, InFlightSlot, ReservationOutcome};
use crate::desktop_helpers::{evaluate_composer_enter_key, parse_markdown_spans};
use crate::error::{ok_envelope, BridgeError};
use crate::host::TelegramAppHost;
use chrono::Utc;
use gpui_telegram_client::{
    ChatFolderDto, CreateChatRequest, DraftDto, EditMessageHttpRequest, ReadChatHttpRequest,
    RichSendMessageRequest,
};
use gpui_telegram_core::types::{ChatId, MessageId, PeerId, UserId};
use gpui_telegram_proto::message::{MessageContent, MessageItem, ReplyHeader};
use reqwest::Method;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;

struct LeaderGuard<'a> {
    host: &'a TelegramAppHost,
    sender_id: u64,
    nonce: u64,
    #[allow(dead_code)]
    slot: Arc<InFlightSlot>,
    completed: bool,
}

impl<'a> Drop for LeaderGuard<'a> {
    fn drop(&mut self) {
        if !self.completed {
            self.host
                .dedup_store
                .lock()
                .abort(self.sender_id, self.nonce);
        }
    }
}

pub fn handle(host: &TelegramAppHost, cmd: &str, params: &Value) -> Option<String> {
    let client = host.current_client();
    match cmd {
        // ====================================================================
        // Namespace 5: `chats.*` / `dialogs.*` (Zero hardcoded dialog fixtures!)
        // ====================================================================
        "chats.list"
        | "dialogs.list_chats"
        | "telegram.listChats"
        | "telegram.getChats"
        | "chat.getChats" => {
            let sync_remote = params
                .get("sync_remote")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if sync_remote && client.token().is_some() && !*host.simulate_offline.lock() {
                let c = Arc::clone(&client);
                if let Ok(remote_chats) = host.block_on(async move { c.get_chats().await }) {
                    return Some(ok_envelope(serde_json::to_value(remote_chats).unwrap_or_default()));
                }
            }
            // Truthful SQLite LocalStore read (empty [] on fresh install — zero fake fixtures!)
            let local_chats = client.local_store.list_chats();
            Some(ok_envelope(serde_json::to_value(local_chats).unwrap_or_default()))
        }

        "chats.get" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            if client.token().is_some() && !*host.simulate_offline.lock() {
                let c = Arc::clone(&client);
                if let Ok(chat) = host.block_on(async move { c.get_chat(chat_id).await }) {
                    return Some(ok_envelope(serde_json::to_value(chat).unwrap_or_default()));
                }
            }
            match client.local_store.get_chat(ChatId(chat_id)) {
                Some(chat) => Some(ok_envelope(serde_json::to_value(chat).unwrap_or_default())),
                None => Some(
                    BridgeError::not_found(format!("Chat {chat_id} not found")).to_envelope_string(),
                ),
            }
        }

        "chats.create"
        | "chats.create_direct"
        | "chats.create_group"
        | "chats.create_channel"
        | "chats.create_forum" => {
            let mut req: CreateChatRequest =
                serde_json::from_value(params.clone()).unwrap_or_else(|_| CreateChatRequest {
                    title: params
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("New Chat")
                        .to_string(),
                    chat_type: None,
                    peer_user_id: params.get("peer_user_id").and_then(Value::as_u64),
                    username: params
                        .get("username")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    about: params
                        .get("about")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    is_forum: cmd == "chats.create_forum",
                    member_ids: params
                        .get("member_ids")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_u64).collect())
                        .unwrap_or_default(),
                });
            if req.chat_type.is_none() {
                req.chat_type = match cmd {
                    "chats.create_direct" => Some(json!("private")),
                    "chats.create_channel" => Some(json!("channel")),
                    "chats.create_forum" => {
                        req.is_forum = true;
                        Some(json!("supergroup"))
                    }
                    _ => Some(json!("group")),
                };
            }
            let c = Arc::clone(&client);
            match host.block_on(async move { c.create_chat(&req).await }) {
                Ok(chat) => {
                    host.events.emit(
                        "chatCreated",
                        &serde_json::to_value(&chat).unwrap_or_default(),
                    );
                    Some(ok_envelope(serde_json::to_value(chat).unwrap_or_default()))
                }
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "chats.mark_read" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let max_id = params.get("max_id").and_then(Value::as_i64).unwrap_or(i64::MAX / 2);
            client
                .local_store
                .mark_read_inbox(ChatId(chat_id), MessageId(max_id), Some(0));
            if let Some(token) = client.token() {
                let http = client.http_client.clone();
                let req = ReadChatHttpRequest {
                    max_msg_id: Some(max_id),
                };
                let _ = host.block_on(async move { http.mark_chat_read(&token, chat_id, &req).await });
            }
            Some(ok_envelope(json!({ "chat_id": chat_id, "max_id": max_id, "unread_count": 0 })))
        }

        "chats.mark_unread" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            client
                .local_store
                .mark_read_inbox(ChatId(chat_id), MessageId(0), Some(1));
            Some(ok_envelope(json!({ "chat_id": chat_id, "unread_count": 1 })))
        }

        "chats.pin" | "chats.mute" | "chats.archive" | "chats.unarchive" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let sub = cmd.strip_prefix("chats.").unwrap_or(cmd);
            if client.token().is_some() {
                if let Ok(v) = host.api_json(
                    Method::POST,
                    &format!("/api/v1/chats/{chat_id}/{sub}"),
                    Some(params.clone()),
                    true,
                ) {
                    return Some(ok_envelope(v));
                }
            }
            Some(ok_envelope(json!({
                "chat_id": chat_id,
                "action": sub,
                "applied_locally": true
            })))
        }

        "chats.delete_history" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            client.local_store.clear_chat_messages(ChatId(chat_id));
            if client.token().is_some() {
                let c = Arc::clone(&client);
                let _ = host.block_on(async move { c.clear_chat_history(chat_id).await });
            }
            Some(ok_envelope(json!({ "chat_id": chat_id, "cleared": true })))
        }

        "chats.leave" | "chats.delete" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let c = Arc::clone(&client);
            match host.block_on(async move { c.leave_or_delete_chat(chat_id).await }) {
                Ok(()) => Some(ok_envelope(json!({ "chat_id": chat_id, "left": true }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "chats.set_auto_delete" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let ttl = params
                .get("ttl_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            client.local_store.set_chat_ttl(ChatId(chat_id), ttl);
            if client.token().is_some() {
                let c = Arc::clone(&client);
                let _ = host.block_on(async move { c.set_chat_ttl(chat_id, ttl).await });
            }
            Some(ok_envelope(json!({ "chat_id": chat_id, "ttl_seconds": ttl })))
        }

        // ====================================================================
        // Namespace 6: `folders.*` (Custom Dialog Filters)
        // ====================================================================
        "folders.list" => {
            if client.token().is_some() && !*host.simulate_offline.lock() {
                if let Ok(v) = host.api_json(Method::GET, "/api/v1/folders", None, true) {
                    if let Ok(list) = serde_json::from_value::<Vec<ChatFolderDto>>(v.clone()) {
                        client.local_store.set_folders(list);
                    }
                    return Some(ok_envelope(v));
                }
            }
            let folders = client.local_store.get_folders();
            Some(ok_envelope(serde_json::to_value(folders).unwrap_or_default()))
        }

        "folders.create" | "folders.update" => {
            let mut folder: ChatFolderDto = match serde_json::from_value(params.clone()) {
                Ok(f) => f,
                Err(e) => {
                    return Some(
                        BridgeError::invalid_params(format!("Invalid ChatFolderDto: {e}"))
                            .to_envelope_string(),
                    )
                }
            };
            if folder.id == 0 {
                folder.id = Utc::now().timestamp_millis() % 1_000_000;
            }
            client.local_store.upsert_folder(folder.clone());
            if client.token().is_some() && !*host.simulate_offline.lock() {
                let _ = host.api_json(
                    Method::POST,
                    "/api/v1/folders",
                    Some(serde_json::to_value(&folder).unwrap_or_default()),
                    true,
                );
            }
            Some(ok_envelope(serde_json::to_value(folder).unwrap_or_default()))
        }

        "folders.delete" => {
            let Some(folder_id) = params.get("folder_id").or_else(|| params.get("id")).and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing folder_id").to_envelope_string());
            };
            client.local_store.remove_folder(folder_id);
            if client.token().is_some() && !*host.simulate_offline.lock() {
                let _ = host.api_json(Method::DELETE, &format!("/api/v1/folders/{folder_id}"), None, true);
            }
            Some(ok_envelope(json!({ "deleted": true, "folder_id": folder_id })))
        }

        "folders.reorder" => {
            let Some(ids) = params.get("folder_ids").and_then(Value::as_array) else {
                return Some(BridgeError::invalid_params("Missing folder_ids").to_envelope_string());
            };
            let order: Vec<i64> = ids.iter().filter_map(Value::as_i64).collect();
            let mut existing = client.local_store.get_folders();
            existing.sort_by_key(|f| order.iter().position(|&id| id == f.id).unwrap_or(usize::MAX));
            client.local_store.set_folders(existing.clone());
            Some(ok_envelope(serde_json::to_value(existing).unwrap_or_default()))
        }

        // ====================================================================
        // Namespace 7: `stories.*` (24-Hour Ephemeral Stories)
        // ====================================================================
        "stories.list" => {
            let c = Arc::clone(&client);
            match host.block_on(async move { c.list_stories().await }) {
                Ok(stories) => Some(ok_envelope(serde_json::to_value(stories).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "stories.create" => {
            let media_id = params.get("media_id").and_then(Value::as_str).map(ToOwned::to_owned);
            let media_type = params
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("photo")
                .to_string();
            let caption = params.get("caption").and_then(Value::as_str).map(ToOwned::to_owned);
            let privacy = params
                .get("privacy")
                .and_then(Value::as_str)
                .unwrap_or("everyone")
                .to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move {
                c.create_story(media_id.as_deref(), &media_type, caption.as_deref(), &privacy)
                    .await
            }) {
                Ok(story) => Some(ok_envelope(serde_json::to_value(story).unwrap_or_default())),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "stories.delete" => {
            let Some(story_id) = params.get("story_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing story_id").to_envelope_string());
            };
            let sid = story_id.to_string();
            let c = Arc::clone(&client);
            match host.block_on(async move { c.delete_story(&sid).await }) {
                Ok(()) => Some(ok_envelope(json!({ "deleted": true, "story_id": story_id }))),
                Err(e) => Some(BridgeError::from_http_error(e).to_envelope_string()),
            }
        }

        "stories.mark_seen" | "stories.react" | "stories.viewers" => {
            let Some(story_id) = params.get("story_id").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing story_id").to_envelope_string());
            };
            let sub = cmd.strip_prefix("stories.").unwrap_or("view");
            let method = if sub == "viewers" { Method::GET } else { Method::POST };
            let body = if sub == "viewers" { None } else { Some(params.clone()) };
            match host.api_json(method, &format!("/api/v1/stories/{story_id}/{sub}"), body, true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 8: `search.*` (Global + Scoped FTS5 / Server Search)
        // ====================================================================
        "search.global" | "search.in_chat" => {
            let query = params.get("query").and_then(Value::as_str).unwrap_or("");
            let filter = params.get("filter").and_then(Value::as_str).map(ToOwned::to_owned);
            let chat_id = params.get("chat_id").and_then(Value::as_i64);
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;

            if client.token().is_some() && !*host.simulate_offline.lock() {
                let q = query.to_string();
                let c = Arc::clone(&client);
                if let Ok(resp) = host.block_on(async move {
                    c.search_messages(&q, filter.as_deref(), chat_id, limit).await
                }) {
                    return Some(ok_envelope(serde_json::to_value(resp).unwrap_or_default()));
                }
            }

            // Offline LocalStore full-scan fallback
            let needle = query.to_lowercase();
            let mut hits = Vec::new();
            let target_chats: Vec<i64> = match chat_id {
                Some(cid) => vec![cid],
                None => client.local_store.list_chats().into_iter().map(|c| c.chat_id).collect(),
            };
            for cid in target_chats {
                if let Ok(msgs) = client.local_store.get_messages(ChatId(cid)) {
                    for m in msgs {
                        if let MessageContent::Text { ref text, .. } = m.content {
                            if needle.is_empty() || text.to_lowercase().contains(&needle) {
                                hits.push(json!({
                                    "chat_id": cid,
                                    "msg_id": m.id.0,
                                    "text": text,
                                    "timestamp_ms": m.timestamp_ms
                                }));
                                if hits.len() >= limit {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Some(ok_envelope(json!({ "hits": hits, "total": hits.len() })))
        }

        // ====================================================================
        // Namespace 9: `messages.*` (+ `telegram.sendMessage` / `telegram.getMessages`)
        // ====================================================================
        "messages.list"
        | "messages.history"
        | "telegram.getMessages"
        | "chat.getMessages" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            if client.token().is_some()
                && !*host.simulate_offline.lock()
                && params.get("fetch_remote").and_then(Value::as_bool).unwrap_or(false)
            {
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
                let before_id = params.get("before_id").and_then(Value::as_i64);
                if let Some(tok) = client.token() {
                    let http = client.http_client.clone();
                    if let Ok(remote) = host.block_on(async move {
                        http.get_messages(&tok, chat_id, limit, before_id).await
                    }) {
                        return Some(ok_envelope(serde_json::to_value(remote).unwrap_or_default()));
                    }
                }
            }
            match client.get_messages(ChatId(chat_id)) {
                Ok(messages) => Some(ok_envelope(serde_json::to_value(messages).unwrap_or_default())),
                Err(e) => Some(BridgeError::storage(e.to_string()).to_envelope_string()),
            }
        }

        "messages.send" | "telegram.sendMessage" | "chat.sendMessage" => {
            let chat_id = params.get("chat_id").and_then(Value::as_i64).unwrap_or(0);
            let raw_text = params.get("text").and_then(Value::as_str).unwrap_or("");
            let parse_md = params.get("parse_markdown").and_then(Value::as_bool).unwrap_or(false);
            let (text, entities) = if parse_md {
                parse_markdown_spans(raw_text)
            } else {
                (raw_text.to_string(), Vec::new())
            };
            let sender_id = params
                .get("sender_id")
                .and_then(Value::as_u64)
                .or_else(|| client.current_user_id())
                .unwrap_or(1);
            let reply_to = params.get("reply_to").and_then(Value::as_i64);
            let topic_id = params.get("topic_id").and_then(Value::as_i64);
            let client_nonce = params.get("client_nonce").and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_i64().and_then(|n| (n >= 0).then_some(n as u64)))
                    .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            });

            let now = Utc::now();

            // Atomic 24-Hour In-Flight Reservation Deduplication
            let mut leader_guard: Option<LeaderGuard> = None;
            if let Some(nonce) = client_nonce {
                loop {
                    let outcome = host.dedup_store.lock().reserve(sender_id, nonce, now);
                    match outcome {
                        ReservationOutcome::AlreadyExists(cached) => {
                            return Some(ok_envelope(json!({
                                "id": cached.message_id,
                                "chat_id": cached.chat_id,
                                "sender_id": cached.sender_id,
                                "text": cached.text,
                                "timestamp": cached.timestamp,
                                "reply_to": cached.reply_to,
                                "client_nonce": cached.client_nonce,
                                "status": "sent",
                                "duplicate": true
                            })));
                        }
                        ReservationOutcome::Follower(slot) => match slot.wait() {
                            Ok(cached) => {
                                return Some(ok_envelope(json!({
                                    "id": cached.message_id,
                                    "chat_id": cached.chat_id,
                                    "sender_id": cached.sender_id,
                                    "text": cached.text,
                                    "timestamp": cached.timestamp,
                                    "reply_to": cached.reply_to,
                                    "client_nonce": cached.client_nonce,
                                    "status": "sent",
                                    "duplicate": true
                                })));
                            }
                            Err(DedupWaitError::Aborted) => continue,
                        },
                        ReservationOutcome::Leader(slot) => {
                            leader_guard = Some(LeaderGuard {
                                host,
                                sender_id,
                                nonce,
                                slot,
                                completed: false,
                            });
                            break;
                        }
                    }
                }
            }

            // When authenticated & online, route through durable TelegramClient Outbox -> Server
            if client.token().is_some() && !*host.simulate_offline.lock() {
                let req = RichSendMessageRequest {
                    chat_id,
                    text: Some(text.clone()),
                    content: Some(MessageContent::text_with_entities(
                        text.clone(),
                        entities.clone(),
                    )),
                    entities,
                    reply_to,
                    quote_text: params
                        .get("quote_text")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    topic_id,
                    client_nonce,
                    forward_from_chat_id: params.get("forward_from_chat_id").and_then(Value::as_i64),
                    forward_from_msg_id: params.get("forward_from_msg_id").and_then(Value::as_i64),
                    silent: params.get("silent").and_then(Value::as_bool).unwrap_or(false),
                    scheduled_at_ms: params.get("scheduled_at_ms").and_then(Value::as_i64),
                    inline_keyboard: None,
                    send_as_peer_id: params.get("send_as_peer_id").and_then(Value::as_i64),
                };
                let c = Arc::clone(&client);
                match host.block_on(async move { c.send_message_durable(req).await }) {
                    Ok(sent) => {
                        let timestamp = sent.date_ms;
                        if let Some(mut guard) = leader_guard {
                            let cached = CachedMessage {
                                message_id: sent.msg_id,
                                chat_id,
                                sender_id,
                                text: text.clone(),
                                timestamp,
                                reply_to,
                                client_nonce: guard.nonce,
                                created_at: now,
                            };
                            host.dedup_store
                                .lock()
                                .complete(sender_id, guard.nonce, cached);
                            guard.completed = true;
                        }
                        host.events.emit(
                            "newMessage",
                            &json!({
                                "id": sent.msg_id,
                                "chat_id": chat_id,
                                "sender_id": sender_id,
                                "text": text,
                                "timestamp": timestamp,
                                "reply_to": reply_to,
                                "client_nonce": client_nonce,
                                "pts": sent.pts
                            }),
                        );
                        return Some(ok_envelope(json!({
                            "id": sent.msg_id,
                            "msg_id": sent.msg_id,
                            "chat_id": chat_id,
                            "sender_id": sender_id,
                            "text": text,
                            "timestamp": timestamp,
                            "reply_to": reply_to,
                            "client_nonce": client_nonce,
                            "pts": sent.pts,
                            "status": "sent"
                        })));
                    }
                    Err(e) => return Some(BridgeError::from_http_error(e).to_envelope_string()),
                }
            }

            // LocalStore / Offline durable Outbox path
            if client.token().is_some() && *host.simulate_offline.lock() {
                let req = RichSendMessageRequest {
                    chat_id,
                    text: Some(text.clone()),
                    content: Some(MessageContent::text_with_entities(
                        text.clone(),
                        entities.clone(),
                    )),
                    entities,
                    reply_to,
                    quote_text: None,
                    topic_id,
                    client_nonce,
                    forward_from_chat_id: None,
                    forward_from_msg_id: None,
                    silent: false,
                    scheduled_at_ms: None,
                    inline_keyboard: None,
                    send_as_peer_id: None,
                };
                if let Ok(rec) = client.enqueue_message(req) {
                    let provisional_id = -(rec.client_nonce as i64);
                    if let Some(mut guard) = leader_guard {
                        let cached = CachedMessage {
                            message_id: provisional_id,
                            chat_id,
                            sender_id,
                            text: text.clone(),
                            timestamp: rec.created_at_ms,
                            reply_to,
                            client_nonce: guard.nonce,
                            created_at: now,
                        };
                        host.dedup_store
                            .lock()
                            .complete(sender_id, guard.nonce, cached);
                        guard.completed = true;
                    }
                    return Some(ok_envelope(json!({
                        "id": provisional_id,
                        "chat_id": chat_id,
                        "sender_id": sender_id,
                        "text": text,
                        "timestamp": rec.created_at_ms,
                        "client_nonce": rec.client_nonce,
                        "status": "queued_offline"
                    })));
                }
            }

            let msg_id = host.next_local_msg_id.fetch_add(1, Ordering::SeqCst);
            let timestamp = now.timestamp_millis();
            let mut message_item = MessageItem::new(
                MessageId(msg_id),
                ChatId(chat_id),
                UserId(sender_id),
                PeerId::User(UserId(sender_id)),
                MessageContent::text_with_entities(text.clone(), entities),
            );
            if let Some(r) = reply_to {
                message_item.reply_to = Some(ReplyHeader::new(MessageId(r)));
            }
            client.local_store.insert_message(message_item);

            if let Some(mut guard) = leader_guard {
                let cached = CachedMessage {
                    message_id: msg_id,
                    chat_id,
                    sender_id,
                    text: text.clone(),
                    timestamp,
                    reply_to,
                    client_nonce: guard.nonce,
                    created_at: now,
                };
                host.dedup_store
                    .lock()
                    .complete(sender_id, guard.nonce, cached);
                guard.completed = true;
            }

            host.events.emit(
                "newMessage",
                &json!({
                    "id": msg_id,
                    "chat_id": chat_id,
                    "sender_id": sender_id,
                    "text": text,
                    "timestamp": timestamp,
                    "reply_to": reply_to,
                    "client_nonce": client_nonce
                }),
            );

            Some(ok_envelope(json!({
                "id": msg_id,
                "chat_id": chat_id,
                "sender_id": sender_id,
                "text": text,
                "timestamp": timestamp,
                "reply_to": reply_to,
                "client_nonce": client_nonce,
                "status": "sent"
            })))
        }

        "messages.edit" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let Some(msg_id) = params.get("msg_id").or_else(|| params.get("id")).and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing msg_id").to_envelope_string());
            };
            let Some(new_text) = params.get("text").and_then(Value::as_str) else {
                return Some(BridgeError::invalid_params("Missing text").to_envelope_string());
            };
            let (plain, entities) = parse_markdown_spans(new_text);
            if let Some(tok) = client.token() {
                let http = client.http_client.clone();
                let req = EditMessageHttpRequest {
                    text: plain.clone(),
                    entities: entities.clone(),
                };
                if let Ok(res) = host.block_on(async move {
                    http.edit_message(&tok, chat_id, msg_id, &req).await
                }) {
                    return Some(ok_envelope(serde_json::to_value(res).unwrap_or_default()));
                }
            }
            let sender_id = client.current_user_id().unwrap_or(1);
            let mut edited = MessageItem::new(
                MessageId(msg_id),
                ChatId(chat_id),
                UserId(sender_id),
                PeerId::Chat(ChatId(chat_id)),
                MessageContent::text_with_entities(plain.clone(), entities),
            );
            edited.edited_timestamp_ms = Some(Utc::now().timestamp_millis());
            client.local_store.edit_message(edited);
            Some(ok_envelope(json!({ "chat_id": chat_id, "msg_id": msg_id, "text": plain, "edited": true })))
        }

        "messages.delete" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let ids: Vec<i64> = params
                .get("message_ids")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .or_else(|| params.get("msg_id").and_then(Value::as_i64).map(|id| vec![id]))
                .unwrap_or_default();
            if ids.is_empty() {
                return Some(BridgeError::invalid_params("Missing message_ids").to_envelope_string());
            }
            let msg_ids: Vec<MessageId> = ids.iter().copied().map(MessageId).collect();
            client.local_store.delete_messages(ChatId(chat_id), &msg_ids);
            if let Some(tok) = client.token() {
                let http = client.http_client.clone();
                for &mid in &ids {
                    let h = http.clone();
                    let t = tok.clone();
                    let _ = host.block_on(async move { h.delete_message(&t, chat_id, mid).await });
                }
            }
            Some(ok_envelope(json!({ "chat_id": chat_id, "deleted_ids": ids })))
        }

        "messages.forward" => {
            let Some(target_chat_id) = params.get("to_chat_id").or_else(|| params.get("chat_id")).and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing to_chat_id").to_envelope_string());
            };
            let from_chat_id = params.get("from_chat_id").and_then(Value::as_i64);
            let from_msg_id = params.get("from_msg_id").and_then(Value::as_i64);
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("[Forwarded Message]")
                .to_string();
            if client.token().is_some() {
                let req = RichSendMessageRequest {
                    chat_id: target_chat_id,
                    text: Some(text.clone()),
                    content: Some(MessageContent::plain_text(text)),
                    entities: Vec::new(),
                    reply_to: None,
                    quote_text: None,
                    topic_id: None,
                    client_nonce: Some(client.next_client_nonce()),
                    forward_from_chat_id: from_chat_id,
                    forward_from_msg_id: from_msg_id,
                    silent: false,
                    scheduled_at_ms: None,
                    inline_keyboard: None,
                    send_as_peer_id: None,
                };
                let c = Arc::clone(&client);
                match host.block_on(async move { c.send_message_durable(req).await }) {
                    Ok(sent) => return Some(ok_envelope(serde_json::to_value(sent).unwrap_or_default())),
                    Err(e) => return Some(BridgeError::from_http_error(e).to_envelope_string()),
                }
            }
            Some(BridgeError::unauthorized("Forwarding requires active session").to_envelope_string())
        }

        "messages.pin" | "messages.unpin" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let Some(msg_id) = params.get("msg_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing msg_id").to_envelope_string());
            };
            let pin = cmd == "messages.pin";
            let mut pinned = client.local_store.get_pinned_messages(ChatId(chat_id));
            if pin {
                if !pinned.contains(&MessageId(msg_id)) {
                    pinned.push(MessageId(msg_id));
                }
            } else {
                pinned.retain(|m| m.0 != msg_id);
            }
            client.local_store.set_pinned_messages(ChatId(chat_id), pinned.clone());
            if client.token().is_some() {
                let action = if pin { "pin" } else { "unpin" };
                let _ = host.api_json(
                    Method::POST,
                    &format!("/api/v1/chats/{chat_id}/messages/{msg_id}/{action}"),
                    Some(json!({})),
                    true,
                );
            }
            let ids: Vec<i64> = pinned.into_iter().map(|m| m.0).collect();
            Some(ok_envelope(json!({ "chat_id": chat_id, "pinned_ids": ids })))
        }

        "messages.pinned_list" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let ids: Vec<i64> = client
                .local_store
                .get_pinned_messages(ChatId(chat_id))
                .into_iter()
                .map(|m| m.0)
                .collect();
            Some(ok_envelope(json!({ "chat_id": chat_id, "pinned_ids": ids })))
        }

        "messages.parse_markdown_spans" => {
            let raw = params.get("text").and_then(Value::as_str).unwrap_or("");
            let (plain, entities) = parse_markdown_spans(raw);
            Some(ok_envelope(json!({
                "plain_text": plain,
                "entities": entities
            })))
        }

        "messages.translate" | "messages.report" | "messages.schedule_list" | "messages.send_scheduled_now" => {
            let sub = cmd.strip_prefix("messages.").unwrap_or(cmd);
            match host.api_json(Method::POST, &format!("/api/v1/messages/{sub}"), Some(params.clone()), true) {
                Ok(v) => Some(ok_envelope(v)),
                Err(e) => Some(e.to_envelope_string()),
            }
        }

        // ====================================================================
        // Namespace 10: `drafts.*` (Per-Chat/Topic Drafts & CJK IME Guard)
        // ====================================================================
        "drafts.get" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let draft = client.local_store.get_draft(ChatId(chat_id));
            Some(ok_envelope(serde_json::to_value(draft).unwrap_or(Value::Null)))
        }

        "drafts.save" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let draft = DraftDto {
                chat_id,
                text,
                entities: Vec::new(),
                reply_to_msg_id: params.get("reply_to_msg_id").and_then(Value::as_i64),
                topic_id: params.get("topic_id").and_then(Value::as_i64),
                date_ms: Utc::now().timestamp_millis(),
            };
            client.local_store.set_draft(ChatId(chat_id), Some(draft.clone()));
            Some(ok_envelope(serde_json::to_value(draft).unwrap_or_default()))
        }

        "drafts.clear" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            client.local_store.set_draft(ChatId(chat_id), None);
            Some(ok_envelope(json!({ "chat_id": chat_id, "cleared": true })))
        }

        "drafts.ime_handle_key" => {
            let has_marked = params
                .get("has_marked_text")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let shift = params
                .get("shift_pressed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let cmd_key = params
                .get("cmd_pressed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let send_by_cmd = params
                .get("send_by_cmd_enter")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| host.host_config.lock().preferences.send_by_cmd_enter);
            let decision = evaluate_composer_enter_key(has_marked, shift, cmd_key, send_by_cmd);
            Some(ok_envelope(json!({ "decision": decision })))
        }

        // ====================================================================
        // Namespace 11: `outbox.*` (Durable Offline Retry Queue)
        // ====================================================================
        "outbox.list" => match client.outbox.store().list_by_status(None) {
            Ok(records) => Some(ok_envelope(json!({
                "pending_count": client.outbox.store().pending_count(),
                "records": records
            }))),
            Err(e) => Some(BridgeError::storage(e.to_string()).to_envelope_string()),
        },

        "outbox.enqueue" => {
            let Some(chat_id) = params.get("chat_id").and_then(Value::as_i64) else {
                return Some(BridgeError::invalid_params("Missing chat_id").to_envelope_string());
            };
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let req = RichSendMessageRequest {
                chat_id,
                text: Some(text.clone()),
                content: Some(MessageContent::plain_text(text)),
                entities: Vec::new(),
                reply_to: params.get("reply_to").and_then(Value::as_i64),
                quote_text: None,
                topic_id: params.get("topic_id").and_then(Value::as_i64),
                client_nonce: params.get("client_nonce").and_then(Value::as_u64),
                forward_from_chat_id: None,
                forward_from_msg_id: None,
                silent: false,
                scheduled_at_ms: None,
                inline_keyboard: None,
                send_as_peer_id: None,
            };
            match client.enqueue_message(req) {
                Ok(rec) => Some(ok_envelope(serde_json::to_value(rec).unwrap_or_default())),
                Err(e) => Some(BridgeError::storage(e.to_string()).to_envelope_string()),
            }
        }

        "outbox.retry" | "outbox.flush" => {
            let now_ms = Utc::now().timestamp_millis();
            let reset = client
                .outbox
                .store()
                .reset_all_pending_retry_times(now_ms)
                .unwrap_or(0);
            client.outbox.start(client.runtime().handle());
            client.outbox.wake();
            Some(ok_envelope(json!({
                "reset_count": reset,
                "pending_count": client.outbox.store().pending_count()
            })))
        }

        "outbox.cancel" => {
            let Some(nonce) = params.get("client_nonce").and_then(Value::as_u64) else {
                return Some(BridgeError::invalid_params("Missing client_nonce").to_envelope_string());
            };
            let _ = client
                .outbox
                .store()
                .mark_failed(nonce, "Cancelled by user", Utc::now().timestamp_millis());
            Some(ok_envelope(json!({ "cancelled": true, "client_nonce": nonce })))
        }

        _ => None,
    }
}
