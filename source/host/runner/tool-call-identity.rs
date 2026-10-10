use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};

const MAX_TRACKED: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSurfaceUpdate {
    pub name: Option<String>,
    pub fields: Map<String, Value>,
}

impl ToolSurfaceUpdate {
    fn with_name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }
}

#[derive(Debug)]
struct CappedMap<T> {
    values: HashMap<String, T>,
    order: VecDeque<String>,
}

impl<T> Default for CappedMap<T> {
    fn default() -> Self {
        Self { values: HashMap::new(), order: VecDeque::new() }
    }
}

impl<T> CappedMap<T> {
    fn cap_before_insert(&mut self) {
        if self.values.len() >= MAX_TRACKED {
            if let Some(key) = self.order.pop_front() {
                self.values.remove(&key);
            }
        }
    }

    fn insert(&mut self, key: String, value: T) {
        self.cap_before_insert();
        if self.values.contains_key(&key) {
            self.order.retain(|existing| existing != &key);
        }
        self.order.push_back(key.clone());
        self.values.insert(key, value);
    }

    fn get(&self, key: &str) -> Option<&T> {
        self.values.get(key)
    }

    fn remove(&mut self, key: &str) -> Option<T> {
        let removed = self.values.remove(key);
        if removed.is_some() {
            self.order.retain(|existing| existing != key);
        }
        removed
    }
}

pub struct ToolCallIdentity<Emit> {
    names: CappedMap<String>,
    held: CappedMap<ToolSurfaceUpdate>,
    emit_update: Emit,
}

impl<Emit> ToolCallIdentity<Emit>
where
    Emit: FnMut(ToolSurfaceUpdate),
{
    pub fn new(emit_update: Emit) -> Self {
        Self {
            names: CappedMap::default(),
            held: CappedMap::default(),
            emit_update,
        }
    }

    pub fn record_model_tool_name(&mut self, id: impl Into<String>, name: impl Into<String>) {
        let id = id.into();
        let name = name.into();
        self.names.insert(id.clone(), name.clone());
        if let Some(update) = self.held.remove(&id) {
            (self.emit_update)(update.with_name(name));
        }
    }

    pub fn resolve_model_tool_name(
        &mut self,
        event: &str,
        id: &str,
        outline: &str,
    ) -> String {
        let value = self.names.get(id).cloned().unwrap_or_else(|| outline.into());
        if event == "toolCallCompleted" {
            self.names.remove(id);
            self.held.remove(id);
        }
        value
    }

    pub fn stash_surface_unresolved_pending(
        &mut self,
        id: impl Into<String>,
        update: ToolSurfaceUpdate,
    ) {
        self.held.insert(id.into(), update);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn late_model_name_releases_held_surface_update() {
        let emitted = Rc::new(RefCell::new(Vec::new()));
        let sink = emitted.clone();
        let mut identity = ToolCallIdentity::new(move |update| sink.borrow_mut().push(update));
        identity.stash_surface_unresolved_pending(
            "call-1",
            ToolSurfaceUpdate { name: None, fields: Map::new() },
        );
        identity.record_model_tool_name("call-1", "browser.open");
        assert_eq!(emitted.borrow()[0].name.as_deref(), Some("browser.open"));
        assert_eq!(
            identity.resolve_model_tool_name("toolCallCompleted", "call-1", "fallback"),
            "browser.open"
        );
        assert_eq!(
            identity.resolve_model_tool_name("other", "call-1", "fallback"),
            "fallback"
        );
    }
}
