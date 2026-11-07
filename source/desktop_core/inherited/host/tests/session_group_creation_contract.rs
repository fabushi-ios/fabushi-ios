use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use fabushi_messaging_core::{
    ActorId, ConversationId, MiniAppGrant, MiniAppPermission, MiniAppSession,
};
use mahayana_host_runtime::extensions::session::gateway::{
    SessionGatewayError, dispatch_production_session_gateway_call,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use serde_json::{Value, json};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "fabushi-session-group-{label}-{}-{suffix}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&path);
    path
}

fn runtime_for_user(root: &std::path::Path, user_id: &str) -> Arc<ProductionSessionWorkers> {
    let agents = root.join("agents");
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
            &agents,
            500,
            Some(user_id.to_string()),
            Arc::new(|| None),
            memory,
        ),
    )
}

fn dispatch_ok(
    runtime: &Arc<ProductionSessionWorkers>,
    method: &str,
    args: Value,
) -> Value {
    dispatch_production_session_gateway_call(runtime, method, &args)
        .unwrap_or_else(|| panic!("unhandled method {method}"))
        .unwrap_or_else(|err| panic!("dispatch {method} failed: {err:?}"))
}

fn dispatch_err(
    runtime: &Arc<ProductionSessionWorkers>,
    method: &str,
    args: Value,
) -> SessionGatewayError {
    dispatch_production_session_gateway_call(runtime, method, &args)
        .unwrap_or_else(|| panic!("unhandled method {method}"))
        .expect_err(&format!("expected dispatch {method} to fail, but it succeeded"))
}

#[test]
fn group_and_channel_creation_listing_and_direct_zero_regression() {
    let root = temp_root("creation-listing");
    let alice = runtime_for_user(&root, "user-alice");

    // 1. Create a 1:1 direct human conversation
    let direct = dispatch_ok(
        &alice,
        "createHumanConversation",
        json!({
            "peerHumanId": "user-bob",
            "title": "Alice and Bob"
        }),
    );
    assert_eq!(direct["kind"], "human");
    let direct_id = direct["id"].as_str().expect("direct id").to_string();
    assert!(direct_id.starts_with("human-direct-"));

    // 2. Create a Group conversation
    let group = dispatch_ok(
        &alice,
        "createGroupConversation",
        json!({
            "groupId": "engineers-core",
            "title": "Core Engineering",
            "participantIds": ["user-bob", "user-charlie"],
            "slowModeSeconds": 10
        }),
    );
    assert_eq!(group["kind"], "group");
    assert_eq!(group["id"], "group-engineers-core");
    assert_eq!(group["title"], "Core Engineering");
    assert_eq!(group["slowModeSeconds"], 10);
    let participants = group["participantIds"].as_array().expect("participants");
    assert!(participants.iter().any(|p| p == "user-alice"));
    assert!(participants.iter().any(|p| p == "user-bob"));
    assert!(participants.iter().any(|p| p == "user-charlie"));

    // 3. Create a Channel conversation
    let channel = dispatch_ok(
        &alice,
        "createChannelConversation",
        json!({
            "channelId": "announcements",
            "title": "Company Announcements",
            "participantIds": ["user-bob"]
        }),
    );
    assert_eq!(channel["kind"], "channel");
    assert_eq!(channel["id"], "channel-announcements");
    assert_eq!(channel["title"], "Company Announcements");

    // 4. Verify listHumanConversations lists all three with distinct kinds
    let list = dispatch_ok(&alice, "listHumanConversations", json!({}));
    let items = list.as_array().expect("conversations array");
    assert_eq!(items.len(), 3);

    let direct_item = items.iter().find(|c| c["id"] == direct_id).expect("direct item");
    assert_eq!(direct_item["kind"], "human");

    let group_item = items.iter().find(|c| c["id"] == "group-engineers-core").expect("group item");
    assert_eq!(group_item["kind"], "group");

    let channel_item = items.iter().find(|c| c["id"] == "channel-announcements").expect("channel item");
    assert_eq!(channel_item["kind"], "channel");
}

#[test]
fn community_send_policy_member_restrictions_and_channel_posting() {
    let root = temp_root("send-policy");
    let alice = runtime_for_user(&root, "user-alice"); // Owner
    let bob = runtime_for_user(&root, "user-bob");     // Member

    // Create channel with Alice (Owner) and Bob (Member)
    dispatch_ok(
        &alice,
        "createChannelConversation",
        json!({
            "channelId": "tech-broadcast",
            "title": "Tech Broadcast",
            "participantIds": ["user-bob"]
        }),
    );

    // Channel post test: Alice as Owner can post
    let msg_alice = dispatch_ok(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": "channel-tech-broadcast",
            "text": "Broadcast from Alice",
            "clientNonce": "alice-msg-1"
        }),
    );
    assert_eq!(msg_alice["content"], "Broadcast from Alice");

    // Bob as regular channel member CANNOT post
    let err_bob = dispatch_err(
        &bob,
        "sendHumanMessage",
        json!({
            "conversationId": "channel-tech-broadcast",
            "text": "Bob attempting to post",
            "clientNonce": "bob-msg-1"
        }),
    );
    let err_str = err_bob.to_string();
    assert!(
        err_str.contains("ChannelPostNotAllowed"),
        "expected ChannelPostNotAllowed, got: {err_str}"
    );

    // Create group conversation
    dispatch_ok(
        &alice,
        "createGroupConversation",
        json!({
            "groupId": "dev-chat",
            "title": "Dev Chat",
            "participantIds": ["user-bob"]
        }),
    );

    // Bob can post in the group
    let bob_msg = dispatch_ok(
        &bob,
        "sendHumanMessage",
        json!({
            "conversationId": "group-dev-chat",
            "text": "Hello devs",
            "clientNonce": "bob-msg-2"
        }),
    );
    assert_eq!(bob_msg["content"], "Hello devs");

    // Restrict Bob from sending messages
    let mut restricted_bob = fabushi_messaging_core::CommunityMember {
        actor_id: fabushi_messaging_core::ActorId::new("user-bob"),
        status: fabushi_messaging_core::MemberStatus::Restricted,
        admin_title: None,
        admin_rights: fabushi_messaging_core::AdminRights::default(),
        restrictions: fabushi_messaging_core::MemberRestrictions {
            send_messages: true,
            ..Default::default()
        },
        joined_at_ms: 1000,
        invited_by: None,
    };
    dispatch_ok(
        &alice,
        "upsertConversationMember",
        json!({
            "conversationId": "group-dev-chat",
            "member": restricted_bob
        }),
    );

    // Bob now fails to send messages
    let err_restricted = dispatch_err(
        &bob,
        "sendHumanMessage",
        json!({
            "conversationId": "group-dev-chat",
            "text": "Bob trying to send while restricted",
            "clientNonce": "bob-msg-3"
        }),
    );
    assert!(err_restricted.to_string().contains("SendMessagesRestricted"));

    // Ban Bob
    restricted_bob.status = fabushi_messaging_core::MemberStatus::Banned;
    dispatch_ok(
        &alice,
        "upsertConversationMember",
        json!({
            "conversationId": "group-dev-chat",
            "member": restricted_bob
        }),
    );

    let err_banned = dispatch_err(
        &bob,
        "sendHumanMessage",
        json!({
            "conversationId": "group-dev-chat",
            "text": "Bob trying to send while banned",
            "clientNonce": "bob-msg-4"
        }),
    );
    assert!(
        err_banned.to_string().contains("MemberBanned")
            || err_banned.to_string().contains("not a conversation participant")
    );
}

