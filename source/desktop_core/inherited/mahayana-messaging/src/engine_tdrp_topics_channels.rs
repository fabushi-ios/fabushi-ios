use crate::actor::ActorId;
use crate::community::{
    CommunityAuditAction, CommunityError, ForumTopicState,
};
use crate::conversation::{
    ConversationDestination, ConversationId, ConversationKind,
    ConversationMessagePosition,
};
use crate::engine::{EngineError, Event, MessagingEngine, MessagingState};
use crate::engine_tdrp_moderation::{
    append_community_audit, require_community_admin, CommunityAdminAction,
};

impl MessagingState {
    pub fn migrate_legacy_topic_child_states(&mut self) {
        let legacy_reads = self
            .topic_read_cursors
            .iter()
            .flat_map(|(conversation_id, by_actor)| {
                let messages = self.messages.get(conversation_id);
                by_actor.iter().flat_map(move |(actor_id, by_topic)| {
                    by_topic.iter().filter_map(move |(topic_id, message_id)| {
                        let message = messages?.get(message_id)?;
                        Some((
                            ConversationDestination::topic(
                                conversation_id.clone(),
                                format!("topic:{topic_id}"),
                            ),
                            actor_id.clone(),
                            ConversationMessagePosition::new(
                                message.created_at_ms,
                                message.id.0.clone(),
                            ),
                        ))
                    })
                })
            })
            .collect::<Vec<_>>();
        for (destination, actor_id, position) in legacy_reads {
            if let Some(child) = self.child_state_mut(destination, actor_id) {
                let _ = child.advance_inbox_read_till(position, None);
            }
        }

        let legacy_drafts = self
            .topic_drafts
            .iter()
            .flat_map(|(conversation_id, by_actor)| {
                by_actor.iter().flat_map(move |(actor_id, by_topic)| {
                    by_topic.iter().map(move |(topic_id, draft)| {
                        (
                            ConversationDestination::topic(
                                conversation_id.clone(),
                                format!("topic:{topic_id}"),
                            ),
                            actor_id.clone(),
                            draft.clone(),
                        )
                    })
                })
            })
            .collect::<Vec<_>>();
        for (destination, actor_id, draft) in legacy_drafts {
            if let Some(child) = self.child_state_mut(destination, actor_id) {
                if child.draft_text.trim().is_empty()
                    && child.draft_reply_to_message_id.is_none()
                    && (!draft.text.trim().is_empty() || draft.reply_to_message_id.is_some())
                {
                    child.set_draft(draft.text, draft.reply_to_message_id, draft.updated_at_ms);
                }
            }
        }
    }
}

impl MessagingEngine {
    pub(crate) fn decide_subscribe_channel(
        &self,
        actor_id: ActorId,
        conversation_id: ConversationId,
        subscribed_at_ms: i64,
    ) -> Result<Vec<Event>, EngineError> {
        self.require_actor(&actor_id)?;
        let conversation = self.require_conversation(&conversation_id)?;
        if !matches!(conversation.kind, ConversationKind::Channel) {
            return Err(EngineError::CommunityPermissionDenied);
        }
        let mut community = self
            .state
            .communities
            .get(&conversation_id)
            .cloned()
            .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
        if !community
            .public_username
            .as_deref()
            .is_some_and(|username| !username.trim().is_empty())
            && !community.is_subscriber(&actor_id)
            && !community.members.contains_key(&actor_id)
        {
            return Err(EngineError::CommunityPermissionDenied);
        }
        let was_subscriber = community.is_subscriber(&actor_id);
        community.subscribe_channel(actor_id.clone(), subscribed_at_ms)?;
        if !was_subscriber {
            append_community_audit(
                &mut community,
                &actor_id,
                CommunityAuditAction::SubscriptionAdded,
                Some(actor_id.clone()),
                None,
                None,
                subscribed_at_ms,
            );
        }
        Ok(vec![Event::CommunityChanged { community }])
    }

    pub(crate) fn decide_unsubscribe_channel(
        &self,
        actor_id: ActorId,
        conversation_id: ConversationId,
        unsubscribed_at_ms: i64,
    ) -> Result<Vec<Event>, EngineError> {
        self.require_actor(&actor_id)?;
        let conversation = self.require_conversation(&conversation_id)?;
        if !matches!(conversation.kind, ConversationKind::Channel) {
            return Err(EngineError::CommunityPermissionDenied);
        }
        if conversation.owner_id.as_ref() == Some(&actor_id) {
            return Err(EngineError::Community(
                CommunityError::OwnerCannotUnsubscribe,
            ));
        }
        let mut community = self
            .state
            .communities
            .get(&conversation_id)
            .cloned()
            .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
        let was_subscriber = community.is_subscriber(&actor_id);
        community.unsubscribe_channel(&actor_id)?;
        if was_subscriber {
            append_community_audit(
                &mut community,
                &actor_id,
                CommunityAuditAction::SubscriptionRemoved,
                Some(actor_id.clone()),
                None,
                None,
                unsubscribed_at_ms,
            );
        }
        Ok(vec![Event::CommunityChanged { community }])
    }

    pub(crate) fn decide_upsert_forum_topic(
        &self,
        actor_id: ActorId,
        topic: ForumTopicState,
    ) -> Result<Vec<Event>, EngineError> {
        let mut community = self
            .state
            .communities
            .get(&topic.conversation_id)
            .cloned()
            .ok_or_else(|| EngineError::CommunityNotFound(topic.conversation_id.clone()))?;
        require_community_admin(&community, &actor_id, CommunityAdminAction::ManageTopics)?;
        if topic.id.trim().is_empty()
            || topic.title.trim().is_empty()
            || topic.title.trim().len() > 128
            || topic.conversation_id != community.conversation_id
        {
            return Err(EngineError::CommunityPermissionDenied);
        }
        let topic_id = topic.id.clone();
        let created_at_ms = topic.created_at_ms;
        community.topics.insert(topic_id.clone(), topic);
        append_community_audit(
            &mut community,
            &actor_id,
            CommunityAuditAction::TopicUpserted,
            None,
            Some(format!("topic:{}", topic_id)),
            None,
            created_at_ms,
        );
        Ok(vec![Event::CommunityChanged { community }])
    }

    pub(crate) fn decide_delete_forum_topic(
        &self,
        actor_id: ActorId,
        conversation_id: ConversationId,
        topic_id: String,
    ) -> Result<Vec<Event>, EngineError> {
        let mut community = self
            .state
            .communities
            .get(&conversation_id)
            .cloned()
            .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
        require_community_admin(&community, &actor_id, CommunityAdminAction::ManageTopics)?;
        community.topics.remove(&topic_id);
        append_community_audit(
            &mut community,
            &actor_id,
            CommunityAuditAction::TopicDeleted,
            None,
            Some(format!("topic:{}", topic_id)),
            None,
            0,
        );
        Ok(vec![Event::CommunityChanged { community }])
    }
}
