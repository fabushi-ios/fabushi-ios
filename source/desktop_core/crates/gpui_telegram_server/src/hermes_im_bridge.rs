use crate::db::{ChatMemberRecord, ChatRecord, ForumTopicRecord, ServerDatabase};
use crate::messaging::pipeline::MessagePipeline;
use crate::search::hybrid_memory::HybridMemoryEngine;
use crate::search::schema::{SearchDocument, SearchQueryParams};
use crate::search::tantivy_indexer::TantivyIndexer;
use crate::server::extract_message_content_text;
use crate::sync::pts_engine::PtsEngine;
use crate::ws::GatewayHub;
use chrono::Utc;
use gpui_telegram_agent::hermes::toolsets::{BoxFuture, PlatformImBridge, PlatformImCaller};
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, TopicId, UserId};
use gpui_telegram_proto::cards::GenerativeCard;
use gpui_telegram_proto::contracts::ForumTopicDto;
use gpui_telegram_proto::message::{MessageContent, MessageItem, ReplyHeader};
use gpui_telegram_proto::miniapp_contracts::{MiniAppManifestDto, MiniAppUiSchema};
use gpui_telegram_proto::sync::{PtsUpdate, ReactionEvent, UpdateEvent};
use gpui_telegram_proto::ws::WsServerFrame;
use serde_json::{json, Value};
use std::sync::{Arc, Weak};
use uuid::Uuid;

pub struct ServerPlatformImBridge {
    db: Arc<ServerDatabase>,
    pipeline: Arc<MessagePipeline>,
    pts_engine: Arc<PtsEngine>,
    gateway_hub: Weak<GatewayHub>,
    search_indexer: Weak<TantivyIndexer>,
    hybrid_memory: Weak<HybridMemoryEngine>,
}

impl ServerPlatformImBridge {
    pub fn new(
        db: Arc<ServerDatabase>,
        pipeline: Arc<MessagePipeline>,
        pts_engine: Arc<PtsEngine>,
        gateway_hub: Weak<GatewayHub>,
        search_indexer: Weak<TantivyIndexer>,
        hybrid_memory: Weak<HybridMemoryEngine>,
    ) -> Self {
        Self {
            db,
            pipeline,
            pts_engine,
            gateway_hub,
            search_indexer,
            hybrid_memory,
        }
    }

    fn resolve_caller_user(&self, caller: &PlatformImCaller) -> Result<UserId, String> {
        if let Some(uid) = caller.user_id.filter(|&u| u > 0) {
            return Ok(UserId(uid));
        }
        if let Some(bid) = caller.bot_id.filter(|&b| b > 0) {
            return Ok(UserId(bid as u64));
        }
        if let Some(ref agent_id) = caller.bound_agent_id {
            if let Ok(bots) = self.db.list_all_bots() {
                if let Some(bot) = bots.into_iter().find(|b| b.agent_id == *agent_id) {
                    return Ok(UserId(bot.bot_user_id as u64));
                }
            }
            if let Ok(Some(bot)) = self.db.get_bot_by_username(agent_id) {
                return Ok(UserId(bot.bot_user_id as u64));
            }
        }
        Err("Authenticated caller user_id or bot_id is required for PlatformIm operations".into())
    }

    fn reject_cross_chat_arg(args: &Value, pinned: i64) -> Result<(), String> {
        if let Some(explicit_val) = args.get("chat_id").filter(|v| !v.is_null()) {
            if explicit_val.as_i64() != Some(pinned) {
                return Err(format!(
                    "Cross-chat target mismatch: caller context is pinned to chat {pinned}, got {explicit_val}"
                ));
            }
        }
        Ok(())
    }

    fn require_chat_id(args: &Value, caller: &PlatformImCaller) -> Result<ChatId, String> {
        if let Some(pinned) = caller.chat_id {
            Self::reject_cross_chat_arg(args, pinned)?;
            if pinned <= 0 {
                return Err("Positive 'chat_id' is required".to_string());
            }
            return Ok(ChatId(pinned));
        }
        args.get("chat_id")
            .and_then(|v| v.as_i64())
            .filter(|&c| c > 0)
            .map(ChatId)
            .ok_or_else(|| "Positive 'chat_id' is required".to_string())
    }

    fn check_membership(&self, chat_id: ChatId, user_id: UserId) -> Result<(ChatRecord, bool), String> {
        let chat = self
            .db
            .get_chat(chat_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Chat {} not found", chat_id.0))?;
        let member = self
            .db
            .get_chat_member(chat_id, user_id)
            .map_err(|e| e.to_string())?;
        let is_creator = chat.creator_id == user_id;
        let is_active = is_creator
            || member
                .as_ref()
                .is_some_and(|m| !ServerDatabase::is_inactive_member_role(&m.role));
        if !is_active {
            return Err(format!("User {} is not an active member of chat {}", user_id.0, chat_id.0));
        }
        let is_admin = is_creator
            || member.as_ref().is_some_and(|m| {
                ServerDatabase::is_admin_member_role(&m.role) || m.admin_rights != 0
            });
        Ok((chat, is_admin))
    }

    fn check_send_permitted(&self, chat_id: ChatId, user_id: UserId) -> Result<(ChatRecord, bool), String> {
        let (chat, is_admin) = self.check_membership(chat_id, user_id)?;
        if let Some(m) = self.db.get_chat_member(chat_id, user_id).map_err(|e| e.to_string())? {
            if m.role.trim().eq_ignore_ascii_case("restricted") && (m.banned_rights & 1) != 0 {
                return Err("User is restricted from sending messages".to_string());
            }
        }
        if matches!(chat.chat_type, 0 | 4) {
            if let Ok(members) = self.db.get_chat_members(chat_id) {
                for peer_uid in members {
                    if peer_uid != user_id
                        && (self.db.is_user_blocked(peer_uid, user_id).unwrap_or(false)
                            || self.db.is_user_blocked(user_id, peer_uid).unwrap_or(false))
                    {
                        return Err("Cannot send messages in a blocked direct chat".to_string());
                    }
                }
            }
        }
        Ok((chat, is_admin))
    }

    fn broadcast_pts(&self, chat_id: ChatId, update: PtsUpdate) {
        if let Some(hub) = self.gateway_hub.upgrade() {
            hub.broadcast_to_chat_and_members(chat_id, None, WsServerFrame::PtsPush(update));
        }
    }

    fn broadcast_event(&self, chat_id: ChatId, event: UpdateEvent) {
        if let Ok((_pts, update)) = self.pts_engine.advance_pts(chat_id, 1, event.clone()) {
            self.broadcast_pts(chat_id, update);
        } else if let Some(hub) = self.gateway_hub.upgrade() {
            hub.broadcast_to_chat_and_members(chat_id, None, WsServerFrame::Update(event));
        }
    }
}

impl PlatformImBridge for ServerPlatformImBridge {
    fn execute_im_tool<'a>(
        &'a self,
        tool_name: &'a str,
        args: &'a Value,
        caller: PlatformImCaller,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let caller_uid = self.resolve_caller_user(&caller)?;
            let now_ms = Utc::now().timestamp_millis();

