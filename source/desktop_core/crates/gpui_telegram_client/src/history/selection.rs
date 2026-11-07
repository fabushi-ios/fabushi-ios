//! Telegram History Message Text Selection & MIME Formatting
//!
//! Citations:
//! - `Telegram/SourceFiles/history/history_message_selection.h:L15-L19` (MessageSelectionKind)
//! - `Telegram/SourceFiles/history/history_message_selection.h:L21-L31` (MessageSelectionFlatEndpoint offset calculation)
//! - `Telegram/SourceFiles/history/history_message_selection.h:L93-L198` (MessageSelection normalization & contains)
//! - `Telegram/SourceFiles/history/history_item_text.cpp:L32` (kSelectedCopyReplyPreviewLimit = 64)

use serde::{Deserialize, Serialize};

/// Upstream `history_item_text.cpp:L32`
pub const K_SELECTED_COPY_REPLY_PREVIEW_LIMIT: usize = 64;

/// Upstream `history_message_selection.h:L15-L19`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MessageSelectionKind {
    #[default]
    None,
    Flat,
    RichPage,
}

/// Upstream TextSelection range
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TextSelection {
    pub from: usize,
    pub to: usize,
}

impl TextSelection {
    pub fn new(from: usize, to: usize) -> Self {
        Self { from, to }
    }

    pub fn is_empty(&self) -> bool {
        self.from >= self.to
    }

    pub fn len(&self) -> usize {
        self.to.saturating_sub(self.from)
    }
}

/// Upstream `history_message_selection.h:L21-L31`: `MessageSelectionFlatEndpoint`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MessageSelectionFlatEndpoint {
    pub symbol: u16,
    pub after_symbol: bool,
}

impl MessageSelectionFlatEndpoint {
    pub fn new(symbol: u16, after_symbol: bool) -> Self {
        Self { symbol, after_symbol }
    }

    /// Upstream `history_message_selection.h:L25-L27`: `offset()`
    pub fn offset(&self) -> u16 {
        self.symbol.saturating_add(if self.after_symbol { 1 } else { 0 })
    }
}

/// Upstream `history_message_selection.h:L93-L198`: `MessageSelection`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MessageSelection {
    pub kind: MessageSelectionKind,
    pub flat: TextSelection,
    pub anchor: MessageSelectionFlatEndpoint,
    pub focus: MessageSelectionFlatEndpoint,
}

impl MessageSelection {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Upstream `history_message_selection.h:L115-L129`: Normalizes anchor/focus into ordered TextSelection
    pub fn from_endpoints(
        anchor: MessageSelectionFlatEndpoint,
        focus: MessageSelectionFlatEndpoint,
    ) -> Self {
        let anchor_offset = anchor.offset();
        let focus_offset = focus.offset();
        if anchor_offset == focus_offset {
            return Self::empty();
        }

        let (from, to) = if anchor_offset < focus_offset {
            (anchor_offset, focus_offset)
        } else {
            (focus_offset, anchor_offset)
        };

        Self {
            kind: MessageSelectionKind::Flat,
            flat: TextSelection::new(from as usize, to as usize),
            anchor,
            focus,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.kind == MessageSelectionKind::None || self.flat.is_empty()
    }

    pub fn is_flat(&self) -> bool {
        self.kind == MessageSelectionKind::Flat && !self.flat.is_empty()
    }

    /// Upstream `history_message_selection.h:L191-L197`: `contains` endpoint check
    pub fn contains(&self, endpoint: &MessageSelectionFlatEndpoint) -> bool {
        if !self.is_flat() {
            return false;
        }
        (endpoint.symbol as usize >= self.flat.from) && ((endpoint.symbol as usize) < self.flat.to)
    }

    /// Extracts the selected slice from `full_text` clamped safely within bounds
    pub fn extract_slice<'a>(&self, full_text: &'a str) -> &'a str {
        if self.is_empty() {
            return "";
        }
        let from = self.flat.from.min(full_text.len());
        let to = self.flat.to.min(full_text.len());
        if from >= to {
            return "";
        }
        &full_text[from..to]
    }
}
