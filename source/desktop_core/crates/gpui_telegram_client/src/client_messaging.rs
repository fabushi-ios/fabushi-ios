use crate::client::TelegramClient;
use crate::net::http_client::HistoricalReaction;
use crate::net::{
    CreatePollHttpRequest, EditMessageHttpRequest, HistoricalMessage, HttpClientError,
    PollHttpResponse, ReactionHttpRequest, ReactionHttpResponse, ReadChatHttpRequest,
    ReadChatHttpResponse, RichSendMessageRequest, RichSendMessageResponse,
};
use crate::outbox::OutboxMessageRecord;
use gpui_telegram_core::types::{ChatId, MessageId, PeerId, TopicId, UserId};
use gpui_telegram_proto::message::{
    MessageContent, MessageEntity, MessageItem, ReactionCount, ReplyHeader,
};
use gpui_telegram_proto::sync::{SyncGetDifferenceRequest, SyncGetDifferenceResponse, TypingAction, TypingEvent};
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};

impl TelegramClient {
    fn stage_provisional_message(&self, req: &RichSendMessageRequest, nonce: u64) {
        let chat_id = ChatId(req.chat_id);
        let sender_id = UserId(self.current_user_id().unwrap_or(0));
        let content = req.content.clone().unwrap_or_else(|| {
            MessageContent::text_with_entities(
                req.text.clone().unwrap_or_default(),
                req.entities.clone(),
            )
        });
        let mut provisional = MessageItem::new(
            self.next_provisional_msg_id(),
            chat_id,
            sender_id,
            PeerId::Chat(chat_id),
            content,
        )
        .with_client_nonce(nonce);
        provisional.topic_id = req.topic_id.map(TopicId);
        if let Some(reply_id) = req.reply_to {
            let mut rh = ReplyHeader::new(MessageId(reply_id));
            if let Some(ref qt) = req.quote_text {
                rh = rh.with_quote(qt.clone());
            }
            provisional = provisional.with_reply_to(rh);
        }
        self.local_store.insert_message(provisional);
    }

    pub fn enqueue_message(
        &self,
        mut req: RichSendMessageRequest,
    ) -> gpui_telegram_core::error::Result<OutboxMessageRecord> {
        let nonce = req.client_nonce.unwrap_or_else(|| self.next_client_nonce());
        req.client_nonce = Some(nonce);
        self.stage_provisional_message(&req, nonce);
        self.outbox.enqueue(&req, nonce)
    }

    pub async fn send_message_durable(
        &self,
        mut req: RichSendMessageRequest,
    ) -> std::result::Result<RichSendMessageResponse, HttpClientError> {
        let nonce = req.client_nonce.unwrap_or_else(|| self.next_client_nonce());
        req.client_nonce = Some(nonce);
        self.stage_provisional_message(&req, nonce);
        self.outbox.send_or_enqueue_direct(&req, nonce).await
    }

    pub async fn send_encrypted_dm(
        &self,
        chat_id: i64,
        recipient_user_id: u64,
        plaintext: &[u8],
    ) -> std::result::Result<RichSendMessageResponse, HttpClientError> {
        let env = self
            .e2ee
            .encrypt_dm(recipient_user_id, plaintext)
            .await
            .map_err(|e| HttpClientError::Api {
                status: 400,
                message: e.to_string(),
            })?;
        let req = RichSendMessageRequest {
            chat_id,
            text: None,
            content: Some(MessageContent::Encrypted(env)),
            entities: Vec::new(),
            reply_to: None,
            quote_text: None,
            topic_id: None,
            client_nonce: Some(self.next_client_nonce()),
            forward_from_chat_id: None,
            forward_from_msg_id: None,
            silent: false,
            scheduled_at_ms: None,
            inline_keyboard: None,
            send_as_peer_id: None,
        };
        self.send_message_durable(req).await
    }

