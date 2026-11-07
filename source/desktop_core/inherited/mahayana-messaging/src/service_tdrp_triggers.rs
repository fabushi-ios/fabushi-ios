use crate::actor::{ActorId, Presence, PresenceStatus};
use crate::conversation::{
    ConversationId, ConversationKind,
};
use crate::engine::{Command, Event};
use crate::message::{
    DeliveryState, Message, MessageId,
    PresenceSendTrigger,
};
use crate::protocol::{
    ClientCommand, ServerEnvelope, ServerEvent, FABUSHI_MESSAGING_PROTOCOL_VERSION,
};
use crate::service::{
    stable_message_id, MessagingService, MessagingServiceError,
};
use crate::store::MessagingStateStore;

impl<S: MessagingStateStore> MessagingService<S> {
    pub(crate) fn set_presence_and_release_triggers(
        &mut self,
        actor_id: &ActorId,
        presence: Presence,
        server_time_ms: i64,
    ) -> Result<Vec<ServerEnvelope>, MessagingServiceError> {
        let was_online = self
            .engine
            .state()
            .actors
            .get(actor_id)
            .is_some_and(|actor| actor.presence.status == PresenceStatus::Online);
        let becomes_online = presence.status == PresenceStatus::Online;

        let mut events = self.engine.execute(Command::SetPresence {
            actor_id: actor_id.clone(),
            presence,
        })?;

        if !was_online && becomes_online {
            let pending = self
                .engine
                .state()
                .pending_presence_sends
                .values()
                .filter(|pending| {
                    matches!(
                        &pending.trigger,
                        PresenceSendTrigger::WhenParticipantOnline { actor_id: target }
                            if target == actor_id
                    )
                })
                .cloned()
                .collect::<Vec<_>>();

            for pending in pending {
                let target_privacy_allows = self
                    .engine
                    .state()
                    .privacy
                    .get(actor_id)
                    .is_none_or(|privacy| {
                        let target_is_contact = self
                            .engine
                            .state()
                            .actors
                            .get(actor_id)
                            .is_some_and(|target| {
                                target
                                    .capabilities
                                    .iter()
                                    .any(|cap| cap == &format!("contact:{}", pending.sender_id.0))
                            });
                        let sender_is_contact = self
                            .engine
                            .state()
                            .actors
                            .get(&pending.sender_id)
                            .is_some_and(|sender| {
                                sender
                                    .capabilities
                                    .iter()
                                    .any(|cap| cap == &format!("contact:{}", actor_id.0))
                            });
                        privacy.can_receive_message_from(
                            &pending.sender_id,
                            target_is_contact,
                            target_is_contact && sender_is_contact,
                        ) && privacy
                            .rules
                            .get(&crate::privacy::PrivacyKey::LastSeen)
                            .is_none_or(|rule| rule.allows(&pending.sender_id, target_is_contact))
                    });
                let queued = target_privacy_allows.then(|| {
                    self.engine.execute(Command::QueueMessage {
                        conversation_id: pending.conversation_id.clone(),
                        local_message_id: pending.local_message_id.clone(),
                        client_message_id: pending.client_message_id.clone(),
                        sender_id: pending.sender_id.clone(),
                        content: pending.content.clone(),
                        reply_to_message_id: pending.reply_to_message_id.clone(),
                        thread_root_message_id: pending.thread_root_message_id.clone(),
                        created_at_ms: server_time_ms,
                        scheduled_at_ms: None,
                        silent: pending.silent,
                        protected_content: pending.protected_content,
                    })
                });
                let Some(Ok(queued)) = queued else {
                    // Send policy or privacy changed while the trigger was pending.
                    // Settle the pending trigger into an explicit terminal Failed message
                    // and retire the pending trigger so it never hangs indefinitely.
                    events.extend(self.engine.execute(Command::FailPresenceTriggeredSend {
                        client_message_id: pending.client_message_id,
                        failed_at_ms: server_time_ms,
                    })?);
                    continue;
                };
                events.extend(queued);
                events.extend(self.engine.execute(Command::AcknowledgeMessage {
                    conversation_id: pending.conversation_id.clone(),
                    local_message_id: pending.local_message_id.clone(),
                    server_message_id: pending.local_message_id.clone(),
                    accepted_at_ms: server_time_ms,
                })?);
                events.extend(self.engine.execute(Command::RemovePresenceTriggeredSend {
                    client_message_id: pending.client_message_id,
                })?);
            }
        }

        let bot_invocations = events
            .iter()
            .filter_map(|event| match event {
                Event::MessageQueued { message }
                    if !matches!(message.delivery_state, DeliveryState::Failed { .. }) =>
                {
                    Some(message)
                }
                _ => None,
            })
            .flat_map(|message| self.bot_invocations_for_message(message))
            .collect::<Vec<_>>();

        self.cursor = self.cursor.saturating_add(events.len() as u64);
        let mut responses = events
            .into_iter()
            .filter_map(|event| self.project_event(actor_id, event, server_time_ms))
            .collect::<Vec<_>>();
        responses.extend(bot_invocations.into_iter().map(|invocation| ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(self.cursor.to_string()),
            server_time_ms,
            event: ServerEvent::BotInvocationRequested { invocation },
        }));
        let journal = self.journal_entries(actor_id, &responses);
        self.persist_with_events(server_time_ms, &journal)?;
        Ok(responses)
    }


    pub(crate) fn typing_event(
        &mut self,
        actor_id: &ActorId,
        conversation_id: ConversationId,
        action: Option<String>,
        expires_at_ms: Option<i64>,
        server_time_ms: i64,
    ) -> Result<Vec<ServerEnvelope>, MessagingServiceError> {
        let conversation = self
            .engine
            .state()
            .conversations
            .get(&conversation_id)
            .ok_or_else(|| {
                MessagingServiceError::UnauthorizedCommand(
                    "typing conversation does not exist".into(),
                )
            })?;
        if !conversation
            .participants
            .iter()
            .any(|participant| &participant.actor_id == actor_id)
        {
            return Err(MessagingServiceError::UnauthorizedCommand(
                "typing requires conversation membership".into(),
            ));
        }
        self.cursor = self.cursor.saturating_add(1);
        let response = ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(self.cursor.to_string()),
            server_time_ms,
            event: ServerEvent::TypingChanged {
                conversation_id,
                actor_id: actor_id.clone(),
                action,
                expires_at_ms,
            },
        };
        let journal = self.journal_entries(actor_id, std::slice::from_ref(&response));
        self.persist_with_events(server_time_ms, &journal)?;
        Ok(vec![response])
    }

    pub(crate) fn idempotent_send_replay(
        &self,
        actor_id: &ActorId,
        command: &ClientCommand,
        server_time_ms: i64,
    ) -> Result<Option<Vec<ServerEnvelope>>, MessagingServiceError> {
        let replay = |existing: &Message| {
            vec![ServerEnvelope {
                protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
                cursor: Some(self.cursor.to_string()),
                server_time_ms,
                event: ServerEvent::MessageChanged {
                    message: existing.clone(),
                },
            }]
        };

        match command {
            ClientCommand::SendMessage {
                conversation_id,
                client_message_id,
                content,
                reply_to_message_id,
                thread_root_message_id,
                scheduled_at_ms,
                silent,
                protected_content,
            } => {
                let stable_id = stable_message_id(actor_id, client_message_id);
                let legacy_id = MessageId::new(format!("local:{}", client_message_id.0));
                let existing = self
                    .engine
                    .state()
                    .messages
                    .get(conversation_id)
                    .and_then(|messages| {
                        messages
                            .get(&stable_id)
                            .or_else(|| messages.get(&legacy_id))
                    });
                let Some(existing) = existing else {
                    return Ok(None);
                };
                let is_saved_messages = self
                    .engine
                    .state()
                    .conversations
                    .get(conversation_id)
                    .is_some_and(|conv| matches!(conv.kind, ConversationKind::SavedMessages));
                let expected_silent = if is_saved_messages { false } else { *silent };
                // The stable/legacy lookup key already binds this replay to the authenticated
                // actor + client_message_id, without extending the canonical Message schema.
                if &existing.sender_id != actor_id
                    || &existing.content != content
                    || &existing.reply_to_message_id != reply_to_message_id
                    || &existing.thread_root_message_id != thread_root_message_id
                    || &existing.scheduled_at_ms != scheduled_at_ms
                    || existing.silent != expected_silent
                    || &existing.protected_content != protected_content
                {
                    return Err(MessagingServiceError::IdempotencyConflict(
                        client_message_id.0.clone(),
                    ));
                }
                Ok(Some(replay(existing)))
            }
            ClientCommand::SendWhenParticipantOnline {
                conversation_id,
                client_message_id,
                target_actor_id,
                content,
                reply_to_message_id,
                thread_root_message_id,
                silent,
                protected_content,
            } => {
                let stable_id = stable_message_id(actor_id, client_message_id);
                if let Some(existing) = self
                    .engine
                    .state()
                    .messages
                    .get(conversation_id)
                    .and_then(|messages| messages.get(&stable_id))
                {
                    if &existing.sender_id != actor_id
                        || &existing.content != content
                        || &existing.reply_to_message_id != reply_to_message_id
                        || &existing.thread_root_message_id != thread_root_message_id
                        || existing.scheduled_at_ms.is_some()
                        || &existing.silent != silent
                        || &existing.protected_content != protected_content
                    {
                        return Err(MessagingServiceError::IdempotencyConflict(
                            client_message_id.0.clone(),
                        ));
                    }
                    return Ok(Some(replay(existing)));
                }
                if let Some(existing) = self
                    .engine
                    .state()
                    .pending_presence_sends
                    .get(client_message_id)
                {
                    let trigger_matches = matches!(
                        &existing.trigger,
                        PresenceSendTrigger::WhenParticipantOnline { actor_id: target }
                            if target == target_actor_id
                    );
                    if &existing.sender_id != actor_id
                        || &existing.conversation_id != conversation_id
                        || !trigger_matches
                        || &existing.content != content
                        || &existing.reply_to_message_id != reply_to_message_id
                        || &existing.thread_root_message_id != thread_root_message_id
                        || &existing.silent != silent
                        || &existing.protected_content != protected_content
                    {
                        return Err(MessagingServiceError::IdempotencyConflict(
                            client_message_id.0.clone(),
                        ));
                    }
                    return Ok(Some(vec![ServerEnvelope {
                        protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
                        cursor: Some(self.cursor.to_string()),
                        server_time_ms,
                        event: ServerEvent::PresenceTriggeredSendChanged {
                            client_message_id: client_message_id.clone(),
                            pending: Some(existing.clone()),
                        },
                    }]));
                }
                Ok(None)
            }
            ClientCommand::ForwardMessage {
                source_conversation_id,
                message_id,
                destination_conversation_id,
                client_message_id,
                thread_root_message_id,
                scheduled_at_ms,
                silent,
                privacy,
            } => {
                let stable_id = stable_message_id(actor_id, client_message_id);
                let legacy_id = MessageId::new(format!("local:{}", client_message_id.0));
                let existing = self
                    .engine
                    .state()
                    .messages
                    .get(destination_conversation_id)
                    .and_then(|messages| {
                        messages
                            .get(&stable_id)
                            .or_else(|| messages.get(&legacy_id))
                    });
                let Some(existing) = existing else {
                    return Ok(None);
                };
                let source = self
                    .engine
                    .state()
                    .messages
                    .get(source_conversation_id)
                    .and_then(|messages| messages.get(message_id));
                let force_preserve = source
                    .is_some_and(|message| message.content.requires_preserving_forward_info());
                let privacy = privacy.normalized_for_source(force_preserve);
                let expected_origin = if privacy.drop_sender_names {
                    None
                } else {
                    Some(
                        source
                            .and_then(|message| message.forward_origin.clone())
                            .unwrap_or_else(|| {
                                format!("{}:{}", source_conversation_id.0, message_id.0)
                            }),
                    )
                };
                let sender_can_keep_custom_emoji = self
                    .engine
                    .state()
                    .actors
                    .get(actor_id)
                    .is_some_and(|actor| {
                        actor.capabilities.iter().any(|cap| {
                            cap.eq_ignore_ascii_case("premium")
                                || cap.eq_ignore_ascii_case("can_send_custom_emoji")
                        })
                    });
                let expected_content = source.map(|message| {
                    let mut content = message.content.clone();
                    if privacy.drop_captions {
                        content.clear_caption();
                    }
                    if privacy.drop_sender_names && !sender_can_keep_custom_emoji {
                        content.strip_custom_emoji();
                    }
                    content
                });
                let is_saved_messages = self
                    .engine
                    .state()
                    .conversations
                    .get(destination_conversation_id)
                    .is_some_and(|conv| matches!(conv.kind, ConversationKind::SavedMessages));
                let expected_silent = if is_saved_messages { false } else { *silent };
                if &existing.sender_id != actor_id
                    || existing.forward_origin.as_deref() != expected_origin.as_deref()
                    || expected_content
                        .as_ref()
                        .is_some_and(|content| &existing.content != content)
                    || &existing.thread_root_message_id != thread_root_message_id
                    || &existing.scheduled_at_ms != scheduled_at_ms
                    || existing.silent != expected_silent
                {
                    return Err(MessagingServiceError::IdempotencyConflict(
                        client_message_id.0.clone(),
                    ));
                }
                Ok(Some(replay(existing)))
            }
            _ => Ok(None),
        }
    }

}
