use crate::db::miniapp_store::{
    normalize_bot_slug, percent_decode_component, sign_webapp_init_data, verify_webapp_init_data,
    BotRegistryRecord, MiniAppRecord, MiniAppSessionRecord, BOTFATHER_USER_ID,
};
use crate::db::ServerDatabase;
use crate::routes::telegram_bot_api::{
    enqueue_web_app_data_update, enqueue_write_access_allowed_update,
    record_incoming_callback_query_for_bot,
};
use crate::routes::{broadcast_chat_pts, check_active_membership, err_resp, map_telegram_err};
use crate::server::{AppState, AuthBearer};
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json, Response};
use gpui_telegram_agent::fabushi_bot::{
    FabushiBotHost, FabushiBotInstance, FabushiBotPatch, FabushiBotProvisionSpec,
};
use gpui_telegram_agent::hermes::memory::MemoryTarget;
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, UserId};
use gpui_telegram_proto::message::{InlineKeyboardButton, MessageContent, MessageItem};
use gpui_telegram_proto::miniapp_contracts::{
    derive_safe_agent_id, normalize_fabushi_avatar_color, normalize_fabushi_avatar_shape,
    validate_bot_command, validate_bot_username, AcceptMiniAppTosRequest, AllowSendMessageRequest,
    AllowSendMessageResponse, BlockBotRequest, BotCommandSpec, BotPublicProfileDto,
    BotRegistryEntry, BotTokenResponse, CanSendMessageResponse, ChatCallbackRequest,
    CloudStorageSetRequest, CreateBotRequest, CreateFabushiBotRequest, CreateMiniAppRequest,
    FabushiBotCreateResponse, FabushiBotSummaryDto, InstallMiniAppRequest, LaunchMiniAppRequest,
    MiniAppActionButton, MiniAppActionRequest, MiniAppActionResponse, MiniAppEntry,
    MiniAppInitDataDto, MiniAppLaunchContext, MiniAppThemeParams, MiniAppUiSchema,
    MiniAppUiWidget, MiniAppWidgetType, NaturalLanguageCreateRequest,
    NaturalLanguageCreateResponse, PatchFabushiBotRequest, PeerBotActionResult,
    ValidateInitDataRequest, ValidateInitDataResponse, CLASSIC_INITIALS_AVATAR_SHAPE,
    GROK_AVATAR_COLOR_IDS, GROK_AVATAR_SHAPES,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

// ============================================================================
// Shared DTO Conversions
// ============================================================================

pub fn bot_record_to_entry(bot: &BotRegistryRecord) -> BotRegistryEntry {
    let commands: Vec<BotCommandSpec> =
        serde_json::from_str(&bot.commands_json).unwrap_or_default();
    let attach_peer_types: Vec<String> =
        serde_json::from_str(&bot.attach_peer_types_json).unwrap_or_default();
    let agent_id = if bot.agent_id.trim().is_empty() {
        derive_safe_agent_id(None, &bot.username, bot.bot_user_id)
    } else {
        bot.agent_id.clone()
    };
    let avatar_shape = normalize_fabushi_avatar_shape(&bot.avatar_shape, &agent_id);
    let avatar_color = normalize_fabushi_avatar_color(&bot.avatar_color, &agent_id);
    let thinking_level = if bot.thinking_level.trim().is_empty() {
        "medium".to_string()
    } else {
        bot.thinking_level.clone()
    };
    let approval_mode = if bot.approval_mode.trim().is_empty() {
        "prompt".to_string()
    } else {
        bot.approval_mode.clone()
    };
    let toolset_preset = if bot.toolset_preset.trim().is_empty() {
        "full".to_string()
    } else {
        bot.toolset_preset.clone()
    };
    let menu_button_type = if bot.menu_button_type.trim().is_empty() {
        if !bot.menu_button_url.trim().is_empty() {
            "web_app".to_string()
        } else {
            "default".to_string()
        }
    } else {
        bot.menu_button_type.trim().to_string()
    };
    BotRegistryEntry {
        bot_user_id: bot.bot_user_id,
        owner_user_id: bot.owner_user_id,
        username: bot.username.clone(),
        display_name: bot.display_name.clone(),
        description: bot.description.clone(),
        about: bot.about.clone(),
        ai_persona: bot.ai_persona.clone(),
        menu_button_text: bot.menu_button_text.clone(),
        menu_button_url: bot.menu_button_url.clone(),
        is_official: bot.is_official,
        is_inline: bot.is_inline,
        allow_attach_menu: bot.allow_attach_menu,
        attach_peer_types,
        commands,
        created_at: bot.created_at,
        agent_id,
        avatar_shape,
        avatar_color,
        preferred_model: bot.preferred_model.clone(),
        thinking_level,
        approval_mode,
        toolset_preset,
        is_fabushi_bot: bot.bot_user_id != BOTFATHER_USER_ID as i64 && !bot.is_standard_bot,
        menu_button_type,
        has_main_app: bot.has_main_app,
        main_app_short_name: bot.main_app_short_name.clone(),
        privacy_policy_url: bot.privacy_policy_url.clone(),
        has_settings: bot.has_settings,
        can_join_groups: bot.can_join_groups,
        mau_count: 0,
        allows_write_to_pm: false,
        app_tos_accepted: false,
        is_blocked_by_viewer: false,
    }
}

pub(crate) fn bot_record_to_entry_for_viewer(
    state: &AppState,
    bot: &BotRegistryRecord,
    viewer_user_id: i64,
) -> BotRegistryEntry {
    let mut entry = bot_record_to_entry(bot);
    entry.mau_count = state.db.compute_bot_mau_30d(bot.bot_user_id).unwrap_or(0);
    if viewer_user_id > 0 {
        let blocked = state
            .db
            .is_user_blocked(UserId(viewer_user_id as u64), UserId(bot.bot_user_id as u64))
            .unwrap_or(false);
        let perm = state
            .db
            .get_user_bot_permission(viewer_user_id, bot.bot_user_id)
            .ok();
        entry.is_blocked_by_viewer = blocked;
        entry.allows_write_to_pm = !blocked && perm.as_ref().is_some_and(|p| p.allows_write_to_pm);
        entry.app_tos_accepted = perm.as_ref().is_some_and(|p| p.app_tos_accepted_at > 0);
    }
    if !bot.is_official
        && (viewer_user_id <= 0
            || crate::routes::hermes::caller_owns_bot_or_admin(
                state,
                UserId(viewer_user_id as u64),
                bot.bot_user_id.max(0) as u64,
            )
            .is_err())
    {
        entry.ai_persona.clear();
    }
    entry
}

pub(crate) fn provision_fabushi_bot_with_host(
    db: &ServerDatabase,
    host: &FabushiBotHost,
    bot: &BotRegistryRecord,
) -> Option<Arc<FabushiBotInstance>> {
    if bot.bot_user_id == BOTFATHER_USER_ID as i64 || bot.is_standard_bot {
        return None;
    }
    let commands: Vec<BotCommandSpec> =
        serde_json::from_str(&bot.commands_json).unwrap_or_default();
    let persona_desc = if !bot.ai_persona.trim().is_empty() {
        bot.ai_persona.clone()
    } else if !bot.description.trim().is_empty() {
        bot.description.clone()
    } else {
        bot.about.clone()
    };
    let title = if !bot.about.trim().is_empty() {
        bot.about.clone()
    } else if bot.is_official {
        "Official Fabushi Agent".to_string()
    } else {
        "Fabushi Agent".to_string()
    };
    let spec = FabushiBotProvisionSpec {
        bot_user_id: bot.bot_user_id,
        owner_user_id: bot.owner_user_id,
        username: bot.username.clone(),
        display_name: bot.display_name.clone(),
        description: persona_desc,
        title,
        agent_id: (!bot.agent_id.trim().is_empty()).then(|| bot.agent_id.clone()),
        avatar_shape: (!bot.avatar_shape.trim().is_empty()).then(|| bot.avatar_shape.clone()),
        avatar_color: (!bot.avatar_color.trim().is_empty()).then(|| bot.avatar_color.clone()),
        preferred_model: (!bot.preferred_model.trim().is_empty())
            .then(|| bot.preferred_model.clone()),
        thinking_level: (!bot.thinking_level.trim().is_empty()).then(|| bot.thinking_level.clone()),
        approval_mode: (!bot.approval_mode.trim().is_empty()).then(|| bot.approval_mode.clone()),
        toolset_preset: (!bot.toolset_preset.trim().is_empty()).then(|| bot.toolset_preset.clone()),
        commands,
        inline_mode_enabled: bot.is_inline,
    };

    let inst = match host.provision_or_sync_bot(spec) {
        Ok(i) => i,
        Err(err) => {
            tracing::warn!(
                bot_user_id = bot.bot_user_id,
                username = %bot.username,
                "Failed to provision FabushiBotInstance: {}",
                err
            );
            return None;
        }
    };

    if bot.agent_id.trim().is_empty()
        || bot.avatar_shape.trim().is_empty()
        || bot.avatar_color.trim().is_empty()
    {
        let prof = inst.profile();
        let st = inst.settings();
        let _ = db.update_bot_fabushi_fields(
            bot.bot_user_id,
            None,
            None,
            None,
            None,
            Some(&inst.agent_id),
            Some(&prof.avatar_shape),
            Some(&prof.avatar_color),
            None,
            Some(&st.thinking_level),
            Some(&st.approval_mode),
            Some(&st.toolset_preset),
        );
    }

    Some(inst)
}

pub(crate) fn ensure_fabushi_bot_hydrated(
    state: &AppState,
    bot_user_id: i64,
) -> Option<Arc<FabushiBotInstance>> {
    if bot_user_id == BOTFATHER_USER_ID as i64 {
        return None;
    }
    if let Some(inst) = state.fabushi_bots.get_by_user_id(bot_user_id) {
        return Some(inst);
    }
    let rec = state
        .db
        .ensure_bot_registry_for_existing_bot_user(bot_user_id, 0, "")
        .ok()
        .flatten()?;
    if rec.is_standard_bot {
        return None;
    }
    provision_fabushi_bot_with_host(&state.db, &state.fabushi_bots, &rec)
}

pub(crate) fn build_fabushi_bot_summary(
    state: &AppState,
    viewer_user_id: i64,
    bot: &BotRegistryRecord,
    inst_opt: Option<&FabushiBotInstance>,
) -> FabushiBotSummaryDto {
    let hydrated = if inst_opt.is_none() {
        ensure_fabushi_bot_hydrated(state, bot.bot_user_id)
    } else {
        None
    };
    let inst_ref = inst_opt.or(hydrated.as_deref());
    let fallback_agent_id = if bot.agent_id.trim().is_empty() {
        derive_safe_agent_id(None, &bot.username, bot.bot_user_id)
    } else {
        bot.agent_id.clone()
    };
    let (agent_id, display_name, description, title, avatar_shape, avatar_color, preferred_model, thinking_level, approval_mode, toolset_preset, notify_on_agent_updates, hidden_from_sidebar, memory_entries_count, skills_count, execution_state) =
        if let Some(inst) = inst_ref {
            let prof = inst.profile();
            let st = inst.settings();
            let mem_count = inst
                .hermes()
                .memory()
                .read(MemoryTarget::Memory)
                .map(|m| m.entries.len())
                .unwrap_or(0);
            let skills_count = inst.hermes().skills().skills_list(None).len();
            let exec_state = inst.execution_state_label().to_string();
            (
                inst.agent_id.clone(),
                prof.name,
                prof.description,
                prof.title,
                prof.avatar_shape,
                prof.avatar_color,
                st.preferred_model,
                st.thinking_level,
                st.approval_mode,
                st.toolset_preset,
                st.sand.notify_on_agent_updates,
                st.sand.hidden_from_sidebar,
                mem_count,
                skills_count,
                exec_state,
            )
        } else {
            (
                fallback_agent_id.clone(),
                bot.display_name.clone(),
                if !bot.ai_persona.is_empty() {
                    bot.ai_persona.clone()
                } else {
                    bot.description.clone()
                },
                bot.about.clone(),
                normalize_fabushi_avatar_shape(&bot.avatar_shape, &fallback_agent_id),
                normalize_fabushi_avatar_color(&bot.avatar_color, &fallback_agent_id),
                bot.preferred_model.clone(),
                if bot.thinking_level.is_empty() {
                    "medium".to_string()
                } else {
                    bot.thinking_level.clone()
                },
                if bot.approval_mode.is_empty() {
                    "prompt".to_string()
                } else {
                    bot.approval_mode.clone()
                },
                if bot.toolset_preset.is_empty() {
                    "full".to_string()
                } else {
                    bot.toolset_preset.clone()
                },
                true,
                false,
                0,
                0,
                "idle".to_string(),
            )
        };

    let companion_chat_id = if viewer_user_id > 0 {
        state
            .db
            .find_companion_chat_id_for_user(bot.bot_user_id, viewer_user_id)
            .ok()
            .flatten()
            .map(|c| c.0)
    } else {
        None
    };

    FabushiBotSummaryDto {
        bot_user_id: bot.bot_user_id,
        owner_user_id: bot.owner_user_id,
        agent_id,
        username: bot.username.clone(),
        display_name,
        description,
        title,
        avatar_shape,
        avatar_color,
        preferred_model,
        thinking_level,
        approval_mode,
        toolset_preset,
        notify_on_agent_updates,
        hidden_from_sidebar,
        memory_entries_count,
        skills_count,
        execution_state,
        companion_chat_id,
    }
}

pub fn miniapp_record_to_entry(
    state: &AppState,
    app: &MiniAppRecord,
    user_id: i64,
    installed_map: Option<&HashMap<String, (i64, bool)>>,
) -> MiniAppEntry {
    let bot_opt = state.db.get_bot_by_user_id(app.bot_user_id).ok().flatten();
    let bot_display_name = bot_opt
        .as_ref()
        .map(|b| b.display_name.clone())
        .unwrap_or_else(|| app.bot_username.clone());

    let (direct_chat_id, is_installed, install_write_access) = if let Some(map) = installed_map {
        match map.get(&app.app_id) {
            Some(&(cid, write)) => (Some(cid), true, write),
            None => (None, false, false),
        }
    } else {
        let installs = state
            .db
            .list_user_installed_mini_apps(user_id)
            .unwrap_or_default();
        match installs.into_iter().find(|(a, _, _)| a.app_id == app.app_id) {
            Some((_, inst, _)) => (Some(inst.companion_chat_id), true, inst.allow_write_access),
            None => (None, false, false),
        }
    };

    let blocked = if user_id > 0 {
        state
            .db
            .is_user_blocked(UserId(user_id as u64), UserId(app.bot_user_id as u64))
            .unwrap_or(false)
    } else {
        false
    };
    let perm = if user_id > 0 {
        state.db.get_user_bot_permission(user_id, app.bot_user_id).ok()
    } else {
        None
    };
    let has_write_access = !blocked
        && (perm.as_ref().is_some_and(|p| p.allows_write_to_pm)
            || (perm.is_none() && install_write_access));
    let app_tos_accepted = perm.as_ref().is_some_and(|p| p.app_tos_accepted_at > 0);
    let last_launched_opt = if user_id > 0 {
        state
            .db
            .get_mini_app_usage_for_user(user_id, &app.app_id)
            .ok()
            .flatten()
    } else {
        None
    };
    let mau_count = state.db.compute_app_mau_30d(&app.app_id).unwrap_or(0);
    let is_main_app = bot_opt
        .as_ref()
        .is_some_and(|b| b.has_main_app && b.main_app_short_name == app.short_name);
    let privacy_policy_url = bot_opt
        .as_ref()
        .map(|b| b.privacy_policy_url.clone())
        .unwrap_or_default();

    let mut ui_schema: MiniAppUiSchema =
        serde_json::from_str(&app.ui_schema_json).unwrap_or_else(|_| fallback_schema_for_app(app));
    if ui_schema.title.trim().is_empty() {
        ui_schema.title = app.title.clone();
    }
    if ui_schema.subtitle.trim().is_empty() {
        ui_schema.subtitle = app.description.clone();
    }
    if let Ok(Some(sess)) = state.db.get_latest_mini_app_session_for_user(user_id, &app.app_id) {
        if let Ok(st) = serde_json::from_str::<Value>(&sess.state_json) {
            ui_schema = sync_ui_schema_with_state(&ui_schema, &st);
        }
    }

    let peer_types: Vec<String> =
        serde_json::from_str(&app.peer_types_json).unwrap_or_else(|_| vec!["users".into()]);

    MiniAppEntry {
        app_id: app.app_id.clone(),
        bot_user_id: app.bot_user_id,
        bot_username: app.bot_username.clone(),
        bot_display_name,
        short_name: app.short_name.clone(),
        title: app.title.clone(),
        description: app.description.clone(),
        category: app.category.clone(),
        icon_emoji: app.icon_emoji.clone(),
        web_url: app.web_url.clone(),
        ui_schema,
        peer_types,
        install_count: app.install_count,
        is_installed,
        has_write_access,
        direct_chat_id,
        created_by_nl: app.created_by_nl,
        updated_at: app.updated_at,
        mau_count,
        is_main_app,
        is_recent_used: last_launched_opt.is_some(),
        last_launched_at: last_launched_opt.unwrap_or(0),
        privacy_policy_url,
        app_tos_accepted,
        header_color: if app.header_color.trim().is_empty() {
            None
        } else {
            Some(app.header_color.clone())
        },
        background_color: if app.background_color.trim().is_empty() {
            None
        } else {
            Some(app.background_color.clone())
        },
    }
}

fn fallback_schema_for_app(app: &MiniAppRecord) -> MiniAppUiSchema {
    MiniAppUiSchema {
        title: app.title.clone(),
        subtitle: app.description.clone(),
        theme: MiniAppThemeParams::default(),
        widgets: vec![MiniAppUiWidget {
            id: "ai_summary".to_string(),
            kind: MiniAppWidgetType::Text,
            label: "小程序状态".to_string(),
            value: app.description.clone(),
            variant: Some("callout".to_string()),
            ..Default::default()
        }],
        main_button: Some(MiniAppActionButton {
            text: format!("打开 {}", app.title),
            is_visible: true,
            is_active: true,
            action_id: "submit".to_string(),
            ..Default::default()
        }),
        secondary_button: Some(MiniAppActionButton {
            text: "发送结果到伴生机器人".to_string(),
            is_visible: true,
            is_active: true,
            action_id: "send_data".to_string(),
            sends_data: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ============================================================================
// BotFather & Companion Bot Onboarding Content / Keyboards
// ============================================================================

pub fn build_botfather_welcome_text() -> String {
    [
        "🤖 欢迎来到官方 **@BotFather（机器人之父）**！",
        "",
        "你可以用 **1:1 Telegram 标准命令** 创建和管理机器人与小程序：",
        "• `/newbot` — 创建标准 Telegram HTTP Bot API 机器人",
        "• `/newapp` — 为机器人创建并挂载 Mini App 小程序",
        "• `/mybots` / `/myapps` — 查看与管理我的机器人和小程序",
        "• `/token` / `/revoke` — 查看或重置 Bot API Token",
        "• `/setname` / `/setdescription` / `/setabouttext` / `/setcommands`",
        "• `/setmenubutton` / `/setmainapp` / `/setprivacypolicy` / `/setinline` / `/setavatar`",
        "• `/deletebot` / `/deleteapp` | `/cancel` — 取消当前步骤 | `/help` — 查看帮助",
        "",
        "✨ **超越 Telegram：一句话自然语言直出 AI 机器人 + 小程序**",
        "直接给我发一句话需求，我将同时为你生成可用的 AI 机器人（Bot Token）与可交互的 Mini App 小程序：",
        "👉 例如：`帮我创建一个咖啡店点单助手机器人和点单小程序，支持美式和拿铁下单并总结订单`",
    ]
    .join("\n")
}

pub fn build_botfather_welcome_keyboard() -> Vec<Vec<InlineKeyboardButton>> {
    vec![
        vec![
            InlineKeyboardButton {
                text: "🤖 新建机器人 (/newbot)".to_string(),
                callback_data: Some("bf:/newbot".to_string()),
                url: None,
                web_app_url: None,
            },
            InlineKeyboardButton {
                text: "🧩 新建小程序 (/newapp)".to_string(),
                callback_data: Some("bf:/newapp".to_string()),
                url: None,
                web_app_url: None,
            },
        ],
        vec![
            InlineKeyboardButton {
                text: "📋 我的机器人 (/mybots)".to_string(),
                callback_data: Some("bf:/mybots".to_string()),
                url: None,
                web_app_url: None,
            },
            InlineKeyboardButton {
                text: "📱 我的小程序 (/myapps)".to_string(),
                callback_data: Some("bf:/myapps".to_string()),
                url: None,
                web_app_url: None,
            },
        ],
        vec![InlineKeyboardButton {
            text: "✨ 一句话 AI 创建机器人+小程序".to_string(),
            callback_data: Some("bf:nl_guide".to_string()),
            url: None,
            web_app_url: None,
        }],
    ]
}

pub fn build_companion_bot_welcome_text_and_keyboard(
    bot_title: &str,
    bot_username: &str,
    bot_desc: &str,
    apps: &[MiniAppRecord],
) -> (String, Vec<Vec<InlineKeyboardButton>>) {
    let mut lines = vec![format!("👋 你好！我是 **{}** (`@{}`)。", bot_title, bot_username)];
    if !bot_desc.trim().is_empty() {
        lines.push(bot_desc.trim().to_string());
    }
    if !apps.is_empty() {
        lines.push(String::new());
        lines.push("📱 **已关联伴生小程序（Mini App）**：".to_string());
        for a in apps {
            lines.push(format!(
                "• {} **{}** (`t.me/{}/{}`) — {}",
                a.icon_emoji, a.title, a.bot_username, a.short_name, a.description
            ));
        }
        lines.push("你可以点击下方按钮直接启动小程序，也可以直接向我提问或发送 `/app`、`/help`！".to_string());
    } else {
        lines.push("我已接入 Hermes AI 大脑，随时可以与你自然语言对话，也可以发送 `/help` 查看指令。".to_string());
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for a in apps.iter().take(2) {
        rows.push(vec![InlineKeyboardButton {
            text: format!("{} 打开 {}", a.icon_emoji, a.title),
            callback_data: None,
            url: None,
            web_app_url: Some(format!("gpui-miniapp://{}/{}", a.bot_username, a.short_name)),
        }]);
    }
    rows.push(vec![
        InlineKeyboardButton {
            text: "✨ /help".to_string(),
            callback_data: Some("/help".to_string()),
            url: None,
            web_app_url: None,
        },
        InlineKeyboardButton {
            text: "🎙️ Talk Mode".to_string(),
            callback_data: Some("talk:start".to_string()),
            url: None,
            web_app_url: None,
        },
    ]);
    (lines.join("\n"), rows)
}

// ============================================================================
// Natural Language Blueprint Synthesizer (Schema-Native + Interactive)
// ============================================================================

#[derive(Debug, Clone)]
pub struct NlBlueprint {
    pub bot_display_name: String,
    pub bot_username_base: String,
    pub bot_description: String,
    pub bot_about: String,
    pub ai_persona: String,
    pub commands: Vec<BotCommandSpec>,
    pub app_short_name: String,
    pub app_title: String,
    pub app_description: String,
    pub app_category: String,
    pub app_icon: String,
    pub ui_schema: MiniAppUiSchema,
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn derive_ascii_bot_base(prompt: &str, domain_prefix: &str) -> String {
    let mut tokens: Vec<String> = prompt
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| s.len() >= 2)
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| !matches!(s.as_str(), "ai" | "bot" | "app" | "mini" | "create" | "build" | "make"))
        .take(2)
        .collect();
    if tokens.is_empty() {
        tokens.push(domain_prefix.to_string());
    }
    let mut joined = tokens.join("_");
    if !joined.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        joined = format!("{domain_prefix}_{joined}");
    }
    if !joined.ends_with("bot") {
        joined.push_str("_bot");
    }
    if joined.len() < 6 {
        joined = format!("{domain_prefix}_ai_bot");
    }
    joined.chars().take(26).collect()
}

fn extract_clean_subject(prompt: &str, fallback: &str) -> String {
    let mut cleaned = prompt.trim().to_string();
    for prefix in [
        "/aibuild",
        "请帮我创建一个",
        "帮我创建一个",
        "帮我创建",
        "帮我做一个",
        "帮我做个",
        "创建一个",
        "新建一个",
        "做一个",
        "生成",
        "create a ",
        "build a ",
        "make a ",
    ] {
        if let Some(rest) = cleaned.strip_prefix(prefix) {
            cleaned = rest.trim().to_string();
            break;
        }
    }
    for sep in ['，', ',', '。', '；', ';', '\n'] {
        if let Some((head, _)) = cleaned.split_once(sep) {
            if head.chars().count() >= 2 {
                cleaned = head.trim().to_string();
                break;
            }
        }
    }
    for tail in [
        "助手机器人和点单小程序",
        "助手机器人和小程序",
        "机器人和点单小程序",
        "机器人和小程序",
        "机器人与小程序",
        "ai机器人",
        "机器人",
        "小程序",
        "mini app",
        "bot",
    ] {
        if let Some(pos) = cleaned.to_lowercase().find(tail) {
            let head = cleaned[..pos].trim();
            if head.chars().count() >= 2 {
                cleaned = head.to_string();
                break;
            }
        }
    }
    if cleaned.chars().count() < 2 || cleaned.chars().count() > 28 {
        fallback.to_string()
    } else {
        cleaned
    }
}

pub fn synthesize_nl_blueprint(prompt: &str) -> NlBlueprint {
    let lower = prompt.to_lowercase();
    let trimmed = prompt.trim();

    if contains_any(
        &lower,
        &["咖啡", "点单", "奶茶", "饮品", "外卖", "餐厅", "coffee", "cafe", "latte", "order"],
    ) {
        let mut items: Vec<String> = Vec::new();
        if lower.contains("美式") || lower.contains("americano") {
            items.push("经典美式 ¥22".to_string());
        }
        if lower.contains("拿铁") || lower.contains("latte") {
            items.push("丝绒拿铁 ¥28".to_string());
        }
        if lower.contains("卡布") || lower.contains("cappuccino") {
            items.push("香浓卡布 ¥28".to_string());
        }
        if lower.contains("生椰") {
            items.push("生椰拿铁 ¥30".to_string());
        }
        if items.is_empty() {
            items = vec![
                "经典美式 ¥22".to_string(),
                "丝绒拿铁 ¥28".to_string(),
                "生椰冷萃 ¥32".to_string(),
            ];
        }
        let subject = extract_clean_subject(trimmed, "精品咖啡点单");
        let bot_name = if subject.contains("助手") {
            subject.clone()
        } else {
            format!("{}助手", subject)
        };
        let app_title = format!("{}小程序", subject);
        let first_item = items[0].clone();
        let schema = MiniAppUiSchema {
            title: app_title.clone(),
            subtitle: format!("由 AI 一句话生成 · 实时点单与订单汇总 ({})", trimmed),
            theme: MiniAppThemeParams {
                button_color: "#92400e".to_string(),
                ..Default::default()
            },
            widgets: vec![
                MiniAppUiWidget {
                    id: "selected_item".to_string(),
                    kind: MiniAppWidgetType::Select,
                    label: "招牌饮品菜单".to_string(),
                    value: first_item,
                    options: items,
                    ..Default::default()
                },
                MiniAppUiWidget {
                    id: "temperature".to_string(),
                    kind: MiniAppWidgetType::Select,
                    label: "温度与甜度".to_string(),
                    value: "冰 · 标准糖".to_string(),
                    options: vec![
                        "冰 · 标准糖".to_string(),
                        "热 · 少糖".to_string(),
                        "去冰 · 无糖".to_string(),
                    ],
                    ..Default::default()
                },
                MiniAppUiWidget {
                    id: "quantity".to_string(),
                    kind: MiniAppWidgetType::Counter,
                    label: "点单杯数".to_string(),
                    value: "1 杯".to_string(),
                    count: 1,
                    min: Some(1),
                    max: Some(20),
                    ..Default::default()
                },
                MiniAppUiWidget {
                    id: "note".to_string(),
                    kind: MiniAppWidgetType::TextInput,
                    label: "口味备注".to_string(),
                    value: String::new(),
                    placeholder: Some("如：双份浓缩 / 燕麦奶".to_string()),
                    ..Default::default()
                },
                MiniAppUiWidget {
                    id: "order_stats".to_string(),
                    kind: MiniAppWidgetType::MetricCard,
                    label: "已点总计".to_string(),
                    value: "¥0.00".to_string(),
                    subtitle: Some("共 0 笔订单".to_string()),
                    count: 0,
                    ..Default::default()
                },
                MiniAppUiWidget {
                    id: "ai_summary".to_string(),
                    kind: MiniAppWidgetType::Text,
                    label: "订单与 AI 店员总结".to_string(),
                    value: "请选择饮品与温度后点击「立即确认下单」，AI 店员将为你汇总订单。".to_string(),
                    variant: Some("callout".to_string()),
                    ..Default::default()
                },
            ],
            main_button: Some(MiniAppActionButton {
                text: "立即确认下单".to_string(),
                color: Some("#92400e".to_string()),
                text_color: Some("#ffffff".to_string()),
                is_visible: true,
                is_active: true,
                action_id: "submit_order".to_string(),
                sends_data: false,
                ..Default::default()
            }),
            secondary_button: Some(MiniAppActionButton {
                text: "发送订单总结到对话".to_string(),
                is_visible: true,
                is_active: true,
                action_id: "send_order_summary".to_string(),
                sends_data: true,
                ..Default::default()
            }),
            copilot_suggestions: vec![
                "总结我今天已下的所有咖啡订单".to_string(),
                "推荐一杯适合下午提神的饮品".to_string(),
            ],
            ..Default::default()
        };
        return NlBlueprint {
            bot_display_name: bot_name.clone(),
            bot_username_base: derive_ascii_bot_base(trimmed, "coffee_order"),
            bot_description: format!("{}：支持自主下单、推荐口味并总结今日订单。", bot_name),
            bot_about: format!("AI 生成点单机器人：{}", trimmed),
            ai_persona: format!(
                "你是「{}」AI 点单助手。原始需求：{}。你可以推荐菜单（例如美式、拿铁），并结合用户在小程序中的实时下单数据总结每杯饮品、总价与备注。",
                bot_name, trimmed
            ),
            commands: vec![
                BotCommandSpec { command: "start".into(), description: "开始点单并打开伴生小程序".into() },
                BotCommandSpec { command: "app".into(), description: "启动点单小程序".into() },
                BotCommandSpec { command: "menu".into(), description: "查看精选菜单".into() },
                BotCommandSpec { command: "orders".into(), description: "AI 总结当前订单".into() },
                BotCommandSpec { command: "help".into(), description: "查看帮助".into() },
            ],
            app_short_name: "order".into(),
            app_title,
            app_description: format!("{} — 实时选品、定制甜度温度并自动汇总订单", subject),
            app_category: "commerce".into(),
            app_icon: "☕".into(),
            ui_schema: schema,
        };
    }

    if contains_any(
        &lower,
        &["待办", "任务", "清单", "打卡", "todo", "task", "habit", "checklist"],
    ) {
        let subject = extract_clean_subject(trimmed, "智能待办清单");
        let app_title = format!("{}小程序", subject);
        let schema = MiniAppUiSchema {
            title: app_title.clone(),
            subtitle: format!("任务管理 + AI 优先级规划 ({})", trimmed),
            theme: MiniAppThemeParams {
                button_color: "#059669".to_string(),
                ..Default::default()
            },
            widgets: vec![
                MiniAppUiWidget {
                    id: "new_task".to_string(),
                    kind: MiniAppWidgetType::TextInput,
                    label: "新增待办任务".to_string(),
                    value: "整理今日关键交付项".to_string(),
                    placeholder: Some("输入待办任务...".to_string()),
                    ..Default::default()
                },
                MiniAppUiWidget {
                    id: "priority".to_string(),
                    kind: MiniAppWidgetType::Select,
                    label: "优先级".to_string(),
                    value: "高优先级".to_string(),
                    options: vec!["高优先级".into(), "普通".into(), "可延后".into()],
                    ..Default::default()
                },
                MiniAppUiWidget {
                    id: "todo_stats".to_string(),
                    kind: MiniAppWidgetType::MetricCard,
                    label: "待办总数".to_string(),
                    value: "1 项".to_string(),
                    subtitle: Some("优先处理高优任务".to_string()),
                    count: 1,
                    ..Default::default()
                },
                MiniAppUiWidget {
                    id: "ai_summary".to_string(),
                    kind: MiniAppWidgetType::Text,
                    label: "清单与 AI 建议".to_string(),
                    value: "当前有 1 项待办任务，建议先集中完成高优先级事项。".to_string(),
                    variant: Some("callout".to_string()),
                    ..Default::default()
                },
            ],
            main_button: Some(MiniAppActionButton {
                text: "添加待办任务".to_string(),
                color: Some("#059669".to_string()),
                text_color: Some("#ffffff".to_string()),
                is_visible: true,
                is_active: true,
                action_id: "add_todo".to_string(),
                ..Default::default()
            }),
            secondary_button: Some(MiniAppActionButton {
                text: "AI 规划并同步到聊天".to_string(),
                is_visible: true,
                is_active: true,
                action_id: "send_todo_summary".to_string(),
                sends_data: true,
                ..Default::default()
            }),
            copilot_suggestions: vec!["按优先级帮我排序今天所有任务".into()],
            ..Default::default()
        };
        return NlBlueprint {
            bot_display_name: format!("{}助手", subject),
            bot_username_base: derive_ascii_bot_base(trimmed, "todo_coach"),
            bot_description: format!("{}：追踪待办任务并使用 AI 自动排优先级。", subject),
            bot_about: format!("AI 生成待办助手：{}", trimmed),
            ai_persona: format!(
                "你是「{}」AI 效率管家。用户需求：{}。随时读取用户的待办小程序状态，协助拆解任务、排期和复盘。",
                subject, trimmed
            ),
            commands: vec![
                BotCommandSpec { command: "start".into(), description: "打开效率助手".into() },
                BotCommandSpec { command: "app".into(), description: "打开待办小程序".into() },
                BotCommandSpec { command: "summary".into(), description: "汇总待办状态".into() },
                BotCommandSpec { command: "help".into(), description: "帮助".into() },
            ],
            app_short_name: "tasks".into(),
            app_title,
            app_description: format!("{} — 高效任务清单与 AI 执行排序", subject),
            app_category: "productivity".into(),
            app_icon: "✅".into(),
            ui_schema: schema,
        };
    }

    if contains_any(
        &lower,
        &["记账", "账单", "分账", "报销", "预算", "expense", "bill", "split", "ledger"],
    ) {
        let subject = extract_clean_subject(trimmed, "智能记账");
        let app_title = format!("{}小程序", subject);
        let mut schema: MiniAppUiSchema =
            serde_json::from_value(crate::db::miniapp_store::default_splitbill_ui_schema())
                .unwrap_or_default();
        schema.title = app_title.clone();
        schema.subtitle = format!("由 BotFather AI 生成 · {}", trimmed);
        return NlBlueprint {
            bot_display_name: format!("{}助手", subject),
            bot_username_base: derive_ascii_bot_base(trimmed, "smart_ledger"),
            bot_description: format!("{}：随时记一笔、分摊与 AI 财务汇总。", subject),
            bot_about: format!("AI 生成记账机器人：{}", trimmed),
            ai_persona: format!(
                "你是「{}」AI 财务助手。需求背景：{}。读取用户记账小程序明细并快速统计收支与 AA 分账。",
                subject, trimmed
            ),
            commands: vec![
                BotCommandSpec { command: "start".into(), description: "启动财务助手".into() },
                BotCommandSpec { command: "app".into(), description: "打开记账小程序".into() },
                BotCommandSpec { command: "ledger".into(), description: "查看最新账本汇总".into() },
                BotCommandSpec { command: "help".into(), description: "帮助".into() },
            ],
            app_short_name: "ledger".into(),
            app_title,
            app_description: format!("{} — 一键记账分账与账单总结", subject),
            app_category: "finance".into(),
            app_icon: "💸".into(),
            ui_schema: schema,
        };
    }

    if contains_any(
        &lower,
        &["投票", "问卷", "调研", "报名", "form", "poll", "vote", "survey", "rsvp"],
    ) {
        let subject = extract_clean_subject(trimmed, "快捷投票调研");
        let app_title = format!("{}小程序", subject);
        let mut schema: MiniAppUiSchema =
            serde_json::from_value(crate::db::miniapp_store::default_pollmaster_ui_schema())
                .unwrap_or_default();
        schema.title = app_title.clone();
        schema.subtitle = format!("由 BotFather AI 生成 · {}", trimmed);
        return NlBlueprint {
            bot_display_name: format!("{}助手", subject),
            bot_username_base: derive_ascii_bot_base(trimmed, "poll_helper"),
            bot_description: format!("{}：收集选项投票并用 AI 归纳团队意见。", subject),
            bot_about: format!("AI 生成投票助手：{}", trimmed),
            ai_persona: format!(
                "你是「{}」AI 调研分析助手。需求：{}。根据小程序投票票数给出清晰结论。",
                subject, trimmed
            ),
            commands: vec![
                BotCommandSpec { command: "start".into(), description: "启动投票助手".into() },
                BotCommandSpec { command: "app".into(), description: "打开投票小程序".into() },
                BotCommandSpec { command: "summary".into(), description: "AI 统计票数结论".into() },
                BotCommandSpec { command: "help".into(), description: "帮助".into() },
            ],
            app_short_name: "poll".into(),
            app_title,
            app_description: format!("{} — 轻量投票与实时统计看板", subject),
            app_category: "team".into(),
            app_icon: "📊".into(),
            ui_schema: schema,
        };
    }

    let subject = extract_clean_subject(trimmed, "智能工作台");
    let bot_name = if subject.ends_with("助手") || subject.to_lowercase().ends_with("assistant") {
        subject.clone()
    } else {
        format!("{} AI 助手", subject)
    };
    let app_title = format!("{}工作台", subject);
    let schema = MiniAppUiSchema {
        title: app_title.clone(),
        subtitle: format!("由 BotFather AI 定制生成 · {}", trimmed),
        theme: MiniAppThemeParams {
            button_color: "#2563eb".to_string(),
            ..Default::default()
        },
        widgets: vec![
            MiniAppUiWidget {
                id: "input_text".to_string(),
                kind: MiniAppWidgetType::TextInput,
                label: "输入内容 / 需求".to_string(),
                value: String::new(),
                placeholder: Some("填写数据或想让 AI 处理的内容...".to_string()),
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "mode".to_string(),
                kind: MiniAppWidgetType::Select,
                label: "处理模式".to_string(),
                value: "标准模式".to_string(),
                options: vec!["标准模式".into(), "深度分析".into(), "精简总结".into()],
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "record_stats".to_string(),
                kind: MiniAppWidgetType::MetricCard,
                label: "累计处理记录".to_string(),
                value: "0 条".to_string(),
                count: 0,
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "ai_summary".to_string(),
                kind: MiniAppWidgetType::Text,
                label: "实时状态与 AI 输出".to_string(),
                value: format!("已为你初始化「{}」交互空间。提交记录后可召唤 AI 实时分析并同步至聊天。", subject),
                variant: Some("callout".to_string()),
                ..Default::default()
            },
        ],
        main_button: Some(MiniAppActionButton {
            text: "保存并让 AI 处理".to_string(),
            color: Some("#2563eb".to_string()),
            text_color: Some("#ffffff".to_string()),
            is_visible: true,
            is_active: true,
            action_id: "submit_studio".to_string(),
            ..Default::default()
        }),
        secondary_button: Some(MiniAppActionButton {
            text: "发送结果到伴生机器人聊天".to_string(),
            is_visible: true,
            is_active: true,
            action_id: "send_studio_summary".to_string(),
            sends_data: true,
            ..Default::default()
        }),
        copilot_suggestions: vec!["用 AI 总结并输出可执行建议".into()],
        ..Default::default()
    };

    NlBlueprint {
        bot_display_name: bot_name.clone(),
        bot_username_base: derive_ascii_bot_base(trimmed, "custom_ai"),
        bot_description: format!("根据指令定制的 AI 机器人与小程序：{}", trimmed),
        bot_about: format!("AI 生成专属助手：{}", subject),
        ai_persona: format!(
            "你是「{}」专属 AI 助手，最初由用户通过自然语言需求创建：「{}」。你同时掌控伴生小程序的数据状态，能随时结合用户提交的数据提供专业回复。",
            bot_name, trimmed
        ),
        commands: vec![
            BotCommandSpec { command: "start".into(), description: "开始对话并了解能力".into() },
            BotCommandSpec { command: "app".into(), description: "打开专属工作台小程序".into() },
            BotCommandSpec { command: "summary".into(), description: "汇总小程序最新数据".into() },
            BotCommandSpec { command: "help".into(), description: "使用帮助".into() },
        ],
        app_short_name: "app".into(),
        app_title,
        app_description: format!("为「{}」定制的可交互 Mini App", subject),
        app_category: "ai".into(),
        app_icon: "✨".into(),
        ui_schema: schema,
    }
}

pub fn execute_nl_create(
    state: &Arc<AppState>,
    owner_user_id: UserId,
    prompt: &str,
) -> Result<(BotRegistryRecord, MiniAppRecord), String> {
    execute_nl_create_with_avatar(state, owner_user_id, prompt, None, None)
}

pub fn execute_nl_create_with_avatar(
    state: &Arc<AppState>,
    owner_user_id: UserId,
    prompt: &str,
    avatar_shape: Option<&str>,
    avatar_color: Option<&str>,
) -> Result<(BotRegistryRecord, MiniAppRecord), String> {
    let bp = synthesize_nl_blueprint(prompt);
    let base_stem = bp
        .bot_username_base
        .strip_suffix("_bot")
        .unwrap_or(&bp.bot_username_base)
        .chars()
        .take(22)
        .collect::<String>();

    let mut chosen_username = bp.bot_username_base.clone();
    for idx in 0..50 {
        let candidate = if idx == 0 {
            chosen_username.clone()
        } else {
            format!("{}_{}_bot", base_stem, idx + 1)
        };
        if state
            .db
            .get_user_by_username(&candidate)
            .ok()
            .flatten()
            .is_none()
        {
            chosen_username = candidate;
            break;
        }
    }

    let commands_json = serde_json::to_string(&bp.commands).unwrap_or_else(|_| "[]".into());
    let menu_url = format!("/miniapps/{}/{}", chosen_username, bp.app_short_name);
    let menu_text = format!("打开 {}", bp.app_title);

    let mut bot = state
        .db
        .create_registered_bot(
            owner_user_id.0 as i64,
            &chosen_username,
            &bp.bot_display_name,
            &bp.bot_description,
            &bp.bot_about,
            &bp.ai_persona,
            &menu_text,
            &menu_url,
            &commands_json,
            true,
            true,
        )
        .map_err(|e| e.to_string())?;

    if avatar_shape.is_some() || avatar_color.is_some() {
        if let Ok(updated) = state.db.update_bot_fabushi_fields(
            bot.bot_user_id,
            None,
            None,
            None,
            None,
            None,
            avatar_shape,
            avatar_color,
            None,
            None,
            None,
            None,
        ) {
            bot = updated;
        }
    }

    crate::routes::hermes::save_bot_persona_kv(
        state,
        UserId(bot.bot_user_id as u64),
        None,
        &bot.display_name,
        Some(&bot.ai_persona),
        Some(&bot.ai_persona),
        None,
        None,
    );
    let _ = provision_fabushi_bot_with_host(&state.db, &state.fabushi_bots, &bot);

    let schema_str = serde_json::to_string(&bp.ui_schema).unwrap_or_else(|_| "{}".into());
    let app = state
        .db
        .create_or_upsert_mini_app(
            bot.bot_user_id,
            &bot.username,
            &bp.app_short_name,
            &bp.app_title,
            &bp.app_description,
            &bp.app_category,
            &bp.app_icon,
            Some(&menu_url),
            &schema_str,
            true,
            true,
            true,
        )
        .map_err(|e| e.to_string())?;

    Ok((bot, app))
}

fn should_trigger_nl_from_text(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    if lower.starts_with("/aibuild") {
        return true;
    }
    if lower.starts_with('/') || lower.chars().count() < 6 {
        return false;
    }
    contains_any(
        &lower,
        &[
            "创建",
            "新建",
            "帮我",
            "做个",
            "做一个",
            "生成",
            "打造",
            "开发",
            "点单",
            "记账",
            "投票",
            "待办",
            "小程序",
            "机器人",
            "助手",
            "create",
            "build",
            "make",
            "bot",
            "mini app",
            "miniapp",
        ],
    )
}

// ============================================================================
// BotFather Full Command FSM & Callback Handler
// ============================================================================

fn format_bot_pick_keyboard(
    action_prefix: &str,
    bots: &[BotRegistryRecord],
) -> Vec<Vec<InlineKeyboardButton>> {
    bots.iter()
        .take(8)
        .map(|b| {
            vec![InlineKeyboardButton {
                text: format!("@{} ({})", b.username, b.display_name),
                callback_data: Some(format!("bf:{}:{}", action_prefix, b.username)),
                url: None,
                web_app_url: None,
            }]
        })
        .collect()
}

fn format_nl_created_reply(
    bot: &BotRegistryRecord,
    app: &MiniAppRecord,
) -> (String, Vec<Vec<InlineKeyboardButton>>) {
    let text = [
        "🎉 **已为你用自然语言一键生成 AI 机器人 + 伴生小程序！**".to_string(),
        String::new(),
        format!("🤖 **AI 机器人**：{} (`@{}`)", bot.display_name, bot.username),
        format!("🔑 **Bot Token**：`{}`", bot.bot_token),
        format!("📝 **AI 人设**：{}", bot.description),
        String::new(),
        format!("🧩 **伴生小程序**：{} **{}**", app.icon_emoji, app.title),
        format!(
            "🔗 **小程序直达**：`t.me/{}/{}` (`gpui-miniapp://{}/{}`)",
            bot.username, app.short_name, bot.username, app.short_name
        ),
        format!("🌐 **WebApp URL**：`{}`", app.web_url),
        String::new(),
        "你可以点击下方按钮直接启动小程序，或打开伴生 AI 机器人对话测试效果！".to_string(),
    ]
    .join("\n");

    let kb = vec![
        vec![InlineKeyboardButton {
            text: format!("{} 打开小程序: {}", app.icon_emoji, app.title),
            callback_data: None,
            url: None,
            web_app_url: Some(format!("gpui-miniapp://{}/{}", bot.username, app.short_name)),
        }],
        vec![
            InlineKeyboardButton {
                text: format!("💬 打开 @{} AI 对话", bot.username),
                callback_data: Some(format!("bf:open_bot:{}", bot.username)),
                url: None,
                web_app_url: None,
            },
            InlineKeyboardButton {
                text: "📋 我的机器人".to_string(),
                callback_data: Some("bf:/mybots".to_string()),
                url: None,
                web_app_url: None,
            },
        ],
    ];
    (text, kb)
}

fn parse_commands_text(input: &str) -> Option<String> {
    let mut cmds = Vec::new();
    for line in input.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        let (raw_cmd, raw_desc) = if let Some((c, d)) = l.split_once(" - ") {
            (c, d)
        } else if let Some((c, d)) = l.split_once('-') {
            (c, d)
        } else if let Some((c, d)) = l.split_once(':') {
            (c, d)
        } else if let Some((c, d)) = l.split_once(' ') {
            (c, d)
        } else {
            (l, "Custom command")
        };
        if let Ok(cmd) = validate_bot_command(&raw_cmd.to_ascii_lowercase()) {
            let desc = raw_desc.trim();
            if !desc.is_empty() {
                cmds.push(BotCommandSpec {
                    command: cmd,
                    description: desc.to_string(),
                });
            }
        }
    }
    if cmds.is_empty() {
        None
    } else {
        serde_json::to_string(&cmds).ok()
    }
}

pub async fn handle_botfather_message(
    state: &Arc<AppState>,
    chat_id: ChatId,
    sender_id: UserId,
    raw_text: &str,
) -> (String, Vec<Vec<InlineKeyboardButton>>) {
    let text = raw_text.trim();
    let uid = sender_id.0 as i64;
    let cid = chat_id.0;
    let lower = text.to_ascii_lowercase();

    if lower == "/cancel" {
        let _ = state.db.clear_botfather_session(uid);
        return (
            "Current operation has been cancelled. 发送 `/newbot`、`/newapp` 或直接用一句话自然语言创建 AI 机器人与小程序。".to_string(),
            build_botfather_welcome_keyboard(),
        );
    }
    if lower == "/start" || lower == "/help" {
        let _ = state.db.clear_botfather_session(uid);
        return (build_botfather_welcome_text(), build_botfather_welcome_keyboard());
    }
    if lower.starts_with("/aibuild") {
        let _ = state.db.clear_botfather_session(uid);
        let prompt = text["/aibuild".len()..].trim();
        if prompt.is_empty() {
            return (
                "请直接告诉我你想创建什么！例如：`/aibuild 帮我创建一个咖啡店点单助手机器人和点单小程序，支持美式和拿铁下单并总结订单`"
                    .to_string(),
                Vec::new(),
            );
        }
        return match execute_nl_create(state, sender_id, prompt) {
            Ok((bot, app)) => format_nl_created_reply(&bot, &app),
            Err(err) => (format!("❌ AI 创建失败: {}", err), Vec::new()),
        };
    }
    if lower == "/newbot" {
        let _ = state
            .db
            .upsert_botfather_session(uid, cid, "awaiting_bot_name", "{}");
        return (
            "Alright, a new bot. How are we going to call it? Please choose a display name for your bot.\n\n好！我们要创建一个新机器人。请发送它的**显示名称**（例如：`Coffee Helper` 或 `咖啡点单助手`）。".to_string(),
            Vec::new(),
        );
    }
    if lower == "/newapp" {
        let bots = state.db.list_bots_by_owner(uid).unwrap_or_default();
        if bots.is_empty() {
            return (
                "你还没有创建自己的机器人。请先发送 `/newbot` 创建一个机器人，或直接用自然语言一句话同时创建机器人+小程序！".to_string(),
                build_botfather_welcome_keyboard(),
            );
        }
        let _ = state
            .db
            .upsert_botfather_session(uid, cid, "awaiting_app_bot", "{}");
        return (
            "请选择或发送要挂载 Mini App 的机器人 `@username`：".to_string(),
            format_bot_pick_keyboard("newapp_for", &bots),
        );
    }
    if lower == "/mybots" {
        let _ = state.db.clear_botfather_session(uid);
        let bots = state.db.list_bots_by_owner(uid).unwrap_or_default();
        if bots.is_empty() {
            return (
                "你名下还没有机器人。点击下方按钮发送 `/newbot` 创建，或直接输入自然语言一键生成！".to_string(),
                build_botfather_welcome_keyboard(),
            );
        }
        let mut lines = vec![format!("📋 **你名下共有 {} 个机器人**：", bots.len())];
        for b in &bots {
            lines.push(format!(
                "• **{}** (`@{}`) — inline: {} | `/token` `/setname` `/deletebot`",
                b.display_name, b.username, b.is_inline
            ));
        }
        let mut kb = format_bot_pick_keyboard("manage_bot", &bots);
        kb.push(vec![InlineKeyboardButton {
            text: "🤖 新建机器人".to_string(),
            callback_data: Some("bf:/newbot".to_string()),
            url: None,
            web_app_url: None,
        }]);
        return (lines.join("\n"), kb);
    }
    if lower == "/myapps" {
        let _ = state.db.clear_botfather_session(uid);
        let apps = state.db.list_mini_apps_by_owner(uid).unwrap_or_default();
        if apps.is_empty() {
            return (
                "你名下还没有创建 Mini App。发送 `/newapp` 为已有机器人绑定小程序，或直接用自然语言创建！".to_string(),
                build_botfather_welcome_keyboard(),
            );
        }
        let mut lines = vec![format!("📱 **你名下共有 {} 个小程序**：", apps.len())];
        let mut kb = Vec::new();
        for a in &apps {
            lines.push(format!(
                "• {} **{}** (`t.me/{}/{}`) — 安装数 {}",
                a.icon_emoji, a.title, a.bot_username, a.short_name, a.install_count
            ));
            kb.push(vec![
                InlineKeyboardButton {
                    text: format!("{} 启动 {}", a.icon_emoji, a.title),
                    callback_data: None,
                    url: None,
                    web_app_url: Some(format!("gpui-miniapp://{}/{}", a.bot_username, a.short_name)),
                },
                InlineKeyboardButton {
                    text: "🗑️ 删除".to_string(),
                    callback_data: Some(format!("bf:deleteapp:{}:{}", a.bot_username, a.short_name)),
                    url: None,
                    web_app_url: None,
                },
            ]);
        }
        return (lines.join("\n"), kb);
    }

    let cmd_action = match lower.as_str() {
        "/token" => Some(("token", "选择要查看 API Token 的机器人，或发送 `@bot_username`：")),
        "/revoke" => Some(("revoke", "选择要重置（Revoke）Token 的机器人：")),
        "/setname" => Some(("setname", "选择要修改名称的机器人：")),
        "/setdescription" => Some(("setdescription", "选择要修改描述（Description）的机器人：")),
        "/setabouttext" => Some(("setabouttext", "选择要修改 About 资料的机器人：")),
        "/setcommands" => Some(("setcommands", "选择要设置斜杠命令的机器人：")),
        "/setmenubutton" => Some(("setmenubutton", "选择要配置 Menu Button 的机器人：")),
        "/setprivacypolicy" => Some(("setprivacypolicy", "选择要设置 Privacy Policy URL 的机器人：")),
        "/setmainapp" => Some(("setmainapp", "选择要配置 Main Mini App 的机器人：")),
        "/setinline" => Some(("setinline", "选择要切换 Inline 模式的机器人：")),
        "/setavatar" => Some(("setavatar", "选择要配置 Grok 动态角色头像的机器人：")),
        "/deletebot" => Some(("deletebot", "选择要永久删除的机器人：")),
        _ => None,
    };
    if let Some((act, prompt_msg)) = cmd_action {
        let bots = state.db.list_bots_by_owner(uid).unwrap_or_default();
        if bots.is_empty() {
            return (
                "你还没有创建的机器人，请先发送 `/newbot` 创建一个机器人。".to_string(),
                build_botfather_welcome_keyboard(),
            );
        }
        let state_str = format!("awaiting_target_bot_for:{act}");
        let _ = state.db.upsert_botfather_session(uid, cid, &state_str, "{}");
        return (
            prompt_msg.to_string(),
            format_bot_pick_keyboard(&format!("pick:{act}"), &bots),
        );
    }
    if lower == "/deleteapp" {
        let apps = state.db.list_mini_apps_by_owner(uid).unwrap_or_default();
        if apps.is_empty() {
            return ("你名下还没有可删除的小程序。".to_string(), Vec::new());
        }
        let _ = state
            .db
            .upsert_botfather_session(uid, cid, "awaiting_delete_app", "{}");
        let kb: Vec<Vec<InlineKeyboardButton>> = apps
            .iter()
            .take(8)
            .map(|a| {
                vec![InlineKeyboardButton {
                    text: format!("🗑️ 删除 {} ({}/{})", a.title, a.bot_username, a.short_name),
                    callback_data: Some(format!("bf:deleteapp:{}:{}", a.bot_username, a.short_name)),
                    url: None,
                    web_app_url: None,
                }]
            })
            .collect();
        return (
            "点击按钮或发送 `<bot_username>/<short_name>` 来删除指定小程序：".to_string(),
            kb,
        );
    }

    if let Ok(Some(sess)) = state.db.get_botfather_session(uid) {
        let mut draft: Map<String, Value> =
            serde_json::from_str(&sess.draft_json).unwrap_or_default();
        match sess.state.as_str() {
            "awaiting_bot_name" => {
                if text.starts_with('/') {
                    return (
                        "名称不能以 `/` 开头。请输入机器人显示名称，或发送 `/cancel` 取消。".to_string(),
                        Vec::new(),
                    );
                }
                draft.insert("display_name".into(), json!(text));
                let draft_str = Value::Object(draft).to_string();
                let _ = state
                    .db
                    .upsert_botfather_session(uid, cid, "awaiting_bot_username", &draft_str);
                return (
                    "Good. Now let's choose a username for your bot. It must end in `bot`. Like this, for example: `TetrisBot` or `tetris_bot`.\n\n现在请发送机器人的 **username**（5-32位字母/数字/下划线，必须以 `bot` 结尾）：".to_string(),
                    Vec::new(),
                );
            }
            "awaiting_bot_username" => {
                let candidate = match validate_bot_username(text) {
                    Ok(valid) => valid,
                    Err(reason) => {
                        return (format!("❌ {} Try again or send `/cancel`.", reason), Vec::new());
                    }
                };
                let display_name = draft
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&candidate)
                    .to_string();
                return match state.db.create_registered_bot_with_options(
                    uid,
                    &candidate,
                    &display_name,
                    "",
                    "",
                    "",
                    "default",
                    "",
                    "",
                    "[]",
                    false,
                    false,
                    true,
                ) {
                    Ok(bot) => {
                        let _ = state.db.clear_botfather_session(uid);
                        let reply = [
                            format!(
                                "Done! Congratulations on your new bot **{}** (`@{}`).",
                                bot.display_name, bot.username
                            ),
                            String::new(),
                            format!("Use this token to access the HTTP API:\n`{}`", bot.bot_token),
                            String::new(),
                            "Keep your token secure and store it safely. 你可以调用 `/bot<token>/getMe`、`getUpdates`、`sendMessage`、`setMyCommands`、`setChatMenuButton` 等标准 Bot API 接入服务端逻辑，或发送 `/newapp` 为它挂载一个 Mini App！".to_string(),
                        ]
                        .join("\n");
                        let kb = vec![
                            vec![
                                InlineKeyboardButton {
                                    text: format!("💬 立即打开 @{}", bot.username),
                                    callback_data: Some(format!("bf:open_bot:{}", bot.username)),
                                    url: None,
                                    web_app_url: None,
                                },
                                InlineKeyboardButton {
                                    text: "🧩 创建小程序 (/newapp)".to_string(),
                                    callback_data: Some(format!("bf:newapp_for:{}", bot.username)),
                                    url: None,
                                    web_app_url: None,
                                },
                            ],
                            vec![InlineKeyboardButton {
                                text: "🎨 动态角色头像 (Grok Avatar)".into(),
                                callback_data: Some(format!("bf:avatar_menu:{}", bot.username)),
                                url: None,
                                web_app_url: None,
                            }],
                        ];
                        (reply, kb)
                    }
                    Err(e) => (
                        format!("❌ {} 请换一个 username 再试一次，或发送 `/cancel` 取消。", e),
                        Vec::new(),
                    ),
                };
            }
            "awaiting_app_bot" => {
                let slug = normalize_bot_slug(text);
                let Some(bot) = state.db.get_bot_by_username(&slug).ok().flatten() else {
                    return (
                        format!("找不到机器人 `@{}`，请检查拼写或发送 `/cancel`。", slug),
                        Vec::new(),
                    );
                };
                if bot.owner_user_id != uid {
                    return (
                        "请选择你名下创建的机器人（不能修改其他人的或官方内置机器人）。".to_string(),
                        Vec::new(),
                    );
                }
                draft.insert("bot_user_id".into(), json!(bot.bot_user_id));
                draft.insert("bot_username".into(), json!(bot.username));
                let _ = state.db.upsert_botfather_session(
                    uid,
                    cid,
                    "awaiting_app_title",
                    &Value::Object(draft).to_string(),
                );
                return (
                    format!("已选定机器人 `@{}`！请输入新 Mini App 的**标题**（Title，例如 `咖啡点单`）：", bot.username),
                    Vec::new(),
                );
            }
            "awaiting_app_title" => {
                draft.insert("title".into(), json!(text));
                let _ = state.db.upsert_botfather_session(
                    uid,
                    cid,
                    "awaiting_app_description",
                    &Value::Object(draft).to_string(),
                );
                return ("请输入该小程序的一句话**功能简介**（Description）：".to_string(), Vec::new());
            }
            "awaiting_app_description" => {
                draft.insert("description".into(), json!(text));
                let _ = state.db.upsert_botfather_session(
                    uid,
                    cid,
                    "awaiting_app_url",
                    &Value::Object(draft).to_string(),
                );
                return (
                    "请发送 Mini App 的 **Web App URL**（https://...），或者发送 `auto` 使用本服务器自动托管的可交互小程序页面：".to_string(),
                    Vec::new(),
                );
            }
            "awaiting_app_url" => {
                let url_val = if matches!(lower.as_str(), "auto" | "default" | "默认" | "跳过" | "skip") {
                    ""
                } else {
                    if !text.starts_with("http://") && !text.starts_with("https://") && !text.starts_with('/') {
                        return (
                            "Web App URL 必须以 `https://` 或 `http://` 开头（或发送 `auto` 使用托管页面）。请重新发送或 `/cancel`：".to_string(),
                            Vec::new(),
                        );
                    }
                    text
                };
                draft.insert("web_url".into(), json!(url_val));
                let _ = state.db.upsert_botfather_session(
                    uid,
                    cid,
                    "awaiting_app_short_name",
                    &Value::Object(draft).to_string(),
                );
                return (
                    "最后一步！请发送 Mini App 的 **short_name**（2-32位字母/数字/下划线，将用于 `t.me/<bot>/<short_name>`，例如 `order` 或 `shop`）：".to_string(),
                    Vec::new(),
                );
            }
            "awaiting_app_short_name" => {
                let bot_id = draft.get("bot_user_id").and_then(|v| v.as_i64()).unwrap_or(0);
                let bot_un = draft
                    .get("bot_username")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let title = draft
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Mini App")
                    .to_string();
                let desc = draft
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let web_url_raw = draft
                    .get("web_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let target_bot = state.db.get_bot_by_user_id(bot_id).ok().flatten();
                let is_standard = target_bot.as_ref().is_some_and(|b| b.is_standard_bot);
                let set_main = if target_bot.as_ref().is_some_and(|b| !b.has_main_app) {
                    Some(true)
                } else {
                    None
                };
                let schema_str = if !web_url_raw.is_empty() {
                    serde_json::to_string(&MiniAppUiSchema {
                        title: title.clone(),
                        subtitle: desc.clone(),
                        ..Default::default()
                    })
                    .unwrap_or_else(|_| "{}".into())
                } else {
                    let mut bp = synthesize_nl_blueprint(&format!("{} {}", title, desc));
                    bp.ui_schema.title = title.clone();
                    if !desc.trim().is_empty() {
                        bp.ui_schema.subtitle = desc.clone();
                    }
                    serde_json::to_string(&bp.ui_schema).unwrap_or_else(|_| "{}".into())
                };
                return match state.db.create_or_upsert_mini_app_with_options(
                    bot_id,
                    &bot_un,
                    text,
                    &title,
                    &desc,
                    "apps",
                    "🧩",
                    if web_url_raw.is_empty() {
                        None
                    } else {
                        Some(&web_url_raw)
                    },
                    &schema_str,
                    true,
                    true,
                    false,
                    "",
                    "",
                    set_main,
                    !is_standard,
                ) {
                    Ok(app) => {
                        let _ = state.db.clear_botfather_session(uid);
                        let menu_note = if is_standard {
                            "该小程序已设为该机器人的 Main Mini App；如需额外开启底部 Menu Button，可发送 `/setmenubutton` 或调用 `setChatMenuButton` API。"
                        } else {
                            "已自动为你开启伴生机器人的 Menu Button 并设为 Main Mini App！"
                        };
                        let reply = format!(
                            "✅ **Mini App 创建并发布成功！**\n\n{} **{}**\n🔗 链接：`t.me/{}/{}`\n🌐 URL：`{}`\n{}",
                            app.icon_emoji, app.title, app.bot_username, app.short_name, app.web_url, menu_note
                        );
                        let kb = vec![vec![InlineKeyboardButton {
                            text: format!("{} 立即打开 {}", app.icon_emoji, app.title),
                            callback_data: None,
                            url: None,
                            web_app_url: Some(format!("gpui-miniapp://{}/{}", app.bot_username, app.short_name)),
                        }]];
                        (reply, kb)
                    }
                    Err(e) => (
                        format!("❌ {} 请发送一个有效的 short_name（如 `order`），或发送 `/cancel` 取消。", e),
                        Vec::new(),
                    ),
                };
            }
            "awaiting_delete_app" => {
                let (bot_part, short_part) = if let Some((b, s)) = text.split_once('/') {
                    (normalize_bot_slug(b), s.trim().to_string())
                } else {
                    let apps = state.db.list_mini_apps_by_owner(uid).unwrap_or_default();
                    if let Some(found) = apps.into_iter().find(|a| a.short_name.eq_ignore_ascii_case(text)) {
                        (found.bot_username, found.short_name)
                    } else {
                        return (
                            "请按 `BotUsername/short_name` 格式指定要删除的小程序，或发送 `/cancel`。".to_string(),
                            Vec::new(),
                        );
                    }
                };
                let _ = state.db.clear_botfather_session(uid);
                return match state.db.delete_mini_app(&bot_part, &short_part, uid) {
                    Ok(true) => (
                        format!("🗑️ 已删除小程序 `t.me/{}/{}`。", bot_part, short_part),
                        Vec::new(),
                    ),
                    Ok(false) => ("未找到指定小程序。".to_string(), Vec::new()),
                    Err(e) => (format!("❌ {}", e), Vec::new()),
                };
            }
            s if s.starts_with("awaiting_target_bot_for:") => {
                let act = &s["awaiting_target_bot_for:".len()..];
                return handle_botfather_pick_bot_for_action(state, uid, cid, act, text);
            }
            s if s.starts_with("awaiting_value_for:") => {
                let act = &s["awaiting_value_for:".len()..];
                let bot_id = draft.get("bot_user_id").and_then(|v| v.as_i64()).unwrap_or(0);
                let _ = state.db.clear_botfather_session(uid);
                return match act {
                    "setname" => match state.db.update_bot_name(bot_id, text) {
                        Ok(b) => {
                            let _ = provision_fabushi_bot_with_host(&state.db, &state.fabushi_bots, &b);
                            (format!("✅ 已将 `@{}` 名称更新为 **{}**。", b.username, b.display_name), Vec::new())
                        }
                        Err(e) => (format!("❌ {}", e), Vec::new()),
                    },
                    "setdescription" => match state.db.update_bot_description(bot_id, text) {
                        Ok(b) => {
                            let _ = provision_fabushi_bot_with_host(&state.db, &state.fabushi_bots, &b);
                            (format!("✅ 已更新 `@{}` 的 Description。", b.username), Vec::new())
                        }
                        Err(e) => (format!("❌ {}", e), Vec::new()),
                    },
                    "setabouttext" => match state.db.update_bot_about(bot_id, text) {
                        Ok(b) => {
                            let _ = provision_fabushi_bot_with_host(&state.db, &state.fabushi_bots, &b);
                            (format!("✅ 已更新 `@{}` 的 About 资料。", b.username), Vec::new())
                        }
                        Err(e) => (format!("❌ {}", e), Vec::new()),
                    },
                    "setcommands" => {
                        let Some(cmds_json) = parse_commands_text(text) else {
                            return (
                                "格式有误，请每行输入一条 `command - description`（例如 `start - 开始对话`）。".to_string(),
                                Vec::new(),
                            );
                        };
                        match state.db.update_bot_commands(bot_id, &cmds_json) {
                            Ok(b) => (format!("✅ 已更新 `@{}` 的斜杠命令列表。", b.username), Vec::new()),
                            Err(e) => (format!("❌ {}", e), Vec::new()),
                        }
                    }
                    "setmenubutton" => {
                        if matches!(lower.as_str(), "default" | "reset" | "none" | "关闭") {
                            match state.db.update_bot_menu_button_typed(bot_id, "default", "", "") {
                                Ok(b) => (format!("✅ 已将 `@{}` 的 Menu Button 重置为 `default`。", b.username), Vec::new()),
                                Err(e) => (format!("❌ {}", e), Vec::new()),
                            }
                        } else if matches!(lower.as_str(), "commands" | "menu") {
                            match state.db.update_bot_menu_button_typed(bot_id, "commands", "", "") {
                                Ok(b) => (format!("✅ 已将 `@{}` 的 Menu Button 设置为 `commands`。", b.username), Vec::new()),
                                Err(e) => (format!("❌ {}", e), Vec::new()),
                            }
                        } else {
                            let (btn_text, btn_url) = if let Some((t, u)) = text.split_once('|') {
                                (t.trim().to_string(), u.trim().to_string())
                            } else if text.starts_with("http://") || text.starts_with("https://") || text.starts_with('/') {
                                ("Open App".to_string(), text.to_string())
                            } else {
                                let existing_url = state
                                    .db
                                    .list_mini_apps_for_bot(bot_id)
                                    .ok()
                                    .and_then(|apps| {
                                        apps.into_iter()
                                            .filter(|a| a.category != "__bot_inline_webapp__")
                                            .last()
                                            .map(|a| a.web_url)
                                    })
                                    .unwrap_or_default();
                                (text.to_string(), existing_url)
                            };
                            match state.db.update_bot_menu_button_typed(bot_id, "web_app", &btn_text, &btn_url) {
                                Ok(b) => (
                                    format!(
                                        "✅ 已更新 `@{}` 的 Menu Button (`web_app`)：`{}` -> `{}`",
                                        b.username, b.menu_button_text, b.menu_button_url
                                    ),
                                    Vec::new(),
                                ),
                                Err(e) => (format!("❌ {}", e), Vec::new()),
                            }
                        }
                    }
                    "setprivacypolicy" => {
                        let url = if matches!(lower.as_str(), "none" | "clear" | "off" | "清空") {
                            ""
                        } else {
                            text
                        };
                        match state.db.update_bot_privacy_policy(bot_id, url) {
                            Ok(b) => (
                                format!("✅ 已更新 `@{}` 的 Privacy Policy URL：`{}`", b.username, b.privacy_policy_url),
                                Vec::new(),
                            ),
                            Err(e) => (format!("❌ {}", e), Vec::new()),
                        }
                    }
                    "setmainapp" => {
                        if matches!(lower.as_str(), "none" | "clear" | "off" | "关闭") {
                            match state.db.update_bot_main_app(bot_id, false, "") {
                                Ok(b) => (format!("✅ 已取消 `@{}` 的 Main Mini App。", b.username), Vec::new()),
                                Err(e) => (format!("❌ {}", e), Vec::new()),
                            }
                        } else {
                            let apps: Vec<_> = state
                                .db
                                .list_mini_apps_for_bot(bot_id)
                                .unwrap_or_default()
                                .into_iter()
                                .filter(|a| a.category != "__bot_inline_webapp__")
                                .collect();
                            if !apps.iter().any(|a| a.short_name == text) {
                                return (
                                    format!("❌ 未在该机器人名下找到 short_name 为 `{}` 的 Mini App。请先 `/newapp` 创建，或发送正确 short_name。", text),
                                    Vec::new(),
                                );
                            }
                            match state.db.update_bot_main_app(bot_id, true, text) {
                                Ok(b) => (
                                    format!("✅ 已将 `t.me/{}/{}` 设为 Main Mini App！", b.username, b.main_app_short_name),
                                    Vec::new(),
                                ),
                                Err(e) => (format!("❌ {}", e), Vec::new()),
                            }
                        }
                    }
                    _ => (build_botfather_welcome_text(), build_botfather_welcome_keyboard()),
                };
            }
            _ => {
                let _ = state.db.clear_botfather_session(uid);
            }
        }
    }

    if should_trigger_nl_from_text(text) {
        return match execute_nl_create(state, sender_id, text) {
            Ok((bot, app)) => format_nl_created_reply(&bot, &app),
            Err(e) => (format!("❌ 创建失败：{}", e), Vec::new()),
        };
    }

    if text.starts_with('/') {
        return (
            format!(
                "不识别的指令 `{}`。请使用 `/newbot`、`/newapp`、`/mybots`、`/myapps`、`/setavatar`、`/help`，或直接发一句自然语言（如：`帮我创建一个咖啡店点单助手机器人和点单小程序`）。",
                text
            ),
            build_botfather_welcome_keyboard(),
        );
    }

    (build_botfather_welcome_text(), build_botfather_welcome_keyboard())
}

fn build_botfather_avatar_menu(bot: &BotRegistryRecord) -> (String, Vec<Vec<InlineKeyboardButton>>) {
    let agent_id = if bot.agent_id.trim().is_empty() {
        derive_safe_agent_id(None, &bot.username, bot.bot_user_id)
    } else {
        bot.agent_id.clone()
    };
    let current_shape = normalize_fabushi_avatar_shape(&bot.avatar_shape, &agent_id);
    let current_color = normalize_fabushi_avatar_color(&bot.avatar_color, &agent_id);

    let text = [
        format!(
            "🎨 **配置 @{} ({}) 的 Grok 动态角色头像**",
            bot.username, bot.display_name
        ),
        format!("• 当前形状：`{}`", current_shape),
        format!("• 当前配色：`{}`", current_color),
        String::new(),
        "点击下方按钮即时切换形状（8种动态几何体 + `classic` 字母头像）或 11 种 Grok 配色："
            .to_string(),
    ]
    .join("\n");

    let mut kb = Vec::new();
    let all_shapes: Vec<&str> = GROK_AVATAR_SHAPES
        .iter()
        .copied()
        .chain(std::iter::once(CLASSIC_INITIALS_AVATAR_SHAPE))
        .collect();
    for chunk in all_shapes.chunks(3) {
        let row = chunk
            .iter()
            .map(|&shape| {
                let label = if current_shape == shape {
                    format!("✓ {shape}")
                } else {
                    shape.to_string()
                };
                InlineKeyboardButton {
                    text: label,
                    callback_data: Some(format!("bf:set_shape:{}:{}", bot.username, shape)),
                    url: None,
                    web_app_url: None,
                }
            })
            .collect();
        kb.push(row);
    }
    for chunk in GROK_AVATAR_COLOR_IDS.chunks(4) {
        let row = chunk
            .iter()
            .map(|&color| {
                let label = if current_color.eq_ignore_ascii_case(color) {
                    format!("✓ {color}")
                } else {
                    color.to_string()
                };
                InlineKeyboardButton {
                    text: label,
                    callback_data: Some(format!("bf:set_color:{}:{}", bot.username, color)),
                    url: None,
                    web_app_url: None,
                }
            })
            .collect();
        kb.push(row);
    }
    kb.push(vec![InlineKeyboardButton {
        text: format!("💬 打开 @{} 对话", bot.username),
        callback_data: Some(format!("bf:open_bot:{}", bot.username)),
        url: None,
        web_app_url: None,
    }]);

    (text, kb)
}

fn apply_botfather_avatar_update(
    state: &Arc<AppState>,
    chat_id: ChatId,
    uid: i64,
    bot_un: &str,
    avatar_shape: Option<&str>,
    avatar_color: Option<&str>,
) -> PeerBotActionResult {
    let clean = normalize_bot_slug(bot_un);
    let Some(bot) = state.db.get_bot_by_username(&clean).ok().flatten() else {
        return PeerBotActionResult {
            ok: false,
            companion_chat_id: Some(chat_id.0),
            toast: Some(format!("未找到机器人 @{}", clean)),
            ..Default::default()
        };
    };
    if bot.owner_user_id != uid {
        return PeerBotActionResult {
            ok: false,
            companion_chat_id: Some(chat_id.0),
            toast: Some("仅机器人创建者可修改头像".into()),
            ..Default::default()
        };
    }

    let updated = match state.db.update_bot_fabushi_fields(
        bot.bot_user_id,
        None,
        None,
        None,
        None,
        None,
        avatar_shape,
        avatar_color,
        None,
        None,
        None,
        None,
    ) {
        Ok(u) => u,
        Err(e) => {
            return PeerBotActionResult {
                ok: false,
                companion_chat_id: Some(chat_id.0),
                toast: Some(format!("头像更新失败: {e}")),
                ..Default::default()
            };
        }
    };

    let _ = ensure_fabushi_bot_hydrated(state, updated.bot_user_id);
    let _ = state.fabushi_bots.update_bot(
        updated.bot_user_id,
        FabushiBotPatch {
            avatar_shape: avatar_shape.map(|_| updated.avatar_shape.clone()),
            avatar_color: avatar_color.map(|_| updated.avatar_color.clone()),
            ..Default::default()
        },
    );

    let (reply, kb) = build_botfather_avatar_menu(&updated);
    let full_reply = format!(
        "✅ 已更新 `@{}` 动态角色头像：`{}` · `{}`\n\n{}",
        updated.username, updated.avatar_shape, updated.avatar_color, reply
    );
    let mid = send_bot_chat_reply(
        state,
        chat_id,
        UserId(BOTFATHER_USER_ID),
        &full_reply,
        kb.clone(),
    )
    .map(|m| m.0)
    .ok();

    PeerBotActionResult {
        ok: true,
        companion_bot: Some(bot_record_to_entry(&updated)),
        companion_chat_id: Some(chat_id.0),
        system_message: Some(full_reply.clone()),
        reply_message_id: mid,
        toast: Some(format!(
            "已更新 @{} 头像: {} / {}",
            updated.username, updated.avatar_shape, updated.avatar_color
        )),
        updated_text: Some(full_reply),
        inline_keyboard: Some(kb),
        ..Default::default()
    }
}

fn handle_botfather_pick_bot_for_action(
    state: &Arc<AppState>,
    uid: i64,
    cid: i64,
    action: &str,
    bot_slug: &str,
) -> (String, Vec<Vec<InlineKeyboardButton>>) {
    let clean = normalize_bot_slug(bot_slug);
    let Some(bot) = state.db.get_bot_by_username(&clean).ok().flatten() else {
        return (
            format!("未找到机器人 `@{}`，请重新输入或发送 `/cancel`。", clean),
            Vec::new(),
        );
    };
    if bot.owner_user_id != uid {
        return ("Permission denied: 你只能管理自己创建的机器人。".to_string(), Vec::new());
    }

    match action {
        "token" => {
            let _ = state.db.clear_botfather_session(uid);
            (
                format!("🔑 `@{}` 的 Bot API Token：\n`{}`", bot.username, bot.bot_token),
                Vec::new(),
            )
        }
        "revoke" => {
            let _ = state.db.clear_botfather_session(uid);
            match state.db.regenerate_bot_token(bot.bot_user_id) {
                Ok(updated) => (
                    format!(
                        "♻️ 已重置 `@{}` 的 API Token，旧 Token 已失效。\n新 Token：\n`{}`",
                        updated.username, updated.bot_token
                    ),
                    Vec::new(),
                ),
                Err(e) => (format!("❌ {}", e), Vec::new()),
            }
        }
        "setinline" => {
            let _ = state.db.clear_botfather_session(uid);
            match state.db.update_bot_inline(bot.bot_user_id, !bot.is_inline) {
                Ok(updated) => (
                    format!(
                        "✅ `@{}` Inline 模式已切换为：**{}**",
                        updated.username,
                        if updated.is_inline { "已开启" } else { "已关闭" }
                    ),
                    Vec::new(),
                ),
                Err(e) => (format!("❌ {}", e), Vec::new()),
            }
        }
        "deletebot" => {
            let _ = state.db.clear_botfather_session(uid);
            match state.db.delete_registered_bot(bot.bot_user_id, uid) {
                Ok(true) => (format!("🗑️ 已删除机器人 `@{}` 及其关联小程序与伴生对话。", bot.username), Vec::new()),
                Ok(false) => ("未找到机器人。".to_string(), Vec::new()),
                Err(e) => (format!("❌ {}", e), Vec::new()),
            }
        }
        "manage_bot" => {
            let _ = state.db.clear_botfather_session(uid);
            let apps = state.db.list_mini_apps_for_bot(bot.bot_user_id).unwrap_or_default();
            let mut kb = vec![
                vec![
                    InlineKeyboardButton {
                        text: "🔑 Token".into(),
                        callback_data: Some(format!("bf:pick:token:{}", bot.username)),
                        url: None,
                        web_app_url: None,
                    },
                    InlineKeyboardButton {
                        text: "♻️ 重置 Token".into(),
                        callback_data: Some(format!("bf:pick:revoke:{}", bot.username)),
                        url: None,
                        web_app_url: None,
                    },
                ],
                vec![
                    InlineKeyboardButton {
                        text: "💬 打开对话".into(),
                        callback_data: Some(format!("bf:open_bot:{}", bot.username)),
                        url: None,
                        web_app_url: None,
                    },
                    InlineKeyboardButton {
                        text: "🧩 新建小程序".into(),
                        callback_data: Some(format!("bf:newapp_for:{}", bot.username)),
                        url: None,
                        web_app_url: None,
                    },
                ],
                vec![
                    InlineKeyboardButton {
                        text: format!("⚡ Inline: {}", if bot.is_inline { "ON" } else { "OFF" }),
                        callback_data: Some(format!("bf:pick:setinline:{}", bot.username)),
                        url: None,
                        web_app_url: None,
                    },
                    InlineKeyboardButton {
                        text: "🗑️ 删除".into(),
                        callback_data: Some(format!("bf:pick:deletebot:{}", bot.username)),
                        url: None,
                        web_app_url: None,
                    },
                ],
            ];
            if let Some(first_app) = apps.first() {
                kb.push(vec![InlineKeyboardButton {
                    text: format!("{} 启动小程序 {}", first_app.icon_emoji, first_app.title),
                    callback_data: None,
                    url: None,
                    web_app_url: Some(format!("gpui-miniapp://{}/{}", first_app.bot_username, first_app.short_name)),
                }]);
            }
            (
                format!(
                    "🤖 **管理机器人 {} (`@{}`)**\n• 描述：{}\n• Inline：{}\n• 关联小程序数：{}",
                    bot.display_name,
                    bot.username,
                    bot.description,
                    bot.is_inline,
                    apps.len()
                ),
                kb,
            )
        }
        "setname"
        | "setdescription"
        | "setabouttext"
        | "setcommands"
        | "setmenubutton"
        | "setprivacypolicy"
        | "setmainapp" => {
            let draft = json!({ "bot_user_id": bot.bot_user_id, "username": bot.username }).to_string();
            let _ = state
                .db
                .upsert_botfather_session(uid, cid, &format!("awaiting_value_for:{action}"), &draft);
            let prompt = match action {
                "setname" => format!("请发送 `@{}` 的新**显示名称**：", bot.username),
                "setdescription" => format!("请发送 `@{}` 的新**简介（Description）**：", bot.username),
                "setabouttext" => format!("请发送 `@{}` 的新 **About 资料**：", bot.username),
                "setcommands" => format!(
                    "请发送 `@{}` 的命令列表，每行一条 `command - description`（例如：\n`start - 开始使用`\n`app - 打开小程序`）：",
                    bot.username
                ),
                "setprivacypolicy" => format!(
                    "请发送 `@{}` 的 **Privacy Policy URL**（`https://...`，或发送 `none` 清空）：",
                    bot.username
                ),
                "setmainapp" => format!(
                    "请发送要设为 `@{}` **Main Mini App** 的 `short_name`（或发送 `none` 取消）：",
                    bot.username
                ),
                _ => format!(
                    "请发送 `@{}` 的 Menu Button 配置（格式 `按钮文案 | URL`，或发送 `default` / `commands`）：",
                    bot.username
                ),
            };
            (prompt, Vec::new())
        }
        "setavatar" => {
            let _ = state.db.clear_botfather_session(uid);
            build_botfather_avatar_menu(&bot)
        }
        _ => (build_botfather_welcome_text(), build_botfather_welcome_keyboard()),
    }
}

pub async fn handle_botfather_callback(
    state: &Arc<AppState>,
    chat_id: ChatId,
    sender_id: UserId,
    data: &str,
) -> PeerBotActionResult {
    let uid = sender_id.0 as i64;
    let cid = chat_id.0;

    if data == "bf:nl_guide" {
        let reply = "✨ 请直接向我发送一句自然语言描述！\n例如：\n• `帮我创建一个咖啡店点单助手机器人和点单小程序，支持美式和拿铁下单并总结订单`\n• `做一个每日待办与习惯打卡机器人和小程序`";
        let mid = send_bot_chat_reply(state, chat_id, UserId(BOTFATHER_USER_ID), reply, Vec::new())
            .map(|m| m.0)
            .ok();
        return PeerBotActionResult {
            ok: true,
            companion_chat_id: Some(cid),
            system_message: Some(reply.to_string()),
            reply_message_id: mid,
            toast: Some("发送自然语言即可一键生成".into()),
            ..Default::default()
        };
    }

    if let Some(username) = data.strip_prefix("bf:open_bot:") {
        if let Ok(Some(bot)) = state.db.get_bot_by_username(username) {
            if let Ok((dm_cid, _)) =
                ensure_companion_bot_dm_with_welcome(state, sender_id, UserId(bot.bot_user_id as u64))
            {
                return PeerBotActionResult {
                    ok: true,
                    companion_bot: Some(bot_record_to_entry_for_viewer(state, &bot, uid)),
                    companion_chat_id: Some(dm_cid.0),
                    toast: Some(format!("已打开 @{} 对话", bot.username)),
                    ..Default::default()
                };
            }
        }
        return PeerBotActionResult {
            ok: false,
            toast: Some("未找到该机器人".into()),
            ..Default::default()
        };
    }

    if let Some(username) = data.strip_prefix("bf:newapp_for:") {
        if let Ok(Some(bot)) = state.db.get_bot_by_username(username) {
            if bot.owner_user_id == uid {
                let draft = json!({ "bot_user_id": bot.bot_user_id, "bot_username": bot.username }).to_string();
                let _ = state.db.upsert_botfather_session(uid, cid, "awaiting_app_title", &draft);
                let prompt = format!("已选择机器人 `@{}`！请输入新 Mini App 的**标题**（Title）：", bot.username);
                let mid = send_bot_chat_reply(state, chat_id, UserId(BOTFATHER_USER_ID), &prompt, Vec::new())
                    .map(|m| m.0)
                    .ok();
                return PeerBotActionResult {
                    ok: true,
                    companion_bot: Some(bot_record_to_entry(&bot)),
                    companion_chat_id: Some(cid),
                    system_message: Some(prompt),
                    reply_message_id: mid,
                    toast: Some(format!("为 @{} 创建小程序", bot.username)),
                    ..Default::default()
                };
            }
        }
        let (reply, kb) = handle_botfather_message(state, chat_id, sender_id, "/newapp").await;
        let mid = send_bot_chat_reply(state, chat_id, UserId(BOTFATHER_USER_ID), &reply, kb)
            .map(|m| m.0)
            .ok();
        return PeerBotActionResult {
            ok: true,
            companion_chat_id: Some(cid),
            system_message: Some(reply),
            reply_message_id: mid,
            toast: Some("已进入 /newapp 流程".into()),
            ..Default::default()
        };
    }

    if let Some(rest) = data.strip_prefix("bf:deleteapp:") {
        if let Some((bot_un, short)) = rest.split_once(':') {
            let reply = match state.db.delete_mini_app(bot_un, short, uid) {
                Ok(true) => format!("🗑️ 已删除小程序 `t.me/{}/{}`。", bot_un, short),
                Ok(false) => "小程序不存在或已删除。".to_string(),
                Err(e) => format!("❌ {}", e),
            };
            let mid = send_bot_chat_reply(state, chat_id, UserId(BOTFATHER_USER_ID), &reply, Vec::new())
                .map(|m| m.0)
                .ok();
            return PeerBotActionResult {
                ok: true,
                companion_chat_id: Some(cid),
                system_message: Some(reply.clone()),
                reply_message_id: mid,
                toast: Some(reply),
                ..Default::default()
            };
        }
    }

    if let Some(rest) = data.strip_prefix("bf:pick:") {
        if let Some((act, bot_un)) = rest.split_once(':') {
            let (reply, kb) = handle_botfather_pick_bot_for_action(state, uid, cid, act, bot_un);
            let kb_opt = if kb.is_empty() { None } else { Some(kb.clone()) };
            let mid = send_bot_chat_reply(state, chat_id, UserId(BOTFATHER_USER_ID), &reply, kb)
                .map(|m| m.0)
                .ok();
            return PeerBotActionResult {
                ok: true,
                companion_chat_id: Some(cid),
                system_message: Some(reply),
                reply_message_id: mid,
                toast: Some(format!("已选择 @{}", bot_un)),
                inline_keyboard: kb_opt,
                ..Default::default()
            };
        }
    }

    if let Some(username) = data.strip_prefix("bf:avatar_menu:") {
        let clean = normalize_bot_slug(username);
        if let Ok(Some(bot)) = state.db.get_bot_by_username(&clean) {
            if bot.owner_user_id != uid {
                return PeerBotActionResult {
                    ok: false,
                    companion_chat_id: Some(cid),
                    toast: Some("仅机器人创建者可配置头像".into()),
                    ..Default::default()
                };
            }
            let (reply, kb) = build_botfather_avatar_menu(&bot);
            let mid = send_bot_chat_reply(
                state,
                chat_id,
                UserId(BOTFATHER_USER_ID),
                &reply,
                kb.clone(),
            )
            .map(|m| m.0)
            .ok();
            return PeerBotActionResult {
                ok: true,
                companion_bot: Some(bot_record_to_entry(&bot)),
                companion_chat_id: Some(cid),
                system_message: Some(reply.clone()),
                reply_message_id: mid,
                toast: Some(format!("配置 @{} 动态头像", bot.username)),
                updated_text: Some(reply),
                inline_keyboard: Some(kb),
                ..Default::default()
            };
        }
        return PeerBotActionResult {
            ok: false,
            companion_chat_id: Some(cid),
            toast: Some("未找到该机器人".into()),
            ..Default::default()
        };
    }

    if let Some(rest) = data.strip_prefix("bf:set_shape:") {
        if let Some((bot_un, shape)) = rest.split_once(':') {
            return apply_botfather_avatar_update(state, chat_id, uid, bot_un, Some(shape), None);
        }
    }

    if let Some(rest) = data.strip_prefix("bf:set_color:") {
        if let Some((bot_un, color)) = rest.split_once(':') {
            return apply_botfather_avatar_update(state, chat_id, uid, bot_un, None, Some(color));
        }
    }

    if let Some(cmd) = data.strip_prefix("bf:/") {
        let slash = format!("/{}", cmd);
        let (reply, kb) = handle_botfather_message(state, chat_id, sender_id, &slash).await;
        let kb_opt = if kb.is_empty() { None } else { Some(kb.clone()) };
        let mid = send_bot_chat_reply(state, chat_id, UserId(BOTFATHER_USER_ID), &reply, kb)
            .map(|m| m.0)
            .ok();
        return PeerBotActionResult {
            ok: true,
            companion_chat_id: Some(cid),
            system_message: Some(reply),
            reply_message_id: mid,
            toast: Some(format!("已执行 {}", slash)),
            inline_keyboard: kb_opt,
            ..Default::default()
        };
    }

    PeerBotActionResult {
        ok: true,
        companion_chat_id: Some(cid),
        toast: Some("BotFather 已处理".into()),
        ..Default::default()
    }
}

pub fn send_bot_chat_reply(
    state: &Arc<AppState>,
    chat_id: ChatId,
    bot_user_id: UserId,
    text: &str,
    kb_rows: Vec<Vec<InlineKeyboardButton>>,
) -> gpui_telegram_core::error::Result<MsgId> {
    let mut bot_msg = MessageItem::new(
        MsgId(0),
        chat_id,
        bot_user_id,
        PeerId::Chat(chat_id),
        MessageContent::plain_text(text.trim()),
    );
    bot_msg.flags.is_out = false;
    if !kb_rows.is_empty() {
        bot_msg.inline_keyboard = Some(kb_rows.clone());
    }
    let pts_update = state.message_pipeline.process_incoming_message(bot_msg)?;
    let mut saved_id = MsgId(0);
    if let gpui_telegram_proto::sync::UpdateEvent::NewMessage(saved) = &pts_update.event {
        saved_id = saved.id;
        if !kb_rows.is_empty() {
            if let Ok(kb_json) = serde_json::to_string(&kb_rows) {
                let _ = state
                    .db
                    .save_message_extras(chat_id, saved.id, Some(&kb_json), None);
            }
        }
    }
    broadcast_chat_pts(state, chat_id, pts_update);
    Ok(saved_id)
}

// ============================================================================
// Companion Bot Provisioning + Every-Bot-Is-AI-Bot Turn Execution
// ============================================================================

pub fn ensure_companion_bot_dm_with_welcome(
    state: &Arc<AppState>,
    user_id: UserId,
    bot_user_id: UserId,
) -> gpui_telegram_core::error::Result<(ChatId, bool)> {
    let (chat_id, created_chat) = state
        .db
        .ensure_companion_bot_chat_record(user_id, bot_user_id)?;

    state.gateway_hub.subscribe_chat(chat_id.0, user_id);
    state.gateway_hub.subscribe_chat(chat_id.0, bot_user_id);

    let existing_msgs = state
        .db
        .list_messages_with_topic(chat_id, None, None, 1)
        .unwrap_or_default();
    if existing_msgs.is_empty() {
        let reg_opt = state.db.get_bot_by_user_id(bot_user_id.0 as i64).ok().flatten();
        let is_standard = reg_opt.as_ref().is_some_and(|r| r.is_standard_bot);
        if !is_standard {
            let (welcome_text, kb) = if bot_user_id.0 == BOTFATHER_USER_ID {
                (build_botfather_welcome_text(), build_botfather_welcome_keyboard())
            } else if let Some(reg) = reg_opt {
                let apps = state
                    .db
                    .list_mini_apps_for_bot(bot_user_id.0 as i64)
                    .unwrap_or_default();
                build_companion_bot_welcome_text_and_keyboard(
                    &reg.display_name,
                    &reg.username,
                    &reg.description,
                    &apps,
                )
            } else {
                let user = state.db.get_user(bot_user_id).ok().flatten();
                let title = user
                    .as_ref()
                    .map(|u| u.first_name.as_str())
                    .unwrap_or("AI Assistant");
                let uname = user
                    .as_ref()
                    .and_then(|u| u.username.as_deref())
                    .unwrap_or("bot");
                let bio = user.as_ref().and_then(|u| u.bio.as_deref()).unwrap_or("");
                build_companion_bot_welcome_text_and_keyboard(title, uname, bio, &[])
            };
            let _ = send_bot_chat_reply(state, chat_id, bot_user_id, &welcome_text, kb);
        }
    }

    Ok((chat_id, created_chat))
}

fn merge_json_patch(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(dst), Value::Object(src)) => {
            for (k, v) in src {
                if v.is_null() {
                    dst.remove(k);
                } else if let Some(existing) = dst.get_mut(k) {
                    if existing.is_object() && v.is_object() {
                        merge_json_patch(existing, v);
                    } else {
                        *existing = v.clone();
                    }
                } else {
                    dst.insert(k.clone(), v.clone());
                }
            }
        }
        (dst, src) => *dst = src.clone(),
    }
}

pub(crate) fn apply_miniapp_state_patches_for_bot_user(
    state: &AppState,
    uid: i64,
    bot_user_id: i64,
    companion_cid: i64,
    apps: &[MiniAppRecord],
    patches: &[Value],
) {
    if apps.is_empty() || patches.is_empty() {
        return;
    }
    let latest_session = state
        .db
        .get_latest_mini_app_state_for_bot_user(uid, bot_user_id)
        .ok()
        .flatten();
    let target_app = latest_session
        .as_ref()
        .and_then(|s| apps.iter().find(|a| a.app_id == s.app_id))
        .unwrap_or(&apps[0]);
    let mut current_state = latest_session
        .as_ref()
        .and_then(|s| serde_json::from_str::<Value>(&s.state_json).ok())
        .unwrap_or_else(|| json!({}));
    if !current_state.is_object() {
        current_state = json!({});
    }
    for patch in patches {
        merge_json_patch(&mut current_state, patch);
    }
    persist_session_state(
        state,
        latest_session.as_ref(),
        uid,
        target_app,
        companion_cid,
        &current_state,
    );
}

pub async fn handle_ai_bot_reply(
    state: &Arc<AppState>,
    chat_id: ChatId,
    sender_id: UserId,
    bot_user_id: UserId,
    text: &str,
) {
    if bot_user_id.0 == BOTFATHER_USER_ID {
        let typing_guard =
            crate::routes::hermes::BotTypingGuard::start(Arc::clone(state), chat_id, bot_user_id);
        let (reply, kb) = handle_botfather_message(state, chat_id, sender_id, text).await;
        let _ = send_bot_chat_reply(state, chat_id, bot_user_id, &reply, kb);
        typing_guard.finish();
        return;
    }

    let reg_bot = state
        .db
        .get_bot_by_user_id(bot_user_id.0 as i64)
        .ok()
        .flatten();
    if reg_bot.as_ref().is_some_and(|b| b.is_standard_bot) {
        return;
    }
    let apps = state
        .db
        .list_mini_apps_for_bot(bot_user_id.0 as i64)
        .unwrap_or_default();
    let latest_session = state
        .db
        .get_latest_mini_app_state_for_bot_user(sender_id.0 as i64, bot_user_id.0 as i64)
        .ok()
        .flatten();
    let latest_state_val: Option<Value> = latest_session
        .as_ref()
        .and_then(|s| serde_json::from_str(&s.state_json).ok());
    let cloud_kv = state
        .db
        .list_cloud_storage(sender_id.0 as i64, bot_user_id.0 as i64)
        .unwrap_or_default();

    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();

    if let Some(ref b) = reg_bot {
        if lower == "/start" {
            let typing_guard = crate::routes::hermes::BotTypingGuard::start(
                Arc::clone(state),
                chat_id,
                bot_user_id,
            );
            let (mut welcome, kb) = build_companion_bot_welcome_text_and_keyboard(
                &b.display_name,
                &b.username,
                &b.description,
                &apps,
            );
            let live_runtime = crate::routes::hermes::format_live_bot_runtime_card_section(
                state,
                sender_id.0,
                bot_user_id,
            );
            welcome.push_str("\n\n");
            welcome.push_str(&live_runtime);
            let _ = send_bot_chat_reply(state, chat_id, bot_user_id, &welcome, kb);
            typing_guard.finish();
            return;
        }
    }

    let (bot_sp, bot_model, _) = crate::routes::hermes::load_bot_persona_config(state, bot_user_id);
    let mut extra_context_sections = Vec::new();
    if latest_state_val.is_some() || !cloud_kv.is_empty() {
        let mut ctx_note = vec!["## Live Companion Mini App State".to_string()];
        if let Some(ref st) = latest_state_val {
            ctx_note.push(format!("- [User Live Mini App State]: {st}"));
        }
        if !cloud_kv.is_empty() {
            if let Ok(kv_str) = serde_json::to_string(&cloud_kv) {
                ctx_note.push(format!("- [Mini App CloudStorage]: {kv_str}"));
            }
        }
        extra_context_sections.push(ctx_note.join("\n"));
    }

    let help_bot_commands_suffix = reg_bot.as_ref().and_then(|b| {
        if lower == "/help" {
            let cmds: Vec<BotCommandSpec> =
                serde_json::from_str(&b.commands_json).unwrap_or_default();
            let mut lines = vec![format!(
                "🤖 **{} (`@{}`) 专属命令**：",
                b.display_name, b.username
            )];
            for c in cmds {
                lines.push(format!("• `/{}` — {}", c.command, c.description));
            }
            Some(lines.join("\n"))
        } else {
            None
        }
    });

    let bot_display_name = reg_bot
        .as_ref()
        .map(|b| b.display_name.clone())
        .or_else(|| crate::routes::hermes::resolve_bot_display_name(state, bot_user_id));

    crate::routes::hermes::execute_bot_turn_streamed(
        state,
        crate::routes::hermes::BotStreamTurnParams {
            chat_id,
            sender_id,
            bot_user_id,
            prompt: text.to_string(),
            base_system_prompt: bot_sp,
            bot_display_name,
            explicit_model: None,
            bot_model: crate::routes::hermes::sanitize_model_override(state, bot_model),
            extra_context_sections,
            companion_apps: apps,
            help_bot_commands_suffix,
        },
    )
    .await;
}

// ============================================================================
// Mini App Schema-Driven State Reducer + UI Schema Sync
// ============================================================================

fn detect_app_domain(schema: &MiniAppUiSchema, app: &MiniAppRecord) -> &'static str {
    if schema.widgets.iter().any(|w| w.id == "completed_sessions") {
        "pomodoro_focus"
    } else if schema.widgets.iter().any(|w| w.id == "split_count") {
        "split_bill"
    } else if schema.widgets.iter().any(|w| w.id == "selected_option") {
        "team_poll"
    } else if schema.widgets.iter().any(|w| w.id == "selected_item") {
        "order_store"
    } else if schema.widgets.iter().any(|w| w.id == "new_task") {
        "todo_tracker"
    } else if app.short_name.eq_ignore_ascii_case("focus") {
        "pomodoro_focus"
    } else if app.short_name.eq_ignore_ascii_case("split") {
        "split_bill"
    } else if app.short_name.eq_ignore_ascii_case("poll") {
        "team_poll"
    } else {
        "custom_ai_studio"
    }
}

