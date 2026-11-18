//! Hermes Agent Native Pure-Rust Runtime.
//!
//! Unifies SQLite FTS5 state, bounded MEMORY.md/USER.md/SOUL.md & Honcho dialectics,
//! progressive SKILL.md management, 3-tier cache-friendly prompt building, 28 toolsets
//! (58 built-in tools + MCP), security/TIR/approval gating, file checkpoints, multi-provider
//! LLM routing (ChatCompletions, CodexResponses, AnthropicMessages, LocalHermes),
//! `/moa`, all 61 slash commands, trajectory export/redaction, cron/heartbeat, and ACP.

pub mod commands;
pub mod context;
pub mod memory;
pub mod providers;
pub mod security;
pub mod skills;
pub mod state_db;
pub mod tools_agentic;
pub mod tools_file;
pub mod tools_terminal;
pub mod tools_web_media;
pub mod toolsets;
pub mod trajectory;

use crate::computer::{ComputerCloudProvider, ComputerLeaseBroker};
use crate::mcp::McpRegistry;
use crate::talk::TalkCoordinator;
use crate::tasks::CronRoutine;
use chrono::Utc;
use commands::{
    CommandBranchInfo, CommandDescriptor, CommandExecutionOutcome, CommandHost, CommandModelInfo,
    CommandProviderInfo, CommandSkillInfo, CommandToolsetInfo, CommandUiEffect, InlineActionButton,
    SlashCommandParser, SlashCommandRegistry,
};
use context::{
    AssembledPrompt, CompressionConfig, ContextCompressor, ContextMessage, ContextRefResolver,
    DiscoveredWorkspaceContext, PromptBuildInput, PromptBuilder, VolatileRuntimeContext,
    WorkspaceContextDiscovery, estimate_tokens,
};
use gpui_telegram_core::error::{Result, TelegramError};
use memory::{HonchoDialectics, MemoryManager, MemoryTarget, ObservationCategory, SoulProfile};
use providers::{
    ApiMode, CacheControl, MoaCoordinator, MoaModelSpec, ProviderChatMessage, ProviderMessageRole,
    ProviderRequest, ProviderRouter, ProviderStreamEvent, ProviderToolCall, ProviderToolSpec,
    SystemPromptTier, TokenUsage,
};
use security::{ApprovalDecisionAction, ApprovalMode, PendingApproval, SecurityEngine};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use skills::{
    CreateSkillRequest, SkillLearningTurn, SkillOrigin, SkillsManager,
};
use state_db::{
    AppendMessageRequest, ApprovalStatus as DbApprovalStatus, CreateApprovalRequest,
    CreateCronJobRequest, CreateKanbanCardRequest, CreateSessionRequest, CronJobPatch,
    CronJobRecord, HermesStateStore, KanbanCardPatch, MessageRecord, RecordCronRunRequest,
    SessionListFilter, SessionPatch, SessionRecord,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Instant;
use tokio::sync::mpsc;
use tools_agentic::LocalAgenticStore;
use tools_file::{FileCheckpointManager, RollbackReport, TodoStore};
use tools_terminal::{ProcessRegistry, TerminalBackendConfig};
use toolsets::{
    AgenticStateBackend, BoxFuture, HermesInlineButton, HermesStateBridge, HermesToolCall,
    HermesToolContext, HermesToolExecutor, HermesToolResult, HermesToolSpec, HermesUiAction,
    PlatformImBridge, SubagentDelegationOutcome, SubagentDelegationRequest, ToolsetId,
    ToolsetPreset, ToolsetRegistry,
};
use trajectory::{
    ExportedTrajectory, TrajectoryCompressionConfig, TrajectoryExporter, TrajectoryFormat,
    TrajectoryMessage, TrajectorySession, TrajectoryToolCall,
};

// ============================================================================
// Engine Configuration & Turn DTOs
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HermesEngineConfig {
    pub workspace_root: PathBuf,
    pub data_dir: Option<PathBuf>,
    pub default_model: String,
    pub max_iterations: usize,
    pub default_preset: ToolsetPreset,
    pub default_approval_mode: ApprovalMode,
    pub context_compression: CompressionConfig,
    pub enable_prompt_caching: bool,
    pub default_max_tokens: Option<u32>,
    pub default_temperature: Option<f32>,
    pub terminal_backend: TerminalBackendConfig,
}

impl Default for HermesEngineConfig {
    fn default() -> Self {
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            workspace_root,
            data_dir: None,
            default_model: "hermes-local-v1".to_string(),
            max_iterations: 30,
            default_preset: ToolsetPreset::Full,
            default_approval_mode: ApprovalMode::Prompt,
            context_compression: CompressionConfig::default(),
            enable_prompt_caching: true,
            default_max_tokens: None,
            default_temperature: None,
            terminal_backend: TerminalBackendConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HermesTurnRequest {
    pub session_id: Option<String>,
    pub chat_id: Option<i64>,
    pub bot_id: Option<String>,
    #[serde(default)]
    pub bot_name: Option<String>,
    pub user_id: Option<String>,
    pub source: String,
    pub prompt: String,
    pub model: Option<String>,
    /// Optional persona / bot instructions override for this turn (persisted into session settings
    /// when non-empty). Preserves Hermes Tier-1 operational doctrine, active toolsets, and the
    /// progressive skills index rather than stripping tool/multi-step execution rules.
    pub system_prompt: Option<String>,
    pub thinking_budget_tokens: Option<usize>,
    pub preset: Option<ToolsetPreset>,
    pub max_iterations: Option<usize>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub skip_slash_commands: bool,
    #[serde(default)]
    pub caller_depth: usize,
    #[serde(default)]
    pub caller_allowed_chat_ids: Option<HashSet<i64>>,
    #[serde(default)]
    pub caller_is_admin: bool,
    #[serde(default)]
    pub caller_owns_bot: Option<bool>,
}

impl HermesTurnRequest {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            source: "direct".to_string(),
            prompt: prompt.into(),
            ..Default::default()
        }
    }

    pub fn with_chat_bot(mut self, chat_id: i64, bot_id: impl Into<String>) -> Self {
        self.chat_id = Some(chat_id);
        self.bot_id = Some(bot_id.into());
        self
    }

    pub fn with_bot_name(mut self, bot_name: impl Into<String>) -> Self {
        self.bot_name = Some(bot_name.into());
        self
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HermesEvent {
    Meta {
        session_id: String,
        turn_id: String,
        model: String,
        provider: String,
        api_mode: String,
        tier1_hash: String,
    },
    TextDelta {
        text: String,
        accumulated: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolStart {
        tool_call_id: String,
        tool_name: String,
        arguments: Value,
    },
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        success: bool,
        output: String,
        duration_ms: u64,
        #[serde(default)]
        inline_buttons: Vec<HermesInlineButton>,
        #[serde(default)]
        ui_actions: Vec<HermesUiAction>,
    },
    ApprovalRequired {
        approval_id: String,
        session_id: String,
        tool_name: String,
        command: String,
        risk_level: String,
        reason: String,
        #[serde(default)]
        buttons: Vec<HermesInlineButton>,
    },
    InlineButtons {
        rows: Vec<Vec<InlineActionButton>>,
    },
    CommandResult {
        outcome: CommandExecutionOutcome,
    },
    GoalProgress {
        iteration: usize,
        max_iterations: usize,
        status: String,
        note: String,
    },
    Done {
        session_id: String,
        turn_id: String,
        response_text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        iterations: usize,
        tool_calls_count: usize,
        usage: TokenUsage,
        #[serde(default)]
        buttons: Vec<Vec<InlineActionButton>>,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedToolStep {
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub success: bool,
    pub output: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HermesTurnResult {
    pub session_id: String,
    pub turn_id: String,
    pub model: String,
    pub provider: String,
    pub api_mode: String,
    pub response_text: String,
    pub reasoning: Option<String>,
    pub iterations: usize,
    pub tool_calls: Vec<RecordedToolStep>,
    pub buttons: Vec<Vec<InlineActionButton>>,
    pub ui_effects: Vec<CommandUiEffect>,
    pub usage: TokenUsage,
    pub events: Vec<HermesEvent>,
}

// ============================================================================
// Runtime Settings, Heartbeat & Message Queue Managers
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionRuntimeSettings {
    pub voice_mode: bool,
    pub verbose_mode: bool,
    pub reasoning_mode: bool,
    pub yolo_mode: bool,
    pub debug_mode: bool,
    pub browser_attached: bool,
    pub active_goal: Option<String>,
    /// Optional custom persona / bot instructions override. Customizes the session `SoulProfile`
    /// while preserving Hermes base operational directives and Tier-1 toolset/self-learning doctrine.
    pub system_prompt_override: Option<String>,
    pub thinking_budget_tokens: Option<usize>,
    pub config_overrides: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub last_tick_ms: Option<i64>,
    pub target_chat_id: Option<i64>,
    pub prompt: String,
    pub ticks_executed: u64,
    pub suppressed_ok_count: u64,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 300,
            last_tick_ms: None,
            target_chat_id: None,
            prompt: "Review active workspace tasks, cron jobs, and approvals. Respond HEARTBEAT_OK if nothing needs attention.".to_string(),
            ticks_executed: 0,
            suppressed_ok_count: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatTickOutcome {
    pub executed_at_ms: i64,
    pub suppressed: bool,
    pub response_text: String,
    pub target_chat_id: Option<i64>,
}

#[derive(Clone, Default)]
pub struct HeartbeatManager {
    inner: Arc<RwLock<HeartbeatConfig>>,
}

impl HeartbeatManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self) -> HeartbeatConfig {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn enable(&self, interval_secs: Option<u64>) -> HeartbeatConfig {
        let mut cfg = self.inner.write().unwrap_or_else(|e| e.into_inner());
        cfg.enabled = true;
        if let Some(secs) = interval_secs {
            cfg.interval_secs = secs.max(5);
        }
        cfg.clone()
    }

    pub fn disable(&self) -> HeartbeatConfig {
        let mut cfg = self.inner.write().unwrap_or_else(|e| e.into_inner());
        cfg.enabled = false;
        cfg.clone()
    }

    pub fn set_config(&self, next: HeartbeatConfig) {
        let mut cfg = self.inner.write().unwrap_or_else(|e| e.into_inner());
        *cfg = next;
    }

    pub fn is_due(&self, now_ms: i64) -> bool {
        let cfg = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if !cfg.enabled {
            return false;
        }
        match cfg.last_tick_ms {
            None => true,
            Some(last) => now_ms.saturating_sub(last) >= (cfg.interval_secs as i64) * 1000,
        }
    }

    pub fn record_tick(&self, now_ms: i64, suppressed: bool) {
        let mut cfg = self.inner.write().unwrap_or_else(|e| e.into_inner());
        cfg.last_tick_ms = Some(now_ms);
        cfg.ticks_executed += 1;
        if suppressed {
            cfg.suppressed_ok_count += 1;
        }
    }

    pub fn is_heartbeat_ok(text: &str) -> bool {
        let trimmed = text.trim();
        trimmed.eq_ignore_ascii_case("HEARTBEAT_OK")
            || trimmed.starts_with("HEARTBEAT_OK")
            || trimmed.ends_with("HEARTBEAT_OK")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedMessage {
    pub id: String,
    pub session_id: String,
    pub text: String,
    pub queued_at_ms: i64,
}

#[derive(Default)]
struct MessageQueueState {
    queues: HashMap<String, VecDeque<QueuedMessage>>,
    steers: HashMap<String, VecDeque<String>>,
    active_cancellations: HashMap<String, Arc<AtomicBool>>,
}

#[derive(Clone, Default)]
pub struct MessageQueueManager {
    inner: Arc<Mutex<MessageQueueState>>,
}

impl MessageQueueManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&self, session_id: &str, text: impl Into<String>) -> QueuedMessage {
        let item = QueuedMessage {
            id: format!("q_{}", uuid::Uuid::new_v4().simple()),
            session_id: session_id.to_string(),
            text: text.into(),
            queued_at_ms: Utc::now().timestamp_millis(),
        };
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state
            .queues
            .entry(session_id.to_string())
            .or_default()
            .push_back(item.clone());
        item
    }

    pub fn pop_next(&self, session_id: &str) -> Option<QueuedMessage> {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.queues.get_mut(session_id).and_then(|q| q.pop_front())
    }

    pub fn list(&self, session_id: &str) -> Vec<QueuedMessage> {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state
            .queues
            .get(session_id)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn clear(&self, session_id: &str) -> usize {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state
            .queues
            .remove(session_id)
            .map(|q| q.len())
            .unwrap_or(0)
    }

    pub fn push_steer(&self, session_id: &str, instruction: impl Into<String>) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state
            .steers
            .entry(session_id.to_string())
            .or_default()
            .push_back(instruction.into());
    }

    pub fn take_steers(&self, session_id: &str) -> Vec<String> {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state
            .steers
            .remove(session_id)
            .map(|q| q.into_iter().collect())
            .unwrap_or_default()
    }

    pub fn register_active_turn(&self, session_id: &str, cancel: Arc<AtomicBool>) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state
            .active_cancellations
            .insert(session_id.to_string(), cancel);
    }

    pub fn clear_active_turn(&self, session_id: &str) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.active_cancellations.remove(session_id);
    }

    pub fn cancel_session(&self, session_id: &str) -> bool {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(flag) = state.active_cancellations.get(session_id) {
            flag.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn is_active(&self, session_id: &str) -> bool {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.active_cancellations.contains_key(session_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronRunOutcome {
    pub job_id: String,
    pub job_name: String,
    pub status: String,
    pub response_text: String,
    pub target_chat_id: Option<i64>,
    pub next_run_at: Option<i64>,
    pub duration_ms: i64,
}

// ============================================================================
// ToolContextBridge: Bridges HermesToolContext <-> HermesStateStore/Memory/Skills
// ============================================================================

struct ToolContextBridge {
    engine: Weak<HermesEngineInner>,
    store: HermesStateStore,
    memory: MemoryManager,
    skills: SkillsManager,
    honcho: HonchoDialectics,
    default_user_id: String,
    chat_id: Option<i64>,
    ui_actions: Arc<Mutex<Vec<HermesUiAction>>>,
    agentic_backend: Option<Arc<dyn AgenticStateBackend>>,
    scope_key: String,
    cron_scope_key: String,
    caller_allowed_chat_ids: Option<HashSet<i64>>,
    caller_is_admin: bool,
    can_manage_bot: bool,
}

impl ToolContextBridge {
    fn allows_chat(&self, chat_id: i64) -> bool {
        if self.caller_is_admin {
            return true;
        }
        match &self.caller_allowed_chat_ids {
            Some(allowed) => allowed.contains(&chat_id),
            None => true,
        }
    }

    fn is_restricted_bot_caller(&self) -> bool {
        !self.can_manage_bot && !self.caller_is_admin
    }

    fn parse_bot_id(args: &Value) -> Option<String> {
        args.get("bot_id").and_then(|v| {
            v.as_str()
                .map(ToString::to_string)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })
    }

    fn resolve_cron_next_run(schedule: &str, tz: Option<&str>) -> Result<Option<i64>> {
        let mut routine = CronRoutine::new("tmp".to_string(), schedule.to_string())
            .with_time_zone(tz.map(ToString::to_string));
        if !routine.validate_expression() {
            return Err(TelegramError::Validation(format!(
                "Invalid cron or @every schedule expression: '{schedule}'"
            )));
        }
        Ok(routine.compute_next_run(Utc::now()).map(|dt| dt.timestamp_millis()))
    }
}

impl HermesStateBridge for ToolContextBridge {
    fn memory_op<'a>(&'a self, args: &'a Value) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            if self.is_restricted_bot_caller() {
                return Err("Forbidden: only bot owner or server admin can access or modify bot memory".to_string());
            }
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("read");
            let target_raw = args.get("target").and_then(|v| v.as_str()).unwrap_or("memory");
            let target = MemoryTarget::parse(target_raw)
                .ok_or_else(|| format!("Invalid memory target '{target_raw}'"))?;
            let content = args
                .get("content")
                .or_else(|| args.get("entry"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let old_text = args
                .get("old_text")
                .or_else(|| args.get("match"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            match action {
                "read" | "view" => {
                    let res = self.memory.read(target).map_err(|e| e.to_string())?;
                    serde_json::to_value(res).map_err(|e| e.to_string())
                }
                "add" | "append" => {
                    let res = self.memory.add(target, content).map_err(|e| e.to_string())?;
                    serde_json::to_value(res).map_err(|e| e.to_string())
                }
                "replace" => {
                    let res = self
                        .memory
                        .replace(target, old_text, content)
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(res).map_err(|e| e.to_string())
                }
                "remove" | "delete" | "rm" => {
                    let needle = if old_text.is_empty() { content } else { old_text };
                    let res = self.memory.remove(target, needle).map_err(|e| e.to_string())?;
                    serde_json::to_value(res).map_err(|e| e.to_string())
                }
                "clear" => {
                    let res = self.memory.clear(target).map_err(|e| e.to_string())?;
                    serde_json::to_value(res).map_err(|e| e.to_string())
                }
                other => Err(format!("Unsupported memory action '{other}'")),
            }
        })
    }

    fn skill_op<'a>(&'a self, args: &'a Value) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            if self.is_restricted_bot_caller() {
                return Err("Forbidden: only bot owner or server admin can access or modify bot skills".to_string());
            }
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");
            match action {
                "list" => {
                    let filter = args.get("filter").and_then(|v| v.as_str());
                    let list = self.skills.skills_list(filter);
                    let items: Vec<Value> = list
                        .into_iter()
                        .map(|d| {
                            json!({
                                "name": d.frontmatter.name,
                                "description": d.frontmatter.description,
                                "tags": d.frontmatter.tags,
                                "origin": d.frontmatter.origin.as_str(),
                                "trusted": d.trusted,
                            })
                        })
                        .collect();
                    Ok(json!({ "skills": items }))
                }
                "view" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'name'".to_string())?;
                    let doc = self.skills.skill_view(name).map_err(|e| e.to_string())?;
                    serde_json::to_value(doc).map_err(|e| e.to_string())
                }
                "create" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'name'".to_string())?
                        .to_string();
                    let body = args.get("content").or_else(|| args.get("body")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let tags = args.get("tags").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter().filter_map(|x| x.as_str().map(ToString::to_string)).collect()
                    });
                    let toolsets = args.get("toolsets").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter().filter_map(|x| x.as_str().map(ToString::to_string)).collect()
                    });
                    let doc = self
                        .skills
                        .skill_create(CreateSkillRequest {
                            name,
                            description,
                            body,
                            platforms: None,
                            toolsets,
                            tags,
                            version: None,
                            origin: Some(SkillOrigin::User),
                            trusted: Some(true),
                        })
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(doc).map_err(|e| e.to_string())
                }
                "patch" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'name'".to_string())?;
                    let old_text = args.get("old_text").and_then(|v| v.as_str()).unwrap_or("");
                    let new_text = args
                        .get("new_text")
                        .or_else(|| args.get("content"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let doc = self
                        .skills
                        .skill_patch(name, old_text, new_text)
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(doc).map_err(|e| e.to_string())
                }
                "delete" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'name'".to_string())?;
                    let deleted = self.skills.skill_delete(name).map_err(|e| e.to_string())?;
                    Ok(json!({ "name": name, "deleted": deleted }))
                }
                other => Err(format!("Unsupported skill action '{other}'")),
            }
        })
    }

    fn session_search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            let clamped = limit.clamp(1, 50);
            let fetch_limit = clamped.saturating_mul(4).max(50);
            let raw_hits = self
                .store
                .session_search(query, None, fetch_limit)
                .map_err(|e| e.to_string())?;
            let mut hits = Vec::new();
            for hit in raw_hits {
                let Ok(Some(sess)) = self.store.get_session(&hit.session_id) else {
                    continue;
                };
                let allowed = if let Some(cid) = self.chat_id {
                    sess.chat_id == Some(cid)
                } else if self.default_user_id != "default_user" {
                    let owner_match = serde_json::from_str::<Value>(&sess.metadata_json)
                        .ok()
                        .and_then(|m| {
                            m.get("owner_user_id").and_then(|v| {
                                v.as_str()
                                    .map(|s| s == self.default_user_id)
                                    .or_else(|| {
                                        v.as_i64()
                                            .map(|n| n.to_string() == self.default_user_id)
                                    })
                            })
                        })
                        .unwrap_or(false);
                    sess.chat_id.is_none() && owner_match
                } else {
                    let has_owner = serde_json::from_str::<Value>(&sess.metadata_json)
                        .ok()
                        .and_then(|m| m.get("owner_user_id").cloned())
                        .is_some_and(|v| !v.is_null());
                    sess.chat_id.is_none() && !has_owner
                };
                if allowed {
                    hits.push(hit);
                    if hits.len() >= clamped {
                        break;
                    }
                }
            }
            Ok(json!({
                "query": query,
                "count": hits.len(),
                "hits": hits,
            }))
        })
    }

    fn honcho_op<'a>(&'a self, args: &'a Value) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("profile");
            let req_user_id = args
                .get("user_id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(uid) = req_user_id {
                if self.default_user_id != "default_user" && uid != self.default_user_id {
                    let numeric_matches = parse_numeric_user_id(Some(uid))
                        .is_some_and(|n| n.to_string() == self.default_user_id);
                    if !numeric_matches {
                        return Err("Cross-user Honcho access denied".to_string());
                    }
                }
            }
            let user_id = if self.default_user_id != "default_user" {
                &self.default_user_id
            } else {
                req_user_id.unwrap_or(&self.default_user_id)
            };
            match action {
                "record_observation" => {
                    let obs = args
                        .get("observation")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'observation'".to_string())?;
                    let cat_raw = args.get("category").and_then(|v| v.as_str()).unwrap_or("fact");
                    let cat = ObservationCategory::parse(cat_raw);
                    let conf = args
                        .get("confidence")
                        .and_then(|v| v.as_f64())
                        .map(|f| f as f32)
                        .unwrap_or(0.85);
                    let rec = self
                        .honcho
                        .record_observation(user_id, cat, obs, conf, None)
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(rec).map_err(|e| e.to_string())
                }
                "query" => {
                    let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                    let res = self.honcho.query(user_id, q).map_err(|e| e.to_string())?;
                    serde_json::to_value(res).map_err(|e| e.to_string())
                }
                "profile" => {
                    let prof = self.honcho.profile(user_id).map_err(|e| e.to_string())?;
                    serde_json::to_value(prof).map_err(|e| e.to_string())
                }
                other => Err(format!("Unsupported honcho action '{other}'")),
            }
        })
    }

    fn cron_op<'a>(&'a self, args: &'a Value) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("list")
                .to_ascii_lowercase();
            let validate_target_cid = |key: &str| -> std::result::Result<Option<i64>, String> {
                if let Some(val) = args.get(key).filter(|v| !v.is_null()) {
                    let cid = val
                        .as_i64()
                        .ok_or_else(|| format!("Invalid integer '{key}'"))?;
                    if !self.allows_chat(cid) {
                        return Err(format!(
                            "Forbidden: caller is not an active member of {key} ({cid})"
                        ));
                    }
                    Ok(Some(cid))
                } else {
                    Ok(None)
                }
            };
            let explicit_target_cid = validate_target_cid("target_chat_id")?;
            let _ = validate_target_cid("chat_id")?;
            if let Some(cid) = self.chat_id {
                if !self.allows_chat(cid) {
                    return Err("Forbidden: caller is not an active member of chat".to_string());
                }
            }

            if let Some(backend) = &self.agentic_backend {
                let mut effective_args = args.clone();
                if action == "create" && explicit_target_cid.is_none() {
                    if let (Some(cid), Some(obj)) = (self.chat_id, effective_args.as_object_mut()) {
                        obj.insert("target_chat_id".to_string(), json!(cid));
                    }
                }
                if matches!(
                    action.as_str(),
                    "get" | "preview" | "history" | "pause" | "resume" | "delete" | "run_now"
                ) && self.caller_allowed_chat_ids.is_some()
                    && !self.caller_is_admin
                {
                    if let Some(id) = args.get("id").and_then(Value::as_str) {
                        if let Ok(job) = backend
                            .cron_op(&self.cron_scope_key, &json!({ "action": "get", "id": id }))
                            .await
                        {
                            if let Some(tcid) = job.get("target_chat_id").and_then(Value::as_i64) {
                                if !self.allows_chat(tcid) {
                                    return Err("Forbidden: caller is not an active member of cron job's target chat".to_string());
                                }
                            }
                        }
                    }
                }
                let mut out = backend
                    .cron_op(&self.cron_scope_key, &effective_args)
                    .await?;
                if action == "list"
                    && self.caller_allowed_chat_ids.is_some()
                    && !self.caller_is_admin
                {
                    if let Some(jobs) = out.get_mut("jobs").and_then(Value::as_array_mut) {
                        jobs.retain(|j| {
                            j.get("target_chat_id")
                                .and_then(Value::as_i64)
                                .is_none_or(|cid| self.allows_chat(cid))
                        });
                    }
                }
                return Ok(out);
            }

            let check_store_job_access = |job: &CronJobRecord| -> std::result::Result<(), String> {
                if let Some(cid) = job.target_chat_id.or(job.origin_chat_id) {
                    if !self.allows_chat(cid) {
                        return Err("Forbidden: caller is not an active member of cron job's chat".to_string());
                    }
                } else if self.caller_allowed_chat_ids.is_some() && !self.caller_is_admin {
                    let owner = self
                        .store
                        .kv_get("cron_job_owner", &job.id)
                        .ok()
                        .flatten();
                    if owner.as_deref() != Some(self.default_user_id.as_str()) {
                        return Err("Forbidden: caller does not own this cron job".to_string());
                    }
                }
                Ok(())
            };

            match action.as_str() {
                "list" => {
                    let mut jobs = self.store.list_cron_jobs(false).map_err(|e| e.to_string())?;
                    if self.caller_allowed_chat_ids.is_some() && !self.caller_is_admin {
                        jobs.retain(|j| check_store_job_access(j).is_ok());
                    }
                    Ok(json!({ "jobs": jobs }))
                }
                "create" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Hermes Scheduled Task")
                        .to_string();
                    let schedule = args
                        .get("schedule")
                        .and_then(|v| v.as_str())
                        .unwrap_or("@every 1h")
                        .to_string();
                    let timezone = args
                        .get("timezone")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);
                    let next_run_at = Self::resolve_cron_next_run(&schedule, timezone.as_deref())
                        .map_err(|e| e.to_string())?;
                    let requested_id = args.get("id").and_then(|v| v.as_str()).map(ToString::to_string);
                    if let Some(ref existing_id) = requested_id {
                        if let Some(existing) = self.store.get_cron_job(existing_id).map_err(|e| e.to_string())? {
                            check_store_job_access(&existing)?;
                        }
                    }
                    let job = self
                        .store
                        .create_cron_job(CreateCronJobRequest {
                            id: requested_id,
                            name,
                            schedule,
                            timezone,
                            target_chat_id: explicit_target_cid.or(self.chat_id),
                            prompt: args.get("prompt").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            model: args.get("model").and_then(|v| v.as_str()).map(ToString::to_string),
                            skills: None,
                            enabled: Some(true),
                            deliver_to: Self::parse_bot_id(args),
                            origin_chat_id: self.chat_id,
                            next_run_at,
                        })
                        .map_err(|e| e.to_string())?;
                    if self.default_user_id != "default_user" {
                        let _ = self.store.kv_set("cron_job_owner", &job.id, &self.default_user_id);
                    }
                    serde_json::to_value(job).map_err(|e| e.to_string())
                }
                "get" | "preview" | "history" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'id'".to_string())?;
                    let jobs = self.store.list_cron_jobs(false).map_err(|e| e.to_string())?;
                    let job = jobs
                        .into_iter()
                        .find(|j| j.id == id)
                        .ok_or_else(|| format!("Cron job '{id}' not found"))?;
                    check_store_job_access(&job)?;
                    if action == "history" {
                        let history = self
                            .store
                            .list_cron_history(id, 20)
                            .map_err(|e| e.to_string())?;
                        return Ok(json!({ "id": id, "history": history }));
                    }
                    if action == "preview" {
                        let mut routine = CronRoutine::new(job.id.clone(), job.schedule.clone())
                            .with_time_zone(job.timezone.clone());
                        let summary = routine.preview_next_7_days(Utc::now());
                        return Ok(json!({
                            "id": job.id,
                            "schedule": job.schedule,
                            "scheduled_fires_next_7_days": summary.scheduled_fires_next_7_days,
                            "fires_on_weekend": summary.fires_on_weekend,
                            "fires_overnight": summary.fires_overnight,
                        }));
                    }
                    serde_json::to_value(job).map_err(|e| e.to_string())
                }
                "pause" | "resume" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'id'".to_string())?;
                    if let Some(job) = self.store.get_cron_job(id).map_err(|e| e.to_string())? {
                        check_store_job_access(&job)?;
                    }
                    let enabled = action == "resume";
                    let updated = self
                        .store
                        .update_cron_job(
                            id,
                            CronJobPatch {
                                enabled: Some(enabled),
                                ..Default::default()
                            },
                        )
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(updated).map_err(|e| e.to_string())
                }
                "delete" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'id'".to_string())?;
                    if let Some(job) = self.store.get_cron_job(id).map_err(|e| e.to_string())? {
                        check_store_job_access(&job)?;
                    }
                    let deleted = self.store.delete_cron_job(id).map_err(|e| e.to_string())?;
                    Ok(json!({ "id": id, "deleted": deleted }))
                }
                "run_now" => {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'id'".to_string())?;
                    if let Some(job) = self.store.get_cron_job(id).map_err(|e| e.to_string())? {
                        check_store_job_access(&job)?;
                    }
                    if let Some(inner) = self.engine.upgrade() {
                        let eng = HermesEngine { inner };
                        let outcome = eng.run_cron_job_now(id).await.map_err(|e| e.to_string())?;
                        return serde_json::to_value(outcome).map_err(|e| e.to_string());
                    }
                    let rec = self
                        .store
                        .record_cron_run(RecordCronRunRequest {
                            job_id: id.to_string(),
                            run_at: Some(Utc::now().timestamp_millis()),
                            status: "completed".to_string(),
                            output_preview: Some("Triggered cron job".to_string()),
                            error: None,
                            duration_ms: 0,
                            next_run_at: None,
                        })
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(rec).map_err(|e| e.to_string())
                }
                other => Err(format!("Unsupported cron action '{other}'")),
            }
        })
    }

    fn kanban_op<'a>(
        &'a self,
        action: &'a str,
        args: &'a Value,
    ) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            if let Some(backend) = &self.agentic_backend {
                return backend.kanban_op(&self.scope_key, action, args).await;
            }
            match action {
                "kanban_list_boards" => {
                    let boards = self.store.list_kanban_boards().map_err(|e| e.to_string())?;
                    Ok(json!({ "boards": boards }))
                }
                "kanban_create_board" => {
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'name'".to_string())?;
                    let desc = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
                    let cols = args.get("columns").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(ToString::to_string))
                            .collect()
                    });
                    let board = self
                        .store
                        .create_kanban_board(name, desc, cols)
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(board).map_err(|e| e.to_string())
                }
                "kanban_get_board" => {
                    let board_id = args
                        .get("board_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'board_id'".to_string())?;
                    let board = self
                        .store
                        .get_kanban_board(board_id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Board '{board_id}' not found"))?;
                    let cards = self
                        .store
                        .list_kanban_cards(board_id, None)
                        .map_err(|e| e.to_string())?;
                    Ok(json!({ "board": board, "cards": cards }))
                }
                "kanban_create_card" => {
                    let board_id = args
                        .get("board_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'board_id'".to_string())?
                        .to_string();
                    let title = args
                        .get("title")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'title'".to_string())?
                        .to_string();
                    let card = self
                        .store
                        .create_kanban_card(CreateKanbanCardRequest {
                            id: None,
                            board_id,
                            column_name: args
                                .get("column")
                                .or_else(|| args.get("column_name"))
                                .and_then(|v| v.as_str())
                                .map(ToString::to_string),
                            title,
                            description: args
                                .get("description")
                                .and_then(|v| v.as_str())
                                .map(ToString::to_string),
                            priority: args
                                .get("priority")
                                .and_then(|v| v.as_str())
                                .map(ToString::to_string),
                            assignee: args
                                .get("assignee")
                                .and_then(|v| v.as_str())
                                .map(ToString::to_string),
                            labels: args.get("labels").and_then(|v| v.as_array()).map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str().map(ToString::to_string))
                                    .collect()
                            }),
                        })
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(card).map_err(|e| e.to_string())
                }
                "kanban_get_card" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let card = self
                        .store
                        .get_kanban_card(card_id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Card '{card_id}' not found"))?;
                    serde_json::to_value(card).map_err(|e| e.to_string())
                }
                "kanban_update_card" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let card = self
                        .store
                        .update_kanban_card(
                            card_id,
                            KanbanCardPatch {
                                title: args.get("title").and_then(|v| v.as_str()).map(ToString::to_string),
                                description: args.get("description").and_then(|v| v.as_str()).map(ToString::to_string),
                                priority: args.get("priority").and_then(|v| v.as_str()).map(ToString::to_string),
                                labels: args.get("labels").and_then(|v| v.as_array()).map(|a| {
                                    a.iter().filter_map(|x| x.as_str().map(ToString::to_string)).collect()
                                }),
                                ..Default::default()
                            },
                        )
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(card).map_err(|e| e.to_string())
                }
                "kanban_move_card" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let col = args
                        .get("column")
                        .or_else(|| args.get("target_column"))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'column'".to_string())?;
                    let pos = args.get("position").and_then(|v| v.as_i64());
                    let card = self
                        .store
                        .move_kanban_card(card_id, col, pos)
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(card).map_err(|e| e.to_string())
                }
                "kanban_delete_card" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let deleted = self.store.delete_kanban_card(card_id).map_err(|e| e.to_string())?;
                    Ok(json!({ "card_id": card_id, "deleted": deleted }))
                }
                "kanban_assign_card" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let assignee = args
                        .get("assignee")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);
                    let card = self
                        .store
                        .update_kanban_card(
                            card_id,
                            KanbanCardPatch {
                                assignee: Some(assignee),
                                ..Default::default()
                            },
                        )
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(card).map_err(|e| e.to_string())
                }
                "kanban_add_comment" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let body = args
                        .get("body")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'body'".to_string())?;
                    let author = args.get("author").and_then(|v| v.as_str()).unwrap_or("hermes");
                    let card = self
                        .store
                        .add_kanban_comment(card_id, author, body)
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(card).map_err(|e| e.to_string())
                }
                "kanban_list_comments" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let card = self
                        .store
                        .get_kanban_card(card_id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Card '{card_id}' not found"))?;
                    Ok(json!({ "card_id": card_id, "comments": card.comments }))
                }
                "kanban_add_checklist" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let items: Vec<String> = args
                        .get("items")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(ToString::to_string))
                                .collect()
                        })
                        .filter(|v: &Vec<String>| !v.is_empty())
                        .unwrap_or_else(|| {
                            args.get("title")
                                .and_then(|v| v.as_str())
                                .map(|t| vec![t.to_string()])
                                .unwrap_or_default()
                        });
                    let mut latest = None;
                    for item in items {
                        latest = Some(
                            self.store
                                .add_kanban_checklist_item(card_id, &item)
                                .map_err(|e| e.to_string())?,
                        );
                    }
                    let card = match latest {
                        Some(c) => c,
                        None => self
                            .store
                            .get_kanban_card(card_id)
                            .map_err(|e| e.to_string())?
                            .ok_or_else(|| format!("Card '{card_id}' not found"))?,
                    };
                    serde_json::to_value(card).map_err(|e| e.to_string())
                }
                "kanban_toggle_checklist" => {
                    let card_id = args
                        .get("card_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing required 'card_id'".to_string())?;
                    let card = self
                        .store
                        .get_kanban_card(card_id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Card '{card_id}' not found"))?;
                    let item_id = if let Some(id) = args.get("item_id").and_then(|v| v.as_str()) {
                        id.to_string()
                    } else {
                        let idx = args
                            .get("item_index")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize;
                        card.checklist
                            .get(idx)
                            .map(|i| i.id.clone())
                            .ok_or_else(|| format!("Checklist item index {idx} out of range"))?
                    };
                    let updated = self
                        .store
                        .toggle_kanban_checklist_item(card_id, &item_id)
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(updated).map_err(|e| e.to_string())
                }
                "kanban_search_cards" => {
                    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                    let board_id = args.get("board_id").and_then(|v| v.as_str());
                    let cards = self
                        .store
                        .search_kanban_cards(query, board_id)
                        .map_err(|e| e.to_string())?;
                    Ok(json!({ "count": cards.len(), "cards": cards }))
                }
                other => Err(format!("Unsupported Kanban tool '{other}'")),
            }
        })
    }

    fn delegate_subagent<'a>(
        &'a self,
        req: SubagentDelegationRequest,
    ) -> BoxFuture<'a, std::result::Result<SubagentDelegationOutcome, String>> {
        Box::pin(async move {
            if req.depth > req.max_depth {
                return Err(format!(
                    "Subagent depth {} exceeds max_depth {}",
                    req.depth, req.max_depth
                ));
            }
            let started = Instant::now();
            let Some(inner) = self.engine.upgrade() else {
                return Err("HermesEngine dropped before subagent execution".to_string());
            };
            let engine = HermesEngine { inner };
            let child_session_id = format!("sub_{}_{}", req.parent_session_id, req.task_id);
            let role_prompt = format!(
                "You are a focused Hermes subagent in role '{}'. Complete the assigned task cleanly and concisely.\nAllowed toolsets: {:?}",
                req.role, req.toolsets
            );
            let child_req = HermesTurnRequest {
                session_id: Some(child_session_id.clone()),
                chat_id: self.chat_id,
                bot_id: None,
                bot_name: None,
                user_id: Some(self.default_user_id.clone()),
                source: "subagent".to_string(),
                prompt: req.prompt.clone(),
                model: req.model.clone(),
                system_prompt: Some(role_prompt),
                thinking_budget_tokens: None,
                preset: None,
                max_iterations: Some(6),
                max_tokens: None,
                temperature: None,
                skip_slash_commands: true,
                caller_depth: req.depth,
                ..Default::default()
            };

            // Spawn on tokio to break mutual recursion of async state machines while enabling parallel subtasks.
            let handle = tokio::spawn(async move {
                let mut reg = if req.toolsets.is_empty() {
                    ToolsetRegistry::from_preset(ToolsetPreset::Safe)
                } else {
                    ToolsetRegistry::from_toolsets(req.toolsets.into_iter())
                };
                // Never allow unbounded recursion if subagents shouldn't nest past max_depth.
                if req.depth >= req.max_depth {
                    let _ = reg.disable(ToolsetId::Subagents);
                }
                engine.set_session_toolset_registry(&child_session_id, reg);
                engine.run_turn(child_req).await
            });

            match handle.await {
                Ok(Ok(res)) => Ok(SubagentDelegationOutcome {
                    task_id: req.task_id,
                    role: req.role,
                    success: true,
                    summary: res.response_text,
                    tool_calls_used: res.tool_calls.len(),
                    duration_ms: started.elapsed().as_millis() as u64,
                }),
                Ok(Err(e)) => Err(e.to_string()),
                Err(join_err) => Err(format!("Subagent task panicked or cancelled: {join_err}")),
            }
        })
    }

    fn emit_ui_action<'a>(
        &'a self,
        action: HermesUiAction,
    ) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            if let Ok(mut guard) = self.ui_actions.lock() {
                guard.push(action.clone());
            }
            if let Some(backend) = &self.agentic_backend {
                if let Ok(val) = backend.desktop_ui_act(&self.scope_key, action.clone()).await {
                    return Ok(val);
                }
            }
            Ok(json!({ "ok": true, "action_type": action.action_type }))
        })
    }

    fn record_checkpoint<'a>(
        &'a self,
        session_id: &'a str,
        turn_index: u64,
        file_path: &'a str,
        original_content: Option<&'a [u8]>,
    ) -> BoxFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            if let Some(backend) = &self.agentic_backend {
                let _ = backend
                    .record_checkpoint(session_id, turn_index, file_path, original_content)
                    .await;
            }
            let content_str = original_content.map(|b| String::from_utf8_lossy(b).into_owned());
            self.store
                .save_checkpoint(
                    session_id,
                    turn_index as i64,
                    file_path,
                    content_str.as_deref(),
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }

    fn record_approval<'a>(
        &'a self,
        approval: PendingApproval,
    ) -> BoxFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            if let Some(backend) = &self.agentic_backend {
                let _ = backend.record_approval(approval.clone()).await;
            }
            self.store
                .enqueue_approval(CreateApprovalRequest {
                    id: Some(approval.id),
                    session_id: approval.session_id,
                    command: approval.command,
                    risk_level: approval.risk_level,
                    pattern_name: approval.rule_id.unwrap_or(approval.tool_name),
                })
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }

    fn connections_op<'a>(&'a self, args: &'a Value) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            if let Some(backend) = &self.agentic_backend {
                backend.connections_op(&self.scope_key, args).await
            } else {
                Err("connections_op not bridged".to_string())
            }
        })
    }

    fn desktop_ui_inspect<'a>(&'a self) -> BoxFuture<'a, std::result::Result<Value, String>> {
        Box::pin(async move {
            if let Some(backend) = &self.agentic_backend {
                backend.desktop_ui_inspect(&self.scope_key).await
            } else {
                Err("desktop_ui_inspect not bridged".to_string())
            }
        })
    }

    fn record_lineage<'a>(
        &'a self,
        task_id: &'a str,
        parent_session_id: Option<&'a str>,
        role: &'a str,
        status: &'a str,
    ) -> BoxFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            if let Some(backend) = &self.agentic_backend {
                backend
                    .record_lineage(&self.scope_key, task_id, parent_session_id, role, status)
                    .await
            } else {
                Ok(())
            }
        })
    }
}

