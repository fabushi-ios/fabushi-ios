//! OpenClaw Plugin SDK Agent Control Plane (ACP) Runtime
//!
//! Mirrors:
//! - `src/plugin-sdk/acp-runtime.ts`
//! - `src/plugin-sdk/acpx.ts`
//! - `src/plugin-sdk/acp-runtime-backend.ts`
//! - `src/plugin-sdk/acp-binding-runtime.ts`
//! - `src/plugin-sdk/acp-binding-resolve-runtime.ts`
//! - `src/plugin-sdk/acp-runtime.test.ts`
//! - `src/plugin-sdk/acp-runtime.manager-api.test.ts`
//! - `packages/acp-core/src/index.ts`
//! - `src/acp/commands.ts`
//! - `src/acp/server.ts`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcpRuntimeStatus {
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpDoctorReport {
    pub backend_name: String,
    pub status: AcpRuntimeStatus,
    pub active_sessions_count: usize,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpError {
    SessionNotFound(String),
    IncognitoSessionEnded(String),
    IncognitoSessionSyncAccessDenied(String),
    BackendError(String),
    UnauthorizedCommand(String),
    LeaseExpired(String),
    LeaseNotFound(String),
    TaintedTurnViolation(String),
    PolicyViolation(String),
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotFound(id) => write!(f, "ACP session not found: '{id}'"),
            Self::IncognitoSessionEnded(id) => write!(f, "Incognito session ended: '{id}'"),
            Self::IncognitoSessionSyncAccessDenied(id) => {
                write!(f, "Sync access denied for incognito session: '{id}'")
            }
            Self::BackendError(msg) => write!(f, "ACP backend error: {msg}"),
            Self::UnauthorizedCommand(cmd) => write!(f, "Unauthorized command: '{cmd}'"),
            Self::LeaseExpired(id) => write!(f, "ACP execution lease expired: '{id}'"),
            Self::LeaseNotFound(id) => write!(f, "ACP execution lease not found: '{id}'"),
            Self::TaintedTurnViolation(msg) => write!(f, "Tainted turn violation: {msg}"),
            Self::PolicyViolation(msg) => write!(f, "Policy violation: {msg}"),
        }
    }
}

