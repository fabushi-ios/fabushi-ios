use crate::db::miniapp_store::MiniAppRecord;
use crate::routes::{
    broadcast_chat_event, broadcast_chat_pts, check_active_membership, err_resp, map_telegram_err,
};
use crate::server::{ensure_default_ai_bot, AppState, AuthBearer};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use chrono::Utc;
use futures::stream::{self, StreamExt};
use gpui_telegram_agent::fabushi_bot::{AgentAddress, AgentGroupAddress, FabushiBotHost, FabushiBotPatch};
use gpui_telegram_agent::hermes::commands::{InlineActionButton, SlashCommandParser};
use gpui_telegram_agent::hermes::memory::{MemoryTarget, ObservationCategory, SoulProfile};
use gpui_telegram_agent::hermes::providers::{ApiMode, CredentialEntry, ProviderConfig};
use gpui_telegram_agent::hermes::security::{
    ApprovalDecisionAction, ApprovalMode, ApprovalStatus as SecurityApprovalStatus,
};
use gpui_telegram_agent::hermes::skills::{CreateSkillRequest, SkillDocument, SkillOrigin};
use gpui_telegram_agent::hermes::state_db::{
    ApprovalRecord, ApprovalStatus, CreateCronJobRequest, CreateKanbanCardRequest, CronJobPatch,
    CronJobRecord, KanbanBoardRecord, KanbanCardPatch, KanbanCardRecord, SessionListFilter,
    SessionRecord,
};
use gpui_telegram_agent::hermes::toolsets::{all_builtin_tool_specs, ToolsetId, ToolsetPreset};
use gpui_telegram_agent::hermes::trajectory::TrajectoryFormat;
use gpui_telegram_agent::hermes::{
    HeartbeatConfig, HermesEngine, HermesEvent, HermesTurnRequest, HermesTurnResult,
};
use gpui_telegram_agent::tasks::CronRoutine;
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, UserId};
use gpui_telegram_proto::computer_contracts::ComputerRunPolicy;
use gpui_telegram_proto::message::{InlineKeyboardButton, MessageContent, MessageItem};
use gpui_telegram_proto::sync::{TypingAction, TypingEvent, UpdateEvent};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

// ============================================================================
// Shared Mapping & Bot Persona Helpers
// ============================================================================

fn parse_optional_bot_id(v: Option<&Value>) -> Option<String> {
    let val = v?;
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    val.as_u64()
        .map(|n| n.to_string())
        .or_else(|| val.as_i64().map(|n| n.to_string()))
}

#[derive(Debug, Deserialize, Default)]
pub struct OptionalBotScopeQuery {
    pub bot_id: Option<u64>,
}

fn parse_bot_id_u64(v: Option<&Value>) -> Option<u64> {
    let val = v?;
    val.as_u64()
        .or_else(|| val.as_i64().filter(|n| *n > 0).map(|n| n as u64))
        .or_else(|| {
            val.as_str()
                .and_then(|s| s.trim().trim_start_matches("bot_").parse::<u64>().ok())
        })
}

pub(crate) fn resolve_hermes_for_bot(state: &AppState, bot_id: Option<u64>) -> Arc<HermesEngine> {
    if let Some(bid) = bot_id {
        if bid != crate::db::miniapp_store::BOTFATHER_USER_ID {
            if let Some(inst) = crate::routes::bots_miniapps::ensure_fabushi_bot_hydrated(state, bid as i64) {
                return inst.hermes();
            }
        }
    }
    Arc::clone(&state.hermes)
}

fn resolve_hermes_for_session(
    state: &AppState,
    bot_id: Option<u64>,
    session_id: &str,
) -> Arc<HermesEngine> {
    if bot_id.is_some() {
        return resolve_hermes_for_bot(state, bot_id);
    }
    if state.hermes.store().get_session(session_id).ok().flatten().is_some() {
        return Arc::clone(&state.hermes);
    }
    for inst in state.fabushi_bots.list_instances() {
        if inst.hermes().store().get_session(session_id).ok().flatten().is_some() {
            return inst.hermes();
        }
    }
    Arc::clone(&state.hermes)
}

pub(crate) fn map_proto_buttons(
    rows: &[Vec<InlineActionButton>],
) -> Vec<Vec<InlineKeyboardButton>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|b| InlineKeyboardButton {
                    text: b.text.clone(),
                    callback_data: if b.callback_data.trim().is_empty() {
                        None
                    } else {
                        Some(b.callback_data.clone())
                    },
                    url: b.url.clone(),
                    web_app_url: None,
                })
                .collect()
        })
        .collect()
}

pub(crate) fn map_buttons_json(rows: &[Vec<InlineActionButton>]) -> Value {
    let converted = map_proto_buttons(rows);
    serde_json::to_value(converted).unwrap_or_else(|_| json!([]))
}

fn format_usage_dto(res: &HermesTurnResult) -> Value {
    json!({
        "total_turns": res.iterations as u64,
        "prompt_tokens": res.usage.input_tokens,
        "completion_tokens": res.usage.output_tokens,
        "total_tokens": res.usage.total_tokens,
        "estimated_cost_usd": res.usage.estimated_cost_usd,
        "tool_calls": res.tool_calls.len() as u64,
        "by_model": [],
    })
}

fn turn_result_to_json(res: &HermesTurnResult) -> Value {
    json!({
        "turn_id": res.turn_id,
        "session_id": res.session_id,
        "status": "completed",
        "response": res.response_text,
        "model": res.model,
        "provider": res.provider,
        "events": res.events,
        "inline_keyboard": map_buttons_json(&res.buttons),
        "tool_calls_count": res.tool_calls.len(),
        "usage": format_usage_dto(res),
    })
}

pub(crate) fn caller_is_admin(state: &AppState, user_id: UserId) -> bool {
    state
        .db
        .get_user(user_id)
        .ok()
        .flatten()
        .is_some_and(|u| u.is_support)
}

pub(crate) fn caller_owns_bot_or_admin(
    state: &AppState,
    user_id: UserId,
    bot_id: u64,
) -> std::result::Result<(), Response> {
    if caller_is_admin(state, user_id) {
        return Ok(());
    }
    let owns_bot = state
        .fabushi_bots
        .get_by_user_id(bot_id as i64)
        .is_some_and(|i| i.settings().owner_user_id == user_id.0 as i64)
        || state
            .db
            .get_bot_by_user_id(bot_id as i64)
            .ok()
            .flatten()
            .is_some_and(|b| b.owner_user_id == user_id.0 as i64);
    if !owns_bot {
        return Err(err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Only bot owner or server admin can access or modify this bot resource",
        ));
    }
    Ok(())
}

pub(crate) fn is_official_bot(state: &AppState, bot_id: u64) -> bool {
    bot_id == 777_000
        || bot_id == crate::db::miniapp_store::BOTFATHER_USER_ID
        || state
            .db
            .get_bot_by_user_id(bot_id as i64)
            .ok()
            .flatten()
            .is_some_and(|b| b.is_official)
}

pub(crate) fn bot_has_human_owner(state: &AppState, bot_id: u64) -> bool {
    state
        .fabushi_bots
        .get_by_user_id(bot_id as i64)
        .is_some_and(|i| i.settings().owner_user_id > 0)
        || state
            .db
            .get_bot_by_user_id(bot_id as i64)
            .ok()
            .flatten()
            .is_some_and(|b| b.owner_user_id > 0)
}

pub(crate) fn is_bot_member_of_chat(state: &AppState, bot_id: u64, chat_id: ChatId) -> bool {
    state
        .db
        .get_chat(chat_id)
        .ok()
        .flatten()
        .is_some_and(|c| c.creator_id == UserId(bot_id))
        || state
            .db
            .get_chat_member(chat_id, UserId(bot_id))
            .ok()
            .flatten()
            .is_some_and(|m| !crate::db::ServerDatabase::is_inactive_member_role(&m.role))
}

fn resolve_effective_bot_id(
    state: &AppState,
    explicit_bot_id: Option<u64>,
    engine: &Arc<HermesEngine>,
) -> Option<u64> {
    explicit_bot_id.or_else(|| {
        if Arc::ptr_eq(engine, &state.hermes) {
            None
        } else {
            state
                .fabushi_bots
                .list_instances()
                .into_iter()
                .find(|i| Arc::ptr_eq(&i.hermes(), engine))
                .map(|i| i.bot_user_id as u64)
        }
    })
}

fn acp_forbidden_response(id: Value, message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32003,
                "message": message,
            }
        })),
    )
        .into_response()
}

pub(crate) fn build_caller_scope(
    state: &AppState,
    user_id: UserId,
) -> gpui_telegram_agent::hermes::CommandCallerScope {
    let allowed_chat_ids: HashSet<i64> = state
        .db
        .list_user_member_chat_ids(user_id)
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.0)
        .collect();
    gpui_telegram_agent::hermes::CommandCallerScope {
        user_id: user_id.0.to_string(),
        allowed_chat_ids,
        is_admin: caller_is_admin(state, user_id),
        bot_config_allowed: true,
    }
}

fn caller_owns_hermes_session_uid(
    state: &AppState,
    user_id: UserId,
    sess: &SessionRecord,
    bot_id_hint: Option<u64>,
) -> bool {
    let caller_str = user_id.0.to_string();
    let meta: Value = serde_json::from_str(&sess.metadata_json).unwrap_or_else(|_| json!({}));
    let owner_uid_match = meta
        .get("owner_user_id")
        .and_then(|v| {
            v.as_str()
                .map(|owner| owner == caller_str)
                .or_else(|| v.as_i64().map(|n| n.to_string() == caller_str))
        })
        .unwrap_or(false);
    let sess_bot_id = bot_id_hint.or_else(|| sess.bot_id.as_deref().and_then(|s| s.parse::<u64>().ok()));
    let bot_owner_match = sess_bot_id.is_some_and(|bid| {
        sess.bot_id.as_deref() == Some(&bid.to_string())
            && (state
                .fabushi_bots
                .get_by_user_id(bid as i64)
                .is_some_and(|i| i.settings().owner_user_id == user_id.0 as i64)
                || state
                    .db
                    .get_bot_by_user_id(bid as i64)
                    .ok()
                    .flatten()
                    .is_some_and(|b| b.owner_user_id == user_id.0 as i64))
    });
    owner_uid_match || bot_owner_match || caller_is_admin(state, user_id)
}

fn caller_owns_hermes_session(
    state: &AppState,
    auth: &AuthBearer,
    sess: &SessionRecord,
    bot_id_hint: Option<u64>,
) -> bool {
    caller_owns_hermes_session_uid(state, auth.user_id, sess, bot_id_hint)
}

fn authorize_hermes_session_access_uid(
    state: &AppState,
    user_id: UserId,
    sess: &SessionRecord,
    bot_id_hint: Option<u64>,
) -> std::result::Result<(), Response> {
    if let Some(cid) = sess.chat_id {
        check_active_membership(state, ChatId(cid), user_id)?;
    } else if !caller_owns_hermes_session_uid(state, user_id, sess, bot_id_hint) {
        return Err(err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Caller is not authorized to access this private Hermes session",
        ));
    }
    Ok(())
}

fn authorize_hermes_session_access(
    state: &AppState,
    auth: &AuthBearer,
    sess: &SessionRecord,
    bot_id_hint: Option<u64>,
) -> std::result::Result<(), Response> {
    authorize_hermes_session_access_uid(state, auth.user_id, sess, bot_id_hint)
}

fn session_to_dto(s: &SessionRecord) -> Value {
    let meta: Value = serde_json::from_str(&s.metadata_json).unwrap_or_else(|_| json!({}));
    let provider = meta
        .get("provider")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(&s.api_mode)
        .to_string();
    let branch_name = meta
        .get("branch_name")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .or_else(|| {
            if s.parent_session_id.is_some() {
                s.title.clone()
            } else {
                None
            }
        });
    json!({
        "session_id": s.id,
        "chat_id": s.chat_id,
        "bot_id": s.bot_id.as_deref().and_then(|b| b.parse::<u64>().ok()),
        "title": s.title,
        "model": s.model,
        "provider": provider,
        "parent_session_id": s.parent_session_id,
        "branch_name": branch_name,
        "message_count": s.message_count.max(0) as usize,
        "token_count": s.input_tokens.saturating_add(s.output_tokens).max(0) as u64,
        "created_at_ms": s.started_at,
        "updated_at_ms": s.ended_at.unwrap_or(s.started_at),
    })
}

fn skill_to_dto(doc: &SkillDocument, updated_at_ms: i64) -> Value {
    let content = if doc.raw_markdown.trim().is_empty() {
        doc.body.clone()
    } else {
        doc.raw_markdown.clone()
    };
    json!({
        "name": doc.frontmatter.name,
        "description": doc.frontmatter.description,
        "category": doc.frontmatter.origin.as_str(),
        "content": content,
        "enabled": true,
        "approved": doc.trusted || doc.security_report.safe_to_auto_install,
        "security_status": format!("{:?}", doc.security_report.risk_level).to_lowercase(),
        "tags": doc.frontmatter.tags,
        "updated_at_ms": updated_at_ms,
    })
}

fn cron_to_dto(rec: &CronJobRecord) -> Value {
    json!({
        "job_id": rec.id,
        "name": rec.name,
        "schedule": rec.schedule,
        "prompt": rec.prompt,
        "chat_id": rec.target_chat_id.or(rec.origin_chat_id),
        "bot_id": rec.deliver_to.as_deref().and_then(|s| s.parse::<u64>().ok()),
        "enabled": rec.enabled,
        "last_run_ms": rec.last_run_at,
        "next_run_ms": rec.next_run_at,
        "last_status": Value::Null,
    })
}

fn card_to_dto(card: &KanbanCardRecord) -> Value {
    json!({
        "card_id": card.id,
        "board_id": card.board_id,
        "title": card.title,
        "description": card.description,
        "column": card.column_name,
        "priority": card.priority,
        "assigned_agent": card.assignee,
        "position": card.position as i32,
        "created_at_ms": card.created_at,
        "updated_at_ms": card.updated_at,
    })
}

fn board_to_dto(board: &KanbanBoardRecord, cards: &[KanbanCardRecord]) -> Value {
    json!({
        "board_id": board.id,
        "name": board.name,
        "columns": board.columns,
        "cards": cards.iter().map(card_to_dto).collect::<Vec<_>>(),
        "created_at_ms": board.created_at,
    })
}

fn approval_status_str(status: ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::ApprovedOnce => "approved_once",
        ApprovalStatus::ApprovedSession => "approved_session",
        ApprovalStatus::ApprovedPermanent => "approved_permanent",
        ApprovalStatus::Denied => "denied",
    }
}

fn sec_approval_status_str(status: SecurityApprovalStatus) -> &'static str {
    match status {
        SecurityApprovalStatus::Pending => "pending",
        SecurityApprovalStatus::ApprovedOnce => "approved_once",
        SecurityApprovalStatus::ApprovedSession => "approved_session",
        SecurityApprovalStatus::ApprovedPermanent => "approved_permanent",
        SecurityApprovalStatus::Denied => "denied",
    }
}

fn db_approval_to_dto(rec: &ApprovalRecord) -> Value {
    json!({
        "approval_id": rec.id,
        "session_id": rec.session_id,
        "tool_name": rec.pattern_name,
        "command_preview": rec.command,
        "risk_level": rec.risk_level,
        "reason": rec.pattern_name,
        "status": approval_status_str(rec.status),
        "created_at_ms": rec.created_at,
        "decided_at_ms": rec.decided_at,
    })
}

fn toolsets_list_json(engine: &HermesEngine) -> Vec<Value> {
    let guard = engine
        .toolsets()
        .read()
        .unwrap_or_else(|e| e.into_inner());
    let all_specs = all_builtin_tool_specs();
    ToolsetId::ALL
        .iter()
        .map(|id| {
            let matching: Vec<_> = all_specs.iter().filter(|s| s.toolset == *id).collect();
            let tools: Vec<&str> = matching.iter().map(|s| s.name.as_str()).collect();
            let risk_level = matching
                .iter()
                .map(|s| match s.risk_level {
                    gpui_telegram_agent::ToolRiskLevel::ReadOnly => "read_only",
                    gpui_telegram_agent::ToolRiskLevel::Low => "low",
                    gpui_telegram_agent::ToolRiskLevel::Medium => "medium",
                    gpui_telegram_agent::ToolRiskLevel::High => "high",
                    gpui_telegram_agent::ToolRiskLevel::CriticalDestructive => "critical",
                })
                .max_by_key(|r| match *r {
                    "critical" => 4,
                    "high" => 3,
                    "medium" => 2,
                    "low" => 1,
                    _ => 0,
                })
                .unwrap_or("low");
            json!({
                "name": id.as_str(),
                "description": id.description(),
                "enabled": guard.is_toolset_enabled(*id),
                "tools": tools,
                "risk_level": risk_level,
            })
        })
        .collect()
}

fn build_memory_state_json(engine: &HermesEngine) -> Result<Value, Response> {
    let mem = engine
        .memory()
        .read(MemoryTarget::Memory)
        .map_err(map_telegram_err)?;
    let usr = engine
        .memory()
        .read(MemoryTarget::User)
        .map_err(map_telegram_err)?;
    let now_ms = Utc::now().timestamp_millis();

    let mut entries = Vec::with_capacity(mem.entries.len() + usr.entries.len());
    for (idx, item) in mem.entries.iter().enumerate() {
        entries.push(json!({
            "id": format!("mem_{}", idx + 1),
            "key": format!("memory_{}", idx + 1),
            "content": item,
            "category": "memory",
            "scope": "memory",
            "char_count": item.chars().count(),
            "updated_at_ms": now_ms,
        }));
    }
    for (idx, item) in usr.entries.iter().enumerate() {
        entries.push(json!({
            "id": format!("usr_{}", idx + 1),
            "key": format!("user_{}", idx + 1),
            "content": item,
            "category": "user",
            "scope": "user",
            "char_count": item.chars().count(),
            "updated_at_ms": now_ms,
        }));
    }

    Ok(json!({
        "entries": entries,
        "memory_markdown": mem.raw,
        "user_markdown": usr.raw,
        "total_chars": mem.char_count + usr.char_count,
        "char_limit": mem.char_limit + usr.char_limit,
    }))
}

fn honcho_profile_to_dto(profile: gpui_telegram_agent::hermes::memory::DialecticProfile) -> Value {
    let notes: Vec<Value> = profile
        .observations
        .into_iter()
        .map(|obs| serde_json::to_value(obs).unwrap_or(Value::Null))
        .collect();
    json!({
        "enabled": true,
        "peer_name": profile.user_id,
        "session_notes": notes,
        "profile_summary": profile.summary,
        "updated_at_ms": profile.updated_at_ms,
    })
}

fn provider_to_dto(
    cfg: &ProviderConfig,
    active_provider: &str,
    model_profiles: &HashMap<String, gpui_telegram_agent::hermes::providers::ModelProfile>,
) -> Value {
    let mut models = Vec::new();
    if !cfg.default_model.trim().is_empty() {
        models.push(cfg.default_model.clone());
    }
    for m in &cfg.fallback_models {
        if !models.contains(m) {
            models.push(m.clone());
        }
    }
    for prof in model_profiles.values() {
        if prof.provider_id == cfg.id && !models.contains(&prof.model) {
            models.push(prof.model.clone());
        }
    }
    json!({
        "provider_id": cfg.id,
        "name": cfg.name,
        "api_mode": cfg.api_mode.as_str(),
        "base_url": if cfg.base_url.trim().is_empty() { None } else { Some(cfg.base_url.clone()) },
        "default_model": cfg.default_model,
        "models": models,
        "enabled": cfg.enabled,
        "has_api_key": !cfg.credentials.entries.is_empty(),
        "is_active": cfg.id == active_provider,
    })
}

