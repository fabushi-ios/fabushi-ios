use gpui_telegram_core::types::{ChatId, UserId};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct SfuBridge {
    pub room_subscribers: HashMap<ChatId, Vec<UserId>>,
}

impl SfuBridge {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn join_room(&mut self, chat_id: ChatId, user_id: UserId) {
        let subs = self.room_subscribers.entry(chat_id).or_default();
        if !subs.contains(&user_id) {
            subs.push(user_id);
        }
    }

    pub fn leave_room(&mut self, chat_id: ChatId, user_id: UserId) {
        if let Some(subs) = self.room_subscribers.get_mut(&chat_id) {
            subs.retain(|&id| id != user_id);
        }
    }

    pub fn get_subscribers(&self, chat_id: ChatId) -> &[UserId] {
        self.room_subscribers
            .get(&chat_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}
