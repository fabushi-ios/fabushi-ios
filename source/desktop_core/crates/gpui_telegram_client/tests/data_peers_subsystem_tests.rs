//! Data Peers & Session Subsystem Tests
//! Module 03 (storage & data) comprehensive verification suite.

use gpui_telegram_client::data::*;

#[test]
fn test_peer_id_encoding_and_serialization() {
    let user_id = UserId(12345678);
    let peer_user = PeerId::from_user(user_id);
    assert!(peer_user.is_user());
    assert!(!peer_user.is_chat());
    assert!(!peer_user.is_channel());
    assert_eq!(peer_user.shift(), PeerId::SHIFT_USER);
    assert_eq!(peer_user.bare(), 12345678);
    assert_eq!(peer_user.to_user(), Some(user_id));

    let chat_id = ChatId(987654);
    let peer_chat = PeerId::from_chat(chat_id);
    assert!(peer_chat.is_chat());
    assert_eq!(peer_chat.shift(), PeerId::SHIFT_CHAT);
    assert_eq!(peer_chat.bare(), 987654);
    assert_eq!(peer_chat.to_chat(), Some(chat_id));

    let channel_id = ChannelId(1999888777);
    let peer_channel = PeerId::from_channel(channel_id);
    assert!(peer_channel.is_channel());
    assert_eq!(peer_channel.shift(), PeerId::SHIFT_CHANNEL);
    assert_eq!(peer_channel.bare(), 1999888777);
    assert_eq!(peer_channel.to_channel(), Some(channel_id));

    let fake_id = FakeChatId(42);
    let peer_fake = PeerId::from_fake(fake_id);
    assert!(peer_fake.is_fake());
    assert_eq!(peer_fake.shift(), PeerId::SHIFT_FAKE);
    assert_eq!(peer_fake.bare(), 42);

    // Serialization & deserialization (modern reserved bit)
    let serialized = peer_user.serialize();
    let deserialized = PeerId::deserialize(serialized);
    assert_eq!(peer_user, deserialized);

    let serialized_channel = peer_channel.serialize();
    let deserialized_channel = PeerId::deserialize(serialized_channel);
    assert_eq!(peer_channel, deserialized_channel);

    // Legacy format deserialization
    // Legacy user: high 32 bits = 0
    let legacy_user_raw = 555666u64;
    let legacy_user = PeerId::deserialize(legacy_user_raw);
    assert!(legacy_user.is_user());
    assert_eq!(legacy_user.bare(), 555666);

    // Legacy chat: 0x100000000
    let legacy_chat_raw = 0x100000000u64 | 777888;
    let legacy_chat = PeerId::deserialize(legacy_chat_raw);
    assert!(legacy_chat.is_chat());
    assert_eq!(legacy_chat.bare(), 777888);

    // Legacy channel: 0x200000000
    let legacy_channel_raw = 0x200000000u64 | 12345;
    let legacy_channel = PeerId::deserialize(legacy_channel_raw);
    assert!(legacy_channel.is_channel());
    assert_eq!(legacy_channel.bare(), 12345);

    // Display
    assert_eq!(format!("{}", peer_user), "user#12345678");
    assert_eq!(format!("{}", peer_chat), "chat#987654");
    assert_eq!(format!("{}", peer_channel), "channel#1999888777");
}

#[test]
fn test_user_data_flags_and_name_resolution() {
    let mut user = UserData::new(UserId(1001));

    // Initially unnamed
    assert_eq!(user.full_name(), "User#1001");

    // Phone fallback
    user.phone = "+1234567890".to_string();
    assert_eq!(user.full_name(), "+1234567890");

    // First name only
    user.set_name("Durov", "");
    assert_eq!(user.full_name(), "Durov");

    // Full name
    user.set_name("Pavel", "Durov");
    assert_eq!(user.full_name(), "Pavel Durov");

    // Flags
    assert!(!user.is_verified());
    assert!(!user.is_premium());
    user.flags.add(UserDataFlags::VERIFIED);
    user.flags.add(UserDataFlags::PREMIUM);
    user.flags.add(UserDataFlags::CONTACT);
    assert!(user.is_verified());
    assert!(user.is_premium());
    assert!(user.is_contact());

    // Online status
    let now = 1700000000;
    user.lastseen = LastseenStatus::online_till(now + 60, false, false);
    assert!(user.is_online(now));
    assert!(!user.is_online(now + 120));

    // Birthday
    let bday = Birthday::new(10, 10, 1984);
    assert!(bday.is_valid());
    assert_eq!(bday.day(), 10);
    assert_eq!(bday.month(), 10);
    assert_eq!(bday.year(), 1984);
    assert_eq!(bday.format_text(), "October 10, 1984");
    user.birthday = Some(bday);
    assert_eq!(user.birthday.unwrap().year(), 1984);
}