    pub async fn send_encrypted_group(
        &self,
        chat_id: i64,
        member_user_ids: &[u64],
        plaintext: &[u8],
    ) -> std::result::Result<RichSendMessageResponse, HttpClientError> {
        let (dist_opt, group_env) = self
            .e2ee
            .encrypt_group(chat_id, member_user_ids, plaintext)
            .await
            .map_err(|e| HttpClientError::Api {
                status: 400,
                message: e.to_string(),
            })?;
        if let Some(dist_env) = dist_opt {
            let dist_req = RichSendMessageRequest {
                chat_id,
                text: None,
                content: Some(MessageContent::Encrypted(dist_env)),
                entities: Vec::new(),
                reply_to: None,
                quote_text: None,
                topic_id: None,
                client_nonce: Some(self.next_client_nonce()),
                forward_from_chat_id: None,
                forward_from_msg_id: None,
                silent: true,
                scheduled_at_ms: None,
                inline_keyboard: None,
                send_as_peer_id: None,
            };
            self.send_message_durable(dist_req).await?;
        }
        let msg_req = RichSendMessageRequest {
            chat_id,
            text: None,
            content: Some(MessageContent::Encrypted(group_env)),
            entities: Vec::new(),
            reply_to: None,
            quote_text: None,
            topic_id: None,
            client_nonce: Some(self.next_client_nonce()),
            forward_from_chat_id: None,
            forward_from_msg_id: None,
            silent: false,
            scheduled_at_ms: None,
            inline_keyboard: None,
            send_as_peer_id: None,
        };
        self.send_message_durable(msg_req).await
    }

    pub async fn send_text_message(
        &self,
        chat_id: i64,
        text: impl Into<String>,
        reply_to: Option<i64>,
    ) -> std::result::Result<RichSendMessageResponse, HttpClientError> {
        let text_str = text.into();
        let req = RichSendMessageRequest {
            chat_id,
            text: Some(text_str.clone()),
            content: Some(MessageContent::plain_text(text_str)),
            entities: Vec::new(),
            reply_to,
            quote_text: None,
            topic_id: None,
            client_nonce: Some(self.next_client_nonce()),
            forward_from_chat_id: None,
            forward_from_msg_id: None,
            silent: false,
            scheduled_at_ms: None,
            inline_keyboard: None,
            send_as_peer_id: None,
        };
        self.send_message_durable(req).await
    }

    pub fn send_ws_typing(&self, chat_id: i64, action: TypingAction) -> bool {
        let uid = UserId(self.current_user_id().unwrap_or(0));
        self.send_ws_frame(WsClientFrame::SendTyping(TypingEvent::new(
            ChatId(chat_id),
            uid,
            action,
        )))
    }

    pub fn subscribe_chat(&self, chat_id: i64) -> bool {
        self.send_ws_frame(WsClientFrame::SubscribeChat { chat_id })
    }

    pub fn unsubscribe_chat(&self, chat_id: i64) -> bool {
        self.send_ws_frame(WsClientFrame::UnsubscribeChat { chat_id })
    }

    pub async fn get_chat_messages(
        &self,
        chat_id: u64,
        limit: Option<usize>,
    ) -> std::result::Result<Vec<HistoricalMessage>, HttpClientError> {
        let token = self.require_token()?;
        self.get_chat_messages_with_token(&token, chat_id, limit).await
    }

    pub async fn get_chat_messages_with_token(
        &self,
        token: &str,
        chat_id: u64,
        limit: Option<usize>,
    ) -> std::result::Result<Vec<HistoricalMessage>, HttpClientError> {
        let mut msgs = self.http_client.get_chat_messages(token, chat_id, limit).await?;
        let cid = ChatId(chat_id as i64);
        for hm in &mut msgs {
            let mid = MessageId(hm.message_id as i64);
            if hm.has_authoritative_reactions() {
                let counts: Vec<ReactionCount> = hm
                    .reactions
                    .iter()
                    .map(|r| ReactionCount::new(r.emoji.clone(), r.count, r.chosen))
                    .collect();
                self.local_store.update_reactions(cid, mid, counts);
            } else if let Some(cached) = self.local_store.get_reactions(cid, mid) {
                hm.reactions = cached
                    .into_iter()
                    .map(|r| HistoricalReaction {
                        emoji: r.emoji,
                        count: r.count,
                        chosen: r.is_chosen_by_me,
                        user_ids: Vec::new(),
                    })
                    .collect();
                hm.reactions_authoritative = true;
            }
        }
        Ok(msgs)
    }

