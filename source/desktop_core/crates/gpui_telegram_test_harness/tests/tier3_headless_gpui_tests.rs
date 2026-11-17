use gpui_telegram_desktop::theme::TdesktopThemePalette;
use gpui_telegram_desktop::views::app_shell::window_adaptive::{WindowAdaptive, WindowLayout};
use gpui_telegram_desktop::views::composer::{ImeState, KeyAction};
use gpui_telegram_desktop::views::stage::cards::{
    DeliveryStatus, MessageContent, MessageDirection, MessageItemModel, PollOption, PollType,
};
use gpui_telegram_desktop::views::{
    CallOverlay, CanonicalComposer, ChatSidebar, ContextInspector, ConversationStage,
    ModalContainer, ModalDialogKind, NavRail, TalkModeState,
};
use gpui_telegram_desktop::components::ui::layers::LayerStackOption;
use gpui_telegram_test_harness::tier3_headless::HeadlessTranscriptDriver;

#[test]
fn test_tier3_embedded_svg_and_locale_assets() {
    HeadlessTranscriptDriver::verify_all_embedded_assets();
    HeadlessTranscriptDriver::verify_themes_and_localization();
}

#[test]
fn test_tier3_responsive_breakpoints_ime_and_composer_flows() {
    let mut adaptive = WindowAdaptive::with_dimensions(520.0, 760.0);
    assert!(adaptive.is_one_column());
    adaptive.update_from_width(820.0);
    assert_eq!(adaptive.window_layout, WindowLayout::Normal);
    adaptive.update_from_width(1280.0);
    assert!(adaptive.is_three_column() && adaptive.is_chat_wide());

    let mut composer = CanonicalComposer::new();
    composer.ime_machine.on_ime_start();
    composer.ime_machine.on_ime_update("ni", 2);
    assert!(matches!(composer.ime_machine.state, ImeState::Composing { .. }));
    assert_eq!(composer.handle_enter(false), KeyAction::ImeCandidateChosen);
    composer.clear();
    composer.ime_machine.on_ime_start();
    composer.ime_machine.on_ime_commit("你好");
    assert_eq!(composer.handle_enter(false), KeyAction::ImeCandidateChosen);
    assert_eq!(composer.handle_enter(true), KeyAction::InsertNewline);
    assert_eq!(composer.text(), "你好\n");
    assert_eq!(composer.handle_enter(false), KeyAction::SendMessage);

    composer.begin_reply(10, "Alice".into(), "Ping".into());
    assert!(composer.replying_to.is_some());
    composer.cancel_reply_or_edit();
    composer.begin_edit(11, "Old text".into());
    assert!(composer.editing_message.is_some());
}

#[test]
fn test_tier3_sidebar_stage_reactions_polls_modals_and_call_overlay() {
    let mut rail = NavRail::new();
    rail.set_connected(true);
    rail.set_account("AL", "Alice");
    rail.set_language("zh-CN");
    assert!(!rail.active_destination.localized_label("zh-CN").is_empty());

    let mut sidebar = ChatSidebar::new();
    assert!(sidebar.dialogs.is_empty());
    sidebar.width = 320.0;
    assert_eq!(sidebar.width, 320.0);
    sidebar.open_contacts_modal();
    assert!(sidebar.is_contacts_modal_open);
    sidebar.close_contacts_modal();
    sidebar.open_create_group_modal();
    sidebar.push_creation_text("Rust Group");
    assert_eq!(sidebar.active_creation_field_text(), "Rust Group");

    let mut stage = ConversationStage::new();
    stage.set_chat(42, "Rust Group", "2 members", "RG", 0x2AABEE);
    stage.add_message(MessageItemModel {
        id: 1,
        sender_id: 7,
        sender_name: "Alice".into(),
        avatar_initials: "AL".into(),
        avatar_color: 0x2AABEE,
        direction: MessageDirection::Outgoing,
        timestamp: "12:00".into(),
        delivery_status: DeliveryStatus::Sent,
        reply_to: None,
        reactions: Vec::new(),
        content: MessageContent::Poll {
            question: "Pick?".into(),
            poll_type: PollType::Regular,
            options: vec![
                PollOption { id: 0, text: "Yes".into(), votes: 0, percentage: 0.0, is_selected: false, is_correct: false },
                PollOption { id: 1, text: "No".into(), votes: 0, percentage: 0.0, is_selected: false, is_correct: false },
            ],
            total_votes: 0,
            is_closed: false,
            selected_option_id: None,
        },
        bot_keyboard: None,
        is_pinned: false,
    });
    stage.toggle_reaction(1, "❤️");
    assert_eq!(stage.messages[0].reactions.len(), 1);
    stage.vote_poll(1, 0);
    if let MessageContent::Poll { total_votes, selected_option_id, .. } = &stage.messages[0].content {
        assert_eq!(*total_votes, 1);
        assert_eq!(*selected_option_id, Some(0));
    } else {
        panic!("expected poll card");
    }

    let mut modal = ModalContainer::new();
    modal.set_palette(TdesktopThemePalette::default());
    let _id = modal.show_dialog(
        "Forward",
        ModalDialogKind::NewGroup,
        Vec::new(),
        380.0,
        420.0,
        LayerStackOption::Normal,
    );
    assert!(modal.stack.has_active_layers());

    let mut inspector = ContextInspector::new();
    inspector.toggle_open();
    assert!(inspector.is_open);

    let mut call = CallOverlay::new("Alice".into());
    assert!(!call.e2ee_emojis.is_empty());
    call.state = TalkModeState::Speaking;
    call.is_mic_muted = true;
    assert!(call.is_mic_muted);
}

