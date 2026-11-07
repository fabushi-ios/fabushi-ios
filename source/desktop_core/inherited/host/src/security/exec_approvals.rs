//! Shell Execution Approvals & Allowlist Evaluation
//!
//! Mirrors:
//! - `docs/tools/exec-approvals.md`
//! - `docs/tools/exec-approvals-advanced.md`
//! - `extensions/discord/src/exec-approvals.ts`
//! - `extensions/codex/src/app-server/config-exec-approvals.ts`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShellAllowlistDecision {
    Allowed,
    Denied(String),
    RequiresManualApproval(String),
}

impl ShellAllowlistDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}

pub fn evaluate_shell_allowlist(command: &str, allowlist: &[String]) -> ShellAllowlistDecision {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return ShellAllowlistDecision::Denied("Command is empty".to_string());
    }

    // Check for dangerous shell chaining operators
    if contains_disallowed_shell_operators(trimmed) {
        return ShellAllowlistDecision::RequiresManualApproval(
            "Command contains shell chaining/pipe operators; manual approval required".to_string(),
        );
    }

    // Check for dangerous subshell or command substitution
    if trimmed.contains("$(") || trimmed.contains('`') {
        return ShellAllowlistDecision::RequiresManualApproval(
            "Command contains command substitution; manual approval required".to_string(),
        );
    }

    // Extract base command token
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return ShellAllowlistDecision::Denied("Empty command tokens".to_string());
    }

    let raw_cmd = tokens[0];
    let normalized_cmd = normalize_command_name(raw_cmd);

    // Exact or prefix matching against allowlist
    for allowed in allowlist {
        let allowed_trimmed = allowed.trim();
        if allowed_trimmed.is_empty() {
            continue;
        }

        // Wildcard support: e.g. "git *" or "cargo *"
        if let Some(prefix) = allowed_trimmed.strip_suffix('*') {
            let prefix_clean = prefix.trim();
            if trimmed.starts_with(prefix_clean) || normalized_cmd == prefix_clean {
                return ShellAllowlistDecision::Allowed;
            }
        }

        // Exact full command match
        if trimmed == allowed_trimmed {
            return ShellAllowlistDecision::Allowed;
        }

        // Base command match if allowlist specifies bare binary
        if normalized_cmd == allowed_trimmed {
            return ShellAllowlistDecision::Allowed;
        }

        // Prefix match on multi-token command
        if trimmed.starts_with(allowed_trimmed) {
            let rest = &trimmed[allowed_trimmed.len()..];
            if rest.is_empty() || rest.starts_with(' ') {
                return ShellAllowlistDecision::Allowed;
            }
        }
    }

    ShellAllowlistDecision::RequiresManualApproval(format!(
        "Command '{}' not in allowlist; requires approval",
        trimmed
    ))
}

fn contains_disallowed_shell_operators(cmd: &str) -> bool {
    // Check outside quoted strings
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let chars: Vec<char> = cmd.chars().collect();
    let len = chars.len();

    let mut i = 0;
    while i < len {
        let c = chars[i];
        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
        } else if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
        } else if !in_single_quote && !in_double_quote {
            // Check operators: ;, &&, ||, |, &
            if c == ';' {
                return true;
            }
            if c == '&' {
                return true;
            }
            if c == '|' {
                return true;
            }
            if c == '\n' || c == '\r' {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn normalize_command_name(raw: &str) -> &str {
    // Strip path prefixes: e.g. "/usr/bin/git" -> "git"
    if let Some(idx) = raw.rfind('/') {
        &raw[idx + 1..]
    } else if let Some(idx) = raw.rfind('\\') {
        &raw[idx + 1..]
    } else {
        raw
    }
}
