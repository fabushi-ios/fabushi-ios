use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use base64::Engine;
use serde_json::Value;

use crate::automations::automation_status_reminder::render_automation_cleared_status_reminder;

use crate::extensions::inference::provider_session::ProviderMessage;
use crate::extensions::attachments::media_understanding_service::append_file_blocks;

use super::sand_agent_profile_prompt::{
    AgentProfileIdentity, agent_profile_identities_equal, render_agent_profile_update,
};
use super::sand_prompt_markers::{
    SAND_HIDDEN_PROMPT_MARKER, SAND_TRUSTED_AUTOMATION_PROMPT_MARKER,
};
use super::shell_terminal_watch::{
    MaterializedUserMessage, RecentTerminalUserMessage, WatermarkResult,
    collect_prepend_user_messages,
};
use super::system_prompt::{
    ReplyContext, USER_MESSAGE_REPLY_REMINDER, append_user_reply_reminder,
    build_attached_files_note, build_reply_context_note, build_user_message_address_note,
};
use super::system_prompt_assembly::{
    ComputerPromptState, RemoteBoxPromptState, append_agent_profile_system_prompt,
    append_computer_system_prompt, append_mcp_system_prompt_sections,
    append_remote_box_system_prompt,
};
use super::video_container::bytes_look_like_video_container;

/// Provider-facing prompt projection for the frozen prompt-collector boundary.
///
/// This intentionally does not mutate the durable Session transcript. The
/// generated Grok path builds a ConversationAction from durable user state;
/// until the full generated Rust action graph is present, the shipping routed
/// provider consumes this equivalent text projection while turn ownership and
/// checkpoint identity keep using the original durable messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPromptProjection {
    pub messages: Vec<ProviderMessage>,
    pub projected_user_text: Option<String>,
    pub prepended_unanswered_questions: bool,
}

