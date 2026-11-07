//! History Item Subsystem Unit & Integration Tests
//!
//! Verifies authentic upstream C++ behavior from `history/` and `history/admin_log/`
//! in compliance with Mandates #15, #18, and #19.

use gpui_telegram_client::history::admin_log::{
    build_admin_log_filter_sections, AdminLogDeleteGroup, AdminLogFilterFlags,
    AdminLogFilterValue, K_DELETE_GROUP_TIME_WINDOW_SECS, K_EVENTS_PER_PAGE,
    K_MAX_CHANNEL_ADMINS, K_PRELOAD_UP_DISPLAY_ITEMS_BUFFER,
};
use gpui_telegram_client::history::item::{
    flags_from_mtp, HistoryItem, MessageFlags, PaidPostType,
    DEFAULT_EDIT_TIME_LIMIT_SECS, DEFAULT_REVOKE_TIME_LIMIT_SECS,
    K_MIN_LOGIN_CODE, K_NOTIFICATION_TEXT_LIMIT, K_PINNED_MESSAGE_TEXT_LIMIT,
    K_TON_CONNECT_REQUEST_MAX_DELAY_MS,
};
use gpui_telegram_client::history::item_components::{
    HistoryMessageViews, K_MAX_RECENT_REPLIERS,
};
use gpui_telegram_client::history::item_reply_markup::{
    HistoryMessageMarkupButton, HistoryMessageMarkupButtonColor,
    HistoryMessageMarkupButtonIcon, HistoryMessageMarkupButtonType,
    HistoryMessageMarkupButtonVisual, HistoryMessageMarkupData, ReplyMarkupFlags,
};
use gpui_telegram_client::history::selection::{
    MessageSelection, MessageSelectionFlatEndpoint, MessageSelectionKind,
    K_SELECTED_COPY_REPLY_PREVIEW_LIMIT,
};

#[test]
fn test_message_flags_bitmask_and_mtp_conversion() {
    let mut flags = MessageFlags::new(0);
    assert_eq!(flags.0, 0);

    flags.insert(MessageFlags::OUTGOING);
    flags.insert(MessageFlags::PINNED);
    assert!(flags.contains(MessageFlags::OUTGOING));
    assert!(flags.contains(MessageFlags::PINNED));
    assert!(!flags.contains(MessageFlags::SILENT));

    flags.remove(MessageFlags::OUTGOING);
    assert!(!flags.contains(MessageFlags::OUTGOING));
    assert!(flags.contains(MessageFlags::PINNED));

    // FlagsFromMTP test
    let mtp_flags = (1 << 1) | (1 << 4) | (1 << 24) | (1 << 27); // out, mentioned, pinned, invert_media
    let converted = flags_from_mtp(12345, mtp_flags, 0);
    assert!(converted.contains(MessageFlags::HISTORY_ENTRY));
    assert!(converted.contains(MessageFlags::OUTGOING));
    assert!(converted.contains(MessageFlags::MENTIONS_ME));
    assert!(converted.contains(MessageFlags::PINNED));
    assert!(converted.contains(MessageFlags::INVERT_MEDIA));
    assert!(!converted.contains(MessageFlags::SILENT));
}