pub(crate) fn resolve_bot_display_name(state: &AppState, bot_user_id: UserId) -> Option<String> {
    let user = state.db.get_user(bot_user_id).ok().flatten()?;
    let first = user.first_name.trim();
    if !first.is_empty() {
        return Some(first.to_string());
    }
    user.username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

pub(crate) fn load_bot_persona_config(
    state: &AppState,
    bot_user_id: UserId,
) -> (Option<String>, Option<String>, Option<String>) {
    let key = bot_user_id.0.to_string();
    let kv_soul = state
        .hermes
        .store()
        .kv_get("bot_soul", &key)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let kv_persona = state
        .hermes
        .store()
        .kv_get("bot_persona", &key)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());

    let user = state.db.get_user(bot_user_id).ok().flatten();
    let mut system_prompt = kv_soul
        .as_ref()
        .and_then(|v| {
            v.get("system_prompt")
                .or_else(|| v.get("content"))
                .and_then(|s| s.as_str())
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            kv_persona
                .as_ref()
                .and_then(|v| {
                    v.get("system_prompt")
                        .or_else(|| v.get("persona"))
                        .and_then(|s| s.as_str())
                })
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
        });

    let reg_bot = state
        .db
        .get_bot_by_user_id(bot_user_id.0 as i64)
        .ok()
        .flatten();
    if system_prompt.is_none() {
        if let Some(ref b) = reg_bot {
            let apps = state
                .db
                .list_mini_apps_for_bot(b.bot_user_id)
                .unwrap_or_default();
            let mut prompt_parts = Vec::new();
            if !b.ai_persona.trim().is_empty() {
                prompt_parts.push(b.ai_persona.trim().to_string());
            } else {
                prompt_parts.push(format!(
                    "Your name is {} (@{}). You are a helpful AI Telegram bot assistant.",
                    b.display_name, b.username
                ));
            }
            if !b.description.trim().is_empty() {
                prompt_parts.push(format!("Bot description: {}", b.description.trim()));
            }
            if !apps.is_empty() {
                let app_list = apps
                    .iter()
                    .map(|a| format!("{} {} (/miniapps/{}/{})", a.icon_emoji, a.title, a.bot_username, a.short_name))
                    .collect::<Vec<_>>()
                    .join("; ");
                prompt_parts.push(format!(
                    "You are also the AI companion bot for Mini App(s): {}. Help users use the Mini App, summarize or act on their Mini App data, and reply concisely in their language.",
                    app_list
                ));
            }
            system_prompt = Some(prompt_parts.join("\n"));
        } else if let Some(u) = &user {
            let display_name = if u.first_name.trim().is_empty() {
                u.username.as_deref().unwrap_or("Assistant")
            } else {
                u.first_name.trim()
            };
            let mut identity = format!(
                "Your name is {} (@{}). Reply naturally and helpfully in the user's language, and never refer to yourself by numeric user ID.",
                display_name,
                u.username.as_deref().unwrap_or("bot")
            );
            if let Some(bio) = u.bio.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                identity.push(' ');
                identity.push_str(bio);
            }
            system_prompt = Some(identity);
        }
    }

    let model = kv_soul
        .as_ref()
        .and_then(|v| v.get("model").and_then(|s| s.as_str()))
        .or_else(|| {
            kv_persona
                .as_ref()
                .and_then(|v| v.get("model").and_then(|s| s.as_str()))
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let welcome = kv_soul
        .as_ref()
        .and_then(|v| v.get("welcome_message").and_then(|s| s.as_str()))
        .or_else(|| {
            kv_persona
                .as_ref()
                .and_then(|v| v.get("welcome_message").and_then(|s| s.as_str()))
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    (system_prompt, model, welcome)
}

pub(crate) fn save_bot_persona_kv(
    state: &AppState,
    bot_user_id: UserId,
    chat_id: Option<ChatId>,
    persona_name: &str,
    persona: Option<&str>,
    system_prompt: Option<&str>,
    model: Option<&str>,
    welcome_message: Option<&str>,
) {
    let key = bot_user_id.0.to_string();
    let payload = json!({
        "bot_id": bot_user_id.0,
        "chat_id": chat_id.map(|c| c.0),
        "persona_name": persona_name,
        "persona": persona,
        "system_prompt": system_prompt.or(persona),
        "model": model,
        "welcome_message": welcome_message,
        "updated_at_ms": Utc::now().timestamp_millis(),
    });
    if let Ok(raw) = serde_json::to_string(&payload) {
        let _ = state.hermes.store().kv_set("bot_persona", &key, &raw);
    }
}

pub(crate) fn build_default_bot_quick_actions(
    res: &HermesTurnResult,
    prompt_hint: &str,
) -> Vec<Vec<InlineKeyboardButton>> {
    let pending_approval_id = res.events.iter().find_map(|ev| match ev {
        HermesEvent::ApprovalRequired { approval_id, .. } => Some(approval_id.as_str()),
        _ => None,
    });
    build_bot_quick_actions_from_parts(&res.buttons, pending_approval_id, prompt_hint)
}

pub(crate) fn build_bot_quick_actions_from_parts(
    buttons: &[Vec<InlineActionButton>],
    pending_approval_id: Option<&str>,
    prompt_hint: &str,
) -> Vec<Vec<InlineKeyboardButton>> {
    if !buttons.is_empty() {
        return map_proto_buttons(buttons);
    }
    if let Some(approval_id) = pending_approval_id.filter(|s| !s.trim().is_empty()) {
        return vec![vec![
            InlineKeyboardButton {
                text: "✅ Approve Once".to_string(),
                callback_data: Some(format!("hermes:approve:{approval_id}:approve_once")),
                url: None,
                web_app_url: None,
            },
            InlineKeyboardButton {
                text: "🛡️ Session".to_string(),
                callback_data: Some(format!("hermes:approve:{approval_id}:approve_session")),
                url: None,
                web_app_url: None,
            },
            InlineKeyboardButton {
                text: "❌ Deny".to_string(),
                callback_data: Some(format!("hermes:approve:{approval_id}:deny")),
                url: None,
                web_app_url: None,
            },
        ]];
    }

    let lower = prompt_hint.to_ascii_lowercase();
    let mut row = Vec::new();
    if lower.contains("voice")
        || lower.contains("talk")
        || lower.contains("call")
        || lower.contains("speak")
    {
        row.push(InlineKeyboardButton {
            text: "🎙️ Talk Mode".to_string(),
            callback_data: Some("talk:start".to_string()),
            url: None,
            web_app_url: None,
        });
    }
    if lower.contains("desktop")
        || lower.contains("computer")
        || lower.contains("screen")
        || lower.contains("browser")
    {
        row.push(InlineKeyboardButton {
            text: "🖥️ Remote Computer".to_string(),
            callback_data: Some("computer:start".to_string()),
            url: None,
            web_app_url: None,
        });
    }
    if row.is_empty() {
        Vec::new()
    } else {
        vec![row]
    }
}

fn is_deepseek_provider(active_provider_id: &str, active_base_url: Option<&str>) -> bool {
    let pid = active_provider_id.to_ascii_lowercase();
    let url = active_base_url.unwrap_or("").to_ascii_lowercase();
    pid.contains("deepseek") || url.contains("deepseek") || url.contains("ai.ombhrum.com")
}

pub(crate) fn sanitize_model_override_for_provider(
    raw: Option<String>,
    active_provider_id: &str,
    active_base_url: Option<&str>,
) -> Option<String> {
    raw.map(|m| m.trim().to_string()).filter(|m| {
        if m.is_empty() {
            return false;
        }
        let lower = m.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "default"
                | "默认"
                | "默认模型"
                | "hermes-local-v1"
                | "local_hermes/hermes-local-v1"
                | "auto"
        ) {
            return false;
        }
        if lower == "deepseek-flash" && !is_deepseek_provider(active_provider_id, active_base_url)
        {
            return false;
        }
        true
    })
}

pub(crate) fn sanitize_model_override(state: &AppState, raw: Option<String>) -> Option<String> {
    let snap = state.hermes.providers().snapshot();
    let active_base_url = snap
        .providers
        .get(&snap.active_provider_id)
        .map(|p| p.base_url.as_str());
    sanitize_model_override_for_provider(raw, &snap.active_provider_id, active_base_url)
}

fn is_autonomous_policy_str(raw: &str) -> bool {
    let normalized: String = raw
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .flat_map(|c| c.to_lowercase())
        .collect();
    matches!(normalized.as_str(), "alwaysallowauthorized" | "yolo")
}

pub(crate) fn bot_execution_policy_is_yolo(
    state: &AppState,
    user_id: u64,
    bot_user_id: UserId,
) -> bool {
    let key = bot_user_id.0.to_string();
    for ns in ["ai_bot_profile", "bot_soul", "bot_persona"] {
        if let Ok(Some(raw)) = state.hermes.store().kv_get(ns, &key) {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                if let Some(policy) = val
                    .get("execution_policy")
                    .or_else(|| val.get("approval_mode"))
                    .and_then(|v| v.as_str())
                {
                    if is_autonomous_policy_str(policy) {
                        return true;
                    }
                    let norm = policy.trim().to_ascii_lowercase();
                    if matches!(
                        norm.as_str(),
                        "ask_every_time" | "allow_read_only" | "disabled" | "prompt" | "deny"
                    ) {
                        return false;
                    }
                }
            }
        }
    }
    state
        .db
        .get_bot_computer_profile_or_default(user_id, bot_user_id.0)
        .map(|p| p.approval_mode == ComputerRunPolicy::AlwaysAllowAuthorized)
        .unwrap_or(false)
}

pub(crate) fn resolve_and_apply_bot_session_policy(
    state: &AppState,
    user_id: u64,
    bot_user_id: UserId,
    chat_id: Option<ChatId>,
    session_id_hint: Option<&str>,
    source: &str,
    model_override: Option<&str>,
    force_yolo: bool,
) -> gpui_telegram_core::error::Result<(String, bool)> {
    let engine = resolve_hermes_for_bot(state, Some(bot_user_id.0));
    let sess = engine.resolve_or_create_session(
        session_id_hint,
        chat_id.map(|c| c.0),
        Some(&bot_user_id.0.to_string()),
        source,
        model_override,
    )?;
    let instance_yolo = state
        .fabushi_bots
        .get_by_user_id(bot_user_id.0 as i64)
        .is_some_and(|i| i.settings().approval_mode == "autonomous");
    let yolo = force_yolo || instance_yolo || bot_execution_policy_is_yolo(state, user_id, bot_user_id);
    if yolo {
        if let Ok(mut sec) = engine.security().write() {
            sec.approvals
                .set_session_mode(sess.id.clone(), ApprovalMode::Yolo);
        }
    }
    Ok((sess.id, yolo))
}

pub(crate) fn build_enriched_bot_turn_context(
    state: &AppState,
    user_id: u64,
    bot_id: Option<u64>,
    bot_display_name: Option<&str>,
    base_system_prompt: Option<String>,
    explicit_model: Option<String>,
    fallback_model: Option<String>,
    extra_sections: &[String],
    include_state_patch_hint: bool,
) -> (Option<String>, Option<String>) {
    let (computer_enriched, raw_model) = crate::routes::computers::enrich_bot_turn_prompt_and_model(
        state,
        user_id,
        bot_id,
        base_system_prompt.clone(),
        explicit_model,
        fallback_model,
    );
    let effective_model = sanitize_model_override(state, raw_model);

    let now_ms = Utc::now().timestamp_millis();
    let profile = bot_id.and_then(|bid| {
        state
            .db
            .get_bot_computer_profile_or_default(user_id, bid)
            .ok()
    });
    let nodes = state
        .db
        .list_computer_nodes(user_id, None, now_ms)
        .unwrap_or_default();

    let default_computer_id = profile
        .as_ref()
        .and_then(|p| p.bound_node_id.clone())
        .or_else(|| {
            bot_id.and_then(|bid| {
                nodes
                    .iter()
                    .find(|n| n.bound_bot_ids.contains(&bid))
                    .map(|n| n.node_id.clone())
            })
        })
        .or_else(|| {
            nodes
                .iter()
                .find(|n| n.online)
                .or_else(|| nodes.first())
                .map(|n| n.node_id.clone())
        });

    let mut prompt_blocks: Vec<String> = Vec::new();
    let persona_body = computer_enriched
        .unwrap_or_else(|| "You are an autonomous Hermes AI Telegram Assistant.".to_string());
    let has_soul_header = persona_body.trim_start().starts_with("# ");
    if !has_soul_header {
        let name_label = bot_display_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Hermes Assistant");
        prompt_blocks.push(format!("# {name_label}\n- Role: Autonomous AI Telegram Agent\n\n## Persona\n{persona_body}"));
    } else {
        prompt_blocks.push(persona_body);
    }

    if !nodes.is_empty() || default_computer_id.is_some() {
        let mut comp_lines = vec!["## Live Computer Node Inventory".to_string()];
        if let Some(ref def_id) = default_computer_id {
            comp_lines.push(format!("- default_computer_id: `{def_id}`"));
        }
        for n in &nodes {
            let host_label = n
                .agent_endpoint_url
                .as_deref()
                .or(n.host.as_deref())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(if n.ip_address.trim().is_empty() {
                    "local"
                } else {
                    n.ip_address.as_str()
                });
            let os_label = if n.os_name.trim().is_empty() {
                "unknown"
            } else {
                n.os_name.trim()
            };
            let is_bound = default_computer_id.as_deref() == Some(n.node_id.as_str())
                || bot_id.is_some_and(|bid| n.bound_bot_ids.contains(&bid));
            comp_lines.push(format!(
                "- id=`{}` | name=\"{}\" | ip_or_host=`{}` | os=`{}` | online={} | policy={} | bound={}",
                n.node_id,
                n.display_name,
                host_label,
                os_label,
                n.online,
                n.run_policy.as_str(),
                is_bound
            ));
        }
        prompt_blocks.push(comp_lines.join("\n"));
    }

    for sec in extra_sections {
        let trimmed = sec.trim();
        if !trimmed.is_empty() {
            prompt_blocks.push(trimmed.to_string());
        }
    }

    if include_state_patch_hint {
        prompt_blocks.push(
            "## Companion Mini App State Patch Protocol\nIf the user asks you to add, update, or modify data in the companion Mini App, include a valid JSON object wrapped in `[STATE_PATCH]{...}[/STATE_PATCH]` at the end of your reply so the live Mini App state updates automatically."
                .to_string(),
        );
    }

    let combined_so_far = prompt_blocks.join("\n\n");
    if !combined_so_far.contains("## Directives") {
        prompt_blocks.push(
            "## Directives\n\
             - Execute multi-step requests iteratively using Hermes tools (`computer_list`, `computer_status`, `computer_lease_acquire`, `computer_exec`, `computer_file`, `computer_configure_cloud`, `computer_bind`, `terminal`, `read_file`, `write_file`, `search_files`, `skill_list`, `skill_view`, `skill_manage`, `memory`) until the task is genuinely verified.\n\
             - Base all answers strictly on verified tool outputs and live state; never fabricate results or suppress tool/provider failures.\n\
             - Never disclose raw SSH passwords, private keys, API keys, or tokens.\n\
             - Capture reusable procedures or user preferences with `skill_manage` / `memory` when helpful, and respond in the user's language."
                .to_string(),
        );
    }

    (Some(prompt_blocks.join("\n\n")), effective_model)
}

pub(crate) struct BotTypingGuard {
    state: Arc<AppState>,
    chat_id: ChatId,
    bot_user_id: UserId,
    heartbeat_task: Option<tokio::task::JoinHandle<()>>,
    stopped: bool,
}

impl BotTypingGuard {
    pub(crate) fn start(state: Arc<AppState>, chat_id: ChatId, bot_user_id: UserId) -> Self {
        Self::emit_typing(&state, chat_id, bot_user_id, TypingAction::Typing);
        let hb_state = Arc::clone(&state);
        let heartbeat_task = tokio::runtime::Handle::try_current().ok().map(|rt| {
            rt.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(4));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                interval.tick().await;
                loop {
                    interval.tick().await;
                    Self::emit_typing(&hb_state, chat_id, bot_user_id, TypingAction::Typing);
                }
            })
        });
        Self {
            state,
            chat_id,
            bot_user_id,
            heartbeat_task,
            stopped: false,
        }
    }

    pub(crate) fn finish(mut self) {
        self.stop_inner();
    }

    fn emit_typing(state: &AppState, chat_id: ChatId, bot_user_id: UserId, action: TypingAction) {
        broadcast_chat_event(
            state,
            chat_id,
            UpdateEvent::UserTyping(TypingEvent::new(chat_id, bot_user_id, action)),
        );
    }

    fn stop_inner(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Some(handle) = self.heartbeat_task.take() {
            handle.abort();
        }
        Self::emit_typing(&self.state, self.chat_id, self.bot_user_id, TypingAction::Cancel);
    }
}

impl Drop for BotTypingGuard {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

fn find_ascii_case_from(haystack: &str, needle: &str, start: usize) -> Option<usize> {
    if start >= haystack.len() || needle.is_empty() || needle.len() > haystack.len() - start {
        return None;
    }
    let needle_lower = needle.to_ascii_lowercase();
    for (rel_idx, _) in haystack[start..].char_indices() {
        let abs = start + rel_idx;
        if let Some(slice) = haystack.get(abs..abs + needle.len()) {
            if slice.eq_ignore_ascii_case(&needle_lower) {
                return Some(abs);
            }
        }
    }
    None
}

pub(crate) fn extract_state_patches(raw: &str) -> Vec<Value> {
    const OPEN_TAG: &str = "[STATE_PATCH]";
    const CLOSE_TAG: &str = "[/STATE_PATCH]";
    let mut patches = Vec::new();
    let mut cursor = 0;
    while let Some(open_idx) = find_ascii_case_from(raw, OPEN_TAG, cursor) {
        let body_start = open_idx + OPEN_TAG.len();
        let Some(close_idx) = find_ascii_case_from(raw, CLOSE_TAG, body_start) else {
            break;
        };
        let mut inner = raw[body_start..close_idx].trim();
        if let Some(stripped) = inner.strip_prefix("```json") {
            inner = stripped.trim();
        } else if let Some(stripped) = inner.strip_prefix("```") {
            inner = stripped.trim();
        }
        if let Some(stripped) = inner.strip_suffix("```") {
            inner = stripped.trim();
        }
        if let Ok(val) = serde_json::from_str::<Value>(inner) {
            if val.is_object() {
                patches.push(val);
            }
        }
        cursor = close_idx + CLOSE_TAG.len();
    }
    patches
}

pub(crate) fn strip_state_patches_for_display(raw: &str) -> String {
    const OPEN_TAG: &str = "[STATE_PATCH]";
    const CLOSE_TAG: &str = "[/STATE_PATCH]";
    let mut out = String::with_capacity(raw.len());
    let mut cursor = 0;
    while let Some(open_idx) = find_ascii_case_from(raw, OPEN_TAG, cursor) {
        out.push_str(&raw[cursor..open_idx]);
        let body_start = open_idx + OPEN_TAG.len();
        if let Some(close_idx) = find_ascii_case_from(raw, CLOSE_TAG, body_start) {
            cursor = close_idx + CLOSE_TAG.len();
        } else {
            // Unclosed tag while streaming: conceal incomplete patch payload.
            cursor = raw.len();
            break;
        }
    }
    if cursor < raw.len() {
        out.push_str(&raw[cursor..]);
    }
    // Conceal a partial trailing `[STATE_PATCH]` prefix (e.g. `[STATE_`) while streaming.
    if let Some(bracket_idx) = out.rfind('[') {
        let tail = &out[bracket_idx..];
        if tail.len() < OPEN_TAG.len()
            && OPEN_TAG[..tail.len()].eq_ignore_ascii_case(tail)
        {
            out.truncate(bracket_idx);
        }
    }
    out.trim().to_string()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let prefix: String = text.chars().take(keep).collect();
    format!("{prefix}…")
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    let skip = char_count.saturating_sub(max_chars.saturating_sub(1));
    let tail: String = text.chars().skip(skip).collect();
    format!("…{tail}")
}

fn is_sensitive_arg_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("password")
        || k.contains("secret")
        || k.contains("token")
        || k.contains("private_key")
        || k.contains("api_key")
        || k.contains("credential")
}

fn format_tool_args_preview(arguments: &Value) -> String {
    if let Some(obj) = arguments.as_object() {
        for preferred in ["command", "path", "node_id", "query", "action", "name", "url"] {
            if let Some(v) = obj.get(preferred).and_then(|x| x.as_str()) {
                let single_line = v.split_whitespace().collect::<Vec<_>>().join(" ");
                if !single_line.is_empty() {
                    return truncate_chars(&single_line, 72);
                }
            }
        }
        let parts: Vec<String> = obj
            .iter()
            .take(3)
            .map(|(k, v)| {
                if is_sensitive_arg_key(k) {
                    format!("{k}=***")
                } else {
                    let s = match v {
                        Value::String(s) => s.split_whitespace().collect::<Vec<_>>().join(" "),
                        other => other.to_string(),
                    };
                    format!("{k}={}", truncate_chars(&s, 28))
                }
            })
            .collect();
        return truncate_chars(&parts.join(", "), 80);
    }
    let compact = arguments.to_string().split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&compact, 72)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepStatus {
    Running,
    Ok,
    Err,
}

#[derive(Debug, Clone)]
struct StepTrace {
    idx: usize,
    tool_call_id: String,
    tool_name: String,
    args_preview: String,
    status: StepStatus,
    duration_ms: Option<u64>,
    result_preview: Option<String>,
}

pub(crate) fn format_live_bot_runtime_card_section(
    state: &AppState,
    user_id: u64,
    bot_user_id: UserId,
) -> String {
    let (active_provider, active_model, _) = state.hermes.providers().active_info();
    let now_ms = Utc::now().timestamp_millis();
    let profile = state
        .db
        .get_bot_computer_profile_or_default(user_id, bot_user_id.0)
        .ok();
    let nodes = state
        .db
        .list_computer_nodes(user_id, None, now_ms)
        .unwrap_or_default();

    let mut lines = vec![format!(
        "⚡ **实时引擎与算力节点**\n• 当前模型：`{}` (`{}`)",
        active_model, active_provider
    )];
    if let Some(ref p) = profile {
        lines.push(format!("• 执行策略：`{}`", p.approval_mode.label_zh()));
    }
    let bound_nodes: Vec<_> = nodes
        .iter()
        .filter(|n| {
            profile
                .as_ref()
                .and_then(|p| p.bound_node_id.as_deref())
                == Some(n.node_id.as_str())
                || n.bound_bot_ids.contains(&bot_user_id.0)
        })
        .collect();
    let display_nodes = if bound_nodes.is_empty() {
        nodes.iter().take(3).collect::<Vec<_>>()
    } else {
        bound_nodes
    };
    if display_nodes.is_empty() {
        lines.push("• 受控电脑：暂无已绑定节点（可在「我的电脑」添加或绑定）".to_string());
    } else {
        for n in display_nodes {
            let status = if n.online { "🟢 在线" } else { "⚪ 离线" };
            lines.push(format!(
                "• 电脑节点：**{}** (`{}`) · {} · {}",
                n.display_name,
                n.node_id,
                status,
                n.run_policy.label_zh()
            ));
        }
    }
    lines.join("\n")
}

struct StreamConsumerState {
    thinking_buf: String,
    tool_steps: Vec<StepTrace>,
    round_answer_buf: String,
    final_done_text: Option<String>,
    goal_status: Option<String>,
    pending_approval_id: Option<String>,
    approval_notice: Option<String>,
    event_buttons: Vec<Vec<InlineActionButton>>,
    terminal_error: Option<String>,
    needs_provider_button: bool,
    saw_terminal_event: bool,
    bot_msg_id: Option<MsgId>,
    last_rendered: String,
    last_kb_json: Option<String>,
    last_edit_at: Option<Instant>,
    dirty: bool,
}

impl StreamConsumerState {
    fn new() -> Self {
        Self {
            thinking_buf: String::new(),
            tool_steps: Vec::new(),
            round_answer_buf: String::new(),
            final_done_text: None,
            goal_status: None,
            pending_approval_id: None,
            approval_notice: None,
            event_buttons: Vec::new(),
            terminal_error: None,
            needs_provider_button: false,
            saw_terminal_event: false,
            bot_msg_id: None,
            last_rendered: String::new(),
            last_kb_json: None,
            last_edit_at: None,
            dirty: false,
        }
    }