const MAX_SELECTED_VIDEO_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptCollectorSelectedImage {
    pub raw: Value,
    pub path: Option<String>,
    pub box_path: Option<String>,
    pub mime_type: Option<String>,
    pub data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptCollectorSelectedVideo {
    pub raw: Value,
    pub path: Option<String>,
    pub box_path: Option<String>,
    pub mime_type: Option<String>,
    pub filename: Option<String>,
    pub fps: Option<u32>,
    pub materialize_to_filesystem: bool,
    pub data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptCollectorSelectedContext {
    pub selected_images: Vec<PromptCollectorSelectedImage>,
    pub selected_videos: Vec<PromptCollectorSelectedVideo>,
    pub attached_file_paths: Vec<String>,
    pub staged_file_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptCollectorTurnAction {
    pub message_id: Option<String>,
    pub text: String,
    pub rich_text: Option<String>,
    pub reply_context: Option<Value>,
    pub hidden: bool,
    pub selected_context: PromptCollectorSelectedContext,
    pub prepend_user_messages: Vec<MaterializedUserMessage>,
}

#[derive(Debug, Clone, Copy)]
pub struct PromptCollectorDynamicUserContext<'a> {
    pub automation_status_reminder: Option<&'a str>,
    pub profile_update: Option<&'a str>,
    pub is_silence_allowed: bool,
}

/// Runner-owned cross-turn state for frozen automation status reminder projection.
///
/// The reference prompt collector suppresses an unchanged status snapshot until
/// compaction advances, and emits one authoritative cleared snapshot when the
/// previous reminder disappears. Host composition may retain this state per
/// Agent, but the transition rules remain owned by the prompt collector.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptCollectorAutomationReminderState {
    last_status_reminder: Option<String>,
    last_compaction_epoch: Option<u64>,
}

impl PromptCollectorAutomationReminderState {
    pub fn reminder_for_turn(
        &self,
        rendered: Option<&str>,
        compaction_epoch: u64,
    ) -> Option<String> {
        let compaction_advanced = self
            .last_compaction_epoch
            .is_some_and(|last| compaction_epoch > last);
        match rendered {
            Some(rendered)
                if self.last_status_reminder.as_deref() == Some(rendered)
                    && !compaction_advanced =>
            {
                None
            }
            Some(rendered) => Some(rendered.to_string()),
            None if self.last_status_reminder.is_none() => None,
            None => {
                let clearing = render_automation_cleared_status_reminder();
                if self.last_status_reminder.as_deref() == Some(clearing.as_str())
                    && !compaction_advanced
                {
                    None
                } else {
                    Some(clearing)
                }
            }
        }
    }

    pub fn note_reminder(&mut self, reminder: Option<&str>, compaction_epoch: u64) {
        let Some(reminder) = reminder else {
            return;
        };
        self.last_status_reminder = Some(reminder.to_string());
        self.last_compaction_epoch = Some(compaction_epoch);
    }

    pub fn reset(&mut self) {
        self.last_status_reminder = None;
        self.last_compaction_epoch = None;
    }
}

pub fn append_profile_system_section_for_turn(
    messages: &mut Vec<ProviderMessage>,
    profile_section: &str,
) {
    append_agent_profile_system_prompt(messages, profile_section);
}

pub fn resolve_profile_update_for_turn(
    identity: &AgentProfileIdentity,
    announced_identity: &AgentProfileIdentity,
) -> Option<String> {
    (!agent_profile_identities_equal(identity, announced_identity))
        .then(|| render_agent_profile_update(identity))
}

pub fn apply_dynamic_user_context_for_turn(
    messages: &mut [ProviderMessage],
    context: PromptCollectorDynamicUserContext<'_>,
) -> bool {
    let automation = context
        .automation_status_reminder
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let profile = context
        .profile_update
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if automation.is_none() && profile.is_none() {
        return false;
    }

    let Some(user) = messages.iter_mut().rfind(|message| message.role == "user") else {
        return false;
    };

    let original = user.content.as_str();
    let mut hidden_prefix = String::new();
    let mut body = original;
    if let Some(rest) = body.strip_prefix(SAND_HIDDEN_PROMPT_MARKER) {
        hidden_prefix.push_str(SAND_HIDDEN_PROMPT_MARKER);
        body = rest;
        if let Some(rest) = body.strip_prefix(SAND_TRUSTED_AUTOMATION_PROMPT_MARKER) {
            hidden_prefix.push_str(SAND_TRUSTED_AUTOMATION_PROMPT_MARKER);
            body = rest;
        }
    }

    let reply_suffix = format!("\n\n{USER_MESSAGE_REPLY_REMINDER}");
    let mut had_reply_reminder = body.ends_with(&reply_suffix);
    if had_reply_reminder {
        body = &body[..body.len() - reply_suffix.len()];
    } else if body == USER_MESSAGE_REPLY_REMINDER {
        had_reply_reminder = true;
        body = "";
    }

    let mut rendered = body.to_string();
    for addition in [automation, profile].into_iter().flatten() {
        rendered = if rendered.is_empty() {
            addition.to_string()
        } else if context.is_silence_allowed {
            format!("{addition}\n\n{rendered}")
        } else {
            format!("{rendered}\n\n{addition}")
        };
    }
    if had_reply_reminder {
        rendered = if rendered.is_empty() {
            USER_MESSAGE_REPLY_REMINDER.to_string()
        } else {
            format!("{rendered}\n\n{USER_MESSAGE_REPLY_REMINDER}")
        };
    }

    user.content = format!("{hidden_prefix}{rendered}");
    true
}

pub fn append_mcp_runtime_sections_for_turn(
    messages: &mut Vec<ProviderMessage>,
    installed: &[Value],
    discovery_unavailable: bool,
    is_subagent_runner: bool,
) {
    append_mcp_system_prompt_sections(
        messages,
        installed,
        discovery_unavailable,
        !is_subagent_runner,
    );
}

pub fn append_remote_runtime_sections_for_turn(
    messages: &mut Vec<ProviderMessage>,
    remote_box: &RemoteBoxPromptState,
    computer: &ComputerPromptState,
) {
    append_remote_box_system_prompt(messages, remote_box);
    append_computer_system_prompt(messages, computer);
}

pub fn selected_media_host_paths_for_turn(args: &Value) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for field in ["selectedImages", "selectedVideos"] {
        for value in args
            .get(field)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(path) = optional_non_empty(value, "path") else {
                continue;
            };
            let path = PathBuf::from(path);
            if !paths.iter().any(|known| known == &path) {
                paths.push(path);
            }
        }
    }
    paths
}