pub fn initial_state_for_app(app: &MiniAppRecord) -> Value {
    let schema: MiniAppUiSchema =
        serde_json::from_str(&app.ui_schema_json).unwrap_or_else(|_| fallback_schema_for_app(app));
    let mut map = Map::new();
    for w in &schema.widgets {
        match w.kind {
            MiniAppWidgetType::Counter => {
                map.insert(w.id.clone(), json!(w.count));
            }
            MiniAppWidgetType::Toggle => {
                map.insert(w.id.clone(), json!(w.checked));
            }
            MiniAppWidgetType::MetricCard => {
                if w.count != 0 {
                    map.insert(w.id.clone(), json!(w.count));
                } else {
                    map.insert(w.id.clone(), json!(w.value));
                }
            }
            _ => {
                map.insert(w.id.clone(), json!(w.value));
            }
        }
    }
    match detect_app_domain(&schema, app) {
        "pomodoro_focus" => {
            map.entry("duration_minutes").or_insert_with(|| json!(25));
            map.entry("completed_sessions").or_insert_with(|| json!(3));
            map.entry("focus_minutes_today").or_insert_with(|| json!(75));
            map.entry("status").or_insert_with(|| json!("ready"));
        }
        "split_bill" => {
            map.entry("amount").or_insert_with(|| json!(180.0));
            map.entry("split_count").or_insert_with(|| json!(3));
            map.entry("per_person").or_insert_with(|| json!(60.0));
            map.entry("entries").or_insert_with(|| {
                json!([{ "item": "团队咖啡与午餐", "amount": 180.0, "people": 3, "per_person": 60.0 }])
            });
        }
        "team_poll" => {
            map.entry("votes").or_insert_with(|| {
                json!({
                    "方案 A: 周五晚发布": 4,
                    "方案 B: 周六上午灰度": 2,
                    "方案 C: 下周一全量": 1
                })
            });
        }
        "order_store" => {
            map.entry("quantity").or_insert_with(|| json!(1));
            map.entry("orders").or_insert_with(|| json!([]));
            map.entry("total_amount").or_insert_with(|| json!(0.0));
        }
        "todo_tracker" => {
            map.entry("tasks").or_insert_with(|| {
                json!([{ "title": "确认核心目标", "priority": "高优先级", "done": false }])
            });
        }
        _ => {
            if app.short_name.eq_ignore_ascii_case("fabushi") {
                map.entry("mode").or_insert_with(|| json!("全球法布施"));
                map.entry("sutra").or_insert_with(|| json!("心经"));
                map.entry("target_country").or_insert_with(|| json!("ALL"));
                map.entry("chant_count").or_insert_with(|| json!(108));
                map.entry("total_transfer_bytes").or_insert_with(|| json!(0));
                map.entry("total_practice_sessions").or_insert_with(|| json!(0));
                map.entry("total_practice_chants").or_insert_with(|| json!(0));
            }
            map.entry("records").or_insert_with(|| json!([]));
        }
    }
    Value::Object(map)
}

