use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCategory {
    Session,
    ModelAndProvider,
    ToolsAndSkills,
    MemoryAndPersona,
    WorkflowAndAutomation,
    SafetyAndControl,
    InspectionAndExport,
    ModesAndUi,
}

impl CommandCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::ModelAndProvider => "Model & Provider",
            Self::ToolsAndSkills => "Tools & Skills",
            Self::MemoryAndPersona => "Memory & Persona",
            Self::WorkflowAndAutomation => "Workflow & Automation",
            Self::SafetyAndControl => "Safety & Control",
            Self::InspectionAndExport => "Inspection & Export",
            Self::ModesAndUi => "Modes & UI",
        }
    }
}

impl fmt::Display for CommandCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Autocomplete & help metadata for a Hermes slash command (used by desktop `/` popup & API).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDescriptor {
    pub name: String,
    pub slash_name: String,
    pub aliases: Vec<String>,
    pub syntax: String,
    pub category: CommandCategory,
    pub description: String,
    pub accepts_args: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedSlashCommand {
    pub raw: String,
    pub command: String,
    pub bot_mention: Option<String>,
    pub args: String,
    pub argv: Vec<String>,
}

pub struct SlashCommandParser;

impl SlashCommandParser {
    /// Parses `/cmd [args...]` or `/cmd@bot_username [args...]`.
    /// Returns `None` for normal text or filesystem paths like `/Users/alice/file.rs`.
    pub fn parse(input: &str) -> Option<ParsedSlashCommand> {
        let trimmed = input.trim();
        let after_slash = trimmed.strip_prefix('/')?;
        if after_slash.is_empty() {
            return None;
        }
        // First whitespace-delimited token
        let (head, rest) = match after_slash.find(char::is_whitespace) {
            Some(idx) => (&after_slash[..idx], after_slash[idx..].trim()),
            None => (after_slash, ""),
        };
        // Reject filesystem paths (`/usr/bin/foo`)
        if head.contains('/') || head.contains('\\') {
            return None;
        }
        let (cmd_raw, bot_mention) = match head.split_once('@') {
            Some((cmd, bot)) if !cmd.is_empty() && !bot.is_empty() => {
                (cmd, Some(bot.to_string()))
            }
            _ => (head, None),
        };
        if !cmd_raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }
        let command = cmd_raw.to_ascii_lowercase();
        let argv = split_shell_args(rest);
        Some(ParsedSlashCommand {
            raw: trimmed.to_string(),
            command,
            bot_mention,
            args: rest.to_string(),
            argv,
        })
    }

    pub fn is_slash_command(input: &str) -> bool {
        Self::parse(input).is_some()
    }

    /// Extracts an active `/` command prefix from composer text for autocomplete popups.
    pub fn extract_autocomplete_prefix(composer_text: &str) -> Option<String> {
        let trimmed = composer_text.trim_start();
        if !trimmed.starts_with('/') {
            return None;
        }
        if trimmed.contains(char::is_whitespace) {
            return None;
        }
        if trimmed[1..].contains('/') {
            return None;
        }
        Some(trimmed.to_ascii_lowercase())
    }
}

