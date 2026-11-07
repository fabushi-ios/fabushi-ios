use fabushi_messaging_core::*;

fn context(actor_id: &str, request_id: &str) -> RequestContext {
    RequestContext {
        request_id: request_id.into(),
        device_id: format!("device:{actor_id}"),
        actor_id: ActorId::new(actor_id),
        session_id: format!("session:{actor_id}"),
        sent_at_ms: 1,
    }
}

fn participant(actor_id: &str, role: ParticipantRole) -> Participant {
    Participant {
        actor_id: ActorId::new(actor_id),
        role,
        joined_at_ms: 1,
        muted_until_ms: None,
    }
}

fn channel_conversation() -> Conversation {
    let mut conversation = Conversation::direct(
        "channel:m6",
        "M6 Dharma Channel",
        vec![
            participant("human:owner", ParticipantRole::Owner),
            participant("human:admin", ParticipantRole::Admin),
        ],
        1,
    );
    conversation.kind = ConversationKind::Channel;
    conversation.owner_id = Some(ActorId::new("human:owner"));
    conversation
}

fn channel_community() -> CommunityState {
    let mut community = CommunityState::new(ConversationId::new("channel:m6"));
    community.public_username = Some("m6".into());
    community.upsert_member(CommunityMember {
        actor_id: ActorId::new("human:owner"),
        status: MemberStatus::Owner,
        admin_title: Some("Owner".into()),
        admin_rights: AdminRights::default(),
        restrictions: MemberRestrictions::default(),
        joined_at_ms: 1,
        invited_by: None,
    });
    community.upsert_member(CommunityMember {
        actor_id: ActorId::new("human:admin"),
        status: MemberStatus::Administrator,
        admin_title: Some("Publisher".into()),
        admin_rights: AdminRights {
            post_messages: true,
            manage_topics: true,
            change_info: true,
            ban_members: true,
            ..AdminRights::default()
        },
        restrictions: MemberRestrictions::default(),
        joined_at_ms: 1,
        invited_by: Some(ActorId::new("human:owner")),
    });
    community
}

fn text_message(text: &str) -> MessageContent {
    MessageContent::Text {
        text: FormattedText::plain(text),
    }
}

fn sync_batch(events: &[ServerEnvelope]) -> &ServerEvent {
    events
        .iter()
        .find_map(|envelope| match &envelope.event {
            ServerEvent::SyncBatch { .. } => Some(&envelope.event),
            _ => None,
        })
        .expect("sync response")
}

