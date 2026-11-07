//! OpenClaw Node Host Runtime Manifest & Command Advertising
//!
//! Mirrors:
//! - `src/node-host/runtime-manifest.ts`
//! - `src/node-host/runtime.ts`
//! - `src/node-host/runner.ts`
//! - `src/node-host/worker-runtime.ts`
//! - `src/node-host/launcher-bootstrap.ts`

use std::collections::{BTreeSet, HashSet};
use serde::{Deserialize, Serialize};

pub const NODE_AGENT_CLI_CLAUDE_RUN_COMMAND: &str = "agent.cli.claude.run.v1";
pub const NODE_DEVICE_APPS_COMMAND: &str = "device.apps";
pub const NODE_EXEC_APPROVALS_COMMANDS: &[&str] =
    &["system.execApprovals.get", "system.execApprovals.set"];
pub const NODE_FS_LIST_DIR_COMMAND: &str = "fs.listDir";
pub const NODE_MCP_TOOLS_CALL_COMMAND: &str = "mcp.tools.call.v1";
pub const NODE_SYSTEM_RUN_COMMAND: &str = "system.run";
pub const NODE_SYSTEM_RUN_COMMANDS: &[&str] =
    &["system.run.prepare", NODE_SYSTEM_RUN_COMMAND, "system.which"];
pub const NODE_TERMINAL_UPLOAD_COMMAND: &str = "terminal.upload";
pub const NODE_DESKTOP_STREAM_COMMAND: &str = "worker.desktop.stream.v1";

pub const SYSTEM_RUN_EXECUTION_CONTEXT_CAPABILITY: &str = "system_run.execution_context";
pub const NODE_CLAUDE_SKILLS_CAPABILITY: &str = "node.claude_skills";
pub const NODE_CLAUDE_NATIVE_AUTH_CAPABILITY: &str = "claude-cli-native-auth-v1";
const MAX_MANIFEST_ENTRY_CHARS: usize = 256;

fn valid_manifest_entry(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_MANIFEST_ENTRY_CHARS
        || value.chars().any(|character| character.is_control())
    {
        None
    } else {
        Some(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeHostManifest {
    pub advertised_commands: Vec<String>,
    pub capabilities: Vec<String>,
    pub is_ephemeral: bool,
}

impl NodeHostManifest {
    /// Build the complete transport-neutral surface from current builtin and
    /// plugin availability. Plugin registries are expected to provide only
    /// currently registered commands/capabilities; the Gateway transport
    /// remains responsible for publishing the returned snapshot.
    pub fn build_with_plugin_manifest(
        available_commands: &HashSet<String>,
        plugin_commands: &[String],
        plugin_capabilities: &[String],
        claude_enabled: bool,
        desktop_streaming_enabled: bool,
        ephemeral: bool,
        allowlist: Option<&HashSet<String>>,
    ) -> Self {
        let mut commands = BTreeSet::new();
        let builtin_commands = NODE_SYSTEM_RUN_COMMANDS
            .iter()
            .copied()
            .chain([
            NODE_DEVICE_APPS_COMMAND,
            NODE_FS_LIST_DIR_COMMAND,
            NODE_MCP_TOOLS_CALL_COMMAND,
            NODE_TERMINAL_UPLOAD_COMMAND,
            ])
            .chain(NODE_EXEC_APPROVALS_COMMANDS.iter().copied());
        for command in builtin_commands {
            if available_commands.contains(command) {
                commands.insert(command.to_string());
            }
        }
        if claude_enabled && available_commands.contains(NODE_AGENT_CLI_CLAUDE_RUN_COMMAND) {
            commands.insert(NODE_AGENT_CLI_CLAUDE_RUN_COMMAND.to_string());
        }
        if desktop_streaming_enabled && available_commands.contains(NODE_DESKTOP_STREAM_COMMAND) {
            commands.insert(NODE_DESKTOP_STREAM_COMMAND.to_string());
        }
        commands.extend(plugin_commands.iter().filter_map(|command| {
            let command = valid_manifest_entry(command)?;
            if ephemeral && matches!(command.as_str(), "screen.snapshot" | "computer.act") {
                None
            } else {
                Some(command)
            }
        }));
        if let Some(allowed) = allowlist {
            commands.retain(|command| allowed.contains(command));
        }

        let mut capabilities = BTreeSet::new();
        if commands.contains(NODE_SYSTEM_RUN_COMMAND) {
            capabilities.insert(SYSTEM_RUN_EXECUTION_CONTEXT_CAPABILITY.to_string());
        }
        if commands.contains(NODE_AGENT_CLI_CLAUDE_RUN_COMMAND) {
            capabilities.insert(NODE_CLAUDE_SKILLS_CAPABILITY.to_string());
            capabilities.insert(NODE_CLAUDE_NATIVE_AUTH_CAPABILITY.to_string());
        }
        capabilities.extend(
            plugin_capabilities
                .iter()
                .filter_map(|capability| {
                    let capability = valid_manifest_entry(capability)?;
                    if ephemeral && matches!(capability.as_str(), "computer" | "screen") {
                        None
                    } else {
                        Some(capability)
                    }
                }),
        );

        Self {
            advertised_commands: commands.into_iter().collect(),
            capabilities: capabilities.into_iter().collect(),
            is_ephemeral: ephemeral,
        }
    }

    pub fn build(
        available_commands: &HashSet<String>,
        claude_enabled: bool,
        desktop_streaming_enabled: bool,
        allowlist: Option<&HashSet<String>>,
    ) -> Self {
        Self::build_with_plugin_manifest(
            available_commands,
            &[],
            &[],
            claude_enabled,
            desktop_streaming_enabled,
            false,
            allowlist,
        )
    }
}
