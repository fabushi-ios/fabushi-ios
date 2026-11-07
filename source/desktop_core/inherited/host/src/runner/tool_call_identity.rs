use std::collections::{HashMap, VecDeque};

use serde_json::{Map, Value};

pub const TOOL_CALL_IDENTITY_CAP: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSurfaceUpdate {
    pub fields: Map<String, Value>,
}

impl ToolSurfaceUpdate {
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.fields.insert("name".into(), Value::String(name.into()));
        self
    }
}

#[derive(Debug, Default)]
pub struct ToolCallIdentity {
    names: HashMap<String, String>,
    name_order: VecDeque<String>,
    held: HashMap<String, ToolSurfaceUpdate>,
    held_order: VecDeque<String>,
}

impl ToolCallIdentity {
    pub fn record_model_tool_name(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
    ) -> Option<ToolSurfaceUpdate> {
        let id = id.into();
        let name = name.into();
        self.insert_name(id.clone(), name.clone());
        self.held.remove(&id).map(|update| {
            self.held_order.retain(|key| key != &id);
            update.with_name(name)
        })
    }

    pub fn resolve_model_tool_name(
        &mut self,
        event: &str,
        id: &str,
        outline: &str,
    ) -> String {
        let resolved = self
            .names
            .get(id)
            .cloned()
            .unwrap_or_else(|| outline.to_string());
        if event == "toolCallCompleted" {
            self.names.remove(id);
            self.held.remove(id);
            self.name_order.retain(|key| key != id);
            self.held_order.retain(|key| key != id);
        }
        resolved
    }

    pub fn stash_surface_unresolved_pending(
        &mut self,
        id: impl Into<String>,
        update: ToolSurfaceUpdate,
    ) {
        let id = id.into();
        if !self.held.contains_key(&id) {
            self.held_order.push_back(id.clone());
        }
        self.held.insert(id.clone(), update);
        while self.held.len() > TOOL_CALL_IDENTITY_CAP {
            let Some(oldest) = self.held_order.pop_front() else {
                break;
            };
            self.held.remove(&oldest);
        }
    }

    pub fn name_count(&self) -> usize {
        self.names.len()
    }

    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    fn insert_name(&mut self, id: String, name: String) {
        if !self.names.contains_key(&id) {
            self.name_order.push_back(id.clone());
        }
        self.names.insert(id, name);
        while self.names.len() > TOOL_CALL_IDENTITY_CAP {
            let Some(oldest) = self.name_order.pop_front() else {
                break;
            };
            self.names.remove(&oldest);
        }
    }
}
