use gpui_telegram_core::error::{Result, TelegramError};
use serde::{Deserialize, Serialize};

pub use mahayana_host_runtime::runner::sand_auto_review::SAND_AUTO_REVIEW_APPROVAL_TTL_MS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    ApproveOnce,
    AlwaysAllow,
    Deny { reason: Option<String> },
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalGate {
    pub gate_id: String,
    pub task_id: String,
    pub tool_name: String,
    pub params_summary: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub ttl_ms: u64,
    pub decision: Option<ApprovalDecision>,
}

impl ApprovalGate {
    pub fn new(
        gate_id: String,
        task_id: String,
        tool_name: String,
        params_summary: String,
    ) -> Self {
        let created_at_ms = chrono::Utc::now().timestamp_millis() as u64;
        let ttl_ms = SAND_AUTO_REVIEW_APPROVAL_TTL_MS;
        let expires_at_ms = created_at_ms.saturating_add(ttl_ms);
        Self {
            gate_id,
            task_id,
            tool_name,
            params_summary,
            created_at_ms,
            expires_at_ms,
            ttl_ms,
            decision: None,
        }
    }

    pub fn with_custom_ttl(mut self, ttl_ms: u64) -> Self {
        self.ttl_ms = ttl_ms;
        self.expires_at_ms = self.created_at_ms.saturating_add(ttl_ms);
        self
    }

    pub fn decide(&mut self, decision: ApprovalDecision) -> Result<()> {
        if self.decision.is_some() {
            return Err(TelegramError::Validation(
                "Approval gate has already reached terminal decision and cannot be re-decided".into(),
            ));
        }
        self.decision = Some(decision);
        Ok(())
    }

    /// Check if the 10-minute (or configured TTL) window has expired
    pub fn check_expiration(&mut self, now_ms: u64) -> bool {
        if self.decision.is_none() && now_ms >= self.expires_at_ms {
            self.decision = Some(ApprovalDecision::Timeout);
            true
        } else {
            false
        }
    }

    pub fn is_approved(&self) -> bool {
        matches!(
            self.decision,
            Some(ApprovalDecision::ApproveOnce) | Some(ApprovalDecision::AlwaysAllow)
        )
    }

    pub fn is_denied(&self) -> bool {
        matches!(self.decision, Some(ApprovalDecision::Deny { .. }))
    }

    pub fn is_expired(&self) -> bool {
        matches!(self.decision, Some(ApprovalDecision::Timeout))
    }

    pub fn remaining_seconds(&self, now_ms: u64) -> u64 {
        if now_ms >= self.expires_at_ms {
            0
        } else {
            (self.expires_at_ms - now_ms) / 1000
        }
    }
}
