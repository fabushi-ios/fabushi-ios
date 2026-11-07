//! Telegram History Item Reply Markup & Interactive Keyboards
//!
//! Citations:
//! - `Telegram/SourceFiles/history/history_item_reply_markup.h:L32-L48` (ReplyMarkupFlags)
//! - `Telegram/SourceFiles/history/history_item_reply_markup.h:L80-L107` (HistoryMessageMarkupButton::Type)
//! - `Telegram/SourceFiles/history/history_item_reply_markup.h:L109-L133` (Visual, TypeIcon, Color)
//! - `Telegram/SourceFiles/history/history_item_reply_markup.cpp:L27-L50` (kButtonTypeIcons mapping)
//! - `Telegram/SourceFiles/history/history_item_reply_markup.cpp:L346-L350` (LoadsOnActivate)
//! - `Telegram/SourceFiles/history/history_item_reply_markup.cpp:L359-L368` (RichPageButtonKey format)
//! - `Telegram/SourceFiles/history/history_item_reply_markup.cpp:L438-L442` (OnlyBuyButton layout rule)
//! - `Telegram/SourceFiles/history/history_item_reply_markup.cpp:L476-L508` (fillForwardedData rewriting SwitchInlineSame)

use serde::{Deserialize, Serialize};

/// Upstream `history_item_reply_markup.h:L32-L48`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReplyMarkupFlags(pub u32);

impl ReplyMarkupFlags {
    pub const NONE: u32 = 1 << 0;
    pub const FORCE_REPLY: u32 = 1 << 1;
    pub const HAS_SWITCH_INLINE_BUTTON: u32 = 1 << 2;
    pub const INLINE: u32 = 1 << 3;
    pub const RESIZE: u32 = 1 << 4;
    pub const SINGLE_USE: u32 = 1 << 5;
    pub const SELECTIVE: u32 = 1 << 6;
    pub const IS_NULL: u32 = 1 << 7;
    pub const ONLY_BUY_BUTTON: u32 = 1 << 8;
    pub const PERSISTENT: u32 = 1 << 9;
    pub const SUGGESTION_DECLINE: u32 = 1 << 10;
    pub const SUGGESTION_ACCEPT: u32 = 1 << 11;
    pub const SUGGESTION_SEPARATOR: u32 = 1 << 12;

    pub fn new(bits: u32) -> Self {
        Self(bits)
    }

    pub fn contains(&self, flag: u32) -> bool {
        (self.0 & flag) == flag
    }

    pub fn insert(&mut self, flag: u32) {
        self.0 |= flag;
    }

    pub fn remove(&mut self, flag: u32) {
        self.0 &= !flag;
    }
}

/// Upstream `history_item_reply_markup.h:L80-L107`: 22 Button Types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HistoryMessageMarkupButtonType {
    #[default]
    Default,
    Url,
    Callback,
    CallbackWithPassword,
    RequestPhone,
    RequestLocation,
    RequestPoll,
    RequestPeer,
    SwitchInline,
    SwitchInlineSame,
    Game,
    Buy,
    Auth,
    UserProfile,
    WebView,
    SimpleWebView,
    CopyText,
    Disabled,
    SuggestDecline,
    SuggestAccept,
    SuggestChange,
    CreateBot,
}

/// Upstream `history_item_reply_markup.h:L109-L116`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HistoryMessageMarkupButtonIcon {
    #[default]
    None,
    Url,
    Payment,
    SwitchPm,
    Webview,
    Copy,
}

/// Upstream `history_item_reply_markup.h:L118-L123`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HistoryMessageMarkupButtonColor {
    #[default]
    Normal,
    Primary,
    Danger,
    Success,
}

/// Upstream `history_item_reply_markup.h:L125-L133`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HistoryMessageMarkupButtonVisual {
    pub icon_id: u64,
    pub color: HistoryMessageMarkupButtonColor,
}

/// Upstream `history_item_reply_markup.h:L80-L168`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryMessageMarkupButton {
    pub button_type: HistoryMessageMarkupButtonType,
    pub visual: HistoryMessageMarkupButtonVisual,
    pub text: String,
    pub forward_text: Option<String>,
    pub data: Vec<u8>,
    pub button_id: i64,
    pub peer_types: u32,
}

impl HistoryMessageMarkupButton {
    pub fn new(
        button_type: HistoryMessageMarkupButtonType,
        text: impl Into<String>,
        visual: HistoryMessageMarkupButtonVisual,
    ) -> Self {
        Self {
            button_type,
            visual,
            text: text.into(),
            forward_text: None,
            data: Vec::new(),
            button_id: 0,
            peer_types: 0,
        }
    }

