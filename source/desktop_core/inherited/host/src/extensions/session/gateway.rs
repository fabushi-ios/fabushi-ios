use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde_json::{Map, Value, json};

use crate::agents::agent_profile::SandAgentProfile;
use crate::extensions::transcript::group_chat_glue::{
    GroupChatGlue, GroupChatGlueError,
};
use crate::selected_image_inputs::read_image_file_dimensions;
use crate::extensions::transcript::send_acceptance::{
    mark_accepted_send_activity, prepare_send_acceptance,
};
use crate::extensions::transcript::send_message_shaping::{
    UserAttachmentOptions, UserMessageOptions, create_user_attachment_entry,
    create_user_message, stat_attached_file_size,
};
use crate::extensions::transcript::send_thread_stamping::resolve_send_reply_threading;
use crate::extensions::transcript::roster_search::{
    AGENT_CONTENT_SEARCH_MAX_RESULTS, RosterContentSearch, search_agents,
    search_human_conversation_messages, search_media,
};
use crate::extensions::transcript::transcript_entry_ids::{
    TranscriptEntryIdKind, next_entry_id,
};
use uuid::Uuid;

use super::agent_db_transcript_pages::{HumanTranscriptPageQuery, TranscriptPageQuery, TranscriptWindowQuery};
use super::agent_session::SandAgentSessionStore;
use super::production::{HumanForwardDestination, MiniAppPostError, ProductionSessionWorkers};
use super::session_profile_files::AgentProfileUpdate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionGatewayError {
    BadRequest(String),
    Internal(String),
}

impl SessionGatewayError {
    fn bad(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl std::fmt::Display for SessionGatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(msg) => write!(f, "{msg}"),
            Self::Internal(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for SessionGatewayError {}
fn recent_recovery_user_messages(
    entries: &[Value],
) -> Vec<crate::runner::RecoveryUserMessage> {
    entries
        .iter()
        .filter(|entry| {
            entry.get("kind").and_then(Value::as_str) == Some("message")
                && entry.get("role").and_then(Value::as_str) == Some("user")
                && entry.get("fromAgent").is_none_or(Value::is_null)
                && entry.get("channel").is_none_or(Value::is_null)
        })
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_str)?.trim();
            if id.is_empty() {
                return None;
            }
            Some(crate::runner::RecoveryUserMessage {
                id: id.to_string(),
                text: entry
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                confirmed: entry.get("confirmed").and_then(Value::as_bool),
            })
        })
        .collect()
}

pub fn persist_accepted_send_prompt(
    session: &Arc<ProductionSessionWorkers>,
    args: &Value,
    accepted: &Value,
) -> Result<Option<String>, SessionGatewayError> {
    Ok(
        persist_accepted_send_prompt_context(session, args, accepted)?
            .echo_entry_id
    )
}