    pub async fn edit_message(
        &self,
        chat_id: i64,
        message_id: i64,
        text: impl Into<String>,
        entities: Vec<MessageEntity>,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        let req = EditMessageHttpRequest {
            chat_id: Some(chat_id),
            message_id: Some(message_id),
            text: text.into(),
            entities,
        };
        self.http_client.edit_message(&token, chat_id, message_id, &req).await
    }

    pub async fn delete_message(
        &self,
        chat_id: i64,
        message_id: i64,
        revoke: bool,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.delete_message(&token, chat_id, message_id, revoke).await?;
        self.local_store
            .delete_messages(ChatId(chat_id), &[MessageId(message_id)]);
        Ok(())
    }

    pub async fn react_to_message(
        &self,
        chat_id: i64,
        message_id: i64,
        emoji: impl Into<String>,
        action: Option<&str>,
    ) -> std::result::Result<ReactionHttpResponse, HttpClientError> {
        let token = self.require_token()?;
        let req = ReactionHttpRequest {
            emoji: emoji.into(),
            action: action.map(|s| s.to_string()),
        };
        let mut resp = self
            .http_client
            .react_to_message(&token, chat_id, message_id, &req)
            .await?;
        if resp.has_authoritative_reactions() {
            self.local_store.update_reactions(
                ChatId(chat_id),
                MessageId(message_id),
                resp.reactions.clone(),
            );
        } else {
            let updated = self.local_store.apply_viewer_reaction_mutation(
                ChatId(chat_id),
                MessageId(message_id),
                &req.emoji,
                req.action.as_deref(),
            );
            resp.reactions = updated;
        }
        Ok(resp)
    }

    pub async fn create_poll(
        &self,
        chat_id: i64,
        req: &CreatePollHttpRequest,
    ) -> std::result::Result<PollHttpResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.create_poll(&token, chat_id, req).await
    }

    pub async fn vote_poll(
        &self,
        poll_id: u64,
        option_ids: Vec<u8>,
    ) -> std::result::Result<PollHttpResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.vote_poll(&token, poll_id, option_ids).await
    }

    pub async fn close_poll(&self, poll_id: u64) -> std::result::Result<PollHttpResponse, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.close_poll(&token, poll_id).await
    }

    pub async fn mark_chat_read(
        &self,
        chat_id: i64,
        max_id: i64,
        topic_id: Option<i64>,
    ) -> std::result::Result<ReadChatHttpResponse, HttpClientError> {
        let token = self.require_token()?;
        let req = ReadChatHttpRequest { max_id, topic_id };
        let resp = self.http_client.mark_chat_read(&token, chat_id, &req).await?;
        self.local_store.mark_read_inbox(
            ChatId(chat_id),
            MessageId(resp.read_inbox_max_id),
            Some(resp.unread_count),
        );
        Ok(resp)
    }

    pub async fn sync_chat_difference(
        &self,
        chat_id: ChatId,
        limit: u32,
    ) -> std::result::Result<SyncGetDifferenceResponse, HttpClientError> {
        let token = self.require_token()?;
        let pts = self.sync_coordinator.last_pts(chat_id);
        let req = SyncGetDifferenceRequest::new(chat_id, pts, limit);
        let diff = self.http_client.sync_get_difference(&token, &req).await?;
        let _ = self.process_ws_frame(WsServerFrame::SyncDiff(diff.clone()));
        if !diff.has_more {
            self.clear_gap_chat(chat_id);
        }
        Ok(diff)
    }

    pub async fn sync_all_gaps(
        &self,
        limit: u32,
    ) -> std::result::Result<Vec<SyncGetDifferenceResponse>, HttpClientError> {
        let chats = self.pending_gap_chats();
        let mut results = Vec::with_capacity(chats.len());
        for cid in chats {
            results.push(self.sync_chat_difference(cid, limit).await?);
        }
        Ok(results)
    }
}
