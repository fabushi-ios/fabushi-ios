//! Comprehensive Subsystem Tests for Module 05: Dialogs Core Data & Indexing
//!
//! Validates:
//! - 64-bit composite sort keys & tie-breakers
//! - UnreadState arithmetic & BadgesForUnread logic
//! - DialogsList slice rotation, date ordering, and binary search by Y
//! - DialogsIndexedList multi-word prefix search & minimal bucket optimization
//! - DialogsPinnedList capacity limits, rotate to top, and reordering
//! - DialogsMainList unread delta tracking & cloud unread synchronization
//! - QuickDialogAction state machines & Lottie icon key resolution
//! - CommunityChatsList sorting & prefix-sum row indexing
//! - Hash/Cashtag query parsing & formatting

use gpui_telegram_client::dialogs::*;

#[test]
fn test_dialog_sort_keys_and_tie_breakers() {
    let now = 1730000000_i64;
    let pos1 = dialog_pos_from_date(now);
    let pos2 = dialog_pos_from_date(now);
    assert_ne!(pos1, 0);
    assert_ne!(pos2, 0);
    assert_ne!(pos1, pos2, "Successive date calls must have distinct monotonic shift bits");
    assert!(pos2 > pos1, "Later touch with same date must have higher shift tie-breaker");

    // Zero date
    assert_eq!(dialog_pos_from_date(0), 0);

    // Fixed on top positions
    let fixed_archive = fixed_on_top_dialog_pos(K_ARCHIVE_FIX_ON_TOP_INDEX);
    let fixed_promo = fixed_on_top_dialog_pos(K_TOP_PROMOTION_FIX_ON_TOP_INDEX);
    assert!(fixed_archive > fixed_promo, "Archive (index 1) must sort above promotion (index 2)");

    // Pinned positions
    let pinned_1 = pinned_dialog_pos(1);
    let pinned_2 = pinned_dialog_pos(2);
    assert!(pinned_1 > pinned_2, "Pinned 1 must sort above pinned 2");

    // Unread on top
    let unread_on_top = unread_on_top_dialog_pos(pos1);
    assert!(unread_on_top > pos1, "Unread on top must set highest bit");
    assert_eq!(unread_on_top & 0x8000000000000000, 0x8000000000000000);

    // Hierarchy check: Fixed on top > Pinned > Unread on top > Standard Date
    assert!(fixed_promo > pinned_1, "Fixed on top must exceed pinned items");
    assert!(pinned_2 > unread_on_top, "Pinned items must exceed unread-on-top items");
    assert!(unread_on_top > pos2, "Unread on top must exceed standard date position");
}

#[test]
fn test_unread_state_math_and_badges_for_unread() {
    let mut state1 = UnreadState {
        messages: 5,
        messages_muted: 2,
        chats: 2,
        chats_muted: 1,
        marks: 1,
        marks_muted: 0,
        reactions: 3,
        reactions_muted: 1,
        mentions: 1,
        polls: 0,
        polls_muted: 0,
        known: true,
    };

    let state2 = UnreadState {
        messages: 3,
        messages_muted: 1,
        chats: 1,
        chats_muted: 0,
        marks: 0,
        marks_muted: 0,
        reactions: 1,
        reactions_muted: 1,
        mentions: 0,
        polls: 2,
        polls_muted: 1,
        known: true,
    };

    // Operator += and -=
    state1 += state2;
    assert_eq!(state1.messages, 8);
    assert_eq!(state1.messages_muted, 3);
    assert_eq!(state1.polls, 2);

    state1 -= state2;
    assert_eq!(state1.messages, 5);
    assert_eq!(state1.messages_muted, 2);
    assert_eq!(state1.polls, 0);

    // Badges calculation: counting messages
    let badges_msg = badges_for_unread(
        &state1,
        CountInBadge::Messages,
        IncludeInBadge::Default,
        false,
        false,
    );
    // counterFull = marks(1) + messages(5) = 6
    // counterMuted = marksMuted(0) + messagesMuted(2) = 2
    // includeMuted = false -> marks = 1, counter = 6 - 2 = 4
    assert!(badges_msg.unread);
    assert_eq!(badges_msg.unread_counter, 4);
    assert!(!badges_msg.unread_muted);
    assert!(badges_msg.mention);
    assert!(badges_msg.reaction);
    assert!(!badges_msg.reaction_muted);

    // Badges calculation: IncludeInBadge::All
    let badges_all = badges_for_unread(
        &state1,
        CountInBadge::Messages,
        IncludeInBadge::All,
        false,
        false,
    );
    // includeMuted = true -> counter = 6
    assert_eq!(badges_all.unread_counter, 6);

    // Single mark dot special case: counter == 1 && marks == 1 -> unread_counter == 0
    let single_mark_state = UnreadState {
        marks: 1,
        known: true,
        ..Default::default()
    };
    let mark_badge = badges_for_unread(
        &single_mark_state,
        CountInBadge::Messages,
        IncludeInBadge::Default,
        false,
        false,
    );
    assert!(mark_badge.unread);
    assert_eq!(mark_badge.unread_counter, 0, "Single mark dot displays no digit (counter=0)");

    // Empty badges state
    let empty_state = UnreadState::default();
    let empty_badge = badges_for_unread(
        &empty_state,
        CountInBadge::Default,
        IncludeInBadge::Default,
        false,
        false,
    );
    assert!(empty_badge.is_empty());
}

