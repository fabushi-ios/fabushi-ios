use crate::error::CliError;
use std::io::{self, BufRead, IsTerminal, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecisionPrompt {
    ApproveOnce,
    Deny,
    NonInteractiveFailClosed,
}

pub fn is_interactive_tty(non_interactive_flag: bool) -> bool {
    !non_interactive_flag && io::stdin().is_terminal() && io::stderr().is_terminal()
}

pub fn prompt_local_approval(
    action_name: &str,
    auto_approve: bool,
    non_interactive: bool,
) -> Result<bool, CliError> {
    if auto_approve {
        return Ok(true);
    }
    if !is_interactive_tty(non_interactive) {
        return Err(CliError::Forbidden(format!(
            "Action '{action_name}' requires explicit approval confirmation (pass --approve / -y in non-interactive mode)"
        )));
    }
    match prompt_terminal(action_name, None) {
        ApprovalDecisionPrompt::ApproveOnce => Ok(true),
        ApprovalDecisionPrompt::Deny => Err(CliError::Forbidden(format!(
            "Action '{action_name}' was denied by user confirmation prompt"
        ))),
        ApprovalDecisionPrompt::NonInteractiveFailClosed => Err(CliError::Forbidden(format!(
            "Action '{action_name}' requires explicit approval confirmation"
        ))),
    }
}

pub fn resolve_remote_gate_decision(
    action_name: &str,
    gate_id: &str,
    expires_at_ms: u64,
    message: &str,
    auto_approve: bool,
    non_interactive: bool,
) -> Result<ApprovalDecisionPrompt, CliError> {
    if auto_approve {
        return Ok(ApprovalDecisionPrompt::ApproveOnce);
    }
    if !is_interactive_tty(non_interactive) {
        return Err(CliError::ApprovalRequired {
            action: action_name.to_string(),
            gate_id: gate_id.to_string(),
            expires_at_ms,
            message: message.to_string(),
        });
    }
    Ok(prompt_terminal(action_name, Some(gate_id)))
}

fn prompt_terminal(action_name: &str, gate_id: Option<&str>) -> ApprovalDecisionPrompt {
    let mut stderr = io::stderr().lock();
    if let Some(gid) = gate_id {
        let _ = write!(
            stderr,
            "High-risk action '{action_name}' requires approval (gate_id={gid}). Approve once? [y/N]: "
        );
    } else {
        let _ = write!(
            stderr,
            "High-risk action '{action_name}' requires explicit approval. Approve? [y/N]: "
        );
    }
    let _ = stderr.flush();

    let mut line = String::new();
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    if handle.read_line(&mut line).is_err() {
        return ApprovalDecisionPrompt::NonInteractiveFailClosed;
    }

    let trimmed = line.trim().to_ascii_lowercase();
    if trimmed == "y" || trimmed == "yes" {
        ApprovalDecisionPrompt::ApproveOnce
    } else {
        ApprovalDecisionPrompt::Deny
    }
}