// ============================================================================
// Caller Scope & CommandHost Adapter bound to (HermesEngine, session_id)
// ============================================================================

#[derive(Debug, Clone)]
pub struct CommandCallerScope {
    pub user_id: String,
    pub allowed_chat_ids: HashSet<i64>,
    pub is_admin: bool,
    pub bot_config_allowed: bool,
}

impl CommandCallerScope {
    pub fn allows_session(&self, sess: &SessionRecord) -> bool {
        if self.is_admin {
            return true;
        }
        if let Some(cid) = sess.chat_id {
            return self.allowed_chat_ids.contains(&cid);
        }
        serde_json::from_str::<Value>(&sess.metadata_json)
            .ok()
            .and_then(|m| {
                m.get("owner_user_id").and_then(|v| {
                    v.as_str()
                        .map(|s| s == self.user_id)
                        .or_else(|| v.as_i64().map(|n| n.to_string() == self.user_id))
                })
            })
            .unwrap_or(false)
    }

    pub fn allows_chat(&self, chat_id: i64) -> bool {
        self.is_admin || self.allowed_chat_ids.contains(&chat_id)
    }

    pub fn allows_bot_config(&self) -> bool {
        self.is_admin || self.bot_config_allowed
    }
}

pub type AcpCallerScope = CommandCallerScope;

struct EngineCommandHost<'a> {
    engine: &'a HermesEngine,
    session_id: String,
    chat_id: Option<i64>,
    bot_id: Option<String>,
    caller_scope: Option<CommandCallerScope>,
}

impl<'a> EngineCommandHost<'a> {
    fn ensure_bot_config_allowed(&self) -> std::result::Result<(), String> {
        let is_bot_engine = self.engine.bound_agent_id().is_some() || self.bot_id.is_some();
        if is_bot_engine {
            if let Some(ref sc) = self.caller_scope {
                if !sc.allows_bot_config() {
                    return Err("Forbidden: only bot owner or server admin can access or modify bot configuration".into());
                }
            }
        }
        Ok(())
    }

    fn scope_key(&self) -> Option<String> {
        self.chat_id
            .map(|cid| scope_key_for(cid, self.bot_id.as_deref().unwrap_or("default")))
    }

    fn update_active_session_pointer(&mut self, new_session_id: &str) {
        self.session_id = new_session_id.to_string();
        if let Some(key) = self.scope_key() {
            let val = json!({ "session_id": new_session_id }).to_string();
            let _ = self
                .engine
                .store()
                .kv_set("active_chat_session", &key, &val);
        }
    }
}