#[test]
fn test_chat_and_channel_admin_rights() {
    let mut chat = ChatData::new(ChatId(501));
    chat.title = "Core Team".to_string();
    chat.flags.add(ChatDataFlags::CREATOR);
    assert!(chat.am_creator());
    assert!(chat.am_in());
    assert!(chat.can_delete_messages());
    assert!(chat.can_add_members());

    let mut non_creator_chat = ChatData::new(ChatId(502));
    assert!(!non_creator_chat.can_delete_messages());
    non_creator_chat.admin_rights.add(ChatAdminRights::DELETE_MESSAGES);
    assert!(non_creator_chat.can_delete_messages());

    // Channel
    let mut channel = ChannelData::new(ChannelId(9001));
    channel.title = "Telegram News".to_string();
    channel.flags.add(ChannelDataFlags::BROADCAST);
    assert!(channel.is_broadcast());
    assert!(!channel.is_megagroup());

    // Broadcast posting requires admin rights or creator
    assert!(!channel.can_post_messages());
    channel.admin_rights.add(ChatAdminRights::POST_MESSAGES);
    assert!(channel.can_post_messages());

    // Supergroup (megagroup) forum
    let mut supergroup = ChannelData::new(ChannelId(9002));
    supergroup.flags.add(ChannelDataFlags::MEGAGROUP);
    supergroup.flags.add(ChannelDataFlags::FORUM);
    assert!(supergroup.is_megagroup());
    assert!(supergroup.is_forum());
    assert!(!supergroup.can_manage_topics());
    supergroup.admin_rights.add(ChatAdminRights::MANAGE_TOPICS);
    assert!(supergroup.can_manage_topics());

    // Restriction check expiration
    let restrictions = ChatRestrictionsInfo {
        flags: ChatRestrictions(ChatRestrictions::SEND_PHOTOS | ChatRestrictions::SEND_FILES),
        until: 1000,
    };
    // Before expiration
    assert!(restrictions.is_restricted_at(ChatRestrictions::SEND_PHOTOS, 500));
    // After expiration
    assert!(!restrictions.is_restricted_at(ChatRestrictions::SEND_PHOTOS, 1500));
}

#[test]
fn test_chat_filter_matching_engine() {
    let mut filter = ChatFilter::new(1, "Personal & Work");
    filter.flags.add(ChatFilterFlags::CONTACTS);
    filter.flags.add(ChatFilterFlags::GROUPS);
    filter.flags.add(ChatFilterFlags::NO_MUTED);

    let contact_peer = PeerId::from_user(UserId(101));
    let group_peer = PeerId::from_chat(ChatId(202));
    let channel_peer = PeerId::from_channel(ChannelId(303));
    let bot_peer = PeerId::from_user(UserId(404));

    // Contact, unmuted -> matches
    let cand_contact = ChatFilterCandidate {
        peer_id: contact_peer,
        is_contact: true,
        is_muted: false,
        ..Default::default()
    };
    assert!(filter.matches(&cand_contact));

    // Contact, but muted -> rejected by NO_MUTED
    let cand_contact_muted = ChatFilterCandidate {
        peer_id: contact_peer,
        is_contact: true,
        is_muted: true,
        ..Default::default()
    };
    assert!(!filter.matches(&cand_contact_muted));

    // Group, unmuted -> matches
    let cand_group = ChatFilterCandidate {
        peer_id: group_peer,
        is_group: true,
        is_muted: false,
        ..Default::default()
    };
    assert!(filter.matches(&cand_group));

    // Channel -> not in filter flags -> rejected
    let cand_channel = ChatFilterCandidate {
        peer_id: channel_peer,
        is_channel: true,
        is_muted: false,
        ..Default::default()
    };
    assert!(!filter.matches(&cand_channel));

    // Bot -> not in filter flags -> rejected
    let cand_bot = ChatFilterCandidate {
        peer_id: bot_peer,
        is_bot: true,
        is_muted: false,
        ..Default::default()
    };
    assert!(!filter.matches(&cand_bot));

    // Channel added to always list -> bypasses type filter and matches!
    filter.always.push(channel_peer);
    assert!(filter.matches(&cand_channel));

    // Group added to never list -> excluded even though flag is set!
    filter.never.push(group_peer);
    assert!(!filter.matches(&cand_group));
}

#[test]
fn test_histories_and_message_unread_accounting() {
    let mut histories = DataHistories::new();
    let peer = PeerId::from_user(UserId(777));

    let history = histories.get_or_create(peer);
    assert_eq!(history.unread.count, 0);

    // Apply incoming message
    let msg1 = MessageItem::new(MsgId(1), peer, "Hello");
    history.apply_new_message(msg1);
    assert_eq!(history.unread.count, 1);

    // Apply another incoming message
    let msg2 = MessageItem::new(MsgId(2), peer, "How are you?");
    history.apply_new_message(msg2);
    assert_eq!(history.unread.count, 2);

    // Total unread across all histories
    assert_eq!(histories.total_unread_count(), 2);

    // Read inbox till msg 2
    histories.read_inbox(peer, MsgId(2));
    let hist_after = histories.find(peer).unwrap();
    assert_eq!(hist_after.unread.count, 0);
    assert_eq!(hist_after.inbox_read_till_id, MsgId(2));
    assert_eq!(histories.total_unread_count(), 0);
}

