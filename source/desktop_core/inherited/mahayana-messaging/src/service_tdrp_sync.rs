use crate::actor::ActorId;
use crate::community::MemberStatus;
use crate::conversation::{
    ConversationId, ConversationKind,
};
use crate::engine::Command;
use crate::message::DeliveryState;
use crate::protocol::{
    ServerEnvelope, ServerEvent, FABUSHI_MESSAGING_PROTOCOL_VERSION,
};
use crate::service::{
    stable_message_id, MessagingService, MessagingServiceError,
};
use crate::store::{JournalEntry, MessagingStateStore};
use std::collections::BTreeSet;

impl<S: MessagingStateStore> MessagingService<S> {
    pub(crate) fn sync_response(
        &self,
        actor_id: &ActorId,
        cursor: Option<&str>,
        limit: u32,
        server_time_ms: i64,
    ) -> Result<Vec<ServerEnvelope>, MessagingServiceError> {
        let Some(requested_cursor) = cursor.and_then(|value| value.parse::<u64>().ok()) else {
            return Ok(vec![self.sync_envelope(actor_id, limit, server_time_ms)]);
        };
        if requested_cursor > self.cursor {
            return Ok(vec![self.sync_envelope(actor_id, limit, server_time_ms)]);
        }
        let Some(slice) = self.store.load_event_journal_after(
            requested_cursor,
            usize::try_from(limit.max(1)).unwrap_or(usize::MAX),
        )?
        else {
            return Ok(vec![self.sync_envelope(actor_id, limit, server_time_ms)]);
        };
        if requested_cursor < slice.floor_cursor
            || (slice.entries.is_empty() && requested_cursor < slice.current_cursor)
        {
            return Ok(vec![self.sync_envelope(actor_id, limit, server_time_ms)]);
        }
        let mut responses = slice
            .entries
            .into_iter()
            .filter(|entry| entry.audience.iter().any(|candidate| candidate == actor_id))
            .filter(|entry| match &entry.envelope.event {
                ServerEvent::TypingChanged {
                    expires_at_ms: Some(expires_at_ms),
                    ..
                } => *expires_at_ms > server_time_ms,
                _ => true,
            })
            .map(|entry| {
                self.project_journal_envelope_for_actor(actor_id, &entry.envelope, server_time_ms)
            })
            .collect::<Vec<_>>();
        responses.push(self.sync_checkpoint_envelope(slice.checkpoint_cursor, server_time_ms));
        Ok(responses)
    }