    fn apply_event(&mut self, ev: HermesEvent) -> bool {
        match ev {
            HermesEvent::Meta { .. } => false,
            HermesEvent::ThinkingDelta { text } => {
                self.thinking_buf.push_str(&text);
                self.bot_msg_id.is_none() && !self.thinking_buf.trim().is_empty()
            }
            HermesEvent::GoalProgress {
                iteration,
                max_iterations,
                status,
                note,
            } => {
                let note_clean = truncate_chars(&note.split_whitespace().collect::<Vec<_>>().join(" "), 120);
                self.goal_status = Some(format!(
                    "🎯 **目标进度 ({iteration}/{max_iterations})**：{status} — {note_clean}"
                ));
                self.bot_msg_id.is_none()
            }
            HermesEvent::ToolStart {
                tool_call_id,
                tool_name,
                arguments,
            } => {
                self.round_answer_buf.clear();
                let idx = self.tool_steps.len() + 1;
                self.tool_steps.push(StepTrace {
                    idx,
                    tool_call_id,
                    tool_name,
                    args_preview: format_tool_args_preview(&arguments),
                    status: StepStatus::Running,
                    duration_ms: None,
                    result_preview: None,
                });
                true
            }
            HermesEvent::ToolResult {
                tool_call_id,
                tool_name,
                success,
                output,
                duration_ms,
                ..
            } => {
                let single_line = output.split_whitespace().collect::<Vec<_>>().join(" ");
                let preview = if single_line.is_empty() {
                    None
                } else {
                    Some(truncate_chars(&single_line, 96))
                };
                let step_idx = self
                    .tool_steps
                    .iter()
                    .rposition(|s| s.tool_call_id == tool_call_id)
                    .or_else(|| {
                        self.tool_steps
                            .iter()
                            .rposition(|s| s.tool_name == tool_name && s.status == StepStatus::Running)
                    });
                if let Some(step) = step_idx.and_then(|idx| self.tool_steps.get_mut(idx)) {
                    step.status = if success { StepStatus::Ok } else { StepStatus::Err };
                    step.duration_ms = Some(duration_ms);
                    step.result_preview = preview;
                }
                true
            }
            HermesEvent::ApprovalRequired {
                approval_id,
                tool_name,
                command,
                risk_level,
                reason,
                ..
            } => {
                self.pending_approval_id = Some(approval_id);
                self.approval_notice = Some(format!(
                    "⚠️ **等待执行审批** (`{risk_level}`)\n工具：`{tool_name}`\n命令：`{}`\n原因：{reason}",
                    truncate_chars(&command, 140)
                ));
                true
            }
            HermesEvent::InlineButtons { rows } => {
                if !rows.is_empty() {
                    self.event_buttons = rows;
                }
                false
            }
            HermesEvent::CommandResult { outcome } => {
                if !outcome.should_continue_to_llm && !outcome.response_text.trim().is_empty() {
                    self.round_answer_buf = outcome.response_text;
                }
                if !outcome.buttons.is_empty() {
                    self.event_buttons = outcome.buttons;
                }
                true
            }
            HermesEvent::TextDelta { text, accumulated } => {
                if !text.is_empty() {
                    self.round_answer_buf.push_str(&text);
                } else if self.round_answer_buf.is_empty() && !accumulated.is_empty() {
                    self.round_answer_buf = accumulated;
                }
                self.bot_msg_id.is_none()
                    && !strip_state_patches_for_display(&self.round_answer_buf)
                        .trim()
                        .is_empty()
            }
            HermesEvent::Done {
                response_text,
                reasoning,
                buttons,
                ..
            } => {
                self.saw_terminal_event = true;
                if !response_text.trim().is_empty() {
                    self.final_done_text = Some(response_text);
                }
                if self.thinking_buf.trim().is_empty() {
                    if let Some(r) = reasoning {
                        self.thinking_buf = r;
                    }
                }
                if !buttons.is_empty() {
                    self.event_buttons = buttons;
                }
                true
            }
            HermesEvent::Error { code, message } => {
                self.saw_terminal_event = true;
                self.terminal_error = Some(format!("⚠️ **执行失败 ({code})**：{message}"));
                if code == "PROVIDER_ERROR" {
                    self.needs_provider_button = true;
                }
                true
            }
        }
    }

    fn raw_answer_text(&self) -> &str {
        self.final_done_text
            .as_deref()
            .unwrap_or(&self.round_answer_buf)
    }

    fn render_steps_block(&self) -> Option<String> {
        if self.tool_steps.is_empty() {
            return None;
        }
        let total = self.tool_steps.len();
        let done = self
            .tool_steps
            .iter()
            .filter(|s| s.status != StepStatus::Running)
            .count();
        let mut lines = vec![format!("🔧 **执行步骤 ({done}/{total})**")];
        let max_display = 8;
        let start_idx = if total > max_display {
            lines.push(format!("…已折叠前 {} 步", total - max_display + 1));
            total - (max_display - 1)
        } else {
            0
        };
        if start_idx > 0 {
            if let Some(first) = self.tool_steps.first() {
                lines.push(Self::format_step_line(first));
            }
        }
        for step in &self.tool_steps[start_idx..] {
            lines.push(Self::format_step_line(step));
        }
        Some(lines.join("\n"))
    }

    fn format_step_line(step: &StepTrace) -> String {
        let icon = match step.status {
            StepStatus::Running => "⏳",
            StepStatus::Ok => "✅",
            StepStatus::Err => "❌",
        };
        let args = if step.args_preview.is_empty() {
            String::new()
        } else {
            format!(" (`{}`)", step.args_preview)
        };
        let timing = match step.duration_ms {
            Some(ms) => format!(" · {ms}ms"),
            None => String::new(),
        };
        let mut line = format!("{}. {icon} `{}`{args}{timing}", step.idx, step.tool_name);
        if step.status == StepStatus::Err {
            if let Some(ref preview) = step.result_preview {
                line.push_str(&format!(" — {}", truncate_chars(preview, 72)));
            }
        }
        line
    }

    fn render(&self, is_final: bool, extra_suffix: Option<&str>) -> String {
        let mut sections: Vec<String> = Vec::new();

        if !is_final && !self.thinking_buf.trim().is_empty() {
            let bounded = tail_chars(self.thinking_buf.trim(), 700);
            let quoted = bounded.replace('\n', "\n> ");
            sections.push(format!("💭 **思考过程**\n> {quoted}"));
        }

        if let Some(steps_md) = self.render_steps_block() {
            sections.push(steps_md);
        }

        if !is_final {
            if let Some(ref g) = self.goal_status {
                sections.push(g.clone());
            }
        }

        let mut clean_answer = strip_state_patches_for_display(self.raw_answer_text());
        if is_final {
            if let Some(suffix) = extra_suffix.map(str::trim).filter(|s| !s.is_empty()) {
                if !clean_answer.is_empty() {
                    clean_answer.push_str("\n\n");
                }
                clean_answer.push_str(suffix);
            }
        }
        if !clean_answer.trim().is_empty() {
            sections.push(clean_answer);
        }

        if let Some(ref approval) = self.approval_notice {
            sections.push(approval.clone());
        }

        if let Some(ref err) = self.terminal_error {
            sections.push(err.clone());
        } else if is_final && sections.is_empty() {
            if !self.thinking_buf.trim().is_empty() {
                sections.push(self.thinking_buf.trim().to_string());
            } else {
                sections.push(
                    "⚠️ 模型未返回有效内容，请检查当前 active provider 与模型配置是否可用。"
                        .to_string(),
                );
            }
        }

        sections.join("\n\n")
    }
}

pub(crate) struct BotStreamTurnParams {
    pub chat_id: ChatId,
    pub sender_id: UserId,
    pub bot_user_id: UserId,
    pub prompt: String,
    pub base_system_prompt: Option<String>,
    pub bot_display_name: Option<String>,
    pub explicit_model: Option<String>,
    pub bot_model: Option<String>,
    pub extra_context_sections: Vec<String>,
    pub companion_apps: Vec<MiniAppRecord>,
    pub help_bot_commands_suffix: Option<String>,
}

fn attach_companion_webapp_button(
    kb_rows: &mut Vec<Vec<InlineKeyboardButton>>,
    apps: &[MiniAppRecord],
) {
    let Some(first) = apps.first() else {
        return;
    };
    let has_webapp_btn = kb_rows
        .iter()
        .flatten()
        .any(|b| b.web_app_url.is_some());
    if !has_webapp_btn {
        kb_rows.insert(
            0,
            vec![InlineKeyboardButton {
                text: format!("{} 打开 {}", first.icon_emoji, first.title),
                callback_data: None,
                url: None,
                web_app_url: Some(format!(
                    "gpui-miniapp://{}/{}",
                    first.bot_username, first.short_name
                )),
            }],
        );
    }
}

fn flush_streamed_bot_message(
    state: &Arc<AppState>,
    params: &BotStreamTurnParams,
    consumer: &mut StreamConsumerState,
    is_final: bool,
) {
    if is_final && !params.companion_apps.is_empty() {
        let patches = extract_state_patches(consumer.raw_answer_text());
        if !patches.is_empty() {
            crate::routes::bots_miniapps::apply_miniapp_state_patches_for_bot_user(
                state,
                params.sender_id.0 as i64,
                params.bot_user_id.0 as i64,
                params.chat_id.0,
                &params.companion_apps,
                &patches,
            );
        }
    }

    let rendered = consumer.render(is_final, params.help_bot_commands_suffix.as_deref());
    if rendered.trim().is_empty() {
        return;
    }

    let mut kb_rows = if is_final || consumer.pending_approval_id.is_some() || !consumer.event_buttons.is_empty() {
        build_bot_quick_actions_from_parts(
            &consumer.event_buttons,
            consumer.pending_approval_id.as_deref(),
            &params.prompt,
        )
    } else {
        Vec::new()
    };
    attach_companion_webapp_button(&mut kb_rows, &params.companion_apps);

    let kb_json_opt = if kb_rows.is_empty() {
        None
    } else {
        serde_json::to_string(&kb_rows).ok()
    };

    if let Some(msg_id) = consumer.bot_msg_id {
        let kb_changed = kb_json_opt.is_some() && kb_json_opt != consumer.last_kb_json;
        if rendered != consumer.last_rendered || kb_changed {
            if kb_changed {
                if let Some(ref kb_json) = kb_json_opt {
                    let _ = state
                        .db
                        .save_message_extras(params.chat_id, msg_id, Some(kb_json), None);
                }
                consumer.last_kb_json = kb_json_opt;
            }
            if let Ok(pts_update) = state.message_pipeline.edit_message(
                params.chat_id,
                msg_id,
                MessageContent::plain_text(&rendered),
            ) {
                broadcast_chat_pts(state, params.chat_id, pts_update);
                consumer.last_rendered = rendered;
            }
        }
    } else {
        let mut bot_msg = MessageItem::new(
            MsgId(0),
            params.chat_id,
            params.bot_user_id,
            PeerId::Chat(params.chat_id),
            MessageContent::plain_text(&rendered),
        );
        bot_msg.flags.is_out = false;
        if let Ok(pts_update) = state.message_pipeline.process_incoming_message(bot_msg) {
            if let UpdateEvent::NewMessage(saved) = &pts_update.event {
                consumer.bot_msg_id = Some(saved.id);
                if let Some(ref kb_json) = kb_json_opt {
                    let _ = state
                        .db
                        .save_message_extras(params.chat_id, saved.id, Some(kb_json), None);
                    consumer.last_kb_json = kb_json_opt;
                }
            }
            broadcast_chat_pts(state, params.chat_id, pts_update);
            consumer.last_rendered = rendered;
        }
    }

    consumer.last_edit_at = Some(Instant::now());
    consumer.dirty = false;
}

pub(crate) async fn execute_bot_turn_streamed(
    state: &Arc<AppState>,
    params: BotStreamTurnParams,
) {
    let typing_guard =
        BotTypingGuard::start(Arc::clone(state), params.chat_id, params.bot_user_id);

    let bot_inst = crate::routes::bots_miniapps::ensure_fabushi_bot_hydrated(
        state,
        params.bot_user_id.0 as i64,
    );
    let target_engine = bot_inst
        .as_ref()
        .map(|i| i.hermes())
        .unwrap_or_else(|| Arc::clone(&state.hermes));
    let mut exec_guard = state.fabushi_bots.begin_bot_execution(
        params.bot_user_id.0 as i64,
        params.chat_id.0,
        params.sender_id.0 as i64,
        &params.prompt,
        None,
    );

    let fallback_bot_model = params.bot_model.clone().or_else(|| {
        bot_inst
            .as_ref()
            .map(|i| i.settings().preferred_model)
            .filter(|m| !m.trim().is_empty())
    });

    let (enriched_sp, effective_model) = build_enriched_bot_turn_context(
        state,
        params.sender_id.0,
        Some(params.bot_user_id.0),
        params.bot_display_name.as_deref(),
        params.base_system_prompt.clone(),
        params.explicit_model.clone(),
        fallback_bot_model,
        &params.extra_context_sections,
        !params.companion_apps.is_empty(),
    );

    let session_id_opt = resolve_and_apply_bot_session_policy(
        state,
        params.sender_id.0,
        params.bot_user_id,
        Some(params.chat_id),
        None,
        "telegram_bot",
        effective_model.as_deref(),
        false,
    )
    .ok()
    .map(|(sid, _)| sid);

    let mut room_groups = Vec::new();
    if let Ok(member_ids) = state.db.get_chat_members(params.chat_id) {
        let bot_members: Vec<AgentAddress> = member_ids
            .iter()
            .filter_map(|uid| state.fabushi_bots.get_by_user_id(uid.0 as i64))
            .map(|inst| inst.agent_address())
            .collect();
        if bot_members.len() > 1 {
            let chat_title = state
                .db
                .get_chat(params.chat_id)
                .ok()
                .flatten()
                .map(|c| c.title)
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| format!("Group {}", params.chat_id.0));
            room_groups.push(AgentGroupAddress {
                address: AgentAddress {
                    id: format!("chat:{}", params.chat_id.0),
                    name: chat_title,
                    description: Some("Shared Telegram multi-bot conversation".to_string()),
                    is_group: true,
                },
                members: bot_members,
            });
        }
    }
    let mentioned_teammates = state
        .fabushi_bots
        .collect_mentioned_teammates(params.bot_user_id.0 as i64, &params.prompt);
    let (fabushi_sp, profile_delta_prefix) = if bot_inst.is_some() {
        let (sp, delta) = state.fabushi_bots.build_turn_system_prompt(
            params.bot_user_id.0 as i64,
            params.chat_id.0,
            enriched_sp.as_deref().unwrap_or(""),
            &room_groups,
            &mentioned_teammates,
        );
        (
            if sp.trim().is_empty() {
                enriched_sp
            } else {
                Some(sp)
            },
            delta,
        )
    } else {
        (enriched_sp, None)
    };

    let caller_scope = build_caller_scope(state, params.sender_id);
    let owns_bot = caller_owns_bot_or_admin(state, params.sender_id, params.bot_user_id.0).is_ok();
    let skip_slash = if params.prompt.trim().starts_with('/') {
        if !owns_bot
            && gpui_telegram_agent::hermes::commands::is_bot_config_command(&params.prompt)
        {
            true
        } else {
            SlashCommandParser::parse(&params.prompt)
                .is_some_and(|parsed| target_engine.commands().get(&parsed.command).is_none())
        }
    } else {
        false
    };

    let final_prompt = match profile_delta_prefix {
        Some(ref delta) if !params.prompt.trim_start().starts_with('/') => {
            format!("{delta}\n\n{}", params.prompt)
        }
        _ => params.prompt.clone(),
    };

    let mut req = HermesTurnRequest::new(final_prompt)
        .with_chat_bot(params.chat_id.0, params.bot_user_id.0.to_string());
    req.session_id = session_id_opt;
    req.bot_name = params.bot_display_name.clone();
    req.user_id = Some(params.sender_id.0.to_string());
    req.source = "telegram_bot".to_string();
    req.preset = Some(
        bot_inst
            .as_ref()
            .map(|inst| FabushiBotHost::toolset_preset_for_name(&inst.settings().toolset_preset))
            .unwrap_or(ToolsetPreset::HermesTelegram),
    );
    req.thinking_budget_tokens = bot_inst.as_ref().and_then(|inst| {
        FabushiBotHost::thinking_budget_for_level(&inst.settings().thinking_level)
    });
    req.model = effective_model;
    req.system_prompt = fabushi_sp;
    req.skip_slash_commands = skip_slash;
    req.caller_allowed_chat_ids = Some(caller_scope.allowed_chat_ids);
    req.caller_is_admin = caller_scope.is_admin;
    req.caller_owns_bot = Some(owns_bot);

    let mut rx = match target_engine.start_turn_stream(req) {
        Ok(rx) => rx,
        Err(err) => {
            if let Some(g) = exec_guard.as_mut() {
                g.mark_failed(err.to_string());
            }
            let mut err_consumer = StreamConsumerState::new();
            err_consumer.terminal_error = Some(format!("⚠️ **无法启动 AI 回复**：{err}"));
            if target_engine.providers().needs_provider_selection() {
                err_consumer.event_buttons.push(vec![InlineActionButton {
                    text: "⚙️ 选择 AI 供应商 (Magpie)".to_string(),
                    callback_data: "hermes:open_ai_providers".to_string(),
                    url: None,
                }]);
            }
            flush_streamed_bot_message(state, &params, &mut err_consumer, true);
            typing_guard.finish();
            return;
        }
    };

    const EDIT_THROTTLE: Duration = Duration::from_millis(350);
    let mut consumer = StreamConsumerState::new();

    enum LoopWake {
        FlushTimer,
        Event(Option<HermesEvent>),
    }

    loop {
        let sleep_fut = async {
            if let Some(last_edit) = consumer.last_edit_at.filter(|_| consumer.dirty) {
                tokio::time::sleep_until(last_edit + EDIT_THROTTLE).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        let wake = tokio::select! {
            _ = sleep_fut => LoopWake::FlushTimer,
            ev = rx.recv() => LoopWake::Event(ev),
        };
        match wake {
            LoopWake::FlushTimer => {
                flush_streamed_bot_message(state, &params, &mut consumer, false);
            }
            LoopWake::Event(None) => break,
            LoopWake::Event(Some(first_ev)) => {
                let mut flush_now = consumer.apply_event(first_ev);
                while let Ok(next_ev) = rx.try_recv() {
                    if consumer.apply_event(next_ev) {
                        flush_now = true;
                    }
                }
                consumer.dirty = true;
                if consumer.saw_terminal_event {
                    if consumer.needs_provider_button
                        && target_engine.providers().needs_provider_selection()
                        && !consumer
                            .event_buttons
                            .iter()
                            .flatten()
                            .any(|b| b.callback_data == "hermes:open_ai_providers")
                    {
                        consumer.event_buttons.push(vec![InlineActionButton {
                            text: "⚙️ 选择 AI 供应商 (Magpie)".to_string(),
                            callback_data: "hermes:open_ai_providers".to_string(),
                            url: None,
                        }]);
                    }
                    flush_streamed_bot_message(state, &params, &mut consumer, true);
                    break;
                } else if flush_now
                    || consumer
                        .last_edit_at
                        .is_none_or(|t| t.elapsed() >= EDIT_THROTTLE)
                {
                    flush_streamed_bot_message(state, &params, &mut consumer, false);
                }
            }
        }
    }

    if !consumer.saw_terminal_event {
        if consumer.needs_provider_button
            && target_engine.providers().needs_provider_selection()
            && !consumer
                .event_buttons
                .iter()
                .flatten()
                .any(|b| b.callback_data == "hermes:open_ai_providers")
        {
            consumer.event_buttons.push(vec![InlineActionButton {
                text: "⚙️ 选择 AI 供应商 (Magpie)".to_string(),
                callback_data: "hermes:open_ai_providers".to_string(),
                url: None,
            }]);
        }
        flush_streamed_bot_message(state, &params, &mut consumer, true);
    }

    if let Some(g) = exec_guard.as_mut() {
        if let Some(ref err) = consumer.terminal_error {
            g.mark_failed(err.clone());
        } else {
            g.mark_success();
        }
    }

    typing_guard.finish();
}

pub(crate) async fn deliver_hermes_proactive_message(
    state: &AppState,
    chat_id: ChatId,
    bot_id_hint: Option<&str>,
    text: &str,
    buttons: Option<Vec<Vec<InlineKeyboardButton>>>,
) -> Option<MessageItem> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if state.db.get_chat(chat_id).ok().flatten().is_none() {
        return None;
    }

    let bot_user_id = bot_id_hint
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map(UserId)
        .filter(|uid| {
            state
                .db
                .get_user(*uid)
                .ok()
                .flatten()
                .is_some_and(|u| u.is_bot)
        })
        .or_else(|| {
            state
                .db
                .get_chat_members(chat_id)
                .ok()
                .and_then(|members| {
                    members.into_iter().find(|uid| {
                        state
                            .db
                            .get_user(*uid)
                            .ok()
                            .flatten()
                            .is_some_and(|u| u.is_bot)
                    })
                })
        })
        .unwrap_or_else(|| ensure_default_ai_bot(&state.db));

    let kb_json_opt = buttons
        .as_ref()
        .filter(|kb| !kb.is_empty())
        .and_then(|kb| serde_json::to_string(kb).ok());

    let mut bot_msg = MessageItem::new(
        MsgId(0),
        chat_id,
        bot_user_id,
        PeerId::Chat(chat_id),
        MessageContent::plain_text(trimmed),
    );
    bot_msg.flags.is_out = false;

    let pts_update = state
        .message_pipeline
        .process_incoming_message(bot_msg)
        .ok()?;
    let saved_msg = match &pts_update.event {
        UpdateEvent::NewMessage(m) => {
            if let Some(ref kb_json) = kb_json_opt {
                let _ = state
                    .db
                    .save_message_extras(chat_id, m.id, Some(kb_json), None);
            }
            Some(m.clone())
        }
        _ => None,
    };
    broadcast_chat_pts(state, chat_id, pts_update);
    saved_msg
}

fn split_spoken_segments(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '\n') {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                segments.push(trimmed);
            }
            current.clear();
        }
    }
    let tail = current.trim().to_string();
    if !tail.is_empty() {
        segments.push(tail);
    }
    if segments.is_empty() && !text.trim().is_empty() {
        segments.push(text.trim().to_string());
    }
    segments
}