pub fn persist_accepted_send_prompt_context(
    session: &Arc<ProductionSessionWorkers>,
    args: &Value,
    accepted: &Value,
) -> Result<crate::extensions::transcript::send_pipeline::PersistedSendContext, SessionGatewayError> {
    use crate::extensions::transcript::send_pipeline::{
        PersistedAcceptedEcho, PersistedSendContext,
    };

    if accepted.get("accepted").and_then(Value::as_bool) != Some(true) {
        return Ok(PersistedSendContext::default());
    }
    let agent_id = args
        .get("agentId")
        .or_else(|| args.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| SessionGatewayError::bad("sendPrompt requires agentId"))?;
    let requested_session_key = args
        .get("sessionKey")
        .or_else(|| args.get("sessionId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let session_key = match requested_session_key {
        Some(session_key) => Some(session_key.to_string()),
        None => session
            .read_session_work_admission(agent_id)
            .map_err(SessionGatewayError::internal)?
            .map(|record| record.session_key),
    };
    let prompt = args
        .get("prompt")
        .or_else(|| args.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let attachment_paths = args
        .get("attachmentPaths")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default();
    if prompt.is_empty() && attachment_paths.is_empty() {
        return Err(SessionGatewayError::bad("sendPrompt requires prompt or attachments"));
    }

    let attachment_names = args.get("attachmentNames").and_then(Value::as_array).cloned().unwrap_or_default();
    let client_nonce = optional_string(args, "clientNonce")?
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let mut existing = session
        .read_agent_transcript_entries(agent_id)
        .map_err(SessionGatewayError::internal)?;

    if let Some(client_nonce) = client_nonce.as_deref() {
        let existing_user_message_id = existing
            .iter()
            .find(|entry| {
                entry.get("clientNonce").and_then(Value::as_str) == Some(client_nonce)
                    && entry.get("kind").and_then(Value::as_str) == Some("message")
                    && entry.get("role").and_then(Value::as_str) == Some("user")
            })
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let existing_echo_id = existing_user_message_id.clone().or_else(|| {
            existing
                .iter()
                .find(|entry| {
                    entry.get("clientNonce").and_then(Value::as_str) == Some(client_nonce)
                })
                .and_then(|entry| entry.get("id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        });
        if existing_echo_id.is_some() {
            return Ok(PersistedSendContext {
                echo_entry_id: existing_echo_id,
                user_message_id: existing_user_message_id,
                recent_user_messages: recent_recovery_user_messages(&existing),
                ..PersistedSendContext::default()
            });
        }
    }

    let preparation = prepare_send_acceptance(session, agent_id, existing.len(), &prompt)
        .map_err(SessionGatewayError::internal)?;
    let needs_roster_refresh =
        preparation.seeded_name.is_some() || preparation.awaiting_cleared;

    let threading = resolve_send_reply_threading(
        &existing,
        optional_string(args, "replyToId")?,
        optional_bool(args, "isFork")?.unwrap_or(false),
    );
    let batch_id = (!attachment_paths.is_empty()).then(|| Uuid::new_v4().to_string());
    let mut staged = Vec::new();
    let mut first_echo_id = None;

    for (index, file_path) in attachment_paths.iter().enumerate() {
        let id = next_entry_id(&existing, TranscriptEntryIdKind::UserAttachment);
        let file_name = attachment_names
            .get(index)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let mut entry = create_user_attachment_entry(
            id.clone(),
            file_path.clone(),
            UserAttachmentOptions {
                file_name,
                batch_id: batch_id.clone(),
                client_nonce: client_nonce.clone(),
                byte_size: stat_attached_file_size(file_path),
                width: read_image_file_dimensions(file_path).map(|value| value.width),
                height: read_image_file_dimensions(file_path).map(|value| value.height),
                reply_to: threading.reply_to_id.clone(),
                branched: threading.is_fork,
            },
        );
        if let Some(session_key) = session_key.as_deref() {
            entry["sessionKey"] = Value::String(session_key.to_string());
        }
        first_echo_id.get_or_insert_with(|| id.clone());
        existing.push(entry.clone());
        staged.push(entry);
    }

    let append_user_message = optional_bool(args, "appendUserMessage")?.unwrap_or(true);
    let mut user_message_id = None;
    if append_user_message && !prompt.is_empty() {
        let id = next_entry_id(&existing, TranscriptEntryIdKind::UserMessage);
        let mut entry = create_user_message(
            id.clone(),
            prompt,
            UserMessageOptions {
                composed_at_ms: optional_f64(args, "composedAtMs"),
                rich_text: optional_string(args, "richText")?.map(ToOwned::to_owned),
                reply_to: threading.reply_to_id.clone(),
                batch_id: batch_id.clone(),
                branched: threading.is_fork,
                client_nonce: client_nonce.clone(),
            },
            system_now_ms(),
        );
        if let Some(session_key) = session_key.as_deref() {
            entry["sessionKey"] = Value::String(session_key.to_string());
        }
        user_message_id = Some(id.clone());
        first_echo_id.get_or_insert_with(|| id.clone());
        existing.push(entry.clone());
        staged.push(entry);
    }

    if !staged.is_empty() {
        if let Some(session_key) = session_key.as_deref() {
            session
                .append_agent_transcript_entries_if_session_admitted(
                    agent_id,
                    session_key,
                    &staged,
                )
                .map_err(SessionGatewayError::internal)?;
        } else {
            session
                .append_agent_transcript_entries(agent_id, &staged)
                .map_err(SessionGatewayError::internal)?;
        }
        mark_accepted_send_activity(session, agent_id, system_now_ms())
            .map_err(SessionGatewayError::internal)?;
    }
    Ok(PersistedSendContext {
        echo_entry_id: user_message_id.clone().or(first_echo_id),
        user_message_id,
        recent_user_messages: recent_recovery_user_messages(&existing),
        accepted_echoes: staged
            .into_iter()
            .map(|entry| PersistedAcceptedEcho {
                entry,
                is_on_active_transcript: false,
            })
            .collect(),
        needs_roster_refresh,
        acceptance_effects_applied: true,
        accepted_durably: true,
    })
}

pub fn dispatch_production_session_gateway_call(
    session: &Arc<ProductionSessionWorkers>,
    method: &str,
    args: &Value,
) -> Option<Result<Value, SessionGatewayError>> {
    dispatch_production_session_gateway_call_with_content_search(session, None, method, args)
}

pub fn dispatch_production_session_gateway_call_with_content_search(
    session: &Arc<ProductionSessionWorkers>,
    content_search: Option<&dyn RosterContentSearch>,
    method: &str,
    args: &Value,
) -> Option<Result<Value, SessionGatewayError>> {
    let group_chat = GroupChatGlue::new(Arc::clone(session));
    dispatch_production_session_gateway_call_with_content_search_and_group_chat(
        session,
        content_search,
        &group_chat,
        method,
        args,
    )
}

pub fn dispatch_production_session_gateway_call_with_content_search_and_group_chat(
    session: &Arc<ProductionSessionWorkers>,
    content_search: Option<&dyn RosterContentSearch>,
    group_chat: &GroupChatGlue,
    method: &str,
    args: &Value,
) -> Option<Result<Value, SessionGatewayError>> {
    let store = SandAgentSessionStore::new(Arc::clone(session));
    let result = match method {
        "countAgents" => session
            .count_owned_agents()
            .map(|count| json!(count))
            .map_err(SessionGatewayError::internal),
        "listAgents" => store
            .list_agents()
            .and_then(|agents| serde_json::to_value(agents).map_err(|error| error.to_string()))
            .map_err(SessionGatewayError::internal),
        "searchAgents" => optional_string(args, "query").and_then(|query| {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or_else(|| {
                    content_search
                        .map(RosterContentSearch::max_results)
                        .unwrap_or(AGENT_CONTENT_SEARCH_MAX_RESULTS)
                });
            search_agents(session, content_search, query.unwrap_or_default(), limit)
                .and_then(|matches| serde_json::to_value(matches).map_err(|error| error.to_string()))
                .map_err(SessionGatewayError::internal)
        }),
        "searchMedia" => optional_string(args, "query").and_then(|query| {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or_else(|| {
                    content_search
                        .map(RosterContentSearch::max_results)
                        .unwrap_or(AGENT_CONTENT_SEARCH_MAX_RESULTS)
                });
            search_media(session, content_search, query.unwrap_or_default(), limit)
                .and_then(|matches| serde_json::to_value(matches).map_err(|error| error.to_string()))
                .map_err(SessionGatewayError::internal)
        }),
        "listHumanConversations" => session
            .list_human_conversations()
            .map(Value::Array)
            .map_err(SessionGatewayError::internal),
        "syncHumanConversations" => session
            .sync_human_conversations()
            .map(Value::Array)
            .map_err(SessionGatewayError::internal),
        "syncHumanConversation" => required_string(args, "conversationId").and_then(|conversation_id| {
            session
                .sync_human_conversation(conversation_id)
                .map(Value::Array)
                .map_err(SessionGatewayError::internal)
        }),
        "createHumanConversation" => required_string(args, "peerHumanId").and_then(|peer_human_id| {
            let local_human_id = session.local_human_id().map_err(SessionGatewayError::internal)?;
            if let Some(requested) = optional_string(args, "localHumanId")? {
                if requested != local_human_id {
                    return Err(SessionGatewayError::bad("localHumanId does not match the authenticated Human identity"));
                }
            }
            let title = optional_string(args, "title")?.unwrap_or(peer_human_id);
            session
                .create_human_conversation(peer_human_id, title)
                .map_err(SessionGatewayError::internal)
        }),
        "createGroupConversation" => (|| {
            let group_id = optional_string(args, "groupId")?
                .or(optional_string(args, "conversationId")?)
                .ok_or_else(|| SessionGatewayError::bad("createGroupConversation requires groupId"))?;
            let title = optional_string(args, "title")?.unwrap_or(group_id);
            let participant_ids = optional_string_array(args, "participantIds")?.unwrap_or_default();
            let slow_mode_seconds = args.get("slowModeSeconds").and_then(Value::as_u64).map(|v| v as u32);
            session
                .create_group_conversation(group_id, title, &participant_ids, slow_mode_seconds)
                .map_err(SessionGatewayError::internal)
        })(),
        "createChannelConversation" => (|| {
            let channel_id = optional_string(args, "channelId")?
                .or(optional_string(args, "conversationId")?)
                .ok_or_else(|| SessionGatewayError::bad("createChannelConversation requires channelId"))?;
            let title = optional_string(args, "title")?.unwrap_or(channel_id);
            let participant_ids = optional_string_array(args, "participantIds")?.unwrap_or_default();
            session
                .create_channel_conversation(channel_id, title, &participant_ids)
                .map_err(SessionGatewayError::internal)
        })(),
        "upsertConversationTopic" => (|| {
            let conversation_id = required_string(args, "conversationId")?;
            let topic_id = required_string(args, "topicId")?;
            let title = required_string(args, "title")?;
            let icon = optional_string(args, "icon")?;
            let closed = optional_bool(args, "closed")?.unwrap_or(false);
            let pinned = optional_bool(args, "pinned")?.unwrap_or(false);
            let hidden = optional_bool(args, "hidden")?.unwrap_or(false);
            session
                .upsert_conversation_topic(conversation_id, topic_id, title, icon, closed, pinned, hidden)
                .map_err(SessionGatewayError::internal)
        })(),
        "listConversationTopics" => required_string(args, "conversationId").and_then(|conversation_id| {
            session
                .list_conversation_topics(conversation_id)
                .map(Value::Array)
                .map_err(SessionGatewayError::internal)
        }),
        "upsertConversationMember" => (|| {
            let conversation_id = required_string(args, "conversationId")?;
            let member_value = args.get("member").unwrap_or(args);
            let member: fabushi_messaging_core::CommunityMember = serde_json::from_value(member_value.clone())
                .map_err(|e| SessionGatewayError::bad(format!("invalid member payload: {e}")))?;
            session
                .upsert_conversation_member(conversation_id, member)
                .map_err(SessionGatewayError::internal)
        })(),
        "pinConversationMessage" => (|| {
            let conversation_id = required_string(args, "conversationId")?;
            let message_id = required_string(args, "messageId")?;
            let topic_id = optional_string(args, "topicId")?;
            session
                .pin_conversation_message(conversation_id, message_id, topic_id)
                .map_err(SessionGatewayError::internal)
        })(),
        "unpinConversationMessage" => (|| {
            let conversation_id = required_string(args, "conversationId")?;
            let message_id = required_string(args, "messageId")?;
            let topic_id = optional_string(args, "topicId")?;
            session
                .unpin_conversation_message(conversation_id, message_id, topic_id)
                .map_err(SessionGatewayError::internal)
        })(),
        "listConversationPins" => (|| {
            let conversation_id = required_string(args, "conversationId")?;
            let topic_id = optional_string(args, "topicId")?;
            session
                .list_conversation_pins(conversation_id, topic_id)
                .map(|pins| Value::Array(pins.into_iter().map(Value::String).collect()))
                .map_err(SessionGatewayError::internal)
        })(),
        "setConversationTopicMute" => (|| {
            let conversation_id = required_string(args, "conversationId")?;
            let topic_id = optional_string(args, "topicId")?;
            let muted_until_ms = match args.get("mutedUntilMs") {
                None | Some(Value::Null) => None,
                _ => optional_i64(args, "mutedUntilMs"),
            };
            session
                .set_conversation_topic_mute(conversation_id, topic_id, muted_until_ms)
                .map_err(SessionGatewayError::internal)
        })(),
        "evaluateMessageNotification" => (|| {
            let conversation_id = required_string(args, "conversationId")?;
            let message_id = required_string(args, "messageId")?;
            session
                .evaluate_message_notification(conversation_id, message_id)
                .map_err(SessionGatewayError::internal)
        })(),
        "publishConversationTyping" => (|| {
            let conversation_id = required_string(args, "conversationId")?;
            let topic_id = optional_string(args, "topicId")?;
            let action = optional_string(args, "action")?;
            session
                .publish_conversation_typing(conversation_id, topic_id, action)
                .map_err(SessionGatewayError::internal)
        })(),
        "validateMediaAttachmentPreflight" => (|| {
            let file_name = optional_string(args, "fileName")?.unwrap_or_default();
            let mime_type = optional_string(args, "mimeType")?.unwrap_or_default();
            let size_bytes = args
                .get("sizeBytes")
                .and_then(Value::as_u64)
                .ok_or_else(|| SessionGatewayError::bad("validateMediaAttachmentPreflight requires numeric sizeBytes"))?;
            let width = args.get("width").and_then(Value::as_u64).map(|v| v as u32);
            let height = args.get("height").and_then(Value::as_u64).map(|v| v as u32);
            session
                .validate_media_attachment_preflight(file_name, mime_type, size_bytes, width, height)
                .map_err(SessionGatewayError::internal)
        })(),
        "searchHumanRecipients" => required_string(args, "sourceConversationId").and_then(|source_conversation_id| {
            required_string(args, "sourceEntryId").and_then(|source_entry_id| {
                let query = optional_string(args, "query")?.unwrap_or_default();
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(50);
                session
                    .search_human_recipients(source_conversation_id, source_entry_id, query, limit)
                    .map(Value::Array)
                    .map_err(SessionGatewayError::internal)
            })
        }),
        "composeShareComment" => (|| {
            let link = match optional_string(args, "link")? {
                Some(l) => l,
                None => match optional_string(args, "shareUrl")? {
                    Some(l) => l,
                    None => return Err(SessionGatewayError::bad("composeShareComment requires link")),
                },
            };
            let link = link.trim();
            if link.is_empty() {
                return Err(SessionGatewayError::bad("link must not be empty"));
            }
            if link.chars().any(|c| c == '\r' || c == '\n' || c.is_control()) {
                return Err(SessionGatewayError::bad("link must not contain newlines or control characters"));
            }
            let comment = match args.get("comment") {
                None | Some(Value::Null) => fabushi_messaging_core::FormattedText::plain(""),
                Some(Value::String(text)) => {
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        fabushi_messaging_core::FormattedText::plain("")
                    } else {
                        fabushi_messaging_core::FormattedText::plain(text)
                    }
                }
                Some(val @ Value::Object(_)) => {
                    serde_json::from_value(val.clone())
                        .map_err(|err| SessionGatewayError::bad(format!("invalid comment: {err}")))?
                }
                Some(_) => return Err(SessionGatewayError::bad("invalid comment: expected object or string")),
            };
            let composed = session
                .compose_share_comment(link, comment)
                .map_err(|err| SessionGatewayError::bad(format!("composeShareComment failed: {err:?}")))?;
            serde_json::to_value(&composed).map_err(|e| SessionGatewayError::internal(e.to_string()))
        })(),
        "forwardHumanMessage" => required_string(args, "sourceConversationId").and_then(|source_conversation_id| {
            required_string(args, "sourceEntryId").and_then(|source_entry_id| {
                let destination_conversation_ids = required_string_array(args, "destinationConversationIds")?;
                let destination_topics: Option<std::collections::HashMap<String, String>> = match args.get("destinationTopics") {
                    None | Some(Value::Null) => None,
                    Some(Value::Object(map)) => {
                        let mut parsed = std::collections::HashMap::new();
                        for (k, v) in map {
                            if let Some(s) = v.as_str() {
                                let trimmed = s.trim();
                                if !trimmed.is_empty() {
                                    parsed.insert(k.clone(), trimmed.to_string());
                                }
                            }
                        }
                        Some(parsed)
                    }
                    Some(_) => return Err(SessionGatewayError::bad("invalid destinationTopics: expected object")),
                };
                let share_url = match optional_string(args, "shareUrl")? {
                    Some(s) => {
                        let trimmed = s.trim();
                        if trimmed.is_empty() {
                            return Err(SessionGatewayError::bad("shareUrl must not be empty"));
                        }
                        if trimmed.chars().any(|c| c == '\r' || c == '\n' || c.is_control()) {
                            return Err(SessionGatewayError::bad("shareUrl must not contain newlines or control characters"));
                        }
                        Some(trimmed)
                    }
                    None => None,
                };
                let raw_comment = match args.get("comment") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(text)) => {
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(fabushi_messaging_core::FormattedText::plain(text))
                        }
                    }
                    Some(val @ Value::Object(_)) => {
                        let parsed: fabushi_messaging_core::FormattedText = serde_json::from_value(val.clone())
                            .map_err(|err| SessionGatewayError::bad(format!("invalid comment: {err}")))?;
                        Some(parsed)
                    }
                    Some(_) => return Err(SessionGatewayError::bad("invalid comment: expected object or string")),
                };
                let comment = match share_url {
                    Some(url) => {
                        let base_comment = raw_comment.unwrap_or_else(|| fabushi_messaging_core::FormattedText::plain(""));
                        let composed = session
                            .compose_share_comment(url, base_comment)
                            .map_err(|err| SessionGatewayError::bad(format!("failed to compose share comment: {err:?}")))?;
                        fabushi_messaging_core::validate_formatted_text(&composed)
                            .map_err(|err| SessionGatewayError::bad(format!("invalid comment: {err}")))?;
                        if composed.text.trim().is_empty() {
                            None
                        } else {
                            Some(composed)
                        }
                    }
                    None => match raw_comment {
                        Some(c) => {
                            fabushi_messaging_core::validate_formatted_text(&c)
                                .map_err(|err| SessionGatewayError::bad(format!("invalid comment: {err}")))?;
                            if c.text.trim().is_empty() {
                                None
                            } else {
                                Some(c)
                            }
                        }
                        None => None,
                    },
                };
                let destinations: Vec<HumanForwardDestination> = destination_conversation_ids
                    .into_iter()
                    .map(|id| {
                        let topic_id = destination_topics.as_ref().and_then(|map| map.get(&id)).cloned();
                        HumanForwardDestination {
                            conversation_id: id,
                            topic_id,
                        }
                    })
                    .collect();
                required_string(args, "clientNonce").and_then(|client_nonce| {
                    session
                        .forward_human_message_to_destinations(
                            source_conversation_id,
                            source_entry_id,
                            &destinations,
                            client_nonce,
                            optional_bool(args, "dropSenderNames")?.unwrap_or(false),
                            optional_bool(args, "dropCaptions")?.unwrap_or(false),
                            comment,
                        )
                        .map_err(SessionGatewayError::internal)
                })
            })
        }),
        "retryHumanMessage" => (|| {
            let object = args.as_object().ok_or_else(|| SessionGatewayError::bad("retryHumanMessage requires identity-only arguments"))?;
            if object.len() != 2 || !object.contains_key("conversationId") || !object.contains_key("clientNonce") {
                return Err(SessionGatewayError::bad("retryHumanMessage accepts only conversationId and clientNonce"));
            }
            let exact = |key: &str| args.get(key).and_then(Value::as_str)
                .filter(|value| !value.is_empty() && *value == value.trim())
                .ok_or_else(|| SessionGatewayError::bad(format!("retryHumanMessage requires an unpadded {key}")));
            let conversation_id = exact("conversationId")?;
            let client_nonce = exact("clientNonce")?;
            if client_nonce.len() > 200 {
                return Err(SessionGatewayError::bad("retryHumanMessage clientNonce exceeds 200 UTF-8 bytes"));
            }
            session.retry_human_message(conversation_id, client_nonce).map_err(SessionGatewayError::internal)
        })(),
        "sendHumanMessage" => required_string(args, "conversationId").and_then(|conversation_id| {
            let local_human_id = session.local_human_id().map_err(SessionGatewayError::internal)?;
            if let Some(requested) = optional_string(args, "senderId")? {
                if requested != local_human_id {
                    return Err(SessionGatewayError::bad("senderId does not match the authenticated Human identity"));
                }
            }
            let text = optional_string(args, "text")?.unwrap_or_default();
            let reply_to_id = optional_string(args, "replyToId")?;
            let topic_id = optional_string(args, "topicId")?;
            let attachments = match args.get("attachments") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(values)) => values.clone(),
                Some(_) => return Err(SessionGatewayError::bad("invalid attachments")),
            };
            let queued_offline = match args.get("queuedOffline") {
                None => false,
                Some(Value::Bool(value)) => *value,
                Some(_) => return Err(SessionGatewayError::bad("queuedOffline must be a boolean")),
            };
            let composed_at_ms = optional_f64(args, "composedAtMs");
            if queued_offline && !composed_at_ms.is_some_and(f64::is_finite) {
                return Err(SessionGatewayError::bad("queuedOffline requires a finite composedAtMs"));
            }
            required_string(args, "clientNonce").and_then(|client_nonce| {
                session
                    .append_human_message_with_topic_and_queue_origin(
                        conversation_id,
                        text,
                        client_nonce,
                        composed_at_ms,
                        reply_to_id,
                        &attachments,
                        topic_id,
                        queued_offline,
                    )
                    .map_err(SessionGatewayError::internal)
            })
        }),
        "issueMiniAppLaunchSession" => (|| {
            let app_id = required_string(args, "appId")?;
            let conversation_id = required_string(args, "conversationId")?;
            let start_parameter = optional_string(args, "startParameter")?;
            let version = optional_string(args, "version")?;
            let package_digest = optional_string(args, "packageDigest")?;
            session
                .issue_miniapp_launch_session(app_id, conversation_id, start_parameter, version, package_digest)
                .map_err(|err| match err {
                    MiniAppPostError::PersistenceFailed(msg) => SessionGatewayError::internal(msg),
                    _ => SessionGatewayError::bad(err.to_string()),
                })
        })(),
        "miniAppPostToConversation" => (|| {
            let launch_session_id = required_string(args, "launchSessionId")
                .or_else(|_| required_string(args, "sessionId"))?;
            let request_id = required_string(args, "requestId")?;

            let request: fabushi_messaging_core::MiniAppRequest = if let Some(req_val) = args.get("request") {
                serde_json::from_value(req_val.clone())
                    .map_err(|e| SessionGatewayError::bad(format!("invalid miniapp request: {e}")))?
            } else if let Some(text) = optional_string(args, "text")? {
                fabushi_messaging_core::MiniAppRequest::SendMessage { text: text.to_string() }
            } else if let Some(data) = optional_string(args, "data")? {
                fabushi_messaging_core::MiniAppRequest::SendData { data: data.to_string() }
            } else {
                return Err(SessionGatewayError::bad("miniAppPostToConversation requires request or text/data"));
            };

            session
                .post_miniapp_message(launch_session_id, request_id, &request)
                .map_err(|err| match err {
                    MiniAppPostError::PersistenceFailed(msg) => SessionGatewayError::internal(msg),
                    _ => SessionGatewayError::bad(err.to_string()),
                })
        })(),
        "cacheConversationMedia" => (|| {
            let conversation_id = required_string(args, "conversationId")?;
            let topic_id = optional_string(args, "topicId")?;
            let key = required_string(args, "key")
                .or_else(|_| required_string(args, "mediaId"))?;
            let size_bytes = args
                .get("sizeBytes")
                .and_then(|v| v.as_u64().or_else(|| v.as_i64().and_then(|i| u64::try_from(i).ok())))
                .ok_or_else(|| SessionGatewayError::bad("cacheConversationMedia requires sizeBytes"))?;
            let mime_type = optional_string(args, "mimeType")?
                .unwrap_or("application/octet-stream");
            session
                .cache_conversation_media(
                    conversation_id,
                    topic_id,
                    key,
                    size_bytes,
                    mime_type,
                )
                .map_err(SessionGatewayError::internal)
        })(),
        "getConversationMediaCacheStats" => required_string(args, "conversationId").and_then(|conversation_id| {
            session
                .get_conversation_media_cache_stats(conversation_id)
                .map_err(SessionGatewayError::internal)
        }),
        "clearConversationMediaCache" => (|| {
            let conversation_id = required_string(args, "conversationId")?;
            let topic_id = optional_string(args, "topicId")?;
            session
                .clear_conversation_media_cache(conversation_id, topic_id)
                .map_err(SessionGatewayError::internal)
        })(),
        "reactHumanMessage" => required_string(args, "conversationId").and_then(|conversation_id| {
            required_string(args, "entryId").and_then(|entry_id| {
                // Emoji bytes are reaction identity; validation must not erase
                // padding before the production admission guard sees it.
                args.get("emoji").and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| SessionGatewayError::bad("missing or invalid emoji"))
                    .and_then(|emoji| {
                    session
                        .toggle_human_message_reaction(conversation_id, entry_id, emoji)
                        .map_err(SessionGatewayError::internal)
                })
            })
        }),
        "getHumanConversationTranscript" => required_string(args, "conversationId").and_then(|conversation_id| {
            let topic_id = match args.get("topicId") {
                None | Some(Value::Null) => None,
                Some(Value::String(topic)) if !topic.trim().is_empty() => Some(topic.as_str()),
                Some(_) => return Err(SessionGatewayError::bad("invalid topicId")),
            };
            session
                .read_human_conversation_transcript_for_topic(conversation_id, topic_id)
                .map(Value::Array)
                .map_err(SessionGatewayError::internal)
        }),
        "getHumanConversationTranscriptTail" => required_string(args, "conversationId").and_then(|conversation_id| {
            let before_cursor = match args.get("beforeCursor") {
                None | Some(Value::Null) => None,
                Some(Value::String(cursor)) if !cursor.is_empty() && cursor.len() <= 256 => Some(cursor.clone()),
                Some(_) => return Err(SessionGatewayError::bad("invalid beforeCursor")),
            };
            let topic_id = match args.get("topicId") {
                None | Some(Value::Null) => None,
                Some(Value::String(topic)) if !topic.trim().is_empty() => Some(topic.clone()),
                Some(_) => return Err(SessionGatewayError::bad("invalid topicId")),
            };
            let query = HumanTranscriptPageQuery {
                before_seq: optional_i64(args, "beforeSeq"),
                before_cursor,
                limit: optional_i64(args, "limit").unwrap_or(200),
                topic_id,
            };
            session
                .read_human_conversation_transcript_tail(conversation_id, query)
                .and_then(|page| {
                    let coverage_incomplete =
                        session.human_conversation_coverage_incomplete(conversation_id)?;
                    let mut response = json!({
                        "entries": page.entries,
                        "nextBeforeSeq": page.next_before_seq,
                        "nextBeforeCursor": page.next_before_cursor,
                    });
                    if coverage_incomplete {
                        if let Some(object) = response.as_object_mut() {
                            object.insert("historyCoverageComplete".into(), json!(false));
                        }
                    }
                    Ok(response)
                })
                .map_err(SessionGatewayError::internal)
        }),
        "searchHumanMessages" => required_string(args, "conversationId").and_then(|conversation_id| {
            required_string(args, "query").and_then(|query| {
                if query.trim().is_empty() {
                    return Err(SessionGatewayError::bad(
                        "searchHumanMessages requires a non-empty query",
                    ));
                }
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(50);
                search_human_conversation_messages(
                    session,
                    content_search,
                    conversation_id,
                    query,
                    limit,
                )
                .map(Value::Array)
                .map_err(SessionGatewayError::internal)
            })
        }),
        "getCallTransportIdentity" => session
            .call_transport_identity()
            .map_err(SessionGatewayError::internal),
        "getCallTransportLease" => required_string(args, "callId").and_then(|call_id| {
            session
                .call_transport_lease(call_id)
                .map_err(SessionGatewayError::internal)
        }),
        "getCallIceServers" => session
            .call_ice_servers()
            .map_err(SessionGatewayError::internal),
        "syncHumanCalls" => session
            .sync_human_calls()
            .and_then(|calls| serde_json::to_value(calls).map_err(|error| error.to_string()))
            .map_err(SessionGatewayError::internal),
        "createCallSession" => required_string(args, "scopeId").and_then(|scope_id| {
            let participant_ids = required_string_array(args, "participantIds")?;
            session
                .create_call_session(scope_id, &participant_ids)
                .and_then(|call| serde_json::to_value(call).map_err(|error| error.to_string()))
                .map_err(SessionGatewayError::internal)
        }),
        "getCallSession" => required_string(args, "callId").and_then(|call_id| {
            session
                .get_call_session(call_id)
                .and_then(|call| serde_json::to_value(call).map_err(|error| error.to_string()))
                .map_err(SessionGatewayError::internal)
        }),
        "listCallSessions" => required_string(args, "scopeId").and_then(|scope_id| {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(50);
            session
                .list_call_sessions(scope_id, limit)
                .and_then(|calls| serde_json::to_value(calls).map_err(|error| error.to_string()))
                .map_err(SessionGatewayError::internal)
        }),
        "transitionCallSession" => required_string(args, "callId").and_then(|call_id| {
            required_u64(args, "generation").and_then(|generation| {
                required_string(args, "action").and_then(|action| {
                    session
                        .transition_call_session(
                            call_id,
                            generation,
                            action,
                            optional_string(args, "terminalReason")?,
                        )
                        .and_then(|call| serde_json::to_value(call).map_err(|error| error.to_string()))
                        .map_err(SessionGatewayError::internal)
                })
            })
        }),
        "updateCallMedia" => required_string(args, "callId").and_then(|call_id| {
            required_u64(args, "generation").and_then(|generation| {
                let capabilities = args.get("mediaCapabilities").filter(|value| !value.is_null());
                let devices = args.get("deviceSelection").filter(|value| !value.is_null());
                session
                    .update_call_media(call_id, generation, capabilities, devices)
                    .and_then(|call| serde_json::to_value(call).map_err(|error| error.to_string()))
                    .map_err(SessionGatewayError::internal)
            })
        }),
        "sendCallSignal" => required_string(args, "callId").and_then(|call_id| {
            required_u64(args, "generation").and_then(|generation| {
                required_u64(args, "seq").and_then(|seq| {
                    required_string(args, "senderDeviceId").and_then(|sender_device_id| {
                        required_string(args, "kind").and_then(|kind| {
                            let payload = args.get("payload").ok_or_else(|| {
                                SessionGatewayError::bad("missing required field: payload")
                            })?;
                            session
                                .append_call_signal(
                                    call_id,
                                    generation,
                                    seq,
                                    sender_device_id,
                                    kind,
                                    payload,
                                )
                                .and_then(|signal| serde_json::to_value(signal).map_err(|error| error.to_string()))
                                .map_err(SessionGatewayError::internal)
                        })
                    })
                })
            })
        }),
        "listCallSignals" => required_string(args, "callId").and_then(|call_id| {
            required_u64(args, "generation").and_then(|generation| {
                let after_seq = args.get("afterSeq").and_then(Value::as_u64).unwrap_or(0);
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(100);
                session
                    .list_call_signals(call_id, generation, after_seq, limit)
                    .and_then(|signals| serde_json::to_value(signals).map_err(|error| error.to_string()))
                    .map_err(SessionGatewayError::internal)
            })
        }),
        "createGroup" => {
            required_string(args, "name").and_then(|name| {
                let description = optional_string(args, "description")?.unwrap_or_default();
                let member_ids = required_string_array(args, "memberIds")?;
                group_chat
                    .create_group(name, description, &member_ids)
                    .map_err(map_group_chat_error)
                    .and_then(|created| {
                        serde_json::to_value(created)
                            .map_err(|error| SessionGatewayError::internal(error.to_string()))
                    })
            })
        },
        "setGroupMembers" => required_string(args, "id").and_then(|group_id| {
            let member_ids = required_string_array(args, "memberIds")?;
            group_chat
                .set_group_members(group_id, &member_ids)
                .map_err(map_group_chat_error)
                .and_then(|summary| {
                    serde_json::to_value(summary)
                        .map_err(|error| SessionGatewayError::internal(error.to_string()))
                })
        }),
        "createAgent" => {
            parse_create_agent_profile(args).and_then(|profile| {
                let origin = optional_string(args, "origin")?.unwrap_or("user");
                let purpose = optional_string(args, "purpose")?;
                let introduction_suppressed =
                    optional_bool(args, "isIntroductionSuppressed")?.unwrap_or(false);
                let record = store
                    .create_session(Some(&profile), origin, purpose)
                    .map_err(SessionGatewayError::internal)?;
                if introduction_suppressed {
                    session
                        .set_agent_introduction_pending(&record.id, false)
                        .map_err(SessionGatewayError::internal)?;
                }
                session
                    .mark_agent_viewed(&record.id, system_now_ms(), false)
                    .map_err(SessionGatewayError::internal)?;
                store
                    .write_active_agent_id(&record.id)
                    .map_err(|error| SessionGatewayError::internal(error.to_string()))?;
                let summary = store
                    .summarize_agent_by_id(&record.id)
                    .map_err(SessionGatewayError::internal)?
                    .ok_or_else(|| {
                        SessionGatewayError::internal(
                            "failed to summarize newly created agent",
                        )
                    })?;
                let transcript = session
                    .read_agent_transcript_entries(&record.id)
                    .map_err(SessionGatewayError::internal)?;
                serde_json::to_value(summary)
                    .map_err(|error| SessionGatewayError::internal(error.to_string()))
                    .map(|agent| json!({ "agent": agent, "transcript": transcript }))
            })
        },
        "updateAgent" => required_string(args, "id").and_then(|agent_id| {
            parse_profile_update(args)
                .and_then(|update| {
                    store
                        .update_agent_profile(agent_id, &update)
                        .map_err(SessionGatewayError::internal)
                })
                .and_then(|summary| {
                    serde_json::to_value(summary)
                        .map_err(|error| SessionGatewayError::internal(error.to_string()))
                })
        }),
        "setAgentUnread" => required_string(args, "id").and_then(|agent_id| {
            required_bool(args, "isUnread").and_then(|is_unread| {
                let at_ms = optional_f64(args, "atMs").unwrap_or_else(system_now_ms);
                store
                    .set_session_unread(agent_id, is_unread, at_ms)
                    .map(|_| Value::Null)
                    .map_err(SessionGatewayError::internal)
            })
        }),
        "setAgentNotifyOnUpdates" => required_string(args, "id").and_then(|agent_id| {
            required_bool(args, "isEnabled").and_then(|enabled| {
                store
                    .set_session_notify_on_updates(agent_id, enabled)
                    .map(|_| Value::Null)
                    .map_err(SessionGatewayError::internal)
            })
        }),
        "setAgentHiddenFromSidebar" => required_string(args, "id").and_then(|agent_id| {
            required_bool(args, "isHidden").and_then(|hidden| {
                store
                    .set_session_hidden_from_sidebar(agent_id, hidden)
                    .map(|_| Value::Null)
                    .map_err(SessionGatewayError::internal)
            })
        }),
        "getAgentAvatar" => required_string(args, "id").and_then(|agent_id| {
            store
                .get_agent_avatar(agent_id)
                .map(|avatar| {
                    json!({
                        "version": avatar.version,
                        "dataUrl": avatar.data_url,
                    })
                })
                .map_err(SessionGatewayError::internal)
        }),
        "setAgentAvatarBytes" => required_string(args, "id").and_then(|agent_id| {
            decode_optional_png(args)
                .and_then(|png| {
                    store
                        .set_agent_avatar_bytes(agent_id, png.as_deref())
                        .map_err(SessionGatewayError::internal)
                })
                .and_then(|summary| {
                    serde_json::to_value(summary)
                        .map_err(|error| SessionGatewayError::internal(error.to_string()))
                })
        }),
        "getAgentTranscript" => required_string(args, "id").and_then(|agent_id| {
            session
                .read_agent_transcript_entries(agent_id)
                .map(Value::Array)
                .map_err(SessionGatewayError::internal)
        }),
        "getAgentTranscriptPage" => required_string(args, "id").and_then(|agent_id| {
            let query = TranscriptPageQuery {
                before_seq: optional_i64(args, "beforeSeq"),
                since_ms: optional_i64(args, "sinceMs"),
                until_ms: optional_i64(args, "untilMs").unwrap_or(i64::MAX),
                limit: optional_i64(args, "limit").unwrap_or(500),
            };
            session
                .read_agent_transcript_page(agent_id, query)
                .map(|page| {
                    json!({
                        "entries": page.entries,
                        "nextBeforeSeq": page.next_before_seq,
                    })
                })
                .map_err(SessionGatewayError::internal)
        }),
        "getAgentTranscriptWindow" => required_string(args, "id").and_then(|agent_id| {
            let query = TranscriptWindowQuery {
                before_seq: optional_i64(args, "beforeSeq"),
                limit: optional_i64(args, "limit").unwrap_or(500),
            };
            session
                .read_agent_transcript_window(agent_id, query)
                .map(|window| {
                    json!({
                        "entries": window.entries,
                        "nextBeforeSeq": window.next_before_seq,
                        "threadCounts": window.thread_counts,
                    })
                })
                .map_err(SessionGatewayError::internal)
        }),
        "openAgentTail" | "getAgentTranscriptTail" => required_string(args, "id").and_then(|agent_id| {
            let query = TranscriptWindowQuery {
                before_seq: optional_i64(args, "beforeSeq"),
                limit: optional_i64(args, "limit").unwrap_or(500),
            };
            session
                .read_agent_transcript_tail(agent_id, query)
                .map(|page| {
                    json!({
                        "entries": page.entries,
                        "nextBeforeSeq": page.next_before_seq,
                    })
                })
                .map_err(SessionGatewayError::internal)
        }),
        "getAgentThread" => required_string(args, "id").and_then(|agent_id| {
            required_string(args, "rootId").and_then(|root_id| {
                session
                    .read_agent_thread(agent_id, root_id)
                    .map(|thread| json!({ "entries": thread.entries }))
                    .map_err(SessionGatewayError::internal)
            })
        }),
        "getConversationOutline" => required_string(args, "id").and_then(|agent_id| {
            session
                .read_agent_outline(agent_id)
                .and_then(|outline| serde_json::to_value(outline).map_err(|error| error.to_string()))
                .map_err(SessionGatewayError::internal)
        }),
        "getAgentChannels" | "refreshChannel" => required_string(args, "id").and_then(|agent_id| {
            session
                .list_agent_channels(agent_id)
                .and_then(|channels| serde_json::to_value(channels).map_err(|error| error.to_string()))
                .map_err(SessionGatewayError::internal)
        }),
        "connectChannel" => required_string(args, "id").and_then(|agent_id| {
            required_string(args, "platform").and_then(|platform| {
                required_string(args, "token").and_then(|token| {
                    session
                        .store_connector_credential(agent_id, platform, "token", token)
                        .map_err(SessionGatewayError::internal)
                        .and_then(|stored| {
                            if !stored {
                                return Err(SessionGatewayError::bad(
                                    "connectChannel rejected invalid agent/platform metadata",
                                ));
                            }
                            session
                                .list_agent_channels(agent_id)
                                .and_then(|channels| {
                                    serde_json::to_value(channels).map_err(|error| error.to_string())
                                })
                                .map_err(SessionGatewayError::internal)
                        })
                })
            })
        }),
        "disconnectChannel" => required_string(args, "id").and_then(|agent_id| {
            required_string(args, "platform").and_then(|platform| {
                session
                    .disconnect_channel(agent_id, platform)
                    .map_err(SessionGatewayError::internal)
                    .and_then(|_| {
                        session
                            .list_agent_channels(agent_id)
                            .and_then(|channels| {
                                serde_json::to_value(channels).map_err(|error| error.to_string())
                            })
                            .map_err(SessionGatewayError::internal)
                    })
            })
        }),
        "retireLegacyTelegramData" => (|| {
            let remove_immediately = optional_bool(args, "removeImmediately")?
                .or(optional_bool(args, "remove_immediately")?)
                .unwrap_or(false);
            session
                .retire_legacy_telegram_data(remove_immediately)
                .and_then(|report| {
                    serde_json::to_value(report)
                        .map_err(|error| super::legacy_telegram_retirement::LegacyTelegramRetirementError::Internal(error.to_string()))
                })
                .map_err(|error| SessionGatewayError::bad(error.to_string()))
        })(),
        _ => return None,
    };
    Some(result)
}