#[test]
fn test_dialogs_list_ordering_and_rotation() {
    let mut list = DialogsList::new(SortMode::Date, 0);

    let e1 = DialogEntryData {
        id: 1,
        title: "Chat 1".into(),
        time_id: 1000,
        ..Default::default()
    };
    let e2 = DialogEntryData {
        id: 2,
        title: "Chat 2".into(),
        time_id: 3000,
        ..Default::default()
    };
    let e3 = DialogEntryData {
        id: 3,
        title: "Chat 3".into(),
        time_id: 2000,
        ..Default::default()
    };

    list.add_to_end(e1);
    list.add_to_end(e2);
    list.add_to_end(e3);

    assert_eq!(list.size(), 3);
    assert!(list.is_sorted_by_date());
    // Descending by date: Chat 2 (3000) at index 0, Chat 3 (2000) at index 1, Chat 1 (1000) at index 2
    assert_eq!(list.rows()[0].entry.id, 2);
    assert_eq!(list.rows()[1].entry.id, 3);
    assert_eq!(list.rows()[2].entry.id, 1);

    // Height & coordinates
    assert_eq!(list.rows()[0].top, 0);
    assert_eq!(list.rows()[0].height, 62);
    assert_eq!(list.rows()[1].top, 62);
    assert_eq!(list.rows()[2].top, 124);
    assert_eq!(list.height(), 186);

    // Binary search by Y coordinate
    let row_at_0 = list.row_at_y(0).expect("Should find row at y=0");
    assert_eq!(row_at_0.entry.id, 2);

    let row_at_65 = list.row_at_y(65).expect("Should find row at y=65");
    assert_eq!(row_at_65.entry.id, 3);

    let row_at_150 = list.row_at_y(150).expect("Should find row at y=150");
    assert_eq!(row_at_150.entry.id, 1);

    assert!(list.row_at_y(200).is_none(), "y out of bounds should return None");

    // Remove item and check subsequent tops shift up
    let removed = list.remove(&DialogKey::History(3));
    assert!(removed);
    assert_eq!(list.size(), 2);
    assert_eq!(list.rows()[0].entry.id, 2);
    assert_eq!(list.rows()[1].entry.id, 1);
    assert_eq!(list.rows()[1].top, 62);
    assert_eq!(list.height(), 124);
}

#[test]
fn test_dialogs_indexed_list_minimal_bucket_filter() {
    let mut indexed = DialogsIndexedList::new(SortMode::Date, 0);

    let d1 = DialogEntryData::new(1, "Alice Smith");
    let d2 = DialogEntryData::new(2, "Bob Jones");
    let d3 = DialogEntryData::new(3, "Charlie Brown");
    let d4 = DialogEntryData::new(4, "Alice Cooper");

    indexed.add_to_end(d1);
    indexed.add_to_end(d2);
    indexed.add_to_end(d3);
    indexed.add_to_end(d4);

    assert_eq!(indexed.size(), 4);

    // Multi-word prefix search
    let matches_alice_smith = indexed.filtered_words(&["ali".to_string(), "smi".to_string()]);
    assert_eq!(matches_alice_smith.len(), 1);
    assert_eq!(matches_alice_smith[0].entry.id, 1);

    let matches_alice_all = indexed.filtered_words(&["ali".to_string()]);
    assert_eq!(matches_alice_all.len(), 2);

    let matches_bob = indexed.filtered_words(&["bob".to_string()]);
    assert_eq!(matches_bob.len(), 1);
    assert_eq!(matches_bob[0].entry.id, 2);

    // Searching for prefix not in any bucket returns empty immediately
    let matches_none = indexed.filtered_words(&["david".to_string()]);
    assert!(matches_none.is_empty());
}

