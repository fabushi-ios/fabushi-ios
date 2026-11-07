//! Target Symbol: ExportDataTypesData
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/data/export_data_types.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum EntityKind {
    #[default]
    Plain,
    Bold,
    Italic,
    Underline,
    Strike,
    Spoiler,
    Code,
    Pre,
    Blockquote,
    Mention,
    Hashtag,
    BotCommand,
    Url,
    Email,
    Phone,
    CustomEmoji,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextPart {
    pub kind: EntityKind,
    pub text: String,
    pub data: Option<String>, // URL or custom emoji id
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RichText {
    pub parts: Vec<TextPart>,
}

impl RichText {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            parts: vec![TextPart {
                kind: EntityKind::Plain,
                text: text.into(),
                data: None,
            }],
        }
    }

    pub fn to_plain_string(&self) -> String {
        self.parts.iter().map(|p| p.text.as_str()).collect()
    }

    pub fn to_html(&self) -> String {
        let mut out = String::new();
        for p in &self.parts {
            let escaped = html_escape(&p.text);
            match p.kind {
                EntityKind::Plain => out.push_str(&escaped),
                EntityKind::Bold => {
                    out.push_str("<b>");
                    out.push_str(&escaped);
                    out.push_str("</b>");
                }
                EntityKind::Italic => {
                    out.push_str("<i>");
                    out.push_str(&escaped);
                    out.push_str("</i>");
                }
                EntityKind::Underline => {
                    out.push_str("<u>");
                    out.push_str(&escaped);
                    out.push_str("</u>");
                }
                EntityKind::Strike => {
                    out.push_str("<s>");
                    out.push_str(&escaped);
                    out.push_str("</s>");
                }
                EntityKind::Spoiler => {
                    out.push_str("<span class=\"spoiler\">");
                    out.push_str(&escaped);
                    out.push_str("</span>");
                }
                EntityKind::Code => {
                    out.push_str("<code>");
                    out.push_str(&escaped);
                    out.push_str("</code>");
                }
                EntityKind::Pre => {
                    out.push_str("<pre>");
                    out.push_str(&escaped);
                    out.push_str("</pre>");
                }
                EntityKind::Blockquote => {
                    out.push_str("<blockquote>");
                    out.push_str(&escaped);
                    out.push_str("</blockquote>");
                }
                EntityKind::Url => {
                    let href = p.data.as_deref().unwrap_or(&p.text);
                    out.push_str(&format!("<a href=\"{}\">{}</a>", html_escape(href), escaped));
                }
                _ => out.push_str(&escaped),
            }
        }
        out
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum InlineButtonActionType {
    #[default]
    Url,
    Callback,
    SwitchInline,
    Game,
    Buy,
    WebView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportInlineButton {
    pub text: String,
    pub action_type: InlineButtonActionType,
    pub data: String,
}

/// Rich structured data types matching upstream `Export::Data::*`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExportDataTypesData {
    pub version: u32,
    pub supported_entities: Vec<EntityKind>,
}

impl ExportDataTypesData {
    pub fn new() -> Self {
        Self {
            version: 1,
            supported_entities: vec![
                EntityKind::Plain,
                EntityKind::Bold,
                EntityKind::Italic,
                EntityKind::Underline,
                EntityKind::Strike,
                EntityKind::Spoiler,
                EntityKind::Code,
                EntityKind::Pre,
                EntityKind::Blockquote,
                EntityKind::Url,
                EntityKind::CustomEmoji,
            ],
        }
    }

    pub fn peer_color_index(peer_id: i64) -> usize {
        let abs = if peer_id < 0 { (-peer_id) as u64 } else { peer_id as u64 };
        (abs % 7) as usize
    }

    pub fn contact_color_index(phone: &str) -> usize {
        let sum: usize = phone.bytes().map(|b| b as usize).sum();
        sum % 7
    }
}
