//! Telegram History Item Runtime Components
//!
//! Citations:
//! - `Telegram/SourceFiles/history/history_item_components.h:L86-L1029`
//! - `Telegram/SourceFiles/history/history_item_components.cpp:L1-L1685`

use serde::{Deserialize, Serialize};

/// Maximum recent replier avatars displayed in message footer
/// Upstream `history_item_components.h:L111`: `kMaxRecentRepliers = 3`
pub const K_MAX_RECENT_REPLIERS: usize = 3;

/// Upstream `history_item_components.h:L86-L95`: `HistoryMessageVia`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessageVia {
    pub bot_id: i64,
    pub username: String,
    pub link_url: Option<String>,
}

/// Upstream `history_item_components.h:L97-L107`: `HistoryMessageGuestChat`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessageGuestChat {
    pub visitor_id: i64,
    pub name: String,
}

/// Upstream `history_item_components.h:L109-L128`: `HistoryMessageViews`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryMessageViews {
    pub views: i32,
    pub replies: i32,
    pub recent_repliers: Vec<i64>,
}

impl HistoryMessageViews {
    pub fn new(views: i32, replies: i32) -> Self {
        Self {
            views,
            replies,
            recent_repliers: Vec::new(),
        }
    }

    pub fn add_replier(&mut self, peer_id: i64) {
        if !self.recent_repliers.contains(&peer_id) {
            if self.recent_repliers.len() >= K_MAX_RECENT_REPLIERS {
                self.recent_repliers.remove(0);
            }
            self.recent_repliers.push(peer_id);
        }
    }
}

/// Upstream `history_item_components.h:L129-L135`: `HistoryMessageSigned`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessageSigned {
    pub author: String,
}

/// Upstream `history_item_components.h:L136-L140`: `HistoryMessageFromRank`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessageFromRank {
    pub rank: String,
}

/// Upstream `history_item_components.h:L141-L145`: `HistoryMessageEdited`
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HistoryMessageEdited {
    pub edit_date: i64,
}

/// Upstream `history_item_components.h:L204-L236`: `HistoryMessageForwarded`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessageForwarded {
    pub original_sender_id: Option<i64>,
    pub original_name: Option<String>,
    pub original_date: i64,
    pub original_id: Option<i64>,
    pub psa_type: Option<String>,
}

/// Upstream `history_item_components.h:L243-L358`: `HistoryMessageSaved`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessageSaved {
    pub sublist_peer_id: i64,
    pub saved_from_peer_id: Option<i64>,
    pub saved_from_name: Option<String>,
}

/// Upstream `history_item_components.h:L359-L446`: `HistoryMessageReply`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryMessageReply {
    pub reply_to_msg_id: i64,
    pub reply_to_top_id: Option<i64>,
    pub reply_to_story_id: Option<i64>,
    pub quote_text: Option<String>,
    pub quote_offset: Option<i32>,
    pub is_forum_post: bool,
}

/// Upstream `history_item_components.h:L447-L456`: `HistoryMessageTranslation`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessageTranslation {
    pub text: String,
    pub target_language: String,
    pub is_used: bool,
}

/// Upstream `history_item_components.h:L693-L699`: `HistoryMessageFactcheck`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessageFactcheck {
    pub text: String,
    pub country: Option<String>,
    pub hash: i64,
    pub need_check: bool,
}

/// Upstream `history_item_components.h:L700-L709`: `HistoryMessageSuggestion`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryMessageSuggestion {
    pub price_stars: i64,
    pub date: i64,
    pub accepted: bool,
    pub rejected: bool,
}

/// Upstream `history_item_components.h:L710-L714`: `HistoryMessageRestrictions`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessageRestrictions {
    pub reason: String,
    pub platform: String,
}

/// Upstream `history_item_components.h:L731-L735`: `HistoryServicePinned`
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HistoryServicePinned {
    pub pinned_msg_id: i64,
}

/// Upstream `history_item_components.h:L741-L762`: `HistoryServiceTopicInfo`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryServiceTopicInfo {
    pub topic_id: i64,
    pub title: String,
    pub icon_emoji_id: Option<u64>,
}

/// Upstream `history_item_components.h:L763-L773`: `HistoryServiceTodoCompletions`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryServiceTodoCompletions {
    pub completed_task_ids: Vec<i64>,
    pub is_completed: bool,
}

/// Upstream `history_item_components.h:L845-L850`: `HistoryServiceGameScore`
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HistoryServiceGameScore {
    pub game_id: i64,
    pub score: i32,
}

/// Upstream `history_item_components.h:L851-L861`: `HistoryServicePayment`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryServicePayment {
    pub currency: String,
    pub total_amount: i64,
}

/// Upstream `history_item_components.h:L886-L897`: `HistoryServiceGramTransfer`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryServiceGramTransfer {
    pub stars: i64,
    pub comment: Option<String>,
}

/// Upstream `history_item_components.h:L898-L923`: `HistoryServiceTonConnectRequest`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryServiceTonConnectRequest {
    pub manifest_name: String,
    pub dapp_name: String,
    pub expires_at: i64,
}

/// Upstream `history_item_components.h:L924-L932`: `HistoryServiceSelfDestruct`
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HistoryServiceSelfDestruct {
    pub ttl_seconds: i64,
}

/// Upstream `history_item_components.h:L940-L944`: `HistoryServiceChatThemeChange`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryServiceChatThemeChange {
    pub theme_name: String,
}

/// Upstream `history_item_components.h:L945-L950`: `HistoryServiceTTLChange`
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HistoryServiceTTLChange {
    pub ttl_seconds: i64,
}

/// Upstream `history_item_components.h:L1026-L1028`: `HistoryMessageSchedulePeriod`
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HistoryMessageSchedulePeriod {
    pub period_seconds: i64,
}