pub fn collect_turn_action_for_projection(
    args: &Value,
    messages: &[ProviderMessage],
) -> Result<PromptCollectorTurnAction, String> {
    let message_id = optional_non_empty(args, "messageId").map(ToOwned::to_owned);
    let text = messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.clone())
        .unwrap_or_default();
    let rich_text = optional_non_empty(args, "richText")
        .map(ToOwned::to_owned)
        .or_else(|| {
            let message_id = message_id.as_deref()?;
            args.get("recentUserMessages")
                .and_then(Value::as_array)?
                .iter()
                .find(|message| optional_non_empty(message, "id") == Some(message_id))
                .and_then(|message| optional_non_empty(message, "richText"))
                .map(ToOwned::to_owned)
        });
    let box_paths = string_map(args.get("boxPathByHostPath"));
    let attached_file_paths = string_array(args, "attachmentPaths")
        .or_else(|| string_array(args, "attachedFilePaths"))
        .unwrap_or_default();
    let staged_file_paths = attached_file_paths
        .iter()
        .filter_map(|path| box_paths.get(path).cloned())
        .collect::<Vec<_>>();

    let selected_images = args
        .get("selectedImages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|raw| {
            let path = optional_non_empty(raw, "path").map(ToOwned::to_owned);
            PromptCollectorSelectedImage {
                raw: raw.clone(),
                box_path: path.as_ref().and_then(|path| box_paths.get(path).cloned()),
                path,
                mime_type: optional_non_empty(raw, "mimeType")
                    .or_else(|| optional_non_empty(raw, "mime_type"))
                    .map(ToOwned::to_owned),
                data: selected_media_bytes(raw),
            }
        })
        .collect::<Vec<_>>();

    let is_subagent = optional_non_empty(args, "parentAgentId").is_some();
    let mut selected_videos = Vec::new();
    for raw in args
        .get("selectedVideos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let path = optional_non_empty(raw, "path").map(ToOwned::to_owned);
        let mut data = selected_media_bytes(raw);
        if is_subagent && data.is_none() && !selected_media_has_external_reference(raw) {
            if let Some(path) = path.as_deref() {
                data = Some(read_subagent_video_bytes(path)?);
            }
        }
        let fps = raw
            .get("fps")
            .and_then(Value::as_f64)
            .filter(|value| {
                value.is_finite()
                    && *value >= 0.0
                    && value.fract() == 0.0
                    && *value <= u32::MAX as f64
            })
            .map(|value| value as u32);
        selected_videos.push(PromptCollectorSelectedVideo {
            raw: raw.clone(),
            box_path: path.as_ref().and_then(|path| box_paths.get(path).cloned()),
            path,
            mime_type: optional_non_empty(raw, "mimeType")
                .or_else(|| optional_non_empty(raw, "mime_type"))
                .map(ToOwned::to_owned),
            filename: optional_non_empty(raw, "filename").map(ToOwned::to_owned),
            fps,
            materialize_to_filesystem: raw
                .get("materializeToFilesystem")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            data,
        });
    }

    Ok(PromptCollectorTurnAction {
        message_id,
        text,
        rich_text,
        reply_context: args.get("replyContext").filter(|value| !value.is_null()).cloned(),
        hidden: args.get("hidden").and_then(Value::as_bool).unwrap_or(false),
        selected_context: PromptCollectorSelectedContext {
            selected_images,
            selected_videos,
            attached_file_paths,
            staged_file_paths,
        },
        prepend_user_messages: Vec::new(),
    })
}

