//! Target Symbol: DataBotApp
//! Telegram Desktop 1:1 Submodule: `data/data_bot_app`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use gpui_telegram_proto::miniapp_contracts::{
    validate_cloud_storage_entry, BotRegistryEntry, CloudStorageItem, MiniAppActionRequest,
    MiniAppActionResponse, MiniAppEntry, MiniAppLaunchContext,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Mini App session prolong timeout (`kProlongTimeout = 60s`).
pub const K_PROLONG_TIMEOUT_SECS: u64 = 60;

/// Attach-menu bot list refresh timeout (`kRefreshBotsTimeout = 3600s`).
pub const K_REFRESH_BOTS_TIMEOUT_SECS: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MinimizedMiniAppTab {
    pub app_key: String,
    pub title: String,
    pub bot_username: String,
    pub short_name: String,
    pub icon_emoji: String,
    pub context: MiniAppLaunchContext,
    pub minimized_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataBotApp {
    pub id: u64,
    pub access_hash: u64,
    pub short_name: String,
    pub title: String,
    pub description: String,
    pub photo_id: Option<u64>,
    pub document_id: Option<u64>,
    pub hash: u64,
    pub is_inactive: bool,
    pub request_write_access: bool,
    #[serde(default)]
    pub cached_bots: BTreeMap<i64, BotRegistryEntry>,
    #[serde(default)]
    pub attach_menu_bots: Vec<BotRegistryEntry>,
    #[serde(default)]
    pub attach_bots_refreshed_at: i64,
    #[serde(default)]
    pub installed_apps: BTreeMap<String, MiniAppEntry>,
    #[serde(default)]
    pub active_context: Option<MiniAppLaunchContext>,
    #[serde(default)]
    pub active_last_prolonged_at: i64,
    #[serde(default)]
    pub minimized_tabs: Vec<MinimizedMiniAppTab>,
    #[serde(default)]
    pub cloud_storage_cache: BTreeMap<String, Vec<CloudStorageItem>>,
}

impl Default for DataBotApp {
    fn default() -> Self {
        Self {
            id: 0,
            access_hash: 0,
            short_name: String::new(),
            title: String::new(),
            description: String::new(),
            photo_id: None,
            document_id: None,
            hash: 0,
            is_inactive: false,
            request_write_access: false,
            cached_bots: BTreeMap::new(),
            attach_menu_bots: Vec::new(),
            attach_bots_refreshed_at: 0,
            installed_apps: BTreeMap::new(),
            active_context: None,
            active_last_prolonged_at: 0,
            minimized_tabs: Vec::new(),
            cloud_storage_cache: BTreeMap::new(),
        }
    }
}

impl DataBotApp {
    pub fn new(
        id: u64,
        access_hash: u64,
        short_name: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            id,
            access_hash,
            short_name: short_name.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn app_key(bot_username: &str, short_name: &str) -> String {
        format!(
            "{}/{}",
            bot_username.trim().trim_start_matches('@').to_ascii_lowercase(),
            short_name.trim().to_ascii_lowercase()
        )
    }

    pub fn needs_attach_bots_refresh(&self, now_ts: i64) -> bool {
        self.attach_bots_refreshed_at <= 0
            || now_ts.saturating_sub(self.attach_bots_refreshed_at)
                >= K_REFRESH_BOTS_TIMEOUT_SECS as i64
    }

    pub fn needs_prolong(&self, now_ts: i64) -> bool {
        self.active_context.is_some()
            && now_ts.saturating_sub(self.active_last_prolonged_at) >= K_PROLONG_TIMEOUT_SECS as i64
    }

    pub fn mark_prolonged(&mut self, now_ts: i64) {
        if self.active_context.is_some() {
            self.active_last_prolonged_at = now_ts;
        }
    }

    pub fn cache_bot(&mut self, bot: BotRegistryEntry) {
        self.cached_bots.insert(bot.bot_user_id, bot);
    }

    pub fn get_bot_by_username(&self, username: &str) -> Option<&BotRegistryEntry> {
        let needle = username.trim().trim_start_matches('@');
        self.cached_bots
            .values()
            .find(|b| b.username.eq_ignore_ascii_case(needle))
    }

    pub fn set_attach_menu_bots(&mut self, bots: Vec<BotRegistryEntry>, now_ts: i64) {
        for b in &bots {
            self.cached_bots.insert(b.bot_user_id, b.clone());
        }
        self.attach_menu_bots = bots;
        self.attach_bots_refreshed_at = now_ts;
    }

    pub fn attach_bots_for_peer_type(&self, peer_type: &str) -> Vec<&BotRegistryEntry> {
        let target = peer_type.trim().to_ascii_lowercase();
        self.attach_menu_bots
            .iter()
            .filter(|b| {
                b.allow_attach_menu
                    && (b.attach_peer_types.is_empty()
                        || b.attach_peer_types
                            .iter()
                            .any(|pt| pt.eq_ignore_ascii_case(&target) || pt == "all"))
            })
            .collect()
    }

    pub fn register_app(&mut self, app: MiniAppEntry) {
        let key = Self::app_key(&app.bot_username, &app.short_name);
        if self.short_name.is_empty() || self.short_name == app.short_name {
            self.short_name = app.short_name.clone();
            self.title = app.title.clone();
            self.description = app.description.clone();
            self.request_write_access = !app.has_write_access;
            if app.bot_user_id > 0 {
                self.id = app.bot_user_id as u64;
            }
        }
        self.installed_apps.insert(key, app);
    }

    pub fn set_installed_apps(&mut self, apps: Vec<MiniAppEntry>) {
        self.installed_apps.clear();
        for app in apps {
            self.register_app(app);
        }
    }

    pub fn uninstall_app(&mut self, bot_username: &str, short_name: &str) -> bool {
        let key = Self::app_key(bot_username, short_name);
        let removed_app = self.installed_apps.remove(&key).is_some();
        self.minimized_tabs.retain(|t| t.app_key != key);
        self.cloud_storage_cache.remove(&key);
        if let Some(ref active) = self.active_context {
            if Self::app_key(&active.app.bot_username, &active.app.short_name) == key {
                self.active_context = None;
            }
        }
        removed_app
    }

    pub fn open_app(&mut self, ctx: MiniAppLaunchContext, now_ts: i64) {
        let new_key = Self::app_key(&ctx.app.bot_username, &ctx.app.short_name);
        if let Some(prev) = self.active_context.take() {
            let prev_key = Self::app_key(&prev.app.bot_username, &prev.app.short_name);
            if prev_key != new_key {
                self.push_minimized_tab(prev, now_ts);
            }
        }
        self.minimized_tabs.retain(|t| t.app_key != new_key);
        self.cache_bot(ctx.companion_bot.clone());
        self.register_app(ctx.app.clone());
        self.cloud_storage_cache
            .insert(new_key, ctx.cloud_storage.clone());
        self.active_context = Some(ctx);
        self.active_last_prolonged_at = now_ts;
    }

    fn push_minimized_tab(&mut self, ctx: MiniAppLaunchContext, now_ts: i64) {
        let key = Self::app_key(&ctx.app.bot_username, &ctx.app.short_name);
        self.minimized_tabs.retain(|t| t.app_key != key);
        self.minimized_tabs.push(MinimizedMiniAppTab {
            app_key: key,
            title: ctx.app.title.clone(),
            bot_username: ctx.app.bot_username.clone(),
            short_name: ctx.app.short_name.clone(),
            icon_emoji: ctx.app.icon_emoji.clone(),
            context: ctx,
            minimized_at: now_ts,
        });
    }

    pub fn minimize_active(&mut self, now_ts: i64) -> Option<MinimizedMiniAppTab> {
        let ctx = self.active_context.take()?;
        self.push_minimized_tab(ctx, now_ts);
        self.minimized_tabs.last().cloned()
    }

    pub fn restore_minimized(
        &mut self,
        app_key: &str,
        now_ts: i64,
    ) -> Option<&MiniAppLaunchContext> {
        let normalized = app_key.trim().trim_start_matches('@').to_ascii_lowercase();
        let pos = self
            .minimized_tabs
            .iter()
            .position(|t| t.app_key == normalized)?;
        let mut tab = self.minimized_tabs.remove(pos);
        if let Some(cached) = self.cloud_storage_cache.get(&normalized) {
            tab.context.cloud_storage = cached.clone();
        }
        self.open_app(tab.context, now_ts);
        self.active_context.as_ref()
    }

    pub fn close_app(&mut self) -> Option<MiniAppLaunchContext> {
        self.active_last_prolonged_at = 0;
        self.active_context.take()
    }

    pub fn close_minimized(&mut self, app_key: &str) -> bool {
        let normalized = app_key.trim().trim_start_matches('@').to_ascii_lowercase();
        let before = self.minimized_tabs.len();
        self.minimized_tabs.retain(|t| t.app_key != normalized);
        self.minimized_tabs.len() < before
    }

    pub fn apply_action_response(&mut self, resp: &MiniAppActionResponse) {
        if let Some(ref mut ctx) = self.active_context {
            ctx.app.ui_schema = resp.ui_schema.clone();
            ctx.state_json = resp.state_json.clone();
            let key = Self::app_key(&ctx.app.bot_username, &ctx.app.short_name);
            if let Some(app_entry) = self.installed_apps.get_mut(&key) {
                app_entry.ui_schema = resp.ui_schema.clone();
            }
        }
        if resp.close_app {
            self.close_app();
        }
    }

    pub fn handle_main_button(&self) -> Option<MiniAppActionRequest> {
        let ctx = self.active_context.as_ref()?;
        let btn = ctx.app.ui_schema.main_button.as_ref()?;
        if !btn.is_visible || !btn.is_active {
            return None;
        }
        let target_chat = ctx.source_chat_id.or(Some(ctx.companion_chat_id));
        if btn.sends_data {
            let payload = serde_json::json!({
                "action_id": btn.action_id,
                "state": ctx.state_json,
                "app": ctx.app.short_name,
            })
            .to_string();
            Some(MiniAppActionRequest {
                action_type: "main_button_click".to_string(),
                query_id: Some(ctx.init_data.query_id.clone()),
                widget_id: if btn.action_id.is_empty() {
                    None
                } else {
                    Some(btn.action_id.clone())
                },
                value: Some(ctx.state_json.clone()),
                data: Some(payload),
                prompt: None,
                chat_id: target_chat,
            })
        } else {
            Some(MiniAppActionRequest {
                action_type: "main_button_click".to_string(),
                query_id: Some(ctx.init_data.query_id.clone()),
                widget_id: if btn.action_id.is_empty() {
                    None
                } else {
                    Some(btn.action_id.clone())
                },
                value: Some(ctx.state_json.clone()),
                data: None,
                prompt: None,
                chat_id: target_chat,
            })
        }
    }

    pub fn handle_secondary_button(&self) -> Option<MiniAppActionRequest> {
        let ctx = self.active_context.as_ref()?;
        let btn = ctx.app.ui_schema.secondary_button.as_ref()?;
        if !btn.is_visible || !btn.is_active {
            return None;
        }
        Some(MiniAppActionRequest {
            action_type: "secondary_button_click".to_string(),
            query_id: Some(ctx.init_data.query_id.clone()),
            widget_id: if btn.action_id.is_empty() {
                None
            } else {
                Some(btn.action_id.clone())
            },
            value: Some(ctx.state_json.clone()),
            data: None,
            prompt: None,
            chat_id: ctx.source_chat_id.or(Some(ctx.companion_chat_id)),
        })
    }

    pub fn get_cloud_storage_items(
        &self,
        bot_username: &str,
        short_name: &str,
    ) -> &[CloudStorageItem] {
        let key = Self::app_key(bot_username, short_name);
        self.cloud_storage_cache
            .get(&key)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    fn upsert_storage_entry(items: &mut Vec<CloudStorageItem>, item: &CloudStorageItem) {
        if let Some(existing) = items.iter_mut().find(|e| e.key == item.key) {
            *existing = item.clone();
        } else {
            items.push(item.clone());
        }
    }

    fn remove_storage_entry(items: &mut Vec<CloudStorageItem>, storage_key: &str) -> bool {
        let before = items.len();
        items.retain(|e| e.key != storage_key);
        items.len() < before
    }

    pub fn upsert_cloud_storage_item(
        &mut self,
        bot_username: &str,
        short_name: &str,
        item: CloudStorageItem,
    ) -> Result<(), String> {
        let app_key = Self::app_key(bot_username, short_name);
        let items = self.cloud_storage_cache.entry(app_key.clone()).or_default();
        validate_cloud_storage_entry(&item.key, &item.value, items)?;
        Self::upsert_storage_entry(items, &item);
        if let Some(ref mut active) = self.active_context {
            if Self::app_key(&active.app.bot_username, &active.app.short_name) == app_key {
                Self::upsert_storage_entry(&mut active.cloud_storage, &item);
            }
        }
        for tab in &mut self.minimized_tabs {
            if tab.app_key == app_key {
                Self::upsert_storage_entry(&mut tab.context.cloud_storage, &item);
            }
        }
        Ok(())
    }

    pub fn remove_cloud_storage_item(
        &mut self,
        bot_username: &str,
        short_name: &str,
        storage_key: &str,
    ) -> bool {
        let app_key = Self::app_key(bot_username, short_name);
        let mut removed = false;
        if let Some(items) = self.cloud_storage_cache.get_mut(&app_key) {
            removed |= Self::remove_storage_entry(items, storage_key);
        }
        if let Some(ref mut active) = self.active_context {
            if Self::app_key(&active.app.bot_username, &active.app.short_name) == app_key {
                removed |= Self::remove_storage_entry(&mut active.cloud_storage, storage_key);
            }
        }
        for tab in &mut self.minimized_tabs {
            if tab.app_key == app_key {
                removed |= Self::remove_storage_entry(&mut tab.context.cloud_storage, storage_key);
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_telegram_proto::miniapp_contracts::{
        MiniAppActionButton, MiniAppInitDataDto, MiniAppLaunchMode, MiniAppUiSchema,
    };

    fn sample_launch_context(bot_username: &str, short_name: &str, title: &str) -> MiniAppLaunchContext {
        MiniAppLaunchContext {
            app: MiniAppEntry {
                app_id: format!("app_{}", short_name),
                bot_user_id: 600_001,
                bot_username: bot_username.to_string(),
                bot_display_name: title.to_string(),
                short_name: short_name.to_string(),
                title: title.to_string(),
                is_installed: true,
                has_write_access: true,
                ui_schema: MiniAppUiSchema {
                    title: title.to_string(),
                    main_button: Some(MiniAppActionButton {
                        text: "Submit".to_string(),
                        action_id: "submit".to_string(),
                        sends_data: true,
                        is_visible: true,
                        is_active: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            companion_bot: BotRegistryEntry {
                bot_user_id: 600_001,
                owner_user_id: 100_001,
                username: bot_username.to_string(),
                display_name: title.to_string(),
                allow_attach_menu: true,
                attach_peer_types: vec!["private".to_string(), "group".to_string()],
                ..Default::default()
            },
            companion_chat_id: 600_000_001,
            source_chat_id: Some(750_000_001),
            launch_mode: MiniAppLaunchMode::CompactSheet,
            start_param: None,
            init_data: MiniAppInitDataDto {
                query_id: "qid_1".to_string(),
                hmac_verified: true,
                ..Default::default()
            },
            cloud_storage: Vec::new(),
            state_json: serde_json::json!({"selected": "latte"}),
        }
    }

    #[test]
    fn test_data_bot_app_lifecycle_minimize_restore_and_storage() {
        let mut store = DataBotApp::default();
        assert!(store.needs_attach_bots_refresh(1000));

        let ctx1 = sample_launch_context("CoffeeOrderBot", "order", "Coffee Order");
        let ctx2 = sample_launch_context("PomodoroFocusBot", "focus", "Pomodoro Focus");

        store.set_attach_menu_bots(vec![ctx1.companion_bot.clone()], 1000);
        assert!(!store.needs_attach_bots_refresh(1050));
        assert!(store.needs_attach_bots_refresh(1000 + K_REFRESH_BOTS_TIMEOUT_SECS as i64));
        assert_eq!(store.attach_bots_for_peer_type("group").len(), 1);
        assert_eq!(store.attach_bots_for_peer_type("channel").len(), 0);

        store.open_app(ctx1.clone(), 1000);
        assert!(!store.needs_prolong(1030));
        assert!(store.needs_prolong(1000 + K_PROLONG_TIMEOUT_SECS as i64));
        let req = store.handle_main_button().expect("main button request");
        assert_eq!(req.action_type, "main_button_click");
        assert!(req.data.is_some());

        // Opening a second app minimizes the first automatically
        store.open_app(ctx2, 1010);
        assert_eq!(store.minimized_tabs.len(), 1);
        assert_eq!(store.minimized_tabs[0].app_key, "coffeeorderbot/order");

        // CloudStorage upsert + removal
        store
            .upsert_cloud_storage_item(
                "CoffeeOrderBot",
                "order",
                CloudStorageItem {
                    key: "favorite".to_string(),
                    value: "iced_latte".to_string(),
                    updated_at: 1011,
                },
            )
            .expect("valid cloud storage item");
        assert_eq!(
            store.get_cloud_storage_items("CoffeeOrderBot", "order").len(),
            1
        );

        // Restore first app; second becomes minimized
        let restored = store
            .restore_minimized("coffeeorderbot/order", 1020)
            .expect("restored");
        assert_eq!(restored.app.short_name, "order");
        assert_eq!(store.minimized_tabs.len(), 1);
        assert_eq!(store.minimized_tabs[0].app_key, "pomodorofocusbot/focus");

        assert!(store.remove_cloud_storage_item("CoffeeOrderBot", "order", "favorite"));
        assert!(store.close_app().is_some());
    }
}