fn sse_response_from_chunks(chunks: Vec<String>) -> Response {
    let byte_stream = stream::iter(
        chunks
            .into_iter()
            .map(|c| Ok::<axum::body::Bytes, Infallible>(axum::body::Bytes::from(c))),
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(byte_stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ============================================================================
// Health, Turns, Events & Slash Commands
// ============================================================================

fn collect_available_models(
    snap: &gpui_telegram_agent::hermes::providers::ProviderRouterState,
) -> Vec<String> {
    let mut models = Vec::new();
    let mut push_unique = |m: &str| {
        let trimmed = m.trim();
        if !trimmed.is_empty() && !models.iter().any(|existing| existing == trimmed) {
            models.push(trimmed.to_string());
        }
    };
    push_unique(&snap.active_model);
    if let Some(active_cfg) = snap.providers.get(&snap.active_provider_id) {
        push_unique(&active_cfg.default_model);
        for m in &active_cfg.fallback_models {
            push_unique(m);
        }
    }
    let mut sorted_providers: Vec<_> = snap.providers.values().collect();
    sorted_providers.sort_by(|a, b| a.id.cmp(&b.id));
    for cfg in sorted_providers {
        if !cfg.enabled {
            continue;
        }
        push_unique(&cfg.default_model);
        for m in &cfg.fallback_models {
            push_unique(m);
        }
    }
    let mut profile_ids: Vec<_> = snap.model_profiles.values().collect();
    profile_ids.sort_by(|a, b| a.id.cmp(&b.id));
    for prof in profile_ids {
        push_unique(&prof.model);
    }
    models
}

pub async fn hermes_status(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
) -> Response {
    let engine = resolve_hermes_for_bot(&state, q.bot_id);
    let (active_provider, active_model, api_mode) = engine.providers().active_info();
    let snap = engine.providers().snapshot();
    let available_models = collect_available_models(&snap);
    let mut provider_list: Vec<_> = snap.providers.values().collect();
    provider_list.sort_by(|a, b| a.id.cmp(&b.id));
    let providers: Vec<Value> = provider_list
        .into_iter()
        .map(|cfg| provider_to_dto(cfg, &snap.active_provider_id, &snap.model_profiles))
        .collect();
    let (toolsets_count, tools_count) = {
        let reg = engine
            .toolsets()
            .read()
            .unwrap_or_else(|e| e.into_inner());
        (reg.enabled_toolsets.len(), reg.active_specs().len())
    };
    let skills_count = engine.skills().skills_list(None).len();
    let cron_count = engine
        .store()
        .list_cron_jobs(false)
        .map(|v| v.len())
        .unwrap_or(0);
    let sessions_count = engine
        .store()
        .list_sessions(SessionListFilter::default())
        .map(|v| v.len())
        .unwrap_or(0);
    let pending_db = engine
        .store()
        .list_approvals(None, true)
        .map(|v| v.len())
        .unwrap_or(0);
    let pending_mem = engine
        .security()
        .read()
        .map(|s| s.approvals.pending.len())
        .unwrap_or(0);
    let heartbeat_enabled = engine.heartbeat().status().enabled;

    (
        StatusCode::OK,
        Json(json!({
            "healthy": true,
            "engine": "hermes-rust",
            "scoped_bot_id": q.bot_id,
            "active_model": active_model,
            "active_provider": active_provider,
            "api_mode": api_mode.as_str(),
            "allow_local_fallback": snap.allow_local_fallback,
            "has_active_subscription": engine.providers().has_active_subscription(),
            "has_configured_provider": engine.providers().has_configured_provider(),
            "needs_provider_selection": engine.providers().needs_provider_selection(),
            "available_models": available_models,
            "providers": providers,
            "toolsets_count": toolsets_count,
            "tools_count": tools_count,
            "skills_count": skills_count,
            "cron_count": cron_count,
            "sessions_count": sessions_count,
            "pending_approvals_count": pending_db.max(pending_mem),
            "heartbeat_enabled": heartbeat_enabled,
        })),
    )
        .into_response()
}

pub async fn hermes_turns_create(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let prompt = payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if prompt.trim().is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_prompt", "prompt is required");
    }

    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64());
    if let Some(cid) = chat_id {
        if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
            return resp;
        }
    }

    let mut session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let bot_id = parse_optional_bot_id(payload.get("bot_id"));
    let mut model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let provider = payload
        .get("provider")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let (Some(prov), Some(m)) = (provider, &model) {
        if !m.contains('/') {
            model = Some(format!("{prov}/{m}"));
        }
    }
    let mut system_prompt = payload
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let mut bot_name: Option<String> = payload
        .get("bot_name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let explicit_model = sanitize_model_override(&state, model.clone());
    let bot_id_u64 = bot_id
        .as_deref()
        .and_then(|b| b.trim().trim_start_matches("bot_").parse::<u64>().ok());
    let engine = resolve_hermes_for_bot(&state, bot_id_u64);
    let sess_engine_opt = session_id
        .as_deref()
        .map(|sid| resolve_hermes_for_session(&state, bot_id_u64, sid));
    let effective_bot_id = sess_engine_opt
        .as_ref()
        .and_then(|se| resolve_effective_bot_id(&state, bot_id_u64, se))
        .or_else(|| resolve_effective_bot_id(&state, bot_id_u64, &engine));
    let owns_bot = effective_bot_id
        .map(|bid| caller_owns_bot_or_admin(&state, auth.user_id, bid).is_ok())
        .unwrap_or(true);
    if let Some(bid) = effective_bot_id {
        if !owns_bot {
            if gpui_telegram_agent::hermes::commands::is_bot_config_command(&prompt) {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Only bot owner or server admin can run bot config/memory/soul/skill commands",
                );
            }
            if let Some(cid) = chat_id {
                if !is_bot_member_of_chat(&state, bid, ChatId(cid))
                    && !is_official_bot(&state, bid)
                {
                    return err_resp(
                        StatusCode::FORBIDDEN,
                        "forbidden",
                        "Target bot is not a member of the specified chat",
                    );
                }
            } else if !is_official_bot(&state, bid)
                && !(!bot_has_human_owner(&state, bid)
                    && state
                        .db
                        .get_bot_computer_profile(auth.user_id.0, bid)
                        .ok()
                        .flatten()
                        .is_some())
            {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Only bot owner or server admin can invoke this bot outside a shared chat",
                );
            }
        }
    }
    let mut bot_fallback_model: Option<String> = None;
    if let Some(bid_num) = bot_id_u64 {
        let (bot_sp, bot_model, _) = load_bot_persona_config(&state, UserId(bid_num));
        if bot_name.is_none() {
            bot_name = resolve_bot_display_name(&state, UserId(bid_num));
        }
        if system_prompt.is_none() {
            let user_computer_sp = if !owns_bot
                && !bot_has_human_owner(&state, bid_num)
                && !is_official_bot(&state, bid_num)
            {
                state
                    .db
                    .get_bot_computer_profile(auth.user_id.0, bid_num)
                    .ok()
                    .flatten()
                    .map(|p| p.persona_prompt)
                    .filter(|s| !s.trim().is_empty())
            } else {
                None
            };
            system_prompt = user_computer_sp.or(bot_sp);
        }
        bot_fallback_model = sanitize_model_override(&state, bot_model);
    }
    let (enriched_sp, effective_model) = build_enriched_bot_turn_context(
        &state,
        auth.user_id.0,
        bot_id_u64,
        bot_name.as_deref(),
        system_prompt,
        explicit_model,
        bot_fallback_model,
        &[],
        false,
    );
    system_prompt = enriched_sp;
    model = effective_model;

    let yolo_mode = payload.get("yolo_mode").and_then(|v| v.as_bool()).unwrap_or(false);
    if session_id.is_none() && chat_id.is_none() {
        session_id = Some(match bot_id_u64 {
            Some(bid) => format!("user_{}_bot_{bid}", auth.user_id.0),
            None => format!("user_{}_default", auth.user_id.0),
        });
    }
    if let Some(ref sid) = session_id {
        let sess_engine = resolve_hermes_for_session(&state, bot_id_u64, sid);
        if let Ok(Some(existing)) = sess_engine.store().get_session(sid) {
            if chat_id.is_some() && existing.chat_id.is_some() && existing.chat_id != chat_id {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Target session belongs to a different chat",
                );
            }
            if let Some(cid) = existing.chat_id {
                if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
                    return resp;
                }
            }
            let owns_session = caller_owns_hermes_session(&state, &auth, &existing, bot_id_u64);
            if !owns_session {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Cannot run turn on a session not owned by caller",
                );
            }
        }
    }
    if let Some(bid_num) = bot_id_u64 {
        if let Ok((sid, _)) = resolve_and_apply_bot_session_policy(
            &state,
            auth.user_id.0,
            UserId(bid_num),
            chat_id.map(ChatId),
            session_id.as_deref(),
            "api",
            model.as_deref(),
            yolo_mode,
        ) {
            if session_id.is_none() {
                session_id = Some(sid);
            }
        }
    } else if yolo_mode {
        if let Some(ref sid) = session_id {
            if let Ok(mut sec) = engine.security().write() {
                sec.approvals
                    .set_session_mode(sid.clone(), ApprovalMode::Yolo);
            }
        }
    }

    let caller_scope = build_caller_scope(&state, auth.user_id);
    let req = HermesTurnRequest {
        session_id,
        chat_id,
        bot_id,
        bot_name,
        user_id: Some(auth.user_id.0.to_string()),
        source: "api".to_string(),
        prompt,
        model,
        system_prompt,
        thinking_budget_tokens: payload
            .get("thinking_budget_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize),
        preset: payload
            .get("preset")
            .and_then(|v| v.as_str())
            .and_then(ToolsetPreset::from_str)
            .or(Some(ToolsetPreset::HermesTelegram)),
        max_iterations: payload
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize),
        max_tokens: payload
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32),
        temperature: payload
            .get("temperature")
            .and_then(|v| v.as_f64())
            .map(|n| n as f32),
        skip_slash_commands: payload
            .get("skip_slash_commands")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        caller_depth: 0,
        caller_allowed_chat_ids: Some(caller_scope.allowed_chat_ids),
        caller_is_admin: caller_scope.is_admin,
        caller_owns_bot: Some(owns_bot),
    };

    match engine.run_turn(req).await {
        Ok(res) => {
            if payload
                .get("deliver_to_chat")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                if let Some(cid) = chat_id {
                    let buttons = build_default_bot_quick_actions(&res, &res.response_text);
                    let kb_opt = if buttons.is_empty() { None } else { Some(buttons) };
                    let _ = deliver_hermes_proactive_message(
                        &state,
                        ChatId(cid),
                        None,
                        &res.response_text,
                        kb_opt,
                    )
                    .await;
                }
            }
            (StatusCode::OK, Json(turn_result_to_json(&res))).into_response()
        }
        Err(e) => {
            if engine.providers().needs_provider_selection() {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "provider_not_configured",
                        "message": e.to_string(),
                        "needs_provider_selection": true,
                        "action": "hermes:open_ai_providers",
                    })),
                )
                    .into_response();
            }
            map_telegram_err(e)
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct EventsQuery {
    pub stream: Option<bool>,
    pub sse: Option<bool>,
    pub bot_id: Option<u64>,
}

pub async fn hermes_turn_events(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(turn_id): Path<String>,
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
) -> Response {
    let mut matched_engine = resolve_hermes_for_bot(&state, query.bot_id);
    let mut events = matched_engine.get_turn_events(&turn_id);
    let mut turn_meta = matched_engine.get_turn_session_owner(&turn_id);
    if events.is_empty() && turn_meta.is_none() && query.bot_id.is_none() {
        for inst in state.fabushi_bots.list_instances() {
            let eng = inst.hermes();
            let bot_events = eng.get_turn_events(&turn_id);
            let bot_meta = eng.get_turn_session_owner(&turn_id);
            if !bot_events.is_empty() || bot_meta.is_some() {
                events = bot_events;
                turn_meta = bot_meta;
                matched_engine = eng;
                break;
            }
        }
    }

    if !events.is_empty() || turn_meta.is_some() {
        let fallback_sid = events.iter().find_map(|ev| match ev {
            HermesEvent::Meta { session_id, .. } | HermesEvent::Done { session_id, .. } => {
                Some(session_id.clone())
            }
            _ => None,
        });
        let (sid_opt, owner_opt) = match turn_meta {
            Some((sid, owner)) => (Some(sid), owner),
            None => (fallback_sid, None),
        };
        if let Some(ref sid) = sid_opt {
            if let Ok(Some(sess)) = matched_engine.store().get_session(sid) {
                if let Err(resp) = authorize_hermes_session_access(&state, &auth, &sess, query.bot_id) {
                    return resp;
                }
            } else if !caller_is_admin(&state, auth.user_id)
                && owner_opt.as_deref() != Some(auth.user_id.0.to_string().as_str())
            {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Caller is not authorized to access this turn's events",
                );
            }
        } else if !caller_is_admin(&state, auth.user_id)
            && owner_opt.as_deref() != Some(auth.user_id.0.to_string().as_str())
        {
            return err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Caller is not authorized to access this turn's events",
            );
        }
    }
    let wants_sse = query.stream == Some(true)
        || query.sse == Some(true)
        || headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/event-stream"));

    if wants_sse {
        let has_error = events
            .iter()
            .any(|ev| matches!(ev, HermesEvent::Error { .. }));
        let has_done = events
            .iter()
            .any(|ev| matches!(ev, HermesEvent::Done { .. }));
        let mut chunks: Vec<String> = events
            .iter()
            .filter_map(|ev| serde_json::to_string(ev).ok())
            .map(|line| format!("event: hermes\ndata: {line}\n\n"))
            .collect();
        if !has_error && has_done {
            chunks.push("data: [DONE]\n\n".to_string());
        }
        return sse_response_from_chunks(chunks);
    }

    (
        StatusCode::OK,
        Json(json!({
            "turn_id": turn_id,
            "events": events,
            "items": events,
        })),
    )
        .into_response()
}

pub async fn hermes_command_run(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
    Json(payload): Json<Value>,
) -> Response {
    let command = payload
        .get("command")
        .or_else(|| payload.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if command.is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_command", "command is required");
    }

    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64());
    if let Some(cid) = chat_id {
        if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
            return resp;
        }
    }

    let session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let bot_id_u64 = q.bot_id.or_else(|| parse_bot_id_u64(payload.get("bot_id")));
    let bot_id = parse_optional_bot_id(payload.get("bot_id"))
        .or_else(|| bot_id_u64.map(|n| n.to_string()));
    let target_engine = if let Some(sid) = session_id {
        let sess_engine = resolve_hermes_for_session(&state, bot_id_u64, sid);
        if let Ok(Some(existing)) = sess_engine.store().get_session(sid) {
            if chat_id.is_some() && existing.chat_id != chat_id {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Target session belongs to a different chat",
                );
            }
            if let Err(resp) = authorize_hermes_session_access(&state, &auth, &existing, bot_id_u64) {
                return resp;
            }
        }
        sess_engine
    } else {
        resolve_hermes_for_bot(&state, bot_id_u64)
    };

    let effective_bot_id = resolve_effective_bot_id(&state, bot_id_u64, &target_engine);
    let owns_bot = effective_bot_id
        .map(|bid| caller_owns_bot_or_admin(&state, auth.user_id, bid).is_ok())
        .unwrap_or(true);

    if let Some(bid) = effective_bot_id {
        if !owns_bot {
            if gpui_telegram_agent::hermes::commands::is_bot_config_command(&command) {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Only bot owner or server admin can run bot config/memory/soul/skill commands",
                );
            }
            if let Some(cid) = chat_id {
                if !is_bot_member_of_chat(&state, bid, ChatId(cid))
                    && !is_official_bot(&state, bid)
                {
                    return err_resp(
                        StatusCode::FORBIDDEN,
                        "forbidden",
                        "Target bot is not a member of the specified chat",
                    );
                }
            } else if !is_official_bot(&state, bid) {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Only bot owner or server admin can access or modify this bot resource",
                );
            }
        }
    }

    let effective_sid = if let Some(sid) = session_id {
        Some(sid.to_string())
    } else if chat_id.is_none() {
        Some(match bot_id_u64 {
            Some(bid) => format!("user_{}_bot_{bid}", auth.user_id.0),
            None => format!("user_{}_default", auth.user_id.0),
        })
    } else {
        None
    };

    let mut caller_scope = build_caller_scope(&state, auth.user_id);
    caller_scope.bot_config_allowed = owns_bot;
    match target_engine.execute_command_scoped(
        effective_sid.as_deref(),
        chat_id,
        bot_id.as_deref(),
        Some(caller_scope),
        &command,
    ) {
        Ok((resolved_sid, outcome)) => (
            StatusCode::OK,
            Json(json!({
                "handled": outcome.handled,
                "command": outcome.command,
                "session_id": resolved_sid,
                "output": outcome.response_text,
                "reply_text": outcome.response_text,
                "inline_keyboard": map_buttons_json(&outcome.buttons),
                "effects": [outcome.ui_effect],
                "data": {
                    "is_error": outcome.is_error,
                    "mutated_fields": outcome.mutated_fields,
                    "should_continue_to_llm": outcome.should_continue_to_llm,
                    "rewritten_prompt": outcome.rewritten_prompt,
                },
            })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct CommandsQuery {
    pub prefix: Option<String>,
}

pub async fn hermes_commands_list(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<CommandsQuery>,
) -> Response {
    let list = match query.prefix.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(prefix) => state.hermes.autocomplete_commands(prefix),
        None => state.hermes.list_commands(),
    };
    let commands: Vec<Value> = list
        .into_iter()
        .map(|d| {
            json!({
                "name": d.name,
                "slash_name": d.slash_name,
                "aliases": d.aliases,
                "syntax": d.syntax,
                "category": d.category.label(),
                "description": d.description,
                "accepts_args": d.accepts_args,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "commands": commands,
            "total": commands.len(),
        })),
    )
        .into_response()
}

// ============================================================================
// Sessions
// ============================================================================

#[derive(Debug, Deserialize, Default)]
pub struct SessionsListQuery {
    pub chat_id: Option<i64>,
    pub bot_id: Option<String>,
    pub source: Option<String>,
    pub active_only: Option<bool>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn hermes_sessions_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<SessionsListQuery>,
) -> Response {
    if let Some(cid) = q.chat_id {
        if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
            return resp;
        }
    }
    let bot_id_u64 = q
        .bot_id
        .as_deref()
        .and_then(|s| s.trim().parse::<u64>().ok());
    let engine = resolve_hermes_for_bot(&state, bot_id_u64);
    let filter_bot_id = if bot_id_u64.is_some() && !Arc::ptr_eq(&engine, &state.hermes) {
        None
    } else {
        q.bot_id.filter(|s| !s.trim().is_empty())
    };
    let limit = q.limit.unwrap_or(100);
    if bot_id_u64.is_some() {
        let filter = SessionListFilter {
            chat_id: q.chat_id,
            bot_id: filter_bot_id,
            source: q.source.filter(|s| !s.trim().is_empty()),
            active_only: q.active_only.unwrap_or(false),
            limit: None,
            offset: None,
        };
        return match engine.store().list_sessions(filter) {
            Ok(list) => {
                let off = q.offset.unwrap_or(0);
                let sessions: Vec<Value> = list
                    .iter()
                    .filter(|s| authorize_hermes_session_access(&state, &auth, s, bot_id_u64).is_ok())
                    .skip(off)
                    .take(limit)
                    .map(session_to_dto)
                    .collect();
                (
                    StatusCode::OK,
                    Json(json!({ "sessions": sessions, "total": sessions.len() })),
                )
                    .into_response()
            }
            Err(e) => map_telegram_err(e),
        };
    }

    let source_opt = q.source.filter(|s| !s.trim().is_empty());
    let active_only = q.active_only.unwrap_or(false);
    let make_filter = || SessionListFilter {
        chat_id: q.chat_id,
        bot_id: filter_bot_id.clone(),
        source: source_opt.clone(),
        active_only,
        limit: None,
        offset: None,
    };
    let mut combined = match state.hermes.store().list_sessions(make_filter()) {
        Ok(list) => list,
        Err(e) => return map_telegram_err(e),
    };
    let mut seen: std::collections::HashSet<String> =
        combined.iter().map(|s| s.id.clone()).collect();
    for inst in state.fabushi_bots.list_instances() {
        if let Ok(list) = inst.hermes().store().list_sessions(make_filter()) {
            for s in list {
                if seen.insert(s.id.clone()) {
                    combined.push(s);
                }
            }
        }
    }
    combined.retain(|s| authorize_hermes_session_access(&state, &auth, s, None).is_ok());
    combined.sort_by(|a, b| {
        b.ended_at
            .unwrap_or(b.started_at)
            .cmp(&a.ended_at.unwrap_or(a.started_at))
    });
    let off = q.offset.unwrap_or(0);
    let paged: Vec<Value> = combined
        .iter()
        .skip(off)
        .take(limit)
        .map(session_to_dto)
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "sessions": paged, "total": paged.len() })),
    )
        .into_response()
}

pub async fn hermes_session_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(q): Query<OptionalBotScopeQuery>,
) -> Response {
    let engine = resolve_hermes_for_session(&state, q.bot_id, &session_id);
    let sess = match engine.store().get_session(&session_id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "session_not_found",
                "Hermes session not found",
            )
        }
        Err(e) => return map_telegram_err(e),
    };
    if let Err(resp) = authorize_hermes_session_access(&state, &auth, &sess, q.bot_id) {
        return resp;
    }
    let messages = engine
        .store()
        .get_messages(&session_id, Some(200))
        .unwrap_or_default();
    let branches = engine
        .store()
        .list_branches(&session_id)
        .unwrap_or_default()
        .iter()
        .map(session_to_dto)
        .collect::<Vec<_>>();

    (
        StatusCode::OK,
        Json(json!({
            "session": session_to_dto(&sess),
            "messages": messages,
            "branches": branches,
        })),
    )
        .into_response()
}

pub async fn hermes_sessions_search(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
    Json(payload): Json<Value>,
) -> Response {
    let query = payload
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let limit = payload
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(25) as usize;
    let bot_id_u64 = q.bot_id.or_else(|| parse_bot_id_u64(payload.get("bot_id")));
    if let Some(sid) = session_id {
        let target_engine = resolve_hermes_for_session(&state, bot_id_u64, sid);
        if let Ok(Some(sess)) = target_engine.store().get_session(sid) {
            if let Err(resp) = authorize_hermes_session_access(&state, &auth, &sess, bot_id_u64) {
                return resp;
            }
        }
    }
    let filter_authorized_hits = |engine: &HermesEngine, hits: Vec<gpui_telegram_agent::hermes::state_db::SessionSearchHit>| -> Vec<gpui_telegram_agent::hermes::state_db::SessionSearchHit> {
        let mut cache: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        hits.into_iter()
            .filter(|hit| {
                *cache.entry(hit.session_id.clone()).or_insert_with(|| {
                    match engine.store().get_session(&hit.session_id) {
                        Ok(Some(sess)) => {
                            authorize_hermes_session_access(&state, &auth, &sess, bot_id_u64).is_ok()
                        }
                        _ => false,
                    }
                })
            })
            .collect()
    };
    if bot_id_u64.is_some() {
        let engine = resolve_hermes_for_bot(&state, bot_id_u64);
        return match engine.store().session_search(&query, session_id, limit.saturating_mul(4).max(100)) {
            Ok(hits) => {
                let mut filtered = filter_authorized_hits(&engine, hits);
                filtered.truncate(limit);
                (
                    StatusCode::OK,
                    Json(json!({ "results": filtered, "hits": filtered })),
                )
                    .into_response()
            }
            Err(e) => map_telegram_err(e),
        };
    }

    let search_limit = limit.saturating_mul(4).max(100);
    let mut all_hits = match state.hermes.store().session_search(&query, session_id, search_limit) {
        Ok(hits) => filter_authorized_hits(&state.hermes, hits),
        Err(e) => return map_telegram_err(e),
    };
    for inst in state.fabushi_bots.list_instances() {
        let bot_engine = inst.hermes();
        if let Ok(hits) = bot_engine.store().session_search(&query, session_id, search_limit) {
            all_hits.extend(filter_authorized_hits(&bot_engine, hits));
        }
    }
    all_hits.truncate(limit);
    (
        StatusCode::OK,
        Json(json!({ "results": all_hits, "hits": all_hits })),
    )
        .into_response()
}