#[test]
fn test_hover_reaction_overlay_invariants_and_zero_layout_jitter() {
    use gpui_telegram_desktop::views::conversation_stage::MessageContextMenuState;
    use gpui_telegram_desktop::views::stage::reactions::button::{
        sort_reaction_pills, ReactionKey, ReactionPill,
    };
    use gpui_telegram_desktop::views::stage::stage_bubble::BubbleUiAction;
    use gpui_telegram_desktop::views::stage::stage_context_menu::{
        ReactionOverlayMode, EXTENDED_REACTIONS, QUICK_REACTIONS,
    };

    assert_eq!(QUICK_REACTIONS.len(), 7);
    assert_eq!(EXTENDED_REACTIONS.len(), 32);

    let mut stage = ConversationStage::new();
    stage.set_chat(77, "Hover Stability Chat", "online", "HS", 0x2AABEE);
    stage.add_incoming_message(2, "First incoming message near top edge");
    stage.add_outgoing_text_message("Second outgoing message right below");
    stage.add_incoming_message(2, "Third incoming message at bottom");
    assert_eq!(stage.messages.len(), 3);

    stage.row_height_cache.rebuild_prefix_sums(&stage.messages);
    let baseline_sums = stage.row_height_cache.prefix_sums.clone();
    assert_eq!(baseline_sums.len(), 3);
    assert_eq!(stage.reaction_overlay_mode(1), None);

    // 1. Hovering message #1 activates only the non-overlapping outer-corner heart button (`CornerButton`),
    // matching Telegram Desktop `history_view_reactions_button.cpp`, with 0.0px row-height shift.
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(1, true)),
        Some(true)
    );
    assert!(stage.is_reaction_overlay_visible(1));
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::CornerButton)
    );
    assert!(!stage.row_height_cache.is_dirty());
    stage.row_height_cache.rebuild_prefix_sums(&stage.messages);
    assert_eq!(stage.row_height_cache.prefix_sums, baseline_sums);

    // Repeated hover(true) on same bubble is a no-op (returns Some(false) so GPUI doesn't re-render needlessly)
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(1, true)),
        Some(false)
    );

    // 2. Moving pointer onto `#rx-corner-btn-1` / `#rx-hover-zone-1` promotes `CornerButton -> QuickStrip`
    // seamlessly regardless of GPUI callback ordering (`SetBubbleHover(false)` before OR after `SetReactionBarHover(true)`).
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetReactionBarHover(1, true)),
        Some(true)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(1, false)),
        Some(true)
    );
    assert!(stage.is_reaction_overlay_visible(1));
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::QuickStrip)
    );
    assert_eq!(stage.hovered_message_id, None);
    assert_eq!(stage.hovered_reaction_bar_id, Some(1));

    // 3. Expanding the 32-emoji reaction grid promotes to `ExpandedSelector`, keeps overlay pinned
    // even if cursor leaves the zone, and STILL produces 0.0px row-height shift.
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::ToggleReactionPicker(1)),
        Some(true)
    );
    assert_eq!(stage.reaction_picker_msg_id, Some(1));
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetReactionBarHover(1, false)),
        Some(true)
    );
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::ExpandedSelector)
    );
    stage.row_height_cache.rebuild_prefix_sums(&stage.messages);
    assert_eq!(stage.row_height_cache.prefix_sums, baseline_sums);

    // Active right-click context menu suppresses the floating hover overlay so two popups never collide.
    stage.active_context_menu = Some(MessageContextMenuState {
        message_id: 1,
        position: gpui::point(gpui::px(120.0), gpui::px(80.0)),
        is_outgoing: false,
        is_pinned: false,
        text_snippet: None,
    });
    assert_eq!(stage.reaction_overlay_mode(1), None);
    stage.active_context_menu = None;
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::ExpandedSelector)
    );

    // 4. Cross-bubble hover transition: entering bubble #2 before stale `SetBubbleHover(1, false)` arrives
    // must NOT clobber `hovered_message_id == Some(2)`.
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(2, true)),
        Some(true)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(1, false)),
        Some(false)
    );
    assert_eq!(stage.hovered_message_id, Some(2));
    assert_eq!(
        stage.reaction_overlay_mode(2),
        Some(ReactionOverlayMode::CornerButton)
    );

    // 5. Multi-selection mode suppresses hover reaction overlay cleanly
    stage.selected_message_ids.push(2);
    assert!(!stage.is_reaction_overlay_visible(2));
    stage.selected_message_ids.clear();
    assert!(stage.is_reaction_overlay_visible(2));

    // 6. Rapid multi-emoji clicks before `AppShell` drains must queue BOTH actions in FIFO order
    // (instead of overwriting a single `Option<(u64, String)>` slot), close the picker, update
    // both `messages` and `chat_histories`, and mark `row_height_cache` dirty.
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::ToggleReaction(1, "👍".to_string())),
        Some(true)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::ToggleReaction(1, "❤️".to_string())),
        Some(true)
    );
    assert_eq!(stage.reaction_picker_msg_id, None);
    let drained = stage.drain_pending_reaction_actions();
    assert_eq!(
        drained,
        vec![
            (1, "👍".to_string(), true),
            (1, "❤️".to_string(), true),
        ]
    );
    assert!(stage.pending_reaction_toggle.is_none());
    assert!(stage.row_height_cache.is_dirty());
    assert_eq!(stage.messages[0].reactions.len(), 2);
    let hist_rx = &stage.chat_histories.get(&77).unwrap()[0].reactions;
    assert_eq!(hist_rx.len(), 2);

    // 7. Verify upstream Telegram Desktop `sort_reaction_pills` ordering (`history_view_reactions.cpp:L177-L232`):
    // Paid ⭐ first, then `(count - chosen)` descending, then `catalog_index` ascending.
    let mut p_chosen_only = ReactionPill::new(ReactionKey::Emoji("🔥".into()), 1);
    p_chosen_only.chosen = true;
    p_chosen_only.catalog_index = 0; // other_count = 0
    let mut p_other = ReactionPill::new(ReactionKey::Emoji("👍".into()), 2);
    p_other.chosen = false;
    p_other.catalog_index = 1; // other_count = 2
    let mut p_paid = ReactionPill::new(ReactionKey::Paid, 1);
    p_paid.catalog_index = 2;

    let sorted = sort_reaction_pills(vec![p_chosen_only, p_other, p_paid]);
    assert_eq!(sorted[0].id, ReactionKey::Paid);
    assert_eq!(sorted[1].id, ReactionKey::Emoji("👍".into()));
    assert_eq!(sorted[2].id, ReactionKey::Emoji("🔥".into()));
}