#[test]
fn slow_mode_cooldown_and_owner_exemption() {
    let root = temp_root("slow-mode");
    let alice = runtime_for_user(&root, "user-alice"); // Owner
    let bob = runtime_for_user(&root, "user-bob");     // Member

    // Create group with 10s slow mode
    dispatch_ok(
        &alice,
        "createGroupConversation",
        json!({
            "groupId": "slow-group",
            "title": "Slow Mode Room",
            "participantIds": ["user-bob"],
            "slowModeSeconds": 10
        }),
    );

    // Bob sends message 1 -> succeeds
    let bob_msg1 = dispatch_ok(
        &bob,
        "sendHumanMessage",
        json!({
            "conversationId": "group-slow-group",
            "text": "First message from Bob",
            "clientNonce": "bob-slow-1"
        }),
    );
    assert_eq!(bob_msg1["content"], "First message from Bob");

    // Bob immediately attempts to send message 2 -> rejected by SlowModeActive
    let err_slow = dispatch_err(
        &bob,
        "sendHumanMessage",
        json!({
            "conversationId": "group-slow-group",
            "text": "Second message from Bob immediately",
            "clientNonce": "bob-slow-2"
        }),
    );
    let err_str = err_slow.to_string();
    assert!(
        err_str.contains("SlowModeActive"),
        "expected SlowModeActive, got: {err_str}"
    );

    // Alice as Owner is exempt: can send multiple messages consecutively
    dispatch_ok(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": "group-slow-group",
            "text": "Alice 1",
            "clientNonce": "alice-slow-1"
        }),
    );
    dispatch_ok(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": "group-slow-group",
            "text": "Alice 2",
            "clientNonce": "alice-slow-2"
        }),
    );
}

#[test]
fn forum_topic_lifecycle_and_administrative_authorization() {
    let root = temp_root("forum-topics");
    let alice = runtime_for_user(&root, "user-alice"); // Owner
    let bob = runtime_for_user(&root, "user-bob");     // Member

    dispatch_ok(
        &alice,
        "createGroupConversation",
        json!({
            "groupId": "forum-group",
            "title": "Forum Group",
            "participantIds": ["user-bob"]
        }),
    );

    // Non-admin (Bob) attempting to create topic is rejected
    let err_non_admin = dispatch_err(
        &bob,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-forum-group",
            "topicId": "topic-general",
            "title": "General Discussion"
        }),
    );
    assert!(err_non_admin.to_string().contains("manage_topics"));

    // Owner (Alice) creates topic-general
    let topic_created = dispatch_ok(
        &alice,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-forum-group",
            "topicId": "topic-general",
            "title": "General Discussion",
            "icon": "💬",
            "pinned": true
        }),
    );
    assert_eq!(topic_created["id"], "topic-general");
    assert_eq!(topic_created["title"], "General Discussion");
    assert_eq!(topic_created["pinned"], true);
    assert_eq!(topic_created["closed"], false);

    // List topics
    let topics = dispatch_ok(
        &alice,
        "listConversationTopics",
        json!({ "conversationId": "group-forum-group" }),
    );
    let topic_list = topics.as_array().expect("topics array");
    assert_eq!(topic_list.len(), 1);
    assert_eq!(topic_list[0]["id"], "topic-general");

    // Bob can send to open topic-general
    let msg_topic = dispatch_ok(
        &bob,
        "sendHumanMessage",
        json!({
            "conversationId": "group-forum-group",
            "text": "Bob in general topic",
            "clientNonce": "bob-topic-1",
            "topicId": "topic-general"
        }),
    );
    assert_eq!(msg_topic["topicId"], "topic-general");

    // Alice closes the topic
    dispatch_ok(
        &alice,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-forum-group",
            "topicId": "topic-general",
            "title": "General Discussion",
            "closed": true
        }),
    );

    // Bob cannot send to closed topic
    let err_closed = dispatch_err(
        &bob,
        "sendHumanMessage",
        json!({
            "conversationId": "group-forum-group",
            "text": "Bob trying in closed topic",
            "clientNonce": "bob-topic-2",
            "topicId": "topic-general"
        }),
    );
    assert!(err_closed.to_string().contains("TopicClosed"));

    // Alice as Owner CAN post in closed topic
    let alice_closed_msg = dispatch_ok(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": "group-forum-group",
            "text": "Alice posting in closed topic announcement",
            "clientNonce": "alice-topic-closed-1",
            "topicId": "topic-general"
        }),
    );
    assert_eq!(alice_closed_msg["topicId"], "topic-general");
}