pub async fn hermes_session_branch(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(q): Query<OptionalBotScopeQuery>,
    Json(payload): Json<Value>,
) -> Response {
    let branch_name = payload
        .get("branch_name")
        .or_else(|| payload.get("title"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let up_to_seq = payload.get("up_to_seq").and_then(|v| v.as_i64());
    let bot_id_u64 = q.bot_id.or_else(|| parse_bot_id_u64(payload.get("bot_id")));
    let engine = resolve_hermes_for_session(&state, bot_id_u64, &session_id);
    match engine.store().get_session(&session_id) {
        Ok(Some(sess)) => {
            if let Err(resp) = authorize_hermes_session_access(&state, &auth, &sess, bot_id_u64) {
                return resp;
            }
        }
        Ok(None) => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "session_not_found",
                "Hermes session not found",
            )
        }
        Err(e) => return map_telegram_err(e),
    }
    match engine
        .store()
        .branch_session(&session_id, up_to_seq, branch_name)
    {
        Ok(branched) => (StatusCode::CREATED, Json(session_to_dto(&branched))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_session_compress(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(q): Query<OptionalBotScopeQuery>,
    payload: Option<Json<Value>>,
) -> Response {
    let focus = payload
        .as_ref()
        .and_then(|Json(v)| v.get("focus").and_then(|s| s.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let payload_bot_id = payload
        .as_ref()
        .and_then(|Json(v)| parse_bot_id_u64(v.get("bot_id")));
    let bot_id_u64 = q.bot_id.or(payload_bot_id);
    let engine = resolve_hermes_for_session(&state, bot_id_u64, &session_id);
    match engine.store().get_session(&session_id) {
        Ok(Some(sess)) => {
            if let Err(resp) = authorize_hermes_session_access(&state, &auth, &sess, bot_id_u64) {
                return resp;
            }
        }
        Ok(None) => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "session_not_found",
                "Hermes session not found",
            )
        }
        Err(e) => return map_telegram_err(e),
    }
    let cmd = match focus {
        Some(f) => format!("/compact {f}"),
        None => "/compact".to_string(),
    };
    match engine.execute_command(Some(&session_id), None, None, &cmd) {
        Ok((sid, outcome)) => (
            StatusCode::OK,
            Json(json!({
                "session_id": sid,
                "compressed": !outcome.is_error,
                "summary": outcome.response_text,
                "mutated_fields": outcome.mutated_fields,
            })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_session_undo(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(q): Query<OptionalBotScopeQuery>,
) -> Response {
    let engine = resolve_hermes_for_session(&state, q.bot_id, &session_id);
    match engine.store().get_session(&session_id) {
        Ok(Some(sess)) => {
            if let Err(resp) = authorize_hermes_session_access(&state, &auth, &sess, q.bot_id) {
                return resp;
            }
        }
        Ok(None) => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "session_not_found",
                "Hermes session not found",
            )
        }
        Err(e) => return map_telegram_err(e),
    }
    match engine.store().undo_last_turn(&session_id) {
        Ok(res) => (StatusCode::OK, Json(json!(res))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_session_export(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(q): Query<OptionalBotScopeQuery>,
    payload: Option<Json<Value>>,
) -> Response {
    let val = payload.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let bot_id_u64 = q.bot_id.or_else(|| parse_bot_id_u64(val.get("bot_id")));
    let engine = resolve_hermes_for_session(&state, bot_id_u64, &session_id);
    match engine.store().get_session(&session_id) {
        Ok(Some(sess)) => {
            if let Err(resp) = authorize_hermes_session_access(&state, &auth, &sess, bot_id_u64) {
                return resp;
            }
        }
        Ok(None) => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "session_not_found",
                "Hermes session not found",
            )
        }
        Err(e) => return map_telegram_err(e),
    }
    let raw_fmt = val
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("markdown")
        .trim()
        .to_ascii_lowercase();
    let redact = val.get("redact").and_then(|v| v.as_bool()).unwrap_or(true);

    if raw_fmt == "markdown" || raw_fmt == "md" {
        let sess = match engine.store().get_session(&session_id) {
            Ok(Some(s)) => s,
            Ok(None) => {
                return err_resp(
                    StatusCode::NOT_FOUND,
                    "session_not_found",
                    "Hermes session not found",
                )
            }
            Err(e) => return map_telegram_err(e),
        };
        let msgs = engine
            .store()
            .get_messages(&session_id, None)
            .unwrap_or_default();
        let mut md = format!(
            "# Hermes Session {}\n\n- Model: `{}`\n- Messages: {}\n\n",
            sess.title.as_deref().unwrap_or(&sess.id),
            sess.model,
            msgs.len()
        );
        for m in msgs {
            md.push_str(&format!("### {} (#{}) \n\n{}\n\n", m.role, m.seq, m.content));
        }
        return (
            StatusCode::OK,
            Json(json!({
                "session_id": sess.id,
                "format": "markdown",
                "content": md,
                "exported_at_ms": Utc::now().timestamp_millis(),
            })),
        )
            .into_response();
    }

    let fmt = TrajectoryFormat::parse(&raw_fmt).unwrap_or(TrajectoryFormat::HermesNative);
    match engine.export_session(&session_id, fmt, redact, None) {
        Ok(exported) => (
            StatusCode::OK,
            Json(json!({
                "session_id": exported.session_id,
                "format": raw_fmt,
                "content": exported.payload,
                "message_count": exported.message_count,
                "redaction_report": exported.redaction_report,
                "exported_at_ms": Utc::now().timestamp_millis(),
            })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

// ============================================================================
// Memory, SOUL & Honcho
// ============================================================================

pub async fn hermes_memory_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
) -> Response {
    if let Some(bid) = q.bot_id {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, q.bot_id);
    match build_memory_state_json(&engine) {
        Ok(val) => (StatusCode::OK, Json(val)).into_response(),
        Err(resp) => resp,
    }
}

pub async fn hermes_memory_put(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
    Json(payload): Json<Value>,
) -> Response {
    let bot_id_u64 = q.bot_id.or_else(|| parse_bot_id_u64(payload.get("bot_id")));
    if let Some(bid) = bot_id_u64 {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, bot_id_u64);
    if let Some(mem_md) = payload.get("memory_markdown").and_then(|v| v.as_str()) {
        if let Err(e) = engine.memory().set_raw(MemoryTarget::Memory, mem_md) {
            return map_telegram_err(e);
        }
    }
    if let Some(user_md) = payload.get("user_markdown").and_then(|v| v.as_str()) {
        if let Err(e) = engine.memory().set_raw(MemoryTarget::User, user_md) {
            return map_telegram_err(e);
        }
    }
    match build_memory_state_json(&engine) {
        Ok(val) => (StatusCode::OK, Json(val)).into_response(),
        Err(resp) => resp,
    }
}

pub async fn hermes_memory_action(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
    Json(payload): Json<Value>,
) -> Response {
    let bot_id_u64 = q.bot_id.or_else(|| parse_bot_id_u64(payload.get("bot_id")));
    if let Some(bid) = bot_id_u64 {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, bot_id_u64);
    let action = payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("add")
        .trim()
        .to_ascii_lowercase();
    let scope_raw = payload
        .get("scope")
        .or_else(|| payload.get("target"))
        .and_then(|v| v.as_str())
        .unwrap_or("memory");
    let target = MemoryTarget::parse(scope_raw).unwrap_or(MemoryTarget::Memory);
    let content = payload
        .get("content")
        .or_else(|| payload.get("entry"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let old_text = payload
        .get("old_text")
        .or_else(|| payload.get("key"))
        .and_then(|v| v.as_str())
        .unwrap_or(content);

    let res = match action.as_str() {
        "add" => engine.memory().add(target, content),
        "replace" | "update" => engine.memory().replace(target, old_text, content),
        "remove" | "delete" => engine.memory().remove(target, old_text),
        "clear" => engine.memory().clear(target),
        "set_raw" | "set" => engine.memory().set_raw(target, content),
        other => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_memory_action",
                &format!("Unsupported memory action '{other}'"),
            );
        }
    };
    if let Err(e) = res {
        return map_telegram_err(e);
    }
    match build_memory_state_json(&engine) {
        Ok(val) => (StatusCode::OK, Json(val)).into_response(),
        Err(resp) => resp,
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct SoulQuery {
    pub bot_id: Option<u64>,
    pub chat_id: Option<i64>,
}

pub async fn hermes_soul_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SoulQuery>,
) -> Response {
    if let Some(cid) = query.chat_id {
        if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
            return resp;
        }
    }
    if let Some(bot_id) = query.bot_id {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bot_id) {
            return resp;
        }
        let engine = resolve_hermes_for_bot(&state, Some(bot_id));
        if let Ok(Some(raw)) = state
            .hermes
            .store()
            .kv_get("bot_soul", &bot_id.to_string())
        {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                return (StatusCode::OK, Json(val)).into_response();
            }
        }
        if let Ok(Some(raw)) = state
            .hermes
            .store()
            .kv_get("bot_persona", &bot_id.to_string())
        {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                let persona_name = val
                    .get("persona_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Hermes Bot")
                    .to_string();
                let sp = val
                    .get("system_prompt")
                    .or_else(|| val.get("persona"))
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
                return (
                    StatusCode::OK,
                    Json(json!({
                        "bot_id": bot_id,
                        "chat_id": query.chat_id.or_else(|| val.get("chat_id").and_then(|v| v.as_i64())),
                        "persona_name": persona_name,
                        "content": sp.clone().unwrap_or_default(),
                        "system_prompt": sp,
                        "welcome_message": val.get("welcome_message").cloned().unwrap_or(Value::Null),
                        "model": val.get("model").cloned().unwrap_or(Value::Null),
                        "updated_at_ms": val.get("updated_at_ms").and_then(|v| v.as_i64()).unwrap_or_else(|| Utc::now().timestamp_millis()),
                    })),
                )
                    .into_response();
            }
        }
        if !Arc::ptr_eq(&engine, &state.hermes) && engine.memory().has_soul() {
            if let Ok(profile) = engine.memory().read_soul() {
                let preferred_model = state
                    .fabushi_bots
                    .get_by_user_id(bot_id as i64)
                    .map(|inst| inst.settings().preferred_model)
                    .filter(|m| !m.trim().is_empty());
                return (
                    StatusCode::OK,
                    Json(json!({
                        "bot_id": bot_id,
                        "chat_id": query.chat_id,
                        "persona_name": profile.name,
                        "content": profile.raw_markdown,
                        "system_prompt": profile.persona,
                        "welcome_message": Value::Null,
                        "model": preferred_model,
                        "updated_at_ms": Utc::now().timestamp_millis(),
                    })),
                )
                    .into_response();
            }
        }
        if let Ok(Some(u)) = state.db.get_user(UserId(bot_id)) {
            let content = u.bio.clone().unwrap_or_default();
            return (
                StatusCode::OK,
                Json(json!({
                    "bot_id": bot_id,
                    "chat_id": query.chat_id,
                    "persona_name": u.first_name,
                    "content": content,
                    "system_prompt": u.bio,
                    "welcome_message": Value::Null,
                    "model": Value::Null,
                    "updated_at_ms": u.updated_at_ms,
                })),
            )
                .into_response();
        }
    }

    match state.hermes.memory().read_soul() {
        Ok(profile) => (
            StatusCode::OK,
            Json(json!({
                "bot_id": query.bot_id,
                "chat_id": query.chat_id,
                "persona_name": profile.name,
                "content": profile.raw_markdown,
                "system_prompt": profile.persona,
                "welcome_message": Value::Null,
                "model": Value::Null,
                "updated_at_ms": Utc::now().timestamp_millis(),
            })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_soul_put(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let bot_id = parse_bot_id_u64(payload.get("bot_id"));
    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64());
    if let Some(bid) = bot_id {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    if let Some(cid) = chat_id {
        if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
            return resp;
        }
    }
    let persona_name = payload
        .get("persona_name")
        .or_else(|| payload.get("name"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Hermes")
        .to_string();
    let content = payload
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let system_prompt = payload
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            if !content.trim().is_empty() {
                Some(content.clone())
            } else {
                None
            }
        });
    let welcome_message = payload
        .get("welcome_message")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let now_ms = Utc::now().timestamp_millis();

    if let Some(bid) = bot_id {
        let engine = resolve_hermes_for_bot(&state, Some(bid));
        if !Arc::ptr_eq(&engine, &state.hermes) {
            if !content.trim().is_empty() {
                let _ = engine.memory().write_soul_raw(&content);
            } else {
                let mut p = SoulProfile::new(
                    &persona_name,
                    "Personal AI Assistant",
                    system_prompt.as_deref().unwrap_or(""),
                );
                p.raw_markdown = p.to_markdown();
                let _ = engine.memory().write_soul(&p);
            }
            let _ = state.fabushi_bots.update_bot(
                bid as i64,
                FabushiBotPatch {
                    display_name: Some(persona_name.clone()),
                    description: system_prompt.clone(),
                    preferred_model: model.clone(),
                    ..Default::default()
                },
            );
            let _ = state.db.update_bot_fabushi_fields(
                bid as i64,
                None,
                None,
                None,
                system_prompt.as_deref(),
                None,
                None,
                None,
                model.as_deref(),
                None,
                None,
                None,
            );
        }
        let dto = json!({
            "bot_id": bid,
            "chat_id": chat_id,
            "persona_name": persona_name,
            "content": if content.trim().is_empty() { system_prompt.clone().unwrap_or_default() } else { content },
            "system_prompt": system_prompt,
            "welcome_message": welcome_message,
            "model": model,
            "updated_at_ms": now_ms,
        });
        if let Ok(raw) = serde_json::to_string(&dto) {
            if let Err(e) = state.hermes.store().kv_set("bot_soul", &bid.to_string(), &raw) {
                return map_telegram_err(e);
            }
            let _ = state
                .hermes
                .store()
                .kv_set("bot_persona", &bid.to_string(), &raw);
        }
        return (StatusCode::OK, Json(dto)).into_response();
    }

    let profile = if !content.trim().is_empty() {
        match state.hermes.memory().write_soul_raw(&content) {
            Ok(p) => p,
            Err(e) => return map_telegram_err(e),
        }
    } else {
        let mut p = SoulProfile::new(
            &persona_name,
            "Personal AI Assistant",
            system_prompt.as_deref().unwrap_or(""),
        );
        p.raw_markdown = p.to_markdown();
        if let Err(e) = state.hermes.memory().write_soul(&p) {
            return map_telegram_err(e);
        }
        p
    };

    (
        StatusCode::OK,
        Json(json!({
            "bot_id": Value::Null,
            "chat_id": chat_id,
            "persona_name": profile.name,
            "content": profile.raw_markdown,
            "system_prompt": system_prompt.unwrap_or(profile.persona),
            "welcome_message": welcome_message,
            "model": model,
            "updated_at_ms": now_ms,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct HonchoQuery {
    pub user_id: Option<String>,
    pub peer_name: Option<String>,
    pub bot_id: Option<u64>,
}

fn authorize_honcho_peer(
    state: &AppState,
    auth: &AuthBearer,
    peer: &str,
) -> std::result::Result<(), Response> {
    if caller_is_admin(state, auth.user_id) {
        return Ok(());
    }
    let trimmed = peer.trim();
    if let Some(cid_str) = trimmed.strip_prefix("chat_") {
        if let Ok(cid) = cid_str.parse::<i64>() {
            check_active_membership(state, ChatId(cid), auth.user_id)?;
            return Ok(());
        }
    }
    if let Some(uid_str) = trimmed.strip_prefix("user_") {
        if let Ok(uid) = uid_str.parse::<u64>() {
            if uid != auth.user_id.0 {
                return Err(err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Cannot access another user's Honcho profile",
                ));
            }
            return Ok(());
        }
    }
    if let Ok(uid) = trimmed.parse::<u64>() {
        if uid != auth.user_id.0 {
            return Err(err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Cannot access another user's Honcho profile",
            ));
        }
        return Ok(());
    }
    if let Ok(Some(u)) = state.db.get_user_by_username(trimmed) {
        if u.user_id != auth.user_id {
            return Err(err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Cannot access another user's Honcho profile",
            ));
        }
    }
    Ok(())
}

pub async fn hermes_honcho_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<HonchoQuery>,
) -> Response {
    if let Some(bid) = q.bot_id {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, q.bot_id);
    let uid = q
        .user_id
        .or(q.peer_name)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("user_{}", auth.user_id.0));
    if let Err(resp) = authorize_honcho_peer(&state, &auth, &uid) {
        return resp;
    }
    match engine.honcho().profile(&uid) {
        Ok(prof) => (StatusCode::OK, Json(honcho_profile_to_dto(prof))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_honcho_post(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
    Json(payload): Json<Value>,
) -> Response {
    let bot_id_u64 = q.bot_id.or_else(|| parse_bot_id_u64(payload.get("bot_id")));
    if let Some(bid) = bot_id_u64 {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, bot_id_u64);
    let uid = payload
        .get("peer_name")
        .or_else(|| payload.get("user_id"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("user_{}", auth.user_id.0));
    if let Err(resp) = authorize_honcho_peer(&state, &auth, &uid) {
        return resp;
    }
    let category = payload
        .get("category")
        .and_then(|v| v.as_str())
        .map(ObservationCategory::parse)
        .unwrap_or(ObservationCategory::Fact);

    if let Some(note) = payload
        .get("note")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Err(e) = engine
            .honcho()
            .record_observation(&uid, category, note, 1.0, None)
        {
            return map_telegram_err(e);
        }
    }

    if let Some(q) = payload
        .get("query")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match engine.honcho().query(&uid, q) {
            Ok(qres) => {
                let prof = engine.honcho().profile(&uid).ok();
                let mut dto = prof
                    .map(honcho_profile_to_dto)
                    .unwrap_or_else(|| json!({ "enabled": true, "peer_name": uid, "session_notes": [] }));
                if let Some(obj) = dto.as_object_mut() {
                    obj.insert("profile_summary".to_string(), json!(qres.answer));
                }
                return (StatusCode::OK, Json(dto)).into_response();
            }
            Err(e) => return map_telegram_err(e),
        }
    }

    match engine.honcho().profile(&uid) {
        Ok(prof) => (StatusCode::OK, Json(honcho_profile_to_dto(prof))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

// ============================================================================
// Skills Hub & Security Quarantine
// ============================================================================

#[derive(Debug, Deserialize, Default)]
pub struct SkillsQuery {
    pub filter: Option<String>,
    pub bot_id: Option<u64>,
}

pub async fn hermes_skills_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<SkillsQuery>,
) -> Response {
    if let Some(bid) = q.bot_id {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, q.bot_id);
    let now_ms = Utc::now().timestamp_millis();
    let skills: Vec<Value> = engine
        .skills()
        .skills_list(q.filter.as_deref())
        .iter()
        .map(|doc| skill_to_dto(doc, now_ms))
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "skills": skills, "total": skills.len() })),
    )
        .into_response()
}

pub async fn hermes_skills_create(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
    Json(payload): Json<Value>,
) -> Response {
    let bot_id_u64 = q.bot_id.or_else(|| parse_bot_id_u64(payload.get("bot_id")));
    if let Some(bid) = bot_id_u64 {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, bot_id_u64);
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let content = payload
        .get("content")
        .or_else(|| payload.get("body"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let category = payload
        .get("category")
        .or_else(|| payload.get("origin"))
        .and_then(|v| v.as_str())
        .map(SkillOrigin::parse);
    let description = payload
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("Custom skill '{}'", name));
    let tags: Option<Vec<String>> = payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().map(ToString::to_string))
                .collect()
        });
    let trusted = payload.get("trusted").and_then(|v| v.as_bool()).unwrap_or(false);

    let res = if content.trim_start().starts_with("---") {
        engine
            .skills()
            .skill_upsert_markdown(&content, category, trusted)
    } else {
        engine.skills().skill_create(CreateSkillRequest {
            name,
            description,
            body: content,
            platforms: None,
            toolsets: None,
            tags,
            version: None,
            origin: category,
            trusted: Some(trusted),
        })
    };

    match res {
        Ok(doc) => (
            StatusCode::CREATED,
            Json(skill_to_dto(&doc, Utc::now().timestamp_millis())),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_skill_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<OptionalBotScopeQuery>,
) -> Response {
    if let Some(bid) = q.bot_id {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, q.bot_id);
    match engine.skills().skill_view(&name) {
        Ok(doc) => (
            StatusCode::OK,
            Json(skill_to_dto(&doc, Utc::now().timestamp_millis())),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_skill_patch(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<OptionalBotScopeQuery>,
    Json(payload): Json<Value>,
) -> Response {
    let bot_id_u64 = q.bot_id.or_else(|| parse_bot_id_u64(payload.get("bot_id")));
    if let Some(bid) = bot_id_u64 {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, bot_id_u64);
    if let (Some(old_text), Some(new_text)) = (
        payload.get("old_text").and_then(|v| v.as_str()),
        payload.get("new_text").and_then(|v| v.as_str()),
    ) {
        return match engine.skills().skill_patch(&name, old_text, new_text) {
            Ok(doc) => (
                StatusCode::OK,
                Json(skill_to_dto(&doc, Utc::now().timestamp_millis())),
            )
                .into_response(),
            Err(e) => map_telegram_err(e),
        };
    }

    let existing = match engine.skills().skill_view(&name) {
        Ok(doc) => doc,
        Err(e) => return map_telegram_err(e),
    };
    let content = payload
        .get("content")
        .or_else(|| payload.get("body"))
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.raw_markdown)
        .to_string();
    let res = if content.trim_start().starts_with("---") {
        engine
            .skills()
            .skill_upsert_markdown(&content, Some(existing.frontmatter.origin), existing.trusted)
    } else {
        let description = payload
            .get("description")
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
            .unwrap_or(existing.frontmatter.description);
        let tags = payload
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or(existing.frontmatter.tags);
        engine.skills().skill_create(CreateSkillRequest {
            name: existing.frontmatter.name,
            description,
            body: content,
            platforms: Some(existing.frontmatter.platforms),
            toolsets: Some(existing.frontmatter.toolsets),
            tags: Some(tags),
            version: Some(existing.frontmatter.version),
            origin: Some(existing.frontmatter.origin),
            trusted: Some(existing.trusted),
        })
    };

    match res {
        Ok(doc) => (
            StatusCode::OK,
            Json(skill_to_dto(&doc, Utc::now().timestamp_millis())),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_skill_delete(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<OptionalBotScopeQuery>,
) -> Response {
    if let Some(bid) = q.bot_id {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, q.bot_id);
    match engine.skills().skill_delete(&name) {
        Ok(true) => (
            StatusCode::OK,
            Json(json!({ "deleted": true, "name": name })),
        )
            .into_response(),
        Ok(false) => err_resp(StatusCode::NOT_FOUND, "skill_not_found", "Skill not found"),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_skill_approve(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<OptionalBotScopeQuery>,
    payload: Option<Json<Value>>,
) -> Response {
    let bot_id_u64 = q.bot_id.or_else(|| {
        payload
            .as_ref()
            .and_then(|Json(v)| parse_bot_id_u64(v.get("bot_id")))
    });
    if let Some(bid) = bot_id_u64 {
        if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, bid) {
            return resp;
        }
    }
    let engine = resolve_hermes_for_bot(&state, bot_id_u64);
    let allow_high_risk = payload
        .as_ref()
        .and_then(|Json(v)| v.get("allow_high_risk").and_then(|b| b.as_bool()))
        .unwrap_or(true);

    let pending = engine.skills().list_pending_skill_writes();
    if let Some(proposal) = pending
        .into_iter()
        .find(|p| p.id == name || p.skill_name.eq_ignore_ascii_case(&name))
    {
        return match engine
            .skills()
            .approve_skill_write(&proposal.id, allow_high_risk)
        {
            Ok(doc) => (
                StatusCode::OK,
                Json(skill_to_dto(&doc, Utc::now().timestamp_millis())),
            )
                .into_response(),
            Err(e) => map_telegram_err(e),
        };
    }

    match engine.skills().skill_view(&name) {
        Ok(existing) => {
            match engine.skills().skill_create(CreateSkillRequest {
                name: existing.frontmatter.name,
                description: existing.frontmatter.description,
                body: existing.body,
                platforms: Some(existing.frontmatter.platforms),
                toolsets: Some(existing.frontmatter.toolsets),
                tags: Some(existing.frontmatter.tags),
                version: Some(existing.frontmatter.version),
                origin: Some(existing.frontmatter.origin),
                trusted: Some(true),
            }) {
                Ok(doc) => (
                    StatusCode::OK,
                    Json(skill_to_dto(&doc, Utc::now().timestamp_millis())),
                )
                    .into_response(),
                Err(e) => map_telegram_err(e),
            }
        }
        Err(e) => map_telegram_err(e),
    }
}

// ============================================================================
// Toolsets & Security Approvals
// ============================================================================

pub async fn hermes_toolsets_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
) -> Response {
    let engine = resolve_hermes_for_bot(&state, q.bot_id);
    let items = toolsets_list_json(&engine);
    (
        StatusCode::OK,
        Json(json!({ "toolsets": items, "total": items.len() })),
    )
        .into_response()
}

pub async fn hermes_toolsets_put(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
    Json(payload): Json<Value>,
) -> Response {
    let bot_id_u64 = q.bot_id.or_else(|| parse_bot_id_u64(payload.get("bot_id")));
    let is_admin = state
        .db
        .get_user(auth.user_id)
        .ok()
        .flatten()
        .is_some_and(|u| u.is_support);
    if let Some(bid) = bot_id_u64 {
        let owns_bot = state
            .fabushi_bots
            .get_by_user_id(bid as i64)
            .is_some_and(|i| i.settings().owner_user_id == auth.user_id.0 as i64)
            || state
                .db
                .get_bot_by_user_id(bid as i64)
                .ok()
                .flatten()
                .is_some_and(|b| b.owner_user_id == auth.user_id.0 as i64);
        if !owns_bot && !is_admin {
            return err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Only bot owner or server admin can modify bot toolsets",
            );
        }
    } else if !is_admin {
        return err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Admin privileges required to modify global Hermes toolsets",
        );
    }
    let engine = resolve_hermes_for_bot(&state, bot_id_u64);
    {
        let mut reg = engine
            .toolsets()
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(preset_name) = payload.get("preset").and_then(|v| v.as_str()) {
            if let Some(preset) = ToolsetPreset::from_str(preset_name) {
                reg.apply_preset(preset);
            } else {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_preset",
                    &format!("Unknown preset '{preset_name}'"),
                );
            }
        } else if let Some(arr) = payload.get("enabled_toolsets").and_then(|v| v.as_array()) {
            for id in ToolsetId::ALL {
                if id != ToolsetId::Core {
                    reg.disable(id);
                }
            }
            for item in arr {
                if let Some(name) = item.as_str() {
                    if let Err(reason) = reg.enable_by_name(name) {
                        return err_resp(StatusCode::BAD_REQUEST, "invalid_toolset", &reason);
                    }
                }
            }
        } else if let Some(name) = payload.get("toolset").and_then(|v| v.as_str()) {
            let enable = payload.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            let res = if enable {
                reg.enable_by_name(name).map(|_| ())
            } else {
                reg.disable_by_name(name).map(|_| ())
            };
            if let Err(reason) = res {
                return err_resp(StatusCode::BAD_REQUEST, "invalid_toolset", &reason);
            }
        }
    }

    let items = toolsets_list_json(&engine);
    (
        StatusCode::OK,
        Json(json!({ "toolsets": items, "total": items.len() })),
    )
        .into_response()
}

pub async fn hermes_approvals_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(q): Query<OptionalBotScopeQuery>,
) -> Response {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut engines = Vec::new();
    if q.bot_id.is_some() {
        engines.push(resolve_hermes_for_bot(&state, q.bot_id));
    } else {
        engines.push(Arc::clone(&state.hermes));
        for inst in state.fabushi_bots.list_instances() {
            engines.push(inst.hermes());
        }
    }

    let is_admin = caller_is_admin(&state, auth.user_id);
    for engine in &engines {
        let can_view_session = |sid: &str| -> bool {
            match engine.store().get_session(sid) {
                Ok(Some(sess)) => authorize_hermes_session_access(&state, &auth, &sess, q.bot_id).is_ok(),
                _ => is_admin,
            }
        };
        if let Ok(sec) = engine.security().read() {
            for p in sec
                .approvals
                .pending
                .iter()
                .chain(sec.approvals.resolved.iter())
            {
                if !can_view_session(&p.session_id) {
                    continue;
                }
                if seen.insert(p.id.clone()) {
                    items.push(json!({
                        "approval_id": p.id,
                        "session_id": p.session_id,
                        "tool_name": p.tool_name,
                        "command_preview": p.command,
                        "risk_level": p.risk_level,
                        "reason": p.reason,
                        "status": sec_approval_status_str(p.status),
                        "created_at_ms": p.created_at.timestamp_millis(),
                        "decided_at_ms": p.decided_at.map(|d| d.timestamp_millis()),
                    }));
                }
            }
        }

        if let Ok(rows) = engine.store().list_approvals(None, false) {
            for row in rows {
                if !can_view_session(&row.session_id) {
                    continue;
                }
                if seen.insert(row.id.clone()) {
                    items.push(db_approval_to_dto(&row));
                }
            }
        }
    }

    (
        StatusCode::OK,
        Json(json!({ "approvals": items, "total": items.len() })),
    )
        .into_response()
}

pub(crate) fn apply_hermes_approval_decision(
    state: &AppState,
    user_id: UserId,
    approval_id: &str,
    raw_decision: &str,
    reason: Option<&str>,
) -> Result<(String, Value), Response> {
    let raw_dec = raw_decision.trim().to_ascii_lowercase();
    let actor = reason
        .map(|r| format!("user_{}:{r}", user_id.0))
        .unwrap_or_else(|| format!("user_{}", user_id.0));

    let (mem_action, db_status) = match raw_dec.as_str() {
        "approve" | "approve_once" | "once" => {
            (ApprovalDecisionAction::ApproveOnce, ApprovalStatus::ApprovedOnce)
        }
        "approve_session" | "session" => (
            ApprovalDecisionAction::ApproveSession,
            ApprovalStatus::ApprovedSession,
        ),
        "approve_permanent" | "permanent" => (
            ApprovalDecisionAction::ApprovePermanent,
            ApprovalStatus::ApprovedPermanent,
        ),
        "yolo" | "enable_yolo" => (
            ApprovalDecisionAction::EnableYolo,
            ApprovalStatus::ApprovedSession,
        ),
        "deny" | "reject" => (ApprovalDecisionAction::Deny, ApprovalStatus::Denied),
        other => {
            return Err(err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_decision",
                &format!("Unsupported approval decision '{other}'"),
            ));
        }
    };

    let mut candidate_engines = vec![Arc::clone(&state.hermes)];
    for inst in state.fabushi_bots.list_instances() {
        candidate_engines.push(inst.hermes());
    }

    for engine in candidate_engines {
        let mem_sid = engine.security().read().ok().and_then(|sec| {
            sec.approvals
                .pending
                .iter()
                .chain(sec.approvals.resolved.iter())
                .find(|p| p.id == approval_id)
                .map(|p| p.session_id.clone())
        });
        let db_sid = engine
            .store()
            .get_approval(approval_id)
            .ok()
            .flatten()
            .map(|r| r.session_id);
        let Some(approval_sid) = mem_sid.or(db_sid) else {
            continue;
        };

        if let Ok(Some(sess)) = engine.store().get_session(&approval_sid) {
            authorize_hermes_session_access_uid(state, user_id, &sess, None)?;
            if matches!(
                raw_dec.as_str(),
                "yolo" | "enable_yolo" | "approve_permanent" | "permanent"
            ) && !caller_owns_hermes_session_uid(state, user_id, &sess, None)
            {
                return Err(err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Only session owner or admin can enable yolo or permanent approval",
                ));
            }
        } else if !caller_is_admin(state, user_id) {
            return Err(err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Caller is not authorized to decide this approval",
            ));
        }

        let mem_res = engine
            .security()
            .write()
            .ok()
            .and_then(|mut sec| {
                sec.approvals
                    .decide(approval_id, mem_action, actor.clone())
                    .ok()
            });
        let db_res = engine
            .store()
            .decide_approval(approval_id, db_status, Some(actor.clone()))
            .ok();

        if let Some(p) = mem_res {
            let status_str = sec_approval_status_str(p.status).to_string();
            return Ok((
                status_str.clone(),
                json!({
                    "approval_id": p.id,
                    "session_id": p.session_id,
                    "tool_name": p.tool_name,
                    "command_preview": p.command,
                    "risk_level": p.risk_level,
                    "reason": p.reason,
                    "status": status_str,
                    "created_at_ms": p.created_at.timestamp_millis(),
                    "decided_at_ms": p.decided_at.map(|d| d.timestamp_millis()),
                }),
            ));
        }

        if let Some(rec) = db_res {
            let dto = db_approval_to_dto(&rec);
            let status_str = dto
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("decided")
                .to_string();
            return Ok((status_str, dto));
        }
    }

    Err(err_resp(
        StatusCode::NOT_FOUND,
        "approval_not_found",
        "Pending approval request not found",
    ))
}

pub async fn hermes_approval_decide(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(approval_id): Path<String>,
    Json(payload): Json<Value>,
) -> Response {
    let raw_dec = payload
        .get("decision")
        .or_else(|| payload.get("action"))
        .and_then(|v| v.as_str())
        .unwrap_or("approve_once");
    let reason = payload.get("reason").and_then(|v| v.as_str());
    match apply_hermes_approval_decision(&state, auth.user_id, &approval_id, raw_dec, reason) {
        Ok((_, body)) => (StatusCode::OK, Json(body)).into_response(),
        Err(resp) => resp,
    }
}

// ============================================================================
// Cron & Heartbeat
// ============================================================================

fn authorize_cron_job_access(
    state: &AppState,
    user_id: UserId,
    job: &CronJobRecord,
) -> std::result::Result<(), Response> {
    if caller_is_admin(state, user_id) {
        return Ok(());
    }
    if let Some(cid) = job.target_chat_id.or(job.origin_chat_id) {
        check_active_membership(state, ChatId(cid), user_id).map(|_| ())
    } else {
        let owner = state
            .hermes
            .store()
            .kv_get("cron_job_owner", &job.id)
            .ok()
            .flatten();
        if owner.as_deref() == Some(user_id.0.to_string().as_str()) {
            Ok(())
        } else {
            Err(err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Only the owner or server admin can access this cron job",
            ))
        }
    }
}

pub async fn hermes_cron_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state.hermes.store().list_cron_jobs(false) {
        Ok(list) => {
            let jobs: Vec<Value> = list
                .iter()
                .filter(|j| authorize_cron_job_access(&state, auth.user_id, j).is_ok())
                .map(cron_to_dto)
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "jobs": jobs, "total": jobs.len() })),
            )
                .into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_cron_create(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let job_id = payload
        .get("job_id")
        .or_else(|| payload.get("id"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let schedule = payload
        .get("schedule")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let prompt = payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64());
    if let Some(cid) = chat_id {
        if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
            return resp;
        }
    }

    let bot_id = parse_optional_bot_id(payload.get("bot_id"));
    if let Some(bid) = parse_bot_id_u64(payload.get("bot_id")) {
        if caller_owns_bot_or_admin(&state, auth.user_id, bid).is_err()
            && !(chat_id.is_some_and(|cid| {
                is_official_bot(&state, bid) || is_bot_member_of_chat(&state, bid, ChatId(cid))
            }))
        {
            return err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Only bot owner or server admin can bind this bot to a cron job",
            );
        }
    }
    let enabled = payload.get("enabled").and_then(|v| v.as_bool());
    let timezone = payload
        .get("timezone")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);

    let mut routine = CronRoutine::new("cron_check".to_string(), schedule.clone())
        .with_time_zone(timezone.clone());
    if !schedule.is_empty() && !routine.validate_expression() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_schedule",
            &format!("Invalid cron or @every expression '{schedule}'"),
        );
    }
    let next_run_at = payload
        .get("next_run_at")
        .and_then(|v| v.as_i64())
        .or_else(|| routine.compute_next_run(Utc::now()).map(|dt| dt.timestamp_millis()));

    if let Some(ref existing_id) = job_id {
        if let Ok(Some(existing)) = state.hermes.store().get_cron_job(existing_id) {
            if let Err(resp) = authorize_cron_job_access(&state, auth.user_id, &existing) {
                return resp;
            }
            let patch = CronJobPatch {
                name: (!name.is_empty()).then_some(name),
                schedule: (!schedule.is_empty()).then_some(schedule),
                prompt: (!prompt.is_empty()).then_some(prompt),
                target_chat_id: chat_id.map(Some),
                deliver_to: bot_id.map(Some),
                enabled,
                next_run_at: next_run_at.map(Some),
                ..Default::default()
            };
            return match state.hermes.store().update_cron_job(existing_id, patch) {
                Ok(rec) => (StatusCode::OK, Json(cron_to_dto(&rec))).into_response(),
                Err(e) => map_telegram_err(e),
            };
        }
    }

    match state.hermes.store().create_cron_job(CreateCronJobRequest {
        id: job_id,
        name,
        schedule,
        timezone,
        target_chat_id: chat_id,
        prompt,
        model: payload
            .get("model")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        skills: None,
        enabled,
        deliver_to: bot_id,
        origin_chat_id: chat_id,
        next_run_at,
    }) {
        Ok(rec) => {
            let _ = state
                .hermes
                .store()
                .kv_set("cron_job_owner", &rec.id, &auth.user_id.0.to_string());
            (StatusCode::CREATED, Json(cron_to_dto(&rec))).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_cron_patch(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Json(payload): Json<Value>,
) -> Response {
    let existing = match state.hermes.store().get_cron_job(&job_id) {
        Ok(Some(j)) => j,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "cron_not_found", "Cron job not found"),
        Err(e) => return map_telegram_err(e),
    };
    if let Err(resp) = authorize_cron_job_access(&state, auth.user_id, &existing) {
        return resp;
    }
    if !caller_is_admin(&state, auth.user_id) {
        if let Some(new_cid) = payload.get("chat_id").and_then(|v| v.as_i64()) {
            if let Err(resp) = check_active_membership(&state, ChatId(new_cid), auth.user_id) {
                return resp;
            }
        }
    }
    if let Some(bid) = parse_bot_id_u64(payload.get("bot_id")) {
        let effective_cid = payload
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .or(existing.target_chat_id)
            .or(existing.origin_chat_id);
        if caller_owns_bot_or_admin(&state, auth.user_id, bid).is_err()
            && !(effective_cid.is_some_and(|cid| {
                is_official_bot(&state, bid) || is_bot_member_of_chat(&state, bid, ChatId(cid))
            }))
        {
            return err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Only bot owner or server admin can bind this bot to a cron job",
            );
        }
    }
    let schedule = payload
        .get("schedule")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let next_run_at = if let Some(ref sched) = schedule {
        let mut routine = CronRoutine::new(job_id.clone(), sched.clone());
        if !routine.validate_expression() {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_schedule",
                &format!("Invalid cron or @every expression '{sched}'"),
            );
        }
        Some(routine.compute_next_run(Utc::now()).map(|dt| dt.timestamp_millis()))
    } else {
        None
    };

    let patch = CronJobPatch {
        name: payload
            .get("name")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        schedule,
        prompt: payload
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        target_chat_id: payload.get("chat_id").map(|v| v.as_i64()),
        deliver_to: payload.get("bot_id").map(|v| parse_optional_bot_id(Some(v))),
        enabled: payload.get("enabled").and_then(|v| v.as_bool()),
        next_run_at,
        ..Default::default()
    };

    match state.hermes.store().update_cron_job(&job_id, patch) {
        Ok(rec) => (StatusCode::OK, Json(cron_to_dto(&rec))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_cron_delete(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    let existing = match state.hermes.store().get_cron_job(&job_id) {
        Ok(Some(j)) => j,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "cron_not_found", "Cron job not found"),
        Err(e) => return map_telegram_err(e),
    };
    if let Err(resp) = authorize_cron_job_access(&state, auth.user_id, &existing) {
        return resp;
    }
    match state.hermes.store().delete_cron_job(&job_id) {
        Ok(true) => (
            StatusCode::OK,
            Json(json!({ "deleted": true, "job_id": job_id })),
        )
            .into_response(),
        Ok(false) => err_resp(StatusCode::NOT_FOUND, "cron_not_found", "Cron job not found"),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_cron_run(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    let existing = match state.hermes.store().get_cron_job(&job_id) {
        Ok(Some(j)) => j,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "cron_not_found", "Cron job not found"),
        Err(e) => return map_telegram_err(e),
    };
    if let Err(resp) = authorize_cron_job_access(&state, auth.user_id, &existing) {
        return resp;
    }
    let deliver_to = existing.deliver_to;

    match state.hermes.run_cron_job_now(&job_id).await {
        Ok(outcome) => {
            if outcome.status == "completed" {
                if let Some(cid) = outcome.target_chat_id {
                    let _ = deliver_hermes_proactive_message(
                        &state,
                        ChatId(cid),
                        deliver_to.as_deref(),
                        &outcome.response_text,
                        None,
                    )
                    .await;
                }
            }
            (StatusCode::OK, Json(json!(outcome))).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_heartbeat_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let cfg = state.hermes.heartbeat().status();
    if let Some(cid) = cfg.target_chat_id {
        if !caller_is_admin(&state, auth.user_id) {
            if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
                return resp;
            }
        }
    }
    let bot_id = state
        .hermes
        .store()
        .kv_get("heartbeat", "bot_id")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<u64>().ok());
    (
        StatusCode::OK,
        Json(json!({
            "enabled": cfg.enabled,
            "interval_secs": cfg.interval_secs,
            "chat_id": cfg.target_chat_id,
            "bot_id": bot_id,
            "prompt": cfg.prompt,
            "last_beat_ms": cfg.last_tick_ms,
        })),
    )
        .into_response()
}

pub async fn hermes_heartbeat_post(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let mut cfg: HeartbeatConfig = state.hermes.heartbeat().status();
    let is_admin = caller_is_admin(&state, auth.user_id);
    if !is_admin {
        if let Some(existing_cid) = cfg.target_chat_id {
            if let Err(resp) = check_active_membership(&state, ChatId(existing_cid), auth.user_id) {
                return resp;
            }
        }
    }
    if let Some(enabled) = payload.get("enabled").and_then(|v| v.as_bool()) {
        cfg.enabled = enabled;
    }
    if let Some(secs) = payload.get("interval_secs").and_then(|v| v.as_u64()) {
        cfg.interval_secs = secs.max(5);
    }
    if let Some(chat_val) = payload.get("chat_id").or_else(|| payload.get("target_chat_id")) {
        if chat_val.is_null() {
            cfg.target_chat_id = None;
        } else if let Some(cid) = chat_val.as_i64() {
            if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
                return resp;
            }
            cfg.target_chat_id = Some(cid);
        }
    }
    let requested_bot_id = parse_bot_id_u64(payload.get("bot_id"));
    if !is_admin {
        if let Some(bid) = requested_bot_id {
            if caller_owns_bot_or_admin(&state, auth.user_id, bid).is_err() {
                let allowed_official = is_official_bot(&state, bid)
                    && cfg
                        .target_chat_id
                        .is_some_and(|cid| is_bot_member_of_chat(&state, bid, ChatId(cid)));
                if !allowed_official {
                    return err_resp(
                        StatusCode::FORBIDDEN,
                        "forbidden",
                        "Only bot owner or server admin can bind this bot to heartbeat",
                    );
                }
            }
        } else if cfg.target_chat_id.is_none() {
            if let Some(existing_bid) = state
                .hermes
                .store()
                .kv_get("heartbeat", "bot_id")
                .ok()
                .flatten()
                .and_then(|s| s.trim().trim_start_matches("bot_").parse::<u64>().ok())
            {
                if !is_official_bot(&state, existing_bid) {
                    if let Err(resp) = caller_owns_bot_or_admin(&state, auth.user_id, existing_bid)
                    {
                        return resp;
                    }
                }
            }
        }
    }
    if let Some(prompt) = payload.get("prompt").and_then(|v| v.as_str()) {
        if !prompt.trim().is_empty() {
            cfg.prompt = prompt.to_string();
        }
    }
    if let Some(bot_id_str) = parse_optional_bot_id(payload.get("bot_id")) {
        let _ = state.hermes.store().kv_set("heartbeat", "bot_id", &bot_id_str);
    }

    state.hermes.heartbeat().set_config(cfg.clone());
    let bot_id = state
        .hermes
        .store()
        .kv_get("heartbeat", "bot_id")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<u64>().ok());

    (
        StatusCode::OK,
        Json(json!({
            "enabled": cfg.enabled,
            "interval_secs": cfg.interval_secs,
            "chat_id": cfg.target_chat_id,
            "bot_id": bot_id,
            "prompt": cfg.prompt,
            "last_beat_ms": cfg.last_tick_ms,
        })),
    )
        .into_response()
}

// ============================================================================
// Kanban Boards & Cards
// ============================================================================

pub async fn hermes_kanban_boards_list(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let boards = match state.hermes.store().list_kanban_boards() {
        Ok(b) => b,
        Err(e) => return map_telegram_err(e),
    };
    let dtos: Vec<Value> = boards
        .iter()
        .map(|board| {
            let cards = state
                .hermes
                .store()
                .list_kanban_cards(&board.id, None)
                .unwrap_or_default();
            board_to_dto(board, &cards)
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "boards": dtos, "total": dtos.len() })),
    )
        .into_response()
}

