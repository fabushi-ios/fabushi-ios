use std::collections::BTreeSet;

use crate::actor::{ActorId, ActorKind, ParticipantRole, PresenceStatus};
use crate::conversation::{
    Conversation, ConversationChildIdentity, ConversationDestination, ConversationId,
    ConversationKind,
};
use crate::engine::{EngineError, Event, MessagingEngine};
use crate::message::{
    resolve_send_menu_details_from_inputs, ClientMessageId, DeliveryState,
    DestinationSendMenuInput, Message, MessageId, SendMenuDetails,
};
use crate::payment::{
    resolve_paid_restriction_from_capabilities, PaidSendApprovalConsumer, PaidSendPlan,
    PaidSendRestrictionState, PaidSendValidationError,
};
use crate::privacy::PrivacyKey;

pub fn destination_message_conversation_id(
    destination: &ConversationDestination,
) -> &ConversationId {
    match destination.child.as_ref() {
        Some(ConversationChildIdentity::Conversation { conversation_id }) => conversation_id,
        _ => &destination.conversation_id,
    }
}

pub fn is_self_saved_messages(conversation: &Conversation, actor_id: &ActorId) -> bool {
    matches!(conversation.kind, ConversationKind::SavedMessages)
        && (conversation.owner_id.as_ref() == Some(actor_id)
            || conversation.participants.iter().any(|participant| {
                &participant.actor_id == actor_id
                    && matches!(participant.role, ParticipantRole::Owner)
            }))
}