impl CommandHost for EngineCommandHost<'_> {
    fn active_session_id(&self) -> String {
        self.session_id.clone()
    }

    fn new_session(&mut self, title: Option<&str>) -> std::result::Result<String, String> {
        let _ = self.engine.store().end_session(&self.session_id);
        let (_, model, api_mode) = self.engine.providers().active_info();
        let metadata = self
            .caller_scope
            .as_ref()
            .map(|sc| json!({ "owner_user_id": sc.user_id }));
        let rec = self
            .engine
            .store()
            .create_session(CreateSessionRequest {
                id: None,
                chat_id: self.chat_id,
                bot_id: self.bot_id.clone(),
                source: Some("hermes".into()),
                model: Some(model),
                api_mode: Some(api_mode.as_str().to_string()),
                title: title.map(ToString::to_string),
                metadata,
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        self.update_active_session_pointer(&rec.id);
        Ok(rec.id)
    }

    fn clear_session_messages(&mut self) -> std::result::Result<usize, String> {
        let msgs = self
            .engine
            .store()
            .get_messages(&self.session_id, None)
            .map_err(|e| e.to_string())?;
        let count = msgs.len();
        self.engine
            .store()
            .replace_session_messages(&self.session_id, Vec::new())
            .map_err(|e| e.to_string())?;
        Ok(count)
    }

    fn branch_session(&mut self, name: Option<&str>) -> std::result::Result<String, String> {
        let branched = self
            .engine
            .store()
            .branch_session(&self.session_id, None, name.map(ToString::to_string))
            .map_err(|e| e.to_string())?;
        self.update_active_session_pointer(&branched.id);
        Ok(branched.id)
    }

    fn list_branches(&self) -> std::result::Result<Vec<CommandBranchInfo>, String> {
        let list = self
            .engine
            .store()
            .list_branches(&self.session_id)
            .map_err(|e| e.to_string())?;
        Ok(list
            .into_iter()
            .map(|s| CommandBranchInfo {
                session_id: s.id.clone(),
                title: s.title.unwrap_or_else(|| "(untitled)".into()),
                parent_session_id: s.parent_session_id,
                message_count: s.message_count.max(0) as usize,
                active: s.id == self.session_id,
            })
            .collect())
    }

    fn checkout_branch(&mut self, target: &str) -> std::result::Result<String, String> {
        if let Some(sess) = self
            .engine
            .store()
            .get_session(target)
            .map_err(|e| e.to_string())?
        {
            if let Some(ref sc) = self.caller_scope {
                if !sc.allows_session(&sess) {
                    return Err("Forbidden: cannot checkout session not owned by caller".into());
                }
            }
            self.update_active_session_pointer(&sess.id);
            return Ok(sess.id);
        }
        let branches = self
            .engine
            .store()
            .list_branches(&self.session_id)
            .map_err(|e| e.to_string())?;
        if let Some(found) = branches.into_iter().find(|b| {
            b.id == target
                || b.title
                    .as_deref()
                    .is_some_and(|t| t.eq_ignore_ascii_case(target))
        }) {
            if let Some(ref sc) = self.caller_scope {
                if !sc.allows_session(&found) {
                    return Err("Forbidden: cannot checkout session not owned by caller".into());
                }
            }
            self.update_active_session_pointer(&found.id);
            return Ok(found.id);
        }
        Err(format!("Branch or session '{target}' not found"))
    }

    fn undo_last_turn(&mut self) -> std::result::Result<String, String> {
        let res = self
            .engine
            .store()
            .undo_last_turn(&self.session_id)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Removed {} message(s) from session `{}` (remaining: {}, seq: {}).",
            res.removed_count, res.session_id, res.remaining_message_count, res.rolled_back_to_seq
        ))
    }

    fn rollback_last_checkpoint(&mut self) -> std::result::Result<String, String> {
        let report = self
            .engine
            .rollback_turn(&self.session_id, None)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Rolled back turn {} in `{}`: {} file(s) restored, {} created file(s) removed.",
            report.turn_index,
            report.session_id,
            report.restored_files.len(),
            report.deleted_files.len()
        ))
    }

    fn diff_workspace(&self) -> std::result::Result<String, String> {
        let out = std::process::Command::new("git")
            .args(["status", "-sb"])
            .current_dir(&self.engine.inner.config.workspace_root)
            .output()
            .map_err(|e| format!("Failed to inspect workspace git status: {e}"))?;
        let status_txt = String::from_utf8_lossy(&out.stdout).to_string();
        let diff_out = std::process::Command::new("git")
            .args(["diff", "--stat"])
            .current_dir(&self.engine.inner.config.workspace_root)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        Ok(format!("{}\n{}", status_txt.trim(), diff_out.trim())
            .trim()
            .to_string())
    }

    fn compact_context(&mut self, focus: Option<&str>) -> std::result::Result<String, String> {
        let msgs = self
            .engine
            .store()
            .get_messages(&self.session_id, None)
            .map_err(|e| e.to_string())?;
        let ctx_msgs: Vec<ContextMessage> = msgs
            .iter()
            .map(|m| ContextMessage {
                role: m.role.clone(),
                content: m.content.clone(),
                tool_call_id: m.tool_call_id.clone(),
                tool_name: m.tool_name.clone(),
                pinned: false,
            })
            .collect();
        let compressor = ContextCompressor::new(self.engine.inner.config.context_compression.clone());
        let outcome = compressor.compress(&ctx_msgs, true);
        if !outcome.compressed {
            return Ok(format!(
                "Session `{}` has {} messages ({} est. tokens); already compact.",
                self.session_id,
                msgs.len(),
                outcome.original_tokens
            ));
        }

        let summary_prefix = focus
            .filter(|f| !f.trim().is_empty())
            .map(|f| format!("[Focus: {}]\n", f.trim()))
            .unwrap_or_default();

        let replacement: Vec<AppendMessageRequest> = outcome
            .messages
            .into_iter()
            .enumerate()
            .map(|(i, m)| {
                let content = if i == 0 && !summary_prefix.is_empty() && m.role == "system" {
                    format!("{summary_prefix}{}", m.content)
                } else {
                    m.content
                };
                AppendMessageRequest {
                    id: None,
                    session_id: self.session_id.clone(),
                    role: m.role,
                    content: content.clone(),
                    reasoning: None,
                    tool_calls_json: None,
                    tool_call_id: m.tool_call_id,
                    tool_name: m.tool_name,
                    token_count: estimate_tokens(&content) as i64,
                    created_at: None,
                }
            })
            .collect();

        self.engine
            .store()
            .replace_session_messages(&self.session_id, replacement)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Compacted session `{}`: {} -> {} tokens (summarized {} middle messages).",
            self.session_id,
            outcome.original_tokens,
            outcome.compressed_tokens,
            outcome.summarized_message_count
        ))
    }

    fn current_model_info(&self) -> CommandModelInfo {
        let (provider_id, model, mode) = self.engine.providers().active_info();
        CommandModelInfo {
            provider_id,
            model,
            api_mode: mode.as_str().to_string(),
        }
    }

    fn set_model(&mut self, model: &str) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        let (prov, resolved_model) = self.engine.providers().set_active_model(model);
        let _ = self.engine.store().update_session(
            &self.session_id,
            SessionPatch {
                model: Some(resolved_model.clone()),
                ..Default::default()
            },
        );
        self.engine.persist_providers_state();
        Ok(format!("{prov}/{resolved_model}"))
    }

    fn list_providers(&self) -> Vec<CommandProviderInfo> {
        let (active_id, _, _) = self.engine.providers().active_info();
        self.engine
            .providers()
            .list_providers()
            .into_iter()
            .map(|p| CommandProviderInfo {
                id: p.id.clone(),
                name: p.name,
                api_mode: p.api_mode.as_str().to_string(),
                default_model: p.default_model,
                active: p.id == active_id,
                enabled: p.enabled,
            })
            .collect()
    }

    fn set_provider(&mut self, provider_id: &str) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        let (prov, model) = self
            .engine
            .providers()
            .set_active(provider_id, None)
            .map_err(|e| e.to_string())?;
        let (_, _, mode) = self.engine.providers().active_info();
        let _ = self.engine.store().update_session(
            &self.session_id,
            SessionPatch {
                model: Some(model.clone()),
                api_mode: Some(mode.as_str().to_string()),
                ..Default::default()
            },
        );
        self.engine.persist_providers_state();
        Ok(format!("Switched active provider to `{prov}` (model `{model}`, mode `{}`).", mode.as_str()))
    }

    fn set_api_mode(&mut self, mode: &str) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        let parsed = ApiMode::parse(mode)
            .ok_or_else(|| format!("Unknown API mode '{mode}'. Use chat_completions, codex_responses, anthropic_messages, gemini_generate_content, or local_hermes."))?;
        self.engine
            .providers()
            .set_api_mode_for_active(parsed)
            .map_err(|e| e.to_string())?;
        let _ = self.engine.store().update_session(
            &self.session_id,
            SessionPatch {
                api_mode: Some(parsed.as_str().to_string()),
                ..Default::default()
            },
        );
        self.engine.persist_providers_state();
        Ok(parsed.as_str().to_string())
    }

    fn list_toolsets(&self) -> Vec<CommandToolsetInfo> {
        let reg_arc = self.engine.session_toolset_registry(&self.session_id);
        let guard = reg_arc.read().unwrap_or_else(|e| e.into_inner());
        let specs = toolsets::all_builtin_tool_specs();
        ToolsetId::ALL
            .iter()
            .map(|id| {
                let tool_count = specs.iter().filter(|s| s.toolset == *id).count();
                CommandToolsetInfo {
                    id: id.as_str().to_string(),
                    enabled: guard.is_toolset_enabled(*id),
                    tool_count,
                    description: id.description().to_string(),
                }
            })
            .collect()
    }

    fn mutate_toolsets(
        &mut self,
        action: &str,
        arg: Option<&str>,
    ) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        let reg_arc = self.engine.session_toolset_registry(&self.session_id);
        let mut reg = reg_arc.write().unwrap_or_else(|e| e.into_inner());
        let name = arg.unwrap_or("").trim();
        let msg = match action {
            "enable" | "add" => {
                if name.is_empty() {
                    return Err("Usage: `/toolsets enable <name|preset>`".into());
                }
                let enabled = reg.enable_by_name(name)?;
                format!("Enabled toolset(s): {}", enabled.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", "))
            }
            "disable" | "remove" => {
                if name.is_empty() {
                    return Err("Usage: `/toolsets disable <name>`".into());
                }
                let id = reg.disable_by_name(name)?;
                format!("Disabled toolset `{}`.", id.as_str())
            }
            "preset" => {
                let preset = ToolsetPreset::from_str(name)
                    .ok_or_else(|| format!("Unknown preset '{name}'. Try: full, safe, minimal, hermes-cli, hermes-telegram, hermes-acp"))?;
                reg.apply_preset(preset);
                format!("Applied toolset preset `{}` ({} active tools).", preset.as_str(), reg.active_specs().len())
            }
            "reset" => {
                reg.apply_preset(self.engine.inner.config.default_preset);
                format!("Reset toolsets to default preset `{}`.", self.engine.inner.config.default_preset.as_str())
            }
            other => {
                if let Some(preset) = ToolsetPreset::from_str(other) {
                    reg.apply_preset(preset);
                    format!("Applied toolset preset `{}`.", preset.as_str())
                } else if ToolsetId::from_str(other).is_some() {
                    let enabled = reg.enable_by_name(other)?;
                    format!("Enabled toolset `{}`.", enabled[0].as_str())
                } else {
                    return Err(format!("Unsupported toolsets action '{other}'"));
                }
            }
        };
        let active_names: Vec<String> = reg
            .enabled_toolsets
            .iter()
            .map(|t| t.as_str().to_string())
            .collect();
        drop(reg);
        let _ = self.engine.store().update_session(
            &self.session_id,
            SessionPatch {
                active_toolsets: Some(active_names),
                ..Default::default()
            },
        );
        Ok(msg)
    }

    fn list_skills(&self) -> Vec<CommandSkillInfo> {
        self.engine
            .skills()
            .skills_list(None)
            .into_iter()
            .map(|d| CommandSkillInfo {
                name: d.frontmatter.name,
                description: d.frontmatter.description,
                origin: d.frontmatter.origin.as_str().to_string(),
                enabled: true,
            })
            .collect()
    }

    fn view_skill(&self, name: &str) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        let doc = self
            .engine
            .skills()
            .skill_view(name)
            .map_err(|e| e.to_string())?;
        Ok(doc.raw_markdown)
    }

    fn learn_skill(&mut self, name: Option<&str>) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        let msgs = self
            .engine
            .store()
            .get_messages(&self.session_id, Some(40))
            .map_err(|e| e.to_string())?;
        if msgs.is_empty() {
            return Err("No messages in current session to distill a skill from.".into());
        }
        let turns: Vec<SkillLearningTurn> = msgs
            .into_iter()
            .map(|m| SkillLearningTurn {
                role: m.role,
                content: m.content,
                tool_name: m.tool_name,
            })
            .collect();
        let sess_title = self
            .engine
            .store()
            .get_session(&self.session_id)
            .ok()
            .flatten()
            .and_then(|s| s.title);
        let skill_name = name
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("learned-{}", &self.session_id[..self.session_id.len().min(12)]));
        let doc = self
            .engine
            .skills()
            .learn_from_session(
                &skill_name,
                sess_title.as_deref().unwrap_or(&skill_name),
                &turns,
                vec!["learned".into()],
            )
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Distilled session `{}` into SKILL.md `{}` ({} chars).",
            self.session_id,
            doc.frontmatter.name,
            doc.raw_markdown.len()
        ))
    }

    fn mcp_action(&mut self, action: &str, arg: Option<&str>) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        match action {
            "list" | "tools" => {
                let reg = self
                    .engine
                    .mcp_registry()
                    .read()
                    .map_err(|_| "MCP registry lock poisoned".to_string())?;
                let tools = reg.list_tools();
                if tools.is_empty() {
                    Ok("No MCP tools registered.".into())
                } else {
                    let lines: Vec<String> = tools
                        .into_iter()
                        .map(|t| format!("- `{}`: {}", t.name, t.description))
                        .collect();
                    Ok(format!("Registered MCP tools:\n{}", lines.join("\n")))
                }
            }
            "call" => {
                let raw = arg.ok_or_else(|| "Usage: `/mcp call <tool_name> [json_args]`".to_string())?;
                let (name, args_json) = raw
                    .split_once(' ')
                    .map(|(n, a)| (n.trim(), a.trim()))
                    .unwrap_or((raw.trim(), "{}"));
                let parsed: Value = serde_json::from_str(args_json)
                    .unwrap_or_else(|_| json!({ "input": args_json }));
                let reg = self
                    .engine
                    .mcp_registry()
                    .read()
                    .map_err(|_| "MCP registry lock poisoned".to_string())?;
                let out = reg.execute_tool(name, parsed)?;
                Ok(serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string()))
            }
            "remove" => {
                let name = arg.ok_or_else(|| "Usage: `/mcp remove <tool_name>`".to_string())?;
                let mut reg = self
                    .engine
                    .mcp_registry()
                    .write()
                    .map_err(|_| "MCP registry lock poisoned".to_string())?;
                if reg.remove(name.trim()).is_some() {
                    Ok(format!("Removed MCP tool `{}`.", name.trim()))
                } else {
                    Err(format!("MCP tool `{}` not found.", name.trim()))
                }
            }
            other => Err(format!("Unsupported MCP action '{other}'")),
        }
    }

    fn memory_action(
        &mut self,
        target: &str,
        action: &str,
        args: &[String],
    ) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        let mem_target = MemoryTarget::parse(target)
            .ok_or_else(|| format!("Invalid memory target '{target}'"))?;
        match action {
            "view" => {
                let res = self.engine.memory().read(mem_target).map_err(|e| e.to_string())?;
                let body = if res.entries.is_empty() {
                    "(empty)".to_string()
                } else {
                    res.entries
                        .iter()
                        .enumerate()
                        .map(|(i, e)| format!("{}. {}", i + 1, e))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                Ok(format!(
                    "{} ({}/{} chars, {:.0}%):\n{}",
                    mem_target.file_name(),
                    res.char_count,
                    res.char_limit,
                    res.usage_percent,
                    body
                ))
            }
            "add" => {
                let text = args.join(" ");
                if text.trim().is_empty() {
                    return Err("Usage: `/memory add <entry>`".into());
                }
                let res = self
                    .engine
                    .memory()
                    .add(mem_target, text.trim())
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "Saved to {} ({}/{} chars). Note: session prompt snapshot stays frozen until `/reload-memory`.",
                    mem_target.file_name(),
                    res.char_count,
                    res.char_limit
                ))
            }
            "remove" => {
                let substr = args.join(" ");
                let res = self
                    .engine
                    .memory()
                    .remove(mem_target, substr.trim())
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "Updated {} (changed={}, {}/{} chars).",
                    mem_target.file_name(),
                    res.changed,
                    res.char_count,
                    res.char_limit
                ))
            }
            "replace" => {
                if args.len() < 2 {
                    return Err("Usage: `/memory replace <old_substr> <new_entry>`".into());
                }
                let old_match = &args[0];
                let new_entry = args[1..].join(" ");
                let res = self
                    .engine
                    .memory()
                    .replace(mem_target, old_match, &new_entry)
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "Replaced entry in {} ({}/{} chars).",
                    mem_target.file_name(),
                    res.char_count,
                    res.char_limit
                ))
            }
            "clear" => {
                self.engine
                    .memory()
                    .clear(mem_target)
                    .map_err(|e| e.to_string())?;
                Ok(format!("Cleared {}.", mem_target.file_name()))
            }
            other => Err(format!("Unsupported memory action '{other}'")),
        }
    }

    fn soul_action(&mut self, action: &str, text: Option<&str>) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        match action {
            "view" => {
                let soul = self.engine.memory().read_soul().map_err(|e| e.to_string())?;
                Ok(soul.to_markdown())
            }
            "set" => {
                let raw = text.unwrap_or("").trim();
                if raw.is_empty() {
                    return Err("Usage: `/soul set <persona text or markdown>`".into());
                }
                let profile = if raw.starts_with('#') || raw.contains("\n## ") {
                    self.engine
                        .memory()
                        .write_soul_raw(raw)
                        .map_err(|e| e.to_string())?
                } else {
                    let mut current = self.engine.memory().read_soul().map_err(|e| e.to_string())?;
                    current.persona = raw.to_string();
                    self.engine
                        .memory()
                        .write_soul(&current)
                        .map_err(|e| e.to_string())?;
                    current
                };
                Ok(format!("Updated SOUL.md identity `{}`:\n{}", profile.name, profile.persona))
            }
            other => Err(format!("Unsupported soul action '{other}'")),
        }
    }

    fn reload_memory(&mut self) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        let snap = self
            .engine
            .memory()
            .reload_session_snapshot(&self.session_id)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Reloaded frozen memory snapshot for `{}`: {} MEMORY.md entries, {} USER.md entries.",
            self.session_id,
            snap.memory_entries.len(),
            snap.user_entries.len()
        ))
    }

    fn honcho_action(&mut self, action: &str, arg: Option<&str>) -> std::result::Result<String, String> {
        let user_id = self
            .chat_id
            .map(|c| format!("chat_{c}"))
            .or_else(|| {
                self.caller_scope
                    .as_ref()
                    .map(|sc| format!("user_{}", sc.user_id))
            })
            .unwrap_or_else(|| "default_user".into());
        match action {
            "profile" | "view" => {
                let prof = self
                    .engine
                    .honcho()
                    .profile(&user_id)
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "Honcho Profile ({}):\n{}\nObservations: {}",
                    prof.user_id,
                    prof.summary,
                    prof.observations.len()
                ))
            }
            "query" => {
                let q = arg.unwrap_or("");
                let res = self
                    .engine
                    .honcho()
                    .query(&user_id, q)
                    .map_err(|e| e.to_string())?;
                Ok(res.answer)
            }
            "observe" | "add" => {
                let obs = arg.ok_or_else(|| "Usage: `/honcho observe <note>`".to_string())?;
                let rec = self
                    .engine
                    .honcho()
                    .record_observation(
                        &user_id,
                        ObservationCategory::Fact,
                        obs,
                        0.85,
                        Some(&self.session_id),
                    )
                    .map_err(|e| e.to_string())?;
                Ok(format!("Recorded Honcho observation `{}`.", rec.id))
            }
            other => Err(format!("Unsupported honcho action '{other}'")),
        }
    }

    fn search_sessions(&self, query: &str) -> std::result::Result<String, String> {
        let fetch_limit = if self.caller_scope.is_some() { 60 } else { 15 };
        let mut hits = self
            .engine
            .store()
            .session_search(query, None, fetch_limit)
            .map_err(|e| e.to_string())?;
        if let Some(ref sc) = self.caller_scope {
            hits.retain(|h| {
                self.engine
                    .store()
                    .get_session(&h.session_id)
                    .ok()
                    .flatten()
                    .is_some_and(|s| sc.allows_session(&s))
            });
            hits.truncate(15);
        }
        if hits.is_empty() {
            return Ok(format!("No session messages matched '{query}'."));
        }
        let lines: Vec<String> = hits
            .into_iter()
            .map(|h| {
                format!(
                    "- `{}` #{} [{}]: {}",
                    h.session_id, h.seq, h.role, h.snippet
                )
            })
            .collect();
        Ok(format!("Session Search Results:\n{}", lines.join("\n")))
    }

    fn session_history(&self, limit: usize) -> std::result::Result<String, String> {
        let msgs = self
            .engine
            .store()
            .get_messages(&self.session_id, Some(limit.clamp(1, 200)))
            .map_err(|e| e.to_string())?;
        if msgs.is_empty() {
            return Ok(format!("Session `{}` has no messages yet.", self.session_id));
        }
        let lines: Vec<String> = msgs
            .into_iter()
            .map(|m| format!("#{} [{}] {}", m.seq, m.role, truncate_chars(&m.content, 180)))
            .collect();
        Ok(format!(
            "History for `{}` (last {}):\n{}",
            self.session_id,
            lines.len(),
            lines.join("\n")
        ))
    }

    fn list_sessions(&self, limit: usize) -> std::result::Result<String, String> {
        let clamped = limit.clamp(1, 100);
        let mut sessions = self
            .engine
            .store()
            .list_sessions(SessionListFilter {
                limit: if self.caller_scope.is_some() {
                    None
                } else {
                    Some(clamped)
                },
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        if let Some(ref sc) = self.caller_scope {
            sessions.retain(|s| sc.allows_session(s));
            sessions.truncate(clamped);
        }
        if sessions.is_empty() {
            return Ok("No sessions recorded.".into());
        }
        let lines: Vec<String> = sessions
            .into_iter()
            .map(|s| {
                let marker = if s.id == self.session_id { "*" } else { " " };
                format!(
                    "{marker} `{}` | model={} | msgs={} | ${:.4} | title={}",
                    s.id,
                    s.model,
                    s.message_count,
                    s.total_cost_usd,
                    s.title.as_deref().unwrap_or("(untitled)")
                )
            })
            .collect();
        Ok(format!("Sessions ({}):\n{}", lines.len(), lines.join("\n")))
    }

    fn export_session(
        &self,
        format: &str,
        save_path: Option<&str>,
    ) -> std::result::Result<(String, String, String), String> {
        let fmt = TrajectoryFormat::parse(format)
            .ok_or_else(|| format!("Unsupported export format '{format}'. Use sharegpt, jsonl, anthropic, or native."))?;
        let exported = self
            .engine
            .export_session(&self.session_id, fmt, true, None)
            .map_err(|e| e.to_string())?;
        let filename = format!("{}.{}", self.session_id, fmt.file_extension());
        if let Some(raw_path) = save_path {
            if let Some(ref sc) = self.caller_scope {
                if !sc.is_admin {
                    return Err(
                        "File path save is disabled over remote command API; omit path to export inline"
                            .into(),
                    );
                }
            }
            let p = Path::new(raw_path);
            let target = if p.is_absolute() {
                p.to_path_buf()
            } else {
                self.engine.inner.config.workspace_root.join(p)
            };
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Create export dir failed: {e}"))?;
            }
            std::fs::write(&target, &exported.payload)
                .map_err(|e| format!("Write export failed: {e}"))?;
        }
        Ok((filename, format.to_string(), exported.payload))
    }

    fn usage_summary(&self, kind: &str) -> std::result::Result<String, String> {
        if kind == "insights" {
            let ins = self
                .engine
                .store()
                .insights(Some(30), self.bot_id.as_deref())
                .map_err(|e| e.to_string())?;
            let top_models = ins
                .top_models
                .iter()
                .map(|m| format!("{} ({} msgs, ${:.4})", m.model, m.message_count, m.total_cost_usd))
                .collect::<Vec<_>>()
                .join(", ");
            let top_tools = ins
                .top_tools
                .iter()
                .map(|t| format!("{} x{}", t.tool_name, t.call_count))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(format!(
                "Hermes 30d Insights:\nSessions: {} (active {})\nMessages: {} (avg {:.1}/sess)\nTokens: {}\nCost: ${:.4}\nTop models: {}\nTop tools: {}",
                ins.session_count,
                ins.active_session_count,
                ins.message_count,
                ins.avg_messages_per_session,
                ins.total_tokens,
                ins.total_cost_usd,
                if top_models.is_empty() { "none" } else { &top_models },
                if top_tools.is_empty() { "none" } else { &top_tools },
            ));
        }

        let summary = self
            .engine
            .store()
            .usage_summary(Some(&self.session_id), None)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Session Usage (`{}`):\nMessages: {}\nInput tokens: {}\nOutput tokens: {}\nTotal tokens: {}\nEstimated cost: ${:.4}",
            self.session_id,
            summary.total_messages,
            summary.total_input_tokens,
            summary.total_output_tokens,
            summary.total_tokens,
            summary.total_cost_usd
        ))
    }

    fn status_report(&self) -> std::result::Result<String, String> {
        let (provider_id, model, api_mode) = self.engine.providers().active_info();
        let sess = self
            .engine
            .store()
            .get_session(&self.session_id)
            .ok()
            .flatten();
        let settings = self.engine.session_settings(&self.session_id);
        let approval_mode = self
            .engine
            .security()
            .read()
            .map(|s| s.approvals.get_session_mode(&self.session_id))
            .unwrap_or(ApprovalMode::Prompt);
        let reg_arc = self.engine.session_toolset_registry(&self.session_id);
        let active_tools = reg_arc
            .read()
            .map(|r| r.active_specs().len())
            .unwrap_or(0);
        let hb = self.engine.heartbeat().status();
        let queue_len = self.engine.queues().list(&self.session_id).len();

        Ok(format!(
            "Hermes Status:\n- Session: `{}` (msgs={})\n- Provider/Model: `{}` / `{}` (api_mode=`{}`)\n- Workspace: {}\n- Active Tools: {}\n- Approval Mode: `{}`\n- Modes: voice={}, verbose={}, reasoning={}, debug={}, yolo={}\n- Heartbeat: enabled={} (every {}s) | Queued messages: {}",
            self.session_id,
            sess.map(|s| s.message_count).unwrap_or(0),
            provider_id,
            model,
            api_mode.as_str(),
            self.engine.inner.config.workspace_root.display(),
            active_tools,
            approval_mode.as_str(),
            settings.voice_mode,
            settings.verbose_mode,
            settings.reasoning_mode,
            settings.debug_mode,
            settings.yolo_mode || approval_mode == ApprovalMode::Yolo,
            hb.enabled,
            hb.interval_secs,
            queue_len
        ))
    }

    fn doctor_report(&self) -> std::result::Result<String, String> {
        let ws = &self.engine.inner.config.workspace_root;
        let fts5 = self.engine.store().fts5_available();
        let jmode = self
            .engine
            .store()
            .journal_mode()
            .unwrap_or_else(|_| "unknown".into());
        let providers = self.engine.providers().list_providers();
        let now_ms = Utc::now().timestamp_millis();
        let prov_summary: Vec<String> = providers
            .iter()
            .map(|p| {
                format!(
                    "{} ({}, creds={}, active={})",
                    p.id,
                    p.api_mode.as_str(),
                    p.credentials.len(),
                    p.credentials.active_count(now_ms)
                )
            })
            .collect();
        let skills_count = self.engine.skills().skills_list_index().len();
        let mcp_count = self
            .engine
            .mcp_registry()
            .read()
            .map(|m| m.list_tools().len())
            .unwrap_or(0);
        Ok(format!(
            "Hermes Doctor Diagnostics:\n- Workspace exists: {} ({})\n- SQLite FTS5: {} | journal_mode: {}\n- Providers ({}): {}\n- Skills indexed: {} | MCP tools: {}\n- Hardcoded legacy remote gateway: NONE (pure-Rust runtime)",
            ws.exists(),
            ws.display(),
            fts5,
            jmode,
            providers.len(),
            prov_summary.join(", "),
            skills_count,
            mcp_count
        ))
    }

    fn context_report(&self) -> std::result::Result<String, String> {
        let assembled = self.engine.assemble_prompt_for_session(
            &self.session_id,
            self.chat_id,
            self.bot_id.as_deref(),
            self.engine.session_settings(&self.session_id).system_prompt_override.as_deref(),
            &[],
        );
        let msgs = self
            .engine
            .store()
            .get_messages(&self.session_id, None)
            .map_err(|e| e.to_string())?;
        let history_tokens: usize = msgs
            .iter()
            .map(|m| estimate_tokens(&m.content) + 4)
            .sum();
        Ok(format!(
            "Context Budget (`{}`):\n- Tier 1 (Stable Identity + Tools + Skills Index): {} est. tokens\n- Tier 2 (Frozen MEMORY.md/USER.md + Workspace + Honcho): {} est. tokens\n- Tier 3 (Volatile Runtime): {} est. tokens\n- Tier 1+2 Hash: {}\n- History ({} msgs): {} est. tokens\n- Total est. prompt tokens: {}",
            self.session_id,
            assembled.tier1_stable.estimated_tokens,
            assembled.tier2_context.estimated_tokens,
            assembled.tier3_volatile.estimated_tokens,
            &assembled.tier1_hash[..12.min(assembled.tier1_hash.len())],
            msgs.len(),
            history_tokens,
            assembled.estimated_tokens + history_tokens
        ))
    }

    fn cron_action(&mut self, action: &str, args: &[String]) -> std::result::Result<String, String> {
        let check_cmd_job_access = |job: &CronJobRecord| -> std::result::Result<(), String> {
            let Some(ref sc) = self.caller_scope else {
                return Ok(());
            };
            if sc.is_admin {
                return Ok(());
            }
            if let Some(cid) = job.target_chat_id.or(job.origin_chat_id) {
                if !sc.allows_chat(cid) {
                    return Err("Forbidden: caller is not a member of cron job's chat".into());
                }
            } else {
                let owner = self
                    .engine
                    .store()
                    .kv_get("cron_job_owner", &job.id)
                    .ok()
                    .flatten();
                if owner.as_deref() != Some(sc.user_id.as_str()) {
                    return Err("Forbidden: caller does not own this cron job".into());
                }
            }
            Ok(())
        };
        match action {
            "list" => {
                let mut jobs = self
                    .engine
                    .store()
                    .list_cron_jobs(false)
                    .map_err(|e| e.to_string())?;
                if self.caller_scope.is_some() {
                    jobs.retain(|j| check_cmd_job_access(j).is_ok());
                }
                if jobs.is_empty() {
                    return Ok("No cron jobs configured.".into());
                }
                let lines: Vec<String> = jobs
                    .into_iter()
                    .map(|j| {
                        format!(
                            "- `{}` [{}] sched=`{}` name=\"{}\"",
                            j.id,
                            if j.enabled { "ON" } else { "PAUSED" },
                            j.schedule,
                            j.name
                        )
                    })
                    .collect();
                Ok(format!("Cron Jobs:\n{}", lines.join("\n")))
            }
            "add" | "create" => {
                let name = args
                    .first()
                    .ok_or_else(|| "Usage: `/cron add <name> <schedule> <prompt>`".to_string())?
                    .to_string();
                let schedule = args
                    .get(1)
                    .ok_or_else(|| "Usage: `/cron add <name> <schedule> <prompt>`".to_string())?
                    .to_string();
                let prompt = if args.len() > 2 {
                    args[2..].join(" ")
                } else {
                    name.clone()
                };
                let next_run_at = ToolContextBridge::resolve_cron_next_run(&schedule, None)
                    .map_err(|e| e.to_string())?;
                let job = self
                    .engine
                    .store()
                    .create_cron_job(CreateCronJobRequest {
                        id: None,
                        name,
                        schedule,
                        timezone: None,
                        target_chat_id: self.chat_id,
                        prompt,
                        model: None,
                        skills: None,
                        enabled: Some(true),
                        deliver_to: self.bot_id.clone(),
                        origin_chat_id: self.chat_id,
                        next_run_at,
                    })
                    .map_err(|e| e.to_string())?;
                if let Some(ref sc) = self.caller_scope {
                    let _ = self
                        .engine
                        .store()
                        .kv_set("cron_job_owner", &job.id, &sc.user_id);
                }
                Ok(format!("Created cron job `{}` (`{}`).", job.id, job.name))
            }
            "pause" | "resume" | "enable" | "disable" => {
                let id = args
                    .first()
                    .ok_or_else(|| format!("Usage: `/cron {action} <job_id>`"))?;
                if self.caller_scope.is_some() {
                    let job = self
                        .engine
                        .store()
                        .get_cron_job(id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Cron job `{id}` not found."))?;
                    check_cmd_job_access(&job)?;
                }
                let enabled = matches!(action, "resume" | "enable");
                self.engine
                    .store()
                    .update_cron_job(
                        id,
                        CronJobPatch {
                            enabled: Some(enabled),
                            ..Default::default()
                        },
                    )
                    .map_err(|e| e.to_string())?;
                Ok(format!("Cron job `{id}` enabled={enabled}."))
            }
            "delete" | "rm" => {
                let id = args
                    .first()
                    .ok_or_else(|| "Usage: `/cron delete <job_id>`".to_string())?;
                if self.caller_scope.is_some() {
                    let job = self
                        .engine
                        .store()
                        .get_cron_job(id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Cron job `{id}` not found."))?;
                    check_cmd_job_access(&job)?;
                }
                let deleted = self
                    .engine
                    .store()
                    .delete_cron_job(id)
                    .map_err(|e| e.to_string())?;
                if deleted {
                    Ok(format!("Deleted cron job `{id}`."))
                } else {
                    Err(format!("Cron job `{id}` not found."))
                }
            }
            "history" => {
                let id = args
                    .first()
                    .ok_or_else(|| "Usage: `/cron history <job_id>`".to_string())?;
                if self.caller_scope.is_some() {
                    let job = self
                        .engine
                        .store()
                        .get_cron_job(id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Cron job `{id}` not found."))?;
                    check_cmd_job_access(&job)?;
                }
                let recs = self
                    .engine
                    .store()
                    .list_cron_history(id, 15)
                    .map_err(|e| e.to_string())?;
                if recs.is_empty() {
                    return Ok(format!("No execution history for cron job `{id}`."));
                }
                let lines: Vec<String> = recs
                    .into_iter()
                    .map(|r| {
                        format!(
                            "- `{}` status={} ({}ms): {}",
                            r.id,
                            r.status,
                            r.duration_ms,
                            r.output_preview.as_deref().unwrap_or("")
                        )
                    })
                    .collect();
                Ok(format!("Cron History (`{id}`):\n{}", lines.join("\n")))
            }
            "run" => {
                let id = args
                    .first()
                    .ok_or_else(|| "Usage: `/cron run <job_id>`".to_string())?;
                if self.caller_scope.is_some() {
                    let job = self
                        .engine
                        .store()
                        .get_cron_job(id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Cron job `{id}` not found."))?;
                    check_cmd_job_access(&job)?;
                }
                Ok(format!("Queued immediate execution for cron job `{id}`."))
            }
            other => Err(format!("Unsupported cron action '{other}'")),
        }
    }

    fn kanban_action(&mut self, action: &str, args: &[String]) -> std::result::Result<String, String> {
        match action {
            "list" | "board" => {
                let boards = self
                    .engine
                    .store()
                    .list_kanban_boards()
                    .map_err(|e| e.to_string())?;
                if boards.is_empty() {
                    return Ok("No Kanban boards found. Create one with `/kanban create <name>`.".into());
                }
                let mut sections = Vec::new();
                for b in boards {
                    let cards = self
                        .engine
                        .store()
                        .list_kanban_cards(&b.id, None)
                        .unwrap_or_default();
                    sections.push(format!(
                        "- `{}` **{}** ({} cards, columns: {})",
                        b.id,
                        b.name,
                        cards.len(),
                        b.columns.join(" | ")
                    ));
                }
                Ok(format!("Kanban Boards:\n{}", sections.join("\n")))
            }
            "create" => {
                let name = if args.is_empty() {
                    "Hermes Board".to_string()
                } else {
                    args.join(" ")
                };
                let board = self
                    .engine
                    .store()
                    .create_kanban_board(&name, "", None)
                    .map_err(|e| e.to_string())?;
                Ok(format!("Created Kanban board `{}` (`{}`).", board.name, board.id))
            }
            "add-card" | "add" => {
                let board_id = args
                    .first()
                    .ok_or_else(|| "Usage: `/kanban add-card <board_id> <title>`".to_string())?
                    .to_string();
                let title = if args.len() > 1 {
                    args[1..].join(" ")
                } else {
                    return Err("Missing card title".into());
                };
                let card = self
                    .engine
                    .store()
                    .create_kanban_card(CreateKanbanCardRequest {
                        board_id,
                        title,
                        ..Default::default()
                    })
                    .map_err(|e| e.to_string())?;
                Ok(format!("Created Kanban card `{}` (`{}`) in `{}`.", card.title, card.id, card.column_name))
            }
            "move" => {
                if args.len() < 2 {
                    return Err("Usage: `/kanban move <card_id> <column>`".into());
                }
                let card_id = &args[0];
                let col = args[1..].join(" ");
                let card = self
                    .engine
                    .store()
                    .move_kanban_card(card_id, &col, None)
                    .map_err(|e| e.to_string())?;
                Ok(format!("Moved card `{}` to `{}`.", card.id, card.column_name))
            }
            other => Err(format!("Unsupported kanban action '{other}'")),
        }
    }

    fn heartbeat_action(
        &mut self,
        action: &str,
        interval_secs: Option<u64>,
    ) -> std::result::Result<String, String> {
        self.ensure_bot_config_allowed()?;
        match action {
            "on" | "enable" | "start" => {
                let cfg = self.engine.heartbeat().enable(interval_secs);
                Ok(format!(
                    "Heartbeat enabled (every {}s).",
                    cfg.interval_secs
                ))
            }
            "off" | "disable" | "stop" => {
                self.engine.heartbeat().disable();
                Ok("Heartbeat disabled.".into())
            }
            "status" => {
                let cfg = self.engine.heartbeat().status();
                Ok(format!(
                    "Heartbeat: enabled={} | interval={}s | ticks={} (suppressed OK={})",
                    cfg.enabled, cfg.interval_secs, cfg.ticks_executed, cfg.suppressed_ok_count
                ))
            }
            other => Err(format!("Unsupported heartbeat action '{other}'. Use on, off, or status.")),
        }
    }

    fn approval_action(
        &mut self,
        action: &str,
        target: Option<&str>,
        _reason: Option<&str>,
    ) -> std::result::Result<String, String> {
        match action {
            "yolo" => {
                let mut sec = self
                    .engine
                    .security()
                    .write()
                    .map_err(|_| "SecurityEngine lock poisoned".to_string())?;
                let current = sec.approvals.get_session_mode(&self.session_id);
                let enable = match target.map(|s| s.to_ascii_lowercase()).as_deref() {
                    Some("on") | Some("true") | Some("enable") => true,
                    Some("off") | Some("false") | Some("disable") => false,
                    _ => current != ApprovalMode::Yolo,
                };
                let next = if enable {
                    ApprovalMode::Yolo
                } else {
                    ApprovalMode::Prompt
                };
                sec.approvals.set_session_mode(self.session_id.clone(), next);
                drop(sec);
                self.engine.update_session_settings(&self.session_id, |s| s.yolo_mode = enable);
                Ok(format!("Session `{}` approval mode set to `{}`.", self.session_id, next.as_str()))
            }
            "permissions" => {
                if let Some(mode_raw) = target {
                    let mode = ApprovalMode::from_str(mode_raw)
                        .ok_or_else(|| format!("Unknown approval mode '{mode_raw}'. Try: prompt, smart_approve, yolo, ask_once, session_allow, permanent_allow, deny"))?;
                    let mut sec = self
                        .engine
                        .security()
                        .write()
                        .map_err(|_| "SecurityEngine lock poisoned".to_string())?;
                    sec.approvals.set_session_mode(self.session_id.clone(), mode);
                    return Ok(format!("Session approval mode set to `{}`.", mode.as_str()));
                }
                let sec = self
                    .engine
                    .security()
                    .read()
                    .map_err(|_| "SecurityEngine lock poisoned".to_string())?;
                let mode = sec.approvals.get_session_mode(&self.session_id);
                let pending = sec.approvals.pending.len();
                Ok(format!("Approval mode: `{}` | pending approvals: {pending}", mode.as_str()))
            }
            "approve" | "deny" => {
                let is_deny = action == "deny";
                let decision_action = if is_deny {
                    ApprovalDecisionAction::Deny
                } else if matches!(target, Some("always") | Some("permanent")) {
                    ApprovalDecisionAction::ApprovePermanent
                } else if matches!(target, Some("session")) {
                    ApprovalDecisionAction::ApproveSession
                } else {
                    ApprovalDecisionAction::ApproveOnce
                };
                let db_status = match decision_action {
                    ApprovalDecisionAction::Deny => DbApprovalStatus::Denied,
                    ApprovalDecisionAction::ApproveOnce => DbApprovalStatus::ApprovedOnce,
                    ApprovalDecisionAction::ApproveSession => DbApprovalStatus::ApprovedSession,
                    ApprovalDecisionAction::ApprovePermanent => DbApprovalStatus::ApprovedPermanent,
                    ApprovalDecisionAction::EnableYolo => DbApprovalStatus::ApprovedSession,
                };

                let mut resolved_id = None;
                let explicit_id = target
                    .filter(|t| !matches!(*t, "once" | "session" | "always" | "permanent"))
                    .map(ToString::to_string);
                if let Ok(mut sec) = self.engine.security().write() {
                    let id_opt = explicit_id.clone().or_else(|| {
                        sec.approvals
                            .pending
                            .iter()
                            .rev()
                            .find(|p| p.session_id == self.session_id)
                            .or_else(|| {
                                if self.caller_scope.is_none() {
                                    sec.approvals.pending.last()
                                } else {
                                    None
                                }
                            })
                            .map(|p| p.id.clone())
                    });
                    if let Some(id) = id_opt {
                        if let Some(ref sc) = self.caller_scope {
                            if !sc.is_admin {
                                if let Some(pending) = sec.approvals.pending.iter().find(|p| p.id == id) {
                                    if pending.session_id != self.session_id {
                                        return Err("Forbidden: approval does not belong to caller session".into());
                                    }
                                }
                            }
                        }
                        if let Ok(done) = sec.approvals.decide(&id, decision_action, "user") {
                            resolved_id = Some(done.id);
                        }
                    }
                }
                if resolved_id.is_none() {
                    let pending = self
                        .engine
                        .store()
                        .list_approvals(Some(&self.session_id), true)
                        .map_err(|e| e.to_string())?;
                    let target_id = explicit_id.or_else(|| pending.first().map(|p| p.id.clone()));
                    if let Some(id) = target_id {
                        if let Some(ref sc) = self.caller_scope {
                            if !sc.is_admin {
                                if let Some(existing) = self.engine.store().get_approval(&id).map_err(|e| e.to_string())? {
                                    if existing.session_id != self.session_id {
                                        return Err("Forbidden: approval does not belong to caller session".into());
                                    }
                                }
                            }
                        }
                        let rec = self
                            .engine
                            .store()
                            .decide_approval(&id, db_status, Some("user".into()))
                            .map_err(|e| e.to_string())?;
                        resolved_id = Some(rec.id);
                    }
                } else if let Some(ref id) = resolved_id {
                    let _ = self
                        .engine
                        .store()
                        .decide_approval(id, db_status, Some("user".into()));
                }

                match resolved_id {
                    Some(id) => Ok(format!("Approval `{id}` updated to `{}`.", db_status.as_str())),
                    None => Err("No pending approval requests for this session.".into()),
                }
            }
            other => Err(format!("Unsupported approval action '{other}'")),
        }
    }

    fn queue_action(&mut self, action: &str, text: Option<&str>) -> std::result::Result<String, String> {
        match action {
            "list" => {
                let items = self.engine.queues().list(&self.session_id);
                if items.is_empty() {
                    Ok("Message queue is empty.".into())
                } else {
                    let lines: Vec<String> = items
                        .into_iter()
                        .enumerate()
                        .map(|(i, q)| format!("{}. [{}] {}", i + 1, q.id, q.text))
                        .collect();
                    Ok(format!("Queued Messages:\n{}", lines.join("\n")))
                }
            }
            "queue" => {
                let msg = text
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "Usage: `/queue <follow-up message>`".to_string())?;
                if msg.eq_ignore_ascii_case("clear") {
                    let n = self.engine.queues().clear(&self.session_id);
                    return Ok(format!("Cleared {n} queued message(s)."));
                }
                let q = self.engine.queues().enqueue(&self.session_id, msg);
                Ok(format!("Queued message `{}` for session `{}`.", q.id, self.session_id))
            }
            "steer" => {
                let instr = text
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "Usage: `/steer <instruction>`".to_string())?;
                self.engine.queues().push_steer(&self.session_id, instr);
                Ok(format!("Injected steering instruction into session `{}`: \"{}\"", self.session_id, instr))
            }
            "interrupt" | "stop" => {
                let stopped = self.engine.cancel_session(&self.session_id);
                Ok(if stopped {
                    format!("Signalled active turn cancellation for `{}`.", self.session_id)
                } else {
                    format!("No active running turn for `{}` (cleared pending steers).", self.session_id)
                })
            }
            other => Err(format!("Unsupported queue action '{other}'")),
        }
    }

    fn set_toggle(&mut self, key: &str, value: Option<&str>) -> std::result::Result<String, String> {
        let parsed = value.and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "1" | "enable" => Some(true),
            "off" | "false" | "0" | "disable" => Some(false),
            _ => None,
        });
        let new_val = self.engine.update_session_settings(&self.session_id, |s| {
            let slot = match key {
                "voice" => &mut s.voice_mode,
                "browser" => &mut s.browser_attached,
                "verbose" => &mut s.verbose_mode,
                "reasoning" => &mut s.reasoning_mode,
                "debug" => &mut s.debug_mode,
                _ => &mut s.verbose_mode,
            };
            *slot = parsed.unwrap_or(!*slot);
            if key == "reasoning" && *slot && s.thinking_budget_tokens.is_none() {
                s.thinking_budget_tokens = Some(1024);
            }
            *slot
        });
        Ok(format!("{key} mode is now {}.", if new_val { "ON" } else { "OFF" }))
    }

    fn config_action(
        &mut self,
        action: &str,
        key: Option<&str>,
        val: Option<&str>,
    ) -> std::result::Result<String, String> {
        match action {
            "show" | "list" => {
                let settings = self.engine.session_settings(&self.session_id);
                serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())
            }
            "set" => {
                let k = key.ok_or_else(|| "Usage: `/config set <key> <value>`".to_string())?;
                let v = val.ok_or_else(|| "Usage: `/config set <key> <value>`".to_string())?;
                self.engine.update_session_settings(&self.session_id, |s| {
                    s.config_overrides.insert(k.to_string(), v.to_string());
                });
                Ok(format!("Set `{k}` = `{v}`."))
            }
            "get" => {
                let k = key.ok_or_else(|| "Usage: `/config get <key>`".to_string())?;
                let settings = self.engine.session_settings(&self.session_id);
                match settings.config_overrides.get(k) {
                    Some(v) => Ok(format!("{k} = {v}")),
                    None => Err(format!("Config key '{k}' not set")),
                }
            }
            other => Err(format!("Unsupported config action '{other}'")),
        }
    }
}