pub async fn hermes_kanban_boards_create(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let description = payload
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let columns: Option<Vec<String>> = payload
        .get("columns")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        });
    match state
        .hermes
        .store()
        .create_kanban_board(name, description, columns)
    {
        Ok(board) => (StatusCode::CREATED, Json(board_to_dto(&board, &[]))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_kanban_board_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(board_id): Path<String>,
) -> Response {
    match state.hermes.store().get_kanban_board(&board_id) {
        Ok(Some(board)) => {
            let cards = state
                .hermes
                .store()
                .list_kanban_cards(&board.id, None)
                .unwrap_or_default();
            (StatusCode::OK, Json(board_to_dto(&board, &cards))).into_response()
        }
        Ok(None) => err_resp(StatusCode::NOT_FOUND, "board_not_found", "Kanban board not found"),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_kanban_cards_create(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let board_id = payload
        .get("board_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let req = CreateKanbanCardRequest {
        id: None,
        board_id,
        column_name: payload
            .get("column")
            .or_else(|| payload.get("column_name"))
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        title,
        description: payload
            .get("description")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        priority: payload
            .get("priority")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        assignee: payload
            .get("assigned_agent")
            .or_else(|| payload.get("assignee"))
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        labels: None,
    };
    match state.hermes.store().create_kanban_card(req) {
        Ok(card) => (StatusCode::CREATED, Json(card_to_dto(&card))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_kanban_cards_patch(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(card_id): Path<String>,
    Json(payload): Json<Value>,
) -> Response {
    let patch = KanbanCardPatch {
        title: payload
            .get("title")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        description: payload
            .get("description")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        priority: payload
            .get("priority")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        assignee: payload
            .get("assigned_agent")
            .or_else(|| payload.get("assignee"))
            .map(|v| v.as_str().map(ToString::to_string)),
        column_name: payload
            .get("column")
            .or_else(|| payload.get("column_name"))
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        position: payload.get("position").and_then(|v| v.as_i64()),
        ..Default::default()
    };
    match state.hermes.store().update_kanban_card(&card_id, patch) {
        Ok(card) => (StatusCode::OK, Json(card_to_dto(&card))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn hermes_kanban_cards_delete(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(card_id): Path<String>,
) -> Response {
    match state.hermes.store().delete_kanban_card(&card_id) {
        Ok(true) => (
            StatusCode::OK,
            Json(json!({ "deleted": true, "card_id": card_id })),
        )
            .into_response(),
        Ok(false) => err_resp(StatusCode::NOT_FOUND, "card_not_found", "Kanban card not found"),
        Err(e) => map_telegram_err(e),
    }
}

// ============================================================================
// Providers, Usage, File Rollback, Terminal Processes, Talk Turn
// ============================================================================

pub async fn hermes_providers_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let snap = state.hermes.providers().snapshot();
    let available_models = collect_available_models(&snap);
    let mut list: Vec<&ProviderConfig> = snap.providers.values().collect();
    list.sort_by(|a, b| a.id.cmp(&b.id));
    let providers: Vec<Value> = list
        .into_iter()
        .map(|cfg| provider_to_dto(cfg, &snap.active_provider_id, &snap.model_profiles))
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "providers": providers,
            "active_provider": snap.active_provider_id,
            "active_model": snap.active_model,
            "allow_local_fallback": snap.allow_local_fallback,
            "available_models": available_models,
        })),
    )
        .into_response()
}

pub async fn hermes_providers_put(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let provider_id = payload
        .get("provider_id")
        .or_else(|| payload.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if provider_id.is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_provider_id",
            "provider_id is required",
        );
    }

    let snap_before = state.hermes.providers().snapshot();
    let mut cfg = snap_before
        .providers
        .get(&provider_id)
        .cloned()
        .unwrap_or_else(|| {
            ProviderConfig::openai_compatible(
                provider_id.clone(),
                payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&provider_id),
                payload
                    .get("base_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("https://api.openai.com/v1"),
                payload
                    .get("default_model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("gpt-4o-mini"),
                "",
            )
        });

    if let Some(name) = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cfg.name = name.to_string();
    }
    if let Some(raw_mode) = payload.get("api_mode").and_then(|v| v.as_str()) {
        if let Some(mode) = ApiMode::parse(raw_mode) {
            cfg.api_mode = mode;
        }
    }
    if let Some(base_url) = payload
        .get("base_url")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cfg.base_url = base_url.to_string();
    }
    if let Some(default_model) = payload
        .get("default_model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cfg.default_model = default_model.to_string();
    }
    if let Some(enabled) = payload.get("enabled").and_then(|v| v.as_bool()) {
        cfg.enabled = enabled;
    }

    state.hermes.providers().register_provider(cfg);
    if let Some(api_key) = payload
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = state.hermes.providers().add_credential(
            &provider_id,
            CredentialEntry::new(
                format!("{provider_id}_api_key"),
                api_key.to_string(),
                "gateway",
            ),
        );
    }

    if payload
        .get("set_active")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let model_override = payload
            .get("default_model")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Err(err) = state.hermes.providers().set_active(&provider_id, model_override) {
            return err_resp(StatusCode::BAD_REQUEST, "provider_activation_error", &err.to_string());
        }
    }

    if let Some(allow_local) = payload
        .get("allow_local_fallback")
        .and_then(|v| v.as_bool())
    {
        state.hermes.providers().set_allow_local_fallback(allow_local);
    }

    state.hermes.persist_providers_state();
    state.fabushi_bots.sync_all_providers();
    let snap_after = state.hermes.providers().snapshot();
    match snap_after.providers.get(&provider_id) {
        Some(updated) => {
            let mut dto = provider_to_dto(
                updated,
                &snap_after.active_provider_id,
                &snap_after.model_profiles,
            );
            if let Some(obj) = dto.as_object_mut() {
                obj.insert(
                    "allow_local_fallback".into(),
                    json!(snap_after.allow_local_fallback),
                );
            }
            (StatusCode::OK, Json(dto)).into_response()
        }
        None => err_resp(StatusCode::NOT_FOUND, "provider_not_found", "Provider not found"),
    }
}

