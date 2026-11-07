//! Target Symbol: DataForum
//! Telegram Desktop 1:1 Submodule: `data/data_forum`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForumTopicSummary {
    pub root_id: i32,
    pub title: String,
    pub icon_id: u64,
    pub color_id: i32,
    pub is_closed: bool,
    pub is_pinned: bool,
    pub is_hidden: bool,
    pub unread_count: u32,
    pub last_message_date: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataForum {
    pub channel_id: u64,
    pub topics: HashMap<i32, ForumTopicSummary>,
    pub ordered_topic_ids: Vec<i32>,
    pub total_unread_topics: u32,
}

impl Default for DataForum {
    fn default() -> Self {
        Self {
            channel_id: 0,
            topics: HashMap::new(),
            ordered_topic_ids: Vec::new(),
            total_unread_topics: 0,
        }
    }
}

impl DataForum {
    pub fn new(channel_id: u64) -> Self {
        Self {
            channel_id,
            topics: HashMap::new(),
            ordered_topic_ids: Vec::new(),
            total_unread_topics: 0,
        }
    }

    pub fn apply_topic_added(&mut self, topic: ForumTopicSummary) {
        let id = topic.root_id;
        self.topics.insert(id, topic);
        if !self.ordered_topic_ids.contains(&id) {
            self.ordered_topic_ids.push(id);
        }
        self.sort_topics();
        self.recalculate_unread();
    }

    pub fn apply_topic_deleted(&mut self, root_id: i32) {
        self.topics.remove(&root_id);
        self.ordered_topic_ids.retain(|&id| id != root_id);
        self.recalculate_unread();
    }

    pub fn topic_for(&self, root_id: i32) -> Option<&ForumTopicSummary> {
        self.topics.get(&root_id)
    }

    pub fn topic_for_mut(&mut self, root_id: i32) -> Option<&mut ForumTopicSummary> {
        self.topics.get_mut(&root_id)
    }

    pub fn sort_topics(&mut self) {
        let topics = &self.topics;
        self.ordered_topic_ids.sort_by(|&a, &b| {
            let ta = topics.get(&a);
            let tb = topics.get(&b);
            match (ta, tb) {
                (Some(x), Some(y)) => {
                    if x.is_pinned != y.is_pinned {
                        y.is_pinned.cmp(&x.is_pinned)
                    } else {
                        y.last_message_date.cmp(&x.last_message_date)
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
    }

    pub fn recalculate_unread(&mut self) {
        self.total_unread_topics = self
            .topics
            .values()
            .filter(|t| t.unread_count > 0)
            .count() as u32;
    }

    pub fn clear_all_unreads(&mut self) {
        for topic in self.topics.values_mut() {
            topic.unread_count = 0;
        }
        self.total_unread_topics = 0;
    }
}
