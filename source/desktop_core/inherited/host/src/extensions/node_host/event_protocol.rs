//! Strict node.event parameter construction.
//!
//! The connection/runtime owners provide the event name and optional payload;
//! this module owns only the canonical `event`/`payloadJSON` envelope. It does
//! not publish frames, own sockets, or decide which events are observable.

use serde::Serialize;
use serde_json::Value;
use super::runtime_state::NodeHostRuntimeState;
use super::session_registry::NodeHostSessionLease;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeEventParams {
    pub event: String,
    #[serde(rename = "payloadJSON")]
    pub payload_json: Option<String>,
}

/// A generation-fenced event ready for a future Gateway transport owner.
///
/// The transport must call `take_if_current` immediately before sending. A
/// reconnect or disconnect retires the lease and makes the publication inert.
#[derive(Debug, Clone)]
pub struct NodeEventPublication {
    runtime: Arc<NodeHostRuntimeState>,
    lease: NodeHostSessionLease,
    params: NodeEventParams,
}

impl NodeEventPublication {
    pub fn admit(
        runtime: Arc<NodeHostRuntimeState>,
        lease: NodeHostSessionLease,
        params: NodeEventParams,
    ) -> Result<Self, String> {
        if !runtime.is_current(&lease) {
            return Err("node.event publication requires the current session generation".into());
        }
        Ok(Self { runtime, lease, params })
    }

    pub fn take_if_current(&self) -> Option<NodeEventParams> {
        self.runtime
            .is_current(&self.lease)
            .then(|| self.params.clone())
    }
}

pub fn build_node_event_params(
    event: impl Into<String>,
    payload: Option<&Value>,
) -> Result<NodeEventParams, String> {
    let event = event.into();
    if event.trim().is_empty() {
        return Err("node.event event name must not be empty".into());
    }
    let payload_json = payload
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| format!("node.event payload serialization failed: {error}"))?;
    Ok(NodeEventParams {
        event: event.into(),
        payload_json,
    })
}

#[cfg(test)]
mod tests {
    use super::{build_node_event_params, NodeEventPublication};
    use crate::extensions::node_host::runtime_state::NodeHostRuntimeState;
    use crate::extensions::node_host::session_registry::{
        NodeHostSessionMetadata, NodeHostSessionLease,
    };
    use std::sync::Arc;

    fn lease(runtime: &NodeHostRuntimeState, node_id: &str) -> NodeHostSessionLease {
        runtime.connect(NodeHostSessionMetadata {
            node_id: node_id.into(),
            display_name: node_id.into(),
            platform: "test".into(),
            device_family: "test".into(),
            commands: vec![],
            capabilities: vec![],
            paired: true,
            gateway_local: false,
        })
    }

    #[test]
    fn preserves_event_name_and_serializes_payload_json() {
        let params = build_node_event_params(
            "system.run.progress",
            Some(&serde_json::json!({"seq": 2, "text": "héllo"})),
        )
        .expect("payload serializes");
        assert_eq!(params.event, "system.run.progress");
        assert_eq!(
            params.payload_json.as_deref(),
            Some(r#"{"seq":2,"text":"héllo"}"#)
        );
    }

    #[test]
    fn represents_undefined_payload_as_json_null_field() {
        let params = build_node_event_params("node.ready", None).expect("event builds");
        assert_eq!(params.payload_json, None);
        assert_eq!(
            serde_json::to_value(params).expect("params serialize"),
            serde_json::json!({"event":"node.ready","payloadJSON":null})
        );
    }

    #[test]
    fn rejects_blank_event_names() {
        assert!(build_node_event_params("  ", None).is_err());
    }

    #[test]
    fn publication_is_retired_by_disconnect_or_successor_generation() {
        let runtime = Arc::new(NodeHostRuntimeState::new());
        let predecessor = lease(&runtime, "node-1");
        let publication = NodeEventPublication::admit(
            Arc::clone(&runtime),
            predecessor.clone(),
            build_node_event_params("node.ready", None).expect("event builds"),
        )
        .expect("current generation admits event");
        assert!(publication.take_if_current().is_some());
        let successor = lease(&runtime, "node-1");
        assert_ne!(predecessor.generation, successor.generation);
        assert!(publication.take_if_current().is_none());
    }
}
