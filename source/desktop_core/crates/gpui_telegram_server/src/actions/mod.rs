use crate::db::schema::{ChatMemberRecord, ChatRecord};
use crate::db::ServerDatabase;
use crate::messaging::pipeline::MessagePipeline;
use crate::search::schema::SearchDocument;
use crate::search::tantivy_indexer::TantivyIndexer;
use crate::server::extract_message_content_text;
use crate::ws::GatewayHub;
use chrono::Utc;
use futures::future::BoxFuture;
use gpui_telegram_core::action_bus::{
    ActionCategory, ActionContext, ActionError, ActionRiskLevel, ChatHistoryMessageItem,
    DeleteMessageInput, DeleteMessageOutput, DialogSummaryItem, GetChatHistoryInput,
    GetChatHistoryOutput, ListDialogsInput, ListDialogsOutput, SendMessageInput, SendMessageOutput,
    TypedPlatformAction,
};
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::{ChatId, MessageFlags, MsgId, PeerId, TopicId, UserId};
use gpui_telegram_proto::message::{MessageContent, MessageItem, ReplyHeader};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use std::collections::HashSet;
use std::sync::Arc;

fn map_telegram_err_to_action(err: TelegramError) -> ActionError {
    match err {
        TelegramError::Validation(m) => ActionError::InvalidInput(m),
        TelegramError::NotFound(m) => ActionError::NotFound(m),
        TelegramError::PermissionDenied(m) => ActionError::Forbidden(m),
        TelegramError::Auth(m) | TelegramError::AuthFailed(m) => ActionError::Unauthorized(m),
        other => ActionError::ExecutionFailed(other.to_string()),
    }
}

fn require_user(ctx: &ActionContext) -> Result<UserId, ActionError> {
    ctx.user_id
        .map(UserId)
        .ok_or_else(|| ActionError::Unauthorized("Authenticated session required".into()))
}

fn verify_active_chat_member(
    db: &ServerDatabase,
    chat_id: ChatId,
    user_id: UserId,
) -> Result<(ChatRecord, Option<ChatMemberRecord>, bool), ActionError> {
    let chat = db
        .get_chat(chat_id)
        .map_err(map_telegram_err_to_action)?
        .ok_or_else(|| ActionError::NotFound(format!("Chat {} not found", chat_id.0)))?;

    let member = db
        .get_chat_member(chat_id, user_id)
        .map_err(map_telegram_err_to_action)?;

    if chat.creator_id == user_id {
        return Ok((chat, member, true));
    }

    let Some(m) = member else {
        return Err(ActionError::Forbidden(
            "Caller is not a member of this chat".into(),
        ));
    };

    if ServerDatabase::is_inactive_member_role(&m.role) {
        return Err(ActionError::Forbidden(
            "Caller membership is not active".into(),
        ));
    }

    let is_admin = ServerDatabase::is_admin_member_role(&m.role);

    Ok((chat, Some(m), is_admin))
}

#[derive(Clone)]
pub struct ServerSendMessageAction {
    pub db: Arc<ServerDatabase>,
    pub pipeline: Arc<MessagePipeline>,
    pub gateway_hub: Arc<GatewayHub>,
    pub search_indexer: Arc<TantivyIndexer>,
}

impl ServerSendMessageAction {
    pub fn new(
        db: Arc<ServerDatabase>,
        pipeline: Arc<MessagePipeline>,
        gateway_hub: Arc<GatewayHub>,
        search_indexer: Arc<TantivyIndexer>,
    ) -> Self {
        Self {
            db,
            pipeline,
            gateway_hub,
            search_indexer,
        }
    }
}

impl TypedPlatformAction for ServerSendMessageAction {
    type Input = SendMessageInput;
    type Output = SendMessageOutput;

