use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptArgs {
    pub source_agent_id: String,
}

impl AdoptArgs {
    pub fn from_value(value: &Value) -> Self {
        Self {
            source_agent_id: value
                .get("source_agent_id")
                .or_else(|| value.get("sourceAgentId"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum AdoptOutcome {
    Unspecified = 0,
    AlreadyParented = 1,
    EdgeOnly = 2,
    StoreImportCompleted = 3,
}

impl Serialize for AdoptOutcome {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i32(*self as i32)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "case", content = "value")]
pub enum AdoptResultValue {
    #[serde(rename = "success")]
    Success(AdoptOutcome),
    #[serde(rename = "error")]
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptResult {
    pub source_agent_id: String,
    pub target_agent_id: String,
    pub project_root_id: String,
    pub result: Option<AdoptResultValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdoptionEdge {
    pub target_agent_id: String,
    pub project_root_id: String,
}

/// A consistent snapshot of a live source agent's store at the adoption boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStoreSnapshot {
    pub project_root_id: String,
    pub updated_at_ms: i64,
    pub history: Vec<Value>,
}

/// The imported source store is durable with the target NativeSession. The complete
/// source history is retained; model context uses a bounded, untrusted projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptedAgentStore {
    pub source_agent_id: String,
    pub project_root_id: String,
    pub source_updated_at_ms: i64,
    pub imported_at_ms: i64,
    pub history: Vec<Value>,
}

pub struct AdoptExecutorResource;

impl AdoptExecutorResource {
    pub fn execute(
        args: AdoptArgs,
        target: &str,
        root: &str,
        source_store: Result<Option<AgentStoreSnapshot>, String>,
        imported_at_ms: i64,
        edges: &mut BTreeMap<String, AgentAdoptionEdge>,
        stores: &mut BTreeMap<String, AdoptedAgentStore>,
    ) -> AdoptResult {
        let source = args.source_agent_id.trim().to_owned();
        let target = target.trim().to_owned();
        let root = root.trim().to_owned();
        let mut output = AdoptResult {
            source_agent_id: source.clone(),
            target_agent_id: target.clone(),
            project_root_id: root.clone(),
            result: None,
        };

        let validation_error = if source.is_empty() {
            Some("sourceAgentId is required")
        } else if source.chars().count() > 512 {
            Some("sourceAgentId must not exceed 512 characters")
        } else if target.is_empty() {
            Some("targetAgentId is required")
        } else if root.is_empty() {
            Some("projectRootId requires an active workspace root")
        } else if source == target {
            Some("cannot adopt an agent into itself")
        } else {
            None
        };
        if let Some(message) = validation_error {
            output.result = Some(AdoptResultValue::Error(message.to_owned()));
            return output;
        }

        if let Some(edge) = edges.get(&source) {
            if edge.target_agent_id != target || edge.project_root_id != root {
                output.result = Some(AdoptResultValue::Error(
                    "source agent is already adopted by a different target or project root"
                        .to_owned(),
                ));
                return output;
            }
            if stores.contains_key(&source) {
                output.result = Some(AdoptResultValue::Success(AdoptOutcome::AlreadyParented));
                return output;
            }

            let snapshot = match source_store {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => {
                    output.result =
                        Some(AdoptResultValue::Success(AdoptOutcome::AlreadyParented));
                    return output;
                }
                Err(message) => {
                    output.result = Some(AdoptResultValue::Error(message));
                    return output;
                }
            };
            if snapshot.project_root_id.trim() != root {
                output.result = Some(AdoptResultValue::Error(
                    "source agent belongs to a different project root".to_owned(),
                ));
                return output;
            }
            if !has_importable_history(&snapshot.history) {
                output.result = Some(AdoptResultValue::Success(AdoptOutcome::AlreadyParented));
                return output;
            }

            stores.insert(
                source.clone(),
                AdoptedAgentStore {
                    source_agent_id: source,
                    project_root_id: root,
                    source_updated_at_ms: snapshot.updated_at_ms,
                    imported_at_ms,
                    history: snapshot.history,
                },
            );
            output.result = Some(AdoptResultValue::Success(
                AdoptOutcome::StoreImportCompleted,
            ));
            return output;
        }

        let snapshot = match source_store {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => {
                output.result = Some(AdoptResultValue::Error(
                    "source agent was not found in the current Native Host session registry"
                        .to_owned(),
                ));
                return output;
            }
            Err(message) => {
                output.result = Some(AdoptResultValue::Error(message));
                return output;
            }
        };
        if snapshot.project_root_id.trim() != root {
            output.result = Some(AdoptResultValue::Error(
                "source agent belongs to a different project root".to_owned(),
            ));
            return output;
        }

        let import_store = has_importable_history(&snapshot.history);
        edges.insert(
            source.clone(),
            AgentAdoptionEdge {
                target_agent_id: target,
                project_root_id: root.clone(),
            },
        );
        if import_store {
            stores.insert(
                source.clone(),
                AdoptedAgentStore {
                    source_agent_id: source,
                    project_root_id: root,
                    source_updated_at_ms: snapshot.updated_at_ms,
                    imported_at_ms,
                    history: snapshot.history,
                },
            );
            output.result = Some(AdoptResultValue::Success(
                AdoptOutcome::StoreImportCompleted,
            ));
        } else {
            output.result = Some(AdoptResultValue::Success(AdoptOutcome::EdgeOnly));
        }
        output
    }
}

fn has_importable_history(history: &[Value]) -> bool {
    history.iter().any(|entry| {
        matches!(
            entry.get("role").and_then(Value::as_str),
            Some("user") | Some("assistant") | Some("tool")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(root: &str, history: Vec<Value>) -> AgentStoreSnapshot {
        AgentStoreSnapshot {
            project_root_id: root.to_owned(),
            updated_at_ms: 7,
            history,
        }
    }

    #[test]
    fn generated_contract_and_outcome_numbers() {
        let mut edges = BTreeMap::new();
        let mut stores = BTreeMap::new();
        let result = AdoptExecutorResource::execute(
            AdoptArgs::from_value(&json!({"sourceAgentId": " source "})),
            "target",
            "/workspace",
            Ok(Some(snapshot(
                "/workspace",
                vec![json!({"role": "user", "content": "source request"})],
            ))),
            42,
            &mut edges,
            &mut stores,
        );
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["sourceAgentId"], "source");
        assert_eq!(value["result"]["value"].as_i64(), Some(3));
        assert_eq!(
            serde_json::to_value(AdoptOutcome::StoreImportCompleted)
                .unwrap()
                .as_i64(),
            Some(3)
        );
        assert_eq!(stores["source"].imported_at_ms, 42);
        assert_eq!(stores["source"].history[0]["content"], "source request");
    }

    #[test]
    fn edge_only_can_recover_into_store_import_and_is_idempotent() {
        let mut edges = BTreeMap::new();
        let mut stores = BTreeMap::new();
        let args = AdoptArgs::from_value(&json!({"source_agent_id": "source"}));

        let first = AdoptExecutorResource::execute(
            args.clone(),
            "target",
            "/workspace",
            Ok(Some(snapshot("/workspace", Vec::new()))),
            10,
            &mut edges,
            &mut stores,
        );
        assert_eq!(first.result, Some(AdoptResultValue::Success(AdoptOutcome::EdgeOnly)));
        assert!(stores.is_empty());

        let recovered = AdoptExecutorResource::execute(
            args.clone(),
            "target",
            "/workspace",
            Ok(Some(snapshot(
                "/workspace",
                vec![json!({"role": "assistant", "content": "source work"})],
            ))),
            11,
            &mut edges,
            &mut stores,
        );
        assert_eq!(
            recovered.result,
            Some(AdoptResultValue::Success(AdoptOutcome::StoreImportCompleted))
        );
        assert_eq!(stores["source"].history[0]["content"], "source work");

        let repeated = AdoptExecutorResource::execute(
            args,
            "target",
            "/workspace",
            Ok(None),
            12,
            &mut edges,
            &mut stores,
        );
        assert_eq!(
            repeated.result,
            Some(AdoptResultValue::Success(AdoptOutcome::AlreadyParented))
        );
        assert_eq!(stores.len(), 1);
    }

    #[test]
    fn missing_busy_and_cross_root_sources_fail_without_mutation() {
        let mut edges = BTreeMap::new();
        let mut stores = BTreeMap::new();
        let args = AdoptArgs::from_value(&json!({"source_agent_id": "source"}));

        let missing = AdoptExecutorResource::execute(
            args.clone(),
            "target",
            "/workspace",
            Ok(None),
            1,
            &mut edges,
            &mut stores,
        );
        assert!(matches!(missing.result, Some(AdoptResultValue::Error(_))));
        let busy = AdoptExecutorResource::execute(
            args.clone(),
            "target",
            "/workspace",
            Err("source agent is busy; retry adoption after its current turn completes".into()),
            2,
            &mut edges,
            &mut stores,
        );
        assert!(matches!(busy.result, Some(AdoptResultValue::Error(_))));
        let wrong_root = AdoptExecutorResource::execute(
            args,
            "target",
            "/workspace",
            Ok(Some(snapshot(
                "/another-workspace",
                vec![json!({"role": "user", "content": "do not import"} )],
            ))),
            3,
            &mut edges,
            &mut stores,
        );
        assert!(matches!(wrong_root.result, Some(AdoptResultValue::Error(_))));
        assert!(edges.is_empty());
        assert!(stores.is_empty());
    }

    #[test]
    fn invalid_scope_self_adoption_and_parent_conflicts_fail_closed() {
        let mut edges = BTreeMap::new();
        let mut stores = BTreeMap::new();

        let missing_source = AdoptExecutorResource::execute(
            AdoptArgs::from_value(&json!({})),
            "target",
            "/workspace",
            Ok(None),
            1,
            &mut edges,
            &mut stores,
        );
        assert!(matches!(
            missing_source.result,
            Some(AdoptResultValue::Error(_))
        ));
        let self_adoption = AdoptExecutorResource::execute(
            AdoptArgs::from_value(&json!({"source_agent_id": "target"})),
            "target",
            "/workspace",
            Ok(None),
            2,
            &mut edges,
            &mut stores,
        );
        assert!(matches!(
            self_adoption.result,
            Some(AdoptResultValue::Error(_))
        ));
        let no_root = AdoptExecutorResource::execute(
            AdoptArgs::from_value(&json!({"source_agent_id": "source"})),
            "target",
            "",
            Ok(None),
            3,
            &mut edges,
            &mut stores,
        );
        assert!(matches!(no_root.result, Some(AdoptResultValue::Error(_))));
        assert!(edges.is_empty());

        edges.insert(
            "source".into(),
            AgentAdoptionEdge {
                target_agent_id: "other-target".into(),
                project_root_id: "/workspace".into(),
            },
        );
        let conflict = AdoptExecutorResource::execute(
            AdoptArgs::from_value(&json!({"source_agent_id": "source"})),
            "target",
            "/workspace",
            Ok(None),
            4,
            &mut edges,
            &mut stores,
        );
        assert!(matches!(conflict.result, Some(AdoptResultValue::Error(_))));
        assert!(stores.is_empty());
    }
}
