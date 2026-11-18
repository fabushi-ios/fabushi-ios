use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::acp::{SubagentLifecycleStatus, SubagentLineage};
use crate::computer::{
    redact_cloud_secrets, AgentCloudServerConfigInput, AgentComputerExecInput,
    AgentComputerExecOutput, ComputerCloudProvider, CuaAction, CuaExecutor,
};
use crate::mcp::{ToolPermissionDock, ToolRiskLevel};
use crate::talk::TalkSessionState;
use crate::tasks::{CronExecutionRecord, CronRoutine};

use mahayana_host_runtime::agents::agent_messaging::{
    SAND_CREATE_AGENT_TOOL_NAME, SAND_SEND_TO_AGENT_TOOL_NAME, SAND_UPDATE_AGENT_TOOL_NAME,
};

use super::security::{scan_content_for_injection, ApprovalMode};
use super::toolsets::{
    HermesInlineButton, HermesToolContext, HermesToolResult, HermesUiAction,
    SubagentDelegationOutcome, SubagentDelegationRequest, ToolsetId,
};

pub const LOCAL_MEMORY_LIMIT_CHARS: usize = 2200;
pub const LOCAL_USER_LIMIT_CHARS: usize = 1375;
pub const MEMORY_ENTRY_DELIMITER: &str = "\n§\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanChecklistItem {
    pub text: String,
    pub checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanChecklist {
    pub id: String,
    pub title: String,
    pub items: Vec<KanbanChecklistItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanCard {
    pub id: String,
    pub board_id: String,
    pub column: String,
    pub position: usize,
    pub title: String,
    pub description: String,
    pub priority: String,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub comments: Vec<KanbanComment>,
    pub checklists: Vec<KanbanChecklist>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanBoard {
    pub id: String,
    pub name: String,
    pub description: String,
    pub columns: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalCronJob {
    pub routine: CronRoutine,
    pub prompt: String,
    pub target_chat_id: Option<i64>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationConnection {
    pub name: String,
    pub provider: String,
    pub endpoint: Option<String>,
    pub masked_secret: Option<String>,
    pub connected: bool,
    pub updated_at: DateTime<Utc>,
    pub last_tested_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSkillEntry {
    pub name: String,
    pub description: String,
    pub content: String,
    pub tags: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopPanelState {
    pub active_destination: String,
    pub open_panels: Vec<String>,
    pub last_action: Option<String>,
}

impl Default for DesktopPanelState {
    fn default() -> Self {
        Self {
            active_destination: "chats".into(),
            open_panels: vec!["chat_stage".into()],
            last_action: None,
        }
    }
}

/// Self-contained stateful agentic store used standalone or alongside `HermesStateBridge`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LocalAgenticStore {
    pub boards: HashMap<String, KanbanBoard>,
    pub cards: HashMap<String, KanbanCard>,
    pub cron_jobs: HashMap<String, LocalCronJob>,
    pub connections: HashMap<String, IntegrationConnection>,
    pub memory_entries: Vec<String>,
    pub user_entries: Vec<String>,
    pub skills: HashMap<String, LocalSkillEntry>,
    pub honcho_observations: HashMap<String, Vec<(String, String, DateTime<Utc>)>>,
    pub session_notes: Vec<(String, String, DateTime<Utc>)>,
    pub lineage: SubagentLineage,
    pub desktop_state: DesktopPanelState,
    pub ui_action_log: Vec<HermesUiAction>,
}

impl LocalAgenticStore {
    pub fn new() -> Self {
        Self::default()
    }
}

pub async fn execute_agentic_tool(
    name: &str,
    args: Value,
    ctx: &HermesToolContext,
) -> HermesToolResult {
    match name {
        "delegate_task" => tool_delegate_task(args, ctx).await,
        "cronjob_manage" => tool_cronjob_manage(args, ctx).await,
        "kanban_list_boards"
        | "kanban_create_board"
        | "kanban_get_board"
        | "kanban_create_card"
        | "kanban_get_card"
        | "kanban_update_card"
        | "kanban_move_card"
        | "kanban_delete_card"
        | "kanban_assign_card"
        | "kanban_add_comment"
        | "kanban_list_comments"
        | "kanban_add_checklist"
        | "kanban_toggle_checklist"
        | "kanban_search_cards" => tool_kanban(name, args, ctx).await,
        "clarify" => tool_clarify(args, ctx).await,
        "desktop_ui_inspect" => tool_desktop_ui_inspect(ctx).await,
        "desktop_ui_act" => tool_desktop_ui_act(args, ctx).await,
        "computer_use" => tool_computer_use(args, ctx),
        "computer_list" => tool_computer_list(ctx).await,
        "computer_bind" => tool_computer_bind(args, ctx).await,
        "computer_configure_cloud" => tool_computer_configure_cloud(args, ctx).await,
        "computer_exec" => tool_computer_exec(args, ctx).await,
        "computer_file" => tool_computer_file(args, ctx).await,
        "computer_screen" => tool_computer_screen(args, ctx).await,
        "voice_session" => tool_voice_session(args, ctx),
        "manage_connections" => tool_manage_connections(args, ctx).await,
        "think" | "reason" => tool_reasoning(name, args),
        "mcp_list_tools" => tool_mcp_list_tools(ctx),
        "mcp_call" => tool_mcp_call(args, ctx),
        "session_search" => tool_session_search(args, ctx).await,
        "memory" => tool_memory(args, ctx).await,
        "skill_manage" => tool_skill_manage(args, ctx).await,
        "honcho_profile" => tool_honcho_profile(args, ctx).await,
        name if name == SAND_SEND_TO_AGENT_TOOL_NAME => tool_fabushi_send_to_agent(args, ctx),
        name if name == SAND_CREATE_AGENT_TOOL_NAME => tool_fabushi_create_agent(args, ctx),
        name if name == SAND_UPDATE_AGENT_TOOL_NAME => tool_fabushi_update_agent(args, ctx),
        _ => HermesToolResult::err(name, format!("Unknown agentic tool '{name}'")),
    }
}

fn tool_fabushi_send_to_agent(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(handler) = ctx.teammate_handler.as_ref() else {
        return HermesToolResult::err(
            SAND_SEND_TO_AGENT_TOOL_NAME,
            "Fabushi teammate runtime handler is not attached",
        );
    };
    let target_id = args
        .get("targetId")
        .or_else(|| args.get("target_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if target_id.is_empty() || message.is_empty() {
        return HermesToolResult::err(
            SAND_SEND_TO_AGENT_TOOL_NAME,
            "SendToAgent requires non-empty targetId and message",
        );
    }
    let priority = args
        .get("priority")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let from_agent_id = ctx
        .bound_agent_id
        .as_deref()
        .map(str::to_string)
        .or_else(|| ctx.bot_id.map(|id| format!("bot-{id}")))
        .unwrap_or_else(|| "main".to_string());

    match handler.send_to_agent(
        &from_agent_id,
        ctx.chat_id,
        target_id,
        message,
        priority,
        ctx.caller_depth,
    ) {
        Ok(payload) => HermesToolResult::ok(
            SAND_SEND_TO_AGENT_TOOL_NAME,
            serde_json::to_string_pretty(&payload).unwrap_or_default(),
            Some(payload),
        ),
        Err(err) => HermesToolResult::err(SAND_SEND_TO_AGENT_TOOL_NAME, err),
    }
}

fn tool_fabushi_create_agent(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(handler) = ctx.teammate_handler.as_ref() else {
        return HermesToolResult::err(
            SAND_CREATE_AGENT_TOOL_NAME,
            "Fabushi teammate runtime handler is not attached",
        );
    };
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let description = args
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if name.is_empty() {
        return HermesToolResult::err(
            SAND_CREATE_AGENT_TOOL_NAME,
            "CreateAgent requires a non-empty name",
        );
    }
    let avatar_shape = args
        .get("avatarShape")
        .or_else(|| args.get("avatar_shape"))
        .and_then(Value::as_str);
    let avatar_color = args
        .get("avatarColor")
        .or_else(|| args.get("avatar_color"))
        .and_then(Value::as_str);
    let creator_id = ctx
        .bound_agent_id
        .as_deref()
        .map(str::to_string)
        .or_else(|| ctx.bot_id.map(|id| format!("bot-{id}")))
        .unwrap_or_else(|| "main".to_string());

    match handler.create_agent(
        &creator_id,
        ctx.chat_id,
        name,
        description,
        avatar_shape,
        avatar_color,
    ) {
        Ok(payload) => HermesToolResult::ok(
            SAND_CREATE_AGENT_TOOL_NAME,
            serde_json::to_string_pretty(&payload).unwrap_or_default(),
            Some(payload),
        ),
        Err(err) => HermesToolResult::err(SAND_CREATE_AGENT_TOOL_NAME, err),
    }
}

fn tool_fabushi_update_agent(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(handler) = ctx.teammate_handler.as_ref() else {
        return HermesToolResult::err(
            SAND_UPDATE_AGENT_TOOL_NAME,
            "Fabushi teammate runtime handler is not attached",
        );
    };
    let caller_id = ctx
        .bound_agent_id
        .as_deref()
        .map(str::to_string)
        .or_else(|| ctx.bot_id.map(|id| format!("bot-{id}")))
        .unwrap_or_else(|| "main".to_string());
    let target_id = args
        .get("targetId")
        .or_else(|| args.get("target_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&caller_id);

    let name = args.get("name").and_then(Value::as_str);
    let description = args.get("description").and_then(Value::as_str);
    let avatar_shape = args
        .get("avatarShape")
        .or_else(|| args.get("avatar_shape"))
        .and_then(Value::as_str);
    let avatar_color = args
        .get("avatarColor")
        .or_else(|| args.get("avatar_color"))
        .and_then(Value::as_str);

    match handler.update_agent(
        &caller_id,
        target_id,
        name,
        description,
        avatar_shape,
        avatar_color,
    ) {
        Ok(payload) => HermesToolResult::ok(
            SAND_UPDATE_AGENT_TOOL_NAME,
            serde_json::to_string_pretty(&payload).unwrap_or_default(),
            Some(payload),
        ),
        Err(err) => HermesToolResult::err(SAND_UPDATE_AGENT_TOOL_NAME, err),
    }
}

// ----------------------------------------------------------------------------
// 1. Subagent Delegation (`delegate_task`)
// ----------------------------------------------------------------------------

async fn tool_delegate_task(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let max_depth = args
        .get("max_depth")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(ctx.max_depth)
        .min(8);

    if ctx.caller_depth >= max_depth {
        return HermesToolResult::err(
            "delegate_task",
            format!(
                "Subagent delegation depth limit reached (depth={} >= max_depth={max_depth})",
                ctx.caller_depth
            ),
        );
    }

    let max_concurrent = args
        .get("max_concurrent")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(ctx.max_concurrent_subagents)
        .clamp(1, 16);

    let mut requests: Vec<SubagentDelegationRequest> = Vec::new();
    if let Some(tasks) = args.get("tasks").and_then(|v| v.as_array()) {
        for (idx, t) in tasks.iter().enumerate() {
            let prompt = t
                .get("prompt")
                .or_else(|| t.get("task"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if prompt.is_empty() {
                continue;
            }
            let id = t
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("sub_{}_{}", idx + 1, uuid::Uuid::new_v4().simple()));
            let role = t
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("worker")
                .to_string();
            let toolsets = parse_toolset_list(t.get("toolsets"));
            let model = t
                .get("model")
                .or_else(|| args.get("model"))
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            requests.push(SubagentDelegationRequest {
                task_id: id,
                parent_session_id: ctx.session_id.clone(),
                role,
                prompt,
                toolsets,
                model,
                depth: ctx.caller_depth + 1,
                max_depth,
            });
        }
    } else if let Some(prompt) = args
        .get("task")
        .or_else(|| args.get("prompt"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let role = args
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("specialist")
            .to_string();
        let toolsets = parse_toolset_list(args.get("toolsets"));
        let model = args
            .get("model")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);
        requests.push(SubagentDelegationRequest {
            task_id: format!("sub_{}", uuid::Uuid::new_v4().simple()),
            parent_session_id: ctx.session_id.clone(),
            role,
            prompt: prompt.to_string(),
            toolsets,
            model,
            depth: ctx.caller_depth + 1,
            max_depth,
        });
    }

    if requests.is_empty() {
        return HermesToolResult::err(
            "delegate_task",
            "Provide 'task'/'prompt' or non-empty 'tasks' array",
        );
    }

    if let Ok(mut store) = ctx.agentic_store.lock() {
        for req in &requests {
            store.lineage.add_agent(
                req.task_id.clone(),
                Some(ctx.session_id.clone()),
                req.role.clone(),
            );
        }
    }
    if let Some(bridge) = &ctx.state_bridge {
        for req in &requests {
            let _ = bridge
                .record_lineage(&req.task_id, Some(&ctx.session_id), &req.role, "running")
                .await;
        }
    }

    let mut all_outcomes: Vec<SubagentDelegationOutcome> = Vec::with_capacity(requests.len());
    for batch in requests.chunks(max_concurrent) {
        let mut join_set = JoinSet::new();
        for req in batch.iter().cloned() {
            let child_ctx = ctx.child_for_subagent(
                format!("{}:{}", ctx.session_id, req.task_id),
                if req.toolsets.is_empty() {
                    None
                } else {
                    Some(&req.toolsets)
                },
            );
            join_set.spawn(async move { run_single_subagent(req, child_ctx).await });
        }
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok(outcome) => all_outcomes.push(outcome),
                Err(e) => all_outcomes.push(SubagentDelegationOutcome {
                    task_id: "unknown".into(),
                    role: "worker".into(),
                    success: false,
                    summary: format!("Subagent task panicked/aborted: {e}"),
                    tool_calls_used: 0,
                    duration_ms: 0,
                }),
            }
        }
    }

    if let Ok(mut store) = ctx.agentic_store.lock() {
        for outcome in &all_outcomes {
            store.lineage.mark_completed(
                &outcome.task_id,
                if outcome.success {
                    SubagentLifecycleStatus::Done
                } else {
                    SubagentLifecycleStatus::Error
                },
            );
        }
    }
    if let Some(bridge) = &ctx.state_bridge {
        for outcome in &all_outcomes {
            let st = if outcome.success { "done" } else { "error" };
            let _ = bridge
                .record_lineage(&outcome.task_id, Some(&ctx.session_id), &outcome.role, st)
                .await;
        }
    }

    let all_ok = all_outcomes.iter().all(|o| o.success);
    let synthesized = all_outcomes
        .iter()
        .map(|o| format!("[{} ({})]: {}", o.role, o.task_id, o.summary))
        .collect::<Vec<_>>()
        .join("\n");

    let mut res = HermesToolResult::ok(
        "delegate_task",
        format!(
            "Completed {} subagent task(s) (depth={}/{}):\n{}",
            all_outcomes.len(),
            ctx.caller_depth + 1,
            max_depth,
            synthesized
        ),
        Some(json!({
            "count": all_outcomes.len(),
            "depth": ctx.caller_depth + 1,
            "max_depth": max_depth,
            "max_concurrent": max_concurrent,
            "outcomes": all_outcomes,
        })),
    );
    if !all_ok {
        res.success = false;
        res.error = Some("One or more delegated subagents failed".into());
    }
    res
}

async fn run_single_subagent(
    req: SubagentDelegationRequest,
    child_ctx: HermesToolContext,
) -> SubagentDelegationOutcome {
    let start = std::time::Instant::now();
    if let Some(bridge) = &child_ctx.state_bridge {
        match bridge.delegate_subagent(req.clone()).await {
            Ok(outcome) => return outcome,
            Err(e) => {
                return SubagentDelegationOutcome {
                    task_id: req.task_id,
                    role: req.role,
                    success: false,
                    summary: e,
                    tool_calls_used: 0,
                    duration_ms: start.elapsed().as_millis() as u64,
                };
            }
        }
    }

    // Standalone deterministic subagent synthesis with isolated context
    let active_tools = child_ctx
        .toolsets
        .read()
        .map(|r| r.active_specs().len())
        .unwrap_or(0);
    SubagentDelegationOutcome {
        task_id: req.task_id,
        role: req.role.clone(),
        success: true,
        summary: format!(
            "Subagent '{}' (depth {}, {} isolated tools) synthesized task: {}",
            req.role, req.depth, active_tools, req.prompt
        ),
        tool_calls_used: 0,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

fn parse_toolset_list(val: Option<&Value>) -> Vec<ToolsetId> {
    val.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().and_then(ToolsetId::from_str))
                .collect()
        })
        .unwrap_or_default()
}

// ----------------------------------------------------------------------------
// 2. Cronjob Management (`cronjob_manage`)
// ----------------------------------------------------------------------------

async fn tool_cronjob_manage(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    if let Some(bridge) = &ctx.state_bridge {
        return match bridge.cron_op(&args).await {
            Ok(val) => HermesToolResult::ok(
                "cronjob_manage",
                serde_json::to_string_pretty(&val).unwrap_or_default(),
                Some(val),
            ),
            Err(e) => HermesToolResult::err("cronjob_manage", e),
        };
    }

    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("list")
        .to_ascii_lowercase();
    let Ok(mut store) = ctx.agentic_store.lock() else {
        return HermesToolResult::err("cronjob_manage", "LocalAgenticStore lock poisoned");
    };

    match action.as_str() {
        "create" => {
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unnamed Cron Job")
                .to_string();
            let schedule = args
                .get("schedule")
                .and_then(|v| v.as_str())
                .unwrap_or("@every 1h")
                .to_string();
            let prompt = args
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let timezone = args
                .get("timezone")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let id = format!("cron_{}", uuid::Uuid::new_v4().simple());
            let mut routine = CronRoutine::new(id.clone(), schedule.clone())
                .with_name(name.clone())
                .with_time_zone(timezone);
            if !routine.validate_expression() {
                return HermesToolResult::err(
                    "cronjob_manage",
                    format!("Invalid cron or @every schedule expression: '{schedule}'"),
                );
            }
            let next_run = routine.compute_next_run(Utc::now());
            let job = LocalCronJob {
                routine,
                prompt,
                target_chat_id: args.get("target_chat_id").and_then(|v| v.as_i64()).or(ctx.chat_id),
                model: args.get("model").and_then(|v| v.as_str()).map(ToString::to_string),
            };
            store.cron_jobs.insert(id.clone(), job.clone());
            HermesToolResult::ok(
                "cronjob_manage",
                format!("Created cron job '{name}' ({id}) next_run={next_run:?}"),
                Some(json!({ "id": id, "job": job })),
            )
        }
        "list" => {
            let mut jobs: Vec<&LocalCronJob> = store.cron_jobs.values().collect();
            jobs.sort_by(|a, b| a.routine.routine_id.cmp(&b.routine.routine_id));
            HermesToolResult::ok(
                "cronjob_manage",
                serde_json::to_string_pretty(&jobs).unwrap_or_default(),
                Some(json!({ "jobs": jobs })),
            )
        }
        "get" | "history" | "preview" => {
            let Some(id) = args.get("id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err("cronjob_manage", "Missing required 'id'");
            };
            let Some(job) = store.cron_jobs.get(id) else {
                return HermesToolResult::err("cronjob_manage", format!("Cron job '{id}' not found"));
            };
            if action == "preview" {
                let summary = job.routine.preview_next_7_days(Utc::now());
                let summary_json = serde_json::json!({
                    "scheduled_fires_next_7_days": summary.scheduled_fires_next_7_days,
                    "fires_on_weekend": summary.fires_on_weekend,
                    "fires_overnight": summary.fires_overnight,
                });
                return HermesToolResult::ok(
                    "cronjob_manage",
                    serde_json::to_string_pretty(&summary_json).unwrap_or_default(),
                    Some(summary_json),
                );
            }
            if action == "history" {
                return HermesToolResult::ok(
                    "cronjob_manage",
                    serde_json::to_string_pretty(&job.routine.execution_records).unwrap_or_default(),
                    Some(json!({ "id": id, "history": job.routine.execution_records })),
                );
            }
            HermesToolResult::ok(
                "cronjob_manage",
                serde_json::to_string_pretty(job).unwrap_or_default(),
                Some(serde_json::to_value(job).unwrap_or_default()),
            )
        }
        "pause" | "resume" => {
            let Some(id) = args.get("id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err("cronjob_manage", "Missing required 'id'");
            };
            let Some(job) = store.cron_jobs.get_mut(id) else {
                return HermesToolResult::err("cronjob_manage", format!("Cron job '{id}' not found"));
            };
            let enabled = action == "resume";
            job.routine.toggle_enabled(enabled);
            HermesToolResult::ok(
                "cronjob_manage",
                format!("Cron job '{id}' enabled={enabled}"),
                Some(json!({ "id": id, "enabled": enabled })),
            )
        }
        "delete" => {
            let Some(id) = args.get("id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err("cronjob_manage", "Missing required 'id'");
            };
            let removed = store.cron_jobs.remove(id);
            if removed.is_none() {
                return HermesToolResult::err("cronjob_manage", format!("Cron job '{id}' not found"));
            }
            HermesToolResult::ok(
                "cronjob_manage",
                format!("Deleted cron job '{id}'"),
                Some(json!({ "deleted": id })),
            )
        }
        "run_now" => {
            let Some(id) = args.get("id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err("cronjob_manage", "Missing required 'id'");
            };
            let Some(job) = store.cron_jobs.get_mut(id) else {
                return HermesToolResult::err("cronjob_manage", format!("Cron job '{id}' not found"));
            };
            let now = Utc::now();
            let rec = CronExecutionRecord {
                run_id: format!("run_{}", uuid::Uuid::new_v4().simple()),
                started_at: now,
                finished_at: Some(now),
                status: "completed".into(),
                output_summary: Some(format!("Triggered prompt: {}", job.prompt)),
            };
            job.routine.record_execution(rec.clone());
            job.routine.compute_next_run(now);
            HermesToolResult::ok(
                "cronjob_manage",
                format!("Executed cron job '{id}' now (run_id={})", rec.run_id),
                Some(serde_json::to_value(&rec).unwrap_or_default()),
            )
        }
        _ => HermesToolResult::err("cronjob_manage", format!("Unsupported cron action '{action}'")),
    }
}

// ----------------------------------------------------------------------------
// 3. All 14 Kanban Tools
// ----------------------------------------------------------------------------

async fn tool_kanban(name: &str, args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    if let Some(bridge) = &ctx.state_bridge {
        return match bridge.kanban_op(name, &args).await {
            Ok(val) => HermesToolResult::ok(
                name,
                serde_json::to_string_pretty(&val).unwrap_or_default(),
                Some(val),
            ),
            Err(e) => HermesToolResult::err(name, e),
        };
    }

    let Ok(mut store) = ctx.agentic_store.lock() else {
        return HermesToolResult::err(name, "LocalAgenticStore lock poisoned");
    };

    match name {
        "kanban_list_boards" => {
            let mut boards: Vec<&KanbanBoard> = store.boards.values().collect();
            boards.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            HermesToolResult::ok(
                name,
                serde_json::to_string_pretty(&boards).unwrap_or_default(),
                Some(json!({ "boards": boards })),
            )
        }
        "kanban_create_board" => {
            let Some(b_name) = args.get("name").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) else {
                return HermesToolResult::err(name, "Missing required 'name'");
            };
            let cols: Vec<String> = args
                .get("columns")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(ToString::to_string)).collect())
                .filter(|v: &Vec<String>| !v.is_empty())
                .unwrap_or_else(|| {
                    vec![
                        "Backlog".into(),
                        "Todo".into(),
                        "In Progress".into(),
                        "Review".into(),
                        "Done".into(),
                    ]
                });
            let board = KanbanBoard {
                id: format!("board_{}", uuid::Uuid::new_v4().simple()),
                name: b_name.to_string(),
                description: args.get("description").and_then(|v| v.as_str()).unwrap_or("").into(),
                columns: cols,
                created_at: Utc::now(),
            };
            store.boards.insert(board.id.clone(), board.clone());
            HermesToolResult::ok(
                name,
                format!("Created Kanban board '{}' ({})", board.name, board.id),
                Some(serde_json::to_value(&board).unwrap_or_default()),
            )
        }
        "kanban_get_board" => {
            let Some(board_id) = args.get("board_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'board_id'");
            };
            let Some(board) = store.boards.get(board_id).cloned() else {
                return HermesToolResult::err(name, format!("Board '{board_id}' not found"));
            };
            let mut cards: Vec<&KanbanCard> = store
                .cards
                .values()
                .filter(|c| c.board_id == board_id)
                .collect();
            cards.sort_by_key(|c| c.position);
            let payload = json!({ "board": board, "cards": cards });
            HermesToolResult::ok(
                name,
                serde_json::to_string_pretty(&payload).unwrap_or_default(),
                Some(payload),
            )
        }
        "kanban_create_card" => {
            let Some(board_id) = args.get("board_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'board_id'");
            };
            let Some(title) = args.get("title").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'title'");
            };
            let default_col = store
                .boards
                .get(board_id)
                .and_then(|b| b.columns.first().cloned())
                .unwrap_or_else(|| "Todo".into());
            if !store.boards.contains_key(board_id) {
                return HermesToolResult::err(name, format!("Board '{board_id}' not found"));
            }
            let column = args
                .get("column")
                .and_then(|v| v.as_str())
                .unwrap_or(&default_col)
                .to_string();
            let pos = store
                .cards
                .values()
                .filter(|c| c.board_id == board_id && c.column == column)
                .count();
            let labels = args
                .get("labels")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(ToString::to_string)).collect())
                .unwrap_or_default();
            let now = Utc::now();
            let card = KanbanCard {
                id: format!("card_{}", uuid::Uuid::new_v4().simple()),
                board_id: board_id.to_string(),
                column,
                position: pos,
                title: title.to_string(),
                description: args.get("description").and_then(|v| v.as_str()).unwrap_or("").into(),
                priority: args.get("priority").and_then(|v| v.as_str()).unwrap_or("medium").into(),
                labels,
                assignee: args.get("assignee").and_then(|v| v.as_str()).map(ToString::to_string),
                comments: Vec::new(),
                checklists: Vec::new(),
                created_at: now,
                updated_at: now,
            };
            store.cards.insert(card.id.clone(), card.clone());
            HermesToolResult::ok(
                name,
                format!("Created card '{}' ({}) in '{}'", card.title, card.id, card.column),
                Some(serde_json::to_value(&card).unwrap_or_default()),
            )
        }
        "kanban_get_card" => {
            let Some(card_id) = args.get("card_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'card_id'");
            };
            let Some(card) = store.cards.get(card_id) else {
                return HermesToolResult::err(name, format!("Card '{card_id}' not found"));
            };
            HermesToolResult::ok(
                name,
                serde_json::to_string_pretty(card).unwrap_or_default(),
                Some(serde_json::to_value(card).unwrap_or_default()),
            )
        }
        "kanban_update_card" => {
            let Some(card_id) = args.get("card_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'card_id'");
            };
            let Some(card) = store.cards.get_mut(card_id) else {
                return HermesToolResult::err(name, format!("Card '{card_id}' not found"));
            };
            if let Some(t) = args.get("title").and_then(|v| v.as_str()) {
                card.title = t.to_string();
            }
            if let Some(d) = args.get("description").and_then(|v| v.as_str()) {
                card.description = d.to_string();
            }
            if let Some(p) = args.get("priority").and_then(|v| v.as_str()) {
                card.priority = p.to_string();
            }
            if let Some(arr) = args.get("labels").and_then(|v| v.as_array()) {
                card.labels = arr.iter().filter_map(|x| x.as_str().map(ToString::to_string)).collect();
            }
            card.updated_at = Utc::now();
            HermesToolResult::ok(
                name,
                format!("Updated card '{}'", card.id),
                Some(serde_json::to_value(card).unwrap_or_default()),
            )
        }
        "kanban_move_card" => {
            let Some(card_id) = args.get("card_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'card_id'");
            };
            let Some(column) = args.get("column").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'column'");
            };
            let Some(card) = store.cards.get_mut(card_id) else {
                return HermesToolResult::err(name, format!("Card '{card_id}' not found"));
            };
            card.column = column.to_string();
            if let Some(pos) = args.get("position").and_then(|v| v.as_u64()) {
                card.position = pos as usize;
            }
            card.updated_at = Utc::now();
            HermesToolResult::ok(
                name,
                format!("Moved card '{}' to '{}'", card.id, card.column),
                Some(serde_json::to_value(card).unwrap_or_default()),
            )
        }
        "kanban_delete_card" => {
            let Some(card_id) = args.get("card_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'card_id'");
            };
            if store.cards.remove(card_id).is_none() {
                return HermesToolResult::err(name, format!("Card '{card_id}' not found"));
            }
            HermesToolResult::ok(
                name,
                format!("Deleted card '{card_id}'"),
                Some(json!({ "deleted": card_id })),
            )
        }
        "kanban_assign_card" => {
            let Some(card_id) = args.get("card_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'card_id'");
            };
            let Some(assignee) = args.get("assignee").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'assignee'");
            };
            let Some(card) = store.cards.get_mut(card_id) else {
                return HermesToolResult::err(name, format!("Card '{card_id}' not found"));
            };
            card.assignee = Some(assignee.to_string());
            card.updated_at = Utc::now();
            HermesToolResult::ok(
                name,
                format!("Assigned card '{card_id}' to '{assignee}'"),
                Some(serde_json::to_value(card).unwrap_or_default()),
            )
        }
        "kanban_add_comment" => {
            let Some(card_id) = args.get("card_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'card_id'");
            };
            let Some(body) = args.get("body").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'body'");
            };
            let author = args.get("author").and_then(|v| v.as_str()).unwrap_or("hermes");
            let Some(card) = store.cards.get_mut(card_id) else {
                return HermesToolResult::err(name, format!("Card '{card_id}' not found"));
            };
            let comment = KanbanComment {
                id: format!("cmt_{}", uuid::Uuid::new_v4().simple()),
                author: author.to_string(),
                body: body.to_string(),
                created_at: Utc::now(),
            };
            card.comments.push(comment.clone());
            card.updated_at = Utc::now();
            HermesToolResult::ok(
                name,
                format!("Added comment {} to card '{card_id}'", comment.id),
                Some(serde_json::to_value(&comment).unwrap_or_default()),
            )
        }
        "kanban_list_comments" => {
            let Some(card_id) = args.get("card_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'card_id'");
            };
            let Some(card) = store.cards.get(card_id) else {
                return HermesToolResult::err(name, format!("Card '{card_id}' not found"));
            };
            HermesToolResult::ok(
                name,
                serde_json::to_string_pretty(&card.comments).unwrap_or_default(),
                Some(json!({ "card_id": card_id, "comments": card.comments })),
            )
        }
        "kanban_add_checklist" => {
            let Some(card_id) = args.get("card_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'card_id'");
            };
            let Some(title) = args.get("title").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'title'");
            };
            let items: Vec<KanbanChecklistItem> = args
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .map(|t| KanbanChecklistItem {
                            text: t.to_string(),
                            checked: false,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let Some(card) = store.cards.get_mut(card_id) else {
                return HermesToolResult::err(name, format!("Card '{card_id}' not found"));
            };
            let checklist = KanbanChecklist {
                id: format!("chk_{}", uuid::Uuid::new_v4().simple()),
                title: title.to_string(),
                items,
            };
            card.checklists.push(checklist.clone());
            card.updated_at = Utc::now();
            HermesToolResult::ok(
                name,
                format!("Added checklist '{}' ({}) to card '{card_id}'", checklist.title, checklist.id),
                Some(serde_json::to_value(&checklist).unwrap_or_default()),
            )
        }
        "kanban_toggle_checklist" => {
            let Some(card_id) = args.get("card_id").and_then(|v| v.as_str()) else {
                return HermesToolResult::err(name, "Missing required 'card_id'");
            };
            let item_index = args.get("item_index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let checklist_id = args.get("checklist_id").and_then(|v| v.as_str());
            let explicit_checked = args.get("checked").and_then(|v| v.as_bool());

            let Some(card) = store.cards.get_mut(card_id) else {
                return HermesToolResult::err(name, format!("Card '{card_id}' not found"));
            };
            let chk = if let Some(cid) = checklist_id {
                card.checklists.iter_mut().find(|c| c.id == cid)
            } else {
                card.checklists.first_mut()
            };
            let Some(checklist) = chk else {
                return HermesToolResult::err(name, "Checklist not found on card");
            };
            let Some(item) = checklist.items.get_mut(item_index) else {
                return HermesToolResult::err(name, format!("Checklist item index {item_index} out of range"));
            };
            item.checked = explicit_checked.unwrap_or(!item.checked);
            card.updated_at = Utc::now();
            HermesToolResult::ok(
                name,
                format!("Checklist item [{}] '{}' checked={}", item_index, item.text, item.checked),
                Some(serde_json::to_value(checklist).unwrap_or_default()),
            )
        }
        "kanban_search_cards" => {
            let q = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let board_id = args.get("board_id").and_then(|v| v.as_str());
            let column = args.get("column").and_then(|v| v.as_str());
            let assignee = args.get("assignee").and_then(|v| v.as_str());
            let priority = args.get("priority").and_then(|v| v.as_str());

            let matches: Vec<&KanbanCard> = store
                .cards
                .values()
                .filter(|c| {
                    board_id.is_none_or(|b| c.board_id == b)
                        && column.is_none_or(|col| c.column.eq_ignore_ascii_case(col))
                        && assignee.is_none_or(|a| c.assignee.as_deref() == Some(a))
                        && priority.is_none_or(|p| c.priority.eq_ignore_ascii_case(p))
                        && (q.is_empty()
                            || c.title.to_ascii_lowercase().contains(&q)
                            || c.description.to_ascii_lowercase().contains(&q))
                })
                .collect();
            HermesToolResult::ok(
                name,
                serde_json::to_string_pretty(&matches).unwrap_or_default(),
                Some(json!({ "count": matches.len(), "cards": matches })),
            )
        }
        _ => HermesToolResult::err(name, format!("Unsupported Kanban tool '{name}'")),
    }
}

