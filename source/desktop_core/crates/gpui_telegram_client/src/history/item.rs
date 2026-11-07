//! Telegram History Item Model & State Transitions
//!
//! Citations:
//! - `Telegram/SourceFiles/history/history_item.h:L89-L107` (HistoryItemCommonFields)
//! - `Telegram/SourceFiles/history/history_item.cpp:L97-L100` (kNotificationTextLimit, kPinnedMessageTextLimit, kMinLoginCode, kTonConnectRequestMaxDelay)
//! - `Telegram/SourceFiles/history/history_item.cpp:L3367-L3372` (isTooOldForEdit)
//! - `Telegram/SourceFiles/history/history_item.cpp:L3374-L3386` (allowsEdit)
//! - `Telegram/SourceFiles/history/history_item.cpp:L3490-L3524` (canDeleteForEveryone)
//! - `Telegram/SourceFiles/history/history_item_helpers.cpp:L949-L991` (FlagsFromMTP)

use serde::{Deserialize, Serialize};

/// Upstream constants from `history_item.cpp:L97-L100`
pub const K_NOTIFICATION_TEXT_LIMIT: usize = 255;
pub const K_PINNED_MESSAGE_TEXT_LIMIT: usize = 16;
pub const K_MIN_LOGIN_CODE: usize = 5;
pub const K_TON_CONNECT_REQUEST_MAX_DELAY_MS: i64 = 24 * 3600 * 1000; // 24 hours
pub const DEFAULT_EDIT_TIME_LIMIT_SECS: i64 = 48 * 3600; // 48 hours
pub const DEFAULT_REVOKE_TIME_LIMIT_SECS: i64 = 48 * 3600; // 48 hours

/// Message Flags bitmask matching upstream `history_item_helpers.cpp:L949-L991`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MessageFlags(pub u64);

impl MessageFlags {
    pub const OUTGOING: u64 = 1 << 0;
    pub const MENTIONS_ME: u64 = 1 << 1;
    pub const MEDIA_IS_UNREAD: u64 = 1 << 2;
    pub const SILENT: u64 = 1 << 3;
    pub const POST: u64 = 1 << 4;
    pub const LEGACY: u64 = 1 << 5;
    pub const HIDE_EDITED: u64 = 1 << 6;
    pub const PINNED: u64 = 1 << 7;
    pub const HAS_FROM_ID: u64 = 1 << 8;
    pub const HAS_REPLY_INFO: u64 = 1 << 9;
    pub const HAS_REPLY_MARKUP: u64 = 1 << 10;
    pub const SHORTCUT_MESSAGE: u64 = 1 << 11;
    pub const IS_OR_WAS_SCHEDULED: u64 = 1 << 12;
    pub const HAS_VIEWS: u64 = 1 << 13;
    pub const NO_FORWARDS: u64 = 1 << 14;
    pub const INVERT_MEDIA: u64 = 1 << 15;
    pub const ESTIMATED_DATE: u64 = 1 << 16;
    pub const TON_PAID_SUGGESTED: u64 = 1 << 17;
    pub const STARS_PAID_SUGGESTED: u64 = 1 << 18;
    pub const CAN_BE_SUMMARIZED: u64 = 1 << 19;
    pub const GUEST_CHAT_VIA_FROM: u64 = 1 << 20;
    pub const HISTORY_ENTRY: u64 = 1 << 21;
    pub const BEING_SENT: u64 = 1 << 22;
    pub const SENDING_FAILED: u64 = 1 << 23;
    pub const LOCAL: u64 = 1 << 24;
    pub const EPHEMERAL: u64 = 1 << 25;
    pub const FAKE_HISTORY_ITEM: u64 = 1 << 26;

    pub fn new(bits: u64) -> Self {
        Self(bits)
    }

    pub fn contains(&self, flag: u64) -> bool {
        (self.0 & flag) == flag
    }

    pub fn insert(&mut self, flag: u64) {
        self.0 |= flag;
    }

    pub fn remove(&mut self, flag: u64) {
        self.0 &= !flag;
    }

    pub fn toggle(&mut self, flag: u64, enabled: bool) {
        if enabled {
            self.insert(flag);
        } else {
            self.remove(flag);
        }
    }
}

/// Upstream `history_item.h:L115-L119`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PaidPostType {
    #[default]
    None,
    Stars,
    Ton,
}

