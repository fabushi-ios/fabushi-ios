//! Tier 3 headless GPUI entity graph, asset source, theme, i18n, and interaction driver.

use std::sync::Arc;

use gpui::{AppContext, Application, AssetSource, Entity};
use gpui_telegram_client::net::http_client::HistoricalMessage;
use gpui_telegram_client::{ChatSummary, ClientConfig, TelegramClient};
use gpui_telegram_desktop::i18n::{tr_key_or, tr_str};
use gpui_telegram_desktop::theme::{
    IconName, TdesktopThemeMode, TdesktopThemePalette, TelegramAssetSource,
};
use gpui_telegram_desktop::views::app_shell::shell_truth::{
    apply_fenced_chat_list_response, apply_fenced_history_response, select_authoritative_dialog,
    try_consume_composer_for_send, SessionRequestFence,
};
use gpui_telegram_desktop::views::composer::KeyAction;
use gpui_telegram_desktop::views::settings::settings_type::SettingsSectionType;
use gpui_telegram_desktop::views::shell::ActiveWorkspacePanel;
use gpui_telegram_desktop::views::AppShell;

/// Drives headless GPUI desktop entity graph, SVG/locale asset checks, and theme transitions.
pub struct HeadlessTranscriptDriver;

impl HeadlessTranscriptDriver {
    /// Verifies all 64 embedded SVG icons and localization packs load through `AssetSource`.
    pub fn verify_all_embedded_assets() {
        let src = TelegramAssetSource::new();
        assert_eq!(IconName::ALL.len(), 64, "Expected 64 canonical SVG icons");
        for &icon in IconName::ALL {
            let loaded = src
                .load(icon.path())
                .expect("AssetSource::load must succeed")
                .expect("Embedded SVG icon must exist");
            let text = std::str::from_utf8(loaded.as_ref()).expect("SVG bytes must be UTF-8");
            assert!(text.contains("<svg"), "Icon {} missing <svg>", icon.path());
        }
        for locale_path in ["langs/en.strings", "langs/zh-CN.strings"] {
            let data = src
                .load(locale_path)
                .expect("load locale strings")
                .expect("locale strings file present");
            let s = std::str::from_utf8(data.as_ref()).expect("locale file UTF-8");
            assert!(s.contains('='), "Locale file {locale_path} must contain entries");
        }
        let icon_entries = src.list("icons").expect("list icons");
        assert!(icon_entries.len() >= IconName::ALL.len());
        let lang_entries = src.list("langs").expect("list langs");
        assert!(lang_entries.iter().any(|p| p.as_ref() == "langs/en.strings"));
        assert!(lang_entries.iter().any(|p| p.as_ref() == "langs/zh-CN.strings"));
    }

    /// Verifies `Night`, `Tinted`, `Day`, and `Classic` palettes plus `en`/`zh-CN` strings.
    pub fn verify_themes_and_localization() {
        let night = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x2AABEE, 14, 12);
        let tinted = TdesktopThemePalette::build(TdesktopThemeMode::Tinted, 0x2AABEE, 14, 12);
        let day = TdesktopThemePalette::build(TdesktopThemeMode::Day, 0x3390EC, 15, 10);
        let classic = TdesktopThemePalette::build(TdesktopThemeMode::Classic, 0x3F8E47, 14, 12);

        assert!(night.mode.is_dark() && tinted.mode.is_dark());
        assert!(!day.mode.is_dark() && !classic.mode.is_dark());
        assert_ne!(night.window_bg, day.window_bg);
        assert_ne!(tinted.window_bg, night.window_bg);
        assert_ne!(classic.msg_out_bg, night.msg_out_bg);
        let aurora = night.with_wallpaper("aurora_dusk");
        assert_ne!(aurora.wallpaper_top, aurora.wallpaper_bottom);
        let (top_c, bot_c) = night.peer_avatar_gradient(42);
        assert_ne!(top_c, 0);
        assert_ne!(bot_c, 0);