// ----------------------------------------------------------------------------
// 4. Clarify, Desktop UI, Computer Use, Voice, Connections, Reasoning, MCP
// ----------------------------------------------------------------------------

async fn tool_clarify(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(question) = args.get("question").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("clarify", "Missing required 'question'");
    };
    let options: Vec<String> = args
        .get("options")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(ToString::to_string)).collect())
        .unwrap_or_default();
    let allow_free_text = args
        .get("allow_free_text")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let multi_select = args
        .get("multi_select")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let prompt_id = format!("clarify_{}", uuid::Uuid::new_v4().simple());
    let buttons: Vec<HermesInlineButton> = options
        .iter()
        .enumerate()
        .map(|(idx, opt)| HermesInlineButton {
            label: opt.clone(),
            callback_data: format!("clarify:{prompt_id}:{idx}:{opt}"),
        })
        .collect();

    let action = HermesUiAction {
        action_type: "clarify_prompt".into(),
        payload: json!({
            "prompt_id": prompt_id,
            "question": question,
            "options": options,
            "allow_free_text": allow_free_text,
            "multi_select": multi_select,
        }),
    };
    ctx.emit_ui(&action);
    if let Some(bridge) = &ctx.state_bridge {
        let _ = bridge.emit_ui_action(action.clone()).await;
    }

    let mut formatted = question.to_string();
    if !options.is_empty() {
        formatted.push('\n');
        for (idx, opt) in options.iter().enumerate() {
            formatted.push_str(&format!("{}. {opt}\n", idx + 1));
        }
    }

    HermesToolResult::ok(
        "clarify",
        formatted,
        Some(json!({
            "prompt_id": prompt_id,
            "question": question,
            "options": options,
            "allow_free_text": allow_free_text,
            "multi_select": multi_select,
        })),
    )
    .with_ui_action(action)
    .with_inline_buttons(buttons)
}

