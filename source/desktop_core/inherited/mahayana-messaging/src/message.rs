use crate::actor::ActorId;
use crate::conversation::ConversationId;
use crate::secret_chat::EncryptedSecretMessage;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(pub String);

impl MessageId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientMessageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEntity {
    pub offset_utf16: u32,
    pub length_utf16: u32,
    pub kind: TextEntityKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    content = "value"
)]
pub enum TextEntityKind {
    Mention,
    MentionActor(ActorId),
    Hashtag,
    Url,
    Email,
    PhoneNumber,
    BotCommand,
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Spoiler,
    Code,
    Pre { language: Option<String> },
    TextUrl(String),
    CustomEmoji(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormattedText {
    pub text: String,
    #[serde(default)]
    pub entities: Vec<TextEntity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextEntityOffsetOverflow;

impl FormattedText {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            entities: Vec::new(),
        }
    }

    /// Prepends unformatted text while preserving entity positions expressed in UTF-16
    /// code units. This matches the canonical text-entity coordinate system used by
    /// clients and avoids treating UTF-8 byte length as a formatting offset.
    ///
    /// The operation is atomic: if the prefix or any shifted entity offset would
    /// overflow u32, neither the text nor its entities are changed.
    pub fn prepend_plain_text(
        &mut self,
        prefix: &str,
    ) -> Result<(), TextEntityOffsetOverflow> {
        let shift = u32::try_from(prefix.encode_utf16().count())
            .map_err(|_| TextEntityOffsetOverflow)?;
        if self
            .entities
            .iter()
            .any(|entity| entity.offset_utf16.checked_add(shift).is_none())
        {
            return Err(TextEntityOffsetOverflow);
        }
        for entity in &mut self.entities {
            entity.offset_utf16 += shift;
        }
        self.text.insert_str(0, prefix);
        Ok(())
    }

    /// Drops `TextEntityKind::CustomEmoji` spans when forward provenance is hidden
    /// for a sender without custom-emoji/premium authoring rights, while preserving
    /// standard formatting entities (`history_view_forward_panel.cpp` / `apiwrap.cpp`).
    pub fn strip_custom_emoji(&mut self) -> usize {
        let before = self.entities.len();
        self.entities
            .retain(|entity| !matches!(entity.kind, TextEntityKind::CustomEmoji(_)));
        before.saturating_sub(self.entities.len())
    }
}

/// Composes a share link with an optional formatted comment by safely prepending the
/// link prefix while preserving UTF-16 entity offsets.
///
/// Fails atomically with `Err(TextEntityOffsetOverflow)` if shifting any entity's
/// offset would overflow `u32`.
pub fn compose_share_comment(
    link: &str,
    mut comment: FormattedText,
) -> Result<FormattedText, TextEntityOffsetOverflow> {
    if link.is_empty() {
        return Ok(comment);
    }
    let prefix = if comment.text.is_empty() {
        link.to_string()
    } else if link.ends_with('\n') {
        link.to_string()
    } else {
        format!("{link}\n")
    };
    comment.prepend_plain_text(&prefix)?;
    Ok(comment)
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FormattedTextValidationError {
    #[error("text entity offset or length overflow")]
    OffsetOverflow,
    #[error("text entity span ({offset}..{end}) exceeds text UTF-16 code units length ({text_utf16_len})")]
    SpanOutOfBounds {
        offset: u32,
        end: u32,
        text_utf16_len: usize,
    },
    #[error("empty text cannot have formatting entities")]
    EmptyTextWithEntities,
}

/// Validates that a FormattedText has consistent coordinates:
/// - Entity offsets and lengths do not overflow u32
/// - Entity spans (offset + length) do not exceed the UTF-16 code unit count of the text
/// - Empty text cannot have formatting entities
pub fn validate_formatted_text(formatted: &FormattedText) -> Result<(), FormattedTextValidationError> {
    let text_utf16_len = formatted.text.encode_utf16().count();
    if formatted.text.is_empty() && !formatted.entities.is_empty() {
        return Err(FormattedTextValidationError::EmptyTextWithEntities);
    }
    for entity in &formatted.entities {
        let end = entity
            .offset_utf16
            .checked_add(entity.length_utf16)
            .ok_or(FormattedTextValidationError::OffsetOverflow)?;
        if (end as usize) > text_utf16_len {
            return Err(FormattedTextValidationError::SpanOutOfBounds {
                offset: entity.offset_utf16,
                end,
                text_utf16_len,
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRef {
    pub id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub thumbnail_id: Option<String>,
    pub local_path: Option<String>,
    pub remote_url: Option<String>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollOption {
    pub id: String,
    pub text: String,
    pub voter_count: u32,
    pub chosen: bool,
    pub correct: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineButton {
    pub text: String,
    pub action: InlineButtonAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum InlineButtonAction {
    Callback {
        data: String,
    },
    Url {
        url: String,
    },
    MiniApp {
        mini_app_id: String,
        start_parameter: Option<String>,
    },
    Pay {
        invoice_id: String,
    },
    SwitchInline {
        query: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyMarkup {
    pub rows: Vec<Vec<InlineButton>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForwardHideAuthorPolicy {
    #[default]
    Allowed,
    DisallowedForcedForwardInfo,
    DisallowedRichPageWithoutAuthorCapability,
    PremiumRequiredForRichPage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardPrivacy {
    #[serde(default)]
    pub drop_sender_names: bool,
    #[serde(default)]
    pub drop_captions: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardOriginAttribution<'a> {
    pub author_id: Option<&'a str>,
    pub author_name: Option<&'a str>,
}

impl ForwardPrivacy {
    pub fn normalized(self) -> Self {
        Self {
            drop_sender_names: self.drop_sender_names || self.drop_captions,
            drop_captions: self.drop_captions,
        }
    }

    /// Evaluates whether forward-author hiding is allowed, disallowed by source/capability,
    /// or gated on sender premium (`CanHideForwardAuthor` / `HideForwardAuthorPremiumRequired`
    /// in `history_view_forward_panel.cpp`).
    pub const fn evaluate_hide_author_policy(
        force_preserve_forward_info: bool,
        has_rich_page: bool,
        can_author_rich_messages: bool,
        sender_has_premium: bool,
    ) -> ForwardHideAuthorPolicy {
        if force_preserve_forward_info {
            ForwardHideAuthorPolicy::DisallowedForcedForwardInfo
        } else if has_rich_page && !can_author_rich_messages {
            ForwardHideAuthorPolicy::DisallowedRichPageWithoutAuthorCapability
        } else if has_rich_page && !sender_has_premium {
            ForwardHideAuthorPolicy::PremiumRequiredForRichPage
        } else {
            ForwardHideAuthorPolicy::Allowed
        }
    }

    /// Clamps forward privacy options against source-item eligibility (`CanHideForwardAuthor` /
    /// `NormalizeForwardOptions`). When the source message requires preserving forward attribution,
    /// both `drop_sender_names` and `drop_captions` are clamped to `false`.
    pub fn normalized_for_source(self, force_preserve_forward_info: bool) -> Self {
        if force_preserve_forward_info {
            Self::default()
        } else {
            self.normalized()
        }
    }

    pub fn normalized_for_policy(self, policy: ForwardHideAuthorPolicy) -> Self {
        if matches!(policy, ForwardHideAuthorPolicy::Allowed) {
            self.normalized()
        } else {
            Self::default()
        }
    }

    /// Cycles `PreserveInfo -> NoSenderNames -> NoNamesAndCaptions (when captions exist) -> PreserveInfo`
    /// (`ForwardPanel::editToNextOption`).
    pub fn cycle_next(self, has_captions: bool, policy: ForwardHideAuthorPolicy) -> Self {
        if !matches!(policy, ForwardHideAuthorPolicy::Allowed) {
            return Self::default();
        }
        let current = self.normalized();
        if !current.drop_sender_names {
            Self {
                drop_sender_names: true,
                drop_captions: false,
            }
        } else if !current.drop_captions && has_captions {
            Self {
                drop_sender_names: true,
                drop_captions: true,
            }
        } else {
            Self::default()
        }
    }

    /// Resolves the canonical forward-origin attribution when forwarding a source message.
    ///
    /// - If `self.normalized().drop_sender_names` is `true`, returns `None` (no origin may be emitted).
    /// - Otherwise, if the source message already carries a forward context whose own privacy
    ///   did not drop sender names (`!source_forward_privacy.normalized().drop_sender_names`)
    ///   and whose origin has at least one non-empty `author_id` or `author_name`, preserves
    ///   that upstream origin (`chained forward`).
    /// - Otherwise (ordinary message or anonymously forwarded source message), attributes the
    ///   forward to the source message's direct author (`source_author_id`, `source_author_name`,
    ///   falling back to `peer_title_fallback` when `source_author_name` is absent and the
    ///   source author is the peer in that direct conversation).
    pub fn resolve_forward_origin_attribution<'a>(
        self,
        source_forward_privacy: Option<Self>,
        source_origin_author_id: Option<&'a str>,
        source_origin_author_name: Option<&'a str>,
        source_author_id: Option<&'a str>,
        source_author_name: Option<&'a str>,
        peer_title_fallback: Option<&'a str>,
    ) -> Option<ForwardOriginAttribution<'a>> {
        if self.normalized().drop_sender_names {
            return None;
        }
        let clean = |value: Option<&'a str>| value.map(str::trim).filter(|s| !s.is_empty());
        if let Some(upstream_privacy) = source_forward_privacy {
            if !upstream_privacy.normalized().drop_sender_names {
                let upstream_id = clean(source_origin_author_id);
                let upstream_name = clean(source_origin_author_name);
                if upstream_id.is_some() || upstream_name.is_some() {
                    return Some(ForwardOriginAttribution {
                        author_id: upstream_id,
                        author_name: upstream_name,
                    });
                }
            }
        }
        let direct_id = clean(source_author_id);
        let direct_name = clean(source_author_name).or_else(|| clean(peer_title_fallback));
        if direct_id.is_some() || direct_name.is_some() {
            Some(ForwardOriginAttribution {
                author_id: direct_id,
                author_name: direct_name,
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    content = "data"
)]
pub enum MessageContent {
    Text {
        text: FormattedText,
    },
    Photo {
        media: MediaRef,
        caption: FormattedText,
        spoiler: bool,
    },
    Video {
        media: MediaRef,
        caption: FormattedText,
        spoiler: bool,
        streaming: bool,
    },
    Animation {
        media: MediaRef,
        caption: FormattedText,
    },
    Audio {
        media: MediaRef,
        caption: FormattedText,
        title: Option<String>,
        performer: Option<String>,
    },
    Voice {
        media: MediaRef,
        caption: FormattedText,
        waveform: Vec<u8>,
    },
    VideoNote {
        media: MediaRef,
    },
    Document {
        media: MediaRef,
        caption: FormattedText,
    },
    Sticker {
        media: MediaRef,
        emoji: Option<String>,
        set_id: Option<String>,
    },
    Contact {
        actor_id: Option<ActorId>,
        display_name: String,
        phone_number: Option<String>,
    },
    Location {
        latitude: f64,
        longitude: f64,
        live_until_ms: Option<i64>,
    },
    Venue {
        latitude: f64,
        longitude: f64,
        title: String,
        address: String,
    },
    Poll {
        question: FormattedText,
        options: Vec<PollOption>,
        anonymous: bool,
        multiple_answers: bool,
        quiz: bool,
    },
    Dice {
        emoji: String,
        value: u8,
    },
    Story {
        story_id: String,
    },
    Invoice {
        invoice_id: String,
    },
    MiniApp {
        mini_app_id: String,
        title: String,
        start_parameter: Option<String>,
    },
    Secret {
        envelope: EncryptedSecretMessage,
    },
    Service {
        action: String,
        text: Option<String>,
    },
}

impl MessageContent {
    pub fn clear_caption(&mut self) {
        let caption = match self {
            Self::Photo { caption, .. }
            | Self::Video { caption, .. }
            | Self::Animation { caption, .. }
            | Self::Audio { caption, .. }
            | Self::Voice { caption, .. }
            | Self::Document { caption, .. } => caption,
            _ => return,
        };
        *caption = FormattedText::plain("");
    }

    pub fn has_caption(&self) -> bool {
        match self {
            Self::Photo { caption, .. }
            | Self::Video { caption, .. }
            | Self::Animation { caption, .. }
            | Self::Audio { caption, .. }
            | Self::Voice { caption, .. }
            | Self::Document { caption, .. } => !caption.text.is_empty(),
            _ => false,
        }
    }

    pub fn requires_preserving_forward_info(&self) -> bool {
        matches!(
            self,
            Self::Poll { .. }
                | Self::Invoice { .. }
                | Self::Story { .. }
                | Self::MiniApp { .. }
        )
    }

    pub fn strip_custom_emoji(&mut self) -> usize {
        match self {
            Self::Text { text }
            | Self::Photo { caption: text, .. }
            | Self::Video { caption: text, .. }
            | Self::Animation { caption: text, .. }
            | Self::Audio { caption: text, .. }
            | Self::Voice { caption: text, .. }
            | Self::Document { caption: text, .. }
            | Self::Poll { question: text, .. } => text.strip_custom_emoji(),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
pub enum DeliveryState {
    Pending { client_message_id: ClientMessageId },
    Sent,
    Delivered,
    Read,
    Failed { code: String, retryable: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionSummary {
    pub reaction: String,
    pub count: u32,
    pub chosen_by_me: bool,
    pub recent_actor_ids: Vec<ActorId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PresenceSendTrigger {
    WhenParticipantOnline { actor_id: ActorId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SendMenuMode {
    #[default]
    Disabled,
    SilentOnly,
    Scheduled,
    ScheduledToUser,
    Reminder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMenuDetails {
    pub mode: SendMenuMode,
    pub allow_silent: bool,
    pub allow_schedule: bool,
    pub allow_when_online: bool,
    pub allow_reminder: bool,
}

impl SendMenuDetails {
    pub const fn from_mode(mode: SendMenuMode) -> Self {
        match mode {
            SendMenuMode::Disabled => Self {
                mode,
                allow_silent: false,
                allow_schedule: false,
                allow_when_online: false,
                allow_reminder: false,
            },
            SendMenuMode::SilentOnly => Self {
                mode,
                allow_silent: true,
                allow_schedule: false,
                allow_when_online: false,
                allow_reminder: false,
            },
            SendMenuMode::Scheduled => Self {
                mode,
                allow_silent: true,
                allow_schedule: true,
                allow_when_online: false,
                allow_reminder: false,
            },
            SendMenuMode::ScheduledToUser => Self {
                mode,
                allow_silent: true,
                allow_schedule: true,
                allow_when_online: true,
                allow_reminder: false,
            },
            SendMenuMode::Reminder => Self {
                mode,
                allow_silent: false,
                allow_schedule: true,
                allow_when_online: false,
                allow_reminder: true,
            },
        }
    }

    pub const fn accepts_options(
        &self,
        silent: bool,
        scheduled_at_ms: Option<i64>,
        when_online: bool,
    ) -> bool {
        if when_online && !self.allow_when_online {
            return false;
        }
        if scheduled_at_ms.is_some() && !self.allow_schedule {
            return false;
        }
        if silent && !self.allow_silent {
            return false;
        }
        if when_online && scheduled_at_ms.is_some() {
            return false;
        }
        if matches!(self.mode, SendMenuMode::Disabled)
            && (silent || scheduled_at_ms.is_some() || when_online)
        {
            return false;
        }
        true
    }

    pub const fn normalize_options(
        &self,
        silent: bool,
        scheduled_at_ms: Option<i64>,
        when_online: bool,
    ) -> Option<(bool, Option<i64>, bool)> {
        if when_online && (!self.allow_when_online || scheduled_at_ms.is_some()) {
            return None;
        }
        if scheduled_at_ms.is_some() && !self.allow_schedule {
            return None;
        }
        let normalized_silent = if matches!(self.mode, SendMenuMode::Reminder) {
            false
        } else {
            silent
        };
        if normalized_silent && !self.allow_silent {
            return None;
        }
        if matches!(self.mode, SendMenuMode::Disabled)
            && (normalized_silent || scheduled_at_ms.is_some() || when_online)
        {
            return None;
        }
        Some((normalized_silent, scheduled_at_ms, when_online))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DestinationSendMenuInput {
    pub conversation_exists: bool,
    pub is_secret: bool,
    pub can_send_messages: bool,
    pub sender_is_participant: bool,
    pub requires_paid_messages: bool,
    pub is_self_saved_messages: bool,
    pub can_schedule_when_online: bool,
}

/// Pure send-menu applicability resolver (`FBCP-ADR-004`).
pub fn resolve_send_menu_details_from_inputs(
    sender_valid: bool,
    destinations: &[DestinationSendMenuInput],
) -> SendMenuDetails {
    if !sender_valid || destinations.is_empty() {
        return SendMenuDetails::from_mode(SendMenuMode::Disabled);
    }
    let mut any_paid = false;
    for destination in destinations {
        if !destination.conversation_exists
            || destination.is_secret
            || !destination.can_send_messages
            || !destination.sender_is_participant
        {
            return SendMenuDetails::from_mode(SendMenuMode::Disabled);
        }
        if destination.requires_paid_messages {
            any_paid = true;
        }
    }
    if any_paid {
        return SendMenuDetails::from_mode(SendMenuMode::SilentOnly);
    }
    if destinations.len() == 1 {
        let single = &destinations[0];
        if single.is_self_saved_messages {
            return SendMenuDetails::from_mode(SendMenuMode::Reminder);
        }
        if single.can_schedule_when_online {
            return SendMenuDetails::from_mode(SendMenuMode::ScheduledToUser);
        }
    }
    SendMenuDetails::from_mode(SendMenuMode::Scheduled)
}

#[cfg(test)]
mod send_menu_policy_tests {
    use super::*;

    #[test]
    fn resolve_send_menu_details_from_inputs_covers_disabled_paid_reminder_online_and_scheduled() {
        let valid_dest = DestinationSendMenuInput {
            conversation_exists: true,
            is_secret: false,
            can_send_messages: true,
            sender_is_participant: true,
            requires_paid_messages: false,
            is_self_saved_messages: false,
            can_schedule_when_online: false,
        };
        assert_eq!(
            resolve_send_menu_details_from_inputs(false, &[valid_dest]).mode,
            SendMenuMode::Disabled
        );
        assert_eq!(
            resolve_send_menu_details_from_inputs(true, &[]).mode,
            SendMenuMode::Disabled
        );
        assert_eq!(
            resolve_send_menu_details_from_inputs(
                true,
                &[DestinationSendMenuInput {
                    is_secret: true,
                    ..valid_dest
                }]
            )
            .mode,
            SendMenuMode::Disabled
        );
        assert_eq!(
            resolve_send_menu_details_from_inputs(
                true,
                &[
                    valid_dest,
                    DestinationSendMenuInput {
                        requires_paid_messages: true,
                        ..valid_dest
                    }
                ]
            )
            .mode,
            SendMenuMode::SilentOnly
        );
        assert_eq!(
            resolve_send_menu_details_from_inputs(
                true,
                &[DestinationSendMenuInput {
                    is_self_saved_messages: true,
                    ..valid_dest
                }]
            )
            .mode,
            SendMenuMode::Reminder
        );
        assert_eq!(
            resolve_send_menu_details_from_inputs(
                true,
                &[DestinationSendMenuInput {
                    can_schedule_when_online: true,
                    ..valid_dest
                }]
            )
            .mode,
            SendMenuMode::ScheduledToUser
        );
        assert_eq!(
            resolve_send_menu_details_from_inputs(true, &[valid_dest, valid_dest]).mode,
            SendMenuMode::Scheduled
        );
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPresenceSend {
    pub local_message_id: MessageId,
    pub conversation_id: ConversationId,
    pub client_message_id: ClientMessageId,
    pub sender_id: ActorId,
    pub trigger: PresenceSendTrigger,
    pub content: MessageContent,
    pub reply_to_message_id: Option<MessageId>,
    pub thread_root_message_id: Option<MessageId>,
    pub silent: bool,
    pub protected_content: bool,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub sender_id: ActorId,
    pub content: MessageContent,
    pub reply_to_message_id: Option<MessageId>,
    pub thread_root_message_id: Option<MessageId>,
    pub forward_origin: Option<String>,
    pub reply_markup: Option<ReplyMarkup>,
    pub reactions: Vec<ReactionSummary>,
    pub delivery_state: DeliveryState,
    pub created_at_ms: i64,
    pub edited_at_ms: Option<i64>,
    pub scheduled_at_ms: Option<i64>,
    pub silent: bool,
    pub protected_content: bool,
    pub pinned: bool,
    pub deleted: bool,
}