        assert_eq!(tr_str("zh-CN", "设置", "Settings"), "设置");
        assert_eq!(tr_str("en", "设置", "Settings"), "Settings");
        assert!(!tr_key_or("en", "lng_settings_chat", "Chat Settings").is_empty());
        assert!(!tr_key_or("zh-CN", "lng_settings_chat", "聊天设置").is_empty());
    }

    /// Boots `gpui::Application::headless()` with `TelegramAssetSource`, instantiates `AppShell`
    /// and all child entities, drives theme, settings routing, chat/history truth sync, and
    /// composer send gating, then exits cleanly (`process::exit(0)` to avoid `CFRunLoopRun`).
    pub fn run_gpui_headless_app_shell_child(server_url: &str) -> ! {
        let config = ClientConfig {
            server_url: server_url.to_string(),
            ..Default::default()
        };
        let client = Arc::new(TelegramClient::new(config));
        let _tokio_guard = client.runtime().enter();

        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx| {
                let svg_probe = cx
                    .asset_source()
                    .load("icons/search.svg")
                    .expect("load search.svg")
                    .expect("search.svg present");
                assert!(std::str::from_utf8(svg_probe.as_ref()).unwrap().contains("<svg"));

                let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
                shell.update(cx, |s, cx| {
                    assert!(!s.is_authenticated);
                    assert!(s.sidebar.read(cx).dialogs.is_empty());
                    assert!(s.stage.read(cx).messages.is_empty());

                    s.is_authenticated = true;
                    s.session_sync.user_id = Some(9001);
                    s.session_sync.session_token = Some("headless_tok".into());
                    s.session_sync.auth_generation = 1;
                    s.session_sync.chat_list_request_generation = 1;
                    s.session_sync.history_request_generation = 1;

                    s.main_menu_drawer.update(cx, |d, _| d.open());
                    assert!(s.main_menu_drawer.read(cx).is_open);
                    s.main_menu_drawer.update(cx, |d, _| d.close());

                    s.apply_theme_selection(
                        TdesktopThemeMode::Day,
                        Some(0x3390EC),
                        Some("default_day".into()),
                        cx,
                    );
                    assert_eq!(s.theme_state.theme_mode, TdesktopThemeMode::Day);
                    assert!(!s.composer.read(cx).palette.mode.is_dark());
                    assert!(!s.nav_rail.read(cx).palette.mode.is_dark());
                    assert!(!s.main_menu_drawer.read(cx).palette.mode.is_dark());

                    s.apply_theme_selection(TdesktopThemeMode::Night, None, None, cx);
                    assert_eq!(s.theme_state.theme_mode, TdesktopThemeMode::Night);

                    assert!(
                        s.sidebar.read(cx).nav_rail.is_some(),
                        "unified sidebar should be attached to NavRail in AppShell"
                    );
                    let was_collapsed = s.sidebar.read(cx).is_collapsed;
                    s.sidebar.update(cx, |sb, sb_cx| {
                        sb.toggle_collapsed_from_user();
                        sb_cx.notify();
                    });
                    assert_ne!(s.sidebar.read(cx).is_collapsed, was_collapsed);
                    assert!(
                        s.sidebar.read(cx).is_collapsed,
                        "toggling unified sidebar should collapse it"
                    );
                    assert!(
                        (s.sidebar.read(cx).effective_width() - 64.0).abs() < f32::EPSILON
                    );
                    s.sidebar.update(cx, |sb, sb_cx| {
                        sb.toggle_collapsed_from_user();
                        sb_cx.notify();
                    });
                    assert!(
                        !s.sidebar.read(cx).is_collapsed,
                        "toggling unified sidebar again should expand it"
                    );
                    assert!(
                        (s.sidebar.read(cx).effective_width() - 288.0).abs() < f32::EPSILON
                    );

                    s.navigate_settings_to(SettingsSectionType::Chat, cx);
                    assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
                    assert_eq!(s.settings_router.read(cx).settings_section, SettingsSectionType::Chat);
                    s.active_panel = ActiveWorkspacePanel::Chats;

                    let fence = SessionRequestFence {
                        auth_generation: 1,
                        request_generation: 1,
                        token: "headless_tok".into(),
                        user_id: 9001,
                    };
                    let chats = vec![ChatSummary {
                        chat_id: 501,
                        title: "Design Guild".into(),
                        chat_type: 1,
                        unread_count: 1,
                        last_message: Some("Hello GPUI".into()),
                        is_group: Some(true),
                        is_bot: Some(false),
                        avatar_color: Some(0x2AABEE),
                        updated_at: Some(1_700_000_000_000),
                    }];
                    let sidebar_ent = s.sidebar.clone();
                    let stage_ent = s.stage.clone();
                    let insp_ent = s.inspector.clone();
                    let comp_ent = s.composer.clone();
                    let is_auth = s.is_authenticated;
                    let tok = s.session_sync.session_token.clone();
                    let uid = s.session_sync.user_id;

                    let (sb_applied, consumed) = sidebar_ent.update(cx, |sb, cx| {
                        stage_ent.update(cx, |st, cx| {
                            insp_ent.update(cx, |insp, cx| {
                                let ok = apply_fenced_chat_list_response(
                                    &fence, true, 1, 1, Some("headless_tok"), Some(9001),
                                    sb, st, insp, &chats,
                                );
                                assert!(ok && select_authoritative_dialog(sb, st, insp, 501));
                                let hist_ok = apply_fenced_history_response(
                                    &fence, true, 1, 1, Some("headless_tok"), Some(9001),
                                    sb, st, 501,
                                    vec![HistoricalMessage {
                                        message_id: 77,
                                        sender_id: 9001,
                                        text: "Synced msg".into(),
                                        content: None,
                                        date: Some(1_700_000_000_000),
                                        reply_to: None,
                                        is_outgoing: Some(true),
                                        ..Default::default()
                                    }],
                                );
                                let sent = comp_ent.update(cx, |c, _| {
                                    c.set_text("Line 1");
                                    assert_eq!(c.handle_enter(true), KeyAction::InsertNewline);
                                    assert_eq!(c.text(), "Line 1\n");
                                    assert_eq!(c.handle_enter(false), KeyAction::SendMessage);
                                    c.submit_primary();
                                    try_consume_composer_for_send(
                                        is_auth, tok.as_deref(), uid, sb, st, c,
                                    )
                                });
                                (hist_ok, sent)
                            })
                        })
                    });
                    assert!(sb_applied);
                    assert_eq!(s.stage.read(cx).messages.len(), 1);
                    assert_eq!(consumed.expect("send target").1, "Line 1");

                    // Verify clicking a message reaction through `ConversationStage` + `AppShell`
                    // `cx.observe` does NOT double-toggle/self-cancel back to empty!
                    s.stage.update(cx, |st, st_cx| {
                        st.handle_bubble_action(
                            gpui_telegram_desktop::views::stage::stage_bubble::BubbleUiAction::ToggleReaction(
                                77,
                                "👍".to_string(),
                            ),
                            st_cx,
                        );
                    });
                });

                // After `shell.update` returns, GPUI flushes `cx.observe(&stage)` -> `AppShell::handle_stage_pending_actions`!
                shell.update(cx, |s, cx| {
                    let rx_after_observe = s
                        .stage
                        .read(cx)
                        .messages
                        .iter()
                        .find(|m| m.id == 77)
                        .expect("msg 77")
                        .reactions
                        .clone();
                    assert_eq!(
                        rx_after_observe.len(),
                        1,
                        "AppShell observer must not double-toggle reaction back to 0"
                    );
                    assert_eq!(rx_after_observe[0].emoji, "👍");
                    assert_eq!(rx_after_observe[0].count, 1);
                    assert!(rx_after_observe[0].user_reacted);

                    // Verify incoming `UpdateEvent::ReactionUpdated` from a DIFFERENT peer (9999 != 9001)
                    // preserves viewer 9001's `user_reacted` flags rather than adopting peer 9999's `is_chosen_by_me`.
                    let peer_ev = gpui_telegram_proto::sync::UpdateEvent::ReactionUpdated(
                        gpui_telegram_proto::sync::ReactionEvent::new(
                            gpui_telegram_core::types::ChatId(501),
                            gpui_telegram_core::types::MessageId(77),
                            gpui_telegram_core::types::UserId(9999),
                            vec![
                                gpui_telegram_proto::message::ReactionCount::new("👍", 2, true),
                                gpui_telegram_proto::message::ReactionCount::new("🔥", 1, true),
                            ],
                        ),
                    );
                    s.apply_update_event(gpui_telegram_core::types::ChatId(501), peer_ev, cx);

                    let rx_after_peer = s
                        .stage
                        .read(cx)
                        .messages
                        .iter()
                        .find(|m| m.id == 77)
                        .expect("msg 77")
                        .reactions
                        .clone();
                    let up = rx_after_peer.iter().find(|r| r.emoji == "👍").unwrap();
                    let fire = rx_after_peer.iter().find(|r| r.emoji == "🔥").unwrap();
                    assert_eq!(up.count, 2);
                    assert!(up.user_reacted, "Viewer 9001 still chose 👍");
                    assert_eq!(fire.count, 1);
                    assert!(
                        !fire.user_reacted,
                        "Peer 9999's 🔥 must not mark viewer 9001 as user_reacted"
                    );

                    // 1. Rapid multi-emoji clicks in a single frame (`❤️` + `🎉`) must BOTH be queued and drained
                    // by `AppShell::handle_stage_pending_actions` instead of overwriting a single `Option` slot!
                    s.session_sync.in_flight_reactions.clear();
                    s.stage.update(cx, |st, st_cx| {
                        st.handle_bubble_action(
                            gpui_telegram_desktop::views::stage::stage_bubble::BubbleUiAction::ToggleReaction(
                                77,
                                "❤️".to_string(),
                            ),
                            st_cx,
                        );
                        st.handle_bubble_action(
                            gpui_telegram_desktop::views::stage::stage_bubble::BubbleUiAction::ToggleReaction(
                                77,
                                "🎉".to_string(),
                            ),
                            st_cx,
                        );
                    });
                });

                shell.update(cx, |s, cx| {
                    assert_eq!(
                        s.session_sync.in_flight_reactions.get(&(501, 77)).copied(),
                        Some(2),
                        "Both rapid emoji toggles must be dispatched and tracked in in_flight_reactions"
                    );

                    // 2. Out-of-order HTTP reaction responses + contiguous `sync_coordinator` pts advance:
                    // Start at `last_pts == 10` with 2 in-flight reactions (`pts=11` arriving before stale `pts=10`).
                    s.client
                        .sync_coordinator
                        .set_pts(gpui_telegram_core::types::ChatId(501), 10);
                    let newer_snapshot = vec![
                        gpui_telegram_desktop::views::stage::cards::ReactionItem {
                            emoji: "👍".into(),
                            count: 2,
                            user_reacted: true,
                        },
                        gpui_telegram_desktop::views::stage::cards::ReactionItem {
                            emoji: "❤️".into(),
                            count: 1,
                            user_reacted: true,
                        },
                        gpui_telegram_desktop::views::stage::cards::ReactionItem {
                            emoji: "🎉".into(),
                            count: 1,
                            user_reacted: true,
                        },
                    ];
                    let stale_snapshot = vec![
                        gpui_telegram_desktop::views::stage::cards::ReactionItem {
                            emoji: "👍".into(),
                            count: 2,
                            user_reacted: true,
                        },
                        gpui_telegram_desktop::views::stage::cards::ReactionItem {
                            emoji: "❤️".into(),
                            count: 1,
                            user_reacted: true,
                        },
                    ];
                    s.apply_http_reaction_response(501, 77, 77, 11, newer_snapshot, cx);
                    assert_eq!(
                        s.client
                            .sync_coordinator
                            .last_pts(gpui_telegram_core::types::ChatId(501)),
                        11,
                        "Contiguous HTTP reaction pts=11 must advance sync_coordinator from 10 to 11"
                    );
                    // Now stale `pts=10` arrives last (`remaining -> 0`): must NOT clobber `🎉` from `pts=11`!
                    s.apply_http_reaction_response(501, 77, 77, 10, stale_snapshot, cx);
                    let rx_coalesced = s
                        .stage
                        .read(cx)
                        .messages
                        .iter()
                        .find(|m| m.id == 77)
                        .unwrap()
                        .reactions
                        .clone();
                    assert!(
                        rx_coalesced.iter().any(|r| r.emoji == "🎉" && r.user_reacted),
                        "Stale out-of-order HTTP pts=10 must not overwrite newer pts=11 reaction state"
                    );

                    // WS watermark (`pts=15`) arriving while an older HTTP request (`pts=14`) is in-flight:
                    s.session_sync.in_flight_reactions.insert((501, 77), 1);
                    let ws_ev = gpui_telegram_proto::sync::UpdateEvent::ReactionUpdated(
                        gpui_telegram_proto::sync::ReactionEvent::new(
                            gpui_telegram_core::types::ChatId(501),
                            gpui_telegram_core::types::MessageId(77),
                            gpui_telegram_core::types::UserId(9999),
                            vec![gpui_telegram_proto::message::ReactionCount::new("⚡", 3, true)],
                        ),
                    );
                    s.record_ws_reaction_pts(501, 15, &ws_ev);
                    s.apply_update_event(gpui_telegram_core::types::ChatId(501), ws_ev, cx);
                    s.apply_http_reaction_response(
                        501,
                        77,
                        77,
                        14,
                        vec![gpui_telegram_desktop::views::stage::cards::ReactionItem {
                            emoji: "👍".into(),
                            count: 1,
                            user_reacted: true,
                        }],
                        cx,
                    );
                    let rx_after_stale_http = &s
                        .stage
                        .read(cx)
                        .messages
                        .iter()
                        .find(|m| m.id == 77)
                        .unwrap()
                        .reactions;
                    assert!(
                        rx_after_stale_http.iter().any(|r| r.emoji == "⚡" && r.count == 3),
                        "Older HTTP pts=14 must not overwrite newer WS pts=15 reaction state"
                    );

                    // 3. Reacting to an optimistic `Sending` message (`client_nonce = 900_777`) queues the
                    // reaction in `pending_nonce_reactions` until `reconcile_outgoing_message` promotes it to `888`!
                    s.session_sync.pending_outgoing_nonces.insert(900_777, 501);
                    s.stage.update(cx, |st, st_cx| {
                        st.add_outgoing_text_message("Sending optimistic msg");
                        let local_id = st.messages.last().unwrap().id;
                        st.messages.last_mut().unwrap().id = 900_777;
                        if let Some(hist) = st.chat_histories.get_mut(&501) {
                            if let Some(m) = hist.iter_mut().find(|m| m.id == local_id) {
                                m.id = 900_777;
                            }
                        }
                        st.hovered_message_id = Some(900_777);
                        st.handle_bubble_action(
                            gpui_telegram_desktop::views::stage::stage_bubble::BubbleUiAction::ToggleReaction(
                                900_777,
                                "🔥".to_string(),
                            ),
                            st_cx,
                        );
                    });
                });

                shell.update(cx, |s, cx| {
                    assert_eq!(
                        s.session_sync.pending_nonce_reactions.get(&900_777),
                        Some(&vec![("🔥".to_string(), true)]),
                        "Reaction on unconfirmed client_nonce must be queued until server_id arrives"
                    );
                    s.session_sync.pending_outgoing_nonces.remove(&900_777);
                    s.session_sync.resolved_outgoing_nonces.insert(900_777, 888);
                    s.reconcile_outgoing_message(
                        501,
                        900_777,
                        888,
                        gpui_telegram_desktop::views::stage::cards::DeliveryStatus::Sent,
                        16,
                        cx,
                    );
                    assert!(!s.session_sync.pending_nonce_reactions.contains_key(&900_777));
                    assert_eq!(
                        s.session_sync.in_flight_reactions.get(&(501, 888)).copied(),
                        Some(1),
                        "Queued reaction must flush automatically against server_id=888 upon reconciliation"
                    );
                    assert_eq!(
                        s.stage.read(cx).hovered_message_id,
                        Some(888),
                        "Hovered message ID must remap cleanly from client_nonce 900_777 to server_id 888"
                    );
                    let reconciled_msg = s
                        .stage
                        .read(cx)
                        .messages
                        .iter()
                        .find(|m| m.id == 888)
                        .expect("reconciled msg 888");
                    assert!(
                        reconciled_msg
                            .reactions
                            .iter()
                            .any(|r| r.emoji == "🔥" && r.user_reacted),
                        "Optimistic reaction on Sending message must survive nonce->server reconciliation"
                    );

                    // 3b. Legacy server ack (`authoritative_reactions = None`, `pts = 17`) on `888`
                    // must NOT overwrite optimistic `🔥` with `vec![]`, MUST persist `🔥` to `LocalStore`,
                    // must survive legacy history reload omitting `"reactions"`, and must reject stale
                    // lower-`pts` WS `ReactionUpdated` (`pts = 16`) while applying newer `pts = 18` with `ev.chat_id = 0`!
                    s.apply_http_reaction_response_with_authority(501, 888, 888, 17, None, cx);
                    assert_eq!(s.session_sync.in_flight_reactions.get(&(501, 888)), None);
                    assert!(
                        s.stage
                            .read(cx)
                            .messages
                            .iter()
                            .find(|m| m.id == 888)
                            .unwrap()
                            .reactions
                            .iter()
                            .any(|r| r.emoji == "🔥" && r.user_reacted),
                        "Legacy non-authoritative reaction_updated ack must preserve stage reaction"
                    );
                    let stored_888 = s
                        .client
                        .local_store
                        .get_reactions(
                            gpui_telegram_core::types::ChatId(501),
                            gpui_telegram_core::types::MessageId(888),
                        )
                        .expect("expected cached reactions for message 888");
                    assert_eq!(stored_888.len(), 1);
                    assert_eq!(stored_888[0].emoji, "🔥");
                    assert!(stored_888[0].is_chosen_by_me);

                    // Legacy history payload omitting `"reactions"` (`!has_authoritative_reactions()`) must NOT wipe `🔥`!
                    let legacy_hm: HistoricalMessage = serde_json::from_str(
                        r#"{"message_id":888,"sender_id":9001,"text":"Sending optimistic msg","date":1700000001000,"is_outgoing":true}"#,
                    )
                    .expect("deserialize legacy HistoricalMessage");
                    assert!(!legacy_hm.has_authoritative_reactions());
                    let sb_clone = s.sidebar.clone();
                    let st_clone = s.stage.clone();
                    let reload_fence = SessionRequestFence {
                        auth_generation: 1,
                        request_generation: 1,
                        token: "headless_tok".into(),
                        user_id: 9001,
                    };
                    sb_clone.update(cx, |sb, cx| {
                        st_clone.update(cx, |st, _| {
                            assert!(
                                gpui_telegram_desktop::views::app_shell::shell_truth::apply_fenced_history_response_preserving(
                                    &reload_fence,
                                    true,
                                    1,
                                    1,
                                    Some("headless_tok"),
                                    Some(9001),
                                    sb,
                                    st,
                                    501,
                                    vec![legacy_hm],
                                    &std::collections::HashSet::new(),
                                )
                            );
                        });
                    });
                    assert!(
                        s.stage
                            .read(cx)
                            .messages
                            .iter()
                            .find(|m| m.id == 888)
                            .unwrap()
                            .reactions
                            .iter()
                            .any(|r| r.emoji == "🔥" && r.user_reacted),
                        "Legacy history reload omitting reactions must preserve reactions on stage & chat_histories"
                    );

                    // Stale WS `ReactionUpdated` (`pts = 16 < 17`, `ev.chat_id = 0`) must be ignored:
                    let stale_ws_rx = gpui_telegram_proto::sync::UpdateEvent::ReactionUpdated(
                        gpui_telegram_proto::sync::ReactionEvent::new(
                            gpui_telegram_core::types::ChatId(0),
                            gpui_telegram_core::types::MessageId(888),
                            gpui_telegram_core::types::UserId(9001),
                            vec![gpui_telegram_proto::message::ReactionCount::new("👎", 1, true)],
                        ),
                    );
                    s.apply_update_event_with_pts(
                        gpui_telegram_core::types::ChatId(501),
                        Some(16),
                        stale_ws_rx,
                        cx,
                    );
                    assert!(
                        !s.stage
                            .read(cx)
                            .messages
                            .iter()
                            .find(|m| m.id == 888)
                            .unwrap()
                            .reactions
                            .iter()
                            .any(|r| r.emoji == "👎"),
                        "Stale lower-pts WS ReactionUpdated (pts=16 < 17) must not overwrite newer HTTP reaction"
                    );

                    // Newer WS `ReactionUpdated` (`pts = 18 > 17`, `ev.chat_id = 0`) falls back to `ChatId(501)` and applies!
                    let newer_ws_rx = gpui_telegram_proto::sync::UpdateEvent::ReactionUpdated(
                        gpui_telegram_proto::sync::ReactionEvent::new(
                            gpui_telegram_core::types::ChatId(0),
                            gpui_telegram_core::types::MessageId(888),
                            gpui_telegram_core::types::UserId(9001),
                            vec![
                                gpui_telegram_proto::message::ReactionCount::new("🔥", 1, true),
                                gpui_telegram_proto::message::ReactionCount::new("🏆", 1, true),
                            ],
                        ),
                    );
                    s.apply_update_event_with_pts(
                        gpui_telegram_core::types::ChatId(501),
                        Some(18),
                        newer_ws_rx,
                        cx,
                    );
                    assert!(
                        s.stage
                            .read(cx)
                            .messages
                            .iter()
                            .find(|m| m.id == 888)
                            .unwrap()
                            .reactions
                            .iter()
                            .any(|r| r.emoji == "🏆" && r.user_reacted),
                        "Newer WS ReactionUpdated with ev.chat_id==0 must fall back to chat_id=501 and update stage"
                    );

                    // 3c. Rapid multi-emoji clicks (`in_flight > 0`) vs intermediate self-echo WS frame:
                    // User clicks ❤️ on `888` (`in_flight = 1`). Before the legacy HTTP response returns,
                    // an earlier self-echo WS `ReactionUpdated` (`user_id == 9001`, omitting ❤️) arrives.
                    // It MUST NOT clobber the optimistic ❤️ on stage before `apply_http_reaction_response_with_authority(..., None)`!
                    s.stage.update(cx, |st, _| {
                        st.toggle_reaction(888, "❤️");
                    });
                    s.session_sync.in_flight_reactions.insert((501, 888), 1);
                    let intermediate_self_echo = gpui_telegram_proto::sync::UpdateEvent::ReactionUpdated(
                        gpui_telegram_proto::sync::ReactionEvent::new(
                            gpui_telegram_core::types::ChatId(501),
                            gpui_telegram_core::types::MessageId(888),
                            gpui_telegram_core::types::UserId(9001),
                            vec![gpui_telegram_proto::message::ReactionCount::new("🔥", 1, true)],
                        ),
                    );
                    s.apply_update_event_with_pts(
                        gpui_telegram_core::types::ChatId(501),
                        Some(19),
                        intermediate_self_echo,
                        cx,
                    );
                    assert!(
                        s.stage
                            .read(cx)
                            .messages
                            .iter()
                            .find(|m| m.id == 888)
                            .unwrap()
                            .reactions
                            .iter()
                            .any(|r| r.emoji == "❤️" && r.user_reacted),
                        "Intermediate self-echo WS frame while in_flight > 0 must not clobber optimistic ❤️"
                    );
                    s.apply_http_reaction_response_with_authority(501, 888, 888, 20, None, cx);
                    let stored_after_multi = s
                        .client
                        .local_store
                        .get_reactions(
                            gpui_telegram_core::types::ChatId(501),
                            gpui_telegram_core::types::MessageId(888),
                        )
                        .expect("cached reactions after multi-click legacy ack");
                    assert!(
                        stored_after_multi
                            .iter()
                            .any(|r| r.emoji == "❤️" && r.is_chosen_by_me),
                        "Legacy HTTP ack after intermediate self-echo must persist optimistic ❤️ to LocalStore"
                    );

                    // 3d. Background-chat `ev.chat_id == 0` resolution while viewing a DIFFERENT chat (`502`):
                    // Switch active chat to `502`, send WS `ReactionUpdated` (`ev.chat_id = 0`, `msg_id = 888`),
                    // verify stale `pts = 19 < 20` is rejected against `(501, 888)` watermark and newer `pts = 21`
                    // updates `chat_histories[501]` + `LocalStore(501, 888)` without polluting chat `502`!
                    s.stage.update(cx, |st, _| {
                        st.set_chat(502, "Second Chat", "online", "SC", 0x50A8EB);
                        st.add_incoming_message(9999, "Unrelated message in 502");
                    });
                    let bg_stale_ws = gpui_telegram_proto::sync::UpdateEvent::ReactionUpdated(
                        gpui_telegram_proto::sync::ReactionEvent::new(
                            gpui_telegram_core::types::ChatId(0),
                            gpui_telegram_core::types::MessageId(888),
                            gpui_telegram_core::types::UserId(9999),
                            vec![gpui_telegram_proto::message::ReactionCount::new("💩", 1, false)],
                        ),
                    );
                    s.apply_update_event_with_pts(
                        gpui_telegram_core::types::ChatId(502),
                        Some(19),
                        bg_stale_ws,
                        cx,
                    );
                    assert!(
                        !s.stage
                            .read(cx)
                            .chat_histories
                            .get(&501)
                            .unwrap()
                            .iter()
                            .find(|m| m.id == 888)
                            .unwrap()
                            .reactions
                            .iter()
                            .any(|r| r.emoji == "💩"),
                        "Stale WS pts=19 with ev.chat_id=0 while viewing chat 502 must resolve (501, 888) watermark=20 and be rejected"
                    );
                    let bg_newer_ws = gpui_telegram_proto::sync::UpdateEvent::ReactionUpdated(
                        gpui_telegram_proto::sync::ReactionEvent::new(
                            gpui_telegram_core::types::ChatId(0),
                            gpui_telegram_core::types::MessageId(888),
                            gpui_telegram_core::types::UserId(9999),
                            vec![
                                gpui_telegram_proto::message::ReactionCount::new("❤️", 1, false),
                                gpui_telegram_proto::message::ReactionCount::new("🎉", 2, false),
                            ],
                        ),
                    );
                    s.apply_update_event_with_pts(
                        gpui_telegram_core::types::ChatId(502),
                        Some(21),
                        bg_newer_ws,
                        cx,
                    );
                    s.stage.update(cx, |st, _| {
                        st.set_chat(501, "Saved Messages", "online", "SM", 0x2AABEE);
                    });
                    let restored_888 = s
                        .stage
                        .read(cx)
                        .messages
                        .iter()
                        .find(|m| m.id == 888)
                        .expect("restored msg 888 on chat 501");
                    assert!(
                        restored_888.reactions.iter().any(|r| r.emoji == "🎉" && r.count == 2)
                            && restored_888.reactions.iter().any(|r| r.emoji == "❤️" && r.user_reacted),
                        "Newer WS pts=21 with ev.chat_id=0 while viewing chat 502 must update chat 501 and preserve viewer's ❤️"
                    );

                    // 4. Right-click `OpenContextMenu` coordinate conversion (window -> stage-local),
                    // `ToggleExpandReactions`, and `Escape` priority over `ContextInspector`!
                    s.set_inspector_open(true, cx);
                    assert!(s.inspector.read(cx).is_open);
                    s.stage.update(cx, |st, st_cx| {
                        st.set_simulated_stage_bounds(Some(gpui::Bounds {
                            origin: gpui::point(gpui::px(340.0), gpui::px(0.0)),
                            size: gpui::size(gpui::px(590.0), gpui::px(640.0)),
                        }));
                        st.handle_bubble_action(
                            gpui_telegram_desktop::views::stage::stage_bubble::BubbleUiAction::OpenContextMenu(
                                888,
                                gpui::point(gpui::px(860.0), gpui::px(220.0)),
                            ),
                            st_cx,
                        );
                        st.handle_context_action(
                            gpui_telegram_desktop::views::stage::stage_context_menu::ContextMenuAction::ToggleExpandReactions,
                            st_cx,
                        );
                    });
                    let menu = s
                        .stage
                        .read(cx)
                        .active_context_menu
                        .clone()
                        .expect("context menu open");
                    assert_eq!(f32::from(menu.position.x), 520.0);
                    assert_eq!(f32::from(menu.position.y), 220.0);
                    assert!(s.stage.read(cx).context_menu_reactions_expanded);

                    // First `Escape` dismisses the context menu + expanded reaction grid while keeping `ContextInspector` open!
                    assert!(s.handle_non_char_key("escape", false, cx));
                    assert!(s.stage.read(cx).active_context_menu.is_none());
                    assert!(!s.stage.read(cx).context_menu_reactions_expanded);
                    assert!(
                        s.inspector.read(cx).is_open,
                        "First Escape must dismiss active popup before closing ContextInspector"
                    );
                    // Second `Escape` now closes `ContextInspector`!
                    assert!(s.handle_non_char_key("escape", false, cx));
                    assert!(!s.inspector.read(cx).is_open);

                    // 5. Modal popups (`Settings`, `Calls`, `TaskCenter`, `McpTools`) keep `ConversationStage`
                    // active underneath AND dismiss via `Escape` BEFORE closing background `ContextInspector`!
                    s.set_inspector_open(true, cx);
                    s.sidebar.update(cx, |sb, _| sb.is_plus_menu_open = true);
                    s.stage.update(cx, |st, _| st.more_menu_open = true);

                    s.navigate_settings_to(SettingsSectionType::Advanced, cx);
                    assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
                    assert_eq!(s.stage.read(cx).active_chat_id, Some(501));
                    assert!(
                        !s.sidebar.read(cx).is_plus_menu_open && !s.stage.read(cx).more_menu_open,
                        "Opening Settings modal must dismiss floating + and ⋮ popups"
                    );

                    // Trigger Advanced -> PowerSaving via real GPUI `cx.observe(&settings_advanced)`
                    let sa = s.settings_router.update(cx, |r, rcx| r.ensure_advanced(rcx));
                    sa.update(cx, |sa, sa_cx| {
                        sa.pending_open_power_saving = true;
                        sa_cx.notify();
                    });
                });

                shell.update(cx, |s, cx| {
                    assert_eq!(
                        s.settings_router.read(cx).settings_section,
                        SettingsSectionType::PowerSaving,
                        "Advanced PowerSaving row must navigate to SettingsSectionType::PowerSaving"
                    );
                    // Escape steps PowerSaving -> Advanced -> Main -> Chats while keeping ContextInspector open!
                    assert!(s.handle_non_char_key("escape", false, cx));
                    assert_eq!(s.settings_router.read(cx).settings_section, SettingsSectionType::Advanced);
                    assert!(s.inspector.read(cx).is_open);

                    assert!(s.handle_non_char_key("escape", false, cx));
                    assert_eq!(s.settings_router.read(cx).settings_section, SettingsSectionType::Main);
                    assert!(s.inspector.read(cx).is_open);

                    assert!(s.handle_non_char_key("escape", false, cx));
                    assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
                    assert!(
                        s.inspector.read(cx).is_open,
                        "Closing foreground Settings modal with Escape must NOT close background ContextInspector"
                    );

                    // Verify Calls, TaskCenter, and McpTools modals also close on Escape before ContextInspector
                    for dest in [
                        gpui_telegram_desktop::views::nav_rail::NavDestination::Calls,
                        gpui_telegram_desktop::views::nav_rail::NavDestination::TaskCenter,
                        gpui_telegram_desktop::views::nav_rail::NavDestination::McpTools,
                    ] {
                        s.activate_nav_destination(dest, cx);
                        assert_ne!(s.active_panel, ActiveWorkspacePanel::Chats);
                        assert_eq!(s.stage.read(cx).active_chat_id, Some(501));
                        assert!(s.handle_non_char_key("escape", false, cx));
                        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
                        assert!(s.inspector.read(cx).is_open);
                    }

                    // 6. ModalContainer text/IME routing must isolate from CanonicalComposer!
                    s.composer.update(cx, |c, _| c.set_text("Composer draft intact"));
                    s.open_modal_dialog(
                        "New Group",
                        gpui_telegram_desktop::views::ModalDialogKind::NewGroup,
                        420.0,
                        380.0,
                        cx,
                    );
                    s.insert_committed_text("Rust GPUI Group", cx);
                    assert_eq!(s.active_text_and_preedit(cx).0, "Rust GPUI Group");
                    assert_eq!(s.composer.read(cx).text(), "Composer draft intact");
                    s.clear_active_field(cx);
                    assert_eq!(s.active_text_and_preedit(cx).0, "");
                    assert_eq!(
                        s.composer.read(cx).text(),
                        "Composer draft intact",
                        "clear_active_field inside ModalContainer must never clear CanonicalComposer"
                    );
                    assert!(s.handle_non_char_key("escape", false, cx));
                    assert!(!s.modal_container.read(cx).is_modal_active());

                    // 7. SettingsPremium observer activates Premium badge on SettingsMain & SettingsBusiness
                    let sp = s.settings_router.update(cx, |r, rcx| r.ensure_premium(rcx));
                    sp.update(cx, |sp, sp_cx| {
                        sp.subscribe(12);
                        sp_cx.notify();
                    });
                });

                shell.update(cx, |s, cx| {
                    let sm = s.settings_router.update(cx, |r, rcx| r.ensure_main(rcx));
                    assert!(sm.read(cx).profile.is_premium);
                    let sb = s.settings_router.update(cx, |r, rcx| r.ensure_business(rcx));
                    assert!(sb.read(cx).business_active);
                });

                println!("GPUI_HEADLESS_APP_SHELL_OK");
                std::process::exit(0);
            });

        std::process::exit(1);
    }
}