pub fn sync_ui_schema_with_state(schema: &MiniAppUiSchema, state_val: &Value) -> MiniAppUiSchema {
    let mut next = schema.clone();
    let Some(obj) = state_val.as_object() else {
        return next;
    };
    for w in &mut next.widgets {
        if let Some(val) = obj.get(&w.id) {
            match w.kind {
                MiniAppWidgetType::Counter => {
                    if let Some(n) = val.as_i64() {
                        w.count = n;
                        w.value = format!("{n}");
                    } else if let Some(s) = val.as_str() {
                        if let Ok(n) = s.trim().parse::<i64>() {
                            w.count = n;
                        }
                        w.value = s.to_string();
                    }
                }
                MiniAppWidgetType::Toggle => {
                    if let Some(b) = val.as_bool() {
                        w.checked = b;
                    }
                }
                MiniAppWidgetType::MetricCard => {
                    if let Some(n) = val.as_i64() {
                        w.count = n;
                    } else if let Some(s) = val.as_str() {
                        w.value = s.to_string();
                    }
                }
                _ => {
                    if let Some(s) = val.as_str() {
                        w.value = s.to_string();
                    } else if !val.is_null() {
                        w.value = val.to_string();
                    }
                }
            }
        }
        // Specialized computed metric syncs
        if w.id == "focus_minutes" {
            if let Some(mins) = obj.get("focus_minutes_today").and_then(|v| v.as_i64()) {
                w.count = mins;
                w.value = format!("{mins} 分钟");
            }
            if let Some(st) = obj.get("status").and_then(|v| v.as_str()) {
                w.subtitle = Some(format!("状态: {st}"));
            }
        } else if w.id == "per_person" {
            if let Some(pp) = obj.get("per_person").and_then(|v| v.as_f64()) {
                w.value = format!("¥{pp:.2}");
            }
            if let Some(cnt) = obj.get("entries").and_then(|v| v.as_array()).map(|a| a.len()) {
                w.subtitle = Some(format!("累计 {cnt} 笔消费"));
            }
        } else if w.id == "order_stats" {
            if let Some(total) = obj.get("total_amount").and_then(|v| v.as_f64()) {
                w.value = format!("¥{total:.2}");
            }
            if let Some(cnt) = obj.get("orders").and_then(|v| v.as_array()).map(|a| a.len()) {
                w.count = cnt as i64;
                w.subtitle = Some(format!("共 {cnt} 笔订单"));
            }
        } else if w.id == "todo_stats" {
            if let Some(cnt) = obj.get("tasks").and_then(|v| v.as_array()).map(|a| a.len()) {
                w.count = cnt as i64;
                w.value = format!("{cnt} 项");
            }
        } else if w.id == "record_stats" {
            if let Some(cnt) = obj.get("records").and_then(|v| v.as_array()).map(|a| a.len()) {
                w.count = cnt as i64;
                w.value = format!("{cnt} 条");
            }
        } else if w.id == "total_votes" {
            if let Some(votes) = obj.get("votes").and_then(|v| v.as_object()) {
                let total: i64 = votes.values().filter_map(|v| v.as_i64()).sum();
                w.count = total;
                w.value = format!("{total} 票");
            }
        }
    }
    next
}

