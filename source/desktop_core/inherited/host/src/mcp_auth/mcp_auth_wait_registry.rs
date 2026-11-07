use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_MCP_AUTH_WAIT_TTL_MS: u64 = 60 * 60 * 1_000;

pub fn normalize_connector_name(value: &str) -> String {
    value.to_ascii_lowercase().chars()
        .filter(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuthCompletionIdentity { pub server_id: String, pub server_name: String }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuthWaitEvent {
    pub agent_id: String,
    pub connector: String,
    pub server_id: Option<String>,
}

#[derive(Debug, Clone)]
struct WaitEntry { agent_id: String, server_id: Option<String>, expires_at_ms: u64 }

pub struct McpAuthWaitRegistry {
    waits: HashMap<String, WaitEntry>,
    ttl_ms: u64,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl Default for McpAuthWaitRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_MCP_AUTH_WAIT_TTL_MS, Box::new(|| {
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()
                .as_millis().try_into().unwrap_or(u64::MAX)
        }))
    }
}

impl McpAuthWaitRegistry {
    pub fn new(ttl_ms: u64, now: Box<dyn Fn() -> u64 + Send + Sync>) -> Self {
        Self { waits: HashMap::new(), ttl_ms, now }
    }

    pub fn register(&mut self, event: McpAuthWaitEvent) {
        self.prune();
        let server_id=event.server_id.filter(|v| !v.is_empty());
        let name_key=normalize_connector_name(&event.connector);
        let key=if !name_key.is_empty() { Some(name_key) } else {
            server_id.as_ref().map(|id| format!("id:{id}"))
        };
        if let Some(key)=key {
            self.waits.insert(key,WaitEntry{
                agent_id:event.agent_id,
                server_id,
                expires_at_ms:(self.now)().saturating_add(self.ttl_ms),
            });
        }
    }

    pub fn take(&mut self, completion:&McpAuthCompletionIdentity)->Option<String>{
        self.prune();
        let name_key=normalize_connector_name(&completion.server_name);
        let mut by_server_id=None;
        let mut by_name=None;
        for key in self.waits.keys().cloned().collect::<Vec<_>>() {
            let Some(entry)=self.waits.get(&key).cloned() else { continue };
            let id_match=entry.server_id.as_deref().is_some_and(|id| id==completion.server_id);
            let name_match=entry.server_id.is_none() && !name_key.is_empty() && key==name_key;
            if !id_match && !name_match { continue; }
            self.waits.remove(&key);
            if id_match { by_server_id=Some(entry.agent_id); } else { by_name=Some(entry.agent_id); }
        }
        by_server_id.or(by_name)
    }

    pub fn prune(&mut self) {
        let now=(self.now)();
        self.waits.retain(|_,entry| entry.expires_at_ms>now);
    }

    pub fn len(&self)->usize { self.waits.len() }
    pub fn is_empty(&self)->bool { self.waits.is_empty() }
}
