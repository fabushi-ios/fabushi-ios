use gpui_telegram_core::types::{ChatId, MessageFlags, MessageId, PeerId, UserId};
use gpui_telegram_proto::message::{MessageContent, MessageItem};

pub fn make_test_message(id: i64, chat_id: i64, user_id: u64, text: &str) -> MessageItem {
    let peer = PeerId::Chat(ChatId(chat_id));
    MessageItem {
        id: MessageId(id),
        chat_id: ChatId(chat_id),
        peer_id: peer,
        to_peer: peer,
        sender_id: UserId(user_id),
        topic_id: None,
        flags: MessageFlags::default(),
        date: chrono::Utc::now(),
        edit_date: None,
        content: MessageContent::Text {
            text: text.to_string(),
            entities: Vec::new(),
        },
        reply_to: None,
        forward_info: None,
        reactions: Vec::new(),
        client_nonce: Some(rand_nonce()),
        inline_keyboard: None,
        ttl_seconds: None,
        expires_at_ms: None,
    }
}

fn rand_nonce() -> u64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64
}