fn parse_number_from_label(label: &str) -> Option<f64> {
    let pos = label.find('¥').or_else(|| label.find('$'))?;
    let after = &label[pos + label[pos..].chars().next()?.len_utf8()..];
    let digits: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.parse::<f64>().ok()
}

fn value_as_f64(v: Option<&Value>, default: f64) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(default),
        Some(Value::String(s)) => s.trim().parse::<f64>().unwrap_or(default),
        _ => default,
    }
}

fn value_as_i64(v: Option<&Value>, default: i64) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(default),
        Some(Value::String(s)) => {
            let digits: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
            digits.parse::<i64>().unwrap_or(default)
        }
        _ => default,
    }
}

pub fn reduce_mini_app_action(
    app: &MiniAppRecord,
    current_state: Value,
    req: &MiniAppActionRequest,
) -> (Value, String, Option<String>, bool) {
    let mut state = match current_state {
        Value::Object(m) => m,
        _ => match initial_state_for_app(app) {
            Value::Object(m) => m,
            _ => Map::new(),
        },
    };
    let schema: MiniAppUiSchema =
        serde_json::from_str(&app.ui_schema_json).unwrap_or_else(|_| fallback_schema_for_app(app));
    let domain = detect_app_domain(&schema, app);
    let action_type = req.action_type.trim();

    if action_type == "widget_event" {
        if let Some(wid) = req.widget_id.as_deref() {
            let val = req.value.clone().unwrap_or(Value::Null);
            state.insert(wid.to_string(), val.clone());
            if wid == "preset" && domain == "pomodoro_focus" {
                let mins = val
                    .as_str()
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(25);
                state.insert("duration_minutes".into(), json!(mins));
            }
            if domain == "split_bill" && (wid == "amount" || wid == "split_count") {
                let amount = value_as_f64(state.get("amount"), 180.0);
                let count = value_as_i64(state.get("split_count"), 3).max(1) as f64;
                let pp = (amount / count * 100.0).round() / 100.0;
                state.insert("per_person".into(), json!(pp));
                state.insert(
                    "ai_summary".into(),
                    json!(format!("当前消费 ¥{amount:.2}，按 {} 人 AA，每人应付 ¥{pp:.2}。", count as i64)),
                );
            }
            return (Value::Object(state), format!("已更新 {}", wid), None, false);
        }
    }

    if action_type == "main_button_click" {
        match domain {
            "pomodoro_focus" => {
                let dur = value_as_i64(state.get("duration_minutes"), 25);
                let prev_count = value_as_i64(state.get("completed_sessions"), 0);
                let prev_mins = value_as_i64(state.get("focus_minutes_today"), 0);
                let task = state.get("task").and_then(|v| v.as_str()).unwrap_or("专注任务").to_string();
                let new_count = prev_count + 1;
                let new_mins = prev_mins + dur;
                state.insert("completed_sessions".into(), json!(new_count));
                state.insert("focus_minutes_today".into(), json!(new_mins));
                state.insert("status".into(), json!("completed"));
                let tip = format!(
                    "太棒了！刚完成「{task}」{dur} 分钟。今日已累计 {new_count} 个番茄（{new_mins} 分钟），建议站起来喝口水休息一下。"
                );
                state.insert("ai_summary".into(), json!(tip));
                return (Value::Object(state), tip, None, false);
            }
            "split_bill" => {
                let item = state
                    .get("item_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("聚会消费")
                    .to_string();
                let amount = value_as_f64(state.get("amount"), 60.0);
                let people = value_as_i64(state.get("split_count"), 3).max(1);
                let pp = (amount / people as f64 * 100.0).round() / 100.0;
                let mut entries = state
                    .get("entries")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                entries.push(json!({ "item": item, "amount": amount, "people": people, "per_person": pp }));
                let total: f64 = entries
                    .iter()
                    .filter_map(|e| e.get("amount").and_then(|v| v.as_f64()))
                    .sum();
                let summary = format!(
                    "已记入「{item}」¥{amount:.2}（{people} 人每人 ¥{pp:.2}）。账本共 {} 笔，累计 ¥{total:.2}。",
                    entries.len()
                );
                state.insert("entries".into(), Value::Array(entries));
                state.insert("per_person".into(), json!(pp));
                state.insert("ai_summary".into(), json!(summary));
                return (Value::Object(state), summary, None, false);
            }
            "team_poll" => {
                let topic = state.get("topic").and_then(|v| v.as_str()).unwrap_or("团队投票").to_string();
                let selected = state
                    .get("selected_option")
                    .and_then(|v| v.as_str())
                    .unwrap_or("方案 A")
                    .to_string();
                let mut votes = state
                    .get("votes")
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                let next_val = votes.get(&selected).and_then(|v| v.as_i64()).unwrap_or(0) + 1;
                votes.insert(selected.clone(), json!(next_val));
                let total_votes: i64 = votes.values().filter_map(|v| v.as_i64()).sum();
                let leader = votes
                    .iter()
                    .max_by_key(|(_, v)| v.as_i64().unwrap_or(0))
                    .map(|(k, v)| format!("{} ({} 票)", k, v.as_i64().unwrap_or(0)))
                    .unwrap_or_default();
                let summary = format!(
                    "已为「{selected}」投票 +1！当前议题《{topic}》累计 {total_votes} 票，领先选项：{leader}。"
                );
                state.insert("votes".into(), Value::Object(votes));
                state.insert("ai_summary".into(), json!(summary));
                return (Value::Object(state), summary, None, false);
            }
            "order_store" => {
                let item = state
                    .get("selected_item")
                    .and_then(|v| v.as_str())
                    .unwrap_or("经典美式 ¥22")
                    .to_string();
                let temp = state
                    .get("temperature")
                    .and_then(|v| v.as_str())
                    .unwrap_or("标准")
                    .to_string();
                let qty = value_as_i64(state.get("quantity"), 1).max(1);
                let unit_price = parse_number_from_label(&item).unwrap_or(25.0);
                let line_total = unit_price * qty as f64;
                let note = state.get("note").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let mut orders = state
                    .get("orders")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                orders.push(json!({
                    "item": item,
                    "temperature": temp,
                    "quantity": qty,
                    "note": note,
                    "subtotal": line_total,
                }));
                let total: f64 = orders
                    .iter()
                    .filter_map(|o| o.get("subtotal").and_then(|v| v.as_f64()))
                    .sum();
                let summary = format!(
                    "✅ 已下单：{item} × {qty}（{temp}）小计 ¥{line_total:.2}。当前共 {} 笔订单，合计 ¥{total:.2}。",
                    orders.len()
                );
                state.insert("orders".into(), Value::Array(orders));
                state.insert("total_amount".into(), json!(total));
                state.insert("ai_summary".into(), json!(summary));
                return (Value::Object(state), summary, None, false);
            }
            "todo_tracker" => {
                let task = state
                    .get("new_task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("新任务")
                    .to_string();
                let prio = state
                    .get("priority")
                    .and_then(|v| v.as_str())
                    .unwrap_or("普通")
                    .to_string();
                let mut tasks = state
                    .get("tasks")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                tasks.push(json!({ "title": task, "priority": prio, "done": false }));
                let summary = format!(
                    "✅ 已添加「{task}」（{prio}），目前清单共 {} 项待办。",
                    tasks.len()
                );
                state.insert("tasks".into(), Value::Array(tasks));
                state.insert("ai_summary".into(), json!(summary));
                return (Value::Object(state), summary, None, false);
            }
            _ => {
                let input = state
                    .get("input_text")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("工作区更新")
                    .to_string();
                let mode = state
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("标准模式")
                    .to_string();
                let mut records = state
                    .get("records")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                records.push(json!({ "input": input, "mode": mode }));
                let summary = format!(
                    "✨ AI 已按「{mode}」处理输入「{input}」，累计保存 {} 条工作区记录。",
                    records.len()
                );
                state.insert("records".into(), Value::Array(records));
                state.insert("ai_summary".into(), json!(summary));
                return (Value::Object(state), summary, None, false);
            }
        }
    }

    if matches!(action_type, "secondary_button_click" | "send_data") {
        let data_text = req
            .data
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                state
                    .get("ai_summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("已同步小程序数据")
                    .to_string()
            });
        let chat_message = format!(
            "{} 来自小程序 **{}** 的同步：\n{}",
            app.icon_emoji, app.title, data_text
        );
        return (
            Value::Object(state),
            "已将结果发送到伴生机器人对话".into(),
            Some(chat_message),
            action_type == "send_data",
        );
    }

    (Value::Object(state), "操作已执行".to_string(), None, false)
}

// ============================================================================
// HTML5 WebView Mini App Page + Official `window.Telegram.WebApp` JS SDK
// ============================================================================

pub fn render_miniapp_html(
    app: &MiniAppRecord,
    bot: &BotRegistryRecord,
    init_data_raw: &str,
    schema: &MiniAppUiSchema,
    initial_state: &Value,
) -> String {
    let schema_json = serde_json::to_string(schema).unwrap_or_else(|_| "{}".into());
    let state_json = serde_json::to_string(initial_state).unwrap_or_else(|_| "{}".into());
    let init_data_escaped = serde_json::to_string(init_data_raw).unwrap_or_else(|_| "\"\"".into());
    let app_title_escaped = html_escape(&app.title);
    let bot_un_escaped = html_escape(&bot.username);
    let short_escaped = html_escape(&app.short_name);

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width,initial-scale=1,maximum-scale=1,user-scalable=no" />
  <title>{title} — @{bot_un}</title>
  <style>
    :root {{
      --tg-theme-bg-color: #ffffff;
      --tg-theme-secondary-bg-color: #f4f6f9;
      --tg-theme-text-color: #111827;
      --tg-theme-hint-color: #6b7280;
      --tg-theme-button-color: #2481cc;
      --tg-theme-button-text-color: #ffffff;
    }}
    body {{
      margin: 0; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      background: var(--tg-theme-bg-color); color: var(--tg-theme-text-color); padding: 16px 16px 90px;
    }}
    .hdr {{ display: flex; align-items: center; gap: 10px; margin-bottom: 16px; }}
    .hdr-icon {{ font-size: 28px; }}
    .hdr h1 {{ font-size: 18px; margin: 0; }}
    .hdr p {{ font-size: 12px; color: var(--tg-theme-hint-color); margin: 2px 0 0; }}
    .card {{ background: var(--tg-theme-secondary-bg-color); border-radius: 12px; padding: 12px; margin-bottom: 12px; }}
    label {{ display: block; font-size: 12px; font-weight: 600; color: var(--tg-theme-hint-color); margin-bottom: 6px; }}
    input {{ width: 100%; box-sizing: border-box; padding: 10px; border: 1px solid #d1d5db; border-radius: 8px; font-size: 14px; }}
    .seg {{ display: flex; flex-wrap: wrap; gap: 8px; }}
    .seg button {{ border: 1px solid #d1d5db; background: #fff; padding: 8px 12px; border-radius: 8px; cursor: pointer; font-size: 13px; }}
    .seg button.active {{ background: var(--tg-theme-button-color); color: var(--tg-theme-button-text-color); border-color: transparent; }}
    .callout {{ background: #eef6ff; border-left: 3px solid var(--tg-theme-button-color); padding: 10px 12px; border-radius: 8px; font-size: 13px; line-height: 1.5; }}
    .metric {{ font-size: 20px; font-weight: 700; color: var(--tg-theme-button-color); }}
    .bar {{ position: fixed; left: 0; right: 0; bottom: 0; padding: 12px 16px; background: #fff; border-top: 1px solid #e5e7eb; display: flex; gap: 8px; }}
    .btn-main {{ flex: 2; background: var(--tg-theme-button-color); color: #fff; border: 0; border-radius: 10px; padding: 12px; font-weight: 600; cursor: pointer; }}
    .btn-sec {{ flex: 1; background: #f3f4f6; color: #111827; border: 0; border-radius: 10px; padding: 12px; font-weight: 500; cursor: pointer; }}
  </style>
  <script>
    window.__TG_INIT_DATA__ = {init_data_escaped};
    window.__TG_BOT_ID__ = {bot_uid};
    window.__TG_API_BASE__ = '/api/v1/miniapps/{bot_un}/{short}';
  </script>
  <script src="/js/telegram-web-app.js"></script>
  <script>
    (function() {{
      function renderButtons() {{
        const wa = window.Telegram && window.Telegram.WebApp;
        if (!wa) return;
        const mb = document.getElementById('tg-main-btn');
        const sb = document.getElementById('tg-sec-btn');
        if (mb && wa.MainButton) {{ mb.style.display = wa.MainButton.isVisible ? 'block' : 'none'; mb.textContent = wa.MainButton.text; }}
        if (sb && wa.SecondaryButton) {{ sb.style.display = wa.SecondaryButton.isVisible ? 'block' : 'none'; sb.textContent = wa.SecondaryButton.text; }}
      }}
      window.__renderButtons = renderButtons;
      const wa = window.Telegram && window.Telegram.WebApp;
      if (wa && wa.MainButton) {{
        const origMainSync = wa.MainButton._sync;
        wa.MainButton._sync = function() {{ origMainSync && origMainSync(); renderButtons(); }};
      }}
      if (wa && wa.SecondaryButton) {{
        const origSecSync = wa.SecondaryButton._sync;
        wa.SecondaryButton._sync = function() {{ origSecSync && origSecSync(); renderButtons(); }};
      }}
    }})();
  </script>
</head>
<body>
  <div id="app"></div>
  <div class="bar">
    <button id="tg-sec-btn" class="btn-sec" style="display:none" onclick="window.Telegram.WebApp.SecondaryButton._fireClick && window.Telegram.WebApp.SecondaryButton._fireClick()"></button>
    <button id="tg-main-btn" class="btn-main" style="display:none" onclick="window.Telegram.WebApp.MainButton._fireClick && window.Telegram.WebApp.MainButton._fireClick()"></button>
  </div>
  <script>
    let SCHEMA = {schema_json};
    let STATE = {state_json};
    const API_BASE = '/api/v1/miniapps/{bot_un}/{short}';
    async function dispatchAction(actionType, widgetId, value) {{
      const res = await fetch(API_BASE + '/action', {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json' }},
        body: JSON.stringify({{ action_type: actionType, widget_id: widgetId, value }})
      }});
      if (res.ok) {{
        const data = await res.json();
        if (data.ui_schema) SCHEMA = data.ui_schema;
        if (data.state_json) STATE = data.state_json;
        renderApp();
      }}
    }}
    function renderApp() {{
      const root = document.getElementById('app');
      root.innerHTML = '';
      const hdr = document.createElement('div');
      hdr.className = 'hdr';
      hdr.innerHTML = '<div class="hdr-icon">{icon}</div><div><h1>' + (SCHEMA.title || '{title}') + '</h1><p>' + (SCHEMA.subtitle || '@{bot_un}') + '</p></div>';
      root.appendChild(hdr);
      (SCHEMA.widgets || []).forEach(w => {{
        const card = document.createElement('div'); card.className = 'card';
        const lbl = document.createElement('label'); lbl.textContent = w.label || w.id; card.appendChild(lbl);
        if (w.kind === 'TextInput') {{
          const inp = document.createElement('input');
          inp.type = 'text'; inp.value = w.value || ''; inp.placeholder = w.placeholder || '';
          inp.onchange = (e) => dispatchAction('widget_event', w.id, e.target.value);
          card.appendChild(inp);
        }} else if (w.kind === 'Select') {{
          const seg = document.createElement('div'); seg.className = 'seg';
          (w.options || []).forEach(opt => {{
            const b = document.createElement('button');
            b.textContent = opt;
            if (w.value === opt) b.className = 'active';
            b.onclick = () => dispatchAction('widget_event', w.id, opt);
            seg.appendChild(b);
          }});
          card.appendChild(seg);
        }} else if (w.kind === 'Counter') {{
          const row = document.createElement('div'); row.className = 'seg';
          const dec = document.createElement('button'); dec.textContent = '-';
          const val = document.createElement('button'); val.textContent = String(w.count); val.className = 'active';
          const inc = document.createElement('button'); inc.textContent = '+';
          dec.onclick = () => dispatchAction('widget_event', w.id, Math.max(w.min ?? 0, w.count - 1));
          inc.onclick = () => dispatchAction('widget_event', w.id, Math.min(w.max ?? 999, w.count + 1));
          row.appendChild(dec); row.appendChild(val); row.appendChild(inc);
          card.appendChild(row);
        }} else if (w.kind === 'MetricCard') {{
          const m = document.createElement('div'); m.className = 'metric';
          m.textContent = w.value || String(w.count);
          card.appendChild(m);
        }} else {{
          const c = document.createElement('div'); c.className = 'callout';
          c.textContent = w.value || '';
          card.appendChild(c);
        }}
        root.appendChild(card);
      }});
      if (SCHEMA.main_button && SCHEMA.main_button.is_visible !== false) {{
        window.Telegram.WebApp.MainButton.setText(SCHEMA.main_button.text || 'Submit').show().onClick(() => dispatchAction('main_button_click', null, null));
      }}
      if (SCHEMA.secondary_button && SCHEMA.secondary_button.is_visible !== false) {{
        window.Telegram.WebApp.SecondaryButton.setText(SCHEMA.secondary_button.text || 'Send').show().onClick(() => dispatchAction('secondary_button_click', null, null));
      }}
      window.Telegram.WebApp.ready();
      window.Telegram.WebApp.expand();
    }}
    renderApp();
  </script>
</body>
</html>"#,
        title = app_title_escaped,
        bot_un = bot_un_escaped,
        bot_uid = bot.bot_user_id,
        short = short_escaped,
        icon = html_escape(&app.icon_emoji),
        init_data_escaped = init_data_escaped,
        schema_json = schema_json,
        state_json = state_json,
    )
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// ============================================================================
// HTTP Route Handlers
// ============================================================================

fn build_installed_map(state: &AppState, user_id: i64) -> HashMap<String, (i64, bool)> {
    state
        .db
        .list_user_installed_mini_apps(user_id)
        .unwrap_or_default()
        .into_iter()
        .map(|(a, inst, _)| (a.app_id, (inst.companion_chat_id, inst.allow_write_access)))
        .collect()
}

/// GET /api/v1/miniapps
pub async fn miniapps_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let q = params.get("q").map(String::as_str).unwrap_or("");
    let cat = params.get("category").map(String::as_str);
    let uid = auth.user_id.0 as i64;
    let installed_map = build_installed_map(&state, uid);
    match state.db.search_published_mini_apps(q, cat) {
        Ok(list) => {
            let apps: Vec<MiniAppEntry> = list
                .iter()
                .map(|a| miniapp_record_to_entry(&state, a, uid, Some(&installed_map)))
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "apps": apps, "total": apps.len() })),
            )
                .into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

/// GET /api/v1/miniapps/installed
pub async fn miniapps_installed(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    let uid = auth.user_id.0 as i64;
    let installed_map = build_installed_map(&state, uid);
    match state.db.list_user_installed_mini_apps(uid) {
        Ok(list) => {
            let apps: Vec<MiniAppEntry> = list
                .iter()
                .map(|(a, _, _)| miniapp_record_to_entry(&state, a, uid, Some(&installed_map)))
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "apps": apps, "total": apps.len() })),
            )
                .into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

/// GET /api/v1/bots/my
pub async fn bots_my(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    match state.db.list_bots_by_owner(auth.user_id.0 as i64) {
        Ok(list) => {
            let bots: Vec<BotRegistryEntry> = list.iter().map(bot_record_to_entry).collect();
            (
                StatusCode::OK,
                Json(json!({ "bots": bots, "total": bots.len() })),
            )
                .into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

/// GET /api/v1/bots/attach-menu
pub async fn bots_attach_menu(auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    let viewer_uid = auth.user_id.0 as i64;
    match state.db.list_attach_menu_bots(viewer_uid) {
        Ok(list) => {
            let bots: Vec<BotRegistryEntry> = list
                .into_iter()
                .map(|(b, _, _)| bot_record_to_entry_for_viewer(&state, &b, viewer_uid))
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "bots": bots, "total": bots.len() })),
            )
                .into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

/// POST /api/v1/bots
pub async fn bots_create(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateBotRequest>,
) -> Response {
    let cmds = if req.commands.is_empty() {
        vec![
            BotCommandSpec { command: "start".into(), description: "开始使用机器人".into() },
            BotCommandSpec { command: "help".into(), description: "查看使用帮助".into() },
        ]
    } else {
        req.commands
    };
    let cmds_json = serde_json::to_string(&cmds).unwrap_or_else(|_| "[]".into());
    let ai_persona = if req.ai_persona.trim().is_empty() {
        format!(
            "你是 {} (@{}) AI 助手。{}",
            req.display_name.trim(),
            req.username.trim().trim_start_matches('@'),
            req.description.trim()
        )
    } else {
        req.ai_persona.clone()
    };

    let mut bot = match state.db.create_registered_bot(
        auth.user_id.0 as i64,
        &req.username,
        &req.display_name,
        &req.description,
        &req.about,
        &ai_persona,
        req.menu_button_text.as_deref().unwrap_or(""),
        req.menu_button_url.as_deref().unwrap_or(""),
        &cmds_json,
        false,
        req.allow_attach_menu,
    ) {
        Ok(b) => b,
        Err(e) => return map_telegram_err(e),
    };

    if req.agent_id.is_some()
        || req.avatar_shape.is_some()
        || req.avatar_color.is_some()
        || req.preferred_model.is_some()
        || req.thinking_level.is_some()
        || req.approval_mode.is_some()
        || req.toolset_preset.is_some()
    {
        if let Ok(updated) = state.db.update_bot_fabushi_fields(
            bot.bot_user_id,
            None,
            None,
            None,
            None,
            req.agent_id.as_deref(),
            req.avatar_shape.as_deref(),
            req.avatar_color.as_deref(),
            req.preferred_model.as_deref(),
            req.thinking_level.as_deref(),
            req.approval_mode.as_deref(),
            req.toolset_preset.as_deref(),
        ) {
            bot = updated;
        }
    }

    crate::routes::hermes::save_bot_persona_kv(
        &state,
        UserId(bot.bot_user_id as u64),
        None,
        &bot.display_name,
        Some(&bot.ai_persona),
        Some(&bot.ai_persona),
        req.preferred_model.as_deref(),
        None,
    );
    let _ = provision_fabushi_bot_with_host(&state.db, &state.fabushi_bots, &bot);

    let companion_cid = ensure_companion_bot_dm_with_welcome(
        &state,
        auth.user_id,
        UserId(bot.bot_user_id as u64),
    )
    .ok()
    .map(|(cid, _)| cid.0);

    let resp = BotTokenResponse {
        bot: bot_record_to_entry(&bot),
        bot_token: bot.bot_token,
        companion_chat_id: companion_cid,
    };
    (StatusCode::CREATED, Json(resp)).into_response()
}

fn resolve_bot_record_by_key(
    state: &AppState,
    key: &str,
) -> gpui_telegram_core::error::Result<Option<BotRegistryRecord>> {
    let trimmed = key.trim().trim_start_matches('@');
    if trimmed.is_empty() {
        return Ok(None);
    }
    if let Ok(uid) = trimmed.parse::<i64>() {
        if let Some(found) = state.db.get_bot_by_user_id(uid)? {
            return Ok(Some(found));
        }
    }
    if let Some(inst) = state.fabushi_bots.get_by_agent_id_or_username(trimmed) {
        if let Some(found) = state.db.get_bot_by_user_id(inst.bot_user_id)? {
            return Ok(Some(found));
        }
    }
    state.db.get_bot_by_username(trimmed)
}

/// GET /api/v1/fabushi-bots
pub async fn fabushi_bots_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let is_admin = crate::routes::hermes::caller_is_admin(&state, auth.user_id);
    match state.db.list_all_bots() {
        Ok(records) => {
            let bots: Vec<FabushiBotSummaryDto> = records
                .iter()
                .filter(|b| {
                    b.bot_user_id != BOTFATHER_USER_ID as i64
                        && !b.is_standard_bot
                        && (is_admin || b.is_official || b.owner_user_id == auth.user_id.0 as i64)
                })
                .map(|b| build_fabushi_bot_summary(&state, auth.user_id.0 as i64, b, None))
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "bots": bots, "total": bots.len() })),
            )
                .into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

/// POST /api/v1/fabushi-bots
pub async fn fabushi_bots_create(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateFabushiBotRequest>,
) -> Response {
    let display_name = req.display_name.trim();
    if display_name.is_empty() {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_display_name",
            "display_name cannot be empty",
        );
    }

    let desired_un = req
        .username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(display_name);
    let username = match state.db.generate_unique_bot_username(desired_un) {
        Ok(u) => u,
        Err(e) => return map_telegram_err(e),
    };

    let cmds = [
        BotCommandSpec {
            command: "start".into(),
            description: "开始使用智能体".into(),
        },
        BotCommandSpec {
            command: "help".into(),
            description: "查看使用帮助".into(),
        },
        BotCommandSpec {
            command: "new".into(),
            description: "开启全新会话".into(),
        },
    ];
    let cmds_json = serde_json::to_string(&cmds).unwrap_or_else(|_| "[]".into());
    let title = if req.title.trim().is_empty() {
        "Fabushi Agent".to_string()
    } else {
        req.title.trim().to_string()
    };
    let description = req.description.trim().to_string();
    let ai_persona = if description.is_empty() {
        format!(
            "你是 {} (@{})，一个拥有独立记忆与技能空间的 Fabushi Agent 智能体。",
            display_name, username
        )
    } else {
        description.clone()
    };

    let mut bot = match state.db.create_registered_bot(
        auth.user_id.0 as i64,
        &username,
        display_name,
        if description.is_empty() {
            &title
        } else {
            &description
        },
        &title,
        &ai_persona,
        "",
        "",
        &cmds_json,
        false,
        false,
    ) {
        Ok(b) => b,
        Err(e) => return map_telegram_err(e),
    };

    if let Ok(updated) = state.db.update_bot_fabushi_fields(
        bot.bot_user_id,
        Some(display_name),
        Some(if description.is_empty() {
            title.as_str()
        } else {
            description.as_str()
        }),
        Some(&title),
        Some(&ai_persona),
        req.agent_id.as_deref(),
        req.avatar_shape.as_deref(),
        req.avatar_color.as_deref(),
        req.preferred_model.as_deref(),
        req.thinking_level.as_deref(),
        req.approval_mode.as_deref(),
        req.toolset_preset.as_deref(),
    ) {
        bot = updated;
    }

    crate::routes::hermes::save_bot_persona_kv(
        &state,
        UserId(bot.bot_user_id as u64),
        None,
        &bot.display_name,
        Some(&bot.ai_persona),
        Some(&bot.ai_persona),
        req.preferred_model.as_deref(),
        None,
    );
    let inst = provision_fabushi_bot_with_host(&state.db, &state.fabushi_bots, &bot);

    let companion_cid = ensure_companion_bot_dm_with_welcome(
        &state,
        auth.user_id,
        UserId(bot.bot_user_id as u64),
    )
    .ok()
    .map(|(cid, _)| cid.0)
    .unwrap_or(0);

    let summary =
        build_fabushi_bot_summary(&state, auth.user_id.0 as i64, &bot, inst.as_deref());
    let resp = FabushiBotCreateResponse {
        bot: summary,
        registry: bot_record_to_entry(&bot),
        bot_token: bot.bot_token,
        companion_chat_id: companion_cid,
    };
    (StatusCode::CREATED, Json(resp)).into_response()
}

/// GET /api/v1/fabushi-bots/{bot_id}
pub async fn fabushi_bot_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> Response {
    match resolve_bot_record_by_key(&state, &bot_id) {
        Ok(Some(bot)) if bot.bot_user_id != BOTFATHER_USER_ID as i64 => {
            if !bot.is_official {
                if let Err(resp) = crate::routes::hermes::caller_owns_bot_or_admin(
                    &state,
                    auth.user_id,
                    bot.bot_user_id as u64,
                ) {
                    return resp;
                }
            }
            let summary = build_fabushi_bot_summary(&state, auth.user_id.0 as i64, &bot, None);
            (StatusCode::OK, Json(summary)).into_response()
        }
        Ok(_) => err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Fabushi bot not found"),
        Err(e) => map_telegram_err(e),
    }
}

/// PATCH /api/v1/fabushi-bots/{bot_id}
pub async fn fabushi_bot_patch(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    Json(req): Json<PatchFabushiBotRequest>,
) -> Response {
    let bot = match resolve_bot_record_by_key(&state, &bot_id) {
        Ok(Some(b)) if b.bot_user_id != BOTFATHER_USER_ID as i64 => b,
        Ok(_) => return err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Fabushi bot not found"),
        Err(e) => return map_telegram_err(e),
    };

    if let Err(resp) = crate::routes::hermes::caller_owns_bot_or_admin(
        &state,
        auth.user_id,
        bot.bot_user_id as u64,
    ) {
        return resp;
    }

    let updated = match state.db.update_bot_fabushi_fields(
        bot.bot_user_id,
        req.display_name.as_deref(),
        req.description.as_deref(),
        req.title.as_deref(),
        req.description.as_deref(),
        None,
        req.avatar_shape.as_deref(),
        req.avatar_color.as_deref(),
        req.preferred_model.as_deref(),
        req.thinking_level.as_deref(),
        req.approval_mode.as_deref(),
        req.toolset_preset.as_deref(),
    ) {
        Ok(u) => u,
        Err(e) => return map_telegram_err(e),
    };

    let _ = ensure_fabushi_bot_hydrated(&state, updated.bot_user_id);
    let inst = state
        .fabushi_bots
        .update_bot(
            updated.bot_user_id,
            FabushiBotPatch {
                display_name: req.display_name.clone(),
                description: req.description.clone(),
                title: req.title.clone(),
                avatar_shape: req.avatar_shape.clone(),
                avatar_color: req.avatar_color.clone(),
                preferred_model: req.preferred_model.clone(),
                thinking_level: req.thinking_level.clone(),
                approval_mode: req.approval_mode.clone(),
                toolset_preset: req.toolset_preset.clone(),
                notify_on_agent_updates: req.notify_on_agent_updates,
                hidden_from_sidebar: req.hidden_from_sidebar,
            },
        )
        .ok();

    crate::routes::hermes::save_bot_persona_kv(
        &state,
        UserId(updated.bot_user_id as u64),
        None,
        &updated.display_name,
        Some(&updated.ai_persona),
        Some(&updated.ai_persona),
        if updated.preferred_model.trim().is_empty() {
            None
        } else {
            Some(updated.preferred_model.as_str())
        },
        None,
    );

    let summary =
        build_fabushi_bot_summary(&state, auth.user_id.0 as i64, &updated, inst.as_deref());
    (StatusCode::OK, Json(summary)).into_response()
}

/// POST /api/v1/miniapps
pub async fn miniapps_create(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateMiniAppRequest>,
) -> Response {
    let slug = normalize_bot_slug(&req.bot_username);
    let Some(bot) = state.db.get_bot_by_username(&slug).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Companion bot not found");
    };
    if bot.owner_user_id != auth.user_id.0 as i64 {
        return err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Only the bot owner can register Mini Apps under this bot",
        );
    }

    let bp = synthesize_nl_blueprint(&format!("{} {}", req.title, req.description));
    let schema = match req.ui_schema {
        Some(mut s) if !s.widgets.is_empty() || !s.title.is_empty() => {
            if s.title.is_empty() {
                s.title = req.title.clone();
            }
            if s.subtitle.is_empty() {
                s.subtitle = req.description.clone();
            }
            s
        }
        _ => {
            let mut s = bp.ui_schema;
            s.title = req.title.clone();
            if !req.description.trim().is_empty() {
                s.subtitle = req.description.clone();
            }
            s
        }
    };
    let schema_str = serde_json::to_string(&schema).unwrap_or_else(|_| "{}".into());

    match state.db.create_or_upsert_mini_app(
        bot.bot_user_id,
        &bot.username,
        &req.short_name,
        &req.title,
        &req.description,
        if req.category.trim().is_empty() {
            &bp.app_category
        } else {
            &req.category
        },
        if req.icon_emoji.trim().is_empty() {
            &bp.app_icon
        } else {
            &req.icon_emoji
        },
        req.web_url.as_deref(),
        &schema_str,
        true,
        req.is_attach_enabled,
        false,
    ) {
        Ok(app) => {
            let entry = miniapp_record_to_entry(&state, &app, auth.user_id.0 as i64, None);
            (StatusCode::CREATED, Json(entry)).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

/// POST /api/v1/botfather/nl-create
pub async fn botfather_nl_create(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Json(req): Json<NaturalLanguageCreateRequest>,
) -> Response {
    let prompt = req.prompt.trim();
    if prompt.is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_prompt", "prompt cannot be empty");
    }
    let (bot, app) = match execute_nl_create_with_avatar(
        &state,
        auth.user_id,
        prompt,
        req.avatar_shape.as_deref(),
        req.avatar_color.as_deref(),
    ) {
        Ok(res) => res,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, "nl_create_failed", &e),
    };

    let (companion_cid, _) = ensure_companion_bot_dm_with_welcome(
        &state,
        auth.user_id,
        UserId(bot.bot_user_id as u64),
    )
    .unwrap_or((ChatId(0), false));

    let (summary_markdown, _) = format_nl_created_reply(&bot, &app);
    let resp = NaturalLanguageCreateResponse {
        bot: bot_record_to_entry(&bot),
        app: Some(miniapp_record_to_entry(
            &state,
            &app,
            auth.user_id.0 as i64,
            None,
        )),
        bot_token: bot.bot_token,
        companion_chat_id: companion_cid.0,
        summary_markdown,
    };
    (StatusCode::CREATED, Json(resp)).into_response()
}

/// POST /api/v1/miniapps/{bot_username}/{short_name}/install
pub async fn miniapps_install(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((bot_username, short_name)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let req: InstallMiniAppRequest = if body.is_empty() {
        InstallMiniAppRequest { allow_write_access: true }
    } else {
        serde_json::from_slice(&body).unwrap_or(InstallMiniAppRequest { allow_write_access: true })
    };

    let Some(app) = state.db.get_mini_app(&bot_username, &short_name).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "miniapp_not_found", "Mini App not found");
    };
    let Some(bot) = state.db.get_bot_by_user_id(app.bot_user_id).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Companion bot not found");
    };

    let bot_uid_u64 = bot.bot_user_id.max(0) as u64;
    let owns_bot =
        crate::routes::hermes::caller_owns_bot_or_admin(&state, auth.user_id, bot_uid_u64).is_ok();
    let is_published_real_app = app.is_published && app.category != ADHOC_WEBAPP_CATEGORY;
    if !owns_bot && !is_published_real_app {
        return err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Only the bot owner or server admin can install unpublished or ad-hoc Mini Apps",
        );
    }

    let (companion_cid, _) = match ensure_companion_bot_dm_with_welcome(
        &state,
        auth.user_id,
        UserId(app.bot_user_id as u64),
    ) {
        Ok(res) => res,
        Err(e) => return map_telegram_err(e),
    };

    if let Err(e) = state.db.install_or_touch_user_mini_app(
        auth.user_id.0 as i64,
        &app.app_id,
        app.bot_user_id,
        companion_cid.0,
        req.allow_write_access,
    ) {
        return map_telegram_err(e);
    }
    let _ = state.db.set_user_bot_write_access(
        auth.user_id.0 as i64,
        app.bot_user_id,
        req.allow_write_access,
    );

    let updated_app = state
        .db
        .get_mini_app(&bot.username, &app.short_name)
        .ok()
        .flatten()
        .unwrap_or(app);

    let entry = miniapp_record_to_entry(&state, &updated_app, auth.user_id.0 as i64, None);
    let bot_entry = bot_record_to_entry_for_viewer(&state, &bot, auth.user_id.0 as i64);
    let toast = format!(
        "已添加「{}」，并为你开通伴生 AI 机器人 @{}",
        entry.title, bot_entry.username
    );

    (
        StatusCode::OK,
        Json(PeerBotActionResult {
            ok: true,
            app: Some(entry),
            companion_bot: Some(bot_entry),
            companion_chat_id: Some(companion_cid.0),
            system_message: Some(toast.clone()),
            toast: Some(toast),
            ..Default::default()
        }),
    )
        .into_response()
}

/// DELETE /api/v1/miniapps/{bot_username}/{short_name}/install
pub async fn miniapps_uninstall(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((bot_username, short_name)): Path<(String, String)>,
) -> Response {
    let Some(app) = state.db.get_mini_app(&bot_username, &short_name).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "miniapp_not_found", "Mini App not found");
    };
    let deleted = state
        .db
        .uninstall_user_mini_app(auth.user_id.0 as i64, &app.app_id)
        .unwrap_or(false);
    let msg = if deleted {
        format!("已从我的小程序移除「{}」", app.title)
    } else {
        "小程序未安装".to_string()
    };
    (
        StatusCode::OK,
        Json(PeerBotActionResult {
            ok: deleted,
            toast: Some(msg),
            ..Default::default()
        }),
    )
        .into_response()
}

const ADHOC_WEBAPP_CATEGORY: &str = "__bot_inline_webapp__";

fn resolve_app_and_bot(
    state: &AppState,
    bot_target: &str,
    short_name: &str,
    custom_url_hint: Option<&str>,
    persist_for_session: bool,
) -> Option<(MiniAppRecord, BotRegistryRecord)> {
    let clean_short = short_name.trim().trim_start_matches('/');
    let clean_custom = custom_url_hint
        .and_then(gpui_telegram_proto::miniapp_contracts::sanitize_webapp_custom_url);

    if let Some(mut app) = state.db.get_mini_app(bot_target, clean_short).ok().flatten() {
        let bot = state.db.get_bot_by_user_id(app.bot_user_id).ok().flatten()?;
        if app.category == ADHOC_WEBAPP_CATEGORY {
            if let Some(ref u) = clean_custom {
                app.web_url = u.clone();
            } else if let Some(menu_url) =
                gpui_telegram_proto::miniapp_contracts::sanitize_webapp_custom_url(
                    &bot.menu_button_url,
                )
            {
                app.web_url = menu_url;
            }
        }
        return Some((app, bot));
    }

    let clean_bot = bot_target.trim().trim_start_matches('@');
    let bot = if clean_bot.is_empty() {
        None
    } else if let Some(b) = state.db.get_bot_by_username(clean_bot).ok().flatten() {
        Some(b)
    } else if let Some(bot_uid) = clean_bot.parse::<i64>().ok().filter(|&id| id > 0) {
        state.db.get_bot_by_user_id(bot_uid).ok().flatten()
    } else {
        None
    }?;

    let apps = state
        .db
        .list_mini_apps_for_bot(bot.bot_user_id)
        .unwrap_or_default();
    if let Some(mut found) = apps
        .iter()
        .find(|a| a.short_name.eq_ignore_ascii_case(clean_short))
        .cloned()
    {
        if found.category == ADHOC_WEBAPP_CATEGORY {
            if let Some(ref u) = clean_custom {
                found.web_url = u.clone();
            } else if let Some(menu_url) =
                gpui_telegram_proto::miniapp_contracts::sanitize_webapp_custom_url(
                    &bot.menu_button_url,
                )
            {
                found.web_url = menu_url;
            }
        }
        return Some((found, bot));
    }

    let is_alias = clean_short.is_empty()
        || clean_short.eq_ignore_ascii_case("app")
        || clean_short.eq_ignore_ascii_case("default")
        || clean_short.eq_ignore_ascii_case("webview");
    if is_alias {
        if !bot.main_app_short_name.trim().is_empty() {
            if let Some(main_app) = apps
                .iter()
                .find(|a| {
                    a.short_name
                        .eq_ignore_ascii_case(bot.main_app_short_name.trim())
                        && a.category != ADHOC_WEBAPP_CATEGORY
                })
                .cloned()
            {
                return Some((main_app, bot));
            }
        }
        if let Some(first_pub) = apps
            .iter()
            .find(|a| a.is_published && a.category != ADHOC_WEBAPP_CATEGORY)
            .cloned()
        {
            return Some((first_pub, bot));
        }
    }

    if !is_alias {
        return None;
    }
    let menu_url = gpui_telegram_proto::miniapp_contracts::sanitize_webapp_custom_url(
        &bot.menu_button_url,
    )
    .filter(|u| !u.starts_with("/miniapps/"));
    if clean_custom.is_none() && menu_url.is_none() {
        return None;
    }

    let resolved_short = if gpui_telegram_proto::miniapp_contracts::validate_miniapp_short_name(
        clean_short,
    )
    .is_ok()
    {
        clean_short.to_string()
    } else {
        "app".to_string()
    };
    let resolved_title = if !bot.menu_button_text.trim().is_empty() {
        bot.menu_button_text.trim().to_string()
    } else if !bot.display_name.trim().is_empty() {
        bot.display_name.trim().to_string()
    } else {
        bot.username.clone()
    };
    let resolved_url = clean_custom.or(menu_url).unwrap_or_default();

    if persist_for_session {
        if let Ok(persisted) = state.db.create_or_upsert_mini_app_with_options(
            bot.bot_user_id,
            &bot.username,
            &resolved_short,
            &resolved_title,
            &bot.about,
            ADHOC_WEBAPP_CATEGORY,
            "🧩",
            if resolved_url.is_empty() {
                None
            } else {
                Some(&resolved_url)
            },
            "{}",
            false,
            bot.allow_attach_menu,
            false,
            "",
            "",
            Some(false),
            false,
        ) {
            return Some((persisted, bot));
        }
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let app = MiniAppRecord {
        app_id: format!(
            "{}_{}",
            bot.bot_user_id,
            resolved_short.to_ascii_lowercase()
        ),
        bot_user_id: bot.bot_user_id,
        bot_username: bot.username.clone(),
        short_name: resolved_short,
        title: resolved_title,
        description: bot.about.clone(),
        category: ADHOC_WEBAPP_CATEGORY.to_string(),
        icon_emoji: "🧩".to_string(),
        web_url: resolved_url,
        ui_schema_json: "{}".to_string(),
        peer_types_json: "[\"users\",\"bots\",\"groups\"]".to_string(),
        is_published: false,
        is_attach_enabled: bot.allow_attach_menu,
        install_count: 0,
        created_by_nl: false,
        created_at: now_ms,
        updated_at: now_ms,
        header_color: String::new(),
        background_color: String::new(),
    };
    Some((app, bot))
}

/// POST /api/v1/miniapps/{bot_username}/{short_name}/launch
pub async fn miniapps_launch(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((bot_username, short_name)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let req: LaunchMiniAppRequest = if body.is_empty() {
        LaunchMiniAppRequest::default()
    } else {
        serde_json::from_slice(&body).unwrap_or_default()
    };

    let Some((mut app, bot)) = resolve_app_and_bot(
        &state,
        &bot_username,
        &short_name,
        req.custom_url.as_deref(),
        false,
    ) else {
        return err_resp(StatusCode::NOT_FOUND, "miniapp_not_found", "Mini App not found");
    };

    let bot_uid_u64 = bot.bot_user_id.max(0) as u64;
    let owns_bot =
        crate::routes::hermes::caller_owns_bot_or_admin(&state, auth.user_id, bot_uid_u64).is_ok();
    let has_external_menu_url = gpui_telegram_proto::miniapp_contracts::sanitize_webapp_custom_url(
        &bot.menu_button_url,
    )
    .is_some_and(|u| !u.starts_with("/miniapps/"));
    let is_published_real_app = app.is_published && app.category != ADHOC_WEBAPP_CATEGORY;

    if !owns_bot && !is_published_real_app {
        if app.category == ADHOC_WEBAPP_CATEGORY {
            if !has_external_menu_url {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Only the bot owner or server admin can launch an ad-hoc Mini App without a configured menu URL",
                );
            }
        } else {
            return err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Only the bot owner or server admin can launch an unpublished Mini App",
            );
        }
    }

    if app.category == ADHOC_WEBAPP_CATEGORY && (owns_bot || has_external_menu_url) {
        let persist_custom_url = if owns_bot {
            req.custom_url.as_deref()
        } else {
            None
        };
        if let Some((persisted_app, _)) = resolve_app_and_bot(
            &state,
            &bot_username,
            &short_name,
            persist_custom_url,
            true,
        ) {
            app = persisted_app;
        }
    }

    let uid = auth.user_id.0 as i64;
    if app.is_published && app.category != ADHOC_WEBAPP_CATEGORY {
        let _ = state.db.record_mini_app_launch(uid, &app.app_id, app.bot_user_id);
    }

    let existing_chat_id = state
        .db
        .find_companion_chat_id_for_user(app.bot_user_id, uid)
        .ok()
        .flatten()
        .map(|c| c.0)
        .unwrap_or(0);

    let is_blocked = state
        .db
        .is_user_blocked(auth.user_id, UserId(app.bot_user_id as u64))
        .unwrap_or(false);
    let perm = state
        .db
        .get_user_bot_permission(uid, app.bot_user_id)
        .ok();
    let allows_write_to_pm = !is_blocked && perm.as_ref().is_some_and(|p| p.allows_write_to_pm);

    let user = state.db.get_user(auth.user_id).ok().flatten();
    let user_obj = json!({
        "id": auth.user_id.0,
        "first_name": user.as_ref().map(|u| u.first_name.as_str()).unwrap_or("User"),
        "last_name": user.as_ref().and_then(|u| u.last_name.as_deref()),
        "username": user.as_ref().and_then(|u| u.username.as_deref()),
        "language_code": "zh-hans",
        "allows_write_to_pm": allows_write_to_pm
    });
    let user_json = serde_json::to_string(&user_obj).unwrap_or_else(|_| "{}".into());

    let query_id = format!("AAH{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    let auth_date = chrono::Utc::now().timestamp();
    let effective_chat_id = req.chat_id.unwrap_or(existing_chat_id);
    let chat_type = if req.chat_id.is_some() {
        "group".to_string()
    } else {
        "sender".to_string()
    };
    let chat_instance = if effective_chat_id > 0 {
        effective_chat_id.to_string()
    } else {
        format!("app_{}", app.bot_user_id)
    };

    let mut sign_params = vec![
        ("auth_date".to_string(), auth_date.to_string()),
        ("chat_instance".to_string(), chat_instance.clone()),
        ("chat_type".to_string(), chat_type.clone()),
        ("query_id".to_string(), query_id.clone()),
        ("user".to_string(), user_json.clone()),
    ];
    if let Some(sp) = req.start_param.as_deref().filter(|s| !s.trim().is_empty()) {
        sign_params.push(("start_param".to_string(), sp.trim().to_string()));
    }

    let (raw_query_string, _data_check_string, hash) =
        sign_webapp_init_data(&bot.bot_token, &sign_params);

    let initial_state = state
        .db
        .get_latest_mini_app_session_for_user(uid, &app.app_id)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Value>(&s.state_json).ok())
        .unwrap_or_else(|| initial_state_for_app(&app));

    let launch_mode_str = serde_json::to_string(&req.launch_mode)
        .unwrap_or_else(|_| "\"CompactSheet\"".into())
        .trim_matches('"')
        .to_string();

    let _ = state.db.create_mini_app_session(
        &query_id,
        uid,
        &app.app_id,
        app.bot_user_id,
        effective_chat_id,
        &launch_mode_str,
        req.start_param.as_deref().unwrap_or(""),
        &raw_query_string,
        auth_date,
        &initial_state.to_string(),
    );

    let updated_app = if app.category == ADHOC_WEBAPP_CATEGORY {
        app.clone()
    } else {
        state
            .db
            .get_mini_app(&bot.username, &app.short_name)
            .ok()
            .flatten()
            .unwrap_or(app)
    };
    let mut app_entry = miniapp_record_to_entry(&state, &updated_app, uid, None);
    if let Some(override_url) = req
        .custom_url
        .as_deref()
        .and_then(gpui_telegram_proto::miniapp_contracts::sanitize_webapp_custom_url)
    {
        app_entry.web_url = override_url;
    }
    let cloud_storage = state
        .db
        .list_cloud_storage_items(uid, bot.bot_user_id)
        .unwrap_or_default();

    let mut companion_bot_entry = bot_record_to_entry_for_viewer(&state, &bot, uid);
    if !bot.is_official && !owns_bot {
        companion_bot_entry.ai_persona.clear();
    }

    let ctx = MiniAppLaunchContext {
        app: app_entry,
        companion_bot: companion_bot_entry,
        companion_chat_id: existing_chat_id,
        source_chat_id: req.chat_id,
        launch_mode: req.launch_mode,
        start_param: req.start_param.clone(),
        init_data: MiniAppInitDataDto {
            query_id,
            user_json,
            auth_date,
            hash,
            start_param: req.start_param,
            chat_type: Some(chat_type),
            chat_instance: Some(chat_instance),
            raw_query_string,
            hmac_verified: true,
        },
        cloud_storage,
        state_json: initial_state,
    };
    (StatusCode::OK, Json(ctx)).into_response()
}

/// POST /api/v1/miniapps/{bot_username}/{short_name}/validate-init-data
pub async fn miniapps_validate_init_data(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((bot_username, short_name)): Path<(String, String)>,
    Json(req): Json<ValidateInitDataRequest>,
) -> Response {
    let Some((app, bot)) = resolve_app_and_bot(&state, &bot_username, &short_name, None, false)
    else {
        return err_resp(StatusCode::NOT_FOUND, "miniapp_not_found", "Mini App not found");
    };

    match verify_webapp_init_data(&bot.bot_token, &req.init_data) {
        Ok(params) => {
            let user_id = params
                .get("user")
                .and_then(|u| serde_json::from_str::<Value>(u).ok())
                .and_then(|v| v.get("id").and_then(|id| id.as_i64()));
            let query_id = params.get("query_id").cloned();
            let auth_date = params.get("auth_date").and_then(|d| d.parse::<i64>().ok());
            (
                StatusCode::OK,
                Json(ValidateInitDataResponse {
                    valid: true,
                    bot_username: bot.username,
                    short_name: app.short_name,
                    user_id,
                    query_id,
                    auth_date,
                    reason: None,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::OK,
            Json(ValidateInitDataResponse {
                valid: false,
                bot_username: bot.username,
                short_name: app.short_name,
                user_id: None,
                query_id: None,
                auth_date: None,
                reason: Some(e.to_string()),
            }),
        )
            .into_response(),
    }
}

/// POST /api/v1/miniapps/{bot_username}/{short_name}/action
pub async fn miniapps_action(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((bot_username, short_name)): Path<(String, String)>,
    Json(req): Json<MiniAppActionRequest>,
) -> Response {
    let Some((app, bot)) = resolve_app_and_bot(&state, &bot_username, &short_name, None, false)
    else {
        return err_resp(StatusCode::NOT_FOUND, "miniapp_not_found", "Mini App not found");
    };
    let uid = auth.user_id.0 as i64;

    let companion_cid = state
        .db
        .find_companion_chat_id_for_user(bot.bot_user_id, uid)
        .ok()
        .flatten()
        .unwrap_or(ChatId(0));

    let bot_uid_u64 = bot.bot_user_id.max(0) as u64;
    let owns_bot = crate::routes::hermes::caller_owns_bot_or_admin(&state, auth.user_id, bot_uid_u64).is_ok();
    let is_published_real_app = app.is_published && app.category != ADHOC_WEBAPP_CATEGORY;
    if !owns_bot && !is_published_real_app {
        return err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Only the bot owner or server admin can invoke actions on unpublished or ad-hoc Mini Apps",
        );
    }
    let in_shared_chat = req.chat_id.is_some_and(|cid| {
        cid > 0
            && check_active_membership(&state, ChatId(cid), auth.user_id).is_ok()
            && crate::routes::hermes::is_bot_member_of_chat(&state, bot_uid_u64, ChatId(cid))
    });
    let has_companion_or_shared_chat = companion_cid.0 > 0 || in_shared_chat;

    let existing_session = req
        .query_id
        .as_deref()
        .and_then(|qid| state.db.get_mini_app_session(qid).ok().flatten())
        .filter(|s| s.user_id == uid)
        .or_else(|| {
            state
                .db
                .get_latest_mini_app_session_for_user(uid, &app.app_id)
                .ok()
                .flatten()
        });

    let current_state = existing_session
        .as_ref()
        .and_then(|s| serde_json::from_str::<Value>(&s.state_json).ok())
        .unwrap_or_else(|| initial_state_for_app(&app));

    let base_schema: MiniAppUiSchema = serde_json::from_str(&app.ui_schema_json)
        .unwrap_or_else(|_| fallback_schema_for_app(&app));

    if req.action_type.trim() == "ai_copilot" {
        if !owns_bot && !bot.is_official && !has_companion_or_shared_chat {
            return err_resp(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Install the Mini App or share a chat with the bot to use AI Copilot",
            );
        }
        let user_prompt = req
            .prompt
            .as_deref()
            .or(req.data.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("请总结当前状态并给出下一步建议");

        let bot_user_id = UserId(bot_uid_u64);
        let allow_bot_persona = owns_bot || bot.is_official || companion_cid.0 > 0;
        let (_, bot_model, _) = if allow_bot_persona {
            crate::routes::hermes::load_bot_persona_config(&state, bot_user_id)
        } else {
            (None, None, None)
        };
        let copilot_persona = allow_bot_persona.then(|| bot.ai_persona.clone());
        let (enriched_sp, effective_model) =
            crate::routes::hermes::build_enriched_bot_turn_context(
                &state,
                uid.max(0) as u64,
                Some(bot_uid_u64),
                Some(&bot.display_name),
                copilot_persona,
                None,
                crate::routes::hermes::sanitize_model_override(&state, bot_model),
                &[],
                true,
            );
        let session_id_opt = crate::routes::hermes::resolve_and_apply_bot_session_policy(
            &state,
            uid.max(0) as u64,
            bot_user_id,
            Some(companion_cid),
            None,
            "miniapp_copilot",
            effective_model.as_deref(),
            false,
        )
        .ok()
        .map(|(sid, _)| sid);

        let caller_scope = crate::routes::hermes::build_caller_scope(&state, auth.user_id);
        let engine = crate::routes::hermes::resolve_hermes_for_bot(&state, Some(bot_uid_u64));

        let mut h_req = gpui_telegram_agent::hermes::HermesTurnRequest::new(format!(
            "[MiniApp: {} (@{}/{})]\nCurrent state: {}\nUser request: {}",
            app.title, bot.username, app.short_name, current_state, user_prompt
        ))
        .with_chat_bot(companion_cid.0, bot.bot_user_id.to_string());
        h_req.session_id = session_id_opt;
        h_req.bot_name = Some(bot.display_name.clone());
        h_req.user_id = Some(auth.user_id.0.to_string());
        h_req.source = "miniapp_copilot".to_string();
        h_req.preset = Some(gpui_telegram_agent::hermes::toolsets::ToolsetPreset::HermesTelegram);
        h_req.model = effective_model;
        h_req.system_prompt = enriched_sp;
        h_req.caller_allowed_chat_ids = Some(caller_scope.allowed_chat_ids);
        h_req.caller_is_admin = caller_scope.is_admin;
        h_req.caller_owns_bot = Some(owns_bot);
        h_req.skip_slash_commands = true;

        let (ok, ai_reply, next_state, toast) = match engine.run_turn(h_req).await {
            Ok(res) if !res.response_text.trim().is_empty() => {
                let raw = res.response_text.trim();
                let patches = crate::routes::hermes::extract_state_patches(raw);
                let cleaned = crate::routes::hermes::strip_state_patches_for_display(raw);
                let mut next_state = current_state.clone();
                if !next_state.is_object() {
                    next_state = json!({});
                }
                for p in &patches {
                    merge_json_patch(&mut next_state, p);
                }
                if let Some(obj) = next_state.as_object_mut() {
                    obj.insert("ai_summary".into(), json!(cleaned));
                }
                persist_session_state(
                    &state,
                    existing_session.as_ref(),
                    uid,
                    &app,
                    companion_cid.0,
                    &next_state,
                );
                (true, cleaned, next_state, "AI Copilot 已更新建议")
            }
            Ok(_) => (
                false,
                "⚠️ AI Copilot 未返回有效回复，请检查当前模型与 Provider 配置。".to_string(),
                current_state,
                "AI Copilot 未返回有效内容",
            ),
            Err(err) => (
                false,
                format!("⚠️ AI Copilot 调用失败：{err}"),
                current_state,
                "AI Copilot 调用失败",
            ),
        };
        let synced_schema = sync_ui_schema_with_state(&base_schema, &next_state);

        return (
            StatusCode::OK,
            Json(MiniAppActionResponse {
                ok,
                ui_schema: synced_schema,
                state_json: next_state,
                companion_bot_message: None,
                companion_chat_id: Some(companion_cid.0),
                toast: Some(toast.into()),
                close_app: false,
                ai_reply: Some(ai_reply),
            }),
        )
            .into_response();
    }

    let (next_state, toast, chat_sync_msg, close_app) =
        if app.short_name.eq_ignore_ascii_case("fabushi") {
            crate::routes::fabushi_miniapp_api::handle_headless_fabushi_action(
                &state,
                &bot,
                &app,
                auth.user_id,
                &req,
                current_state,
            )
        } else {
            reduce_mini_app_action(&app, current_state, &req)
        };
    persist_session_state(&state, existing_session.as_ref(), uid, &app, companion_cid.0, &next_state);
    let synced_schema = sync_ui_schema_with_state(&base_schema, &next_state);

    if req.action_type.trim() == "send_data" && bot.is_standard_bot {
        let raw_data = req
            .data
            .as_deref()
            .or_else(|| next_state.get("last_sent_data").and_then(|v| v.as_str()))
            .unwrap_or("")
            .trim();
        let dm_cid = if companion_cid.0 > 0 {
            companion_cid
        } else {
            state
                .db
                .ensure_companion_bot_chat_record(auth.user_id, UserId(bot.bot_user_id as u64))
                .map(|(cid, _)| cid)
                .unwrap_or(ChatId(0))
        };
        let _ = state.db.mark_user_started_bot_chat(uid, bot.bot_user_id);
        let msg_id = if dm_cid.0 > 0 {
            let mut item = MessageItem::new(
                MsgId(0),
                dm_cid,
                auth.user_id,
                PeerId::User(auth.user_id),
                MessageContent::plain_text(if raw_data.is_empty() {
                    app.title.as_str()
                } else {
                    raw_data
                }),
            );
            item.flags.is_out = true;
            if let Ok(pts) = state.message_pipeline.process_incoming_message(item) {
                let mid = match &pts.event {
                    gpui_telegram_proto::sync::UpdateEvent::NewMessage(m) => m.id,
                    _ => MsgId(0),
                };
                broadcast_chat_pts(&state, dm_cid, pts);
                mid
            } else {
                MsgId(0)
            }
        } else {
            MsgId(0)
        };
        enqueue_web_app_data_update(&state, &bot, &app, auth.user_id, msg_id, raw_data);
    }

    let mut delivered_chat_id = if companion_cid.0 > 0 {
        Some(companion_cid.0)
    } else {
        None
    };
    if !bot.is_standard_bot {
        if let Some(ref msg_text) = chat_sync_msg {
            let target_cid = req
                .chat_id
                .filter(|&cid| {
                    cid > 0
                        && check_active_membership(&state, ChatId(cid), auth.user_id).is_ok()
                        && crate::routes::hermes::is_bot_member_of_chat(
                            &state,
                            bot.bot_user_id as u64,
                            ChatId(cid),
                        )
                })
                .map(ChatId)
                .unwrap_or(companion_cid);
            if target_cid.0 > 0 {
                delivered_chat_id = Some(target_cid.0);
                let kb = vec![vec![InlineKeyboardButton {
                    text: format!("{} 打开 {}", app.icon_emoji, app.title),
                    callback_data: None,
                    url: None,
                    web_app_url: Some(format!("gpui-miniapp://{}/{}", bot.username, app.short_name)),
                }]];
                let _ = send_bot_chat_reply(
                    &state,
                    target_cid,
                    UserId(bot.bot_user_id as u64),
                    msg_text,
                    kb.clone(),
                );
                if target_cid != companion_cid && companion_cid.0 > 0 {
                    let _ = send_bot_chat_reply(
                        &state,
                        companion_cid,
                        UserId(bot.bot_user_id as u64),
                        msg_text,
                        kb,
                    );
                }
            }
        }
    }

    (
        StatusCode::OK,
        Json(MiniAppActionResponse {
            ok: true,
            ui_schema: synced_schema,
            state_json: next_state,
            companion_bot_message: chat_sync_msg,
            companion_chat_id: delivered_chat_id,
            toast: Some(toast),
            close_app,
            ai_reply: None,
        }),
    )
        .into_response()
}

fn persist_session_state(
    state: &AppState,
    existing: Option<&MiniAppSessionRecord>,
    uid: i64,
    app: &MiniAppRecord,
    companion_cid: i64,
    next_state: &Value,
) {
    let serialized = next_state.to_string();
    if let Some(sess) = existing {
        let _ = state.db.update_mini_app_session_state(&sess.query_id, &serialized);
    } else {
        let qid = format!("AAH{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let now_sec = chrono::Utc::now().timestamp();
        let _ = state.db.create_mini_app_session(
            &qid,
            uid,
            &app.app_id,
            app.bot_user_id,
            companion_cid,
            "CompactSheet",
            "",
            "",
            now_sec,
            &serialized,
        );
    }
}

/// GET /api/v1/miniapps/{bot_username}/{short_name}/storage
pub async fn miniapps_storage_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((bot_username, short_name)): Path<(String, String)>,
) -> Response {
    let Some((app, _bot)) = resolve_app_and_bot(&state, &bot_username, &short_name, None, false)
    else {
        return err_resp(StatusCode::NOT_FOUND, "miniapp_not_found", "Mini App not found");
    };
    let uid = auth.user_id.0 as i64;
    match state.db.list_cloud_storage_items(uid, app.bot_user_id) {
        Ok(items) => {
            let entries: BTreeMap<String, String> = items
                .iter()
                .map(|i| (i.key.clone(), i.value.clone()))
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "items": items, "entries": entries, "total": items.len() })),
            )
                .into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

/// PUT /api/v1/miniapps/{bot_username}/{short_name}/storage
pub async fn miniapps_storage_put(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((bot_username, short_name)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let Some((app, _bot)) = resolve_app_and_bot(&state, &bot_username, &short_name, None, false)
    else {
        return err_resp(StatusCode::NOT_FOUND, "miniapp_not_found", "Mini App not found");
    };
    let uid = auth.user_id.0 as i64;

    if let Ok(single) = serde_json::from_slice::<CloudStorageSetRequest>(&body) {
        let mut map = BTreeMap::new();
        map.insert(single.key, single.value);
        return match state.db.put_cloud_storage_entries(uid, app.bot_user_id, &map) {
            Ok(mut items) => {
                let item = items.pop().unwrap_or_default();
                (StatusCode::OK, Json(item)).into_response()
            }
            Err(e) => map_telegram_err(e),
        };
    }

    let val: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                "Expected { key, value } or { entries: {...} }",
            );
        }
    };
    let Some(obj) = val.get("entries").and_then(|v| v.as_object()) else {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "Missing key/value or entries object",
        );
    };
    let mut map = BTreeMap::new();
    for (k, v) in obj {
        let v_str = match v.as_str() {
            Some(s) => s.to_string(),
            None => v.to_string(),
        };
        map.insert(k.clone(), v_str);
    }
    match state.db.put_cloud_storage_entries(uid, app.bot_user_id, &map) {
        Ok(items) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "items": items })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

/// DELETE /api/v1/miniapps/{bot_username}/{short_name}/storage
pub async fn miniapps_storage_delete(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path((bot_username, short_name)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
    body: Bytes,
) -> Response {
    let Some((app, _bot)) = resolve_app_and_bot(&state, &bot_username, &short_name, None, false)
    else {
        return err_resp(StatusCode::NOT_FOUND, "miniapp_not_found", "Mini App not found");
    };
    let uid = auth.user_id.0 as i64;
    let mut keys: Vec<String> = Vec::new();
    if let Some(k) = params.get("key").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        keys.push(k.to_string());
    }
    if !body.is_empty() {
        if let Ok(val) = serde_json::from_slice::<Value>(&body) {
            if let Some(k) = val.get("key").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                keys.push(k.to_string());
            }
            if let Some(arr) = val.get("keys").and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(s) = item.as_str().filter(|s| !s.is_empty()) {
                        keys.push(s.to_string());
                    }
                }
            }
        }
    }
    if keys.is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "missing_key", "Provide ?key=... or JSON key");
    }
    match state.db.delete_cloud_storage_keys(uid, app.bot_user_id, &keys) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "deleted": true, "ok": true, "keys": keys })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