    fn name(&self) -> &'static str {
        "send_message"
    }

    fn category(&self) -> ActionCategory {
        ActionCategory::Messaging
    }

    fn description(&self) -> &'static str {
        "Send a text message to a specific chat or topic"
    }

    fn risk_level(&self) -> ActionRiskLevel {
        ActionRiskLevel::Low
    }

    fn execute<'a>(
        &'a self,
        ctx: ActionContext,
        input: Self::Input,
    ) -> BoxFuture<'a, Result<Self::Output, ActionError>> {
        Box::pin(async move {
            let user_id = require_user(&ctx)?;

            if input.chat_id <= 0 {
                return Err(ActionError::InvalidInput(
                    "chat_id must be a positive integer".into(),
                ));
            }
            let text = input.text.trim();
            if text.is_empty() || text.chars().count() > 4096 {
                return Err(ActionError::InvalidInput(
                    "Message text must be 1..=4096 characters".into(),
                ));
            }
            if let Some(r) = input.reply_to_id {
                if r <= 0 {
                    return Err(ActionError::InvalidInput(
                        "reply_to_id must be a positive integer".into(),
                    ));
                }
            }
            if let Some(t) = input.topic_id {
                if t <= 0 {
                    return Err(ActionError::InvalidInput(
                        "topic_id must be a positive integer".into(),
                    ));
                }
            }
            if let Some(n) = input.client_nonce {
                if n == 0 || n > i64::MAX as u64 {
                    return Err(ActionError::InvalidInput(
                        "client_nonce must be between 1 and i64::MAX".into(),
                    ));
                }
            }

            let chat_id = ChatId(input.chat_id);
            let (chat, member_opt, _) = verify_active_chat_member(&self.db, chat_id, user_id)?;
            if let Some(m) = member_opt {
                if m.role.trim().eq_ignore_ascii_case("restricted") && (m.banned_rights & 1) != 0 {
                    return Err(ActionError::Forbidden(
                        "User is restricted from sending messages".into(),
                    ));
                }
            }
            if matches!(chat.chat_type, 0 | 4) {
                if let Ok(members) = self.db.get_chat_members(chat_id) {
                    for peer_uid in members {
                        if peer_uid != user_id
                            && (self.db.is_user_blocked(peer_uid, user_id).unwrap_or(false)
                                || self.db.is_user_blocked(user_id, peer_uid).unwrap_or(false))
                        {
                            return Err(ActionError::Forbidden(
                                "Cannot send messages in a blocked direct chat".into(),
                            ));
                        }
                    }
                }
            }

            let to_peer = if chat.chat_type == 0 {
                PeerId::User(user_id)
            } else {
                PeerId::Chat(chat_id)
            };
            let content = MessageContent::Text {
                text: text.to_string(),
                entities: Vec::new(),
            };
            let mut item = MessageItem::new(MsgId(0), chat_id, user_id, to_peer, content);
            item.topic_id = input.topic_id.map(TopicId);
            item.client_nonce = input.client_nonce;
            item.flags = MessageFlags {
                is_out: true,
                is_silent: input.silent,
                ..Default::default()
            };
            if let Some(r_id) = input.reply_to_id {
                item.reply_to = Some(ReplyHeader::new(MsgId(r_id)));
            }

            let pts_update = self
                .pipeline
                .process_incoming_message_strict(item)
                .map_err(map_telegram_err_to_action)?;

            let (msg_id, date_ms, effective_nonce) = match &pts_update.event {
                UpdateEvent::NewMessage(m) | UpdateEvent::EditMessage(m) => (
                    m.id,
                    m.date.timestamp_millis(),
                    m.client_nonce.or(input.client_nonce).unwrap_or(0),
                ),
                _ => (
                    MsgId(0),
                    pts_update.timestamp_ms,
                    input.client_nonce.unwrap_or(0),
                ),
            };

            if pts_update.pts_count > 0 {
                self.search_indexer.index_message_nonblocking(SearchDocument::new(
                    msg_id.0.to_string(),
                    chat_id.0.to_string(),
                    user_id.0.to_string(),
                    date_ms,
                    text.to_string(),
                    None,
                ));
                self.search_indexer.commit_nonblocking();
                self.gateway_hub.broadcast_to_chat_and_members(
                    chat_id,
                    None,
                    WsServerFrame::PtsPush(pts_update.clone()),
                );
            }

            Ok(SendMessageOutput {
                message_id: msg_id.0,
                chat_id: input.chat_id,
                pts: pts_update.pts,
                date_ms,
                client_nonce: effective_nonce,
            })
        })
    }
}

