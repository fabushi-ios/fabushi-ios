//! First-Class Fabushi Bot Runtime (`Agent` aggregate) backed by `mahayana-host-runtime`
//! and `fabushi-messaging-core`, absorbing isolated per-Bot Hermes capabilities.
//!
//! Every created or hydrated bot receives:
//! - Validated `agent_id` directory under `<agents_root>/<agent_id>/`
//! - `profile.json` (`SandAgentProfile` with geometric `avatarShape` & `avatarColor`)
//! - `settings.json` (`SandAgentSettings` + preferred model, thinking level, approval mode, toolset preset)
//! - Isolated `HermesEngine` instance (`MEMORY.md`, `USER.md`, `SOUL.md`, `skills/`, `hermes_state.db`,
//!   Honcho, checkpoints, 58+ tools) synced with host LLM provider credentials
//! - Isolated `AcpRunner` (`ProductionTurnAgentOwner` + `SandAgentRunner`)
//! - Teammate collaboration (`render_agent_directory_system_prompt`, `SendToAgent`, `CreateAgent`,
//!   `UpdateAgent`) and mid-session `AgentProfilePromptSnapshot` delta announcements.

use crate::acp::AcpRunner;
use crate::computer::ComputerCloudProvider;
use crate::hermes::memory::{MemoryTarget, SoulProfile};
use crate::hermes::security::ApprovalMode;
use crate::hermes::toolsets::{
    AgenticStateBackend, FabushiTeammateHandler, PlatformImBridge, ToolsetPreset,
};
use crate::hermes::{HermesEngine, HermesEngineConfig};
use chrono::Utc;
use fabushi_messaging_core::actor::ActorId;
use fabushi_messaging_core::bot::{BotCommand, BotInvocation, BotProfile, BotRegistry};
use fabushi_messaging_core::conversation::ConversationId;
use fabushi_messaging_core::message::FormattedText;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_proto::miniapp_contracts::{
    derive_safe_agent_id, normalize_fabushi_avatar_color, normalize_fabushi_avatar_shape,
    BotCommandSpec, FabushiBotSummaryDto, BOTFATHER_USER_ID,
};
pub use mahayana_host_runtime::agents::agent_messaging::{AgentAddress, AgentGroupAddress};
use mahayana_host_runtime::agents::agent_messaging::{
    build_mentioned_agents_context, render_agent_directory_system_prompt,
};
use mahayana_host_runtime::agents::agent_profile::{
    get_sand_profile_path, read_sand_profile_file, write_sand_profile_file, SandAgentProfile,
};
use mahayana_host_runtime::agents::agent_roster::normalize_agent_id;
use mahayana_host_runtime::agents::settings_file::{
    get_sand_settings_path, read_sand_settings_file, write_sand_settings_file, SandAgentSettings,
    DEFAULT_HIDDEN_FROM_SIDEBAR, DEFAULT_NOTIFY_ON_AGENT_UPDATES,
};
use mahayana_host_runtime::runner::sand_agent_profile_prompt::{
    normalize_agent_profile_identity, persist_announced_agent_profile_snapshot,
    render_agent_profile_update, resolve_agent_profile_prompt_snapshot, AgentProfileIdentity,
    AgentProfilePromptSnapshot,
};
use mahayana_host_runtime::storage::agent_paths::assert_valid_sand_agent_id;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tempfile::TempDir;

