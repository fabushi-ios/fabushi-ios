use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::percent_encode_rfc3986;
use gpui_telegram_proto::contracts::{
    ChatDetailDto, ChatFolderDto, CreateChatRequest, CreatePollHttpRequest, DraftDto,
    EditMessageHttpRequest, InviteLinkDto, PinnedMessageDto, PollHttpResponse, ReactionHttpRequest,
    ReactionHttpResponse, ReadChatHttpRequest, ReadChatHttpResponse,
    SendMessageHttpRequest as ProtoSendMessageHttpRequest,
    SendMessageHttpResponse as ProtoSendMessageHttpResponse, VotePollHttpRequest,
};
use gpui_telegram_proto::sync::{SyncGetDifferenceRequest, SyncGetDifferenceResponse};
use reqwest::Method;
use serde::Serialize;

#[derive(Serialize)]
struct CreateInviteBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expire_date_ms: Option<i64>,
    request_needed: bool,
}

#[derive(Serialize)]
struct JoinInviteBody<'a> {
    code: &'a str,
}

#[derive(Serialize)]
struct BoolFlagBody {
    #[serde(rename = "archived", skip_serializing_if = "Option::is_none")]
    archived: Option<bool>,
    #[serde(rename = "pinned", skip_serializing_if = "Option::is_none")]
    pinned: Option<bool>,
    #[serde(rename = "muted", skip_serializing_if = "Option::is_none")]
    muted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mute_until_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SendAsPeerDto {
    pub peer_id: u64,
    pub title: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Serialize)]
struct PinMessageBody {
    message_id: i64,
}

