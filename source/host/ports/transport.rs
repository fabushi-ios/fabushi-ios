use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct SandUpdate {
    pub update_type: String,
    pub fields: Map<String, Value>,
}

pub struct SandTransport<Ingest> {
    ingest: Ingest,
    last_sent_message_id: Option<String>,
    last_reaction_applied: bool,
}

impl<Ingest> SandTransport<Ingest>
where
    Ingest: FnMut(&SandUpdate) -> Option<String>,
{
    pub fn new(ingest: Ingest) -> Self {
        Self {
            ingest,
            last_sent_message_id: None,
            last_reaction_applied: false,
        }
    }

    pub fn on_update(&mut self, update: &SandUpdate) {
        let assigned_id = (self.ingest)(update);
        match update.update_type.as_str() {
            "send-message" => self.last_sent_message_id = assigned_id,
            "react-to-message" => self.last_reaction_applied = assigned_id.is_some(),
            _ => {}
        }
    }

    pub fn last_sent_message_id(&self) -> Option<&str> {
        self.last_sent_message_id.as_deref()
    }

    pub fn last_reaction_applied(&self) -> bool {
        self.last_reaction_applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracks_send_id_and_reaction_acceptance_from_ingest_result() {
        let mut next = 0u8;
        let mut transport = SandTransport::new(|update: &SandUpdate| {
            next += 1;
            (update.update_type != "drop").then(|| format!("id-{next}"))
        });
        transport.on_update(&SandUpdate { update_type: "send-message".into(), fields: Map::new() });
        assert_eq!(transport.last_sent_message_id(), Some("id-1"));
        transport.on_update(&SandUpdate { update_type: "react-to-message".into(), fields: Map::new() });
        assert!(transport.last_reaction_applied());
    }
}