/// Upstream `history_item.h:L109-L113`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HistoryReactionSource {
    #[default]
    Selector,
    Quick,
    Existing,
}

/// Upstream `history_item.h:L89-L107`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryItemCommonFields {
    pub id: i64,
    pub flags: MessageFlags,
    pub from_peer_id: i64,
    pub reply_to_msg_id: Option<i64>,
    pub reply_to_top_id: Option<i64>,
    pub date: i64,
    pub schedule_repeat_period: i64,
    pub shortcut_id: i64,
    pub stars_paid: i32,
    pub via_bot_id: Option<i64>,
    pub post_author: String,
    pub grouped_id: u64,
    pub effect_id: u64,
    pub ignore_forward_from: bool,
    pub ignore_forward_captions: bool,
    pub media_spoiler: bool,
}

/// Upstream `history_item.h:L121-L845`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: i64,
    pub peer_id: i64,
    pub from_id: i64,
    pub date: i64,
    pub text: String,
    pub flags: MessageFlags,
    pub ttl_destroy_at: i64,
    pub boosts_applied: i32,
    pub stars_paid: i32,
    pub shortcut_id: i64,
    pub grouped_id: u64,
    pub effect_id: u64,
    pub views_count: Option<i32>,
    pub forwards_count: Option<i32>,
    pub replies_count: Option<i32>,
    pub edit_date: Option<i64>,
    pub post_author: Option<String>,
    pub paid_type: PaidPostType,
    pub is_service: bool,
    pub is_scheduled: bool,
    pub is_welcome_template: bool,
    pub is_editing_media: bool,
    pub has_via_bot: bool,
    pub is_forwarded: bool,
}

impl HistoryItem {
    pub fn new(id: i64, peer_id: i64, from_id: i64, date: i64, text: impl Into<String>) -> Self {
        Self {
            id,
            peer_id,
            from_id,
            date,
            text: text.into(),
            flags: MessageFlags::default(),
            ttl_destroy_at: 0,
            boosts_applied: 0,
            stars_paid: 0,
            shortcut_id: 0,
            grouped_id: 0,
            effect_id: 0,
            views_count: None,
            forwards_count: None,
            replies_count: None,
            edit_date: None,
            post_author: None,
            paid_type: PaidPostType::None,
            is_service: false,
            is_scheduled: false,
            is_welcome_template: false,
            is_editing_media: false,
            has_via_bot: false,
            is_forwarded: false,
        }
    }

    pub fn out(&self) -> bool {
        self.flags.contains(MessageFlags::OUTGOING)
    }

    pub fn is_pinned(&self) -> bool {
        self.flags.contains(MessageFlags::PINNED)
    }

    pub fn is_silent(&self) -> bool {
        self.flags.contains(MessageFlags::SILENT)
    }

    pub fn is_post(&self) -> bool {
        self.flags.contains(MessageFlags::POST)
    }

    pub fn invert_media(&self) -> bool {
        self.flags.contains(MessageFlags::INVERT_MEDIA)
    }

    pub fn has_views(&self) -> bool {
        self.flags.contains(MessageFlags::HAS_VIEWS)
    }

    pub fn hide_edited_badge(&self) -> bool {
        self.flags.contains(MessageFlags::HIDE_EDITED)
    }

    pub fn is_legacy_message(&self) -> bool {
        self.flags.contains(MessageFlags::LEGACY)
    }

    pub fn is_regular(&self) -> bool {
        !self.is_service && !self.is_scheduled && !self.is_welcome_template && self.shortcut_id == 0
    }

    /// Upstream `history_item.cpp:L3367-L3372`
    pub fn is_too_old_for_edit(&self, now: i64, edit_time_limit: i64, can_edit_indefinitely: bool) -> bool {
        if can_edit_indefinitely || self.is_scheduled || self.is_welcome_template {
            return false;
        }
        (now - self.date) >= edit_time_limit
    }

    /// Upstream `history_item.cpp:L3374-L3386`
    pub fn allows_edit(&self, now: i64, edit_time_limit: i64, can_edit_indefinitely: bool) -> bool {
        if self.is_service {
            return false;
        }
        if self.has_via_bot || self.is_forwarded {
            return false;
        }
        if self.is_too_old_for_edit(now, edit_time_limit, can_edit_indefinitely) {
            return false;
        }
        if self.is_legacy_message() || self.is_editing_media {
            return false;
        }
        if self.paid_type != PaidPostType::None {
            return false;
        }
        true
    }