/// GET /miniapps/{bot_username}/{short_name}
pub async fn serve_miniapp_html(
    State(state): State<Arc<AppState>>,
    Path((bot_username, short_name)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let custom_hint = params.get("url").map(String::as_str);
    let Some((app, bot)) =
        resolve_app_and_bot(&state, &bot_username, &short_name, custom_hint, false)
    else {
        return (StatusCode::NOT_FOUND, Html("<h1>404 Mini App Not Found</h1>".to_string()))
            .into_response();
    };
    let init_data = params.get("init_data").cloned().unwrap_or_default();

    if let Ok(Some(bundle_html)) = state.db.get_mini_app_hosted_bundle(&app.app_id) {
        let init_escaped = serde_json::to_string(&init_data).unwrap_or_else(|_| "\"\"".into());
        let inject = format!(
            r#"<script>window.__TG_INIT_DATA__ = {}; window.__TG_BOT_ID__ = {};</script><script src="/js/telegram-web-app.js"></script>"#,
            init_escaped, bot.bot_user_id
        );
        let final_html = if let Some(idx) = bundle_html.find("</head>") {
            let mut s = String::with_capacity(bundle_html.len() + inject.len() + 1);
            s.push_str(&bundle_html[..idx]);
            s.push_str(&inject);
            s.push_str(&bundle_html[idx..]);
            s
        } else {
            format!("{}\n{}", inject, bundle_html)
        };
        return (StatusCode::OK, Html(final_html)).into_response();
    }

    let trimmed_url = app.web_url.trim();
    let self_suffix = format!("/miniapps/{}/{}", bot.username, app.short_name);
    if (trimmed_url.starts_with("http://") || trimmed_url.starts_with("https://"))
        && !trimmed_url.ends_with(&self_suffix)
    {
        let sep = if trimmed_url.contains('#') { "&" } else { "#" };
        let iframe_src = if init_data.is_empty() {
            trimmed_url.to_string()
        } else {
            format!(
                "{}{}tgWebAppData={}",
                trimmed_url,
                sep,
                crate::db::miniapp_store::percent_encode_component(&init_data)
            )
        };
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width,initial-scale=1,maximum-scale=1,user-scalable=no" />
  <title>{title} — @{bot_un}</title>
  <script>window.__TG_INIT_DATA__ = {init_escaped}; window.__TG_BOT_ID__ = {bot_uid};</script>
  <script src="/js/telegram-web-app.js"></script>
  <style>
    html, body {{ margin: 0; padding: 0; width: 100%; height: 100%; overflow: hidden; background: {bg}; }}
    iframe {{ border: 0; width: 100%; height: 100%; display: block; }}
  </style>
</head>
<body>
  <iframe id="tg-app-frame" src="{iframe_src}" allow="clipboard-read; clipboard-write; microphone; camera"></iframe>
  <script>
    (function() {{
      var frame = document.getElementById('tg-app-frame');
      window.addEventListener('message', function(ev) {{
        var d = ev && ev.data;
        if (!d || typeof d !== 'object') return;
        if (ev.source === frame.contentWindow && d.type === 'telegram_web_app_event') {{
          if (window.TelegramWebviewProxy && window.TelegramWebviewProxy.postEvent) {{
            window.TelegramWebviewProxy.postEvent(d.eventType, JSON.stringify(d.eventData || {{}}));
          }}
        }} else if (ev.source === window.parent && frame.contentWindow) {{
          frame.contentWindow.postMessage(d, '*');
        }}
      }});
    }})();
  </script>
</body>
</html>"#,
            title = html_escape(&app.title),
            bot_un = html_escape(&bot.username),
            init_escaped = serde_json::to_string(&init_data).unwrap_or_else(|_| "\"\"".into()),
            bot_uid = bot.bot_user_id,
            bg = if app.background_color.trim().is_empty() {
                "#ffffff"
            } else {
                app.background_color.trim()
            },
            iframe_src = html_escape(&iframe_src),
        );
        return (StatusCode::OK, Html(html)).into_response();
    }

    let schema: MiniAppUiSchema =
        serde_json::from_str(&app.ui_schema_json).unwrap_or_else(|_| fallback_schema_for_app(&app));
    let initial_state = initial_state_for_app(&app);
    let html = render_miniapp_html(&app, &bot, &init_data, &schema, &initial_state);
    (StatusCode::OK, Html(html)).into_response()
}