fn selected_media_bytes(value: &Value) -> Option<Vec<u8>> {
    if let Some(data) = value.get("data").and_then(decode_media_bytes) {
        return Some(data);
    }
    let data_or_blob = value
        .get("dataOrBlobId")
        .or_else(|| value.get("data_or_blob_id"))
        .and_then(Value::as_object)?;
    match data_or_blob.get("case").and_then(Value::as_str) {
        Some("data") => data_or_blob.get("value").and_then(decode_media_bytes),
        Some("blobIdWithData") | Some("blob_id_with_data") => data_or_blob
            .get("value")
            .and_then(|value| value.get("data"))
            .and_then(decode_media_bytes),
        _ => None,
    }
}

fn selected_media_has_external_reference(value: &Value) -> bool {
    if ["blobId", "blob_id", "promptUploadRef", "prompt_upload_ref", "signedUrl", "signed_url"]
        .into_iter()
        .any(|field| value.get(field).is_some_and(|value| !value.is_null()))
    {
        return true;
    }
    value
        .get("dataOrBlobId")
        .or_else(|| value.get("data_or_blob_id"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("case"))
        .and_then(Value::as_str)
        .is_some_and(|case| !matches!(case, "data" | "blobIdWithData" | "blob_id_with_data"))
}

fn decode_media_bytes(value: &Value) -> Option<Vec<u8>> {
    if let Some(values) = value.as_array() {
        return values
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .filter(|value| *value <= u8::MAX as u64)
                    .map(|value| value as u8)
            })
            .collect();
    }
    if let Some(raw) = value.as_str() {
        let payload = raw
            .split_once(',')
            .filter(|(prefix, _)| prefix.contains(";base64"))
            .map(|(_, payload)| payload)
            .unwrap_or(raw);
        return base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()
            .or_else(|| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(payload)
                    .ok()
            });
    }
    let object = value.as_object()?;
    let mut indexed = object
        .iter()
        .filter_map(|(index, value)| {
            let index = index.parse::<usize>().ok()?;
            let byte = value.as_u64().filter(|value| *value <= u8::MAX as u64)? as u8;
            Some((index, byte))
        })
        .collect::<Vec<_>>();
    if indexed.is_empty() {
        return None;
    }
    indexed.sort_by_key(|(index, _)| *index);
    indexed
        .iter()
        .enumerate()
        .all(|(expected, (index, _))| expected == *index)
        .then(|| indexed.into_iter().map(|(_, byte)| byte).collect())
}

fn read_subagent_video_bytes(path: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("could not inspect selected video {path}: {error}"))?;
    if metadata.len() > MAX_SELECTED_VIDEO_BYTES {
        return Err(format!("selected video {path} exceeds the 100 MiB turn limit"));
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("could not read selected video {path}: {error}"))?;
    if !bytes_look_like_video_container(&bytes) {
        return Err(format!(
            "selected video {path} does not match a supported video container"
        ));
    }
    Ok(bytes)
}

pub fn apply_staged_attachment_paths_for_turn(
    args: &mut Value,
    staged: &BTreeMap<PathBuf, String>,
) -> usize {
    let Some(object) = args.as_object_mut() else {
        return 0;
    };
    if staged.is_empty() {
        object.remove("boxPathByHostPath");
        return 0;
    }
    object.insert(
        "boxPathByHostPath".into(),
        Value::Object(
            staged
                .iter()
                .map(|(host_path, box_path)| {
                    (
                        host_path.to_string_lossy().into_owned(),
                        Value::String(box_path.clone()),
                    )
                })
                .collect(),
        ),
    );
    staged.len()
}

fn is_confirmed_user_entry(entry: &Value) -> bool {
    entry.get("kind").and_then(Value::as_str) == Some("message")
        && entry.get("role").and_then(Value::as_str) == Some("user")
        && entry.get("fromAgent").is_none_or(Value::is_null)
        && entry.get("channel").is_none_or(Value::is_null)
        && entry.get("confirmed").and_then(Value::as_bool) == Some(true)
}