    /// Upstream `history_item.cpp:L3490-L3524`
    pub fn can_delete_for_everyone(
        &self,
        now: i64,
        is_self: bool,
        is_user: bool,
        is_channel: bool,
        is_bot: bool,
        revoke_time_limit: i64,
        revoke_private_time_limit: i64,
        revoke_private_inbox: bool,
    ) -> bool {
        if is_self {
            return false; // Message to myself is deleted locally without "for everyone" dialog
        }
        if is_channel {
            return false; // Channels delete directly without individual revoke time limit
        }
        if self.is_post() || !self.is_regular() {
            return false;
        }

        let time_limit = if is_user {
            revoke_private_time_limit
        } else {
            revoke_time_limit
        };
        let is_too_old = (now - self.date) >= time_limit;
        if is_too_old {
            return false;
        }

        if is_user && is_bot {
            return false; // Bots cannot be revoked
        }

        if !self.out() {
            if is_user {
                return revoke_private_inbox;
            }
        }

        true
    }

    /// Upstream `history_item.cpp:L97-L98`: Truncates notification text to 255 chars
    pub fn notification_text(&self) -> String {
        if self.text.len() <= K_NOTIFICATION_TEXT_LIMIT {
            self.text.clone()
        } else {
            let mut s = self.text[..K_NOTIFICATION_TEXT_LIMIT].to_string();
            s.push('…');
            s
        }
    }

    /// Upstream TTL expiration check
    pub fn is_ttl_expired(&self, now: i64) -> bool {
        self.ttl_destroy_at > 0 && now >= self.ttl_destroy_at
    }
}

/// Upstream `history_item_edition.h:L26-L56`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryMessageEdition {
    pub is_edit_hide: bool,
    pub is_media_unread: bool,
    pub repeat_period: i64,
    pub edit_date: i64,
    pub views: Option<i32>,
    pub forwards: Option<i32>,
    pub ttl: i64,
    pub use_same_views: bool,
    pub use_same_forwards: bool,
    pub use_same_replies: bool,
    pub use_same_markup: bool,
    pub use_same_reactions: bool,
    pub use_same_suggest: bool,
    pub save_previous_media: bool,
    pub invert_media: bool,
    pub text: String,
    pub from_rank: Option<String>,
}

/// Upstream `history_item_edition.h:L58-L65`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryMessageContent {
    pub text: String,
    pub invert_media: bool,
    pub hide_edited: bool,
}

/// Upstream `history_item_helpers.cpp:L949-L991`: `FlagsFromMTP` mapping
pub fn flags_from_mtp(id: i64, mtp_flags: u64, local_flags: u64) -> MessageFlags {
    let mut f = local_flags;
    if id > 0 {
        f |= MessageFlags::HISTORY_ENTRY;
    }
    // MTP message flags mapping
    if (mtp_flags & (1 << 1)) != 0 { f |= MessageFlags::OUTGOING; }
    if (mtp_flags & (1 << 4)) != 0 { f |= MessageFlags::MENTIONS_ME; }
    if (mtp_flags & (1 << 5)) != 0 { f |= MessageFlags::MEDIA_IS_UNREAD; }
    if (mtp_flags & (1 << 13)) != 0 { f |= MessageFlags::SILENT; }
    if (mtp_flags & (1 << 14)) != 0 { f |= MessageFlags::POST; }
    if (mtp_flags & (1 << 16)) != 0 { f |= MessageFlags::LEGACY; }
    if (mtp_flags & (1 << 21)) != 0 { f |= MessageFlags::HIDE_EDITED; }
    if (mtp_flags & (1 << 24)) != 0 { f |= MessageFlags::PINNED; }
    if (mtp_flags & (1 << 8)) != 0 { f |= MessageFlags::HAS_FROM_ID; }
    if (mtp_flags & (1 << 3)) != 0 { f |= MessageFlags::HAS_REPLY_INFO; }
    if (mtp_flags & (1 << 6)) != 0 { f |= MessageFlags::HAS_REPLY_MARKUP; }
    if (mtp_flags & (1 << 10)) != 0 { f |= MessageFlags::HAS_VIEWS; }
    if (mtp_flags & (1 << 26)) != 0 { f |= MessageFlags::NO_FORWARDS; }
    if (mtp_flags & (1 << 27)) != 0 { f |= MessageFlags::INVERT_MEDIA; }
    MessageFlags(f)
}