#[test]
fn test_folders_management() {
    let mut folder_mgr = DataFolder::new();
    let folder = folder_mgr.get_or_create(Folder::ARCHIVE_ID, "Archive");
    assert!(folder.is_archive());

    let p1 = PeerId::from_user(UserId(1));
    let p2 = PeerId::from_chat(ChatId(2));

    folder.add_peer(p1);
    folder.add_peer(p2);
    assert!(folder.contains(p1));
    assert!(folder.contains(p2));

    folder.pin_peer(p1);
    assert_eq!(folder.pinned, vec![p1]);

    folder.remove_peer(p1);
    assert!(!folder.contains(p1));
    assert!(!folder.pinned.contains(&p1));
}

#[test]
fn test_data_session_central_repository() {
    let mut session = DataSession::new();

    // Process User
    let u_id = UserId(999);
    session.process_user(u_id, "Nikolai", "Durov", "+441234567", Some("nikolai"));
    let user = session.user_loaded(u_id).expect("User must be loaded");
    assert_eq!(user.full_name(), "Nikolai Durov");
    assert_eq!(user.usernames.primary_username(), Some("nikolai"));

    // Process Chat
    let c_id = ChatId(888);
    session.process_chat(c_id, "Developers", u_id, vec![u_id, UserId(1001)]);
    let chat = session.chat_loaded(c_id).expect("Chat must be loaded");
    assert_eq!(chat.title, "Developers");
    assert_eq!(chat.count, 2);

    // Process Channel
    let ch_id = ChannelId(777);
    session.process_channel(ch_id, "Telegram Broadcast", Some("telegram"), false);
    let channel = session.channel_loaded(ch_id).expect("Channel must be loaded");
    assert_eq!(channel.title, "Telegram Broadcast");
    assert!(channel.is_broadcast());

    // History coordination
    let p_channel = PeerId::from_channel(ch_id);
    let h = session.history(p_channel);
    h.set_unread_count(5);
    assert_eq!(session.histories.find(p_channel).unwrap().unread.count, 5);

    // Clear
    session.clear();
    assert!(session.user_loaded(u_id).is_none());
    assert!(session.chat_loaded(c_id).is_none());
    assert!(session.channel_loaded(ch_id).is_none());
}

#[test]
fn test_components_and_raw_subsystems() {
    // 1. Raw Countries Bounds
    let raw_bounds = raw::RawCountriesBoundsRaw::new();
    // London: ~51.5074, -0.1278 -> GB
    assert_eq!(raw_bounds.find_country(51.5074, -0.1278), Some("GB"));
    // New York: ~40.7128, -74.0060 -> US
    assert_eq!(raw_bounds.find_country(40.7128, -74.0060), Some("US"));

    // 2. Top Peers
    let mut top_peers = components::TopPeersComponents::new();
    let p1 = PeerId::from_user(UserId(1));
    let p2 = PeerId::from_user(UserId(2));
    top_peers.increment(p1, 5.0);
    top_peers.increment(p2, 10.0);
    assert_eq!(top_peers.top_peers(), vec![p2, p1]);

    // 3. Scheduled Messages
    let mut scheduled = components::ScheduledMessagesComponents::new();
    let msg = MessageItem::new(MsgId(-10), p1, "Happy Birthday!");
    scheduled.schedule(p1, msg, 1800000000);
    assert_eq!(scheduled.count_for_peer(p1), 1);
    assert!(scheduled.cancel(p1, MsgId(-10)));
    assert_eq!(scheduled.count_for_peer(p1), 0);

    // 4. Sponsored Messages
    let mut sponsored = components::SponsoredMessagesComponents::new();
    let ch = PeerId::from_channel(ChannelId(1234));
    sponsored.set_messages(ch, vec![components::sponsored_messages::SponsoredMessage {
        random_id: vec![1, 2, 3],
        title: "Telegram Stars".to_string(),
        text: "Buy stars now".to_string(),
        link: "https://t.me/stars".to_string(),
        button_text: "View".to_string(),
        sponsor_info: "Telegram".to_string(),
    }]);
    assert_eq!(sponsored.get_messages(ch).unwrap().len(), 1);
    sponsored.clear(ch);
    assert!(sponsored.get_messages(ch).is_none());

    // 5. Credits
    let mut credits = components::CreditsComponents::with_stars(500, 0.02);
    assert_eq!(credits.stars(), 500);
    assert!(credits.can_withdraw());
    credits.lock_withdraw();
    assert!(!credits.can_withdraw());
}
