use std::collections::HashSet;

use crate::oauth::{OAuthPendingRegistry, PendingOAuth};
use crate::oauth::mcp_oauth_callback_listener::OAuthCallback;
use crate::oauth::mcp_oauth_loopback_registry::parse_loopback_redirect;
use crate::protocol::Failure;

pub const MCP_OAUTH_PENDING_TTL_MS: u64 = 11 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthPendingPayload {
    pub redirect_url: String,
    pub state: String,
    pub server_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthForwarderAction {
    StartListener {
        origin: String,
        redirect_url: String,
        state: String,
    },
    CloseListener {
        origin: String,
    },
    Complete {
        callback: OAuthCallback,
    },
}

#[derive(Debug, Default)]
pub struct McpOAuthForwarderState {
    pending: OAuthPendingRegistry,
    listening_origins: HashSet<String>,
}

impl McpOAuthForwarderState {
    pub fn handle_pending(
        &mut self,
        payload: McpOAuthPendingPayload,
        now_ms: u64,
    ) -> Result<Vec<OAuthForwarderAction>, Failure> {
        if payload.state.trim().is_empty() || payload.server_name.trim().is_empty() {
            return Err(Failure::new(
                "MCP_OAUTH_INVALID_PENDING",
                "pending OAuth requires state and server identity",
            ));
        }
        let redirect = parse_loopback_redirect(&payload.redirect_url)?;
        let mut affected_origins = self
            .pending
            .remove_server_with_removed(&payload.server_name)
            .into_iter()
            .map(|pending| pending.origin)
            .collect::<HashSet<_>>();
        if let Some(replaced) = self.pending.remove(&redirect.origin, &payload.state) {
            affected_origins.insert(replaced.origin);
        }

        self.pending.track(PendingOAuth {
            origin: redirect.origin.clone(),
            state: payload.state.clone(),
            server_name: payload.server_name,
            expires_at_ms: now_ms.saturating_add(MCP_OAUTH_PENDING_TTL_MS),
        });

        let mut actions = self.close_drained(now_ms, affected_origins);
        if !self.listening_origins.contains(&redirect.origin) {
            actions.push(OAuthForwarderAction::StartListener {
                origin: redirect.origin,
                redirect_url: payload.redirect_url,
                state: payload.state,
            });
        }
        Ok(actions)
    }

    pub fn listener_started(
        &mut self,
        origin: &str,
        now_ms: u64,
    ) -> Vec<OAuthForwarderAction> {
        if self.pending.has_pending_for(origin, now_ms) {
            self.listening_origins.insert(origin.to_string());
            Vec::new()
        } else {
            vec![OAuthForwarderAction::CloseListener {
                origin: origin.to_string(),
            }]
        }
    }

    pub fn listener_failed(
        &mut self,
        origin: &str,
        state: &str,
        now_ms: u64,
    ) -> Vec<OAuthForwarderAction> {
        self.pending.remove(origin, state);
        self.close_drained(now_ms, [origin.to_string()])
    }

    pub fn resolve_server(&self, origin: &str, state: &str, now_ms: u64) -> Option<&str> {
        self.pending.resolve(origin, state, now_ms)
    }

    pub fn begin_callback(
        &self,
        origin: &str,
        code: impl Into<String>,
        state: impl Into<String>,
        now_ms: u64,
    ) -> Result<OAuthForwarderAction, Failure> {
        let state = state.into();
        let code = code.into();
        let server_name = self
            .pending
            .resolve(origin, &state, now_ms)
            .ok_or_else(|| Failure::new("MCP_OAUTH_UNKNOWN_STATE", "OAuth state is not pending"))?
            .to_string();
        Ok(OAuthForwarderAction::Complete {
            callback: OAuthCallback {
                code,
                state,
                server_name,
            }
            .validate()?,
        })
    }

    pub fn settle_callback(
        &mut self,
        origin: &str,
        state: &str,
        now_ms: u64,
    ) -> Vec<OAuthForwarderAction> {
        self.pending.remove(origin, state);
        self.close_drained(now_ms, [origin.to_string()])
    }

    pub fn expire(&mut self, now_ms: u64) -> Vec<OAuthForwarderAction> {
        let affected = self
            .pending
            .expire_with_removed(now_ms)
            .into_iter()
            .map(|pending| pending.origin)
            .collect::<HashSet<_>>();
        self.close_drained(now_ms, affected)
    }

    pub fn remove_server(
        &mut self,
        server_name: &str,
        now_ms: u64,
    ) -> Vec<OAuthForwarderAction> {
        let affected = self
            .pending
            .remove_server_with_removed(server_name)
            .into_iter()
            .map(|pending| pending.origin)
            .collect::<HashSet<_>>();
        self.close_drained(now_ms, affected)
    }

    pub fn dispose(&mut self) -> Vec<OAuthForwarderAction> {
        self.pending.remove_all();
        let mut origins = self.listening_origins.drain().collect::<Vec<_>>();
        origins.sort();
        origins
            .into_iter()
            .map(|origin| OAuthForwarderAction::CloseListener { origin })
            .collect()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn listener_count(&self) -> usize {
        self.listening_origins.len()
    }

    fn close_drained(
        &mut self,
        now_ms: u64,
        origins: impl IntoIterator<Item = String>,
    ) -> Vec<OAuthForwarderAction> {
        let mut actions = Vec::new();
        for origin in origins {
            if self.pending.has_pending_for(&origin, now_ms) {
                continue;
            }
            if self.listening_origins.remove(&origin) {
                actions.push(OAuthForwarderAction::CloseListener { origin });
            }
        }
        actions
    }
}