async fn tool_desktop_ui_inspect(ctx: &HermesToolContext) -> HermesToolResult {
    if let Some(bridge) = &ctx.state_bridge {
        if let Ok(payload) = bridge.desktop_ui_inspect().await {
            return HermesToolResult::ok(
                "desktop_ui_inspect",
                serde_json::to_string_pretty(&payload).unwrap_or_default(),
                Some(payload),
            );
        }
    }
    let Ok(store) = ctx.agentic_store.lock() else {
        return HermesToolResult::err("desktop_ui_inspect", "LocalAgenticStore lock poisoned");
    };
    let payload = json!({
        "desktop_state": store.desktop_state,
        "recent_ui_actions": store.ui_action_log.iter().rev().take(10).collect::<Vec<_>>(),
    });
    HermesToolResult::ok(
        "desktop_ui_inspect",
        serde_json::to_string_pretty(&payload).unwrap_or_default(),
        Some(payload),
    )
}

async fn tool_desktop_ui_act(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(action_type) = args.get("action_type").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("desktop_ui_act", "Missing required 'action_type'");
    };
    let payload = args.get("payload").cloned().unwrap_or_else(|| json!({}));

    if let Ok(mut store) = ctx.agentic_store.lock() {
        store.desktop_state.last_action = Some(action_type.to_string());
        if let Some(dest) = payload.get("destination").and_then(|v| v.as_str()) {
            store.desktop_state.active_destination = dest.to_string();
        }
        if let Some(panel) = payload.get("panel").and_then(|v| v.as_str()) {
            if !store.desktop_state.open_panels.iter().any(|p| p == panel) {
                store.desktop_state.open_panels.push(panel.to_string());
            }
        }
    }

    let action = HermesUiAction {
        action_type: action_type.to_string(),
        payload: payload.clone(),
    };
    ctx.emit_ui(&action);
    if let Some(bridge) = &ctx.state_bridge {
        let _ = bridge.emit_ui_action(action.clone()).await;
    }

    HermesToolResult::ok(
        "desktop_ui_act",
        format!("Dispatched desktop UI action '{action_type}'"),
        Some(json!({ "action_type": action_type, "payload": payload })),
    )
    .with_ui_action(action)
}

