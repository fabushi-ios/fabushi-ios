//! Adversarial Data Subsystem Tests
//! Module 03 Data Subsystem comprehensive adversarial verification suite.

use std::collections::{HashMap, HashSet};
use gpui_telegram_client::data::*;
use gpui_telegram_client::data::data_story::*;
use gpui_telegram_client::data::data_stories::*;
use gpui_telegram_client::data::data_poll::*;

#[test]
fn test_adversarial_peer_id_bitfields_and_fuzzing() {
    // 1. All discriminator bit ranges
    let u = PeerId::from_user(UserId(12345));
    assert!(u.is_user() && !u.is_chat() && !u.is_channel() && !u.is_fake());
    assert_eq!(u.shift(), PeerId::SHIFT_USER);

    let c = PeerId::from_chat(ChatId(23456));
    assert!(!c.is_user() && c.is_chat() && !c.is_channel() && !c.is_fake());
    assert_eq!(c.shift(), PeerId::SHIFT_CHAT);

    let ch = PeerId::from_channel(ChannelId(34567));
    assert!(!ch.is_user() && !ch.is_chat() && ch.is_channel() && !ch.is_fake());
    assert_eq!(ch.shift(), PeerId::SHIFT_CHANNEL);

    let f = PeerId::from_fake(FakeChatId(45678));
    assert!(!f.is_user() && !f.is_chat() && !f.is_channel() && f.is_fake());
    assert_eq!(f.shift(), PeerId::SHIFT_FAKE);

    // 2. Max bare ID bounds (2^48 - 1)
    let max_bare = (1u64 << 48) - 1;
    let u_max = PeerId::from_user(UserId(max_bare));
    assert_eq!(u_max.bare(), max_bare);
    assert_eq!(u_max.shift(), PeerId::SHIFT_USER);
    assert!(u_max.is_user());

    let ch_max = PeerId::from_channel(ChannelId(max_bare));
    assert_eq!(ch_max.bare(), max_bare);
    assert_eq!(ch_max.shift(), PeerId::SHIFT_CHANNEL);
    assert!(ch_max.is_channel());

    // 3. Roundtrip serialization for modern & legacy formats
    let raw_modern = u_max.serialize();
    let deser_modern = PeerId::deserialize(raw_modern);
    assert_eq!(deser_modern, u_max);

    let raw_ch = ch_max.serialize();
    let deser_ch = PeerId::deserialize(raw_ch);
    assert_eq!(deser_ch, ch_max);

    // Legacy serialization roundtrip
    let legacy_user_raw = 888999u64;
    let legacy_user = PeerId::deserialize(legacy_user_raw);
    assert!(legacy_user.is_user());
    assert_eq!(legacy_user.bare(), 888999);

    let legacy_chat_raw = 0x100000000u64 | 444555;
    let legacy_chat = PeerId::deserialize(legacy_chat_raw);
    assert!(legacy_chat.is_chat());
    assert_eq!(legacy_chat.bare(), 444555);

    let legacy_ch_raw = 0x200000000u64 | 111222;
    let legacy_ch = PeerId::deserialize(legacy_ch_raw);
    assert!(legacy_ch.is_channel());
    assert_eq!(legacy_ch.bare(), 111222);

    // 4. Invariant fuzzing over 10,000 randomized peer IDs
    let mut seen = HashSet::new();
    let mut map = HashMap::new();
    let mut state: u64 = 0x123456789ABCDEF0;
    for i in 0..10_000u64 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let bare = state & ((1u64 << 48) - 1);
        let peer = match i % 4 {
            0 => PeerId::from_user(UserId(bare)),
            1 => PeerId::from_chat(ChatId(bare)),
            2 => PeerId::from_channel(ChannelId(bare)),
            _ => PeerId::from_fake(FakeChatId(bare)),
        };

        let ser = peer.serialize();
        let deser = PeerId::deserialize(ser);
        assert_eq!(deser, peer);
        assert_eq!(deser.bare(), bare);

        seen.insert(peer);
        map.insert(peer, i);
    }
    assert_eq!(seen.len(), 10_000);
    assert_eq!(map.len(), 10_000);
}

#[test]
fn test_adversarial_chat_admin_rights_bitwise_fuzzing() {
    let mut rights = ChatAdminRights::default();
    assert_eq!(rights.0, 0);

    let all_flags = [
        ChatAdminRights::CHANGE_INFO,
        ChatAdminRights::POST_MESSAGES,
        ChatAdminRights::EDIT_MESSAGES,
        ChatAdminRights::DELETE_MESSAGES,
        ChatAdminRights::BAN_USERS,
        ChatAdminRights::INVITE_BY_LINK_OR_ADD,
        ChatAdminRights::PIN_MESSAGES,
        ChatAdminRights::ADD_ADMINS,
        ChatAdminRights::ANONYMOUS,
        ChatAdminRights::MANAGE_CALL,
        ChatAdminRights::OTHER,
        ChatAdminRights::MANAGE_TOPICS,
        ChatAdminRights::POST_STORIES,
        ChatAdminRights::EDIT_STORIES,
        ChatAdminRights::DELETE_STORIES,
        ChatAdminRights::MANAGE_DIRECT,
        ChatAdminRights::MANAGE_RANKS,
        ChatAdminRights::PROCESS_JOIN_REQUESTS,
        ChatAdminRights::MANAGE_LINKED_PEERS,
        ChatAdminRights::MANAGE_WELCOME_MESSAGES,
    ];

    for &flag in &all_flags {
        assert!(!rights.has(flag));
        rights.add(flag);
        assert!(rights.has(flag));
        rights.remove(flag);
        assert!(!rights.has(flag));
    }

    let group = ChatAdminRights(ChatAdminRights::ALL_GROUP_RIGHTS);
    assert!(group.has(ChatAdminRights::CHANGE_INFO));
    assert!(group.has(ChatAdminRights::BAN_USERS));

    let channel = ChatAdminRights(ChatAdminRights::ALL_CHANNEL_RIGHTS);
    assert!(channel.has(ChatAdminRights::POST_MESSAGES));
    assert!(channel.has(ChatAdminRights::EDIT_MESSAGES));

    let all = ChatAdminRights::all();
    for &flag in &all_flags {
        assert!(all.has(flag));
    }

    let mut restr = ChatRestrictionsInfo::default();
    restr.flags.add(ChatRestrictions::SEND_MESSAGES);
    restr.until = 1700000000;

    assert!(restr.is_restricted_at(ChatRestrictions::SEND_MESSAGES, 1699999999));
    assert!(!restr.is_restricted_at(ChatRestrictions::SEND_MESSAGES, 1700000000));
    assert!(!restr.is_restricted_at(ChatRestrictions::SEND_MESSAGES, 1700000001));

    restr.until = 0;
    assert!(restr.is_restricted_at(ChatRestrictions::SEND_MESSAGES, 1999999999));
}