#[test]
fn channel_subscription_broadcast_pagination_and_topic_state_are_actor_scoped() {
    let mut service = MessagingService::load(MemoryStateStore::default()).unwrap();
    for actor_id in [
        "human:owner",
        "human:admin",
        "human:subscriber",
        "human:outsider",
    ] {
        service
            .handle(
                ClientEnvelope::new(
                    context(actor_id, &format!("profile:{actor_id}")),
                    ClientCommand::UpsertProfile {
                        actor: Actor::human(actor_id, actor_id),
                    },
                ),
                1,
            )
            .unwrap();
    }

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "create-channel"),
                ClientCommand::CreateConversation {
                    conversation: channel_conversation(),
                },
            ),
            2,
        )
        .unwrap();
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "create-community"),
                ClientCommand::UpdateCommunity {
                    community: channel_community(),
                },
            ),
            3,
        )
        .unwrap();
    service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "subscribe"),
                ClientCommand::SubscribeChannel {
                    conversation_id: ConversationId::new("channel:m6"),
                },
            ),
            4,
        )
        .unwrap();

    let members_page = service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "members-1"),
                ClientCommand::ListCommunityMembers {
                    conversation_id: ConversationId::new("channel:m6"),
                    cursor: None,
                    limit: 1,
                },
            ),
            5,
        )
        .unwrap();
    let next_cursor = match &members_page[0].event {
        ServerEvent::CommunityMembersPage {
            members,
            next_cursor,
            ..
        } => {
            assert_eq!(members.len(), 1);
            next_cursor.clone().expect("member page cursor")
        }
        event => panic!("unexpected member page: {event:?}"),
    };
    let members_page_2 = service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "members-2"),
                ClientCommand::ListCommunityMembers {
                    conversation_id: ConversationId::new("channel:m6"),
                    cursor: Some(next_cursor),
                    limit: 10,
                },
            ),
            6,
        )
        .unwrap();
    assert!(matches!(
        &members_page_2[0].event,
        ServerEvent::CommunityMembersPage { members, .. } if members.len() == 2
    ));

    let subscriber_audit_denied = service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "audit-denied"),
                ClientCommand::ListCommunityAuditLog {
                    conversation_id: ConversationId::new("channel:m6"),
                    cursor: None,
                    limit: 100,
                },
            ),
            7,
        )
        .unwrap_err();
    assert!(matches!(
        subscriber_audit_denied,
        MessagingServiceError::UnauthorizedCommand(_)
    ));

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "publish"),
                ClientCommand::SendMessage {
                    conversation_id: ConversationId::new("channel:m6"),
                    client_message_id: ClientMessageId("client:publish".into()),
                    content: text_message("broadcast"),
                    reply_to_message_id: None,
                    thread_root_message_id: None,
                    scheduled_at_ms: None,
                    silent: false,
                    protected_content: false,
                },
            ),
            8,
        )
        .unwrap();

    let subscriber_sync = service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "sync-subscriber"),
                ClientCommand::Sync {
                    cursor: None,
                    limit: 100,
                },
            ),
            9,
        )
        .unwrap();
    match sync_batch(&subscriber_sync) {
        ServerEvent::SyncBatch {
            conversations,
            messages,
            communities,
            ..
        } => {
            assert!(conversations
                .iter()
                .any(|conversation| conversation.id == ConversationId::new("channel:m6")));
            assert!(messages
                .iter()
                .any(|message| { message.conversation_id == ConversationId::new("channel:m6") }));
            assert!(communities.iter().any(|community| {
                community.conversation_id == ConversationId::new("channel:m6")
                    && community.admin_log.is_empty()
            }));
        }
        event => panic!("unexpected subscriber sync: {event:?}"),
    }

    let outsider_sync = service
        .handle(
            ClientEnvelope::new(
                context("human:outsider", "sync-outsider"),
                ClientCommand::Sync {
                    cursor: None,
                    limit: 100,
                },
            ),
            10,
        )
        .unwrap();
    match sync_batch(&outsider_sync) {
        ServerEvent::SyncBatch {
            conversations,
            messages,
            ..
        } => {
            assert!(conversations.is_empty());
            assert!(messages.is_empty());
        }
        event => panic!("unexpected outsider sync: {event:?}"),
    }

    let audit = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "audit-owner"),
                ClientCommand::ListCommunityAuditLog {
                    conversation_id: ConversationId::new("channel:m6"),
                    cursor: None,
                    limit: 100,
                },
            ),
            11,
        )
        .unwrap();
    assert!(matches!(
        &audit[0].event,
        ServerEvent::CommunityAuditLogPage { entries, .. }
            if entries.iter().any(|entry| entry.action == CommunityAuditAction::SubscriptionAdded)
    ));

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "topic-create"),
                ClientCommand::UpsertForumTopic {
                    topic: ForumTopicState {
                        id: "study".into(),
                        conversation_id: ConversationId::new("channel:m6"),
                        title: "Study".into(),
                        icon: None,
                        creator_id: ActorId::new("human:owner"),
                        created_at_ms: 12,
                        pinned: false,
                        closed: false,
                        hidden: false,
                        unread_count: 0,
                        last_message_id: None,
                    },
                },
            ),
            12,
        )
        .unwrap();
    let topic_message = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "topic-post"),
                ClientCommand::SendMessage {
                    conversation_id: ConversationId::new("channel:m6"),
                    client_message_id: ClientMessageId("client:topic".into()),
                    content: text_message("topic post"),
                    reply_to_message_id: None,
                    thread_root_message_id: Some(MessageId::new("topic:study")),
                    scheduled_at_ms: None,
                    silent: false,
                    protected_content: false,
                },
            ),
            13,
        )
        .unwrap()
        .into_iter()
        .find_map(|envelope| match envelope.event {
            ServerEvent::MessageAdded { message } => Some(message.id),
            _ => None,
        })
        .expect("topic message");

    let forwarded_topic = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "topic-forward"),
                ClientCommand::ForwardMessage {
                    source_conversation_id: ConversationId::new("channel:m6"),
                    message_id: topic_message.clone(),
                    destination_conversation_id: ConversationId::new("channel:m6"),
                    client_message_id: ClientMessageId("client:topic-forward".into()),
                    thread_root_message_id: Some(MessageId::new("topic:study")),
                    scheduled_at_ms: None,
                    silent: false,
                    privacy: ForwardPrivacy::default(),
                },
            ),
            14,
        )
        .unwrap()
        .into_iter()
        .find_map(|envelope| match envelope.event {
            ServerEvent::MessageAdded { message } => Some(message),
            _ => None,
        })
        .expect("forwarded topic message");
    assert_eq!(
        forwarded_topic.thread_root_message_id.as_ref(),
        Some(&MessageId::new("topic:study"))
    );

    let missing_topic = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "topic-forward-missing"),
                ClientCommand::ForwardMessage {
                    source_conversation_id: ConversationId::new("channel:m6"),
                    message_id: topic_message.clone(),
                    destination_conversation_id: ConversationId::new("channel:m6"),
                    client_message_id: ClientMessageId("client:topic-forward-missing".into()),
                    thread_root_message_id: Some(MessageId::new("topic:missing")),
                    scheduled_at_ms: None,
                    silent: false,
                    privacy: ForwardPrivacy::default(),
                },
            ),
            14,
        )
        .unwrap_err();
    assert!(matches!(
        missing_topic,
        MessagingServiceError::Engine(EngineError::ForumTopicNotFound { topic_id, .. })
            if topic_id == "missing"
    ));

    for (request_suffix, closed, hidden) in [
        ("closed", true, false),
        ("hidden", false, true),
    ] {
        service
            .handle(
                ClientEnvelope::new(
                    context("human:owner", &format!("topic-{request_suffix}-update")),
                    ClientCommand::UpsertForumTopic {
                        topic: ForumTopicState {
                            id: "study".into(),
                            conversation_id: ConversationId::new("channel:m6"),
                            title: "Study".into(),
                            icon: None,
                            creator_id: ActorId::new("human:owner"),
                            created_at_ms: 12,
                            pinned: false,
                            closed,
                            hidden,
                            unread_count: 0,
                            last_message_id: Some(topic_message.0.clone()),
                        },
                    },
                ),
                14,
            )
            .unwrap();
        let ineligible_topic = service
            .handle(
                ClientEnvelope::new(
                    context("human:owner", &format!("topic-forward-{request_suffix}")),
                    ClientCommand::ForwardMessage {
                        source_conversation_id: ConversationId::new("channel:m6"),
                        message_id: topic_message.clone(),
                        destination_conversation_id: ConversationId::new("channel:m6"),
                        client_message_id: ClientMessageId(
                            format!("client:topic-forward-{request_suffix}"),
                        ),
                        thread_root_message_id: Some(MessageId::new("topic:study")),
                        scheduled_at_ms: None,
                        silent: false,
                        privacy: ForwardPrivacy::default(),
                    },
                ),
                14,
            )
            .unwrap_err();
        assert!(matches!(
            ineligible_topic,
            MessagingServiceError::Engine(EngineError::ForumTopicClosed { topic_id, .. })
                if topic_id == "study"
        ));
    }

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "topic-reopen"),
                ClientCommand::UpsertForumTopic {
                    topic: ForumTopicState {
                        id: "study".into(),
                        conversation_id: ConversationId::new("channel:m6"),
                        title: "Study".into(),
                        icon: None,
                        creator_id: ActorId::new("human:owner"),
                        created_at_ms: 12,
                        pinned: false,
                        closed: false,
                        hidden: false,
                        unread_count: 0,
                        last_message_id: Some(topic_message.0.clone()),
                    },
                },
            ),
            14,
        )
        .unwrap();

    let topic_before_read = service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "topic-sync-before-read"),
                ClientCommand::Sync {
                    cursor: None,
                    limit: 100,
                },
            ),
            14,
        )
        .unwrap();
    assert!(matches!(
        sync_batch(&topic_before_read),
        ServerEvent::SyncBatch { conversations, .. }
            if conversations.iter().any(|conversation| {
                conversation.id == ConversationId::new("channel:m6")
                    && conversation.topics.iter().any(|topic| {
                        // The subscriber has not read either the original topic post
                        // or the successful same-topic forward exercised above.
                        topic.id == "study" && topic.unread_count == 2
                    })
            })
    ));

    service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "topic-draft"),
                ClientCommand::SetTopicDraft {
                    conversation_id: ConversationId::new("channel:m6"),
                    topic_id: "study".into(),
                    text: "draft".into(),
                    reply_to_message_id: None,
                },
            ),
            15,
        )
        .unwrap();
    service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "topic-read-first"),
                ClientCommand::MarkTopicRead {
                    conversation_id: ConversationId::new("channel:m6"),
                    topic_id: "study".into(),
                    message_id: topic_message,
                },
            ),
            16,
        )
        .unwrap();
    let topic_after_first_read = service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "topic-sync-after-first-read"),
                ClientCommand::Sync {
                    cursor: None,
                    limit: 100,
                },
            ),
            17,
        )
        .unwrap();
    assert!(matches!(
        sync_batch(&topic_after_first_read),
        ServerEvent::SyncBatch {
            conversations,
            topic_drafts,
            ..
        } if conversations.iter().any(|conversation| {
            conversation.id == ConversationId::new("channel:m6")
                && conversation.topics.iter().any(|topic| {
                    // Reading the older topic post advances the actor-scoped cursor
                    // only through that message; the later same-topic forward remains unread.
                    topic.id == "study" && topic.unread_count == 1
                })
        }) && topic_drafts.iter().any(|draft| draft.topic_id == "study")
    ));

    service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "topic-read-latest"),
                ClientCommand::MarkTopicRead {
                    conversation_id: ConversationId::new("channel:m6"),
                    topic_id: "study".into(),
                    message_id: forwarded_topic.id,
                },
            ),
            18,
        )
        .unwrap();
    let topic_after_latest_read = service
        .handle(
            ClientEnvelope::new(
                context("human:subscriber", "topic-sync-after-latest-read"),
                ClientCommand::Sync {
                    cursor: None,
                    limit: 100,
                },
            ),
            19,
        )
        .unwrap();
    assert!(matches!(
        sync_batch(&topic_after_latest_read),
        ServerEvent::SyncBatch {
            conversations,
            topic_drafts,
            ..
        } if conversations.iter().any(|conversation| {
            conversation.id == ConversationId::new("channel:m6")
                && conversation.topics.iter().any(|topic| {
                    topic.id == "study" && topic.unread_count == 0
                })
        }) && topic_drafts.iter().any(|draft| draft.topic_id == "study")
    ));
}