            match tool_name {
                "send_message" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let _ = self.check_send_permitted(chat_id, caller_uid)?;

                    let content = if let Some(card_val) = args.get("generative_card") {
                        let card: GenerativeCard = serde_json::from_value(card_val.clone())
                            .map_err(|e| format!("Invalid generative_card: {e}"))?;
                        MessageContent::GenerativeCard(card)
                    } else {
                        let text = args
                            .get("text")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| "Non-empty 'text' or 'generative_card' is required".to_string())?;
                        MessageContent::plain_text(text)
                    };

                    let mut msg = MessageItem::new(
                        MsgId(0),
                        chat_id,
                        caller_uid,
                        PeerId::Chat(chat_id),
                        content.clone(),
                    );
                    if let Some(rid) = args.get("reply_to_id").and_then(|v| v.as_i64()) {
                        msg.reply_to = Some(ReplyHeader::new(MsgId(rid)));
                    }
                    msg.topic_id = args.get("topic_id").and_then(|v| v.as_i64()).map(TopicId);
                    msg.flags.is_silent = args.get("silent").and_then(|v| v.as_bool()).unwrap_or(false);
                    msg.client_nonce = args
                        .get("client_nonce")
                        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));

                    let pts_update = self
                        .pipeline
                        .process_incoming_message_strict(msg)
                        .map_err(|e| e.to_string())?;

                    let (msg_id, date_ms, indexed_text) = match &pts_update.event {
                        UpdateEvent::NewMessage(saved) => (
                            saved.id.0,
                            saved.date.timestamp_millis(),
                            extract_message_content_text(&saved.content),
                        ),
                        _ => (0, now_ms, extract_message_content_text(&content)),
                    };

                    if let Some(idx) = self.search_indexer.upgrade() {
                        idx.index_message_nonblocking(SearchDocument::new(
                            msg_id.to_string(),
                            chat_id.0.to_string(),
                            caller_uid.0.to_string(),
                            date_ms,
                            indexed_text.clone(),
                            None,
                        ));
                        idx.commit_nonblocking();
                    }
                    if let Some(hm) = self.hybrid_memory.upgrade() {
                        let doc_id = format!("{}:{}", chat_id.0, msg_id);
                        hm.index_message_or_file(
                            &doc_id,
                            chat_id.0,
                            "message",
                            &indexed_text,
                            vec![],
                        );
                    }
                    let pts = pts_update.pts;
                    self.broadcast_pts(chat_id, pts_update);
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "message_id": msg_id,
                        "pts": pts,
                        "text": indexed_text,
                    }))
                }
                "edit_message" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let (_chat, is_admin) = self.check_membership(chat_id, caller_uid)?;
                    let msg_id = args
                        .get("message_id")
                        .and_then(|v| v.as_i64())
                        .filter(|&id| id > 0)
                        .ok_or_else(|| "Positive 'message_id' is required".to_string())?;
                    let text = args
                        .get("text")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "Non-empty 'text' is required".to_string())?;
                    let existing = self
                        .db
                        .get_message(chat_id, MsgId(msg_id))
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Message {msg_id} not found"))?;
                    if existing.sender_id != caller_uid && !is_admin {
                        return Err("Permission denied: cannot edit another user's message".into());
                    }
                    let pts_update = self
                        .pipeline
                        .edit_message(chat_id, MsgId(msg_id), MessageContent::plain_text(text))
                        .map_err(|e| e.to_string())?;
                    if let Some(idx) = self.search_indexer.upgrade() {
                        idx.index_message_nonblocking(SearchDocument::new(
                            msg_id.to_string(),
                            chat_id.0.to_string(),
                            existing.sender_id.0.to_string(),
                            existing.date_ms,
                            text.to_string(),
                            None,
                        ));
                        idx.commit_nonblocking();
                    }
                    if let Some(hm) = self.hybrid_memory.upgrade() {
                        let doc_id = format!("{}:{}", chat_id.0, msg_id);
                        hm.index_message_or_file(
                            &doc_id,
                            chat_id.0,
                            "message",
                            text,
                            vec![],
                        );
                    }
                    let pts = pts_update.pts;
                    self.broadcast_pts(chat_id, pts_update);
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "message_id": msg_id,
                        "pts": pts,
                        "text": text,
                    }))
                }
                "delete_message" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let (_chat, is_admin) = self.check_membership(chat_id, caller_uid)?;
                    let mut ids: Vec<i64> = args
                        .get("message_ids")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|x| x.as_i64()).collect())
                        .unwrap_or_default();
                    if ids.is_empty() {
                        if let Some(id) = args.get("message_id").and_then(|v| v.as_i64()) {
                            ids.push(id);
                        }
                    }
                    if ids.is_empty() {
                        return Err("'message_id' or 'message_ids' is required".into());
                    }
                    for &id in &ids {
                        if let Ok(Some(m)) = self.db.get_message(chat_id, MsgId(id)) {
                            if m.sender_id != caller_uid && !is_admin {
                                return Err("Permission denied: cannot delete another user's message".into());
                            }
                        }
                    }
                    let revoke = args.get("revoke").and_then(|v| v.as_bool()).unwrap_or(true);
                    let prev_pins = self.db.list_pinned_messages(chat_id).unwrap_or_default();
                    let removed_pinned = prev_pins.iter().any(|pid| ids.contains(pid));
                    let msg_ids: Vec<MsgId> = ids.iter().copied().map(MsgId).collect();
                    let pts_update = self
                        .pipeline
                        .delete_messages(chat_id, msg_ids, revoke)
                        .map_err(|e| e.to_string())?;
                    if let Some(idx) = self.search_indexer.upgrade() {
                        for &id in &ids {
                            idx.delete_chat_message_nonblocking(chat_id.0.to_string(), id.to_string());
                        }
                        idx.commit_nonblocking();
                    }
                    if let Some(hm) = self.hybrid_memory.upgrade() {
                        for &id in &ids {
                            let doc_id = format!("{}:{}", chat_id.0, id);
                            hm.remove_document(&doc_id);
                        }
                    }
                    let pts = pts_update.pts;
                    self.broadcast_pts(chat_id, pts_update);
                    if removed_pinned {
                        let pinned_ids = self.db.list_pinned_messages(chat_id).unwrap_or_default();
                        self.broadcast_event(
                            chat_id,
                            UpdateEvent::PinnedMessagesChanged {
                                chat_id,
                                pinned_ids: pinned_ids.into_iter().map(MsgId).collect(),
                            },
                        );
                    }
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "deleted_ids": ids,
                        "pts": pts,
                    }))
                }
                "pin_message" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let (_chat, is_admin) = self.check_membership(chat_id, caller_uid)?;
                    if !is_admin {
                        return Err("Admin rights required to pin/unpin messages".into());
                    }
                    let msg_id = args
                        .get("message_id")
                        .and_then(|v| v.as_i64())
                        .filter(|&id| id > 0)
                        .ok_or_else(|| "Positive 'message_id' is required".to_string())?;
                    let unpin = args.get("unpin").and_then(|v| v.as_bool()).unwrap_or(false);
                    if !unpin {
                        match self.db.get_message(chat_id, MsgId(msg_id)) {
                            Ok(Some(m)) if m.deleted_at_ms.is_none() => {}
                            Ok(_) => return Err("Message not found or deleted".into()),
                            Err(e) => return Err(e.to_string()),
                        }
                    }
                    let pinned_ids = if unpin {
                        self.db
                            .unpin_message(chat_id, Some(MsgId(msg_id)))
                            .map_err(|e| e.to_string())?
                    } else {
                        self.db
                            .pin_message(chat_id, MsgId(msg_id), caller_uid)
                            .map_err(|e| e.to_string())?
                    };
                    self.broadcast_event(
                        chat_id,
                        UpdateEvent::PinnedMessagesChanged {
                            chat_id,
                            pinned_ids: pinned_ids.iter().copied().map(MsgId).collect(),
                        },
                    );
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "pinned_message_ids": pinned_ids,
                    }))
                }
                "add_reaction" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let _ = self.check_membership(chat_id, caller_uid)?;
                    let msg_id = args
                        .get("message_id")
                        .and_then(|v| v.as_i64())
                        .filter(|&id| id > 0)
                        .ok_or_else(|| "Positive 'message_id' is required".to_string())?;
                    let emoji = args
                        .get("emoji")
                        .or_else(|| args.get("reaction"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "'emoji' is required".to_string())?;
                    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("toggle");
                    let (pts, details, counts) = self
                        .db
                        .toggle_reaction_atomic(chat_id, MsgId(msg_id), caller_uid, emoji, action)
                        .map_err(|e| e.to_string())?;
                    let ev = UpdateEvent::ReactionUpdated(ReactionEvent::new(
                        chat_id,
                        MsgId(msg_id),
                        caller_uid,
                        counts.clone(),
                    ));
                    self.broadcast_pts(
                        chat_id,
                        PtsUpdate::new(chat_id, pts, 1, ev, now_ms),
                    );
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "message_id": msg_id,
                        "pts": pts,
                        "reactions": counts,
                        "details": details,
                    }))
                }
                "search_messages" => {
                    let query = args
                        .get("query")
                        .or_else(|| args.get("q"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "Non-empty 'query' is required".to_string())?;
                    let limit = args
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(20)
                        .clamp(1, 100) as usize;
                    let scoped_chat = if let Some(pinned) = caller.chat_id {
                        Self::reject_cross_chat_arg(args, pinned)?;
                        if pinned <= 0 {
                            return Err("Positive 'chat_id' is required".to_string());
                        }
                        Some(pinned)
                    } else {
                        args.get("chat_id").and_then(|v| v.as_i64()).filter(|&c| c > 0)
                    };
                    let allowed_chats: Vec<i64> = if let Some(cid) = scoped_chat {
                        let _ = self.check_membership(ChatId(cid), caller_uid)?;
                        vec![cid]
                    } else {
                        self.db
                            .list_user_member_chat_ids(caller_uid)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|c| c.0)
                            .collect()
                    };

                    let mut hits_json = Vec::new();
                    if let Some(hm) = self.hybrid_memory.upgrade() {
                        let hybrid_hits = hm.search_with_limit(query, None, &allowed_chats, None, limit);
                        for h in hybrid_hits {
                            hits_json.push(json!({
                                "message_id": h.message_id,
                                "chat_id": h.chat_id,
                                "content": h.content,
                                "rrf_score": h.rrf_score,
                                "text_rank": h.text_rank,
                                "vector_rank": h.vector_rank,
                                "graph_rank": h.graph_rank,
                            }));
                        }
                    }
                    if hits_json.is_empty() {
                        if let Some(idx) = self.search_indexer.upgrade() {
                            if let Ok(t_hits) = idx.search_hits_for_chats(query, &allowed_chats, limit) {
                                for h in t_hits {
                                    hits_json.push(json!({
                                        "message_id": h.message_id,
                                        "chat_id": h.chat_id.parse::<i64>().unwrap_or(0),
                                        "content": h.snippet_html,
                                        "score": h.score,
                                        "date_ms": h.date,
                                    }));
                                }
                            } else if let Ok(t_hits) = idx.search_hits(&SearchQueryParams::new(query).with_limit(limit)) {
                                for h in t_hits {
                                    if let Ok(cid) = h.chat_id.parse::<i64>() {
                                        if allowed_chats.contains(&cid) {
                                            hits_json.push(json!({
                                                "message_id": h.message_id,
                                                "chat_id": cid,
                                                "content": h.snippet_html,
                                                "score": h.score,
                                            }));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(json!({
                        "query": query,
                        "count": hits_json.len(),
                        "hits": hits_json,
                    }))
                }
                "create_channel" => {
                    let title = args
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "Non-empty 'title' is required".to_string())?
                        .to_string();
                    let is_forum = args.get("is_forum").and_then(|v| v.as_bool()).unwrap_or(false);
                    let mut chat_type: u8 = match args
                        .get("chat_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("channel")
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "group" => 1,
                        "supergroup" => 2,
                        "channel" => 3,
                        _ => 3,
                    };
                    if is_forum && chat_type == 1 {
                        chat_type = 2;
                    }
                    let about = args.get("about").and_then(|v| v.as_str()).map(ToString::to_string);
                    let username = args
                        .get("username")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().trim_start_matches('@').to_string())
                        .filter(|s| !s.is_empty());
                    let chat_id = self.db.next_chat_id().map_err(|e| e.to_string())?;
                    let peer_type = if matches!(chat_type, 2 | 3) { 3 } else { 2 };
                    let rec = ChatRecord {
                        chat_id,
                        chat_type,
                        peer_type,
                        title: title.clone(),
                        username,
                        about,
                        creator_id: caller_uid,
                        is_forum,
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
                    let mut members = vec![ChatMemberRecord {
                        chat_id,
                        user_id: caller_uid,
                        role: "owner".into(),
                        permissions_bitmask: 0xFFFF,
                        admin_title: None,
                        admin_rights: 0xFFFF,
                        banned_rights: 0,
                        until_date_ms: None,
                        joined_at_ms: now_ms,
                        invited_by: None,
                        updated_at_ms: now_ms,
                    }];
                    if let Some(arr) = args.get("member_ids").and_then(|v| v.as_array()) {
                        for m in arr.iter().filter_map(|x| x.as_u64()).map(UserId) {
                            if m != caller_uid && !members.iter().any(|r| r.user_id == m) {
                                members.push(ChatMemberRecord {
                                    chat_id,
                                    user_id: m,
                                    role: "member".into(),
                                    permissions_bitmask: 0xFFFF,
                                    admin_title: None,
                                    admin_rights: 0,
                                    banned_rights: 0,
                                    until_date_ms: None,
                                    joined_at_ms: now_ms,
                                    invited_by: Some(caller_uid),
                                    updated_at_ms: now_ms,
                                });
                            }
                        }
                    }
                    self.db
                        .create_chat_with_members(&rec, &members)
                        .map_err(|e| e.to_string())?;
                    if let Some(hub) = self.gateway_hub.upgrade() {
                        for m in &members {
                            hub.subscribe_chat(chat_id.0, m.user_id);
                        }
                    }
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "title": title,
                        "chat_type": chat_type,
                        "is_forum": is_forum,
                    }))
                }
                "manage_topic" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let (_chat, is_admin) = self.check_membership(chat_id, caller_uid)?;
                    let action = args
                        .get("action")
                        .and_then(|v| v.as_str())
                        .unwrap_or("list")
                        .to_ascii_lowercase();
                    match action.as_str() {
                        "list" => {
                            let topics = self.db.list_forum_topics(chat_id).map_err(|e| e.to_string())?;
                            Ok(json!({ "chat_id": chat_id.0, "topics": topics }))
                        }
                        "create" => {
                            let title = args
                                .get("title")
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .ok_or_else(|| "Non-empty 'title' is required".to_string())?
                                .to_string();
                            let existing = self.db.list_forum_topics(chat_id).map_err(|e| e.to_string())?;
                            let next_id = existing.iter().map(|t| t.topic_id.0).max().unwrap_or(0) + 1;
                            let topic = ForumTopicRecord {
                                chat_id,
                                topic_id: TopicId(next_id),
                                title: title.clone(),
                                icon_color: 0x6FB9F0,
                                icon_emoji_id: None,
                                creator_id: caller_uid,
                                is_pinned: args.get("is_pinned").and_then(|v| v.as_bool()).unwrap_or(false),
                                is_closed: false,
                                is_hidden: false,
                                last_message_id: None,
                                unread_count: 0,
                                created_at_ms: now_ms,
                            };
                            self.db.create_forum_topic(&topic).map_err(|e| e.to_string())?;
                            let dto = ForumTopicDto {
                                chat_id: topic.chat_id.0,
                                topic_id: topic.topic_id.0,
                                title: topic.title.clone(),
                                icon_color: topic.icon_color,
                                icon_emoji_id: topic.icon_emoji_id.map(|v| v as i64),
                                creator_id: topic.creator_id.0,
                                is_closed: topic.is_closed,
                                is_pinned: topic.is_pinned,
                                is_hidden: topic.is_hidden,
                                unread_count: topic.unread_count,
                                last_message_id: topic.last_message_id.map(|m| m.0),
                                created_at_ms: topic.created_at_ms,
                            };
                            self.broadcast_event(
                                chat_id,
                                UpdateEvent::ForumTopicChanged { chat_id, topic: dto.clone() },
                            );
                            Ok(json!({ "success": true, "chat_id": chat_id.0, "topic": dto }))
                        }
                        "update" => {
                            if !is_admin {
                                return Err("Admin rights required to update forum topics".into());
                            }
                            let topic_id = args
                                .get("topic_id")
                                .and_then(|v| v.as_i64())
                                .filter(|&t| t > 0)
                                .ok_or_else(|| "Positive 'topic_id' is required".to_string())?;
                            let updated = self
                                .db
                                .update_forum_topic(
                                    chat_id,
                                    TopicId(topic_id),
                                    args.get("title").and_then(|v| v.as_str()),
                                    None,
                                    None,
                                    args.get("is_pinned").and_then(|v| v.as_bool()),
                                    args.get("is_closed").and_then(|v| v.as_bool()),
                                    args.get("is_hidden").and_then(|v| v.as_bool()),
                                )
                                .map_err(|e| e.to_string())?;
                            let dto = ForumTopicDto {
                                chat_id: updated.chat_id.0,
                                topic_id: updated.topic_id.0,
                                title: updated.title.clone(),
                                icon_color: updated.icon_color,
                                icon_emoji_id: updated.icon_emoji_id.map(|v| v as i64),
                                creator_id: updated.creator_id.0,
                                is_closed: updated.is_closed,
                                is_pinned: updated.is_pinned,
                                is_hidden: updated.is_hidden,
                                unread_count: updated.unread_count,
                                last_message_id: updated.last_message_id.map(|m| m.0),
                                created_at_ms: updated.created_at_ms,
                            };
                            self.broadcast_event(
                                chat_id,
                                UpdateEvent::ForumTopicChanged { chat_id, topic: dto.clone() },
                            );
                            Ok(json!({ "success": true, "chat_id": chat_id.0, "topic": dto }))
                        }
                        "delete" => {
                            if !is_admin {
                                return Err("Admin rights required to delete forum topics".into());
                            }
                            let topic_id = args
                                .get("topic_id")
                                .and_then(|v| v.as_i64())
                                .filter(|&t| t > 0)
                                .ok_or_else(|| "Positive 'topic_id' is required".to_string())?;
                            let deleted = self
                                .db
                                .delete_forum_topic(chat_id, TopicId(topic_id))
                                .map_err(|e| e.to_string())?;
                            Ok(json!({ "success": deleted, "chat_id": chat_id.0, "topic_id": topic_id }))
                        }
                        other => Err(format!("Unsupported manage_topic action '{other}'")),
                    }
                }
                "list_members" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let _ = self.check_membership(chat_id, caller_uid)?;
                    let members = self
                        .db
                        .get_chat_member_details(chat_id)
                        .map_err(|e| e.to_string())?;
                    Ok(json!({ "chat_id": chat_id.0, "count": members.len(), "members": members }))
                }
                "kick_member" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let (_chat, is_admin) = self.check_membership(chat_id, caller_uid)?;
                    if !is_admin {
                        return Err("Admin rights required to kick members".into());
                    }
                    let target_uid = args
                        .get("user_id")
                        .and_then(|v| v.as_u64())
                        .filter(|&u| u > 0)
                        .ok_or_else(|| "Positive 'user_id' is required".to_string())?;
                    let removed = self
                        .db
                        .remove_chat_member(chat_id, UserId(target_uid))
                        .map_err(|e| e.to_string())?;
                    if removed {
                        if let Some(hub) = self.gateway_hub.upgrade() {
                            hub.unsubscribe_chat(chat_id.0, UserId(target_uid));
                        }
                    }
                    Ok(json!({
                        "success": removed,
                        "chat_id": chat_id.0,
                        "kicked_user_id": target_uid,
                    }))
                }
                "create_poll" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let _ = self.check_send_permitted(chat_id, caller_uid)?;
                    let question = args
                        .get("question")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "Non-empty 'question' is required".to_string())?;
                    let options: Vec<String> = args
                        .get("options")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(str::trim).filter(|s| !s.is_empty()).map(ToString::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let is_anonymous = args.get("is_anonymous").and_then(|v| v.as_bool()).unwrap_or(true);
                    let multiple_choice = args.get("multiple_choice").and_then(|v| v.as_bool()).unwrap_or(false);
                    let is_quiz = args.get("is_quiz").and_then(|v| v.as_bool()).unwrap_or(false);
                    let correct_option_id = args.get("correct_option_id").and_then(|v| v.as_u64()).map(|n| n as u8);
                    let topic_id = args.get("topic_id").and_then(|v| v.as_i64());

                    let (msg_id, pts, poll, item) = self
                        .db
                        .create_poll_atomic(
                            chat_id,
                            caller_uid,
                            question,
                            &options,
                            is_anonymous,
                            multiple_choice,
                            is_quiz,
                            correct_option_id,
                            None,
                            topic_id,
                        )
                        .map_err(|e| e.to_string())?;
                    self.broadcast_pts(
                        chat_id,
                        PtsUpdate::new(chat_id, pts, 1, UpdateEvent::NewMessage(item), now_ms),
                    );
                    let poll_id_num = poll.poll_id;
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "message_id": msg_id.0,
                        "poll_id": poll_id_num,
                        "pts": pts,
                        "poll": poll,
                    }))
                }
                "vote_poll" => {
                    let poll_id = args
                        .get("poll_id")
                        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                        .ok_or_else(|| "Numeric 'poll_id' is required".to_string())?;
                    let (poll_chat_id, _) = self
                        .db
                        .get_poll_for_user(poll_id, caller_uid)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Poll {poll_id} not found"))?;
                    if let Some(pinned) = caller.chat_id {
                        Self::reject_cross_chat_arg(args, pinned)?;
                        if pinned != poll_chat_id.0 {
                            return Err(format!(
                                "Cross-chat target mismatch: poll {poll_id} belongs to chat {}, not pinned chat {pinned}",
                                poll_chat_id.0
                            ));
                        }
                    }
                    if let Some(explicit_chat) = args.get("chat_id").and_then(|v| v.as_i64()) {
                        if explicit_chat != poll_chat_id.0 {
                            return Err(format!(
                                "Poll {poll_id} belongs to chat {}, not chat {}",
                                poll_chat_id.0, explicit_chat
                            ));
                        }
                    }
                    let is_member = self
                        .db
                        .is_user_in_chat(poll_chat_id, caller_uid)
                        .map_err(|e| e.to_string())?;
                    if !is_member {
                        return Err(format!(
                            "User {} is not an active member of chat {}",
                            caller_uid.0, poll_chat_id.0
                        ));
                    }
                    let _ = self.check_membership(poll_chat_id, caller_uid)?;
                    let mut option_ids: Vec<u8> = args
                        .get("option_ids")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|x| x.as_u64().map(|n| n as u8)).collect())
                        .unwrap_or_default();
                    if option_ids.is_empty() {
                        if let Some(idx) = args.get("option_index").and_then(|v| v.as_u64()) {
                            option_ids.push(idx as u8);
                        }
                    }
                    let (cid, mid, pts, poll) = self
                        .db
                        .vote_poll_atomic(poll_id, caller_uid, &option_ids)
                        .map_err(|e| e.to_string())?;
                    self.broadcast_pts(
                        cid,
                        PtsUpdate::new(
                            cid,
                            pts,
                            1,
                            UpdateEvent::PollUpdated {
                                chat_id: cid,
                                message_id: mid,
                                poll: poll.clone(),
                            },
                            now_ms,
                        ),
                    );
                    Ok(json!({
                        "success": true,
                        "chat_id": cid.0,
                        "message_id": mid.0,
                        "pts": pts,
                        "poll": poll,
                    }))
                }
                "launch_miniapp" => {
                    let bot_username = args
                        .get("bot_username")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "'bot_username' is required".to_string())?;
                    let short_name = args
                        .get("short_name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "'short_name' is required".to_string())?;
                    let app = self
                        .db
                        .get_mini_app(bot_username, short_name)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("MiniApp '{bot_username}/{short_name}' not found"))?;
                    let chat_id = if let Some(pinned) = caller.chat_id {
                        Self::reject_cross_chat_arg(args, pinned)?;
                        pinned
                    } else {
                        args.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(0)
                    };
                    if chat_id > 0 {
                        let _ = self.check_membership(ChatId(chat_id), caller_uid)?;
                    }
                    let start_param = args.get("start_param").and_then(|v| v.as_str()).unwrap_or("");
                    let query_id = format!("ma_{}", Uuid::new_v4().simple());
                    let session = self
                        .db
                        .create_mini_app_session(
                            &query_id,
                            caller_uid.0 as i64,
                            &app.app_id,
                            app.bot_user_id,
                            chat_id,
                            "inline",
                            start_param,
                            "",
                            Utc::now().timestamp(),
                            "{}",
                        )
                        .map_err(|e| e.to_string())?;
                    let ui_schema: MiniAppUiSchema =
                        serde_json::from_str(&app.ui_schema_json).unwrap_or_default();
                    let peer_types: Vec<String> =
                        serde_json::from_str(&app.peer_types_json).unwrap_or_default();
                    let manifest = MiniAppManifestDto {
                        app_id: app.app_id.clone(),
                        bot_username: app.bot_username.clone(),
                        short_name: app.short_name.clone(),
                        title: app.title.clone(),
                        description: app.description.clone(),
                        category: app.category.clone(),
                        icon_emoji: app.icon_emoji.clone(),
                        web_url: app.web_url.clone(),
                        version: "1.0.0".to_string(),
                        peer_types,
                        requested_permissions: Vec::new(),
                        ui_schema,
                    };
                    let agent_manifest = manifest.to_agent_manifest();
                    Ok(json!({
                        "success": true,
                        "app_id": app.app_id,
                        "query_id": session.query_id,
                        "session": session,
                        "manifest": manifest,
                        "agent_manifest": agent_manifest,
                    }))
                }
                "invoke_miniapp_action" => {
                    let bot_username = args
                        .get("bot_username")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "'bot_username' is required".to_string())?;
                    let short_name = args
                        .get("short_name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "'short_name' is required".to_string())?;
                    let action_type = args
                        .get("action_type")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "'action_type' is required".to_string())?;
                    let app = self
                        .db
                        .get_mini_app(bot_username, short_name)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("MiniApp '{bot_username}/{short_name}' not found"))?;
                    let explicit_chat = args.get("chat_id").and_then(|v| v.as_i64()).filter(|&c| c > 0);
                    let target_chat = if let Some(pinned) = caller.chat_id {
                        Self::reject_cross_chat_arg(args, pinned)?;
                        Some(pinned).filter(|&c| c > 0)
                    } else {
                        explicit_chat
                    };
                    if let Some(chat_id) = target_chat {
                        let _ = self.check_membership(ChatId(chat_id), caller_uid)?;
                    }
                    if let Some(qid) = args.get("query_id").and_then(|v| v.as_str()) {
                        let session = self
                            .db
                            .get_mini_app_session(qid)
                            .map_err(|e| e.to_string())?
                            .ok_or_else(|| format!("MiniApp session '{qid}' not found"))?;
                        if session.user_id != caller_uid.0 as i64 {
                            return Err("Permission denied: MiniApp session belongs to another user".into());
                        }
                        if session.app_id != app.app_id {
                            return Err("MiniApp session does not match target MiniApp".into());
                        }
                        if let Some(pinned) = caller.chat_id {
                            if session.chat_id != pinned {
                                return Err(format!(
                                    "Cross-chat target mismatch: MiniApp session '{qid}' belongs to chat {}, not pinned chat {pinned}",
                                    session.chat_id
                                ));
                            }
                        } else if let Some(cid) = explicit_chat {
                            if session.chat_id != cid {
                                return Err(format!(
                                    "MiniApp session '{qid}' belongs to chat {}, not chat {cid}",
                                    session.chat_id
                                ));
                            }
                        }
                        if session.chat_id > 0 {
                            let _ = self.check_membership(ChatId(session.chat_id), caller_uid)?;
                        }
                    }

                    let ui_schema: MiniAppUiSchema =
                        serde_json::from_str(&app.ui_schema_json).unwrap_or_default();
                    let peer_types: Vec<String> =
                        serde_json::from_str(&app.peer_types_json).unwrap_or_default();
                    let manifest_dto = MiniAppManifestDto {
                        app_id: app.app_id.clone(),
                        bot_username: app.bot_username.clone(),
                        short_name: app.short_name.clone(),
                        title: app.title.clone(),
                        description: app.description.clone(),
                        category: app.category.clone(),
                        icon_emoji: app.icon_emoji.clone(),
                        web_url: app.web_url.clone(),
                        version: "1.0.0".to_string(),
                        peer_types,
                        requested_permissions: Vec::new(),
                        ui_schema,
                    };
                    let agent_manifest = manifest_dto.to_agent_manifest();
                    let mut action_params = serde_json::Map::new();
                    if let Some(v) = args.get("value").filter(|v| !v.is_null()) {
                        action_params.insert("value".to_string(), v.clone());
                    }
                    if let Some(d) = args.get("data").filter(|v| !v.is_null()) {
                        action_params.insert("data".to_string(), d.clone());
                    }
                    if let Some(w) = args.get("widget_id").filter(|v| !v.is_null()) {
                        action_params.insert("widget_id".to_string(), w.clone());
                    }
                    if let Some(obj) = args.get("params").and_then(|v| v.as_object()) {
                        for (k, v) in obj {
                            action_params.insert(k.clone(), v.clone());
                        }
                    }
                    let approved = args.get("approved").and_then(|v| v.as_bool()).unwrap_or(false);
                    let execution = agent_manifest.execute_action(
                        action_type,
                        &Value::Object(action_params),
                        approved,
                    )?;

                    let next_state = json!({
                        "last_action": action_type,
                        "widget_id": args.get("widget_id"),
                        "value": args.get("value"),
                        "data": args.get("data"),
                        "invoked_by": caller_uid.0,
                        "invoked_at_ms": now_ms,
                    });
                    if let Some(qid) = args.get("query_id").and_then(|v| v.as_str()) {
                        let state_str = serde_json::to_string(&next_state).unwrap_or_else(|_| "{}".into());
                        self.db
                            .update_mini_app_session_state(qid, &state_str)
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(json!({
                        "success": true,
                        "app_id": app.app_id,
                        "action_type": action_type,
                        "execution": execution,
                        "state": next_state,
                    }))
                }
                other => Err(format!("Unsupported PlatformIm tool '{other}'")),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::AppState;
    use crate::ServerConfig;
    use gpui_telegram_agent::hermes::toolsets::{HermesToolExecutor, ToolsetPreset};
    use std::sync::atomic::AtomicBool;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_server_platform_im_bridge_executes_all_14_tools_via_hermes() {
        let dir = TempDir::new().expect("create tempdir");
        let db_path = dir.path().join("im_bridge_test.db");
        let db = Arc::new(ServerDatabase::open(&db_path).expect("open db"));
        let mut cfg = ServerConfig::default();
        cfg.media_storage_path = dir.path().join("media").display().to_string();
        let state = Arc::new(AppState::with_db(cfg, db.clone()));

        let (alice, _, _) = state
            .session_manager
            .register("alice_im", "Pass123456!", "Alice", None, "CLI")
            .unwrap();
        let (bob, _, _) = state
            .session_manager
            .register("bob_im", "Pass123456!", "Bob", None, "CLI")
            .unwrap();
        let now_ms = Utc::now().timestamp_millis();
        for (uid, uname, fname) in [
            (alice.user_id, "alice_im", "Alice"),
            (bob.user_id, "bob_im", "Bob"),
        ] {
            db.create_user(&crate::db::schema::UserRecord {
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
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            })
            .expect("create_user in db");
        }
        let alice_uid_str = alice.user_id.0.to_string();

        // Register bot first and seed a published MiniApp under @pomodoro_test_bot/timer
        let bot = db
            .create_registered_bot(
                alice.user_id.0 as i64,
                "pomodoro_test_bot",
                "Pomodoro Bot",
                "Pomodoro bot",
                "Focus",
                "helpful",
                "",
                "",
                "[]",
                false,
                true,
            )
            .expect("register bot");
        let schema_json = crate::db::miniapp_store::default_pomodoro_ui_schema().to_string();
        db.create_or_upsert_mini_app_with_options(
            bot.bot_user_id,
            "pomodoro_test_bot",
            "timer",
            "Pomodoro Timer",
            "Focus timer",
            "productivity",
            "🍅",
            None,
            &schema_json,
            true,
            true,
            false,
            "",
            "",
            None,
            true,
        )
        .expect("seed miniapp");

        let ctx = state.hermes.build_tool_context_with_user(
            "sess_platform_im_test",
            None,
            None,
            Some(&alice_uid_str),
            1,
            Some(ToolsetPreset::Full),
            0,
            Arc::new(AtomicBool::new(false)),
        );
        assert!(
            ctx.platform_im_bridge.is_some(),
            "AppState::with_db must wire ServerPlatformImBridge into HermesEngine"
        );

        // 1. create_channel (forum group so topic management works too)
        let r_create = HermesToolExecutor::execute(
            "create_channel",
            json!({
                "title": "M4 Platform IM War Room",
                "chat_type": "supergroup",
                "is_forum": true,
                "about": "Integration test chat",
                "member_ids": [bob.user_id.0]
            }),
            &ctx,
        )
        .await;
        assert!(r_create.success, "create_channel failed: {:?}", r_create.error);
        let v_create: Value = serde_json::from_str(&r_create.output).unwrap();
        let chat_id = v_create["chat_id"].as_i64().unwrap();
        assert!(chat_id > 0);

        // 2. list_members
        let r_members =
            HermesToolExecutor::execute("list_members", json!({ "chat_id": chat_id }), &ctx).await;
        assert!(r_members.success, "list_members failed: {:?}", r_members.error);
        let v_members: Value = serde_json::from_str(&r_members.output).unwrap();
        assert_eq!(v_members["count"].as_u64(), Some(2));

        // 3. manage_topic (create + list)
        let r_topic = HermesToolExecutor::execute(
            "manage_topic",
            json!({ "chat_id": chat_id, "action": "create", "title": "Hybrid Search" }),
            &ctx,
        )
        .await;
        assert!(r_topic.success, "manage_topic failed: {:?}", r_topic.error);
        let v_topic: Value = serde_json::from_str(&r_topic.output).unwrap();
        let topic_id = v_topic["topic"]["topic_id"].as_i64().unwrap();

        // 4. send_message (including GenerativeCard or text)
        let r_send = HermesToolExecutor::execute(
            "send_message",
            json!({
                "chat_id": chat_id,
                "topic_id": topic_id,
                "text": "Platform IM native tool check alpha99"
            }),
            &ctx,
        )
        .await;
        assert!(r_send.success, "send_message failed: {:?}", r_send.error);
        let v_send: Value = serde_json::from_str(&r_send.output).unwrap();
        let msg_id = v_send["message_id"].as_i64().unwrap();

        // 5. edit_message
        let r_edit = HermesToolExecutor::execute(
            "edit_message",
            json!({
                "chat_id": chat_id,
                "message_id": msg_id,
                "text": "Platform IM native tool check beta88"
            }),
            &ctx,
        )
        .await;
        assert!(r_edit.success, "edit_message failed: {:?}", r_edit.error);

        // 6. pin_message
        let r_pin = HermesToolExecutor::execute(
            "pin_message",
            json!({ "chat_id": chat_id, "message_id": msg_id }),
            &ctx,
        )
        .await;
        assert!(r_pin.success, "pin_message failed: {:?}", r_pin.error);

        // 7. add_reaction
        let r_react = HermesToolExecutor::execute(
            "add_reaction",
            json!({ "chat_id": chat_id, "message_id": msg_id, "emoji": "🚀", "action": "add" }),
            &ctx,
        )
        .await;
        assert!(r_react.success, "add_reaction failed: {:?}", r_react.error);

        // 8. search_messages (powered by HybridMemoryEngine / TantivyIndexer with ACL)
        let r_search = HermesToolExecutor::execute(
            "search_messages",
            json!({ "chat_id": chat_id, "query": "beta88" }),
            &ctx,
        )
        .await;
        assert!(r_search.success, "search_messages failed: {:?}", r_search.error);
        let v_search: Value = serde_json::from_str(&r_search.output).unwrap();
        assert!(v_search["count"].as_u64().unwrap_or(0) >= 1);

        // 9. create_poll & 10. vote_poll
        let r_poll = HermesToolExecutor::execute(
            "create_poll",
            json!({
                "chat_id": chat_id,
                "question": "Ship M4?",
                "options": ["Yes", "Now"]
            }),
            &ctx,
        )
        .await;
        assert!(r_poll.success, "create_poll failed: {:?}", r_poll.error);
        let v_poll: Value = serde_json::from_str(&r_poll.output).unwrap();
        let poll_id = v_poll["poll_id"].as_i64().unwrap();

        let r_vote = HermesToolExecutor::execute(
            "vote_poll",
            json!({ "chat_id": chat_id, "poll_id": poll_id, "option_ids": [1] }),
            &ctx,
        )
        .await;
        assert!(r_vote.success, "vote_poll failed: {:?}", r_vote.error);

        // 11. launch_miniapp & 12. invoke_miniapp_action
        let r_launch = HermesToolExecutor::execute(
            "launch_miniapp",
            json!({ "bot_username": "pomodoro_test_bot", "short_name": "timer", "chat_id": chat_id }),
            &ctx,
        )
        .await;
        assert!(r_launch.success, "launch_miniapp failed: {:?}", r_launch.error);
        let v_launch: Value = serde_json::from_str(&r_launch.output).unwrap();
        let query_id = v_launch["query_id"].as_str().unwrap().to_string();
        assert!(v_launch["agent_manifest"]["actions"].is_array());

        let r_invoke = HermesToolExecutor::execute(
            "invoke_miniapp_action",
            json!({
                "bot_username": "pomodoro_test_bot",
                "short_name": "timer",
                "action_type": "widget:completed_sessions",
                "query_id": query_id,
                "value": 25
            }),
            &ctx,
        )
        .await;
        assert!(r_invoke.success, "invoke_miniapp_action failed: {:?}", r_invoke.error);

        // Undeclared MiniApp action must be rejected
        let r_bad_invoke = HermesToolExecutor::execute(
            "invoke_miniapp_action",
            json!({
                "bot_username": "pomodoro_test_bot",
                "short_name": "timer",
                "action_type": "undeclared_action_xyz",
                "query_id": query_id,
            }),
            &ctx,
        )
        .await;
        assert!(!r_bad_invoke.success, "undeclared action_type must be rejected");

        // 13. kick_member
        let r_kick = HermesToolExecutor::execute(
            "kick_member",
            json!({ "chat_id": chat_id, "user_id": bob.user_id.0 }),
            &ctx,
        )
        .await;
        assert!(r_kick.success, "kick_member failed: {:?}", r_kick.error);

        // 14. delete_message
        let r_del = HermesToolExecutor::execute(
            "delete_message",
            json!({ "chat_id": chat_id, "message_id": msg_id, "revoke": true }),
            &ctx,
        )
        .await;
        assert!(r_del.success, "delete_message failed: {:?}", r_del.error);
    }

    #[tokio::test]
    async fn test_platform_im_vote_poll_acl_miniapp_session_and_cross_chat_memory() {
        let dir = TempDir::new().expect("create tempdir");
        let db_path = dir.path().join("im_bridge_acl_test.db");
        let db = Arc::new(ServerDatabase::open(&db_path).expect("open db"));
        let mut cfg = ServerConfig::default();
        cfg.media_storage_path = dir.path().join("media").display().to_string();
        let state = Arc::new(AppState::with_db(cfg, db.clone()));

        let (alice, _, _) = state
            .session_manager
            .register("alice_acl", "Pass123456!", "Alice", None, "CLI")
            .unwrap();
        let (eve, _, _) = state
            .session_manager
            .register("eve_acl", "Pass123456!", "Eve", None, "CLI")
            .unwrap();
        let now_ms = Utc::now().timestamp_millis();
        for (uid, uname, fname) in [
            (alice.user_id, "alice_acl", "Alice"),
            (eve.user_id, "eve_acl", "Eve"),
        ] {
            db.create_user(&crate::db::schema::UserRecord {
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
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            })
            .unwrap();
        }

        let bot = db
            .create_registered_bot(
                alice.user_id.0 as i64,
                "acl_pomodoro_bot",
                "Pomodoro Bot",
                "Pomodoro bot",
                "Focus",
                "helpful",
                "",
                "",
                "[]",
                false,
                true,
            )
            .unwrap();
        let schema_json = crate::db::miniapp_store::default_pomodoro_ui_schema().to_string();
        db.create_or_upsert_mini_app_with_options(
            bot.bot_user_id,
            "acl_pomodoro_bot",
            "timer",
            "Pomodoro Timer",
            "Focus timer",
            "productivity",
            "🍅",
            None,
            &schema_json,
            true,
            true,
            false,
            "",
            "",
            None,
            true,
        )
        .unwrap();

        let ctx_alice = state.hermes.build_tool_context_with_user(
            "sess_alice_acl",
            None,
            None,
            Some(&alice.user_id.0.to_string()),
            1,
            Some(ToolsetPreset::Full),
            0,
            Arc::new(AtomicBool::new(false)),
        );
        let ctx_eve = state.hermes.build_tool_context_with_user(
            "sess_eve_acl",
            None,
            None,
            Some(&eve.user_id.0.to_string()),
            1,
            Some(ToolsetPreset::Full),
            0,
            Arc::new(AtomicBool::new(false)),
        );

        let r_chat1 = HermesToolExecutor::execute(
            "create_channel",
            json!({ "title": "Alice Secret 1", "chat_type": "group" }),
            &ctx_alice,
        )
        .await;
        let chat1 = serde_json::from_str::<Value>(&r_chat1.output).unwrap()["chat_id"]
            .as_i64()
            .unwrap();
        let r_chat2 = HermesToolExecutor::execute(
            "create_channel",
            json!({ "title": "Alice Secret 2", "chat_type": "group" }),
            &ctx_alice,
        )
        .await;
        let chat2 = serde_json::from_str::<Value>(&r_chat2.output).unwrap()["chat_id"]
            .as_i64()
            .unwrap();

        // Non-member Eve must not be able to vote in Alice's poll
        let r_poll = HermesToolExecutor::execute(
            "create_poll",
            json!({ "chat_id": chat1, "question": "Secret?", "options": ["Yes", "No"] }),
            &ctx_alice,
        )
        .await;
        let poll_id = serde_json::from_str::<Value>(&r_poll.output).unwrap()["poll_id"]
            .as_i64()
            .unwrap();
        let r_eve_vote = HermesToolExecutor::execute(
            "vote_poll",
            json!({ "poll_id": poll_id, "option_ids": [0] }),
            &ctx_eve,
        )
        .await;
        assert!(!r_eve_vote.success, "non-member vote_poll must be rejected");

        // Cross-user MiniApp query_id mutation must be rejected
        let r_launch = HermesToolExecutor::execute(
            "launch_miniapp",
            json!({ "bot_username": "acl_pomodoro_bot", "short_name": "timer" }),
            &ctx_alice,
        )
        .await;
        let query_id = serde_json::from_str::<Value>(&r_launch.output).unwrap()["query_id"]
            .as_str()
            .unwrap()
            .to_string();
        let r_eve_ma = HermesToolExecutor::execute(
            "invoke_miniapp_action",
            json!({
                "bot_username": "acl_pomodoro_bot",
                "short_name": "timer",
                "action_type": "main_button_click",
                "query_id": query_id,
                "value": "hijack",
            }),
            &ctx_eve,
        )
        .await;
        assert!(!r_eve_ma.success, "cross-user MiniApp session mutation must fail");

        // Cross-chat msg_id = 1 isolation via send_message & delete_message (use fresh chat3 & chat1 before poll or check msg 2)
        let r_chat3 = HermesToolExecutor::execute(
            "create_channel",
            json!({ "title": "Alice Secret 3", "chat_type": "group" }),
            &ctx_alice,
        )
        .await;
        let chat3 = serde_json::from_str::<Value>(&r_chat3.output).unwrap()["chat_id"]
            .as_i64()
            .unwrap();
        let m1 = HermesToolExecutor::execute(
            "send_message",
            json!({ "chat_id": chat3, "text": "alpha_cross_chat_token_111" }),
            &ctx_alice,
        )
        .await;
        assert!(m1.success);
        let m1_id = serde_json::from_str::<Value>(&m1.output).unwrap()["message_id"]
            .as_i64()
            .unwrap();
        let m2 = HermesToolExecutor::execute(
            "send_message",
            json!({ "chat_id": chat2, "text": "beta_cross_chat_token_222" }),
            &ctx_alice,
        )
        .await;
        assert!(m2.success);
        let m2_id = serde_json::from_str::<Value>(&m2.output).unwrap()["message_id"]
            .as_i64()
            .unwrap();
        assert_eq!(m1_id, m2_id, "both chats start at message_id 1");

        let s1 = HermesToolExecutor::execute(
            "search_messages",
            json!({ "chat_id": chat3, "query": "alpha_cross_chat_token_111" }),
            &ctx_alice,
        )
        .await;
        assert!(serde_json::from_str::<Value>(&s1.output).unwrap()["count"].as_u64().unwrap_or(0) >= 1);

        let del2 = HermesToolExecutor::execute(
            "delete_message",
            json!({ "chat_id": chat2, "message_id": m2_id, "revoke": true }),
            &ctx_alice,
        )
        .await;
        assert!(del2.success);
        let s1_after = HermesToolExecutor::execute(
            "search_messages",
            json!({ "chat_id": chat3, "query": "alpha_cross_chat_token_111" }),
            &ctx_alice,
        )
        .await;
        assert!(
            serde_json::from_str::<Value>(&s1_after.output).unwrap()["count"].as_u64().unwrap_or(0) >= 1,
            "deleting chat2 msg 1 must not delete chat3 msg 1"
        );
    }
}