// ============================================================================
// HermesEngine Core
// ============================================================================

struct HermesEngineInner {
    config: HermesEngineConfig,
    store: HermesStateStore,
    memory: MemoryManager,
    skills: SkillsManager,
    honcho: HonchoDialectics,
    providers: ProviderRouter,
    security: Arc<RwLock<SecurityEngine>>,
    toolsets: Arc<RwLock<ToolsetRegistry>>,
    session_toolsets: RwLock<HashMap<String, Arc<RwLock<ToolsetRegistry>>>>,
    session_settings: RwLock<HashMap<String, SessionRuntimeSettings>>,
    checkpoints: Arc<Mutex<FileCheckpointManager>>,
    processes: Arc<ProcessRegistry>,
    mcp_registry: Arc<RwLock<McpRegistry>>,
    talk_coordinator: Arc<Mutex<TalkCoordinator>>,
    computer_broker: Arc<Mutex<ComputerLeaseBroker>>,
    computer_provider: RwLock<Option<Arc<dyn ComputerCloudProvider>>>,
    todos: Arc<Mutex<TodoStore>>,
    agentic_store: Arc<Mutex<LocalAgenticStore>>,
    commands: SlashCommandRegistry,
    heartbeat: HeartbeatManager,
    queues: MessageQueueManager,
    turn_events: RwLock<HashMap<String, Vec<HermesEvent>>>,
    turn_metadata: RwLock<HashMap<String, (String, Option<String>)>>,
    turn_event_order: Mutex<VecDeque<String>>,
    ui_actions: Arc<Mutex<Vec<HermesUiAction>>>,
    teammate_handler: RwLock<Option<Arc<dyn toolsets::FabushiTeammateHandler>>>,
    platform_im_bridge: RwLock<Option<Arc<dyn PlatformImBridge>>>,
    agentic_backend: RwLock<Option<Arc<dyn AgenticStateBackend>>>,
    bound_agent_id: RwLock<Option<String>>,
}

#[derive(Clone)]
pub struct HermesEngine {
    inner: Arc<HermesEngineInner>,
}

fn parse_numeric_user_id(raw: Option<&str>) -> Option<u64> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .strip_prefix("user_")
        .unwrap_or(trimmed)
        .parse::<u64>()
        .ok()
}

