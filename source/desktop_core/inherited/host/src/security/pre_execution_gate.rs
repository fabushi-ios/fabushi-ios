//! Unified Pre-Execution Security Gate
//!
//! Evaluates shell allowlists, command obfuscation detection, outbound SSRF
//! protection, and skill source safety prior to tool invocation or command dispatch.

use serde::{Deserialize, Serialize};
use super::exec_approvals::{evaluate_shell_allowlist, ShellAllowlistDecision};
use super::obfuscation::{detect_command_obfuscation, ObfuscationDetectionResult};
use super::skill_scanner::{scan_skill_source, SkillScanReport};
use super::ssrf::SsrFPolicy;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreExecutionVerdict {
    Allowed,
    Denied(String),
    RequiresManualApproval(String),
}

impl PreExecutionVerdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}

pub struct PreExecutionSecurityGate;

impl PreExecutionSecurityGate {
    /// Evaluates a shell command against both obfuscation detection and shell allowlist.
    pub fn evaluate_shell_command(command: &str, allowlist: &[String]) -> PreExecutionVerdict {
        // Step 1: Detect obfuscation techniques
        let obf_result = detect_command_obfuscation(command);
        if let ObfuscationDetectionResult::Obfuscated { reason, detected_pattern } = obf_result {
            return PreExecutionVerdict::Denied(format!(
                "Command blocked by obfuscation filter: {reason} (pattern: {detected_pattern})"
            ));
        }

        // Step 2: Evaluate shell allowlist
        match evaluate_shell_allowlist(command, allowlist) {
            ShellAllowlistDecision::Allowed => PreExecutionVerdict::Allowed,
            ShellAllowlistDecision::Denied(msg) => PreExecutionVerdict::Denied(msg),
            ShellAllowlistDecision::RequiresManualApproval(msg) => {
                PreExecutionVerdict::RequiresManualApproval(msg)
            }
        }
    }

    /// Evaluates an outbound HTTP/HTTPS destination URL against SSRF policy.
    pub fn evaluate_outbound_url(url_str: &str, policy: &SsrFPolicy) -> PreExecutionVerdict {
        match policy.validate_url(url_str) {
            Ok(_) => PreExecutionVerdict::Allowed,
            Err(err_msg) => PreExecutionVerdict::Denied(err_msg),
        }
    }

    /// Evaluates skill or plugin source code before loading/registration.
    pub fn evaluate_skill_source(source_text: &str) -> PreExecutionVerdict {
        let report: SkillScanReport = scan_skill_source(source_text);
        if !report.is_safe {
            let critical_findings: Vec<String> = report
                .findings
                .into_iter()
                .filter(|f| f.severity == "critical" || f.severity == "high")
                .map(|f| format!("[{}] {}", f.rule_id, f.message))
                .collect();
            PreExecutionVerdict::Denied(format!(
                "Skill source failed security scan: {}",
                critical_findings.join("; ")
            ))
        } else {
            PreExecutionVerdict::Allowed
        }
    }
}
