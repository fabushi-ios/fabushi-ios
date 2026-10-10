use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const WIRE_VERSION: u32 = 1;
const ACCOUNT_SLOT: &str = "host";
pub(crate) const FAMILY: &str = "client-side-tool-v2";
const CALL_MESSAGE_TYPE: &str = "aiserver.v1.ClientSideToolV2Call";
const RESULT_MESSAGE_TYPE: &str = "aiserver.v1.ClientSideToolV2Result";

#[derive(Debug)]
pub(crate) struct ClientSideToolV2Producer {
    epoch: String,
    sequences: BTreeMap<String, u64>,
    open_calls: BTreeMap<String, BTreeSet<String>>,
}

impl Default for ClientSideToolV2Producer {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientSideToolV2Producer {
    pub(crate) fn new() -> Self {
        Self::with_epoch(Uuid::new_v4().to_string())
    }

    #[cfg(test)]
    pub(crate) fn with_epoch(epoch: impl Into<String>) -> Self {
        Self {
            epoch: epoch.into(),
            sequences: BTreeMap::new(),
            open_calls: BTreeMap::new(),
        }
    }

    #[cfg(not(test))]
    fn with_epoch(epoch: impl Into<String>) -> Self {
        Self {
            epoch: epoch.into(),
            sequences: BTreeMap::new(),
            open_calls: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn epoch(&self) -> &str {
        &self.epoch
    }

    pub(crate) fn publish_call(&mut self, agent_id: &str, tool_call_id: &str) -> Option<Value> {
        if agent_id.is_empty() || tool_call_id.is_empty() {
            return None;
        }
        self.open_calls
            .entry(agent_id.to_string())
            .or_default()
            .insert(tool_call_id.to_string());
        Some(self.event(
            agent_id,
            "call",
            Some((CALL_MESSAGE_TYPE, encode_identity(3, tool_call_id))),
        ))
    }

    pub(crate) fn publish_result(&mut self, agent_id: &str, tool_call_id: &str) -> Option<Value> {
        if agent_id.is_empty() || tool_call_id.is_empty() {
            return None;
        }
        let open = self.open_calls.get_mut(agent_id)?;
        if !open.remove(tool_call_id) {
            return None;
        }
        if open.is_empty() {
            self.open_calls.remove(agent_id);
        }
        Some(self.event(
            agent_id,
            "result",
            Some((RESULT_MESSAGE_TYPE, encode_identity(35, tool_call_id))),
        ))
    }

    #[cfg(test)]
    pub(crate) fn reset(&mut self, agent_id: &str) -> Option<Value> {
        if agent_id.is_empty() {
            return None;
        }
        self.open_calls.remove(agent_id);
        Some(self.event(agent_id, "reset", None))
    }

    fn event(&mut self, agent_id: &str, kind: &str, message: Option<(&str, Vec<u8>)>) -> Value {
        let sequence = self.next_sequence(agent_id);
        let message = message.map(|(message_type, bytes)| {
            json!({
                "encoding": "protobuf-base64",
                "messageType": message_type,
                "bytes": base64::engine::general_purpose::STANDARD.encode(bytes),
            })
        });
        let mut value = json!({
            "version": WIRE_VERSION,
            "kind": kind,
            "accountSlot": ACCOUNT_SLOT,
            "agentId": agent_id,
            "epoch": self.epoch,
            "sequence": sequence,
        });
        if let Some(message) = message {
            value["message"] = message;
        }
        value
    }

    fn next_sequence(&mut self, agent_id: &str) -> u64 {
        let next = self
            .sequences
            .get(agent_id)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        self.sequences.insert(agent_id.to_string(), next);
        next
    }
}

fn encode_identity(field_number: u64, tool_call_id: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(tool_call_id.len() + 8);
    push_varint(&mut bytes, (field_number << 3) | 2);
    push_varint(&mut bytes, tool_call_id.len() as u64);
    bytes.extend_from_slice(tool_call_id.as_bytes());
    bytes
}

fn push_varint(output: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn producer_fences_open_calls_and_orders_each_agent() {
        let mut producer = ClientSideToolV2Producer::with_epoch("epoch-1");
        let call = producer.publish_call("agent-a", "call-1").expect("call");
        assert_eq!(call["epoch"], "epoch-1");
        assert_eq!(call["sequence"], 1);
        assert_eq!(call["kind"], "call");
        let duplicate = producer.publish_call("agent-a", "call-1").expect("duplicate call envelope");
        assert_eq!(duplicate["sequence"], 2);
        assert!(producer.publish_result("agent-a", "unknown").is_none());
        let result = producer
            .publish_result("agent-a", "call-1")
            .expect("result");
        assert_eq!(result["sequence"], 3);
        assert_eq!(result["kind"], "result");
        let other = producer.publish_call("agent-b", "call-2").expect("other");
        assert_eq!(other["sequence"], 1);
    }

    #[test]
    fn reset_keeps_epoch_and_monotonic_sequence() {
        let mut producer = ClientSideToolV2Producer::with_epoch("epoch-1");
        producer.publish_call("agent-a", "call-1").expect("call");
        let reset = producer.reset("agent-a").expect("reset");
        assert_eq!(reset["epoch"], "epoch-1");
        assert_eq!(reset["sequence"], 2);
        assert_eq!(reset["kind"], "reset");
        assert!(producer.publish_result("agent-a", "call-1").is_none());
        let next = producer.publish_call("agent-a", "call-2").expect("next");
        assert_eq!(next["sequence"], 3);
    }
}
