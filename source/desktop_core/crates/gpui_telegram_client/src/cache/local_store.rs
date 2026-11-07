use gpui_telegram_core::error::Result;
use gpui_telegram_core::types::ChatId;
use gpui_telegram_proto::message::MessageItem;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Default, Clone)]
pub struct LocalStore {
    messages: Arc<RwLock<HashMap<ChatId, Vec<MessageItem>>>>,
}

impl LocalStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_message(&self, message: MessageItem) {
        self.messages
            .write()
            .entry(message.chat_id)
            .or_default()
            .push(message);
    }

    pub fn get_messages(&self, chat_id: ChatId) -> Result<Vec<MessageItem>> {
        Ok(self.messages.read().get(&chat_id).cloned().unwrap_or_default())
    }
}