impl HermesEngine {
    pub fn new(config: HermesEngineConfig) -> Result<Self> {
        let store = match &config.data_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                HermesStateStore::open(dir.join("hermes_state.db"))?
            }
            None => HermesStateStore::open_in_memory()?,
        };

        let memory = match &config.data_dir {
            Some(dir) => MemoryManager::with_dir_and_store(dir.clone(), store.clone(), "global"),
            None => MemoryManager::with_store(store.clone(), "global"),
        };
        let skills = match &config.data_dir {
            Some(dir) => SkillsManager::with_dir(dir.join("skills"))?,
            None => SkillsManager::in_memory(),
        };
        let honcho = HonchoDialectics::with_store(store.clone());

        let force_live_providers =
            std::env::var("HERMES_FORCE_LIVE_PROVIDERS").ok().as_deref() == Some("1");
        let providers = if config.data_dir.is_some() || force_live_providers {
            ProviderRouter::from_env()
        } else {
            ProviderRouter::new()
        };
        let has_hermes_provider_override = std::env::var("HERMES_PROVIDER")
            .ok()
            .is_some_and(|v| !v.trim().is_empty());
        if let Ok(Some(raw)) = store.kv_get("runtime", "providers_state") {
            if let Ok(saved) =
                serde_json::from_str::<crate::hermes::providers::ProviderRouterState>(&raw)
            {
                if !has_hermes_provider_override {
                    let existing_providers: HashSet<String> = providers
                        .list_providers()
                        .into_iter()
                        .map(|p| p.id)
                        .collect();
                    let saved_active_is_subscription = saved
                        .providers
                        .get(&saved.active_provider_id)
                        .is_some_and(|c| c.is_subscription)
                        || saved
                            .subscriptions
                            .iter()
                            .any(|s| s.id == saved.active_provider_id);
                    for (_id, cfg) in saved.providers {
                        if cfg.id != "local_hermes"
                            && cfg.enabled
                            && !cfg.is_subscription
                        {
                            providers.register_provider(cfg);
                        }
                    }
                    if !saved.active_provider_id.is_empty()
                        && saved.active_provider_id != "local_hermes"
                        && (!saved_active_is_subscription
                            || existing_providers.contains(&saved.active_provider_id))
                    {
                        let model_opt = (!saved.active_model.trim().is_empty())
                            .then_some(saved.active_model.as_str());
                        if providers
                            .set_active(&saved.active_provider_id, model_opt)
                            .is_err()
                        {
                            let _ = providers
                                .activate_subscription(&saved.active_provider_id, model_opt);
                        }
                    }
                    if !saved.fallback_chain.is_empty() {
                        let current_providers: HashSet<String> = providers
                            .list_providers()
                            .into_iter()
                            .map(|p| p.id)
                            .collect();
                        let filtered_chain: Vec<_> = saved
                            .fallback_chain
                            .into_iter()
                            .filter(|t| current_providers.contains(&t.provider_id))
                            .collect();
                        if !filtered_chain.is_empty() {
                            providers.set_fallback_chain(filtered_chain);
                        }
                    }
                    providers.set_allow_local_fallback(saved.allow_local_fallback);
                }
                providers.set_routing_strategy(saved.routing_strategy);
                providers.set_masking_config(saved.masking);
                for (_gid, group) in saved.routing_groups {
                    let _ = providers.upsert_routing_group(group);
                }
            }
        }
        let trimmed_default_model = config.default_model.trim();
        if !is_placeholder_default_model(trimmed_default_model)
            && providers.active_info().1 == "hermes-local-v1"
        {
            // Keep local_hermes unless another provider was configured by env,
            // while honoring explicit non-placeholder configured model metadata when present.
            let snap = providers.snapshot();
            if snap.model_profiles.contains_key(trimmed_default_model)
                || snap.providers.values().any(|p| p.id != "local_hermes" && p.enabled)
            {
                let _ = providers.set_active_model(trimmed_default_model);
            }
        }

        let security = SecurityEngine::new(config.workspace_root.clone())
            .with_mode(config.default_approval_mode);

        let inner = Arc::new(HermesEngineInner {
            store,
            memory,
            skills,
            honcho,
            providers,
            security: Arc::new(RwLock::new(security)),
            toolsets: Arc::new(RwLock::new(ToolsetRegistry::from_preset(config.default_preset))),
            session_toolsets: RwLock::new(HashMap::new()),
            session_settings: RwLock::new(HashMap::new()),
            checkpoints: Arc::new(Mutex::new(FileCheckpointManager::new())),
            processes: Arc::new(ProcessRegistry::new()),
            mcp_registry: Arc::new(RwLock::new(McpRegistry::new())),
            talk_coordinator: Arc::new(Mutex::new(TalkCoordinator::new())),
            computer_broker: Arc::new(Mutex::new(ComputerLeaseBroker::new())),
            computer_provider: RwLock::new(None),
            todos: Arc::new(Mutex::new(TodoStore::new())),
            agentic_store: Arc::new(Mutex::new(LocalAgenticStore::new())),
            commands: SlashCommandRegistry::new(),
            heartbeat: HeartbeatManager::new(),
            queues: MessageQueueManager::new(),
            turn_events: RwLock::new(HashMap::new()),
            turn_metadata: RwLock::new(HashMap::new()),
            turn_event_order: Mutex::new(VecDeque::new()),
            ui_actions: Arc::new(Mutex::new(Vec::new())),
            teammate_handler: RwLock::new(None),
            platform_im_bridge: RwLock::new(None),
            agentic_backend: RwLock::new(None),
            bound_agent_id: RwLock::new(None),
            config,
        });
        Ok(Self { inner })
    }

    pub fn in_memory() -> Result<Self> {
        Self::new(HermesEngineConfig::default())
    }

    pub fn open(data_dir: impl AsRef<Path>, workspace_root: impl AsRef<Path>) -> Result<Self> {
        Self::new(HermesEngineConfig {
            workspace_root: workspace_root.as_ref().to_path_buf(),
            data_dir: Some(data_dir.as_ref().to_path_buf()),
            ..Default::default()
        })
    }

    pub fn store(&self) -> &HermesStateStore {
        &self.inner.store
    }

    pub fn memory(&self) -> &MemoryManager {
        &self.inner.memory
    }

    pub fn skills(&self) -> &SkillsManager {
        &self.inner.skills
    }

    pub fn honcho(&self) -> &HonchoDialectics {
        &self.inner.honcho
    }

    pub fn providers(&self) -> &ProviderRouter {
        &self.inner.providers
    }

    pub fn sync_providers_from(&self, upstream: &ProviderRouter) {
        self.inner.providers.sync_from_router(upstream);
    }

    pub fn set_teammate_handler(&self, handler: Arc<dyn toolsets::FabushiTeammateHandler>) {
        if let Ok(mut guard) = self.inner.teammate_handler.write() {
            *guard = Some(handler);
        }
    }

    pub fn teammate_handler(&self) -> Option<Arc<dyn toolsets::FabushiTeammateHandler>> {
        self.inner
            .teammate_handler
            .read()
            .ok()
            .and_then(|g| g.clone())
    }

    pub fn set_platform_im_bridge(&self, bridge: Arc<dyn PlatformImBridge>) {
        if let Ok(mut guard) = self.inner.platform_im_bridge.write() {
            *guard = Some(bridge);
        }
    }

    pub fn platform_im_bridge(&self) -> Option<Arc<dyn PlatformImBridge>> {
        self.inner
            .platform_im_bridge
            .read()
            .ok()
            .and_then(|g| g.clone())
    }

    pub fn set_agentic_backend(&self, backend: Arc<dyn AgenticStateBackend>) {
        if let Ok(mut guard) = self.inner.agentic_backend.write() {
            *guard = Some(backend);
        }
    }

    pub fn agentic_backend(&self) -> Option<Arc<dyn AgenticStateBackend>> {
        self.inner
            .agentic_backend
            .read()
            .ok()
            .and_then(|g| g.clone())
    }

    pub fn agentic_store(&self) -> Arc<Mutex<LocalAgenticStore>> {
        Arc::clone(&self.inner.agentic_store)
    }

    pub fn set_bound_agent_id(&self, agent_id: impl Into<String>) {
        if let Ok(mut guard) = self.inner.bound_agent_id.write() {
            *guard = Some(agent_id.into());
        }
    }

    pub fn bound_agent_id(&self) -> Option<String> {
        self.inner
            .bound_agent_id
            .read()
            .ok()
            .and_then(|g| g.clone())
    }

    pub fn persist_providers_state(&self) {
        let snap = self.inner.providers.snapshot();
        if let Ok(raw) = serde_json::to_string(&snap) {
            let _ = self.inner.store.kv_set("runtime", "providers_state", &raw);
        }
    }

    pub fn security(&self) -> &Arc<RwLock<SecurityEngine>> {
        &self.inner.security
    }

    pub fn toolsets(&self) -> &Arc<RwLock<ToolsetRegistry>> {
        &self.inner.toolsets
    }

    pub fn checkpoints(&self) -> &Arc<Mutex<FileCheckpointManager>> {
        &self.inner.checkpoints
    }

    pub fn processes(&self) -> &Arc<ProcessRegistry> {
        &self.inner.processes
    }

    pub fn mcp_registry(&self) -> &Arc<RwLock<McpRegistry>> {
        &self.inner.mcp_registry
    }

    pub fn talk_coordinator(&self) -> &Arc<Mutex<TalkCoordinator>> {
        &self.inner.talk_coordinator
    }

    pub fn computer_broker(&self) -> &Arc<Mutex<ComputerLeaseBroker>> {
        &self.inner.computer_broker
    }

    pub fn set_computer_provider(&self, provider: Arc<dyn ComputerCloudProvider>) {
        if let Ok(mut guard) = self.inner.computer_provider.write() {
            *guard = Some(provider);
        }
    }

    pub fn computer_provider(&self) -> Option<Arc<dyn ComputerCloudProvider>> {
        self.inner
            .computer_provider
            .read()
            .ok()
            .and_then(|g| g.clone())
    }

    pub fn todos(&self) -> &Arc<Mutex<TodoStore>> {
        &self.inner.todos
    }

    pub fn commands(&self) -> &SlashCommandRegistry {
        &self.inner.commands
    }

    pub fn heartbeat(&self) -> &HeartbeatManager {
        &self.inner.heartbeat
    }

    pub fn queues(&self) -> &MessageQueueManager {
        &self.inner.queues
    }

    pub fn config(&self) -> &HermesEngineConfig {
        &self.inner.config
    }

    pub fn list_commands(&self) -> Vec<CommandDescriptor> {
        self.inner.commands.list_descriptors().to_vec()
    }

    pub fn autocomplete_commands(&self, prefix: &str) -> Vec<CommandDescriptor> {
        self.inner.commands.autocomplete(prefix)
    }

    pub fn drain_ui_actions(&self) -> Vec<HermesUiAction> {
        self.inner
            .ui_actions
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }

    pub fn get_turn_events(&self, turn_id: &str) -> Vec<HermesEvent> {
        self.inner
            .turn_events
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(turn_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn get_turn_session_owner(&self, turn_id: &str) -> Option<(String, Option<String>)> {
        self.inner
            .turn_metadata
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(turn_id)
            .cloned()
    }

    fn record_turn_metadata(&self, turn_id: &str, session_id: &str, owner_user_id: Option<&str>) {
        if let Ok(mut meta) = self.inner.turn_metadata.write() {
            meta.insert(
                turn_id.to_string(),
                (session_id.to_string(), owner_user_id.map(ToString::to_string)),
            );
        }
    }

    fn record_turn_events(&self, turn_id: &str, events: Vec<HermesEvent>) {
        if let Ok(mut map) = self.inner.turn_events.write() {
            map.insert(turn_id.to_string(), events);
            if let Ok(mut order) = self.inner.turn_event_order.lock() {
                order.push_back(turn_id.to_string());
                while order.len() > 128 {
                    if let Some(old) = order.pop_front() {
                        map.remove(&old);
                        if let Ok(mut meta) = self.inner.turn_metadata.write() {
                            meta.remove(&old);
                        }
                    }
                }
            }
        }
    }

    pub fn session_settings(&self, session_id: &str) -> SessionRuntimeSettings {
        self.inner
            .session_settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn update_session_settings<R>(
        &self,
        session_id: &str,
        f: impl FnOnce(&mut SessionRuntimeSettings) -> R,
    ) -> R {
        let mut map = self
            .inner
            .session_settings
            .write()
            .unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(session_id.to_string()).or_default();
        f(entry)
    }

    pub fn session_toolset_registry(&self, session_id: &str) -> Arc<RwLock<ToolsetRegistry>> {
        if let Some(existing) = self
            .inner
            .session_toolsets
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
        {
            return existing;
        }
        let cloned_default = self
            .inner
            .toolsets
            .read()
            .map(|r| r.clone())
            .unwrap_or_else(|_| ToolsetRegistry::from_preset(self.inner.config.default_preset));
        let arc = Arc::new(RwLock::new(cloned_default));
        self.inner
            .session_toolsets
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id.to_string(), Arc::clone(&arc));
        arc
    }

    pub fn set_session_toolset_registry(&self, session_id: &str, registry: ToolsetRegistry) {
        self.inner
            .session_toolsets
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id.to_string(), Arc::new(RwLock::new(registry)));
    }

    pub fn resolve_or_create_session(
        &self,
        session_id: Option<&str>,
        chat_id: Option<i64>,
        bot_id: Option<&str>,
        source: &str,
        model: Option<&str>,
    ) -> Result<SessionRecord> {
        let (_, default_model, default_mode) = self.inner.providers.active_info();
        let active_model = model
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or(default_model);

        if let Some(sid) = session_id.map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(existing) = self.inner.store.get_session(sid)? {
                return Ok(existing);
            }
            return self.inner.store.create_session(CreateSessionRequest {
                id: Some(sid.to_string()),
                chat_id,
                bot_id: bot_id.map(ToString::to_string),
                source: Some(source.to_string()),
                model: Some(active_model),
                api_mode: Some(default_mode.as_str().to_string()),
                ..Default::default()
            });
        }

        if let Some(cid) = chat_id {
            let bot = bot_id.unwrap_or("default");
            let scope = scope_key_for(cid, bot);
            if let Ok(Some(kv_raw)) = self.inner.store.kv_get("active_chat_session", &scope) {
                if let Ok(kv) = serde_json::from_str::<Value>(&kv_raw) {
                    if let Some(active_sid) = kv.get("session_id").and_then(|v| v.as_str()) {
                        if let Some(existing) = self.inner.store.get_session(active_sid)? {
                            if existing.ended_at.is_none() {
                                return Ok(existing);
                            }
                        }
                    }
                }
            }
            let rec = self.inner.store.get_or_create_chat_session(
                cid,
                bot,
                CreateSessionRequest {
                    id: None,
                    chat_id: Some(cid),
                    bot_id: Some(bot.to_string()),
                    source: Some(source.to_string()),
                    model: Some(active_model),
                    api_mode: Some(default_mode.as_str().to_string()),
                    ..Default::default()
                },
            )?;
            let kv_val = json!({ "session_id": rec.id }).to_string();
            let _ = self
                .inner
                .store
                .kv_set("active_chat_session", &scope, &kv_val);
            return Ok(rec);
        }

        if let Some(existing) = self.inner.store.get_session("default_session")? {
            return Ok(existing);
        }
        self.inner.store.create_session(CreateSessionRequest {
            id: Some("default_session".to_string()),
            chat_id: None,
            bot_id: bot_id.map(ToString::to_string),
            source: Some(source.to_string()),
            model: Some(active_model),
            api_mode: Some(default_mode.as_str().to_string()),
            ..Default::default()
        })
    }

    pub fn build_tool_context(
        &self,
        session_id: &str,
        chat_id: Option<i64>,
        bot_id: Option<&str>,
        turn_index: u64,
        preset: Option<ToolsetPreset>,
        caller_depth: usize,
        cancellation: Arc<AtomicBool>,
    ) -> HermesToolContext {
        self.build_tool_context_with_user(
            session_id,
            chat_id,
            bot_id,
            None,
            turn_index,
            preset,
            caller_depth,
            cancellation,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_tool_context_with_user(
        &self,
        session_id: &str,
        chat_id: Option<i64>,
        bot_id: Option<&str>,
        raw_user_id: Option<&str>,
        turn_index: u64,
        preset: Option<ToolsetPreset>,
        caller_depth: usize,
        cancellation: Arc<AtomicBool>,
    ) -> HermesToolContext {
        self.build_tool_context_scoped(
            session_id,
            chat_id,
            bot_id,
            raw_user_id,
            turn_index,
            preset,
            caller_depth,
            cancellation,
            None,
            false,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_tool_context_scoped(
        &self,
        session_id: &str,
        chat_id: Option<i64>,
        bot_id: Option<&str>,
        raw_user_id: Option<&str>,
        turn_index: u64,
        preset: Option<ToolsetPreset>,
        caller_depth: usize,
        cancellation: Arc<AtomicBool>,
        caller_allowed_chat_ids: Option<HashSet<i64>>,
        caller_is_admin: bool,
        can_manage_bot: bool,
    ) -> HermesToolContext {
        let user_id = raw_user_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                chat_id
                    .map(|c| format!("chat_{c}"))
                    .unwrap_or_else(|| "default_user".to_string())
            });
        let parsed_user_id = parse_numeric_user_id(raw_user_id);
        let bound_agent = self.bound_agent_id();
        let scope_key = bound_agent
            .clone()
            .unwrap_or_else(|| "host".to_string());
        let cron_scope_key = if let Some(agent_id) = bound_agent {
            if let Some(cid) = chat_id {
                format!("{agent_id}:chat_{cid}")
            } else if user_id != "default_user" {
                format!("{agent_id}:user_{user_id}")
            } else {
                agent_id
            }
        } else if let Some(cid) = chat_id {
            format!("chat_{cid}")
        } else if user_id != "default_user" {
            format!("user_{user_id}")
        } else {
            "host".to_string()
        };
        let bridge = Arc::new(ToolContextBridge {
            engine: Arc::downgrade(&self.inner),
            store: self.inner.store.clone(),
            memory: self.inner.memory.clone(),
            skills: self.inner.skills.clone(),
            honcho: self.inner.honcho.clone(),
            default_user_id: user_id,
            chat_id,
            ui_actions: Arc::clone(&self.inner.ui_actions),
            agentic_backend: self.agentic_backend(),
            scope_key,
            cron_scope_key,
            caller_allowed_chat_ids,
            caller_is_admin,
            can_manage_bot,
        });

        let bot_id_i64 = bot_id.and_then(|s| {
            s.trim()
                .trim_start_matches("bot_")
                .parse::<i64>()
                .ok()
        });

        let toolsets_reg = self.session_toolset_registry(session_id);
        if let Some(p) = preset {
            if let Ok(mut reg) = toolsets_reg.write() {
                reg.apply_preset(p);
            }
        }

        let mut ctx = HermesToolContext::new(self.inner.config.workspace_root.clone())
            .with_session(session_id, turn_index)
            .with_chat_and_bot(chat_id, bot_id_i64)
            .with_user_id(parsed_user_id)
            .with_state_bridge(bridge)
            .with_mcp_registry(Arc::clone(&self.inner.mcp_registry));

        ctx.security = Arc::clone(&self.inner.security);
        ctx.toolsets = toolsets_reg;
        ctx.checkpoints = Arc::clone(&self.inner.checkpoints);
        ctx.processes = Arc::clone(&self.inner.processes);
        ctx.talk_coordinator = Arc::clone(&self.inner.talk_coordinator);
        ctx.computer_broker = Arc::clone(&self.inner.computer_broker);
        ctx.computer_provider = self.computer_provider();
        ctx.todos = Arc::clone(&self.inner.todos);
        ctx.agentic_store = Arc::clone(&self.inner.agentic_store);
        ctx.caller_depth = caller_depth;
        ctx.cancellation = cancellation;
        ctx.terminal_backend = self.inner.config.terminal_backend.clone();
        ctx.teammate_handler = self.teammate_handler();
        ctx.platform_im_bridge = self.platform_im_bridge();
        ctx.bound_agent_id = self.bound_agent_id();
        ctx
    }

    pub fn execute_command(
        &self,
        session_id: Option<&str>,
        chat_id: Option<i64>,
        bot_id: Option<&str>,
        text: &str,
    ) -> Result<(String, CommandExecutionOutcome)> {
        self.execute_command_scoped(session_id, chat_id, bot_id, None, text)
    }

    pub fn execute_command_scoped(
        &self,
        session_id: Option<&str>,
        chat_id: Option<i64>,
        bot_id: Option<&str>,
        scope: Option<CommandCallerScope>,
        text: &str,
    ) -> Result<(String, CommandExecutionOutcome)> {
        if let Some(ref sc) = scope {
            let is_bot_engine = self.bound_agent_id().is_some() || bot_id.is_some();
            if is_bot_engine && !sc.allows_bot_config() && commands::is_bot_config_command(text) {
                return Err(TelegramError::PermissionDenied(
                    "Only bot owner or server admin can run bot config/memory/soul/skill commands"
                        .into(),
                ));
            }
        }
        let sess = self.resolve_or_create_session(session_id, chat_id, bot_id, "command", None)?;
        if let Some(ref sc) = scope {
            let mut meta: Value =
                serde_json::from_str(&sess.metadata_json).unwrap_or_else(|_| json!({}));
            if let Some(obj) = meta.as_object_mut() {
                if !obj.contains_key("owner_user_id") {
                    obj.insert("owner_user_id".to_string(), json!(sc.user_id));
                    let _ = self.inner.store.update_session(
                        &sess.id,
                        SessionPatch {
                            metadata: Some(meta),
                            ..Default::default()
                        },
                    );
                }
            }
        }
        let mut host = EngineCommandHost {
            engine: self,
            session_id: sess.id.clone(),
            chat_id: chat_id.or(sess.chat_id),
            bot_id: bot_id.map(ToString::to_string).or(sess.bot_id.clone()),
            caller_scope: scope,
        };
        let outcome = self.inner.commands.execute_raw(text, &mut host);
        Ok((host.session_id, outcome))
    }

    pub fn cancel_session(&self, session_id: &str) -> bool {
        self.inner.queues.cancel_session(session_id)
    }

    pub fn steer_session(&self, session_id: &str, instruction: &str) {
        self.inner.queues.push_steer(session_id, instruction);
    }

    pub fn queue_message(&self, session_id: &str, text: &str) -> QueuedMessage {
        self.inner.queues.enqueue(session_id, text)
    }

    pub async fn run_next_queued(&self, session_id: &str) -> Result<Option<HermesTurnResult>> {
        let Some(item) = self.inner.queues.pop_next(session_id) else {
            return Ok(None);
        };
        let sess = self.inner.store.get_session(session_id)?;
        let res = self
            .run_turn(HermesTurnRequest {
                session_id: Some(session_id.to_string()),
                chat_id: sess.as_ref().and_then(|s| s.chat_id),
                bot_id: sess.and_then(|s| s.bot_id),
                source: "queue".to_string(),
                prompt: item.text,
                ..Default::default()
            })
            .await?;
        Ok(Some(res))
    }

    pub fn rollback_turn(
        &self,
        session_id: &str,
        turn_index: Option<u64>,
    ) -> Result<RollbackReport> {
        let mut cp = self
            .inner
            .checkpoints
            .lock()
            .map_err(|_| TelegramError::Internal("Checkpoint lock poisoned".into()))?;
        match cp.rollback_turn(session_id, turn_index) {
            Ok(report) => {
                let _ = self
                    .inner
                    .store
                    .delete_checkpoints_for_turn(session_id, report.turn_index as i64);
                Ok(report)
            }
            Err(_mem_err) => {
                // Fall back to SQLite checkpoints if in-memory checkpoints were restarted cleanly.
                let target_turn = match turn_index {
                    Some(t) => t as i64,
                    None => self
                        .inner
                        .store
                        .latest_checkpoint_turn(session_id)?
                        .ok_or_else(|| {
                            TelegramError::NotFound(format!(
                                "No file checkpoints recorded for session '{session_id}'"
                            ))
                        })?,
                };
                let recs = self
                    .inner
                    .store
                    .list_checkpoints(session_id, Some(target_turn))?;
                if recs.is_empty() {
                    return Err(TelegramError::NotFound(format!(
                        "No checkpoints found for turn {target_turn} in session '{session_id}'"
                    )));
                }
                let mut restored_files = Vec::new();
                let mut deleted_files = Vec::new();
                for rec in recs {
                    let p = Path::new(&rec.file_path);
                    match rec.original_content {
                        Some(orig) => {
                            if let Some(parent) = p.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            std::fs::write(p, orig.as_bytes())?;
                            restored_files.push(rec.file_path);
                        }
                        None => {
                            if p.exists() {
                                std::fs::remove_file(p)?;
                            }
                            deleted_files.push(rec.file_path);
                        }
                    }
                }
                self.inner
                    .store
                    .delete_checkpoints_for_turn(session_id, target_turn)?;
                Ok(RollbackReport {
                    session_id: session_id.to_string(),
                    turn_index: target_turn as u64,
                    restored_files,
                    deleted_files,
                })
            }
        }
    }

    pub fn export_session(
        &self,
        session_id: &str,
        format: TrajectoryFormat,
        redact: bool,
        compression: Option<&TrajectoryCompressionConfig>,
    ) -> Result<ExportedTrajectory> {
        let sess = self
            .inner
            .store
            .get_session(session_id)?
            .ok_or_else(|| TelegramError::NotFound(format!("Session '{session_id}' not found")))?;
        let msgs = self.inner.store.get_messages(session_id, None)?;
        let sys_prompt = self
            .assemble_prompt_for_session(session_id, sess.chat_id, sess.bot_id.as_deref(), None, &[])
            .combined_system_prompt;

        let traj_msgs: Vec<TrajectoryMessage> = msgs
            .into_iter()
            .map(|m| {
                let tool_calls: Vec<TrajectoryToolCall> = m
                    .tool_calls_json
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<Vec<ProviderToolCall>>(raw).ok())
                    .map(|list| {
                        list.into_iter()
                            .map(|tc| TrajectoryToolCall {
                                id: tc.id,
                                name: tc.name,
                                arguments: tc.arguments,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                TrajectoryMessage {
                    seq: m.seq,
                    role: m.role,
                    content: m.content,
                    reasoning: m.reasoning,
                    tool_calls,
                    tool_call_id: m.tool_call_id,
                    tool_name: m.tool_name,
                    token_count: m.token_count.max(0) as usize,
                    is_error: false,
                }
            })
            .collect();

        let bot_id_num = sess
            .bot_id
            .as_deref()
            .and_then(|s| s.trim_start_matches("bot_").parse::<i64>().ok())
            .unwrap_or(0);

        let traj_session = TrajectorySession {
            session_id: sess.id,
            chat_id: sess.chat_id.unwrap_or(0),
            bot_id: bot_id_num,
            model: sess.model,
            api_mode: sess.api_mode,
            title: sess.title.unwrap_or_default(),
            system_prompt: sys_prompt,
            messages: traj_msgs,
            input_tokens: sess.input_tokens.max(0) as u64,
            output_tokens: sess.output_tokens.max(0) as u64,
            total_cost_usd: sess.total_cost_usd,
            metadata: serde_json::from_str(&sess.metadata_json).unwrap_or(Value::Null),
        };

        TrajectoryExporter::export_session(&traj_session, format, redact, compression)
            .map_err(TelegramError::Internal)
    }

    pub async fn run_turn(&self, req: HermesTurnRequest) -> Result<HermesTurnResult> {
        let mut rx = self.start_turn_stream(req)?;
        let mut events = Vec::new();
        let mut session_id = String::new();
        let mut turn_id = String::new();
        let mut model = String::new();
        let mut provider = String::new();
        let mut api_mode = String::new();
        let mut response_text = String::new();
        let mut reasoning = None;
        let mut iterations = 0usize;
        let mut tool_calls = Vec::new();
        let mut pending_args: HashMap<String, Value> = HashMap::new();
        let mut buttons = Vec::new();
        let mut ui_effects = Vec::new();
        let mut usage = TokenUsage::default();
        let mut error_msg: Option<String> = None;

        while let Some(ev) = rx.recv().await {
            match &ev {
                HermesEvent::Meta {
                    session_id: sid,
                    turn_id: tid,
                    model: m,
                    provider: p,
                    api_mode: am,
                    ..
                } => {
                    session_id = sid.clone();
                    turn_id = tid.clone();
                    model = m.clone();
                    provider = p.clone();
                    api_mode = am.clone();
                }
                HermesEvent::ToolStart {
                    tool_call_id,
                    arguments,
                    ..
                } => {
                    pending_args.insert(tool_call_id.clone(), arguments.clone());
                }
                HermesEvent::ToolResult {
                    tool_call_id,
                    tool_name,
                    success,
                    output,
                    duration_ms,
                    ..
                } => {
                    let args = pending_args.remove(tool_call_id).unwrap_or(Value::Null);
                    tool_calls.push(RecordedToolStep {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: tool_name.clone(),
                        arguments: args,
                        success: *success,
                        output: output.clone(),
                        duration_ms: *duration_ms,
                    });
                }
                HermesEvent::CommandResult { outcome } => {
                    if outcome.is_error && !outcome.should_continue_to_llm {
                        response_text = outcome.response_text.clone();
                    }
                    ui_effects.push(outcome.ui_effect.clone());
                    if !outcome.buttons.is_empty() {
                        buttons.extend(outcome.buttons.clone());
                    }
                }
                HermesEvent::InlineButtons { rows } => {
                    buttons.extend(rows.clone());
                }
                HermesEvent::Done {
                    session_id: sid,
                    turn_id: tid,
                    response_text: txt,
                    reasoning: r,
                    iterations: iters,
                    usage: u,
                    buttons: btns,
                    ..
                } => {
                    if session_id.is_empty() {
                        session_id = sid.clone();
                    }
                    if turn_id.is_empty() {
                        turn_id = tid.clone();
                    }
                    response_text = txt.clone();
                    reasoning = r.clone();
                    iterations = *iters;
                    usage = u.clone();
                    if !btns.is_empty() {
                        buttons = btns.clone();
                    }
                }
                HermesEvent::Error { code, message } => {
                    error_msg = Some(format!("{code}: {message}"));
                }
                _ => {}
            }
            events.push(ev);
        }

        if let Some(err) = error_msg {
            if response_text.is_empty() {
                return Err(TelegramError::Internal(err));
            }
        }

        Ok(HermesTurnResult {
            session_id,
            turn_id,
            model,
            provider,
            api_mode,
            response_text,
            reasoning,
            iterations,
            tool_calls,
            buttons,
            ui_effects,
            usage,
            events,
        })
    }

    pub async fn run_turn_stream(
        &self,
        req: HermesTurnRequest,
    ) -> Result<mpsc::Receiver<HermesEvent>> {
        self.start_turn_stream(req)
    }

    pub async fn run_turn_stream_with_handle(
        &self,
        req: HermesTurnRequest,
    ) -> Result<(mpsc::Receiver<HermesEvent>, tokio::task::JoinHandle<()>)> {
        self.start_turn_stream_with_handle(req)
    }

    pub fn start_turn_stream(
        &self,
        req: HermesTurnRequest,
    ) -> Result<mpsc::Receiver<HermesEvent>> {
        self.start_turn_stream_with_handle(req).map(|(rx, _)| rx)
    }

    pub fn start_turn_stream_with_handle(
        &self,
        req: HermesTurnRequest,
    ) -> Result<(mpsc::Receiver<HermesEvent>, tokio::task::JoinHandle<()>)> {
        let sess = self.resolve_or_create_session(
            req.session_id.as_deref(),
            req.chat_id,
            req.bot_id.as_deref(),
            if req.source.trim().is_empty() {
                "direct"
            } else {
                &req.source
            },
            req.model.as_deref(),
        )?;

        if let Some(ref uid) = req.user_id {
            let mut meta: Value =
                serde_json::from_str(&sess.metadata_json).unwrap_or_else(|_| json!({}));
            if let Some(obj) = meta.as_object_mut() {
                if !obj.contains_key("owner_user_id") {
                    obj.insert("owner_user_id".to_string(), json!(uid));
                    let _ = self.inner.store.update_session(
                        &sess.id,
                        SessionPatch {
                            metadata: Some(meta),
                            ..Default::default()
                        },
                    );
                }
            }
        }

        if let Some(ref sp) = req.system_prompt {
            if !sp.trim().is_empty() {
                self.update_session_settings(&sess.id, |s| {
                    s.system_prompt_override = Some(sp.clone());
                });
            }
        }

        struct ActiveTurnClearGuard {
            engine: HermesEngine,
            session_id: String,
            cancel_flag: Arc<AtomicBool>,
        }
        impl Drop for ActiveTurnClearGuard {
            fn drop(&mut self) {
                self.cancel_flag.store(true, Ordering::SeqCst);
                self.engine.inner.queues.clear_active_turn(&self.session_id);
            }
        }

        let (tx, rx) = mpsc::channel::<HermesEvent>(128);
        let engine = self.clone();
        let initial_session_id = sess.id.clone();
        let turn_id = format!("turn_{}", uuid::Uuid::new_v4().simple());
        engine.record_turn_metadata(&turn_id, &initial_session_id, req.user_id.as_deref());

        let task = tokio::spawn(async move {
            let mut recorded_events = Vec::new();
            let cancel_flag = Arc::new(AtomicBool::new(false));
            engine
                .inner
                .queues
                .register_active_turn(&initial_session_id, Arc::clone(&cancel_flag));
            let _active_guard = ActiveTurnClearGuard {
                engine: engine.clone(),
                session_id: initial_session_id.clone(),
                cancel_flag: Arc::clone(&cancel_flag),
            };

            let run_fut = async {
                // 1. Slash Command Intercept
                if !req.skip_slash_commands && SlashCommandParser::is_slash_command(&req.prompt) {
                    let cmd_scope = req.user_id.as_deref().map(|uid| {
                        let mut allowed = req.caller_allowed_chat_ids.clone().unwrap_or_default();
                        if let Some(cid) = req.chat_id.or(sess.chat_id) {
                            allowed.insert(cid);
                        }
                        CommandCallerScope {
                            user_id: uid.to_string(),
                            allowed_chat_ids: allowed,
                            is_admin: req.caller_is_admin,
                            bot_config_allowed: req.caller_owns_bot.unwrap_or(true),
                        }
                    });
                    match engine.execute_command_scoped(
                        Some(&initial_session_id),
                        req.chat_id.or(sess.chat_id),
                        req.bot_id.as_deref().or(sess.bot_id.as_deref()),
                        cmd_scope,
                        &req.prompt,
                    ) {
                        Ok((resolved_sid, outcome)) => {
                            emit_event(
                                &tx,
                                &mut recorded_events,
                                HermesEvent::CommandResult {
                                    outcome: outcome.clone(),
                                },
                            )
                            .await;
                            if !outcome.buttons.is_empty() {
                                emit_event(
                                    &tx,
                                    &mut recorded_events,
                                    HermesEvent::InlineButtons {
                                        rows: outcome.buttons.clone(),
                                    },
                                )
                                .await;
                            }

                            match &outcome.ui_effect {
                                CommandUiEffect::LaunchMoa { prompt, models } => {
                                    engine
                                        .run_moa_turn(
                                            &resolved_sid,
                                            &turn_id,
                                            prompt,
                                            models,
                                            &tx,
                                            &mut recorded_events,
                                        )
                                        .await;
                                }
                                CommandUiEffect::StartGoalLoop {
                                    goal,
                                    max_iterations,
                                } => {
                                    engine
                                        .run_goal_loop(
                                            &resolved_sid,
                                            &turn_id,
                                            goal,
                                            *max_iterations,
                                            &req,
                                            cancel_flag.clone(),
                                            &tx,
                                            &mut recorded_events,
                                        )
                                        .await;
                                }
                                CommandUiEffect::RunCronNow { job_id } => {
                                    match engine.run_cron_job_now(job_id).await {
                                        Ok(cron_res) => {
                                            emit_event(
                                                &tx,
                                                &mut recorded_events,
                                                HermesEvent::Done {
                                                    session_id: resolved_sid.clone(),
                                                    turn_id: turn_id.clone(),
                                                    response_text: format!(
                                                        "Cron job `{}` completed ({}ms):\n{}",
                                                        cron_res.job_name,
                                                        cron_res.duration_ms,
                                                        cron_res.response_text
                                                    ),
                                                    reasoning: None,
                                                    iterations: 1,
                                                    tool_calls_count: 0,
                                                    usage: TokenUsage::default(),
                                                    buttons: Vec::new(),
                                                },
                                            )
                                            .await;
                                        }
                                        Err(e) => {
                                            emit_event(
                                                &tx,
                                                &mut recorded_events,
                                                HermesEvent::Error {
                                                    code: "CRON_RUN_FAILED".into(),
                                                    message: e.to_string(),
                                                },
                                            )
                                            .await;
                                        }
                                    }
                                }
                                CommandUiEffect::SteerActiveTurn {
                                    session_id: steer_sid,
                                    instruction,
                                } if !engine.inner.queues.is_active(steer_sid) => {
                                    // No currently active turn to intercept, execute steer immediately on session.
                                    engine
                                        .run_agent_loop(
                                            steer_sid,
                                            &turn_id,
                                            &HermesTurnRequest {
                                                session_id: Some(steer_sid.clone()),
                                                prompt: instruction.clone(),
                                                skip_slash_commands: true,
                                                ..req.clone()
                                            },
                                            cancel_flag.clone(),
                                            &tx,
                                            &mut recorded_events,
                                        )
                                        .await;
                                }
                                _ => {
                                    if outcome.should_continue_to_llm {
                                        let rewritten = outcome
                                            .rewritten_prompt
                                            .clone()
                                            .unwrap_or_else(|| req.prompt.clone());
                                        let mut next_req = req.clone();
                                        next_req.prompt = rewritten;
                                        engine
                                            .run_agent_loop(
                                                &resolved_sid,
                                                &turn_id,
                                                &next_req,
                                                cancel_flag.clone(),
                                                &tx,
                                                &mut recorded_events,
                                            )
                                            .await;
                                    } else {
                                        emit_event(
                                            &tx,
                                            &mut recorded_events,
                                            HermesEvent::Done {
                                                session_id: resolved_sid.clone(),
                                                turn_id: turn_id.clone(),
                                                response_text: outcome.response_text.clone(),
                                                reasoning: None,
                                                iterations: 0,
                                                tool_calls_count: 0,
                                                usage: TokenUsage::default(),
                                                buttons: outcome.buttons.clone(),
                                            },
                                        )
                                        .await;
                                    }
                                }
                            }
                            return;
                        }
                        Err(e) => {
                            emit_event(
                                &tx,
                                &mut recorded_events,
                                HermesEvent::Error {
                                    code: "COMMAND_ERROR".into(),
                                    message: e.to_string(),
                                },
                            )
                            .await;
                            return;
                        }
                    }
                }

                // 2. Normal AIAgent Loop
                engine
                    .run_agent_loop(
                        &initial_session_id,
                        &turn_id,
                        &req,
                        cancel_flag.clone(),
                        &tx,
                        &mut recorded_events,
                    )
                    .await;
            };

            // Abort promptly if streaming receiver disconnects across any branch
            tokio::select! {
                biased;
                _ = tx.closed() => {
                    cancel_flag.store(true, Ordering::SeqCst);
                }
                _ = run_fut => {}
            }

            engine.inner.queues.clear_active_turn(&initial_session_id);
            engine.record_turn_events(&turn_id, recorded_events);
        });

        Ok((rx, task))
    }

    async fn run_moa_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        prompt: &str,
        models: &[String],
        tx: &mpsc::Sender<HermesEvent>,
        recorded: &mut Vec<HermesEvent>,
    ) {
        let user_id = self
            .inner
            .store
            .get_session(session_id)
            .ok()
            .flatten()
            .and_then(|s| s.chat_id)
            .map(|c| format!("chat_{c}"))
            .unwrap_or_else(|| "default_user".into());
        let _ = self.inner.store.append_message(AppendMessageRequest {
            id: None,
            session_id: session_id.to_string(),
            role: "user".into(),
            content: prompt.to_string(),
            token_count: estimate_tokens(prompt) as i64,
            ..Default::default()
        });
        let _ = self
            .inner
            .honcho
            .ingest_user_message(&user_id, Some(session_id), prompt);

        let (active_prov, active_model, active_mode) = self.inner.providers.active_info();
        let meta = HermesEvent::Meta {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            model: active_model.clone(),
            provider: active_prov,
            api_mode: format!("{}:moa", active_mode.as_str()),
            tier1_hash: "moa".into(),
        };
        recorded.push(meta.clone());
        let _ = tx.send(meta).await;

        let mut coordinator = MoaCoordinator::new();
        if !models.is_empty() {
            let specs: Vec<MoaModelSpec> = models
                .iter()
                .enumerate()
                .map(|(idx, m)| {
                    let (pid, mod_name) = if let Some((p, mdl)) = m.split_once('/') {
                        (p.to_string(), mdl.to_string())
                    } else {
                        ("local_hermes".to_string(), m.clone())
                    };
                    MoaModelSpec {
                        provider_id: pid,
                        model: mod_name,
                        perspective_role: Some(format!("Perspective #{}", idx + 1)),
                    }
                })
                .collect();
            coordinator = coordinator.with_models(specs);
        }

        let assembled = self.assemble_prompt_for_session(session_id, None, None, None, &[]);
        let mut base_req = ProviderRequest::new(
            active_model.clone(),
            vec![ProviderChatMessage::user(prompt)],
        );
        base_req.system_tiers = tiers_from_assembled(&assembled);

        match coordinator.run(&self.inner.providers, base_req).await {
            Ok(moa) => {
                let text = moa.synthesis;
                let td = HermesEvent::TextDelta {
                    text: text.clone(),
                    accumulated: text.clone(),
                };
                recorded.push(td.clone());
                let _ = tx.send(td).await;

                let _ = self.inner.store.append_message(AppendMessageRequest {
                    id: None,
                    session_id: session_id.to_string(),
                    role: "assistant".into(),
                    content: text.clone(),
                    token_count: estimate_tokens(&text) as i64,
                    ..Default::default()
                });
                let _ = self.inner.store.record_token_usage(
                    session_id,
                    moa.total_usage.input_tokens as i64,
                    moa.total_usage.output_tokens as i64,
                    moa.total_usage.estimated_cost_usd,
                );

                let done = HermesEvent::Done {
                    session_id: session_id.to_string(),
                    turn_id: turn_id.to_string(),
                    response_text: text,
                    reasoning: None,
                    iterations: moa.candidates.len(),
                    tool_calls_count: 0,
                    usage: moa.total_usage,
                    buttons: Vec::new(),
                };
                recorded.push(done.clone());
                let _ = tx.send(done).await;
            }
            Err(e) => {
                let err = HermesEvent::Error {
                    code: "MOA_FAILED".into(),
                    message: e.to_string(),
                };
                recorded.push(err.clone());
                let _ = tx.send(err).await;
            }
        }
    }

    async fn run_goal_loop(
        &self,
        session_id: &str,
        turn_id: &str,
        goal: &str,
        max_iterations: usize,
        base_req: &HermesTurnRequest,
        cancel_flag: Arc<AtomicBool>,
        tx: &mpsc::Sender<HermesEvent>,
        recorded: &mut Vec<HermesEvent>,
    ) {
        self.update_session_settings(session_id, |s| s.active_goal = Some(goal.to_string()));
        let steps = max_iterations.clamp(1, 8);
        let mut last_summary = String::new();
        let mut total_usage = TokenUsage::default();
        let mut completed_steps = 0usize;

        for step in 1..=steps {
            if cancel_flag.load(Ordering::SeqCst) {
                break;
            }
            completed_steps = step;
            let prog = HermesEvent::GoalProgress {
                iteration: step,
                max_iterations: steps,
                status: "running".into(),
                note: format!("Evaluating goal step {step}/{steps}: {goal}"),
            };
            recorded.push(prog.clone());
            let _ = tx.send(prog).await;

            let step_prompt = if step == 1 {
                format!(
                    "Autonomous Goal Loop (Step 1/{steps}):\nObjective: {goal}\nUse available tools if needed, verify progress, and end with `[GOAL_COMPLETE]` once verified."
                )
            } else {
                format!(
                    "Autonomous Goal Verification (Step {step}/{steps}):\nObjective: {goal}\nPrevious progress: {}\nVerify completion or finish remaining steps. Include `[GOAL_COMPLETE]` when fully satisfied.",
                    truncate_chars(&last_summary, 400)
                )
            };

            let mut step_req = base_req.clone();
            step_req.session_id = Some(session_id.to_string());
            step_req.prompt = step_prompt;
            step_req.skip_slash_commands = true;
            step_req.max_iterations = Some(4);

            let (sub_tx, mut sub_rx) = mpsc::channel(64);
            let mut sub_recorded = Vec::new();
            self.run_agent_loop(
                session_id,
                turn_id,
                &step_req,
                Arc::clone(&cancel_flag),
                &sub_tx,
                &mut sub_recorded,
            )
            .await;
            drop(sub_tx);

            let mut step_text = String::new();
            let mut step_used_tools = false;
            while let Some(ev) = sub_rx.recv().await {
                match &ev {
                    HermesEvent::ToolStart { .. } => step_used_tools = true,
                    HermesEvent::Done {
                        response_text,
                        usage,
                        ..
                    } => {
                        step_text = response_text.clone();
                        total_usage.accumulate(usage);
                        continue;
                    }
                    _ => {}
                }
                recorded.push(ev.clone());
                let _ = tx.send(ev).await;
            }

            if !step_text.is_empty() {
                last_summary = step_text;
            }

            // Stop early when explicitly marked complete or a non-tool verified pass concludes.
            if last_summary.contains("[GOAL_COMPLETE]")
                || (step >= 1 && !step_used_tools && !last_summary.trim().is_empty())
            {
                let done_prog = HermesEvent::GoalProgress {
                    iteration: step,
                    max_iterations: steps,
                    status: "completed".into(),
                    note: "Goal verified and completed.".into(),
                };
                recorded.push(done_prog.clone());
                let _ = tx.send(done_prog).await;
                break;
            }
        }

        self.update_session_settings(session_id, |s| s.active_goal = None);
        let cleaned = last_summary.replace("[GOAL_COMPLETE]", "").trim().to_string();
        let done = HermesEvent::Done {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            response_text: if cleaned.is_empty() {
                format!("Goal loop finished after {completed_steps} step(s).")
            } else {
                cleaned
            },
            reasoning: None,
            iterations: completed_steps,
            tool_calls_count: 0,
            usage: total_usage,
            buttons: Vec::new(),
        };
        recorded.push(done.clone());
        let _ = tx.send(done).await;
    }

    async fn run_agent_loop(
        &self,
        session_id: &str,
        turn_id: &str,
        req: &HermesTurnRequest,
        cancel_flag: Arc<AtomicBool>,
        tx: &mpsc::Sender<HermesEvent>,
        recorded: &mut Vec<HermesEvent>,
    ) {
        let sess = match self.inner.store.get_session(session_id) {
            Ok(Some(s)) => s,
            Ok(None) => {
                let e = HermesEvent::Error {
                    code: "SESSION_NOT_FOUND".into(),
                    message: format!("Session '{session_id}' not found"),
                };
                recorded.push(e.clone());
                let _ = tx.send(e).await;
                return;
            }
            Err(err) => {
                let e = HermesEvent::Error {
                    code: "STORE_ERROR".into(),
                    message: err.to_string(),
                };
                recorded.push(e.clone());
                let _ = tx.send(e).await;
                return;
            }
        };

        // Resolve @file, @dir, @diff, @staged, @git:<n>, @url references inside workspace
        let ref_resolver = ContextRefResolver::new(&self.inner.config.workspace_root);
        let resolved_refs = ref_resolver.resolve(&req.prompt);

        // Persist user prompt cleanly in SQLite and Honcho dialectics
        let user_id = req
            .user_id
            .clone()
            .or_else(|| sess.chat_id.map(|c| format!("chat_{c}")))
            .unwrap_or_else(|| "default_user".to_string());
        let _ = self
            .inner
            .honcho
            .ingest_user_message(&user_id, Some(session_id), &req.prompt);

        let user_record = match self.inner.store.append_message(AppendMessageRequest {
            id: None,
            session_id: session_id.to_string(),
            role: "user".into(),
            content: req.prompt.clone(),
            reasoning: None,
            tool_calls_json: None,
            tool_call_id: None,
            tool_name: None,
            token_count: estimate_tokens(&req.prompt) as i64,
            created_at: None,
        }) {
            Ok(rec) => rec,
            Err(e) => {
                let ev = HermesEvent::Error {
                    code: "STORE_APPEND_FAILED".into(),
                    message: e.to_string(),
                };
                recorded.push(ev.clone());
                let _ = tx.send(ev).await;
                return;
            }
        };

        let mut history_records = self
            .inner
            .store
            .get_messages(session_id, None)
            .unwrap_or_default();

        let turn_index = history_records
            .iter()
            .filter(|m| m.role == "user")
            .count()
            .max(1) as u64;

        let tool_ctx = self.build_tool_context_scoped(
            session_id,
            req.chat_id.or(sess.chat_id),
            req.bot_id.as_deref().or(sess.bot_id.as_deref()),
            req.user_id.as_deref(),
            turn_index,
            req.preset,
            req.caller_depth,
            Arc::clone(&cancel_flag),
            req.caller_allowed_chat_ids.clone(),
            req.caller_is_admin,
            req.caller_owns_bot.unwrap_or(true) || req.caller_is_admin,
        );

        let active_specs = HermesToolExecutor::list_specs(&tool_ctx);
        let provider_tools: Vec<ProviderToolSpec> = active_specs
            .iter()
            .map(|s| ProviderToolSpec {
                name: s.name.clone(),
                description: s.description.clone(),
                parameters: s.parameters_schema.clone(),
            })
            .collect();

        let mut prefetch_hints: Vec<String> = resolved_refs.warnings.clone();
        let active_settings = self.session_settings(session_id);
        let (active_prov, active_model, active_mode) = self.inner.providers.active_info();
        let chosen_model = req
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&active_model)
            .to_string();

        let persona_override = req
            .system_prompt
            .as_deref()
            .or(active_settings.system_prompt_override.as_deref());
        let assembled = self.assemble_prompt_for_session_with_name(
            session_id,
            req.chat_id.or(sess.chat_id),
            req.bot_id.as_deref().or(sess.bot_id.as_deref()),
            req.bot_name.as_deref(),
            persona_override,
            &prefetch_hints,
        );
        let _ = prefetch_hints.drain(..);

        // Emit Meta event cleanly
        let meta_ev = HermesEvent::Meta {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            model: chosen_model.clone(),
            provider: active_prov.clone(),
            api_mode: active_mode.as_str().to_string(),
            tier1_hash: assembled.tier1_hash.clone(),
        };
        recorded.push(meta_ev.clone());
        let _ = tx.send(meta_ev).await;

        // Apply automatic context compression if conversation exceeds token budget
        let compressor = ContextCompressor::new(self.inner.config.context_compression.clone());
        let ctx_view: Vec<ContextMessage> = history_records
            .iter()
            .map(|m| ContextMessage {
                role: m.role.clone(),
                content: m.content.clone(),
                tool_call_id: m.tool_call_id.clone(),
                tool_name: m.tool_name.clone(),
                pinned: false,
            })
            .collect();
        if compressor.should_compress(&ctx_view) {
            let outcome = compressor.compress(&ctx_view, false);
            if outcome.compressed {
                let replacement: Vec<AppendMessageRequest> = outcome
                    .messages
                    .into_iter()
                    .map(|m| AppendMessageRequest {
                        id: None,
                        session_id: session_id.to_string(),
                        role: m.role,
                        content: m.content.clone(),
                        reasoning: None,
                        tool_calls_json: None,
                        tool_call_id: m.tool_call_id,
                        tool_name: m.tool_name,
                        token_count: estimate_tokens(&m.content) as i64,
                        created_at: None,
                    })
                    .collect();
                if let Ok(new_recs) = self.inner.store.replace_session_messages(session_id, replacement) {
                    history_records = new_recs;
                }
            }
        }

        // Convert stored messages into ProviderChatMessages (expanding @refs on latest user turn)
        let mut llm_messages =
            records_to_provider_messages(&history_records, &user_record.id, &resolved_refs.expanded_prompt);
        if self.inner.config.enable_prompt_caching {
            if let Some(last) = llm_messages.last_mut() {
                last.cache_control = Some(CacheControl::ephemeral());
            }
        }

        let meta_json = self.build_provider_metadata(
            session_id,
            persona_override,
            req.bot_id.as_deref().or(sess.bot_id.as_deref()),
            req.bot_name.as_deref(),
        );
        let max_iters = req
            .max_iterations
            .unwrap_or(self.inner.config.max_iterations)
            .clamp(1, 64);

        let mut total_usage = TokenUsage::default();
        let mut collected_buttons: Vec<Vec<InlineActionButton>> = Vec::new();
        let mut tool_calls_count = 0usize;
        let mut final_response_text = String::new();
        let mut combined_reasoning: Option<String> = None;
        let mut iterations_executed = 0usize;
        let mut guardrails = ToolGuardrailTracker::default();
        let mut empty_recovery_attempted = false;
        let mut grace_synthesis_reason: Option<&'static str> = None;

        for iter in 1..=max_iters {
            if cancel_flag.load(Ordering::SeqCst) {
                if final_response_text.is_empty() {
                    final_response_text = "Turn cancelled by user.".to_string();
                }
                break;
            }

            // Pick up any mid-turn /steer instructions injected while tools were running
            for steer in self.inner.queues.take_steers(session_id) {
                let msg = format!("[User steering instruction]: {steer}");
                let _ = self.inner.store.append_message(AppendMessageRequest {
                    id: None,
                    session_id: session_id.to_string(),
                    role: "user".into(),
                    content: msg.clone(),
                    token_count: estimate_tokens(&msg) as i64,
                    ..Default::default()
                });
                llm_messages.push(ProviderChatMessage::user(msg));
            }

            iterations_executed = iter;

            let mut prov_req = ProviderRequest::new(chosen_model.clone(), llm_messages.clone());
            prov_req.system_tiers = tiers_from_assembled(&assembled);
            prov_req.tools = provider_tools.clone();
            prov_req.temperature = req.temperature.or(self.inner.config.default_temperature);
            prov_req.max_tokens = req.max_tokens.or(self.inner.config.default_max_tokens);
            prov_req.thinking_budget_tokens = req
                .thinking_budget_tokens
                .or(active_settings.thinking_budget_tokens)
                .or(if active_settings.reasoning_mode { Some(1024) } else { None });
            prov_req.stream = true;
            prov_req.metadata = meta_json.clone();

            struct AbortJoinOnDrop<T>(Option<tokio::task::JoinHandle<T>>);
            impl<T> Drop for AbortJoinOnDrop<T> {
                fn drop(&mut self) {
                    if let Some(h) = self.0.take() {
                        h.abort();
                    }
                }
            }

            let (ptx, mut prx) = mpsc::channel::<ProviderStreamEvent>(64);
            let tx_stream = tx.clone();
            let mut forwarder_guard = AbortJoinOnDrop(Some(tokio::spawn(async move {
                let mut stream_events = Vec::new();
                let mut acc = String::new();
                loop {
                    let sev_opt = tokio::select! {
                        biased;
                        _ = tx_stream.closed() => None,
                        sev = prx.recv() => sev,
                    };
                    let Some(sev) = sev_opt else {
                        break;
                    };
                    match sev {
                        ProviderStreamEvent::ThinkingDelta(t) => {
                            let ev = HermesEvent::ThinkingDelta { text: t };
                            stream_events.push(ev.clone());
                            if tx_stream.send(ev).await.is_err() {
                                break;
                            }
                        }
                        ProviderStreamEvent::TextDelta(t) => {
                            acc.push_str(&t);
                            let ev = HermesEvent::TextDelta {
                                text: t,
                                accumulated: acc.clone(),
                            };
                            stream_events.push(ev.clone());
                            if tx_stream.send(ev).await.is_err() {
                                break;
                            }
                        }
                        ProviderStreamEvent::ToolCallReady(_) | ProviderStreamEvent::Usage(_) => {}
                    }
                }
                stream_events
            })));

            let comp_res = tokio::select! {
                biased;
                _ = tx.closed() => {
                    cancel_flag.store(true, Ordering::SeqCst);
                    Err(providers::ProviderError::Transport("Stream client disconnected".to_string()))
                }
                res = self.inner.providers.complete(prov_req, Some(ptx)) => res,
            };
            let streamed_evs = if let Some(h) = forwarder_guard.0.take() {
                h.await.unwrap_or_default()
            } else {
                Vec::new()
            };
            recorded.extend(streamed_evs);

            let resp = match comp_res {
                Ok(r) => r,
                Err(err) => {
                    let ev = HermesEvent::Error {
                        code: "PROVIDER_ERROR".into(),
                        message: err.to_string(),
                    };
                    recorded.push(ev.clone());
                    let _ = tx.send(ev).await;
                    return;
                }
            };

            total_usage.accumulate(&resp.usage);
            let _ = self.inner.store.record_token_usage(
                session_id,
                resp.usage.input_tokens as i64,
                resp.usage.output_tokens as i64,
                resp.usage.estimated_cost_usd,
            );

            if let Some(ref r_text) = resp.reasoning {
                combined_reasoning = Some(match combined_reasoning {
                    Some(prev) => format!("{prev}\n{r_text}"),
                    None => r_text.clone(),
                });
            }

            if resp.tool_calls.is_empty() {
                // Empty-Response Continuation Recovery: if tools ran previously in this turn and
                // the model returned empty text with zero tool calls, issue ONE recovery nudge.
                if resp.content.trim().is_empty()
                    && iter > 1
                    && tool_calls_count > 0
                    && !empty_recovery_attempted
                {
                    empty_recovery_attempted = true;
                    llm_messages.push(ProviderChatMessage::user(EMPTY_RESPONSE_RECOVERY_NUDGE));
                    if iter < max_iters {
                        continue;
                    }
                    grace_synthesis_reason = Some("empty_response_after_tools");
                    break;
                }

                final_response_text = resp.content.clone();
                let _ = self.inner.store.append_message(AppendMessageRequest {
                    id: None,
                    session_id: session_id.to_string(),
                    role: "assistant".into(),
                    content: final_response_text.clone(),
                    reasoning: resp.reasoning.clone(),
                    tool_calls_json: None,
                    tool_call_id: None,
                    tool_name: None,
                    token_count: estimate_tokens(&final_response_text) as i64,
                    created_at: None,
                });
                break;
            }

            // Fill empty/blank tool call IDs deterministically (`call_<iter>_<idx>`) before persisting
            // the assistant message so assistant `tool_calls` and tool responses stay paired.
            let mut normalized_calls = resp.tool_calls.clone();
            for (idx, tc) in normalized_calls.iter_mut().enumerate() {
                if tc.id.trim().is_empty() {
                    tc.id = format!("call_{iter}_{idx}");
                }
            }

            // Persist assistant message containing tool calls
            let tool_calls_json = serde_json::to_string(&normalized_calls).ok();
            let _ = self.inner.store.append_message(AppendMessageRequest {
                id: None,
                session_id: session_id.to_string(),
                role: "assistant".into(),
                content: resp.content.clone(),
                reasoning: resp.reasoning.clone(),
                tool_calls_json,
                tool_call_id: None,
                tool_name: None,
                token_count: estimate_tokens(&resp.content) as i64,
                created_at: None,
            });
            llm_messages.push(ProviderChatMessage::assistant_with_tools(
                resp.content.clone(),
                normalized_calls.clone(),
                resp.reasoning.clone(),
            ));

            // Execute each requested tool with JSON repair, non-fatal error envelope, and stall/cycle guardrails.
            let mut short_circuit_after_batch = false;
            for tc in &normalized_calls {
                if cancel_flag.load(Ordering::SeqCst) {
                    break;
                }
                tool_calls_count += 1;

                if short_circuit_after_batch {
                    let skipped_msg = build_non_fatal_tool_error_envelope(
                        &tc.name,
                        "Tool execution skipped: stall/cycle guardrail short-circuited the loop.",
                    );
                    let _ = self.inner.store.append_message(AppendMessageRequest {
                        id: None,
                        session_id: session_id.to_string(),
                        role: "tool".into(),
                        content: skipped_msg.clone(),
                        reasoning: None,
                        tool_calls_json: None,
                        tool_call_id: Some(tc.id.clone()),
                        tool_name: Some(tc.name.clone()),
                        token_count: estimate_tokens(&skipped_msg) as i64,
                        created_at: None,
                    });
                    llm_messages.push(ProviderChatMessage::tool_result(
                        tc.id.clone(),
                        tc.name.clone(),
                        skipped_msg,
                    ));
                    continue;
                }

                let repaired = repair_tool_call_arguments(&tc.arguments);
                let start_args_val = match &repaired {
                    Ok(v) => v.clone(),
                    Err(_) => Value::String(tc.arguments.clone()),
                };

                let start_ev = HermesEvent::ToolStart {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    arguments: start_args_val,
                };
                recorded.push(start_ev.clone());
                let _ = tx.send(start_ev).await;

                let (result, canonical_args) = match repaired {
                    Ok(args_val) if args_val.is_object() => {
                        let canonical = canonicalize_json(&args_val);
                        let call = HermesToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            arguments: args_val,
                        };
                        let res = HermesToolExecutor::execute_call(&call, &tool_ctx).await;
                        (res, canonical)
                    }
                    Ok(non_obj) => {
                        let canonical = canonicalize_json(&non_obj);
                        let res = HermesToolResult::err(
                            &tc.name,
                            "Tool arguments must be a JSON object.",
                        )
                        .with_call_id(tc.id.clone());
                        (res, canonical)
                    }
                    Err(parse_err) => {
                        let res = HermesToolResult::err(
                            &tc.name,
                            format!("Invalid tool arguments JSON: {parse_err}"),
                        )
                        .with_call_id(tc.id.clone());
                        (res, tc.arguments.trim().to_string())
                    }
                };

                let mut output_for_model = if result.success {
                    result.output.clone()
                } else {
                    build_non_fatal_tool_error_envelope(&tc.name, &result.output)
                };

                if let Ok(sec) = self.inner.security.read() {
                    let scan = sec.scan_output(&output_for_model);
                    if scan.flagged {
                        let summary = scan
                            .findings
                            .iter()
                            .map(|f| format!("{} ({})", f.description, f.rule_id))
                            .collect::<Vec<_>>()
                            .join("; ");
                        output_for_model = format!(
                            "[Security notice: untrusted tool output flagged potential injection: {}]\n{}",
                            summary,
                            scan.sanitized_text
                        );
                    }
                }

                // Evaluate stall streaks, period 2..=4 cycles, and >= 512 char duplicate output stubs.
                let guardrail_eval = guardrails.record_and_evaluate(
                    &tc.id,
                    &tc.name,
                    &canonical_args,
                    &output_for_model,
                );
                if let Some(stub) = guardrail_eval.stubbed_output {
                    output_for_model = stub;
                }
                for warn in guardrail_eval.warning_messages {
                    if !output_for_model.is_empty() {
                        output_for_model.push('\n');
                    }
                    output_for_model.push_str(&warn);
                }
                if guardrail_eval.short_circuit {
                    grace_synthesis_reason = Some("guardrail_short_circuit");
                    short_circuit_after_batch = true;
                }

                // Surface approval prompt / inline buttons when a tool requested confirmation
                if !result.inline_buttons.is_empty() {
                    let apr_ev_opt = if let Ok(sec) = self.inner.security.read() {
                        sec.approvals
                            .pending
                            .iter()
                            .rev()
                            .find(|p| p.session_id == session_id)
                            .map(|pending| HermesEvent::ApprovalRequired {
                                approval_id: pending.id.clone(),
                                session_id: pending.session_id.clone(),
                                tool_name: pending.tool_name.clone(),
                                command: pending.command.clone(),
                                risk_level: pending.risk_level.clone(),
                                reason: pending.reason.clone(),
                                buttons: result.inline_buttons.clone(),
                            })
                    } else {
                        None
                    };
                    if let Some(apr_ev) = apr_ev_opt {
                        recorded.push(apr_ev.clone());
                        let _ = tx.send(apr_ev).await;
                    }

                    let row: Vec<InlineActionButton> = result
                        .inline_buttons
                        .iter()
                        .map(|b| InlineActionButton {
                            text: b.label.clone(),
                            callback_data: b.callback_data.clone(),
                            url: None,
                        })
                        .collect();
                    collected_buttons.push(row.clone());
                    let btn_ev = HermesEvent::InlineButtons { rows: vec![row] };
                    recorded.push(btn_ev.clone());
                    let _ = tx.send(btn_ev).await;
                }

                let res_ev = HermesEvent::ToolResult {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    success: result.success,
                    output: result.output.clone(),
                    duration_ms: result.duration_ms,
                    inline_buttons: result.inline_buttons.clone(),
                    ui_actions: result.ui_actions.clone(),
                };
                recorded.push(res_ev.clone());
                let _ = tx.send(res_ev).await;

                let _ = self.inner.store.append_message(AppendMessageRequest {
                    id: None,
                    session_id: session_id.to_string(),
                    role: "tool".into(),
                    content: output_for_model.clone(),
                    reasoning: None,
                    tool_calls_json: None,
                    tool_call_id: Some(tc.id.clone()),
                    tool_name: Some(tc.name.clone()),
                    token_count: estimate_tokens(&output_for_model) as i64,
                    created_at: None,
                });

                llm_messages.push(ProviderChatMessage::tool_result(
                    tc.id.clone(),
                    tc.name.clone(),
                    output_for_model,
                ));
            }

            if grace_synthesis_reason.is_some() {
                break;
            }

            if iter == max_iters
                && final_response_text.is_empty()
                && !cancel_flag.load(Ordering::SeqCst)
            {
                grace_synthesis_reason = Some("max_iterations");
            }
        }

        // Grace Synthesis Final Call: whenever max_iterations or a stall/cycle guardrail short-circuits
        // while tool calls were still occurring, run ONE final tool-less completion call (`tools: vec![]`)
        // to synthesize everything accomplished and observed so far instead of dumping a raw error.
        if !cancel_flag.load(Ordering::SeqCst) && final_response_text.is_empty() {
            if let Some(reason) = grace_synthesis_reason {
                if reason != "empty_response_after_tools" {
                    llm_messages.push(ProviderChatMessage::user(GRACE_SYNTHESIS_NOTICE));
                }

                let mut final_req = ProviderRequest::new(chosen_model.clone(), llm_messages.clone());
                final_req.system_tiers = tiers_from_assembled(&assembled);
                final_req.tools = Vec::new();
                final_req.temperature = req.temperature.or(self.inner.config.default_temperature);
                final_req.max_tokens = req.max_tokens.or(self.inner.config.default_max_tokens);
                final_req.thinking_budget_tokens = req
                    .thinking_budget_tokens
                    .or(active_settings.thinking_budget_tokens)
                    .or(if active_settings.reasoning_mode { Some(1024) } else { None });
                final_req.stream = true;
                final_req.metadata = meta_json.clone();

                struct SynthAbortOnDrop<T>(Option<tokio::task::JoinHandle<T>>);
                impl<T> Drop for SynthAbortOnDrop<T> {
                    fn drop(&mut self) {
                        if let Some(h) = self.0.take() {
                            h.abort();
                        }
                    }
                }

                let (ptx, mut prx) = mpsc::channel::<ProviderStreamEvent>(64);
                let tx_stream = tx.clone();
                let mut forwarder_guard = SynthAbortOnDrop(Some(tokio::spawn(async move {
                    let mut stream_events = Vec::new();
                    let mut acc = String::new();
                    loop {
                        let sev_opt = tokio::select! {
                            biased;
                            _ = tx_stream.closed() => None,
                            sev = prx.recv() => sev,
                        };
                        let Some(sev) = sev_opt else {
                            break;
                        };
                        match sev {
                            ProviderStreamEvent::ThinkingDelta(t) => {
                                let ev = HermesEvent::ThinkingDelta { text: t };
                                stream_events.push(ev.clone());
                                if tx_stream.send(ev).await.is_err() {
                                    break;
                                }
                            }
                            ProviderStreamEvent::TextDelta(t) => {
                                acc.push_str(&t);
                                let ev = HermesEvent::TextDelta {
                                    text: t,
                                    accumulated: acc.clone(),
                                };
                                stream_events.push(ev.clone());
                                if tx_stream.send(ev).await.is_err() {
                                    break;
                                }
                            }
                            ProviderStreamEvent::ToolCallReady(_)
                            | ProviderStreamEvent::Usage(_) => {}
                        }
                    }
                    stream_events
                })));

                let synth_res = tokio::select! {
                    biased;
                    _ = tx.closed() => {
                        cancel_flag.store(true, Ordering::SeqCst);
                        Err(providers::ProviderError::Transport("Stream client disconnected".to_string()))
                    }
                    res = self.inner.providers.complete(final_req, Some(ptx)) => res,
                };
                let streamed_evs = if let Some(h) = forwarder_guard.0.take() {
                    h.await.unwrap_or_default()
                } else {
                    Vec::new()
                };
                recorded.extend(streamed_evs);

                match synth_res {
                    Ok(synth_resp) => {
                        iterations_executed += 1;
                        total_usage.accumulate(&synth_resp.usage);
                        let _ = self.inner.store.record_token_usage(
                            session_id,
                            synth_resp.usage.input_tokens as i64,
                            synth_resp.usage.output_tokens as i64,
                            synth_resp.usage.estimated_cost_usd,
                        );
                        if let Some(ref r_text) = synth_resp.reasoning {
                            combined_reasoning = Some(match combined_reasoning {
                                Some(prev) => format!("{prev}\n{r_text}"),
                                None => r_text.clone(),
                            });
                        }
                        final_response_text = if synth_resp.content.trim().is_empty() {
                            format!(
                                "Completed {tool_calls_count} tool call(s) across {iterations_executed} step(s)."
                            )
                        } else {
                            synth_resp.content.clone()
                        };
                        let _ = self.inner.store.append_message(AppendMessageRequest {
                            id: None,
                            session_id: session_id.to_string(),
                            role: "assistant".into(),
                            content: final_response_text.clone(),
                            reasoning: synth_resp.reasoning.clone(),
                            tool_calls_json: None,
                            tool_call_id: None,
                            tool_name: None,
                            token_count: estimate_tokens(&final_response_text) as i64,
                            created_at: None,
                        });
                    }
                    Err(err) => {
                        final_response_text = format!(
                            "Completed {tool_calls_count} tool call(s); grace synthesis encountered an upstream error: {err}"
                        );
                        let _ = self.inner.store.append_message(AppendMessageRequest {
                            id: None,
                            session_id: session_id.to_string(),
                            role: "assistant".into(),
                            content: final_response_text.clone(),
                            reasoning: None,
                            tool_calls_json: None,
                            tool_call_id: None,
                            tool_name: None,
                            token_count: estimate_tokens(&final_response_text) as i64,
                            created_at: None,
                        });
                    }
                }
            }
        }

        let done_ev = HermesEvent::Done {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            response_text: final_response_text,
            reasoning: combined_reasoning,
            iterations: iterations_executed,
            tool_calls_count,
            usage: total_usage,
            buttons: collected_buttons,
        };
        recorded.push(done_ev.clone());
        let _ = tx.send(done_ev).await;
    }

    pub fn assemble_prompt_for_session(
        &self,
        session_id: &str,
        chat_id: Option<i64>,
        bot_id: Option<&str>,
        persona_override: Option<&str>,
        ephemeral_prefetch: &[String],
    ) -> AssembledPrompt {
        self.assemble_prompt_for_session_with_name(
            session_id,
            chat_id,
            bot_id,
            None,
            persona_override,
            ephemeral_prefetch,
        )
    }

    pub fn assemble_prompt_for_session_with_name(
        &self,
        session_id: &str,
        chat_id: Option<i64>,
        bot_id: Option<&str>,
        bot_name_hint: Option<&str>,
        persona_override: Option<&str>,
        ephemeral_prefetch: &[String],
    ) -> AssembledPrompt {
        let ws_ctx: Option<DiscoveredWorkspaceContext> =
            WorkspaceContextDiscovery::discover(&self.inner.config.workspace_root, None).ok();
        let mem_snap = self
            .inner
            .memory
            .get_or_capture_session_snapshot(session_id)
            .ok();

        let mut soul = if let Some(override_txt) = persona_override.map(str::trim).filter(|s| !s.is_empty()) {
            let mut parsed = SoulProfile::parse(override_txt);
            // Preserve base Hermes directives when override only customizes bot persona/instructions;
            // Tier-1 Operational & Self-Learning Doctrine is also injected unconditionally by PromptBuilder.
            if parsed.directives.is_empty() {
                parsed.directives = SoulProfile::default().directives;
            }
            parsed
        } else {
            let stored = self.inner.memory.read_soul().ok();
            let ws_soul = ws_ctx
                .as_ref()
                .and_then(|w| w.soul_markdown())
                .map(SoulProfile::parse);
            match (stored, ws_soul) {
                (Some(s), _) if !s.raw_markdown.trim().is_empty() && s.name != "Hermes" => s,
                (_, Some(w)) => w,
                (Some(s), _) => s,
                (None, None) => SoulProfile::default(),
            }
        };
        if let Some(hint) = bot_name_hint.map(str::trim).filter(|s| !s.is_empty()) {
            if soul.name == "Hermes" || soul.name.trim().is_empty() {
                soul.name = hint.to_string();
            }
        }

        let user_id = chat_id
            .map(|c| format!("chat_{c}"))
            .unwrap_or_else(|| "default_user".into());
        let honcho_summary = self.inner.honcho.render_prompt_summary(&user_id).ok().filter(|s| !s.trim().is_empty());

        let reg_arc = self.session_toolset_registry(session_id);
        let active_toolsets: Vec<String> = reg_arc
            .read()
            .map(|r| {
                r.enabled_toolsets
                    .iter()
                    .map(|t| t.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default();

        let settings = self.session_settings(session_id);
        let approval_mode = self
            .inner
            .security
            .read()
            .map(|s| s.approvals.get_session_mode(session_id))
            .unwrap_or(ApprovalMode::Prompt);
        let (_, model, _) = self.inner.providers.active_info();

        PromptBuilder::new().build(PromptBuildInput {
            soul: Some(&soul),
            active_toolsets: &active_toolsets,
            skills: Some(&self.inner.skills),
            memory_snapshot: mem_snap.as_ref(),
            workspace_context: ws_ctx.as_ref(),
            honcho_summary: honcho_summary.as_deref(),
            volatile: VolatileRuntimeContext {
                session_id: session_id.to_string(),
                chat_id,
                bot_id: bot_id.map(ToString::to_string),
                source: "hermes".to_string(),
                model,
                timestamp_utc: None,
                voice_mode: settings.voice_mode,
                verbose_mode: settings.verbose_mode,
                reasoning_mode: settings.reasoning_mode,
                yolo_mode: settings.yolo_mode || approval_mode == ApprovalMode::Yolo,
                ephemeral_prefetch: ephemeral_prefetch.to_vec(),
            },
            enable_prompt_caching: self.inner.config.enable_prompt_caching,
        })
    }

    fn build_provider_metadata(
        &self,
        session_id: &str,
        persona_override: Option<&str>,
        bot_id: Option<&str>,
        bot_name_hint: Option<&str>,
    ) -> Value {
        let mut soul = if let Some(ov) = persona_override.map(str::trim).filter(|s| !s.is_empty()) {
            SoulProfile::parse(ov)
        } else {
            self.inner.memory.read_soul().unwrap_or_default()
        };
        if let Some(hint) = bot_name_hint.map(str::trim).filter(|s| !s.is_empty()) {
            if soul.name == "Hermes" || soul.name.trim().is_empty() {
                soul.name = hint.to_string();
            }
        }
        let snap = self
            .inner
            .memory
            .get_or_capture_session_snapshot(session_id)
            .ok();
        let skills_idx = self.inner.skills.render_progressive_index();
        let bot_name = bot_name_hint
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                let s = soul.name.trim();
                (!s.is_empty() && s != "Hermes").then(|| s.to_string())
            })
            .or_else(|| {
                bot_id
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit()))
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| "Hermes".to_string());

        json!({
            "bot_name": bot_name,
            "soul_persona": soul.persona,
            "memory_snapshot": snap.as_ref().map(|s| s.memory_raw.as_str()).unwrap_or(""),
            "user_snapshot": snap.as_ref().map(|s| s.user_raw.as_str()).unwrap_or(""),
            "skills_index": skills_idx,
        })
    }

    pub async fn run_cron_job_now(&self, job_id: &str) -> Result<CronRunOutcome> {
        let jobs = self.inner.store.list_cron_jobs(false)?;
        let job = jobs
            .into_iter()
            .find(|j| j.id == job_id)
            .ok_or_else(|| TelegramError::NotFound(format!("Cron job '{job_id}' not found")))?;

        let start = Instant::now();
        let run_at = Utc::now().timestamp_millis();
        let req = HermesTurnRequest {
            session_id: Some(format!("cron_{}", job.id)),
            chat_id: job.target_chat_id.or(job.origin_chat_id),
            bot_id: job.deliver_to.clone(),
            source: "cron".to_string(),
            prompt: job.prompt.clone(),
            model: job.model.clone(),
            skip_slash_commands: true,
            ..Default::default()
        };

        let result = self.run_turn(req).await;
        let duration_ms = start.elapsed().as_millis() as i64;
        let next_run_at = ToolContextBridge::resolve_cron_next_run(&job.schedule, job.timezone.as_deref())
            .ok()
            .flatten();

        match result {
            Ok(turn_res) => {
                let preview = truncate_chars(&turn_res.response_text, 240);
                let _ = self.inner.store.record_cron_run(RecordCronRunRequest {
                    job_id: job.id.clone(),
                    run_at: Some(run_at),
                    status: "completed".to_string(),
                    output_preview: Some(preview),
                    error: None,
                    duration_ms,
                    next_run_at,
                });
                Ok(CronRunOutcome {
                    job_id: job.id,
                    job_name: job.name,
                    status: "completed".to_string(),
                    response_text: turn_res.response_text,
                    target_chat_id: job.target_chat_id.or(job.origin_chat_id),
                    next_run_at,
                    duration_ms,
                })
            }
            Err(e) => {
                let _ = self.inner.store.record_cron_run(RecordCronRunRequest {
                    job_id: job.id.clone(),
                    run_at: Some(run_at),
                    status: "failed".to_string(),
                    output_preview: None,
                    error: Some(e.to_string()),
                    duration_ms,
                    next_run_at,
                });
                Err(e)
            }
        }
    }

    pub async fn run_due_cron_jobs(&self) -> Result<Vec<CronRunOutcome>> {
        let now_ms = Utc::now().timestamp_millis();
        let due = self.inner.store.list_due_cron_jobs(now_ms)?;
        let mut outcomes = Vec::new();
        for job in due {
            match self.run_cron_job_now(&job.id).await {
                Ok(out) => outcomes.push(out),
                Err(e) => outcomes.push(CronRunOutcome {
                    job_id: job.id,
                    job_name: job.name,
                    status: "failed".to_string(),
                    response_text: e.to_string(),
                    target_chat_id: job.target_chat_id.or(job.origin_chat_id),
                    next_run_at: None,
                    duration_ms: 0,
                }),
            }
        }
        Ok(outcomes)
    }

    pub async fn tick_heartbeat_if_due(&self) -> Result<Option<HeartbeatTickOutcome>> {
        let now_ms = Utc::now().timestamp_millis();
        if !self.inner.heartbeat.is_due(now_ms) {
            return Ok(None);
        }
        let cfg = self.inner.heartbeat.status();
        let res = self
            .run_turn(HermesTurnRequest {
                session_id: Some("hermes_heartbeat".to_string()),
                chat_id: cfg.target_chat_id,
                source: "heartbeat".to_string(),
                prompt: cfg.prompt.clone(),
                skip_slash_commands: true,
                max_iterations: Some(3),
                ..Default::default()
            })
            .await?;

        let suppressed = HeartbeatManager::is_heartbeat_ok(&res.response_text);
        self.inner.heartbeat.record_tick(now_ms, suppressed);
        Ok(Some(HeartbeatTickOutcome {
            executed_at_ms: now_ms,
            suppressed,
            response_text: res.response_text,
            target_chat_id: cfg.target_chat_id,
        }))
    }

    /// Standard Agent Client Protocol (ACP) JSON-RPC 2.0 dispatcher.
    pub async fn acp_json_rpc(&self, rpc_req: Value) -> Value {
        self.acp_json_rpc_scoped(rpc_req, None).await
    }

    /// Caller-scoped Agent Client Protocol (ACP) JSON-RPC 2.0 dispatcher.
    pub async fn acp_json_rpc_scoped(&self, rpc_req: Value, scope: Option<AcpCallerScope>) -> Value {
        let id = rpc_req.get("id").cloned().unwrap_or(Value::Null);
        let method = rpc_req
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let params = rpc_req.get("params").cloned().unwrap_or(json!({}));

        match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "1.0",
                    "serverInfo": {
                        "name": "hermes-rust-agent",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": {
                        "sessions": true,
                        "tools": true,
                        "slashCommands": self.inner.commands.count(),
                        "streaming": true,
                    }
                }
            }),
            "session/new" => {
                let title = params.get("title").and_then(|v| v.as_str()).map(ToString::to_string);
                let chat_id = params.get("chat_id").and_then(|v| v.as_i64());
                let bot_id = params.get("bot_id").and_then(|v| v.as_str()).map(ToString::to_string);
                if let Some(ref sc) = scope {
                    if let Some(cid) = chat_id {
                        if !sc.allows_chat(cid) {
                            return rpc_error(id, -32003, "Forbidden: caller is not an active member of target chat");
                        }
                    }
                }
                let metadata = scope
                    .as_ref()
                    .map(|sc| json!({ "owner_user_id": sc.user_id }));
                match self.inner.store.create_session(CreateSessionRequest {
                    chat_id,
                    bot_id,
                    title,
                    source: Some("acp".into()),
                    metadata,
                    ..Default::default()
                }) {
                    Ok(sess) => json!({ "jsonrpc": "2.0", "id": id, "result": { "session_id": sess.id, "session": sess } }),
                    Err(e) => rpc_error(id, -32000, &e.to_string()),
                }
            }
            "session/list" => {
                let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
                match self.inner.store.list_sessions(SessionListFilter {
                    limit: if scope.is_some() { None } else { Some(limit) },
                    ..Default::default()
                }) {
                    Ok(mut list) => {
                        if let Some(ref sc) = scope {
                            list.retain(|s| sc.allows_session(s));
                            list.truncate(limit);
                        }
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "sessions": list } })
                    }
                    Err(e) => rpc_error(id, -32000, &e.to_string()),
                }
            }
            "session/prompt" => {
                let prompt = params
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut session_id = params
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string);
                let chat_id = params.get("chat_id").and_then(|v| v.as_i64());
                let bot_id = params.get("bot_id").and_then(|v| v.as_str()).map(ToString::to_string);
                let model = params
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
                let mut req_user_id = None;
                let mut caller_allowed_chat_ids = None;
                let mut caller_is_admin = false;
                let mut caller_owns_bot = None;
                if let Some(ref sc) = scope {
                    if self.bound_agent_id().is_some()
                        && !sc.allows_bot_config()
                        && commands::is_bot_config_command(&prompt)
                    {
                        return rpc_error(
                            id,
                            -32003,
                            "Forbidden: only bot owner or server admin can run bot config/memory/soul/skill commands",
                        );
                    }
                    if let Some(cid) = chat_id {
                        if !sc.allows_chat(cid) {
                            return rpc_error(id, -32003, "Forbidden: caller is not an active member of target chat");
                        }
                    }
                    if let Some(ref sid) = session_id {
                        if let Ok(Some(existing)) = self.inner.store.get_session(sid) {
                            if !sc.allows_session(&existing) {
                                return rpc_error(id, -32003, "Forbidden: caller does not own target session");
                            }
                            if chat_id.is_some() && existing.chat_id.is_some() && existing.chat_id != chat_id {
                                return rpc_error(id, -32003, "Forbidden: session chat_id mismatch");
                            }
                        }
                    } else if chat_id.is_none() {
                        session_id = Some(format!("user_{}_acp_default", sc.user_id));
                    }
                    req_user_id = Some(sc.user_id.clone());
                    caller_allowed_chat_ids = Some(sc.allowed_chat_ids.clone());
                    caller_is_admin = sc.is_admin;
                    caller_owns_bot = Some(sc.allows_bot_config());
                }
                match self
                    .run_turn(HermesTurnRequest {
                        session_id,
                        chat_id,
                        bot_id,
                        user_id: req_user_id,
                        caller_allowed_chat_ids,
                        caller_is_admin,
                        caller_owns_bot,
                        source: "acp".to_string(),
                        prompt,
                        model,
                        ..Default::default()
                    })
                    .await
                {
                    Ok(res) => json!({ "jsonrpc": "2.0", "id": id, "result": res }),
                    Err(e) => rpc_error(id, -32000, &e.to_string()),
                }
            }
            "session/cancel" => {
                let sid = params.get("session_id").and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
                if let Some(ref sc) = scope {
                    if sid.is_empty() {
                        return rpc_error(id, -32602, "session_id is required");
                    }
                    match self.inner.store.get_session(sid) {
                        Ok(Some(sess)) => {
                            if !sc.allows_session(&sess) {
                                return rpc_error(id, -32003, "Forbidden: caller does not own target session");
                            }
                        }
                        Ok(None) => {
                            return rpc_error(id, -32003, "Forbidden: unknown session_id");
                        }
                        Err(e) => return rpc_error(id, -32000, &e.to_string()),
                    }
                }
                let cancelled = self.cancel_session(sid);
                json!({ "jsonrpc": "2.0", "id": id, "result": { "session_id": sid, "cancelled": cancelled } })
            }
            "tools/list" => {
                let specs: Vec<HermesToolSpec> = if let Some(ref sc) = scope {
                    if let Some(sid) = params
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        if let Ok(Some(sess)) = self.inner.store.get_session(sid) {
                            if !sc.allows_session(&sess) {
                                return rpc_error(id, -32003, "Forbidden: caller cannot inspect session tools");
                            }
                        }
                        let reg = self.session_toolset_registry(sid);
                        reg.read()
                            .map(|r| r.active_specs())
                            .unwrap_or_else(|_| toolsets::all_builtin_tool_specs())
                    } else {
                        self.inner
                            .toolsets
                            .read()
                            .map(|r| r.active_specs())
                            .unwrap_or_else(|_| toolsets::all_builtin_tool_specs())
                    }
                } else {
                    let sid = params.get("session_id").and_then(|v| v.as_str()).unwrap_or("default_session");
                    let reg = self.session_toolset_registry(sid);
                    reg.read()
                        .map(|r| r.active_specs())
                        .unwrap_or_else(|_| toolsets::all_builtin_tool_specs())
                };
                json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": specs } })
            }
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let (sid, ctx_chat_id, uid_owned, allowed_chats, is_admin, can_manage_bot): (
                    String,
                    Option<i64>,
                    Option<String>,
                    Option<HashSet<i64>>,
                    bool,
                    bool,
                ) = if let Some(ref sc) = scope {
                    if self.bound_agent_id().is_some()
                        && !sc.allows_bot_config()
                        && matches!(name, "memory" | "skill_manage" | "skill_list" | "skill_view")
                    {
                        return rpc_error(
                            id,
                            -32003,
                            "Forbidden: only bot owner or server admin can access or modify bot memory or skills",
                        );
                    }
                    if let Some(val) = params.get("user_id").filter(|v| !v.is_null()) {
                        let req_uid_str = val
                            .as_str()
                            .map(str::trim)
                            .map(ToString::to_string)
                            .or_else(|| val.as_i64().map(|n| n.to_string()))
                            .or_else(|| val.as_u64().map(|n| n.to_string()))
                            .unwrap_or_default();
                        if !req_uid_str.is_empty() && !sc.is_admin {
                            let matches_caller = req_uid_str == sc.user_id
                                || parse_numeric_user_id(Some(&req_uid_str))
                                    .is_some_and(|n| n.to_string() == sc.user_id);
                            if !matches_caller {
                                return rpc_error(id, -32003, "Caller cannot impersonate another user_id");
                            }
                        }
                    }
                    let params_chat_id = if let Some(val) = params.get("chat_id").filter(|v| !v.is_null()) {
                        let Some(cid) = val.as_i64() else {
                            return rpc_error(id, -32602, "Invalid integer chat_id");
                        };
                        if !sc.allows_chat(cid) {
                            return rpc_error(id, -32003, "Forbidden: caller is not an active member of target chat");
                        }
                        Some(cid)
                    } else {
                        None
                    };
                    if let Some(arg_cid_val) = args.get("chat_id").filter(|v| !v.is_null()) {
                        let Some(arg_cid) = arg_cid_val.as_i64() else {
                            return rpc_error(id, -32602, "Invalid integer arguments.chat_id");
                        };
                        if arg_cid > 0 && !sc.allows_chat(arg_cid) {
                            return rpc_error(id, -32003, "Forbidden: caller is not an active member of target chat");
                        }
                    }
                    if let Some(target_cid_val) = args.get("target_chat_id").filter(|v| !v.is_null()) {
                        let Some(target_cid) = target_cid_val.as_i64() else {
                            return rpc_error(id, -32602, "Invalid integer arguments.target_chat_id");
                        };
                        if !sc.allows_chat(target_cid) {
                            return rpc_error(id, -32003, "Forbidden: caller is not an active member of target_chat_id");
                        }
                    }
                    let raw_sid = params
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    let (eff_sid, eff_cid) = if let Some(s) = raw_sid {
                        if let Ok(Some(sess)) = self.inner.store.get_session(s) {
                            if !sc.allows_session(&sess) {
                                return rpc_error(id, -32003, "Forbidden: caller does not own target session");
                            }
                            if params_chat_id.is_some()
                                && sess.chat_id.is_some()
                                && sess.chat_id != params_chat_id
                            {
                                return rpc_error(id, -32003, "Forbidden: session chat_id mismatch");
                            }
                            (s.to_string(), sess.chat_id.or(params_chat_id))
                        } else {
                            (s.to_string(), params_chat_id)
                        }
                    } else {
                        (format!("user_{}_acp_default", sc.user_id), params_chat_id)
                    };
                    (
                        eff_sid,
                        eff_cid,
                        Some(sc.user_id.clone()),
                        Some(sc.allowed_chat_ids.clone()),
                        sc.is_admin,
                        sc.allows_bot_config(),
                    )
                } else {
                    let sid = params
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default_session")
                        .to_string();
                    let uid = params
                        .get("user_id")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);
                    let cid = params.get("chat_id").and_then(|v| v.as_i64());
                    (sid, cid, uid, None, false, true)
                };
                let ctx = self.build_tool_context_scoped(
                    &sid,
                    ctx_chat_id,
                    None,
                    uid_owned.as_deref(),
                    1,
                    None,
                    0,
                    Arc::new(AtomicBool::new(false)),
                    allowed_chats,
                    is_admin,
                    can_manage_bot,
                );
                let res = HermesToolExecutor::execute(name, args, &ctx).await;
                json!({ "jsonrpc": "2.0", "id": id, "result": res })
            }
            "command/execute" => {
                let text = params.get("command").and_then(|v| v.as_str()).unwrap_or("");
                let raw_sid = params
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let chat_id = params.get("chat_id").and_then(|v| v.as_i64());
                let bot_id = params.get("bot_id").and_then(|v| v.as_str());
                let mut owned_sid: Option<String> = raw_sid.map(ToString::to_string);
                if let Some(ref sc) = scope {
                    if (self.bound_agent_id().is_some() || bot_id.is_some())
                        && !sc.allows_bot_config()
                        && commands::is_bot_config_command(text)
                    {
                        return rpc_error(
                            id,
                            -32003,
                            "Forbidden: only bot owner or server admin can run bot config/memory/soul/skill commands",
                        );
                    }
                    if let Some(cid) = chat_id {
                        if !sc.allows_chat(cid) {
                            return rpc_error(id, -32003, "Forbidden: caller is not an active member of target chat");
                        }
                    }
                    if let Some(s) = raw_sid {
                        if let Ok(Some(sess)) = self.inner.store.get_session(s) {
                            if !sc.allows_session(&sess) {
                                return rpc_error(id, -32003, "Forbidden: caller does not own target session");
                            }
                            if chat_id.is_some() && sess.chat_id != chat_id {
                                return rpc_error(id, -32003, "Forbidden: session chat_id mismatch");
                            }
                        }
                    } else if chat_id.is_none() {
                        owned_sid = Some(format!("user_{}_acp_default", sc.user_id));
                    }
                }
                match self.execute_command_scoped(owned_sid.as_deref(), chat_id, bot_id, scope, text) {
                    Ok((resolved_sid, out)) => {
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "session_id": resolved_sid, "outcome": out } })
                    }
                    Err(TelegramError::PermissionDenied(msg)) => rpc_error(id, -32003, &msg),
                    Err(e) => rpc_error(id, -32000, &e.to_string()),
                }
            }
            _ => rpc_error(id, -32601, &format!("Method '{method}' not found")),
        }
    }
}