fn tool_computer_use(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(action_name) = args.get("action").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("computer_use", "Missing required 'action'");
    };
    let Ok(mut broker) = ctx.computer_broker.lock() else {
        return HermesToolResult::err("computer_use", "ComputerLeaseBroker lock poisoned");
    };

    match action_name {
        "acquire_lease" => {
            let device_id = args
                .get("device_id")
                .and_then(|v| v.as_str())
                .unwrap_or("local_desktop");
            match broker.acquire_lease(&ctx.session_id, device_id) {
                Some(lease) => HermesToolResult::ok(
                    "computer_use",
                    format!(
                        "Acquired ComputerControlLease {} (gen={}, device={})",
                        lease.lease_id, lease.generation, lease.target_device_id
                    ),
                    Some(serde_json::to_value(&lease).unwrap_or_default()),
                ),
                None => HermesToolResult::err("computer_use", "Failed to acquire ComputerControlLease"),
            }
        }
        "release_lease" => {
            let lease_id = args
                .get("lease_id")
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
                .or_else(|| broker.active_lease().map(|l| l.lease_id.clone()))
                .unwrap_or_default();
            let ok = broker.release_lease(&lease_id);
            HermesToolResult::ok(
                "computer_use",
                format!("Released lease '{lease_id}': {ok}"),
                Some(json!({ "released": ok, "lease_id": lease_id })),
            )
        }
        "emergency_takeover" => {
            let generation = broker.emergency_takeover();
            HermesToolResult::ok(
                "computer_use",
                format!("Emergency human takeover triggered; bumped generation fence to {generation}"),
                Some(json!({ "generation": generation, "active": false })),
            )
        }
        "lease_status" => {
            let active = broker.is_lease_active();
            let generation = broker.current_generation();
            let lease = broker.active_lease().cloned();
            HermesToolResult::ok(
                "computer_use",
                format!("Computer lease active={active}, generation={generation}"),
                Some(json!({ "active": active, "generation": generation, "lease": lease })),
            )
        }
        "click" | "move_cursor" | "key_press" | "type_string" | "screenshot" | "wait" => {
            let cua = match action_name {
                "click" => CuaAction::Click {
                    x: args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    y: args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    button: args.get("button").and_then(|v| v.as_str()).unwrap_or("left").into(),
                },
                "move_cursor" => CuaAction::MoveCursor {
                    x: args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    y: args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                },
                "key_press" => CuaAction::KeyPress {
                    key: args.get("key").and_then(|v| v.as_str()).unwrap_or("Return").into(),
                },
                "type_string" => CuaAction::TypeString {
                    text: args.get("text").and_then(|v| v.as_str()).unwrap_or("").into(),
                },
                "screenshot" => CuaAction::TakeScreenshot,
                "wait" => CuaAction::Wait {
                    wait_ms: args.get("wait_ms").and_then(|v| v.as_u64()).unwrap_or(250) as u32,
                },
                _ => unreachable!(),
            };
            let Some(lease) = broker.active_lease().cloned() else {
                return HermesToolResult::err(
                    "computer_use",
                    "No active ComputerControlLease; call computer_use(action='acquire_lease') first",
                );
            };
            let generation = broker.current_generation();
            match CuaExecutor::execute_action_fenced(&cua, &lease, generation) {
                Ok(msg) => {
                    let proto = CuaExecutor::action_to_protocol(&cua);
                    HermesToolResult::ok(
                        "computer_use",
                        msg,
                        Some(json!({
                            "generation": generation,
                            "lease_id": lease.lease_id,
                            "protocol_action": proto,
                        })),
                    )
                }
                Err(e) => HermesToolResult::err("computer_use", e.to_string()),
            }
        }
        other => HermesToolResult::err("computer_use", format!("Unknown computer_use action '{other}'")),
    }
}