pub(crate) fn parse_create_agent_profile(args: &Value) -> Result<SandAgentProfile, SessionGatewayError> {
    let name = required_string(args, "name")?;
    let description = optional_string(args, "description")?.unwrap_or_default();
    Ok(SandAgentProfile {
        name: name.to_string(),
        description: description.to_string(),
        title: optional_string(args, "title")?.unwrap_or_default().to_string(),
        avatar_shape: optional_string(args, "avatarShape")?
            .unwrap_or_default()
            .to_string(),
        avatar_color: optional_string(args, "avatarColor")?
            .unwrap_or_default()
            .to_string(),
    })
}

pub(crate) fn parse_profile_update(args: &Value) -> Result<AgentProfileUpdate, SessionGatewayError> {
    let profile = args
        .get("profile")
        .and_then(Value::as_object)
        .ok_or_else(|| SessionGatewayError::bad("missing or invalid profile"))?;
    let name = object_required_string(profile, "name")?;
    let description = object_required_string(profile, "description")?;
    Ok(AgentProfileUpdate {
        name: name.to_string(),
        description: description.to_string(),
        title: object_optional_string(profile, "title")?.map(ToOwned::to_owned),
        avatar_shape: object_optional_string(profile, "avatarShape")?.map(ToOwned::to_owned),
        avatar_color: object_optional_string(profile, "avatarColor")?.map(ToOwned::to_owned),
    })
}

