use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOAuth {
    pub origin: String,
    pub state: String,
    pub server_name: String,
    pub expires_at_ms: u64,
}

#[derive(Debug, Default)]
pub struct OAuthPendingRegistry {
    by_key: HashMap<(String, String), PendingOAuth>,
}

impl OAuthPendingRegistry {
    pub fn track(&mut self, pending: PendingOAuth) {
        self.remove_server(&pending.server_name);
        self.by_key.insert((pending.origin.clone(), pending.state.clone()), pending);
    }

    pub fn resolve(&self, origin: &str, state: &str, now_ms: u64) -> Option<&str> {
        self.by_key
            .get(&(origin.to_string(), state.to_string()))
            .filter(|entry| entry.expires_at_ms > now_ms)
            .map(|entry| entry.server_name.as_str())
    }

    pub fn remove(&mut self, origin: &str, state: &str) -> Option<PendingOAuth> {
        self.by_key.remove(&(origin.to_string(), state.to_string()))
    }

    pub fn expire(&mut self, now_ms: u64) {
        self.by_key.retain(|_, entry| entry.expires_at_ms > now_ms);
    }

    pub fn expire_with_removed(&mut self, now_ms: u64) -> Vec<PendingOAuth> {
        let expired = self
            .by_key
            .iter()
            .filter_map(|(key, entry)| (entry.expires_at_ms <= now_ms).then(|| key.clone()))
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|key| self.by_key.remove(&key))
            .collect()
    }

    pub fn remove_server(&mut self, server_name: &str) {
        self.by_key.retain(|_, entry| entry.server_name != server_name);
    }

    pub fn remove_server_with_removed(&mut self, server_name: &str) -> Vec<PendingOAuth> {
        let keys = self
            .by_key
            .iter()
            .filter_map(|(key, entry)| (entry.server_name == server_name).then(|| key.clone()))
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|key| self.by_key.remove(&key))
            .collect()
    }

    pub fn remove_all(&mut self) -> Vec<PendingOAuth> {
        self.by_key.drain().map(|(_, value)| value).collect()
    }

    pub fn has_pending_for(&self, origin: &str, now_ms: u64) -> bool {
        self.by_key
            .values()
            .any(|entry| entry.origin == origin && entry.expires_at_ms > now_ms)
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

pub mod mcp_oauth_callback_listener;
pub mod mcp_oauth_forwarder;
pub mod mcp_oauth_loopback_registry;