pub async fn hermes_usage_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let summary = match state.hermes.store().usage_summary(None, None) {
        Ok(s) => s,
        Err(e) => return map_telegram_err(e),
    };
    let tool_calls: u64 = state
        .hermes
        .store()
        .insights(None, None)
        .map(|i| i.top_tools.iter().map(|t| t.call_count.max(0) as u64).sum())
        .unwrap_or(0);

    (
        StatusCode::OK,
        Json(json!({
            "total_turns": summary.total_messages.max(0) as u64,
            "prompt_tokens": summary.total_input_tokens.max(0) as u64,
            "completion_tokens": summary.total_output_tokens.max(0) as u64,
            "total_tokens": summary.total_tokens.max(0) as u64,
            "estimated_cost_usd": summary.total_cost_usd,
            "tool_calls": tool_calls,
            "by_model": summary.by_model,
        })),
    )
        .into_response()
}

pub async fn hermes_rollback_post(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if session_id.is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_session_id",
            "session_id is required",
        );
    }
    let bot_id_u64 = parse_bot_id_u64(payload.get("bot_id"));
    let engine = resolve_hermes_for_session(&state, bot_id_u64, &session_id);
    match engine.store().get_session(&session_id) {
        Ok(Some(sess)) => {
            if let Err(resp) = authorize_hermes_session_access(&state, &auth, &sess, bot_id_u64) {
                return resp;
            }
        }
        Ok(None) => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "session_not_found",
                "Hermes session not found",
            );
        }
        Err(e) => return map_telegram_err(e),
    }
    let turn_index = payload
        .get("turn_index")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            payload
                .get("turn_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok())
        });

    match engine.rollback_turn(&session_id, turn_index) {
        Ok(report) => {
            let mut restored_files = report.restored_files;
            restored_files.extend(report.deleted_files.iter().map(|f| format!("deleted:{f}")));
            (
                StatusCode::OK,
                Json(json!({
                    "rolled_back": true,
                    "session_id": report.session_id,
                    "restored_files": restored_files,
                    "message": format!("Rolled back turn {}", report.turn_index),
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::OK,
            Json(json!({
                "rolled_back": false,
                "session_id": session_id,
                "restored_files": [],
                "message": e.to_string(),
            })),
        )
            .into_response(),
    }
}

pub async fn hermes_terminal_processes_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let list = state.hermes.processes().list().await;
    let processes: Vec<Value> = list
        .into_iter()
        .map(|p| {
            json!({
                "process_id": p.process_id,
                "session_id": Value::Null,
                "command": p.command,
                "status": if p.running { "running" } else { "exited" },
                "pid": p.pid,
                "exit_code": p.exit_code,
                "started_at_ms": p.started_at.timestamp_millis(),
                "output_tail": format!("stdout: {} bytes, stderr: {} bytes", p.stdout_bytes, p.stderr_bytes),
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "processes": processes, "total": processes.len() })),
    )
        .into_response()
}

pub async fn hermes_talk_turn(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let transcript = payload
        .get("transcript")
        .or_else(|| payload.get("prompt"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if transcript.is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_transcript",
            "transcript is required",
        );
    }

    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64());
    if let Some(cid) = chat_id {
        if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
            return resp;
        }
    }

    let bot_id_u64 = parse_bot_id_u64(payload.get("bot_id"));
    let engine = resolve_hermes_for_bot(&state, bot_id_u64);
    let barge_in = payload
        .get("barge_in")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let voice_preset = payload
        .get("voice_preset")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);

    let requested_sid = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let sess_engine_opt = requested_sid
        .as_deref()
        .map(|req_sid| resolve_hermes_for_session(&state, bot_id_u64, &format!("hermes_{req_sid}")));
    let effective_bot_id = sess_engine_opt
        .as_ref()
        .and_then(|se| resolve_effective_bot_id(&state, bot_id_u64, se))
        .or_else(|| resolve_effective_bot_id(&state, bot_id_u64, &engine));
    let owns_bot = effective_bot_id
        .map(|bid| caller_owns_bot_or_admin(&state, auth.user_id, bid).is_ok())
        .unwrap_or(true);

    if let Some(ref req_sid) = requested_sid {
        let talk_check_sid = format!("hermes_{req_sid}");
        let sess_engine = sess_engine_opt.as_ref().unwrap_or(&engine);
        if let Ok(Some(existing)) = sess_engine.store().get_session(&talk_check_sid) {
            if chat_id.is_some() && existing.chat_id.is_some() && existing.chat_id != chat_id {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Target session belongs to a different chat",
                );
            }
            if let Err(resp) = authorize_hermes_session_access(&state, &auth, &existing, bot_id_u64) {
                return resp;
            }
        }
        if let Ok(rows) = state.db.load_talk_sessions() {
            if let Some((_, owner_uid, _, _, _)) = rows.into_iter().find(|(sid, _, _, _, _)| sid == req_sid) {
                if owner_uid != auth.user_id.0 && !caller_is_admin(&state, auth.user_id) {
                    return err_resp(
                        StatusCode::FORBIDDEN,
                        "forbidden",
                        "Caller is not authorized to access this talk session",
                    );
                }
            }
        }
    }

    if let Some(bid) = effective_bot_id {
        if !owns_bot {
            if gpui_telegram_agent::hermes::commands::is_bot_config_command(&transcript) {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Only bot owner or server admin can run bot config/memory/soul/skill commands",
                );
            }
            if let Some(cid) = chat_id {
                if !is_bot_member_of_chat(&state, bid, ChatId(cid))
                    && !is_official_bot(&state, bid)
                {
                    return err_resp(
                        StatusCode::FORBIDDEN,
                        "forbidden",
                        "Target bot is not a member of the specified chat",
                    );
                }
            } else if !is_official_bot(&state, bid)
                && !(!bot_has_human_owner(&state, bid)
                    && state
                        .db
                        .get_bot_computer_profile(auth.user_id.0, bid)
                        .ok()
                        .flatten()
                        .is_some())
            {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Only bot owner or server admin can invoke this bot outside a shared chat",
                );
            }
        }
    }

    let session_id = {
        let mut coord = state.talk_coordinator.write();
        let sid = requested_sid.unwrap_or_else(|| {
            format!("talk_{}_{}", auth.user_id.0, Uuid::new_v4().simple())
        });
        coord.session_id = sid.clone();
        if barge_in {
            let _ = coord.handle_barge_in();
        }
        coord.state = gpui_telegram_agent::TalkSessionState::Thinking;
        coord.signal_seq = coord.signal_seq.saturating_add(1);
        sid
    };

    let persona_bot_id = bot_id_u64.or(effective_bot_id);
    let (bot_sp, bot_model, _) = persona_bot_id
        .map(|bid| {
            let (sp, model, welcome) = load_bot_persona_config(&state, UserId(bid));
            let user_computer_sp = if !owns_bot
                && !bot_has_human_owner(&state, bid)
                && !is_official_bot(&state, bid)
            {
                state
                    .db
                    .get_bot_computer_profile(auth.user_id.0, bid)
                    .ok()
                    .flatten()
                    .map(|p| p.persona_prompt)
                    .filter(|s| !s.trim().is_empty())
            } else {
                None
            };
            (user_computer_sp.or(sp), model, welcome)
        })
        .unwrap_or((None, None, None));

    let voice_sys = format!(
        "{} Keep responses concise and natural for real-time voice playback{}.",
        bot_sp.unwrap_or_else(|| "You are Hermes Voice Assistant.".to_string()),
        voice_preset
            .as_deref()
            .map(|v| format!(" (voice preset: {v})"))
            .unwrap_or_default()
    );
    let bot_display_name = persona_bot_id.and_then(|bid| resolve_bot_display_name(&state, UserId(bid)));
    let (enriched_sp, effective_model) = build_enriched_bot_turn_context(
        &state,
        auth.user_id.0,
        persona_bot_id,
        bot_display_name.as_deref(),
        Some(voice_sys),
        None,
        sanitize_model_override(&state, bot_model),
        &[],
        false,
    );

    let talk_sid = format!("hermes_{session_id}");
    if let Some(bid) = effective_bot_id {
        let _ = resolve_and_apply_bot_session_policy(
            &state,
            auth.user_id.0,
            UserId(bid),
            chat_id.map(ChatId),
            Some(&talk_sid),
            "talk_mode",
            effective_model.as_deref(),
            false,
        );
    }

    let caller_scope = build_caller_scope(&state, auth.user_id);
    let req = HermesTurnRequest {
        session_id: Some(talk_sid),
        chat_id,
        bot_id: effective_bot_id.map(|id| id.to_string()),
        bot_name: bot_display_name,
        user_id: Some(auth.user_id.0.to_string()),
        source: "talk_mode".to_string(),
        prompt: transcript.clone(),
        model: effective_model,
        system_prompt: enriched_sp,
        preset: Some(ToolsetPreset::HermesTelegram),
        caller_allowed_chat_ids: Some(caller_scope.allowed_chat_ids),
        caller_is_admin: caller_scope.is_admin,
        caller_owns_bot: Some(owns_bot),
        ..Default::default()
    };

    let run_engine = sess_engine_opt.unwrap_or(engine);
    let res = match run_engine.run_turn(req).await {
        Ok(r) => r,
        Err(e) => return map_telegram_err(e),
    };

    let tts_segments = split_spoken_segments(&res.response_text);
    let now_ms = Utc::now().timestamp_millis();
    {
        let mut coord = state.talk_coordinator.write();
        coord.state = gpui_telegram_agent::TalkSessionState::Speaking;
        coord.signal_seq = coord.signal_seq.saturating_add(1);
    }
    let _ = state.db.upsert_talk_session(
        &session_id,
        auth.user_id.0,
        "speaking",
        &json!([
            { "role": "user", "text": transcript, "ts": now_ms },
            { "role": "assistant", "text": res.response_text, "ts": Utc::now().timestamp_millis() }
        ]),
        now_ms,
    );

    (
        StatusCode::OK,
        Json(json!({
            "session_id": session_id,
            "call_id": Value::Null,
            "state": "speaking",
            "transcript": transcript,
            "reply_text": res.response_text,
            "tts_segments": tts_segments,
            "bot_id": bot_id_u64,
            "chat_id": chat_id,
        })),
    )
        .into_response()
}

// ============================================================================
// OpenAI-Compatible API Gateway & ACP JSON-RPC
// ============================================================================

pub(crate) fn resolve_optional_bearer_user(headers: &HeaderMap, state: &AppState) -> Option<UserId> {
    let auth_hdr = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = auth_hdr
        .strip_prefix("Bearer ")
        .or_else(|| auth_hdr.strip_prefix("bearer "))?
        .trim();
    if token.is_empty() {
        return None;
    }
    if let Some(session) = state.session_manager.verify_token(token) {
        return Some(session.user_id);
    }
    let session_id = token.split('.').next().unwrap_or(token);
    let db_sess = state.db.get_session(session_id).ok().flatten()?;
    if db_sess.revoked_at_ms.is_some()
        || db_sess.expires_at_ms <= Utc::now().timestamp_millis()
    {
        return None;
    }
    let created_at =
        chrono::DateTime::from_timestamp_millis(db_sess.created_at_ms).unwrap_or_else(Utc::now);
    let expires_at =
        chrono::DateTime::from_timestamp_millis(db_sess.expires_at_ms).unwrap_or_else(Utc::now);
    let sess_tok = gpui_telegram_core::auth::SessionToken::new_with_hash(
        db_sess.session_id.clone(),
        db_sess.user_id,
        created_at,
        expires_at,
        &db_sess.device_id,
        db_sess.token_hash,
    );
    state.session_manager.insert(sess_tok);
    state
        .session_manager
        .verify_token(token)
        .map(|s| s.user_id)
}

pub async fn openai_chat_completions(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Value>,
) -> Response {
    let caller_uid = auth.user_id;
    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64());
    if let Some(cid) = chat_id {
        if let Err(resp) = check_active_membership(&state, ChatId(cid), caller_uid) {
            return resp;
        }
    }

    let messages = payload
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if messages.is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_messages",
            "messages array must be non-empty",
        );
    }

    let mut sys_parts = Vec::new();
    let mut convo_parts = Vec::new();
    for msg in &messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if role == "system" {
            if !content.trim().is_empty() {
                sys_parts.push(content.trim().to_string());
            }
        } else {
            convo_parts.push((role.to_string(), content.to_string()));
        }
    }
    let prompt = if convo_parts.len() == 1 && convo_parts[0].0 == "user" {
        convo_parts[0].1.clone()
    } else {
        convo_parts
            .iter()
            .map(|(r, c)| format!("{r}: {c}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let explicit_session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let sess_engine = if let Some(ref sid) = explicit_session_id {
        let eng = resolve_hermes_for_session(&state, None, sid);
        if let Ok(Some(existing)) = eng.store().get_session(sid) {
            if chat_id.is_some() && existing.chat_id.is_some() && existing.chat_id != chat_id {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Target session belongs to a different chat",
                );
            }
            if let Err(resp) = authorize_hermes_session_access(&state, &auth, &existing, None) {
                return resp;
            }
            if !caller_owns_hermes_session(&state, &auth, &existing, None) {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Cannot run completion on a session not owned by caller",
                );
            }
        }
        eng
    } else {
        Arc::clone(&state.hermes)
    };
    let session_id = explicit_session_id
        .unwrap_or_else(|| format!("openai_{}", Uuid::new_v4().simple()));
    let model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let stream_flag = payload.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);

    let effective_bot_id = resolve_effective_bot_id(&state, None, &sess_engine);
    let owns_bot = effective_bot_id
        .map(|bid| caller_owns_bot_or_admin(&state, caller_uid, bid).is_ok())
        .unwrap_or(true);
    let caller_scope = build_caller_scope(&state, caller_uid);
    let req = HermesTurnRequest {
        session_id: Some(session_id),
        chat_id,
        bot_id: None,
        bot_name: None,
        user_id: Some(caller_uid.0.to_string()),
        source: "openai_gateway".to_string(),
        prompt,
        model: model.clone(),
        system_prompt: (!sys_parts.is_empty()).then(|| sys_parts.join("\n\n")),
        thinking_budget_tokens: None,
        preset: None,
        max_iterations: None,
        max_tokens: payload
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32),
        temperature: payload
            .get("temperature")
            .and_then(|v| v.as_f64())
            .map(|n| n as f32),
        skip_slash_commands: true,
        caller_depth: 0,
        caller_allowed_chat_ids: Some(caller_scope.allowed_chat_ids),
        caller_is_admin: caller_scope.is_admin,
        caller_owns_bot: Some(owns_bot),
    };

    if stream_flag {
        struct AbortTaskOnDrop<T>(Option<tokio::task::JoinHandle<T>>);
        impl<T> Drop for AbortTaskOnDrop<T> {
            fn drop(&mut self) {
                if let Some(handle) = self.0.take() {
                    handle.abort();
                }
            }
        }

        let (rx, task) = match sess_engine.run_turn_stream_with_handle(req).await {
            Ok(pair) => pair,
            Err(e) => return map_telegram_err(e),
        };
        let cmpl_id = format!("chatcmpl-{}", Uuid::new_v4().simple());
        let created = Utc::now().timestamp();
        let default_model = model.unwrap_or_else(|| sess_engine.providers().active_info().1);
        let s = stream::unfold(
            (rx, AbortTaskOnDrop(Some(task)), false, false, cmpl_id, default_model),
            move |(mut rx, mut task_guard, mut finished, mut emitted_text, id, mdl)| async move {
                if finished {
                    return None;
                }
                while let Some(ev) = rx.recv().await {
                    match ev {
                        HermesEvent::TextDelta { text, .. } => {
                            if !text.is_empty() {
                                emitted_text = true;
                            }
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": mdl,
                                "choices": [{
                                    "index": 0,
                                    "delta": { "content": text },
                                    "finish_reason": Value::Null,
                                }]
                            });
                            let payload_str = format!("data: {chunk}\n\n");
                            return Some((
                                Ok::<axum::body::Bytes, Infallible>(axum::body::Bytes::from(payload_str)),
                                (rx, task_guard, false, emitted_text, id, mdl),
                            ));
                        }
                        HermesEvent::Done { response_text, .. } => {
                            if let Some(handle) = task_guard.0.take() {
                                let _ = handle.await;
                            }
                            let fallback_delta = if !emitted_text && !response_text.is_empty() {
                                json!({ "role": "assistant", "content": response_text })
                            } else {
                                json!({})
                            };
                            let final_chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": mdl,
                                "choices": [{
                                    "index": 0,
                                    "delta": fallback_delta,
                                    "finish_reason": "stop",
                                }]
                            });
                            let payload_str = format!("data: {final_chunk}\n\ndata: [DONE]\n\n");
                            finished = true;
                            return Some((
                                Ok(axum::body::Bytes::from(payload_str)),
                                (rx, task_guard, finished, emitted_text, id, mdl),
                            ));
                        }
                        HermesEvent::Error { code, message } => {
                            if let Some(handle) = task_guard.0.take() {
                                let _ = handle.await;
                            }
                            let err_chunk = json!({
                                "error": {
                                    "message": message,
                                    "type": "upstream_error",
                                    "code": code,
                                }
                            });
                            let payload_str = format!("data: {err_chunk}\n\n");
                            finished = true;
                            return Some((
                                Ok(axum::body::Bytes::from(payload_str)),
                                (rx, task_guard, finished, emitted_text, id, mdl),
                            ));
                        }
                        _ => continue,
                    }
                }
                if let Some(handle) = task_guard.0.take() {
                    let _ = handle.await;
                }
                let err_chunk = json!({
                    "error": {
                        "message": "Hermes stream terminated unexpectedly without completion",
                        "type": "upstream_error",
                    }
                });
                Some((
                    Ok(axum::body::Bytes::from(format!("data: {err_chunk}\n\n"))),
                    (rx, task_guard, true, emitted_text, id, mdl),
                ))
            },
        )
        .boxed();

        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(s))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    match sess_engine.run_turn(req).await {
        Ok(res) => (
            StatusCode::OK,
            Json(json!({
                "id": format!("chatcmpl-{}", res.turn_id),
                "object": "chat.completion",
                "created": Utc::now().timestamp(),
                "model": res.model,
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": res.response_text,
                    },
                    "finish_reason": "stop",
                }],
                "usage": {
                    "prompt_tokens": res.usage.input_tokens,
                    "completion_tokens": res.usage.output_tokens,
                    "total_tokens": res.usage.total_tokens,
                },
                "hermes_session_id": res.session_id,
            })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn openai_models_list(State(state): State<Arc<AppState>>) -> Response {
    let snap = state.hermes.providers().snapshot();
    let mut seen = HashSet::new();
    let mut data = Vec::new();
    let now = Utc::now().timestamp();

    for prof in snap.model_profiles.values() {
        if seen.insert(prof.model.clone()) {
            data.push(json!({
                "id": prof.model,
                "object": "model",
                "created": now,
                "owned_by": prof.provider_id,
            }));
        }
    }
    for p in snap.providers.values() {
        if !p.default_model.trim().is_empty() && seen.insert(p.default_model.clone()) {
            data.push(json!({
                "id": p.default_model,
                "object": "model",
                "created": now,
                "owned_by": p.id,
            }));
        }
    }

    (
        StatusCode::OK,
        Json(json!({
            "object": "list",
            "data": data,
        })),
    )
        .into_response()
}