fn object_required_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, SessionGatewayError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| SessionGatewayError::bad(format!("missing or invalid profile.{field}")))
}

fn object_optional_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<Option<&'a str>, SessionGatewayError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(SessionGatewayError::bad(format!(
            "invalid profile.{field}"
        ))),
    }
}

fn map_group_chat_error(error: GroupChatGlueError) -> SessionGatewayError {
    match error {
        GroupChatGlueError::BadRequest(message) => SessionGatewayError::bad(message),
        GroupChatGlueError::Internal(message) => SessionGatewayError::internal(message),
    }
}

fn required_string_array(
    args: &Value,
    field: &str,
) -> Result<Vec<String>, SessionGatewayError> {
    let values = args
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| SessionGatewayError::bad(format!("missing or invalid {field}")))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| SessionGatewayError::bad(format!("{field} must contain only strings")))
        })
        .collect()
}

fn optional_string_array(
    args: &Value,
    field: &str,
) -> Result<Option<Vec<String>>, SessionGatewayError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(values)) => {
            let res = values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| SessionGatewayError::bad(format!("{field} must contain only strings")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(res))
        }
        _ => Err(SessionGatewayError::bad(format!("{field} must be an array of strings"))),
    }
}

pub(crate) fn optional_string<'a>(
    args: &'a Value,
    field: &str,
) -> Result<Option<&'a str>, SessionGatewayError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(SessionGatewayError::bad(format!("invalid {field}"))),
    }
}

