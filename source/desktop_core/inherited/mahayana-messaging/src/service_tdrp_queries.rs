use crate::actor::{ActorId, ParticipantRole};
use crate::community::{
    CommunityState, MemberStatus,
};
use crate::conversation::{
    Conversation, ConversationId, ConversationKind,
};
use crate::protocol::{
    ServerEnvelope, ServerEvent, FABUSHI_MESSAGING_PROTOCOL_VERSION,
};
use crate::search::{
    recipient_search_authorized, RecipientAuthorizationInput, RecipientSearchRequirements,
    SearchIndex, SearchQuery, SearchResultKind,
};
use crate::service::{
    MessagingService, MessagingServiceError,
};
use crate::store::MessagingStateStore;
use crate::wallet::WalletAccountId;
use std::collections::BTreeSet;

impl<S: MessagingStateStore> MessagingService<S> {
    pub(crate) fn search_envelope(
        &self,
        actor_id: &ActorId,
        query: SearchQuery,
        server_time_ms: i64,
    ) -> ServerEnvelope {
        let state = self.engine.state();
        let visible_conversations = state
            .conversations
            .values()
            .filter(|conversation| {
                self.actor_can_see_conversation(actor_id, conversation)
                    || state
                        .communities
                        .get(&conversation.id)
                        .and_then(|community| community.public_username.as_ref())
                        .is_some()
            })
            .map(|conversation| conversation.id.clone())
            .collect::<BTreeSet<_>>();
        let mut index = SearchIndex::default();
        for actor in state.actors.values().cloned() {
            index.index_actor(actor);
        }
        for conversation in state
            .conversations
            .values()
            .filter(|conversation| visible_conversations.contains(&conversation.id))
            .cloned()
        {
            index.index_conversation(conversation);
        }
        for message in visible_conversations
            .iter()
            .filter_map(|conversation_id| state.messages.get(conversation_id))
            .flat_map(|messages| messages.values())
            .filter(|message| !message.deleted)
            .cloned()
        {
            index.index_message(message);
        }
        let results = index.search(&query);
        ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(self.cursor.to_string()),
            server_time_ms,
            event: ServerEvent::SearchResults { query, results },
        }
    }

    pub(crate) fn recipient_search_envelope(
        &self,
        actor_id: &ActorId,
        query: SearchQuery,
        requirements: RecipientSearchRequirements,
        server_time_ms: i64,
    ) -> ServerEnvelope {
        let state = self.engine.state();
        let eligible_conversations = state
            .conversations
            .values()
            .filter(|conversation| {
                self.actor_can_see_conversation(actor_id, conversation)
                    && self.actor_can_send_to_recipient(actor_id, conversation, requirements)
            })
            .map(|conversation| conversation.id.clone())
            .collect::<BTreeSet<_>>();
        let mut index = SearchIndex::default();
        for conversation in state
            .conversations
            .values()
            .filter(|conversation| eligible_conversations.contains(&conversation.id))
            .cloned()
        {
            index.index_conversation(conversation);
        }
        let results = index
            .search(&query)
            .into_iter()
            .filter(|result| matches!(result.kind, SearchResultKind::Conversation))
            .collect();
        ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(self.cursor.to_string()),
            server_time_ms,
            event: ServerEvent::SearchResults { query, results },
        }
    }

    pub(crate) fn actor_can_send_to_recipient(
        &self,
        actor_id: &ActorId,
        conversation: &Conversation,
        requirements: RecipientSearchRequirements,
    ) -> bool {
        let state = self.engine.state();
        let sender_is_participant = conversation
            .participants
            .iter()
            .any(|participant| &participant.actor_id == actor_id)
            || conversation.owner_id.as_ref() == Some(actor_id);
        let community_member = state
            .communities
            .get(&conversation.id)
            .and_then(|community| community.members.get(actor_id));
        let restricted = community_member
            .filter(|member| matches!(member.status, MemberStatus::Restricted));
        let channel_posting_allowed = if matches!(conversation.kind, ConversationKind::Channel) {
            conversation.owner_id.as_ref() == Some(actor_id)
                || conversation.participants.iter().any(|participant| {
                    &participant.actor_id == actor_id
                        && matches!(
                            participant.role,
                            ParticipantRole::Owner | ParticipantRole::Admin
                        )
                })
                || community_member.is_some_and(|member| {
                    matches!(member.status, MemberStatus::Administrator)
                        && member.admin_rights.post_messages
                })
        } else {
            true
        };

        recipient_search_authorized(
            RecipientAuthorizationInput {
                conversation_kind: conversation.kind,
                sender_is_participant,
                can_send_messages: conversation.permissions.can_send_messages,
                can_send_media: conversation.permissions.can_send_media,
                can_send_polls: conversation.permissions.can_send_polls,
                can_send_other: conversation.permissions.can_send_other,
                can_send_inline: conversation.permissions.can_send_inline,
                can_send_games: conversation.permissions.can_send_games,
                community_forbidden: community_member.is_some_and(|member| {
                    matches!(member.status, MemberStatus::Left | MemberStatus::Banned)
                }),
                community_restrict_messages: restricted
                    .is_some_and(|member| member.restrictions.send_messages),
                community_restrict_media: restricted
                    .is_some_and(|member| member.restrictions.send_media),
                community_restrict_polls: restricted
                    .is_some_and(|member| member.restrictions.send_polls),
                community_restrict_other: restricted
                    .is_some_and(|member| member.restrictions.send_other),
                community_restrict_inline: restricted
                    .is_some_and(|member| member.restrictions.send_inline),
                community_restrict_games: restricted
                    .is_some_and(|member| member.restrictions.send_games),
                channel_posting_allowed,
            },
            requirements,
        )
    }

    pub(crate) fn community_members_page(
        &self,
        actor_id: &ActorId,
        conversation_id: ConversationId,
        cursor: Option<&str>,
        limit: u32,
        server_time_ms: i64,
    ) -> Result<ServerEnvelope, MessagingServiceError> {
        let community = self.authorized_community(actor_id, &conversation_id)?;
        let (members, next_cursor) = community.member_page(cursor, limit as usize);
        Ok(ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(self.cursor.to_string()),
            server_time_ms,
            event: ServerEvent::CommunityMembersPage {
                conversation_id,
                members,
                next_cursor,
            },
        })
    }

    pub(crate) fn community_audit_page(
        &self,
        actor_id: &ActorId,
        conversation_id: ConversationId,
        cursor: Option<&str>,
        limit: u32,
        server_time_ms: i64,
    ) -> Result<ServerEnvelope, MessagingServiceError> {
        let community = self.authorized_community(actor_id, &conversation_id)?;
        let is_admin = community.members.get(actor_id).is_some_and(|member| {
            matches!(
                member.status,
                MemberStatus::Owner | MemberStatus::Administrator
            )
        });
        if !is_admin {
            return Err(MessagingServiceError::UnauthorizedCommand(
                "community audit log requires owner/admin access".into(),
            ));
        }
        let (entries, next_cursor) = community.audit_page(cursor, limit as usize);
        Ok(ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(self.cursor.to_string()),
            server_time_ms,
            event: ServerEvent::CommunityAuditLogPage {
                conversation_id,
                entries,
                next_cursor,
            },
        })
    }

    pub(crate) fn authorized_community(
        &self,
        actor_id: &ActorId,
        conversation_id: &ConversationId,
    ) -> Result<&CommunityState, MessagingServiceError> {
        let denied = || {
            MessagingServiceError::UnauthorizedCommand(
                "community operation requires membership or channel subscription".into(),
            )
        };
        let community = self
            .engine
            .state()
            .communities
            .get(conversation_id)
            .ok_or_else(denied)?;
        let has_access = community.members.get(actor_id).is_some_and(|member| {
            !matches!(member.status, MemberStatus::Left | MemberStatus::Banned)
        }) || community.is_subscriber(actor_id)
            || self
                .engine
                .state()
                .conversations
                .get(conversation_id)
                .and_then(|conversation| conversation.owner_id.as_ref())
                .is_some_and(|owner_id| owner_id == actor_id);
        if !has_access {
            return Err(denied());
        }
        Ok(community)
    }


    pub(crate) fn wallet_status_envelope(&self, actor_id: &ActorId, server_time_ms: i64) -> ServerEnvelope {
        let account_id = WalletAccountId(format!("wallet:{}", actor_id.0));
        let account = self
            .engine
            .state()
            .wallet
            .accounts
            .get(&account_id)
            .cloned();
        let mut recent_entries = self
            .engine
            .state()
            .wallet
            .entries
            .values()
            .filter(|entry| {
                entry.from_account_id.as_ref() == Some(&account_id)
                    || entry.to_account_id.as_ref() == Some(&account_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        recent_entries.sort_by_key(|entry| std::cmp::Reverse(entry.created_at_ms));
        recent_entries.truncate(50);
        ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(self.cursor.to_string()),
            server_time_ms,
            event: ServerEvent::WalletStatus {
                account,
                recent_entries,
            },
        }
    }


    pub(crate) fn actor_can_see_conversation(&self, actor_id: &ActorId, conversation: &Conversation) -> bool {
        if let Some(community) = self.engine.state().communities.get(&conversation.id) {
            return conversation.owner_id.as_ref() == Some(actor_id)
                || community.members.get(actor_id).is_some_and(|member| {
                    !matches!(member.status, MemberStatus::Left | MemberStatus::Banned)
                })
                || (matches!(conversation.kind, ConversationKind::Channel)
                    && community.is_subscriber(actor_id));
        }
        conversation.owner_id.as_ref() == Some(actor_id)
            || conversation
                .participants
                .iter()
                .any(|participant| &participant.actor_id == actor_id)
    }

}
