use std::collections::HashMap;

pub const DEFAULT_MCP_AUTH_WAIT_TTL_MS: u64 = 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuthCompletionIdentity {
    pub server_id: String,
    pub server_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WaitEntry {
    agent_id: String,
    server_id: Option<String>,
    expires_at_ms: u64,
}

pub fn normalize_connector_name(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

#[derive(Debug, Clone)]
pub struct McpAuthWaitRegistry {
    waits: HashMap<String, WaitEntry>,
    ttl_ms: u64,
}

impl Default for McpAuthWaitRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_MCP_AUTH_WAIT_TTL_MS)
    }
}

impl McpAuthWaitRegistry {
    pub fn new(ttl_ms: u64) -> Self {
        Self {
            waits: HashMap::new(),
            ttl_ms,
        }
    }

    pub fn register(
        &mut self,
        agent_id: impl Into<String>,
        connector: &str,
        server_id: Option<&str>,
        now_ms: u64,
    ) {
        self.prune(now_ms);
        let server_id = server_id.filter(|value| !value.is_empty()).map(str::to_owned);
        let name_key = normalize_connector_name(connector);
        let key = if !name_key.is_empty() {
            Some(name_key)
        } else {
            server_id.as_ref().map(|value| format!("id:{value}"))
        };
        if let Some(key) = key {
            self.waits.insert(
                key,
                WaitEntry {
                    agent_id: agent_id.into(),
                    server_id,
                    expires_at_ms: now_ms.saturating_add(self.ttl_ms),
                },
            );
        }
    }

    pub fn take(
        &mut self,
        completion: &McpAuthCompletionIdentity,
        now_ms: u64,
    ) -> Option<String> {
        self.prune(now_ms);
        let name_key = normalize_connector_name(&completion.server_name);
        let mut by_server_id = None;
        let mut by_name = None;
        let keys: Vec<String> = self.waits.keys().cloned().collect();

        for key in keys {
            let Some(entry) = self.waits.get(&key) else {
                continue;
            };
            let id_match = entry
                .server_id
                .as_deref()
                .is_some_and(|value| value == completion.server_id);
            let name_match = entry.server_id.is_none() && !name_key.is_empty() && key == name_key;
            if !id_match && !name_match {
                continue;
            }
            let entry = self.waits.remove(&key).expect("wait key came from registry");
            if id_match {
                by_server_id = Some(entry.agent_id);
            } else {
                by_name = Some(entry.agent_id);
            }
        }

        by_server_id.or(by_name)
    }

    pub fn prune(&mut self, now_ms: u64) {
        self.waits
            .retain(|_, entry| entry.expires_at_ms > now_ms);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.waits.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_connector_names() {
        assert_eq!(normalize_connector_name("Google Drive"), "googledrive");
        assert_eq!(normalize_connector_name("MCP_GitHub-1"), "mcpgithub1");
    }

    #[test]
    fn server_id_match_has_priority() {
        let mut registry = McpAuthWaitRegistry::default();
        registry.register("agent-name", "GitHub", None, 10);
        registry.register("agent-id", "Other", Some("server-1"), 10);

        let agent = registry.take(
            &McpAuthCompletionIdentity {
                server_id: "server-1".into(),
                server_name: "GitHub".into(),
            },
            11,
        );

        assert_eq!(agent.as_deref(), Some("agent-id"));
    }

    #[test]
    fn expired_wait_is_not_resumed() {
        let mut registry = McpAuthWaitRegistry::new(100);
        registry.register("agent", "GitHub", None, 10);
        assert_eq!(registry.len(), 1);
        let agent = registry.take(
            &McpAuthCompletionIdentity {
                server_id: "server".into(),
                server_name: "GitHub".into(),
            },
            110,
        );
        assert_eq!(agent, None);
        assert_eq!(registry.len(), 0);
    }
}