fn require_computer_provider_and_user(
    tool_name: &str,
    ctx: &HermesToolContext,
) -> Result<(Arc<dyn ComputerCloudProvider>, u64, Option<u64>), HermesToolResult> {
    let Some(provider) = ctx.computer_provider.as_ref().map(Arc::clone) else {
        return Err(HermesToolResult::err(
            tool_name,
            "ComputerCloudProvider is not configured in HermesToolContext",
        ));
    };
    let Some(user_id) = ctx.user_id else {
        return Err(HermesToolResult::err(
            tool_name,
            "Missing user_id in HermesToolContext for computer operation",
        ));
    };
    let bot_id = ctx.bot_id.and_then(|id| u64::try_from(id).ok());
    Ok((provider, user_id, bot_id))
}

async fn tool_computer_list(ctx: &HermesToolContext) -> HermesToolResult {
    let Some(provider) = ctx.computer_provider.as_ref().map(Arc::clone) else {
        let lease = tool_computer_use(json!({ "action": "lease_status" }), ctx);
        let payload = json!({
            "fallback": "lease_status",
            "computers": [],
            "lease": lease.structured,
        });
        return HermesToolResult::ok(
            "computer_list",
            format!("ComputerCloudProvider unavailable; fallback lease status: {}", lease.output),
            Some(payload),
        );
    };

    let Some(user_id) = ctx.user_id else {
        return HermesToolResult::err(
            "computer_list",
            "Missing user_id in HermesToolContext for computer_list",
        );
    };
    let bot_id = ctx.bot_id.and_then(|id| u64::try_from(id).ok());

    match provider.list_computers(user_id, bot_id).await {
        Ok(nodes) => {
            let payload = json!({
                "count": nodes.len(),
                "computers": nodes,
            });
            HermesToolResult::ok("computer_list", payload.to_string(), Some(payload))
        }
        Err(err) => HermesToolResult::err("computer_list", redact_cloud_secrets(&err.to_string())),
    }
}

async fn tool_computer_bind(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let (provider, user_id, bot_id) = match require_computer_provider_and_user("computer_bind", ctx) {
        Ok(v) => v,
        Err(err_res) => return err_res,
    };
    let Some(bot_id) = bot_id else {
        return HermesToolResult::err(
            "computer_bind",
            "Missing active bot_id in HermesToolContext; cannot bind computer",
        );
    };

    let node_id = args
        .get("node_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    match provider.bind_bot_computer(user_id, bot_id, node_id).await {
        Ok(node) => {
            let payload = json!({
                "bound": true,
                "bot_id": bot_id,
                "node": node,
            });
            HermesToolResult::ok("computer_bind", payload.to_string(), Some(payload))
        }
        Err(err) => HermesToolResult::err("computer_bind", redact_cloud_secrets(&err.to_string())),
    }
}

async fn tool_computer_configure_cloud(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let (provider, user_id, bot_id) =
        match require_computer_provider_and_user("computer_configure_cloud", ctx) {
            Ok(v) => v,
            Err(err_res) => return err_res,
        };

    let agent_endpoint_url = args
        .get("agent_endpoint_url")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let host_arg = args
        .get("host")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            agent_endpoint_url.as_deref().and_then(|url| {
                let no_scheme = url
                    .strip_prefix("https://")
                    .or_else(|| url.strip_prefix("http://"))?;
                let auth = no_scheme.split(['/', '?', '#']).next()?;
                let host_port = auth.rsplit('@').next().unwrap_or(auth);
                Some(
                    host_port
                        .rsplit_once(':')
                        .map(|(h, _)| h)
                        .unwrap_or(host_port)
                        .to_string(),
                )
            })
        });

    let Some(host) = host_arg else {
        return HermesToolResult::err(
            "computer_configure_cloud",
            "Missing required 'host' parameter",
        );
    };

    let Some(secret) = args
        .get("secret")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(ToString::to_string)
    else {
        return HermesToolResult::err(
            "computer_configure_cloud",
            "Missing required non-empty 'secret' parameter",
        );
    };

    let auth_method = args
        .get("auth_method")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if agent_endpoint_url.is_some() {
                "agent_http_token".to_string()
            } else if secret.contains("-----BEGIN ") && secret.contains("PRIVATE KEY-----") {
                "ssh_private_key".to_string()
            } else {
                "ssh_password".to_string()
            }
        });

    let port = args
        .get("port")
        .and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .or_else(|| {
            agent_endpoint_url.as_deref().and_then(|url| {
                let no_scheme = url
                    .strip_prefix("https://")
                    .or_else(|| url.strip_prefix("http://"))?;
                let auth = no_scheme.split(['/', '?', '#']).next()?;
                let host_port = auth.rsplit('@').next().unwrap_or(auth);
                host_port
                    .rsplit_once(':')
                    .and_then(|(_, p)| p.parse::<u16>().ok())
            })
        })
        .unwrap_or_else(|| {
            if auth_method == "agent_http_token" {
                8080
            } else {
                22
            }
        });

    let username = args
        .get("username")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "root".to_string());

    let display_name = args
        .get("display_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Cloud-{host}"));

    let workdir = args
        .get("workdir")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let bind_to_bot = args
        .get("bind_to_bot")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let node_id = args
        .get("node_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let cfg = AgentCloudServerConfigInput {
        node_id,
        display_name,
        host,
        port,
        username,
        auth_method,
        secret,
        agent_endpoint_url,
        workdir,
        bind_to_bot,
    };

    match provider.configure_cloud_server(user_id, bot_id, cfg).await {
        Ok(node) => {
            let payload = json!({
                "node": node,
                "node_id": node.node_id,
                "display_name": node.display_name,
                "host": node.host,
                "online": node.online,
                "bound_to_this_bot": node.bound_to_this_bot,
                "run_policy": node.run_policy,
                "secret_redacted": true,
            });
            HermesToolResult::ok("computer_configure_cloud", payload.to_string(), Some(payload))
        }
        Err(err) => HermesToolResult::err(
            "computer_configure_cloud",
            redact_cloud_secrets(&err.to_string()),
        ),
    }
}

