use crate::actor::{ActorId, ActorKind};
use crate::bot::BotInvocation;
use crate::community::MemberStatus;
use crate::message::{
    Message, MessageContent,
};
use crate::protocol::ClientCommand;
use crate::service::{
    sanitize_invocation_component, MessagingService, MessagingServiceError,
};
use crate::store::MessagingStateStore;

impl<S: MessagingStateStore> MessagingService<S> {
    pub(crate) fn bot_invocations_for_message(&self, message: &Message) -> Vec<BotInvocation> {
        let sender = match self.engine.state().actors.get(&message.sender_id) {
            Some(sender) => sender,
            None => return Vec::new(),
        };
        if matches!(
            sender.kind,
            ActorKind::Bot | ActorKind::Assistant | ActorKind::Service
        ) {
            return Vec::new();
        }
        let MessageContent::Text { text } = &message.content else {
            return Vec::new();
        };
        let Some(conversation) = self
            .engine
            .state()
            .conversations
            .get(&message.conversation_id)
        else {
            return Vec::new();
        };
        let command = text
            .text
            .trim()
            .strip_prefix('/')
            .and_then(|value| value.split_whitespace().next())
            .map(|value| value.trim_start_matches('@').to_string())
            .filter(|value| !value.is_empty());
        conversation
            .participants
            .iter()
            .filter_map(|participant| {
                if participant.actor_id == message.sender_id {
                    return None;
                }
                let actor = self.engine.state().actors.get(&participant.actor_id)?;
                if !matches!(actor.kind, ActorKind::Bot | ActorKind::Assistant) {
                    return None;
                }
                Some(BotInvocation {
                    id: format!(
                        "invoke:auto:{}:{}",
                        sanitize_invocation_component(&message.id.0),
                        sanitize_invocation_component(&actor.id.0)
                    ),
                    bot_id: actor.id.clone(),
                    sender_id: message.sender_id.clone(),
                    conversation_id: message.conversation_id.clone(),
                    command: command.clone(),
                    text: text.clone(),
                    reply_to_message_id: message
                        .reply_to_message_id
                        .as_ref()
                        .map(|id| id.0.clone()),
                    metadata: std::collections::BTreeMap::from([
                        ("source".into(), "messaging-service".into()),
                        ("messageId".into(), message.id.0.clone()),
                    ]),
                    created_at_ms: message.created_at_ms,
                })
            })
            .collect()
    }