#[test]
fn forward_to_topic_destination_contract() {
    let root = temp_root("forward-topic");
    let alice = runtime_for_user(&root, "user-alice");

    // Setup source conversation with a message
    let direct = dispatch_ok(
        &alice,
        "createHumanConversation",
        json!({
            "peerHumanId": "user-bob",
            "title": "Alice and Bob Direct"
        }),
    );
    let source_conv_id = direct["id"].as_str().expect("source conv id");
    let sent_source = dispatch_ok(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": source_conv_id,
            "text": "Original message to forward",
            "clientNonce": "source-nonce-1"
        }),
    );
    let source_entry_id = sent_source["id"].as_str().expect("source entry id");

    // Setup destination group with a topic
    dispatch_ok(
        &alice,
        "createGroupConversation",
        json!({
            "groupId": "target-group",
            "title": "Target Group",
            "participantIds": ["user-bob"]
        }),
    );
    dispatch_ok(
        &alice,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-target-group",
            "topicId": "topic-incoming",
            "title": "Incoming Shares"
        }),
    );

    // Forward to destination group and specify destinationTopics mapping + comment
    let forward_res = dispatch_ok(
        &alice,
        "forwardHumanMessage",
        json!({
            "sourceConversationId": source_conv_id,
            "sourceEntryId": source_entry_id,
            "destinationConversationIds": ["group-target-group"],
            "destinationTopics": {
                "group-target-group": "topic-incoming"
            },
            "clientNonce": "fwd-nonce-1",
            "comment": "Check this out!"
        }),
    );
    let destinations = forward_res["destinations"].as_array().expect("destinations");
    assert_eq!(destinations.len(), 1);
    assert_eq!(destinations[0]["status"], "sent");

    // Verify transcript in target group: comment and forwarded message both stamped with topic-incoming
    let owner = alice
        .open_human_conversation_db_owner("group-target-group")
        .expect("target db owner");
    let entries = owner.get_transcript_entries().expect("transcript");
    assert_eq!(entries.len(), 2);

    let comment_entry = &entries[0];
    assert_eq!(comment_entry["content"], "Check this out!");
    assert_eq!(comment_entry["topicId"], "topic-incoming");

    let fwd_entry = &entries[1];
    assert_eq!(fwd_entry["content"], "Original message to forward");
    assert_eq!(fwd_entry["topicId"], "topic-incoming");
}

#[test]
fn topic_destination_eligibility_fail_closed_contract() {
    let root = temp_root("topic-eligibility");
    let alice = runtime_for_user(&root, "user-alice"); // Owner
    let bob = runtime_for_user(&root, "user-bob");     // Regular member in target group

    // Alice creates target group
    dispatch_ok(
        &alice,
        "createGroupConversation",
        json!({
            "groupId": "target-community",
            "title": "Target Community",
            "participantIds": ["user-bob"]
        }),
    );

    // Create a closed topic
    dispatch_ok(
        &alice,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-target-community",
            "topicId": "topic-closed",
            "title": "Closed Topic",
            "closed": true
        }),
    );

    // Create a hidden topic
    dispatch_ok(
        &alice,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-target-community",
            "topicId": "topic-hidden",
            "title": "Hidden Topic",
            "hidden": true
        }),
    );

    // Bob creates source message in direct conversation with Alice
    let bob_source = dispatch_ok(
        &bob,
        "createHumanConversation",
        json!({
            "peerHumanId": "user-alice",
            "title": "Bob and Alice Direct"
        }),
    );
    assert_eq!(bob_source["kind"], "human");
    let bob_source_conv_id = bob_source["id"].as_str().expect("bob source conv id");
    let bob_msg = dispatch_ok(
        &bob,
        "sendHumanMessage",
        json!({
            "conversationId": bob_source_conv_id,
            "text": "Bob's source message",
            "clientNonce": "bob-elig-src-1"
        }),
    );
    let bob_source_entry_id = bob_msg["id"].as_str().expect("bob source id");

    // 1. Bob forwards to non-existent topic -> fails closed (TopicNotFound)
    let fwd_missing = dispatch_ok(
        &bob,
        "forwardHumanMessage",
        json!({
            "sourceConversationId": bob_source_conv_id,
            "sourceEntryId": bob_source_entry_id,
            "destinationConversationIds": ["group-target-community"],
            "destinationTopics": {
                "group-target-community": "topic-does-not-exist"
            },
            "clientNonce": "fwd-missing-1"
        }),
    );
    let dest_missing = &fwd_missing["destinations"][0];
    assert_eq!(dest_missing["status"], "failed");
    assert!(
        dest_missing["error"]
            .as_str()
            .unwrap_or_default()
            .contains("TopicNotFound"),
        "error should contain TopicNotFound: {:?}",
        dest_missing["error"]
    );

    // 2. Bob forwards to closed topic -> fails closed (TopicClosed)
    let fwd_closed = dispatch_ok(
        &bob,
        "forwardHumanMessage",
        json!({
            "sourceConversationId": bob_source_conv_id,
            "sourceEntryId": bob_source_entry_id,
            "destinationConversationIds": ["group-target-community"],
            "destinationTopics": {
                "group-target-community": "topic-closed"
            },
            "clientNonce": "fwd-closed-1"
        }),
    );
    let dest_closed = &fwd_closed["destinations"][0];
    assert_eq!(dest_closed["status"], "failed");
    assert!(
        dest_closed["error"]
            .as_str()
            .unwrap_or_default()
            .contains("TopicClosed"),
        "error should contain TopicClosed: {:?}",
        dest_closed["error"]
    );

    // 3. Bob forwards to hidden topic -> fails closed (TopicNotFound)
    let fwd_hidden = dispatch_ok(
        &bob,
        "forwardHumanMessage",
        json!({
            "sourceConversationId": bob_source_conv_id,
            "sourceEntryId": bob_source_entry_id,
            "destinationConversationIds": ["group-target-community"],
            "destinationTopics": {
                "group-target-community": "topic-hidden"
            },
            "clientNonce": "fwd-hidden-1"
        }),
    );
    let dest_hidden = &fwd_hidden["destinations"][0];
    assert_eq!(dest_hidden["status"], "failed");
    assert!(
        dest_hidden["error"]
            .as_str()
            .unwrap_or_default()
            .contains("TopicNotFound"),
        "error should contain TopicNotFound: {:?}",
        dest_hidden["error"]
    );

    // 4. Verify target group transcript has NOT been mutated by any of the failed forwards!
    let target_owner = alice
        .open_human_conversation_db_owner("group-target-community")
        .expect("target db owner");
    let entries = target_owner.get_transcript_entries().expect("transcript");
    assert!(
        entries.is_empty(),
        "fail-closed policy must not leave any transcript rows on failed topic destinations!"
    );
}