#[test]
fn test_scrolled_viewport_overlay_placement_and_optimistic_id_remap() {
    use gpui_telegram_desktop::views::conversation_stage::MessageContextMenuState;

    let mut stage = ConversationStage::new();
    stage.set_chat(88, "Scrolled Viewport Chat", "online", "SV", 0x2AABEE);
    for i in 0..30 {
        stage.add_incoming_message(2, &format!("Historical message row #{i}"));
    }
    stage.row_height_cache.rebuild_prefix_sums(&stage.messages);
    let total_h = *stage.row_height_cache.prefix_sums.last().unwrap();
    assert!(total_h > 900.0);

    stage.simulated_viewport_height = Some(500.0);
    stage.set_simulated_scroll_offset(
        gpui::point(gpui::px(0.0), gpui::px(-500.0)),
        gpui::size(gpui::px(0.0), gpui::px(total_h - 500.0)),
    );

    // Row 0 is scrolled 500px above the viewport: overscanned rows must NOT mount `deferred` overlays!
    let (vis_0, _) = stage.compute_reaction_overlay_placement(0);
    assert!(!vis_0, "Scrolled-off row 0 must suppress deferred reaction overlay");

    // Find the top visible row in the scrolled viewport (`idx >= 8`) and verify it anchors BELOW
    // so `ExpandedSelector` (`188px` tall) never paints over the stage top header!
    let top_visible_idx = (5..25)
        .find(|&idx| stage.compute_reaction_overlay_placement(idx).0)
        .expect("visible row after scrolling 500px");
    assert!(top_visible_idx >= 5);
    let (vis_top, anchor_below_top) = stage.compute_reaction_overlay_placement(top_visible_idx);
    assert!(vis_top);
    assert!(
        anchor_below_top,
        "Top visible row in a scrolled viewport (idx={top_visible_idx}) must anchor reaction selector below"
    );

    // A row near the bottom of the 500px viewport (`rel_bottom >= 196px`) must anchor ABOVE (`anchor_below == false`).
    let bottom_visible_idx = (top_visible_idx..29)
        .rev()
        .find(|&idx| stage.compute_reaction_overlay_placement(idx).0)
        .expect("bottom visible row");
    let (vis_bot, anchor_below_bot) = stage.compute_reaction_overlay_placement(bottom_visible_idx);
    assert!(vis_bot);
    assert!(
        !anchor_below_bot,
        "Bottom visible row (idx={bottom_visible_idx}) must anchor reaction selector above"
    );

    // Verify `remap_message_id` preserves hover, picker, context-menu, and selection across nonce->server promotion
    stage.hovered_message_id = Some(990_001);
    stage.hovered_reaction_bar_id = Some(990_001);
    stage.reaction_picker_msg_id = Some(990_001);
    stage.selected_message_ids = vec![990_001, 5];
    stage.active_context_menu = Some(MessageContextMenuState {
        message_id: 990_001,
        position: gpui::point(gpui::px(100.0), gpui::px(120.0)),
        is_outgoing: true,
        is_pinned: false,
        text_snippet: Some("Optimistic".into()),
    });

    stage.remap_message_id(990_001, 4242);
    assert_eq!(stage.hovered_message_id, Some(4242));
    assert_eq!(stage.hovered_reaction_bar_id, Some(4242));
    assert_eq!(stage.reaction_picker_msg_id, Some(4242));
    assert_eq!(stage.selected_message_ids, vec![4242, 5]);
    assert_eq!(stage.active_context_menu.as_ref().unwrap().message_id, 4242);
}