#[test]
fn typed_child_read_and_draft_share_the_canonical_conversation_state() {
    let mut service = MessagingService::load(MemoryStateStore::default()).unwrap();
    for actor_id in ["human:owner", "human:peer", "human:outsider"] {
        service
            .handle(
                ClientEnvelope::new(
                    context(actor_id, &format!("profile:{actor_id}")),
                    ClientCommand::UpsertProfile {
                        actor: Actor::human(actor_id, actor_id),
                    },
                ),
                1,
            )
            .unwrap();
    }

    let conversation_id = ConversationId::new("conversation:saved-child");
    let mut conversation = Conversation::direct(
        conversation_id.0.clone(),
        "Saved child fixture",
        vec![participant("human:owner", ParticipantRole::Owner)],
        2,
    );
    conversation.kind = ConversationKind::SavedMessages;
    conversation.owner_id = Some(ActorId::new("human:owner"));
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "create-saved-child"),
                ClientCommand::CreateConversation { conversation },
            ),
            2,
        )
        .unwrap();

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "send-saved-child"),
                ClientCommand::SendMessage {
                    conversation_id: conversation_id.clone(),
                    client_message_id: ClientMessageId("client:saved-child".into()),
                    content: text_message("canonical child message"),
                    reply_to_message_id: None,
                    thread_root_message_id: None,
                    scheduled_at_ms: None,
                    silent: false,
                    protected_content: false,
                },
            ),
            3,
        )
        .unwrap();

    let message_id = service
        .engine()
        .state()
        .messages
        .get(&conversation_id)
        .and_then(|messages| messages.values().next())
        .expect("saved child message")
        .id
        .clone();
    let destination = ConversationDestination::saved_sublist(
        conversation_id.clone(),
        ActorId::new("human:peer"),
    );

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "draft-saved-child"),
                ClientCommand::SetConversationChildDraft {
                    destination: destination.clone(),
                    text: "child draft".into(),
                    reply_to_message_id: Some(message_id.clone()),
                },
            ),
            4,
        )
        .unwrap();
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "read-saved-child"),
                ClientCommand::MarkConversationChildRead {
                    destination: destination.clone(),
                    message_id: message_id.clone(),
                },
            ),
            5,
        )
        .unwrap();

    let child = service
        .engine()
        .state()
        .conversation_child_states
        .iter()
        .find(|state| {
            state.destination == destination && state.actor_id == ActorId::new("human:owner")
        })
        .expect("canonical child state");
    assert_eq!(child.draft_text, "child draft");
    assert_eq!(
        child.draft_reply_to_message_id.as_deref(),
        Some(message_id.0.as_str())
    );
    assert_eq!(
        child.inbox_read_till.as_ref().map(|position| position.message_id.as_str()),
        Some(message_id.0.as_str())
    );

    let invalid_saved_child = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "draft-missing-saved-child"),
                ClientCommand::SetConversationChildDraft {
                    destination: ConversationDestination::saved_sublist(
                        conversation_id.clone(),
                        ActorId::new("human:missing"),
                    ),
                    text: "must not materialize".into(),
                    reply_to_message_id: None,
                },
            ),
            6,
        )
        .unwrap_err();
    assert!(matches!(
        invalid_saved_child,
        MessagingServiceError::Engine(EngineError::InvalidConversationChildDestination)
    ));

    let child_conversation_id = ConversationId::new("conversation:nested-child");
    let child_conversation = Conversation::direct(
        child_conversation_id.0.clone(),
        "Nested child fixture",
        vec![
            participant("human:owner", ParticipantRole::Owner),
            participant("human:peer", ParticipantRole::Member),
        ],
        7,
    );
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "create-nested-child"),
                ClientCommand::CreateConversation {
                    conversation: child_conversation,
                },
            ),
            7,
        )
        .unwrap();
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "send-nested-child"),
                ClientCommand::SendMessage {
                    conversation_id: child_conversation_id.clone(),
                    client_message_id: ClientMessageId("client:nested-child".into()),
                    content: text_message("nested child message"),
                    reply_to_message_id: None,
                    thread_root_message_id: None,
                    scheduled_at_ms: None,
                    silent: false,
                    protected_content: false,
                },
            ),
            8,
        )
        .unwrap();
    let nested_message_id = service
        .engine()
        .state()
        .messages
        .get(&child_conversation_id)
        .and_then(|messages| messages.values().next())
        .expect("nested child message")
        .id
        .clone();
    let nested_destination = ConversationDestination::nested_conversation(
        conversation_id.clone(),
        child_conversation_id.clone(),
    );
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "read-nested-child"),
                ClientCommand::MarkConversationChildRead {
                    destination: nested_destination.clone(),
                    message_id: nested_message_id.clone(),
                },
            ),
            9,
        )
        .unwrap();
    let nested_child = service
        .engine()
        .state()
        .conversation_child_states
        .iter()
        .find(|state| {
            state.destination == nested_destination
                && state.actor_id == ActorId::new("human:owner")
        })
        .expect("nested canonical child state");
    assert_eq!(
        nested_child
            .inbox_read_till
            .as_ref()
            .map(|position| position.message_id.as_str()),
        Some(nested_message_id.0.as_str())
    );

    let denied = service
        .handle(
            ClientEnvelope::new(
                context("human:outsider", "draft-saved-child-denied"),
                ClientCommand::SetConversationChildDraft {
                    destination,
                    text: "forbidden".into(),
                    reply_to_message_id: None,
                },
            ),
            10,
        )
        .unwrap_err();
    assert!(matches!(
        denied,
        MessagingServiceError::UnauthorizedCommand(reason)
            if reason.contains("conversation child state update requires membership")
    ));
}