#[test]
fn pin_and_unpin_conversation_and_topic_messages_enforces_admin_rights() {
    let root = temp_root("pin-unpin");
    let alice = runtime_for_user(&root, "user-alice"); // Owner
    let bob = runtime_for_user(&root, "user-bob");     // Member (later Admin)
    let charlie = runtime_for_user(&root, "user-charlie"); // Regular Member

    // Create group
    dispatch_ok(
        &alice,
        "createGroupConversation",
        json!({
            "groupId": "pin-test",
            "title": "Pin Test Group",
            "participantIds": ["user-bob", "user-charlie"]
        }),
    );

    // Create topic
    dispatch_ok(
        &alice,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-pin-test",
            "topicId": "topic-dev",
            "title": "Dev Topic"
        }),
    );

    // Alice sends messages in main and topic
    let msg_main = dispatch_ok(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": "group-pin-test",
            "text": "Main announcement",
            "clientNonce": "alice-pin-main-1"
        }),
    );
    let msg_main_id = msg_main["id"].as_str().expect("main msg id");

    let msg_topic = dispatch_ok(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": "group-pin-test",
            "text": "Dev topic announcement",
            "clientNonce": "alice-pin-topic-1",
            "topicId": "topic-dev"
        }),
    );
    let msg_topic_id = msg_topic["id"].as_str().expect("topic msg id");

    // Regular member (Charlie) tries to pin -> fails closed
    let err_charlie = dispatch_err(
        &charlie,
        "pinConversationMessage",
        json!({
            "conversationId": "group-pin-test",
            "messageId": msg_main_id
        }),
    );
    assert!(err_charlie.to_string().contains("pin_messages"));

    // Owner (Alice) pins main message
    let pin_main_res = dispatch_ok(
        &alice,
        "pinConversationMessage",
        json!({
            "conversationId": "group-pin-test",
            "messageId": msg_main_id
        }),
    );
    assert_eq!(pin_main_res["childScope"], "main");
    assert_eq!(pin_main_res["pinnedBy"], "user-alice");
    assert_eq!(pin_main_res["pinnedMessageIds"], json!([msg_main_id]));

    // Alice pins topic message in topic-dev
    let pin_topic_res = dispatch_ok(
        &alice,
        "pinConversationMessage",
        json!({
            "conversationId": "group-pin-test",
            "messageId": msg_topic_id,
            "topicId": "topic-dev"
        }),
    );
    assert_eq!(pin_topic_res["childScope"], "topic:topic-dev");
    assert_eq!(pin_topic_res["pinnedBy"], "user-alice");
    assert_eq!(pin_topic_res["pinnedMessageIds"], json!([msg_topic_id]));

    // Verify scope isolation via listConversationPins
    let main_pins = dispatch_ok(
        &alice,
        "listConversationPins",
        json!({ "conversationId": "group-pin-test" }),
    );
    assert_eq!(main_pins, json!([msg_main_id]));

    let topic_pins = dispatch_ok(
        &alice,
        "listConversationPins",
        json!({
            "conversationId": "group-pin-test",
            "topicId": "topic-dev"
        }),
    );
    assert_eq!(topic_pins, json!([msg_topic_id]));

    // Promote Bob to Administrator with pin_messages: true
    let admin_bob = fabushi_messaging_core::CommunityMember {
        actor_id: fabushi_messaging_core::ActorId::new("user-bob"),
        status: fabushi_messaging_core::MemberStatus::Administrator,
        admin_title: Some("Moderator".into()),
        admin_rights: fabushi_messaging_core::AdminRights {
            pin_messages: true,
            ..Default::default()
        },
        restrictions: Default::default(),
        joined_at_ms: 1000,
        invited_by: None,
    };
    dispatch_ok(
        &alice,
        "upsertConversationMember",
        json!({
            "conversationId": "group-pin-test",
            "member": admin_bob
        }),
    );

    // Bob as Admin unpins topic message
    let unpin_res = dispatch_ok(
        &bob,
        "unpinConversationMessage",
        json!({
            "conversationId": "group-pin-test",
            "messageId": msg_topic_id,
            "topicId": "topic-dev"
        }),
    );
    assert_eq!(unpin_res["pinnedMessageIds"], json!([]));

    let topic_pins_after = dispatch_ok(
        &bob,
        "listConversationPins",
        json!({
            "conversationId": "group-pin-test",
            "topicId": "topic-dev"
        }),
    );
    assert_eq!(topic_pins_after, json!([]));

    // Main pin remains intact
    let main_pins_after = dispatch_ok(
        &bob,
        "listConversationPins",
        json!({ "conversationId": "group-pin-test" }),
    );
    assert_eq!(main_pins_after, json!([msg_main_id]));
}

