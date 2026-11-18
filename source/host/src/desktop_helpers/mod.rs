//! UI-agnostic pure-Rust helpers ported from `crates/gpui_telegram_desktop`:
//! - Bilingual localization (`zh-CN` / `en-US`)
//! - User UI preferences (`theme_mode`, `font_size_pt`, `bubble_free_mode`, `liquid_glass_enabled`, `send_by_cmd_enter`)
//! - Semver release checker (`ReleaseUpdateChecker`)
//! - CJK Pinyin `markedText` composition safety (`MarkedTextStateMachine`)
//! - Hold-Slide-Lock voice/video orb state machine (`VoiceGestureStateMachine`)
//! - UTF-16 Markdown span parser (`parse_markdown_spans`)

use gpui_telegram_proto::message::{MessageEntity, MessageEntityType};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AppLanguage {
    #[default]
    #[serde(rename = "en-US", alias = "en")]
    EnUs,
    #[serde(rename = "zh-CN", alias = "zh", alias = "zh-Hans")]
    ZhCn,
}

impl AppLanguage {
    pub fn from_code(raw: &str) -> Self {
        let low = raw.trim().to_ascii_lowercase();
        if low.starts_with("zh") {
            Self::ZhCn
        } else {
            Self::EnUs
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::EnUs => "en-US",
            Self::ZhCn => "zh-CN",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::EnUs => Self::ZhCn,
            Self::ZhCn => Self::EnUs,
        }
    }
}

pub fn translate_key(lang: AppLanguage, key: &str) -> String {
    match (lang, key) {
        (AppLanguage::ZhCn, "tab.contacts") => "联系人".into(),
        (AppLanguage::EnUs, "tab.contacts") => "Contacts".into(),
        (AppLanguage::ZhCn, "tab.calls") => "通话".into(),
        (AppLanguage::EnUs, "tab.calls") => "Calls".into(),
        (AppLanguage::ZhCn, "tab.chats") => "对话".into(),
        (AppLanguage::EnUs, "tab.chats") => "Chats".into(),
        (AppLanguage::ZhCn, "tab.settings") => "设置".into(),
        (AppLanguage::EnUs, "tab.settings") => "Settings".into(),
        (AppLanguage::ZhCn, "search.placeholder") => "搜索消息或联系人".into(),
        (AppLanguage::EnUs, "search.placeholder") => "Search messages or peers".into(),
        (AppLanguage::ZhCn, "composer.placeholder") => "发送消息...".into(),
        (AppLanguage::EnUs, "composer.placeholder") => "Message...".into(),
        (AppLanguage::ZhCn, "auth.sign_in") => "登录 Fabushi".into(),
        (AppLanguage::EnUs, "auth.sign_in") => "Sign in to Fabushi".into(),
        (AppLanguage::ZhCn, "miniapp.global_dharma") => "全球法布施".into(),
        (AppLanguage::EnUs, "miniapp.global_dharma") => "Global Dharma Giving".into(),
        (AppLanguage::ZhCn, "hermes.studio") => "Hermes 智能体工坊".into(),
        (AppLanguage::EnUs, "hermes.studio") => "Hermes Agent Studio".into(),
        (_, other) => other.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppPreferences {
    pub theme_mode: String,
    pub accent_hex: String,
    pub font_size_pt: u8,
    pub bubble_free_mode: bool,
    pub liquid_glass_enabled: bool,
    pub send_by_cmd_enter: bool,
    pub language: String,
    pub auto_lock_minutes: u32,
    #[serde(default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            theme_mode: "day".to_string(),
            accent_hex: "#007AFF".to_string(),
            font_size_pt: 17,
            bubble_free_mode: false,
            liquid_glass_enabled: true,
            send_by_cmd_enter: false,
            language: "en-US".to_string(),
            auto_lock_minutes: 0,
            extra: BTreeMap::new(),
        }
    }
}

