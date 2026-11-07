//! Generation-fenced state for optional Node Host publications.
//!
//! This module deliberately does not choose a plugin/skills/runner producer,
//! open a socket, own a timer, or persist product state. It gives the future
//! connection owner one narrow place to retain the latest payload and to
//! reject callbacks from retired publication attempts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const MAX_OPTIONAL_PUBLICATION_BYTES: usize = 256 * 1024;
pub const MAX_RETRY_AFTER_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum NodeHostOptionalPublicationKind {
    PluginTools,
    Skills,
    RunnerInventory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeHostOptionalPublicationOutcome {
    Published,
    Unsupported,
    Rejected,
    Retryable { retry_after_ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeHostOptionalPublicationStatus {
    Pending,
    Settled(NodeHostOptionalPublicationOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeHostOptionalPublicationRecord {
    pub kind: NodeHostOptionalPublicationKind,
    pub generation: u64,
    pub attempt: u32,
    pub payload_json: String,
    pub status: NodeHostOptionalPublicationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHostOptionalPublicationHandle {
    kind: NodeHostOptionalPublicationKind,
    generation: u64,
    attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeHostOptionalPublicationSettlement {
    Applied(NodeHostOptionalPublicationRecord),
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeHostOptionalPublicationError {
    InvalidGeneration,
    EmptyPayload,
    PayloadTooLarge,
    InvalidJson,
}

impl std::fmt::Display for NodeHostOptionalPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidGeneration => "optional publication generation must be non-zero",
            Self::EmptyPayload => "optional publication payload is empty",
            Self::PayloadTooLarge => "optional publication payload exceeds the size bound",
            Self::InvalidJson => "optional publication payload is not valid JSON",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for NodeHostOptionalPublicationError {}

#[derive(Debug, Default)]
pub struct NodeHostOptionalPublicationOwner {
    generation: u64,
    records: BTreeMap<NodeHostOptionalPublicationKind, NodeHostOptionalPublicationRecord>,
}

impl NodeHostOptionalPublicationOwner {
    pub fn new(generation: u64) -> Result<Self, NodeHostOptionalPublicationError> {
        if generation == 0 {
            return Err(NodeHostOptionalPublicationError::InvalidGeneration);
        }
        Ok(Self {
            generation,
            records: BTreeMap::new(),
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn begin(
        &mut self,
        kind: NodeHostOptionalPublicationKind,
        payload_json: String,
    ) -> Result<NodeHostOptionalPublicationHandle, NodeHostOptionalPublicationError> {
        validate_payload(&payload_json)?;
        let attempt = self
            .records
            .get(&kind)
            .map_or(1, |record| record.attempt.saturating_add(1));
        self.records.insert(
            kind,
            NodeHostOptionalPublicationRecord {
                kind,
                generation: self.generation,
                attempt,
                payload_json,
                status: NodeHostOptionalPublicationStatus::Pending,
            },
        );
        Ok(NodeHostOptionalPublicationHandle {
            kind,
            generation: self.generation,
            attempt,
        })
    }

    pub fn settle(
        &mut self,
        handle: &NodeHostOptionalPublicationHandle,
        outcome: NodeHostOptionalPublicationOutcome,
    ) -> NodeHostOptionalPublicationSettlement {
        let Some(record) = self.records.get_mut(&handle.kind) else {
            return NodeHostOptionalPublicationSettlement::Stale;
        };
        if record.generation != self.generation
            || handle.generation != self.generation
            || record.attempt != handle.attempt
            || !matches!(&record.status, NodeHostOptionalPublicationStatus::Pending)
        {
            return NodeHostOptionalPublicationSettlement::Stale;
        }
        let outcome = match outcome {
            NodeHostOptionalPublicationOutcome::Retryable { retry_after_ms } => {
                NodeHostOptionalPublicationOutcome::Retryable {
                    retry_after_ms: retry_after_ms.min(MAX_RETRY_AFTER_MS),
                }
            }
            outcome => outcome,
        };
        record.status = NodeHostOptionalPublicationStatus::Settled(outcome);
        NodeHostOptionalPublicationSettlement::Applied(record.clone())
    }

    pub fn current(
        &self,
        kind: NodeHostOptionalPublicationKind,
    ) -> Option<&NodeHostOptionalPublicationRecord> {
        self.records.get(&kind)
    }

    pub fn retire_to(
        &mut self,
        generation: u64,
    ) -> Result<(), NodeHostOptionalPublicationError> {
        if generation == 0 || generation <= self.generation {
            return Err(NodeHostOptionalPublicationError::InvalidGeneration);
        }
        self.generation = generation;
        self.records.clear();
        Ok(())
    }
}

fn validate_payload(payload_json: &str) -> Result<(), NodeHostOptionalPublicationError> {
    if payload_json.is_empty() {
        return Err(NodeHostOptionalPublicationError::EmptyPayload);
    }
    if payload_json.len() > MAX_OPTIONAL_PUBLICATION_BYTES {
        return Err(NodeHostOptionalPublicationError::PayloadTooLarge);
    }
    serde_json::from_str::<serde_json::Value>(payload_json)
        .map(|_| ())
        .map_err(|_| NodeHostOptionalPublicationError::InvalidJson)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_attempt_cannot_settle_replaced_payload() {
        let mut owner = NodeHostOptionalPublicationOwner::new(1).unwrap();
        let first = owner
            .begin(NodeHostOptionalPublicationKind::Skills, r#"{"v":1}"#.into())
            .unwrap();
        let second = owner
            .begin(NodeHostOptionalPublicationKind::Skills, r#"{"v":2}"#.into())
            .unwrap();
        assert_eq!(
            owner.settle(
                &first,
                NodeHostOptionalPublicationOutcome::Published
            ),
            NodeHostOptionalPublicationSettlement::Stale
        );
        assert!(matches!(
            owner.settle(
                &second,
                NodeHostOptionalPublicationOutcome::Retryable {
                    retry_after_ms: MAX_RETRY_AFTER_MS * 2,
                }
            ),
            NodeHostOptionalPublicationSettlement::Applied(_)
        ));
        assert_eq!(
            &owner
                .current(NodeHostOptionalPublicationKind::Skills)
                .unwrap()
                .status,
            &NodeHostOptionalPublicationStatus::Settled(
                NodeHostOptionalPublicationOutcome::Retryable {
                    retry_after_ms: MAX_RETRY_AFTER_MS,
                }
            )
        );
    }

    #[test]
    fn retirement_removes_old_publications() {
        let mut owner = NodeHostOptionalPublicationOwner::new(3).unwrap();
        let handle = owner
            .begin(
                NodeHostOptionalPublicationKind::RunnerInventory,
                "[]".into(),
            )
            .unwrap();
        owner.retire_to(4).unwrap();
        assert_eq!(owner.generation(), 4);
        assert!(owner
            .current(NodeHostOptionalPublicationKind::RunnerInventory)
            .is_none());
        assert_eq!(
            owner.settle(&handle, NodeHostOptionalPublicationOutcome::Published),
            NodeHostOptionalPublicationSettlement::Stale
        );
    }
}