/// POST /api/v1/chats/{chat_id}/callback
pub async fn chat_callback(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(chat_id): Path<i64>,
    Json(req): Json<ChatCallbackRequest>,
) -> Response {
    let cid = ChatId(chat_id);
    if let Err(r) = check_active_membership(&state, cid, auth.user_id) {
        return r;
    }
    let data = req.callback_data.trim();
    if data.is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_callback", "callback_data cannot be empty");
    }

    let members = state.db.get_chat_members(cid).unwrap_or_default();
    let is_botfather_chat = data.starts_with("bf:") || members.contains(&UserId(BOTFATHER_USER_ID));
    if is_botfather_chat && data.starts_with("bf:") {
        let res = handle_botfather_callback(&state, cid, auth.user_id, data).await;
        return (StatusCode::OK, Json(res)).into_response();
    }

    if data == "hermes:open_ai_providers" {
        return (
            StatusCode::OK,
            Json(PeerBotActionResult {
                ok: true,
                companion_chat_id: Some(chat_id),
                toast: Some("请打开「设置 → AI 订阅与模型供应商」选择或配置模型供应商".to_string()),
                ..Default::default()
            }),
        )
            .into_response();
    }

    if let Some(rest) = data.strip_prefix("hermes:approve:") {
        let mut parts = rest.splitn(2, ':');
        if let (Some(approval_id), Some(decision)) = (parts.next(), parts.next()) {
            match crate::routes::hermes::apply_hermes_approval_decision(
                &state,
                auth.user_id,
                approval_id,
                decision,
                None,
            ) {
                Ok((status_str, _)) => {
                    let updated_text = format!("🛡️ 审批 `{}` 已更新为：**{}**", approval_id, status_str);
                    if req.message_id > 0 {
                        if let Ok(pts) = state.message_pipeline.edit_message(
                            cid,
                            MsgId(req.message_id),
                            MessageContent::plain_text(&updated_text),
                        ) {
                            let _ = state.db.save_message_extras(
                                cid,
                                MsgId(req.message_id),
                                Some("[]"),
                                None,
                            );
                            broadcast_chat_pts(&state, cid, pts);
                        }
                    }
                    return (
                        StatusCode::OK,
                        Json(PeerBotActionResult {
                            ok: true,
                            companion_chat_id: Some(chat_id),
                            toast: Some(format!("审批已提交: {}", status_str)),
                            updated_text: Some(updated_text),
                            inline_keyboard: Some(Vec::new()),
                            ..Default::default()
                        }),
                    )
                        .into_response();
                }
                Err(resp) => return resp,
            }
        }
    }

    let bot_member = members
        .into_iter()
        .find(|uid| *uid != auth.user_id && state.db.get_user(*uid).ok().flatten().is_some_and(|u| u.is_bot));

    if let Some(bot_uid) = bot_member {
        let reg_bot = state
            .db
            .get_bot_by_user_id(bot_uid.0 as i64)
            .ok()
            .flatten();
        let is_standard = reg_bot.as_ref().is_some_and(|b| b.is_standard_bot);

        let _cb_qid = record_incoming_callback_query_for_bot(
            &state,
            cid,
            auth.user_id,
            bot_uid,
            MsgId(req.message_id),
            data,
        );

        if is_standard {
            return (
                StatusCode::OK,
                Json(PeerBotActionResult {
                    ok: true,
                    companion_chat_id: Some(chat_id),
                    toast: Some(format!("已发送回调至 @{}", reg_bot.as_ref().map(|b| b.username.as_str()).unwrap_or("bot"))),
                    ..Default::default()
                }),
            )
                .into_response();
        }

        if data == "talk:start" || data == "computer:start" {
            let reply = if data == "talk:start" {
                "🎙️ Talk Mode 已就绪！你可以直接发送语音转文字消息，或继续在这里输入你的问题。"
            } else {
                "🖥️ Remote Computer 面板已就绪！请通过工具栏或指令下发需要执行的操作。"
            };
            let mid = send_bot_chat_reply(&state, cid, bot_uid, reply, Vec::new())
                .map(|m| m.0)
                .ok();
            return (
                StatusCode::OK,
                Json(PeerBotActionResult {
                    ok: true,
                    companion_chat_id: Some(chat_id),
                    system_message: Some(reply.to_string()),
                    reply_message_id: mid,
                    toast: Some(reply.to_string()),
                    ..Default::default()
                }),
            )
                .into_response();
        }

        if data.starts_with('/') {
            handle_ai_bot_reply(&state, cid, auth.user_id, bot_uid, data).await;
            return (
                StatusCode::OK,
                Json(PeerBotActionResult {
                    ok: true,
                    companion_chat_id: Some(chat_id),
                    toast: Some(format!("已执行 {}", data)),
                    ..Default::default()
                }),
            )
                .into_response();
        }
    }

    (
        StatusCode::OK,
        Json(PeerBotActionResult {
            ok: true,
            companion_chat_id: Some(chat_id),
            toast: Some(format!("已触发操作: {}", data)),
            ..Default::default()
        }),
    )
        .into_response()
}

