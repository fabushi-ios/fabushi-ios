use crate::actor::ActorId;
use crate::engine::{EngineError, Event, MessagingEngine};
use crate::message::MessageContent;
use crate::story::{Story, StoryId};

pub fn message_content_uses_media(content: &MessageContent) -> bool {
    matches!(
        content,
        MessageContent::Photo { .. }
            | MessageContent::Video { .. }
            | MessageContent::Animation { .. }
            | MessageContent::Audio { .. }
            | MessageContent::Voice { .. }
            | MessageContent::VideoNote { .. }
            | MessageContent::Document { .. }
            | MessageContent::Sticker { .. }
    )
}

pub fn message_content_uses_other(content: &MessageContent) -> bool {
    matches!(
        content,
        MessageContent::Sticker { .. }
            | MessageContent::Animation { .. }
            | MessageContent::Dice { .. }
    )
}

impl MessagingEngine {
    pub fn story_visibility_flags(&self, owner_id: &ActorId, viewer_id: &ActorId) -> (bool, bool) {
        let Some(owner) = self.state.actors.get(owner_id) else {
            return (false, false);
        };
        let contact_cap = format!("contact:{}", viewer_id.0);
        let close_friend_cap = format!("close_friend:{}", viewer_id.0);
        let is_close_friend = owner
            .capabilities
            .iter()
            .any(|cap| cap == &close_friend_cap);
        let is_contact = is_close_friend
            || owner
                .capabilities
                .iter()
                .any(|cap| cap == &contact_cap);
        (is_contact, is_close_friend)
    }

    pub(crate) fn decide_publish_story(
        &self,
        actor_id: ActorId,
        story: Story,
    ) -> Result<Vec<Event>, EngineError> {
        self.require_actor(&actor_id)?;
        if story.owner_id != actor_id
            || story.id.0.trim().is_empty()
            || story.media.id.trim().is_empty()
            || story.expires_at_ms <= story.created_at_ms
        {
            return Err(EngineError::InvalidStory);
        }
        Ok(vec![Event::StoryChanged { story }])
    }

    pub(crate) fn decide_delete_story(
        &self,
        actor_id: ActorId,
        story_id: StoryId,
    ) -> Result<Vec<Event>, EngineError> {
        let story = self
            .state
            .stories
            .get(&story_id)
            .ok_or_else(|| EngineError::StoryNotFound(story_id.clone()))?;
        if story.owner_id != actor_id {
            return Err(EngineError::StoryPermissionDenied(story_id));
        }
        Ok(vec![Event::StoryDeleted { story_id }])
    }

    pub(crate) fn decide_view_story(
        &self,
        actor_id: ActorId,
        story_id: StoryId,
        viewed_at_ms: i64,
    ) -> Result<Vec<Event>, EngineError> {
        self.require_actor(&actor_id)?;
        let mut story = self
            .state
            .stories
            .get(&story_id)
            .cloned()
            .ok_or_else(|| EngineError::StoryNotFound(story_id.clone()))?;
        let (is_contact, is_close_friend) =
            self.story_visibility_flags(&story.owner_id, &actor_id);
        if !story.is_visible_to(&actor_id, is_contact, is_close_friend) {
            return Err(EngineError::StoryPermissionDenied(story_id));
        }
        story.record_view(actor_id, viewed_at_ms)?;
        Ok(vec![Event::StoryChanged { story }])
    }

    pub(crate) fn decide_react_story(
        &self,
        actor_id: ActorId,
        story_id: StoryId,
        reaction: Option<String>,
    ) -> Result<Vec<Event>, EngineError> {
        let mut story = self
            .state
            .stories
            .get(&story_id)
            .cloned()
            .ok_or_else(|| EngineError::StoryNotFound(story_id.clone()))?;
        let (is_contact, is_close_friend) =
            self.story_visibility_flags(&story.owner_id, &actor_id);
        if !story.is_visible_to(&actor_id, is_contact, is_close_friend) {
            return Err(EngineError::StoryPermissionDenied(story_id));
        }
        story.react(&actor_id, reaction)?;
        Ok(vec![Event::StoryChanged { story }])
    }
}