#[test]
fn topic_mute_and_mention_override_notification_evaluation() {
    let root = temp_root("topic-mute");
    let alice = runtime_for_user(&root, "user-alice"); // Owner / Sender
    let bob = runtime_for_user(&root, "user-bob");     // Member / Recipient

    dispatch_ok(
        &alice,
        "createGroupConversation",
        json!({
            "groupId": "mute-test",
            "title": "Mute Test Group",
            "participantIds": ["user-bob"]
        }),
    );

    dispatch_ok(
        &alice,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-mute-test",
            "topicId": "topic-muted",
            "title": "Muted Topic"
        }),
    );

    dispatch_ok(
        &alice,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-mute-test",
            "topicId": "topic-normal",
            "title": "Normal Topic"
        }),
    );

    // Bob mutes topic-muted for 1 hour
    let far_future = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64 + 3_600_000;
    dispatch_ok(
        &bob,
        "setConversationTopicMute",
        json!({
            "conversationId": "group-mute-test",
            "topicId": "topic-muted",
            "mutedUntilMs": far_future
        }),
    );

    // 1. Ordinary message in muted topic
    let msg_muted = dispatch_ok(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": "group-mute-test",
            "text": "Quiet update in muted topic",
            "clientNonce": "alice-mute-1",
            "topicId": "topic-muted"
        }),
    );

    // 2. Mention message in muted topic (penetrates mute)
    let msg_mention = dispatch_ok(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": "group-mute-test",
            "text": "Hey @user-bob check this out!",
            "clientNonce": "alice-mute-2",
            "topicId": "topic-muted"
        }),
    );

    // 3. Ordinary message in unmuted topic
    let msg_normal = dispatch_ok(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": "group-mute-test",
            "text": "Normal topic update",
            "clientNonce": "alice-mute-3",
            "topicId": "topic-normal"
        }),
    );

    // Bob evaluates notifications
    let eval_muted = dispatch_ok(
        &bob,
        "evaluateMessageNotification",
        json!({
            "conversationId": "group-mute-test",
            "messageId": msg_muted["id"]
        }),
    );
    assert_eq!(eval_muted["decision"], "Suppressed");
    assert_eq!(eval_muted["reason"], "TopicMuted");
    assert_eq!(eval_muted["deliver"], false);

    let eval_mention = dispatch_ok(
        &bob,
        "evaluateMessageNotification",
        json!({
            "conversationId": "group-mute-test",
            "messageId": msg_mention["id"]
        }),
    );
    assert_eq!(eval_mention["decision"], "Notify");
    assert_eq!(eval_mention["deliver"], true);

    let eval_normal = dispatch_ok(
        &bob,
        "evaluateMessageNotification",
        json!({
            "conversationId": "group-mute-test",
            "messageId": msg_normal["id"]
        }),
    );
    assert_eq!(eval_normal["decision"], "Notify");
    assert_eq!(eval_normal["deliver"], true);
}

#[test]
fn typing_publication_throttling_and_permission_enforcement() {
    let root = temp_root("typing-pub");
    let alice = runtime_for_user(&root, "user-alice"); // Owner
    let bob = runtime_for_user(&root, "user-bob");     // Member

    dispatch_ok(
        &alice,
        "createGroupConversation",
        json!({
            "groupId": "typing-test",
            "title": "Typing Test Room",
            "participantIds": ["user-bob"]
        }),
    );

    dispatch_ok(
        &alice,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-typing-test",
            "topicId": "topic-chat",
            "title": "Chat Topic"
        }),
    );

    // First typing publication -> published
    let t1 = dispatch_ok(
        &bob,
        "publishConversationTyping",
        json!({
            "conversationId": "group-typing-test",
            "topicId": "topic-chat",
            "action": "typing"
        }),
    );
    assert_eq!(t1["decision"], "published");
    assert_eq!(t1["actorId"], "user-bob");

    // Immediate second call -> throttled
    let t2 = dispatch_ok(
        &bob,
        "publishConversationTyping",
        json!({
            "conversationId": "group-typing-test",
            "topicId": "topic-chat",
            "action": "typing"
        }),
    );
    assert_eq!(t2["decision"], "throttled");
    assert_eq!(t2["actorId"], "user-bob");

    // Alice closes the topic -> Bob cannot publish typing
    dispatch_ok(
        &alice,
        "upsertConversationTopic",
        json!({
            "conversationId": "group-typing-test",
            "topicId": "topic-chat",
            "title": "Chat Topic",
            "closed": true
        }),
    );
    let err_closed = dispatch_err(
        &bob,
        "publishConversationTyping",
        json!({
            "conversationId": "group-typing-test",
            "topicId": "topic-chat"
        }),
    );
    assert!(err_closed.to_string().contains("TopicClosed") || err_closed.to_string().contains("closed"));

    // Restrict Bob from sending messages
    let restricted_bob = fabushi_messaging_core::CommunityMember {
        actor_id: fabushi_messaging_core::ActorId::new("user-bob"),
        status: fabushi_messaging_core::MemberStatus::Restricted,
        admin_title: None,
        admin_rights: Default::default(),
        restrictions: fabushi_messaging_core::MemberRestrictions {
            send_messages: true,
            ..Default::default()
        },
        joined_at_ms: 1000,
        invited_by: None,
    };
    dispatch_ok(
        &alice,
        "upsertConversationMember",
        json!({
            "conversationId": "group-typing-test",
            "member": restricted_bob
        }),
    );
    let err_restricted = dispatch_err(
        &bob,
        "publishConversationTyping",
        json!({ "conversationId": "group-typing-test" }),
    );
    assert!(err_restricted.to_string().contains("SendMessagesRestricted") || err_restricted.to_string().contains("restricted"));
}