// ============================================================================
// Private Helpers
// ============================================================================

async fn emit_event(
    tx: &mpsc::Sender<HermesEvent>,
    log: &mut Vec<HermesEvent>,
    ev: HermesEvent,
) {
    log.push(ev.clone());
    let _ = tx.send(ev).await;
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    })
}

fn scope_key_for(chat_id: i64, bot_id: &str) -> String {
    format!("{chat_id}:{bot_id}")
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn tiers_from_assembled(assembled: &AssembledPrompt) -> Vec<SystemPromptTier> {
    let mut tiers = Vec::new();
    if !assembled.tier1_stable.content.trim().is_empty() {
        tiers.push(SystemPromptTier {
            label: "tier1_stable".into(),
            text: assembled.tier1_stable.content.clone(),
            cache_ephemeral: assembled.tier1_stable.cache_control.is_some(),
        });
    }
    if !assembled.tier2_context.content.trim().is_empty() {
        tiers.push(SystemPromptTier {
            label: "tier2_context".into(),
            text: assembled.tier2_context.content.clone(),
            cache_ephemeral: assembled.tier2_context.cache_control.is_some(),
        });
    }
    if !assembled.tier3_volatile.content.trim().is_empty() {
        tiers.push(SystemPromptTier::volatile(
            "tier3_volatile",
            assembled.tier3_volatile.content.clone(),
        ));
    }
    tiers
}

fn records_to_provider_messages(
    records: &[MessageRecord],
    current_user_msg_id: &str,
    expanded_current_prompt: &str,
) -> Vec<ProviderChatMessage> {
    let mut out = Vec::with_capacity(records.len());
    for rec in records {
        let content = if rec.id == current_user_msg_id {
            expanded_current_prompt.to_string()
        } else {
            rec.content.clone()
        };
        match rec.role.as_str() {
            "system" => out.push(ProviderChatMessage {
                role: ProviderMessageRole::System,
                content,
                reasoning: None,
                tool_calls: Vec::new(),
                tool_call_id: None,
                name: None,
                cache_control: None,
            }),
            "user" => out.push(ProviderChatMessage::user(content)),
            "tool" => out.push(ProviderChatMessage::tool_result(
                rec.tool_call_id.clone().unwrap_or_default(),
                rec.tool_name.clone().unwrap_or_else(|| "tool".into()),
                content,
            )),
            "assistant" => {
                let tool_calls: Vec<ProviderToolCall> = rec
                    .tool_calls_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                if tool_calls.is_empty() {
                    let mut msg = ProviderChatMessage::assistant(content);
                    msg.reasoning = rec.reasoning.clone();
                    out.push(msg);
                } else {
                    out.push(ProviderChatMessage::assistant_with_tools(
                        content,
                        tool_calls,
                        rec.reasoning.clone(),
                    ));
                }
            }
            _ => out.push(ProviderChatMessage::user(content)),
        }
    }
    out
}

const GRACE_SYNTHESIS_NOTICE: &str = "[System Notice] Tool iteration budget reached. Do NOT call any more tools; synthesize a thorough, accurate final response summarizing everything accomplished and observed so far.";
const EMPTY_RESPONSE_RECOVERY_NUDGE: &str = "You just completed tool execution above; now provide a clear, complete final response based on the results.";

fn is_placeholder_default_model(model: &str) -> bool {
    let trimmed = model.trim();
    trimmed.is_empty()
        || matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "deepseek-flash" | "hermes-local-v1" | "default" | "auto"
        )
        || trimmed == "默认模型"
}