async fn tool_computer_exec(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let (provider, user_id, bot_id) = match require_computer_provider_and_user("computer_exec", ctx) {
        Ok(v) => v,
        Err(err_res) => return err_res,
    };

    let Some(command) = args
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return HermesToolResult::err("computer_exec", "Missing required 'command' parameter");
    };

    let node_id = args
        .get("node_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let cwd = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let req = AgentComputerExecInput {
        node_id,
        caller_bot_id: bot_id,
        op: "shell_exec".to_string(),
        command: Some(command),
        cwd,
        ..Default::default()
    };

    match provider.execute_on_computer(user_id, req).await {
        Ok(out) => exec_output_to_tool_result("computer_exec", out),
        Err(err) => HermesToolResult::err("computer_exec", redact_cloud_secrets(&err.to_string())),
    }
}

async fn tool_computer_file(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let (provider, user_id, bot_id) = match require_computer_provider_and_user("computer_file", ctx) {
        Ok(v) => v,
        Err(err_res) => return err_res,
    };

    let op_raw = args
        .get("op")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let op = match op_raw.as_str() {
        "read" | "read_file" => "read_file",
        "write" | "write_file" => "write_file",
        "list" | "list_dir" => "list_dir",
        other => {
            return HermesToolResult::err(
                "computer_file",
                format!("Unsupported computer_file op '{other}'; expected read|write|list"),
            );
        }
    };

    let Some(path) = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return HermesToolResult::err("computer_file", "Missing required 'path' parameter");
    };

    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);
    if op == "write_file" && content.is_none() {
        return HermesToolResult::err(
            "computer_file",
            "Missing required 'content' parameter for write operation",
        );
    }

    let node_id = args
        .get("node_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let req = AgentComputerExecInput {
        node_id,
        caller_bot_id: bot_id,
        op: op.to_string(),
        path: Some(path),
        content,
        ..Default::default()
    };

    match provider.execute_on_computer(user_id, req).await {
        Ok(out) => exec_output_to_tool_result("computer_file", out),
        Err(err) => HermesToolResult::err("computer_file", redact_cloud_secrets(&err.to_string())),
    }
}

async fn tool_computer_screen(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let (provider, user_id, bot_id) =
        match require_computer_provider_and_user("computer_screen", ctx) {
            Ok(v) => v,
            Err(err_res) => return err_res,
        };

    let op_raw = args
        .get("op")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let op = match op_raw.as_str() {
        "capture" | "capture_screen" => "capture_screen",
        "click" => "click",
        "move" => "move",
        "type" => "type",
        "key" => "key",
        other => {
            return HermesToolResult::err(
                "computer_screen",
                format!(
                    "Unsupported computer_screen op '{other}'; expected capture|click|move|type|key"
                ),
            );
        }
    };

    let x = args
        .get("x")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok());
    let y = args
        .get("y")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok());
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if op == "type" && text.as_ref().is_none_or(|t| t.is_empty()) {
        return HermesToolResult::err(
            "computer_screen",
            "Missing required 'text' parameter for 'type' operation",
        );
    }
    if op == "key" && key.is_none() {
        return HermesToolResult::err(
            "computer_screen",
            "Missing required 'key' parameter for 'key' operation",
        );
    }

    let node_id = args
        .get("node_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let req = AgentComputerExecInput {
        node_id,
        caller_bot_id: bot_id,
        op: op.to_string(),
        x,
        y,
        text,
        key,
        ..Default::default()
    };

    match provider.execute_on_computer(user_id, req).await {
        Ok(out) => exec_output_to_tool_result("computer_screen", out),
        Err(err) => HermesToolResult::err("computer_screen", redact_cloud_secrets(&err.to_string())),
    }
}

fn exec_output_to_tool_result(tool_name: &str, out: AgentComputerExecOutput) -> HermesToolResult {
    let safe_out = AgentComputerExecOutput {
        node_id: out.node_id,
        ok: out.ok,
        exit_code: out.exit_code,
        stdout: redact_cloud_secrets(&out.stdout),
        stderr: redact_cloud_secrets(&out.stderr),
        entries: out.entries,
        frame_data_url: out.frame_data_url,
        executed_by: out.executed_by,
    };
    let payload = serde_json::to_value(&safe_out).unwrap_or_else(|_| {
        json!({
            "node_id": safe_out.node_id,
            "ok": safe_out.ok,
            "exit_code": safe_out.exit_code,
        })
    });

    if safe_out.ok {
        HermesToolResult::ok(tool_name, payload.to_string(), Some(payload))
    } else {
        let err_msg = if !safe_out.stderr.trim().is_empty() {
            format!(
                "exit_code={} on {}: {}",
                safe_out.exit_code,
                safe_out.node_id,
                safe_out.stderr.trim()
            )
        } else {
            format!(
                "Command/operation failed on {} with exit_code={}",
                safe_out.node_id, safe_out.exit_code
            )
        };
        let mut res = HermesToolResult::err(tool_name, err_msg);
        res.structured = Some(payload);
        res
    }
}

fn tool_voice_session(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status");
    let Ok(mut talk) = ctx.talk_coordinator.lock() else {
        return HermesToolResult::err("voice_session", "TalkCoordinator lock poisoned");
    };
    match action {
        "status" => {}
        "start" => {
            if let Err(e) = talk.transition(TalkSessionState::Listening) {
                return HermesToolResult::err("voice_session", e);
            }
        }
        "barge_in" => {
            talk.handle_barge_in();
        }
        "resume" => {
            talk.resume_listening();
        }
        "close" => {
            talk.close();
        }
        other => return HermesToolResult::err("voice_session", format!("Unknown voice action '{other}'")),
    }
    HermesToolResult::ok(
        "voice_session",
        format!(
            "Talk session '{}' state={:?} gen={} buffered_chunks={}",
            talk.session_id,
            talk.state,
            talk.generation,
            talk.buffered_chunks_count()
        ),
        Some(json!({
            "session_id": talk.session_id,
            "state": talk.state,
            "generation": talk.generation,
            "signal_seq": talk.signal_seq,
            "buffered_chunks": talk.buffered_chunks_count(),
        })),
    )
}

fn mask_secret(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.len() <= 6 {
        return "****".into();
    }
    let prefix: String = trimmed.chars().take(4).collect();
    format!("{prefix}****")
}

async fn tool_manage_connections(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("list")
        .to_ascii_lowercase();
    let local_res = {
        let Ok(mut store) = ctx.agentic_store.lock() else {
            return HermesToolResult::err("manage_connections", "LocalAgenticStore lock poisoned");
        };

        match action.as_str() {
            "list" => {
                let mut list: Vec<&IntegrationConnection> = store.connections.values().collect();
                list.sort_by(|a, b| a.name.cmp(&b.name));
                HermesToolResult::ok(
                    "manage_connections",
                    serde_json::to_string_pretty(&list).unwrap_or_default(),
                    Some(json!({ "connections": list })),
                )
            }
            "connect" => {
                let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
                    return HermesToolResult::err("manage_connections", "Missing required 'name'");
                };
                let provider = args
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or(name)
                    .to_string();
                let endpoint = args
                    .get("endpoint")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
                let masked_secret = args
                    .get("api_key")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(mask_secret);
                let entry = IntegrationConnection {
                    name: name.to_string(),
                    provider,
                    endpoint,
                    masked_secret,
                    connected: true,
                    updated_at: Utc::now(),
                    last_tested_at: None,
                };
                store.connections.insert(name.to_string(), entry.clone());
                HermesToolResult::ok(
                    "manage_connections",
                    format!("Connected integration '{name}' (credentials safely masked)"),
                    Some(serde_json::to_value(&entry).unwrap_or_default()),
                )
            }
            "disconnect" => {
                let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
                    return HermesToolResult::err("manage_connections", "Missing required 'name'");
                };
                if let Some(entry) = store.connections.get_mut(name) {
                    entry.connected = false;
                    entry.masked_secret = None;
                    entry.updated_at = Utc::now();
                    HermesToolResult::ok(
                        "manage_connections",
                        format!("Disconnected integration '{name}'"),
                        Some(serde_json::to_value(entry).unwrap_or_default()),
                    )
                } else {
                    HermesToolResult::err("manage_connections", format!("Connection '{name}' not found"))
                }
            }
            "test" => {
                let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
                    return HermesToolResult::err("manage_connections", "Missing required 'name'");
                };
                if let Some(entry) = store.connections.get_mut(name) {
                    entry.last_tested_at = Some(Utc::now());
                    let healthy = entry.connected;
                    HermesToolResult::ok(
                        "manage_connections",
                        format!("Tested connection '{name}': connected={healthy}"),
                        Some(json!({ "name": name, "healthy": healthy, "connection": entry })),
                    )
                } else {
                    HermesToolResult::err("manage_connections", format!("Connection '{name}' not found"))
                }
            }
            "get" => {
                let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
                    return HermesToolResult::err("manage_connections", "Missing required 'name'");
                };
                match store.connections.get(name) {
                    Some(entry) => HermesToolResult::ok(
                        "manage_connections",
                        serde_json::to_string_pretty(entry).unwrap_or_default(),
                        Some(serde_json::to_value(entry).unwrap_or_default()),
                    ),
                    None => HermesToolResult::err("manage_connections", format!("Connection '{name}' not found")),
                }
            }
            _ => HermesToolResult::err("manage_connections", format!("Unsupported action '{action}'")),
        }
    };

    if let Some(bridge) = &ctx.state_bridge {
        match bridge.connections_op(&args).await {
            Ok(val) => {
                return HermesToolResult::ok(
                    "manage_connections",
                    serde_json::to_string_pretty(&val).unwrap_or_default(),
                    Some(val),
                );
            }
            Err(err) if err == "connections_op not bridged" => {}
            Err(err) => {
                // If local store didn't find it either, propagate backend error
                if !local_res.success {
                    return HermesToolResult::err("manage_connections", err);
                }
            }
        }
    }
    local_res
}

