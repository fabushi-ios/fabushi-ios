pub mod calls_group_calls;
pub mod chat_admin_moderation;
pub mod export_payments_bots;
pub mod folders_filters;
pub mod forum_topics;
pub mod polls_todo;
pub mod settings_cloud_passkeys;
pub mod stories_gifts_stars;

pub use calls_group_calls::*;
pub use chat_admin_moderation::*;
pub use export_payments_bots::*;
pub use folders_filters::*;
pub use forum_topics::*;
pub use polls_todo::*;
pub use settings_cloud_passkeys::*;
pub use stories_gifts_stars::*;

use crate::protocol::Failure;
use serde_json::Value;

#[derive(Debug, Default)]
pub struct TdrpCoordinatorState {
    pub folders: folders_filters::FolderFilterState,
    pub topics: forum_topics::ForumTopicsState,
    pub polls_todo: polls_todo::PollsTodoState,
    pub stories_gifts_stars: stories_gifts_stars::StoriesGiftsStarsState,
    pub moderation: chat_admin_moderation::ChatAdminModerationState,
    pub calls: calls_group_calls::CallsGroupCallsState,
    pub settings: settings_cloud_passkeys::SettingsCloudPasskeysState,
    pub export_payments_bots: export_payments_bots::ExportPaymentsBotsState,
}

impl TdrpCoordinatorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn normalize_method(method: &str) -> &str {
        method
            .strip_prefix("feature.")
            .or_else(|| method.strip_prefix("tdrp."))
            .unwrap_or(method)
    }

    pub fn is_handled(method: &str) -> bool {
        let op = Self::normalize_method(method);
        op.starts_with("folders.")
            || op.starts_with("topics.")
            || op.starts_with("polls.")
            || op.starts_with("todo.")
            || op.starts_with("stories.")
            || op.starts_with("stars.")
            || op.starts_with("gifts.")
            || op.starts_with("admin.")
            || op.starts_with("calls.")
            || op.starts_with("groupCalls.")
            || op.starts_with("settings.")
            || op.starts_with("auth.")
            || op.starts_with("export.")
            || op.starts_with("payments.")
            || op.starts_with("bots.")
    }

    pub fn dispatch(&mut self, method: &str, args: &Value) -> Option<Result<Value, Failure>> {
        let op = Self::normalize_method(method);
        if op.starts_with("folders.") {
            Some(self.folders.handle_route(op, args))
        } else if op.starts_with("topics.") {
            Some(self.topics.handle_route(op, args))
        } else if op.starts_with("polls.") || op.starts_with("todo.") {
            Some(self.polls_todo.handle_route(op, args))
        } else if op.starts_with("stories.") || op.starts_with("stars.") || op.starts_with("gifts.") {
            Some(self.stories_gifts_stars.handle_route(op, args))
        } else if op.starts_with("admin.") {
            Some(self.moderation.handle_route(op, args))
        } else if op.starts_with("calls.") || op.starts_with("groupCalls.") {
            Some(self.calls.handle_route(op, args))
        } else if op.starts_with("settings.") || op.starts_with("auth.") {
            Some(self.settings.handle_route(op, args))
        } else if op.starts_with("export.") || op.starts_with("payments.") || op.starts_with("bots.") {
            Some(self.export_payments_bots.handle_route(op, args))
        } else {
            None
        }
    }
}
