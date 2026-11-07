use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub require_confirmation: bool,
}

impl PluginToolDefinition {
    pub fn new(name: impl Into<String>, description: impl Into<String>, input_schema: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            require_confirmation: false,
        }
    }

    pub fn validate_arguments(&self, args: &Value) -> Result<(), String> {
        let obj = args.as_object().ok_or_else(|| "Tool arguments must be a JSON object".to_string())?;

        // Check required fields from schema if present
        if let Some(required) = self.input_schema.get("required").and_then(|r| r.as_array()) {
            for req in required {
                if let Some(req_name) = req.as_str() {
                    if !obj.contains_key(req_name) {
                        return Err(format!("Missing required parameter: '{req_name}'"));
                    }
                }
            }
        }
        Ok(())
    }
}

pub type CommandHandler = Arc<dyn Fn(&Value) -> Result<Value, String> + Send + Sync>;

#[derive(Clone, Default)]
pub struct PluginCommandDispatcher {
    handlers: Arc<Mutex<HashMap<String, CommandHandler>>>,
}

impl PluginCommandDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(&self, name: &str, handler: F)
    where
        F: Fn(&Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        let mut handlers = self.handlers.lock().unwrap();
        handlers.insert(name.to_string(), Arc::new(handler));
    }

    pub fn dispatch(&self, name: &str, args: &Value, _timeout: Duration) -> Result<Value, String> {
        let handler = {
            let handlers = self.handlers.lock().unwrap();
            handlers.get(name).cloned().ok_or_else(|| format!("Unknown command: '{name}'"))?
        };

        handler(args)
    }
}

#[derive(Clone, Default)]
pub struct BrowserHookBridge {
    tab_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl BrowserHookBridge {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn acquire_tab_control(&self, target_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.tab_locks.lock().unwrap();
        locks.entry(target_id.to_string()).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
    }

    pub fn execute_with_tab_lock<T, F>(&self, target_id: &str, action: F) -> Result<T, String>
    where
        F: FnOnce() -> Result<T, String>,
    {
        let tab_lock = self.acquire_tab_control(target_id);
        let _guard = tab_lock.lock().map_err(|_| "Failed to lock browser tab control".to_string())?;
        action()
    }
}