#[test]
fn media_preflight_and_batch_limit_contract() {
    let root = temp_root("media-preflight");
    let alice = runtime_for_user(&root, "user-alice");

    // 1. Valid attachment preflight
    let valid = dispatch_ok(
        &alice,
        "validateMediaAttachmentPreflight",
        json!({
            "fileName": "avatar.png",
            "mimeType": "image/png",
            "sizeBytes": 1024,
            "width": 256,
            "height": 256
        }),
    );
    assert_eq!(valid["valid"], true);
    assert_eq!(valid["fileName"], "avatar.png");
    assert_eq!(valid["sizeBytes"], 1024);

    // 2. Empty filename rejected
    let err_empty = dispatch_err(
        &alice,
        "validateMediaAttachmentPreflight",
        json!({
            "fileName": "",
            "mimeType": "image/png",
            "sizeBytes": 1024
        }),
    );
    assert!(err_empty.to_string().contains("EmptyFileName") || err_empty.to_string().contains("empty"));

    // 3. Zero-byte attachment rejected
    let err_zero = dispatch_err(
        &alice,
        "validateMediaAttachmentPreflight",
        json!({
            "fileName": "empty.png",
            "mimeType": "image/png",
            "sizeBytes": 0
        }),
    );
    assert!(err_zero.to_string().contains("ZeroByteAttachment") || err_zero.to_string().contains("zero"));

    // 4. Over 32 MiB rejected
    let oversized = 33 * 1024 * 1024;
    let err_large = dispatch_err(
        &alice,
        "validateMediaAttachmentPreflight",
        json!({
            "fileName": "oversized.zip",
            "mimeType": "application/zip",
            "sizeBytes": oversized
        }),
    );
    assert!(err_large.to_string().contains("AttachmentTooLarge") || err_large.to_string().contains("exceeds"));

    // 5. sendHumanMessage with > 6 attachments fails closed
    let direct = dispatch_ok(
        &alice,
        "createHumanConversation",
        json!({
            "peerHumanId": "user-bob",
            "title": "Direct Media Test"
        }),
    );
    let direct_id = direct["id"].as_str().expect("direct conversation id");

    let seven_attachments = json!([
        { "name": "f1.txt", "path": "/tmp/f1.txt" },
        { "name": "f2.txt", "path": "/tmp/f2.txt" },
        { "name": "f3.txt", "path": "/tmp/f3.txt" },
        { "name": "f4.txt", "path": "/tmp/f4.txt" },
        { "name": "f5.txt", "path": "/tmp/f5.txt" },
        { "name": "f6.txt", "path": "/tmp/f6.txt" },
        { "name": "f7.txt", "path": "/tmp/f7.txt" }
    ]);
    let err_batch = dispatch_err(
        &alice,
        "sendHumanMessage",
        json!({
            "conversationId": direct_id,
            "text": "Batch of 7 attachments",
            "clientNonce": "alice-media-batch-1",
            "attachments": seven_attachments
        }),
    );
    assert!(
        err_batch.to_string().contains("TooManyAttachments")
            || err_batch.to_string().contains("attachment preflight failed")
            || err_batch.to_string().contains("exceeds maximum")
    );
}

#[test]
fn media_cache_scoped_conversation_and_topic_lifecycle_contract() {
    let root = temp_root("media-cache-contract");
    let alice = runtime_for_user(&root, "user-alice");

    let direct1 = dispatch_ok(
        &alice,
        "createHumanConversation",
        json!({
            "peerHumanId": "user-bob",
            "title": "Alice and Bob"
        }),
    );
    let conv1 = direct1["id"].as_str().expect("conv1 id");

    let direct2 = dispatch_ok(
        &alice,
        "createHumanConversation",
        json!({
            "peerHumanId": "user-charlie",
            "title": "Alice and Charlie"
        }),
    );
    let conv2 = direct2["id"].as_str().expect("conv2 id");

    // 1. Cache media entries for conv1 across topicA and topicB
    let c1 = dispatch_ok(
        &alice,
        "cacheConversationMedia",
        json!({
            "conversationId": conv1,
            "topicId": "topic-a",
            "key": "m1.png",
            "sizeBytes": 100,
            "mimeType": "image/png"
        }),
    );
    assert_eq!(c1["cached"], true);
    assert_eq!(c1["totalBytesForConversation"], 100);

    let c2 = dispatch_ok(
        &alice,
        "cacheConversationMedia",
        json!({
            "conversationId": conv1,
            "topicId": "topic-a",
            "key": "m2.png",
            "sizeBytes": 200,
            "mimeType": "image/png"
        }),
    );
    assert_eq!(c2["cached"], true);
    assert_eq!(c2["totalBytesForConversation"], 300);

    let c3 = dispatch_ok(
        &alice,
        "cacheConversationMedia",
        json!({
            "conversationId": conv1,
            "topicId": "topic-b",
            "key": "m3.png",
            "sizeBytes": 300,
            "mimeType": "image/png"
        }),
    );
    assert_eq!(c3["cached"], true);
    assert_eq!(c3["totalBytesForConversation"], 600);

    // Cache media for conv2 (no topic)
    let c4 = dispatch_ok(
        &alice,
        "cacheConversationMedia",
        json!({
            "conversationId": conv2,
            "key": "m4.png",
            "sizeBytes": 400,
            "mimeType": "image/png"
        }),
    );
    assert_eq!(c4["cached"], true);
    assert_eq!(c4["totalBytesForConversation"], 400);

    // 2. Query stats
    let stats1 = dispatch_ok(
        &alice,
        "getConversationMediaCacheStats",
        json!({
            "conversationId": conv1
        }),
    );
    assert_eq!(stats1["totalBytesForConversation"], 600);
    assert_eq!(stats1["entryCount"], 3);

    let stats2 = dispatch_ok(
        &alice,
        "getConversationMediaCacheStats",
        json!({
            "conversationId": conv2
        }),
    );
    assert_eq!(stats2["totalBytesForConversation"], 400);
    assert_eq!(stats2["entryCount"], 1);

    // 3. Clear only topic-a in conv1
    let clear_topic = dispatch_ok(
        &alice,
        "clearConversationMediaCache",
        json!({
            "conversationId": conv1,
            "topicId": "topic-a"
        }),
    );
    assert_eq!(clear_topic["cleared"], true);
    assert_eq!(clear_topic["clearedCount"], 2);
    assert_eq!(clear_topic["clearedBytes"], 300);
    assert_eq!(clear_topic["remainingBytesForConversation"], 300);

    let stats1_after_topic = dispatch_ok(
        &alice,
        "getConversationMediaCacheStats",
        json!({
            "conversationId": conv1
        }),
    );
    assert_eq!(stats1_after_topic["totalBytesForConversation"], 300);
    assert_eq!(stats1_after_topic["entryCount"], 1);

    // 4. Clear all media in conv1
    let clear_conv = dispatch_ok(
        &alice,
        "clearConversationMediaCache",
        json!({
            "conversationId": conv1
        }),
    );
    assert_eq!(clear_conv["cleared"], true);
    assert_eq!(clear_conv["clearedCount"], 1);
    assert_eq!(clear_conv["clearedBytes"], 300);
    assert_eq!(clear_conv["remainingBytesForConversation"], 0);

    // conv2 should still be completely intact
    let stats2_after = dispatch_ok(
        &alice,
        "getConversationMediaCacheStats",
        json!({
            "conversationId": conv2
        }),
    );
    assert_eq!(stats2_after["totalBytesForConversation"], 400);
    assert_eq!(stats2_after["entryCount"], 1);
}