// ============================================================================
// 1:1 Telegram Bot Profile, Recent Apps & Permission Gate Endpoints
// ============================================================================

fn resolve_target_bot_for_profile(
    state: &AppState,
    raw_target: &str,
) -> Option<BotRegistryRecord> {
    let decoded = percent_decode_component(raw_target);
    let clean = decoded.trim().trim_start_matches('@');
    if clean.is_empty() {
        return None;
    }
    if let Ok(bot_id) = clean.parse::<i64>() {
        if let Ok(Some(found)) = state.db.get_bot_by_user_id(bot_id) {
            return Some(found);
        }
    }
    state.db.get_bot_by_username(clean).ok().flatten()
}

pub(crate) fn grant_bot_write_access_internal(
    state: &Arc<AppState>,
    user_id: UserId,
    bot: &BotRegistryRecord,
    web_app_name: Option<&str>,
    from_attachment_menu: bool,
    from_request: bool,
) -> gpui_telegram_core::error::Result<AllowSendMessageResponse> {
    let bot_uid = UserId(bot.bot_user_id as u64);
    if state.db.is_user_blocked(user_id, bot_uid).unwrap_or(false) {
        let _ = state.db.unblock_user(user_id, bot_uid);
    }
    state
        .db
        .set_user_bot_write_access(user_id.0 as i64, bot.bot_user_id, true)?;

    let (cid, _) = state
        .db
        .ensure_companion_bot_chat_record(user_id, bot_uid)?;
    state.gateway_hub.subscribe_chat(cid.0, user_id);
    state.gateway_hub.subscribe_chat(cid.0, bot_uid);

    let service_text = "您已允许该机器人在其网页应用中向您发送消息。";
    let saved_id = send_bot_chat_reply(state, cid, bot_uid, service_text, Vec::new())?;

    enqueue_write_access_allowed_update(
        state,
        bot,
        user_id,
        if saved_id.0 > 0 { Some(saved_id) } else { None },
        web_app_name,
        from_attachment_menu,
        from_request,
    );

    Ok(AllowSendMessageResponse {
        ok: true,
        chat_id: cid.0,
        service_message_id: if saved_id.0 > 0 {
            Some(saved_id.0)
        } else {
            None
        },
        service_text: service_text.to_string(),
    })
}

/// GET /api/v1/bots/profile/{target}
pub async fn bots_public_profile(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(target): Path<String>,
) -> Response {
    let Some(bot) = resolve_target_bot_for_profile(&state, &target) else {
        return err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Bot not found");
    };
    let uid = auth.user_id.0 as i64;
    let mut bot_entry = bot_record_to_entry_for_viewer(&state, &bot, uid);
    if !bot.is_official
        && crate::routes::hermes::caller_owns_bot_or_admin(
            &state,
            auth.user_id,
            bot.bot_user_id as u64,
        )
        .is_err()
    {
        bot_entry.ai_persona.clear();
    }
    let installed_map = build_installed_map(&state, uid);
    let raw_apps = state
        .db
        .list_mini_apps_for_bot(bot.bot_user_id)
        .unwrap_or_default();
    let apps: Vec<MiniAppEntry> = raw_apps
        .into_iter()
        .filter(|a| a.is_published)
        .map(|a| miniapp_record_to_entry(&state, &a, uid, Some(&installed_map)))
        .collect();
    let main_app = if bot.has_main_app && !bot.main_app_short_name.trim().is_empty() {
        apps.iter()
            .find(|a| a.short_name == bot.main_app_short_name)
            .cloned()
    } else {
        None
    };

    let existing_chat_id = state
        .db
        .find_companion_chat_id_for_user(bot.bot_user_id, uid)
        .ok()
        .flatten()
        .map(|c| c.0);
    let perm = state.db.get_user_bot_permission(uid, bot.bot_user_id).ok();
    let is_blocked = state
        .db
        .is_user_blocked(auth.user_id, UserId(bot.bot_user_id as u64))
        .unwrap_or(false);
    let is_muted = existing_chat_id
        .and_then(|cid| state.db.get_user_chat_setting(auth.user_id, ChatId(cid)).ok())
        .is_some_and(|(_, _, muted, _)| muted);

    let dto = BotPublicProfileDto {
        bot: bot_entry,
        main_app,
        apps,
        existing_chat_id,
        has_started_chat: perm.as_ref().is_some_and(|p| p.has_started_chat),
        allows_write_to_pm: !is_blocked && perm.as_ref().is_some_and(|p| p.allows_write_to_pm),
        app_tos_accepted: perm.as_ref().is_some_and(|p| p.app_tos_accepted_at > 0),
        is_blocked,
        is_muted,
    };
    (StatusCode::OK, Json(dto)).into_response()
}

/// GET /api/v1/miniapps/recent
pub async fn miniapps_recent(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let uid = auth.user_id.0 as i64;
    let installed_map = build_installed_map(&state, uid);
    match state.db.list_recent_mini_apps(uid, 30) {
        Ok(list) => {
            let apps: Vec<MiniAppEntry> = list
                .into_iter()
                .map(|(a, last_ts)| {
                    let mut entry =
                        miniapp_record_to_entry(&state, &a, uid, Some(&installed_map));
                    entry.is_recent_used = true;
                    entry.last_launched_at = last_ts;
                    entry
                })
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "apps": apps, "total": apps.len() })),
            )
                .into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

/// GET /api/v1/bots/{bot_id}/can-send-message
pub async fn bots_can_send_message(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<i64>,
) -> Response {
    let Some(bot) = state.db.get_bot_by_user_id(bot_id).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Bot not found");
    };
    let uid = auth.user_id.0 as i64;
    let is_blocked = state
        .db
        .is_user_blocked(auth.user_id, UserId(bot.bot_user_id as u64))
        .unwrap_or(false);
    let perm = state
        .db
        .get_user_bot_permission(uid, bot.bot_user_id)
        .ok();
    let allows_write_to_pm = !is_blocked && perm.as_ref().is_some_and(|p| p.allows_write_to_pm);
    let has_started_chat = perm.as_ref().is_some_and(|p| p.has_started_chat);
    let can_send = !is_blocked && (allows_write_to_pm || has_started_chat);

    (
        StatusCode::OK,
        Json(CanSendMessageResponse {
            can_send,
            allows_write_to_pm,
            has_started_chat,
            is_blocked,
        }),
    )
        .into_response()
}

/// POST /api/v1/bots/{bot_id}/allow-send-message
pub async fn bots_allow_send_message(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<i64>,
    body: Bytes,
) -> Response {
    let req: AllowSendMessageRequest = if body.is_empty() {
        AllowSendMessageRequest::default()
    } else {
        serde_json::from_slice(&body).unwrap_or_default()
    };
    let Some(bot) = state.db.get_bot_by_user_id(bot_id).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Bot not found");
    };

    let resolved_name = req.web_app_name.clone().or_else(|| {
        req.app_id.as_deref().and_then(|aid| {
            state
                .db
                .list_mini_apps_for_bot(bot.bot_user_id)
                .ok()
                .and_then(|apps| {
                    apps.into_iter()
                        .find(|a| a.app_id == aid)
                        .map(|a| a.title)
                })
        })
    });

    match grant_bot_write_access_internal(
        &state,
        auth.user_id,
        &bot,
        resolved_name.as_deref(),
        req.from_attachment_menu,
        !req.from_attachment_menu,
    ) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

/// POST /api/v1/bots/{bot_id}/accept-app-tos
pub async fn bots_accept_app_tos(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<i64>,
    body: Bytes,
) -> Response {
    let req: AcceptMiniAppTosRequest = if body.is_empty() {
        AcceptMiniAppTosRequest::default()
    } else {
        serde_json::from_slice(&body).unwrap_or_default()
    };
    let Some(bot) = state.db.get_bot_by_user_id(bot_id).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Bot not found");
    };
    let uid = auth.user_id.0 as i64;
    if let Err(e) = state.db.set_user_bot_tos_accepted(uid, bot.bot_user_id) {
        return map_telegram_err(e);
    }

    if req.allow_write_access == Some(true) {
        let app_name = req.app_id.as_deref().and_then(|aid| {
            state
                .db
                .list_mini_apps_for_bot(bot.bot_user_id)
                .ok()
                .and_then(|apps| apps.into_iter().find(|a| a.app_id == aid).map(|a| a.title))
        });
        if let Err(e) = grant_bot_write_access_internal(
            &state,
            auth.user_id,
            &bot,
            app_name.as_deref(),
            false,
            true,
        ) {
            return map_telegram_err(e);
        }
    }

    (
        StatusCode::OK,
        Json(json!({ "ok": true, "app_tos_accepted": true })),
    )
        .into_response()
}