#[test]
fn typed_child_lifecycle_pagination_pin_active_payment_and_destroy_are_actor_scoped() {
    let mut service = MessagingService::load(MemoryStateStore::default()).unwrap();
    for actor_id in ["human:owner", "human:peer", "human:outsider"] {
        service
            .handle(
                ClientEnvelope::new(
                    context(actor_id, &format!("profile:lifecycle:{actor_id}")),
                    ClientCommand::UpsertProfile {
                        actor: Actor::human(actor_id, actor_id),
                    },
                ),
                1,
            )
            .unwrap();
    }

    let parent_id = ConversationId::new("conversation:child-lifecycle");
    let child_id = ConversationId::new("conversation:child-lifecycle:nested");
    let saved_parent_id = ConversationId::new("conversation:child-lifecycle:saved");
    let mut saved_parent = Conversation::direct(
        saved_parent_id.0.clone(),
        "Saved child lifecycle parent",
        vec![participant("human:owner", ParticipantRole::Owner)],
        2,
    );
    saved_parent.kind = ConversationKind::SavedMessages;
    saved_parent.owner_id = Some(ActorId::new("human:owner"));
    for (request_id, conversation) in [
        (
            "create-child-lifecycle-parent",
            Conversation::direct(
                parent_id.0.clone(),
                "Child lifecycle parent",
                vec![
                    participant("human:owner", ParticipantRole::Owner),
                    participant("human:peer", ParticipantRole::Member),
                ],
                2,
            ),
        ),
        (
            "create-child-lifecycle-nested",
            Conversation::direct(
                child_id.0.clone(),
                "Child lifecycle nested",
                vec![
                    participant("human:owner", ParticipantRole::Owner),
                    participant("human:peer", ParticipantRole::Member),
                ],
                2,
            ),
        ),
        ("create-child-lifecycle-saved", saved_parent),
    ] {
        service
            .handle(
                ClientEnvelope::new(
                    context("human:owner", request_id),
                    ClientCommand::CreateConversation { conversation },
                ),
                2,
            )
            .unwrap();
    }

    let sent = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "send-child-lifecycle"),
                ClientCommand::SendMessage {
                    conversation_id: child_id.clone(),
                    client_message_id: ClientMessageId("client:child-lifecycle".into()),
                    content: text_message("child page item"),
                    reply_to_message_id: None,
                    thread_root_message_id: None,
                    scheduled_at_ms: None,
                    silent: false,
                    protected_content: false,
                },
            ),
            3,
        )
        .unwrap();
    let message_id = sent
        .iter()
        .find_map(|envelope| match &envelope.event {
            ServerEvent::MessageChanged { message } => Some(message.id.clone()),
            _ => None,
        })
        .expect("sent message");

    let destination =
        ConversationDestination::nested_conversation(parent_id.clone(), child_id.clone());
    let saved_destination =
        ConversationDestination::saved_sublist(saved_parent_id.clone(), ActorId::new("human:peer"));

    let invalid_direct_saved = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "direct-saved-child-denied"),
                ClientCommand::SetConversationChildNoPaidMessages {
                    destination: ConversationDestination::saved_sublist(
                        parent_id.clone(),
                        ActorId::new("human:peer"),
                    ),
                    no_paid_messages: true,
                },
            ),
            3,
        )
        .unwrap_err();
    assert!(matches!(
        invalid_direct_saved,
        MessagingServiceError::Engine(EngineError::InvalidConversationChildDestination)
    ));

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "child-window"),
                ClientCommand::ReplaceConversationChildWindow {
                    destination: destination.clone(),
                    message_ids: vec![message_id.clone()],
                    skipped_before: Some(0),
                    skipped_after: Some(0),
                    full_count: Some(1),
                },
            ),
            4,
        )
        .unwrap();
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "child-pin"),
                ClientCommand::SetConversationChildPinned {
                    destination: destination.clone(),
                    pinned: true,
                },
            ),
            5,
        )
        .unwrap();
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "child-active"),
                ClientCommand::SetConversationChildActive {
                    destination: destination.clone(),
                    active: true,
                },
            ),
            6,
        )
        .unwrap();
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "child-marked-unread"),
                ClientCommand::SetConversationChildMarkedUnread {
                    destination: destination.clone(),
                    marked_unread: true,
                },
            ),
            7,
        )
        .unwrap();
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "child-no-paid"),
                ClientCommand::SetConversationChildNoPaidMessages {
                    destination: saved_destination.clone(),
                    no_paid_messages: true,
                },
            ),
            8,
        )
        .unwrap();

    let unprovable_saved_page = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "saved-page-fail-closed"),
                ClientCommand::ReplaceConversationChildWindow {
                    destination: saved_destination.clone(),
                    message_ids: vec![message_id.clone()],
                    skipped_before: Some(0),
                    skipped_after: Some(0),
                    full_count: Some(1),
                },
            ),
            9,
        )
        .unwrap_err();
    assert!(matches!(
        unprovable_saved_page,
        MessagingServiceError::Engine(EngineError::ConversationChildMessageMismatch)
    ));

    let first_sync = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "sync-child-lifecycle"),
                ClientCommand::Sync {
                    cursor: None,
                    limit: 100,
                },
            ),
            10,
        )
        .unwrap();
    match sync_batch(&first_sync) {
        ServerEvent::SyncBatch {
            conversation_children,
            ..
        } => {
            assert!(conversation_children.iter().any(|child| {
                child.destination == destination
                    && child.actor_id == ActorId::new("human:owner")
                    && child.pagination.message_ids == vec![message_id.0.clone()]
                    && child.pinned
                    && child.active
                    && child.marked_unread
            }));
            assert!(conversation_children.iter().any(|child| {
                child.destination == saved_destination
                    && child.actor_id == ActorId::new("human:owner")
                    && child.no_paid_messages
            }));
        }
        event => panic!("unexpected child lifecycle sync: {event:?}"),
    }

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "child-empty-window"),
                ClientCommand::ReplaceConversationChildWindow {
                    destination: destination.clone(),
                    message_ids: Vec::new(),
                    skipped_before: Some(0),
                    skipped_after: Some(0),
                    full_count: Some(0),
                },
            ),
            11,
        )
        .unwrap();
    let empty = service
        .engine()
        .state()
        .conversation_child_states
        .iter()
        .find(|child| {
            child.destination == destination && child.actor_id == ActorId::new("human:owner")
        })
        .expect("empty child remains persisted");
    assert!(!empty.pinned);
    assert!(empty.restore_pinned_when_non_empty);

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "child-restored-window"),
                ClientCommand::ReplaceConversationChildWindow {
                    destination: destination.clone(),
                    message_ids: vec![message_id.clone()],
                    skipped_before: Some(0),
                    skipped_after: Some(0),
                    full_count: Some(1),
                },
            ),
            12,
        )
        .unwrap();
    let restored = service
        .engine()
        .state()
        .conversation_child_states
        .iter()
        .find(|child| {
            child.destination == destination && child.actor_id == ActorId::new("human:owner")
        })
        .expect("restored child");
    assert!(restored.pinned);
    assert!(!restored.restore_pinned_when_non_empty);

    let denied_saved_unread = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "saved-unread-denied"),
                ClientCommand::SetConversationChildMarkedUnread {
                    destination: saved_destination.clone(),
                    marked_unread: true,
                },
            ),
            13,
        )
        .unwrap_err();
    assert!(matches!(
        denied_saved_unread,
        MessagingServiceError::Engine(EngineError::InvalidConversationChildDestination)
    ));

    let outsider_denied = service
        .handle(
            ClientEnvelope::new(
                context("human:outsider", "child-destroy-denied"),
                ClientCommand::DestroyConversationChild {
                    destination: destination.clone(),
                },
            ),
            14,
        )
        .unwrap_err();
    assert!(matches!(
        outsider_denied,
        MessagingServiceError::UnauthorizedCommand(reason)
            if reason.contains("conversation child state update requires membership")
    ));

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "child-destroy"),
                ClientCommand::DestroyConversationChild {
                    destination: destination.clone(),
                },
            ),
            15,
        )
        .unwrap();
    assert!(!service
        .engine()
        .state()
        .conversation_child_states
        .iter()
        .any(|child| {
            child.destination == destination && child.actor_id == ActorId::new("human:owner")
        }));

    let after_destroy = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "sync-after-child-destroy"),
                ClientCommand::Sync {
                    cursor: None,
                    limit: 100,
                },
            ),
            16,
        )
        .unwrap();
    assert!(matches!(
        sync_batch(&after_destroy),
        ServerEvent::SyncBatch {
            conversation_children,
            ..
        } if conversation_children.iter().all(|child| child.destination != destination)
            && conversation_children.iter().any(|child| child.destination == saved_destination)
    ));

    // Canonical SavedSublist message-membership relation + MessageQueued pin restoration:
    // 1. Peer sends a message in `child_id`, and owner forwards it into `saved_parent_id`
    //    with preserved forward origin (`drop_sender_names: false`).
    let peer_send_envelopes = service
        .handle(
            ClientEnvelope::new(
                context("human:peer", "peer-source-for-saved-sublist"),
                ClientCommand::SendMessage {
                    conversation_id: child_id.clone(),
                    client_message_id: ClientMessageId("client:peer-source-1".into()),
                    content: text_message("from peer to saved"),
                    reply_to_message_id: None,
                    thread_root_message_id: None,
                    scheduled_at_ms: None,
                    silent: false,
                    protected_content: false,
                },
            ),
            17,
        )
        .unwrap();
    let peer_source_msg_id = peer_send_envelopes
        .iter()
        .find_map(|env| match &env.event {
            ServerEvent::MessageChanged { message } => Some(message.id.clone()),
            _ => None,
        })
        .expect("peer source message id");

    let preserved_fwd_envelopes = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "forward-preserved-to-saved"),
                ClientCommand::ForwardMessage {
                    source_conversation_id: child_id.clone(),
                    message_id: peer_source_msg_id.clone(),
                    destination_conversation_id: saved_parent_id.clone(),
                    client_message_id: ClientMessageId("batch:saved-sublist-preserved".into()),
                    thread_root_message_id: None,
                    scheduled_at_ms: None,
                    silent: false,
                    privacy: ForwardPrivacy {
                        drop_sender_names: false,
                        drop_captions: false,
                    },
                },
            ),
            18,
        )
        .unwrap();
    let preserved_saved_msg_id = preserved_fwd_envelopes
        .iter()
        .find_map(|env| match &env.event {
            ServerEvent::MessageAdded { message } | ServerEvent::MessageChanged { message } => {
                Some(message.id.clone())
            }
            _ => None,
        })
        .expect("preserved saved message id");

    // Non-empty SavedSublist window succeeds for a message genuinely forwarded from `human:peer`
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "saved-page-provable-peer"),
                ClientCommand::ReplaceConversationChildWindow {
                    destination: saved_destination.clone(),
                    message_ids: vec![preserved_saved_msg_id.clone()],
                    skipped_before: Some(0),
                    skipped_after: Some(0),
                    full_count: Some(1),
                },
            ),
            19,
        )
        .unwrap();

    // 2. Forwarding with `drop_sender_names: true` clears `forward_origin`, so it belongs to
    //    `human:owner`'s self sublist and fails closed for `human:peer`'s sublist.
    let anon_fwd_envelopes = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "forward-anon-to-saved"),
                ClientCommand::ForwardMessage {
                    source_conversation_id: child_id.clone(),
                    message_id: peer_source_msg_id.clone(),
                    destination_conversation_id: saved_parent_id.clone(),
                    client_message_id: ClientMessageId("batch:saved-sublist-anon".into()),
                    thread_root_message_id: None,
                    scheduled_at_ms: None,
                    silent: false,
                    privacy: ForwardPrivacy {
                        drop_sender_names: true,
                        drop_captions: false,
                    },
                },
            ),
            20,
        )
        .unwrap();
    let anon_saved_msg_id = anon_fwd_envelopes
        .iter()
        .find_map(|env| match &env.event {
            ServerEvent::MessageAdded { message } | ServerEvent::MessageChanged { message } => {
                Some(message.id.clone())
            }
            _ => None,
        })
        .expect("anon saved message id");

    let anon_on_peer_sublist = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "saved-page-anon-mismatch"),
                ClientCommand::ReplaceConversationChildWindow {
                    destination: saved_destination.clone(),
                    message_ids: vec![anon_saved_msg_id.clone()],
                    skipped_before: Some(0),
                    skipped_after: Some(0),
                    full_count: Some(1),
                },
            ),
            21,
        )
        .unwrap_err();
    assert!(matches!(
        anon_on_peer_sublist,
        MessagingServiceError::Engine(EngineError::ConversationChildMessageMismatch)
    ));

    let owner_self_sublist = ConversationDestination::saved_sublist(
        saved_parent_id.clone(),
        ActorId::new("human:owner"),
    );
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "saved-page-owner-self-sublist"),
                ClientCommand::ReplaceConversationChildWindow {
                    destination: owner_self_sublist,
                    message_ids: vec![anon_saved_msg_id],
                    skipped_before: Some(0),
                    skipped_after: Some(0),
                    full_count: Some(1),
                },
            ),
            22,
        )
        .unwrap();

    // 3. Pin `saved_destination`, make it locally empty, and verify queuing a new forwarded
    //    message from `human:peer` automatically restores `pinned = true`.
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "pin-saved-sublist"),
                ClientCommand::SetConversationChildPinned {
                    destination: saved_destination.clone(),
                    pinned: true,
                },
            ),
            23,
        )
        .unwrap();
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "empty-saved-sublist-window"),
                ClientCommand::ReplaceConversationChildWindow {
                    destination: saved_destination.clone(),
                    message_ids: Vec::new(),
                    skipped_before: Some(0),
                    skipped_after: Some(0),
                    full_count: Some(0),
                },
            ),
            24,
        )
        .unwrap();
    let empty_saved = service
        .engine()
        .state()
        .conversation_child_states
        .iter()
        .find(|child| child.destination == saved_destination)
        .expect("empty saved sublist");
    assert!(!empty_saved.pinned);
    assert!(empty_saved.restore_pinned_when_non_empty);

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "forward-second-preserved-to-saved"),
                ClientCommand::ForwardMessage {
                    source_conversation_id: child_id.clone(),
                    message_id: peer_source_msg_id,
                    destination_conversation_id: saved_parent_id.clone(),
                    client_message_id: ClientMessageId("batch:saved-sublist-restore-pin".into()),
                    thread_root_message_id: None,
                    scheduled_at_ms: None,
                    silent: false,
                    privacy: ForwardPrivacy {
                        drop_sender_names: false,
                        drop_captions: false,
                    },
                },
            ),
            25,
        )
        .unwrap();
    let restored_saved = service
        .engine()
        .state()
        .conversation_child_states
        .iter()
        .find(|child| child.destination == saved_destination)
        .expect("restored saved sublist");
    assert!(restored_saved.pinned);
    assert!(!restored_saved.restore_pinned_when_non_empty);

    // 4. Legacy `topic_read_cursors` and `topic_drafts` migrate into `conversation_child_states`
    //    when `MessagingEngine::from_state` loads a snapshot.
    let mut legacy_state = service.engine().state().clone();
    legacy_state.conversation_child_states.clear();
    legacy_state
        .topic_read_cursors
        .entry(child_id.clone())
        .or_default()
        .entry(ActorId::new("human:owner"))
        .or_default()
        .insert("legacy-topic".into(), message_id.clone());
    legacy_state
        .topic_drafts
        .entry(child_id.clone())
        .or_default()
        .entry(ActorId::new("human:owner"))
        .or_default()
        .insert(
            "legacy-topic".into(),
            TopicDraft {
                conversation_id: child_id.clone(),
                topic_id: "legacy-topic".into(),
                actor_id: ActorId::new("human:owner"),
                text: "migrated topic draft".into(),
                reply_to_message_id: None,
                updated_at_ms: 42,
            },
        );
    let migrated_engine = MessagingEngine::from_state(legacy_state);
    let migrated_topic_dest =
        ConversationDestination::topic(child_id, "topic:legacy-topic".to_string());
    let migrated_child = migrated_engine
        .state()
        .conversation_child_states
        .iter()
        .find(|child| {
            child.destination == migrated_topic_dest
                && child.actor_id == ActorId::new("human:owner")
        })
        .expect("migrated legacy topic child state");
    assert_eq!(
        migrated_child
            .inbox_read_till
            .as_ref()
            .map(|pos| pos.message_id.as_str()),
        Some(message_id.0.as_str())
    );
    assert_eq!(migrated_child.draft_text.as_str(), "migrated topic draft");
}