fn sha256_hex(input: &str) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(input.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn build_non_fatal_tool_error_envelope(tool_name: &str, err_msg: &str) -> String {
    json!({
        "ok": false,
        "tool": tool_name,
        "error": err_msg,
        "hint": "Fix the arguments or choose another approach and continue."
    })
    .to_string()
}

fn strip_markdown_code_fence(input: &str) -> &str {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let after_lang = rest
            .find('\n')
            .map(|idx| &rest[idx + 1..])
            .unwrap_or(rest);
        if let Some(end_idx) = after_lang.rfind("```") {
            return after_lang[..end_idx].trim();
        }
        return after_lang.trim();
    }
    if let Some(fence_start) = trimmed.find("```") {
        let after_open = &trimmed[fence_start + 3..];
        if let Some(nl_idx) = after_open.find('\n') {
            let body = &after_open[nl_idx + 1..];
            if let Some(end_idx) = body.rfind("```") {
                return body[..end_idx].trim();
            }
        }
    }
    trimmed
}

fn extract_outer_json_span(input: &str) -> Option<&str> {
    let mut start_byte: Option<usize> = None;
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for (byte_idx, ch) in input.char_indices() {
        if start_byte.is_none() {
            if ch == '{' {
                start_byte = Some(byte_idx);
                stack.push('}');
            } else if ch == '[' {
                start_byte = Some(byte_idx);
                stack.push(']');
            }
            continue;
        }

        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                if stack.last().copied() == Some(ch) {
                    stack.pop();
                    if stack.is_empty() {
                        if let Some(start) = start_byte {
                            return Some(&input[start..byte_idx + ch.len_utf8()]);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Fallback to widest open..close span if braces were slightly imbalanced.
    if let Some(start) = start_byte {
        let first_ch = input[start..].chars().next()?;
        let close_ch = if first_ch == '{' { '}' } else { ']' };
        if let Some(end) = input.rfind(close_ch) {
            if end > start {
                return Some(&input[start..=end]);
            }
        }
    }
    None
}

fn strip_trailing_json_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && matches!(chars[j], '}' | ']') {
                i += 1;
                continue;
            }
        }
        out.push(ch);
        i += 1;
    }

    out
}

fn repair_tool_call_arguments_with_depth(
    raw: &str,
    depth: usize,
) -> std::result::Result<Value, String> {
    if depth > 2 {
        return Err("Nested JSON string exceeded repair depth limit".to_string());
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }

    let resolve_parsed = |val: Value| -> std::result::Result<Value, String> {
        match val {
            Value::Null => Ok(json!({})),
            Value::String(inner) => {
                let inner_trimmed = inner.trim();
                if inner_trimmed.is_empty() {
                    Ok(json!({}))
                } else if inner_trimmed.starts_with('{')
                    || inner_trimmed.starts_with('[')
                    || inner_trimmed.starts_with("```")
                {
                    repair_tool_call_arguments_with_depth(inner_trimmed, depth + 1)
                } else {
                    Err("Tool arguments string did not contain a JSON object".to_string())
                }
            }
            other => Ok(other),
        }
    };

    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return resolve_parsed(v);
    }

    let unfenced = strip_markdown_code_fence(trimmed);
    if let Ok(v) = serde_json::from_str::<Value>(unfenced) {
        return resolve_parsed(v);
    }

    let cleaned_unfenced = strip_trailing_json_commas(unfenced);
    if let Ok(v) = serde_json::from_str::<Value>(&cleaned_unfenced) {
        return resolve_parsed(v);
    }

    if let Some(span) = extract_outer_json_span(unfenced) {
        if let Ok(v) = serde_json::from_str::<Value>(span) {
            return resolve_parsed(v);
        }
        let cleaned_span = strip_trailing_json_commas(span);
        if let Ok(v) = serde_json::from_str::<Value>(&cleaned_span) {
            return resolve_parsed(v);
        }
    }

    Err(serde_json::from_str::<Value>(trimmed)
        .expect_err("failed parse after repair fallback")
        .to_string())
}

pub(crate) fn repair_tool_call_arguments(raw: &str) -> std::result::Result<Value, String> {
    repair_tool_call_arguments_with_depth(raw, 0)
}

fn write_canonical_json(val: &Value, out: &mut String) {
    match val {
        Value::Object(map) => {
            out.push('{');
            let mut entries: Vec<(&str, &Value)> =
                map.iter().map(|(k, v)| (k.as_str(), v)).collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (idx, (k, v)) in entries.into_iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_else(|_| format!("\"{k}\"")));
                out.push(':');
                write_canonical_json(v, out);
            }
            out.push('}');
        }
        Value::Array(arr) => {
            out.push('[');
            for (idx, item) in arr.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                write_canonical_json(item, out);
            }
            out.push(']');
        }
        scalar => {
            out.push_str(&serde_json::to_string(scalar).unwrap_or_else(|_| "null".into()));
        }
    }
}

