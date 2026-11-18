use crate::db::{ChatMemberRecord, ChatRecord, ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MsgId, UserId};
use gpui_telegram_proto::miniapp_contracts::{
    default_fabushi_avatar_color, default_fabushi_avatar_shape, derive_safe_agent_id,
    normalize_fabushi_avatar_color, normalize_fabushi_avatar_shape, validate_bot_username,
    validate_miniapp_short_name, CloudStorageItem, MiniAppActionButton, MiniAppThemeParams,
    MiniAppUiSchema, MiniAppUiWidget, MiniAppWidgetType, CLOUD_STORAGE_MAX_KEYS,
    CLOUD_STORAGE_MAX_KEY_BYTES, CLOUD_STORAGE_MAX_TOTAL_BYTES, CLOUD_STORAGE_MAX_VALUE_BYTES,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use subtle::ConstantTimeEq;

pub const BOTFATHER_USER_ID: u64 = 777_001;
pub const POMODORO_BOT_USER_ID: u64 = 777_010;
pub const SPLITBILL_BOT_USER_ID: u64 = 777_011;
pub const POLLMASTER_BOT_USER_ID: u64 = 777_012;

pub const USER_BOT_ID_START: i64 = 600_001;
pub const USER_BOT_ID_MAX: i64 = 699_999;
pub const COMPANION_CHAT_ID_MIN: i64 = 600_000_000;
pub const COMPANION_CHAT_ID_MAX: i64 = 699_999_999;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotRegistryRecord {
    pub bot_user_id: i64,
    pub owner_user_id: i64,
    pub username: String,
    pub display_name: String,
    pub bot_token: String,
    pub description: String,
    pub about: String,
    pub ai_persona: String,
    pub menu_button_text: String,
    pub menu_button_url: String,
    pub is_official: bool,
    pub is_inline: bool,
    pub allow_attach_menu: bool,
    pub attach_peer_types_json: String,
    pub commands_json: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub agent_id: String,
    pub avatar_shape: String,
    pub avatar_color: String,
    pub preferred_model: String,
    pub thinking_level: String,
    pub approval_mode: String,
    pub toolset_preset: String,
    pub menu_button_type: String,
    pub has_main_app: bool,
    pub main_app_short_name: String,
    pub privacy_policy_url: String,
    pub has_settings: bool,
    pub can_join_groups: bool,
    pub webhook_url: String,
    pub webhook_secret_token: String,
    pub is_standard_bot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotfatherSessionRecord {
    pub user_id: i64,
    pub chat_id: i64,
    pub state: String,
    pub draft_json: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiniAppRecord {
    pub app_id: String,
    pub bot_user_id: i64,
    pub bot_username: String,
    pub short_name: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub icon_emoji: String,
    pub web_url: String,
    pub ui_schema_json: String,
    pub peer_types_json: String,
    pub is_published: bool,
    pub is_attach_enabled: bool,
    pub install_count: i64,
    pub created_by_nl: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub header_color: String,
    pub background_color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserBotPermissionRecord {
    pub user_id: i64,
    pub bot_user_id: i64,
    pub allows_write_to_pm: bool,
    pub app_tos_accepted_at: i64,
    pub has_started_chat: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotApiUpdateRecord {
    pub update_id: i64,
    pub bot_user_id: i64,
    pub payload_json: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiniAppUsageEventRecord {
    pub user_id: i64,
    pub app_id: String,
    pub bot_user_id: i64,
    pub last_launched_at: i64,
    pub launch_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMiniAppRecord {
    pub user_id: i64,
    pub app_id: String,
    pub bot_user_id: i64,
    pub companion_chat_id: i64,
    pub allow_write_access: bool,
    pub installed_at: i64,
    pub last_launched_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiniAppSessionRecord {
    pub query_id: String,
    pub user_id: i64,
    pub app_id: String,
    pub bot_user_id: i64,
    pub chat_id: i64,
    pub launch_mode: String,
    pub start_param: String,
    pub init_data_raw: String,
    pub auth_date: i64,
    pub state_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Computes RFC 2104 HMAC-SHA256.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k_block = [0u8; 64];
    if key.len() > 64 {
        let digest = Sha256::digest(key);
        k_block[..32].copy_from_slice(&digest);
    } else {
        k_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k_block[i];
        opad[i] ^= k_block[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    let outer_digest = outer.finalize();

    let mut out = [0u8; 32];
    out.copy_from_slice(&outer_digest);
    out
}

pub fn bytes_to_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn percent_encode_component(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        let is_unreserved =
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if is_unreserved {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
    out
}

pub fn percent_decode_component(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                decoded.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                    decoded.push((hi << 4) | lo);
                    i += 3;
                    continue;
                }
                decoded.push(bytes[i]);
                i += 1;
            }
            other => {
                decoded.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Signs Telegram WebApp initData per official specification:
/// secret_key = HMAC_SHA256(key="WebAppData", msg=bot_token)
/// hash = hex(HMAC_SHA256(key=secret_key, msg=data_check_string))
/// Returns `(raw_query_string, data_check_string, hash)`.
pub fn sign_webapp_init_data(bot_token: &str, params: &[(String, String)]) -> (String, String, String) {
    let mut sorted = BTreeMap::new();
    for (k, v) in params {
        if k != "hash" {
            sorted.insert(k.clone(), v.clone());
        }
    }
    let data_check_string = sorted
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");

    let secret_key = hmac_sha256(b"WebAppData", bot_token.as_bytes());
    let signature = hmac_sha256(&secret_key, data_check_string.as_bytes());
    let hash = bytes_to_lower_hex(&signature);

    let mut query_parts = sorted
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                percent_encode_component(k),
                percent_encode_component(v)
            )
        })
        .collect::<Vec<_>>();
    query_parts.push(format!("hash={}", hash));
    let raw_query_string = query_parts.join("&");

    (raw_query_string, data_check_string, hash)
}

/// Verifies Telegram WebApp `initData` using constant-time HMAC-SHA256 verification.
pub fn verify_webapp_init_data(
    bot_token: &str,
    init_data_raw: &str,
) -> Result<BTreeMap<String, String>> {
    let mut params = BTreeMap::new();
    let mut provided_hash: Option<String> = None;

    for pair in init_data_raw.trim().trim_start_matches('?').split('&') {
        if pair.is_empty() {
            continue;
        }
        let (raw_k, raw_v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        let key = percent_decode_component(raw_k);
        let val = percent_decode_component(raw_v);
        if key == "hash" {
            provided_hash = Some(val);
        } else {
            params.insert(key, val);
        }
    }

    let provided_hash = provided_hash
        .ok_or_else(|| TelegramError::Auth("initData is missing hash parameter".into()))?;
    let provided_lower = provided_hash.to_ascii_lowercase();
    if provided_lower.len() != 64 {
        return Err(TelegramError::Auth(
            "initData hash length is invalid".into(),
        ));
    }

    let data_check_string = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");
    let secret_key = hmac_sha256(b"WebAppData", bot_token.as_bytes());
    let expected = bytes_to_lower_hex(&hmac_sha256(&secret_key, data_check_string.as_bytes()));

    if provided_lower.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8() != 1 {
        return Err(TelegramError::Auth(
            "initData HMAC-SHA256 verification failed".into(),
        ));
    }
    params.insert("hash".to_string(), provided_lower);
    Ok(params)
}

pub fn generate_bot_token(bot_user_id: i64) -> String {
    let raw = format!(
        "{}:{}:{}",
        bot_user_id,
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4()
    );
    let digest = Sha256::digest(raw.as_bytes());
    let hex_part = bytes_to_lower_hex(&digest[..18]);
    format!("{bot_user_id}:AA{}", &hex_part[..32])
}

pub fn normalize_bot_slug(input: &str) -> String {
    input.trim().trim_start_matches('@').to_string()
}

pub fn default_pomodoro_ui_schema() -> Value {
    let schema = MiniAppUiSchema {
        title: "番茄专注 AI".to_string(),
        subtitle: "深度心流计时 + AI 每日专注复盘".to_string(),
        theme: MiniAppThemeParams {
            button_color: "#e11d48".to_string(),
            ..Default::default()
        },
        widgets: vec![
            MiniAppUiWidget {
                id: "task".to_string(),
                kind: MiniAppWidgetType::TextInput,
                label: "当前专注目标".to_string(),
                value: "完成 GPUI Telegram 核心开发".to_string(),
                placeholder: Some("输入你要专注完成的任务...".to_string()),
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "preset".to_string(),
                kind: MiniAppWidgetType::Select,
                label: "计时模式".to_string(),
                value: "25 分钟专注".to_string(),
                options: vec![
                    "25 分钟专注".to_string(),
                    "15 分钟冲刺".to_string(),
                    "5 分钟休息".to_string(),
                ],
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "completed_sessions".to_string(),
                kind: MiniAppWidgetType::Counter,
                label: "今日完成番茄".to_string(),
                value: "3 个".to_string(),
                count: 3,
                min: Some(0),
                max: Some(48),
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "focus_minutes".to_string(),
                kind: MiniAppWidgetType::MetricCard,
                label: "累计专注时长".to_string(),
                value: "75 分钟".to_string(),
                subtitle: Some("状态: ready".to_string()),
                count: 75,
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "ai_summary".to_string(),
                kind: MiniAppWidgetType::Text,
                label: "AI 专注教练".to_string(),
                value: "当前节奏很稳，建议开启 25 分钟免打扰深度冲刺。".to_string(),
                variant: Some("callout".to_string()),
                ..Default::default()
            },
        ],
        main_button: Some(MiniAppActionButton {
            text: "完成本轮专注 (+25m)".to_string(),
            color: Some("#e11d48".to_string()),
            text_color: Some("#ffffff".to_string()),
            is_visible: true,
            is_active: true,
            action_id: "complete_focus".to_string(),
            sends_data: false,
            ..Default::default()
        }),
        secondary_button: Some(MiniAppActionButton {
            text: "AI 复盘并发送到对话".to_string(),
            is_visible: true,
            is_active: true,
            action_id: "send_focus_summary".to_string(),
            sends_data: true,
            ..Default::default()
        }),
        back_button_visible: false,
        settings_button_visible: false,
        copilot_suggestions: vec![
            "帮我总结今日番茄专注效率".to_string(),
            "拆解下一个 25 分钟核心任务".to_string(),
        ],
    };
    serde_json::to_value(schema).unwrap_or_else(|_| json!({}))
}

pub fn default_splitbill_ui_schema() -> Value {
    let schema = MiniAppUiSchema {
        title: "AA 记账分账".to_string(),
        subtitle: "聚餐/团建/旅行费用一键 AA 结算".to_string(),
        theme: MiniAppThemeParams {
            button_color: "#0284c7".to_string(),
            ..Default::default()
        },
        widgets: vec![
            MiniAppUiWidget {
                id: "item_name".to_string(),
                kind: MiniAppWidgetType::TextInput,
                label: "消费项目".to_string(),
                value: "团队咖啡与午餐".to_string(),
                placeholder: Some("例如：周末火锅、咖啡拼单...".to_string()),
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "amount".to_string(),
                kind: MiniAppWidgetType::TextInput,
                label: "总金额 (CNY)".to_string(),
                value: "180".to_string(),
                placeholder: Some("180".to_string()),
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "split_count".to_string(),
                kind: MiniAppWidgetType::Counter,
                label: "分摊人数".to_string(),
                value: "3 人".to_string(),
                count: 3,
                min: Some(1),
                max: Some(50),
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "per_person".to_string(),
                kind: MiniAppWidgetType::MetricCard,
                label: "每人应付 (AA)".to_string(),
                value: "¥60.00".to_string(),
                subtitle: Some("累计 1 笔消费".to_string()),
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "ai_summary".to_string(),
                kind: MiniAppWidgetType::Text,
                label: "分账实时账本".to_string(),
                value: "共计 1 笔消费 ¥180.00，每人应付 ¥60.00。".to_string(),
                variant: Some("callout".to_string()),
                ..Default::default()
            },
        ],
        main_button: Some(MiniAppActionButton {
            text: "记入一笔并计算 AA".to_string(),
            color: Some("#0284c7".to_string()),
            text_color: Some("#ffffff".to_string()),
            is_visible: true,
            is_active: true,
            action_id: "add_expense".to_string(),
            sends_data: false,
            ..Default::default()
        }),
        secondary_button: Some(MiniAppActionButton {
            text: "生成账单发送到聊天".to_string(),
            is_visible: true,
            is_active: true,
            action_id: "send_bill_summary".to_string(),
            sends_data: true,
            ..Default::default()
        }),
        back_button_visible: false,
        settings_button_visible: false,
        copilot_suggestions: vec![
            "汇总所有 AA 账单并发到群里".to_string(),
            "按 4 人重新均摊计算".to_string(),
        ],
    };
    serde_json::to_value(schema).unwrap_or_else(|_| json!({}))
}

pub fn default_pollmaster_ui_schema() -> Value {
    let schema = MiniAppUiSchema {
        title: "团队投票助手".to_string(),
        subtitle: "轻量团队决策与 AI 意见收敛".to_string(),
        theme: MiniAppThemeParams {
            button_color: "#7c3aed".to_string(),
            ..Default::default()
        },
        widgets: vec![
            MiniAppUiWidget {
                id: "topic".to_string(),
                kind: MiniAppWidgetType::TextInput,
                label: "投票议题".to_string(),
                value: "本周版本发布窗口选择".to_string(),
                placeholder: Some("输入决策议题...".to_string()),
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "selected_option".to_string(),
                kind: MiniAppWidgetType::Select,
                label: "投票选项".to_string(),
                value: "方案 A: 周五晚发布".to_string(),
                options: vec![
                    "方案 A: 周五晚发布".to_string(),
                    "方案 B: 周六上午灰度".to_string(),
                    "方案 C: 下周一全量".to_string(),
                ],
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "total_votes".to_string(),
                kind: MiniAppWidgetType::MetricCard,
                label: "累计投票".to_string(),
                value: "7 票".to_string(),
                subtitle: Some("方案 A 领先".to_string()),
                count: 7,
                ..Default::default()
            },
            MiniAppUiWidget {
                id: "ai_summary".to_string(),
                kind: MiniAppWidgetType::Text,
                label: "实时票数与 AI 结论".to_string(),
                value: "当前「方案 A: 周五晚发布」领先（4/7 票，占比 57%）。".to_string(),
                variant: Some("callout".to_string()),
                ..Default::default()
            },
        ],
        main_button: Some(MiniAppActionButton {
            text: "提交这一票".to_string(),
            color: Some("#7c3aed".to_string()),
            text_color: Some("#ffffff".to_string()),
            is_visible: true,
            is_active: true,
            action_id: "cast_vote".to_string(),
            sends_data: false,
            ..Default::default()
        }),
        secondary_button: Some(MiniAppActionButton {
            text: "发送投票结论到聊天".to_string(),
            is_visible: true,
            is_active: true,
            action_id: "send_poll_result".to_string(),
            sends_data: true,
            ..Default::default()
        }),
        back_button_visible: false,
        settings_button_visible: false,
        copilot_suggestions: vec![
            "统计各方案得票并给出执行建议".to_string(),
            "将投票结论同步至伴生机器人".to_string(),
        ],
    };
    serde_json::to_value(schema).unwrap_or_else(|_| json!({}))
}

fn row_to_bot(row: &rusqlite::Row<'_>) -> rusqlite::Result<BotRegistryRecord> {
    Ok(BotRegistryRecord {
        bot_user_id: row.get(0)?,
        owner_user_id: row.get(1)?,
        username: row.get(2)?,
        display_name: row.get(3)?,
        bot_token: row.get(4)?,
        description: row.get(5)?,
        about: row.get(6)?,
        ai_persona: row.get(7)?,
        menu_button_text: row.get(8)?,
        menu_button_url: row.get(9)?,
        is_official: row.get::<_, i64>(10)? != 0,
        is_inline: row.get::<_, i64>(11)? != 0,
        allow_attach_menu: row.get::<_, i64>(12)? != 0,
        attach_peer_types_json: row.get(13)?,
        commands_json: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
        agent_id: row.get::<_, String>(17).unwrap_or_default(),
        avatar_shape: row.get::<_, String>(18).unwrap_or_default(),
        avatar_color: row.get::<_, String>(19).unwrap_or_default(),
        preferred_model: row.get::<_, String>(20).unwrap_or_default(),
        thinking_level: row
            .get::<_, String>(21)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "medium".to_string()),
        approval_mode: row
            .get::<_, String>(22)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "prompt".to_string()),
        toolset_preset: row
            .get::<_, String>(23)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "full".to_string()),
        menu_button_type: row
            .get::<_, String>(24)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "default".to_string()),
        has_main_app: row.get::<_, i64>(25).unwrap_or(0) != 0,
        main_app_short_name: row.get::<_, String>(26).unwrap_or_default(),
        privacy_policy_url: row.get::<_, String>(27).unwrap_or_default(),
        has_settings: row.get::<_, i64>(28).unwrap_or(0) != 0,
        can_join_groups: row.get::<_, i64>(29).unwrap_or(1) != 0,
        webhook_url: row.get::<_, String>(30).unwrap_or_default(),
        webhook_secret_token: row.get::<_, String>(31).unwrap_or_default(),
        is_standard_bot: row.get::<_, i64>(32).unwrap_or(0) != 0,
    })
}

fn row_to_mini_app(row: &rusqlite::Row<'_>) -> rusqlite::Result<MiniAppRecord> {
    Ok(MiniAppRecord {
        app_id: row.get(0)?,
        bot_user_id: row.get(1)?,
        bot_username: row.get(2)?,
        short_name: row.get(3)?,
        title: row.get(4)?,
        description: row.get(5)?,
        category: row.get(6)?,
        icon_emoji: row.get(7)?,
        web_url: row.get(8)?,
        ui_schema_json: row.get(9)?,
        peer_types_json: row.get(10)?,
        is_published: row.get::<_, i64>(11)? != 0,
        is_attach_enabled: row.get::<_, i64>(12)? != 0,
        install_count: row.get(13)?,
        created_by_nl: row.get::<_, i64>(14)? != 0,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
        header_color: row.get::<_, String>(17).unwrap_or_default(),
        background_color: row.get::<_, String>(18).unwrap_or_default(),
    })
}

fn row_to_user_mini_app(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserMiniAppRecord> {
    Ok(UserMiniAppRecord {
        user_id: row.get(0)?,
        app_id: row.get(1)?,
        bot_user_id: row.get(2)?,
        companion_chat_id: row.get(3)?,
        allow_write_access: row.get::<_, i64>(4)? != 0,
        installed_at: row.get(5)?,
        last_launched_at: row.get(6)?,
    })
}

fn row_to_mini_app_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<MiniAppSessionRecord> {
    Ok(MiniAppSessionRecord {
        query_id: row.get(0)?,
        user_id: row.get(1)?,
        app_id: row.get(2)?,
        bot_user_id: row.get(3)?,
        chat_id: row.get(4)?,
        launch_mode: row.get(5)?,
        start_param: row.get(6)?,
        init_data_raw: row.get(7)?,
        auth_date: row.get(8)?,
        state_json: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

const BOT_COLS: &str = "bot_user_id, owner_user_id, username, display_name, bot_token, description, about, ai_persona, menu_button_text, menu_button_url, is_official, is_inline, allow_attach_menu, attach_peer_types_json, commands_json, created_at, updated_at, agent_id, avatar_shape, avatar_color, preferred_model, thinking_level, approval_mode, toolset_preset, menu_button_type, has_main_app, main_app_short_name, privacy_policy_url, has_settings, can_join_groups, webhook_url, webhook_secret_token, is_standard_bot";
const MINIAPP_COLS: &str = "app_id, bot_user_id, bot_username, short_name, title, description, category, icon_emoji, web_url, ui_schema_json, peer_types_json, is_published, is_attach_enabled, install_count, created_by_nl, created_at, updated_at, header_color, background_color";

impl ServerDatabase {
    /// Seeds ONLY official @BotFather (777_001) idempotently and unpublishes any legacy built-in demo Mini Apps.
    pub fn seed_botfather_and_flagship_apps(&self) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            // 1. Seed @BotFather (777_001)
            let botfather_commands = json!([
                { "command": "start", "description": "打开 BotFather 主菜单与 AI 创建向导" },
                { "command": "newbot", "description": "创建一个新机器人（分步引导）" },
                { "command": "newapp", "description": "为已有机器人创建新的 Mini App" },
                { "command": "mybots", "description": "查看并管理你创建的所有机器人" },
                { "command": "myapps", "description": "查看并管理你的 Mini Apps" },
                { "command": "aibuild", "description": "一句话自然语言同时创建 AI 机器人与小程序" },
                { "command": "token", "description": "查看机器人 API Token" },
                { "command": "revoke", "description": "重置机器人 API Token" },
                { "command": "setname", "description": "修改机器人显示名称" },
                { "command": "setdescription", "description": "修改机器人简介" },
                { "command": "setabouttext", "description": "修改机器人 About 资料" },
                { "command": "setcommands", "description": "设置机器人斜杠命令列表" },
                { "command": "setmenubutton", "description": "配置机器人聊天输入框菜单按钮" },
                { "command": "setprivacypolicy", "description": "设置机器人隐私政策链接" },
                { "command": "setmainapp", "description": "设置机器人主 Mini App" },
                { "command": "setinline", "description": "切换机器人 Inline 模式" },
                { "command": "deletebot", "description": "删除我创建的机器人" },
                { "command": "deleteapp", "description": "删除我创建的小程序" },
                { "command": "cancel", "description": "取消当前进行中的操作" },
                { "command": "help", "description": "查看所有命令与使用说明" }
            ])
            .to_string();

            tx.execute(
                r#"INSERT INTO users (
                    user_id, username, phone, first_name, last_name, bio,
                    password_hash, is_bot, is_verified, is_support, is_scam, is_fake,
                    profile_photo_file_id, account_generation, created_at_ms, updated_at_ms
                ) VALUES (?1, 'BotFather', NULL, 'BotFather', NULL, ?2, NULL, 1, 1, 1, 0, 0, NULL, 1, ?3, ?3)
                ON CONFLICT(user_id) DO UPDATE SET
                    username = 'BotFather',
                    first_name = 'BotFather',
                    is_bot = 1,
                    is_verified = 1"#,
                params![
                    BOTFATHER_USER_ID as i64,
                    "BotFather is the one bot to rule them all. Create Telegram bots, Mini Apps, or build both from a single sentence.",
                    now_ms
                ],
            )
            .map_db()?;

            let bf_token = format!("{}:AAOfficialBotFatherMasterRegistryV1", BOTFATHER_USER_ID);
            tx.execute(
                r#"INSERT INTO bots_registry (
                    bot_user_id, owner_user_id, username, display_name, bot_token,
                    description, about, ai_persona, menu_button_text, menu_button_url,
                    is_official, is_inline, allow_attach_menu, attach_peer_types_json,
                    commands_json, created_at, updated_at, menu_button_type, is_standard_bot
                ) VALUES (?1, 0, 'BotFather', 'BotFather', ?2, ?3, ?4, ?5, '', '', 1, 0, 0, '["users","bots"]', ?6, ?7, ?7, 'default', 0)
                ON CONFLICT(bot_user_id) DO UPDATE SET
                    commands_json = excluded.commands_json,
                    updated_at = excluded.updated_at"#,
                params![
                    BOTFATHER_USER_ID as i64,
                    bf_token,
                    "官方机器人与小程序创建中心 (机器人之父)。发送 /newbot、/newapp 或直接用自然语言一句话同时生成 AI Bot + 小程序。",
                    "Official BotFather — Create bots, Mini Apps, tokens & AI experiences.",
                    "You are BotFather, the official bot and Mini App creator.",
                    botfather_commands,
                    now_ms
                ],
            )
            .map_db()?;

            // 2. Ensure legacy built-in host demo Mini Apps (777010, 777011, 777012) are unpublished on existing local DBs.
            tx.execute(
                "UPDATE mini_apps SET is_published = 0 WHERE bot_user_id IN (777010, 777011, 777012)",
                [],
            )
            .map_db()?;

            Ok(())
        })
    }

    /// Creates both a `users` bot record and a `bots_registry` entry atomically with ID in 600_001..699_999.
    #[allow(clippy::too_many_arguments)]
    pub fn create_registered_bot(
        &self,
        owner_user_id: i64,
        username: &str,
        display_name: &str,
        description: &str,
        about: &str,
        ai_persona: &str,
        menu_button_text: &str,
        menu_button_url: &str,
        commands_json: &str,
        is_inline: bool,
        allow_attach_menu: bool,
    ) -> Result<BotRegistryRecord> {
        let menu_type = if menu_button_url.trim().is_empty() {
            "default"
        } else {
            "web_app"
        };
        self.create_registered_bot_with_options(
            owner_user_id,
            username,
            display_name,
            description,
            about,
            ai_persona,
            menu_type,
            menu_button_text,
            menu_button_url,
            commands_json,
            is_inline,
            allow_attach_menu,
            false,
        )
    }

    /// Creates a registered bot with explicit `menu_button_type` and `is_standard_bot` flag.
    #[allow(clippy::too_many_arguments)]
    pub fn create_registered_bot_with_options(
        &self,
        owner_user_id: i64,
        username: &str,
        display_name: &str,
        description: &str,
        about: &str,
        ai_persona: &str,
        menu_button_type: &str,
        menu_button_text: &str,
        menu_button_url: &str,
        commands_json: &str,
        is_inline: bool,
        allow_attach_menu: bool,
        is_standard_bot: bool,
    ) -> Result<BotRegistryRecord> {
        let clean_un = validate_bot_username(username).map_err(TelegramError::Validation)?;
        let clean_name = display_name.trim();
        if clean_name.is_empty() {
            return Err(TelegramError::Validation(
                "Bot display name cannot be empty".into(),
            ));
        }
        let normalized_menu_type = match menu_button_type.trim() {
            "commands" => "commands",
            "web_app" => "web_app",
            _ => "default",
        };
        let now_ms = chrono::Utc::now().timestamp_millis();

        self.pool.with_transaction(|tx| {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT user_id FROM users WHERE LOWER(username) = LOWER(?1)",
                    params![clean_un],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?;
            if existing.is_some() {
                return Err(TelegramError::Validation(format!(
                    "Sorry, username @{} is already taken",
                    clean_un
                )));
            }

            let max_bot_id: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(user_id), 600000) FROM users WHERE user_id >= 600000 AND user_id < 700000",
                    [],
                    |r| r.get(0),
                )
                .map_db()?;
            let bot_user_id = (max_bot_id + 1).max(USER_BOT_ID_START);
            if bot_user_id > USER_BOT_ID_MAX {
                return Err(TelegramError::Validation(
                    "Bot registry capacity reached for user-created bots".into(),
                ));
            }

            let bot_token = generate_bot_token(bot_user_id);
            let bio_text = if !description.trim().is_empty() {
                description.trim()
            } else {
                about.trim()
            };

            tx.execute(
                r#"INSERT INTO users (
                    user_id, username, phone, first_name, last_name, bio,
                    password_hash, is_bot, is_verified, is_support, is_scam, is_fake,
                    profile_photo_file_id, account_generation, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, NULL, ?3, NULL, ?4, NULL, 1, 0, 0, 0, 0, NULL, 1, ?5, ?5)"#,
                params![bot_user_id, clean_un, clean_name, bio_text, now_ms],
            )
            .map_db()?;

            let derived_agent_id = derive_safe_agent_id(None, &clean_un, bot_user_id);
            let derived_shape = default_fabushi_avatar_shape(&derived_agent_id);
            let derived_color = default_fabushi_avatar_color(&derived_agent_id);

            tx.execute(
                r#"INSERT INTO bots_registry (
                    bot_user_id, owner_user_id, username, display_name, bot_token,
                    description, about, ai_persona, menu_button_text, menu_button_url,
                    is_official, is_inline, allow_attach_menu, attach_peer_types_json,
                    commands_json, created_at, updated_at,
                    agent_id, avatar_shape, avatar_color,
                    preferred_model, thinking_level, approval_mode, toolset_preset,
                    menu_button_type, has_main_app, main_app_short_name,
                    privacy_policy_url, has_settings, can_join_groups,
                    webhook_url, webhook_secret_token, is_standard_bot
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12, '["users","bots","groups","channels"]', ?13, ?14, ?14, ?15, ?16, ?17, '', 'medium', 'prompt', 'full', ?18, 0, '', '', 0, 1, '', '', ?19)"#,
                params![
                    bot_user_id,
                    owner_user_id,
                    clean_un,
                    clean_name,
                    bot_token,
                    description.trim(),
                    about.trim(),
                    ai_persona.trim(),
                    menu_button_text.trim(),
                    menu_button_url.trim(),
                    is_inline as i64,
                    allow_attach_menu as i64,
                    commands_json,
                    now_ms,
                    derived_agent_id,
                    derived_shape,
                    derived_color,
                    normalized_menu_type,
                    is_standard_bot as i64,
                ],
            )
            .map_db()?;

            tx.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn get_bot_by_token(&self, token: &str) -> Result<Option<BotRegistryRecord>> {
        let clean = token.trim();
        if clean.is_empty() {
            return Ok(None);
        }
        self.pool.with_reader(|conn| {
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_token = ?1"),
                params![clean],
                row_to_bot,
            )
            .optional()
            .map_db()
        })
    }

    pub fn get_bot_by_user_id(&self, bot_user_id: i64) -> Result<Option<BotRegistryRecord>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .optional()
            .map_db()
        })
    }

    pub fn get_bot_by_username(&self, username: &str) -> Result<Option<BotRegistryRecord>> {
        let clean = normalize_bot_slug(username);
        if clean.is_empty() {
            return Ok(None);
        }
        self.pool.with_reader(|conn| {
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE LOWER(username) = LOWER(?1)"),
                params![clean],
                row_to_bot,
            )
            .optional()
            .map_db()
        })
    }

    pub fn list_bots_by_owner(&self, owner_user_id: i64) -> Result<Vec<BotRegistryRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {BOT_COLS} FROM bots_registry WHERE owner_user_id = ?1 ORDER BY created_at DESC"
                ))
                .map_db()?;
            let rows = stmt
                .query_map(params![owner_user_id], row_to_bot)
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })
    }

    pub fn list_all_bots(&self) -> Result<Vec<BotRegistryRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {BOT_COLS} FROM bots_registry ORDER BY is_official DESC, created_at ASC"
                ))
                .map_db()?;
            let rows = stmt
                .query_map([], row_to_bot)
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })
    }

    pub fn list_attach_menu_bots(
        &self,
        user_id: i64,
    ) -> Result<Vec<(BotRegistryRecord, Option<MiniAppRecord>, bool)>> {
        let bots = self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT DISTINCT {BOT_COLS} FROM bots_registry
                     WHERE allow_attach_menu = 1 OR bot_user_id IN (
                         SELECT bot_user_id FROM mini_apps WHERE is_published = 1
                     )
                     ORDER BY is_official DESC, created_at ASC"
                ))
                .map_db()?;
            let rows = stmt
                .query_map([], row_to_bot)
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })?;

        let mut out = Vec::with_capacity(bots.len());
        for bot in bots {
            let apps = self.list_mini_apps_for_bot(bot.bot_user_id)?;
            let primary = apps.into_iter().find(|a| a.is_published);
            let installed = if let Some(ref app) = primary {
                self.pool.with_reader(|conn| {
                    let count: i64 = conn
                        .query_row(
                            "SELECT COUNT(*) FROM user_mini_apps WHERE user_id = ?1 AND app_id = ?2",
                            params![user_id, app.app_id],
                            |r| r.get(0),
                        )
                        .map_db()?;
                    Ok(count > 0)
                })?
            } else {
                false
            };
            out.push((bot, primary, installed));
        }
        Ok(out)
    }

    pub fn regenerate_bot_token(&self, bot_user_id: i64) -> Result<BotRegistryRecord> {
        let new_token = generate_bot_token(bot_user_id);
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE bots_registry SET bot_token = ?1, updated_at = ?2 WHERE bot_user_id = ?3",
                params![new_token, now_ms, bot_user_id],
            )
            .map_db()?;
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn update_bot_name(
        &self,
        bot_user_id: i64,
        display_name: &str,
    ) -> Result<BotRegistryRecord> {
        let name = display_name.trim();
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            tx.execute(
                "UPDATE users SET first_name = ?1, updated_at_ms = ?2 WHERE user_id = ?3",
                params![name, now_ms, bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "UPDATE bots_registry SET display_name = ?1, updated_at = ?2 WHERE bot_user_id = ?3",
                params![name, now_ms, bot_user_id],
            )
            .map_db()?;
            tx.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn update_bot_description(
        &self,
        bot_user_id: i64,
        description: &str,
    ) -> Result<BotRegistryRecord> {
        let desc = description.trim();
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            tx.execute(
                "UPDATE bots_registry SET description = ?1, updated_at = ?2 WHERE bot_user_id = ?3",
                params![desc, now_ms, bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "UPDATE users SET bio = ?1, updated_at_ms = ?2 WHERE user_id = ?3",
                params![desc, now_ms, bot_user_id],
            )
            .map_db()?;
            tx.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn update_bot_about(&self, bot_user_id: i64, about: &str) -> Result<BotRegistryRecord> {
        let ab = about.trim();
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE bots_registry SET about = ?1, updated_at = ?2 WHERE bot_user_id = ?3",
                params![ab, now_ms, bot_user_id],
            )
            .map_db()?;
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn update_bot_commands(
        &self,
        bot_user_id: i64,
        commands_json: &str,
    ) -> Result<BotRegistryRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE bots_registry SET commands_json = ?1, updated_at = ?2 WHERE bot_user_id = ?3",
                params![commands_json, now_ms, bot_user_id],
            )
            .map_db()?;
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn update_bot_menu_button(
        &self,
        bot_user_id: i64,
        text: &str,
        url: &str,
    ) -> Result<BotRegistryRecord> {
        let menu_type = if url.trim().is_empty() {
            "default"
        } else {
            "web_app"
        };
        self.update_bot_menu_button_typed(bot_user_id, menu_type, text, url)
    }

    pub fn update_bot_menu_button_typed(
        &self,
        bot_user_id: i64,
        menu_button_type: &str,
        text: &str,
        url: &str,
    ) -> Result<BotRegistryRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let norm_type = match menu_button_type.trim() {
            "commands" => "commands",
            "web_app" => "web_app",
            _ => "default",
        };
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE bots_registry SET menu_button_type = ?1, menu_button_text = ?2, menu_button_url = ?3, updated_at = ?4 WHERE bot_user_id = ?5",
                params![norm_type, text.trim(), url.trim(), now_ms, bot_user_id],
            )
            .map_db()?;
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn update_bot_main_app(
        &self,
        bot_user_id: i64,
        has_main_app: bool,
        main_app_short_name: &str,
    ) -> Result<BotRegistryRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE bots_registry SET has_main_app = ?1, main_app_short_name = ?2, updated_at = ?3 WHERE bot_user_id = ?4",
                params![has_main_app as i64, main_app_short_name.trim(), now_ms, bot_user_id],
            )
            .map_db()?;
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn update_bot_privacy_policy(
        &self,
        bot_user_id: i64,
        privacy_policy_url: &str,
    ) -> Result<BotRegistryRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE bots_registry SET privacy_policy_url = ?1, updated_at = ?2 WHERE bot_user_id = ?3",
                params![privacy_policy_url.trim(), now_ms, bot_user_id],
            )
            .map_db()?;
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn update_bot_webhook(
        &self,
        bot_user_id: i64,
        webhook_url: &str,
        webhook_secret_token: &str,
    ) -> Result<BotRegistryRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE bots_registry SET webhook_url = ?1, webhook_secret_token = ?2, updated_at = ?3 WHERE bot_user_id = ?4",
                params![webhook_url.trim(), webhook_secret_token.trim(), now_ms, bot_user_id],
            )
            .map_db()?;
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn update_bot_settings_flags(
        &self,
        bot_user_id: i64,
        has_settings: Option<bool>,
        can_join_groups: Option<bool>,
    ) -> Result<BotRegistryRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"UPDATE bots_registry
                   SET has_settings = COALESCE(?1, has_settings),
                       can_join_groups = COALESCE(?2, can_join_groups),
                       updated_at = ?3
                   WHERE bot_user_id = ?4"#,
                params![
                    has_settings.map(|v| v as i64),
                    can_join_groups.map(|v| v as i64),
                    now_ms,
                    bot_user_id
                ],
            )
            .map_db()?;
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    pub fn update_bot_inline(&self, bot_user_id: i64, is_inline: bool) -> Result<BotRegistryRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE bots_registry SET is_inline = ?1, updated_at = ?2 WHERE bot_user_id = ?3",
                params![is_inline as i64, now_ms, bot_user_id],
            )
            .map_db()?;
            conn.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_bot_fabushi_fields(
        &self,
        bot_user_id: i64,
        display_name: Option<&str>,
        description: Option<&str>,
        about: Option<&str>,
        ai_persona: Option<&str>,
        agent_id: Option<&str>,
        avatar_shape: Option<&str>,
        avatar_color: Option<&str>,
        preferred_model: Option<&str>,
        thinking_level: Option<&str>,
        approval_mode: Option<&str>,
        toolset_preset: Option<&str>,
    ) -> Result<BotRegistryRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let current = tx
                .query_row(
                    &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                    params![bot_user_id],
                    row_to_bot,
                )
                .map_db()?;

            let next_name = display_name
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&current.display_name)
                .to_string();
            let next_desc = description
                .map(str::trim)
                .map(ToString::to_string)
                .unwrap_or_else(|| current.description.clone());
            let next_about = about
                .map(str::trim)
                .map(ToString::to_string)
                .unwrap_or_else(|| current.about.clone());
            let next_persona = ai_persona
                .map(str::trim)
                .map(ToString::to_string)
                .unwrap_or_else(|| current.ai_persona.clone());
            let next_agent_id = agent_id
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| derive_safe_agent_id(Some(s), &current.username, bot_user_id))
                .unwrap_or_else(|| {
                    if current.agent_id.trim().is_empty() {
                        derive_safe_agent_id(None, &current.username, bot_user_id)
                    } else {
                        current.agent_id.clone()
                    }
                });
            let next_shape = avatar_shape
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| normalize_fabushi_avatar_shape(s, &next_agent_id))
                .unwrap_or_else(|| {
                    normalize_fabushi_avatar_shape(&current.avatar_shape, &next_agent_id)
                });
            let next_color = avatar_color
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| normalize_fabushi_avatar_color(s, &next_agent_id))
                .unwrap_or_else(|| {
                    normalize_fabushi_avatar_color(&current.avatar_color, &next_agent_id)
                });
            let next_model = preferred_model
                .map(str::trim)
                .map(ToString::to_string)
                .unwrap_or_else(|| current.preferred_model.clone());
            let next_thinking = thinking_level
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if current.thinking_level.is_empty() {
                        "medium"
                    } else {
                        current.thinking_level.as_str()
                    }
                })
                .to_string();
            let next_approval = approval_mode
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if current.approval_mode.is_empty() {
                        "prompt"
                    } else {
                        current.approval_mode.as_str()
                    }
                })
                .to_string();
            let next_preset = toolset_preset
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if current.toolset_preset.is_empty() {
                        "full"
                    } else {
                        current.toolset_preset.as_str()
                    }
                })
                .to_string();

            tx.execute(
                r#"UPDATE bots_registry
                   SET display_name = ?1,
                       description = ?2,
                       about = ?3,
                       ai_persona = ?4,
                       agent_id = ?5,
                       avatar_shape = ?6,
                       avatar_color = ?7,
                       preferred_model = ?8,
                       thinking_level = ?9,
                       approval_mode = ?10,
                       toolset_preset = ?11,
                       updated_at = ?12
                   WHERE bot_user_id = ?13"#,
                params![
                    next_name,
                    next_desc,
                    next_about,
                    next_persona,
                    next_agent_id,
                    next_shape,
                    next_color,
                    next_model,
                    next_thinking,
                    next_approval,
                    next_preset,
                    now_ms,
                    bot_user_id
                ],
            )
            .map_db()?;

            let bio_text = if !next_desc.is_empty() {
                next_desc.as_str()
            } else {
                next_about.as_str()
            };
            tx.execute(
                "UPDATE users SET first_name = ?1, bio = ?2, updated_at_ms = ?3 WHERE user_id = ?4",
                params![next_name, bio_text, now_ms, bot_user_id],
            )
            .map_db()?;

            tx.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .map_db()
        })
    }

    /// Ensures any existing `users` row with `is_bot = 1` (e.g. default AI bot `777_000` or
    /// custom contact bots created via `/api/v1/contacts/add`) has a corresponding `bots_registry`
    /// entry with Fabushi identity columns populated.
    pub fn ensure_bot_registry_for_existing_bot_user(
        &self,
        bot_user_id: i64,
        owner_user_id: i64,
        default_persona: &str,
    ) -> Result<Option<BotRegistryRecord>> {
        if bot_user_id == BOTFATHER_USER_ID as i64 {
            return self.get_bot_by_user_id(bot_user_id);
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let user_row: Option<(Option<String>, String, String, String, i64)> = tx
                .query_row(
                    "SELECT username, first_name, COALESCE(last_name, ''), COALESCE(bio, ''), is_bot FROM users WHERE user_id = ?1",
                    params![bot_user_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()
                .map_db()?;
            let Some((username_opt, first_name, last_name, bio, is_bot)) = user_row else {
                return Ok(None);
            };
            if is_bot == 0 {
                return Ok(None);
            }

            let existing: Option<BotRegistryRecord> = tx
                .query_row(
                    &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                    params![bot_user_id],
                    row_to_bot,
                )
                .optional()
                .map_db()?;

            if let Some(rec) = existing {
                if !rec.agent_id.trim().is_empty()
                    && !rec.avatar_shape.trim().is_empty()
                    && !rec.avatar_color.trim().is_empty()
                {
                    return Ok(Some(rec));
                }
                let derived_id = if rec.agent_id.trim().is_empty() {
                    derive_safe_agent_id(None, &rec.username, bot_user_id)
                } else {
                    rec.agent_id.clone()
                };
                let derived_shape = normalize_fabushi_avatar_shape(&rec.avatar_shape, &derived_id);
                let derived_color = normalize_fabushi_avatar_color(&rec.avatar_color, &derived_id);
                tx.execute(
                    r#"UPDATE bots_registry
                       SET agent_id = ?1,
                           avatar_shape = ?2,
                           avatar_color = ?3,
                           thinking_level = CASE WHEN thinking_level = '' THEN 'medium' ELSE thinking_level END,
                           approval_mode = CASE WHEN approval_mode = '' THEN 'prompt' ELSE approval_mode END,
                           toolset_preset = CASE WHEN toolset_preset = '' THEN 'full' ELSE toolset_preset END
                       WHERE bot_user_id = ?4"#,
                    params![derived_id, derived_shape, derived_color, bot_user_id],
                )
                .map_db()?;
                return tx
                    .query_row(
                        &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                        params![bot_user_id],
                        row_to_bot,
                    )
                    .optional()
                    .map_db();
            }

            let raw_un = username_opt
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_start_matches('@').to_string())
                .unwrap_or_else(|| format!("fabushi_{bot_user_id}_bot"));
            let full_name = if last_name.trim().is_empty() {
                first_name.trim().to_string()
            } else {
                format!("{} {}", first_name.trim(), last_name.trim())
            };
            let display_name = if full_name.is_empty() {
                raw_un.clone()
            } else {
                full_name
            };
            let desc = if !bio.trim().is_empty() {
                bio.trim().to_string()
            } else if !default_persona.trim().is_empty() {
                default_persona.trim().to_string()
            } else {
                format!("Fabushi Bot {display_name}")
            };
            let is_official = (777_000..778_000).contains(&bot_user_id);
            let derived_id = derive_safe_agent_id(None, &raw_un, bot_user_id);
            let derived_shape = default_fabushi_avatar_shape(&derived_id);
            let derived_color = default_fabushi_avatar_color(&derived_id);
            let bot_token = generate_bot_token(bot_user_id);

            tx.execute(
                r#"INSERT INTO bots_registry (
                    bot_user_id, owner_user_id, username, display_name, bot_token,
                    description, about, ai_persona, menu_button_text, menu_button_url,
                    is_official, is_inline, allow_attach_menu, attach_peer_types_json,
                    commands_json, created_at, updated_at,
                    agent_id, avatar_shape, avatar_color,
                    preferred_model, thinking_level, approval_mode, toolset_preset
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '', '', ?9, 0, 0, '["users","bots","groups","channels"]', '[]', ?10, ?10, ?11, ?12, ?13, '', 'medium', 'prompt', 'full')"#,
                params![
                    bot_user_id,
                    owner_user_id,
                    raw_un,
                    display_name,
                    bot_token,
                    desc,
                    bio.trim(),
                    default_persona.trim(),
                    is_official as i64,
                    now_ms,
                    derived_id,
                    derived_shape,
                    derived_color,
                ],
            )
            .map_db()?;

            tx.query_row(
                &format!("SELECT {BOT_COLS} FROM bots_registry WHERE bot_user_id = ?1"),
                params![bot_user_id],
                row_to_bot,
            )
            .optional()
            .map_db()
        })
    }

    /// Generates a unique valid `<slug>_bot` username from a display name or preferred slug.
    pub fn generate_unique_bot_username(&self, desired_or_name: &str) -> Result<String> {
        let trimmed = desired_or_name.trim().trim_start_matches('@');
        if let Ok(valid) = validate_bot_username(trimmed) {
            let taken = self.pool.with_reader(|conn| {
                let exists: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM users WHERE LOWER(username) = LOWER(?1)",
                        params![valid],
                        |r| r.get(0),
                    )
                    .map_db()?;
                Ok(exists > 0)
            })?;
            if !taken {
                return Ok(valid);
            }
        }

        let mut base = String::new();
        let mut last_underscore = false;
        for ch in trimmed.chars() {
            if ch.is_ascii_alphanumeric() {
                base.push(ch.to_ascii_lowercase());
                last_underscore = false;
            } else if !last_underscore && !base.is_empty() {
                base.push('_');
                last_underscore = true;
            }
        }
        while base.ends_with('_') {
            base.pop();
        }
        if base.is_empty() || base.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            base = if base.is_empty() {
                "fabushi_agent".to_string()
            } else {
                format!("bot_{base}")
            };
        }
        if let Some(stripped) = base.strip_suffix("_bot") {
            base = stripped.to_string();
        } else if let Some(stripped) = base.strip_suffix("bot") {
            if stripped.len() >= 3 {
                base = stripped.to_string();
            }
        }
        while base.ends_with('_') {
            base.pop();
        }
        if base.is_empty() {
            base = "fabushi_agent".to_string();
        }

        for attempt in 0..200usize {
            let suffix = if attempt == 0 {
                "_bot".to_string()
            } else {
                format!("_{}_bot", attempt + 1)
            };
            let max_prefix = 32usize.saturating_sub(suffix.len());
            let mut prefix: String = base.chars().take(max_prefix).collect();
            while prefix.ends_with('_') {
                prefix.pop();
            }
            if prefix.is_empty() {
                prefix = "agent".to_string();
            }
            let candidate = format!("{prefix}{suffix}");
            if validate_bot_username(&candidate).is_err() {
                continue;
            }
            let taken = self.pool.with_reader(|conn| {
                let exists: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM users WHERE LOWER(username) = LOWER(?1)",
                        params![candidate],
                        |r| r.get(0),
                    )
                    .map_db()?;
                Ok(exists > 0)
            })?;
            if !taken {
                return Ok(candidate);
            }
        }

        Ok(format!(
            "fabushi_{}_bot",
            (chrono::Utc::now().timestamp_millis() % 1_000_000).abs()
        ))
    }

    pub fn find_companion_chat_id_for_user(
        &self,
        bot_user_id: i64,
        user_id: i64,
    ) -> Result<Option<ChatId>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                r#"SELECT c.chat_id FROM chats c
                   JOIN chat_members m1 ON m1.chat_id = c.chat_id AND m1.user_id = ?1
                   JOIN chat_members m2 ON m2.chat_id = c.chat_id AND m2.user_id = ?2
                   WHERE c.chat_type IN (0, 4)
                     AND (SELECT COUNT(*) FROM chat_members mx WHERE mx.chat_id = c.chat_id) = 2
                   ORDER BY c.chat_id ASC
                   LIMIT 1"#,
                params![user_id, bot_user_id],
                |r| r.get::<_, i64>(0).map(ChatId),
            )
            .optional()
            .map_db()
        })
    }

    pub fn list_all_bot_user_ids_from_users(&self) -> Result<Vec<i64>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare("SELECT user_id FROM users WHERE is_bot = 1 ORDER BY user_id ASC")
                .map_db()?;
            let rows = stmt
                .query_map([], |r| r.get::<_, i64>(0))
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })
    }

    pub fn delete_registered_bot(&self, bot_user_id: i64, owner_user_id: i64) -> Result<bool> {
        self.pool.with_transaction(|tx| {
            let owned: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT owner_user_id, is_official FROM bots_registry WHERE bot_user_id = ?1",
                    params![bot_user_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_db()?;
            let Some((owner, is_official)) = owned else {
                return Ok(false);
            };
            if is_official != 0 || owner != owner_user_id {
                return Err(TelegramError::PermissionDenied(
                    "Only the bot owner can delete a user-created bot".into(),
                ));
            }
            // Clean up Mini App & CloudStorage state tied to the bot before dropping chats/user
            tx.execute(
                "DELETE FROM mini_app_sessions WHERE bot_user_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM mini_app_cloud_storage WHERE bot_user_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM user_mini_apps WHERE bot_user_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM user_bot_permissions WHERE bot_user_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM mini_app_usage_events WHERE bot_user_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM bot_api_updates WHERE bot_user_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM mini_app_hosted_bundles WHERE app_id IN (SELECT app_id FROM mini_apps WHERE bot_user_id = ?1)",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM mini_apps WHERE bot_user_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM bots_registry WHERE bot_user_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;

            // Remove 1:1 companion DM chats involving this bot so dead DMs and welcome messages disappear
            let companion_chat_ids: Vec<i64> = {
                let mut stmt = tx
                    .prepare(
                        r#"SELECT DISTINCT c.chat_id
                           FROM chats c
                           LEFT JOIN chat_members cm ON cm.chat_id = c.chat_id
                           WHERE c.chat_type IN (0, 4)
                             AND (cm.user_id = ?1 OR c.creator_id = ?1)"#,
                    )
                    .map_db()?;
                let rows = stmt
                    .query_map(params![bot_user_id], |r| r.get(0))
                    .map_db()?;
                rows.collect::<std::result::Result<Vec<i64>, _>>().map_db()?
            };
            for cid in companion_chat_ids {
                tx.execute("DELETE FROM pts_updates WHERE chat_id = ?1", params![cid])
                    .map_db()?;
                tx.execute("DELETE FROM pts_sequences WHERE chat_id = ?1", params![cid])
                    .map_db()?;
                tx.execute(
                    "DELETE FROM chat_message_sequences WHERE chat_id = ?1",
                    params![cid],
                )
                .map_db()?;
                tx.execute("DELETE FROM chats WHERE chat_id = ?1", params![cid])
                    .map_db()?;
            }

            // Clean up all non-cascading FK references to users(user_id) before deleting the bot user
            tx.execute(
                "DELETE FROM pinned_messages WHERE pinned_by = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM polls WHERE creator_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM chat_invites WHERE creator_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM forum_topics WHERE creator_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM messages WHERE sender_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM message_dedup WHERE sender_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM media_files WHERE uploader_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM calls WHERE caller_id = ?1 OR callee_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM orders WHERE user_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "UPDATE chat_members SET invited_by = NULL WHERE invited_by = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            tx.execute(
                "UPDATE chats SET creator_id = ?2 WHERE creator_id = ?1",
                params![bot_user_id, owner_user_id],
            )
            .map_db()?;

            tx.execute("DELETE FROM users WHERE user_id = ?1", params![bot_user_id])
                .map_db()?;
            Ok(true)
        })
    }

    // --- BotFather Conversation State Machine Persistence ---

    pub fn get_botfather_session(&self, user_id: i64) -> Result<Option<BotfatherSessionRecord>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT user_id, chat_id, state, draft_json, updated_at FROM botfather_sessions WHERE user_id = ?1",
                params![user_id],
                |r| {
                    Ok(BotfatherSessionRecord {
                        user_id: r.get(0)?,
                        chat_id: r.get(1)?,
                        state: r.get(2)?,
                        draft_json: r.get(3)?,
                        updated_at: r.get(4)?,
                    })
                },
            )
            .optional()
            .map_db()
        })
    }

    pub fn upsert_botfather_session(
        &self,
        user_id: i64,
        chat_id: i64,
        state: &str,
        draft_json: &str,
    ) -> Result<BotfatherSessionRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO botfather_sessions (user_id, chat_id, state, draft_json, updated_at)
                   VALUES (?1, ?2, ?3, ?4, ?5)
                   ON CONFLICT(user_id) DO UPDATE SET
                       chat_id = excluded.chat_id,
                       state = excluded.state,
                       draft_json = excluded.draft_json,
                       updated_at = excluded.updated_at"#,
                params![user_id, chat_id, state, draft_json, now_ms],
            )
            .map_db()?;
            Ok(BotfatherSessionRecord {
                user_id,
                chat_id,
                state: state.to_string(),
                draft_json: draft_json.to_string(),
                updated_at: now_ms,
            })
        })
    }

    pub fn clear_botfather_session(&self, user_id: i64) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "DELETE FROM botfather_sessions WHERE user_id = ?1",
                params![user_id],
            )
            .map_db()?;
            Ok(())
        })
    }

    // --- Mini Apps Catalog & CRUD ---

    #[allow(clippy::too_many_arguments)]
    pub fn create_or_upsert_mini_app(
        &self,
        bot_user_id: i64,
        bot_username: &str,
        short_name: &str,
        title: &str,
        description: &str,
        category: &str,
        icon_emoji: &str,
        web_url: Option<&str>,
        ui_schema_json: &str,
        is_published: bool,
        is_attach_enabled: bool,
        created_by_nl: bool,
    ) -> Result<MiniAppRecord> {
        self.create_or_upsert_mini_app_with_options(
            bot_user_id,
            bot_username,
            short_name,
            title,
            description,
            category,
            icon_emoji,
            web_url,
            ui_schema_json,
            is_published,
            is_attach_enabled,
            created_by_nl,
            "",
            "",
            None,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_or_upsert_mini_app_with_options(
        &self,
        bot_user_id: i64,
        bot_username: &str,
        short_name: &str,
        title: &str,
        description: &str,
        category: &str,
        icon_emoji: &str,
        web_url: Option<&str>,
        ui_schema_json: &str,
        is_published: bool,
        is_attach_enabled: bool,
        created_by_nl: bool,
        header_color: &str,
        background_color: &str,
        set_as_main_app: Option<bool>,
        auto_menu_button: bool,
    ) -> Result<MiniAppRecord> {
        let clean_short = validate_miniapp_short_name(short_name.trim_start_matches('/'))
            .map_err(TelegramError::Validation)?;
        let clean_title = title.trim();
        if clean_title.is_empty() {
            return Err(TelegramError::Validation(
                "Mini App title cannot be empty".into(),
            ));
        }
        let clean_bot_un = normalize_bot_slug(bot_username);
        let resolved_url = match web_url.map(str::trim).filter(|s| !s.is_empty()) {
            Some(u) => u.to_string(),
            None => format!("/miniapps/{}/{}", clean_bot_un, clean_short),
        };
        let app_id = format!("{}_{}", bot_user_id, clean_short.to_ascii_lowercase());
        let icon = if icon_emoji.trim().is_empty() {
            "🧩"
        } else {
            icon_emoji.trim()
        };
        let cat = if category.trim().is_empty() {
            "productivity"
        } else {
            category.trim()
        };
        let now_ms = chrono::Utc::now().timestamp_millis();

        self.pool.with_transaction(|tx| {
            tx.execute(
                r#"INSERT INTO mini_apps (
                    app_id, bot_user_id, bot_username, short_name, title,
                    description, category, icon_emoji, web_url, ui_schema_json,
                    peer_types_json, is_published, is_attach_enabled, install_count,
                    created_by_nl, created_at, updated_at, header_color, background_color
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, '["users","bots","groups","channels"]', ?11, ?12, 0, ?13, ?14, ?14, ?15, ?16)
                ON CONFLICT(bot_username, short_name) DO UPDATE SET
                    title = excluded.title,
                    description = excluded.description,
                    category = excluded.category,
                    icon_emoji = excluded.icon_emoji,
                    web_url = excluded.web_url,
                    ui_schema_json = excluded.ui_schema_json,
                    is_published = excluded.is_published,
                    is_attach_enabled = excluded.is_attach_enabled,
                    header_color = CASE WHEN excluded.header_color != '' THEN excluded.header_color ELSE mini_apps.header_color END,
                    background_color = CASE WHEN excluded.background_color != '' THEN excluded.background_color ELSE mini_apps.background_color END,
                    updated_at = excluded.updated_at"#,
                params![
                    app_id,
                    bot_user_id,
                    clean_bot_un,
                    clean_short,
                    clean_title,
                    description.trim(),
                    cat,
                    icon,
                    resolved_url,
                    ui_schema_json,
                    is_published as i64,
                    is_attach_enabled as i64,
                    created_by_nl as i64,
                    now_ms,
                    header_color.trim(),
                    background_color.trim(),
                ],
            )
            .map_db()?;

            let (is_standard, has_main_app): (i64, i64) = tx
                .query_row(
                    "SELECT is_standard_bot, has_main_app FROM bots_registry WHERE bot_user_id = ?1",
                    params![bot_user_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_db()?
                .unwrap_or((0, 0));

            let should_set_main = match set_as_main_app {
                Some(v) => v,
                None => has_main_app == 0,
            };
            if should_set_main {
                tx.execute(
                    "UPDATE bots_registry SET has_main_app = 1, main_app_short_name = ?1, updated_at = ?2 WHERE bot_user_id = ?3",
                    params![clean_short, now_ms, bot_user_id],
                )
                .map_db()?;
            }

            // Only auto-configure Menu Button for non-standard AI companion bots when requested.
            if auto_menu_button && is_standard == 0 {
                let default_btn_text = format!("打开 {}", clean_title);
                tx.execute(
                    r#"UPDATE bots_registry
                       SET menu_button_type = CASE WHEN menu_button_url = '' THEN 'web_app' ELSE menu_button_type END,
                           menu_button_text = CASE WHEN menu_button_text = '' THEN ?1 ELSE menu_button_text END,
                           menu_button_url = CASE WHEN menu_button_url = '' THEN ?2 ELSE menu_button_url END,
                           allow_attach_menu = CASE WHEN ?3 != 0 THEN 1 ELSE allow_attach_menu END,
                           updated_at = ?4
                       WHERE bot_user_id = ?5"#,
                    params![
                        default_btn_text,
                        resolved_url,
                        is_attach_enabled as i64,
                        now_ms,
                        bot_user_id
                    ],
                )
                .map_db()?;
            } else if is_attach_enabled {
                tx.execute(
                    "UPDATE bots_registry SET allow_attach_menu = 1, updated_at = ?1 WHERE bot_user_id = ?2",
                    params![now_ms, bot_user_id],
                )
                .map_db()?;
            }

            tx.query_row(
                &format!("SELECT {MINIAPP_COLS} FROM mini_apps WHERE app_id = ?1"),
                params![app_id],
                row_to_mini_app,
            )
            .map_db()
        })
    }

    pub fn get_mini_app(
        &self,
        bot_username: &str,
        short_name: &str,
    ) -> Result<Option<MiniAppRecord>> {
        let clean_bot = normalize_bot_slug(bot_username);
        let clean_short = short_name.trim().trim_start_matches('/');
        self.pool.with_reader(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {MINIAPP_COLS} FROM mini_apps WHERE LOWER(bot_username) = LOWER(?1) AND LOWER(short_name) = LOWER(?2)"
                ),
                params![clean_bot, clean_short],
                row_to_mini_app,
            )
            .optional()
            .map_db()
        })
    }

    pub fn list_mini_apps_for_bot(&self, bot_user_id: i64) -> Result<Vec<MiniAppRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {MINIAPP_COLS} FROM mini_apps WHERE bot_user_id = ?1 ORDER BY created_at DESC"
                ))
                .map_db()?;
            let rows = stmt
                .query_map(params![bot_user_id], row_to_mini_app)
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })
    }

    pub fn list_mini_apps_by_owner(&self, owner_user_id: i64) -> Result<Vec<MiniAppRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT m.app_id, m.bot_user_id, m.bot_username, m.short_name, m.title, m.description, m.category, m.icon_emoji, m.web_url, m.ui_schema_json, m.peer_types_json, m.is_published, m.is_attach_enabled, m.install_count, m.created_by_nl, m.created_at, m.updated_at, m.header_color, m.background_color
                     FROM mini_apps m
                     JOIN bots_registry b ON b.bot_user_id = m.bot_user_id
                     WHERE b.owner_user_id = ?1 AND m.category != '__bot_inline_webapp__'
                     ORDER BY m.created_at DESC",
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![owner_user_id], row_to_mini_app)
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })
    }

    pub fn search_published_mini_apps(
        &self,
        query: &str,
        category: Option<&str>,
    ) -> Result<Vec<MiniAppRecord>> {
        let q = query.trim().to_ascii_lowercase();
        let cat = category
            .map(str::trim)
            .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("all"));
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {MINIAPP_COLS} FROM mini_apps WHERE is_published = 1 ORDER BY install_count DESC, created_at DESC"
                ))
                .map_db()?;
            let all = stmt
                .query_map([], row_to_mini_app)
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            let filtered = all
                .into_iter()
                .filter(|app| {
                    let cat_ok = match cat {
                        Some(c) => app.category.eq_ignore_ascii_case(c),
                        None => true,
                    };
                    let q_ok = q.is_empty()
                        || app.title.to_ascii_lowercase().contains(&q)
                        || app.description.to_ascii_lowercase().contains(&q)
                        || app.bot_username.to_ascii_lowercase().contains(&q)
                        || app.short_name.to_ascii_lowercase().contains(&q)
                        || app.category.to_ascii_lowercase().contains(&q);
                    cat_ok && q_ok
                })
                .collect();
            Ok(filtered)
        })
    }

    pub fn delete_mini_app(
        &self,
        bot_username: &str,
        short_name: &str,
        owner_user_id: i64,
    ) -> Result<bool> {
        let clean_bot = normalize_bot_slug(bot_username);
        let clean_short = short_name.trim().trim_start_matches('/');
        self.pool.with_transaction(|tx| {
            let found: Option<(String, i64, i64)> = tx
                .query_row(
                    r#"SELECT m.app_id, b.owner_user_id, b.is_official
                       FROM mini_apps m
                       JOIN bots_registry b ON b.bot_user_id = m.bot_user_id
                       WHERE LOWER(m.bot_username) = LOWER(?1) AND LOWER(m.short_name) = LOWER(?2)"#,
                    params![clean_bot, clean_short],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_db()?;
            let Some((app_id, owner, is_official)) = found else {
                return Ok(false);
            };
            if is_official != 0 || owner != owner_user_id {
                return Err(TelegramError::PermissionDenied(
                    "Only the bot owner can delete this Mini App".into(),
                ));
            }
            tx.execute(
                "DELETE FROM user_mini_apps WHERE app_id = ?1",
                params![app_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM mini_app_sessions WHERE app_id = ?1",
                params![app_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM mini_app_usage_events WHERE app_id = ?1",
                params![app_id],
            )
            .map_db()?;
            tx.execute(
                "DELETE FROM mini_app_hosted_bundles WHERE app_id = ?1",
                params![app_id],
            )
            .map_db()?;
            tx.execute("DELETE FROM mini_apps WHERE app_id = ?1", params![app_id])
                .map_db()?;
            let now_ms = chrono::Utc::now().timestamp_millis();
            let internal_menu_url = format!("/miniapps/{clean_bot}/{clean_short}");
            tx.execute(
                "UPDATE bots_registry
                 SET has_main_app = CASE WHEN LOWER(main_app_short_name) = LOWER(?2) THEN 0 ELSE has_main_app END,
                     main_app_short_name = CASE WHEN LOWER(main_app_short_name) = LOWER(?2) THEN '' ELSE main_app_short_name END,
                     menu_button_type = CASE WHEN LOWER(menu_button_url) = LOWER(?3) THEN 'commands' ELSE menu_button_type END,
                     menu_button_url = CASE WHEN LOWER(menu_button_url) = LOWER(?3) THEN '' ELSE menu_button_url END,
                     updated_at = ?4
                 WHERE LOWER(username) = LOWER(?1)",
                params![clean_bot, clean_short, internal_menu_url, now_ms],
            )
            .map_db()?;
            Ok(true)
        })
    }

    // --- Companion Bot Chat Provisioning (600_000_000..699_999_999) ---

    /// Idempotently creates or resolves a 1:1 DM chat (`chat_type = 4`) between `user_id` and `bot_user_id`
    /// in the strict companion bot range `600_000_000..=699_999_999` (isolated from Welcome Bot 700M..750M & human DMs 750M+).
    /// Returns `(chat_id, created_now)`.
    pub fn ensure_companion_bot_chat_record(
        &self,
        user_id: UserId,
        bot_user_id: UserId,
    ) -> Result<(ChatId, bool)> {
        let bot_user = self
            .get_user(bot_user_id)?
            .ok_or_else(|| TelegramError::NotFound(format!("Bot {} not found", bot_user_id.0)))?;

        // 1. Check if a 1:1 bot/direct chat between user_id and bot_user_id already exists
        let existing_chat: Option<i64> = self.pool.with_reader(|conn| {
            conn.query_row(
                r#"SELECT c.chat_id FROM chats c
                   JOIN chat_members m1 ON m1.chat_id = c.chat_id AND m1.user_id = ?1
                   JOIN chat_members m2 ON m2.chat_id = c.chat_id AND m2.user_id = ?2
                   WHERE c.chat_type IN (0, 4)
                     AND (SELECT COUNT(*) FROM chat_members mx WHERE mx.chat_id = c.chat_id) = 2
                   ORDER BY c.chat_id ASC
                   LIMIT 1"#,
                params![user_id.0 as i64, bot_user_id.0 as i64],
                |r| r.get(0),
            )
            .optional()
            .map_db()
        })?;

        if let Some(cid) = existing_chat {
            return Ok((ChatId(cid), false));
        }

        // 2. Allocate deterministic, collision-free ChatId inside 600_000_000..=699_999_999
        let base_slot = COMPANION_CHAT_ID_MIN
            + ((user_id.0 as i64 % 9_999) * 10_000)
            + ((bot_user_id.0 as i64 % 9_999) + 1);
        let mut candidate = base_slot.clamp(COMPANION_CHAT_ID_MIN, COMPANION_CHAT_ID_MAX);
        let now_ms = chrono::Utc::now().timestamp_millis();

        let chat_id = self.pool.with_transaction(|tx| {
            loop {
                let exists: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM chats WHERE chat_id = ?1",
                        params![candidate],
                        |r| r.get(0),
                    )
                    .map_db()?;
                if exists == 0 {
                    break;
                }
                candidate += 1;
                if candidate > COMPANION_CHAT_ID_MAX {
                    candidate = COMPANION_CHAT_ID_MIN;
                }
            }

            let title = if bot_user.first_name.trim().is_empty() {
                bot_user
                    .username
                    .clone()
                    .unwrap_or_else(|| format!("Bot {}", bot_user_id.0))
            } else {
                bot_user.first_name.clone()
            };

            let ch = ChatRecord {
                chat_id: ChatId(candidate),
                chat_type: 4,
                peer_type: 1,
                title,
                username: Some(format!("dm_{}_{}", user_id.0, bot_user_id.0)),
                about: bot_user.bio.clone(),
                creator_id: user_id,
                is_forum: false,
                flags: 0,
                photo_file_id: None,
                slowmode_seconds: 0,
                linked_chat_id: None,
                pinned_message_id: None,
                pts: 0,
                last_message_id: MsgId(0),
                last_message_date_ms: 0,
                unread_count: 0,
                account_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            };

            tx.execute(
                r#"INSERT INTO chats (
                    chat_id, chat_type, peer_type, title, username, about, creator_id,
                    is_forum, flags, photo_file_id, slowmode_seconds, linked_chat_id,
                    pinned_message_id, pts, last_message_id, last_message_date_ms,
                    unread_count, account_generation, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, NULL, 0, NULL, NULL, 0, 0, 0, 0, 1, ?8, ?8)"#,
                params![
                    ch.chat_id.0,
                    ch.chat_type as i64,
                    ch.peer_type as i64,
                    ch.title,
                    ch.username,
                    ch.about,
                    ch.creator_id.0 as i64,
                    now_ms
                ],
            )
            .map_db()?;

            let m1 = ChatMemberRecord {
                chat_id: ch.chat_id,
                user_id,
                role: "owner".into(),
                permissions_bitmask: 0xFFFF,
                admin_title: None,
                admin_rights: 0,
                banned_rights: 0,
                until_date_ms: None,
                joined_at_ms: now_ms,
                invited_by: Some(user_id),
                updated_at_ms: now_ms,
            };
            let m2 = ChatMemberRecord {
                chat_id: ch.chat_id,
                user_id: bot_user_id,
                role: "member".into(),
                permissions_bitmask: 0xFFFF,
                admin_title: None,
                admin_rights: 0,
                banned_rights: 0,
                until_date_ms: None,
                joined_at_ms: now_ms,
                invited_by: Some(user_id),
                updated_at_ms: now_ms,
            };

            for m in [m1, m2] {
                tx.execute(
                    r#"INSERT INTO chat_members (
                        chat_id, user_id, role, permissions_bitmask, admin_title,
                        admin_rights, banned_rights, until_date_ms, joined_at_ms,
                        invited_by, updated_at_ms
                    ) VALUES (?1, ?2, ?3, ?4, NULL, 0, 0, NULL, ?5, ?6, ?5)"#,
                    params![
                        m.chat_id.0,
                        m.user_id.0 as i64,
                        m.role,
                        m.permissions_bitmask as i64,
                        now_ms,
                        m.invited_by.map(|u| u.0 as i64)
                    ],
                )
                .map_db()?;
            }

            tx.execute(
                r#"INSERT INTO contacts (owner_user_id, contact_user_id, first_name, last_name, phone, created_at_ms, updated_at_ms)
                   VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?4)
                   ON CONFLICT(owner_user_id, contact_user_id) DO UPDATE SET
                       first_name = excluded.first_name,
                       updated_at_ms = excluded.updated_at_ms"#,
                params![user_id.0 as i64, bot_user_id.0 as i64, ch.title, now_ms],
            )
            .map_db()?;

            Ok(ch.chat_id)
        })?;

        Ok((chat_id, true))
    }

    pub fn install_or_touch_user_mini_app(
        &self,
        user_id: i64,
        app_id: &str,
        bot_user_id: i64,
        companion_chat_id: i64,
        allow_write_access: bool,
    ) -> Result<(UserMiniAppRecord, bool)> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let existing: Option<UserMiniAppRecord> = tx
                .query_row(
                    "SELECT user_id, app_id, bot_user_id, companion_chat_id, allow_write_access, installed_at, last_launched_at FROM user_mini_apps WHERE user_id = ?1 AND app_id = ?2",
                    params![user_id, app_id],
                    row_to_user_mini_app,
                )
                .optional()
                .map_db()?;

            let is_new = existing.is_none();
            if is_new {
                tx.execute(
                    r#"INSERT INTO user_mini_apps (user_id, app_id, bot_user_id, companion_chat_id, allow_write_access, installed_at, last_launched_at)
                       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)"#,
                    params![user_id, app_id, bot_user_id, companion_chat_id, allow_write_access as i64, now_ms],
                )
                .map_db()?;
                tx.execute(
                    "UPDATE mini_apps SET install_count = install_count + 1, updated_at = ?1 WHERE app_id = ?2",
                    params![now_ms, app_id],
                )
                .map_db()?;
            } else {
                tx.execute(
                    "UPDATE user_mini_apps SET companion_chat_id = ?1, allow_write_access = ?2, last_launched_at = ?3 WHERE user_id = ?4 AND app_id = ?5",
                    params![companion_chat_id, allow_write_access as i64, now_ms, user_id, app_id],
                )
                .map_db()?;
            }

            let rec = tx
                .query_row(
                    "SELECT user_id, app_id, bot_user_id, companion_chat_id, allow_write_access, installed_at, last_launched_at FROM user_mini_apps WHERE user_id = ?1 AND app_id = ?2",
                    params![user_id, app_id],
                    row_to_user_mini_app,
                )
                .map_db()?;
            Ok((rec, is_new))
        })
    }

    pub fn uninstall_user_mini_app(&self, user_id: i64, app_id: &str) -> Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let deleted = tx
                .execute(
                    "DELETE FROM user_mini_apps WHERE user_id = ?1 AND app_id = ?2",
                    params![user_id, app_id],
                )
                .map_db()?;
            if deleted > 0 {
                tx.execute(
                    "UPDATE mini_apps SET install_count = MAX(0, install_count - 1), updated_at = ?1 WHERE app_id = ?2",
                    params![now_ms, app_id],
                )
                .map_db()?;
            }
            Ok(deleted > 0)
        })
    }

    pub fn list_user_installed_mini_apps(
        &self,
        user_id: i64,
    ) -> Result<Vec<(MiniAppRecord, UserMiniAppRecord, BotRegistryRecord)>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"SELECT m.app_id, m.bot_user_id, m.bot_username, m.short_name, m.title, m.description, m.category, m.icon_emoji, m.web_url, m.ui_schema_json, m.peer_types_json, m.is_published, m.is_attach_enabled, m.install_count, m.created_by_nl, m.created_at, m.updated_at, m.header_color, m.background_color,
                              u.user_id, u.app_id, u.bot_user_id, u.companion_chat_id, u.allow_write_access, u.installed_at, u.last_launched_at,
                              b.bot_user_id, b.owner_user_id, b.username, b.display_name, b.bot_token, b.description, b.about, b.ai_persona, b.menu_button_text, b.menu_button_url, b.is_official, b.is_inline, b.allow_attach_menu, b.attach_peer_types_json, b.commands_json, b.created_at, b.updated_at, b.agent_id, b.avatar_shape, b.avatar_color, b.preferred_model, b.thinking_level, b.approval_mode, b.toolset_preset, b.menu_button_type, b.has_main_app, b.main_app_short_name, b.privacy_policy_url, b.has_settings, b.can_join_groups, b.webhook_url, b.webhook_secret_token, b.is_standard_bot
                       FROM user_mini_apps u
                       JOIN mini_apps m ON m.app_id = u.app_id
                       JOIN bots_registry b ON b.bot_user_id = m.bot_user_id
                       WHERE u.user_id = ?1
                       ORDER BY u.last_launched_at DESC"#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id], |r| {
                    let app = MiniAppRecord {
                        app_id: r.get(0)?,
                        bot_user_id: r.get(1)?,
                        bot_username: r.get(2)?,
                        short_name: r.get(3)?,
                        title: r.get(4)?,
                        description: r.get(5)?,
                        category: r.get(6)?,
                        icon_emoji: r.get(7)?,
                        web_url: r.get(8)?,
                        ui_schema_json: r.get(9)?,
                        peer_types_json: r.get(10)?,
                        is_published: r.get::<_, i64>(11)? != 0,
                        is_attach_enabled: r.get::<_, i64>(12)? != 0,
                        install_count: r.get(13)?,
                        created_by_nl: r.get::<_, i64>(14)? != 0,
                        created_at: r.get(15)?,
                        updated_at: r.get(16)?,
                        header_color: r.get::<_, String>(17).unwrap_or_default(),
                        background_color: r.get::<_, String>(18).unwrap_or_default(),
                    };
                    let install = UserMiniAppRecord {
                        user_id: r.get(19)?,
                        app_id: r.get(20)?,
                        bot_user_id: r.get(21)?,
                        companion_chat_id: r.get(22)?,
                        allow_write_access: r.get::<_, i64>(23)? != 0,
                        installed_at: r.get(24)?,
                        last_launched_at: r.get(25)?,
                    };
                    let bot = BotRegistryRecord {
                        bot_user_id: r.get(26)?,
                        owner_user_id: r.get(27)?,
                        username: r.get(28)?,
                        display_name: r.get(29)?,
                        bot_token: r.get(30)?,
                        description: r.get(31)?,
                        about: r.get(32)?,
                        ai_persona: r.get(33)?,
                        menu_button_text: r.get(34)?,
                        menu_button_url: r.get(35)?,
                        is_official: r.get::<_, i64>(36)? != 0,
                        is_inline: r.get::<_, i64>(37)? != 0,
                        allow_attach_menu: r.get::<_, i64>(38)? != 0,
                        attach_peer_types_json: r.get(39)?,
                        commands_json: r.get(40)?,
                        created_at: r.get(41)?,
                        updated_at: r.get(42)?,
                        agent_id: r.get::<_, String>(43).unwrap_or_default(),
                        avatar_shape: r.get::<_, String>(44).unwrap_or_default(),
                        avatar_color: r.get::<_, String>(45).unwrap_or_default(),
                        preferred_model: r.get::<_, String>(46).unwrap_or_default(),
                        thinking_level: r
                            .get::<_, String>(47)
                            .ok()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "medium".to_string()),
                        approval_mode: r
                            .get::<_, String>(48)
                            .ok()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "prompt".to_string()),
                        toolset_preset: r
                            .get::<_, String>(49)
                            .ok()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "full".to_string()),
                        menu_button_type: r
                            .get::<_, String>(50)
                            .ok()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "default".to_string()),
                        has_main_app: r.get::<_, i64>(51).unwrap_or(0) != 0,
                        main_app_short_name: r.get::<_, String>(52).unwrap_or_default(),
                        privacy_policy_url: r.get::<_, String>(53).unwrap_or_default(),
                        has_settings: r.get::<_, i64>(54).unwrap_or(0) != 0,
                        can_join_groups: r.get::<_, i64>(55).unwrap_or(1) != 0,
                        webhook_url: r.get::<_, String>(56).unwrap_or_default(),
                        webhook_secret_token: r.get::<_, String>(57).unwrap_or_default(),
                        is_standard_bot: r.get::<_, i64>(58).unwrap_or(0) != 0,
                    };
                    Ok((app, install, bot))
                })
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })
    }

    // --- Mini App Sessions & State ---

    #[allow(clippy::too_many_arguments)]
    pub fn create_mini_app_session(
        &self,
        query_id: &str,
        user_id: i64,
        app_id: &str,
        bot_user_id: i64,
        chat_id: i64,
        launch_mode: &str,
        start_param: &str,
        init_data_raw: &str,
        auth_date: i64,
        state_json: &str,
    ) -> Result<MiniAppSessionRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO mini_app_sessions (
                    query_id, user_id, app_id, bot_user_id, chat_id,
                    launch_mode, start_param, init_data_raw, auth_date, state_json,
                    created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)"#,
                params![
                    query_id,
                    user_id,
                    app_id,
                    bot_user_id,
                    chat_id,
                    launch_mode,
                    start_param,
                    init_data_raw,
                    auth_date,
                    state_json,
                    now_ms
                ],
            )
            .map_db()?;
            conn.query_row(
                "SELECT query_id, user_id, app_id, bot_user_id, chat_id, launch_mode, start_param, init_data_raw, auth_date, state_json, created_at, updated_at FROM mini_app_sessions WHERE query_id = ?1",
                params![query_id],
                row_to_mini_app_session,
            )
            .map_db()
        })
    }

    pub fn get_mini_app_session(&self, query_id: &str) -> Result<Option<MiniAppSessionRecord>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT query_id, user_id, app_id, bot_user_id, chat_id, launch_mode, start_param, init_data_raw, auth_date, state_json, created_at, updated_at FROM mini_app_sessions WHERE query_id = ?1",
                params![query_id],
                row_to_mini_app_session,
            )
            .optional()
            .map_db()
        })
    }

    pub fn get_latest_mini_app_session_for_user(
        &self,
        user_id: i64,
        app_id: &str,
    ) -> Result<Option<MiniAppSessionRecord>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT query_id, user_id, app_id, bot_user_id, chat_id, launch_mode, start_param, init_data_raw, auth_date, state_json, created_at, updated_at FROM mini_app_sessions WHERE user_id = ?1 AND app_id = ?2 ORDER BY updated_at DESC LIMIT 1",
                params![user_id, app_id],
                row_to_mini_app_session,
            )
            .optional()
            .map_db()
        })
    }

    pub fn get_latest_mini_app_state_for_bot_user(
        &self,
        user_id: i64,
        bot_user_id: i64,
    ) -> Result<Option<MiniAppSessionRecord>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT query_id, user_id, app_id, bot_user_id, chat_id, launch_mode, start_param, init_data_raw, auth_date, state_json, created_at, updated_at FROM mini_app_sessions WHERE user_id = ?1 AND bot_user_id = ?2 ORDER BY updated_at DESC LIMIT 1",
                params![user_id, bot_user_id],
                row_to_mini_app_session,
            )
            .optional()
            .map_db()
        })
    }

    pub fn update_mini_app_session_state(&self, query_id: &str, state_json: &str) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE mini_app_sessions SET state_json = ?1, updated_at = ?2 WHERE query_id = ?3",
                params![state_json, now_ms, query_id],
            )
            .map_db()?;
            Ok(())
        })
    }

    // --- Telegram CloudStorage (Per-User Per-Bot Quota Enforced) ---

    pub fn list_cloud_storage(
        &self,
        user_id: i64,
        bot_user_id: i64,
    ) -> Result<BTreeMap<String, String>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT storage_key, storage_value FROM mini_app_cloud_storage WHERE user_id = ?1 AND bot_user_id = ?2 ORDER BY storage_key ASC",
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id, bot_user_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows.into_iter().collect())
        })
    }

    pub fn list_cloud_storage_items(
        &self,
        user_id: i64,
        bot_user_id: i64,
    ) -> Result<Vec<CloudStorageItem>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT storage_key, storage_value, updated_at FROM mini_app_cloud_storage WHERE user_id = ?1 AND bot_user_id = ?2 ORDER BY storage_key ASC",
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id, bot_user_id], |r| {
                    Ok(CloudStorageItem {
                        key: r.get(0)?,
                        value: r.get(1)?,
                        updated_at: r.get(2)?,
                    })
                })
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })
    }

    pub fn put_cloud_storage_entries(
        &self,
        user_id: i64,
        bot_user_id: i64,
        entries: &BTreeMap<String, String>,
    ) -> Result<Vec<CloudStorageItem>> {
        for (k, v) in entries {
            if k.is_empty() || k.len() > CLOUD_STORAGE_MAX_KEY_BYTES {
                return Err(TelegramError::Validation(format!(
                    "CloudStorage key '{}' length must be 1..={} bytes",
                    k, CLOUD_STORAGE_MAX_KEY_BYTES
                )));
            }
            if v.len() > CLOUD_STORAGE_MAX_VALUE_BYTES {
                return Err(TelegramError::Validation(format!(
                    "CloudStorage value for '{}' exceeds {} bytes",
                    k, CLOUD_STORAGE_MAX_VALUE_BYTES
                )));
            }
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let mut existing = BTreeMap::new();
            {
                let mut stmt = tx
                    .prepare(
                        "SELECT storage_key, storage_value FROM mini_app_cloud_storage WHERE user_id = ?1 AND bot_user_id = ?2",
                    )
                    .map_db()?;
                let mut rows = stmt.query(params![user_id, bot_user_id]).map_db()?;
                while let Some(r) = rows.next().map_db()? {
                    existing.insert(r.get::<_, String>(0).map_db()?, r.get::<_, String>(1).map_db()?);
                }
            }
            for (k, v) in entries {
                existing.insert(k.clone(), v.clone());
            }
            if existing.len() > CLOUD_STORAGE_MAX_KEYS {
                return Err(TelegramError::Validation(format!(
                    "CloudStorage key limit exceeded (max {} keys)",
                    CLOUD_STORAGE_MAX_KEYS
                )));
            }
            let total_bytes: usize = existing.iter().map(|(k, v)| k.len() + v.len()).sum();
            if total_bytes > CLOUD_STORAGE_MAX_TOTAL_BYTES {
                return Err(TelegramError::Validation(format!(
                    "CloudStorage quota exceeded (max {} bytes)",
                    CLOUD_STORAGE_MAX_TOTAL_BYTES
                )));
            }

            let mut updated_items = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                tx.execute(
                    r#"INSERT INTO mini_app_cloud_storage (user_id, bot_user_id, storage_key, storage_value, updated_at)
                       VALUES (?1, ?2, ?3, ?4, ?5)
                       ON CONFLICT(user_id, bot_user_id, storage_key) DO UPDATE SET
                           storage_value = excluded.storage_value,
                           updated_at = excluded.updated_at"#,
                    params![user_id, bot_user_id, k, v, now_ms],
                )
                .map_db()?;
                updated_items.push(CloudStorageItem {
                    key: k.clone(),
                    value: v.clone(),
                    updated_at: now_ms,
                });
            }
            Ok(updated_items)
        })
    }

    pub fn delete_cloud_storage_keys(
        &self,
        user_id: i64,
        bot_user_id: i64,
        keys: &[String],
    ) -> Result<()> {
        self.pool.with_transaction(|tx| {
            for k in keys {
                tx.execute(
                    "DELETE FROM mini_app_cloud_storage WHERE user_id = ?1 AND bot_user_id = ?2 AND storage_key = ?3",
                    params![user_id, bot_user_id, k],
                )
                .map_db()?;
            }
            Ok(())
        })
    }

    // --- Telegram Bot Permissions, Usage Events, MAU, Bot API Updates & Hosted Bundles ---

    pub fn get_user_bot_permission(
        &self,
        user_id: i64,
        bot_user_id: i64,
    ) -> Result<UserBotPermissionRecord> {
        self.pool.with_reader(|conn| {
            let rec = conn
                .query_row(
                    "SELECT user_id, bot_user_id, allows_write_to_pm, app_tos_accepted_at, has_started_chat, updated_at FROM user_bot_permissions WHERE user_id = ?1 AND bot_user_id = ?2",
                    params![user_id, bot_user_id],
                    |r| {
                        Ok(UserBotPermissionRecord {
                            user_id: r.get(0)?,
                            bot_user_id: r.get(1)?,
                            allows_write_to_pm: r.get::<_, i64>(2)? != 0,
                            app_tos_accepted_at: r.get(3)?,
                            has_started_chat: r.get::<_, i64>(4)? != 0,
                            updated_at: r.get(5)?,
                        })
                    },
                )
                .optional()
                .map_db()?;
            Ok(rec.unwrap_or(UserBotPermissionRecord {
                user_id,
                bot_user_id,
                allows_write_to_pm: false,
                app_tos_accepted_at: 0,
                has_started_chat: false,
                updated_at: 0,
            }))
        })
    }

    pub fn set_user_bot_write_access(
        &self,
        user_id: i64,
        bot_user_id: i64,
        allows_write_to_pm: bool,
    ) -> Result<UserBotPermissionRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            tx.execute(
                r#"INSERT INTO user_bot_permissions (user_id, bot_user_id, allows_write_to_pm, app_tos_accepted_at, has_started_chat, updated_at)
                   VALUES (?1, ?2, ?3, 0, 0, ?4)
                   ON CONFLICT(user_id, bot_user_id) DO UPDATE SET
                       allows_write_to_pm = excluded.allows_write_to_pm,
                       updated_at = excluded.updated_at"#,
                params![user_id, bot_user_id, allows_write_to_pm as i64, now_ms],
            )
            .map_db()?;
            tx.execute(
                "UPDATE user_mini_apps SET allow_write_access = ?1 WHERE user_id = ?2 AND bot_user_id = ?3",
                params![allows_write_to_pm as i64, user_id, bot_user_id],
            )
            .map_db()?;
            tx.query_row(
                "SELECT user_id, bot_user_id, allows_write_to_pm, app_tos_accepted_at, has_started_chat, updated_at FROM user_bot_permissions WHERE user_id = ?1 AND bot_user_id = ?2",
                params![user_id, bot_user_id],
                |r| {
                    Ok(UserBotPermissionRecord {
                        user_id: r.get(0)?,
                        bot_user_id: r.get(1)?,
                        allows_write_to_pm: r.get::<_, i64>(2)? != 0,
                        app_tos_accepted_at: r.get(3)?,
                        has_started_chat: r.get::<_, i64>(4)? != 0,
                        updated_at: r.get(5)?,
                    })
                },
            )
            .map_db()
        })
    }

    pub fn set_user_bot_tos_accepted(
        &self,
        user_id: i64,
        bot_user_id: i64,
    ) -> Result<UserBotPermissionRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO user_bot_permissions (user_id, bot_user_id, allows_write_to_pm, app_tos_accepted_at, has_started_chat, updated_at)
                   VALUES (?1, ?2, 0, ?3, 0, ?3)
                   ON CONFLICT(user_id, bot_user_id) DO UPDATE SET
                       app_tos_accepted_at = CASE WHEN user_bot_permissions.app_tos_accepted_at > 0 THEN user_bot_permissions.app_tos_accepted_at ELSE excluded.app_tos_accepted_at END,
                       updated_at = excluded.updated_at"#,
                params![user_id, bot_user_id, now_ms],
            )
            .map_db()?;
            conn.query_row(
                "SELECT user_id, bot_user_id, allows_write_to_pm, app_tos_accepted_at, has_started_chat, updated_at FROM user_bot_permissions WHERE user_id = ?1 AND bot_user_id = ?2",
                params![user_id, bot_user_id],
                |r| {
                    Ok(UserBotPermissionRecord {
                        user_id: r.get(0)?,
                        bot_user_id: r.get(1)?,
                        allows_write_to_pm: r.get::<_, i64>(2)? != 0,
                        app_tos_accepted_at: r.get(3)?,
                        has_started_chat: r.get::<_, i64>(4)? != 0,
                        updated_at: r.get(5)?,
                    })
                },
            )
            .map_db()
        })
    }

    pub fn mark_user_started_bot_chat(
        &self,
        user_id: i64,
        bot_user_id: i64,
    ) -> Result<UserBotPermissionRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO user_bot_permissions (user_id, bot_user_id, allows_write_to_pm, app_tos_accepted_at, has_started_chat, updated_at)
                   VALUES (?1, ?2, 0, 0, 1, ?3)
                   ON CONFLICT(user_id, bot_user_id) DO UPDATE SET
                       has_started_chat = 1,
                       updated_at = excluded.updated_at"#,
                params![user_id, bot_user_id, now_ms],
            )
            .map_db()?;
            conn.query_row(
                "SELECT user_id, bot_user_id, allows_write_to_pm, app_tos_accepted_at, has_started_chat, updated_at FROM user_bot_permissions WHERE user_id = ?1 AND bot_user_id = ?2",
                params![user_id, bot_user_id],
                |r| {
                    Ok(UserBotPermissionRecord {
                        user_id: r.get(0)?,
                        bot_user_id: r.get(1)?,
                        allows_write_to_pm: r.get::<_, i64>(2)? != 0,
                        app_tos_accepted_at: r.get(3)?,
                        has_started_chat: r.get::<_, i64>(4)? != 0,
                        updated_at: r.get(5)?,
                    })
                },
            )
            .map_db()
        })
    }

    pub fn record_mini_app_launch(
        &self,
        user_id: i64,
        app_id: &str,
        bot_user_id: i64,
    ) -> Result<i64> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO mini_app_usage_events (user_id, app_id, bot_user_id, last_launched_at, launch_count)
                   VALUES (?1, ?2, ?3, ?4, 1)
                   ON CONFLICT(user_id, app_id) DO UPDATE SET
                       last_launched_at = excluded.last_launched_at,
                       launch_count = mini_app_usage_events.launch_count + 1"#,
                params![user_id, app_id, bot_user_id, now_ms],
            )
            .map_db()?;
            Ok(now_ms)
        })
    }

    pub fn list_recent_mini_apps(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<(MiniAppRecord, i64)>> {
        let lim = limit.clamp(1, 100) as i64;
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT m.app_id, m.bot_user_id, m.bot_username, m.short_name, m.title, m.description, m.category, m.icon_emoji, m.web_url, m.ui_schema_json, m.peer_types_json, m.is_published, m.is_attach_enabled, m.install_count, m.created_by_nl, m.created_at, m.updated_at, m.header_color, m.background_color, e.last_launched_at
                     FROM mini_app_usage_events e
                     JOIN mini_apps m ON m.app_id = e.app_id
                     WHERE e.user_id = ?1 AND m.is_published = 1
                     ORDER BY e.last_launched_at DESC
                     LIMIT ?2",
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id, lim], |r| {
                    let app = row_to_mini_app(r)?;
                    let launched_at: i64 = r.get(19)?;
                    Ok((app, launched_at))
                })
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })
    }

    pub fn get_mini_app_usage_for_user(
        &self,
        user_id: i64,
        app_id: &str,
    ) -> Result<Option<i64>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT last_launched_at FROM mini_app_usage_events WHERE user_id = ?1 AND app_id = ?2",
                params![user_id, app_id],
                |r| r.get(0),
            )
            .optional()
            .map_db()
        })
    }

    pub fn compute_bot_mau_30d(&self, bot_user_id: i64) -> Result<i64> {
        let cutoff_ms = chrono::Utc::now().timestamp_millis() - 30 * 86_400_000;
        self.pool.with_reader(|conn| {
            conn.query_row(
                r#"SELECT COUNT(DISTINCT user_id) FROM (
                       SELECT user_id FROM mini_app_usage_events WHERE bot_user_id = ?1 AND last_launched_at >= ?2
                       UNION
                       SELECT user_id FROM user_bot_permissions WHERE bot_user_id = ?1 AND updated_at >= ?2 AND (has_started_chat = 1 OR allows_write_to_pm = 1)
                   )"#,
                params![bot_user_id, cutoff_ms],
                |r| r.get(0),
            )
            .map_db()
        })
    }

    pub fn compute_app_mau_30d(&self, app_id: &str) -> Result<i64> {
        let cutoff_ms = chrono::Utc::now().timestamp_millis() - 30 * 86_400_000;
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT COUNT(DISTINCT user_id) FROM mini_app_usage_events WHERE app_id = ?1 AND last_launched_at >= ?2",
                params![app_id, cutoff_ms],
                |r| r.get(0),
            )
            .map_db()
        })
    }

    pub fn upsert_mini_app_hosted_bundle(&self, app_id: &str, html_content: &str) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO mini_app_hosted_bundles (app_id, html_content, updated_at)
                   VALUES (?1, ?2, ?3)
                   ON CONFLICT(app_id) DO UPDATE SET
                       html_content = excluded.html_content,
                       updated_at = excluded.updated_at"#,
                params![app_id, html_content, now_ms],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_mini_app_hosted_bundle(&self, app_id: &str) -> Result<Option<String>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT html_content FROM mini_app_hosted_bundles WHERE app_id = ?1",
                params![app_id],
                |r| r.get(0),
            )
            .optional()
            .map_db()
        })
    }

    pub fn enqueue_bot_api_update(
        &self,
        bot_user_id: i64,
        payload_json: &str,
    ) -> Result<BotApiUpdateRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "INSERT INTO bot_api_updates (bot_user_id, payload_json, created_at_ms) VALUES (?1, ?2, ?3)",
                params![bot_user_id, payload_json, now_ms],
            )
            .map_db()?;
            let update_id = conn.last_insert_rowid();
            Ok(BotApiUpdateRecord {
                update_id,
                bot_user_id,
                payload_json: payload_json.to_string(),
                created_at_ms: now_ms,
            })
        })
    }

    pub fn get_bot_api_updates(
        &self,
        bot_user_id: i64,
        offset: Option<i64>,
        limit: usize,
    ) -> Result<Vec<BotApiUpdateRecord>> {
        let lim = limit.clamp(1, 100) as i64;
        self.pool.with_transaction(|tx| {
            if let Some(off) = offset {
                tx.execute(
                    "DELETE FROM bot_api_updates WHERE bot_user_id = ?1 AND update_id < ?2",
                    params![bot_user_id, off],
                )
                .map_db()?;
            }
            let min_id = offset.unwrap_or(0);
            let mut stmt = tx
                .prepare(
                    "SELECT update_id, bot_user_id, payload_json, created_at_ms
                     FROM bot_api_updates
                     WHERE bot_user_id = ?1 AND update_id >= ?2
                     ORDER BY update_id ASC
                     LIMIT ?3",
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![bot_user_id, min_id, lim], |r| {
                    Ok(BotApiUpdateRecord {
                        update_id: r.get(0)?,
                        bot_user_id: r.get(1)?,
                        payload_json: r.get(2)?,
                        created_at_ms: r.get(3)?,
                    })
                })
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })
    }

    pub fn count_bot_api_updates(&self, bot_user_id: i64) -> Result<i64> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM bot_api_updates WHERE bot_user_id = ?1",
                params![bot_user_id],
                |r| r.get(0),
            )
            .map_db()
        })
    }

    pub fn clear_bot_api_updates(&self, bot_user_id: i64) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "DELETE FROM bot_api_updates WHERE bot_user_id = ?1",
                params![bot_user_id],
            )
            .map_db()?;
            Ok(())
        })
    }
}
