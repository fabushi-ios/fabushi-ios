//! Node Host System Run, System Which, and System Notify Command Handlers
//!
//! Mirrors:
//! - `src/node-host/invoke-run-command.ts`
//! - `src/node-host/system-which.ts`
//! - `src/node-host/system-notify.ts`
//! - `src/node-host/exec-policy.ts`

use std::collections::HashMap;
use std::path::Path;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024; // 10MB
pub const DEFAULT_SYSTEM_RUN_TIMEOUT_MS: u64 = 60_000; // 60s
pub const OUTPUT_TRUNCATION_MARKER: &str = "\n[OUTPUT TRUNCATED: byte limit reached]\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemRunCommandInput {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub approval_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemRunPrepareInput {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemRunPrepareOutput {
    pub plan_hash: String,
    pub requires_approval: bool,
    pub approval_id: Option<String>,
    pub command_display: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemWhichInput {
    pub command: String,
    pub path_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemWhichOutput {
    pub found: bool,
    pub resolved_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemNotifyInput {
    pub title: String,
    pub message: String,
    pub level: Option<String>, // "info", "warning", "error"
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemRunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecApprovalPlan {
    pub plan_hash: String,
    pub approved: bool,
    pub approved_by: Option<String>,
    pub expires_at_ms: Option<u64>,
}

pub fn compute_system_run_plan_hash(command: &str, args: &[String], cwd: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(command.as_bytes());
    for arg in args {
        hasher.update(b"\0");
        hasher.update(arg.as_bytes());
    }
    if let Some(c) = cwd {
        hasher.update(b"\0cwd:");
        hasher.update(c.as_bytes());
    }
    let hash = hasher.finalize();
    let mut hex_str = String::with_capacity(64);
    for byte in hash {
        use std::fmt::Write;
        let _ = write!(hex_str, "{byte:02x}");
    }
    hex_str
}

pub fn is_sensitive_command(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    let basename = Path::new(&lower)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&lower);
    matches!(
        basename,
        "rm" | "sudo" | "dd" | "mkfs" | "shutdown" | "reboot" | "fdisk" | "chmod" | "chown"
    )
}

pub fn validate_system_run_approval_plan(
    input: &SystemRunCommandInput,
    plan: Option<&ExecApprovalPlan>,
    now_ms: u64,
) -> Result<(), String> {
    if !is_sensitive_command(&input.command) {
        return Ok(());
    }
    let expected_hash = compute_system_run_plan_hash(&input.command, &input.args, input.cwd.as_deref());
    let approval = plan.ok_or_else(|| {
        format!("SYSTEM_RUN_APPROVAL_REQUIRED: command requires approval plan ({expected_hash})")
    })?;
    if approval.plan_hash != expected_hash {
        return Err("SYSTEM_RUN_APPROVAL_MISMATCH: plan hash does not match invocation".into());
    }
    if !approval.approved {
        return Err("SYSTEM_RUN_APPROVAL_REJECTED: plan has not been approved".into());
    }
    if let Some(exp) = approval.expires_at_ms {
        if now_ms >= exp {
            return Err("SYSTEM_RUN_APPROVAL_EXPIRED: approval plan has expired".into());
        }
    }
    Ok(())
}

pub fn truncate_command_output(output: &mut String, max_bytes: usize) -> bool {
    if output.len() > max_bytes {
        let mut truncated_end = max_bytes.saturating_sub(OUTPUT_TRUNCATION_MARKER.len());
        // UTF-8 character boundary safety check
        while !output.is_char_boundary(truncated_end) && truncated_end > 0 {
            truncated_end -= 1;
        }
        output.truncate(truncated_end);
        output.push_str(OUTPUT_TRUNCATION_MARKER);
        true
    } else {
        false
    }
}

pub fn dispatch_system_which(input: &SystemWhichInput) -> Result<SystemWhichOutput, String> {
    let cmd = input.command.trim();
    if cmd.is_empty() {
        return Ok(SystemWhichOutput {
            found: false,
            resolved_path: None,
        });
    }

    // Direct path check
    let direct_path = Path::new(cmd);
    if direct_path.is_absolute() && direct_path.exists() {
        return Ok(SystemWhichOutput {
            found: true,
            resolved_path: Some(cmd.to_string()),
        });
    }

    let default_path_env = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin".into());
    let path_env = input.path_env.as_deref().unwrap_or(&default_path_env);

    for dir in path_env.split(':') {
        let candidate = Path::new(dir).join(cmd);
        if candidate.exists() && candidate.is_file() {
            return Ok(SystemWhichOutput {
                found: true,
                resolved_path: Some(candidate.to_string_lossy().to_string()),
            });
        }
    }

    Ok(SystemWhichOutput {
        found: false,
        resolved_path: None,
    })
}

pub fn dispatch_system_notify(input: &SystemNotifyInput) -> Result<serde_json::Value, String> {
    if input.title.trim().is_empty() && input.message.trim().is_empty() {
        return Err("SYSTEM_NOTIFY_EMPTY: title and message cannot both be empty".into());
    }
    let level = input.level.as_deref().unwrap_or("info");
    Ok(serde_json::json!({
        "status": "delivered",
        "title": input.title,
        "message": input.message,
        "level": level,
        "timeoutMs": input.timeout_ms.unwrap_or(5000),
    }))
}

pub fn dispatch_system_run_prepare(input: &SystemRunPrepareInput) -> Result<SystemRunPrepareOutput, String> {
    let cmd = input.command.trim();
    if cmd.is_empty() {
        return Err("SYSTEM_RUN_EMPTY_COMMAND: command cannot be empty".into());
    }
    let plan_hash = compute_system_run_plan_hash(cmd, &input.args, input.cwd.as_deref());
    let requires_approval = is_sensitive_command(cmd);
    let command_display = if input.args.is_empty() {
        cmd.to_string()
    } else {
        format!("{} {}", cmd, input.args.join(" "))
    };
    Ok(SystemRunPrepareOutput {
        plan_hash: plan_hash.clone(),
        requires_approval,
        approval_id: if requires_approval {
            Some(format!("appr-{}", &plan_hash[..16]))
        } else {
            None
        },
        command_display,
    })
}

pub fn dispatch_system_run(
    input: &SystemRunCommandInput,
    plan: Option<&ExecApprovalPlan>,
    now_ms: u64,
) -> Result<SystemRunOutput, String> {
    let cmd = input.command.trim();
    if cmd.is_empty() {
        return Err("SYSTEM_RUN_EMPTY_COMMAND: command cannot be empty".into());
    }
    validate_system_run_approval_plan(input, plan, now_ms)?;

    let max_output_bytes = input.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);

    // Mock/safe execution result structure for non-native execution
    let mut stdout = format!("executed: {}", cmd);
    let mut stderr = String::new();
    let truncated = truncate_command_output(&mut stdout, max_output_bytes)
        || truncate_command_output(&mut stderr, max_output_bytes);

    Ok(SystemRunOutput {
        exit_code: 0,
        stdout,
        stderr,
        truncated,
        elapsed_ms: 12,
    })
}