#[test]
fn test_dialogs_pinned_list_capacity_and_rotation() {
    let mut pinned = DialogsPinnedList::new(0, 3);

    let k1 = DialogKey::History(101);
    let k2 = DialogKey::History(102);
    let k3 = DialogKey::History(103);
    let k4 = DialogKey::History(104);

    pinned.add_pinned(k1);
    pinned.add_pinned(k2);
    pinned.add_pinned(k3);
    assert_eq!(pinned.size(), 3);
    assert_eq!(pinned.order(), &[k1, k2, k3]);

    // Adding 4th item via add_pinned beyond limit 3 replaces tail (k3)
    pinned.add_pinned(k4);
    assert_eq!(pinned.size(), 3);
    assert_eq!(pinned.order(), &[k1, k2, k4]);

    // Setting pinned on existing item (k2) rotates it to top (index 0)
    pinned.set_pinned(k2, true);
    assert_eq!(pinned.order(), &[k2, k1, k4], "k2 must rotate to top position");

    // 1-indexed pin positions
    assert_eq!(pinned.pinned_index(&k2), 1);
    assert_eq!(pinned.pinned_index(&k1), 2);
    assert_eq!(pinned.pinned_index(&k4), 3);
    assert_eq!(pinned.pinned_index(&k3), 0);

    // Reorder k1 and k4
    let reordered = pinned.reorder(k1, k4);
    assert!(reordered);
    assert_eq!(pinned.order(), &[k2, k4, k1]);

    // Setting pinned on newly pinned item beyond limit evicts tail (k1) and rotates to top
    pinned.set_pinned(k3, true);
    assert_eq!(pinned.order(), &[k3, k2, k4]);
    assert_eq!(pinned.pinned_index(&k3), 1);
    assert_eq!(pinned.pinned_index(&k1), 0);

    // Unpin k3
    pinned.set_pinned(k3, false);
    assert_eq!(pinned.size(), 2);
    assert_eq!(pinned.order(), &[k2, k4]);
}

#[test]
fn test_main_list_unread_delta_and_cloud_state() {
    let mut main_list = DialogsMainList::new(0, 5);

    let e1 = DialogEntryData {
        id: 10,
        title: "News Channel".into(),
        time_id: 1000,
        unread_state: UnreadState {
            messages: 10,
            messages_muted: 0,
            known: true,
            ..Default::default()
        },
        ..Default::default()
    };

    let e2 = DialogEntryData {
        id: 20,
        title: "Muted Group".into(),
        time_id: 2000,
        unread_state: UnreadState {
            messages: 4,
            messages_muted: 4,
            known: true,
            ..Default::default()
        },
        ..Default::default()
    };

    main_list.add_entry(e1);
    main_list.add_entry(e2);

    let unread = main_list.unread_state();
    assert_eq!(unread.messages, 14);
    assert_eq!(unread.messages_muted, 4);

    // Cloud unread update for folder metadata
    main_list.update_cloud_unread(5, 5, 2, 2);
    assert_eq!(main_list.unread_state().messages, 10);
    // Archive cloud unread counts all as muted
    assert_eq!(main_list.unread_state().messages_muted, 10);

    // Setting loaded switches back to accurate local tally
    main_list.set_loaded(true);
    assert_eq!(main_list.unread_state().messages, 14);
    assert_eq!(main_list.unread_state().messages_muted, 4);
}

#[test]
fn test_quick_action_state_machine_and_lottie_icons() {
    // Mute resolution
    let mute_unmuted = resolve_quick_dialog_label(
        QuickDialogAction::Mute,
        false, // not self
        false, // not muted
        false,
        false,
        false,
        true,
        false,
    );
    assert_eq!(mute_unmuted, QuickDialogActionLabel::Mute);
    assert_eq!(resolve_quick_dialog_lottie_icon_name(mute_unmuted), "swipe_mute");

    let mute_muted = resolve_quick_dialog_label(
        QuickDialogAction::Mute,
        false,
        true, // already muted
        false,
        false,
        false,
        true,
        false,
    );
    assert_eq!(mute_muted, QuickDialogActionLabel::Unmute);
    assert_eq!(resolve_quick_dialog_lottie_icon_name(mute_muted), "swipe_unmute");

    // Pin resolution
    let pin_unpinned = resolve_quick_dialog_label(
        QuickDialogAction::Pin,
        false, false, false, false, false, true, false,
    );
    assert_eq!(pin_unpinned, QuickDialogActionLabel::Pin);
    assert_eq!(resolve_quick_dialog_lottie_icon_name(pin_unpinned), "swipe_pin");

    let pin_pinned = resolve_quick_dialog_label(
        QuickDialogAction::Pin,
        false, false, true, false, false, true, false,
    );
    assert_eq!(pin_pinned, QuickDialogActionLabel::Unpin);
    assert_eq!(resolve_quick_dialog_lottie_icon_name(pin_pinned), "swipe_unpin");

    // Read resolution
    let read_with_unread = resolve_quick_dialog_label(
        QuickDialogAction::Read,
        false, false, false, true, false, true, false,
    );
    assert_eq!(read_with_unread, QuickDialogActionLabel::Read);
    assert_eq!(resolve_quick_dialog_lottie_icon_name(read_with_unread), "swipe_read");

    // Ungroup community
    let ungroup = resolve_quick_dialog_label(
        QuickDialogAction::Mute,
        false, false, false, false, false, true, true,
    );
    assert_eq!(ungroup, QuickDialogActionLabel::Ungroup);
    assert_eq!(resolve_quick_dialog_lottie_icon_name(ungroup), "swipe_ungroup");
    assert_eq!(resolve_quick_action_bg_hex(ungroup), "#E53935");

    // Font size shrinking
    let normal_font = fit_swipe_action_font_size(4, 100);
    assert_eq!(normal_font, 13);
    let shrunk_font = fit_swipe_action_font_size(25, 60);
    assert!(shrunk_font < 13);
    assert!(shrunk_font >= 5);
}