fn tool_reasoning(name: &str, args: Value) -> HermesToolResult {
    let thought = args
        .get("thought")
        .or_else(|| args.get("problem"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let conclusion = args
        .get("conclusion")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let steps = args
        .get("steps")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    HermesToolResult::ok(
        name,
        if conclusion.is_empty() {
            format!("Recorded reasoning scratchpad ({} step(s))", steps.len())
        } else {
            format!("Reasoning concluded: {conclusion}")
        },
        Some(json!({
            "thought": thought,
            "steps": steps,
            "conclusion": conclusion,
            "confidence": args.get("confidence").cloned().unwrap_or(Value::Null),
        })),
    )
}

fn tool_mcp_list_tools(ctx: &HermesToolContext) -> HermesToolResult {
    let Ok(reg) = ctx.mcp_registry.read() else {
        return HermesToolResult::err("mcp_list_tools", "McpRegistry lock poisoned");
    };
    let tools = reg.list_tools();
    HermesToolResult::ok(
        "mcp_list_tools",
        serde_json::to_string_pretty(&tools).unwrap_or_default(),
        Some(json!({ "count": tools.len(), "tools": tools })),
    )
}

fn tool_mcp_call(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(tool_name) = args.get("name").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("mcp_call", "Missing required 'name'");
    };
    let call_args = args.get("arguments").cloned().unwrap_or_else(|| json!({}));

    let Ok(reg) = ctx.mcp_registry.read() else {
        return HermesToolResult::err("mcp_call", "McpRegistry lock poisoned");
    };
    let Some(def) = reg.get(tool_name) else {
        return HermesToolResult::err("mcp_call", format!("MCP tool '{tool_name}' is not registered"));
    };

    let dock = ToolPermissionDock::new();
    let assessed = dock.evaluate_risk(def, &call_args);
    let mode = ctx
        .security
        .read()
        .map(|s| s.approvals.get_session_mode(&ctx.session_id))
        .unwrap_or(ApprovalMode::Prompt);
    if assessed == ToolRiskLevel::CriticalDestructive && mode == ApprovalMode::Deny {
        return HermesToolResult::err(
            "mcp_call",
            format!("MCP tool '{tool_name}' blocked due to CriticalDestructive risk assessment"),
        );
    }

    match reg.execute_tool(tool_name, call_args) {
        Ok(val) => HermesToolResult::ok(
            "mcp_call",
            serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string()),
            Some(val),
        ),
        Err(e) => HermesToolResult::err("mcp_call", e),
    }
}

// ----------------------------------------------------------------------------
// 5. Session Search, Memory, Skills, Honcho (bridge-first with local fallback)
// ----------------------------------------------------------------------------

async fn tool_session_search(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(query) = args.get("query").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("session_search", "Missing required 'query'");
    };
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

    if let Some(bridge) = &ctx.state_bridge {
        return match bridge.session_search(query, limit).await {
            Ok(val) => HermesToolResult::ok(
                "session_search",
                serde_json::to_string_pretty(&val).unwrap_or_default(),
                Some(val),
            ),
            Err(e) => HermesToolResult::err("session_search", e),
        };
    }

    let Ok(store) = ctx.agentic_store.lock() else {
        return HermesToolResult::err("session_search", "LocalAgenticStore lock poisoned");
    };
    let q_lower = query.to_ascii_lowercase();
    let hits: Vec<Value> = store
        .session_notes
        .iter()
        .filter(|(_, text, _)| text.to_ascii_lowercase().contains(&q_lower) || text.contains(query))
        .take(limit)
        .map(|(sid, text, ts)| json!({ "session_id": sid, "snippet": text, "timestamp": ts }))
        .collect();
    HermesToolResult::ok(
        "session_search",
        serde_json::to_string_pretty(&hits).unwrap_or_default(),
        Some(json!({ "query": query, "hits": hits })),
    )
}

async fn tool_memory(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    if let Some(bridge) = &ctx.state_bridge {
        return match bridge.memory_op(&args).await {
            Ok(val) => HermesToolResult::ok(
                "memory",
                serde_json::to_string_pretty(&val).unwrap_or_default(),
                Some(val),
            ),
            Err(e) => HermesToolResult::err("memory", e),
        };
    }

    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("read");
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("memory");
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("").trim();
    let old_text = args.get("old_text").and_then(|v| v.as_str()).unwrap_or("");

    let Ok(mut store) = ctx.agentic_store.lock() else {
        return HermesToolResult::err("memory", "LocalAgenticStore lock poisoned");
    };

    let (entries, limit) = if target == "user" {
        (&mut store.user_entries, LOCAL_USER_LIMIT_CHARS)
    } else {
        (&mut store.memory_entries, LOCAL_MEMORY_LIMIT_CHARS)
    };

    match action {
        "read" => {
            let rendered = entries.join(MEMORY_ENTRY_DELIMITER);
            HermesToolResult::ok(
                "memory",
                if rendered.is_empty() { "(empty)".into() } else { rendered.clone() },
                Some(json!({
                    "target": target,
                    "chars": rendered.chars().count(),
                    "limit": limit,
                    "entries": entries,
                })),
            )
        }
        "add" => {
            if content.is_empty() {
                return HermesToolResult::err("memory", "Missing 'content' to add");
            }
            let mut next = entries.clone();
            next.push(content.to_string());
            let total = next.join(MEMORY_ENTRY_DELIMITER).chars().count();
            if total > limit {
                return HermesToolResult::err(
                    "memory",
                    format!("Memory budget exceeded ({total} > {limit} chars)"),
                );
            }
            *entries = next;
            HermesToolResult::ok(
                "memory",
                format!("Added entry to {target} ({total}/{limit} chars)"),
                Some(json!({ "target": target, "chars": total, "limit": limit })),
            )
        }
        "replace" => {
            if let Some(pos) = entries.iter().position(|e| e.contains(old_text)) {
                let mut next = entries.clone();
                next[pos] = content.to_string();
                let total = next.join(MEMORY_ENTRY_DELIMITER).chars().count();
                if total > limit {
                    return HermesToolResult::err(
                        "memory",
                        format!("Memory budget exceeded ({total} > {limit} chars)"),
                    );
                }
                *entries = next;
                HermesToolResult::ok(
                    "memory",
                    format!("Replaced entry in {target} ({total}/{limit} chars)"),
                    Some(json!({ "target": target, "chars": total, "limit": limit })),
                )
            } else {
                HermesToolResult::err("memory", format!("No entry matching '{old_text}' found"))
            }
        }
        "remove" => {
            let needle = if old_text.is_empty() { content } else { old_text };
            let before = entries.len();
            entries.retain(|e| !e.contains(needle));
            let removed = before - entries.len();
            HermesToolResult::ok(
                "memory",
                format!("Removed {removed} matching entry(s) from {target}"),
                Some(json!({ "removed": removed })),
            )
        }
        other => HermesToolResult::err("memory", format!("Unsupported memory action '{other}'")),
    }
}