/// POST /api/v1/bots/{bot_id}/block
pub async fn bots_block(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<i64>,
    Json(req): Json<BlockBotRequest>,
) -> Response {
    let Some(bot) = state.db.get_bot_by_user_id(bot_id).ok().flatten() else {
        return err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Bot not found");
    };
    let uid = auth.user_id.0 as i64;
    let bot_uid = UserId(bot.bot_user_id as u64);
    if req.blocked {
        if let Err(e) = state.db.block_user(auth.user_id, bot_uid) {
            return map_telegram_err(e);
        }
        let _ = state.db.set_user_bot_write_access(uid, bot.bot_user_id, false);
    } else if let Err(e) = state.db.unblock_user(auth.user_id, bot_uid) {
        return map_telegram_err(e);
    }

    (
        StatusCode::OK,
        Json(json!({ "ok": true, "blocked": req.blocked })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use crate::db::ServerDatabase;
    use crate::server::build_router_with_state;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request},
        Router,
    };
    use gpui_telegram_proto::miniapp_contracts::{
        BotRegistryEntry, MiniAppActionRequest, MiniAppActionResponse, MiniAppEntry,
        MiniAppLaunchContext, NaturalLanguageCreateResponse, PeerBotActionResult,
        ValidateInitDataRequest, ValidateInitDataResponse,
    };
    use std::time::Duration;
    use tower::ServiceExt;

    fn test_app() -> (Arc<AppState>, Router) {
        let db = Arc::new(ServerDatabase::open_in_memory().expect("open in-memory db"));
        let state = Arc::new(AppState::with_db(ServerConfig::default(), db));
        let router = build_router_with_state(state.clone());
        (state, router)
    }

    async fn http_json(
        router: &Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (status, bytes) = http_bytes(router, method, uri, token, body).await;
        let val = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() })
        });
        (status, val)
    }

    async fn http_bytes(
        router: &Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Vec<u8>) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let req = if let Some(b) = body {
            builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&b).unwrap()))
                .unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, bytes.to_vec())
    }

    async fn register_user(router: &Router, username: &str) -> (u64, String) {
        let (status, val) = http_json(
            router,
            "POST",
            "/api/v1/auth/register",
            None,
            Some(json!({
                "username": username,
                "password": "TestPassword123!",
                "first_name": username,
                "last_name": "Tester",
                "device_name": "TestDesktop"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "register {username}: {val}");
        (
            val["user_id"].as_u64().expect("user_id"),
            val["token"].as_str().expect("token").to_string(),
        )
    }

    async fn send_chat_text_and_wait_bot_reply(
        router: &Router,
        token: &str,
        chat_id: i64,
        text: &str,
        expected_min_count: usize,
    ) -> Vec<Value> {
        let (st, send_res) = http_json(
            router,
            "POST",
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(token),
            Some(json!({ "text": text })),
        )
        .await;
        assert!(
            st.is_success(),
            "send message '{text}' to chat {chat_id} failed: {st} {send_res}"
        );
        for _ in 0..80 {
            let (lst, list_res) = http_json(
                router,
                "GET",
                &format!("/api/v1/chats/{chat_id}/messages?limit=50"),
                Some(token),
                None,
            )
            .await;
            if lst == StatusCode::OK {
                if let Some(msgs) = list_res["messages"].as_array() {
                    if msgs.len() >= expected_min_count
                        && msgs.iter().any(|m| {
                            m["sender_id"].as_u64().is_some_and(|sid| sid >= 500_000)
                                && m["id"].as_i64().unwrap_or(0) > send_res["message_id"].as_i64().unwrap_or(0)
                        })
                    {
                        return msgs.clone();
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let (_, final_res) = http_json(
            router,
            "GET",
            &format!("/api/v1/chats/{chat_id}/messages?limit=50"),
            Some(token),
            None,
        )
        .await;
        final_res["messages"].as_array().cloned().unwrap_or_default()
    }

    #[tokio::test]
    async fn test_botfather_seeding_human_id_isolation_and_global_search() {
        let (state, router) = test_app();

        // 1. Registered human users stay in 100_001+ range, never colliding with 777_001 BotFather
        let (alice_id, alice_tok) = register_user(&router, "alice_bf").await;
        let (bob_id, _) = register_user(&router, "bob_bf").await;
        assert!(alice_id >= 100_001 && alice_id < 500_000, "alice_id={alice_id}");
        assert_eq!(bob_id, alice_id + 1, "bob_id={bob_id}");

        // 2. Global search finds BotFather via English & Chinese aliases, and NO host demo Mini Apps are pre-seeded
        let (st, raw_search) = http_json(
            &router,
            "GET",
            "/api/v1/search?q=机器人之父&category=apps",
            Some(&alice_tok),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let found_bots: Vec<BotRegistryEntry> =
            serde_json::from_value(raw_search["bots"].clone()).expect("parse bots");
        assert!(
            found_bots.iter().any(|b| b.bot_user_id == BOTFATHER_USER_ID as i64),
            "bots should include BotFather: {:?}",
            found_bots
        );

        let (st_empty, empty_apps_res) = http_json(
            &router,
            "GET",
            "/api/v1/search?q=&category=apps",
            Some(&alice_tok),
            None,
        )
        .await;
        assert_eq!(st_empty, StatusCode::OK);
        let empty_apps: Vec<MiniAppEntry> =
            serde_json::from_value(empty_apps_res["apps"].clone()).expect("parse empty-q apps");
        assert!(
            !empty_apps.iter().any(|a| matches!(a.bot_user_id, 777_010..=777_012)),
            "built-in demo Mini Apps must NOT be seeded: {:?}",
            empty_apps
        );

        // Register a developer/test bot + Mini App and verify search & peer resolution
        let focus_bot = state
            .db
            .create_registered_bot(
                alice_id as i64,
                "TestFocusBot",
                "Test Focus Bot",
                "Focus timer bot",
                "Pomodoro timer",
                "Focus helper",
                "Open Focus",
                "/miniapps/TestFocusBot/focus",
                "[]",
                false,
                true,
            )
            .expect("create TestFocusBot");
        let schema_str = crate::db::miniapp_store::default_pomodoro_ui_schema().to_string();
        let _focus_app = state
            .db
            .create_or_upsert_mini_app(
                focus_bot.bot_user_id,
                &focus_bot.username,
                "focus",
                "Focus Timer",
                "番茄专注计时器",
                "productivity",
                "🍅",
                None,
                &schema_str,
                true,
                true,
                false,
            )
            .expect("create focus app");

        let (st_all, all_apps_res) = http_json(
            &router,
            "GET",
            "/api/v1/search?q=&category=apps",
            Some(&alice_tok),
            None,
        )
        .await;
        assert_eq!(st_all, StatusCode::OK);
        let all_apps: Vec<MiniAppEntry> =
            serde_json::from_value(all_apps_res["apps"].clone()).expect("parse empty-q apps");
        assert!(all_apps.iter().any(|a| a.short_name == "focus"));

        // 3. Peer resolve works for @BotFather and 机器人之父 and bot/short deep link
        let (st_p1, p1) = http_json(
            &router,
            "GET",
            "/api/v1/peers/resolve?username=BotFather",
            Some(&alice_tok),
            None,
        )
        .await;
        assert_eq!(st_p1, StatusCode::OK);
        assert_eq!(p1["user"]["user_id"].as_u64(), Some(BOTFATHER_USER_ID));
        assert_eq!(p1["user"]["is_bot"].as_bool(), Some(true));

        let (st_p2, p2) = http_json(
            &router,
            "GET",
            "/api/v1/peers/resolve?username=机器人之父",
            Some(&alice_tok),
            None,
        )
        .await;
        assert_eq!(st_p2, StatusCode::OK);
        assert_eq!(p2["user"]["user_id"].as_u64(), Some(BOTFATHER_USER_ID));

        let (st_p3, p3) = http_json(
            &router,
            "GET",
            "/api/v1/peers/resolve?username=TestFocusBot/focus",
            Some(&alice_tok),
            None,
        )
        .await;
        assert_eq!(st_p3, StatusCode::OK);
        assert_eq!(p3["mini_app"]["short_name"].as_str(), Some("focus"));

        // 4. Adding @BotFather creates DM chat in 600_000_000..=699_999_999 with welcome keyboard
        let (st_add, add_res) = http_json(
            &router,
            "POST",
            "/api/v1/contacts/add",
            Some(&alice_tok),
            Some(json!({ "username": "@BotFather" })),
        )
        .await;
        assert!(st_add.is_success(), "add BotFather failed: {st_add} {add_res}");
        let bf_cid = add_res["chat_id"].as_i64().expect("BotFather chat_id");
        assert!(
            (600_000_000..700_000_000).contains(&bf_cid),
            "companion chat_id {bf_cid} outside 600M..700M range"
        );
        assert_eq!(add_res["is_bot"].as_bool(), Some(true));

        let (st_m, msgs_res) = http_json(
            &router,
            "GET",
            &format!("/api/v1/chats/{bf_cid}/messages?limit=10"),
            Some(&alice_tok),
            None,
        )
        .await;
        assert_eq!(st_m, StatusCode::OK);
        let msgs = msgs_res["messages"].as_array().expect("messages array");
        assert!(
            !msgs.is_empty(),
            "BotFather DM should have initial welcome message"
        );
        assert!(
            msgs[0]["inline_keyboard"].as_array().is_some_and(|kb| !kb.is_empty()),
            "BotFather welcome should contain inline keyboard buttons: {:?}",
            msgs[0]
        );
    }

    #[tokio::test]
    async fn test_botfather_slash_command_fsm_and_management() {
        let (_state, router) = test_app();
        let (_, token) = register_user(&router, "fsm_owner").await;

        let (_, add_res) = http_json(
            &router,
            "POST",
            "/api/v1/contacts/add",
            Some(&token),
            Some(json!({ "username": "BotFather" })),
        )
        .await;
        let bf_cid = add_res["chat_id"].as_i64().unwrap();

        // /newbot -> display name -> invalid username -> valid username
        let msgs = send_chat_text_and_wait_bot_reply(&router, &token, bf_cid, "/newbot", 3).await;
        assert!(msgs.last().unwrap()["text"].as_str().unwrap().contains("显示名称"));

        let msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            bf_cid,
            "Weather Helper",
            msgs.len() + 2,
        )
        .await;
        assert!(msgs.last().unwrap()["text"].as_str().unwrap().contains("username"));

        let msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            bf_cid,
            "weather_helper_invalid",
            msgs.len() + 2,
        )
        .await;
        assert!(
            msgs.last().unwrap()["text"].as_str().unwrap().contains("bot"),
            "expected validation hint: {:?}",
            msgs.last()
        );

        let msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            bf_cid,
            "weather_helper_bot",
            msgs.len() + 2,
        )
        .await;
        let created_text = msgs.last().unwrap()["text"].as_str().unwrap();
        assert!(
            created_text.contains("@weather_helper_bot") && created_text.contains(":"),
            "expected bot username & token in reply: {created_text}"
        );

        // Verify via GET /api/v1/bots/my
        let (st_my, my_bots) = http_json(&router, "GET", "/api/v1/bots/my", Some(&token), None).await;
        assert_eq!(st_my, StatusCode::OK);
        let bots_arr = my_bots["bots"].as_array().unwrap();
        assert_eq!(bots_arr.len(), 1);
        assert_eq!(bots_arr[0]["username"].as_str(), Some("weather_helper_bot"));

        // /newapp step-by-step FSM
        let msgs = send_chat_text_and_wait_bot_reply(&router, &token, bf_cid, "/newapp", msgs.len() + 2).await;
        let msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            bf_cid,
            "@weather_helper_bot",
            msgs.len() + 2,
        )
        .await;
        let msgs = send_chat_text_and_wait_bot_reply(&router, &token, bf_cid, "天气预报", msgs.len() + 2).await;
        let msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            bf_cid,
            "查看城市实时天气与穿衣建议",
            msgs.len() + 2,
        )
        .await;
        let msgs = send_chat_text_and_wait_bot_reply(&router, &token, bf_cid, "auto", msgs.len() + 2).await;
        let msgs = send_chat_text_and_wait_bot_reply(&router, &token, bf_cid, "weather", msgs.len() + 2).await;
        let app_reply = msgs.last().unwrap();
        assert!(app_reply["text"].as_str().unwrap().contains("t.me/weather_helper_bot/weather"));
        assert!(
            app_reply["inline_keyboard"].to_string().contains("gpui-miniapp://weather_helper_bot/weather"),
            "expected web_app_url button in reply: {app_reply}"
        );

        // /token & /revoke
        let msgs = send_chat_text_and_wait_bot_reply(&router, &token, bf_cid, "/token", msgs.len() + 2).await;
        let msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            bf_cid,
            "@weather_helper_bot",
            msgs.len() + 2,
        )
        .await;
        let first_tok_msg = msgs.last().unwrap()["text"].as_str().unwrap().to_string();
        assert!(first_tok_msg.contains(':'));

        let msgs = send_chat_text_and_wait_bot_reply(&router, &token, bf_cid, "/revoke", msgs.len() + 2).await;
        let msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            bf_cid,
            "@weather_helper_bot",
            msgs.len() + 2,
        )
        .await;
        let revoked_msg = msgs.last().unwrap()["text"].as_str().unwrap().to_string();
        assert_ne!(revoked_msg, first_tok_msg);

        // /setname, /setcommands, /setinline
        let msgs = send_chat_text_and_wait_bot_reply(&router, &token, bf_cid, "/setname", msgs.len() + 2).await;
        let msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            bf_cid,
            "@weather_helper_bot",
            msgs.len() + 2,
        )
        .await;
        let _msgs = send_chat_text_and_wait_bot_reply(&router, &token, bf_cid, "Weather Pro AI", msgs.len() + 2).await;

        let (st_my2, my_bots2) = http_json(&router, "GET", "/api/v1/bots/my", Some(&token), None).await;
        assert_eq!(st_my2, StatusCode::OK);
        assert_eq!(my_bots2["bots"][0]["display_name"].as_str(), Some("Weather Pro AI"));

        // Install & launch companion Mini App + CloudStorage + bot message BEFORE /deletebot
        // to verify full FK cascade cleanup (chats, messages, sessions, cloud storage, users).
        let (st_inst, _) = http_json(
            &router,
            "POST",
            "/api/v1/miniapps/weather_helper_bot/weather/install",
            Some(&token),
            Some(json!({ "allow_write_access": true })),
        )
        .await;
        assert_eq!(st_inst, StatusCode::OK);
        let (st_launch, _) = http_json(
            &router,
            "POST",
            "/api/v1/miniapps/weather_helper_bot/weather/launch",
            Some(&token),
            Some(json!({})),
        )
        .await;
        assert_eq!(st_launch, StatusCode::OK);
        let (st_put, _) = http_json(
            &router,
            "PUT",
            "/api/v1/miniapps/weather_helper_bot/weather/storage",
            Some(&token),
            Some(json!({ "key": "city", "value": "Shanghai" })),
        )
        .await;
        assert_eq!(st_put, StatusCode::OK);

        // /deletebot
        let msgs = send_chat_text_and_wait_bot_reply(&router, &token, bf_cid, "/deletebot", 2).await;
        let del_msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            bf_cid,
            "@weather_helper_bot",
            msgs.len() + 2,
        )
        .await;
        let del_reply = del_msgs.last().unwrap()["text"].as_str().unwrap_or("");
        assert!(
            del_reply.contains("已删除"),
            "/deletebot should succeed cleanly even with active companion DM/messages/sessions: {del_reply}"
        );
        let (_, my_bots_after) = http_json(&router, "GET", "/api/v1/bots/my", Some(&token), None).await;
        assert!(
            my_bots_after["bots"].as_array().unwrap().is_empty(),
            "bot should be deleted: {my_bots_after}"
        );
    }

    #[tokio::test]
    async fn test_nl_creation_and_companion_ai_bot_awareness() {
        let (_state, router) = test_app();
        let (_, token) = register_user(&router, "coffee_creator").await;

        // 1. One-shot natural language creation via BotFather Studio endpoint
        let (st_nl, nl_raw) = http_json(
            &router,
            "POST",
            "/api/v1/botfather/nl-create",
            Some(&token),
            Some(json!({
                "prompt": "帮我创建一个咖啡店点单助手机器人和点单小程序，支持美式和拿铁下单并总结订单",
                "publish": true
            })),
        )
        .await;
        assert_eq!(st_nl, StatusCode::CREATED, "nl-create failed: {nl_raw}");
        let nl: NaturalLanguageCreateResponse =
            serde_json::from_value(nl_raw).expect("parse nl create response");
        let bot = nl.bot;
        let app = nl.app.expect("created app");
        assert!(bot.username.to_lowercase().ends_with("bot"));
        assert!(!app.short_name.is_empty());
        assert!(
            (600_000_000..700_000_000).contains(&nl.companion_chat_id),
            "companion_chat_id={}",
            nl.companion_chat_id
        );
        assert!(
            !app.ui_schema.widgets.is_empty() && app.ui_schema.main_button.is_some(),
            "NL schema should contain interactive widgets and main button"
        );

        // 2. Also test sending natural language directly inside @BotFather chat
        let (_, bf_add) = http_json(
            &router,
            "POST",
            "/api/v1/contacts/add",
            Some(&token),
            Some(json!({ "username": "BotFather" })),
        )
        .await;
        let bf_cid = bf_add["chat_id"].as_i64().unwrap();
        let bf_msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            bf_cid,
            "帮我创建一个团队每日待办打卡机器人和任务看板小程序",
            3,
        )
        .await;
        let nl_chat_reply = bf_msgs.last().unwrap();
        assert!(
            nl_chat_reply["text"].as_str().unwrap().contains("t.me/"),
            "BotFather NL reply should contain published link: {nl_chat_reply}"
        );
        assert!(
            nl_chat_reply["inline_keyboard"].to_string().contains("gpui-miniapp://"),
            "BotFather NL reply should include launch WebApp button: {nl_chat_reply}"
        );

        // 3. Submit an order in the Coffee Mini App and check live state + chat sync
        let (st_act, act_raw) = http_json(
            &router,
            "POST",
            &format!("/api/v1/miniapps/{}/{}/action", bot.username, app.short_name),
            Some(&token),
            Some(serde_json::to_value(MiniAppActionRequest {
                query_id: None,
                chat_id: Some(nl.companion_chat_id),
                action_type: "widget_event".to_string(),
                widget_id: Some("selected_item".to_string()),
                value: Some(json!("冰拿铁 ¥22")),
                data: None,
                prompt: None,
            })
            .unwrap()),
        )
        .await;
        assert_eq!(st_act, StatusCode::OK, "widget_event failed: {act_raw}");

        let (st_submit, submit_raw) = http_json(
            &router,
            "POST",
            &format!("/api/v1/miniapps/{}/{}/action", bot.username, app.short_name),
            Some(&token),
            Some(serde_json::to_value(MiniAppActionRequest {
                query_id: None,
                chat_id: Some(nl.companion_chat_id),
                action_type: "main_button_click".to_string(),
                widget_id: None,
                value: None,
                data: None,
                prompt: None,
            })
            .unwrap()),
        )
        .await;
        assert_eq!(st_submit, StatusCode::OK, "submit failed: {submit_raw}");
        let submit_resp: MiniAppActionResponse = serde_json::from_value(submit_raw).unwrap();
        assert_eq!(
            submit_resp.state_json["orders"].as_array().map(|a| a.len()),
            Some(1),
            "order should be recorded in state: {}",
            submit_resp.state_json
        );

        let (st_sync, sync_raw) = http_json(
            &router,
            "POST",
            &format!("/api/v1/miniapps/{}/{}/action", bot.username, app.short_name),
            Some(&token),
            Some(serde_json::to_value(MiniAppActionRequest {
                query_id: None,
                chat_id: Some(nl.companion_chat_id),
                action_type: "secondary_button_click".to_string(),
                widget_id: None,
                value: None,
                data: None,
                prompt: None,
            })
            .unwrap()),
        )
        .await;
        assert_eq!(st_sync, StatusCode::OK, "secondary button sync failed: {sync_raw}");
        let sync_resp: MiniAppActionResponse = serde_json::from_value(sync_raw).unwrap();
        assert!(sync_resp.companion_bot_message.is_some());

        // 4. Every created bot is an AI Bot aware of live Mini App state & attaches web_app_url button
        let companion_msgs = send_chat_text_and_wait_bot_reply(
            &router,
            &token,
            nl.companion_chat_id,
            "总结一下我刚才在小程序里的咖啡订单",
            3,
        )
        .await;
        let last_bot_msg = companion_msgs.last().unwrap();
        let reply_txt = last_bot_msg["text"].as_str().unwrap_or("");
        assert!(
            reply_txt.contains("冰拿铁") || reply_txt.contains("订单"),
            "companion AI bot reply should reflect live order state: {reply_txt}"
        );
        assert!(
            last_bot_msg["inline_keyboard"]
                .to_string()
                .contains(&format!("gpui-miniapp://{}/{}", bot.username, app.short_name)),
            "companion bot reply should attach web_app_url button: {last_bot_msg}"
        );
    }

    #[tokio::test]
    async fn test_miniapp_launch_hmac_init_data_actions_cloud_storage_and_html_sdk() {
        let (state, router) = test_app();
        let (uid, token) = register_user(&router, "miniapp_tester").await;

        // Provision test Fabushi bot + Focus Mini App (no built-in demo seeding)
        let focus_bot = state
            .db
            .create_registered_bot(
                uid as i64,
                "FocusHelperBot",
                "Focus Helper",
                "Focus companion bot",
                "Focus helper",
                "Focus coach",
                "Open Focus",
                "/miniapps/FocusHelperBot/focus",
                "[]",
                false,
                true,
            )
            .expect("create FocusHelperBot");
        let _ = state
            .db
            .create_or_upsert_mini_app(
                focus_bot.bot_user_id,
                &focus_bot.username,
                "focus",
                "Pomodoro Focus",
                "Focus Timer",
                "productivity",
                "🍅",
                None,
                &crate::db::miniapp_store::default_pomodoro_ui_schema().to_string(),
                true,
                true,
                false,
            )
            .expect("create focus app");

        // 0. Launching BEFORE install does NOT create DM chat or auto-grant write access, and records recent usage
        let (st_pre, pre_raw) = http_json(
            &router,
            "POST",
            "/api/v1/miniapps/FocusHelperBot/focus/launch",
            Some(&token),
            Some(json!({})),
        )
        .await;
        assert_eq!(st_pre, StatusCode::OK, "pre-install launch failed: {pre_raw}");
        let pre_ctx: MiniAppLaunchContext = serde_json::from_value(pre_raw).unwrap();
        assert_eq!(pre_ctx.companion_chat_id, 0, "launch must NOT auto-create chat");
        assert!(!pre_ctx.app.is_installed, "launch must NOT auto-install app");
        assert!(
            pre_ctx.init_data.user_json.contains("\"allows_write_to_pm\":false"),
            "initData user JSON must reflect allows_write_to_pm=false: {}",
            pre_ctx.init_data.user_json
        );
        let (st_rec, rec_val) =
            http_json(&router, "GET", "/api/v1/miniapps/recent", Some(&token), None).await;
        assert_eq!(st_rec, StatusCode::OK);
        assert!(rec_val["apps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["short_name"] == "focus" && a["is_recent_used"] == true));

        // 1. Install @FocusHelperBot/focus -> provisions companion bot DM & grants write access
        let (st_inst, inst_raw) = http_json(
            &router,
            "POST",
            "/api/v1/miniapps/FocusHelperBot/focus/install",
            Some(&token),
            Some(json!({ "allow_write_access": true })),
        )
        .await;
        assert_eq!(st_inst, StatusCode::OK, "install failed: {inst_raw}");
        let inst: PeerBotActionResult = serde_json::from_value(inst_raw).unwrap();
        let companion_cid = inst.companion_chat_id.expect("companion_chat_id");
        assert!((600_000_000..700_000_000).contains(&companion_cid));

        // 2. Installed list & attach menu reflect the installed Mini App & companion bot
        let (st_list, installed_val) =
            http_json(&router, "GET", "/api/v1/miniapps/installed", Some(&token), None).await;
        assert_eq!(st_list, StatusCode::OK);
        assert!(installed_val["apps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["short_name"] == "focus"));

        let (st_att, attach_val) =
            http_json(&router, "GET", "/api/v1/bots/attach-menu", Some(&token), None).await;
        assert_eq!(st_att, StatusCode::OK);
        assert!(!attach_val["bots"].as_array().unwrap().is_empty());

        // 3. Launch Mini App and verify exact Telegram WebApp HMAC-SHA256 initData
        let (st_launch, launch_raw) = http_json(
            &router,
            "POST",
            "/api/v1/miniapps/FocusHelperBot/focus/launch",
            Some(&token),
            Some(json!({ "chat_id": companion_cid, "start_param": "sprint1" })),
        )
        .await;
        assert_eq!(st_launch, StatusCode::OK, "launch failed: {launch_raw}");
        let launch: MiniAppLaunchContext = serde_json::from_value(launch_raw).unwrap();
        assert!(!launch.init_data.raw_query_string.is_empty());
        assert!(!launch.init_data.hash.is_empty());
        assert_eq!(launch.init_data.start_param.as_deref(), Some("sprint1"));
        assert!(launch.init_data.hmac_verified);
        assert!(
            launch.init_data.user_json.contains("\"allows_write_to_pm\":true"),
            "initData user JSON should reflect allows_write_to_pm=true after grant: {}",
            launch.init_data.user_json
        );

        let (st_val_ok, val_ok_raw) = http_json(
            &router,
            "POST",
            "/api/v1/miniapps/FocusHelperBot/focus/validate-init-data",
            Some(&token),
            Some(serde_json::to_value(ValidateInitDataRequest {
                init_data: launch.init_data.raw_query_string.clone(),
            }).unwrap()),
        )
        .await;
        assert_eq!(st_val_ok, StatusCode::OK);
        let val_ok: ValidateInitDataResponse = serde_json::from_value(val_ok_raw).unwrap();
        assert!(val_ok.valid, "valid initData rejected: {:?}", val_ok.reason);
        assert_eq!(val_ok.user_id, Some(uid as i64));

        // Tamper initData -> must fail HMAC verification
        let tampered = launch.init_data.raw_query_string.replacen("sprint1", "hacked1", 1);
        let (st_val_bad, val_bad_raw) = http_json(
            &router,
            "POST",
            "/api/v1/miniapps/FocusHelperBot/focus/validate-init-data",
            Some(&token),
            Some(serde_json::to_value(ValidateInitDataRequest {
                init_data: tampered,
            }).unwrap()),
        )
        .await;
        assert_eq!(st_val_bad, StatusCode::OK);
        let val_bad: ValidateInitDataResponse = serde_json::from_value(val_bad_raw).unwrap();
        assert!(!val_bad.valid, "tampered initData MUST be rejected");

        // 4. Interactive actions (widget_event, main_button_click, secondary_button_click, ai_copilot)
        let (st_act, act_raw) = http_json(
            &router,
            "POST",
            "/api/v1/miniapps/FocusHelperBot/focus/action",
            Some(&token),
            Some(serde_json::to_value(MiniAppActionRequest {
                query_id: Some(launch.init_data.query_id.clone()),
                chat_id: Some(companion_cid),
                action_type: "main_button_click".to_string(),
                widget_id: None,
                value: None,
                data: None,
                prompt: None,
            }).unwrap()),
        )
        .await;
        assert_eq!(st_act, StatusCode::OK, "focus main action failed: {act_raw}");
        let act_res: MiniAppActionResponse = serde_json::from_value(act_raw).unwrap();
        let expected_sessions = launch.state_json["completed_sessions"].as_i64().unwrap_or(0) + 1;
        assert_eq!(act_res.state_json["completed_sessions"].as_i64(), Some(expected_sessions));

        let (st_cop, cop_raw) = http_json(
            &router,
            "POST",
            "/api/v1/miniapps/FocusHelperBot/focus/action",
            Some(&token),
            Some(serde_json::to_value(MiniAppActionRequest {
                query_id: Some(launch.init_data.query_id.clone()),
                chat_id: Some(companion_cid),
                action_type: "ai_copilot".to_string(),
                widget_id: None,
                value: None,
                data: None,
                prompt: Some("安排下一轮番茄休息建议".to_string()),
            }).unwrap()),
        )
        .await;
        assert_eq!(st_cop, StatusCode::OK, "ai_copilot failed: {cop_raw}");
        let cop_res: MiniAppActionResponse = serde_json::from_value(cop_raw).unwrap();
        assert!(cop_res.ai_reply.as_deref().is_some_and(|s| !s.is_empty()));

        // 5. CloudStorage CRUD & Telegram quota enforcement
        let (st_put, put_val) = http_json(
            &router,
            "PUT",
            "/api/v1/miniapps/FocusHelperBot/focus/storage",
            Some(&token),
            Some(json!({ "key": "daily_goal", "value": "8" })),
        )
        .await;
        assert_eq!(st_put, StatusCode::OK, "storage put failed: {put_val}");
        assert_eq!(put_val["key"].as_str(), Some("daily_goal"));

        let (st_get, get_val) = http_json(
            &router,
            "GET",
            "/api/v1/miniapps/FocusHelperBot/focus/storage",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(st_get, StatusCode::OK);
        assert_eq!(get_val["entries"]["daily_goal"].as_str(), Some("8"));

        let oversize_val = "x".repeat(4097);
        let (st_quota, _) = http_json(
            &router,
            "PUT",
            "/api/v1/miniapps/FocusHelperBot/focus/storage",
            Some(&token),
            Some(json!({ "key": "too_big", "value": oversize_val })),
        )
        .await;
        assert_eq!(st_quota, StatusCode::BAD_REQUEST, ">4096B value must fail");

        let (st_del, _) = http_json(
            &router,
            "DELETE",
            "/api/v1/miniapps/FocusHelperBot/focus/storage?key=daily_goal",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(st_del, StatusCode::OK);

        // 6. HTML5 WebApp SDK endpoint + /js/telegram-web-app.js
        let (st_html, html_bytes) =
            http_bytes(&router, "GET", "/miniapps/FocusHelperBot/focus", None, None).await;
        assert_eq!(st_html, StatusCode::OK);
        let html = String::from_utf8_lossy(&html_bytes);
        assert!(html.contains("/js/telegram-web-app.js") && html.contains("window.Telegram") && html.contains("WebApp"));

        let (st_js, js_bytes) =
            http_bytes(&router, "GET", "/js/telegram-web-app.js", None, None).await;
        assert_eq!(st_js, StatusCode::OK);
        let js_src = String::from_utf8_lossy(&js_bytes);
        assert!(js_src.contains("requestWriteAccess") && js_src.contains("MainButton") && js_src.contains("CloudStorage"));

        // 7. Chat inline callback route
        let (st_cb, cb_raw) = http_json(
            &router,
            "POST",
            &format!("/api/v1/chats/{companion_cid}/callback"),
            Some(&token),
            Some(
                serde_json::to_value(gpui_telegram_proto::miniapp_contracts::ChatCallbackRequest {
                    message_id: 1,
                    callback_data: "talk:start".to_string(),
                })
                .unwrap(),
            ),
        )
        .await;
        assert_eq!(st_cb, StatusCode::OK, "callback failed: {cb_raw}");
        assert_eq!(cb_raw["ok"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn test_telegram_bot_api_and_miniapp_permissions_parity() {
        let (_state, router) = test_app();
        let (uid, user_tok) = register_user(&router, "dev_owner").await;

        // 1. Add BotFather and create a standard Telegram bot via /newbot
        let (_, bf_add) = http_json(
            &router,
            "POST",
            "/api/v1/contacts/add",
            Some(&user_tok),
            Some(json!({ "username": "BotFather" })),
        )
        .await;
        let bf_cid = bf_add["chat_id"].as_i64().unwrap();
        let m1 = send_chat_text_and_wait_bot_reply(&router, &user_tok, bf_cid, "/newbot", 3).await;
        let m2 = send_chat_text_and_wait_bot_reply(&router, &user_tok, bf_cid, "Wallet Companion", m1.len() + 2).await;
        let _m3 = send_chat_text_and_wait_bot_reply(&router, &user_tok, bf_cid, "wallet_demo_bot", m2.len() + 2).await;

        let (st_my, my_val) = http_json(&router, "GET", "/api/v1/bots/my", Some(&user_tok), None).await;
        assert_eq!(st_my, StatusCode::OK);
        let bot_entry: BotRegistryEntry = serde_json::from_value(my_val["bots"][0].clone()).unwrap();
        assert!(!bot_entry.is_fabushi_bot, "/newbot must create standard bot, not Fabushi AI bot");
        assert_eq!(bot_entry.menu_button_type, "default");
        assert!(bot_entry.commands.is_empty());

        let bot_token = _state
            .db
            .get_bot_by_user_id(bot_entry.bot_user_id)
            .expect("query created bot")
            .expect("bot must exist")
            .bot_token;
        assert!(
            _m3.last()
                .and_then(|m| m["text"].as_str())
                .is_some_and(|t| t.contains(&bot_token)),
            "BotFather /newbot reply should contain issued token"
        );

        // 2. /bot<token>/getMe + publishMiniApp + setMyCommands + setChatMenuButton + setMyPrivacyPolicy
        let (st_me, me_val) = http_json(
            &router,
            "GET",
            &format!("/bot{bot_token}/getMe"),
            None,
            None,
        )
        .await;
        assert_eq!(st_me, StatusCode::OK, "getMe failed: {me_val}");
        assert_eq!(me_val["ok"].as_bool(), Some(true));
        assert_eq!(me_val["result"]["username"].as_str(), Some("wallet_demo_bot"));

        let (st_pub, pub_val) = http_json(
            &router,
            "POST",
            &format!("/bot{bot_token}/publishMiniApp"),
            None,
            Some(json!({
                "short_name": "wallet",
                "title": "Demo Wallet",
                "description": "Self-custodial wallet",
                "is_main_app": true,
                "header_color": "#2481cc",
                "html_bundle": "<!DOCTYPE html><html><head><title>DemoWallet</title></head><body><h1>Wallet</h1></body></html>"
            })),
        )
        .await;
        assert_eq!(st_pub, StatusCode::OK, "publishMiniApp failed: {pub_val}");
        let app_id = pub_val["result"]["app_id"].as_str().unwrap().to_string();

        let (st_bundle, bundle_bytes) =
            http_bytes(&router, "GET", "/miniapps/wallet_demo_bot/wallet", None, None).await;
        assert_eq!(st_bundle, StatusCode::OK);
        let bundle_str = String::from_utf8_lossy(&bundle_bytes);
        assert!(bundle_str.contains("DemoWallet") && bundle_str.contains("/js/telegram-web-app.js"));

        // 3. Public profile does NOT create chat, returns main_app and permissions
        let (st_prof, prof_raw) = http_json(
            &router,
            "GET",
            "/api/v1/bots/profile/wallet_demo_bot",
            Some(&user_tok),
            None,
        )
        .await;
        assert_eq!(st_prof, StatusCode::OK, "profile failed: {prof_raw}");
        let prof: BotPublicProfileDto = serde_json::from_value(prof_raw).unwrap();
        assert!(prof.existing_chat_id.is_none());
        assert_eq!(prof.main_app.as_ref().map(|a| a.short_name.as_str()), Some("wallet"));
        assert!(!prof.allows_write_to_pm);
        assert!(!prof.has_started_chat);

        // 4. Bot cannot initiate DM until write access granted or /start sent
        let (st_can0, can0_val) = http_json(
            &router,
            "GET",
            &format!("/api/v1/bots/{}/can-send-message", bot_entry.bot_user_id),
            Some(&user_tok),
            None,
        )
        .await;
        assert_eq!(st_can0, StatusCode::OK);
        assert_eq!(can0_val["can_send"].as_bool(), Some(false));

        let (st_403, err_403) = http_json(
            &router,
            "POST",
            &format!("/bot{bot_token}/sendMessage"),
            None,
            Some(json!({ "chat_id": uid, "text": "Unsolicited DM" })),
        )
        .await;
        assert_eq!(st_403, StatusCode::FORBIDDEN);
        assert_eq!(err_403["ok"].as_bool(), Some(false));
        assert_eq!(err_403["error_code"].as_i64(), Some(403));

        // 5. Accept App ToS + Grant write access -> creates DM, inserts service msg, enables sendMessage
        let (st_tos, _) = http_json(
            &router,
            "POST",
            &format!("/api/v1/bots/{}/accept-app-tos", bot_entry.bot_user_id),
            Some(&user_tok),
            Some(json!({ "app_id": app_id, "allow_write_access": true })),
        )
        .await;
        assert_eq!(st_tos, StatusCode::OK);

        let (st_send_ok, send_ok_val) = http_json(
            &router,
            "POST",
            &format!("/bot{bot_token}/sendMessage"),
            None,
            Some(json!({
                "chat_id": uid,
                "text": "Welcome to Demo Wallet",
                "reply_markup": {
                    "inline_keyboard": [[
                        { "text": "Open", "web_app": { "url": "https://example.com/wallet" } },
                        { "text": "Ping", "callback_data": "ping:ok" }
                    ]]
                }
            })),
        )
        .await;
        assert_eq!(st_send_ok, StatusCode::OK, "sendMessage failed: {send_ok_val}");
        assert_eq!(send_ok_val["ok"].as_bool(), Some(true));
        let bot_msg_id = send_ok_val["result"]["message_id"].as_i64().unwrap();

        // 6. Blocking suppresses sendMessage -> unblocking restores; user /start does NOT trigger fake AI reply and enqueues update
        let (st_blk, _) = http_json(
            &router,
            "POST",
            &format!("/api/v1/bots/{}/block", bot_entry.bot_user_id),
            Some(&user_tok),
            Some(json!({ "blocked": true })),
        )
        .await;
        assert_eq!(st_blk, StatusCode::OK);
        let (st_blk_send, _) = http_json(
            &router,
            "POST",
            &format!("/bot{bot_token}/sendMessage"),
            None,
            Some(json!({ "chat_id": uid, "text": "Blocked attempt" })),
        )
        .await;
        assert_eq!(st_blk_send, StatusCode::FORBIDDEN);

        let _ = http_json(
            &router,
            "POST",
            &format!("/api/v1/bots/{}/block", bot_entry.bot_user_id),
            Some(&user_tok),
            Some(json!({ "blocked": false })),
        )
        .await;

        let (st_prof2, prof2_val) = http_json(
            &router,
            "GET",
            "/api/v1/bots/profile/wallet_demo_bot",
            Some(&user_tok),
            None,
        )
        .await;
        assert_eq!(st_prof2, StatusCode::OK);
        let dm_cid = prof2_val["existing_chat_id"].as_i64().unwrap();

        let _ = http_json(
            &router,
            "POST",
            &format!("/api/v1/chats/{dm_cid}/callback"),
            Some(&user_tok),
            Some(json!({ "message_id": bot_msg_id, "callback_data": "ping:ok" })),
        )
        .await;
        let _ = http_json(
            &router,
            "POST",
            &format!("/api/v1/chats/{dm_cid}/messages"),
            Some(&user_tok),
            Some(json!({ "text": "/start" })),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(80)).await;

        let (st_hist, hist_val) = http_json(
            &router,
            "GET",
            &format!("/api/v1/chats/{dm_cid}/messages?limit=20"),
            Some(&user_tok),
            None,
        )
        .await;
        assert_eq!(st_hist, StatusCode::OK);
        let hist = hist_val["messages"].as_array().unwrap();
        assert!(
            hist.iter().any(|m| m["text"].as_str() == Some("您已允许该机器人在其网页应用中向您发送消息。")),
            "expected service message in DM: {hist:?}"
        );

        let (st_upd, upd_val) = http_json(
            &router,
            "GET",
            &format!("/bot{bot_token}/getUpdates?offset=0&limit=50"),
            None,
            None,
        )
        .await;
        assert_eq!(st_upd, StatusCode::OK, "getUpdates failed: {upd_val}");
        let updates = upd_val["result"].as_array().unwrap();
        assert!(
            updates.iter().any(|u| u["message"]["write_access_allowed"]["from_request"].as_bool() == Some(true)),
            "expected write_access_allowed update: {updates:?}"
        );
        assert!(
            updates.iter().any(|u| u["callback_query"]["data"].as_str() == Some("ping:ok")),
            "expected callback_query update: {updates:?}"
        );
        assert!(
            updates.iter().any(|u| u["message"]["text"].as_str() == Some("/start")),
            "expected /start message update: {updates:?}"
        );
    }
}