#[test]
fn test_adversarial_chat_filters_precedence_and_edge_cases() {
    let mut filter = ChatFilter::new(1, "Work");
    let p_always = PeerId::from_user(UserId(101));
    let p_never = PeerId::from_user(UserId(102));
    let p_pinned = PeerId::from_user(UserId(103));
    let p_both = PeerId::from_user(UserId(104));

    filter.always.push(p_always);
    filter.always.push(p_both);
    filter.never.push(p_never);
    filter.never.push(p_both);
    filter.pinned.push(p_pinned);

    let cand_always = ChatFilterCandidate { peer_id: p_always, ..Default::default() };
    assert!(filter.matches(&cand_always));

    let cand_pinned = ChatFilterCandidate { peer_id: p_pinned, ..Default::default() };
    assert!(filter.matches(&cand_pinned));

    let cand_both = ChatFilterCandidate { peer_id: p_both, ..Default::default() };
    assert!(filter.matches(&cand_both));

    let cand_never = ChatFilterCandidate { peer_id: p_never, ..Default::default() };
    assert!(!filter.matches(&cand_never));

    let mut neg_filter = ChatFilter::new(2, "Unread & Unmuted");
    neg_filter.flags.add(ChatFilterFlags::NO_READ);
    neg_filter.flags.add(ChatFilterFlags::NO_MUTED);

    let cand_read = ChatFilterCandidate {
        peer_id: PeerId::from_user(UserId(201)),
        is_read: true,
        is_muted: false,
        ..Default::default()
    };
    assert!(!neg_filter.matches(&cand_read));

    let cand_muted = ChatFilterCandidate {
        peer_id: PeerId::from_user(UserId(202)),
        is_read: false,
        is_muted: true,
        ..Default::default()
    };
    assert!(!neg_filter.matches(&cand_muted));

    let cand_valid = ChatFilterCandidate {
        peer_id: PeerId::from_user(UserId(203)),
        is_read: false,
        is_muted: false,
        ..Default::default()
    };
    assert!(neg_filter.matches(&cand_valid));
}

#[test]
fn test_adversarial_story_expiration_boundary_arithmetic() {
    let now = 1700000000i64;
    let story = DataStory::new(1, 1001, now, 24);
    let expected_expires = now + 24 * 3600;
    assert_eq!(story.expires, expected_expires);

    assert!(!story.is_expired(expected_expires - 1));
    assert!(story.is_expired(expected_expires));
    assert!(story.is_expired(expected_expires + 1));

    let mut stories = DataStories::new(1001);
    stories.set_active_stories(vec![1, 2, 3, 4, 5], 2);
    assert_eq!(stories.unread_count(), 3);
    assert!(stories.has_unseen_stories());

    stories.mark_read(4);
    assert_eq!(stories.unread_count(), 1);

    stories.mark_read(5);
    assert_eq!(stories.unread_count(), 0);
    assert!(!stories.has_unseen_stories());
}

#[test]
fn test_adversarial_poll_voting_concurrency_and_limits() {
    let mut poll = DataPoll::new(1, "What is your favorite language?");
    poll.add_answer(vec![1], "Rust", true);
    poll.add_answer(vec![2], "C++", false);
    poll.add_answer(vec![3], "Go", false);

    assert_eq!(poll.total_voters, 0);
    assert!(poll.vote(&[]).is_err());
    assert!(poll.vote(&[vec![1], vec![2]]).is_err());

    assert!(poll.vote(&[vec![1]]).is_ok());
    assert_eq!(poll.total_voters, 1);
    assert_eq!(poll.answers[0].votes, 1);
    assert!(poll.voted());

    assert!(poll.vote(&[vec![2]]).is_ok());
    assert_eq!(poll.total_voters, 1);
    assert_eq!(poll.answers[0].votes, 0);
    assert_eq!(poll.answers[1].votes, 1);

    poll.close();
    assert!(poll.vote(&[vec![3]]).is_err());

    let mut multi_poll = DataPoll::new(2, "Select all languages you use");
    multi_poll.is_multi_choice = true;
    multi_poll.add_answer(vec![1], "Rust", false);
    multi_poll.add_answer(vec![2], "Python", false);
    multi_poll.add_answer(vec![3], "TypeScript", false);

    assert!(multi_poll.vote(&[vec![1], vec![3]]).is_ok());
    assert_eq!(multi_poll.total_voters, 1);
    assert_eq!(multi_poll.answers[0].votes, 1);
    assert_eq!(multi_poll.answers[1].votes, 0);
    assert_eq!(multi_poll.answers[2].votes, 1);
}