#[test]
fn miniapp_open_invoice_and_service_call_permission_and_lifecycle_contract() {
    let root = temp_root("miniapp-ext-contract");
    let alice = runtime_for_user(&root, "user-alice");

    // 1. Create a direct conversation
    let direct = dispatch_ok(
        &alice,
        "createHumanConversation",
        json!({
            "peerHumanId": "user-bob",
            "title": "Alice and Bob"
        }),
    );
    let conv_id = direct["id"].as_str().expect("conv id").to_string();

    let app_id = "org.fabushi.service-suite";
    let launch_session_id = "launch-sess-001";
    let now_ms = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis()) as i64;

    // Register a base launch session without Payments or ServiceCall permissions
    let base_session = MiniAppSession {
        id: launch_session_id.to_string(),
        mini_app_id: app_id.to_string(),
        actor_id: ActorId::new("user-alice"),
        conversation_id: Some(ConversationId::new(conv_id.clone())),
        start_parameter: Some("test_start".into()),
        granted_permissions: vec![],
        opened_at_ms: now_ms,
        expires_at_ms: now_ms + 3_600_000,
    };
    let base_grant = MiniAppGrant {
        mini_app_id: app_id.to_string(),
        actor_id: ActorId::new("user-alice"),
        permissions: vec![],
        granted_at_ms: now_ms,
        expires_at_ms: None,
    };
    alice
        .register_miniapp_launch_session_for_test(base_session.clone(), base_grant.clone())
        .expect("register base launch session");

    // 2. OpenInvoice without Payments permission fails closed
    let err_invoice_nogrant = dispatch_err(
        &alice,
        "miniAppPostToConversation",
        json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-inv-1",
            "request": {
                "type": "openInvoice",
                "invoiceId": "inv-9988"
            }
        }),
    );
    assert!(
        err_invoice_nogrant.to_string().contains("Payments")
            || err_invoice_nogrant.to_string().contains("permission denied")
    );

    // Now grant Payments permission
    let invoice_session = MiniAppSession {
        granted_permissions: vec![MiniAppPermission::Payments],
        ..base_session.clone()
    };
    let invoice_grant = MiniAppGrant {
        permissions: vec![MiniAppPermission::Payments],
        ..base_grant.clone()
    };
    alice
        .register_miniapp_launch_session_for_test(invoice_session, invoice_grant)
        .expect("update session with payments");

    let invoice_ok = dispatch_ok(
        &alice,
        "miniAppPostToConversation",
        json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-inv-1",
            "request": {
                "type": "openInvoice",
                "invoiceId": "inv-9988"
            }
        }),
    );
    assert_eq!(invoice_ok["conversationId"], conv_id);

    // Verify transcript recorded invoice audit entry
    let transcript = alice
        .read_human_conversation_transcript(&conv_id)
        .expect("read transcript");
    assert!(
        transcript
            .iter()
            .any(|e| e.get("content").and_then(Value::as_str).is_some_and(|c| c.contains("[MiniApp Invoice Opened] invoice_id=inv-9988")))
    );

    // 3. StartServiceCall Voice mode:
    // With only ServiceCall permission (NO Microphone), Voice mode fails closed
    let call_id = "call-svc-001";
    let svc_only_session = MiniAppSession {
        granted_permissions: vec![MiniAppPermission::ServiceCall],
        ..base_session.clone()
    };
    let svc_only_grant = MiniAppGrant {
        permissions: vec![MiniAppPermission::ServiceCall],
        ..base_grant.clone()
    };
    alice
        .register_miniapp_launch_session_for_test(svc_only_session, svc_only_grant)
        .expect("update session with service call only");

    let err_voice_no_mic = dispatch_err(
        &alice,
        "miniAppPostToConversation",
        json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-call-voice-1",
            "request": {
                "type": "startServiceCall",
                "callId": call_id,
                "mode": "voice"
            }
        }),
    );
    assert!(
        err_voice_no_mic.to_string().contains("Microphone")
            || err_voice_no_mic.to_string().contains("permission denied")
    );

    // Hybrid mode also fails closed without Microphone
    let err_hybrid_no_mic = dispatch_err(
        &alice,
        "miniAppPostToConversation",
        json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-call-hybrid-1",
            "request": {
                "type": "startServiceCall",
                "callId": call_id,
                "mode": "hybrid"
            }
        }),
    );
    assert!(
        err_hybrid_no_mic.to_string().contains("Microphone")
            || err_hybrid_no_mic.to_string().contains("permission denied")
    );

    // Text mode succeeds with ServiceCall permission alone!
    let text_call_id = "call-svc-text";
    let start_text_ok = dispatch_ok(
        &alice,
        "miniAppPostToConversation",
        json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-call-text-1",
            "request": {
                "type": "startServiceCall",
                "callId": text_call_id,
                "mode": "text"
            }
        }),
    );
    assert_eq!(start_text_ok["conversationId"], conv_id);
    let call_session = alice
        .get_miniapp_service_call(text_call_id)
        .expect("service call session exists");
    assert_eq!(
        call_session.state,
        fabushi_messaging_core::MiniAppServiceCallState::Active
    );

    // 4. Voice call with both ServiceCall and Microphone permissions
    let full_session = MiniAppSession {
        granted_permissions: vec![
            MiniAppPermission::ServiceCall,
            MiniAppPermission::Microphone,
        ],
        ..base_session.clone()
    };
    let full_grant = MiniAppGrant {
        permissions: vec![
            MiniAppPermission::ServiceCall,
            MiniAppPermission::Microphone,
        ],
        ..base_grant.clone()
    };
    alice
        .register_miniapp_launch_session_for_test(full_session, full_grant)
        .expect("update session with service call and microphone");

    let voice_call_id = "call-svc-voice";
    let start_voice_ok = dispatch_ok(
        &alice,
        "miniAppPostToConversation",
        json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-call-voice-ok",
            "request": {
                "type": "startServiceCall",
                "callId": voice_call_id,
                "mode": "voice"
            }
        }),
    );
    assert_eq!(start_voice_ok["conversationId"], conv_id);
    let voice_session = alice
        .get_miniapp_service_call(voice_call_id)
        .expect("voice service call session exists");
    assert_eq!(
        voice_session.state,
        fabushi_messaging_core::MiniAppServiceCallState::Active
    );

    // 5. Submit Service Call inputs:
    // DTMF input
    let dtmf_res = dispatch_ok(
        &alice,
        "miniAppPostToConversation",
        json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-dtmf-1",
            "request": {
                "type": "submitServiceCallInput",
                "callId": voice_call_id,
                "input": {
                    "type": "dtmf",
                    "digits": "1234"
                }
            }
        }),
    );
    assert_eq!(dtmf_res["conversationId"], conv_id);

    // SpeechTranscript input requires Microphone permission
    // Test with service call only (revoke Microphone):
    alice
        .register_miniapp_launch_session_for_test(
            MiniAppSession {
                granted_permissions: vec![MiniAppPermission::ServiceCall],
                ..base_session.clone()
            },
            MiniAppGrant {
                permissions: vec![MiniAppPermission::ServiceCall],
                ..base_grant.clone()
            },
        )
        .expect("revert microphone");

    let speech_no_mic = dispatch_err(
        &alice,
        "miniAppPostToConversation",
        json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-speech-nomic",
            "request": {
                "type": "submitServiceCallInput",
                "callId": voice_call_id,
                "input": {
                    "type": "speechTranscript",
                    "text": "Hello support",
                    "isFinal": true
                }
            }
        }),
    );
    assert!(
        speech_no_mic.to_string().contains("Microphone")
            || speech_no_mic.to_string().contains("permission denied")
    );

    // Re-grant Microphone permission and submit speech input
    alice
        .register_miniapp_launch_session_for_test(
            MiniAppSession {
                granted_permissions: vec![
                    MiniAppPermission::ServiceCall,
                    MiniAppPermission::Microphone,
                ],
                ..base_session.clone()
            },
            MiniAppGrant {
                permissions: vec![
                    MiniAppPermission::ServiceCall,
                    MiniAppPermission::Microphone,
                ],
                ..base_grant.clone()
            },
        )
        .expect("regrant microphone");

    let speech_ok = dispatch_ok(
        &alice,
        "miniAppPostToConversation",
        json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-speech-ok",
            "request": {
                "type": "submitServiceCallInput",
                "callId": voice_call_id,
                "input": {
                    "type": "speechTranscript",
                    "text": "Hello support",
                    "isFinal": true
                }
            }
        }),
    );
    assert_eq!(speech_ok["conversationId"], conv_id);

    // Verify call turns in state machine
    let voice_session_after_turns = alice
        .get_miniapp_service_call(voice_call_id)
        .expect("voice session exists");
    assert_eq!(voice_session_after_turns.turns.len(), 2);

    // 6. EndServiceCall
    let end_ok = dispatch_ok(
        &alice,
        "miniAppPostToConversation",
        json!({
            "launchSessionId": launch_session_id,
            "requestId": "req-end-call",
            "request": {
                "type": "endServiceCall",
                "callId": voice_call_id
            }
        }),
    );
    assert_eq!(end_ok["conversationId"], conv_id);

    let ended_session = alice
        .get_miniapp_service_call(voice_call_id)
        .expect("voice session exists");
    assert_eq!(
        ended_session.state,
        fabushi_messaging_core::MiniAppServiceCallState::Ended
    );
    assert!(ended_session.ended_at_ms.is_some());

    // Verify all audit entries are recorded in the transcript
    let final_transcript = alice
        .read_human_conversation_transcript(&conv_id)
        .expect("read transcript");
    let has_started = final_transcript.iter().any(|e| {
        e.get("content")
            .and_then(Value::as_str)
            .is_some_and(|c| c.contains("[MiniApp ServiceCall Started]") && c.contains(voice_call_id))
    });
    let has_dtmf = final_transcript.iter().any(|e| {
        e.get("content")
            .and_then(Value::as_str)
            .is_some_and(|c| c.contains("[MiniApp ServiceCall DTMF]") && c.contains("1234"))
    });
    let has_speech = final_transcript.iter().any(|e| {
        e.get("content")
            .and_then(Value::as_str)
            .is_some_and(|c| c.contains("[MiniApp ServiceCall Speech]") && c.contains("Hello support"))
    });
    let has_ended = final_transcript.iter().any(|e| {
        e.get("content")
            .and_then(Value::as_str)
            .is_some_and(|c| c.contains("[MiniApp ServiceCall Ended]") && c.contains(voice_call_id))
    });
    assert!(has_started, "transcript must contain ServiceCall Started entry");
    assert!(has_dtmf, "transcript must contain ServiceCall DTMF entry");
    assert!(has_speech, "transcript must contain ServiceCall Speech entry");
    assert!(has_ended, "transcript must contain ServiceCall Ended entry");
}