pub fn collect_unconfirmed_user_messages_with_watermark_for_turn(
    args: &Value,
    watermark: &WatermarkResult,
) -> Vec<MaterializedUserMessage> {
    let current_message_id = optional_non_empty(args, "messageId");
    let recent_user_messages = args
        .get("recentUserMessages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| {
            let id = message.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            Some(RecentTerminalUserMessage {
                id: id.to_string(),
                text: message
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                rich_text: message
                    .get("richText")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect::<Vec<_>>();
    collect_prepend_user_messages(&recent_user_messages, current_message_id, watermark)
}

pub fn prepend_unconfirmed_user_messages_with_watermark_and_collect_for_turn(
    args: &Value,
    messages: &mut Vec<ProviderMessage>,
    watermark: &WatermarkResult,
) -> Vec<MaterializedUserMessage> {
    let prepended = collect_unconfirmed_user_messages_with_watermark_for_turn(args, watermark);
    if prepended.is_empty() {
        return prepended;
    }
    let Some(current_user_index) = messages.iter().rposition(|message| message.role == "user") else {
        return Vec::new();
    };
    let unanswered = unanswered_questions_note(args);
    let insert_index = if !unanswered.is_empty()
        && current_user_index > 0
        && messages[current_user_index - 1].role == "user"
        && messages[current_user_index - 1].content == unanswered
    {
        current_user_index - 1
    } else {
        current_user_index
    };
    for (offset, message) in prepended.iter().enumerate() {
        messages.insert(
            insert_index + offset,
            ProviderMessage {
                role: "user".into(),
                content: message.text.clone(),
            },
        );
    }
    prepended
}

pub fn prepend_unconfirmed_user_messages_with_watermark_for_turn(
    args: &Value,
    messages: &mut Vec<ProviderMessage>,
    watermark: &WatermarkResult,
) -> usize {
    prepend_unconfirmed_user_messages_with_watermark_and_collect_for_turn(
        args,
        messages,
        watermark,
    )
    .len()
}

pub fn prepend_unconfirmed_user_messages_for_turn(
    args: &Value,
    messages: &mut Vec<ProviderMessage>,
    transcript_entries: &[Value],
) -> usize {
    let current_message_id = optional_non_empty(args, "messageId");
    let recent_user_messages = args
        .get("recentUserMessages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| {
            let id = message.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            Some(RecentTerminalUserMessage {
                id: id.to_string(),
                text: message
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                rich_text: message
                    .get("richText")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect::<Vec<_>>();

    let last_user_message_id = transcript_entries
        .iter()
        .rev()
        .filter(|entry| is_confirmed_user_entry(entry))
        .find_map(|entry| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
        });
    let has_user_turn = transcript_entries.iter().any(is_confirmed_user_entry);
    let prepended = collect_prepend_user_messages(
        &recent_user_messages,
        current_message_id,
        &WatermarkResult {
            last_user_message_id,
            has_user_turn,
        },
    );
    if prepended.is_empty() {
        return 0;
    }

    let Some(current_user_index) = messages.iter().rposition(|message| message.role == "user") else {
        return 0;
    };
    let count = prepended.len();
    for (offset, message) in prepended.into_iter().enumerate() {
        messages.insert(
            current_user_index + offset,
            ProviderMessage {
                role: "user".into(),
                content: message.text,
            },
        );
    }
    count
}

pub fn project_provider_messages_for_turn(
    args: &Value,
    messages: &[ProviderMessage],
) -> ProviderPromptProjection {
    let Some(user_index) = messages.iter().rposition(|message| message.role == "user") else {
        return ProviderPromptProjection {
            messages: messages.to_vec(),
            projected_user_text: None,
            prepended_unanswered_questions: false,
        };
    };

    let mut projected = messages.to_vec();
    let raw_user_text = projected[user_index].content.clone();
    let message_id = optional_non_empty(args, "messageId");
    let reply_context = parse_reply_context(args.get("replyContext"));
    let attachment_paths = string_array(args, "attachmentPaths")
        .or_else(|| string_array(args, "attachedFilePaths"))
        .unwrap_or_default();
    let attachment_sizes = number_map(args.get("attachedFileSizes"));
    let box_paths = string_map(args.get("boxPathByHostPath"));

    let address = build_user_message_address_note(message_id);
    let reply = build_reply_context_note(reply_context.as_ref());
    let mut text = [address, reply]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !raw_user_text.is_empty() {
        text = if text.is_empty() {
            raw_user_text
        } else {
            format!("{text}\n{raw_user_text}")
        };
    }

    let attachments = build_attached_files_note(
        &attachment_paths,
        &box_paths,
        &attachment_sizes,
    );
    if !attachments.is_empty() {
        text = append_file_blocks(&text, Some(&attachments));
    }

    let hidden = args.get("hidden").and_then(Value::as_bool) == Some(true);
    if args
        .get("appendReplyReminder")
        .and_then(Value::as_bool)
        == Some(true)
        && !hidden
    {
        text = append_user_reply_reminder(&text);
    }
    if hidden {
        let trusted_automation = args
            .get("automationWake")
            .and_then(Value::as_object)
            .is_some_and(|wake| {
                wake.get("containsUntrustedEventText")
                    .and_then(Value::as_bool)
                    != Some(true)
            });
        text = format!(
            "{SAND_HIDDEN_PROMPT_MARKER}{}{text}",
            if trusted_automation {
                SAND_TRUSTED_AUTOMATION_PROMPT_MARKER
            } else {
                ""
            }
        );
    }

    projected[user_index].content = text.clone();

    let unanswered = unanswered_questions_note(args);
    let prepended_unanswered_questions = !unanswered.is_empty();
    if prepended_unanswered_questions {
        projected.insert(
            user_index,
            ProviderMessage {
                role: "user".into(),
                content: unanswered,
            },
        );
    }

    ProviderPromptProjection {
        messages: projected,
        projected_user_text: Some(text),
        prepended_unanswered_questions,
    }
}

fn parse_reply_context(value: Option<&Value>) -> Option<ReplyContext> {
    let value = value?.as_object()?;
    let target_id = value.get("targetId")?.as_str()?.trim();
    let quote = value.get("quote")?.as_str()?.trim();
    if target_id.is_empty() || quote.is_empty() {
        return None;
    }
    Some(ReplyContext {
        target_id: target_id.to_string(),
        quote: quote.to_string(),
    })
}

fn optional_non_empty<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn string_array(value: &Value, field: &str) -> Option<Vec<String>> {
    let rows = value.get(field)?.as_array()?;
    Some(
        rows.iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

fn number_map(value: Option<&Value>) -> HashMap<String, f64> {
    value
        .and_then(Value::as_object)
        .map(|rows| {
            rows.iter()
                .filter_map(|(key, value)| value.as_f64().map(|value| (key.clone(), value)))
                .collect()
        })
        .unwrap_or_default()
}

fn string_map(value: Option<&Value>) -> HashMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|rows| {
            rows.iter()
                .filter_map(|(key, value)| {
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn unanswered_questions_user_message_for_turn(
    args: &Value,
) -> Option<MaterializedUserMessage> {
    let text = unanswered_questions_note(args);
    (!text.is_empty()).then(|| MaterializedUserMessage {
        text,
        message_id: String::new(),
        rich_text: None,
    })
}

fn unanswered_questions_note(args: &Value) -> String {
    let mut questions = string_array(args, "skippedQuestionPrompts").unwrap_or_default();
    questions.extend(string_array(args, "dismissedQuestionPrompts").unwrap_or_default());
    if questions.is_empty() {
        String::new()
    } else {
        format!(
            "Unanswered questions:\n{}",
            questions
                .iter()
                .map(|question| format!("- {question}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}
