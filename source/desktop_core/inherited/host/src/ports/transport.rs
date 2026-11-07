use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct SandUpdate {
    pub kind: String,
    pub fields: Map<String, Value>,
}

pub struct SandTransport<F>
where
    F: FnMut(&SandUpdate) -> Option<String>,
{
    ingest: F,
    last_sent_message_id: Option<String>,
    last_reaction_applied: bool,
}

impl<F> SandTransport<F>
where
    F: FnMut(&SandUpdate) -> Option<String>,
{
    pub fn new(ingest: F) -> Self {
        Self { ingest, last_sent_message_id: None, last_reaction_applied: false }
    }

    pub fn on_update(&mut self, update: &SandUpdate) {
        let assigned_id = (self.ingest)(update);
        match update.kind.as_str() {
            "send-message" => self.last_sent_message_id = assigned_id,
            "react-to-message" => self.last_reaction_applied = assigned_id.is_some(),
            _ => {}
        }
    }

    pub fn last_sent_message_id(&self) -> Option<&str> { self.last_sent_message_id.as_deref() }
    pub fn last_reaction_applied(&self) -> bool { self.last_reaction_applied }
}
