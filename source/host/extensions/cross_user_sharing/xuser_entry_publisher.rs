use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

type RelaySend = Arc<dyn Fn(&Value) -> Result<Value, String> + Send + Sync>;

pub struct SandXuserEntryPublisher {
    send: RelaySend,
    ordered: Mutex<()>,
}

impl SandXuserEntryPublisher {
    pub fn new(send: RelaySend) -> Self {
        Self { send, ordered: Mutex::new(()) }
    }

    pub fn publish_human_message(
        &self,
        room_id: &str,
        entry_id: &str,
        author_auth_id: &str,
        author_name: &str,
        text: &str,
    ) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        self.publish(room_id, json!({
            "kind": "human-message",
            "entryId": entry_id,
            "authorAuthId": author_auth_id,
            "authorName": author_name,
            "text": text,
            "images": [],
            "timestampMs": now_ms(),
        }))
    }

    pub fn publish_agent_message(
        &self,
        room_id: &str,
        entry_id: &str,
        owner_auth_id: &str,
        agent_id: &str,
        agent_name: &str,
        text: &str,
    ) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        self.publish(room_id, json!({
            "kind": "agent-message",
            "entryId": entry_id,
            "agentOwnerAuthId": owner_auth_id,
            "agentId": agent_id,
            "agentName": agent_name,
            "text": text,
            "images": [],
            "timestampMs": now_ms(),
        }))
    }

    fn publish(&self, room_id: &str, entry: Value) -> Result<(), String> {
        let room_id = room_id.trim();
        if room_id.is_empty() {
            return Err("roomId is required".into());
        }
        let _ordered = self.ordered.lock().unwrap_or_else(|p| p.into_inner());
        (self.send)(&json!({
            "kind": "room-entry",
            "roomId": room_id,
            "entry": entry,
        }))?;
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publisher_orders_real_room_entry_wire_shapes() {
        let sent = Arc::new(Mutex::new(Vec::<Value>::new()));
        let out = Arc::clone(&sent);
        let publisher = SandXuserEntryPublisher::new(Arc::new(move |value| {
            out.lock().unwrap().push(value.clone());
            Ok(json!({"ok": true}))
        }));
        publisher.publish_human_message("room", "u1", "me", "Me", "hello").unwrap();
        publisher.publish_agent_message("room", "a1", "me", "agent", "Agent", "reply").unwrap();
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["kind"], "room-entry");
        assert_eq!(sent[0]["entry"]["kind"], "human-message");
        assert_eq!(sent[1]["entry"]["kind"], "agent-message");
    }
}