#[derive(Clone)]
pub struct ServerGetChatHistoryAction {
    pub db: Arc<ServerDatabase>,
}

impl ServerGetChatHistoryAction {
    pub fn new(db: Arc<ServerDatabase>) -> Self {
        Self { db }
    }
}

impl TypedPlatformAction for ServerGetChatHistoryAction {
    type Input = GetChatHistoryInput;
    type Output = GetChatHistoryOutput;

    fn name(&self) -> &'static str {
        "get_chat_history"
    }

    fn category(&self) -> ActionCategory {
        ActionCategory::Messaging
    }

    fn description(&self) -> &'static str {
        "Retrieve recent messages and history for a given chat"
    }

    fn risk_level(&self) -> ActionRiskLevel {
        ActionRiskLevel::ReadOnly
    }

    fn execute<'a>(
        &'a self,
        ctx: ActionContext,
        input: Self::Input,
    ) -> BoxFuture<'a, Result<Self::Output, ActionError>> {
        Box::pin(async move {
            let user_id = require_user(&ctx)?;

            if input.chat_id <= 0 {
                return Err(ActionError::InvalidInput(format!(
                    "Invalid chat_id {}",
                    input.chat_id
                )));
            }
            if !(1..=100).contains(&input.limit) {
                return Err(ActionError::InvalidInput(
                    "limit must be between 1 and 100".into(),
                ));
            }
            if let Some(offset_id) = input.offset_id {
                if offset_id <= 0 {
                    return Err(ActionError::InvalidInput(
                        "offset_id must be a positive integer".into(),
                    ));
                }
            }
            if let Some(topic_id) = input.topic_id {
                if topic_id <= 0 {
                    return Err(ActionError::InvalidInput(
                        "topic_id must be a positive integer".into(),
                    ));
                }
            }

            let chat_id = ChatId(input.chat_id);
            verify_active_chat_member(&self.db, chat_id, user_id)?;

            let _ = self
                .db
                .purge_expired_ttl_messages(Utc::now().timestamp_millis());

            let (rows, has_more) = self
                .db
                .list_messages_page(
                    chat_id,
                    input.topic_id.map(TopicId),
                    input.offset_id,
                    None,
                    input.limit,
                )
                .map_err(map_telegram_err_to_action)?;

            let messages = rows
                .into_iter()
                .map(|rec| {
                    let text = MessagePipeline::record_to_item(&rec)
                        .map(|item| extract_message_content_text(&item.content))
                        .unwrap_or_default();
                    ChatHistoryMessageItem {
                        message_id: rec.message_id.0,
                        sender_id: rec.sender_id.0 as i64,
                        text,
                        date_ms: rec.date_ms,
                    }
                })
                .collect();

            Ok(GetChatHistoryOutput {
                chat_id: input.chat_id,
                messages,
                has_more,
            })
        })
    }
}

#[derive(Clone)]
pub struct ServerListDialogsAction {
    pub db: Arc<ServerDatabase>,
}

impl ServerListDialogsAction {
    pub fn new(db: Arc<ServerDatabase>) -> Self {
        Self { db }
    }
}

impl TypedPlatformAction for ServerListDialogsAction {
    type Input = ListDialogsInput;
    type Output = ListDialogsOutput;

