use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::percent_encode_rfc3986;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HermesInlineButtonDto {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_app_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HermesUsageSummaryDto {
    #[serde(default)]
    pub total_turns: u64,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub estimated_cost_usd: f64,
    #[serde(default)]
    pub tool_calls: u64,
    #[serde(default)]
    pub by_model: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HermesStatusDto {
    #[serde(default)]
    pub healthy: bool,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub active_model: String,
    #[serde(default)]
    pub active_provider: String,
    #[serde(default)]
    pub api_mode: String,
    #[serde(default)]
    pub toolsets_count: usize,
    #[serde(default)]
    pub tools_count: usize,
    #[serde(default)]
    pub skills_count: usize,
    #[serde(default)]
    pub cron_count: usize,
    #[serde(default)]
    pub sessions_count: usize,
    #[serde(default)]
    pub pending_approvals_count: usize,
    #[serde(default)]
    pub heartbeat_enabled: bool,
    #[serde(default)]
    pub allow_local_fallback: bool,
    #[serde(default)]
    pub has_active_subscription: bool,
    #[serde(default)]
    pub has_configured_provider: bool,
    #[serde(default)]
    pub needs_provider_selection: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesCommandDescriptor {
    pub name: String,
    pub slash_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub syntax: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub accepts_args: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesCommandRequest {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HermesCommandResponse {
    #[serde(default)]
    pub handled: bool,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub reply_text: Option<String>,
    #[serde(default)]
    pub inline_keyboard: Vec<Vec<HermesInlineButtonDto>>,
    #[serde(default)]
    pub effects: Vec<Value>,
    #[serde(default)]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HermesTurnRequest {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yolo_mode: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HermesTurnResponse {
    pub turn_id: String,
    pub session_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub response: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub events: Vec<Value>,
    #[serde(default)]
    pub inline_keyboard: Vec<Vec<HermesInlineButtonDto>>,
    #[serde(default)]
    pub tool_calls_count: usize,
    #[serde(default)]
    pub usage: Option<HermesUsageSummaryDto>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesSessionDto {
    pub session_id: String,
    #[serde(default)]
    pub chat_id: Option<i64>,
    #[serde(default)]
    pub bot_id: Option<u64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub parent_session_id: Option<String>,
    #[serde(default)]
    pub branch_name: Option<String>,
    #[serde(default)]
    pub message_count: usize,
    #[serde(default)]
    pub token_count: u64,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HermesSessionDetailDto {
    pub session: HermesSessionDto,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub branches: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesSessionSearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesSessionBranchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_turn_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesSessionExportResponse {
    pub session_id: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub exported_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesMemoryEntryDto {
    pub id: String,
    #[serde(default)]
    pub key: String,
    pub content: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub char_count: usize,
    #[serde(default)]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesMemoryStateDto {
    #[serde(default)]
    pub entries: Vec<HermesMemoryEntryDto>,
    #[serde(default)]
    pub memory_markdown: String,
    #[serde(default)]
    pub user_markdown: String,
    #[serde(default)]
    pub total_chars: usize,
    #[serde(default)]
    pub char_limit: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesMemoryMutationRequest {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_text: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesSoulDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
    #[serde(default)]
    pub persona_name: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HermesHonchoStateDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub peer_name: Option<String>,
    #[serde(default)]
    pub session_notes: Vec<Value>,
    #[serde(default)]
    pub profile_summary: String,
    #[serde(default)]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesHonchoRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesSkillDto {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub approved: bool,
    #[serde(default)]
    pub security_status: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesSaveSkillRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesToolsetDto {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub risk_level: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesUpdateToolsetsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_toolsets: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesApprovalDto {
    pub approval_id: String,
    #[serde(default)]
    pub session_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub command_preview: String,
    #[serde(default)]
    pub risk_level: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub decided_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesApprovalDecisionRequest {
    pub decision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesTerminalProcessDto {
    pub process_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub started_at_ms: i64,
    #[serde(default)]
    pub output_tail: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesCronJobDto {
    pub job_id: String,
    pub name: String,
    pub schedule: String,
    pub prompt: String,
    #[serde(default)]
    pub chat_id: Option<i64>,
    #[serde(default)]
    pub bot_id: Option<u64>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub last_run_ms: Option<i64>,
    #[serde(default)]
    pub next_run_ms: Option<i64>,
    #[serde(default)]
    pub last_status: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesSaveCronRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub name: String,
    pub schedule: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesHeartbeatConfigDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub interval_secs: u64,
    #[serde(default)]
    pub chat_id: Option<i64>,
    #[serde(default)]
    pub bot_id: Option<u64>,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub last_beat_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesKanbanCardDto {
    pub card_id: String,
    pub board_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub column: String,
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub assigned_agent: Option<String>,
    #[serde(default)]
    pub position: i32,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesKanbanBoardDto {
    pub board_id: String,
    pub name: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub cards: Vec<HermesKanbanCardDto>,
    #[serde(default)]
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesCreateKanbanBoardRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesCreateKanbanCardRequest {
    pub board_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_agent: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesMoveKanbanCardRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesProviderConfigDto {
    pub provider_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub api_mode: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub has_api_key: bool,
    #[serde(default)]
    pub is_active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesSaveProviderRequest {
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_active: Option<bool>,
}

// ============================================================================
// Magpie Gateway & Provider Selection DTOs
// ============================================================================

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpiePresetRegionDto {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpiePresetDto {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini_url: Option<String>,
    #[serde(default)]
    pub default_protocol: String,
    #[serde(default)]
    pub supported_protocols: Vec<String>,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub suggested_models: Vec<String>,
    #[serde(default)]
    pub no_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_label: Option<String>,
    #[serde(default)]
    pub key_plans: bool,
    #[serde(default)]
    pub split_accounts: bool,
    #[serde(default)]
    pub regions: Vec<MagpiePresetRegionDto>,
    #[serde(default)]
    pub header_hints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    #[serde(default)]
    pub env_var_hints: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieProviderDto {
    pub provider_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub api_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub has_api_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masked_key: Option<String>,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    #[serde(default)]
    pub is_subscription: bool,
    #[serde(default)]
    pub no_key_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_strategy: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieSubscriptionDto {
    pub id: String,
    pub kind: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MagpieRoutingGroupMemberDto {
    pub target: String,
    #[serde(default = "default_magpie_weight")]
    pub weight: u32,
    #[serde(default = "default_magpie_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_1m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_1m: Option<f64>,
    #[serde(default)]
    pub free: bool,
}

fn default_magpie_weight() -> u32 {
    1
}

fn default_magpie_true() -> bool {
    true
}

fn default_sticky_ttl_ms() -> i64 {
    300_000
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MagpieRoutingGroupDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub strategy: String,
    #[serde(default)]
    pub members: Vec<MagpieRoutingGroupMemberDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_fast_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_deep_model: Option<String>,
    #[serde(default = "default_sticky_ttl_ms")]
    pub sticky_ttl_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieMaskingConfigDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_magpie_true")]
    pub mask_secrets: bool,
    #[serde(default = "default_magpie_true")]
    pub mask_personal: bool,
    #[serde(default)]
    pub custom_words: Vec<String>,
    #[serde(default)]
    pub allow_list: Vec<String>,
    #[serde(default = "default_mask_action")]
    pub default_action: String,
    #[serde(default)]
    pub kind_actions: BTreeMap<String, String>,
}

fn default_mask_action() -> String {
    "mask".to_string()
}

impl Default for MagpieMaskingConfigDto {
    fn default() -> Self {
        Self {
            enabled: false,
            mask_secrets: true,
            mask_personal: true,
            custom_words: Vec::new(),
            allow_list: Vec::new(),
            default_action: default_mask_action(),
            kind_actions: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieMaskingHitDto {
    pub kind: String,
    pub placeholder: String,
    pub action: String,
    pub preview_hint: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieMaskingPreviewResponse {
    #[serde(default)]
    pub masked_text: String,
    #[serde(default)]
    pub hits: Vec<MagpieMaskingHitDto>,
    #[serde(default)]
    pub hit_count: usize,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MagpieRouteTraceDto {
    #[serde(default)]
    pub timestamp_ms: i64,
    #[serde(default)]
    pub requested_model: String,
    #[serde(default)]
    pub resolved_provider_id: String,
    #[serde(default)]
    pub resolved_model: String,
    #[serde(default)]
    pub strategy: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub candidates_considered: usize,
    #[serde(default)]
    pub masking_hits: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MagpieStateDto {
    #[serde(default)]
    pub presets: Vec<MagpiePresetDto>,
    #[serde(default)]
    pub providers: Vec<MagpieProviderDto>,
    #[serde(default)]
    pub active_provider: String,
    #[serde(default)]
    pub active_model: String,
    #[serde(default)]
    pub allow_local_fallback: bool,
    #[serde(default)]
    pub has_active_subscription: bool,
    #[serde(default)]
    pub has_configured_provider: bool,
    #[serde(default)]
    pub needs_provider_selection: bool,
    #[serde(default)]
    pub subscriptions: Vec<MagpieSubscriptionDto>,
    #[serde(default)]
    pub routing_groups: Vec<MagpieRoutingGroupDto>,
    #[serde(default)]
    pub routing_strategy: String,
    #[serde(default)]
    pub masking: MagpieMaskingConfigDto,
    #[serde(default)]
    pub recent_route_traces: Vec<MagpieRouteTraceDto>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieSaveProviderRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub custom_headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_key_required: Option<bool>,
    #[serde(default = "default_magpie_true")]
    pub activate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_local_fallback: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MagpieSaveProviderResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub provider: MagpieProviderDto,
    #[serde(default)]
    pub state: MagpieStateDto,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MagpieDeleteProviderResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub deleted_id: String,
    #[serde(default)]
    pub state: MagpieStateDto,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieProbeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieProbeResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieSubscriptionsScanResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub subscriptions: Vec<MagpieSubscriptionDto>,
    #[serde(default)]
    pub scanned_at_ms: i64,
    #[serde(default)]
    pub has_active_subscription: bool,
    #[serde(default)]
    pub needs_provider_selection: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MagpieGroupsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<MagpieRoutingGroupDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_strategy: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MagpieGroupsResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub routing_groups: Vec<MagpieRoutingGroupDto>,
    #[serde(default)]
    pub routing_strategy: String,
    #[serde(default)]
    pub state: MagpieStateDto,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieMaskingPreviewRequest {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<MagpieMaskingConfigDto>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MagpieMaskingUpdateResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub masking: MagpieMaskingConfigDto,
    #[serde(default)]
    pub state: MagpieStateDto,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagpieImportRequest {
    pub raw: String,
    #[serde(default)]
    pub activate: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MagpieImportResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub imported_count: usize,
    #[serde(default)]
    pub imported_provider_ids: Vec<String>,
    #[serde(default)]
    pub state: MagpieStateDto,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesRollbackRequest {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesRollbackResponse {
    #[serde(default)]
    pub rolled_back: bool,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub restored_files: Vec<String>,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenAiChatCompletionsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub messages: Vec<OpenAiChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesTalkTurnRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<u64>,
    pub transcript: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barge_in: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesTalkTurnResponse {
    pub session_id: String,
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub transcript: String,
    #[serde(default)]
    pub reply_text: String,
    #[serde(default)]
    pub tts_segments: Vec<String>,
    #[serde(default)]
    pub bot_id: Option<u64>,
    #[serde(default)]
    pub chat_id: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesCallInviteRequest {
    pub callee_id: u64,
    #[serde(default)]
    pub is_video: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesCallInviteResponse {
    pub call_id: String,
    pub caller_id: u64,
    pub callee_id: u64,
    #[serde(default)]
    pub chat_id: Option<i64>,
    #[serde(default)]
    pub is_video: bool,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub duration_secs: u32,
    #[serde(default)]
    pub rating: Option<u8>,
    #[serde(default)]
    pub started_at_ms: i64,
    #[serde(default)]
    pub ended_at_ms: Option<i64>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub bot_name: Option<String>,
    #[serde(default)]
    pub talk_session_id: Option<String>,
    #[serde(default)]
    pub talk_state: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HermesRichChatDto {
    pub chat_id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub about: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub chat_type: u8,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub is_group: bool,
    #[serde(default)]
    pub is_channel: bool,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub is_forum: bool,
    #[serde(default)]
    pub unread_count: u32,
    #[serde(default)]
    pub last_message: Option<String>,
    #[serde(default)]
    pub peer_user_id: Option<u64>,
    #[serde(default)]
    pub peer_username: Option<String>,
    #[serde(default)]
    pub peer_display_name: Option<String>,
    #[serde(default)]
    pub peer_phone: Option<String>,
    #[serde(default)]
    pub peer_bio: Option<String>,
    #[serde(default)]
    pub is_archived: bool,
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub is_muted: bool,
    #[serde(default)]
    pub is_blocked: bool,
    #[serde(default)]
    pub message_ttl_seconds: u32,
    #[serde(default)]
    pub pts: u64,
    #[serde(default)]
    pub members_count: usize,
    #[serde(default)]
    pub menu_button_text: Option<String>,
    #[serde(default)]
    pub menu_button_url: Option<String>,
    #[serde(default)]
    pub bot_short_name: Option<String>,
    #[serde(default)]
    pub has_mini_app: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HermesRichMessageDto {
    pub message_id: i64,
    #[serde(default)]
    pub chat_id: i64,
    #[serde(default)]
    pub sender_id: u64,
    #[serde(default)]
    pub sender_name: Option<String>,
    #[serde(default)]
    pub sender_initials: Option<String>,
    #[serde(default)]
    pub is_bot_sender: bool,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub content: Option<Value>,
    #[serde(default)]
    pub date_ms: Option<i64>,
    #[serde(default)]
    pub reply_to: Option<i64>,
    #[serde(default)]
    pub is_outgoing: Option<bool>,
    #[serde(default)]
    pub inline_keyboard: Option<Vec<Vec<HermesInlineButtonDto>>>,
    #[serde(default)]
    pub bot_keyboard: Option<Vec<Vec<HermesInlineButtonDto>>>,
}

fn with_bot_scope(base: &str, bot_id: Option<u64>) -> String {
    match bot_id {
        Some(id) if base.contains('?') => format!("{base}&bot_id={id}"),
        Some(id) => format!("{base}?bot_id={id}"),
        None => base.to_string(),
    }
}

#[derive(Clone)]
pub struct HermesClient {
    http: HttpClient,
}

impl HermesClient {
    pub fn new(http: HttpClient) -> Self {
        Self { http }
    }

    pub fn http_client(&self) -> &HttpClient {
        &self.http
    }

    pub async fn status(&self, token: &str) -> Result<HermesStatusDto, HttpClientError> {
        self.status_for_bot(token, None).await
    }

    pub async fn status_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
    ) -> Result<HermesStatusDto, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/status", bot_id);
        self.http
            .request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn run_turn(
        &self,
        token: &str,
        req: &HermesTurnRequest,
    ) -> Result<HermesTurnResponse, HttpClientError> {
        self.http
            .request_json(Method::POST, "/api/v1/hermes/turns", Some(token), Some(req))
            .await
    }

    pub async fn get_turn_events(
        &self,
        token: &str,
        turn_id: &str,
    ) -> Result<Vec<Value>, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/turns/{}/events",
            percent_encode_rfc3986(turn_id)
        );
        self.http
            .request_vec_or_field(Method::GET, &path, Some(token), &["events", "items"])
            .await
    }

    pub async fn run_command(
        &self,
        token: &str,
        req: &HermesCommandRequest,
    ) -> Result<HermesCommandResponse, HttpClientError> {
        self.http
            .request_json(Method::POST, "/api/v1/hermes/command", Some(token), Some(req))
            .await
    }

    pub async fn list_commands(
        &self,
        token: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<HermesCommandDescriptor>, HttpClientError> {
        let path = match prefix {
            Some(p) if !p.is_empty() => {
                format!("/api/v1/hermes/commands?prefix={}", percent_encode_rfc3986(p))
            }
            _ => "/api/v1/hermes/commands".to_string(),
        };
        self.http
            .request_vec_or_field(Method::GET, &path, Some(token), &["commands", "items"])
            .await
    }

    pub async fn list_sessions(
        &self,
        token: &str,
    ) -> Result<Vec<HermesSessionDto>, HttpClientError> {
        self.list_sessions_for_bot(token, None).await
    }

    pub async fn list_sessions_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
    ) -> Result<Vec<HermesSessionDto>, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/sessions", bot_id);
        self.http
            .request_vec_or_field(Method::GET, &path, Some(token), &["sessions", "items"])
            .await
    }

    pub async fn get_session(
        &self,
        token: &str,
        session_id: &str,
    ) -> Result<HermesSessionDetailDto, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/sessions/{}",
            percent_encode_rfc3986(session_id)
        );
        self.http
            .request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn search_sessions(
        &self,
        token: &str,
        req: &HermesSessionSearchRequest,
    ) -> Result<Vec<Value>, HttpClientError> {
        let resp: Value = self
            .http
            .request_json(
                Method::POST,
                "/api/v1/hermes/sessions/search",
                Some(token),
                Some(req),
            )
            .await?;
        Ok(resp
            .get("results")
            .or_else(|| resp.get("sessions"))
            .or_else(|| resp.get("hits"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    pub async fn branch_session(
        &self,
        token: &str,
        session_id: &str,
        req: &HermesSessionBranchRequest,
    ) -> Result<HermesSessionDto, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/sessions/{}/branch",
            percent_encode_rfc3986(session_id)
        );
        self.http
            .request_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn compress_session(
        &self,
        token: &str,
        session_id: &str,
    ) -> Result<Value, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/sessions/{}/compress",
            percent_encode_rfc3986(session_id)
        );
        self.http
            .request_json(Method::POST, &path, Some(token), Some(&serde_json::json!({})))
            .await
    }

    pub async fn undo_session(
        &self,
        token: &str,
        session_id: &str,
    ) -> Result<Value, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/sessions/{}/undo",
            percent_encode_rfc3986(session_id)
        );
        self.http
            .request_json(Method::POST, &path, Some(token), Some(&serde_json::json!({})))
            .await
    }

    pub async fn export_session(
        &self,
        token: &str,
        session_id: &str,
        format: Option<&str>,
    ) -> Result<HermesSessionExportResponse, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/sessions/{}/export",
            percent_encode_rfc3986(session_id)
        );
        let body = serde_json::json!({ "format": format.unwrap_or("markdown") });
        self.http
            .request_json(Method::POST, &path, Some(token), Some(&body))
            .await
    }

    pub async fn get_memory(&self, token: &str) -> Result<HermesMemoryStateDto, HttpClientError> {
        self.get_memory_for_bot(token, None).await
    }

    pub async fn get_memory_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
    ) -> Result<HermesMemoryStateDto, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/memory", bot_id);
        self.http
            .request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn save_memory(
        &self,
        token: &str,
        memory_markdown: &str,
        user_markdown: Option<&str>,
    ) -> Result<HermesMemoryStateDto, HttpClientError> {
        self.save_memory_for_bot(token, None, memory_markdown, user_markdown)
            .await
    }

    pub async fn save_memory_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
        memory_markdown: &str,
        user_markdown: Option<&str>,
    ) -> Result<HermesMemoryStateDto, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/memory", bot_id);
        let body = serde_json::json!({
            "bot_id": bot_id,
            "memory_markdown": memory_markdown,
            "user_markdown": user_markdown,
        });
        self.http
            .request_json(Method::PUT, &path, Some(token), Some(&body))
            .await
    }

    pub async fn mutate_memory(
        &self,
        token: &str,
        req: &HermesMemoryMutationRequest,
    ) -> Result<HermesMemoryStateDto, HttpClientError> {
        self.mutate_memory_for_bot(token, None, req).await
    }

    pub async fn mutate_memory_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
        req: &HermesMemoryMutationRequest,
    ) -> Result<HermesMemoryStateDto, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/memory/action", bot_id);
        self.http
            .request_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn get_soul(
        &self,
        token: &str,
        bot_id: Option<u64>,
    ) -> Result<HermesSoulDto, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/soul", bot_id);
        self.http
            .request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn save_soul(
        &self,
        token: &str,
        soul: &HermesSoulDto,
    ) -> Result<HermesSoulDto, HttpClientError> {
        self.http
            .request_json(Method::PUT, "/api/v1/hermes/soul", Some(token), Some(soul))
            .await
    }

    pub async fn get_honcho(&self, token: &str) -> Result<HermesHonchoStateDto, HttpClientError> {
        self.get_honcho_for_bot(token, None).await
    }

    pub async fn get_honcho_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
    ) -> Result<HermesHonchoStateDto, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/honcho", bot_id);
        self.http
            .request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn post_honcho(
        &self,
        token: &str,
        req: &HermesHonchoRequest,
    ) -> Result<HermesHonchoStateDto, HttpClientError> {
        self.post_honcho_for_bot(token, None, req).await
    }

    pub async fn post_honcho_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
        req: &HermesHonchoRequest,
    ) -> Result<HermesHonchoStateDto, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/honcho", bot_id);
        self.http
            .request_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn list_skills(&self, token: &str) -> Result<Vec<HermesSkillDto>, HttpClientError> {
        self.list_skills_for_bot(token, None).await
    }

    pub async fn list_skills_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
    ) -> Result<Vec<HermesSkillDto>, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/skills", bot_id);
        self.http
            .request_vec_or_field(Method::GET, &path, Some(token), &["skills", "items"])
            .await
    }

    pub async fn get_skill(
        &self,
        token: &str,
        name: &str,
    ) -> Result<HermesSkillDto, HttpClientError> {
        let path = format!("/api/v1/hermes/skills/{}", percent_encode_rfc3986(name));
        self.http
            .request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn save_skill(
        &self,
        token: &str,
        req: &HermesSaveSkillRequest,
    ) -> Result<HermesSkillDto, HttpClientError> {
        self.save_skill_for_bot(token, None, req).await
    }

    pub async fn save_skill_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
        req: &HermesSaveSkillRequest,
    ) -> Result<HermesSkillDto, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/skills", bot_id);
        self.http
            .request_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn patch_skill(
        &self,
        token: &str,
        name: &str,
        patch: &Value,
    ) -> Result<HermesSkillDto, HttpClientError> {
        let path = format!("/api/v1/hermes/skills/{}", percent_encode_rfc3986(name));
        self.http
            .request_json(Method::PATCH, &path, Some(token), Some(patch))
            .await
    }

    pub async fn delete_skill(&self, token: &str, name: &str) -> Result<(), HttpClientError> {
        self.delete_skill_for_bot(token, None, name).await
    }

    pub async fn delete_skill_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
        name: &str,
    ) -> Result<(), HttpClientError> {
        let base = format!("/api/v1/hermes/skills/{}", percent_encode_rfc3986(name));
        let path = with_bot_scope(&base, bot_id);
        self.http
            .request_empty(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn approve_skill(
        &self,
        token: &str,
        name: &str,
    ) -> Result<HermesSkillDto, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/skills/{}/approve",
            percent_encode_rfc3986(name)
        );
        self.http
            .request_json(Method::POST, &path, Some(token), Some(&serde_json::json!({})))
            .await
    }

    pub async fn list_toolsets(
        &self,
        token: &str,
    ) -> Result<Vec<HermesToolsetDto>, HttpClientError> {
        self.list_toolsets_for_bot(token, None).await
    }

    pub async fn list_toolsets_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
    ) -> Result<Vec<HermesToolsetDto>, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/toolsets", bot_id);
        self.http
            .request_vec_or_field(Method::GET, &path, Some(token), &["toolsets", "items"])
            .await
    }

    pub async fn update_toolsets(
        &self,
        token: &str,
        req: &HermesUpdateToolsetsRequest,
    ) -> Result<Vec<HermesToolsetDto>, HttpClientError> {
        self.update_toolsets_for_bot(token, None, req).await
    }

    pub async fn update_toolsets_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
        req: &HermesUpdateToolsetsRequest,
    ) -> Result<Vec<HermesToolsetDto>, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/toolsets", bot_id);
        let resp: Value = self
            .http
            .request_json(Method::PUT, &path, Some(token), Some(req))
            .await?;
        let arr = resp
            .get("toolsets")
            .cloned()
            .unwrap_or(resp);
        Ok(serde_json::from_value(arr)?)
    }

    pub async fn list_approvals(
        &self,
        token: &str,
    ) -> Result<Vec<HermesApprovalDto>, HttpClientError> {
        self.list_approvals_for_bot(token, None).await
    }

    pub async fn list_approvals_for_bot(
        &self,
        token: &str,
        bot_id: Option<u64>,
    ) -> Result<Vec<HermesApprovalDto>, HttpClientError> {
        let path = with_bot_scope("/api/v1/hermes/approvals", bot_id);
        self.http
            .request_vec_or_field(Method::GET, &path, Some(token), &["approvals", "items"])
            .await
    }

    pub async fn decide_approval(
        &self,
        token: &str,
        approval_id: &str,
        req: &HermesApprovalDecisionRequest,
    ) -> Result<HermesApprovalDto, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/approvals/{}/decide",
            percent_encode_rfc3986(approval_id)
        );
        self.http
            .request_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn list_cron(&self, token: &str) -> Result<Vec<HermesCronJobDto>, HttpClientError> {
        self.http
            .request_vec_or_field(
                Method::GET,
                "/api/v1/hermes/cron",
                Some(token),
                &["jobs", "cron", "items"],
            )
            .await
    }

    pub async fn save_cron(
        &self,
        token: &str,
        req: &HermesSaveCronRequest,
    ) -> Result<HermesCronJobDto, HttpClientError> {
        self.http
            .request_json(Method::POST, "/api/v1/hermes/cron", Some(token), Some(req))
            .await
    }

    pub async fn update_cron(
        &self,
        token: &str,
        job_id: &str,
        patch: &Value,
    ) -> Result<HermesCronJobDto, HttpClientError> {
        let path = format!("/api/v1/hermes/cron/{}", percent_encode_rfc3986(job_id));
        self.http
            .request_json(Method::PATCH, &path, Some(token), Some(patch))
            .await
    }

    pub async fn delete_cron(&self, token: &str, job_id: &str) -> Result<(), HttpClientError> {
        let path = format!("/api/v1/hermes/cron/{}", percent_encode_rfc3986(job_id));
        self.http
            .request_empty(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn run_cron(&self, token: &str, job_id: &str) -> Result<Value, HttpClientError> {
        let path = format!("/api/v1/hermes/cron/{}/run", percent_encode_rfc3986(job_id));
        self.http
            .request_json(Method::POST, &path, Some(token), Some(&serde_json::json!({})))
            .await
    }

    pub async fn get_heartbeat(
        &self,
        token: &str,
    ) -> Result<HermesHeartbeatConfigDto, HttpClientError> {
        self.http
            .request_json(
                Method::GET,
                "/api/v1/hermes/heartbeat",
                Some(token),
                None::<&()>,
            )
            .await
    }

    pub async fn save_heartbeat(
        &self,
        token: &str,
        cfg: &HermesHeartbeatConfigDto,
    ) -> Result<HermesHeartbeatConfigDto, HttpClientError> {
        self.http
            .request_json(Method::POST, "/api/v1/hermes/heartbeat", Some(token), Some(cfg))
            .await
    }

    pub async fn kanban_boards(
        &self,
        token: &str,
    ) -> Result<Vec<HermesKanbanBoardDto>, HttpClientError> {
        self.http
            .request_vec_or_field(
                Method::GET,
                "/api/v1/hermes/kanban/boards",
                Some(token),
                &["boards", "items"],
            )
            .await
    }

    pub async fn kanban_get_board(
        &self,
        token: &str,
        board_id: &str,
    ) -> Result<HermesKanbanBoardDto, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/kanban/boards/{}",
            percent_encode_rfc3986(board_id)
        );
        self.http
            .request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn kanban_create_board(
        &self,
        token: &str,
        req: &HermesCreateKanbanBoardRequest,
    ) -> Result<HermesKanbanBoardDto, HttpClientError> {
        self.http
            .request_json(
                Method::POST,
                "/api/v1/hermes/kanban/boards",
                Some(token),
                Some(req),
            )
            .await
    }

    pub async fn kanban_create_card(
        &self,
        token: &str,
        req: &HermesCreateKanbanCardRequest,
    ) -> Result<HermesKanbanCardDto, HttpClientError> {
        self.http
            .request_json(
                Method::POST,
                "/api/v1/hermes/kanban/cards",
                Some(token),
                Some(req),
            )
            .await
    }

    pub async fn kanban_move_card(
        &self,
        token: &str,
        card_id: &str,
        req: &HermesMoveKanbanCardRequest,
    ) -> Result<HermesKanbanCardDto, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/kanban/cards/{}",
            percent_encode_rfc3986(card_id)
        );
        self.http
            .request_json(Method::PATCH, &path, Some(token), Some(req))
            .await
    }

    pub async fn kanban_delete_card(
        &self,
        token: &str,
        card_id: &str,
    ) -> Result<(), HttpClientError> {
        let path = format!(
            "/api/v1/hermes/kanban/cards/{}",
            percent_encode_rfc3986(card_id)
        );
        self.http
            .request_empty(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn usage_stats(
        &self,
        token: &str,
    ) -> Result<HermesUsageSummaryDto, HttpClientError> {
        self.http
            .request_json(Method::GET, "/api/v1/hermes/usage", Some(token), None::<&()>)
            .await
    }

    pub async fn list_providers(
        &self,
        token: &str,
    ) -> Result<Vec<HermesProviderConfigDto>, HttpClientError> {
        self.http
            .request_vec_or_field(
                Method::GET,
                "/api/v1/hermes/providers",
                Some(token),
                &["providers", "items"],
            )
            .await
    }

    pub async fn save_provider(
        &self,
        token: &str,
        req: &HermesSaveProviderRequest,
    ) -> Result<HermesProviderConfigDto, HttpClientError> {
        self.http
            .request_json(
                Method::PUT,
                "/api/v1/hermes/providers",
                Some(token),
                Some(req),
            )
            .await
    }

    pub async fn rollback_turn(
        &self,
        token: &str,
        req: &HermesRollbackRequest,
    ) -> Result<HermesRollbackResponse, HttpClientError> {
        self.http
            .request_json(
                Method::POST,
                "/api/v1/hermes/rollback",
                Some(token),
                Some(req),
            )
            .await
    }

    pub async fn terminal_processes(
        &self,
        token: &str,
    ) -> Result<Vec<HermesTerminalProcessDto>, HttpClientError> {
        self.http
            .request_vec_or_field(
                Method::GET,
                "/api/v1/hermes/terminal/processes",
                Some(token),
                &["processes", "items"],
            )
            .await
    }

    pub async fn chat_completions(
        &self,
        token: &str,
        req: &OpenAiChatCompletionsRequest,
    ) -> Result<Value, HttpClientError> {
        self.http
            .request_json(Method::POST, "/v1/chat/completions", Some(token), Some(req))
            .await
    }

    pub async fn openai_models(&self, token: &str) -> Result<Value, HttpClientError> {
        self.http
            .request_json(Method::GET, "/v1/models", Some(token), None::<&()>)
            .await
    }

    pub async fn acp_rpc(&self, token: &str, rpc_payload: &Value) -> Result<Value, HttpClientError> {
        self.http
            .request_json(
                Method::POST,
                "/api/v1/hermes/acp",
                Some(token),
                Some(rpc_payload),
            )
            .await
    }

    pub async fn talk_turn(
        &self,
        token: &str,
        req: &HermesTalkTurnRequest,
    ) -> Result<HermesTalkTurnResponse, HttpClientError> {
        self.http
            .request_json(
                Method::POST,
                "/api/v1/hermes/talk/turn",
                Some(token),
                Some(req),
            )
            .await
    }

    pub async fn call_invite(
        &self,
        token: &str,
        req: &HermesCallInviteRequest,
    ) -> Result<HermesCallInviteResponse, HttpClientError> {
        self.http
            .request_json(Method::POST, "/api/v1/calls/invite", Some(token), Some(req))
            .await
    }

    pub async fn list_chats_rich(
        &self,
        token: &str,
    ) -> Result<Vec<HermesRichChatDto>, HttpClientError> {
        self.http
            .request_vec_or_field(Method::GET, "/api/v1/chats", Some(token), &["chats"])
            .await
    }

    pub async fn get_chat_rich(
        &self,
        token: &str,
        chat_id: i64,
    ) -> Result<HermesRichChatDto, HttpClientError> {
        let path = format!("/api/v1/chats/{}", chat_id);
        self.http
            .request_json(Method::GET, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn messages_history_rich(
        &self,
        token: &str,
        chat_id: i64,
        limit: Option<usize>,
    ) -> Result<Vec<HermesRichMessageDto>, HttpClientError> {
        let path = match limit {
            Some(l) => format!("/api/v1/messages/history?chat_id={}&limit={}", chat_id, l),
            None => format!("/api/v1/messages/history?chat_id={}", chat_id),
        };
        self.http
            .request_vec_or_field(Method::GET, &path, Some(token), &["messages"])
            .await
    }

    pub async fn magpie_state(
        &self,
        token: &str,
    ) -> Result<MagpieStateDto, HttpClientError> {
        self.http
            .request_json(
                Method::GET,
                "/api/v1/hermes/magpie/state",
                Some(token),
                None::<&()>,
            )
            .await
    }

    pub async fn magpie_save_provider(
        &self,
        token: &str,
        req: &MagpieSaveProviderRequest,
    ) -> Result<MagpieSaveProviderResponse, HttpClientError> {
        self.http
            .request_json(
                Method::POST,
                "/api/v1/hermes/magpie/providers",
                Some(token),
                Some(req),
            )
            .await
    }

    pub async fn magpie_delete_provider(
        &self,
        token: &str,
        provider_id: &str,
    ) -> Result<MagpieDeleteProviderResponse, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/magpie/providers/{}",
            percent_encode_rfc3986(provider_id)
        );
        self.http
            .request_json(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn magpie_probe_provider(
        &self,
        token: &str,
        provider_id: &str,
        req: Option<&MagpieProbeRequest>,
    ) -> Result<MagpieProbeResponse, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/magpie/providers/{}/probe",
            percent_encode_rfc3986(provider_id)
        );
        self.http
            .request_json(Method::POST, &path, Some(token), req)
            .await
    }

    pub async fn magpie_scan_subscriptions(
        &self,
        token: &str,
    ) -> Result<MagpieSubscriptionsScanResponse, HttpClientError> {
        self.http
            .request_json(
                Method::POST,
                "/api/v1/hermes/magpie/subscriptions/scan",
                Some(token),
                None::<&()>,
            )
            .await
    }

    pub async fn magpie_upsert_group(
        &self,
        token: &str,
        req: &MagpieGroupsRequest,
    ) -> Result<MagpieGroupsResponse, HttpClientError> {
        self.http
            .request_json(
                Method::POST,
                "/api/v1/hermes/magpie/groups",
                Some(token),
                Some(req),
            )
            .await
    }

    pub async fn magpie_delete_group(
        &self,
        token: &str,
        group_id: &str,
    ) -> Result<MagpieGroupsResponse, HttpClientError> {
        let path = format!(
            "/api/v1/hermes/magpie/groups/{}",
            percent_encode_rfc3986(group_id)
        );
        self.http
            .request_json(Method::DELETE, &path, Some(token), None::<&()>)
            .await
    }

    pub async fn magpie_update_masking(
        &self,
        token: &str,
        cfg: &MagpieMaskingConfigDto,
    ) -> Result<MagpieMaskingUpdateResponse, HttpClientError> {
        self.http
            .request_json(
                Method::PUT,
                "/api/v1/hermes/magpie/masking",
                Some(token),
                Some(cfg),
            )
            .await
    }

    pub async fn magpie_preview_masking(
        &self,
        token: &str,
        req: &MagpieMaskingPreviewRequest,
    ) -> Result<MagpieMaskingPreviewResponse, HttpClientError> {
        self.http
            .request_json(
                Method::POST,
                "/api/v1/hermes/magpie/masking/preview",
                Some(token),
                Some(req),
            )
            .await
    }

    pub async fn magpie_import(
        &self,
        token: &str,
        req: &MagpieImportRequest,
    ) -> Result<MagpieImportResponse, HttpClientError> {
        self.http
            .request_json(
                Method::POST,
                "/api/v1/hermes/magpie/import",
                Some(token),
                Some(req),
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magpie_client_dtos_roundtrip_json() {
        let req = MagpieSaveProviderRequest {
            preset_id: Some("deepseek".to_string()),
            api_key: Some("sk-secret-123".to_string()),
            activate: true,
            ..Default::default()
        };
        let raw = serde_json::to_string(&req).expect("serialize save provider");
        let parsed: MagpieSaveProviderRequest =
            serde_json::from_str(&raw).expect("deserialize save provider");
        assert_eq!(req, parsed);

        let group_req = MagpieGroupsRequest {
            group: Some(MagpieRoutingGroupDto {
                id: "fast-pool".to_string(),
                name: "Fast Pool".to_string(),
                strategy: "smart".to_string(),
                members: vec![MagpieRoutingGroupMemberDto {
                    target: "deepseek/deepseek-chat".to_string(),
                    weight: 2,
                    enabled: true,
                    input_cost_per_1m: Some(0.14),
                    output_cost_per_1m: Some(0.28),
                    free: false,
                }],
                intent_fast_model: Some("deepseek/deepseek-chat".to_string()),
                intent_deep_model: Some("deepseek/deepseek-reasoner".to_string()),
                sticky_ttl_ms: 300_000,
            }),
            routing_strategy: Some("smart".to_string()),
        };
        let group_json = serde_json::to_value(&group_req).expect("serialize group req");
        let parsed_group: MagpieGroupsRequest =
            serde_json::from_value(group_json).expect("deserialize group req");
        assert_eq!(group_req, parsed_group);
    }
}