#[test]
fn slow_mode_and_moderation_are_enforced_by_the_rust_state_machine() {
    let mut engine = MessagingEngine::new();
    for actor_id in ["human:owner", "human:admin", "human:member"] {
        engine
            .execute(Command::UpsertActor {
                actor: Actor::human(actor_id, actor_id),
            })
            .unwrap();
    }

    let mut conversation = Conversation::direct(
        "group:m6",
        "M6 Study Group",
        vec![
            participant("human:owner", ParticipantRole::Owner),
            participant("human:admin", ParticipantRole::Admin),
            participant("human:member", ParticipantRole::Member),
        ],
        1,
    );
    conversation.kind = ConversationKind::Group;
    conversation.owner_id = Some(ActorId::new("human:owner"));
    engine
        .execute(Command::UpsertConversation { conversation })
        .unwrap();

    let mut community = CommunityState::new(ConversationId::new("group:m6"));
    community.upsert_member(CommunityMember {
        actor_id: ActorId::new("human:owner"),
        status: MemberStatus::Owner,
        admin_title: None,
        admin_rights: AdminRights::default(),
        restrictions: MemberRestrictions::default(),
        joined_at_ms: 1,
        invited_by: None,
    });
    community.upsert_member(CommunityMember {
        actor_id: ActorId::new("human:admin"),
        status: MemberStatus::Administrator,
        admin_title: None,
        admin_rights: AdminRights {
            manage_topics: true,
            ban_members: true,
            ..AdminRights::default()
        },
        restrictions: MemberRestrictions::default(),
        joined_at_ms: 1,
        invited_by: Some(ActorId::new("human:owner")),
    });
    community.upsert_member(CommunityMember {
        actor_id: ActorId::new("human:member"),
        status: MemberStatus::Member,
        admin_title: None,
        admin_rights: AdminRights::default(),
        restrictions: MemberRestrictions::default(),
        joined_at_ms: 1,
        invited_by: Some(ActorId::new("human:owner")),
    });
    engine
        .execute(Command::UpdateCommunity {
            actor_id: ActorId::new("human:owner"),
            community,
        })
        .unwrap();

    engine
        .execute(Command::SetCommunitySlowMode {
            actor_id: ActorId::new("human:owner"),
            conversation_id: ConversationId::new("group:m6"),
            seconds: Some(10),
            changed_at_ms: 10,
        })
        .unwrap();
    let admin_slow_mode_denied = engine
        .execute(Command::SetCommunitySlowMode {
            actor_id: ActorId::new("human:admin"),
            conversation_id: ConversationId::new("group:m6"),
            seconds: Some(20),
            changed_at_ms: 11,
        })
        .unwrap_err();
    assert_eq!(
        admin_slow_mode_denied,
        EngineError::CommunityPermissionDenied
    );

    engine
        .execute(Command::QueueMessage {
            conversation_id: ConversationId::new("group:m6"),
            local_message_id: MessageId::new("message:m6:first"),
            client_message_id: ClientMessageId("client:m6:first".into()),
            sender_id: ActorId::new("human:member"),
            content: text_message("first"),
            reply_to_message_id: None,
            thread_root_message_id: None,
            created_at_ms: 100,
            scheduled_at_ms: None,
            silent: false,
            protected_content: false,
        })
        .unwrap();
    let slow_mode_error = engine
        .execute(Command::QueueMessage {
            conversation_id: ConversationId::new("group:m6"),
            local_message_id: MessageId::new("message:m6:second"),
            client_message_id: ClientMessageId("client:m6:second".into()),
            sender_id: ActorId::new("human:member"),
            content: text_message("second"),
            reply_to_message_id: None,
            thread_root_message_id: None,
            created_at_ms: 500,
            scheduled_at_ms: None,
            silent: false,
            protected_content: false,
        })
        .unwrap_err();
    assert!(matches!(
        slow_mode_error,
        EngineError::SlowModeActive {
            conversation_id,
            retry_at_ms: 10_100
        } if conversation_id == ConversationId::new("group:m6")
    ));

    engine
        .execute(Command::UpsertForumTopic {
            actor_id: ActorId::new("human:admin"),
            topic: ForumTopicState {
                id: "study".into(),
                conversation_id: ConversationId::new("group:m6"),
                title: "Study".into(),
                icon: None,
                creator_id: ActorId::new("human:admin"),
                created_at_ms: 10_100,
                pinned: false,
                closed: false,
                hidden: false,
                unread_count: 0,
                last_message_id: None,
            },
        })
        .unwrap();
    let topic_message_id = MessageId::new("message:m6:topic");
    engine
        .execute(Command::QueueMessage {
            conversation_id: ConversationId::new("group:m6"),
            local_message_id: topic_message_id.clone(),
            client_message_id: ClientMessageId("client:m6:topic".into()),
            sender_id: ActorId::new("human:member"),
            content: text_message("topic"),
            reply_to_message_id: None,
            thread_root_message_id: Some(MessageId::new("topic:study")),
            created_at_ms: 10_100,
            scheduled_at_ms: None,
            silent: false,
            protected_content: false,
        })
        .unwrap();
    engine
        .execute(Command::MarkTopicRead {
            conversation_id: ConversationId::new("group:m6"),
            topic_id: "study".into(),
            actor_id: ActorId::new("human:member"),
            message_id: topic_message_id,
        })
        .unwrap();
    engine
        .execute(Command::SetTopicDraft {
            draft: TopicDraft {
                conversation_id: ConversationId::new("group:m6"),
                topic_id: "study".into(),
                actor_id: ActorId::new("human:member"),
                text: "draft".into(),
                reply_to_message_id: None,
                updated_at_ms: 10_101,
            },
        })
        .unwrap();

    let banned_member = CommunityMember {
        actor_id: ActorId::new("human:member"),
        status: MemberStatus::Banned,
        admin_title: None,
        admin_rights: AdminRights::default(),
        restrictions: MemberRestrictions::default(),
        joined_at_ms: 10_102,
        invited_by: Some(ActorId::new("human:owner")),
    };
    engine
        .execute(Command::ModerateCommunityMember {
            actor_id: ActorId::new("human:admin"),
            conversation_id: ConversationId::new("group:m6"),
            member: banned_member,
            reason: Some("spam".into()),
            decided_at_ms: 10_102,
        })
        .unwrap();
    {
        let state = engine.state();
        let group_id = ConversationId::new("group:m6");
        let member_id = ActorId::new("human:member");
        let community = &state.communities[&group_id];
        assert_eq!(community.members[&member_id].status, MemberStatus::Banned);
        assert!(!state.conversations[&group_id]
            .participants
            .iter()
            .any(|participant| participant.actor_id == member_id));
    }
    let banned_send_error = engine
        .execute(Command::QueueMessage {
            conversation_id: ConversationId::new("group:m6"),
            local_message_id: MessageId::new("message:m6:banned"),
            client_message_id: ClientMessageId("client:m6:banned".into()),
            sender_id: ActorId::new("human:member"),
            content: text_message("blocked"),
            reply_to_message_id: None,
            thread_root_message_id: None,
            created_at_ms: 20_000,
            scheduled_at_ms: None,
            silent: false,
            protected_content: false,
        })
        .unwrap_err();
    assert!(matches!(
        banned_send_error,
        EngineError::SenderNotParticipant {
            conversation_id,
            actor_id
        } if conversation_id == ConversationId::new("group:m6")
            && actor_id == ActorId::new("human:member")
    ));

    let final_community = &engine.state().communities[&ConversationId::new("group:m6")];
    assert!(final_community
        .admin_log
        .iter()
        .any(|entry| entry.action == CommunityAuditAction::SlowModeChanged));
    assert!(final_community
        .admin_log
        .iter()
        .any(|entry| entry.action == CommunityAuditAction::TopicUpserted));
    assert!(final_community
        .admin_log
        .iter()
        .any(|entry| entry.action == CommunityAuditAction::MemberChanged));
}