    /// Upstream `history_item_reply_markup.cpp:L27-L50`: `kButtonTypeIcons`
    pub fn icon(&self) -> HistoryMessageMarkupButtonIcon {
        match self.button_type {
            HistoryMessageMarkupButtonType::Url | HistoryMessageMarkupButtonType::Auth => {
                HistoryMessageMarkupButtonIcon::Url
            }
            HistoryMessageMarkupButtonType::Buy => HistoryMessageMarkupButtonIcon::Payment,
            HistoryMessageMarkupButtonType::SwitchInline
            | HistoryMessageMarkupButtonType::SwitchInlineSame => {
                HistoryMessageMarkupButtonIcon::SwitchPm
            }
            HistoryMessageMarkupButtonType::WebView
            | HistoryMessageMarkupButtonType::SimpleWebView => {
                HistoryMessageMarkupButtonIcon::Webview
            }
            HistoryMessageMarkupButtonType::CopyText => HistoryMessageMarkupButtonIcon::Copy,
            _ => HistoryMessageMarkupButtonIcon::None,
        }
    }

    /// Upstream `history_item_reply_markup.cpp:L346-L350`: `LoadsOnActivate`
    pub fn loads_on_activate(&self) -> bool {
        matches!(
            self.button_type,
            HistoryMessageMarkupButtonType::Callback
                | HistoryMessageMarkupButtonType::CallbackWithPassword
                | HistoryMessageMarkupButtonType::Game
        )
    }

    /// Upstream `history_item_reply_markup.cpp:L359-L368`: `RichPageButtonKey` format `type;buttonId;peerTypes;data`
    pub fn rich_page_button_key(&self) -> String {
        format!(
            "{:?};{};{};{}",
            self.button_type,
            self.button_id,
            self.peer_types,
            String::from_utf8_lossy(&self.data)
        )
    }
}

/// Upstream `history_item_reply_markup.h:L181-L199`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryMessageMarkupData {
    pub rows: Vec<Vec<HistoryMessageMarkupButton>>,
    pub flags: ReplyMarkupFlags,
    pub placeholder: Option<String>,
}

impl HistoryMessageMarkupData {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            flags: ReplyMarkupFlags(ReplyMarkupFlags::IS_NULL),
            placeholder: None,
        }
    }

    pub fn is_null(&self) -> bool {
        self.flags.contains(ReplyMarkupFlags::IS_NULL)
    }

    /// Upstream `history_item_reply_markup.cpp:L438-L442`: Layout optimization setting `OnlyBuyButton`
    pub fn update_layout_flags(&mut self) {
        if self.rows.len() == 1 && self.rows[0].len() == 1 {
            if self.rows[0][0].button_type == HistoryMessageMarkupButtonType::Buy {
                self.flags.insert(ReplyMarkupFlags::ONLY_BUY_BUTTON);
                return;
            }
        }
        self.flags.remove(ReplyMarkupFlags::ONLY_BUY_BUTTON);
    }

    /// Upstream `history_item_reply_markup.cpp:L476-L508`: `fillForwardedData`
    /// Rewrites `SwitchInlineSame` -> `SwitchInline` and uses `forward_text` if non-empty
    pub fn to_forwarded_data(&self) -> Self {
        let mut forwarded_rows = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let mut forwarded_row = Vec::with_capacity(row.len());
            for button in row {
                let new_type = if button.button_type == HistoryMessageMarkupButtonType::SwitchInlineSame {
                    HistoryMessageMarkupButtonType::SwitchInline
                } else {
                    button.button_type
                };
                let text = button
                    .forward_text
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(|| button.text.clone());

                let mut b = HistoryMessageMarkupButton::new(new_type, text, button.visual);
                b.data = button.data.clone();
                b.button_id = button.button_id;
                b.peer_types = button.peer_types;
                forwarded_row.push(b);
            }
            if !forwarded_row.empty() {
                forwarded_rows.push(forwarded_row);
            }
        }

        let mut data = Self {
            rows: forwarded_rows,
            flags: self.flags,
            placeholder: self.placeholder.clone(),
        };
        data.update_layout_flags();
        data
    }
}

/// Helper extension trait for empty check
trait SliceEmpty {
    fn empty(&self) -> bool;
}
impl<T> SliceEmpty for Vec<T> {
    fn empty(&self) -> bool {
        self.is_empty()
    }
}