    pub(crate) fn validate_command_authorization(
        &self,
        actor_id: &ActorId,
        command: &ClientCommand,
    ) -> Result<(), MessagingServiceError> {
        let denied = |reason: &str| MessagingServiceError::UnauthorizedCommand(reason.into());
        match command {
            ClientCommand::UpsertProfile { actor } if &actor.id != actor_id => {
                return Err(denied(
                    "profile actor id does not match authenticated actor",
                ));
            }
            ClientCommand::CreateConversation { conversation } => {
                let caller_is_participant = conversation
                    .participants
                    .iter()
                    .any(|participant| &participant.actor_id == actor_id);
                if !caller_is_participant
                    || conversation
                        .owner_id
                        .as_ref()
                        .is_some_and(|owner| owner != actor_id)
                {
                    return Err(denied("conversation creator must be an owner/participant"));
                }
            }
            ClientCommand::SetMarkedUnread {
                conversation_id, ..
            }
            | ClientCommand::SetDraft {
                conversation_id, ..
            } => {
                let existing = self
                    .engine
                    .state()
                    .conversations
                    .get(conversation_id)
                    .ok_or_else(|| denied("conversation state target does not exist"))?;
                let caller_is_member = existing
                    .participants
                    .iter()
                    .any(|participant| &participant.actor_id == actor_id)
                    || existing.owner_id.as_ref() == Some(actor_id)
                    || self
                        .engine
                        .state()
                        .communities
                        .get(conversation_id)
                        .is_some_and(|community| community.is_subscriber(actor_id));
                if !caller_is_member {
                    return Err(denied("conversation state update requires membership"));
                }
            }
            ClientCommand::MarkConversationChildRead { destination, .. }
            | ClientCommand::SetConversationChildDraft { destination, .. }
            | ClientCommand::ReplaceConversationChildWindow { destination, .. }
            | ClientCommand::SetConversationChildPinned { destination, .. }
            | ClientCommand::SetConversationChildActive { destination, .. }
            | ClientCommand::SetConversationChildMarkedUnread { destination, .. }
            | ClientCommand::SetConversationChildNoPaidMessages { destination, .. }
            | ClientCommand::DestroyConversationChild { destination, .. } => {
                let conversation_id = &destination.conversation_id;
                let existing = self
                    .engine
                    .state()
                    .conversations
                    .get(conversation_id)
                    .ok_or_else(|| denied("conversation child target does not exist"))?;
                let caller_is_member = existing
                    .participants
                    .iter()
                    .any(|participant| &participant.actor_id == actor_id)
                    || existing.owner_id.as_ref() == Some(actor_id)
                    || self
                        .engine
                        .state()
                        .communities
                        .get(conversation_id)
                        .is_some_and(|community| community.is_subscriber(actor_id));
                if !caller_is_member {
                    return Err(denied("conversation child state update requires membership"));
                }
            }
            ClientCommand::MarkTopicRead {
                conversation_id, ..
            }
            | ClientCommand::SetTopicDraft {
                conversation_id, ..
            }
            | ClientCommand::ListCommunityMembers {
                conversation_id, ..
            }
            | ClientCommand::ListCommunityAuditLog {
                conversation_id, ..
            } => {
                self.authorized_community(actor_id, conversation_id)?;
            }
            ClientCommand::UpdateConversationInfo {
                conversation_id, ..
            }
            | ClientCommand::SetConversationParticipant {
                conversation_id, ..
            }
            | ClientCommand::RemoveConversationParticipant {
                conversation_id, ..
            } => {
                if let Some(community) = self.engine.state().communities.get(conversation_id) {
                    let caller = community
                        .members
                        .get(actor_id)
                        .ok_or_else(|| denied("community management requires membership"))?;
                    let caller_is_owner = matches!(caller.status, MemberStatus::Owner);
                    let caller_can_manage = caller_is_owner
                        || (matches!(caller.status, MemberStatus::Administrator)
                            && caller.admin_rights.add_admins);
                    if !caller_can_manage {
                        return Err(denied("community management requires admin rights"));
                    }
                    match command {
                        ClientCommand::SetConversationParticipant { participant, .. } => {
                            let target = community.members.get(&participant.actor_id);
                            if target
                                .is_some_and(|member| matches!(member.status, MemberStatus::Owner))
                                || matches!(participant.role, crate::actor::ParticipantRole::Owner)
                            {
                                return Err(denied("community owner cannot be changed"));
                            }
                            if !caller_is_owner
                                && target.is_some_and(|member| {
                                    matches!(member.status, MemberStatus::Administrator)
                                })
                            {
                                return Err(denied("admins cannot manage other administrators"));
                            }
                        }
                        ClientCommand::RemoveConversationParticipant {
                            actor_id: target_actor_id,
                            ..
                        } if community
                            .members
                            .get(target_actor_id)
                            .is_some_and(|member| {
                                matches!(
                                    member.status,
                                    MemberStatus::Owner | MemberStatus::Administrator
                                )
                            })
                            && !caller_is_owner =>
                        {
                            return Err(denied("admins cannot remove owner/admin members"));
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                let existing = self
                    .engine
                    .state()
                    .conversations
                    .get(conversation_id)
                    .ok_or_else(|| denied("conversation management target does not exist"))?;
                let caller = existing
                    .participants
                    .iter()
                    .find(|participant| &participant.actor_id == actor_id)
                    .ok_or_else(|| denied("conversation management requires membership"))?;
                if !matches!(
                    caller.role,
                    crate::actor::ParticipantRole::Owner | crate::actor::ParticipantRole::Admin
                ) {
                    return Err(denied("conversation management requires owner/admin role"));
                }
                match command {
                    ClientCommand::SetConversationParticipant { participant, .. } => {
                        let existing_target = existing
                            .participants
                            .iter()
                            .find(|item| item.actor_id == participant.actor_id);
                        if participant.role == crate::actor::ParticipantRole::Owner
                            && existing.owner_id.as_ref() != Some(&participant.actor_id)
                        {
                            return Err(denied(
                                "conversation owner cannot be reassigned through participant management",
                            ));
                        }
                        if caller.role == crate::actor::ParticipantRole::Admin
                            && (matches!(
                                participant.role,
                                crate::actor::ParticipantRole::Owner
                                    | crate::actor::ParticipantRole::Admin
                            ) || existing_target.is_some_and(|target| {
                                matches!(
                                    target.role,
                                    crate::actor::ParticipantRole::Owner
                                        | crate::actor::ParticipantRole::Admin
                                )
                            }))
                        {
                            return Err(denied("admins cannot manage owner/admin roles"));
                        }
                    }
                    ClientCommand::RemoveConversationParticipant {
                        actor_id: target_actor_id,
                        ..
                    } => {
                        if existing.owner_id.as_ref() == Some(target_actor_id) {
                            return Err(denied("conversation owner cannot be removed"));
                        }
                        if caller.role == crate::actor::ParticipantRole::Admin
                            && existing
                                .participants
                                .iter()
                                .find(|item| &item.actor_id == target_actor_id)
                                .is_some_and(|target| {
                                    matches!(
                                        target.role,
                                        crate::actor::ParticipantRole::Owner
                                            | crate::actor::ParticipantRole::Admin
                                    )
                                })
                        {
                            return Err(denied("admins cannot remove owner/admin participants"));
                        }
                    }
                    _ => {}
                }
            }
            ClientCommand::UpdateConversation { conversation } => {
                if let Some(community) = self.engine.state().communities.get(&conversation.id) {
                    let member = community
                        .members
                        .get(actor_id)
                        .ok_or_else(|| denied("community update requires membership"))?;
                    if !matches!(member.status, MemberStatus::Owner)
                        && !(matches!(member.status, MemberStatus::Administrator)
                            && member.admin_rights.change_info)
                    {
                        return Err(denied("community update requires change_info permission"));
                    }
                    return Ok(());
                }
                let existing = self
                    .engine
                    .state()
                    .conversations
                    .get(&conversation.id)
                    .ok_or_else(|| denied("conversation update target does not exist"))?;
                let caller = existing
                    .participants
                    .iter()
                    .find(|participant| &participant.actor_id == actor_id)
                    .ok_or_else(|| denied("conversation update requires membership"))?;
                if !matches!(
                    caller.role,
                    crate::actor::ParticipantRole::Owner | crate::actor::ParticipantRole::Admin
                ) {
                    return Err(denied("conversation update requires owner/admin role"));
                }
            }
            ClientCommand::CreateInvoice { invoice } if &invoice.seller_id != actor_id => {
                return Err(denied(
                    "invoice seller id does not match authenticated actor",
                ));
            }
            ClientCommand::GrantMiniApp { grant } if &grant.actor_id != actor_id => {
                return Err(denied(
                    "Mini App grant actor does not match authenticated actor",
                ));
            }
            ClientCommand::OpenMiniApp { session } if &session.actor_id != actor_id => {
                return Err(denied(
                    "Mini App session actor does not match authenticated actor",
                ));
            }
            _ => {}
        }
        Ok(())
    }

}
