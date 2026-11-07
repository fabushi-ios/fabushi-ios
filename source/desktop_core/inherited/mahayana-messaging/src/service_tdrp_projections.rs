use crate::actor::ActorId;
use crate::community::{
    CommunityState, MemberStatus,
};
use crate::conversation::{
    Conversation, ConversationDestination, ConversationId,
    Topic,
};
use crate::engine::topic_id_from_root;
use crate::engine::Event;
use crate::message::{
    Message, MessageContent,
};
use crate::protocol::{
    ServerEnvelope, ServerEvent, FABUSHI_MESSAGING_PROTOCOL_VERSION,
};
use crate::service::MessagingService;
use crate::store::MessagingStateStore;

impl<S: MessagingStateStore> MessagingService<S> {
    pub(crate) fn project_conversation_for_actor(
        &self,
        actor_id: &ActorId,
        conversation: &Conversation,
        server_time_ms: i64,
    ) -> Conversation {
        let state = self.engine.state();
        let mut projected = conversation.clone();
        let last_read = state
            .read_cursors
            .get(&conversation.id)
            .and_then(|cursors| cursors.get(actor_id));
        projected.last_read_message_id = last_read.map(|message_id| message_id.0.clone());

        let mut ordered_messages = state
            .messages
            .get(&conversation.id)
            .into_iter()
            .flat_map(|messages| messages.values())
            .collect::<Vec<_>>();
        ordered_messages.sort_by(|left, right| {
            left.created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        let read_index = last_read.and_then(|message_id| {
            ordered_messages
                .iter()
                .position(|message| &message.id == message_id)
        });
        let unread = ordered_messages
            .iter()
            .enumerate()
            .filter(|(index, message)| {
                let after_read = match read_index {
                    Some(read_index) => *index > read_index,
                    None => true,
                };
                let scheduled_is_visible = match message.scheduled_at_ms {
                    Some(scheduled_at_ms) => scheduled_at_ms <= server_time_ms,
                    None => true,
                };
                after_read
                    && scheduled_is_visible
                    && !message.deleted
                    && &message.sender_id != actor_id
            })
            .count();
        projected.unread_count = u32::try_from(unread).unwrap_or(u32::MAX);
        projected.marked_unread = state
            .marked_unread_by_actor
            .get(&conversation.id)
            .is_some_and(|actors| actors.contains(actor_id));
        if let Some(community) = state.communities.get(&conversation.id) {
            projected.topics = community
                .topics
                .values()
                .map(|topic| Topic {
                    id: topic.id.clone(),
                    title: topic.title.clone(),
                    icon: topic.icon.clone(),
                    created_by: topic.creator_id.clone(),
                    closed: topic.closed,
                    hidden: topic.hidden,
                    unread_count: 0,
                })
                .collect();
            for topic in &mut projected.topics {
                topic.unread_count =
                    self.topic_unread_count(actor_id, &conversation.id, &topic.id, server_time_ms);
            }
        }
        projected
    }

    pub(crate) fn topic_unread_count(
        &self,
        actor_id: &ActorId,
        conversation_id: &ConversationId,
        topic_id: &str,
        server_time_ms: i64,
    ) -> u32 {
        let state = self.engine.state();
        let destination = ConversationDestination::topic(
            conversation_id.clone(),
            format!("topic:{topic_id}"),
        );
        let typed_cursor = state
            .conversation_child_states
            .iter()
            .find(|child| child.destination == destination && &child.actor_id == actor_id)
            .and_then(|child| child.inbox_read_till.as_ref())
            .map(|position| position.message_id.as_str());
        let legacy_cursor = state
            .topic_read_cursors
            .get(conversation_id)
            .and_then(|by_actor| by_actor.get(actor_id))
            .and_then(|by_topic| by_topic.get(topic_id))
            .map(|message_id| message_id.0.as_str());
        let cursor_message_id = typed_cursor.or(legacy_cursor);
        let mut messages = state
            .messages
            .get(conversation_id)
            .into_iter()
            .flat_map(|messages| messages.values())
            .collect::<Vec<_>>();
        messages.sort_by(|left, right| {
            left.created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        let read_index = cursor_message_id.and_then(|message_id| {
            messages
                .iter()
                .position(|message| message.id.0 == message_id)
        });
        let unread = messages
            .iter()
            .enumerate()
            .filter(|(index, message)| {
                let after_read = match read_index {
                    Some(read_index) => *index > read_index,
                    None => true,
                };
                after_read
                    && message
                        .thread_root_message_id
                        .as_ref()
                        .is_some_and(|root| topic_id_from_root(root) == Some(topic_id))
                    && message
                        .scheduled_at_ms
                        .is_none_or(|scheduled| scheduled <= server_time_ms)
                    && !message.deleted
                    && &message.sender_id != actor_id
            })
            .count();
        u32::try_from(unread).unwrap_or(u32::MAX)
    }

    pub(crate) fn project_community_for_actor(
        &self,
        actor_id: &ActorId,
        community: &CommunityState,
        server_time_ms: i64,
    ) -> CommunityState {
        let mut projected = community.clone();
        let member = community.members.get(actor_id);
        let is_admin = member.is_some_and(|member| {
            matches!(
                member.status,
                MemberStatus::Owner | MemberStatus::Administrator
            )
        });
        let can_invite = member.is_some_and(|member| {
            matches!(member.status, MemberStatus::Owner)
                || (matches!(member.status, MemberStatus::Administrator)
                    && member.admin_rights.invite_members)
        });
        if !is_admin {
            projected.pending_join_requests.clear();
            projected.admin_log.clear();
        }
        if !can_invite {
            projected.pending_join_requests.clear();
            for invite in projected.invite_links.values_mut() {
                // Invite tokens are bearer credentials and must never be included in a
                // regular member/subscriber sync payload.
                invite.token.clear();
            }
        }
        for topic in projected.topics.values_mut() {
            topic.unread_count = self.topic_unread_count(
                actor_id,
                &community.conversation_id,
                &topic.id,
                server_time_ms,
            );
        }
        projected
    }

    pub(crate) fn project_message_for_actor(&self, actor_id: &ActorId, message: &Message) -> Message {
        let mut projected = message.clone();
        if let MessageContent::Poll { options, .. } = &mut projected.content {
            let selected = self
                .engine
                .state()
                .poll_votes
                .get(&message.conversation_id)
                .and_then(|messages| messages.get(&message.id))
                .and_then(|actors| actors.get(actor_id));
            for option in options {
                option.chosen = selected.is_some_and(|ids| ids.contains(&option.id));
            }
        }
        projected
    }


    pub(crate) fn project_event(
        &self,
        actor_id: &ActorId,
        event: Event,
        server_time_ms: i64,
    ) -> Option<ServerEnvelope> {
        let server_event = match event {
            Event::ActorUpserted { actor } => ServerEvent::ActorChanged { actor },
            Event::PrivacySettingsUpdated { .. } => return None,
            Event::PresenceUpdated { actor_id, presence } => {
                ServerEvent::PresenceChanged { actor_id, presence }
            }
            Event::ConversationUpserted { conversation } => ServerEvent::ConversationChanged {
                conversation: self.project_conversation_for_actor(
                    actor_id,
                    &conversation,
                    server_time_ms,
                ),
            },
            Event::ConversationInfoUpdated {
                conversation_id, ..
            } => self
                .engine
                .state()
                .conversations
                .get(&conversation_id)
                .cloned()
                .map(|conversation| ServerEvent::ConversationChanged {
                    conversation: self.project_conversation_for_actor(
                        actor_id,
                        &conversation,
                        server_time_ms,
                    ),
                })?,
            Event::ConversationParticipantUpserted {
                conversation_id, ..
            } => self
                .engine
                .state()
                .conversations
                .get(&conversation_id)
                .cloned()
                .map(|conversation| ServerEvent::ConversationParticipantChanged {
                    conversation: self.project_conversation_for_actor(
                        actor_id,
                        &conversation,
                        server_time_ms,
                    ),
                    removed_actor_id: None,
                })?,
            Event::ConversationParticipantRemoved {
                conversation_id,
                actor_id: removed_actor_id,
            } => self
                .engine
                .state()
                .conversations
                .get(&conversation_id)
                .cloned()
                .map(|conversation| ServerEvent::ConversationParticipantChanged {
                    conversation: self.project_conversation_for_actor(
                        actor_id,
                        &conversation,
                        server_time_ms,
                    ),
                    removed_actor_id: Some(removed_actor_id),
                })?,
            Event::ConversationArchived {
                conversation_id, ..
            }
            | Event::ConversationPinned {
                conversation_id, ..
            }
            | Event::ConversationNotificationsUpdated {
                conversation_id, ..
            } => self
                .engine
                .state()
                .conversations
                .get(&conversation_id)
                .cloned()
                .map(|conversation| ServerEvent::ConversationChanged {
                    conversation: self.project_conversation_for_actor(
                        actor_id,
                        &conversation,
                        server_time_ms,
                    ),
                })?,
            Event::ConversationMarkedUnread {
                conversation_id,
                actor_id,
                marked_unread,
            } => ServerEvent::MarkedUnreadChanged {
                conversation_id,
                actor_id,
                marked_unread,
            },
            Event::DraftChanged { draft } => ServerEvent::DraftChanged { draft },
            Event::FolderUpserted { folder } => ServerEvent::FolderChanged { folder },
            Event::FolderDeleted { folder_id } => ServerEvent::FolderDeleted { folder_id },
            Event::PresenceTriggeredSendQueued { pending } => {
                ServerEvent::PresenceTriggeredSendChanged {
                    client_message_id: pending.client_message_id.clone(),
                    pending: Some(pending),
                }
            }
            Event::PresenceTriggeredSendRemoved { client_message_id } => {
                ServerEvent::PresenceTriggeredSendChanged {
                    client_message_id,
                    pending: None,
                }
            }
            Event::MessageQueued { message } => ServerEvent::MessageAdded { message },
            Event::MessageAcknowledged {
                conversation_id,
                server_message_id,
                ..
            }
            | Event::DeliveryStateUpdated {
                conversation_id,
                message_id: server_message_id,
                ..
            }
            | Event::MessageEdited {
                conversation_id,
                message_id: server_message_id,
                ..
            }
            | Event::ReactionUpdated {
                conversation_id,
                message_id: server_message_id,
                ..
            }
            | Event::MessagePinned {
                conversation_id,
                message_id: server_message_id,
                ..
            }
            | Event::PollVoteChanged {
                conversation_id,
                message_id: server_message_id,
                ..
            } => self
                .engine
                .state()
                .messages
                .get(&conversation_id)
                .and_then(|messages| messages.get(&server_message_id))
                .cloned()
                .map(|message| ServerEvent::MessageChanged { message })?,
            Event::MessagesDeleted {
                conversation_id,
                message_ids,
            } => ServerEvent::MessagesDeleted {
                conversation_id,
                message_ids,
            },
            Event::ConversationRead {
                conversation_id,
                actor_id,
                message_id,
            } => ServerEvent::ReadChanged {
                conversation_id,
                actor_id,
                message_id,
            },
            Event::TopicReadChanged {
                conversation_id,
                topic_id,
                actor_id,
                message_id,
            } => ServerEvent::TopicReadChanged {
                conversation_id,
                topic_id,
                actor_id,
                message_id,
            },
            Event::TopicDraftChanged { draft } => ServerEvent::TopicDraftChanged { draft },
            Event::ConversationChildReadChanged { .. }
            | Event::ConversationChildDraftChanged { .. }
            | Event::ConversationChildWindowReplaced { .. }
            | Event::ConversationChildPinnedChanged { .. }
            | Event::ConversationChildActiveChanged { .. }
            | Event::ConversationChildMarkedUnreadChanged { .. }
            | Event::ConversationChildNoPaidMessagesChanged { .. }
            | Event::ConversationChildDestroyed { .. } => return None,
            Event::InvoiceCreated { invoice } => ServerEvent::InvoiceChanged { invoice },
            Event::OrderUpserted { order } => ServerEvent::OrderChanged { order },
            Event::WalletChanged { .. } => return None,
            Event::StoryChanged { story } => ServerEvent::StoryChanged { story },
            Event::StoryDeleted { story_id } => ServerEvent::StoryDeleted { story_id },
            Event::CommunityChanged { community } => ServerEvent::CommunityChanged {
                community: self.project_community_for_actor(actor_id, &community, server_time_ms),
            },
            Event::BotRegistryChanged {
                profile, execution, ..
            } => ServerEvent::BotChanged { profile, execution },
            Event::MiniAppInstalled { manifest } => ServerEvent::MiniAppChanged { manifest },
            Event::MiniAppGrantUpdated { .. } => return None,
            Event::MiniAppOpened { session } => ServerEvent::MiniAppOpened { session },
            Event::MiniAppResponded {
                session_id,
                request_id,
                response,
            } => ServerEvent::MiniAppResult {
                session_id,
                request_id,
                response,
            },
        };
        Some(ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(self.cursor.to_string()),
            server_time_ms,
            event: server_event,
        })
    }


    pub(crate) fn public_journal_envelope(&self, response: &ServerEnvelope) -> ServerEnvelope {
        let mut envelope = response.clone();
        if let ServerEvent::CommunityChanged { community } = &mut envelope.event {
            // Journal entries are shared by every audience member. Keep bearer invite
            // tokens and private moderation history out of the replayable copy; the direct
            // response and actor-scoped snapshot remain available to authorized admins.
            community.admin_log.clear();
            community.pending_join_requests.clear();
            for topic in community.topics.values_mut() {
                topic.unread_count = 0;
            }
            for invite in community.invite_links.values_mut() {
                invite.token.clear();
            }
        }
        envelope
    }

    pub(crate) fn project_journal_envelope_for_actor(
        &self,
        actor_id: &ActorId,
        envelope: &ServerEnvelope,
        server_time_ms: i64,
    ) -> ServerEnvelope {
        if let ServerEvent::CommunityChanged { community } = &envelope.event {
            return ServerEnvelope {
                protocol_version: envelope.protocol_version,
                cursor: envelope.cursor.clone(),
                server_time_ms: envelope.server_time_ms,
                event: ServerEvent::CommunityChanged {
                    community: self.project_community_for_actor(
                        actor_id,
                        community,
                        server_time_ms,
                    ),
                },
            };
        }
        self.public_journal_envelope(envelope)
    }

}