pub fn compare_semver(current: &str, candidate: &str) -> std::cmp::Ordering {
    fn parse_parts(v: &str) -> (u64, u64, u64) {
        let trimmed = v.trim().trim_start_matches('v').split('-').next().unwrap_or("0.0.0");
        let mut nums = trimmed.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
        (
            nums.next().unwrap_or(0),
            nums.next().unwrap_or(0),
            nums.next().unwrap_or(0),
        )
    }
    parse_parts(current).cmp(&parse_parts(candidate))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImeKeyDecision {
    CommitSend,
    InsertNewline,
    ConsumeMarkedComposition,
    PassThrough,
}

pub fn evaluate_composer_enter_key(
    has_marked_text: bool,
    shift_pressed: bool,
    cmd_pressed: bool,
    send_by_cmd_enter: bool,
) -> ImeKeyDecision {
    if has_marked_text {
        return ImeKeyDecision::ConsumeMarkedComposition;
    }
    if shift_pressed {
        return ImeKeyDecision::InsertNewline;
    }
    if send_by_cmd_enter {
        if cmd_pressed {
            ImeKeyDecision::CommitSend
        } else {
            ImeKeyDecision::InsertNewline
        }
    } else {
        ImeKeyDecision::CommitSend
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceGestureState {
    Idle,
    RecordingHold,
    LockedHandsFree,
    Cancelled,
}

pub const VOICE_CANCEL_DELTA_X_PT: f32 = -72.0;
pub const VOICE_LOCK_DELTA_Y_PT: f32 = -56.0;

pub fn evaluate_voice_gesture(current: VoiceGestureState, dx_pt: f32, dy_pt: f32, released: bool) -> VoiceGestureState {
    match current {
        VoiceGestureState::Idle => VoiceGestureState::RecordingHold,
        VoiceGestureState::LockedHandsFree => VoiceGestureState::LockedHandsFree,
        VoiceGestureState::Cancelled => VoiceGestureState::Cancelled,
        VoiceGestureState::RecordingHold => {
            if dx_pt <= VOICE_CANCEL_DELTA_X_PT {
                VoiceGestureState::Cancelled
            } else if dy_pt <= VOICE_LOCK_DELTA_Y_PT {
                VoiceGestureState::LockedHandsFree
            } else if released {
                VoiceGestureState::Idle
            } else {
                VoiceGestureState::RecordingHold
            }
        }
    }
}

/// Parses inline Markdown markers (`**bold**`, `*italic*`, `` `code` ``, ```` ```pre``` ````,
/// `||spoiler||`, `~~strikethrough~~`, `[label](url)`) into plain text + UTF-16 `MessageEntity` offsets.
pub fn parse_markdown_spans(raw: &str) -> (String, Vec<MessageEntity>) {
    let mut plain = String::with_capacity(raw.len());
    let mut utf16_cursor: u32 = 0;
    let mut entities = Vec::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;

    let find_closing = |chars: &[char], start: usize, delim: &[char]| -> Option<usize> {
        let dlen = delim.len();
        if start + dlen > chars.len() {
            return None;
        }
        let mut idx = start;
        while idx + dlen <= chars.len() {
            if &chars[idx..idx + dlen] == delim {
                return Some(idx);
            }
            idx += 1;
        }
        None
    };

    while i < chars.len() {
        // ```lang\ncode```
        if i + 3 <= chars.len() && &chars[i..i + 3] == &['`', '`', '`'] {
            if let Some(end) = find_closing(&chars, i + 3, &['`', '`', '`']) {
                let inner: String = chars[i + 3..end].iter().collect();
                let (language, body) = match inner.split_once('\n') {
                    Some((lang, rest)) if !lang.contains(' ') => (
                         (!lang.trim().is_empty()).then(|| lang.trim().to_string()),
                        rest.to_string(),
                    ),
                    _ => (None, inner),
                };
                let u16_len = body.encode_utf16().count() as u32;
                if u16_len > 0 {
                    entities.push(MessageEntity::new(
                        utf16_cursor,
                        u16_len,
                        MessageEntityType::Pre { language },
                    ));
                    plain.push_str(&body);
                    utf16_cursor += u16_len;
                }
                i = end + 3;
                continue;
            }
        }

        // **bold**
        if i + 2 <= chars.len() && &chars[i..i + 2] == &['*', '*'] {
            if let Some(end) = find_closing(&chars, i + 2, &['*', '*']) {
                let body: String = chars[i + 2..end].iter().collect();
                let u16_len = body.encode_utf16().count() as u32;
                if u16_len > 0 {
                    entities.push(MessageEntity::new(
                        utf16_cursor,
                        u16_len,
                        MessageEntityType::Bold,
                    ));
                    plain.push_str(&body);
                    utf16_cursor += u16_len;
                    i = end + 2;
                    continue;
                }
            }
        }

        // ||spoiler||
        if i + 2 <= chars.len() && &chars[i..i + 2] == &['|', '|'] {
            if let Some(end) = find_closing(&chars, i + 2, &['|', '|']) {
                let body: String = chars[i + 2..end].iter().collect();
                let u16_len = body.encode_utf16().count() as u32;
                if u16_len > 0 {
                    entities.push(MessageEntity::new(
                        utf16_cursor,
                        u16_len,
                        MessageEntityType::Spoiler,
                    ));
                    plain.push_str(&body);
                    utf16_cursor += u16_len;
                    i = end + 2;
                    continue;
                }
            }
        }

        // ~~strikethrough~~
        if i + 2 <= chars.len() && &chars[i..i + 2] == &['~', '~'] {
            if let Some(end) = find_closing(&chars, i + 2, &['~', '~']) {
                let body: String = chars[i + 2..end].iter().collect();
                let u16_len = body.encode_utf16().count() as u32;
                if u16_len > 0 {
                    entities.push(MessageEntity::new(
                        utf16_cursor,
                        u16_len,
                        MessageEntityType::Strikethrough,
                    ));
                    plain.push_str(&body);
                    utf16_cursor += u16_len;
                    i = end + 2;
                    continue;
                }
            }
        }

        // `code`
        if chars[i] == '`' {
            if let Some(end) = find_closing(&chars, i + 1, &['`']) {
                let body: String = chars[i + 1..end].iter().collect();
                let u16_len = body.encode_utf16().count() as u32;
                if u16_len > 0 {
                    entities.push(MessageEntity::new(
                        utf16_cursor,
                        u16_len,
                        MessageEntityType::Code,
                    ));
                    plain.push_str(&body);
                    utf16_cursor += u16_len;
                    i = end + 1;
                    continue;
                }
            }
        }

        // *italic*
        if chars[i] == '*' {
            if let Some(end) = find_closing(&chars, i + 1, &['*']) {
                let body: String = chars[i + 1..end].iter().collect();
                let u16_len = body.encode_utf16().count() as u32;
                if u16_len > 0 && !body.contains('\n') {
                    entities.push(MessageEntity::new(
                        utf16_cursor,
                        u16_len,
                        MessageEntityType::Italic,
                    ));
                    plain.push_str(&body);
                    utf16_cursor += u16_len;
                    i = end + 1;
                    continue;
                }
            }
        }

        // [label](url)
        if chars[i] == '[' {
            if let Some(close_br) = find_closing(&chars, i + 1, &[']']) {
                if close_br + 1 < chars.len() && chars[close_br + 1] == '(' {
                    if let Some(close_paren) = find_closing(&chars, close_br + 2, &[')']) {
                        let label: String = chars[i + 1..close_br].iter().collect();
                        let url: String = chars[close_br + 2..close_paren].iter().collect();
                        let u16_len = label.encode_utf16().count() as u32;
                        if u16_len > 0 && !url.trim().is_empty() {
                            entities.push(MessageEntity::new(
                                utf16_cursor,
                                u16_len,
                                MessageEntityType::TextUrl { url },
                            ));
                            plain.push_str(&label);
                            utf16_cursor += u16_len;
                            i = close_paren + 1;
                            continue;
                        }
                    }
                }
            }
        }

        let ch = chars[i];
        plain.push(ch);
        utf16_cursor += ch.len_utf16() as u32;
        i += 1;
    }

    (plain, entities)
}
