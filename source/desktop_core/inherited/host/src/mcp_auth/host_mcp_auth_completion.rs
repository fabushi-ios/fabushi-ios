use std::sync::{Arc, Mutex};

use super::mcp_auth_wait_registry::{
    McpAuthCompletionIdentity, McpAuthWaitEvent, McpAuthWaitRegistry,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostMcpAuthCompletionEvent {
    pub server_id: String,
    pub server_name: String,
    pub account_key: String,
    pub outcome: String,
    pub requesting_agent_id: Option<String>,
}

pub trait HostMcpAuthMcp: Send + Sync {
    fn note_auth_completed_elsewhere(
        &self,
        server_id: &str,
        account_key: &str,
    ) -> Option<String>;
    fn restart(&self) -> Result<(), String>;
}

pub trait HostMcpAuthTranscript: Send + Sync {
    fn resume_after_mcp_auth(
        &self,
        agent_id: &str,
        server_name: &str,
        account_key: &str,
    ) -> Result<(), String>;
}

pub struct HostMcpAuthCompletion {
    mcp: Arc<dyn HostMcpAuthMcp>,
    transcript: Arc<dyn HostMcpAuthTranscript>,
    pub waits: Mutex<McpAuthWaitRegistry>,
}

impl HostMcpAuthCompletion {
    pub fn new(
        mcp: Arc<dyn HostMcpAuthMcp>,
        transcript: Arc<dyn HostMcpAuthTranscript>,
        wait_registry: Option<McpAuthWaitRegistry>,
    ) -> Self {
        Self {
            mcp,
            transcript,
            waits: Mutex::new(wait_registry.unwrap_or_default()),
        }
    }

    pub fn register_connect_card(&self, event: McpAuthWaitEvent) {
        self.waits
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .register(event);
    }

    pub fn resolve(&self, completion: &HostMcpAuthCompletionEvent) {
        let waiting_agent_id = self
            .waits
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take(&McpAuthCompletionIdentity {
                server_id: completion.server_id.clone(),
                server_name: completion.server_name.clone(),
            });
        let watch_agent_id = self.mcp.note_auth_completed_elsewhere(
            &completion.server_id,
            &completion.account_key,
        );
        if completion.outcome == "cancelled" {
            return;
        }
        let agent_id = completion
            .requesting_agent_id
            .clone()
            .or(watch_agent_id)
            .or(waiting_agent_id);
        if let Some(agent_id) = agent_id {
            let _ = self.transcript.resume_after_mcp_auth(
                &agent_id,
                &completion.server_name,
                &completion.account_key,
            );
        }
    }

    pub fn resolve_desktop(&self, completion: &HostMcpAuthCompletionEvent) {
        let _ = self.mcp.restart();
        self.resolve(completion);
    }
}