async fn tool_skill_manage(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    if let Some(bridge) = &ctx.state_bridge {
        return match bridge.skill_op(&args).await {
            Ok(val) => HermesToolResult::ok(
                "skill_manage",
                serde_json::to_string_pretty(&val).unwrap_or_default(),
                Some(val),
            ),
            Err(e) => HermesToolResult::err("skill_manage", e),
        };
    }

    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");
    let Ok(mut store) = ctx.agentic_store.lock() else {
        return HermesToolResult::err("skill_manage", "LocalAgenticStore lock poisoned");
    };

    match action {
        "list" => {
            let list: Vec<Value> = store
                .skills
                .values()
                .map(|s| json!({ "name": s.name, "description": s.description, "tags": s.tags }))
                .collect();
            HermesToolResult::ok(
                "skill_manage",
                serde_json::to_string_pretty(&list).unwrap_or_default(),
                Some(json!({ "skills": list })),
            )
        }
        "view" => {
            let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
                return HermesToolResult::err("skill_manage", "Missing 'name'");
            };
            match store.skills.get(name) {
                Some(s) => HermesToolResult::ok(
                    "skill_manage",
                    s.content.clone(),
                    Some(serde_json::to_value(s).unwrap_or_default()),
                ),
                None => HermesToolResult::err("skill_manage", format!("Skill '{name}' not found")),
            }
        }
        "create" => {
            let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
                return HermesToolResult::err("skill_manage", "Missing 'name'");
            };
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let desc = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let tags = args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(ToString::to_string)).collect())
                .unwrap_or_default();
            let entry = LocalSkillEntry {
                name: name.to_string(),
                description: desc.to_string(),
                content: content.to_string(),
                tags,
                updated_at: Utc::now(),
            };
            store.skills.insert(name.to_string(), entry.clone());
            HermesToolResult::ok(
                "skill_manage",
                format!("Created skill '{name}'"),
                Some(serde_json::to_value(&entry).unwrap_or_default()),
            )
        }
        "patch" => {
            let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
                return HermesToolResult::err("skill_manage", "Missing 'name'");
            };
            let Some(s) = store.skills.get_mut(name) else {
                return HermesToolResult::err("skill_manage", format!("Skill '{name}' not found"));
            };
            if let (Some(old), Some(new)) = (
                args.get("old_text").and_then(|v| v.as_str()),
                args.get("new_text").and_then(|v| v.as_str()),
            ) {
                s.content = s.content.replace(old, new);
            } else if let Some(c) = args.get("content").and_then(|v| v.as_str()) {
                s.content = c.to_string();
            }
            s.updated_at = Utc::now();
            HermesToolResult::ok(
                "skill_manage",
                format!("Patched skill '{name}'"),
                Some(serde_json::to_value(s).unwrap_or_default()),
            )
        }
        "delete" => {
            let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
                return HermesToolResult::err("skill_manage", "Missing 'name'");
            };
            if store.skills.remove(name).is_none() {
                return HermesToolResult::err("skill_manage", format!("Skill '{name}' not found"));
            }
            HermesToolResult::ok(
                "skill_manage",
                format!("Deleted skill '{name}'"),
                Some(json!({ "deleted": name })),
            )
        }
        "scan" => {
            let text = args
                .get("content")
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
                .or_else(|| {
                    args.get("name")
                        .and_then(|v| v.as_str())
                        .and_then(|n| store.skills.get(n))
                        .map(|s| s.content.clone())
                })
                .unwrap_or_default();
            let scan = scan_content_for_injection(&text);
            HermesToolResult::ok(
                "skill_manage",
                format!("Skill security scan flagged={}", scan.flagged),
                Some(serde_json::to_value(&scan).unwrap_or_default()),
            )
        }
        other => HermesToolResult::err("skill_manage", format!("Unsupported skill action '{other}'")),
    }
}

async fn tool_honcho_profile(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    if let Some(bridge) = &ctx.state_bridge {
        return match bridge.honcho_op(&args).await {
            Ok(val) => HermesToolResult::ok(
                "honcho_profile",
                serde_json::to_string_pretty(&val).unwrap_or_default(),
                Some(val),
            ),
            Err(e) => HermesToolResult::err("honcho_profile", e),
        };
    }

    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("profile");
    let user_id = args.get("user_id").and_then(|v| v.as_str()).unwrap_or("default_user");
    let Ok(mut store) = ctx.agentic_store.lock() else {
        return HermesToolResult::err("honcho_profile", "LocalAgenticStore lock poisoned");
    };

    match action {
        "record_observation" => {
            let obs = args.get("observation").and_then(|v| v.as_str()).unwrap_or("");
            let cat = args.get("category").and_then(|v| v.as_str()).unwrap_or("fact");
            if obs.trim().is_empty() {
                return HermesToolResult::err("honcho_profile", "Missing 'observation'");
            }
            store
                .honcho_observations
                .entry(user_id.to_string())
                .or_default()
                .push((cat.to_string(), obs.to_string(), Utc::now()));
            HermesToolResult::ok(
                "honcho_profile",
                format!("Recorded Honcho observation for '{user_id}'"),
                Some(json!({ "user_id": user_id, "category": cat, "observation": obs })),
            )
        }
        "query" => {
            let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_ascii_lowercase();
            let list: Vec<Value> = store
                .honcho_observations
                .get(user_id)
                .map(|obs| {
                    obs.iter()
                        .filter(|(c, o, _)| {
                            q.is_empty()
                                || c.to_ascii_lowercase().contains(&q)
                                || o.to_ascii_lowercase().contains(&q)
                        })
                        .map(|(c, o, ts)| json!({ "category": c, "observation": o, "recorded_at": ts }))
                        .collect()
                })
                .unwrap_or_default();
            HermesToolResult::ok(
                "honcho_profile",
                serde_json::to_string_pretty(&list).unwrap_or_default(),
                Some(json!({ "user_id": user_id, "matches": list })),
            )
        }
        "profile" => {
            let obs = store
                .honcho_observations
                .get(user_id)
                .cloned()
                .unwrap_or_default();
            HermesToolResult::ok(
                "honcho_profile",
                format!("Honcho profile '{user_id}': {} observation(s)", obs.len()),
                Some(json!({ "user_id": user_id, "observation_count": obs.len() })),
            )
        }
        other => HermesToolResult::err("honcho_profile", format!("Unsupported honcho action '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpToolDefinition;
    use super::super::toolsets::HermesToolExecutor;

    #[tokio::test]
    async fn test_all_14_kanban_tools_and_agentic_suite() {
        let ctx = HermesToolContext::new(".");

        let b = HermesToolExecutor::execute(
            "kanban_create_board",
            json!({ "name": "Roadmap" }),
            &ctx,
        )
        .await;
        assert!(b.success);
        let board_id = b.structured.unwrap()["id"].as_str().unwrap().to_string();

        assert!(HermesToolExecutor::execute("kanban_list_boards", json!({}), &ctx).await.success);
        assert!(HermesToolExecutor::execute("kanban_get_board", json!({ "board_id": board_id }), &ctx).await.success);

        let c = HermesToolExecutor::execute(
            "kanban_create_card",
            json!({ "board_id": board_id, "title": "Ship Hermes", "priority": "high" }),
            &ctx,
        )
        .await;
        assert!(c.success);
        let card_id = c.structured.unwrap()["id"].as_str().unwrap().to_string();

        assert!(HermesToolExecutor::execute("kanban_get_card", json!({ "card_id": card_id }), &ctx).await.success);
        assert!(HermesToolExecutor::execute("kanban_update_card", json!({ "card_id": card_id, "description": "Details" }), &ctx).await.success);
        assert!(HermesToolExecutor::execute("kanban_move_card", json!({ "card_id": card_id, "column": "In Progress" }), &ctx).await.success);
        assert!(HermesToolExecutor::execute("kanban_assign_card", json!({ "card_id": card_id, "assignee": "alice" }), &ctx).await.success);
        assert!(HermesToolExecutor::execute("kanban_add_comment", json!({ "card_id": card_id, "body": "LGTM" }), &ctx).await.success);
        assert!(HermesToolExecutor::execute("kanban_list_comments", json!({ "card_id": card_id }), &ctx).await.success);

        let chk = HermesToolExecutor::execute(
            "kanban_add_checklist",
            json!({ "card_id": card_id, "title": "Checks", "items": ["Unit tests"] }),
            &ctx,
        )
        .await;
        assert!(chk.success);
        assert!(HermesToolExecutor::execute("kanban_toggle_checklist", json!({ "card_id": card_id, "item_index": 0 }), &ctx).await.success);

        let search = HermesToolExecutor::execute("kanban_search_cards", json!({ "query": "Hermes" }), &ctx).await;
        assert!(search.success);
        assert_eq!(search.structured.unwrap()["count"], 1);

        assert!(HermesToolExecutor::execute("kanban_delete_card", json!({ "card_id": card_id }), &ctx).await.success);

        // Concurrent subagent delegation + depth guard
        let del = HermesToolExecutor::execute(
            "delegate_task",
            json!({
                "tasks": [
                    { "role": "researcher", "prompt": "Check docs" },
                    { "role": "reviewer", "prompt": "Check code" }
                ],
                "max_concurrent": 2,
                "max_depth": 3
            }),
            &ctx,
        )
        .await;
        assert!(del.success);
        assert_eq!(del.structured.unwrap()["count"], 2);

        // Clarify produces Telegram inline buttons
        let clar = HermesToolExecutor::execute(
            "clarify",
            json!({ "question": "Proceed?", "options": ["Yes", "No"] }),
            &ctx,
        )
        .await;
        assert!(clar.success);
        assert_eq!(clar.inline_buttons.len(), 2);

        // MCP bridge
        {
            let mut reg = ctx.mcp_registry.write().unwrap();
            reg.register_with_handler(
                McpToolDefinition::new("echo_mcp", "Echo", "{}", ToolRiskLevel::ReadOnly),
                std::sync::Arc::new(|a| Ok(json!({ "echoed": a }))),
            );
        }
        assert!(HermesToolExecutor::execute("mcp_list_tools", json!({}), &ctx).await.success);
        let mcp_res = HermesToolExecutor::execute(
            "mcp_call",
            json!({ "name": "echo_mcp", "arguments": { "hello": 1 } }),
            &ctx,
        )
        .await;
        assert!(mcp_res.success);
        assert!(mcp_res.output.contains("hello"));

        // CUA computer_use acquire + fenced click
        assert!(HermesToolExecutor::execute("computer_use", json!({ "action": "acquire_lease", "device_id": "box1" }), &ctx).await.success);
        assert!(HermesToolExecutor::execute("computer_use", json!({ "action": "click", "x": 100, "y": 200 }), &ctx).await.success);
        assert!(HermesToolExecutor::execute("computer_use", json!({ "action": "emergency_takeover" }), &ctx).await.success);
    }
}