    fn name(&self) -> &'static str {
        "list_dialogs"
    }

    fn category(&self) -> ActionCategory {
        ActionCategory::Dialogs
    }

    fn description(&self) -> &'static str {
        "List recent dialogs, active chats, and conversation summaries"
    }

    fn risk_level(&self) -> ActionRiskLevel {
        ActionRiskLevel::ReadOnly
    }

    fn execute<'a>(
        &'a self,
        ctx: ActionContext,
        input: Self::Input,
    ) -> BoxFuture<'a, Result<Self::Output, ActionError>> {
        Box::pin(async move {
            let user_id = require_user(&ctx)?;

            if !(1..=200).contains(&input.limit) {
                return Err(ActionError::InvalidInput(
                    "limit must be between 1 and 200".into(),
                ));
            }

            let chats = self
                .db
                .list_user_chats(user_id)
                .map_err(map_telegram_err_to_action)?;

            let folder_opt = if let Some(fid) = input.folder_id {
                let folders = self
                    .db
                    .list_folders(user_id)
                    .map_err(map_telegram_err_to_action)?;
                let found = folders
                    .into_iter()
                    .find(|f| f.id == fid as i64)
                    .ok_or_else(|| ActionError::NotFound(format!("Folder {} not found", fid)))?;
                Some(found)
            } else {
                None
            };

            let mut dialogs = Vec::new();
            let mut total_unread_chats = 0u32;

            for c in chats {
                let unread_count = self
                    .db
                    .get_read_state(c.chat_id, user_id, TopicId(0))
                    .ok()
                    .flatten()
                    .map(|rs| rs.unread_count)
                    .unwrap_or(c.unread_count);

                if unread_count > 0 {
                    total_unread_chats = total_unread_chats.saturating_add(1);
                }

                if let Some(ref folder) = folder_opt {
                    let cid = c.chat_id.0;
                    if folder.exclude_chats.contains(&cid) {
                        continue;
                    }
                    let explicit_include = folder.include_chats.contains(&cid);
                    if !explicit_include {
                        let (is_archived, _, is_muted, _) = self
                            .db
                            .get_user_chat_setting(user_id, c.chat_id)
                            .unwrap_or((false, false, false, None));
                        if folder.exclude_muted && is_muted {
                            continue;
                        }
                        if folder.exclude_read && unread_count == 0 {
                            continue;
                        }
                        if folder.exclude_archived && is_archived {
                            continue;
                        }
                        let type_matches = match c.chat_type {
                            1 | 2 => folder.include_groups,
                            3 => folder.include_channels,
                            4 => folder.include_bots,
                            0 | 5 => folder.include_contacts || folder.include_non_contacts,
                            _ => false,
                        };
                        if !type_matches {
                            continue;
                        }
                    }
                }

                if input.unread_only && unread_count == 0 {
                    continue;
                }

                let last_message_text = if c.last_message_id.0 > 0 {
                    self.db
                        .get_message(c.chat_id, c.last_message_id)
                        .ok()
                        .flatten()
                        .and_then(|rec| {
                            MessagePipeline::record_to_item(&rec)
                                .ok()
                                .map(|item| extract_message_content_text(&item.content))
                        })
                        .filter(|s| !s.is_empty())
                } else {
                    None
                };

                dialogs.push(DialogSummaryItem {
                    chat_id: c.chat_id.0,
                    title: c.title,
                    unread_count,
                    last_message_text,
                    last_message_date_ms: c.last_message_date_ms,
                });
            }

            dialogs.truncate(input.limit as usize);

            Ok(ListDialogsOutput {
                dialogs,
                total_unread_chats,
            })
        })
    }
}

#[derive(Clone)]
pub struct ServerDeleteMessageAction {
    pub db: Arc<ServerDatabase>,
    pub pipeline: Arc<MessagePipeline>,
    pub gateway_hub: Arc<GatewayHub>,
    pub search_indexer: Arc<TantivyIndexer>,
}