pub(crate) fn canonicalize_json(val: &Value) -> String {
    let mut out = String::new();
    write_canonical_json(val, &mut out);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolCallSignature {
    tool_name: String,
    canonical_args: String,
    result_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct GuardrailEval {
    stubbed_output: Option<String>,
    warning_messages: Vec<String>,
    short_circuit: bool,
}

#[derive(Debug, Default)]
struct ToolGuardrailTracker {
    history: Vec<ToolCallSignature>,
    seen_large_outputs: HashMap<String, (String, usize)>,
}

impl ToolGuardrailTracker {
    fn record_and_evaluate(
        &mut self,
        call_id: &str,
        tool_name: &str,
        canonical_args: &str,
        raw_output: &str,
    ) -> GuardrailEval {
        let mut eval = GuardrailEval::default();
        let result_hash = sha256_hex(raw_output);
        let char_count = raw_output.chars().count();

        if char_count >= 512 {
            if let Some((orig_call_id, orig_chars)) = self.seen_large_outputs.get(&result_hash) {
                let short_hash = &result_hash[..12.min(result_hash.len())];
                eval.stubbed_output = Some(format!(
                    "[Duplicate output stub: {orig_chars} chars identical to earlier call_id `{orig_call_id}` (sha256:{short_hash}). Refer to `{orig_call_id}` above.]"
                ));
            } else {
                self.seen_large_outputs
                    .insert(result_hash.clone(), (call_id.to_string(), char_count));
            }
        }

        self.history.push(ToolCallSignature {
            tool_name: tool_name.to_string(),
            canonical_args: canonical_args.to_string(),
            result_hash,
        });

        let last_sig = self.history.last().expect("pushed signature");
        let streak = self
            .history
            .iter()
            .rev()
            .take_while(|sig| *sig == last_sig)
            .count();

        if streak >= 5 {
            eval.warning_messages.push(
                "[Guardrail Warning: Identical tool call repeated 5 times with identical results. Stopping tool loop and synthesizing final response.]"
                    .to_string(),
            );
            eval.short_circuit = true;
            return eval;
        }

        if streak == 3 {
            eval.warning_messages.push(
                "[Guardrail Warning: You called this exact tool with identical arguments and got the same result 3 times. Change approach or finish your answer.]"
                    .to_string(),
            );
        }

        if let Some((period, repeats)) = detect_multi_tool_cycle(&self.history) {
            if repeats >= 3 {
                eval.warning_messages.push(format!(
                    "[Guardrail Warning: Detected a repeating {period}-step tool call cycle ({repeats} repetitions). Stopping runaway loop and synthesizing final response.]"
                ));
                eval.short_circuit = true;
            } else {
                eval.warning_messages.push(format!(
                    "[Guardrail Warning: Detected a repeating {period}-step tool call cycle ({repeats} repetitions). Break the cycle by changing approach or finishing your answer.]"
                ));
            }
        }

        eval
    }
}

fn detect_multi_tool_cycle(history: &[ToolCallSignature]) -> Option<(usize, usize)> {
    let n = history.len();
    for period in 2..=4 {
        if n < period * 2 {
            continue;
        }
        let pattern = &history[n - period..n];
        if pattern.iter().all(|sig| sig == &pattern[0]) {
            continue;
        }
        let mut repeats = 1usize;
        let mut offset = period;
        while n >= offset + period {
            if &history[n - offset - period..n - offset] == pattern {
                repeats += 1;
                offset += period;
            } else {
                break;
            }
        }
        if repeats >= 2 {
            return Some((period, repeats));
        }
    }
    None
}

// ============================================================================
// End-to-End Engine Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_engine() -> (TempDir, HermesEngine) {
        let dir = TempDir::new().expect("create temp workspace");
        let cfg = HermesEngineConfig {
            workspace_root: dir.path().to_path_buf(),
            data_dir: Some(dir.path().join(".hermes_data")),
            default_model: "hermes-local-v1".into(),
            ..Default::default()
        };
        let engine = HermesEngine::new(cfg).expect("init HermesEngine");
        let _ = engine
            .providers()
            .set_active("local_hermes", Some("hermes-local-v1"));
        (dir, engine)
    }

    #[tokio::test]
    async fn test_deterministic_chat_bot_session_continuity_and_custom_persona() {
        let (_dir, engine) = test_engine();
        let req1 = HermesTurnRequest::new("Hello there, introduce yourself briefly")
            .with_chat_bot(101, "bot_alpha")
            .with_system_prompt("# Alpha Navigator\n- Name: Alpha Navigator\n- Role: Research Guide\n## Persona\nAn analytical navigator.");
        let res1 = engine.run_turn(req1).await.expect("first turn ok");
        assert!(!res1.session_id.is_empty());
        assert!(!res1.response_text.contains("大乘"));

        let req2 = HermesTurnRequest::new("Second message in same conversation")
            .with_chat_bot(101, "bot_alpha");
        let res2 = engine.run_turn(req2).await.expect("second turn ok");
        assert_eq!(res1.session_id, res2.session_id, "must reuse deterministic session");

        // Different bot on the same chat gets a distinct session
        let req_other_bot = HermesTurnRequest::new("Hello from beta")
            .with_chat_bot(101, "bot_beta");
        let res_beta = engine.run_turn(req_other_bot).await.expect("beta turn ok");
        assert_ne!(res_beta.session_id, res1.session_id);

        let msgs = engine
            .store()
            .get_messages(&res1.session_id, None)
            .expect("read persisted messages");
        assert_eq!(msgs.len(), 4, "2 user + 2 assistant messages persisted");
    }

    #[tokio::test]
    async fn test_tool_execution_checkpoint_rollback_and_undo() {
        let (dir, engine) = test_engine();
        let target_path = dir.path().join("notes.txt");
        std::fs::write(&target_path, "original content").expect("seed file");

        let write_cmd = format!(
            "@tool:write_file({{\"path\":\"notes.txt\",\"content\":\"modified by hermes\"}})"
        );
        let res = engine
            .run_turn(HermesTurnRequest::new(write_cmd).with_chat_bot(202, "bot_tools"))
            .await
            .expect("tool turn ok");

        assert!(res.tool_calls.iter().any(|t| t.tool_name == "write_file" && t.success));
        assert_eq!(
            std::fs::read_to_string(&target_path).unwrap(),
            "modified by hermes"
        );

        // /rollback restores the original file bytes
        let rb = engine
            .run_turn(HermesTurnRequest::new("/rollback").with_chat_bot(202, "bot_tools"))
            .await
            .expect("rollback command ok");
        assert!(rb.response_text.contains("Rolled back"));
        assert_eq!(
            std::fs::read_to_string(&target_path).unwrap(),
            "original content"
        );

        // /undo pops the last user+tool+assistant turn cleanly
        let before = engine
            .store()
            .get_messages(&res.session_id, None)
            .unwrap()
            .len();
        assert!(before >= 3);
        let undo_res = engine
            .run_turn(HermesTurnRequest::new("/undo").with_chat_bot(202, "bot_tools"))
            .await
            .expect("undo command ok");
        assert!(undo_res.response_text.contains("Removed"));
    }

    #[tokio::test]
    async fn test_slash_commands_moa_goal_and_export() {
        let (_dir, engine) = test_engine();

        // Help & status
        let help = engine
            .run_turn(HermesTurnRequest::new("/help").with_chat_bot(303, "bot_cmd"))
            .await
            .unwrap();
        assert!(help.response_text.contains("61 commands") || help.response_text.contains("/model"));

        // Memory + reload-memory
        let mem = engine
            .run_turn(
                HermesTurnRequest::new("/memory add User prefers concise Rust diffs")
                    .with_chat_bot(303, "bot_cmd"),
            )
            .await
            .unwrap();
        assert!(mem.response_text.contains("MEMORY.md"));
        let reload = engine
            .run_turn(HermesTurnRequest::new("/reload-memory").with_chat_bot(303, "bot_cmd"))
            .await
            .unwrap();
        assert!(reload.response_text.contains("Reloaded frozen memory snapshot"));

        // Mixture of Agents (/moa)
        let moa = engine
            .run_turn(
                HermesTurnRequest::new("/moa Compare SQLite WAL vs rollback journals")
                    .with_chat_bot(303, "bot_cmd"),
            )
            .await
            .unwrap();
        assert!(!moa.response_text.is_empty());

        // Autonomous /goal loop
        let goal = engine
            .run_turn(
                HermesTurnRequest::new("/goal Verify system architecture health")
                    .with_chat_bot(303, "bot_cmd"),
            )
            .await
            .unwrap();
        assert!(!goal.response_text.is_empty());
        assert!(goal
            .events
            .iter()
            .any(|e| matches!(e, HermesEvent::GoalProgress { .. })));

        // Export trajectory with redaction
        let exported = engine
            .run_turn(HermesTurnRequest::new("/export sharegpt").with_chat_bot(303, "bot_cmd"))
            .await
            .unwrap();
        assert!(exported.response_text.contains("Exported"));
    }

    #[tokio::test]
    async fn test_queue_steer_cron_and_acp_rpc() {
        let (_dir, engine) = test_engine();

        let q = engine
            .run_turn(HermesTurnRequest::new("/queue Follow up after current work").with_session("s_q"))
            .await
            .unwrap();
        assert!(q.response_text.contains("Queued message"));

        let queued_res = engine
            .run_next_queued("s_q")
            .await
            .unwrap()
            .expect("queued turn should run");
        assert!(!queued_res.response_text.is_empty());

        let cron_add = engine
            .run_turn(
                HermesTurnRequest::new("/cron add health_check \"@every 1h\" Check workspace")
                    .with_session("s_q"),
            )
            .await
            .unwrap();
        assert!(cron_add.response_text.contains("Created cron job"));

        let rpc = engine
            .acp_json_rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }))
            .await;
        assert_eq!(rpc["result"]["serverInfo"]["name"], "hermes-rust-agent");
    }

    async fn spawn_mock_openai_server(
        responses: Vec<Value>,
    ) -> (
        std::net::SocketAddr,
        Arc<std::sync::Mutex<Vec<Value>>>,
        tokio::task::JoinHandle<()>,
    ) {
        use std::collections::VecDeque;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock openai listener");
        let addr = listener.local_addr().expect("local addr");
        let captured = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let captured_clone = captured.clone();
        let queue = Arc::new(std::sync::Mutex::new(VecDeque::from(responses)));

        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let next_resp = {
                    let mut q = queue.lock().unwrap();
                    q.pop_front()
                };
                let Some(resp_json) = next_resp else {
                    break;
                };
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let mut header_end: Option<usize> = None;
                let mut content_len: usize = 0;

                loop {
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos);
                            let headers = String::from_utf8_lossy(&raw[..pos]);
                            for line in headers.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse::<usize>().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(hend) = header_end {
                        if raw.len() >= hend + 4 + content_len {
                            break;
                        }
                    }
                }

                if let Some(hend) = header_end {
                    let body_end = (hend + 4 + content_len).min(raw.len());
                    if let Ok(req_val) = serde_json::from_slice::<Value>(&raw[hend + 4..body_end]) {
                        captured_clone.lock().unwrap().push(req_val);
                    }
                }

                let body = resp_json.to_string();
                let http = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(http.as_bytes()).await;
                let _ = stream.shutdown().await;

                if queue.lock().unwrap().is_empty() {
                    break;
                }
            }
        });

        (addr, captured, handle)
    }

    fn openai_text_response(text: &str) -> Value {
        json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "finish_reason": "stop",
                "message": { "role": "assistant", "content": text }
            }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        })
    }

    fn openai_tool_call_response(calls: Vec<(&str, &str, &str)>) -> Value {
        let tc_json: Vec<Value> = calls
            .into_iter()
            .map(|(id, name, args)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args }
                })
            })
            .collect();
        json!({
            "id": "chatcmpl-mock-tc",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": tc_json
                }
            }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 8, "total_tokens": 20 }
        })
    }

    #[test]
    fn test_repair_tool_call_arguments_quirks_and_canonical_json() {
        assert_eq!(HermesEngineConfig::default().max_iterations, 30);

        assert_eq!(repair_tool_call_arguments("").unwrap(), json!({}));
        assert_eq!(repair_tool_call_arguments("   \n\t ").unwrap(), json!({}));

        let fenced = "Here you go:\n```json\n{\"b\": [1, 2,], \"a\": \"hi,}\",}\n```\nDone.";
        let parsed = repair_tool_call_arguments(fenced).expect("fence + trailing comma repaired");
        assert_eq!(parsed, json!({ "a": "hi,}", "b": [1, 2] }));
        assert_eq!(
            canonicalize_json(&parsed),
            r#"{"a":"hi,}","b":[1,2]}"#.to_string()
        );

        let array_wrapped = "prefix [\"x\", \"y\",] suffix";
        assert_eq!(
            repair_tool_call_arguments(array_wrapped).expect("outer array repaired"),
            json!(["x", "y"])
        );

        assert!(repair_tool_call_arguments("{unterminated: nope").is_err());
    }

    #[test]
    fn test_tool_guardrail_tracker_streaks_cycles_and_large_output_stubs() {
        let mut tracker = ToolGuardrailTracker::default();
        let big_output = "Z".repeat(600);

        // Call 1: large output cached under c_1
        let e1 = tracker.record_and_evaluate("c_1", "read_file", r#"{"path":"a.txt"}"#, &big_output);
        assert!(e1.stubbed_output.is_none());
        assert!(e1.warning_messages.is_empty());
        assert!(!e1.short_circuit);

        // Call 2: duplicate large output replaced with compact stub pointing to c_1
        let e2 = tracker.record_and_evaluate("c_2", "read_file", r#"{"path":"a.txt"}"#, &big_output);
        let stub = e2.stubbed_output.expect("duplicate >=512 chars must stub");
        assert!(stub.contains("c_1"));
        assert!(stub.contains("600 chars"));

        // Call 3: streak == 3 warning appended
        let e3 = tracker.record_and_evaluate("c_3", "read_file", r#"{"path":"a.txt"}"#, &big_output);
        assert!(e3.stubbed_output.is_some());
        assert!(!e3.short_circuit);
        assert!(e3
            .warning_messages
            .iter()
            .any(|w| w.contains("got the same result 3 times")));

        // Call 4 & 5: streak >= 5 short-circuits cleanly
        let _ = tracker.record_and_evaluate("c_4", "read_file", r#"{"path":"a.txt"}"#, &big_output);
        let e5 = tracker.record_and_evaluate("c_5", "read_file", r#"{"path":"a.txt"}"#, &big_output);
        assert!(e5.short_circuit);

        // Separate tracker: period-2 repeating cycle (A, B, A, B, A, B)
        let mut cycle_tracker = ToolGuardrailTracker::default();
        for i in 1..=3 {
            let ea = cycle_tracker.record_and_evaluate(&format!("a{i}"), "tool_a", "{}", "out_a");
            assert!(!ea.short_circuit);
            let eb = cycle_tracker.record_and_evaluate(&format!("b{i}"), "tool_b", "{}", "out_b");
            if i == 2 {
                assert!(
                    eb.warning_messages
                        .iter()
                        .any(|w| w.contains("2-step tool call cycle")),
                    "expected cycle warning at 2nd repetition"
                );
                assert!(!eb.short_circuit);
            } else if i == 3 {
                assert!(eb.short_circuit, "expected clean loop break on 3rd repetition");
            }
        }
    }

    #[tokio::test]
    async fn test_turn_loop_json_repair_unknown_tool_non_fatal_and_empty_call_id() {
        let (_dir, engine) = test_engine();
        let (addr, captured, server) = spawn_mock_openai_server(vec![
            // Iter 1: empty call_id, malformed JSON + trailing comma + fence for UNKNOWN tool
            openai_tool_call_response(vec![(
                "",
                "nonexistent_tool_xyz",
                "```json\n{\"foo\": 1,}\n```",
            )]),
            // Iter 2: LLM recovers using error envelope and calls valid `think` with empty "" args
            openai_tool_call_response(vec![("", "think", "")]),
            // Iter 3: final answer
            openai_text_response("Recovered from unknown tool error cleanly."),
        ])
        .await;

        engine.providers().register_provider(providers::ProviderConfig::openai_compatible(
            "mock_openai",
            "Mock OpenAI",
            format!("http://{addr}/v1"),
            "mock-gpt",
            "k",
        ));
        let _ = engine.providers().set_active("mock_openai", Some("mock-gpt"));
        engine.providers().set_allow_local_fallback(false);

        let res = engine
            .run_turn(HermesTurnRequest::new("Test unknown tool recovery").with_session("s_repair"))
            .await
            .expect("turn must not fail on unknown tool");
        let _ = server.await;

        assert_eq!(res.response_text, "Recovered from unknown tool error cleanly.");
        assert_eq!(res.tool_calls.len(), 2);
        assert_eq!(res.tool_calls[0].tool_name, "nonexistent_tool_xyz");
        assert!(!res.tool_calls[0].success);

        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        let msgs2 = reqs[1]["messages"].as_array().unwrap();
        let tool_msg = msgs2
            .iter()
            .find(|m| m["role"] == "tool")
            .expect("tool envelope passed back to LLM");
        assert_eq!(tool_msg["tool_call_id"], "call_1_0");
        let env: Value = serde_json::from_str(tool_msg["content"].as_str().unwrap()).unwrap();
        assert_eq!(env["ok"], false);
        assert_eq!(env["tool"], "nonexistent_tool_xyz");
        assert!(env["hint"].as_str().unwrap().contains("Fix the arguments"));
    }

    #[tokio::test]
    async fn test_turn_loop_empty_response_recovery() {
        let (_dir, engine) = test_engine();
        let (addr, captured, server) = spawn_mock_openai_server(vec![
            openai_tool_call_response(vec![("c1", "think", "{\"thought\":\"checking\"}")]),
            // Iter 2: model returns empty content and 0 tool calls
            openai_text_response("   "),
            // Iter 3: after recovery nudge, model provides complete text
            openai_text_response("Synthesized final response after recovery nudge."),
        ])
        .await;

        engine.providers().register_provider(providers::ProviderConfig::openai_compatible(
            "mock_openai",
            "Mock OpenAI",
            format!("http://{addr}/v1"),
            "mock-gpt",
            "k",
        ));
        let _ = engine.providers().set_active("mock_openai", Some("mock-gpt"));
        engine.providers().set_allow_local_fallback(false);

        let res = engine
            .run_turn(HermesTurnRequest::new("Do work").with_session("s_empty_rec"))
            .await
            .expect("turn ok");
        let _ = server.await;

        assert_eq!(
            res.response_text,
            "Synthesized final response after recovery nudge."
        );
        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        let last_msgs = reqs[2]["messages"].as_array().unwrap();
        assert!(last_msgs.iter().any(|m| {
            m["content"]
                .as_str()
                .is_some_and(|c| c.contains("You just completed tool execution above"))
        }));
    }

    #[tokio::test]
    async fn test_turn_loop_grace_synthesis_on_max_iterations() {
        let (_dir, engine) = test_engine();
        let (addr, captured, server) = spawn_mock_openai_server(vec![
            openai_tool_call_response(vec![("t1", "think", "{\"thought\":\"step1\"}")]),
            openai_tool_call_response(vec![("t2", "think", "{\"thought\":\"step2\"}")]),
            // 3rd call is the Grace Synthesis Final Call (`tools` empty)
            openai_text_response("Graceful synthesis summarizing steps 1 and 2."),
        ])
        .await;

        engine.providers().register_provider(providers::ProviderConfig::openai_compatible(
            "mock_openai",
            "Mock OpenAI",
            format!("http://{addr}/v1"),
            "mock-gpt",
            "k",
        ));
        let _ = engine.providers().set_active("mock_openai", Some("mock-gpt"));
        engine.providers().set_allow_local_fallback(false);

        let mut req = HermesTurnRequest::new("Keep digging").with_session("s_grace");
        req.max_iterations = Some(2);
        let res = engine.run_turn(req).await.expect("grace synthesis ok");
        let _ = server.await;

        assert_eq!(
            res.response_text,
            "Graceful synthesis summarizing steps 1 and 2."
        );
        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        let final_req = &reqs[2];
        assert!(
            !final_req["tools"]
                .as_array()
                .is_some_and(|arr| !arr.is_empty()),
            "grace synthesis call must send zero tools: {:?}",
            final_req["tools"]
        );
        assert!(final_req["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"]
                .as_str()
                .is_some_and(|c| c.contains("Tool iteration budget reached"))));
    }

    #[tokio::test]
    async fn test_turn_loop_guardrail_streak_five_short_circuits_into_grace_synthesis() {
        let (dir, engine) = test_engine();
        std::fs::write(dir.path().join("big.txt"), "A".repeat(600)).expect("seed big file");

        let (addr, captured, server) = spawn_mock_openai_server(vec![
            openai_tool_call_response(vec![("r1", "read_file", "{\"path\":\"big.txt\"}")]),
            openai_tool_call_response(vec![("r2", "read_file", "{\"path\":\"big.txt\"}")]),
            openai_tool_call_response(vec![("r3", "read_file", "{\"path\":\"big.txt\"}")]),
            openai_tool_call_response(vec![("r4", "read_file", "{\"path\":\"big.txt\"}")]),
            openai_tool_call_response(vec![("r5", "read_file", "{\"path\":\"big.txt\"}")]),
            // Short-circuits immediately after 5th call into Grace Synthesis (`tools: []`)
            openai_text_response("Halted runaway read loop and summarized file."),
        ])
        .await;

        engine.providers().register_provider(providers::ProviderConfig::openai_compatible(
            "mock_openai",
            "Mock OpenAI",
            format!("http://{addr}/v1"),
            "mock-gpt",
            "k",
        ));
        let _ = engine.providers().set_active("mock_openai", Some("mock-gpt"));
        engine.providers().set_allow_local_fallback(false);

        let mut req = HermesTurnRequest::new("Read big.txt").with_session("s_streak");
        req.max_iterations = Some(20);
        let res = engine.run_turn(req).await.expect("short-circuit turn ok");
        let _ = server.await;

        assert_eq!(
            res.response_text,
            "Halted runaway read loop and summarized file."
        );
        assert_eq!(res.tool_calls.len(), 5);

        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 6, "5 tool iterations + 1 grace synthesis call");
        assert!(!reqs[5]["tools"]
            .as_array()
            .is_some_and(|arr| !arr.is_empty()));

        let stored = engine
            .store()
            .get_messages(&res.session_id, None)
            .expect("messages persisted");
        let tool_msgs: Vec<_> = stored.iter().filter(|m| m.role == "tool").collect();
        assert_eq!(tool_msgs.len(), 5);
        assert!(tool_msgs[1].content.contains("Duplicate output stub"));
        assert!(tool_msgs[1].content.contains("`r1`"));
        assert!(tool_msgs[2]
            .content
            .contains("got the same result 3 times"));
    }

    #[test]
    fn test_system_prompt_override_preserves_hermes_doctrine_and_tier1_cache() {
        let (_dir, engine) = test_engine();
        let a1 = engine.assemble_prompt_for_session_with_name(
            "s_override",
            Some(99),
            Some("7"),
            Some("OpsBot"),
            Some("You are OpsBot, managing GCP-Free-e2-micro."),
            &[],
        );
        let a2 = engine.assemble_prompt_for_session_with_name(
            "s_override",
            Some(99),
            Some("7"),
            Some("OpsBot"),
            Some("You are OpsBot, managing GCP-Free-e2-micro."),
            &[],
        );

        assert_eq!(a1.tier1_stable.content, a2.tier1_stable.content);
        assert_eq!(a1.tier1_hash, a2.tier1_hash);
        assert!(a1
            .tier1_stable
            .content
            .contains("You are OpsBot, managing GCP-Free-e2-micro."));
        assert!(a1
            .tier1_stable
            .content
            .contains("Autonomous Execution & Self-Learning Doctrine"));
        assert!(a1
            .tier1_stable
            .content
            .contains("Respect workspace boundaries"));
    }

    #[test]
    fn test_in_memory_provider_isolation_and_placeholder_default_model_skip() {
        for placeholder in [
            "",
            "deepseek-flash",
            "DeepSeek-Flash",
            "hermes-local-v1",
            "default",
            "auto",
            "默认模型",
        ] {
            assert!(
                is_placeholder_default_model(placeholder),
                "expected placeholder: {placeholder}"
            );
        }
        assert!(!is_placeholder_default_model("auto/gemini-3.8-flash"));
        assert!(!is_placeholder_default_model("gpt-4o"));

        let dir = TempDir::new().expect("temp dir");

        // `:memory:` engine (data_dir: None) uses offline ProviderRouter::new() unless forced live
        let mem_engine = HermesEngine::new(HermesEngineConfig {
            workspace_root: dir.path().to_path_buf(),
            data_dir: None,
            default_model: "deepseek-flash".into(),
            ..Default::default()
        })
        .expect("init :memory: engine");
        let (mem_pid, mem_model, mem_mode) = mem_engine.providers().active_info();
        assert_eq!(mem_pid, "local_hermes");
        assert_eq!(mem_model, "hermes-local-v1");
        assert_eq!(mem_mode, ApiMode::LocalHermes);
        assert!(mem_engine.providers().snapshot().allow_local_fallback);

        // Disk-backed engine with saved magpie_local preserves discovered/saved model despite "deepseek-flash"
        let data_dir = dir.path().join(".hermes_data");
        {
            let disk_engine =
                HermesEngine::open(&data_dir, dir.path()).expect("init disk engine");
            disk_engine
                .providers()
                .register_provider(providers::ProviderConfig::openai_compatible(
                    "magpie_local",
                    "Magpie Local",
                    "http://127.0.0.1:3425/v1",
                    "auto/gemini-3.8-flash",
                    "magpie-key",
                ));
            let _ = disk_engine
                .providers()
                .set_active("magpie_local", Some("auto/gemini-3.8-flash"));
            disk_engine.persist_providers_state();
        }

        let reopened = HermesEngine::new(HermesEngineConfig {
            workspace_root: dir.path().to_path_buf(),
            data_dir: Some(data_dir),
            default_model: "deepseek-flash".into(),
            ..Default::default()
        })
        .expect("reopen disk engine");
        let (re_pid, re_model, _) = reopened.providers().active_info();
        assert_eq!(re_pid, "magpie_local");
        assert_eq!(re_model, "auto/gemini-3.8-flash");
    }

    #[test]
    fn test_sqlite_restore_does_not_resurrect_deleted_subscription_and_restores_allow_local_fallback()
    {
        let dir = TempDir::new().expect("temp dir");
        let data_dir = dir.path().join(".hermes_data");
        {
            let engine = HermesEngine::open(&data_dir, dir.path()).expect("init disk engine");
            let mut deleted_sub = providers::ProviderConfig::openai_compatible(
                "codex_chatgpt:acct_deleted",
                "Codex Deleted",
                "https://chatgpt.com/backend-api/codex/responses",
                "gpt-4o",
                "stale-token",
            );
            deleted_sub.is_subscription = true;
            engine.providers().register_provider(deleted_sub);
            engine
                .providers()
                .register_provider(providers::ProviderConfig::openai_compatible(
                    "deepseek",
                    "DeepSeek",
                    "https://api.deepseek.com/v1",
                    "deepseek-chat",
                    "sk-deepseek-valid",
                ));
            let _ = engine
                .providers()
                .set_active("codex_chatgpt:acct_deleted", Some("gpt-4o"));
            engine.providers().set_fallback_chain(vec![
                providers::FallbackTarget {
                    provider_id: "codex_chatgpt:acct_deleted".to_string(),
                    model: "gpt-4o".to_string(),
                },
                providers::FallbackTarget {
                    provider_id: "deepseek".to_string(),
                    model: "deepseek-chat".to_string(),
                },
            ]);
            engine.providers().set_allow_local_fallback(false);
            engine.persist_providers_state();
        }

        // Reboot on same SQLite DB when `codex_chatgpt:acct_deleted` is NOT discovered on disk
        let reopened = HermesEngine::open(&data_dir, dir.path()).expect("reopen disk engine");
        let snap = reopened.providers().snapshot();
        assert!(
            !snap.providers.contains_key("codex_chatgpt:acct_deleted"),
            "deleted subscription must not be resurrected from SQLite"
        );
        assert!(
            snap.providers.contains_key("deepseek"),
            "non-subscription provider must still be restored from SQLite"
        );
        assert_ne!(
            snap.active_provider_id, "codex_chatgpt:acct_deleted",
            "deleted subscription must not be forced active"
        );
        assert_eq!(snap.active_provider_id, "local_hermes");
        assert!(
            !snap.allow_local_fallback,
            "saved allow_local_fallback=false must be restored"
        );
        assert!(
            snap.fallback_chain
                .iter()
                .all(|t| t.provider_id != "codex_chatgpt:acct_deleted"),
            "deleted subscription must be pruned from restored fallback_chain"
        );
    }

    #[tokio::test]
    async fn test_start_turn_stream_drop_rx_aborts_upstream_promptly() {
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let addr = listener.local_addr().expect("local addr");

        let dropped = Arc::new(AtomicBool::new(false));
        let dropped_srv = Arc::clone(&dropped);
        let aborted = Arc::new(AtomicBool::new(false));
        let aborted_srv = Arc::clone(&aborted);
        struct DropGuard(Arc<AtomicBool>);
        impl Drop for DropGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let srv = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let _guard = DropGuard(dropped_srv);

            // Read the full incoming HTTP POST request (headers up to \r\n\r\n + Content-Length body)
            let mut req_buf = Vec::with_capacity(4096);
            let mut tmp = [0u8; 4096];
            let drained = tokio::time::timeout(Duration::from_secs(5), async {
                let mut hdr_end: Option<usize> = None;
                let mut expected_total: Option<usize> = None;
                loop {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => break false,
                        Ok(n) => {
                            req_buf.extend_from_slice(&tmp[..n]);
                            if hdr_end.is_none() {
                                if let Some(pos) =
                                    req_buf.windows(4).position(|w| w == b"\r\n\r\n")
                                {
                                    hdr_end = Some(pos);
                                    let hdr_str = String::from_utf8_lossy(&req_buf[..pos]);
                                    let content_len = hdr_str
                                        .lines()
                                        .find_map(|line| {
                                            let (k, v) = line.split_once(':')?;
                                            if k.trim().eq_ignore_ascii_case("content-length") {
                                                v.trim().parse::<usize>().ok()
                                            } else {
                                                None
                                            }
                                        })
                                        .unwrap_or(0);
                                    expected_total = Some(pos + 4 + content_len);
                                }
                            }
                            if let Some(total) = expected_total {
                                if req_buf.len() >= total {
                                    break true;
                                }
                            }
                        }
                    }
                }
            })
            .await
            .expect("mock server drain HTTP request timeout");
            assert!(drained, "expected complete HTTP request headers + body");

            let hdr = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = sock.write_all(hdr.as_bytes()).await;
            let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"FIRST_\"}}]}\n\n";
            let chunk = format!("{:x}\r\n{}\r\n", sse.len(), sse);
            let _ = sock.write_all(chunk.as_bytes()).await;
            let _ = sock.flush().await;
            // Wait until client closes TCP socket (EOF/Err) or 5s timeout
            let mut rbuf = [0u8; 64];
            let _ = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match sock.read(&mut rbuf).await {
                        Ok(0) | Err(_) => {
                            aborted_srv.store(true, Ordering::SeqCst);
                            break;
                        }
                        Ok(_) => continue,
                    }
                }
            })
            .await;
        });

        let dir = TempDir::new().expect("temp dir");
        let engine = HermesEngine::open(dir.path().join(".hermes"), dir.path()).expect("open");
        engine.providers().set_allow_local_fallback(false);
        let base_url = format!("http://{addr}/v1");
        engine
            .providers()
            .register_provider(providers::ProviderConfig::openai_compatible(
                "mock_ds",
                "Mock DS",
                base_url,
                "mock-model",
                "sk-mock",
            ));
        engine
            .providers()
            .set_active("mock_ds", Some("mock-model"))
            .expect("set active");

        let mut req = HermesTurnRequest::new("hello");
        req.skip_slash_commands = true;
        let mut rx = engine.run_turn_stream(req).await.expect("run_turn_stream");

        let mut saw_first = false;
        while let Some(ev) = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("recv timeout")
        {
            if let HermesEvent::TextDelta { text, .. } = ev {
                if text.contains("FIRST_") {
                    saw_first = true;
                    break;
                }
            }
        }
        assert!(saw_first, "expected FIRST_ delta");
        assert!(
            !dropped.load(Ordering::SeqCst),
            "upstream stream must remain open before drop(rx)"
        );
        assert!(
            !aborted.load(Ordering::SeqCst),
            "upstream socket must not abort before drop(rx)"
        );
        drop(rx);

        tokio::time::timeout(Duration::from_millis(250), async {
            while !(dropped.load(Ordering::SeqCst) && aborted.load(Ordering::SeqCst)) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("upstream socket must close within 250ms after drop(rx)");
        let _ = srv.await;
    }
}