#[test]
fn deleting_messages_clears_reply_drafts_without_losing_text_and_reconciles_child_windows() {
    let mut engine = MessagingEngine::new();
    for actor_id in ["human:owner", "human:admin"] {
        engine
            .execute(Command::UpsertActor {
                actor: Actor::human(actor_id, actor_id),
            })
            .unwrap();
    }
    engine
        .execute(Command::UpsertConversation {
            conversation: channel_conversation(),
        })
        .unwrap();
    engine
        .execute(Command::UpdateCommunity {
            actor_id: ActorId::new("human:owner"),
            community: channel_community(),
        })
        .unwrap();
    engine
        .execute(Command::UpsertForumTopic {
            actor_id: ActorId::new("human:owner"),
            topic: ForumTopicState {
                id: "study".into(),
                conversation_id: ConversationId::new("channel:m6"),
                title: "Study".into(),
                icon: None,
                creator_id: ActorId::new("human:owner"),
                created_at_ms: 10,
                pinned: false,
                closed: false,
                hidden: false,
                unread_count: 0,
                last_message_id: None,
            },
        })
        .unwrap();

    let conversation_id = ConversationId::new("channel:m6");
    let first_message_id = MessageId::new("message:m6:1");
    let second_message_id = MessageId::new("message:m6:2");
    for (idx, message_id) in [first_message_id.clone(), second_message_id.clone()]
        .into_iter()
        .enumerate()
    {
        engine
            .execute(Command::QueueMessage {
                conversation_id: conversation_id.clone(),
                local_message_id: message_id,
                client_message_id: ClientMessageId(format!("client:m6:{idx}")),
                sender_id: ActorId::new("human:owner"),
                content: text_message("topic post"),
                reply_to_message_id: None,
                thread_root_message_id: Some(MessageId::new("topic:study")),
                created_at_ms: 20 + i64::try_from(idx).unwrap_or(0),
                scheduled_at_ms: None,
                silent: false,
                protected_content: false,
            })
            .unwrap();
    }

    // Root draft with text + reply to first_message_id (text should survive deletion)
    engine
        .execute(Command::SetDraft {
            draft: ConversationDraft {
                conversation_id: conversation_id.clone(),
                actor_id: ActorId::new("human:owner"),
                text: "keep root text".into(),
                reply_to_message_id: Some(first_message_id.0.clone()),
                updated_at_ms: 30,
            },
        })
        .unwrap();
    // Root draft with empty text + reply to first_message_id (should be removed when reply target is deleted)
    engine
        .execute(Command::SetDraft {
            draft: ConversationDraft {
                conversation_id: conversation_id.clone(),
                actor_id: ActorId::new("human:admin"),
                text: String::new(),
                reply_to_message_id: Some(first_message_id.0.clone()),
                updated_at_ms: 31,
            },
        })
        .unwrap();
    // Topic draft with text + reply to first_message_id (text should survive deletion)
    engine
        .execute(Command::SetTopicDraft {
            draft: TopicDraft {
                conversation_id: conversation_id.clone(),
                topic_id: "study".into(),
                actor_id: ActorId::new("human:owner"),
                text: "keep topic text".into(),
                reply_to_message_id: Some(first_message_id.0.clone()),
                updated_at_ms: 32,
            },
        })
        .unwrap();

    let topic_destination = ConversationDestination::topic(conversation_id.clone(), "topic:study");
    engine
        .execute(Command::SetConversationChildPinned {
            destination: topic_destination.clone(),
            actor_id: ActorId::new("human:owner"),
            pinned: true,
        })
        .unwrap();
    engine
        .execute(Command::ReplaceConversationChildWindow {
            destination: topic_destination.clone(),
            actor_id: ActorId::new("human:owner"),
            message_ids: vec![first_message_id.clone()],
            skipped_before: Some(0),
            skipped_after: Some(0),
            full_count: Some(1),
        })
        .unwrap();

    engine
        .execute(Command::DeleteMessages {
            conversation_id: conversation_id.clone(),
            message_ids: vec![first_message_id.clone()],
        })
        .unwrap();

    let state = engine.state();
    let owner_draft = state.drafts[&conversation_id]
        .get(&ActorId::new("human:owner"))
        .expect("owner root draft text survives");
    assert_eq!(owner_draft.text, "keep root text");
    assert_eq!(owner_draft.reply_to_message_id, None);
    assert!(!state.drafts[&conversation_id].contains_key(&ActorId::new("human:admin")));

    let owner_topic_draft = state.topic_drafts[&conversation_id][&ActorId::new("human:owner")]
        .get("study")
        .expect("owner topic draft text survives");
    assert_eq!(owner_topic_draft.text, "keep topic text");
    assert_eq!(owner_topic_draft.reply_to_message_id, None);

    let child_state = state
        .conversation_child_states
        .iter()
        .find(|child| {
            child.destination == topic_destination && child.actor_id == ActorId::new("human:owner")
        })
        .expect("topic child state");
    assert_eq!(child_state.draft_text, "keep topic text");
    assert_eq!(child_state.draft_reply_to_message_id, None);
    assert!(child_state.pagination.message_ids.is_empty());
    assert_eq!(child_state.pagination.full_count, Some(0));
    assert!(!child_state.pinned);
    assert!(child_state.restore_pinned_when_non_empty);

    // Local-to-server identity settlement rekeys reply_to_message_id, drafts,
    // topic_drafts, read cursors, and child pagination windows in place.
    let reply_child_id = MessageId::new("message:m6:3");
    engine
        .execute(Command::QueueMessage {
            conversation_id: conversation_id.clone(),
            local_message_id: reply_child_id.clone(),
            client_message_id: ClientMessageId("client:m6:3".into()),
            sender_id: ActorId::new("human:owner"),
            content: text_message("reply to pending second"),
            reply_to_message_id: Some(second_message_id.clone()),
            thread_root_message_id: Some(MessageId::new("topic:study")),
            created_at_ms: 40,
            scheduled_at_ms: None,
            silent: false,
            protected_content: false,
        })
        .unwrap();
    engine
        .execute(Command::SetTopicDraft {
            draft: TopicDraft {
                conversation_id: conversation_id.clone(),
                topic_id: "study".into(),
                actor_id: ActorId::new("human:owner"),
                text: "replying to second".into(),
                reply_to_message_id: Some(second_message_id.0.clone()),
                updated_at_ms: 41,
            },
        })
        .unwrap();
    engine
        .execute(Command::ReplaceConversationChildWindow {
            destination: topic_destination.clone(),
            actor_id: ActorId::new("human:owner"),
            message_ids: vec![second_message_id.clone(), reply_child_id.clone()],
            skipped_before: Some(0),
            skipped_after: Some(0),
            full_count: Some(2),
        })
        .unwrap();
    let settled_second_id = MessageId::new("server:m6:202");
    engine
        .execute(Command::AcknowledgeMessage {
            conversation_id: conversation_id.clone(),
            local_message_id: second_message_id,
            server_message_id: settled_second_id.clone(),
            accepted_at_ms: 45,
        })
        .unwrap();

    let state = engine.state();
    assert_eq!(
        state.messages[&conversation_id][&reply_child_id].reply_to_message_id,
        Some(settled_second_id.clone())
    );
    let rekeyed_child = state
        .conversation_child_states
        .iter()
        .find(|child| {
            child.destination == topic_destination && child.actor_id == ActorId::new("human:owner")
        })
        .expect("rekeyed topic child state");
    assert_eq!(
        rekeyed_child.pagination.message_ids,
        vec![settled_second_id.0.clone(), reply_child_id.0]
    );
    assert!(rekeyed_child.pinned);
}