impl ServerDeleteMessageAction {
    pub fn new(
        db: Arc<ServerDatabase>,
        pipeline: Arc<MessagePipeline>,
        gateway_hub: Arc<GatewayHub>,
        search_indexer: Arc<TantivyIndexer>,
    ) -> Self {
        Self {
            db,
            pipeline,
            gateway_hub,
            search_indexer,
        }
    }
}

impl TypedPlatformAction for ServerDeleteMessageAction {
    type Input = DeleteMessageInput;
    type Output = DeleteMessageOutput;

    fn name(&self) -> &'static str {
        "delete_message"
    }

    fn category(&self) -> ActionCategory {
        ActionCategory::Messaging
    }

    fn description(&self) -> &'static str {
        "Delete one or more messages from a chat (destructive, requires approval)"
    }

    fn risk_level(&self) -> ActionRiskLevel {
        ActionRiskLevel::High
    }

    fn execute<'a>(
        &'a self,
        ctx: ActionContext,
        input: Self::Input,
    ) -> BoxFuture<'a, Result<Self::Output, ActionError>> {
        Box::pin(async move {
            if !ctx.approved {
                return Err(ActionError::Forbidden(
                    "Explicit approval required for delete_message".into(),
                ));
            }
            let user_id = require_user(&ctx)?;

            if input.chat_id <= 0 {
                return Err(ActionError::InvalidInput(
                    "chat_id must be a positive integer".into(),
                ));
            }
            if input.message_ids.is_empty() {
                return Err(ActionError::InvalidInput(
                    "No message IDs specified for deletion".into(),
                ));
            }
            let mut seen = HashSet::new();
            let mut dedup_ids = Vec::with_capacity(input.message_ids.len());
            for mid in input.message_ids {
                if mid <= 0 {
                    return Err(ActionError::InvalidInput(format!(
                        "Invalid message_id {mid}; all message IDs must be positive"
                    )));
                }
                if seen.insert(mid) {
                    dedup_ids.push(mid);
                }
            }

            let chat_id = ChatId(input.chat_id);
            let (_, _, is_admin) = verify_active_chat_member(&self.db, chat_id, user_id)?;

            let mut deletable_ids = Vec::new();
            for mid in dedup_ids {
                if let Some(msg) = self
                    .db
                    .get_message(chat_id, MsgId(mid))
                    .map_err(map_telegram_err_to_action)?
                {
                    if msg.deleted_at_ms.is_none() {
                        if msg.sender_id != user_id && !is_admin {
                            return Err(ActionError::Forbidden(
                                "Cannot delete another user's message".into(),
                            ));
                        }
                        deletable_ids.push(MsgId(mid));
                    }
                }
            }

            if !deletable_ids.is_empty() {
                let prev_pins = self.db.list_pinned_messages(chat_id).unwrap_or_default();
                let removed_pinned = prev_pins
                    .iter()
                    .any(|pid| deletable_ids.iter().any(|mid| mid.0 == *pid));
                let pts_update = self
                    .pipeline
                    .delete_messages(chat_id, deletable_ids.clone(), input.for_everyone)
                    .map_err(map_telegram_err_to_action)?;

                for mid in &deletable_ids {
                    self.search_indexer
                        .delete_chat_message_nonblocking(chat_id.0.to_string(), mid.0.to_string());
                }
                self.search_indexer.commit_nonblocking();
                self.gateway_hub.broadcast_to_chat_and_members(
                    chat_id,
                    None,
                    WsServerFrame::PtsPush(pts_update),
                );
                if removed_pinned {
                    let pinned_ids = self.db.list_pinned_messages(chat_id).unwrap_or_default();
                    self.gateway_hub.broadcast_to_chat_and_members(
                        chat_id,
                        None,
                        WsServerFrame::Update(UpdateEvent::PinnedMessagesChanged {
                            chat_id,
                            pinned_ids: pinned_ids.into_iter().map(MsgId).collect(),
                        }),
                    );
                }
            }

            Ok(DeleteMessageOutput {
                deleted_count: deletable_ids.len() as u32,
                chat_id: input.chat_id,
            })
        })
    }
}