impl std::error::Error for AcpError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpSessionEntry {
    pub session_id: String,
    pub is_incognito: bool,
    pub is_active: bool,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpCommandDefinition {
    pub name: String,
    pub description: String,
    pub requires_review: bool,
    pub required_scopes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSecurityContext {
    pub is_tainted: bool,
    pub auto_review_approved: bool,
    pub unreviewed_sources: Vec<String>,
    pub execution_history: Vec<String>,
}

impl SessionSecurityContext {
    pub fn requires_approval_for_storage_mutation(&self) -> bool {
        self.is_tainted && !self.auto_review_approved
    }

    pub fn record_tool_execution(&mut self, tool_name: &str, reviewed: bool) {
        self.execution_history.push(tool_name.to_string());
        if !reviewed {
            self.is_tainted = true;
            self.auto_review_approved = false;
            if !self.unreviewed_sources.contains(&tool_name.to_string()) {
                self.unreviewed_sources.push(tool_name.to_string());
            }
        }
    }

    pub fn clear_taint(&mut self) {
        self.is_tainted = false;
        self.auto_review_approved = true;
        self.unreviewed_sources.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpDispatchPolicy {
    pub policy_id: String,
    pub allow_unreviewed_tools: bool,
    pub default_lease_ttl_ms: u64,
    pub permitted_commands: Vec<String>,
}

impl Default for AcpDispatchPolicy {
    fn default() -> Self {
        Self {
            policy_id: "default-policy".to_string(),
            allow_unreviewed_tools: true,
            default_lease_ttl_ms: 30_000,
            permitted_commands: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpExecutionLease {
    pub lease_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub acquired_at_ms: u64,
    pub expires_at_ms: u64,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpSettlementReceipt {
    pub receipt_id: String,
    pub session_id: String,
    pub run_id: String,
    pub terminal_outcome: String,
    pub total_tokens_used: u64,
    pub settled_at_ms: u64,
    pub audit_verified: bool,
}

#[derive(Debug, Default)]
pub struct AcpSessionManager {
    sessions: HashMap<String, AcpSessionEntry>,
    commands: HashMap<String, AcpCommandDefinition>,
    security_contexts: HashMap<String, SessionSecurityContext>,
    leases: HashMap<String, AcpExecutionLease>,
    policies: HashMap<String, AcpDispatchPolicy>,
    receipts: Vec<AcpSettlementReceipt>,
    lease_counter: u64,
    receipt_counter: u64,
}

impl AcpSessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            commands: HashMap::new(),
            security_contexts: HashMap::new(),
            leases: HashMap::new(),
            policies: HashMap::new(),
            receipts: Vec::new(),
            lease_counter: 0,
            receipt_counter: 0,
        }
    }

    pub fn register_session(&mut self, session: AcpSessionEntry) {
        let sid = session.session_id.clone();
        self.sessions.insert(sid.clone(), session);
        self.security_contexts.entry(sid).or_default();
    }

    pub fn end_session(&mut self, session_id: &str) -> Result<(), AcpError> {
        let entry = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AcpError::SessionNotFound(session_id.to_string()))?;
        entry.is_active = false;
        // Invalidate active leases for this session
        for lease in self.leases.values_mut() {
            if lease.session_id == session_id {
                lease.is_active = false;
            }
        }
        Ok(())
    }

    pub fn verify_session_access(
        &self,
        session_id: &str,
        require_sync: bool,
    ) -> Result<&AcpSessionEntry, AcpError> {
        let entry = self
            .sessions
            .get(session_id)
            .ok_or_else(|| AcpError::SessionNotFound(session_id.to_string()))?;

        if entry.is_incognito {
            if !entry.is_active {
                return Err(AcpError::IncognitoSessionEnded(session_id.to_string()));
            }
            if require_sync {
                return Err(AcpError::IncognitoSessionSyncAccessDenied(
                    session_id.to_string(),
                ));
            }
        }

        Ok(entry)
    }

    pub fn generate_doctor_report(&self, backend_name: &str) -> AcpDoctorReport {
        let active_count = self.sessions.values().filter(|s| s.is_active).count();
        AcpDoctorReport {
            backend_name: backend_name.to_string(),
            status: AcpRuntimeStatus::Ready,
            active_sessions_count: active_count,
            error_message: None,
        }
    }

    // --- Command Surface (OCRM-RESP-AGENT-ACP-COMMANDS-001) ---

    pub fn register_command(&mut self, cmd: AcpCommandDefinition) {
        self.commands.insert(cmd.name.clone(), cmd);
    }

    pub fn get_available_commands(
        &self,
        session_id: &str,
    ) -> Result<Vec<AcpCommandDefinition>, AcpError> {
        self.verify_session_access(session_id, false)?;

        let policy = self.policies.get(session_id).cloned().unwrap_or_default();
        let mut cmds = Vec::new();

        for (name, def) in &self.commands {
            if policy.permitted_commands.is_empty() || policy.permitted_commands.contains(name) {
                cmds.push(def.clone());
            }
        }

        cmds.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(cmds)
    }

    // --- Viral Turn Tainting & Auto-Review Gate ---

    pub fn record_tool_execution(
        &mut self,
        session_id: &str,
        tool_name: &str,
        reviewed: bool,
    ) -> Result<(), AcpError> {
        self.verify_session_access(session_id, false)?;
        let ctx = self
            .security_contexts
            .entry(session_id.to_string())
            .or_default();
        ctx.record_tool_execution(tool_name, reviewed);
        Ok(())
    }

    pub fn is_session_tainted(&self, session_id: &str) -> Result<bool, AcpError> {
        self.verify_session_access(session_id, false)?;
        Ok(self
            .security_contexts
            .get(session_id)
            .map(|ctx| ctx.is_tainted)
            .unwrap_or(false))
    }

    pub fn requires_approval_for_storage_mutation(
        &self,
        session_id: &str,
    ) -> Result<bool, AcpError> {
        self.verify_session_access(session_id, false)?;
        Ok(self
            .security_contexts
            .get(session_id)
            .map(|ctx| ctx.requires_approval_for_storage_mutation())
            .unwrap_or(false))
    }

    pub fn resolve_auto_review(
        &mut self,
        session_id: &str,
        approved: bool,
    ) -> Result<(), AcpError> {
        self.verify_session_access(session_id, false)?;
        let ctx = self
            .security_contexts
            .entry(session_id.to_string())
            .or_default();
        if approved {
            ctx.clear_taint();
        } else {
            ctx.auto_review_approved = false;
        }
        Ok(())
    }

    // --- Execution Lease Watchdog (OCRM-RESP-AGENT-ACP-POLICY-001) ---

    pub fn acquire_lease(
        &mut self,
        session_id: &str,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Result<AcpExecutionLease, AcpError> {
        let session = self.verify_session_access(session_id, false)?;
        let agent_id = session.agent_id.clone();

        self.lease_counter += 1;
        let lease_id = format!("lease-{}-{}", session_id, self.lease_counter);
        let lease = AcpExecutionLease {
            lease_id: lease_id.clone(),
            session_id: session_id.to_string(),
            agent_id,
            acquired_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
            is_active: true,
        };

        self.leases.insert(lease_id, lease.clone());
        Ok(lease)
    }

    pub fn validate_lease(&self, lease_id: &str, now_ms: u64) -> Result<(), AcpError> {
        let lease = self
            .leases
            .get(lease_id)
            .ok_or_else(|| AcpError::LeaseNotFound(lease_id.to_string()))?;

        if !lease.is_active || now_ms >= lease.expires_at_ms {
            return Err(AcpError::LeaseExpired(lease_id.to_string()));
        }

        self.verify_session_access(&lease.session_id, false)?;
        Ok(())
    }

    pub fn release_lease(&mut self, lease_id: &str) -> Result<(), AcpError> {
        let lease = self
            .leases
            .get_mut(lease_id)
            .ok_or_else(|| AcpError::LeaseNotFound(lease_id.to_string()))?;
        lease.is_active = false;
        Ok(())
    }

    pub fn sweep_zombie_leases(&mut self, now_ms: u64) -> usize {
        let mut expired = 0;
        for lease in self.leases.values_mut() {
            if lease.is_active && now_ms >= lease.expires_at_ms {
                lease.is_active = false;
                expired += 1;
            }
        }
        expired
    }

    // --- Dispatch Policy & Settlement Receipts ---

    pub fn set_dispatch_policy(&mut self, session_id: &str, policy: AcpDispatchPolicy) {
        self.policies.insert(session_id.to_string(), policy);
    }

    pub fn dispatch_command(
        &mut self,
        session_id: &str,
        command_name: &str,
        lease_id: Option<&str>,
        now_ms: u64,
    ) -> Result<(), AcpError> {
        self.verify_session_access(session_id, false)?;

        if let Some(lid) = lease_id {
            self.validate_lease(lid, now_ms)?;
        }

        let cmd = self
            .commands
            .get(command_name)
            .ok_or_else(|| AcpError::UnauthorizedCommand(command_name.to_string()))?;

        let policy = self.policies.get(session_id).cloned().unwrap_or_default();
        if !policy.permitted_commands.is_empty()
            && !policy.permitted_commands.contains(&command_name.to_string())
        {
            return Err(AcpError::PolicyViolation(format!(
                "Command '{command_name}' not allowed by session dispatch policy"
            )));
        }

        if cmd.requires_review && !policy.allow_unreviewed_tools {
            let ctx = self.security_contexts.get(session_id);
            if let Some(c) = ctx {
                if c.is_tainted && !c.auto_review_approved {
                    return Err(AcpError::TaintedTurnViolation(format!(
                        "Execution blocked: session is tainted by unreviewed tools: {:?}",
                        c.unreviewed_sources
                    )));
                }
            }
        }

        Ok(())
    }

    pub fn emit_settlement_receipt(&mut self, mut receipt: AcpSettlementReceipt) -> String {
        self.receipt_counter += 1;
        if receipt.receipt_id.is_empty() {
            receipt.receipt_id = format!("receipt-{}-{}", receipt.session_id, self.receipt_counter);
        }
        let rid = receipt.receipt_id.clone();
        self.receipts.push(receipt);
        rid
    }

    pub fn get_settlement_receipts(&self, session_id: &str) -> Vec<AcpSettlementReceipt> {
        self.receipts
            .iter()
            .filter(|r| r.session_id == session_id)
            .cloned()
            .collect()
    }
}