#[derive(Debug, Clone)]
pub struct FabushiBotProvisionSpec {
    pub bot_user_id: i64,
    pub owner_user_id: i64,
    pub username: String,
    pub display_name: String,
    pub description: String,
    pub title: String,
    pub agent_id: Option<String>,
    pub avatar_shape: Option<String>,
    pub avatar_color: Option<String>,
    pub preferred_model: Option<String>,
    pub thinking_level: Option<String>,
    pub approval_mode: Option<String>,
    pub toolset_preset: Option<String>,
    pub commands: Vec<BotCommandSpec>,
    pub inline_mode_enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FabushiBotPatch {
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub title: Option<String>,
    pub avatar_shape: Option<String>,
    pub avatar_color: Option<String>,
    pub preferred_model: Option<String>,
    pub thinking_level: Option<String>,
    pub approval_mode: Option<String>,
    pub toolset_preset: Option<String>,
    pub notify_on_agent_updates: Option<bool>,
    pub hidden_from_sidebar: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct FabushiBotRuntimeSettings {
    pub sand: SandAgentSettings,
    pub bot_user_id: i64,
    pub owner_user_id: i64,
    pub username: String,
    pub preferred_model: String,
    pub thinking_level: String,
    pub approval_mode: String,
    pub toolset_preset: String,
}

pub struct FabushiBotInstance {
    pub bot_user_id: i64,
    pub agent_id: String,
    pub agent_dir: PathBuf,
    profile: RwLock<SandAgentProfile>,
    settings: RwLock<FabushiBotRuntimeSettings>,
    hermes: Arc<HermesEngine>,
    acp_runner: Arc<AcpRunner>,
    profile_snapshots: Mutex<HashMap<i64, AgentProfilePromptSnapshot>>,
    active_executions: AtomicUsize,
}

impl FabushiBotInstance {
    pub fn hermes(&self) -> Arc<HermesEngine> {
        Arc::clone(&self.hermes)
    }

    pub fn acp_runner(&self) -> Arc<AcpRunner> {
        Arc::clone(&self.acp_runner)
    }

    pub fn profile(&self) -> SandAgentProfile {
        self.profile
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn settings(&self) -> FabushiBotRuntimeSettings {
        self.settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn execution_state_label(&self) -> &'static str {
        if self.active_executions.load(Ordering::SeqCst) > 0 {
            "running"
        } else {
            "idle"
        }
    }

    pub fn agent_address(&self) -> AgentAddress {
        let profile = self.profile();
        let desc = profile.description.trim();
        AgentAddress {
            id: self.agent_id.clone(),
            name: profile.name,
            description: (!desc.is_empty()).then(|| desc.to_string()),
            is_group: false,
        }
    }

    pub fn to_summary_dto(&self, companion_chat_id: Option<i64>) -> FabushiBotSummaryDto {
        let profile = self.profile();
        let settings = self.settings();
        let memory_entries_count = self
            .hermes
            .memory()
            .read(MemoryTarget::Memory)
            .map(|r| r.entries.len())
            .unwrap_or(0);
        let skills_count = self.hermes.skills().skills_list(None).len();
        let effective_model = if !settings.preferred_model.trim().is_empty() {
            settings.preferred_model.clone()
        } else {
            self.hermes.providers().active_info().1
        };

        FabushiBotSummaryDto {
            bot_user_id: self.bot_user_id,
            owner_user_id: settings.owner_user_id,
            agent_id: self.agent_id.clone(),
            username: settings.username,
            display_name: profile.name,
            description: profile.description,
            title: profile.title,
            avatar_shape: profile.avatar_shape,
            avatar_color: profile.avatar_color,
            preferred_model: effective_model,
            thinking_level: settings.thinking_level,
            approval_mode: settings.approval_mode,
            toolset_preset: settings.toolset_preset,
            notify_on_agent_updates: settings.sand.notify_on_agent_updates,
            hidden_from_sidebar: settings.sand.hidden_from_sidebar,
            memory_entries_count,
            skills_count,
            execution_state: self.execution_state_label().to_string(),
            companion_chat_id,
        }
    }
}

pub struct FabushiExecutionGuard {
    instance: Arc<FabushiBotInstance>,
    registry: Arc<Mutex<BotRegistry>>,
    execution_id: String,
    succeeded: bool,
    error: Option<String>,
}

impl FabushiExecutionGuard {
    pub fn mark_success(&mut self) {
        self.succeeded = true;
        self.error = None;
    }

    pub fn mark_failed(&mut self, err: impl Into<String>) {
        self.succeeded = false;
        self.error = Some(err.into());
    }
}

impl Drop for FabushiExecutionGuard {
    fn drop(&mut self) {
        let _ = self.instance.active_executions.fetch_sub(1, Ordering::SeqCst);
        if let Ok(mut reg) = self.registry.lock() {
            let now_ms = Utc::now().timestamp_millis();
            let _ = reg.finish_execution(
                &self.execution_id,
                self.succeeded,
                now_ms,
                self.error.clone(),
            );
        }
    }
}

pub struct FabushiBotHost {
    agents_root: PathBuf,
    _ephemeral_dir: Option<TempDir>,
    host_hermes: Arc<HermesEngine>,
    computer_provider: RwLock<Option<Arc<dyn ComputerCloudProvider>>>,
    teammate_handler: RwLock<Option<Arc<dyn FabushiTeammateHandler>>>,
    platform_im_bridge: RwLock<Option<Arc<dyn PlatformImBridge>>>,
    agentic_backend: RwLock<Option<Arc<dyn AgenticStateBackend>>>,
    instances: RwLock<HashMap<i64, Arc<FabushiBotInstance>>>,
    agent_id_index: RwLock<HashMap<String, i64>>,
    username_index: RwLock<HashMap<String, i64>>,
    core_registry: Arc<Mutex<BotRegistry>>,
}

impl FabushiBotHost {
    pub fn new_with_dir(
        agents_root: impl AsRef<Path>,
        host_hermes: Arc<HermesEngine>,
    ) -> Result<Self> {
        let root = agents_root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            agents_root: root,
            _ephemeral_dir: None,
            host_hermes,
            computer_provider: RwLock::new(None),
            teammate_handler: RwLock::new(None),
            platform_im_bridge: RwLock::new(None),
            agentic_backend: RwLock::new(None),
            instances: RwLock::new(HashMap::new()),
            agent_id_index: RwLock::new(HashMap::new()),
            username_index: RwLock::new(HashMap::new()),
            core_registry: Arc::new(Mutex::new(BotRegistry::default())),
        })
    }

    pub fn new_ephemeral(host_hermes: Arc<HermesEngine>) -> Result<Self> {
        let temp_dir = TempDir::new().map_err(|e| {
            TelegramError::Internal(format!("Failed to create ephemeral Fabushi agents dir: {e}"))
        })?;
        let root = temp_dir.path().to_path_buf();
        Ok(Self {
            agents_root: root,
            _ephemeral_dir: Some(temp_dir),
            host_hermes,
            computer_provider: RwLock::new(None),
            teammate_handler: RwLock::new(None),
            platform_im_bridge: RwLock::new(None),
            agentic_backend: RwLock::new(None),
            instances: RwLock::new(HashMap::new()),
            agent_id_index: RwLock::new(HashMap::new()),
            username_index: RwLock::new(HashMap::new()),
            core_registry: Arc::new(Mutex::new(BotRegistry::default())),
        })
    }

    pub fn agents_root(&self) -> &Path {
        &self.agents_root
    }

    pub fn set_computer_provider(&self, provider: Arc<dyn ComputerCloudProvider>) {
        if let Ok(mut guard) = self.computer_provider.write() {
            *guard = Some(Arc::clone(&provider));
        }
        if let Ok(instances) = self.instances.read() {
            for inst in instances.values() {
                inst.hermes.set_computer_provider(Arc::clone(&provider));
            }
        }
    }

    pub fn set_teammate_handler(&self, handler: Arc<dyn FabushiTeammateHandler>) {
        if let Ok(mut guard) = self.teammate_handler.write() {
            *guard = Some(Arc::clone(&handler));
        }
        if let Ok(instances) = self.instances.read() {
            for inst in instances.values() {
                inst.hermes.set_teammate_handler(Arc::clone(&handler));
            }
        }
    }

    pub fn set_platform_im_bridge(&self, bridge: Arc<dyn PlatformImBridge>) {
        if let Ok(mut guard) = self.platform_im_bridge.write() {
            *guard = Some(Arc::clone(&bridge));
        }
        self.host_hermes.set_platform_im_bridge(Arc::clone(&bridge));
        if let Ok(instances) = self.instances.read() {
            for inst in instances.values() {
                inst.hermes.set_platform_im_bridge(Arc::clone(&bridge));
            }
        }
    }

    pub fn set_agentic_backend(&self, backend: Arc<dyn AgenticStateBackend>) {
        if let Ok(mut guard) = self.agentic_backend.write() {
            *guard = Some(Arc::clone(&backend));
        }
        self.host_hermes.set_agentic_backend(Arc::clone(&backend));
        if let Ok(instances) = self.instances.read() {
            for inst in instances.values() {
                inst.hermes.set_agentic_backend(Arc::clone(&backend));
            }
        }
    }

    pub fn sync_all_providers(&self) {
        let upstream = self.host_hermes.providers();
        if let Ok(instances) = self.instances.read() {
            for inst in instances.values() {
                inst.hermes.sync_providers_from(upstream);
            }
        }
    }

    pub fn normalize_thinking_level(raw: Option<&str>) -> String {
        match raw.unwrap_or("").trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "0" => "off".to_string(),
            "low" | "fast" => "low".to_string(),
            "high" | "deep" => "high".to_string(),
            _ => "medium".to_string(),
        }
    }