#[test]
fn test_message_edit_time_bounds() {
    let base_time = 1_700_000_000;
    let mut item = HistoryItem::new(101, 1001, 2001, base_time, "Hello world");

    // 1 hour later: editing allowed
    let t1 = base_time + 3600;
    assert!(!item.is_too_old_for_edit(t1, DEFAULT_EDIT_TIME_LIMIT_SECS, false));
    assert!(item.allows_edit(t1, DEFAULT_EDIT_TIME_LIMIT_SECS, false));

    // 49 hours later: too old for edit under 48h limit
    let t2 = base_time + 49 * 3600;
    assert!(item.is_too_old_for_edit(t2, DEFAULT_EDIT_TIME_LIMIT_SECS, false));
    assert!(!item.allows_edit(t2, DEFAULT_EDIT_TIME_LIMIT_SECS, false));

    // Saved Messages / Self exception: can edit indefinitely
    assert!(!item.is_too_old_for_edit(t2, DEFAULT_EDIT_TIME_LIMIT_SECS, true));
    assert!(item.allows_edit(t2, DEFAULT_EDIT_TIME_LIMIT_SECS, true));

    // Scheduled messages can be edited beyond normal limit
    item.is_scheduled = true;
    assert!(!item.is_too_old_for_edit(t2, DEFAULT_EDIT_TIME_LIMIT_SECS, false));
    item.is_scheduled = false;

    // Paid posts cannot be edited
    item.paid_type = PaidPostType::Stars;
    assert!(!item.allows_edit(t1, DEFAULT_EDIT_TIME_LIMIT_SECS, false));
    item.paid_type = PaidPostType::None;

    // Service messages cannot be edited
    item.is_service = true;
    assert!(!item.allows_edit(t1, DEFAULT_EDIT_TIME_LIMIT_SECS, false));
    item.is_service = false;

    // Forwarded messages cannot be edited
    item.is_forwarded = true;
    assert!(!item.allows_edit(t1, DEFAULT_EDIT_TIME_LIMIT_SECS, false));
    item.is_forwarded = false;

    // Messages with via bot cannot be edited
    item.has_via_bot = true;
    assert!(!item.allows_edit(t1, DEFAULT_EDIT_TIME_LIMIT_SECS, false));
}

#[test]
fn test_delete_for_everyone_rules() {
    let base_time = 1_700_000_000;
    let mut item = HistoryItem::new(102, 1001, 2001, base_time, "Delete test");
    item.flags.insert(MessageFlags::OUTGOING);

    // Recent message in 1-on-1 user chat: can delete for everyone
    let t1 = base_time + 1800; // 30 mins
    assert!(item.can_delete_for_everyone(
        t1, false, true, false, false,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        true,
    ));

    // Message to myself: false (deleted locally without revoke prompt)
    assert!(!item.can_delete_for_everyone(
        t1, true, true, false, false,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        true,
    ));

    // Channel post: false (channel admin deletion handles directly)
    assert!(!item.can_delete_for_everyone(
        t1, false, false, true, false,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        true,
    ));

    // Recipient is a bot: false (bots receive updates and cannot be revoked)
    assert!(!item.can_delete_for_everyone(
        t1, false, true, false, true,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        true,
    ));

    // Too old message (>48h)
    let t2 = base_time + 50 * 3600;
    assert!(!item.can_delete_for_everyone(
        t2, false, true, false, false,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        true,
    ));

    // Incoming message when revoke_private_inbox is false
    let incoming_item = HistoryItem::new(103, 1001, 2001, base_time, "Incoming");
    assert!(!incoming_item.can_delete_for_everyone(
        t1, false, true, false, false,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        DEFAULT_REVOKE_TIME_LIMIT_SECS,
        false, // disabled
    ));
}

