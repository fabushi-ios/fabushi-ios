use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::protocol::Failure;

pub const CLIENT_SIDE_TOOL_V2_FAMILY: &str = "client-side-tool-v2";
pub const CLIENT_SIDE_TOOL_V2_WIRE_VERSION: u32 = 1;
pub const CLIENT_SIDE_TOOL_V2_ACCOUNT_SLOT: &str = "host";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolMessageKind {
    Call,
    Result,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodedToolMessage {
    pub encoding: String,
    pub message_type: String,
    pub bytes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTransportEvent {
    pub version: u32,
    pub kind: ToolMessageKind,
    pub account_slot: String,
    pub agent_id: String,
    pub epoch: String,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<EncodedToolMessage>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererToolEvent {
    pub version: u32,
    pub kind: ToolMessageKind,
    pub account_slot: String,
    pub agent_id: String,
    pub epoch: String,
    pub sequence: u64,
    pub message_type: Option<String>,
    pub bytes: Option<Vec<u8>>,
}

#[derive(Debug, Default)]
struct AgentFence {
    epoch: String,
    sequence: u64,
    retired_epochs: HashSet<String>,
    updates_by_tool_call_id: HashMap<String, Vec<ToolTransportEvent>>,
}

#[derive(Debug, Default)]
pub struct ClientSideToolV2Relay {
    agents: HashMap<String, AgentFence>,
    // Compatibility path for the first Rust slice. Kept until all callers
    // migrate to the versioned transport contract above.
    pending: HashMap<String, String>,
}

fn read_varint(bytes: &[u8], cursor: &mut usize) -> Option<u64> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    while *cursor < bytes.len() && shift <= 63 {
        let byte = bytes[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
    None
}

fn extract_length_delimited_field(bytes: &[u8], wanted_field: u32) -> Option<Vec<u8>> {
    let mut cursor = 0;
    while cursor < bytes.len() {
        let key = read_varint(bytes, &mut cursor)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x07) as u8;
        match wire {
            0 => {
                read_varint(bytes, &mut cursor)?;
            }
            1 => cursor = cursor.checked_add(8)?,
            2 => {
                let len = usize::try_from(read_varint(bytes, &mut cursor)?).ok()?;
                let end = cursor.checked_add(len)?;
                if end > bytes.len() {
                    return None;
                }
                if field == wanted_field {
                    return Some(bytes[cursor..end].to_vec());
                }
                cursor = end;
            }
            5 => cursor = cursor.checked_add(4)?,
            _ => return None,
        }
        if cursor > bytes.len() {
            return None;
        }
    }
    None
}

fn decode_message(kind: ToolMessageKind, message: &EncodedToolMessage) -> Option<(Vec<u8>, String)> {
    if message.encoding != "protobuf-base64" {
        return None;
    }
    let expected = match kind {
        ToolMessageKind::Call => "aiserver.v1.ClientSideToolV2Call",
        ToolMessageKind::Result => "aiserver.v1.ClientSideToolV2Result",
        ToolMessageKind::Reset => return None,
    };
    if message.message_type != expected || message.bytes.is_empty() || message.bytes.len() % 4 != 0 {
        return None;
    }
    let bytes = STANDARD.decode(&message.bytes).ok()?;
    if STANDARD.encode(&bytes) != message.bytes {
        return None;
    }
    let tool_call_field = match kind {
        ToolMessageKind::Call => 3,
        ToolMessageKind::Result => 35,
        ToolMessageKind::Reset => unreachable!(),
    };
    let tool_call_bytes = extract_length_delimited_field(&bytes, tool_call_field)?;
    let tool_call_id = String::from_utf8(tool_call_bytes).ok()?;
    if tool_call_id.is_empty() {
        return None;
    }
    Some((bytes, tool_call_id))
}

fn materialize(event: &ToolTransportEvent) -> Option<RendererToolEvent> {
    if event.kind == ToolMessageKind::Reset {
        return Some(RendererToolEvent {
            version: event.version,
            kind: event.kind,
            account_slot: event.account_slot.clone(),
            agent_id: event.agent_id.clone(),
            epoch: event.epoch.clone(),
            sequence: event.sequence,
            message_type: None,
            bytes: None,
        });
    }
    let message = event.message.as_ref()?;
    let (bytes, _) = decode_message(event.kind, message)?;
    Some(RendererToolEvent {
        version: event.version,
        kind: event.kind,
        account_slot: event.account_slot.clone(),
        agent_id: event.agent_id.clone(),
        epoch: event.epoch.clone(),
        sequence: event.sequence,
        message_type: Some(message.message_type.clone()),
        bytes: Some(bytes),
    })
}

impl ClientSideToolV2Relay {
    pub fn accept_value(&mut self, raw: Value) -> Option<RendererToolEvent> {
        let event = serde_json::from_value::<ToolTransportEvent>(raw).ok()?;
        self.accept(event)
    }

    pub fn accept(&mut self, event: ToolTransportEvent) -> Option<RendererToolEvent> {
        if event.version != CLIENT_SIDE_TOOL_V2_WIRE_VERSION
            || event.account_slot != CLIENT_SIDE_TOOL_V2_ACCOUNT_SLOT
            || event.agent_id.is_empty()
            || event.epoch.is_empty()
            || event.sequence < 1
        {
            return None;
        }

        if event.kind != ToolMessageKind::Reset && event.message.is_none() {
            return None;
        }

        let fence = self
            .agents
            .entry(event.agent_id.clone())
            .or_insert_with(|| AgentFence {
                epoch: event.epoch.clone(),
                ..AgentFence::default()
            });

        if event.epoch != fence.epoch {
            if fence.retired_epochs.contains(&event.epoch) {
                return None;
            }
            if !fence.epoch.is_empty() {
                fence.retired_epochs.insert(fence.epoch.clone());
            }
            fence.epoch = event.epoch.clone();
            fence.sequence = 0;
            fence.updates_by_tool_call_id.clear();
        }
        if event.sequence <= fence.sequence {
            return None;
        }
        fence.sequence = event.sequence;

        if event.kind == ToolMessageKind::Reset {
            fence.updates_by_tool_call_id.clear();
            return materialize(&event);
        }

        let (_, tool_call_id) = decode_message(event.kind, event.message.as_ref()?)?;
        match event.kind {
            ToolMessageKind::Call => {
                fence
                    .updates_by_tool_call_id
                    .insert(tool_call_id, vec![event.clone()]);
            }
            ToolMessageKind::Result => {
                let lifecycle = fence.updates_by_tool_call_id.get_mut(&tool_call_id)?;
                if lifecycle.first().is_none_or(|first| first.kind != ToolMessageKind::Call) {
                    return None;
                }
                let call = lifecycle[0].clone();
                *lifecycle = vec![call, event.clone()];
            }
            ToolMessageKind::Reset => unreachable!(),
        }
        materialize(&event)
    }

    pub fn replay(&self) -> Vec<RendererToolEvent> {
        let mut events = self
            .agents
            .values()
            .flat_map(|fence| fence.updates_by_tool_call_id.values())
            .flat_map(|updates| updates.iter())
            .collect::<Vec<_>>();
        events.sort_by_key(|event| event.sequence);
        events.into_iter().filter_map(materialize).collect()
    }

    pub fn clear(&mut self) {
        self.agents.clear();
        self.pending.clear();
    }

    pub fn begin(&mut self, call_id: impl Into<String>, tool_name: impl Into<String>) -> Result<(), Failure> {
        let call_id = call_id.into();
        if call_id.trim().is_empty() || self.pending.contains_key(&call_id) {
            return Err(Failure::new("TOOL_RELAY_DUPLICATE", "tool call id is empty or already pending"));
        }
        self.pending.insert(call_id, tool_name.into());
        Ok(())
    }

    pub fn settle(&mut self, call_id: &str, output: Value) -> Result<(String, Value), Failure> {
        let tool = self.pending.remove(call_id).ok_or_else(|| Failure::new(
            "TOOL_RELAY_UNKNOWN_CALL", format!("unknown client-side tool call {call_id}")
        ))?;
        Ok((tool, output))
    }
}