    pub(crate) fn sync_checkpoint_envelope(&self, cursor: u64, server_time_ms: i64) -> ServerEnvelope {
        ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(cursor.to_string()),
            server_time_ms,
            event: ServerEvent::SyncBatch {
                actors: Vec::new(),
                conversations: Vec::new(),
                messages: Vec::new(),
                folders: Vec::new(),
                drafts: Vec::new(),
                topic_drafts: Vec::new(),
                pending_presence_sends: Vec::new(),
                conversation_children: Vec::new(),
                invoices: Vec::new(),
                orders: Vec::new(),
                stories: Vec::new(),
                communities: Vec::new(),
                bots: Vec::new(),
                bot_executions: Vec::new(),
                mini_apps: Vec::new(),
                next_cursor: Some(cursor.to_string()),
            },
        }
    }


    pub(crate) fn sync_envelope(&self, actor_id: &ActorId, limit: u32, server_time_ms: i64) -> ServerEnvelope {
        let state = self.engine.state();
        let max_items = usize::try_from(limit.max(1)).unwrap_or(usize::MAX);
        let visible_conversation_ids = state
            .conversations
            .values()
            .filter(|conversation| self.actor_can_see_conversation(actor_id, conversation))
            .map(|conversation| conversation.id.clone())
            .collect::<BTreeSet<_>>();
        let visible_actor_ids = visible_conversation_ids
            .iter()
            .filter_map(|conversation_id| state.conversations.get(conversation_id))
            .flat_map(|conversation| {
                conversation
                    .participants
                    .iter()
                    .filter(|participant| {
                        self.actor_can_see_conversation(&participant.actor_id, conversation)
                    })
                    .map(|participant| participant.actor_id.clone())
                    .chain(
                        state
                            .communities
                            .get(&conversation.id)
                            .into_iter()
                            .flat_map(|community| {
                                community
                                    .members
                                    .values()
                                    .filter(|member| {
                                        !matches!(
                                            member.status,
                                            MemberStatus::Left | MemberStatus::Banned
                                        )
                                    })
                                    .map(|member| member.actor_id.clone())
                                    .chain(community.subscribers.keys().cloned())
                            }),
                    )
            })
            .chain(std::iter::once(actor_id.clone()))
            .collect::<BTreeSet<_>>();
        ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(self.cursor.to_string()),
            server_time_ms,
            event: ServerEvent::SyncBatch {
                // Actor/conversation metadata is the lightweight navigation index and must
                // be complete in a snapshot. Applying the message/event limit here used to
                // return only the first N contacts/conversations and then advance next_cursor
                // to the current journal position, so the omitted rows could never arrive via
                // later delta sync. Keep heavy collections bounded below, not the left-rail index.
                actors: visible_actor_ids
                    .iter()
                    .filter_map(|id| state.actors.get(id))
                    .cloned()
                    .collect(),
                conversations: visible_conversation_ids
                    .iter()
                    .filter_map(|id| state.conversations.get(id))
                    .map(|conversation| {
                        self.project_conversation_for_actor(actor_id, conversation, server_time_ms)
                    })
                    .collect(),
                messages: visible_conversation_ids
                    .iter()
                    .filter_map(|id| state.messages.get(id))
                    .flat_map(|messages| messages.values())
                    .filter(|message| {
                        !matches!(message.delivery_state, DeliveryState::Failed { .. })
                            || &message.sender_id == actor_id
                    })
                    .take(max_items)
                    .map(|message| self.project_message_for_actor(actor_id, message))
                    .collect(),
                folders: state
                    .folders
                    .values()
                    .filter(|folder| {
                        folder
                            .conversation_ids
                            .iter()
                            .any(|id| visible_conversation_ids.contains(id))
                    })
                    .take(max_items)
                    .cloned()
                    .collect(),
                drafts: visible_conversation_ids
                    .iter()
                    .filter_map(|conversation_id| state.drafts.get(conversation_id))
                    .filter_map(|by_actor| by_actor.get(actor_id))
                    .cloned()
                    .collect(),
                topic_drafts: visible_conversation_ids
                    .iter()
                    .filter_map(|conversation_id| state.topic_drafts.get(conversation_id))
                    .filter_map(|by_actor| by_actor.get(actor_id))
                    .flat_map(|by_topic| by_topic.values())
                    .cloned()
                    .collect(),
                pending_presence_sends: state
                    .pending_presence_sends
                    .values()
                    .filter(|pending| &pending.sender_id == actor_id)
                    .cloned()
                    .collect(),
                conversation_children: state
                    .conversation_child_states
                    .iter()
                    .filter(|child| {
                        &child.actor_id == actor_id
                            && visible_conversation_ids.contains(&child.destination.conversation_id)
                    })
                    .take(max_items)
                    .cloned()
                    .collect(),
                invoices: state
                    .invoices
                    .values()
                    .filter(|invoice| visible_conversation_ids.contains(&invoice.conversation_id))
                    .take(max_items)
                    .cloned()
                    .collect(),
                orders: state
                    .orders
                    .values()
                    .filter(|order| {
                        &order.buyer_id == actor_id
                            || state
                                .invoices
                                .get(&order.invoice_id)
                                .is_some_and(|invoice| &invoice.seller_id == actor_id)
                    })
                    .take(max_items)
                    .cloned()
                    .collect(),
                stories: state
                    .stories
                    .values()
                    .filter(|story| {
                        let (is_contact, is_close_friend) =
                            self.engine.story_visibility_flags(&story.owner_id, actor_id);
                        (story.pinned_to_profile || story.expires_at_ms > server_time_ms)
                            && story.is_visible_to(actor_id, is_contact, is_close_friend)
                    })
                    .take(max_items)
                    .cloned()
                    .collect(),
                communities: visible_conversation_ids
                    .iter()
                    .filter_map(|id| state.communities.get(id))
                    .take(max_items)
                    .map(|community| {
                        self.project_community_for_actor(actor_id, community, server_time_ms)
                    })
                    .collect(),
                bots: state
                    .bots
                    .bots
                    .values()
                    .filter(|profile| visible_actor_ids.contains(&profile.actor_id))
                    .take(max_items)
                    .cloned()
                    .collect(),
                bot_executions: state
                    .bots
                    .executions
                    .values()
                    .filter(|execution| {
                        &execution.bot_id == actor_id
                            || visible_actor_ids.contains(&execution.bot_id)
                    })
                    .take(max_items)
                    .cloned()
                    .collect(),
                mini_apps: state.mini_apps.values().take(max_items).cloned().collect(),
                next_cursor: Some(self.cursor.to_string()),
            },
        }
    }

    pub(crate) fn mark_direct_messages_delivered(
        &mut self,
        actor_id: &ActorId,
        server_time_ms: i64,
    ) -> Result<(), MessagingServiceError> {
        let pending = self
            .engine
            .state()
            .conversations
            .values()
            .filter(|conversation| {
                matches!(
                    conversation.kind,
                    ConversationKind::Direct | ConversationKind::Secret
                ) && conversation
                    .participants
                    .iter()
                    .any(|participant| &participant.actor_id == actor_id)
            })
            .flat_map(|conversation| {
                self.engine
                    .state()
                    .messages
                    .get(&conversation.id)
                    .into_iter()
                    .flat_map(|messages| messages.values())
                    .filter(|message| {
                        &message.sender_id != actor_id
                            && message.delivery_state == DeliveryState::Sent
                    })
                    .map(|message| (conversation.id.clone(), message.id.clone()))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Ok(());
        }
        let mut events = Vec::new();
        for (conversation_id, message_id) in pending {
            events.extend(self.engine.execute(Command::SetDeliveryState {
                conversation_id,
                message_id,
                state: DeliveryState::Delivered,
            })?);
        }
        self.cursor = self.cursor.saturating_add(events.len() as u64);
        let responses = events
            .into_iter()
            .filter_map(|event| self.project_event(actor_id, event, server_time_ms))
            .collect::<Vec<_>>();
        let journal = self.journal_entries(actor_id, &responses);
        self.persist_with_events(server_time_ms, &journal)?;
        Ok(())
    }


    pub(crate) fn journal_entries(
        &self,
        initiator: &ActorId,
        responses: &[ServerEnvelope],
    ) -> Vec<JournalEntry> {
        responses
            .iter()
            .filter(|response| !matches!(&response.event, ServerEvent::SyncBatch { .. }))
            .map(|response| JournalEntry {
                envelope: match &response.event {
                    ServerEvent::CommunityChanged { community } => {
                        let canonical = self
                            .engine
                            .state()
                            .communities
                            .get(&community.conversation_id)
                            .cloned()
                            .unwrap_or_else(|| community.clone());
                        self.public_journal_envelope(&ServerEnvelope {
                            protocol_version: response.protocol_version,
                            cursor: response.cursor.clone(),
                            server_time_ms: response.server_time_ms,
                            event: ServerEvent::CommunityChanged {
                                community: canonical,
                            },
                        })
                    }
                    _ => self.public_journal_envelope(response),
                },
                audience: self.event_audience(initiator, &response.event),
            })
            .collect()
    }


    pub(crate) fn event_audience(&self, initiator: &ActorId, event: &ServerEvent) -> Vec<ActorId> {
        let mut audience = BTreeSet::from([initiator.clone()]);
        match event {
            ServerEvent::ActorChanged { actor } => {
                audience.insert(actor.id.clone());
                for conversation in
                    self.engine
                        .state()
                        .conversations
                        .values()
                        .filter(|conversation| {
                            conversation
                                .participants
                                .iter()
                                .any(|participant| participant.actor_id == actor.id)
                        })
                {
                    Self::extend_conversation_audience(&mut audience, conversation);
                }
            }
            ServerEvent::PresenceTriggeredSendChanged {
                client_message_id,
                pending,
            } => {
                audience.clear();
                if let Some(pending) = pending {
                    audience.insert(pending.sender_id.clone());
                } else if let Some(sender_id) = self
                    .engine
                    .state()
                    .messages
                    .values()
                    .flat_map(|messages| messages.values())
                    .find(|message| {
                        message.id == stable_message_id(&message.sender_id, client_message_id)
                    })
                    .map(|message| message.sender_id.clone())
                {
                    audience.insert(sender_id);
                } else {
                    audience.insert(initiator.clone());
                }
            }
            ServerEvent::PresenceChanged { actor_id, .. } => {
                audience.insert(actor_id.clone());
                for conversation in
                    self.engine
                        .state()
                        .conversations
                        .values()
                        .filter(|conversation| {
                            conversation
                                .participants
                                .iter()
                                .any(|participant| &participant.actor_id == actor_id)
                        })
                {
                    Self::extend_conversation_audience(&mut audience, conversation);
                }
            }
            ServerEvent::ConversationChanged { conversation } => {
                Self::extend_conversation_audience(&mut audience, conversation);
            }
            ServerEvent::ConversationParticipantChanged {
                conversation,
                removed_actor_id,
            } => {
                Self::extend_conversation_audience(&mut audience, conversation);
                if let Some(actor_id) = removed_actor_id {
                    audience.insert(actor_id.clone());
                }
            }
            ServerEvent::MarkedUnreadChanged { actor_id, .. } => {
                audience.clear();
                audience.insert(actor_id.clone());
            }
            ServerEvent::DraftChanged { draft } => {
                audience.clear();
                audience.insert(draft.actor_id.clone());
            }
            ServerEvent::TopicDraftChanged { draft } => {
                audience.clear();
                audience.insert(draft.actor_id.clone());
            }
            ServerEvent::MessageAdded { message } | ServerEvent::MessageChanged { message } => {
                if matches!(message.delivery_state, DeliveryState::Failed { .. }) {
                    audience.clear();
                    audience.insert(message.sender_id.clone());
                } else {
                    self.extend_conversation_id_audience(&mut audience, &message.conversation_id);
                }
            }
            ServerEvent::MessagesDeleted {
                conversation_id, ..
            }
            | ServerEvent::ReadChanged {
                conversation_id, ..
            }
            | ServerEvent::TypingChanged {
                conversation_id, ..
            } => {
                self.extend_conversation_id_audience(&mut audience, conversation_id);
            }
            ServerEvent::TopicReadChanged { actor_id, .. } => {
                audience.clear();
                audience.insert(actor_id.clone());
            }
            ServerEvent::InvoiceChanged { invoice } => {
                self.extend_conversation_id_audience(&mut audience, &invoice.conversation_id);
            }
            ServerEvent::OrderChanged { order } => {
                audience.insert(order.buyer_id.clone());
                if let Some(invoice) = self.engine.state().invoices.get(&order.invoice_id) {
                    audience.insert(invoice.seller_id.clone());
                }
            }
            ServerEvent::StoryChanged { story } => {
                audience.insert(story.owner_id.clone());
                for candidate_id in self.engine.state().actors.keys() {
                    let (is_contact, is_close_friend) =
                        self.engine.story_visibility_flags(&story.owner_id, candidate_id);
                    if story.is_visible_to(candidate_id, is_contact, is_close_friend) {
                        audience.insert(candidate_id.clone());
                    }
                }
            }
            ServerEvent::CommunityChanged { community } => {
                self.extend_conversation_id_audience(&mut audience, &community.conversation_id);
            }
            ServerEvent::BotChanged { profile, execution } => {
                if let Some(profile) = profile {
                    audience.insert(profile.actor_id.clone());
                }
                if let Some(execution) = execution {
                    audience.insert(execution.bot_id.clone());
                }
            }
            ServerEvent::BotInvocationRequested { invocation } => {
                audience.insert(invocation.sender_id.clone());
                audience.insert(invocation.bot_id.clone());
                self.extend_conversation_id_audience(&mut audience, &invocation.conversation_id);
            }
            ServerEvent::MiniAppOpened { session } => {
                audience.insert(session.actor_id.clone());
            }
            ServerEvent::MiniAppResult { session_id, .. } => {
                if let Some(session) = self.engine.state().mini_app_sessions.get(session_id) {
                    audience.insert(session.actor_id.clone());
                }
            }
            ServerEvent::SyncBatch { .. }
            | ServerEvent::SearchResults { .. }
            | ServerEvent::FolderChanged { .. }
            | ServerEvent::FolderDeleted { .. }
            | ServerEvent::BlobUploadChanged { .. }
            | ServerEvent::BlobReady { .. }
            | ServerEvent::BlobDeleted { .. }
            | ServerEvent::WalletStatus { .. }
            | ServerEvent::StoryDeleted { .. }
            | ServerEvent::MiniAppChanged { .. }
            | ServerEvent::CommunityMembersPage { .. }
            | ServerEvent::CommunityAuditLogPage { .. }
            | ServerEvent::Error { .. } => {}
        }
        audience.into_iter().collect()
    }

    pub(crate) fn extend_conversation_id_audience(
        &self,
        audience: &mut BTreeSet<ActorId>,
        conversation_id: &ConversationId,
    ) {
        if let Some(conversation) = self.engine.state().conversations.get(conversation_id) {
            if let Some(community) = self.engine.state().communities.get(conversation_id) {
                audience.extend(
                    conversation
                        .participants
                        .iter()
                        .filter(|participant| {
                            conversation.owner_id.as_ref() == Some(&participant.actor_id)
                                || community.members.get(&participant.actor_id).is_some_and(
                                    |member| {
                                        !matches!(
                                            member.status,
                                            MemberStatus::Left | MemberStatus::Banned
                                        )
                                    },
                                )
                                || (matches!(conversation.kind, ConversationKind::Channel)
                                    && community.is_subscriber(&participant.actor_id))
                        })
                        .map(|participant| participant.actor_id.clone()),
                );
                audience.extend(
                    community
                        .members
                        .values()
                        .filter(|member| {
                            !matches!(member.status, MemberStatus::Left | MemberStatus::Banned)
                        })
                        .map(|member| member.actor_id.clone()),
                );
                if matches!(conversation.kind, ConversationKind::Channel) {
                    audience.extend(community.subscribers.keys().cloned());
                }
                if let Some(owner_id) = &conversation.owner_id {
                    audience.insert(owner_id.clone());
                }
            } else {
                Self::extend_conversation_audience(audience, conversation);
            }
        }
    }

    pub(crate) fn extend_conversation_audience(
        audience: &mut BTreeSet<ActorId>,
        conversation: &crate::conversation::Conversation,
    ) {
        audience.extend(
            conversation
                .participants
                .iter()
                .map(|participant| participant.actor_id.clone()),
        );
        if let Some(owner_id) = &conversation.owner_id {
            audience.insert(owner_id.clone());
        }
    }

}