#[test]
fn test_reply_markup_layout_rules_and_activation() {
    let visual = HistoryMessageMarkupButtonVisual {
        icon_id: 0,
        color: HistoryMessageMarkupButtonColor::Normal,
    };

    // Single Buy button optimization: OnlyBuyButton flag
    let mut markup = HistoryMessageMarkupData::new();
    let buy_btn = HistoryMessageMarkupButton::new(
        HistoryMessageMarkupButtonType::Buy,
        "Pay 50 Stars",
        visual,
    );
    markup.rows = vec![vec![buy_btn]];
    markup.update_layout_flags();
    assert!(markup.flags.contains(ReplyMarkupFlags::ONLY_BUY_BUTTON));

    // Multi-button grid does NOT set OnlyBuyButton
    let url_btn = HistoryMessageMarkupButton::new(
        HistoryMessageMarkupButtonType::Url,
        "Open Link",
        visual,
    );
    markup.rows[0].push(url_btn);
    markup.update_layout_flags();
    assert!(!markup.flags.contains(ReplyMarkupFlags::ONLY_BUY_BUTTON));

    // Icon mapping tests
    let b_url = HistoryMessageMarkupButton::new(HistoryMessageMarkupButtonType::Url, "URL", visual);
    assert_eq!(b_url.icon(), HistoryMessageMarkupButtonIcon::Url);

    let b_pay = HistoryMessageMarkupButton::new(HistoryMessageMarkupButtonType::Buy, "Buy", visual);
    assert_eq!(b_pay.icon(), HistoryMessageMarkupButtonIcon::Payment);

    let b_switch = HistoryMessageMarkupButton::new(HistoryMessageMarkupButtonType::SwitchInline, "Switch", visual);
    assert_eq!(b_switch.icon(), HistoryMessageMarkupButtonIcon::SwitchPm);

    let b_web = HistoryMessageMarkupButton::new(HistoryMessageMarkupButtonType::WebView, "App", visual);
    assert_eq!(b_web.icon(), HistoryMessageMarkupButtonIcon::Webview);

    let b_copy = HistoryMessageMarkupButton::new(HistoryMessageMarkupButtonType::CopyText, "Copy", visual);
    assert_eq!(b_copy.icon(), HistoryMessageMarkupButtonIcon::Copy);

    // LoadsOnActivate tests
    let b_cb = HistoryMessageMarkupButton::new(HistoryMessageMarkupButtonType::Callback, "CB", visual);
    assert!(b_cb.loads_on_activate());
    assert!(!b_url.loads_on_activate());

    // RichPageButtonKey format
    let mut b_key = HistoryMessageMarkupButton::new(HistoryMessageMarkupButtonType::Callback, "Btn", visual);
    b_key.button_id = 98765;
    b_key.peer_types = 3;
    b_key.data = b"custom_action".to_vec();
    let key = b_key.rich_page_button_key();
    assert_eq!(key, "Callback;98765;3;custom_action");

    // Forwarded data rewriting: SwitchInlineSame -> SwitchInline, text -> forward_text
    let mut b_same = HistoryMessageMarkupButton::new(HistoryMessageMarkupButtonType::SwitchInlineSame, "Original Text", visual);
    b_same.forward_text = Some("Forwarded Text".to_string());
    markup.rows = vec![vec![b_same]];
    let forwarded = markup.to_forwarded_data();
    assert_eq!(forwarded.rows[0][0].button_type, HistoryMessageMarkupButtonType::SwitchInline);
    assert_eq!(forwarded.rows[0][0].text, "Forwarded Text");
}

#[test]
fn test_selection_endpoints_and_range_math() {
    let ep1 = MessageSelectionFlatEndpoint::new(10, false);
    assert_eq!(ep1.offset(), 10);

    let ep2 = MessageSelectionFlatEndpoint::new(10, true);
    assert_eq!(ep2.offset(), 11);

    // Forward selection
    let anchor = MessageSelectionFlatEndpoint::new(5, false); // offset 5
    let focus = MessageSelectionFlatEndpoint::new(20, false); // offset 20
    let sel = MessageSelection::from_endpoints(anchor, focus);
    assert_eq!(sel.kind, MessageSelectionKind::Flat);
    assert_eq!(sel.flat.from, 5);
    assert_eq!(sel.flat.to, 20);
    assert_eq!(sel.flat.len(), 15);

    // Backward drag selection (normalized to from < to)
    let sel_back = MessageSelection::from_endpoints(focus, anchor);
    assert_eq!(sel_back.flat.from, 5);
    assert_eq!(sel_back.flat.to, 20);

    // Endpoint contains
    let mid = MessageSelectionFlatEndpoint::new(12, false);
    assert!(sel.contains(&mid));
    let outside = MessageSelectionFlatEndpoint::new(22, false);
    assert!(!sel.contains(&outside));

    // Extract slice
    let full = "The quick brown fox jumps over the lazy dog";
    let extracted = sel.extract_slice(full);
    assert_eq!(extracted, &full[5..20]);
}