pub async fn hermes_acp_rpc(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(rpc_req): Json<Value>,
) -> Response {
    let rpc_id = rpc_req.get("id").cloned().unwrap_or(Value::Null);
    let method = rpc_req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = rpc_req.get("params");
    let bot_id_u64 = params.and_then(|p| parse_bot_id_u64(p.get("bot_id")));
    let sid = params
        .and_then(|p| p.get("session_id"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let engine = if let Some(s) = sid {
        resolve_hermes_for_session(&state, bot_id_u64, s)
    } else {
        resolve_hermes_for_bot(&state, bot_id_u64)
    };
    let effective_bot_id = resolve_effective_bot_id(&state, bot_id_u64, &engine);
    let owns_bot = effective_bot_id
        .map(|bid| caller_owns_bot_or_admin(&state, auth.user_id, bid).is_ok())
        .unwrap_or(true);
    if let Some(bid) = effective_bot_id {
        if !owns_bot {
            if method == "tools/call" {
                let tool_name = params
                    .and_then(|p| p.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if matches!(
                    tool_name,
                    "memory" | "skill_manage" | "skill_list" | "skill_view"
                ) {
                    return acp_forbidden_response(
                        rpc_id,
                        "Only bot owner or server admin can access or modify bot memory or skills",
                    );
                }
            }
            if method == "command/execute" {
                let cmd_text = params
                    .and_then(|p| p.get("command"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if gpui_telegram_agent::hermes::commands::is_bot_config_command(cmd_text) {
                    return acp_forbidden_response(
                        rpc_id,
                        "Only bot owner or server admin can run bot config/memory/soul/skill commands",
                    );
                }
            }
            if method == "session/prompt" {
                let prompt_text = params
                    .and_then(|p| p.get("prompt"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if gpui_telegram_agent::hermes::commands::is_bot_config_command(prompt_text) {
                    return acp_forbidden_response(
                        rpc_id,
                        "Only bot owner or server admin can run bot config/memory/soul/skill commands",
                    );
                }
            }
            if matches!(
                method,
                "session/new" | "session/prompt" | "tools/call" | "command/execute"
            ) {
                let chat_id = params.and_then(|p| p.get("chat_id").and_then(|v| v.as_i64()));
                if let Some(cid) = chat_id {
                    if check_active_membership(&state, ChatId(cid), auth.user_id).is_err() {
                        return acp_forbidden_response(
                            rpc_id,
                            "Forbidden: caller is not an active member of target chat",
                        );
                    }
                    if !is_bot_member_of_chat(&state, bid, ChatId(cid))
                        && !is_official_bot(&state, bid)
                    {
                        return acp_forbidden_response(
                            rpc_id,
                            "Forbidden: target bot is not a member of specified chat",
                        );
                    }
                } else if !is_official_bot(&state, bid) {
                    return acp_forbidden_response(
                        rpc_id,
                        "Only bot owner or server admin can access or modify this bot resource",
                    );
                }
            }
        }
    }
    let mut scope = build_caller_scope(&state, auth.user_id);
    scope.bot_config_allowed = owns_bot;
    let resp = engine.acp_json_rpc_scoped(rpc_req, Some(scope)).await;
    let status = if resp
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_i64())
        == Some(-32003)
    {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::OK
    };
    (status, Json(resp)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use crate::db::ServerDatabase;
    use gpui_telegram_agent::hermes::providers::TokenUsage;
    use gpui_telegram_proto::ws::WsServerFrame;
    use tokio::sync::mpsc;

    struct TempMediaGuard(std::path::PathBuf);
    impl TempMediaGuard {
        fn new(prefix: &str) -> Self {
            let p = std::env::temp_dir().join(format!("{prefix}_{}", Uuid::new_v4().simple()));
            let _ = std::fs::create_dir_all(&p);
            Self(p)
        }
    }
    impl Drop for TempMediaGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn test_state(prefix: &str) -> (TempMediaGuard, Arc<AppState>) {
        let dir = TempMediaGuard::new(prefix);
        let db = Arc::new(ServerDatabase::open_in_memory().expect("in-memory db"));
        let mut cfg = ServerConfig::default();
        cfg.media_storage_path = dir.0.to_string_lossy().into_owned();
        let state = Arc::new(AppState::with_db(cfg, db));
        (dir, state)
    }

    #[test]
    fn test_sanitize_model_override_filters_placeholders() {
        for raw in [
            "",
            "   ",
            "default",
            "DEFAULT",
            "默认",
            "默认模型",
            "hermes-local-v1",
            "local_hermes/hermes-local-v1",
            "auto",
            "Auto",
        ] {
            assert_eq!(
                sanitize_model_override_for_provider(
                    Some(raw.to_string()),
                    "magpie_local",
                    Some("http://127.0.0.1:3425/v1"),
                ),
                None,
                "raw={raw}"
            );
        }
        // "deepseek-flash" is stripped on non-DeepSeek providers (e.g. magpie_local)
        // so ServerConfig's default does not override magpie_local's auto/gemini-3.8-flash.
        assert_eq!(
            sanitize_model_override_for_provider(
                Some(" deepseek-flash ".to_string()),
                "magpie_local",
                Some("http://127.0.0.1:3425/v1"),
            ),
            None
        );
        // "deepseek-flash" MUST be preserved when a DeepSeek or ai.ombhrum.com provider is active.
        assert_eq!(
            sanitize_model_override_for_provider(
                Some(" deepseek-flash ".to_string()),
                "deepseek",
                Some("https://api.deepseek.com/v1"),
            )
            .as_deref(),
            Some("deepseek-flash")
        );
        assert_eq!(
            sanitize_model_override_for_provider(
                Some("deepseek-flash".to_string()),
                "custom_openai",
                Some("https://ai.ombhrum.com/v1"),
            )
            .as_deref(),
            Some("deepseek-flash")
        );
        assert_eq!(
            sanitize_model_override_for_provider(
                Some(" deepseek-chat ".to_string()),
                "magpie_local",
                Some("http://127.0.0.1:3425/v1"),
            )
            .as_deref(),
            Some("deepseek-chat")
        );
        assert_eq!(
            sanitize_model_override_for_provider(
                Some("claude-sonnet-4-20250514".to_string()),
                "magpie_local",
                Some("http://127.0.0.1:3425/v1"),
            )
            .as_deref(),
            Some("claude-sonnet-4-20250514")
        );

        let (_dir, state) = test_state("hermes_sanitize_model");
        use gpui_telegram_agent::hermes::providers::{ApiMode, CredentialPool, ProviderConfig};
        state.hermes.providers().register_provider(ProviderConfig {
            id: "magpie_test".to_string(),
            name: "Magpie Test".to_string(),
            api_mode: ApiMode::ChatCompletions,
            base_url: "http://127.0.0.1:3425/v1".to_string(),
            default_model: "auto/gemini-3.8-flash".to_string(),
            headers: Default::default(),
            credentials: CredentialPool::new(),
            fallback_models: Vec::new(),
            timeout_secs: 30,
            enabled: true,
            ..Default::default()
        });
        state
            .hermes
            .providers()
            .set_active("magpie_test", None)
            .expect("activate magpie_test");
        assert_eq!(
            sanitize_model_override(&state, Some("deepseek-flash".to_string())),
            None
        );

        state.hermes.providers().register_provider(ProviderConfig {
            id: "deepseek_test".to_string(),
            name: "DeepSeek Test".to_string(),
            api_mode: ApiMode::ChatCompletions,
            base_url: "https://api.deepseek.com/v1".to_string(),
            default_model: "deepseek-flash".to_string(),
            headers: Default::default(),
            credentials: CredentialPool::new(),
            fallback_models: Vec::new(),
            timeout_secs: 30,
            enabled: true,
            ..Default::default()
        });
        state
            .hermes
            .providers()
            .set_active("deepseek_test", None)
            .expect("activate deepseek_test");
        assert_eq!(
            sanitize_model_override(&state, Some("deepseek-flash".to_string())).as_deref(),
            Some("deepseek-flash")
        );
    }

    #[test]
    fn test_is_autonomous_policy_str_variants() {
        for p in [
            "always_allow_authorized",
            "AlwaysAllowAuthorized",
            "always-allow-authorized",
            "yolo",
            "YOLO",
        ] {
            assert!(is_autonomous_execution_policy_for_test(p), "p={p}");
        }
        for p in ["ask_every_time", "allow_read_only", "disabled", "prompt"] {
            assert!(!is_autonomous_execution_policy_for_test(p), "p={p}");
        }
    }

    fn is_autonomous_execution_policy_for_test(raw: &str) -> bool {
        is_autonomous_policy_str(raw)
    }

    #[test]
    fn test_state_patch_extraction_and_display_stripping() {
        let streaming = "已加入冰拿铁\n[STATE_PATCH]{\"orders\": [\"冰拿";
        assert_eq!(strip_state_patches_for_display(streaming), "已加入冰拿铁");

        let partial_prefix = "更新中...[STATE_";
        assert_eq!(strip_state_patches_for_display(partial_prefix), "更新中...");

        let completed = "已完成点单！\n[STATE_PATCH]```json\n{\"orders\": [\"冰拿铁 x1\"], \"total_amount\": 28}\n```[/STATE_PATCH]\n还有其他需要吗？";
        assert_eq!(
            strip_state_patches_for_display(completed),
            "已完成点单！\n\n还有其他需要吗？"
        );
        let patches = extract_state_patches(completed);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0]["total_amount"], 28);
    }

    #[test]
    fn test_stream_consumer_render_streaming_and_final() {
        let mut sc = StreamConsumerState::new();
        sc.apply_event(HermesEvent::ThinkingDelta {
            text: "正在检查远程节点磁盘和进程状态".into(),
        });
        sc.apply_event(HermesEvent::ToolStart {
            tool_call_id: "c1".into(),
            tool_name: "computer_exec".into(),
            arguments: json!({"node_id": "cloud_1", "command": "df -h && uname -a"}),
        });
        let live_md = sc.render(false, None);
        assert!(live_md.contains("💭 **思考过程**"));
        assert!(live_md.contains("🔧 **执行步骤 (0/1)**"));
        assert!(live_md.contains("⏳ `computer_exec`"));

        sc.apply_event(HermesEvent::ToolResult {
            tool_call_id: "c1".into(),
            tool_name: "computer_exec".into(),
            success: true,
            output: "Filesystem Size Used Avail".into(),
            duration_ms: 145,
            inline_buttons: Vec::new(),
            ui_actions: Vec::new(),
        });
        sc.apply_event(HermesEvent::TextDelta {
            text: "服务器运行正常。".into(),
            accumulated: "服务器运行正常。".into(),
        });
        sc.apply_event(HermesEvent::Done {
            session_id: "s1".into(),
            turn_id: "t1".into(),
            response_text: "服务器运行正常。[STATE_PATCH]{\"ok\":true}[/STATE_PATCH]".into(),
            reasoning: None,
            iterations: 2,
            tool_calls_count: 1,
            usage: TokenUsage::default(),
            buttons: Vec::new(),
        });
        let final_md = sc.render(true, None);
        assert!(final_md.contains("🔧 **执行步骤 (1/1)**"));
        assert!(final_md.contains("✅ `computer_exec`"));
        assert!(final_md.contains("145ms"));
        assert!(final_md.contains("服务器运行正常。"));
        assert!(!final_md.contains("STATE_PATCH"));
        assert!(!final_md.contains("💭 **思考过程**"));
    }

    #[tokio::test]
    async fn test_bot_typing_guard_emits_cancel_on_drop_and_yolo_policy_applies() {
        let (_dir, state) = test_state("hermes_typing_guard");
        let uid = UserId(100_005);
        let bot_id = UserId(600_001);
        let chat_id = ChatId(990_001);
        let now_ms = Utc::now().timestamp_millis();
        state
            .db
            .create_user(&crate::db::schema::UserRecord {
                user_id: uid,
                username: Some("typing_guard_u".to_string()),
                phone: None,
                first_name: "TypingGuard".to_string(),
                last_name: None,
                bio: None,
                password_hash: None,
                is_bot: false,
                is_verified: false,
                is_support: false,
                is_scam: false,
                is_fake: false,
                profile_photo_file_id: None,
                account_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            })
            .expect("create user in db");
        state
            .db
            .create_chat(&crate::db::schema::ChatRecord {
                chat_id,
                chat_type: 0,
                peer_type: 1,
                title: "Typing Guard Chat".to_string(),
                username: None,
                about: None,
                creator_id: uid,
                is_forum: false,
                flags: 0,
                photo_file_id: None,
                slowmode_seconds: 0,
                linked_chat_id: None,
                pinned_message_id: None,
                pts: 0,
                last_message_id: gpui_telegram_core::types::MsgId(0),
                last_message_date_ms: now_ms,
                unread_count: 0,
                account_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            })
            .expect("create chat in db");
        let (tx, mut rx) = mpsc::unbounded_channel();
        state.gateway_hub.register(uid, tx);
        state.gateway_hub.subscribe_chat(chat_id.0, uid);

        {
            let _guard = BotTypingGuard::start(Arc::clone(&state), chat_id, bot_id);
        }

        let mut actions = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if let WsServerFrame::Update(UpdateEvent::UserTyping(ev)) = frame {
                actions.push(ev.action);
            }
        }
        assert!(
            actions.contains(&TypingAction::Typing),
            "expected TypingAction::Typing, got {actions:?}"
        );
        assert_eq!(
            actions.last(),
            Some(&TypingAction::Cancel),
            "expected final TypingAction::Cancel on drop, got {actions:?}"
        );

        let (sid, is_yolo) = resolve_and_apply_bot_session_policy(
            &state,
            uid.0,
            bot_id,
            Some(chat_id),
            None,
            "telegram_bot",
            None,
            false,
        )
        .expect("resolve policy");
        assert!(is_yolo, "default bot computer policy AlwaysAllowAuthorized must map to Yolo");
        let mode = state
            .hermes
            .security()
            .read()
            .expect("sec lock")
            .approvals
            .get_session_mode(&sid);
        assert_eq!(mode, ApprovalMode::Yolo);
    }

    #[tokio::test]
    async fn test_hermes_turn_events_sse_omits_done_on_error_and_emits_on_clean_done() {
        use axum::body::to_bytes;
        use tower::ServiceExt;

        let (_dir, state) = test_state("hermes_turn_events_sse");
        let (user, _sess, tok) = state
            .session_manager
            .register("sse_ev_user", "Pass123456!", "SseUser", None, "CLI")
            .unwrap();

        // 1. Trigger a PROVIDER_ERROR turn via run_turn_stream (remove all providers, disable local fallback)
        {
            let provs = state.hermes.providers();
            for p in provs.list_providers() {
                if p.id != "local_hermes" {
                    provs.remove_provider(&p.id);
                }
            }
            provs.set_allow_local_fallback(false);
        }
        let mut req = HermesTurnRequest::new("hello");
        req.skip_slash_commands = true;
        req.user_id = Some(user.user_id.0.to_string());
        req.session_id = Some(format!("sse_ev_sess_{}", user.user_id.0));
        let mut rx = state.hermes.run_turn_stream(req).await.expect("stream start");
        let mut err_turn_id = String::new();
        while let Some(ev) = rx.recv().await {
            if let HermesEvent::Meta { turn_id, .. } = ev {
                err_turn_id = turn_id;
            }
        }
        assert!(!err_turn_id.is_empty());
        // Wait briefly for record_turn_events to settle after channel close
        for _ in 0..50 {
            if !state.hermes.get_turn_events(&err_turn_id).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let app = crate::build_router_with_state(state.clone());
        let resp_err = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/v1/hermes/turns/{err_turn_id}/events?sse=true"))
                    .header("authorization", format!("Bearer {tok}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_err.status(), StatusCode::OK);
        let body_err = String::from_utf8(
            to_bytes(resp_err.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body_err.contains("\"type\":\"error\""), "got: {body_err}");
        assert!(
            !body_err.contains("data: [DONE]"),
            "errored turn SSE must NOT emit [DONE]: {body_err}"
        );

        // 2. Trigger a clean completed turn (allow local_hermes)
        state.hermes.providers().set_allow_local_fallback(true);
        let _ = state
            .hermes
            .providers()
            .set_active("local_hermes", Some("hermes-local-v1"));
        let mut ok_req = HermesTurnRequest::new("hello clean");
        ok_req.skip_slash_commands = true;
        ok_req.user_id = Some(user.user_id.0.to_string());
        ok_req.session_id = Some(format!("sse_ev_sess_{}", user.user_id.0));
        let ok_res = state.hermes.run_turn(ok_req).await.expect("clean turn");
        for _ in 0..50 {
            if !state.hermes.get_turn_events(&ok_res.turn_id).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let resp_ok = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/api/v1/hermes/turns/{}/events?sse=true",
                        ok_res.turn_id
                    ))
                    .header("authorization", format!("Bearer {tok}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_ok.status(), StatusCode::OK);
        let body_ok = String::from_utf8(
            to_bytes(resp_ok.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body_ok.contains("\"type\":\"done\""), "got: {body_ok}");
        assert!(
            body_ok.contains("data: [DONE]"),
            "clean completed turn SSE must emit [DONE]: {body_ok}"
        );
    }

    #[tokio::test]
    async fn test_openai_chat_completions_midstream_client_abort_cancels_upstream() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let upstream_dropped = Arc::new(AtomicUsize::new(0));
        let upstream_dropped_srv = Arc::clone(&upstream_dropped);

        struct DropFlag(Arc<AtomicUsize>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let mock_app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |_body: axum::body::Bytes| {
                let flag = Arc::clone(&upstream_dropped_srv);
                async move {
                    let s = stream::unfold((0u8, Some(DropFlag(flag))), |(step, guard)| async move {
                        if step == 0 {
                            let chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"FIRST_\"}}]}\n\n";
                            Some((
                                Ok::<axum::body::Bytes, Infallible>(axum::body::Bytes::from(chunk)),
                                (1, guard),
                            ))
                        } else {
                            let _hold = guard;
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            None
                        }
                    });
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from_stream(s))
                        .unwrap()
                }
            }),
        );
        let mock_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_addr = mock_listener.local_addr().unwrap();
        let mock_srv = tokio::spawn(async move {
            let _ = axum::serve(mock_listener, mock_app).await;
        });

        let (_dir, state) = test_state("openai_abort");
        let (_user, _sess, tok) = state
            .session_manager
            .register("abort_user", "Pass123456!", "AbortUser", None, "CLI")
            .unwrap();
        {
            let provs = state.hermes.providers();
            for p in provs.list_providers() {
                if p.id != "local_hermes" {
                    provs.remove_provider(&p.id);
                }
            }
            provs.register_provider(ProviderConfig::openai_compatible(
                "mock_ds_abort",
                "Mock DeepSeek Abort",
                format!("http://{mock_addr}/v1"),
                "deepseek-chat",
                "sk-mock",
            ));
            provs
                .set_active("mock_ds_abort", Some("deepseek-chat"))
                .expect("set active");
        }

        let app = crate::build_router_with_state(state);
        let gw_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gw_addr = gw_listener.local_addr().unwrap();
        let gw_srv = tokio::spawn(async move {
            let _ = axum::serve(gw_listener, app).await;
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{gw_addr}/v1/chat/completions"))
            .bearer_auth(&tok)
            .json(&json!({
                "model": "deepseek-chat",
                "stream": true,
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let mut bs = resp.bytes_stream();
        let first = tokio::time::timeout(Duration::from_secs(3), bs.next())
            .await
            .expect("first chunk timeout")
            .expect("first chunk some")
            .expect("first chunk ok");
        let first_str = String::from_utf8_lossy(&first);
        assert!(first_str.contains("FIRST_"), "got {first_str}");

        // Client disconnects mid-stream
        drop(bs);

        tokio::time::timeout(Duration::from_millis(250), async {
            while upstream_dropped.load(Ordering::SeqCst) < 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("upstream DropFlag must fire within 250ms after openai client disconnect");

        gw_srv.abort();
        mock_srv.abort();
    }
}
