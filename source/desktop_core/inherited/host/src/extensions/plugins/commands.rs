use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

// ---------------------------------------------------------------------------
// Reserved Commands
// ---------------------------------------------------------------------------

pub const RESERVED_COMMAND_NAMES: &[&str] = &[
    "help",
    "commands",
    "status",
    "diagnostics",
    "codex",
    "whoami",
    "context",
    "btw",
    "stop",
    "restart",
    "reset",
    "new",
    "compact",
    "config",
    "debug",
    "allowlist",
    "activation",
    "skill",
    "learn",
    "loop",
    "subagents",
    "kill",
    "steer",
    "tell",
    "model",
    "models",
    "queue",
    "send",
    "bash",
    "exec",
    "think",
    "verbose",
    "reasoning",
    "elevated",
    "usage",
];

pub fn is_reserved_command_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    RESERVED_COMMAND_NAMES.contains(&lower.as_str())
}

// ---------------------------------------------------------------------------
// Command Definition & Registration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCommandDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub accepts_args: bool,
    #[serde(default)]
    pub required_scopes: Vec<String>,
    #[serde(default)]
    pub ownership: Option<String>, // "reserved" or standard
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub agent_prompt_guidance: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredPluginCommand {
    pub command: PluginCommandDefinition,
    pub plugin_id: String,
    pub trusted_owner_status_exposure: bool,
}

impl RegisteredPluginCommand {
    pub fn is_trusted_reserved_owner(&self) -> bool {
        self.command.ownership.as_deref() == Some("reserved")
    }

    pub fn can_expose_sender_is_owner(&self) -> bool {
        !self.command.required_scopes.is_empty() || self.trusted_owner_status_exposure
    }
}

pub struct CommandValidator;
impl CommandValidator {
    pub fn validate_command(
        command: &PluginCommandDefinition,
        allow_reserved: bool,
    ) -> Result<(), String> {
        let trimmed_name = command.name.trim();
        if trimmed_name.is_empty() {
            return Err("Command name cannot be empty".to_string());
        }
        if !allow_reserved && is_reserved_command_name(trimmed_name) {
            return Err(format!(
                "Command name \"{}\" is a reserved built-in command",
                trimmed_name
            ));
        }
        if command.description.trim().is_empty() {
            return Err("Command description cannot be empty".to_string());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Command Execution Lock
// ---------------------------------------------------------------------------

pub struct CommandExecutionLock {
    active_count: AtomicUsize,
    retired: AtomicBool,
    drain_cond: Condvar,
    drain_mutex: Mutex<()>,
}

impl Default for CommandExecutionLock {
    fn default() -> Self {
        Self {
            active_count: AtomicUsize::new(0),
            retired: AtomicBool::new(false),
            drain_cond: Condvar::new(),
            drain_mutex: Mutex::new(()),
        }
    }
}

impl CommandExecutionLock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_active_count(&self) -> usize {
        self.active_count.load(Ordering::SeqCst)
    }

    pub fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
    }

    pub fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    pub fn with_execution<T, F: FnOnce() -> T>(&self, run: F) -> Result<T, String> {
        if self.is_retired() {
            return Err("Plugin registry is retired".to_string());
        }
        self.active_count.fetch_add(1, Ordering::SeqCst);
        let result = run();
        let prev = self.active_count.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            let _guard = self.drain_mutex.lock().unwrap();
            self.drain_cond.notify_all();
        }
        Ok(result)
    }

