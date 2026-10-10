use crate::actor::{ActorId, Participant};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConversationId(pub String);

impl ConversationId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
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

}