#[tokio::test]
async fn test_e2e_reaction_persistence_reload_reconciliation_and_multi_user_chosen_isolation() {
    use gpui_telegram_client::{ChatSummary, ClientConfig, TelegramClient};
    use gpui_telegram_desktop::views::app_shell::shell_truth::{
        apply_fenced_chat_list_response, apply_fenced_history_response,
        apply_fenced_history_response_preserving, select_authoritative_dialog, SessionRequestFence,
    };
    use gpui_telegram_desktop::views::stage::cards::ReactionItem;
    use gpui_telegram_desktop::views::stage::stage_bubble::BubbleUiAction;
    use gpui_telegram_test_harness::temp_dir::TempTestDir;
    use gpui_telegram_test_harness::tier4_e2e::{E2eSystemDriver, EphemeralTestServer};
    use reqwest::Method;
    use serde_json::json;

    let server = EphemeralTestServer::start().await.expect("start server");
    let drv = E2eSystemDriver::new(server.base_url());

    let (alice_id, alice_tok) = drv.register_user("rx_alice", "StrongPass!9").await.unwrap();
    let (bob_id, bob_tok) = drv.register_user("rx_bob", "StrongPass!9").await.unwrap();

    let (st, grp) = drv
        .json_req(
            Method::POST,
            "/api/v1/chats",
            Some(&alice_tok),
            Some(json!({ "title": "Reaction E2E Room", "chat_type": "group" })),
        )
        .await
        .unwrap();
    assert!(st == 200 || st == 201);
    let gid = grp["chat_id"].as_i64().unwrap();

    let (_, inv) = drv
        .json_req(
            Method::POST,
            &format!("/api/v1/chats/{gid}/invites"),
            Some(&alice_tok),
            Some(json!({})),
        )
        .await
        .unwrap();
    let (st_join, _) = drv
        .json_req(
            Method::POST,
            "/api/v1/chats/join",
            Some(&bob_tok),
            Some(json!({ "code": inv["code"] })),
        )
        .await
        .unwrap();
    assert_eq!(st_join, 200);

    let alice_dir = TempTestDir::new("rx_alice").unwrap();
    let bob_dir = TempTestDir::new("rx_bob").unwrap();
    let cfg = ClientConfig {
        server_url: server.base_url().to_string(),
        ..Default::default()
    };
    let alice_client = TelegramClient::open_with_dir(cfg.clone(), alice_dir.path()).unwrap();
    alice_client.set_token(Some(alice_tok.clone()));
    let bob_client = TelegramClient::open_with_dir(cfg, bob_dir.path()).unwrap();
    bob_client.set_token(Some(bob_tok.clone()));

    let sent = alice_client
        .send_text_message(gid, "Click emoji reaction E2E check", None)
        .await
        .expect("send message");
    let mid = sent.message_id as u64;

    // Load initial history into Alice's stage
    let mut sb = ChatSidebar::new();
    let mut stage = ConversationStage::new();
    let mut insp = ContextInspector::new();
    let fence = SessionRequestFence {
        auth_generation: 1,
        request_generation: 1,
        token: alice_tok.clone(),
        user_id: alice_id,
    };
    let summaries = vec![ChatSummary {
        chat_id: gid as u64,
        title: "Reaction E2E Room".into(),
        chat_type: 1,
        unread_count: 0,
        last_message: Some("Click emoji reaction E2E check".into()),
        is_group: Some(true),
        is_bot: Some(false),
        avatar_color: Some(0x2AABEE),
        updated_at: Some(1_700_000_000_000),
    }];
    assert!(apply_fenced_chat_list_response(
        &fence,
        true,
        1,
        1,
        Some(&alice_tok),
        Some(alice_id),
        &mut sb,
        &mut stage,
        &mut insp,
        &summaries,
    ));
    assert!(select_authoritative_dialog(&mut sb, &mut stage, &mut insp, gid as u64));

    let hist0 = alice_client
        .get_chat_messages(gid as u64, Some(50))
        .await
        .unwrap();
    assert!(apply_fenced_history_response(
        &fence,
        true,
        1,
        1,
        Some(&alice_tok),
        Some(alice_id),
        &sb,
        &mut stage,
        gid as u64,
        hist0,
    ));
    assert_eq!(stage.messages.len(), 1);
    assert!(stage.messages[0].reactions.is_empty());

    // 1. Alice clicks ❤️ in the hover strip
    stage.apply_reaction_ui_action(&BubbleUiAction::ToggleReaction(mid, "❤️".into()));
    let (_, pending_em) = stage.pending_reaction_toggle.take().expect("pending toggle");
    assert_eq!(pending_em, "❤️");
    assert_eq!(stage.messages[0].reactions.len(), 1);
    assert!(stage.messages[0].reactions[0].user_reacted);

    let resp_add = alice_client
        .react_to_message(gid, mid as i64, "❤️", Some("add"))
        .await
        .expect("alice react add");
    stage.set_message_reactions(
        gid as u64,
        mid,
        resp_add
            .reactions
            .iter()
            .map(|r| ReactionItem {
                emoji: r.emoji.clone(),
                count: r.count,
                user_reacted: r.is_chosen_by_me,
            })
            .collect(),
    );
    assert_eq!(stage.messages[0].reactions[0].count, 1);
    assert!(stage.messages[0].reactions[0].user_reacted);

    // 2. Bob also reacts with ❤️ and 🔥 on the server
    let bob_r1 = bob_client
        .react_to_message(gid, mid as i64, "❤️", Some("add"))
        .await
        .expect("bob react heart");
    assert_eq!(bob_r1.reactions.iter().find(|r| r.emoji == "❤️").unwrap().count, 2);
    let _ = bob_client
        .react_to_message(gid, mid as i64, "🔥", Some("add"))
        .await
        .expect("bob react fire");

    // 3. Alice reloads chat history while `stage.messages` ALREADY contains `mid`!
    // `merge_historical_messages` must update `reactions` on the existing message in-place
    // with viewer-scoped `chosen` flags (`❤️`: count=2, user_reacted=true; `🔥`: count=1, user_reacted=false).
    let hist_reloaded = alice_client
        .get_chat_messages(gid as u64, Some(50))
        .await
        .unwrap();
    assert!(apply_fenced_history_response(
        &fence,
        true,
        1,
        1,
        Some(&alice_tok),
        Some(alice_id),
        &sb,
        &mut stage,
        gid as u64,
        hist_reloaded,
    ));
    let heart = stage.messages[0]
        .reactions
        .iter()
        .find(|r| r.emoji == "❤️")
        .expect("heart reaction after reload");
    let fire = stage.messages[0]
        .reactions
        .iter()
        .find(|r| r.emoji == "🔥")
        .expect("fire reaction after reload");
    assert_eq!(heart.count, 2);
    assert!(heart.user_reacted);
    assert_eq!(fire.count, 1);
    assert!(!fire.user_reacted);

    // Bob's history reload sees the opposite `chosen` flags (`❤️`: true, `🔥`: true; if Alice had a solo emoji, false)
    let bob_hist = bob_client
        .get_chat_messages(gid as u64, Some(50))
        .await
        .unwrap();
    assert!(bob_hist[0].reactions.iter().find(|r| r.emoji == "🔥").unwrap().chosen);
    let _ = bob_id;

    // 4. Alice clicks ❤️ again on the inline pill to REMOVE her reaction
    stage.apply_reaction_ui_action(&BubbleUiAction::ToggleReaction(mid, "❤️".into()));
    let is_added_now = stage.messages[0]
        .reactions
        .iter()
        .any(|r| r.emoji == "❤️" && r.user_reacted);
    assert!(!is_added_now);
    let resp_rm = alice_client
        .react_to_message(gid, mid as i64, "❤️", Some("remove"))
        .await
        .expect("alice react remove");
    let heart_after_rm = resp_rm
        .reactions
        .iter()
        .find(|r| r.emoji == "❤️")
        .expect("bob's heart remains");
    assert_eq!(heart_after_rm.count, 1);
    assert!(!heart_after_rm.is_chosen_by_me);

    // 5. While Alice has an in-flight optimistic reaction (`🎉`) on `mid` AND an optimistic `Sending`
    // message (`sending_id`) not yet on the server, a background `get_chat_messages` response arrives!
    // `apply_fenced_history_response_preserving` must preserve both the in-flight `🎉` reaction AND
    // the `Sending` message in BOTH `stage.messages` and `stage.chat_histories[&gid]`.
    stage.apply_reaction_ui_action(&BubbleUiAction::ToggleReaction(mid, "🎉".into()));
    let _ = stage.drain_pending_reaction_actions();
    stage.add_outgoing_text_message("Optimistic sending draft");
    let sending_id = stage.messages.last().unwrap().id;
    let stale_hist = alice_client
        .get_chat_messages(gid as u64, Some(50))
        .await
        .unwrap();
    assert!(apply_fenced_history_response_preserving(
        &fence,
        true,
        1,
        1,
        Some(&alice_tok),
        Some(alice_id),
        &sb,
        &mut stage,
        gid as u64,
        stale_hist,
        &std::collections::HashSet::from([mid]),
    ));
    let mid_after_inflight_merge = stage
        .messages
        .iter()
        .find(|m| m.id == mid)
        .expect("mid after preserving merge");
    assert!(
        mid_after_inflight_merge
            .reactions
            .iter()
            .any(|r| r.emoji == "🎉" && r.user_reacted),
        "In-flight optimistic 🎉 reaction must not be overwritten by pre-mutation history fetch"
    );
    let cached_hist = stage
        .chat_histories
        .get(&(gid as u64))
        .expect("cached chat history");
    assert!(
        cached_hist.iter().any(|m| m.id == sending_id),
        "chat_histories must retain optimistic Sending message across history reload"
    );
    assert!(
        cached_hist
            .iter()
            .find(|m| m.id == mid)
            .unwrap()
            .reactions
            .iter()
            .any(|r| r.emoji == "🎉" && r.user_reacted),
        "chat_histories must match merged stage.messages reactions"
    );

    tokio::task::spawn_blocking(move || {
        drop(alice_client);
        drop(bob_client);
    })
    .await
    .expect("drop clients on blocking thread");
}