impl HttpClient {
    pub async fn create_chat(&self, token: &str, req: &CreateChatRequest) -> Result<ChatDetailDto, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/chats/create", Some(token), Some(req)).await
    }

    pub async fn provision_welcome_chat(&self, token: &str) -> Result<ChatDetailDto, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/chats/welcome", Some(token), None::<&()>).await
    }

    pub async fn delete_chat(&self, token: &str, chat_id: i64) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}", chat_id);
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>).await
    }

    pub async fn clear_chat_history(&self, token: &str, chat_id: i64) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}/clear", chat_id);
        self.request_empty(Method::POST, &path, Some(token), Some(&serde_json::json!({}))).await
    }

    pub async fn leave_or_delete_chat(&self, token: &str, chat_id: i64) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}/leave", chat_id);
        self.request_empty(Method::POST, &path, Some(token), Some(&serde_json::json!({}))).await
    }

    pub async fn set_chat_ttl(&self, token: &str, chat_id: i64, ttl_seconds: u32) -> Result<u32, HttpClientError> {
        let path = format!("/api/v1/chats/{}/ttl", chat_id);
        let body = serde_json::json!({ "ttl_seconds": ttl_seconds });
        let resp: serde_json::Value = self.request_json(Method::POST, &path, Some(token), Some(&body)).await?;
        Ok(resp
            .get("ttl_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(ttl_seconds as u64) as u32)
    }

    pub async fn list_invites(&self, token: &str, chat_id: i64) -> Result<Vec<InviteLinkDto>, HttpClientError> {
        let path = format!("/api/v1/chats/{}/invites", chat_id);
        self.request_vec_or_field(Method::GET, &path, Some(token), &["invites", "items"]).await
    }

    pub async fn create_invite(
        &self,
        token: &str,
        chat_id: i64,
        title: Option<&str>,
        usage_limit: Option<u32>,
        expire_date_ms: Option<i64>,
        request_needed: bool,
    ) -> Result<InviteLinkDto, HttpClientError> {
        let path = format!("/api/v1/chats/{}/invites", chat_id);
        let body = CreateInviteBody { title, usage_limit, expire_date_ms, request_needed };
        self.request_json(Method::POST, &path, Some(token), Some(&body)).await
    }

    pub async fn revoke_invite(&self, token: &str, chat_id: i64, invite_id: &str) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}/invites/{}", chat_id, percent_encode_rfc3986(invite_id));
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>).await
    }

    pub async fn join_invite(&self, token: &str, code: &str) -> Result<ChatDetailDto, HttpClientError> {
        let body = JoinInviteBody { code };
        self.request_json(Method::POST, "/api/v1/invites/join", Some(token), Some(&body)).await
    }

    pub async fn list_folders(&self, token: &str) -> Result<Vec<ChatFolderDto>, HttpClientError> {
        self.request_vec_or_field(Method::GET, "/api/v1/folders", Some(token), &["folders", "items"]).await
    }

    pub async fn save_folder(&self, token: &str, folder: &ChatFolderDto) -> Result<ChatFolderDto, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/folders", Some(token), Some(folder)).await
    }

    pub async fn update_folder(&self, token: &str, folder_id: i64, folder: &ChatFolderDto) -> Result<ChatFolderDto, HttpClientError> {
        let path = format!("/api/v1/folders/{}", folder_id);
        self.request_json(Method::PUT, &path, Some(token), Some(folder)).await
    }

    pub async fn delete_folder(&self, token: &str, folder_id: i64) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/folders/{}", folder_id);
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>).await
    }

    pub async fn set_chat_archived(&self, token: &str, chat_id: i64, archived: bool) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}/archive", chat_id);
        let body = BoolFlagBody { archived: Some(archived), pinned: None, muted: None, mute_until_ms: None };
        self.request_empty(Method::POST, &path, Some(token), Some(&body)).await
    }

    pub async fn set_chat_pinned(&self, token: &str, chat_id: i64, pinned: bool) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}/pin", chat_id);
        let body = BoolFlagBody { archived: None, pinned: Some(pinned), muted: None, mute_until_ms: None };
        self.request_empty(Method::POST, &path, Some(token), Some(&body)).await
    }

    pub async fn set_chat_muted(&self, token: &str, chat_id: i64, muted: bool, mute_until_ms: Option<i64>) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}/mute", chat_id);
        let body = BoolFlagBody { archived: None, pinned: None, muted: Some(muted), mute_until_ms };
        self.request_empty(Method::POST, &path, Some(token), Some(&body)).await
    }

    pub async fn list_pinned_messages(&self, token: &str, chat_id: i64) -> Result<Vec<PinnedMessageDto>, HttpClientError> {
        let path = format!("/api/v1/chats/{}/pins", chat_id);
        self.request_vec_or_field(Method::GET, &path, Some(token), &["pins", "pinned_messages", "items"]).await
    }

    pub async fn pin_message(&self, token: &str, chat_id: i64, message_id: i64) -> Result<PinnedMessageDto, HttpClientError> {
        let path = format!("/api/v1/chats/{}/pins", chat_id);
        let body = PinMessageBody { message_id };
        self.request_json(Method::POST, &path, Some(token), Some(&body)).await
    }

    pub async fn unpin_message(&self, token: &str, chat_id: i64, message_id: i64) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}/pins/{}", chat_id, message_id);
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>).await
    }

    pub async fn get_draft(&self, token: &str, chat_id: i64) -> Result<Option<DraftDto>, HttpClientError> {
        let path = format!("/api/v1/chats/{}/draft", chat_id);
        let val: serde_json::Value = self.request_json(Method::GET, &path, Some(token), None::<&()>).await?;
        if val.is_null() {
            return Ok(None);
        }
        if let Some(inner) = val.get("draft") {
            return Ok(serde_json::from_value(inner.clone())?);
        }
        Ok(Some(serde_json::from_value(val)?))
    }

    pub async fn save_draft(&self, token: &str, chat_id: i64, draft: &DraftDto) -> Result<DraftDto, HttpClientError> {
        let path = format!("/api/v1/chats/{}/draft", chat_id);
        let val: serde_json::Value = self.request_json(Method::PUT, &path, Some(token), Some(draft)).await?;
        if let Some(inner) = val.get("draft") {
            return Ok(serde_json::from_value(inner.clone())?);
        }
        Ok(serde_json::from_value(val)?)
    }

    pub async fn get_send_as_peers(&self, token: &str, chat_id: i64) -> Result<Vec<SendAsPeerDto>, HttpClientError> {
        let path = format!("/api/v1/chats/{}/send_as", chat_id);
        self.request_vec_or_field(Method::GET, &path, Some(token), &["peers", "items"]).await
    }

    pub async fn clear_draft(&self, token: &str, chat_id: i64) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}/draft", chat_id);
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>).await
    }

    pub async fn send_message_rich(&self, token: &str, req: &ProtoSendMessageHttpRequest) -> Result<ProtoSendMessageHttpResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/messages/send", Some(token), Some(req)).await
    }

    pub async fn edit_message(&self, token: &str, chat_id: i64, message_id: i64, req: &EditMessageHttpRequest) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}/messages/{}", chat_id, message_id);
        self.request_empty(Method::PUT, &path, Some(token), Some(req)).await
    }

    pub async fn delete_message(&self, token: &str, chat_id: i64, message_id: i64, revoke: bool) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/chats/{}/messages/{}?revoke={}", chat_id, message_id, revoke);
        self.request_empty(Method::DELETE, &path, Some(token), None::<&()>).await
    }

    pub async fn react_to_message(&self, token: &str, chat_id: i64, message_id: i64, req: &ReactionHttpRequest) -> Result<ReactionHttpResponse, HttpClientError> {
        let path = format!("/api/v1/chats/{}/messages/{}/reactions", chat_id, message_id);
        match self.request_json(Method::POST, &path, Some(token), Some(req)).await {
            Err(HttpClientError::Api { status, .. }) if status == 404 || status == 405 => {
                #[derive(Serialize)]
                struct LegacyReactionBody<'a> {
                    chat_id: i64,
                    message_id: i64,
                    emoji: &'a str,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    action: Option<&'a str>,
                }
                let legacy = LegacyReactionBody {
                    chat_id,
                    message_id,
                    emoji: &req.emoji,
                    action: req.action.as_deref(),
                };
                self.request_json(Method::POST, "/api/v1/messages/reaction", Some(token), Some(&legacy)).await
            }
            other => other,
        }
    }

    pub async fn create_poll(&self, token: &str, chat_id: i64, req: &CreatePollHttpRequest) -> Result<PollHttpResponse, HttpClientError> {
        let path = format!("/api/v1/chats/{}/polls", chat_id);
        self.request_json(Method::POST, &path, Some(token), Some(req)).await
    }

    pub async fn vote_poll(&self, token: &str, poll_id: u64, option_ids: Vec<u8>) -> Result<PollHttpResponse, HttpClientError> {
        let path = format!("/api/v1/polls/{}/vote", poll_id);
        let body = VotePollHttpRequest { option_ids };
        self.request_json(Method::POST, &path, Some(token), Some(&body)).await
    }

    pub async fn close_poll(&self, token: &str, poll_id: u64) -> Result<PollHttpResponse, HttpClientError> {
        let path = format!("/api/v1/polls/{}/close", poll_id);
        self.request_json(Method::POST, &path, Some(token), None::<&()>).await
    }

    pub async fn mark_chat_read(&self, token: &str, chat_id: i64, req: &ReadChatHttpRequest) -> Result<ReadChatHttpResponse, HttpClientError> {
        let path = format!("/api/v1/chats/{}/read", chat_id);
        self.request_json(Method::POST, &path, Some(token), Some(req)).await
    }

    pub async fn sync_get_difference(&self, token: &str, req: &SyncGetDifferenceRequest) -> Result<SyncGetDifferenceResponse, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/sync/get_difference", Some(token), Some(req)).await
    }
}
