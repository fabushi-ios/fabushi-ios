use crate::actor::{Actor, ActorId};
use crate::conversation::{Conversation, ConversationId, ConversationKind};
use crate::message::{Message, MessageContent, MessageId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchScope {
    Global,
    Conversation,
    Contacts,
    Bots,
    Groups,
    Channels,
    Media,
    Files,
    Links,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub text: String,
    pub scope: SearchScope,
    pub conversation_id: Option<ConversationId>,
    pub sender_id: Option<ActorId>,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipientContentKind {
    Standard,
    Secret,
    Service,
}

impl Default for RecipientContentKind {
    fn default() -> Self {
        Self::Standard
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RecipientSearchRequirements {
    pub require_media: bool,
    pub require_polls: bool,
    pub require_send_other: bool,
    pub require_inline: bool,
    pub require_game: bool,
    pub source_protected_content: bool,
    pub content_kind: RecipientContentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecipientAuthorizationInput {
    pub conversation_kind: ConversationKind,
    pub sender_is_participant: bool,
    pub can_send_messages: bool,
    pub can_send_media: bool,
    pub can_send_polls: bool,
    pub can_send_other: bool,
    pub can_send_inline: bool,
    pub can_send_games: bool,
    pub community_forbidden: bool,
    pub community_restrict_messages: bool,
    pub community_restrict_media: bool,
    pub community_restrict_polls: bool,
    pub community_restrict_other: bool,
    pub community_restrict_inline: bool,
    pub community_restrict_games: bool,
    pub channel_posting_allowed: bool,
}

impl RecipientAuthorizationInput {
    pub fn direct_default(sender_is_participant: bool) -> Self {
        Self {
            conversation_kind: ConversationKind::Direct,
            sender_is_participant,
            can_send_messages: true,
            can_send_media: true,
            can_send_polls: true,
            can_send_other: true,
            can_send_inline: true,
            can_send_games: true,
            community_forbidden: false,
            community_restrict_messages: false,
            community_restrict_media: false,
            community_restrict_polls: false,
            community_restrict_other: false,
            community_restrict_inline: false,
            community_restrict_games: false,
            channel_posting_allowed: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DestinationAuthorizationDenial {
    SourceProtectedContent,
    SenderNotParticipant,
    MessageSendPermissionDenied,
    MediaSendPermissionDenied,
    PollSendPermissionDenied,
    OtherSendPermissionDenied,
    InlineSendPermissionDenied,
    GameSendPermissionDenied,
    CommunitySendRestricted,
    CommunityMediaRestricted,
    CommunityPollRestricted,
    CommunityOtherRestricted,
    CommunityInlineRestricted,
    CommunityGameRestricted,
    ChannelPostingRestricted,
    SecretPlaintextRejected,
    SecretContentOutsideSecretConversation,
    TopicNotFound,
    TopicClosed,
}

/// Pure destination authorization policy shared by `ProductionSessionWorkers` and
/// `MessagingEngine` (`FBCP-ADR-004`).
pub fn authorize_destination(
    input: RecipientAuthorizationInput,
    requirements: RecipientSearchRequirements,
) -> Result<(), DestinationAuthorizationDenial> {
    if requirements.source_protected_content {
        return Err(DestinationAuthorizationDenial::SourceProtectedContent);
    }
    if !input.sender_is_participant {
        return Err(DestinationAuthorizationDenial::SenderNotParticipant);
    }
    if !input.can_send_messages {
        return Err(DestinationAuthorizationDenial::MessageSendPermissionDenied);
    }
    if requirements.require_media && !input.can_send_media {
        return Err(DestinationAuthorizationDenial::MediaSendPermissionDenied);
    }
    if requirements.require_polls && !input.can_send_polls {
        return Err(DestinationAuthorizationDenial::PollSendPermissionDenied);
    }
    if requirements.require_send_other && !input.can_send_other {
        return Err(DestinationAuthorizationDenial::OtherSendPermissionDenied);
    }
    if requirements.require_inline && !input.can_send_inline {
        return Err(DestinationAuthorizationDenial::InlineSendPermissionDenied);
    }
    if requirements.require_game && !input.can_send_games {
        return Err(DestinationAuthorizationDenial::GameSendPermissionDenied);
    }
    if input.community_forbidden || input.community_restrict_messages {
        return Err(DestinationAuthorizationDenial::CommunitySendRestricted);
    }
    if requirements.require_media && input.community_restrict_media {
        return Err(DestinationAuthorizationDenial::CommunityMediaRestricted);
    }
    if requirements.require_polls && input.community_restrict_polls {
        return Err(DestinationAuthorizationDenial::CommunityPollRestricted);
    }
    if requirements.require_send_other && input.community_restrict_other {
        return Err(DestinationAuthorizationDenial::CommunityOtherRestricted);
    }
    if requirements.require_inline && input.community_restrict_inline {
        return Err(DestinationAuthorizationDenial::CommunityInlineRestricted);
    }
    if requirements.require_game && input.community_restrict_games {
        return Err(DestinationAuthorizationDenial::CommunityGameRestricted);
    }

    if matches!(input.conversation_kind, ConversationKind::Channel) {
        if !input.channel_posting_allowed {
            return Err(DestinationAuthorizationDenial::ChannelPostingRestricted);
        }
        if requirements.require_game {
            return Err(DestinationAuthorizationDenial::GameSendPermissionDenied);
        }
    }

    match (requirements.content_kind, input.conversation_kind) {
        (RecipientContentKind::Standard, ConversationKind::Secret) => {
            Err(DestinationAuthorizationDenial::SecretPlaintextRejected)
        }
        (RecipientContentKind::Secret, ConversationKind::Secret) => Ok(()),
        (RecipientContentKind::Secret, _) => {
            Err(DestinationAuthorizationDenial::SecretContentOutsideSecretConversation)
        }
        (RecipientContentKind::Service, _) | (RecipientContentKind::Standard, _) => Ok(()),
    }
}

/// Canonical SearchRecipients authorization boundary.
///
/// Callers must apply this before indexing or exposing a destination. Search/UI
/// code is not permitted to broaden this decision after the fact.
pub fn recipient_search_authorized(
    input: RecipientAuthorizationInput,
    requirements: RecipientSearchRequirements,
) -> bool {
    authorize_destination(input, requirements).is_ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicDestinationEligibilityInput {
    pub topic_requested: bool,
    pub topic_exists: bool,
    pub topic_closed: bool,
    pub can_manage_topics: bool,
}

pub fn authorize_topic_destination(
    input: RecipientAuthorizationInput,
    requirements: RecipientSearchRequirements,
    topic: TopicDestinationEligibilityInput,
) -> Result<(), DestinationAuthorizationDenial> {
    authorize_destination(input, requirements)?;
    if topic.topic_requested {
        if !topic.topic_exists {
            return Err(DestinationAuthorizationDenial::TopicNotFound);
        }
        if topic.topic_closed && !topic.can_manage_topics {
            return Err(DestinationAuthorizationDenial::TopicClosed);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchResultKind {
    Actor,
    Conversation,
    Message,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub kind: SearchResultKind,
    pub id: String,
    pub conversation_id: Option<ConversationId>,
    pub title: String,
    pub snippet: String,
    pub timestamp_ms: Option<i64>,
    pub score: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchIndex {
    actor_terms: BTreeMap<ActorId, BTreeSet<String>>,
    conversation_terms: BTreeMap<ConversationId, BTreeSet<String>>,
    message_terms: BTreeMap<(ConversationId, MessageId), BTreeSet<String>>,
    actors: BTreeMap<ActorId, Actor>,
    conversations: BTreeMap<ConversationId, Conversation>,
    messages: BTreeMap<(ConversationId, MessageId), Message>,
}

impl SearchIndex {
    pub fn index_actor(&mut self, actor: Actor) {
        let mut text = actor.display_name.clone();
        if let Some(username) = &actor.username {
            text.push(' ');
            text.push_str(username);
        }
        if let Some(bio) = &actor.bio {
            text.push(' ');
            text.push_str(bio);
        }
        self.actor_terms.insert(actor.id.clone(), tokenize(&text));
        self.actors.insert(actor.id.clone(), actor);
    }

    pub fn index_conversation(&mut self, conversation: Conversation) {
        let text = format!(
            "{} {}",
            conversation.title,
            conversation.description.as_deref().unwrap_or_default()
        );
        self.conversation_terms
            .insert(conversation.id.clone(), tokenize(&text));
        self.conversations
            .insert(conversation.id.clone(), conversation);
    }

    pub fn index_message(&mut self, message: Message) {
        let terms = tokenize(&message_search_text(&message));
        self.message_terms
            .insert((message.conversation_id.clone(), message.id.clone()), terms);
        self.messages.insert(
            (message.conversation_id.clone(), message.id.clone()),
            message,
        );
    }

    pub fn remove_message(&mut self, conversation_id: &ConversationId, message_id: &MessageId) {
        let key = (conversation_id.clone(), message_id.clone());
        self.message_terms.remove(&key);
        self.messages.remove(&key);
    }

    pub fn search(&self, query: &SearchQuery) -> Vec<SearchResult> {
        let terms = tokenize(&query.text);
        let allow_empty_scoped_collection = matches!(
            query.scope,
            SearchScope::Media | SearchScope::Files | SearchScope::Links
        );
        if terms.is_empty() && !allow_empty_scoped_collection {
            return Vec::new();
        }
        let mut results = Vec::new();
        if !terms.is_empty()
            && matches!(
                query.scope,
                SearchScope::Global | SearchScope::Contacts | SearchScope::Bots
            )
        {
            for (id, indexed) in &self.actor_terms {
                let Some(actor) = self.actors.get(id) else {
                    continue;
                };
                let allowed = match query.scope {
                    SearchScope::Contacts => matches!(actor.kind, crate::actor::ActorKind::Human),
                    SearchScope::Bots => matches!(
                        actor.kind,
                        crate::actor::ActorKind::Bot | crate::actor::ActorKind::Assistant
                    ),
                    _ => true,
                };
                if allowed {
                    let score = score_terms(&terms, indexed);
                    if score > 0 {
                        results.push(SearchResult {
                            kind: SearchResultKind::Actor,
                            id: id.0.clone(),
                            conversation_id: None,
                            title: actor.display_name.clone(),
                            snippet: actor.bio.clone().unwrap_or_default(),
                            timestamp_ms: actor.presence.last_seen_at_ms,
                            score,
                        });
                    }
                }
            }
        }
        if !terms.is_empty()
            && matches!(
                query.scope,
                SearchScope::Global | SearchScope::Groups | SearchScope::Channels
            )
        {
            for (id, indexed) in &self.conversation_terms {
                let Some(conversation) = self.conversations.get(id) else {
                    continue;
                };
                let allowed = match query.scope {
                    SearchScope::Groups => matches!(
                        conversation.kind,
                        crate::conversation::ConversationKind::Group
                    ),
                    SearchScope::Channels => matches!(
                        conversation.kind,
                        crate::conversation::ConversationKind::Channel
                    ),
                    _ => true,
                };
                if allowed {
                    let score = score_terms(&terms, indexed);
                    if score > 0 {
                        results.push(SearchResult {
                            kind: SearchResultKind::Conversation,
                            id: id.0.clone(),
                            conversation_id: Some(id.clone()),
                            title: conversation.title.clone(),
                            snippet: conversation.description.clone().unwrap_or_default(),
                            timestamp_ms: Some(conversation.updated_at_ms),
                            score,
                        });
                    }
                }
            }
        }
        if matches!(
            query.scope,
            SearchScope::Global
                | SearchScope::Conversation
                | SearchScope::Media
                | SearchScope::Files
                | SearchScope::Links
        ) {
            for ((conversation_id, message_id), indexed) in &self.message_terms {
                let Some(message) = self
                    .messages
                    .get(&(conversation_id.clone(), message_id.clone()))
                else {
                    continue;
                };
                if query
                    .conversation_id
                    .as_ref()
                    .is_some_and(|id| id != conversation_id)
                    || query
                        .sender_id
                        .as_ref()
                        .is_some_and(|id| id != &message.sender_id)
                    || query
                        .from_ms
                        .is_some_and(|from| message.created_at_ms < from)
                    || query.to_ms.is_some_and(|to| message.created_at_ms > to)
                    || !content_matches_scope(&message.content, query.scope)
                {
                    continue;
                }
                let search_text = message_search_text(message);
                let score = if terms.is_empty() {
                    1
                } else {
                    let term_score = score_terms(&terms, indexed);
                    if term_score > 0 {
                        term_score
                    } else if query_matches_fields(&query.text, &[&search_text]) {
                        4
                    } else {
                        0
                    }
                };
                if score > 0 {
                    results.push(SearchResult {
                        kind: SearchResultKind::Message,
                        id: message_id.0.clone(),
                        conversation_id: Some(conversation_id.clone()),
                        title: self
                            .conversations
                            .get(conversation_id)
                            .map(|value| value.title.clone())
                            .unwrap_or_else(|| conversation_id.0.clone()),
                        snippet: search_text,
                        timestamp_ms: Some(message.created_at_ms),
                        score,
                    });
                }
            }
        }
        results.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| right.timestamp_ms.cmp(&left.timestamp_ms))
        });
        results.truncate(usize::try_from(query.limit.max(1)).unwrap_or(usize::MAX));
        results
    }
}

pub fn tokenize(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != '@' && character != '#'
        })
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn score_terms(query: &BTreeSet<String>, indexed: &BTreeSet<String>) -> u32 {
    query
        .iter()
        .map(|term| {
            if indexed.contains(term) {
                10
            } else if indexed.iter().any(|candidate| candidate.contains(term)) {
                4
            } else {
                0
            }
        })
        .sum()
}

/// Canonical forward-origin author visibility boundary for search and transcript projection.
///
/// Enforces `ForwardPrivacy::normalized()` (`drop_sender_names || drop_captions`) so
/// origin author attribution is never indexed or exposed when sender names or captions
/// were dropped.
pub fn visible_forward_origin_author<'a>(
    privacy: crate::message::ForwardPrivacy,
    origin_author_name: Option<&'a str>,
) -> Option<&'a str> {
    if privacy.normalized().drop_sender_names {
        return None;
    }
    origin_author_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Canonical domain multi-field query matcher for conversation/message/media search.
///
/// Matches when the normalized query is a contiguous substring of any field or the
/// joined fields, or when every tokenized query term matches across the tokenized
/// or lowered fields.
pub fn query_matches_fields(query: &str, fields: &[&str]) -> bool {
    let normalized = query.trim().to_lowercase();
    if normalized.is_empty() {
        return false;
    }
    let non_empty: Vec<&str> = fields
        .iter()
        .map(|field| field.trim())
        .filter(|field| !field.is_empty())
        .collect();
    if non_empty.is_empty() {
        return false;
    }
    if non_empty
        .iter()
        .any(|field| field.to_lowercase().contains(&normalized))
    {
        return true;
    }
    let joined = non_empty.join(" ");
    let joined_lower = joined.to_lowercase();
    if joined_lower.contains(&normalized) {
        return true;
    }
    let query_terms = tokenize(&normalized);
    if query_terms.is_empty() {
        return false;
    }
    let indexed = tokenize(&joined_lower);
    query_terms.iter().all(|term| {
        indexed.contains(term)
            || indexed.iter().any(|candidate| candidate.contains(term))
            || joined_lower.contains(term.as_str())
    })
}

fn join_non_empty_search_parts(parts: &[Option<&str>]) -> String {
    parts
        .iter()
        .filter_map(|part| part.map(str::trim).filter(|value| !value.is_empty()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn formatted_text_has_link(text: &crate::message::FormattedText) -> bool {
    text.entities.iter().any(|entity| {
        matches!(
            &entity.kind,
            crate::message::TextEntityKind::Url | crate::message::TextEntityKind::TextUrl(_)
        )
    }) || {
        let lower = text.text.to_ascii_lowercase();
        lower.contains("http://") || lower.contains("https://")
    }
}

fn message_search_text(message: &Message) -> String {
    match &message.content {
        MessageContent::Text { text } => text.text.clone(),
        MessageContent::Photo { media, caption, .. }
        | MessageContent::Video { media, caption, .. }
        | MessageContent::Animation { media, caption, .. }
        | MessageContent::Voice { media, caption, .. }
        | MessageContent::Document { media, caption, .. } => join_non_empty_search_parts(&[
            Some(caption.text.as_str()),
            media.file_name.as_deref(),
            media.mime_type.as_deref(),
        ]),
        MessageContent::Audio {
            media,
            caption,
            title,
            performer,
        } => join_non_empty_search_parts(&[
            Some(caption.text.as_str()),
            title.as_deref(),
            performer.as_deref(),
            media.file_name.as_deref(),
            media.mime_type.as_deref(),
        ]),
        MessageContent::VideoNote { media } => join_non_empty_search_parts(&[
            media.file_name.as_deref(),
            media.mime_type.as_deref(),
        ]),
        MessageContent::Sticker { media, emoji, .. } => join_non_empty_search_parts(&[
            emoji.as_deref(),
            media.file_name.as_deref(),
            media.mime_type.as_deref(),
        ]),
        MessageContent::Contact {
            display_name,
            phone_number,
            ..
        } => join_non_empty_search_parts(&[
            Some(display_name.as_str()),
            phone_number.as_deref(),
        ]),
        MessageContent::Poll {
            question, options, ..
        } => {
            let mut parts = vec![question.text.trim()];
            for option in options {
                let trimmed = option.text.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
            }
            parts.into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
        }
        MessageContent::Venue { title, address, .. } => format!("{title} {address}"),
        MessageContent::Service { action, text } => {
            format!("{} {}", action, text.as_deref().unwrap_or_default())
        }
        MessageContent::Invoice { invoice_id } => invoice_id.clone(),
        MessageContent::MiniApp {
            mini_app_id, title, ..
        } => format!("{title} {mini_app_id}"),
        _ => String::new(),
    }
}

fn content_matches_scope(content: &MessageContent, scope: SearchScope) -> bool {
    match scope {
        SearchScope::Media => matches!(
            content,
            MessageContent::Photo { .. }
                | MessageContent::Video { .. }
                | MessageContent::Animation { .. }
                | MessageContent::Audio { .. }
                | MessageContent::Voice { .. }
                | MessageContent::VideoNote { .. }
                | MessageContent::Sticker { .. }
        ),
        SearchScope::Files => matches!(content, MessageContent::Document { .. }),
        SearchScope::Links => match content {
            MessageContent::Text { text } => formatted_text_has_link(text),
            MessageContent::Photo { caption, .. }
            | MessageContent::Video { caption, .. }
            | MessageContent::Animation { caption, .. }
            | MessageContent::Audio { caption, .. }
            | MessageContent::Voice { caption, .. }
            | MessageContent::Document { caption, .. } => formatted_text_has_link(caption),
            _ => false,
        },
        _ => true,
    }
}


#[cfg(test)]
mod recipient_policy_tests {
    use super::*;

    #[test]
    fn recipient_authorization_fails_before_exposure_for_typed_share_requirements() {
        let direct = RecipientAuthorizationInput::direct_default(true);
        assert!(recipient_search_authorized(
            direct,
            RecipientSearchRequirements::default()
        ));

        let mut no_other = direct;
        no_other.can_send_other = false;
        assert!(!recipient_search_authorized(
            no_other,
            RecipientSearchRequirements {
                require_send_other: true,
                ..RecipientSearchRequirements::default()
            }
        ));

        let mut inline_restricted = direct;
        inline_restricted.community_restrict_inline = true;
        assert!(!recipient_search_authorized(
            inline_restricted,
            RecipientSearchRequirements {
                require_inline: true,
                ..RecipientSearchRequirements::default()
            }
        ));

        let mut channel = direct;
        channel.conversation_kind = ConversationKind::Channel;
        channel.channel_posting_allowed = true;
        assert!(!recipient_search_authorized(
            channel,
            RecipientSearchRequirements {
                require_game: true,
                ..RecipientSearchRequirements::default()
            }
        ));

        assert!(!recipient_search_authorized(
            direct,
            RecipientSearchRequirements {
                source_protected_content: true,
                ..RecipientSearchRequirements::default()
            }
        ));

        let mut secret = direct;
        secret.conversation_kind = ConversationKind::Secret;
        assert!(!recipient_search_authorized(
            secret,
            RecipientSearchRequirements::default()
        ));
        assert!(recipient_search_authorized(
            secret,
            RecipientSearchRequirements {
                content_kind: RecipientContentKind::Secret,
                ..RecipientSearchRequirements::default()
            }
        ));
        assert!(!recipient_search_authorized(
            direct,
            RecipientSearchRequirements {
                content_kind: RecipientContentKind::Secret,
                ..RecipientSearchRequirements::default()
            }
        ));
    }

    #[test]
    fn recipient_requirements_deserialize_old_media_poll_shape_with_safe_defaults() {
        let parsed: RecipientSearchRequirements =
            serde_json::from_str(r#"{"requireMedia":true,"requirePolls":false}"#).unwrap();
        assert!(parsed.require_media);
        assert!(!parsed.require_polls);
        assert!(!parsed.require_send_other);
        assert!(!parsed.require_inline);
        assert!(!parsed.require_game);
        assert!(!parsed.source_protected_content);
        assert_eq!(parsed.content_kind, RecipientContentKind::Standard);
    }

    #[test]
    fn forward_origin_visibility_and_multi_term_field_matching_enforce_domain_policy() {
        use crate::message::ForwardPrivacy;

        assert_eq!(
            visible_forward_origin_author(
                ForwardPrivacy {
                    drop_sender_names: false,
                    drop_captions: false,
                },
                Some("  Alice Origin  ")
            ),
            Some("Alice Origin")
        );
        assert_eq!(
            visible_forward_origin_author(
                ForwardPrivacy {
                    drop_sender_names: true,
                    drop_captions: false,
                },
                Some("Alice Origin")
            ),
            None
        );
        assert_eq!(
            visible_forward_origin_author(
                ForwardPrivacy {
                    drop_sender_names: false,
                    drop_captions: true,
                },
                Some("Alice Origin")
            ),
            None
        );

        let fields = ["Alice Origin", "Quarterly planning summary", "quarterly-report.xlsx"];
        assert!(query_matches_fields("alice quarterly.xlsx", &fields));
        assert!(query_matches_fields("origin summary", &fields));
        assert!(!query_matches_fields("alice missing-token", &fields));
        assert!(!query_matches_fields("   ", &fields));

        let keep = ForwardPrivacy {
            drop_sender_names: false,
            drop_captions: false,
        };
        let drop_sender = ForwardPrivacy {
            drop_sender_names: true,
            drop_captions: false,
        };
        assert_eq!(
            keep.resolve_forward_origin_attribution(
                Some(keep),
                Some("carol-id"),
                Some("  Carol Origin  "),
                Some("bob-id"),
                Some("Bob Intermediate"),
                Some("Bob Peer"),
            ),
            Some(crate::message::ForwardOriginAttribution {
                author_id: Some("carol-id"),
                author_name: Some("Carol Origin"),
            })
        );
        assert_eq!(
            keep.resolve_forward_origin_attribution(
                Some(drop_sender),
                Some("carol-id"),
                Some("Carol Origin"),
                Some("bob-id"),
                None,
                Some("  Bob Peer  "),
            ),
            Some(crate::message::ForwardOriginAttribution {
                author_id: Some("bob-id"),
                author_name: Some("Bob Peer"),
            })
        );
        assert_eq!(
            drop_sender.resolve_forward_origin_attribution(
                Some(keep),
                Some("carol-id"),
                Some("Carol Origin"),
                Some("bob-id"),
                Some("Bob Intermediate"),
                Some("Bob Peer"),
            ),
            None
        );
    }

    #[test]
    fn search_index_supports_scoped_media_files_links_collections_and_filename_matching() {
        use crate::actor::ActorId;
        use crate::message::{DeliveryState, FormattedText, MediaRef};

        let conv_id = ConversationId::new("conv-search");
        let sender = ActorId::new("human-alice");
        let base_media = |id: &str, file_name: &str, mime_type: &str| MediaRef {
            id: id.into(),
            file_name: Some(file_name.into()),
            mime_type: Some(mime_type.into()),
            size_bytes: Some(1024),
            width: Some(1280),
            height: Some(720),
            duration_ms: None,
            thumbnail_id: None,
            local_path: None,
            remote_url: None,
            content_hash: None,
        };
        let make_msg = |id: &str, ts: i64, content: MessageContent| Message {
            id: MessageId::new(id),
            conversation_id: conv_id.clone(),
            sender_id: sender.clone(),
            content,
            reply_to_message_id: None,
            thread_root_message_id: None,
            forward_origin: None,
            reply_markup: None,
            reactions: Vec::new(),
            delivery_state: DeliveryState::Sent,
            created_at_ms: ts,
            edited_at_ms: None,
            scheduled_at_ms: None,
            silent: false,
            protected_content: false,
            pinned: false,
            deleted: false,
        };

        let mut index = SearchIndex::default();
        index.index_message(make_msg(
            "msg-photo",
            100,
            MessageContent::Photo {
                media: base_media("m1", "architecture-diagram.png", "image/png"),
                caption: FormattedText::plain("See https://docs.example.com/spec"),
                spoiler: false,
            },
        ));
        index.index_message(make_msg(
            "msg-doc",
            200,
            MessageContent::Document {
                media: base_media("m2", "quarterly-budget.xlsx", "application/vnd.ms-excel"),
                caption: FormattedText::plain("Finance summary"),
            },
        ));
        index.index_message(make_msg(
            "msg-link-text",
            300,
            MessageContent::Text {
                text: FormattedText::plain("Reference https://fabushi.example/roadmap"),
            },
        ));

        let empty_media = index.search(&SearchQuery {
            text: String::new(),
            scope: SearchScope::Media,
            conversation_id: Some(conv_id.clone()),
            sender_id: None,
            from_ms: None,
            to_ms: None,
            limit: 10,
        });
        assert_eq!(empty_media.len(), 1);
        assert_eq!(empty_media[0].id, "msg-photo");

        let empty_files = index.search(&SearchQuery {
            text: String::new(),
            scope: SearchScope::Files,
            conversation_id: Some(conv_id.clone()),
            sender_id: None,
            from_ms: None,
            to_ms: None,
            limit: 10,
        });
        assert_eq!(empty_files.len(), 1);
        assert_eq!(empty_files[0].id, "msg-doc");

        let empty_links = index.search(&SearchQuery {
            text: String::new(),
            scope: SearchScope::Links,
            conversation_id: Some(conv_id.clone()),
            sender_id: None,
            from_ms: None,
            to_ms: None,
            limit: 10,
        });
        assert_eq!(
            empty_links.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            vec!["msg-link-text", "msg-photo"]
        );

        let filename_search = index.search(&SearchQuery {
            text: "quarterly-budget.xlsx".into(),
            scope: SearchScope::Files,
            conversation_id: Some(conv_id),
            sender_id: None,
            from_ms: None,
            to_ms: None,
            limit: 10,
        });
        assert_eq!(filename_search.len(), 1);
        assert_eq!(filename_search[0].id, "msg-doc");
    }

    #[test]
    fn authorize_destination_covers_allow_and_each_typed_denial_path() {
        let base_in = RecipientAuthorizationInput::direct_default(true);
        let base_req = RecipientSearchRequirements::default();
        assert_eq!(authorize_destination(base_in, base_req), Ok(()));

        assert_eq!(
            authorize_destination(
                base_in,
                RecipientSearchRequirements {
                    source_protected_content: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::SourceProtectedContent)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    sender_is_participant: false,
                    ..base_in
                },
                base_req
            ),
            Err(DestinationAuthorizationDenial::SenderNotParticipant)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    can_send_messages: false,
                    ..base_in
                },
                base_req
            ),
            Err(DestinationAuthorizationDenial::MessageSendPermissionDenied)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    can_send_media: false,
                    ..base_in
                },
                RecipientSearchRequirements {
                    require_media: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::MediaSendPermissionDenied)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    can_send_polls: false,
                    ..base_in
                },
                RecipientSearchRequirements {
                    require_polls: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::PollSendPermissionDenied)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    can_send_other: false,
                    ..base_in
                },
                RecipientSearchRequirements {
                    require_send_other: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::OtherSendPermissionDenied)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    can_send_inline: false,
                    ..base_in
                },
                RecipientSearchRequirements {
                    require_inline: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::InlineSendPermissionDenied)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    can_send_games: false,
                    ..base_in
                },
                RecipientSearchRequirements {
                    require_game: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::GameSendPermissionDenied)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    community_forbidden: true,
                    ..base_in
                },
                base_req
            ),
            Err(DestinationAuthorizationDenial::CommunitySendRestricted)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    community_restrict_media: true,
                    ..base_in
                },
                RecipientSearchRequirements {
                    require_media: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::CommunityMediaRestricted)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    community_restrict_polls: true,
                    ..base_in
                },
                RecipientSearchRequirements {
                    require_polls: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::CommunityPollRestricted)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    community_restrict_other: true,
                    ..base_in
                },
                RecipientSearchRequirements {
                    require_send_other: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::CommunityOtherRestricted)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    community_restrict_inline: true,
                    ..base_in
                },
                RecipientSearchRequirements {
                    require_inline: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::CommunityInlineRestricted)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    community_restrict_games: true,
                    ..base_in
                },
                RecipientSearchRequirements {
                    require_game: true,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::CommunityGameRestricted)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    conversation_kind: ConversationKind::Channel,
                    channel_posting_allowed: false,
                    ..base_in
                },
                base_req
            ),
            Err(DestinationAuthorizationDenial::ChannelPostingRestricted)
        );
        assert_eq!(
            authorize_destination(
                RecipientAuthorizationInput {
                    conversation_kind: ConversationKind::Secret,
                    ..base_in
                },
                base_req
            ),
            Err(DestinationAuthorizationDenial::SecretPlaintextRejected)
        );
        assert_eq!(
            authorize_destination(
                base_in,
                RecipientSearchRequirements {
                    content_kind: RecipientContentKind::Secret,
                    ..base_req
                }
            ),
            Err(DestinationAuthorizationDenial::SecretContentOutsideSecretConversation)
        );
    }

    #[test]
    fn authorize_topic_destination_enforces_presence_and_closed_topic_rights() {
        let base_in = RecipientAuthorizationInput::direct_default(true);
        let base_req = RecipientSearchRequirements::default();

        // 1. Underlying failure takes precedence before topic check
        let unauth_in = RecipientAuthorizationInput {
            sender_is_participant: false,
            ..base_in
        };
        let bad_topic = TopicDestinationEligibilityInput {
            topic_requested: true,
            topic_exists: false,
            topic_closed: false,
            can_manage_topics: false,
        };
        assert_eq!(
            authorize_topic_destination(unauth_in, base_req, bad_topic),
            Err(DestinationAuthorizationDenial::SenderNotParticipant)
        );

        // 2. Topic not requested ignores topic flags
        let not_requested = TopicDestinationEligibilityInput {
            topic_requested: false,
            topic_exists: false,
            topic_closed: true,
            can_manage_topics: false,
        };
        assert_eq!(
            authorize_topic_destination(base_in, base_req, not_requested),
            Ok(())
        );

        // 3. Topic requested but does not exist -> TopicNotFound
        let missing_topic = TopicDestinationEligibilityInput {
            topic_requested: true,
            topic_exists: false,
            topic_closed: false,
            can_manage_topics: true,
        };
        assert_eq!(
            authorize_topic_destination(base_in, base_req, missing_topic),
            Err(DestinationAuthorizationDenial::TopicNotFound)
        );

        // 4. Topic closed and caller cannot manage topics -> TopicClosed
        let closed_topic_no_manage = TopicDestinationEligibilityInput {
            topic_requested: true,
            topic_exists: true,
            topic_closed: true,
            can_manage_topics: false,
        };
        assert_eq!(
            authorize_topic_destination(base_in, base_req, closed_topic_no_manage),
            Err(DestinationAuthorizationDenial::TopicClosed)
        );

        // 5. Topic closed and caller can manage topics -> Ok
        let closed_topic_with_manage = TopicDestinationEligibilityInput {
            topic_requested: true,
            topic_exists: true,
            topic_closed: true,
            can_manage_topics: true,
        };
        assert_eq!(
            authorize_topic_destination(base_in, base_req, closed_topic_with_manage),
            Ok(())
        );

        // 6. Topic open -> Ok regardless of manage_topics flag
        let open_topic_no_manage = TopicDestinationEligibilityInput {
            topic_requested: true,
            topic_exists: true,
            topic_closed: false,
            can_manage_topics: false,
        };
        assert_eq!(
            authorize_topic_destination(base_in, base_req, open_topic_no_manage),
            Ok(())
        );

        let open_topic_with_manage = TopicDestinationEligibilityInput {
            topic_requested: true,
            topic_exists: true,
            topic_closed: false,
            can_manage_topics: true,
        };
        assert_eq!(
            authorize_topic_destination(base_in, base_req, open_topic_with_manage),
            Ok(())
        );
    }

    #[test]
    fn topic_destination_denial_and_eligibility_input_serde_roundtrip() {
        let denials = vec![
            DestinationAuthorizationDenial::TopicNotFound,
            DestinationAuthorizationDenial::TopicClosed,
        ];
        for denial in denials {
            let json = serde_json::to_string(&denial).expect("serialize");
            let roundtrip: DestinationAuthorizationDenial =
                serde_json::from_str(&json).expect("deserialize");
            assert_eq!(denial, roundtrip);
        }

        let input = TopicDestinationEligibilityInput {
            topic_requested: true,
            topic_exists: true,
            topic_closed: true,
            can_manage_topics: false,
        };
        let json = serde_json::to_string(&input).expect("serialize");
        let roundtrip: TopicDestinationEligibilityInput =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(input, roundtrip);
    }
}
