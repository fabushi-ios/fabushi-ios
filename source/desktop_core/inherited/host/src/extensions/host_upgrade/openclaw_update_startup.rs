//! OpenClaw Update Startup, Update Triage, and Supervisor Policy
//!
//! Mirrors:
//! - `src/infra/update-startup.ts`
//! - `src/infra/update-startup.test.ts`
//! - `src/infra/update-triage.ts`
//! - `src/infra/update-triage.test.ts`
//! - `src/infra/update-status-schedule.ts`
//! - `src/infra/update-step-identity.ts`
//! - `src/infra/update-step-result.ts`
//! - `src/infra/update-timeout-provenance.ts`

use serde::{Deserialize, Serialize};

pub const EXTERNAL_SUPERVISOR_UPDATE_REQUIRED_REASON: &str =
    "Gateway update requires an external service manager or supervisor restart.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateScheduleState {
    Idle,
    Scheduled,
    Deferred,
    Applying,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateAvailable {
    pub target_version: String,
    pub current_version: String,
    pub release_notes_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTriageReport {
    pub exit_code: Option<i32>,
    pub failure_reason: String,
    pub suggested_remedy: String,
    pub requires_supervisor: bool,
}

impl UpdateTriageReport {
    pub fn from_error(message: &str, exit_code: Option<i32>, is_supervised: bool) -> Self {
        let requires_supervisor = is_supervised || message.contains("supervisor");
        let remedy = if requires_supervisor {
            "Restart via system supervisor service (e.g. systemctl restart openclaw)".to_string()
        } else {
            "Run 'openclaw update' manually in terminal".to_string()
        };

        Self {
            exit_code,
            failure_reason: message.trim().to_string(),
            suggested_remedy: remedy,
            requires_supervisor,
        }
    }
}

pub fn is_update_check_due(last_check_ms: u64, now_ms: u64, check_interval_ms: u64) -> bool {
    now_ms.saturating_sub(last_check_ms) >= check_interval_ms
}