#[test]
fn test_admin_log_filter_bitmasks_and_categories() {
    assert_eq!(AdminLogFilterFlags::ADMIN_RIGHTS, AdminLogFilterFlags::PROMOTE | AdminLogFilterFlags::DEMOTE);
    assert_eq!(AdminLogFilterFlags::RESTRICTIONS, AdminLogFilterFlags::BAN | AdminLogFilterFlags::UNBAN | AdminLogFilterFlags::KICK | AdminLogFilterFlags::UNKICK);
    assert_eq!(AdminLogFilterFlags::MEMBERS_NEW, AdminLogFilterFlags::JOIN | AdminLogFilterFlags::INVITE);
    assert_eq!(AdminLogFilterFlags::MEMBERS_REMOVED, AdminLogFilterFlags::LEAVE);

    let mut filter = AdminLogFilterValue::all();
    assert!(filter.matches_event(AdminLogFilterFlags::JOIN, 1234));

    // Specific flag filter
    filter.flags = Some(AdminLogFilterFlags::new(AdminLogFilterFlags::RESTRICTIONS));
    assert!(filter.matches_event(AdminLogFilterFlags::BAN, 1234));
    assert!(!filter.matches_event(AdminLogFilterFlags::JOIN, 1234));

    // Specific admin filter
    filter.admin_user_ids = Some(vec![1001, 1002]);
    assert!(filter.matches_event(AdminLogFilterFlags::BAN, 1001));
    assert!(!filter.matches_event(AdminLogFilterFlags::BAN, 9999));

    // Sections for Channel vs Group
    let channel_sections = build_admin_log_filter_sections(true);
    let group_sections = build_admin_log_filter_sections(false);
    assert_eq!(channel_sections[0].title, "Subscribers");
    assert_eq!(group_sections[0].title, "Members");

    // Group includes Topics and Pinned Messages, Channel does not
    let group_settings = &group_sections[1];
    assert!(group_settings.entries.iter().any(|e| e.flags == AdminLogFilterFlags::TOPICS));
    let channel_settings = &channel_sections[1];
    assert!(!channel_settings.entries.iter().any(|e| e.flags == AdminLogFilterFlags::TOPICS));
}

#[test]
fn test_admin_log_delete_grouping_coalescing() {
    let mut group = AdminLogDeleteGroup::new(1001, 555, 1_700_000_000, 10);
    assert_eq!(group.count(), 1);

    // Within 5s by same admin -> coalesced
    let coalesced1 = group.try_coalesce(555, 1_700_000_003, 11);
    assert!(coalesced1);
    assert_eq!(group.count(), 2);

    // Within 5s window from start (first_event_date) -> coalesced
    let coalesced2 = group.try_coalesce(555, 1_700_000_005, 12);
    assert!(coalesced2);
    assert_eq!(group.count(), 3);

    // 6 seconds from start: NOT coalesced
    let not_coalesced_time = group.try_coalesce(555, 1_700_000_006, 13);
    assert!(!not_coalesced_time);
    assert_eq!(group.count(), 3);

    // Different admin: NOT coalesced
    let not_coalesced_admin = group.try_coalesce(777, 1_700_000_002, 14);
    assert!(!not_coalesced_admin);
    assert_eq!(group.count(), 3);
}

#[test]
fn test_notification_and_pinned_text_limits() {
    assert_eq!(K_NOTIFICATION_TEXT_LIMIT, 255);
    assert_eq!(K_PINNED_MESSAGE_TEXT_LIMIT, 16);
    assert_eq!(K_MIN_LOGIN_CODE, 5);
    assert_eq!(K_TON_CONNECT_REQUEST_MAX_DELAY_MS, 86_400_000);
    assert_eq!(K_MAX_CHANNEL_ADMINS, 200);
    assert_eq!(K_EVENTS_PER_PAGE, 100);
    assert_eq!(K_PRELOAD_UP_DISPLAY_ITEMS_BUFFER, 4);
    assert_eq!(K_DELETE_GROUP_TIME_WINDOW_SECS, 5);
    assert_eq!(K_MAX_RECENT_REPLIERS, 3);
    assert_eq!(K_SELECTED_COPY_REPLY_PREVIEW_LIMIT, 64);

    let short_item = HistoryItem::new(1, 1, 1, 100, "Short text");
    assert_eq!(short_item.notification_text(), "Short text");

    let long_text = "a".repeat(300);
    let long_item = HistoryItem::new(2, 1, 1, 100, long_text);
    let truncated = long_item.notification_text();
    assert_eq!(truncated.chars().count(), 256); // 255 + ellipsis
    assert!(truncated.ends_with('…'));
}

#[test]
fn test_views_recent_repliers_capacity() {
    let mut views = HistoryMessageViews::new(100, 5);
    views.add_replier(10);
    views.add_replier(20);
    views.add_replier(30);
    assert_eq!(views.recent_repliers.len(), 3);

    // Adding 4th replier evicts oldest (capacity capped at K_MAX_RECENT_REPLIERS = 3)
    views.add_replier(40);
    assert_eq!(views.recent_repliers.len(), 3);
    assert_eq!(views.recent_repliers, vec![20, 30, 40]);
}
