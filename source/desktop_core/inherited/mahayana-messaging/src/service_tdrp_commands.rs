use crate::actor::ActorId;
use crate::conversation::{
    ConversationDraft, ConversationKind, TopicDraft,
};
use crate::engine::Command;
use crate::message::{
    DeliveryState, PendingPresenceSend,
    PresenceSendTrigger,
};
use crate::protocol::ClientCommand;
use crate::service::{
    stable_message_id, MessagingService,
};
use crate::store::MessagingStateStore;

impl<S: MessagingStateStore> MessagingService<S> {
    pub(crate) fn project_command(
        &self,
        actor_id: &ActorId,
        command: ClientCommand,
        now_ms: i64,
    ) -> Vec<Command> {
        match command {
            ClientCommand::Sync { .. } => Vec::new(),
            ClientCommand::UpsertProfile { actor } => vec![Command::UpsertActor { actor }],
            ClientCommand::UpsertPrivacySettings { settings } => {
                vec![Command::UpsertPrivacySettings {
                    actor_id: actor_id.clone(),
                    settings,
                }]
            }
            ClientCommand::SetPresence { presence } => vec![Command::SetPresence {
                actor_id: actor_id.clone(),
                presence,
            }],
            ClientCommand::CreateConversation { conversation } => {
                vec![Command::UpsertConversation { conversation }]
            }
            ClientCommand::UpdateConversation { conversation } => {
                let is_community_backed = self
                    .engine
                    .state()
                    .communities
                    .contains_key(&conversation.id);
                if is_community_backed {
                    vec![Command::UpdateConversationInfo {
                        conversation_id: conversation.id,
                        title: conversation.title,
                        description: conversation.description,
                    }]
                } else {
                    vec![Command::UpsertConversation { conversation }]
                }
            }
            ClientCommand::UpdateConversationInfo {
                conversation_id,
                title,
                description,
            } => vec![Command::UpdateConversationInfo {
                conversation_id,
                title,
                description,
            }],
            ClientCommand::SetConversationParticipant {
                conversation_id,
                participant,
            } => vec![Command::SetConversationParticipant {
                conversation_id,
                participant,
            }],
            ClientCommand::RemoveConversationParticipant {
                conversation_id,
                actor_id: target_actor_id,
            } => vec![Command::RemoveConversationParticipant {
                conversation_id,
                actor_id: target_actor_id,
            }],
            ClientCommand::ArchiveConversation {
                conversation_id,
                archived,
            } => vec![Command::ArchiveConversation {
                conversation_id,
                archived,
            }],
            ClientCommand::PinConversation {
                conversation_id,
                pinned,
            } => vec![Command::PinConversation {
                conversation_id,
                pinned,
            }],
            ClientCommand::SetMarkedUnread {
                conversation_id,
                marked_unread,
            } => vec![Command::SetMarkedUnread {
                conversation_id,
                actor_id: actor_id.clone(),
                marked_unread,
            }],
            ClientCommand::SetDraft {
                conversation_id,
                text,
                reply_to_message_id,
            } => vec![Command::SetDraft {
                draft: ConversationDraft {
                    conversation_id,
                    actor_id: actor_id.clone(),
                    text,
                    reply_to_message_id: reply_to_message_id.map(|id| id.0),
                    updated_at_ms: now_ms,
                },
            }],
            ClientCommand::SetConversationNotifications {
                conversation_id,
                settings,
            } => vec![Command::SetConversationNotifications {
                conversation_id,
                settings,
            }],
            ClientCommand::UpsertFolder { folder } => vec![Command::UpsertFolder { folder }],
            ClientCommand::DeleteFolder { folder_id } => vec![Command::DeleteFolder { folder_id }],
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
                let message_id = stable_message_id(actor_id, &client_message_id);
                let replaces_pending_presence = self
                    .engine
                    .state()
                    .pending_presence_sends
                    .get(&client_message_id)
                    .is_some_and(|pending| {
                        &pending.sender_id == actor_id && pending.conversation_id == conversation_id
                    });
                let mut commands = vec![
                    Command::QueueMessage {
                        conversation_id: conversation_id.clone(),
                        local_message_id: message_id.clone(),
                        client_message_id: client_message_id.clone(),
                        sender_id: actor_id.clone(),
                        content,
                        reply_to_message_id,
                        thread_root_message_id,
                        created_at_ms: now_ms,
                        scheduled_at_ms,
                        silent,
                        protected_content,
                    },
                    Command::AcknowledgeMessage {
                        conversation_id,
                        local_message_id: message_id.clone(),
                        server_message_id: message_id,
                        accepted_at_ms: now_ms,
                    },
                ];
                if replaces_pending_presence {
                    commands.push(Command::RemovePresenceTriggeredSend { client_message_id });
                }
                commands
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
            } => vec![Command::QueuePresenceTriggeredSend {
                pending: PendingPresenceSend {
                    local_message_id: stable_message_id(actor_id, &client_message_id),
                    conversation_id,
                    client_message_id,
                    sender_id: actor_id.clone(),
                    trigger: PresenceSendTrigger::WhenParticipantOnline {
                        actor_id: target_actor_id,
                    },
                    content,
                    reply_to_message_id,
                    thread_root_message_id,
                    silent,
                    protected_content,
                    created_at_ms: now_ms,
                },
            }],
            ClientCommand::CancelPresenceTriggeredSend { client_message_id } => {
                vec![Command::CancelPresenceTriggeredSend {
                    actor_id: actor_id.clone(),
                    client_message_id,
                }]
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
            } => vec![Command::ForwardMessage {
                source_conversation_id,
                message_id,
                destination_conversation_id,
                local_message_id: stable_message_id(actor_id, &client_message_id),
                client_message_id,
                sender_id: actor_id.clone(),
                thread_root_message_id,
                created_at_ms: now_ms,
                scheduled_at_ms,
                silent,
                privacy,
            }],
            ClientCommand::EditMessage {
                conversation_id,
                message_id,
                content,
            } => vec![Command::EditMessage {
                conversation_id,
                message_id,
                content,
                edited_at_ms: now_ms,
            }],
            ClientCommand::DeleteMessages {
                conversation_id,
                message_ids,
                ..
            } => vec![Command::DeleteMessages {
                conversation_id,
                message_ids,
            }],
            ClientCommand::MarkRead {
                conversation_id,
                message_id,
            } => {
                let mut commands = vec![Command::MarkRead {
                    conversation_id: conversation_id.clone(),
                    actor_id: actor_id.clone(),
                    message_id: message_id.clone(),
                }];
                let should_mark_message_read = self
                    .engine
                    .state()
                    .conversations
                    .get(&conversation_id)
                    .is_some_and(|conversation| {
                        matches!(
                            conversation.kind,
                            ConversationKind::Direct | ConversationKind::Secret
                        )
                    })
                    && self
                        .engine
                        .state()
                        .messages
                        .get(&conversation_id)
                        .and_then(|messages| messages.get(&message_id))
                        .is_some_and(|message| &message.sender_id != actor_id);
                if should_mark_message_read {
                    commands.push(Command::SetDeliveryState {
                        conversation_id,
                        message_id,
                        state: DeliveryState::Read,
                    });
                }
                commands
            }
            ClientCommand::MarkTopicRead {
                conversation_id,
                topic_id,
                message_id,
            } => vec![Command::MarkTopicRead {
                conversation_id,
                topic_id,
                actor_id: actor_id.clone(),
                message_id,
            }],
            ClientCommand::MarkConversationChildRead {
                destination,
                message_id,
            } => vec![Command::MarkConversationChildRead {
                destination,
                actor_id: actor_id.clone(),
                message_id,
            }],
            ClientCommand::SetTopicDraft {
                conversation_id,
                topic_id,
                text,
                reply_to_message_id,
            } => vec![Command::SetTopicDraft {
                draft: TopicDraft {
                    conversation_id,
                    topic_id,
                    actor_id: actor_id.clone(),
                    text,
                    reply_to_message_id: reply_to_message_id.map(|id| id.0),
                    updated_at_ms: now_ms,
                },
            }],
            ClientCommand::SetConversationChildDraft {
                destination,
                text,
                reply_to_message_id,
            } => vec![Command::SetConversationChildDraft {
                destination,
                actor_id: actor_id.clone(),
                text,
                reply_to_message_id,
                updated_at_ms: now_ms,
            }],
            ClientCommand::ReplaceConversationChildWindow {
                destination,
                message_ids,
                skipped_before,
                skipped_after,
                full_count,
            } => vec![Command::ReplaceConversationChildWindow {
                destination,
                actor_id: actor_id.clone(),
                message_ids,
                skipped_before,
                skipped_after,
                full_count,
            }],
            ClientCommand::SetConversationChildPinned {
                destination,
                pinned,
            } => vec![Command::SetConversationChildPinned {
                destination,
                actor_id: actor_id.clone(),
                pinned,
            }],
            ClientCommand::SetConversationChildActive {
                destination,
                active,
            } => vec![Command::SetConversationChildActive {
                destination,
                actor_id: actor_id.clone(),
                active,
            }],
            ClientCommand::SetConversationChildMarkedUnread {
                destination,
                marked_unread,
            } => vec![Command::SetConversationChildMarkedUnread {
                destination,
                actor_id: actor_id.clone(),
                marked_unread,
            }],
            ClientCommand::SetConversationChildNoPaidMessages {
                destination,
                no_paid_messages,
            } => vec![Command::SetConversationChildNoPaidMessages {
                destination,
                actor_id: actor_id.clone(),
                no_paid_messages,
            }],
            ClientCommand::DestroyConversationChild { destination } => {
                vec![Command::DestroyConversationChild {
                    destination,
                    actor_id: actor_id.clone(),
                }]
            }
            ClientCommand::SetReaction {
                conversation_id,
                message_id,
                reaction,
            } => vec![Command::SetReaction {
                conversation_id,
                message_id,
                reaction,
            }],
            ClientCommand::PinMessage {
                conversation_id,
                message_id,
                pinned,
            } => vec![Command::PinMessage {
                conversation_id,
                message_id,
                pinned,
            }],
            ClientCommand::VotePoll {
                conversation_id,
                message_id,
                option_ids,
            } => vec![Command::VotePoll {
                conversation_id,
                message_id,
                actor_id: actor_id.clone(),
                option_ids,
            }],
            ClientCommand::Search { .. }
            | ClientCommand::SearchRecipients { .. }
            | ClientCommand::ListCommunityMembers { .. }
            | ClientCommand::ListCommunityAuditLog { .. }
            | ClientCommand::StartTyping { .. }
            | ClientCommand::StopTyping { .. } => Vec::new(),
            ClientCommand::CreateInvoice { invoice } => vec![Command::CreateInvoice { invoice }],
            ClientCommand::CheckoutInvoice {
                invoice_id,
                order_id,
                customer,
            } => vec![Command::CheckoutInvoice {
                invoice_id,
                order_id,
                buyer_id: actor_id.clone(),
                customer,
                created_at_ms: now_ms,
            }],
            ClientCommand::RefundOrder {
                order_id,
                request_id,
            } => vec![Command::RefundOrder {
                order_id,
                seller_id: actor_id.clone(),
                request_id,
                refunded_at_ms: now_ms,
            }],
            ClientCommand::WalletStatus => Vec::new(),
            ClientCommand::PublishStory { story } => vec![Command::PublishStory {
                actor_id: actor_id.clone(),
                story,
            }],
            ClientCommand::DeleteStory { story_id } => vec![Command::DeleteStory {
                actor_id: actor_id.clone(),
                story_id,
            }],
            ClientCommand::ViewStory { story_id } => vec![Command::ViewStory {
                actor_id: actor_id.clone(),
                story_id,
                viewed_at_ms: now_ms,
            }],
            ClientCommand::ReactStory { story_id, reaction } => vec![Command::ReactStory {
                actor_id: actor_id.clone(),
                story_id,
                reaction,
            }],
            ClientCommand::UpdateCommunity { community } => vec![Command::UpdateCommunity {
                actor_id: actor_id.clone(),
                community,
            }],
            ClientCommand::SubscribeChannel { conversation_id } => {
                vec![Command::SubscribeChannel {
                    actor_id: actor_id.clone(),
                    conversation_id,
                    subscribed_at_ms: now_ms,
                }]
            }
            ClientCommand::UnsubscribeChannel { conversation_id } => {
                vec![Command::UnsubscribeChannel {
                    actor_id: actor_id.clone(),
                    conversation_id,
                    unsubscribed_at_ms: now_ms,
                }]
            }
            ClientCommand::SetCommunitySlowMode {
                conversation_id,
                seconds,
            } => vec![Command::SetCommunitySlowMode {
                actor_id: actor_id.clone(),
                conversation_id,
                seconds,
                changed_at_ms: now_ms,
            }],
            ClientCommand::ModerateCommunityMember {
                conversation_id,
                member,
                reason,
            } => vec![Command::ModerateCommunityMember {
                actor_id: actor_id.clone(),
                conversation_id,
                member,
                reason,
                decided_at_ms: now_ms,
            }],
            ClientCommand::SetCommunityMember {
                conversation_id,
                member,
            } => vec![Command::SetCommunityMember {
                actor_id: actor_id.clone(),
                conversation_id,
                member,
            }],
            ClientCommand::CreateInviteLink { invite } => vec![Command::CreateInviteLink {
                actor_id: actor_id.clone(),
                invite,
            }],
            ClientCommand::RevokeInviteLink {
                conversation_id,
                invite_id,
            } => vec![Command::RevokeInviteLink {
                actor_id: actor_id.clone(),
                conversation_id,
                invite_id,
                revoked_at_ms: now_ms,
            }],
            ClientCommand::RequestCommunityJoin { request } => {
                vec![Command::RequestCommunityJoin {
                    actor_id: actor_id.clone(),
                    request,
                }]
            }
            ClientCommand::RespondCommunityJoin {
                conversation_id,
                requester_id,
                approved,
            } => vec![Command::RespondCommunityJoin {
                actor_id: actor_id.clone(),
                conversation_id,
                requester_id,
                approved,
                decided_at_ms: now_ms,
            }],
            ClientCommand::UpsertForumTopic { topic } => vec![Command::UpsertForumTopic {
                actor_id: actor_id.clone(),
                topic,
            }],
            ClientCommand::DeleteForumTopic {
                conversation_id,
                topic_id,
            } => vec![Command::DeleteForumTopic {
                actor_id: actor_id.clone(),
                conversation_id,
                topic_id,
            }],
            ClientCommand::RegisterBot { profile } => vec![Command::RegisterBot {
                actor_id: actor_id.clone(),
                profile,
            }],
            ClientCommand::InvokeBot { invocation } => vec![Command::BeginBotInvocation {
                actor_id: actor_id.clone(),
                invocation,
                created_at_ms: now_ms,
            }],
            ClientCommand::FinishBotExecution {
                execution_id,
                success,
                error,
            } => vec![Command::FinishBotExecution {
                actor_id: actor_id.clone(),
                execution_id,
                success,
                finished_at_ms: now_ms,
                error,
            }],
            ClientCommand::InstallMiniApp { manifest } => {
                vec![Command::InstallMiniApp { manifest }]
            }
            ClientCommand::GrantMiniApp { grant } => vec![Command::GrantMiniApp { grant }],
            ClientCommand::OpenMiniApp { session } => vec![Command::OpenMiniApp { session }],
            ClientCommand::MiniAppCall {
                session_id,
                request_id,
                request,
            } => vec![Command::MiniAppCall {
                session_id,
                request_id,
                request,
            }],
            ClientCommand::BeginBlobUpload { .. }
            | ClientCommand::AppendBlobChunk { .. }
            | ClientCommand::FinishBlobUpload { .. }
            | ClientCommand::DeleteBlob { .. } => Vec::new(),
        }
    }

}