    pub fn wait_for_drain(&self) {
        let mut guard = self.drain_mutex.lock().unwrap();
        while self.active_count.load(Ordering::SeqCst) > 0 {
            guard = self.drain_cond.wait(guard).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Code Mode Executor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeModeOwnerCandidate {
    pub plugin_id: String,
    pub is_bundled: bool,
    pub code_mode_executors: Vec<String>,
    pub available_for_control_plane: bool,
    pub passes_base_policy: bool,
}

pub struct CodeModeExecutorResolver;
impl CodeModeExecutorResolver {
    pub fn resolve_executor(
        executor_id: &str,
        candidates: &[CodeModeOwnerCandidate],
    ) -> Result<String, String> {
        let matching: Vec<&CodeModeOwnerCandidate> = candidates
            .iter()
            .filter(|c| {
                c.code_mode_executors.iter().any(|e| e == executor_id)
                    && c.available_for_control_plane
                    && (c.is_bundled || c.passes_base_policy)
            })
            .collect();

        if matching.is_empty() {
            return Err(format!(
                "Code Mode executor \"{}\" is unavailable or disabled. Enable its plugin or select another executor.",
                executor_id
            ));
        }

        if matching.len() > 1 {
            let owners_str = matching
                .iter()
                .map(|c| c.plugin_id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "Code Mode executor \"{}\" has multiple plugin owners: {}. Enable only one owner.",
                executor_id, owners_str
            ));
        }

        Ok(matching[0].plugin_id.clone())
    }
}

// ---------------------------------------------------------------------------
// Compaction Provider Registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionProviderEntry {
    pub id: String,
    pub label: String,
    pub owner_plugin_id: String,
}

pub struct CompactionProviderRegistry {
    providers: Mutex<Vec<CompactionProviderEntry>>,
    active_owners: Mutex<HashSet<String>>,
}

impl Default for CompactionProviderRegistry {
    fn default() -> Self {
        Self {
            providers: Mutex::new(Vec::new()),
            active_owners: Mutex::new(HashSet::new()),
        }
    }
}

impl CompactionProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        id: &str,
        label: &str,
        owner_plugin_id: &str,
    ) -> Result<(), String> {
        let mut p = self.providers.lock().unwrap();
        if let Some(existing) = p.iter().find(|e| e.id == id) {
            return Err(format!(
                "compaction provider already registered: {} (owner: {})",
                id, existing.owner_plugin_id
            ));
        }
        let mut o = self.active_owners.lock().unwrap();
        o.insert(owner_plugin_id.to_string());
        p.push(CompactionProviderEntry {
            id: id.to_string(),
            label: label.to_string(),
            owner_plugin_id: owner_plugin_id.to_string(),
        });
        Ok(())
    }

    pub fn unregister_owner(&self, plugin_id: &str) {
        let mut o = self.active_owners.lock().unwrap();
        o.remove(plugin_id);
    }

    pub fn get_provider(&self, id: &str) -> Result<CompactionProviderEntry, String> {
        let p = self.providers.lock().unwrap();
        let entry = p
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or_else(|| format!("Compaction provider not found: {id}"))?;
        let o = self.active_owners.lock().unwrap();
        if !o.contains(&entry.owner_plugin_id) {
            return Err("Compaction provider owner instance was reloaded, disabled or retired".to_string());
        }
        Ok(entry)
    }
}

// ---------------------------------------------------------------------------
// Plugin Command Matching & Prompt Guidance
// ---------------------------------------------------------------------------

pub struct CommandRegistry;
impl CommandRegistry {
    pub fn match_command<'a>(
        commands: &'a [RegisteredPluginCommand],
        command_body: &str,
        channel: Option<&str>,
    ) -> Option<(&'a RegisteredPluginCommand, Option<String>)> {
        let trimmed = command_body.trim();
        let (cmd_name, rest) = match trimmed.find(|c: char| c.is_whitespace()) {
            Some(idx) => (&trimmed[..idx], Some(trimmed[idx + 1..].trim().to_string())),
            None => (trimmed, None),
        };
        let cmd_clean = cmd_name.strip_prefix('/').unwrap_or(cmd_name);

        for registered in commands {
            if registered.command.name.eq_ignore_ascii_case(cmd_clean) {
                if let Some(ch) = channel {
                    if !registered.command.channels.is_empty()
                        && !registered.command.channels.iter().any(|c| c.eq_ignore_ascii_case(ch))
                    {
                        continue;
                    }
                }
                return Some((registered, rest));
            }
        }
        None
    }

    pub fn list_agent_prompt_guidance(commands: &[RegisteredPluginCommand]) -> Vec<String> {
        let mut sorted = commands.to_vec();
        sorted.sort_by(|a, b| {
            a.plugin_id
                .cmp(&b.plugin_id)
                .then_with(|| a.command.name.cmp(&b.command.name))
        });

        let mut lines = Vec::new();
        let mut seen = HashSet::new();
        for cmd in sorted {
            for g in &cmd.command.agent_prompt_guidance {
                let trimmed = g.trim();
                if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                    lines.push(trimmed.to_string());
                }
            }
        }
        lines
    }
}
