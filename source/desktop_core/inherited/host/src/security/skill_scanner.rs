//! Skill Source Security Scanner
//!
//! Scans skill and tool definitions for leaked secrets, destructive shell
//! commands, unescaped system command executions, and suspicious download pipes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillScanFinding {
    pub severity: String, // "critical", "high", "medium", "low"
    pub rule_id: String,
    pub message: String,
    pub line_number: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillScanReport {
    pub is_safe: bool,
    pub findings: Vec<SkillScanFinding>,
}

pub fn scan_skill_source(source_text: &str) -> SkillScanReport {
    let mut findings = Vec::new();

    let lines: Vec<&str> = source_text.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let line_lower = line.to_ascii_lowercase();

        // 1. Destructive system commands
        if line_lower.contains("rm -rf /")
            || line_lower.contains("rm -rf ~")
            || line_lower.contains("mkfs.")
            || line_lower.contains("dd if=/dev/zero")
        {
            findings.push(SkillScanFinding {
                severity: "critical".to_string(),
                rule_id: "destructive-system-command".to_string(),
                message: format!("Line {line_num} contains potentially destructive system command"),
                line_number: Some(line_num),
            });
        }

        // 2. Sensitive system file access
        if line_lower.contains("/etc/shadow")
            || line_lower.contains("/etc/sudoers")
            || line_lower.contains("~/.ssh/id_rsa")
            || line_lower.contains("~/.ssh/id_ed25519")
        {
            findings.push(SkillScanFinding {
                severity: "high".to_string(),
                rule_id: "sensitive-system-file-access".to_string(),
                message: format!("Line {line_num} references protected credential or system files"),
                line_number: Some(line_num),
            });
        }

        // 3. Unsafe remote download and execution
        if (line_lower.contains("curl ") || line_lower.contains("wget "))
            && (line_lower.contains("| sh") || line_lower.contains("| bash") || line_lower.contains("| zsh") || line_lower.contains("| python"))
        {
            findings.push(SkillScanFinding {
                severity: "critical".to_string(),
                rule_id: "remote-code-download-exec".to_string(),
                message: format!("Line {line_num} downloads and executes remote script directly"),
                line_number: Some(line_num),
            });
        }

        // 4. Hardcoded private keys
        if line.contains("-----BEGIN PRIVATE KEY-----")
            || line.contains("-----BEGIN RSA PRIVATE KEY-----")
            || line.contains("-----BEGIN EC PRIVATE KEY-----")
        {
            findings.push(SkillScanFinding {
                severity: "critical".to_string(),
                rule_id: "hardcoded-private-key".to_string(),
                message: format!("Line {line_num} contains embedded cryptographic private key"),
                line_number: Some(line_num),
            });
        }

        // 5. Hardcoded API tokens
        if line.contains("AKIA") && line.len() >= 20 {
            // Potential AWS Access Key ID
            findings.push(SkillScanFinding {
                severity: "high".to_string(),
                rule_id: "hardcoded-aws-access-key".to_string(),
                message: format!("Line {line_num} appears to contain hardcoded AWS Access Key ID"),
                line_number: Some(line_num),
            });
        }
        if line.contains("ghp_") || line.contains("gho_") {
            // Potential GitHub token
            findings.push(SkillScanFinding {
                severity: "high".to_string(),
                rule_id: "hardcoded-github-token".to_string(),
                message: format!("Line {line_num} appears to contain hardcoded GitHub personal access token"),
                line_number: Some(line_num),
            });
        }
    }

    let is_safe = !findings.iter().any(|f| f.severity == "critical" || f.severity == "high");

    SkillScanReport {
        is_safe,
        findings,
    }
}
