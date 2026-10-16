use crate::actor::ActorId;
use crate::conversation::ConversationId;
use crate::secret_chat::EncryptedSecretMessage;
use serde::{Deserialize, Serialize};

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
pub struct MediaGroupMetadata {
    pub id: String,
    pub index: u16,
    pub count: u16,
}

impl MediaGroupMetadata {
    pub const CLIENT_MESSAGE_PREFIX: &'static str = "ios-media-group:";
    pub const MAX_GROUP_ITEMS: u16 = 64;

    pub fn from_client_message_id(value: &ClientMessageId) -> Option<Self> {
        let payload = value.0.strip_prefix(Self::CLIENT_MESSAGE_PREFIX)?;
        let mut parts = payload.rsplitn(3, ':');
        let count = parts.next()?.parse::<u16>().ok()?;
        let index = parts.next()?.parse::<u16>().ok()?;
        let id = parts.next()?.trim();
        if id.is_empty()
            || id.len() > 128
            || count < 2
            || count > Self::MAX_GROUP_ITEMS
            || index >= count
            || !id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return None;
        }
        Some(Self {
            id: id.to_string(),
            index,
            count,
        })
    }

    pub fn client_message_id(&self) -> ClientMessageId {
        ClientMessageId(format!(
            "{}{}:{}:{}",
            Self::CLIENT_MESSAGE_PREFIX,
            self.id,
            self.index,
            self.count
        ))
    }
}

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
pub struct ForwardPrivacy {
    #[serde(default)]
    pub drop_sender_names: bool,
    #[serde(default)]
    pub drop_captions: bool,
}

impl ForwardPrivacy {
    pub fn normalized(self) -> Self {
        Self {
            drop_sender_names: self.drop_sender_names || self.drop_captions,
            drop_captions: self.drop_captions,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_group: Option<MediaGroupMetadata>,
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

#[cfg(test)]
mod media_group_metadata_tests {
    use super::{ClientMessageId, MediaGroupMetadata};

    #[test]
    fn structured_media_client_ids_round_trip_and_reject_invalid_groups() {
        let metadata = MediaGroupMetadata {
            id: "7c0c58b1-4bdb-4e7f-b7d1-f8ca78f5e5dd".into(),
            index: 1,
            count: 3,
        };
        let client_id = metadata.client_message_id();
        assert_eq!(
            MediaGroupMetadata::from_client_message_id(&client_id),
            Some(metadata)
        );

        for invalid in [
            "ios-media-group:g:0:1",
            "ios-media-group:g:2:2",
            "ios-media-group:g:0:65",
            "ios-media-group:bad/group:0:2",
            "ios:ordinary",
        ] {
            assert_eq!(
                MediaGroupMetadata::from_client_message_id(&ClientMessageId(invalid.into())),
                None
            );
        }
    }
}