pub(crate) fn optional_bool(args: &Value, field: &str) -> Result<Option<bool>, SessionGatewayError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(SessionGatewayError::bad(format!("invalid {field}"))),
    }
}

fn required_bool(args: &Value, field: &str) -> Result<bool, SessionGatewayError> {
    args.get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| SessionGatewayError::bad(format!("missing or invalid {field}")))
}

fn required_u64(args: &Value, field: &str) -> Result<u64, SessionGatewayError> {
    args.get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| SessionGatewayError::bad(format!("missing or invalid required field: {field}")))
}

fn optional_f64(args: &Value, field: &str) -> Option<f64> {
    args.get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn system_now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}

fn decode_optional_png(args: &Value) -> Result<Option<Vec<u8>>, SessionGatewayError> {
    match args.get("pngBase64") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(encoded)) => BASE64_STANDARD
            .decode(encoded)
            .map(Some)
            .map_err(|_| SessionGatewayError::bad("invalid pngBase64")),
        Some(_) => Err(SessionGatewayError::bad("invalid pngBase64")),
    }
}

fn required_string<'a>(
    args: &'a Value,
    field: &str,
) -> Result<&'a str, SessionGatewayError> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| SessionGatewayError::bad(format!("missing or invalid {field}")))
}

fn optional_i64(args: &Value, field: &str) -> Option<i64> {
    let value = args.get(field)?;
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .map(|value| value.trunc() as i64)
        })
}