    pub fn thinking_budget_for_level(level: &str) -> Option<usize> {
        match level.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "0" => None,
            "low" | "fast" => Some(1024),
            "high" | "deep" => Some(12_288),
            _ => Some(4096),
        }
    }

    pub fn normalize_approval_mode_name(raw: Option<&str>) -> String {
        match raw.unwrap_or("").trim().to_ascii_lowercase().as_str() {
            "autonomous" | "auto" | "full" | "yolo" | "always" => "autonomous".to_string(),
            "strict" | "deny" | "off" => "strict".to_string(),
            _ => "prompt".to_string(),
        }
    }

    pub fn approval_mode_for_name(name: &str) -> ApprovalMode {
        match name.trim().to_ascii_lowercase().as_str() {
            "autonomous" | "auto" | "full" | "yolo" => ApprovalMode::Yolo,
            "always" | "permanent" | "permanent_allow" => ApprovalMode::PermanentAllow,
            "smart" | "smart_approve" => ApprovalMode::SmartApprove,
            "strict" | "deny" | "off" => ApprovalMode::Deny,
            _ => ApprovalMode::Prompt,
        }
    }

    pub fn normalize_toolset_preset_name(raw: Option<&str>) -> String {
        ToolsetPreset::from_str(raw.unwrap_or("full"))
            .unwrap_or(ToolsetPreset::Full)
            .as_str()
            .to_string()
    }

    pub fn toolset_preset_for_name(name: &str) -> ToolsetPreset {
        ToolsetPreset::from_str(name).unwrap_or(ToolsetPreset::Full)
    }

    fn resolve_unique_agent_id(
        &self,
        preferred: Option<&str>,
        username: &str,
        bot_user_id: i64,
    ) -> Result<String> {
        let mut candidate = normalize_agent_id(&derive_safe_agent_id(
            preferred,
            username,
            bot_user_id,
        ));
        if assert_valid_sand_agent_id(&candidate).is_err() {
            candidate = format!("fabushi-agent-{}", bot_user_id.max(0));
        }

        let owner_conflict = self
            .agent_id_index
            .read()
            .ok()
            .and_then(|idx| idx.get(&candidate).copied())
            .is_some_and(|existing_uid| existing_uid != bot_user_id);

        if owner_conflict {
            let suffix = format!("{}-{}", candidate, bot_user_id.max(0));
            candidate = suffix.chars().take(60).collect();
        }
        assert_valid_sand_agent_id(&candidate).map_err(|e| {
            TelegramError::Validation(format!("Invalid Fabushi agent_id: {e}"))
        })?;
        Ok(candidate)
    }

    fn register_in_core_registry(
        &self,
        bot_user_id: i64,
        agent_id: &str,
        profile: &SandAgentProfile,
        commands: &[BotCommandSpec],
        inline_mode_enabled: bool,
    ) {
        let mut parsed_commands = Vec::new();
        for cmd in commands {
            let clean = cmd.command.trim().trim_start_matches('/');
            if !clean.is_empty()
                && clean.len() <= 32
                && clean
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                parsed_commands.push(BotCommand {
                    command: clean.to_string(),
                    description: cmd.description.clone(),
                    scopes: BTreeSet::new(),
                });
            }
        }
        let core_profile = BotProfile {
            actor_id: ActorId::new(format!("fabushi:{bot_user_id}:{agent_id}")),
            description: profile.description.clone(),
            about: profile.title.clone(),
            commands: parsed_commands,
            inline_mode_enabled,
            inline_placeholder: None,
            groups_allowed: true,
            privacy_mode: false,
            mini_app_id: None,
            payment_provider_ids: Vec::new(),
            business_mode: false,
        };
        if let Ok(mut reg) = self.core_registry.lock() {
            let _ = reg.register(core_profile);
        }
    }

    fn write_settings_json(path: &Path, settings: &FabushiBotRuntimeSettings) {
        let mut map = Map::new();
        map.insert(
            "notifyOnAgentUpdates".to_string(),
            Value::Bool(settings.sand.notify_on_agent_updates),
        );
        map.insert(
            "hiddenFromSidebar".to_string(),
            Value::Bool(settings.sand.hidden_from_sidebar),
        );
        map.insert("botUserId".to_string(), Value::from(settings.bot_user_id));
        map.insert(
            "ownerUserId".to_string(),
            Value::from(settings.owner_user_id),
        );
        map.insert(
            "botUsername".to_string(),
            Value::String(settings.username.clone()),
        );
        map.insert(
            "preferredModel".to_string(),
            Value::String(settings.preferred_model.clone()),
        );
        map.insert(
            "thinkingLevel".to_string(),
            Value::String(settings.thinking_level.clone()),
        );
        map.insert(
            "approvalMode".to_string(),
            Value::String(settings.approval_mode.clone()),
        );
        map.insert(
            "toolsetPreset".to_string(),
            Value::String(settings.toolset_preset.clone()),
        );
        let _ = write_sand_settings_file(path, &map);
    }

    /// Provisions (or synchronizes) a first-class Fabushi Bot instance on disk & in memory.
    pub fn provision_or_sync_bot(
        &self,
        spec: FabushiBotProvisionSpec,
    ) -> Result<Arc<FabushiBotInstance>> {
        if spec.bot_user_id == BOTFATHER_USER_ID as i64 {
            return Err(TelegramError::Validation(
                "BotFather factory user cannot be registered as a conversational agent"
                    .to_string(),
            ));
        }

        if let Some(existing) = self.get_by_user_id(spec.bot_user_id) {
            // Sync metadata updates without recreating isolated engine/files
            {
                let mut prof = existing.profile.write().unwrap_or_else(|e| e.into_inner());
                if !spec.display_name.trim().is_empty() {
                    prof.name = spec.display_name.trim().to_string();
                }
                if !spec.description.trim().is_empty() {
                    prof.description = spec.description.trim().to_string();
                }
                if !spec.title.trim().is_empty() {
                    prof.title = spec.title.trim().to_string();
                }
                if let Some(ref shape) = spec.avatar_shape {
                    if !shape.trim().is_empty() {
                        prof.avatar_shape =
                            normalize_fabushi_avatar_shape(shape, &existing.agent_id);
                    }
                }
                if let Some(ref color) = spec.avatar_color {
                    if !color.trim().is_empty() {
                        prof.avatar_color =
                            normalize_fabushi_avatar_color(color, &existing.agent_id);
                    }
                }
                let _ = write_sand_profile_file(get_sand_profile_path(&existing.agent_dir), &prof);
            }
            {
                let mut st = existing.settings.write().unwrap_or_else(|e| e.into_inner());
                if !spec.username.trim().is_empty() {
                    st.username = spec.username.trim().trim_start_matches('@').to_string();
                }
                if let Some(ref m) = spec.preferred_model {
                    if !m.trim().is_empty() {
                        st.preferred_model = m.trim().to_string();
                    }
                }
                if let Some(ref t) = spec.thinking_level {
                    if !t.trim().is_empty() {
                        st.thinking_level = Self::normalize_thinking_level(Some(t));
                    }
                }
                if let Some(ref a) = spec.approval_mode {
                    if !a.trim().is_empty() {
                        st.approval_mode = Self::normalize_approval_mode_name(Some(a));
                    }
                }
                if let Some(ref p) = spec.toolset_preset {
                    if !p.trim().is_empty() {
                        st.toolset_preset = Self::normalize_toolset_preset_name(Some(p));
                    }
                }
                Self::write_settings_json(get_sand_settings_path(&existing.agent_dir).as_path(), &st);
            }
            existing
                .hermes
                .sync_providers_from(self.host_hermes.providers());
            let prof_snap = existing.profile();
            self.register_in_core_registry(
                spec.bot_user_id,
                &existing.agent_id,
                &prof_snap,
                &spec.commands,
                spec.inline_mode_enabled,
            );
            return Ok(existing);
        }

        let agent_id = self.resolve_unique_agent_id(
            spec.agent_id.as_deref(),
            &spec.username,
            spec.bot_user_id,
        )?;
        let agent_dir = self.agents_root.join(&agent_id);
        std::fs::create_dir_all(&agent_dir)?;

        let profile_path = get_sand_profile_path(&agent_dir);
        let disk_profile = read_sand_profile_file(&profile_path);
        let display_name = if !spec.display_name.trim().is_empty() {
            spec.display_name.trim().to_string()
        } else if let Some(ref dp) = disk_profile {
            dp.name.clone()
        } else {
            spec.username.clone()
        };
        let description = if !spec.description.trim().is_empty() {
            spec.description.trim().to_string()
        } else {
            disk_profile
                .as_ref()
                .map(|p| p.description.clone())
                .unwrap_or_default()
        };
        let title = if !spec.title.trim().is_empty() {
            spec.title.trim().to_string()
        } else {
            disk_profile
                .as_ref()
                .map(|p| p.title.clone())
                .unwrap_or_default()
        };
        let avatar_shape = spec
            .avatar_shape
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| normalize_fabushi_avatar_shape(s, &agent_id))
            .or_else(|| {
                disk_profile
                    .as_ref()
                    .filter(|p| !p.avatar_shape.trim().is_empty())
                    .map(|p| normalize_fabushi_avatar_shape(&p.avatar_shape, &agent_id))
            })
            .unwrap_or_else(|| normalize_fabushi_avatar_shape("", &agent_id));
        let avatar_color = spec
            .avatar_color
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| normalize_fabushi_avatar_color(s, &agent_id))
            .or_else(|| {
                disk_profile
                    .as_ref()
                    .filter(|p| !p.avatar_color.trim().is_empty())
                    .map(|p| normalize_fabushi_avatar_color(&p.avatar_color, &agent_id))
            })
            .unwrap_or_else(|| normalize_fabushi_avatar_color("", &agent_id));

        let profile = SandAgentProfile {
            name: display_name.clone(),
            description: description.clone(),
            title,
            avatar_shape,
            avatar_color,
        };
        write_sand_profile_file(&profile_path, &profile).map_err(|e| {
            TelegramError::Internal(format!("Failed to write Fabushi profile.json: {e}"))
        })?;

        let settings_path = get_sand_settings_path(&agent_dir);
        let disk_settings = if settings_path.exists() {
            read_sand_settings_file(&settings_path)
        } else {
            SandAgentSettings {
                notify_on_agent_updates: DEFAULT_NOTIFY_ON_AGENT_UPDATES,
                hidden_from_sidebar: DEFAULT_HIDDEN_FROM_SIDEBAR,
            }
        };
        let preferred_model = spec
            .preferred_model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| self.host_hermes.providers().active_info().1);
        let thinking_level = Self::normalize_thinking_level(spec.thinking_level.as_deref());
        let approval_mode = Self::normalize_approval_mode_name(spec.approval_mode.as_deref());
        let toolset_preset = Self::normalize_toolset_preset_name(spec.toolset_preset.as_deref());
        let clean_username = spec.username.trim().trim_start_matches('@').to_string();

        let runtime_settings = FabushiBotRuntimeSettings {
            sand: disk_settings,
            bot_user_id: spec.bot_user_id,
            owner_user_id: spec.owner_user_id,
            username: clean_username.clone(),
            preferred_model: preferred_model.clone(),
            thinking_level,
            approval_mode: approval_mode.clone(),
            toolset_preset: toolset_preset.clone(),
        };
        Self::write_settings_json(&settings_path, &runtime_settings);

        // Initialize isolated per-Bot HermesEngine absorbing all Hermes tools, skills, memory,
        // Honcho, checkpoints & slash commands inside `<agents_root>/<agent_id>/`.
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let bot_hermes_cfg = HermesEngineConfig {
            workspace_root,
            data_dir: Some(agent_dir.clone()),
            default_model: preferred_model.clone(),
            default_preset: Self::toolset_preset_for_name(&toolset_preset),
            default_approval_mode: Self::approval_mode_for_name(&approval_mode),
            ..Default::default()
        };
        let bot_hermes = Arc::new(HermesEngine::new(bot_hermes_cfg)?);
        bot_hermes.sync_providers_from(self.host_hermes.providers());
        bot_hermes.set_bound_agent_id(&agent_id);
        if let Some(provider) = self
            .computer_provider
            .read()
            .ok()
            .and_then(|g| g.clone())
        {
            bot_hermes.set_computer_provider(provider);
        } else if let Some(provider) = self.host_hermes.computer_provider() {
            bot_hermes.set_computer_provider(provider);
        }
        if let Some(handler) = self.teammate_handler.read().ok().and_then(|g| g.clone()) {
            bot_hermes.set_teammate_handler(handler);
        }
        if let Some(bridge) = self.platform_im_bridge.read().ok().and_then(|g| g.clone()) {
            bot_hermes.set_platform_im_bridge(bridge);
        } else if let Some(bridge) = self.host_hermes.platform_im_bridge() {
            bot_hermes.set_platform_im_bridge(bridge);
        }
        if let Some(backend) = self.agentic_backend.read().ok().and_then(|g| g.clone()) {
            bot_hermes.set_agentic_backend(backend);
        } else if let Some(backend) = self.host_hermes.agentic_backend() {
            bot_hermes.set_agentic_backend(backend);
        }

        // Seed SOUL.md in this bot's isolated MemoryManager if empty and persona is provided
        if !description.is_empty() && !bot_hermes.memory().has_soul() {
            let _ = bot_hermes.memory().write_soul(&SoulProfile::new(
                &display_name,
                if profile.title.is_empty() {
                    "Fabushi Agent"
                } else {
                    profile.title.as_str()
                },
                &description,
            ));
        }

        let acp_runner =
            Arc::new(AcpRunner::new(&preferred_model).with_hermes_engine(Arc::clone(&bot_hermes)));

        let instance = Arc::new(FabushiBotInstance {
            bot_user_id: spec.bot_user_id,
            agent_id: agent_id.clone(),
            agent_dir,
            profile: RwLock::new(profile.clone()),
            settings: RwLock::new(runtime_settings),
            hermes: bot_hermes,
            acp_runner,
            profile_snapshots: Mutex::new(HashMap::new()),
            active_executions: AtomicUsize::new(0),
        });

        self.register_in_core_registry(
            spec.bot_user_id,
            &agent_id,
            &profile,
            &spec.commands,
            spec.inline_mode_enabled,
        );

        if let Ok(mut map) = self.instances.write() {
            map.insert(spec.bot_user_id, Arc::clone(&instance));
        }
        if let Ok(mut idx) = self.agent_id_index.write() {
            idx.insert(agent_id, spec.bot_user_id);
        }
        if !clean_username.is_empty() {
            if let Ok(mut uidx) = self.username_index.write() {
                uidx.insert(clean_username.to_ascii_lowercase(), spec.bot_user_id);
            }
        }

        Ok(instance)
    }

    pub fn update_bot(
        &self,
        bot_user_id: i64,
        patch: FabushiBotPatch,
    ) -> Result<Arc<FabushiBotInstance>> {
        let Some(instance) = self.get_by_user_id(bot_user_id) else {
            return Err(TelegramError::NotFound(format!(
                "Fabushi bot {bot_user_id} not provisioned"
            )));
        };

        {
            let mut prof = instance.profile.write().unwrap_or_else(|e| e.into_inner());
            if let Some(ref name) = patch.display_name {
                let clean = name.trim();
                if !clean.is_empty() {
                    prof.name = clean.to_string();
                }
            }
            if let Some(ref desc) = patch.description {
                prof.description = desc.trim().to_string();
            }
            if let Some(ref title) = patch.title {
                prof.title = title.trim().to_string();
            }
            if let Some(ref shape) = patch.avatar_shape {
                prof.avatar_shape = normalize_fabushi_avatar_shape(shape, &instance.agent_id);
            }
            if let Some(ref color) = patch.avatar_color {
                prof.avatar_color = normalize_fabushi_avatar_color(color, &instance.agent_id);
            }
            write_sand_profile_file(get_sand_profile_path(&instance.agent_dir), &prof).map_err(
                |e| TelegramError::Internal(format!("Failed to update profile.json: {e}")),
            )?;
            let _ = instance.hermes.memory().write_soul(&SoulProfile::new(
                &prof.name,
                if prof.title.is_empty() {
                    "Fabushi Agent"
                } else {
                    prof.title.as_str()
                },
                &prof.description,
            ));
        }

        {
            let mut st = instance.settings.write().unwrap_or_else(|e| e.into_inner());
            if let Some(ref m) = patch.preferred_model {
                st.preferred_model = m.trim().to_string();
            }
            if let Some(ref t) = patch.thinking_level {
                st.thinking_level = Self::normalize_thinking_level(Some(t));
            }
            if let Some(ref a) = patch.approval_mode {
                st.approval_mode = Self::normalize_approval_mode_name(Some(a));
                if let Ok(mut sec) = instance.hermes.security().write() {
                    sec.approvals.default_mode = Self::approval_mode_for_name(&st.approval_mode);
                }
            }
            if let Some(ref p) = patch.toolset_preset {
                st.toolset_preset = Self::normalize_toolset_preset_name(Some(p));
                if let Ok(mut reg) = instance.hermes.toolsets().write() {
                    reg.apply_preset(Self::toolset_preset_for_name(&st.toolset_preset));
                }
            }
            if let Some(notify) = patch.notify_on_agent_updates {
                st.sand.notify_on_agent_updates = notify;
            }
            if let Some(hidden) = patch.hidden_from_sidebar {
                st.sand.hidden_from_sidebar = hidden;
            }
            Self::write_settings_json(get_sand_settings_path(&instance.agent_dir).as_path(), &st);
        }

        let updated_profile = instance.profile();
        self.register_in_core_registry(bot_user_id, &instance.agent_id, &updated_profile, &[], false);

        Ok(instance)
    }

    pub fn get_by_user_id(&self, bot_user_id: i64) -> Option<Arc<FabushiBotInstance>> {
        self.instances
            .read()
            .ok()
            .and_then(|map| map.get(&bot_user_id).cloned())
    }

    pub fn get_by_agent_id_or_username(&self, target: &str) -> Option<Arc<FabushiBotInstance>> {
        let trimmed = target.trim().trim_start_matches('@');
        if trimmed.is_empty() {
            return None;
        }
        if let Ok(uid) = trimmed.parse::<i64>() {
            if let Some(found) = self.get_by_user_id(uid) {
                return Some(found);
            }
        }
        let norm_agent = normalize_agent_id(trimmed);
        if let Some(uid) = self
            .agent_id_index
            .read()
            .ok()
            .and_then(|idx| idx.get(&norm_agent).copied())
        {
            if let Some(found) = self.get_by_user_id(uid) {
                return Some(found);
            }
        }
        let lower = trimmed.to_ascii_lowercase();
        if let Some(uid) = self
            .username_index
            .read()
            .ok()
            .and_then(|idx| idx.get(&lower).copied())
        {
            return self.get_by_user_id(uid);
        }
        None
    }

    pub fn list_instances(&self) -> Vec<Arc<FabushiBotInstance>> {
        let mut out: Vec<_> = self
            .instances
            .read()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        out.sort_by_key(|inst| inst.bot_user_id);
        out
    }

    pub fn begin_bot_execution(
        &self,
        bot_user_id: i64,
        chat_id: i64,
        sender_user_id: i64,
        text: &str,
        command: Option<&str>,
    ) -> Option<FabushiExecutionGuard> {
        let instance = self.get_by_user_id(bot_user_id)?;
        instance.active_executions.fetch_add(1, Ordering::SeqCst);
        let invocation_id = format!(
            "inv_{}_{}_{}",
            bot_user_id,
            chat_id,
            uuid::Uuid::new_v4().simple()
        );
        let invocation = BotInvocation {
            id: invocation_id.clone(),
            bot_id: ActorId::new(format!("fabushi:{bot_user_id}:{}", instance.agent_id)),
            sender_id: ActorId::new(format!("user:{sender_user_id}")),
            conversation_id: ConversationId::new(format!("chat:{chat_id}")),
            command: command.map(ToString::to_string),
            text: FormattedText::plain(text),
            reply_to_message_id: None,
            metadata: Default::default(),
            created_at_ms: Utc::now().timestamp_millis(),
        };
        let execution_id = if let Ok(mut reg) = self.core_registry.lock() {
            reg.begin_execution(&invocation, Utc::now().timestamp_millis())
                .map(|ex| ex.id)
                .unwrap_or_else(|_| format!("bot-execution:{invocation_id}"))
        } else {
            format!("bot-execution:{invocation_id}")
        };
        Some(FabushiExecutionGuard {
            instance,
            registry: Arc::clone(&self.core_registry),
            execution_id,
            succeeded: false,
            error: None,
        })
    }

    /// Composes the Fabushi-native system prompt (Agent Identity + Teammate Directory +
    /// Mentioned Agents Context) and returns any live `<<SAND_AGENT_PROFILE_UPDATE:v1:...>>`
    /// prompt prefix if the agent profile was edited since the last turn in `chat_id`.
    pub fn build_turn_system_prompt(
        &self,
        bot_user_id: i64,
        chat_id: i64,
        base_system_prompt: &str,
        room_groups: &[AgentGroupAddress],
        mentioned_teammates: &[AgentAddress],
    ) -> (String, Option<String>) {
        let Some(instance) = self.get_by_user_id(bot_user_id) else {
            return (base_system_prompt.to_string(), None);
        };
        let profile = instance.profile();
        let current_identity = normalize_agent_profile_identity(AgentProfileIdentity {
            name: profile.name.clone(),
            description: profile.description.clone(),
        });

        let mut profile_lines = vec![format!(
            "## Agent Identity (Fabushi Agent `{}`)",
            instance.agent_id
        )];
        profile_lines.push(format!("Name: {}", profile.name));
        if !profile.title.trim().is_empty() {
            profile_lines.push(format!("Role: {}", profile.title));
        }
        if !profile.description.trim().is_empty() {
            profile_lines.push(format!("Persona & Mandate: {}", profile.description));
        }
        profile_lines.push(format!(
            "Isolated Home: {}",
            instance.agent_dir.display()
        ));
        let profile_section = profile_lines.join("\n");

        let others: Vec<AgentAddress> = self
            .list_instances()
            .into_iter()
            .filter(|inst| inst.bot_user_id != bot_user_id)
            .map(|inst| inst.agent_address())
            .collect();
        let root_str = self.agents_root.to_string_lossy();
        let directory_prompt =
            render_agent_directory_system_prompt(&others, room_groups, Some(root_str.as_ref()));

        let mut profile_update_prefix = None;
        if let Ok(mut snaps) = instance.profile_snapshots.lock() {
            let prev = snaps.get(&chat_id).cloned();
            let resolved = resolve_agent_profile_prompt_snapshot(
                prev.as_ref(),
                0,
                &profile_section,
                current_identity.clone(),
            );
            if prev.is_some() && resolved.announced_identity != current_identity {
                profile_update_prefix = Some(render_agent_profile_update(&current_identity));
                if let Some(updated) = persist_announced_agent_profile_snapshot(
                    Some(&resolved),
                    &resolved,
                    &current_identity,
                ) {
                    snaps.insert(chat_id, updated);
                }
            } else {
                snaps.insert(chat_id, resolved);
            }
        }

        let mut sections = vec![profile_section];
        if !base_system_prompt.trim().is_empty() {
            sections.push(base_system_prompt.trim().to_string());
        }
        sections.push(directory_prompt);
        if let Some(mentioned_ctx) = build_mentioned_agents_context(mentioned_teammates) {
            sections.push(mentioned_ctx);
        }

        (sections.join("\n\n"), profile_update_prefix)
    }

    /// Scans `text` for `@username` or `@agent_id` mentions referencing other Fabushi teammates.
    pub fn collect_mentioned_teammates(&self, caller_bot_id: i64, text: &str) -> Vec<AgentAddress> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'@' {
                let prev_ident =
                    i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
                if !prev_ident {
                    let start = i + 1;
                    let mut end = start;
                    while end < bytes.len()
                        && (bytes[end].is_ascii_alphanumeric()
                            || bytes[end] == b'_'
                            || bytes[end] == b'-')
                    {
                        end += 1;
                    }
                    if end > start {
                        let token = &text[start..end];
                        if let Some(inst) = self.get_by_agent_id_or_username(token) {
                            if inst.bot_user_id != caller_bot_id && seen.insert(inst.bot_user_id) {
                                out.push(inst.agent_address());
                            }
                        }
                    }
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
        out
    }
}