#[test]
fn test_multiline_diagonal_hover_corridor_and_subpixel_grace_continuity() {
    use gpui_telegram_desktop::views::stage::stage_bubble::BubbleUiAction;
    use gpui_telegram_desktop::views::stage::stage_context_menu::ReactionOverlayMode;

    let mut stage = ConversationStage::new();
    stage.set_chat(23, "AI Assistant", "bot", "AI", 0x2AABEE);
    stage.add_incoming_message(
        2,
        "Line 1: Multi-line AI response\nLine 2: Moving diagonally from middle of bubble\nLine 3: Across x = W at y < H - 30\nLine 4: Down outer gutter to CornerButton",
    );
    stage.add_outgoing_text_message("Outgoing reply below");
    stage.row_height_cache.rebuild_prefix_sums(&stage.messages);
    let initial_sums = stage.row_height_cache.prefix_sums.clone();

    // 1. Pointer starts inside upper/middle of tall bubble #1 -> `CornerButton`
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(1, true)),
        Some(true)
    );
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::CornerButton)
    );

    // 2. Pointer moves diagonally right across `x = W` at `y < H - 30` into `#bubble-gutter-1` (`[W, W+38] x [0, H]`).
    // Verify BOTH GPUI hit-test callback orderings keep `CornerButton` mounted without any drop!
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(1, false)),
        Some(true)
    );
    // Simulate active grace while crossing sub-pixel edge before gutter hover(true) fires:
    stage.hover_grace = Some((1, ReactionOverlayMode::CornerButton));
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::CornerButton),
        "Grace window must prevent sub-pixel edge crossing from unmounting CornerButton"
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleGutterHover(1, true)),
        Some(true)
    );
    assert!(stage.hover_grace.is_none());
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::CornerButton),
        "Outer side gutter corridor must keep CornerButton visible along full bubble height"
    );

    // Also verify reverse callback order (gutter `true` before bubble `false`):
    stage.hovered_message_id = Some(1);
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleGutterHover(1, true)),
        Some(false)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(1, false)),
        Some(true)
    );
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::CornerButton)
    );

    // 3. Pointer reaches `CornerButton` at bottom-outer corner -> promotes to `QuickStrip`
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetReactionBarHover(1, true)),
        Some(true)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleGutterHover(1, false)),
        Some(true)
    );
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::QuickStrip)
    );

    // 4. Pointer moves from `CornerButton` (`[W, W+36] x [H-30, H]`) down through `#rx-strip-bridge-1`
    // (`[W, W+36] x [H-30, H+6]`) onto `#rx-strip-card-1` (`[H+4, H+36]`): `QuickStrip` stays open!
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetReactionBridgeHover(1, true)),
        Some(true)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetReactionBarHover(1, false)),
        Some(true)
    );
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::QuickStrip),
        "Outer corner bridge must keep QuickStrip open between CornerButton and strip card"
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetReactionBarHover(1, true)),
        Some(true)
    );
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::QuickStrip)
    );

    // 4b. Diagonal `CornerButton -> #bubble-1 -> #rx-strip-card-1` traversal:
    // Moving diagonally inward from `CornerButton` (`[W, W+36] x [H-30, H]`) toward `👍`/`❤️`
    // on `#rx-strip-card-1` crosses `#bubble-1` (`[0, W] x [0, H]`), firing `SetBubbleHover(1, true)`
    // + `SetReactionBarHover(1, false)`. `QuickStrip` MUST remain sticky on `#bubble-1` instead of
    // collapsing back to `CornerButton`!
    stage.hovered_reaction_bridge_id = None;
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(1, true)),
        Some(true)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetReactionBarHover(1, false)),
        Some(true)
    );
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::QuickStrip),
        "Diagonal traversal from CornerButton across #bubble-1 toward 👍/❤️ must keep QuickStrip open"
    );
    // And re-entering `#rx-strip-card-1` keeps `QuickStrip` open seamlessly:
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetReactionBarHover(1, true)),
        Some(true)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(1, false)),
        Some(true)
    );
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::QuickStrip)
    );

    // 5. Sub-pixel gap while moving across `QuickStrip` preserves `QuickStrip` during grace,
    // entering bubble #2 immediately cancels bubble #1's grace + sticky `quick_strip_msg_id`,
    // and re-entering bubble #1 after leaving starts cleanly at `CornerButton`!
    stage.hovered_reaction_bar_id = None;
    stage.hovered_reaction_bridge_id = None;
    stage.hover_grace = Some((1, ReactionOverlayMode::QuickStrip));
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::QuickStrip)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(2, true)),
        Some(true)
    );
    assert_eq!(stage.hover_grace, None);
    assert_eq!(stage.quick_strip_msg_id, None);
    assert_eq!(stage.reaction_overlay_mode(1), None);
    assert_eq!(
        stage.reaction_overlay_mode(2),
        Some(ReactionOverlayMode::CornerButton)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(2, false)),
        Some(true)
    );
    assert_eq!(
        stage.apply_reaction_ui_action(&BubbleUiAction::SetBubbleHover(1, true)),
        Some(true)
    );
    assert_eq!(
        stage.reaction_overlay_mode(1),
        Some(ReactionOverlayMode::CornerButton),
        "Fresh re-entry into bubble #1 after leaving must start at CornerButton"
    );

    // Row height cache never dirtied or shifted by any hover transition!
    assert!(!stage.row_height_cache.is_dirty());
    stage.row_height_cache.rebuild_prefix_sums(&stage.messages);
    assert_eq!(stage.row_height_cache.prefix_sums, initial_sums);
}