#[test]
fn test_community_chats_list_sorting_and_prefix_sum() {
    let mut comm_list = CommunityChatsList::new(999, CommunityChatsKind::Joined);

    let entries = vec![
        CommunityChatEntry {
            peer_id: 1,
            title: "Chat 1".into(),
            sort_key: 1000,
            time_id: 50,
            is_joined: true,
            is_broadcast: false,
            members_count: Some(10),
        },
        CommunityChatEntry {
            peer_id: 2,
            title: "Chat 2".into(),
            sort_key: 3000,
            time_id: 10,
            is_joined: true,
            is_broadcast: false,
            members_count: Some(50),
        },
        CommunityChatEntry {
            peer_id: 3,
            title: "Chat 3".into(),
            sort_key: 3000,
            time_id: 20, // Higher time_id tie breaker
            is_joined: true,
            is_broadcast: true,
            members_count: Some(100),
        },
    ];

    comm_list.rebuild(entries, 62);
    assert_eq!(comm_list.view.size(), 3);
    // Sort order descending by (sort_key, time_id):
    // Chat 3 (3000, 20) -> Chat 2 (3000, 10) -> Chat 1 (1000, 50)
    assert_eq!(comm_list.view.rows()[0].peer_id, 3);
    assert_eq!(comm_list.view.rows()[1].peer_id, 2);
    assert_eq!(comm_list.view.rows()[2].peer_id, 1);

    // Prefix sum tops and height
    assert_eq!(comm_list.view.row_top(0), Some(0));
    assert_eq!(comm_list.view.row_top(1), Some(62));
    assert_eq!(comm_list.view.row_top(2), Some(124));
    assert_eq!(comm_list.view.height(), 186);

    // Binary search by Y offset
    assert_eq!(comm_list.view.index_by_y(0), Some(0));
    assert_eq!(comm_list.view.index_by_y(60), Some(0));
    assert_eq!(comm_list.view.index_by_y(62), Some(1));
    assert_eq!(comm_list.view.index_by_y(150), Some(2));
    assert_eq!(comm_list.view.index_by_y(200), None);
}

#[test]
fn test_hash_and_cashtag_parsing() {
    assert_eq!(is_hash_or_cashtag_search_query("#telegram"), HashOrCashtag::Hashtag);
    assert_eq!(is_hash_or_cashtag_search_query("  #rust  "), HashOrCashtag::Hashtag);
    assert_eq!(is_hash_or_cashtag_search_query("#invalid with space"), HashOrCashtag::None);
    assert_eq!(is_hash_or_cashtag_search_query("$TON"), HashOrCashtag::Cashtag);
    assert_eq!(is_hash_or_cashtag_search_query("$BTC"), HashOrCashtag::Cashtag);
    assert_eq!(is_hash_or_cashtag_search_query("$ton_lowercase"), HashOrCashtag::None);
    assert_eq!(is_hash_or_cashtag_search_query("regular text"), HashOrCashtag::None);

    let fixed = fix_hashtag_search_query("cool  feature  ", 4, HashOrCashtag::Hashtag);
    assert!(fixed.text.starts_with('#'));
    assert!(!fixed.text.contains("  "));
    assert_eq!(fixed.text, "#coolfeature");

    let fixed_start = fix_hashtag_search_query("cool  feature", 0, HashOrCashtag::Hashtag);
    assert_eq!(fixed_start.text, "#coolfeature");
}