fn split_shell_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    if let Some(next_ch) = chars.next() {
                        current.push(next_ch);
                    }
                } else {
                    current.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                } else if c.is_whitespace() {
                    if !current.is_empty() {
                        args.push(std::mem::take(&mut current));
                    }
                } else {
                    current.push(c);
                }
            }
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineActionButton {
    pub text: String,
    pub callback_data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandUiEffect {
    None,
    OpenTalkMode,
    OpenRemoteComputer,
    OpenHermesStudio { tab: String },
    ClearChatDisplay,
    RefreshSessionState,
    LaunchMoa { prompt: String, models: Vec<String> },
    StartGoalLoop { goal: String, max_iterations: usize },
    RunCronNow { job_id: String },
    StopActiveTurn { session_id: String },
    SteerActiveTurn { session_id: String, instruction: String },
    QueueMessage { session_id: String, message: String },
    LearnFromSession { skill_name: Option<String> },
    ShowDiff,
    ExportFile { filename: String, format: String, content: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandExecutionOutcome {
    pub handled: bool,
    pub command: String,
    pub response_text: String,
    pub is_error: bool,
    #[serde(default)]
    pub buttons: Vec<Vec<InlineActionButton>>,
    pub ui_effect: CommandUiEffect,
    #[serde(default)]
    pub mutated_fields: Vec<String>,
    pub should_continue_to_llm: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewritten_prompt: Option<String>,
}

impl CommandExecutionOutcome {
    pub fn ok(command: impl Into<String>, response_text: impl Into<String>) -> Self {
        Self {
            handled: true,
            command: command.into(),
            response_text: response_text.into(),
            is_error: false,
            buttons: Vec::new(),
            ui_effect: CommandUiEffect::None,
            mutated_fields: Vec::new(),
            should_continue_to_llm: false,
            rewritten_prompt: None,
        }
    }

    pub fn err(command: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            handled: true,
            command: command.into(),
            response_text: message.into(),
            is_error: true,
            buttons: Vec::new(),
            ui_effect: CommandUiEffect::None,
            mutated_fields: Vec::new(),
            should_continue_to_llm: false,
            rewritten_prompt: None,
        }
    }

    pub fn with_effect(mut self, effect: CommandUiEffect) -> Self {
        self.ui_effect = effect;
        self
    }

    pub fn with_buttons(mut self, buttons: Vec<Vec<InlineActionButton>>) -> Self {
        self.buttons = buttons;
        self
    }

    pub fn with_mutated(mut self, fields: Vec<String>) -> Self {
        self.mutated_fields = fields;
        self
    }
}

// ============================================================================
// Command Host Abstraction (Implemented by HermesEngine + Standalone Test Host)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandModelInfo {
    pub provider_id: String,
    pub model: String,
    pub api_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandProviderInfo {
    pub id: String,
    pub name: String,
    pub api_mode: String,
    pub default_model: String,
    pub active: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandToolsetInfo {
    pub id: String,
    pub enabled: bool,
    pub tool_count: usize,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSkillInfo {
    pub name: String,
    pub description: String,
    pub origin: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandBranchInfo {
    pub session_id: String,
    pub title: String,
    pub parent_session_id: Option<String>,
    pub message_count: usize,
    pub active: bool,
}

pub trait CommandHost {
    fn active_session_id(&self) -> String;
    fn new_session(&mut self, title: Option<&str>) -> Result<String, String>;
    fn clear_session_messages(&mut self) -> Result<usize, String>;
    fn branch_session(&mut self, name: Option<&str>) -> Result<String, String>;
    fn list_branches(&self) -> Result<Vec<CommandBranchInfo>, String>;
    fn checkout_branch(&mut self, target: &str) -> Result<String, String>;
    fn undo_last_turn(&mut self) -> Result<String, String>;
    fn rollback_last_checkpoint(&mut self) -> Result<String, String>;
    fn diff_workspace(&self) -> Result<String, String>;
    fn compact_context(&mut self, focus: Option<&str>) -> Result<String, String>;

    fn current_model_info(&self) -> CommandModelInfo;
    fn set_model(&mut self, model: &str) -> Result<String, String>;
    fn list_providers(&self) -> Vec<CommandProviderInfo>;
    fn set_provider(&mut self, provider_id: &str) -> Result<String, String>;
    fn set_api_mode(&mut self, mode: &str) -> Result<String, String>;

    fn list_toolsets(&self) -> Vec<CommandToolsetInfo>;
    fn mutate_toolsets(&mut self, action: &str, arg: Option<&str>) -> Result<String, String>;
    fn list_skills(&self) -> Vec<CommandSkillInfo>;
    fn view_skill(&self, name: &str) -> Result<String, String>;
    fn learn_skill(&mut self, name: Option<&str>) -> Result<String, String>;
    fn mcp_action(&mut self, action: &str, arg: Option<&str>) -> Result<String, String>;

    fn memory_action(&mut self, target: &str, action: &str, args: &[String]) -> Result<String, String>;
    fn soul_action(&mut self, action: &str, text: Option<&str>) -> Result<String, String>;
    fn reload_memory(&mut self) -> Result<String, String>;
    fn honcho_action(&mut self, action: &str, arg: Option<&str>) -> Result<String, String>;

    fn search_sessions(&self, query: &str) -> Result<String, String>;
    fn session_history(&self, limit: usize) -> Result<String, String>;
    fn list_sessions(&self, limit: usize) -> Result<String, String>;
    fn export_session(
        &self,
        format: &str,
        save_path: Option<&str>,
    ) -> Result<(String, String, String), String>;
    fn usage_summary(&self, kind: &str) -> Result<String, String>;
    fn status_report(&self) -> Result<String, String>;
    fn doctor_report(&self) -> Result<String, String>;
    fn context_report(&self) -> Result<String, String>;

    fn cron_action(&mut self, action: &str, args: &[String]) -> Result<String, String>;
    fn kanban_action(&mut self, action: &str, args: &[String]) -> Result<String, String>;
    fn heartbeat_action(&mut self, action: &str, interval_secs: Option<u64>) -> Result<String, String>;

    fn approval_action(
        &mut self,
        action: &str,
        target: Option<&str>,
        reason: Option<&str>,
    ) -> Result<String, String>;
    fn queue_action(&mut self, action: &str, text: Option<&str>) -> Result<String, String>;
    fn set_toggle(&mut self, key: &str, value: Option<&str>) -> Result<String, String>;
    fn config_action(&mut self, action: &str, key: Option<&str>, val: Option<&str>) -> Result<String, String>;
}

// ============================================================================
// Standalone In-Memory Command Host (for standalone tests & lightweight usage)
// ============================================================================

#[derive(Debug, Clone)]
pub struct StandaloneCommandHost {
    pub session_id: String,
    pub model: String,
    pub provider_id: String,
    pub api_mode: String,
    pub messages: Vec<(String, String)>,
    pub branches: Vec<CommandBranchInfo>,
    pub toolsets: HashMap<String, bool>,
    pub memory_entries: Vec<String>,
    pub user_entries: Vec<String>,
    pub soul_text: String,
    pub toggles: HashMap<String, bool>,
    pub yolo: bool,
    pub queued: Vec<String>,
    pub cron_jobs: Vec<String>,
    pub kanban_cards: Vec<String>,
    pub heartbeat_enabled: bool,
    pub heartbeat_interval_secs: u64,
}

impl Default for StandaloneCommandHost {
    fn default() -> Self {
        let mut toolsets = HashMap::new();
        for ts in ["core", "file", "terminal", "web", "memory", "skills", "kanban", "cronjob"] {
            toolsets.insert(ts.to_string(), true);
        }
        let mut toggles = HashMap::new();
        toggles.insert("verbose".to_string(), false);
        toggles.insert("reasoning".to_string(), true);
        toggles.insert("debug".to_string(), false);
        toggles.insert("voice".to_string(), false);
        toggles.insert("browser".to_string(), false);

        Self {
            session_id: "sess_default".to_string(),
            model: "hermes-local-v1".to_string(),
            provider_id: "local_hermes".to_string(),
            api_mode: "local_hermes".to_string(),
            messages: Vec::new(),
            branches: vec![CommandBranchInfo {
                session_id: "sess_default".to_string(),
                title: "main".to_string(),
                parent_session_id: None,
                message_count: 0,
                active: true,
            }],
            toolsets,
            memory_entries: Vec::new(),
            user_entries: Vec::new(),
            soul_text: "Hermes autonomous local assistant.".to_string(),
            toggles,
            yolo: false,
            queued: Vec::new(),
            cron_jobs: Vec::new(),
            kanban_cards: Vec::new(),
            heartbeat_enabled: false,
            heartbeat_interval_secs: 300,
        }
    }
}

impl CommandHost for StandaloneCommandHost {
    fn active_session_id(&self) -> String {
        self.session_id.clone()
    }

    fn new_session(&mut self, title: Option<&str>) -> Result<String, String> {
        let id = format!("sess_{}", uuid::Uuid::new_v4().simple());
        self.session_id = id.clone();
        self.messages.clear();
        self.branches.push(CommandBranchInfo {
            session_id: id.clone(),
            title: title.unwrap_or("New Session").to_string(),
            parent_session_id: None,
            message_count: 0,
            active: true,
        });
        Ok(id)
    }

    fn clear_session_messages(&mut self) -> Result<usize, String> {
        let n = self.messages.len();
        self.messages.clear();
        Ok(n)
    }

    fn branch_session(&mut self, name: Option<&str>) -> Result<String, String> {
        let parent = self.session_id.clone();
        let id = format!("branch_{}", uuid::Uuid::new_v4().simple());
        for b in &mut self.branches {
            b.active = false;
        }
        self.branches.push(CommandBranchInfo {
            session_id: id.clone(),
            title: name.unwrap_or("branch").to_string(),
            parent_session_id: Some(parent),
            message_count: self.messages.len(),
            active: true,
        });
        self.session_id = id.clone();
        Ok(id)
    }

    fn list_branches(&self) -> Result<Vec<CommandBranchInfo>, String> {
        Ok(self.branches.clone())
    }

    fn checkout_branch(&mut self, target: &str) -> Result<String, String> {
        let found = self
            .branches
            .iter()
            .find(|b| b.session_id == target || b.title == target)
            .map(|b| b.session_id.clone())
            .ok_or_else(|| format!("Branch '{target}' not found"))?;
        for b in &mut self.branches {
            b.active = b.session_id == found;
        }
        self.session_id = found.clone();
        Ok(found)
    }

    fn undo_last_turn(&mut self) -> Result<String, String> {
        if self.messages.is_empty() {
            return Ok("No turns to undo.".into());
        }
        self.messages.pop();
        Ok("Undid last turn.".into())
    }

    fn rollback_last_checkpoint(&mut self) -> Result<String, String> {
        Ok("No file checkpoints pending in standalone mode.".into())
    }

    fn diff_workspace(&self) -> Result<String, String> {
        Ok("No workspace modifications tracked.".into())
    }

    fn compact_context(&mut self, _focus: Option<&str>) -> Result<String, String> {
        Ok(format!("Compacted {} messages.", self.messages.len()))
    }

    fn current_model_info(&self) -> CommandModelInfo {
        CommandModelInfo {
            provider_id: self.provider_id.clone(),
            model: self.model.clone(),
            api_mode: self.api_mode.clone(),
        }
    }

    fn set_model(&mut self, model: &str) -> Result<String, String> {
        self.model = model.to_string();
        Ok(self.model.clone())
    }

    fn list_providers(&self) -> Vec<CommandProviderInfo> {
        vec![CommandProviderInfo {
            id: self.provider_id.clone(),
            name: "Local Hermes".into(),
            api_mode: self.api_mode.clone(),
            default_model: self.model.clone(),
            active: true,
            enabled: true,
        }]
    }

    fn set_provider(&mut self, provider_id: &str) -> Result<String, String> {
        self.provider_id = provider_id.to_string();
        Ok(self.provider_id.clone())
    }

    fn set_api_mode(&mut self, mode: &str) -> Result<String, String> {
        self.api_mode = mode.to_string();
        Ok(self.api_mode.clone())
    }

    fn list_toolsets(&self) -> Vec<CommandToolsetInfo> {
        let mut out: Vec<_> = self
            .toolsets
            .iter()
            .map(|(k, &v)| CommandToolsetInfo {
                id: k.clone(),
                enabled: v,
                tool_count: 1,
                description: format!("{k} toolset"),
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    fn mutate_toolsets(&mut self, action: &str, arg: Option<&str>) -> Result<String, String> {
        match action {
            "enable" => {
                let name = arg.ok_or("Missing toolset name")?;
                self.toolsets.insert(name.to_string(), true);
                Ok(format!("Enabled toolset '{name}'"))
            }
            "disable" => {
                let name = arg.ok_or("Missing toolset name")?;
                self.toolsets.insert(name.to_string(), false);
                Ok(format!("Disabled toolset '{name}'"))
            }
            _ => Ok(format!("Active toolsets: {}", self.toolsets.len())),
        }
    }

    fn list_skills(&self) -> Vec<CommandSkillInfo> {
        vec![CommandSkillInfo {
            name: "hermes-research".into(),
            description: "Structured research & synthesis".into(),
            origin: "bundled".into(),
            enabled: true,
        }]
    }

    fn view_skill(&self, name: &str) -> Result<String, String> {
        Ok(format!("Skill '{name}' (standalone view)"))
    }

    fn learn_skill(&mut self, name: Option<&str>) -> Result<String, String> {
        Ok(format!(
            "Scheduled procedural skill extraction: {}",
            name.unwrap_or("auto")
        ))
    }

    fn mcp_action(&mut self, action: &str, arg: Option<&str>) -> Result<String, String> {
        Ok(format!("MCP action={action} arg={}", arg.unwrap_or("")))
    }

    fn memory_action(&mut self, target: &str, action: &str, args: &[String]) -> Result<String, String> {
        let store = if target == "user" {
            &mut self.user_entries
        } else {
            &mut self.memory_entries
        };
        match action {
            "add" => {
                let val = args.join(" ");
                store.push(val.clone());
                Ok(format!("Added to {target}: {val}"))
            }
            "remove" => {
                let val = args.join(" ");
                store.retain(|e| !e.contains(&val));
                Ok(format!("Removed entries matching '{val}' from {target}"))
            }
            _ => Ok(if store.is_empty() {
                format!("{target} store is empty.")
            } else {
                store.join("\n§\n")
            }),
        }
    }

    fn soul_action(&mut self, action: &str, text: Option<&str>) -> Result<String, String> {
        if action == "set" {
            if let Some(t) = text {
                self.soul_text = t.to_string();
            }
        }
        Ok(self.soul_text.clone())
    }

    fn reload_memory(&mut self) -> Result<String, String> {
        Ok("Reloaded frozen memory snapshot from disk/DB.".into())
    }

    fn honcho_action(&mut self, action: &str, arg: Option<&str>) -> Result<String, String> {
        Ok(format!(
            "Honcho dialectic profile ({action}): {}",
            arg.unwrap_or("ready")
        ))
    }

    fn search_sessions(&self, query: &str) -> Result<String, String> {
        Ok(format!("Search results for '{query}': 0 matches in standalone host"))
    }

    fn session_history(&self, limit: usize) -> Result<String, String> {
        Ok(format!(
            "Session {} has {} messages (limit={limit}).",
            self.session_id,
            self.messages.len()
        ))
    }

    fn list_sessions(&self, _limit: usize) -> Result<String, String> {
        Ok(format!("Active session: {}", self.session_id))
    }

    fn export_session(
        &self,
        format: &str,
        _save_path: Option<&str>,
    ) -> Result<(String, String, String), String> {
        let ext = if format == "openai" || format == "openai_jsonl" {
            "jsonl"
        } else {
            "json"
        };
        Ok((
            format!("{}.{ext}", self.session_id),
            format.to_string(),
            format!("{{\"id\":\"{}\",\"format\":\"{format}\"}}", self.session_id),
        ))
    }

    fn usage_summary(&self, kind: &str) -> Result<String, String> {
        Ok(format!("Usage ({kind}): 0 tokens | $0.0000"))
    }

    fn status_report(&self) -> Result<String, String> {
        Ok(format!(
            "Session: {} | Model: {} ({}) | API Mode: {} | YOLO: {}",
            self.session_id, self.model, self.provider_id, self.api_mode, self.yolo
        ))
    }

    fn doctor_report(&self) -> Result<String, String> {
        Ok("Hermes Doctor: all subsystems nominal.".into())
    }

    fn context_report(&self) -> Result<String, String> {
        Ok(format!(
            "Context: {} messages | {} memory entries",
            self.messages.len(),
            self.memory_entries.len()
        ))
    }

    fn cron_action(&mut self, action: &str, args: &[String]) -> Result<String, String> {
        if action == "add" && !args.is_empty() {
            self.cron_jobs.push(args.join(" "));
        }
        Ok(format!("Cron ({action}): {} jobs", self.cron_jobs.len()))
    }

    fn kanban_action(&mut self, action: &str, args: &[String]) -> Result<String, String> {
        if action == "add" && !args.is_empty() {
            self.kanban_cards.push(args.join(" "));
        }
        Ok(format!("Kanban ({action}): {} cards", self.kanban_cards.len()))
    }

    fn heartbeat_action(&mut self, action: &str, interval_secs: Option<u64>) -> Result<String, String> {
        match action {
            "on" | "enable" => self.heartbeat_enabled = true,
            "off" | "disable" => self.heartbeat_enabled = false,
            _ => {}
        }
        if let Some(secs) = interval_secs {
            self.heartbeat_interval_secs = secs;
        }
        Ok(format!(
            "Heartbeat: enabled={} interval={}s",
            self.heartbeat_enabled, self.heartbeat_interval_secs
        ))
    }

    fn approval_action(
        &mut self,
        action: &str,
        target: Option<&str>,
        _reason: Option<&str>,
    ) -> Result<String, String> {
        if action == "yolo" {
            self.yolo = match target {
                Some("off") | Some("false") => false,
                Some("on") | Some("true") => true,
                _ => !self.yolo,
            };
            return Ok(format!("YOLO mode: {}", if self.yolo { "ON" } else { "OFF" }));
        }
        Ok(format!("Approval action '{action}' applied ({})", target.unwrap_or("latest")))
    }

    fn queue_action(&mut self, action: &str, text: Option<&str>) -> Result<String, String> {
        if action == "queue" {
            if let Some(t) = text {
                self.queued.push(t.to_string());
            }
        }
        Ok(format!("Queue ({action}): {} pending", self.queued.len()))
    }

    fn set_toggle(&mut self, key: &str, value: Option<&str>) -> Result<String, String> {
        let next = match value.map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("on") | Some("true") | Some("1") => true,
            Some("off") | Some("false") | Some("0") => false,
            _ => !self.toggles.get(key).copied().unwrap_or(false),
        };
        self.toggles.insert(key.to_string(), next);
        Ok(format!("{key}: {}", if next { "ON" } else { "OFF" }))
    }

    fn config_action(
        &mut self,
        action: &str,
        key: Option<&str>,
        val: Option<&str>,
    ) -> Result<String, String> {
        Ok(format!(
            "Config ({action}) {}={}",
            key.unwrap_or("all"),
            val.unwrap_or("")
        ))
    }
}

// ============================================================================
// SlashCommandRegistry (All 61+ Hermes Commands + Autocomplete)
// ============================================================================

#[derive(Debug, Clone)]
pub struct SlashCommandRegistry {
    descriptors: Vec<CommandDescriptor>,
    index_by_name: HashMap<String, usize>,
}

impl Default for SlashCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SlashCommandRegistry {
    pub fn new() -> Self {
        let mut reg = Self {
            descriptors: Vec::new(),
            index_by_name: HashMap::new(),
        };
        reg.register_builtin_commands();
        reg
    }

    fn add(
        &mut self,
        name: &str,
        aliases: &[&str],
        syntax: &str,
        category: CommandCategory,
        description: &str,
        accepts_args: bool,
    ) {
        let idx = self.descriptors.len();
        let desc = CommandDescriptor {
            name: name.to_string(),
            slash_name: format!("/{name}"),
            aliases: aliases.iter().map(|s| (*s).to_string()).collect(),
            syntax: syntax.to_string(),
            category,
            description: description.to_string(),
            accepts_args,
        };
        self.index_by_name.insert(name.to_string(), idx);
        for alias in aliases {
            self.index_by_name.entry((*alias).to_string()).or_insert(idx);
        }
        self.descriptors.push(desc);
    }

    fn register_builtin_commands(&mut self) {
        use CommandCategory::*;

        // --- Session & History Management ---
        self.add("help", &["h", "?"], "/help [command|category]", Session, "Show Hermes slash commands and usage help", true);
        self.add("new", &["start"], "/new [title]", Session, "Start a fresh conversation session", true);
        self.add("reset", &[], "/reset", Session, "Reset current conversation and reload fresh snapshot", false);
        self.add("clear", &[], "/clear", Session, "Clear messages in the active session", false);
        self.add("compact", &[], "/compact [focus]", Session, "Summarize middle turns to fit context budget", true);
        self.add("compress", &[], "/compress [focus]", Session, "Compress active conversation context with structured summary", true);
        self.add("undo", &[], "/undo", Session, "Undo the last user/assistant turn pair", false);
        self.add("rollback", &[], "/rollback [turn]", Session, "Restore files from automatic pre-edit checkpoints", true);
        self.add("branch", &[], "/branch [name]", Session, "Fork current session into a new named branch", true);
        self.add("branches", &[], "/branches", Session, "List all branches of the current conversation tree", false);
        self.add("checkout", &[], "/checkout <branch_id|title>", Session, "Switch active session to another branch", true);

        // --- Model, Provider & MoA ---
        self.add("model", &["m"], "/model [provider/model]", ModelAndProvider, "View or switch the active LLM model", true);
        self.add("provider", &[], "/provider [list|use <id>]", ModelAndProvider, "Inspect or switch active inference provider", true);
        self.add("api-mode", &["apimode"], "/api-mode [chat_completions|codex_responses|anthropic_messages|gemini_generate_content|local_hermes]", ModelAndProvider, "Switch wire protocol mode", true);
        self.add("moa", &[], "/moa <prompt>", ModelAndProvider, "Run Mixture-of-Agents across multiple models/perspectives and synthesize", true);

        // --- Tools, Toolsets, Skills & MCP ---
        self.add("toolsets", &[], "/toolsets [list|enable <name>|disable <name>|preset <name>]", ToolsAndSkills, "Inspect or toggle the 28 Hermes toolsets", true);
        self.add("tools", &[], "/tools [filter]", ToolsAndSkills, "List active tools exposed to the model", true);
        self.add("skills", &[], "/skills [search|list|approve <name>]", ToolsAndSkills, "List and manage SKILL.md skills", true);
        self.add("skill", &[], "/skill <name>", ToolsAndSkills, "View full instructions for a specific SKILL.md skill", true);
        self.add("learn", &[], "/learn [skill_name]", ToolsAndSkills, "Extract a reusable procedural skill from the current session", true);
        self.add("mcp", &[], "/mcp [list|call <tool> <json>|status]", ToolsAndSkills, "Inspect and invoke MCP server tools", true);

        // --- Memory, Persona & Honcho ---
        self.add("memory", &["mem"], "/memory [view|add <text>|replace <old> => <new>|remove <text>]", MemoryAndPersona, "Inspect or edit bounded MEMORY.md (2200 chars)", true);
        self.add("soul", &[], "/soul [view|set <persona>]", MemoryAndPersona, "Inspect or update the bot's SOUL.md persona", true);
        self.add("user", &[], "/user [view|add <text>|remove <text>]", MemoryAndPersona, "Inspect or edit bounded USER.md profile (1375 chars)", true);
        self.add("honcho", &[], "/honcho [profile|query <text>|observe <note>]", MemoryAndPersona, "Inspect or query Honcho dialectic user model", true);
        self.add("reload-memory", &["reload_memory"], "/reload-memory", MemoryAndPersona, "Refresh frozen MEMORY.md/USER.md snapshot mid-session", false);

        // --- Search, History & Export ---
        self.add("search", &["find"], "/search <query>", InspectionAndExport, "Full-text search (FTS5 + CJK) across past sessions", true);
        self.add("history", &[], "/history [limit]", InspectionAndExport, "Show recent turns in the active session", true);
        self.add("sessions", &[], "/sessions [limit]", InspectionAndExport, "List persisted Hermes sessions", true);
        self.add("export", &[], "/export [sharegpt|openai|anthropic|native]", InspectionAndExport, "Export redacted training trajectory for this session", true);
        self.add("save", &[], "/save [path] [format]", InspectionAndExport, "Save redacted session trajectory or transcript to file", true);

        // --- Telemetry & Diagnostics ---
        self.add("usage", &[], "/usage", InspectionAndExport, "Show token usage, cache hit stats, and cost breakdown", false);
        self.add("cost", &[], "/cost", InspectionAndExport, "Show estimated USD cost for current session & totals", false);
        self.add("insights", &[], "/insights", InspectionAndExport, "Show session analytics, top tools, and usage insights", false);
        self.add("status", &[], "/status", InspectionAndExport, "Show active session, model, toolsets, and background loops", false);
        self.add("doctor", &[], "/doctor", InspectionAndExport, "Run diagnostics on SQLite DB, memory budgets, providers, and tools", false);
        self.add("diff", &[], "/diff", InspectionAndExport, "Show workspace git diff or modified checkpoint summary", false);
        self.add("context", &[], "/context", InspectionAndExport, "Inspect 3-tier system prompt tokens, frozen snapshot, and skill index", false);
        self.add("version", &["ver"], "/version", InspectionAndExport, "Display Hermes Rust Engine version and protocol capabilities", false);

        // --- Workflow, Cron, Kanban, Goals & Heartbeat ---
        self.add("cron", &[], "/cron [list|add <expr> <prompt>|pause <id>|resume <id>|run <id>|delete <id>|history]", WorkflowAndAutomation, "Manage scheduled cron jobs", true);
        self.add("jobs", &[], "/jobs", WorkflowAndAutomation, "List active cron jobs and background processes", false);
        self.add("kanban", &[], "/kanban [board|add <title>|move <card_id> <col>|list]", WorkflowAndAutomation, "Manage Hermes Kanban boards and task cards", true);
        self.add("board", &[], "/board [board_id]", WorkflowAndAutomation, "Open or inspect the Kanban board", true);
        self.add("goal", &[], "/goal <objective>", WorkflowAndAutomation, "Run an autonomous multi-step /goal verification loop", true);
        self.add("heartbeat", &[], "/heartbeat [on|off|interval <secs>|status]", WorkflowAndAutomation, "Configure periodic proactive heartbeat ticks", true);

        // --- Safety, Permissions & Turn Control ---
        self.add("approve", &[], "/approve [id|latest]", SafetyAndControl, "Approve a pending dangerous command or skill write", true);
        self.add("deny", &[], "/deny [id|latest] [reason]", SafetyAndControl, "Deny a pending dangerous command", true);
        self.add("yolo", &[], "/yolo [on|off]", SafetyAndControl, "Toggle auto-approval YOLO mode (dangerous commands bypass prompt)", true);
        self.add("permissions", &["perms"], "/permissions [mode|list]", SafetyAndControl, "Inspect or configure tool approval mode and pending queue", true);
        self.add("queue", &[], "/queue <message>", SafetyAndControl, "Queue a follow-up message behind an in-flight turn", true);
        self.add("steer", &[], "/steer <instruction>", SafetyAndControl, "Steer an active turn with updated instructions", true);
        self.add("interrupt", &[], "/interrupt", SafetyAndControl, "Interrupt the currently streaming agent turn", false);
        self.add("stop", &["cancel"], "/stop", SafetyAndControl, "Stop the active turn and background goal loop", false);

        // --- Modes, Voice, Computer Use, Browser & Settings ---
        self.add("voice", &[], "/voice [on|off|tts <text>]", ModesAndUi, "Toggle voice mode or synthesize speech preview", true);
        self.add("talk", &[], "/talk", ModesAndUi, "Launch live AI Talk Mode voice conversation", false);
        self.add("computer", &["cua"], "/computer", ModesAndUi, "Open Remote Computer Use workbench", false);
        self.add("browser", &[], "/browser [url|snapshot|status]", ModesAndUi, "Control or toggle CDP browser automation", true);
        self.add("verbose", &[], "/verbose [on|off]", ModesAndUi, "Toggle verbose tool output streaming", true);
        self.add("reasoning", &["think"], "/reasoning [on|off|budget <tokens>]", ModesAndUi, "Toggle or adjust model thinking/reasoning visibility", true);
        self.add("debug", &[], "/debug [on|off]", ModesAndUi, "Toggle runtime debug trace output", true);
        self.add("config", &[], "/config [show|set <key> <val>]", ModesAndUi, "View or update Hermes runtime configuration", true);
    }

    pub fn list_descriptors(&self) -> &[CommandDescriptor] {
        &self.descriptors
    }

    pub fn count(&self) -> usize {
        self.descriptors.len()
    }

    pub fn get(&self, name_or_alias: &str) -> Option<&CommandDescriptor> {
        let normalized = name_or_alias
            .trim()
            .trim_start_matches('/')
            .split('@')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let idx = *self.index_by_name.get(&normalized)?;
        self.descriptors.get(idx)
    }

    /// Returns ranked autocomplete candidates for a `/` prefix (e.g. `"/me"` or `"mem"`).
    pub fn autocomplete(&self, prefix: &str) -> Vec<CommandDescriptor> {
        let q = prefix
            .trim()
            .trim_start_matches('/')
            .to_ascii_lowercase();
        if q.is_empty() {
            return self.descriptors.clone();
        }

        let mut exact_prefix = Vec::new();
        let mut alias_prefix = Vec::new();
        let mut substring = Vec::new();

        for desc in &self.descriptors {
            if desc.name.starts_with(&q) {
                exact_prefix.push(desc.clone());
            } else if desc.aliases.iter().any(|a| a.starts_with(&q)) {
                alias_prefix.push(desc.clone());
            } else if desc.name.contains(&q)
                || desc.description.to_ascii_lowercase().contains(&q)
            {
                substring.push(desc.clone());
            }
        }

        exact_prefix.extend(alias_prefix);
        exact_prefix.extend(substring);
        exact_prefix
    }

    pub fn execute_raw(&self, input: &str, host: &mut dyn CommandHost) -> CommandExecutionOutcome {
        let Some(parsed) = SlashCommandParser::parse(input) else {
            return CommandExecutionOutcome {
                handled: false,
                command: String::new(),
                response_text: String::new(),
                is_error: false,
                buttons: Vec::new(),
                ui_effect: CommandUiEffect::None,
                mutated_fields: Vec::new(),
                should_continue_to_llm: false,
                rewritten_prompt: None,
            };
        };
        self.execute(&parsed, host)
    }

    pub fn execute(
        &self,
        parsed: &ParsedSlashCommand,
        host: &mut dyn CommandHost,
    ) -> CommandExecutionOutcome {
        let canonical = self
            .get(&parsed.command)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| parsed.command.clone());

        let args_str = parsed.args.trim();
        let first_arg = parsed.argv.first().map(String::as_str);

        match canonical.as_str() {
            "help" => self.exec_help(args_str),

            "new" | "reset" => {
                let title = (!args_str.is_empty()).then_some(args_str);
                match host.new_session(title) {
                    Ok(id) => CommandExecutionOutcome::ok(
                        canonical,
                        format!("Started fresh Hermes session `{id}`."),
                    )
                    .with_effect(CommandUiEffect::RefreshSessionState)
                    .with_mutated(vec!["session_id".into()]),
                    Err(e) => CommandExecutionOutcome::err(canonical, e),
                }
            }

            "clear" => match host.clear_session_messages() {
                Ok(n) => CommandExecutionOutcome::ok(
                    "clear",
                    format!("Cleared {n} messages from active session `{}`.", host.active_session_id()),
                )
                .with_effect(CommandUiEffect::ClearChatDisplay)
                .with_mutated(vec!["messages".into()]),
                Err(e) => CommandExecutionOutcome::err("clear", e),
            },

            "compact" | "compress" => {
                let focus = (!args_str.is_empty()).then_some(args_str);
                match host.compact_context(focus) {
                    Ok(summary) => CommandExecutionOutcome::ok(canonical, summary)
                        .with_mutated(vec!["messages".into(), "context".into()]),
                    Err(e) => CommandExecutionOutcome::err(canonical, e),
                }
            }

            "undo" => match host.undo_last_turn() {
                Ok(msg) => CommandExecutionOutcome::ok("undo", msg)
                    .with_effect(CommandUiEffect::RefreshSessionState)
                    .with_mutated(vec!["messages".into()]),
                Err(e) => CommandExecutionOutcome::err("undo", e),
            },

            "rollback" => match host.rollback_last_checkpoint() {
                Ok(msg) => CommandExecutionOutcome::ok("rollback", msg)
                    .with_mutated(vec!["workspace_files".into()]),
                Err(e) => CommandExecutionOutcome::err("rollback", e),
            },

            "branch" => {
                let name = (!args_str.is_empty()).then_some(args_str);
                match host.branch_session(name) {
                    Ok(id) => CommandExecutionOutcome::ok(
                        "branch",
                        format!("Created & switched to session branch `{id}`."),
                    )
                    .with_effect(CommandUiEffect::RefreshSessionState)
                    .with_mutated(vec!["session_id".into(), "branches".into()]),
                    Err(e) => CommandExecutionOutcome::err("branch", e),
                }
            }

            "branches" => match host.list_branches() {
                Ok(branches) => {
                    if branches.is_empty() {
                        CommandExecutionOutcome::ok("branches", "No branches recorded yet.")
                    } else {
                        let lines: Vec<String> = branches
                            .into_iter()
                            .map(|b| {
                                let mark = if b.active { "*" } else { " " };
                                format!(
                                    "[{mark}] `{}` — {} ({} msgs)",
                                    b.session_id, b.title, b.message_count
                                )
                            })
                            .collect();
                        CommandExecutionOutcome::ok(
                            "branches",
                            format!("Session branches:\n{}", lines.join("\n")),
                        )
                    }
                }
                Err(e) => CommandExecutionOutcome::err("branches", e),
            },

            "checkout" => {
                let Some(target) = first_arg else {
                    return CommandExecutionOutcome::err(
                        "checkout",
                        "Usage: `/checkout <branch_id|title>`",
                    );
                };
                match host.checkout_branch(target) {
                    Ok(id) => CommandExecutionOutcome::ok(
                        "checkout",
                        format!("Switched active session to branch `{id}`."),
                    )
                    .with_effect(CommandUiEffect::RefreshSessionState)
                    .with_mutated(vec!["session_id".into()]),
                    Err(e) => CommandExecutionOutcome::err("checkout", e),
                }
            }

            "model" => {
                if args_str.is_empty() {
                    let info = host.current_model_info();
                    CommandExecutionOutcome::ok(
                        "model",
                        format!(
                            "Active model: `{}` (provider: `{}`, api_mode: `{}`)",
                            info.model, info.provider_id, info.api_mode
                        ),
                    )
                } else {
                    match host.set_model(args_str) {
                        Ok(msg) => CommandExecutionOutcome::ok("model", msg)
                            .with_mutated(vec!["model".into()]),
                        Err(e) => CommandExecutionOutcome::err("model", e),
                    }
                }
            }

            "provider" => {
                if args_str.is_empty() || first_arg == Some("list") {
                    let list = host.list_providers();
                    let lines: Vec<String> = list
                        .into_iter()
                        .map(|p| {
                            let mark = if p.active { "*" } else { " " };
                            format!(
                                "[{mark}] `{}` ({}) — mode={}, default_model=`{}`",
                                p.id, p.name, p.api_mode, p.default_model
                            )
                        })
                        .collect();
                    CommandExecutionOutcome::ok(
                        "provider",
                        format!("Configured providers:\n{}", lines.join("\n")),
                    )
                } else {
                    let prov_id = if first_arg == Some("use") {
                        parsed.argv.get(1).map(String::as_str).unwrap_or("")
                    } else {
                        args_str
                    };
                    if prov_id.is_empty() {
                        return CommandExecutionOutcome::err(
                            "provider",
                            "Usage: `/provider [list|use <provider_id>]`",
                        );
                    }
                    match host.set_provider(prov_id) {
                        Ok(msg) => CommandExecutionOutcome::ok("provider", msg)
                            .with_mutated(vec!["provider".into()]),
                        Err(e) => CommandExecutionOutcome::err("provider", e),
                    }
                }
            }

            "api-mode" => {
                if args_str.is_empty() {
                    let info = host.current_model_info();
                    CommandExecutionOutcome::ok(
                        "api-mode",
                        format!(
                            "Current API mode: `{}`.\nSupported: `chat_completions`, `codex_responses`, `anthropic_messages`, `gemini_generate_content`, `local_hermes`.",
                            info.api_mode
                        ),
                    )
                } else {
                    match host.set_api_mode(args_str) {
                        Ok(msg) => CommandExecutionOutcome::ok("api-mode", msg)
                            .with_mutated(vec!["api_mode".into()]),
                        Err(e) => CommandExecutionOutcome::err("api-mode", e),
                    }
                }
            }

            "moa" => {
                if args_str.is_empty() {
                    return CommandExecutionOutcome::err(
                        "moa",
                        "Usage: `/moa <prompt>` — fans request across multiple models/perspectives and synthesizes.",
                    );
                }
                let mut out = CommandExecutionOutcome::ok(
                    "moa",
                    format!("Dispatching Mixture-of-Agents synthesis for: \"{args_str}\""),
                )
                .with_effect(CommandUiEffect::LaunchMoa {
                    prompt: args_str.to_string(),
                    models: Vec::new(),
                });
                out.should_continue_to_llm = true;
                out.rewritten_prompt = Some(args_str.to_string());
                out
            }

            "toolsets" | "tools" => {
                let action = first_arg.unwrap_or("list");
                let arg = parsed.argv.get(1).map(String::as_str);
                if action == "list" && arg.is_none() {
                    let list = host.list_toolsets();
                    let lines: Vec<String> = list
                        .into_iter()
                        .map(|t| {
                            let mark = if t.enabled { "ON " } else { "OFF" };
                            format!("[{mark}] `{}` ({} tools) — {}", t.id, t.tool_count, t.description)
                        })
                        .collect();
                    CommandExecutionOutcome::ok(
                        canonical,
                        format!("Hermes Toolsets ({}):\n{}", lines.len(), lines.join("\n")),
                    )
                } else {
                    match host.mutate_toolsets(action, arg) {
                        Ok(msg) => CommandExecutionOutcome::ok(canonical, msg)
                            .with_mutated(vec!["toolsets".into()]),
                        Err(e) => CommandExecutionOutcome::err(canonical, e),
                    }
                }
            }

            "skills" => {
                if matches!(first_arg, Some("view")) {
                    if let Some(name) = parsed.argv.get(1) {
                        return match host.view_skill(name) {
                            Ok(txt) => CommandExecutionOutcome::ok("skills", txt),
                            Err(e) => CommandExecutionOutcome::err("skills", e),
                        };
                    }
                }
                let skills = host.list_skills();
                let filter = args_str.to_ascii_lowercase();
                let lines: Vec<String> = skills
                    .into_iter()
                    .filter(|s| {
                        filter.is_empty()
                            || filter == "list"
                            || s.name.to_ascii_lowercase().contains(&filter)
                            || s.description.to_ascii_lowercase().contains(&filter)
                    })
                    .map(|s| format!("- `{}` [{}]: {}", s.name, s.origin, s.description))
                    .collect();
                CommandExecutionOutcome::ok(
                    "skills",
                    if lines.is_empty() {
                        "No matching skills found.".into()
                    } else {
                        format!("Hermes Skills:\n{}", lines.join("\n"))
                    },
                )
            }

            "skill" => {
                let Some(name) = first_arg else {
                    return CommandExecutionOutcome::err("skill", "Usage: `/skill <name>`");
                };
                match host.view_skill(name) {
                    Ok(txt) => CommandExecutionOutcome::ok("skill", txt),
                    Err(e) => CommandExecutionOutcome::err("skill", e),
                }
            }

            "learn" => {
                let name = (!args_str.is_empty()).then_some(args_str);
                match host.learn_skill(name) {
                    Ok(msg) => CommandExecutionOutcome::ok("learn", msg)
                        .with_effect(CommandUiEffect::LearnFromSession {
                            skill_name: name.map(ToString::to_string),
                        })
                        .with_mutated(vec!["skills".into()]),
                    Err(e) => CommandExecutionOutcome::err("learn", e),
                }
            }

            "mcp" => {
                let action = first_arg.unwrap_or("list");
                let rest = if parsed.argv.len() > 1 {
                    Some(parsed.argv[1..].join(" "))
                } else {
                    None
                };
                match host.mcp_action(action, rest.as_deref()) {
                    Ok(msg) => CommandExecutionOutcome::ok("mcp", msg),
                    Err(e) => CommandExecutionOutcome::err("mcp", e),
                }
            }

            "memory" | "user" => {
                let target = if canonical == "user" { "user" } else { "memory" };
                let (action, rest_args): (&str, Vec<String>) = match first_arg {
                    None => ("view", Vec::new()),
                    Some("view") | Some("list") | Some("show") => ("view", Vec::new()),
                    Some("add") | Some("append") => ("add", parsed.argv[1..].to_vec()),
                    Some("remove") | Some("rm") | Some("del") => {
                        ("remove", parsed.argv[1..].to_vec())
                    }
                    Some("replace") => ("replace", parsed.argv[1..].to_vec()),
                    Some(_) => ("add", parsed.argv.clone()),
                };
                match host.memory_action(target, action, &rest_args) {
                    Ok(msg) => CommandExecutionOutcome::ok(canonical, msg)
                        .with_mutated(vec![target.into()]),
                    Err(e) => CommandExecutionOutcome::err(canonical, e),
                }
            }

            "soul" => {
                let (action, text) = if args_str.is_empty() || first_arg == Some("view") {
                    ("view", None)
                } else if first_arg == Some("set") {
                    let body = args_str.strip_prefix("set").unwrap_or("").trim();
                    ("set", Some(body))
                } else {
                    ("set", Some(args_str))
                };
                match host.soul_action(action, text) {
                    Ok(msg) => CommandExecutionOutcome::ok("soul", msg)
                        .with_mutated(if action == "set" { vec!["soul".into()] } else { vec![] }),
                    Err(e) => CommandExecutionOutcome::err("soul", e),
                }
            }

            "reload-memory" => match host.reload_memory() {
                Ok(msg) => CommandExecutionOutcome::ok("reload-memory", msg)
                    .with_mutated(vec!["memory_snapshot".into()]),
                Err(e) => CommandExecutionOutcome::err("reload-memory", e),
            },

            "honcho" => {
                let action = first_arg.unwrap_or("profile");
                let rest = if parsed.argv.len() > 1 {
                    Some(parsed.argv[1..].join(" "))
                } else {
                    None
                };
                match host.honcho_action(action, rest.as_deref()) {
                    Ok(msg) => CommandExecutionOutcome::ok("honcho", msg),
                    Err(e) => CommandExecutionOutcome::err("honcho", e),
                }
            }

            "search" => {
                if args_str.is_empty() {
                    return CommandExecutionOutcome::err("search", "Usage: `/search <query>`");
                }
                match host.search_sessions(args_str) {
                    Ok(msg) => CommandExecutionOutcome::ok("search", msg),
                    Err(e) => CommandExecutionOutcome::err("search", e),
                }
            }

            "history" => {
                let limit = first_arg.and_then(|s| s.parse::<usize>().ok()).unwrap_or(20);
                match host.session_history(limit) {
                    Ok(msg) => CommandExecutionOutcome::ok("history", msg),
                    Err(e) => CommandExecutionOutcome::err("history", e),
                }
            }

            "sessions" => {
                let limit = first_arg.and_then(|s| s.parse::<usize>().ok()).unwrap_or(20);
                match host.list_sessions(limit) {
                    Ok(msg) => CommandExecutionOutcome::ok("sessions", msg),
                    Err(e) => CommandExecutionOutcome::err("sessions", e),
                }
            }

            "export" | "save" => {
                let (format, path) = if canonical == "save" {
                    let p = first_arg;
                    let fmt = parsed.argv.get(1).map(String::as_str).unwrap_or("sharegpt");
                    (fmt, p)
                } else {
                    let fmt = first_arg.unwrap_or("sharegpt");
                    let p = parsed.argv.get(1).map(String::as_str);
                    (fmt, p)
                };
                match host.export_session(format, path) {
                    Ok((filename, fmt, content)) => {
                        let preview = if content.len() > 1200 {
                            format!("{}...\n({} bytes total)", &content[..1200], content.len())
                        } else {
                            content.clone()
                        };
                        CommandExecutionOutcome::ok(
                            canonical,
                            format!("Exported redacted `{fmt}` trajectory `{filename}`:\n```json\n{preview}\n```"),
                        )
                        .with_effect(CommandUiEffect::ExportFile {
                            filename,
                            format: fmt,
                            content,
                        })
                    }
                    Err(e) => CommandExecutionOutcome::err(canonical, e),
                }
            }

            "usage" | "cost" | "insights" => match host.usage_summary(&canonical) {
                Ok(msg) => CommandExecutionOutcome::ok(canonical, msg),
                Err(e) => CommandExecutionOutcome::err(canonical, e),
            },

            "status" => match host.status_report() {
                Ok(msg) => CommandExecutionOutcome::ok("status", msg),
                Err(e) => CommandExecutionOutcome::err("status", e),
            },

            "doctor" => match host.doctor_report() {
                Ok(msg) => CommandExecutionOutcome::ok("doctor", msg),
                Err(e) => CommandExecutionOutcome::err("doctor", e),
            },

            "diff" => match host.diff_workspace() {
                Ok(msg) => CommandExecutionOutcome::ok("diff", msg)
                    .with_effect(CommandUiEffect::ShowDiff),
                Err(e) => CommandExecutionOutcome::err("diff", e),
            },

            "context" => match host.context_report() {
                Ok(msg) => CommandExecutionOutcome::ok("context", msg),
                Err(e) => CommandExecutionOutcome::err("context", e),
            },

            "version" => CommandExecutionOutcome::ok(
                "version",
                "Hermes Pure-Rust Agent Engine v1.0.0 (Wire Protocols: chat_completions, codex_responses, anthropic_messages, gemini_generate_content, local_hermes | 28 Toolsets | 61+ Slash Commands)",
            ),

            "cron" | "jobs" => {
                let action = if canonical == "jobs" {
                    "list"
                } else {
                    first_arg.unwrap_or("list")
                };
                let rest = if parsed.argv.len() > 1 {
                    &parsed.argv[1..]
                } else {
                    &[]
                };
                let mut effect = CommandUiEffect::None;
                if action == "run" {
                    if let Some(job_id) = rest.first() {
                        effect = CommandUiEffect::RunCronNow {
                            job_id: job_id.clone(),
                        };
                    }
                }
                match host.cron_action(action, rest) {
                    Ok(msg) => CommandExecutionOutcome::ok(canonical, msg)
                        .with_effect(effect)
                        .with_mutated(if action != "list" && action != "history" {
                            vec!["cron_jobs".into()]
                        } else {
                            vec![]
                        }),
                    Err(e) => CommandExecutionOutcome::err(canonical, e),
                }
            }

            "kanban" | "board" => {
                let action = if canonical == "board" {
                    "board"
                } else {
                    first_arg.unwrap_or("list")
                };
                let rest = if canonical == "board" {
                    &parsed.argv[..]
                } else if parsed.argv.len() > 1 {
                    &parsed.argv[1..]
                } else {
                    &[]
                };
                match host.kanban_action(action, rest) {
                    Ok(msg) => CommandExecutionOutcome::ok(canonical, msg)
                        .with_effect(CommandUiEffect::OpenHermesStudio {
                            tab: "kanban".to_string(),
                        })
                        .with_mutated(vec!["kanban".into()]),
                    Err(e) => CommandExecutionOutcome::err(canonical, e),
                }
            }

            "goal" => {
                if args_str.is_empty() {
                    return CommandExecutionOutcome::err(
                        "goal",
                        "Usage: `/goal <objective>` — runs an autonomous verification loop until goal completion.",
                    );
                }
                let mut out = CommandExecutionOutcome::ok(
                    "goal",
                    format!("Autonomous `/goal` loop engaged for: \"{args_str}\""),
                )
                .with_effect(CommandUiEffect::StartGoalLoop {
                    goal: args_str.to_string(),
                    max_iterations: 8,
                });
                out.should_continue_to_llm = true;
                out.rewritten_prompt = Some(args_str.to_string());
                out
            }

            "heartbeat" => {
                let action = first_arg.unwrap_or("status");
                let interval = parsed
                    .argv
                    .iter()
                    .find_map(|a| a.parse::<u64>().ok());
                match host.heartbeat_action(action, interval) {
                    Ok(msg) => CommandExecutionOutcome::ok("heartbeat", msg)
                        .with_mutated(vec!["heartbeat".into()]),
                    Err(e) => CommandExecutionOutcome::err("heartbeat", e),
                }
            }

            "approve" | "deny" | "yolo" | "permissions" => {
                let target = first_arg;
                let reason = if canonical == "deny" && parsed.argv.len() > 1 {
                    Some(parsed.argv[1..].join(" "))
                } else {
                    None
                };
                match host.approval_action(&canonical, target, reason.as_deref()) {
                    Ok(msg) => CommandExecutionOutcome::ok(canonical, msg)
                        .with_mutated(vec!["approvals".into()]),
                    Err(e) => CommandExecutionOutcome::err(canonical, e),
                }
            }

            "queue" => {
                if args_str.is_empty() {
                    match host.queue_action("list", None) {
                        Ok(msg) => CommandExecutionOutcome::ok("queue", msg),
                        Err(e) => CommandExecutionOutcome::err("queue", e),
                    }
                } else {
                    let sid = host.active_session_id();
                    match host.queue_action("queue", Some(args_str)) {
                        Ok(msg) => CommandExecutionOutcome::ok("queue", msg)
                            .with_effect(CommandUiEffect::QueueMessage {
                                session_id: sid,
                                message: args_str.to_string(),
                            })
                            .with_mutated(vec!["message_queue".into()]),
                        Err(e) => CommandExecutionOutcome::err("queue", e),
                    }
                }
            }

            "steer" => {
                if args_str.is_empty() {
                    return CommandExecutionOutcome::err("steer", "Usage: `/steer <instruction>`");
                }
                let sid = host.active_session_id();
                match host.queue_action("steer", Some(args_str)) {
                    Ok(msg) => CommandExecutionOutcome::ok("steer", msg)
                        .with_effect(CommandUiEffect::SteerActiveTurn {
                            session_id: sid,
                            instruction: args_str.to_string(),
                        })
                        .with_mutated(vec!["steer".into()]),
                    Err(e) => CommandExecutionOutcome::err("steer", e),
                }
            }

            "interrupt" | "stop" => {
                let sid = host.active_session_id();
                match host.queue_action(&canonical, None) {
                    Ok(msg) => CommandExecutionOutcome::ok(canonical, msg)
                        .with_effect(CommandUiEffect::StopActiveTurn { session_id: sid })
                        .with_mutated(vec!["active_turn".into()]),
                    Err(e) => CommandExecutionOutcome::err(canonical, e),
                }
            }

            "talk" => CommandExecutionOutcome::ok(
                "talk",
                "Launching AI Talk Mode voice session...",
            )
            .with_effect(CommandUiEffect::OpenTalkMode)
            .with_buttons(vec![vec![InlineActionButton {
                text: "Launch Talk Mode".into(),
                callback_data: "/talk".into(),
                url: None,
            }]]),

            "computer" => CommandExecutionOutcome::ok(
                "computer",
                "Opening Remote Computer Use workbench...",
            )
            .with_effect(CommandUiEffect::OpenRemoteComputer)
            .with_buttons(vec![vec![InlineActionButton {
                text: "Remote Computer".into(),
                callback_data: "/computer".into(),
                url: None,
            }]]),

            "voice" | "browser" | "verbose" | "reasoning" | "debug" => {
                match host.set_toggle(&canonical, first_arg) {
                    Ok(msg) => {
                        let mutated = canonical.clone();
                        CommandExecutionOutcome::ok(canonical, msg).with_mutated(vec![mutated])
                    }
                    Err(e) => CommandExecutionOutcome::err(canonical, e),
                }
            }

            "config" => {
                let action = first_arg.unwrap_or("show");
                let key = parsed.argv.get(1).map(String::as_str);
                let val = parsed.argv.get(2).map(String::as_str);
                match host.config_action(action, key, val) {
                    Ok(msg) => CommandExecutionOutcome::ok("config", msg),
                    Err(e) => CommandExecutionOutcome::err("config", e),
                }
            }

            unknown => CommandExecutionOutcome::err(
                unknown,
                format!("Unknown Hermes slash command `/{unknown}`. Type `/help` to list all {} commands.", self.descriptors.len()),
            ),
        }
    }

    fn exec_help(&self, filter: &str) -> CommandExecutionOutcome {
        if !filter.is_empty() {
            if let Some(desc) = self.get(filter) {
                let aliases = if desc.aliases.is_empty() {
                    "none".to_string()
                } else {
                    desc.aliases
                        .iter()
                        .map(|a| format!("/{a}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                return CommandExecutionOutcome::ok(
                    "help",
                    format!(
                        "**{}** ({})\nSyntax: `{}`\nAliases: {}\n{}",
                        desc.slash_name, desc.category, desc.syntax, aliases, desc.description
                    ),
                );
            }
        }

        let mut by_cat: Vec<(CommandCategory, Vec<&CommandDescriptor>)> = Vec::new();
        for cat in [
            CommandCategory::Session,
            CommandCategory::ModelAndProvider,
            CommandCategory::ToolsAndSkills,
            CommandCategory::MemoryAndPersona,
            CommandCategory::WorkflowAndAutomation,
            CommandCategory::SafetyAndControl,
            CommandCategory::InspectionAndExport,
            CommandCategory::ModesAndUi,
        ] {
            let items: Vec<_> = self
                .descriptors
                .iter()
                .filter(|d| d.category == cat)
                .collect();
            if !items.is_empty() {
                by_cat.push((cat, items));
            }
        }

        let mut sections = vec![format!(
            "Hermes Command Reference ({} commands):",
            self.descriptors.len()
        )];
        for (cat, items) in by_cat {
            let lines: Vec<String> = items
                .into_iter()
                .map(|d| format!("  `{}` — {}", d.syntax, d.description))
                .collect();
            sections.push(format!("### {}\n{}", cat, lines.join("\n")));
        }

        CommandExecutionOutcome::ok("help", sections.join("\n\n")).with_buttons(vec![vec![
            InlineActionButton {
                text: "Launch Talk Mode".into(),
                callback_data: "/talk".into(),
                url: None,
            },
            InlineActionButton {
                text: "Remote Computer".into(),
                callback_data: "/computer".into(),
                url: None,
            },
        ]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_61_required_commands_registered_and_executable() {
        let required = [
            "help", "new", "reset", "clear", "model", "provider", "api-mode", "toolsets",
            "tools", "skills", "skill", "learn", "memory", "soul", "user", "compact",
            "compress", "undo", "rollback", "branch", "branches", "checkout", "search",
            "history", "sessions", "export", "save", "usage", "cost", "insights", "status",
            "doctor", "cron", "jobs", "kanban", "board", "approve", "deny", "yolo",
            "permissions", "queue", "steer", "interrupt", "stop", "goal", "heartbeat",
            "voice", "talk", "computer", "browser", "mcp", "honcho", "moa", "diff",
            "context", "verbose", "reasoning", "debug", "config", "reload-memory", "version",
        ];

        let reg = SlashCommandRegistry::new();
        assert!(reg.count() >= 61, "Expected >= 61 commands, got {}", reg.count());

        let mut host = StandaloneCommandHost::default();
        for name in required {
            assert!(reg.get(name).is_some(), "Missing command /{name}");
            let input = match name {
                "checkout" => "/checkout sess_default".to_string(),
                "skill" => "/skill hermes-research".to_string(),
                "search" => "/search rust".to_string(),
                "goal" => "/goal verify all tests".to_string(),
                "moa" => "/moa compare designs".to_string(),
                "steer" => "/steer focus on safety".to_string(),
                _ => format!("/{name}"),
            };
            let out = reg.execute_raw(&input, &mut host);
            assert!(out.handled, "/{name} was not handled");
            assert!(!out.is_error, "/{name} returned error: {}", out.response_text);
        }
    }

    #[test]
    fn test_autocomplete_and_telegram_bot_mention_parsing() {
        let reg = SlashCommandRegistry::new();
        let mem_matches = reg.autocomplete("/me");
        assert!(mem_matches.iter().any(|d| d.name == "memory"));

        let parsed = SlashCommandParser::parse("/help@my_hermes_bot memory").unwrap();
        assert_eq!(parsed.command, "help");
        assert_eq!(parsed.bot_mention.as_deref(), Some("my_hermes_bot"));
        assert_eq!(parsed.args, "memory");

        assert!(SlashCommandParser::parse("/Users/alice/project/src/main.rs").is_none());
    }
}