impl MessagingEngine {
    /// Resolves the canonical `SavedSublist` participant for a message stored in a
    /// `SavedMessages` parent conversation (`TDRP-R9-SHARE-THREAD-DESTINATION-001` / `SB-02`):
    /// - Messages forwarded with preserved origin (`forward_origin.is_some()`) belong to
    ///   the root origin message's `sender_id` (walked up to 8 hops).
    /// - Direct self-notes or messages forwarded with `drop_sender_names: true`
    ///   (`forward_origin.is_none()`) belong to `message.sender_id` (the self sublist).
    pub fn saved_sublist_participant_for_message(
        &self,
        parent_conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> Option<ActorId> {
        let conversation = self.state.conversations.get(parent_conversation_id)?;
        if !matches!(conversation.kind, ConversationKind::SavedMessages) {
            return None;
        }
        let message = self
            .state
            .messages
            .get(parent_conversation_id)?
            .get(message_id)?;
        if message.deleted {
            return None;
        }
        let Some(mut origin) = message.forward_origin.as_deref() else {
            return Some(message.sender_id.clone());
        };
        let mut resolved_sender: Option<ActorId> = None;
        for _ in 0..8 {
            let source_msg = self
                .state
                .messages
                .iter()
                .find_map(|(conv_id, msgs)| {
                    let msg_raw = origin
                        .strip_prefix(conv_id.0.as_str())?
                        .strip_prefix(':')?;
                    msgs.get(&MessageId(msg_raw.to_string()))
                });
            let Some(source_msg) = source_msg else {
                break;
            };
            resolved_sender = Some(source_msg.sender_id.clone());
            match source_msg.forward_origin.as_deref() {
                Some(next_origin) if !next_origin.is_empty() && next_origin != origin => {
                    origin = next_origin;
                }
                _ => break,
            }
        }
        resolved_sender
    }

    pub fn can_schedule_when_online(
        &self,
        sender_id: &ActorId,
        conversation_id: &ConversationId,
    ) -> Option<ActorId> {
        let sender = self.state.actors.get(sender_id)?;
        let conversation = self.state.conversations.get(conversation_id)?;
        if !matches!(conversation.kind, ConversationKind::Direct) {
            return None;
        }
        let sender_is_participant = conversation
            .participants
            .iter()
            .any(|participant| &participant.actor_id == sender_id)
            || conversation.owner_id.as_ref() == Some(sender_id);
        if !sender_is_participant || !conversation.permissions.can_send_messages {
            return None;
        }
        let mut peer_ids = conversation
            .participants
            .iter()
            .map(|participant| &participant.actor_id)
            .chain(conversation.owner_id.iter())
            .filter(|actor_id| *actor_id != sender_id)
            .collect::<BTreeSet<_>>();
        if peer_ids.len() != 1 {
            return None;
        }
        let target_actor_id = peer_ids.pop_first()?;
        let target = self.state.actors.get(target_actor_id)?;
        if target.kind != ActorKind::Human || target.presence.status == PresenceStatus::Online {
            return None;
        }
        if target.id.0.starts_with("service:")
            || target.capabilities.iter().any(|cap| {
                matches!(
                    cap.as_str(),
                    "service_notifications"
                        | "notifications_service"
                        | "paid_messages_required"
                        | "stars_per_message_required"
                )
            })
        {
            return None;
        }
        if let Some(target_privacy) = self.state.privacy.get(target_actor_id) {
            let target_considers_sender_contact = target
                .capabilities
                .iter()
                .any(|cap| cap == &format!("contact:{}", sender_id.0));
            let sender_considers_target_contact = sender
                .capabilities
                .iter()
                .any(|cap| cap == &format!("contact:{}", target_actor_id.0));
            if !target_privacy.can_receive_message_from(
                sender_id,
                target_considers_sender_contact,
                target_considers_sender_contact && sender_considers_target_contact,
            ) {
                return None;
            }
            if let Some(last_seen_rule) = target_privacy.rules.get(&PrivacyKey::LastSeen) {
                if !last_seen_rule.allows(sender_id, target_considers_sender_contact) {
                    return None;
                }
            }
        }
        Some(target_actor_id.clone())
    }

    pub fn resolve_send_menu_details(
        &self,
        sender_id: &ActorId,
        destination_conversation_ids: &[ConversationId],
    ) -> SendMenuDetails {
        let destinations = destination_conversation_ids
            .iter()
            .cloned()
            .map(|conversation_id| ConversationDestination {
                conversation_id,
                child: None,
            })
            .collect::<Vec<_>>();
        self.resolve_destination_send_menu_details(sender_id, &destinations)
    }

    pub fn resolve_destination_send_menu_details(
        &self,
        sender_id: &ActorId,
        destinations: &[ConversationDestination],
    ) -> SendMenuDetails {
        let sender_valid = self.state.actors.contains_key(sender_id);
        let inputs = destinations
            .iter()
            .map(|destination| {
                let effective_conversation_id = destination_message_conversation_id(destination);
                let Some(conversation) = self.state.conversations.get(effective_conversation_id)
                else {
                    return DestinationSendMenuInput::default();
                };
                let sender_is_participant = conversation
                    .participants
                    .iter()
                    .any(|participant| &participant.actor_id == sender_id)
                    || conversation.owner_id.as_ref() == Some(sender_id);
                let child_exempts_paid = self
                    .state
                    .conversation_child_states
                    .iter()
                    .find(|child| &child.destination == destination && &child.actor_id == sender_id)
                    .is_some_and(|child| child.no_paid_messages);
                let requires_paid_messages = if child_exempts_paid {
                    false
                } else {
                    let saved_sublist_requires_paid = match &destination.child {
                        Some(ConversationChildIdentity::SavedSublist { participant_id }) => self
                            .state
                            .actors
                            .get(participant_id)
                            .is_some_and(|actor| {
                                actor.capabilities.iter().any(|cap| {
                                    matches!(
                                        cap.as_str(),
                                        "paid_messages_required" | "stars_per_message_required"
                                    )
                                })
                            }),
                        _ => false,
                    };
                    let peer_requires_paid = conversation
                        .participants
                        .iter()
                        .map(|participant| &participant.actor_id)
                        .chain(conversation.owner_id.iter())
                        .filter(|actor_id| *actor_id != sender_id)
                        .any(|actor_id| {
                            self.state.actors.get(actor_id).is_some_and(|actor| {
                                actor.capabilities.iter().any(|cap| {
                                    matches!(
                                        cap.as_str(),
                                        "paid_messages_required" | "stars_per_message_required"
                                    )
                                })
                            })
                        });
                    saved_sublist_requires_paid || peer_requires_paid
                };
                DestinationSendMenuInput {
                    conversation_exists: true,
                    is_secret: matches!(conversation.kind, ConversationKind::Secret),
                    can_send_messages: conversation.permissions.can_send_messages,
                    sender_is_participant,
                    requires_paid_messages,
                    is_self_saved_messages: destination.child.is_none()
                        && is_self_saved_messages(conversation, sender_id),
                    can_schedule_when_online: destination.child.is_none()
                        && self
                            .can_schedule_when_online(sender_id, effective_conversation_id)
                            .is_some(),
                }
            })
            .collect::<Vec<_>>();
        resolve_send_menu_details_from_inputs(sender_valid, &inputs)
    }

    pub fn resolve_destination_paid_restriction(
        &self,
        sender_id: &ActorId,
        destination: &ConversationDestination,
        ignore_paid_restrictions: bool,
    ) -> PaidSendRestrictionState {
        let effective_conversation_id = destination_message_conversation_id(destination);
        let Some(conversation) = self.state.conversations.get(effective_conversation_id) else {
            return resolve_paid_restriction_from_capabilities(
                false,
                std::iter::empty(),
                ignore_paid_restrictions,
                false,
            );
        };
        let mut candidate_actors: BTreeSet<&ActorId> = conversation
            .participants
            .iter()
            .map(|participant| &participant.actor_id)
            .chain(conversation.owner_id.iter())
            .filter(|actor_id| *actor_id != sender_id)
            .collect();
        if let Some(ConversationChildIdentity::SavedSublist { participant_id }) =
            &destination.child
        {
            if participant_id != sender_id {
                candidate_actors.insert(participant_id);
            }
        }
        let child_exempts_paid = self
            .state
            .conversation_child_states
            .iter()
            .find(|child| &child.destination == destination && &child.actor_id == sender_id)
            .is_some_and(|child| child.no_paid_messages);
        let capability_sets = candidate_actors.into_iter().map(|actor_id| {
            self.state
                .actors
                .get(actor_id)
                .map(|actor| actor.capabilities.as_slice())
        });
        resolve_paid_restriction_from_capabilities(
            true,
            capability_sets,
            ignore_paid_restrictions,
            child_exempts_paid,
        )
    }

    pub fn compute_paid_send_plan(
        &self,
        sender_id: &ActorId,
        destinations: &[ConversationDestination],
        outgoing_message_count: u32,
        ignore_paid_restrictions: bool,
    ) -> Result<PaidSendPlan, PaidSendValidationError> {
        let resolved = destinations
            .iter()
            .map(|destination| {
                (
                    destination.clone(),
                    self.resolve_destination_paid_restriction(
                        sender_id,
                        destination,
                        ignore_paid_restrictions,
                    ),
                )
            })
            .collect();
        PaidSendPlan::from_resolved(resolved, outgoing_message_count)
    }

    pub fn revalidate_and_consume_paid_send(
        &self,
        sender_id: &ActorId,
        destination: &ConversationDestination,
        message_count: u32,
        ignore_paid_restrictions: bool,
        consumer: &mut PaidSendApprovalConsumer,
    ) -> Result<i64, PaidSendValidationError> {
        let live_restriction = self.resolve_destination_paid_restriction(
            sender_id,
            destination,
            ignore_paid_restrictions,
        );
        consumer.consume_for_destination(destination, live_restriction, message_count)
    }

    pub(crate) fn decide_cancel_presence_triggered_send(
        &self,
        actor_id: ActorId,
        client_message_id: ClientMessageId,
    ) -> Result<Vec<Event>, EngineError> {
        self.require_actor(&actor_id)?;
        let Some(existing) = self.state.pending_presence_sends.get(&client_message_id) else {
            return Ok(Vec::new());
        };
        if existing.sender_id != actor_id {
            return Err(EngineError::InvalidPresenceTriggeredSend);
        }
        Ok(vec![Event::PresenceTriggeredSendRemoved {
            client_message_id,
        }])
    }

    pub(crate) fn decide_fail_presence_triggered_send(
        &self,
        client_message_id: ClientMessageId,
        failed_at_ms: i64,
    ) -> Result<Vec<Event>, EngineError> {
        let Some(pending) = self
            .state
            .pending_presence_sends
            .get(&client_message_id)
            .cloned()
        else {
            return Ok(Vec::new());
        };
        let message = Message {
            id: pending.local_message_id.clone(),
            conversation_id: pending.conversation_id.clone(),
            sender_id: pending.sender_id.clone(),
            content: pending.content,
            reply_to_message_id: pending.reply_to_message_id,
            thread_root_message_id: pending.thread_root_message_id,
            forward_origin: None,
            reply_markup: None,
            reactions: Vec::new(),
            delivery_state: DeliveryState::Failed {
                code: "presence_trigger_policy_rejected".into(),
                retryable: false,
            },
            created_at_ms: failed_at_ms,
            edited_at_ms: None,
            scheduled_at_ms: None,
            silent: pending.silent,
            protected_content: pending.protected_content,
            pinned: false,
            deleted: false,
        };
        Ok(vec![
            Event::MessageQueued { message },
            Event::PresenceTriggeredSendRemoved { client_message_id },
        ])
    }

    pub(crate) fn decide_remove_presence_triggered_send(
        &self,
        client_message_id: ClientMessageId,
    ) -> Result<Vec<Event>, EngineError> {
        if self
            .state
            .pending_presence_sends
            .contains_key(&client_message_id)
        {
            Ok(vec![Event::PresenceTriggeredSendRemoved {
                client_message_id,
            }])
        } else {
            Ok(Vec::new())
        }
    }
}
