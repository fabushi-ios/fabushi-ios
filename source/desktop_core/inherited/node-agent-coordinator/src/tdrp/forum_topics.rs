use crate::protocol::Failure;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForumTopic {
    pub topic_id: u32,
    pub community_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_emoji: Option<String>,
    pub icon_color: u32,
    pub is_pinned: bool,
    pub is_closed: bool,
    pub is_hidden: bool,
    pub creation_date: i64,
    pub unread_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_message_id: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ForumTopicsState {
    pub topics: BTreeMap<(String, u32), ForumTopic>,
    pub next_topic_ids: BTreeMap<String, u32>,
}

impl ForumTopicsState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list_topics(&self, community_id: &str) -> Vec<ForumTopic> {
        let mut list: Vec<ForumTopic> = self
            .topics
            .iter()
            .filter(|((comm, _), _)| comm == community_id)
            .map(|(_, topic)| topic.clone())
            .collect();
        // Pinned topics first, then descending creation_date
        list.sort_by(|a, b| {
            b.is_pinned
                .cmp(&a.is_pinned)
                .then_with(|| b.creation_date.cmp(&a.creation_date))
        });
        list
    }

    pub fn get_topic(&self, community_id: &str, topic_id: u32) -> Option<&ForumTopic> {
        self.topics.get(&(community_id.to_string(), topic_id))
    }

    pub fn create_topic(
        &mut self,
        community_id: String,
        title: String,
        icon_emoji: Option<String>,
        icon_color: u32,
        now_ms: i64,
    ) -> Result<ForumTopic, Failure> {
        if title.trim().is_empty() {
            return Err(Failure::new(
                "INVALID_TOPIC_TITLE",
                "topic title cannot be empty",
            ));
        }
        let counter = self
            .next_topic_ids
            .entry(community_id.clone())
            .or_insert(1);
        let topic_id = *counter;
        *counter += 1;

        let topic = ForumTopic {
            topic_id,
            community_id: community_id.clone(),
            title,
            icon_emoji,
            icon_color,
            is_pinned: false,
            is_closed: false,
            is_hidden: false,
            creation_date: now_ms,
            unread_count: 0,
            top_message_id: None,
        };
        self.topics
            .insert((community_id, topic_id), topic.clone());
        Ok(topic)
    }

    pub fn edit_topic(
        &mut self,
        community_id: &str,
        topic_id: u32,
        title: Option<String>,
        icon_emoji: Option<String>,
        icon_color: Option<u32>,
    ) -> Result<ForumTopic, Failure> {
        let key = (community_id.to_string(), topic_id);
        let topic = self
            .topics
            .get_mut(&key)
            .ok_or_else(|| Failure::new("TOPIC_NOT_FOUND", format!("topic {topic_id} not found")))?;

        if let Some(t) = title {
            if t.trim().is_empty() {
                return Err(Failure::new(
                    "INVALID_TOPIC_TITLE",
                    "topic title cannot be empty",
                ));
            }
            topic.title = t;
        }
        if icon_emoji.is_some() {
            topic.icon_emoji = icon_emoji;
        }
        if let Some(c) = icon_color {
            topic.icon_color = c;
        }
        Ok(topic.clone())
    }

    pub fn toggle_pin(
        &mut self,
        community_id: &str,
        topic_id: u32,
        pinned: bool,
    ) -> Result<ForumTopic, Failure> {
        let key = (community_id.to_string(), topic_id);
        let topic = self
            .topics
            .get_mut(&key)
            .ok_or_else(|| Failure::new("TOPIC_NOT_FOUND", format!("topic {topic_id} not found")))?;
        topic.is_pinned = pinned;
        Ok(topic.clone())
    }

    pub fn toggle_close(
        &mut self,
        community_id: &str,
        topic_id: u32,
        closed: bool,
    ) -> Result<ForumTopic, Failure> {
        let key = (community_id.to_string(), topic_id);
        let topic = self
            .topics
            .get_mut(&key)
            .ok_or_else(|| Failure::new("TOPIC_NOT_FOUND", format!("topic {topic_id} not found")))?;
        topic.is_closed = closed;
        Ok(topic.clone())
    }

    pub fn toggle_hide(
        &mut self,
        community_id: &str,
        topic_id: u32,
        hidden: bool,
    ) -> Result<ForumTopic, Failure> {
        let key = (community_id.to_string(), topic_id);
        let topic = self
            .topics
            .get_mut(&key)
            .ok_or_else(|| Failure::new("TOPIC_NOT_FOUND", format!("topic {topic_id} not found")))?;
        topic.is_hidden = hidden;
        Ok(topic.clone())
    }

    pub fn delete_topic(&mut self, community_id: &str, topic_id: u32) -> Result<bool, Failure> {
        let key = (community_id.to_string(), topic_id);
        if self.topics.remove(&key).is_none() {
            return Err(Failure::new(
                "TOPIC_NOT_FOUND",
                format!("topic {topic_id} not found"),
            ));
        }
        Ok(true)
    }

    pub fn update_unread(
        &mut self,
        community_id: &str,
        topic_id: u32,
        count: u32,
        top_message_id: Option<u64>,
    ) -> Result<ForumTopic, Failure> {
        let key = (community_id.to_string(), topic_id);
        let topic = self
            .topics
            .get_mut(&key)
            .ok_or_else(|| Failure::new("TOPIC_NOT_FOUND", format!("topic {topic_id} not found")))?;
        topic.unread_count = count;
        if top_message_id.is_some() {
            topic.top_message_id = top_message_id;
        }
        Ok(topic.clone())
    }

    pub fn handle_route(&mut self, method: &str, args: &Value) -> Result<Value, Failure> {
        let op = method.strip_prefix("topics.").unwrap_or(method);
        match op {
            "list" => {
                let community_id = args
                    .get("communityId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "communityId required"))?;
                let topics = self.list_topics(community_id);
                Ok(json!({ "topics": topics }))
            }
            "create" => {
                let community_id = args
                    .get("communityId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "communityId required"))?
                    .to_string();
                let title = args
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let icon_emoji = args
                    .get("iconEmoji")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let icon_color = args
                    .get("iconColor")
                    .and_then(Value::as_u64)
                    .unwrap_or(0x6FB9F0) as u32;
                let now_ms = args
                    .get("nowMs")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                let topic =
                    self.create_topic(community_id, title, icon_emoji, icon_color, now_ms)?;
                Ok(json!({ "topic": topic }))
            }
            "edit" => {
                let community_id = args
                    .get("communityId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "communityId required"))?;
                let topic_id = args
                    .get("topicId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "topicId required"))?
                    as u32;
                let title = args
                    .get("title")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let icon_emoji = args
                    .get("iconEmoji")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let icon_color = args
                    .get("iconColor")
                    .and_then(Value::as_u64)
                    .map(|c| c as u32);

                let topic =
                    self.edit_topic(community_id, topic_id, title, icon_emoji, icon_color)?;
                Ok(json!({ "topic": topic }))
            }
            "togglePin" => {
                let community_id = args
                    .get("communityId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "communityId required"))?;
                let topic_id = args
                    .get("topicId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "topicId required"))?
                    as u32;
                let pinned = args.get("pinned").and_then(Value::as_bool).unwrap_or(true);
                let topic = self.toggle_pin(community_id, topic_id, pinned)?;
                Ok(json!({ "topic": topic }))
            }
            "toggleClose" => {
                let community_id = args
                    .get("communityId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "communityId required"))?;
                let topic_id = args
                    .get("topicId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "topicId required"))?
                    as u32;
                let closed = args.get("closed").and_then(Value::as_bool).unwrap_or(true);
                let topic = self.toggle_close(community_id, topic_id, closed)?;
                Ok(json!({ "topic": topic }))
            }
            "toggleHide" => {
                let community_id = args
                    .get("communityId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "communityId required"))?;
                let topic_id = args
                    .get("topicId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "topicId required"))?
                    as u32;
                let hidden = args.get("hidden").and_then(Value::as_bool).unwrap_or(true);
                let topic = self.toggle_hide(community_id, topic_id, hidden)?;
                Ok(json!({ "topic": topic }))
            }
            "delete" => {
                let community_id = args
                    .get("communityId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "communityId required"))?;
                let topic_id = args
                    .get("topicId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "topicId required"))?
                    as u32;
                let deleted = self.delete_topic(community_id, topic_id)?;
                Ok(json!({ "deleted": deleted }))
            }
            "updateUnread" => {
                let community_id = args
                    .get("communityId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "communityId required"))?;
                let topic_id = args
                    .get("topicId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "topicId required"))?
                    as u32;
                let count = args
                    .get("unreadCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                let top_message_id = args.get("topMessageId").and_then(Value::as_u64);
                let topic =
                    self.update_unread(community_id, topic_id, count, top_message_id)?;
                Ok(json!({ "topic": topic }))
            }
            other => Err(Failure::new(
                "UNKNOWN_TOPIC_METHOD",
                format!("unrecognized topic method {other}"),
            )),
        }
    }
}