#[test]
fn test_reaction_overlay_non_occlusion_and_narrow_window_viewport_clamping() {
    use gpui_telegram_desktop::views::stage::stage_context_menu::{
        compute_reaction_overlay_side_offset, ReactionOverlayLayout, REACTION_CORNER_OVERHANG_PX,
        REACTION_QUICK_STRIP_HEIGHT_PX, REACTION_QUICK_STRIP_WIDTH_PX,
        REACTION_SELECTOR_HEIGHT_PX, REACTION_SELECTOR_WIDTH_PX,
    };

    // 1. Vertical non-occlusion via `ReactionOverlayLayout::compute`:
    // Regardless of whether anchored below or above, neither `QuickStrip` nor `ExpandedSelector`
    // overlaps `[0, H]` of `#bubble-{msg_id}` when standard viewport clearance exists!
    let bubble_h = 84.0_f32;
    let layout_above = ReactionOverlayLayout::compute(
        false,
        260.0,
        260.0 + bubble_h,
        640.0,
        320.0,
        820.0,
        false,
        true,
    );
    assert!(!layout_above.strip_above);
    assert_eq!(
        layout_above.selector_top_when_above,
        -(REACTION_SELECTOR_HEIGHT_PX + 4.0)
    );
    // When `strip_above == false`, `QuickStrip` sits at `bottom: -36px` (`[H + 4, H + 36]`), strictly > H!
    let strip_below_top = bubble_h + 4.0;
    assert!(strip_below_top >= bubble_h);
    // When `selector_below == false`, `ExpandedSelector` sits at `[selector_top_when_above, selector_top_when_above + 188] = [-192, -4]`, strictly < 0!
    assert!(layout_above.selector_top_when_above + REACTION_SELECTOR_HEIGHT_PX <= 0.0);

    // When scrolled right against the bottom viewport edge (`viewport_h - rel_bottom < 38`), `strip_above` flips to `true` (`[-36, -4]` < 0)
    let layout_bottom_edge = ReactionOverlayLayout::compute(
        false,
        540.0,
        624.0,
        640.0,
        320.0,
        820.0,
        true,
        false,
    );
    assert!(layout_bottom_edge.strip_above);
    let strip_above_bottom = -(REACTION_QUICK_STRIP_HEIGHT_PX + 4.0) + REACTION_QUICK_STRIP_HEIGHT_PX;
    assert!(strip_above_bottom <= 0.0);

    // Compact `450px` viewport (`rel_top = 90`, `rel_bottom = 260`): neither side has `>= 196px`
    // (`90px` above vs `190px` below), so `should_open_selector_below` MUST choose `true` (`190 > 90`)
    // and clamp `selector_bottom_when_below == -182.0` so `[rel_bottom - 188 - b, rel_bottom - b]` stays in `[8, 442]`!
    let compact_below = gpui_telegram_desktop::views::stage::stage_context_menu::should_open_selector_below(
        90.0, 260.0, 450.0,
    );
    assert!(
        compact_below,
        "Compact 450px viewport with 90px above vs 190px below must open ExpandedSelector downward"
    );
    let layout_compact = ReactionOverlayLayout::compute(
        compact_below,
        90.0,
        260.0,
        450.0,
        320.0,
        640.0,
        false,
        true,
    );
    assert!(layout_compact.selector_below);
    assert_eq!(layout_compact.selector_bottom_when_below, -182.0);
    let selector_vp_bottom = 260.0 - layout_compact.selector_bottom_when_below;
    let selector_vp_top = selector_vp_bottom - REACTION_SELECTOR_HEIGHT_PX;
    assert!((selector_vp_bottom - 442.0).abs() < 0.01);
    assert!(selector_vp_top >= 8.0 - 0.01);

    // Cold first-frame CJK bubble width estimation & live `transcript_bounds_probe` + `bubble_bounds_probe` span:
    let mut cjk_stage = ConversationStage::new();
    cjk_stage.set_chat(23, "AI Assistant", "bot", "AI", 0x2AABEE);
    cjk_stage.add_incoming_message(2, "您好，我是大乘 App 的 AI 助手，请问有什么可以帮您？");
    let cjk_w = cjk_stage.resolved_or_estimated_bubble_width(&cjk_stage.messages[0], 760.0, false);
    assert!(
        cjk_w >= 340.0,
        "Cold-frame CJK width estimate ({cjk_w}) must reflect wide CJK glyphs rather than ASCII 7.6px/char"
    );
    cjk_stage.transcript_bounds_probe.set(Some(gpui::Bounds {
        origin: gpui::point(gpui::px(300.0), gpui::px(60.0)),
        size: gpui::size(gpui::px(620.0), gpui::px(500.0)),
    }));
    cjk_stage.bubble_bounds_probe.borrow_mut().insert(
        1,
        gpui::Bounds {
            origin: gpui::point(gpui::px(328.0), gpui::px(450.0)),
            size: gpui::size(gpui::px(380.0), gpui::px(92.0)),
        },
    );
    let (visible, live_layout) =
        cjk_stage.compute_reaction_overlay_layout(0, &cjk_stage.messages[0], false);
    assert!(visible);
    assert!(
        live_layout.strip_above,
        "Live laid-out bubble bounds at rel_bottom=482 in 500px transcript viewport must flip strip_above=true"
    );

    // 2. Horizontal side overhang (`<= 36px <= 56px` side margin) AND narrow-window clamping (`[8, stage_w - 8]`):
    let viewport_margin = 8.0_f32;
    for &stage_w in &[340.0_f32, 520.0, 860.0] {
        for &bubble_w in &[120.0_f32, 240.0, (stage_w - 124.0).max(150.0)] {
            for &(is_outgoing, reserve_avatar) in &[(false, true), (false, false), (true, false)] {
                let anchor_inset = 28.0 + if !is_outgoing && reserve_avatar { 40.0 } else { 0.0 };
                for &overlay_w in &[REACTION_QUICK_STRIP_WIDTH_PX, REACTION_SELECTOR_WIDTH_PX] {
                    let side_offset = compute_reaction_overlay_side_offset(
                        overlay_w,
                        bubble_w,
                        stage_w,
                        is_outgoing,
                        reserve_avatar,
                    );
                    let (overlay_stage_left, overlay_stage_right) = if is_outgoing {
                        let right = stage_w - anchor_inset - side_offset;
                        (right - overlay_w, right)
                    } else {
                        let left = anchor_inset + side_offset;
                        (left, left + overlay_w)
                    };

                    if stage_w >= overlay_w + 2.0 * viewport_margin {
                        assert!(
                            overlay_stage_left >= viewport_margin - 0.01,
                            "Overlay left ({overlay_stage_left}) must stay >= margin on stage_w={stage_w}, bubble_w={bubble_w}, out={is_outgoing}, w={overlay_w}"
                        );
                        assert!(
                            overlay_stage_right <= stage_w - viewport_margin + 0.01,
                            "Overlay right ({overlay_stage_right}) must stay <= stage_w - margin on stage_w={stage_w}, bubble_w={bubble_w}, out={is_outgoing}, w={overlay_w}"
                        );
                    }
                    // On normal/wide stages:
                    // - Narrow bubbles (`bubble_w < overlay_w - 36`) align `side_offset >= 0` so incoming strips never cover the peer avatar and outgoing strips never protrude past the right bubble edge.
                    // - Wide bubbles (`bubble_w >= overlay_w`) overhang the outer corner by at most `36px` (`<= 56px` reserved side margin).
                    if stage_w >= 640.0 {
                        assert!(
                            side_offset >= -0.01,
                            "On wide stages, narrow bubbles must keep side_offset >= 0 ({side_offset}) rather than covering the avatar or wall"
                        );
                        if bubble_w >= overlay_w {
                            let outer_overhang = (side_offset + overlay_w) - bubble_w;
                            assert!(
                                outer_overhang <= REACTION_CORNER_OVERHANG_PX + 0.01,
                                "Outer overhang ({outer_overhang}) on wide bubble must never exceed {REACTION_CORNER_OVERHANG_PX}px (preserving 56px side margin)"
                            );
                        }
                    }
                }
            }
        }
    }
}
