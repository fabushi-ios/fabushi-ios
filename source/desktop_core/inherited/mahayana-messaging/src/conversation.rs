use crate::actor::{ActorId, Participant};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConversationId(pub String);

impl ConversationId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_valid(&self) -> bool {
        let value = self.0.trim();
        !value.is_empty() && value.len() <= 200
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationKind {
    Direct,
    Group,
    Channel,
    SavedMessages,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryVisibility {
    NewMembersOnly,
    AllMembers,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettings {
    pub muted_until_ms: Option<i64>,
    pub sound: Option<String>,
    pub show_preview: bool,
    pub notify_mentions: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            muted_until_ms: None,
            sound: None,
            show_preview: true,
            notify_mentions: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPermissions {
    pub can_send_messages: bool,
    pub can_send_media: bool,
    pub can_send_polls: bool,
    /// Canonical capability used by share/link recipient authorization.
    /// Missing values from legacy persisted state fail closed.
    #[serde(default)]
    pub can_send_other: bool,
    /// Canonical capability used by inline-result recipient authorization.
    /// Missing values from legacy persisted state fail closed.
    #[serde(default)]
    pub can_send_inline: bool,
    /// Canonical capability used by game recipient authorization.
    /// Missing values from legacy persisted state fail closed.
    #[serde(default)]
    pub can_send_games: bool,
    pub can_add_members: bool,
    pub can_pin_messages: bool,
    pub can_manage_topics: bool,
    pub can_manage_calls: bool,
}

impl Default for ConversationPermissions {
    fn default() -> Self {
        Self {
            can_send_messages: true,
            can_send_media: true,
            can_send_polls: true,
            can_send_other: true,
            can_send_inline: true,
            can_send_games: true,
            can_add_members: true,
            can_pin_messages: true,
            can_manage_topics: true,
            can_manage_calls: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Topic {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    pub created_by: ActorId,
    pub closed: bool,
    pub hidden: bool,
    #[serde(default)]
    pub unread_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: ConversationId,
    pub kind: ConversationKind,
    pub title: String,
    pub description: Option<String>,
    pub avatar_url: Option<String>,
    pub participants: Vec<Participant>,
    pub owner_id: Option<ActorId>,
    pub last_message_id: Option<String>,
    pub last_read_message_id: Option<String>,
    pub unread_count: u32,
    pub mention_count: u32,
    pub pinned_message_ids: Vec<String>,
    pub notification_settings: NotificationSettings,
    pub permissions: ConversationPermissions,
    pub history_visibility: HistoryVisibility,
    pub topics: Vec<Topic>,
    pub folder_ids: Vec<String>,
    pub archived: bool,
    pub pinned: bool,
    pub marked_unread: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Conversation {
    pub fn direct(
        id: impl Into<String>,
        title: impl Into<String>,
        participants: Vec<Participant>,
        now_ms: i64,
    ) -> Self {
        Self {
            id: ConversationId::new(id),
            kind: ConversationKind::Direct,
            title: title.into(),
            description: None,
            avatar_url: None,
            participants,
            owner_id: None,
            last_message_id: None,
            last_read_message_id: None,
            unread_count: 0,
            mention_count: 0,
            pinned_message_ids: Vec::new(),
            notification_settings: NotificationSettings::default(),
            permissions: ConversationPermissions::default(),
            history_visibility: HistoryVisibility::AllMembers,
            topics: Vec::new(),
            folder_ids: Vec::new(),
            archived: false,
            pinned: false,
            marked_unread: false,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDraft {
    pub conversation_id: ConversationId,
    pub actor_id: ActorId,
    pub text: String,
    pub reply_to_message_id: Option<String>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicDraft {
    pub conversation_id: ConversationId,
    pub topic_id: String,
    pub actor_id: ActorId,
    pub text: String,
    pub reply_to_message_id: Option<String>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationFolder {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    pub conversation_ids: Vec<ConversationId>,
    pub include_contacts: bool,
    pub include_bots: bool,
    pub include_groups: bool,
    pub include_channels: bool,
    pub exclude_muted: bool,
    pub exclude_read: bool,
    pub exclude_archived: bool,
}


/// Source-neutral identity for a selectable child destination inside the canonical
/// Conversation owner. A topic is keyed by its root Message identity, while a
/// saved sublist is keyed by the participant whose saved history it represents.
/// A nested Conversation covers community child histories without creating a
/// second Group/Channel/SavedMessages owner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
pub enum ConversationChildIdentity {
    Topic { root_message_id: String },
    SavedSublist { participant_id: ActorId },
    Conversation { conversation_id: ConversationId },
}

impl ConversationChildIdentity {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Topic { root_message_id } => {
                let value = root_message_id.trim();
                !value.is_empty() && value.len() <= 200
            }
            Self::SavedSublist { participant_id } => participant_id.is_valid(),
            Self::Conversation { conversation_id } => conversation_id.is_valid(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDestination {
    pub conversation_id: ConversationId,
    pub child: Option<ConversationChildIdentity>,
}

impl ConversationDestination {
    pub fn root(conversation_id: ConversationId) -> Self {
        Self {
            conversation_id,
            child: None,
        }
    }

    pub fn topic(
        conversation_id: ConversationId,
        root_message_id: impl Into<String>,
    ) -> Self {
        Self {
            conversation_id,
            child: Some(ConversationChildIdentity::Topic {
                root_message_id: root_message_id.into(),
            }),
        }
    }

    pub fn saved_sublist(
        conversation_id: ConversationId,
        participant_id: ActorId,
    ) -> Self {
        Self {
            conversation_id,
            child: Some(ConversationChildIdentity::SavedSublist { participant_id }),
        }
    }

    pub fn nested_conversation(
        conversation_id: ConversationId,
        child_conversation_id: ConversationId,
    ) -> Self {
        Self {
            conversation_id,
            child: Some(ConversationChildIdentity::Conversation {
                conversation_id: child_conversation_id,
            }),
        }
    }

    pub fn is_valid(&self) -> bool {
        self.conversation_id.is_valid()
            && self.child.as_ref().map_or(true, ConversationChildIdentity::is_valid)
            && !matches!(
                &self.child,
                Some(ConversationChildIdentity::Conversation { conversation_id })
                    if conversation_id == &self.conversation_id
            )
    }

    /// Mirrors the upstream Thread distinction without importing its UI/runtime:
    /// topics and community histories can be marked read but not manually marked
    /// unread; self SavedSublist and monoforum-admin history rows cannot toggle
    /// unread at all.
    pub fn can_toggle_unread(
        &self,
        currently_unread: bool,
        context: ConversationChildUnreadContext,
    ) -> bool {
        if (matches!(&self.child, Some(ConversationChildIdentity::Topic { .. }))
            || context.parent_is_community)
            && !currently_unread
        {
            return false;
        }
        if matches!(
            &self.child,
            Some(ConversationChildIdentity::SavedSublist { .. })
        ) && context.parent_is_self
        {
            return false;
        }
        if self.child.is_none() && context.actor_is_monoforum_admin {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessagePosition {
    pub created_at_ms: i64,
    pub message_id: String,
}

impl ConversationMessagePosition {
    pub fn new(created_at_ms: i64, message_id: impl Into<String>) -> Self {
        Self {
            created_at_ms,
            message_id: message_id.into(),
        }
    }

    pub fn is_valid(&self) -> bool {
        let value = self.message_id.trim();
        !value.is_empty() && value.len() <= 200
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationChildPaginationState {
    pub message_ids: Vec<String>,
    pub skipped_before: Option<u32>,
    pub skipped_after: Option<u32>,
    pub full_count: Option<u32>,
}

impl ConversationChildPaginationState {
    pub fn replace_window(
        &mut self,
        message_ids: Vec<String>,
        skipped_before: Option<u32>,
        skipped_after: Option<u32>,
        full_count: Option<u32>,
    ) -> bool {
        if message_ids
            .iter()
            .any(|message_id| message_id.trim().is_empty() || message_id.len() > 200)
        {
            return false;
        }
        let unique = message_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        if unique.len() != message_ids.len() {
            return false;
        }
        if let (Some(before), Some(after), Some(full)) =
            (skipped_before, skipped_after, full_count)
        {
            let visible = u32::try_from(message_ids.len()).unwrap_or(u32::MAX);
            if before.saturating_add(visible).saturating_add(after) != full {
                return false;
            }
        }
        self.message_ids = message_ids;
        self.skipped_before = skipped_before;
        self.skipped_after = skipped_after;
        self.full_count = full_count;
        true
    }

    pub fn has_gap_before(&self) -> bool {
        self.skipped_before.is_none_or(|count| count > 0)
    }

    pub fn has_gap_after(&self) -> bool {
        self.skipped_after.is_none_or(|count| count > 0)
    }

    pub fn remove_messages(
        &mut self,
        deleted_message_ids: &std::collections::BTreeSet<String>,
    ) -> usize {
        if deleted_message_ids.is_empty() || self.message_ids.is_empty() {
            return 0;
        }
        let before_len = self.message_ids.len();
        self.message_ids
            .retain(|message_id| !deleted_message_ids.contains(message_id));
        let removed = before_len.saturating_sub(self.message_ids.len());
        if removed > 0 {
            let removed_u32 = u32::try_from(removed).unwrap_or(u32::MAX);
            self.full_count = self
                .full_count
                .map(|full_count| full_count.saturating_sub(removed_u32));
        }
        removed
    }

    /// Rekeys a settled local message identity to its canonical server message
    /// identity (`data_history_messages.cpp:502-511`). If `to_id` is already
    /// present in the window, collapses the duplicate `from_id` in place and
    /// decrements `full_count` by 1 so the window remains deduplicated.
    pub fn rekey_message_id(&mut self, from_id: &str, to_id: &str) -> bool {
        let from_id = from_id.trim();
        let to_id = to_id.trim();
        if from_id.is_empty()
            || to_id.is_empty()
            || to_id.len() > 200
            || from_id == to_id
            || self.message_ids.is_empty()
        {
            return false;
        }
        let Some(from_index) = self.message_ids.iter().position(|id| id == from_id) else {
            return false;
        };
        if self.message_ids.iter().any(|id| id == to_id) {
            self.message_ids.remove(from_index);
            self.full_count = self
                .full_count
                .map(|full_count| full_count.saturating_sub(1));
        } else {
            self.message_ids[from_index] = to_id.to_string();
        }
        true
    }

    /// Marks the newest end of the window as incomplete while retaining cached
    /// `message_ids` and `skipped_before` (`SparseIdsList::invalidateBottom`).
    pub fn invalidate_bottom(&mut self) {
        self.skipped_after = None;
        self.full_count = None;
    }

    /// Establishes a known complete empty window (`SparseIdsList::removeAll`).
    pub fn clear_to_complete_empty(&mut self) {
        self.message_ids.clear();
        self.skipped_before = Some(0);
        self.skipped_after = Some(0);
        self.full_count = Some(0);
    }
}

/// Account-scoped state for a typed child inside the canonical Conversation owner.
///
/// This carries the source-neutral responsibilities shared by topic, saved-sublist,
/// and nested/community child histories. It is deliberately not a SavedMessages
/// or Telegram owner: the destination identity remains canonical Conversation
/// state and the actor-scoped lifecycle can be persisted by MessagingState.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationChildRuntimeState {
    pub destination: ConversationDestination,
    pub actor_id: ActorId,
    pub inbox_read_till: Option<ConversationMessagePosition>,
    pub outbox_read_till: Option<ConversationMessagePosition>,
    pub unread_count: Option<u32>,
    pub marked_unread: bool,
    pub draft_text: String,
    pub draft_reply_to_message_id: Option<String>,
    pub draft_updated_at_ms: Option<i64>,
    pub pinned: bool,
    pub restore_pinned_when_non_empty: bool,
    pub active: bool,
    pub no_paid_messages: bool,
    pub pagination: ConversationChildPaginationState,
}

impl ConversationChildRuntimeState {
    pub fn new(destination: ConversationDestination, actor_id: ActorId) -> Option<Self> {
        if !destination.is_valid() || !actor_id.is_valid() {
            return None;
        }
        Some(Self {
            destination,
            actor_id,
            inbox_read_till: None,
            outbox_read_till: None,
            unread_count: None,
            marked_unread: false,
            draft_text: String::new(),
            draft_reply_to_message_id: None,
            draft_updated_at_ms: None,
            pinned: false,
            restore_pinned_when_non_empty: false,
            active: false,
            no_paid_messages: false,
            pagination: ConversationChildPaginationState::default(),
        })
    }

    pub fn advance_inbox_read_till(
        &mut self,
        position: ConversationMessagePosition,
        unread_count: Option<u32>,
    ) -> bool {
        if !position.is_valid()
            || self
                .inbox_read_till
                .as_ref()
                .is_some_and(|current| position < *current)
        {
            return false;
        }
        self.inbox_read_till = Some(position);
        if unread_count.is_some() || self.unread_count.is_none() {
            self.unread_count = unread_count;
        }
        self.marked_unread = false;
        true
    }

    pub fn advance_outbox_read_till(&mut self, position: ConversationMessagePosition) -> bool {
        if !position.is_valid()
            || self
                .outbox_read_till
                .as_ref()
                .is_some_and(|current| position < *current)
        {
            return false;
        }
        self.outbox_read_till = Some(position);
        true
    }

    pub fn set_draft(
        &mut self,
        text: impl Into<String>,
        reply_to_message_id: Option<String>,
        updated_at_ms: i64,
    ) {
        self.draft_text = text.into();
        self.draft_reply_to_message_id = reply_to_message_id;
        self.draft_updated_at_ms = Some(updated_at_ms);
    }

    pub fn clear_draft(&mut self) {
        self.draft_text.clear();
        self.draft_reply_to_message_id = None;
        self.draft_updated_at_ms = None;
    }

    /// Clears `draft_reply_to_message_id` when the referenced reply target is
    /// deleted, while preserving non-empty composer draft text (`ClearDraftReplyTo`).
    pub fn clear_draft_reply_to(
        &mut self,
        deleted_message_ids: &std::collections::BTreeSet<String>,
    ) -> bool {
        if !self
            .draft_reply_to_message_id
            .as_ref()
            .is_some_and(|reply_id| deleted_message_ids.contains(reply_id))
        {
            return false;
        }
        if self.draft_text.trim().is_empty() {
            self.clear_draft();
        } else {
            self.draft_reply_to_message_id = None;
        }
        true
    }

    /// Reconciles child draft reply targets and pagination window accounting when
    /// messages in the child's backing conversation are deleted.
    pub fn remove_deleted_messages(
        &mut self,
        deleted_message_ids: &std::collections::BTreeSet<String>,
    ) -> bool {
        let cleared_reply = self.clear_draft_reply_to(deleted_message_ids);
        let removed = self.pagination.remove_messages(deleted_message_ids);
        if removed > 0 && self.pagination.message_ids.is_empty() {
            self.note_locally_empty();
        }
        cleared_reply || removed > 0
    }

    /// Rekeys a settled local message identity across child draft reply targets,
    /// pagination windows, and read watermarks (`data_history_messages.cpp:502-511`).
    pub fn rekey_message_id(
        &mut self,
        from_id: &str,
        to_id: &str,
        accepted_at_ms: Option<i64>,
    ) -> bool {
        let from_id = from_id.trim();
        let to_id = to_id.trim();
        if from_id.is_empty() || to_id.is_empty() || to_id.len() > 200 || from_id == to_id {
            return false;
        }
        let mut changed = false;
        if self.draft_reply_to_message_id.as_deref() == Some(from_id) {
            self.draft_reply_to_message_id = Some(to_id.to_string());
            changed = true;
        }
        if self.pagination.rekey_message_id(from_id, to_id) {
            changed = true;
        }
        if let Some(pos) = self.inbox_read_till.as_mut() {
            if pos.message_id == from_id {
                pos.message_id = to_id.to_string();
                if let Some(ts) = accepted_at_ms {
                    pos.created_at_ms = pos.created_at_ms.max(ts);
                }
                changed = true;
            }
        }
        if let Some(pos) = self.outbox_read_till.as_mut() {
            if pos.message_id == from_id {
                pos.message_id = to_id.to_string();
                if let Some(ts) = accepted_at_ms {
                    pos.created_at_ms = pos.created_at_ms.max(ts);
                }
                changed = true;
            }
        }
        changed
    }

    pub fn invalidate_bottom(&mut self) {
        self.pagination.invalidate_bottom();
    }

    pub fn clear_to_complete_empty(&mut self) {
        self.pagination.clear_to_complete_empty();
        self.note_locally_empty();
    }

    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    /// Matches the upstream empty-child pin continuity contract: an empty child
    /// may disappear from the list temporarily without losing the user's pin.
    pub fn note_locally_empty(&mut self) {
        if self.pinned {
            self.pinned = false;
            self.restore_pinned_when_non_empty = true;
        }
    }

    pub fn note_non_empty(&mut self) {
        if self.restore_pinned_when_non_empty {
            self.pinned = true;
            self.restore_pinned_when_non_empty = false;
        }
    }

    /// Clears transient child-owned state when the exact child identity is
    /// destroyed. The parent Conversation remains intact and is not selected as
    /// a silent fallback.
    pub fn destroy(&mut self) {
        self.active = false;
        self.clear_draft();
        self.pagination = ConversationChildPaginationState::default();
        self.unread_count = None;
        self.marked_unread = false;
        self.pinned = false;
        self.restore_pinned_when_non_empty = false;
        self.no_paid_messages = false;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConversationChildUnreadContext {
    pub parent_is_self: bool,
    pub parent_is_community: bool,
    pub actor_is_monoforum_admin: bool,
}

/// Ephemeral picker selection over canonical Conversation identities. Destruction
/// is identity-specific: destroying a topic/sublist/nested child clears only an
/// exact matching selection and never collapses it silently to the parent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationDestinationSelection {
    selected: Option<ConversationDestination>,
}

impl ConversationDestinationSelection {
    pub fn selected(&self) -> Option<&ConversationDestination> {
        self.selected.as_ref()
    }

    pub fn select(&mut self, destination: ConversationDestination) -> bool {
        if !destination.is_valid() {
            return false;
        }
        self.selected = Some(destination);
        true
    }

    pub fn deselect(&mut self) {
        self.selected = None;
    }

    pub fn clear_destroyed_child(
        &mut self,
        parent_conversation_id: &ConversationId,
        child: &ConversationChildIdentity,
    ) -> bool {
        let matches = self.selected.as_ref().is_some_and(|selected| {
            &selected.conversation_id == parent_conversation_id
                && selected.child.as_ref() == Some(child)
        });
        if matches {
            self.selected = None;
        }
        matches
    }
}

/// Live forward draft state bound to a `ConversationDestination`
/// (`ForwardPanel::update` / `itemRemoved` / `applyOptions` / `editToNextOption`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardDraftState {
    pub destination: ConversationDestination,
    pub source_conversation_id: ConversationId,
    pub source_message_ids: Vec<String>,
    pub privacy: crate::message::ForwardPrivacy,
    pub has_captions: bool,
    pub hide_author_policy: crate::message::ForwardHideAuthorPolicy,
}

impl ForwardDraftState {
    pub fn new(
        destination: ConversationDestination,
        source_conversation_id: ConversationId,
        source_message_ids: Vec<String>,
        requested_privacy: crate::message::ForwardPrivacy,
        has_captions: bool,
        hide_author_policy: crate::message::ForwardHideAuthorPolicy,
    ) -> Option<Self> {
        if !destination.is_valid() || !source_conversation_id.is_valid() {
            return None;
        }
        let mut unique_ids = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for raw in source_message_ids {
            let trimmed = raw.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                unique_ids.push(trimmed.to_string());
            }
        }
        if unique_ids.is_empty() {
            return None;
        }
        let mut draft = Self {
            destination,
            source_conversation_id,
            source_message_ids: unique_ids,
            privacy: crate::message::ForwardPrivacy::default(),
            has_captions,
            hide_author_policy,
        };
        draft.apply_options(requested_privacy);
        Some(draft)
    }

    pub fn is_empty(&self) -> bool {
        self.source_message_ids.is_empty()
    }

    pub fn apply_options(
        &mut self,
        requested_privacy: crate::message::ForwardPrivacy,
    ) -> crate::message::ForwardPrivacy {
        let mut normalized = requested_privacy.normalized_for_policy(self.hide_author_policy);
        if !self.has_captions && normalized.drop_captions {
            normalized.drop_captions = false;
        }
        self.privacy = normalized;
        self.privacy
    }

    pub fn cycle_to_next_option(&mut self) -> crate::message::ForwardPrivacy {
        self.privacy = self
            .privacy
            .cycle_next(self.has_captions, self.hide_author_policy);
        self.privacy
    }

    /// Removes deleted source message IDs from the live forward draft (`ForwardPanel::itemRemoved`).
    pub fn remove_deleted_source_messages(
        &mut self,
        deleted_message_ids: &std::collections::BTreeSet<String>,
    ) -> bool {
        if deleted_message_ids.is_empty() || self.source_message_ids.is_empty() {
            return false;
        }
        let before = self.source_message_ids.len();
        self.source_message_ids
            .retain(|id| !deleted_message_ids.contains(id));
        if self.source_message_ids.is_empty() {
            self.privacy = crate::message::ForwardPrivacy::default();
        }
        self.source_message_ids.len() != before
    }

    /// Clears the live forward draft when its typed `Topic`, `SavedSublist`, or
    /// nested `Conversation` destination is destroyed.
    pub fn clear_if_destination_destroyed(
        &mut self,
        parent_conversation_id: &ConversationId,
        destroyed_child: &ConversationChildIdentity,
    ) -> bool {
        let matches = &self.destination.conversation_id == parent_conversation_id
            && self.destination.child.as_ref() == Some(destroyed_child);
        if matches {
            self.source_message_ids.clear();
            self.privacy = crate::message::ForwardPrivacy::default();
        }
        matches
    }
}

#[cfg(test)]
mod child_destination_tests {
    use super::*;

    #[test]
    fn topic_saved_sublist_and_nested_conversation_keep_distinct_identity() {
        let parent = ConversationId::new("conversation:parent");
        let topic = ConversationDestination::topic(parent.clone(), "topic:42");
        let saved = ConversationDestination::saved_sublist(
            parent.clone(),
            ActorId::new("human:42"),
        );
        let nested = ConversationDestination::nested_conversation(
            parent.clone(),
            ConversationId::new("conversation:child"),
        );

        assert!(topic.is_valid());
        assert!(saved.is_valid());
        assert!(nested.is_valid());
        assert_ne!(topic, saved);
        assert_ne!(saved, nested);
        assert_ne!(topic, nested);
        assert!(!ConversationDestination::nested_conversation(
            parent.clone(),
            parent,
        )
        .is_valid());
    }

    #[test]
    fn destroying_exact_child_clears_selection_without_parent_fallback() {
        let parent = ConversationId::new("conversation:parent");
        let topic_child = ConversationChildIdentity::Topic {
            root_message_id: "topic:42".into(),
        };
        let saved_child = ConversationChildIdentity::SavedSublist {
            participant_id: ActorId::new("human:42"),
        };
        let mut selection = ConversationDestinationSelection::default();
        assert!(selection.select(ConversationDestination {
            conversation_id: parent.clone(),
            child: Some(topic_child.clone()),
        }));

        assert!(!selection.clear_destroyed_child(&parent, &saved_child));
        assert!(selection.selected().is_some());
        assert!(selection.clear_destroyed_child(&parent, &topic_child));
        assert!(selection.selected().is_none());
    }

    #[test]
    fn unread_toggle_policy_preserves_topic_community_and_saved_sublist_rules() {
        let parent = ConversationId::new("conversation:parent");
        let topic = ConversationDestination::topic(parent.clone(), "topic:42");
        assert!(!topic.can_toggle_unread(
            false,
            ConversationChildUnreadContext::default(),
        ));
        assert!(topic.can_toggle_unread(
            true,
            ConversationChildUnreadContext::default(),
        ));

        let community = ConversationDestination::root(parent.clone());
        assert!(!community.can_toggle_unread(
            false,
            ConversationChildUnreadContext {
                parent_is_community: true,
                ..ConversationChildUnreadContext::default()
            },
        ));

        let saved = ConversationDestination::saved_sublist(
            parent.clone(),
            ActorId::new("human:42"),
        );
        assert!(!saved.can_toggle_unread(
            true,
            ConversationChildUnreadContext {
                parent_is_self: true,
                ..ConversationChildUnreadContext::default()
            },
        ));

        let monoforum_admin = ConversationDestination::root(parent);
        assert!(!monoforum_admin.can_toggle_unread(
            true,
            ConversationChildUnreadContext {
                actor_is_monoforum_admin: true,
                ..ConversationChildUnreadContext::default()
            },
        ));
    }

    #[test]
    fn child_runtime_read_cursor_never_moves_backwards() {
        let destination = ConversationDestination::saved_sublist(
            ConversationId::new("conversation:self"),
            ActorId::new("human:peer"),
        );
        let mut state =
            ConversationChildRuntimeState::new(destination, ActorId::new("human:self"))
                .expect("valid child state");

        assert!(state.advance_inbox_read_till(
            ConversationMessagePosition::new(20, "message:20"),
            Some(3),
        ));
        state.marked_unread = true;
        assert!(!state.advance_inbox_read_till(
            ConversationMessagePosition::new(10, "message:10"),
            Some(9),
        ));
        assert_eq!(
            state.inbox_read_till,
            Some(ConversationMessagePosition::new(20, "message:20"))
        );
        assert_eq!(state.unread_count, Some(3));
        assert!(state.marked_unread);

        assert!(state.advance_inbox_read_till(
            ConversationMessagePosition::new(30, "message:30"),
            Some(0),
        ));
        assert_eq!(state.unread_count, Some(0));
        assert!(!state.marked_unread);
    }

    #[test]
    fn child_runtime_page_accounting_rejects_duplicates_and_bad_gaps() {
        let mut page = ConversationChildPaginationState::default();
        assert!(!page.replace_window(
            vec!["message:1".into(), "message:1".into()],
            Some(0),
            Some(0),
            Some(2),
        ));
        assert!(!page.replace_window(
            vec!["message:2".into(), "message:1".into()],
            Some(4),
            Some(3),
            Some(8),
        ));
        assert!(page.replace_window(
            vec!["message:2".into(), "message:1".into()],
            Some(4),
            Some(3),
            Some(9),
        ));
        assert!(page.has_gap_before());
        assert!(page.has_gap_after());

        assert!(page.replace_window(
            vec!["message:2".into(), "message:1".into()],
            Some(0),
            Some(0),
            Some(2),
        ));
        assert!(!page.has_gap_before());
        assert!(!page.has_gap_after());
    }

    #[test]
    fn child_runtime_preserves_pin_across_temporary_empty_and_clears_on_destroy() {
        let destination = ConversationDestination::saved_sublist(
            ConversationId::new("conversation:self"),
            ActorId::new("human:peer"),
        );
        let mut state =
            ConversationChildRuntimeState::new(destination, ActorId::new("human:self"))
                .expect("valid child state");
        state.pinned = true;
        state.active = true;
        state.no_paid_messages = true;
        state.set_draft("draft", Some("message:reply".into()), 42);
        assert!(state.pagination.replace_window(
            vec!["message:1".into()],
            Some(0),
            Some(0),
            Some(1),
        ));

        state.note_locally_empty();
        assert!(!state.pinned);
        assert!(state.restore_pinned_when_non_empty);
        state.note_non_empty();
        assert!(state.pinned);
        assert!(!state.restore_pinned_when_non_empty);

        state.destroy();
        assert!(!state.active);
        assert!(!state.pinned);
        assert!(!state.no_paid_messages);
        assert!(state.draft_text.is_empty());
        assert!(state.pagination.message_ids.is_empty());
    }

    #[test]
    fn child_runtime_removes_deleted_messages_and_clears_matching_draft_reply_without_losing_text() {
        let destination = ConversationDestination::topic(
            ConversationId::new("conversation:parent"),
            "topic:42",
        );
        let mut state =
            ConversationChildRuntimeState::new(destination, ActorId::new("human:self"))
                .expect("valid child state");
        state.pinned = true;
        state.set_draft("surviving draft text", Some("message:reply".into()), 99);
        assert!(state.pagination.replace_window(
            vec!["message:reply".into(), "message:keep".into()],
            Some(1),
            Some(0),
            Some(3),
        ));

        let deleted = std::collections::BTreeSet::from(["message:reply".to_string()]);
        assert!(state.remove_deleted_messages(&deleted));
        assert_eq!(state.draft_text, "surviving draft text");
        assert_eq!(state.draft_reply_to_message_id, None);
        assert_eq!(state.draft_updated_at_ms, Some(99));
        assert_eq!(state.pagination.message_ids, vec!["message:keep".to_string()]);
        assert_eq!(state.pagination.full_count, Some(2));
        assert!(state.pinned);

        state.set_draft("   ", Some("message:keep".into()), 100);
        let deleted_last = std::collections::BTreeSet::from(["message:keep".to_string()]);
        assert!(state.remove_deleted_messages(&deleted_last));
        assert!(state.draft_text.is_empty());
        assert_eq!(state.draft_reply_to_message_id, None);
        assert_eq!(state.draft_updated_at_ms, None);
        assert!(state.pagination.message_ids.is_empty());
        assert_eq!(state.pagination.full_count, Some(1));
        assert!(!state.pinned);
        assert!(state.restore_pinned_when_non_empty);
    }

    #[test]
    fn child_runtime_rekeys_local_to_server_message_id_and_invalidates_bottom_window() {
        let destination = ConversationDestination::topic(
            ConversationId::new("conversation:parent"),
            "topic:42",
        );
        let mut state =
            ConversationChildRuntimeState::new(destination, ActorId::new("human:self"))
                .expect("valid child state");
        state.pinned = true;
        state.set_draft("replying to pending", Some("local:pending:1".into()), 50);
        assert!(state.advance_inbox_read_till(
            ConversationMessagePosition::new(10, "local:pending:1"),
            Some(0),
        ));
        assert!(state.advance_outbox_read_till(
            ConversationMessagePosition::new(10, "local:pending:1"),
        ));
        assert!(state.pagination.replace_window(
            vec!["msg:older".into(), "local:pending:1".into()],
            Some(1),
            Some(0),
            Some(3),
        ));

        assert!(state.rekey_message_id("local:pending:1", "server:settled:100", Some(25)));
        assert_eq!(
            state.draft_reply_to_message_id.as_deref(),
            Some("server:settled:100")
        );
        assert_eq!(
            state.pagination.message_ids,
            vec!["msg:older".to_string(), "server:settled:100".to_string()]
        );
        assert_eq!(state.pagination.full_count, Some(3));
        assert_eq!(
            state.inbox_read_till,
            Some(ConversationMessagePosition::new(25, "server:settled:100"))
        );
        assert_eq!(
            state.outbox_read_till,
            Some(ConversationMessagePosition::new(25, "server:settled:100"))
        );

        // If the server ID was already inserted before rekeying a duplicate local ID,
        // the window collapses the duplicate and decrements full_count by 1.
        assert!(state.pagination.replace_window(
            vec![
                "msg:older".into(),
                "local:dup:2".into(),
                "server:settled:200".into(),
            ],
            Some(1),
            Some(0),
            Some(4),
        ));
        assert!(state.rekey_message_id("local:dup:2", "server:settled:200", Some(30)));
        assert_eq!(
            state.pagination.message_ids,
            vec!["msg:older".to_string(), "server:settled:200".to_string()]
        );
        assert_eq!(state.pagination.full_count, Some(3));

        // invalidate_bottom keeps cached message_ids and skipped_before while clearing
        // skipped_after and full_count.
        state.invalidate_bottom();
        assert_eq!(
            state.pagination.message_ids,
            vec!["msg:older".to_string(), "server:settled:200".to_string()]
        );
        assert_eq!(state.pagination.skipped_before, Some(1));
        assert_eq!(state.pagination.skipped_after, None);
        assert_eq!(state.pagination.full_count, None);
        assert!(state.pagination.has_gap_after());

        // clear_to_complete_empty establishes a known complete empty range and notes locally empty.
        state.clear_to_complete_empty();
        assert!(state.pagination.message_ids.is_empty());
        assert_eq!(state.pagination.skipped_before, Some(0));
        assert_eq!(state.pagination.skipped_after, Some(0));
        assert_eq!(state.pagination.full_count, Some(0));
        assert!(!state.pagination.has_gap_before());
        assert!(!state.pagination.has_gap_after());
        assert!(!state.pinned);
        assert!(state.restore_pinned_when_non_empty);
    }

}
