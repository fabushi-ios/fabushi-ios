use crate::client::TelegramClient;
use crate::net::{
    ChatDetailDto, ChatDetails, ChatFolderDto, ChatMemberItem, ChatSummary, CreateChatRequest,
    DraftDto, ForumTopicItem, HttpClientError, InviteLinkDto, PinnedMessageDto,
};
use gpui_telegram_core::types::{ChatId, MessageId};

impl TelegramClient {
    pub async fn get_chats(&self) -> std::result::Result<Vec<ChatSummary>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_chats(&token).await
    }

    pub async fn create_chat(
        &self,
        req: &CreateChatRequest,
    ) -> std::result::Result<ChatDetailDto, HttpClientError> {
        let token = self.require_token()?;
        let chat = self.http_client.create_chat(&token, req).await?;
        self.local_store.upsert_chat(chat.clone());
        Ok(chat)
    }

    pub async fn provision_welcome_chat(&self) -> std::result::Result<ChatDetailDto, HttpClientError> {
        let token = self.require_token()?;
        let chat = self.http_client.provision_welcome_chat(&token).await?;
        self.local_store.upsert_chat(chat.clone());
        Ok(chat)
    }

    pub async fn get_chat(&self, chat_id: i64) -> std::result::Result<ChatDetails, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_chat(&token, chat_id).await
    }

    pub async fn update_chat_info(
        &self,
        chat_id: i64,
        title: Option<&str>,
        about: Option<&str>,
    ) -> std::result::Result<bool, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.update_chat_info(&token, chat_id, title, about).await
    }

    pub async fn delete_chat(&self, chat_id: i64) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.delete_chat(&token, chat_id).await?;
        self.local_store.remove_chat(ChatId(chat_id));
        Ok(())
    }

    pub async fn clear_chat_history(&self, chat_id: i64) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.clear_chat_history(&token, chat_id).await?;
        self.local_store.clear_chat_messages(ChatId(chat_id));
        Ok(())
    }

    pub async fn leave_or_delete_chat(&self, chat_id: i64) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.leave_or_delete_chat(&token, chat_id).await?;
        self.local_store.remove_chat(ChatId(chat_id));
        Ok(())
    }

    pub async fn set_chat_ttl(
        &self,
        chat_id: i64,
        ttl_seconds: u32,
    ) -> std::result::Result<u32, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.set_chat_ttl(&token, chat_id, ttl_seconds).await
    }

    pub async fn get_chat_members(
        &self,
        chat_id: i64,
    ) -> std::result::Result<Vec<ChatMemberItem>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.get_chat_members(&token, chat_id).await
    }

    pub async fn add_chat_members(
        &self,
        chat_id: i64,
        user_ids: &[u64],
    ) -> std::result::Result<bool, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.add_chat_members(&token, chat_id, user_ids).await
    }

    pub async fn update_chat_member_role(
        &self,
        chat_id: i64,
        user_id: u64,
        role: &str,
        admin_title: Option<&str>,
        admin_rights: Option<u64>,
    ) -> std::result::Result<bool, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .update_chat_member_role(&token, chat_id, user_id, role, admin_title, admin_rights)
            .await
    }

    pub async fn remove_chat_member(
        &self,
        chat_id: i64,
        user_id: u64,
    ) -> std::result::Result<bool, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.remove_chat_member(&token, chat_id, user_id).await
    }

    pub async fn list_invites(
        &self,
        chat_id: i64,
    ) -> std::result::Result<Vec<InviteLinkDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_invites(&token, chat_id).await
    }

    pub async fn create_invite(
        &self,
        chat_id: i64,
        title: Option<&str>,
        usage_limit: Option<u32>,
        expire_date_ms: Option<i64>,
        request_needed: bool,
    ) -> std::result::Result<InviteLinkDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .create_invite(&token, chat_id, title, usage_limit, expire_date_ms, request_needed)
            .await
    }

    pub async fn revoke_invite(
        &self,
        chat_id: i64,
        invite_id: &str,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.revoke_invite(&token, chat_id, invite_id).await
    }

    pub async fn join_invite(&self, code: &str) -> std::result::Result<ChatDetailDto, HttpClientError> {
        let token = self.require_token()?;
        let chat = self.http_client.join_invite(&token, code).await?;
        self.local_store.upsert_chat(chat.clone());
        Ok(chat)
    }

    pub async fn list_forum_topics(
        &self,
        chat_id: i64,
    ) -> std::result::Result<Vec<ForumTopicItem>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_forum_topics(&token, chat_id).await
    }

    pub async fn create_forum_topic(
        &self,
        chat_id: i64,
        title: &str,
        icon_color: i32,
        icon_emoji_id: Option<i64>,
    ) -> std::result::Result<ForumTopicItem, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .create_forum_topic(&token, chat_id, title, icon_color, icon_emoji_id)
            .await
    }

    pub async fn update_forum_topic(
        &self,
        chat_id: i64,
        topic_id: i64,
        title: Option<&str>,
        is_pinned: Option<bool>,
        is_closed: Option<bool>,
    ) -> std::result::Result<bool, HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .update_forum_topic(&token, chat_id, topic_id, title, is_pinned, is_closed)
            .await
    }

    pub async fn delete_forum_topic(
        &self,
        chat_id: i64,
        topic_id: i64,
    ) -> std::result::Result<bool, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.delete_forum_topic(&token, chat_id, topic_id).await
    }

    pub async fn list_folders(&self) -> std::result::Result<Vec<ChatFolderDto>, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.list_folders(&token).await
    }

    pub async fn save_folder(
        &self,
        folder: &ChatFolderDto,
    ) -> std::result::Result<ChatFolderDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.save_folder(&token, folder).await
    }

    pub async fn update_folder(
        &self,
        folder_id: i64,
        folder: &ChatFolderDto,
    ) -> std::result::Result<ChatFolderDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.update_folder(&token, folder_id, folder).await
    }

    pub async fn delete_folder(&self, folder_id: i64) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.delete_folder(&token, folder_id).await
    }

    pub async fn set_chat_archived(
        &self,
        chat_id: i64,
        archived: bool,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.set_chat_archived(&token, chat_id, archived).await
    }

    pub async fn set_chat_pinned(
        &self,
        chat_id: i64,
        pinned: bool,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.set_chat_pinned(&token, chat_id, pinned).await
    }

    pub async fn set_chat_muted(
        &self,
        chat_id: i64,
        muted: bool,
        mute_until_ms: Option<i64>,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client
            .set_chat_muted(&token, chat_id, muted, mute_until_ms)
            .await
    }

    pub async fn list_pinned_messages(
        &self,
        chat_id: i64,
    ) -> std::result::Result<Vec<PinnedMessageDto>, HttpClientError> {
        let token = self.require_token()?;
        let pins = self.http_client.list_pinned_messages(&token, chat_id).await?;
        self.local_store.set_pinned_messages(
            ChatId(chat_id),
            pins.iter().map(|p| MessageId(p.message_id)).collect(),
        );
        Ok(pins)
    }

    pub async fn pin_message(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> std::result::Result<PinnedMessageDto, HttpClientError> {
        let token = self.require_token()?;
        self.http_client.pin_message(&token, chat_id, message_id).await
    }

    pub async fn unpin_message(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.unpin_message(&token, chat_id, message_id).await
    }

    pub async fn get_draft(
        &self,
        chat_id: i64,
    ) -> std::result::Result<Option<DraftDto>, HttpClientError> {
        let token = self.require_token()?;
        let draft = self.http_client.get_draft(&token, chat_id).await?;
        self.local_store.set_draft(ChatId(chat_id), draft.clone());
        Ok(draft)
    }

    pub async fn save_draft(
        &self,
        chat_id: i64,
        draft: &DraftDto,
    ) -> std::result::Result<DraftDto, HttpClientError> {
        let token = self.require_token()?;
        let saved = self.http_client.save_draft(&token, chat_id, draft).await?;
        self.local_store.set_draft(ChatId(chat_id), Some(saved.clone()));
        Ok(saved)
    }

    pub async fn clear_draft(&self, chat_id: i64) -> std::result::Result<(), HttpClientError> {
        let token = self.require_token()?;
        self.http_client.clear_draft(&token, chat_id).await?;
        self.local_store.set_draft(ChatId(chat_id), None);
        Ok(())
    }
}
