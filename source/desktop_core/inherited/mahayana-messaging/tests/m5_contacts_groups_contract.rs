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

fn community_member(actor_id: &str, status: MemberStatus) -> CommunityMember {
    CommunityMember {
        actor_id: ActorId::new(actor_id),
        status,
        admin_title: None,
        admin_rights: AdminRights::default(),
        restrictions: MemberRestrictions::default(),
        joined_at_ms: 1,
        invited_by: Some(ActorId::new("human:owner")),
    }
}

fn group_conversation(id: &str) -> Conversation {
    let mut conversation = Conversation::direct(
        id,
        "Dharma Study Group",
        vec![
            participant("human:owner", ParticipantRole::Owner),
            participant("human:admin", ParticipantRole::Admin),
            participant("human:member", ParticipantRole::Member),
            participant("human:restricted", ParticipantRole::Restricted),
        ],
        1,
    );
    conversation.kind = ConversationKind::Group;
    conversation.owner_id = Some(ActorId::new("human:owner"));
    conversation.permissions.can_send_messages = true;
    conversation.permissions.can_send_media = true;
    conversation.permissions.can_send_polls = true;
    conversation
}

fn community(id: &str) -> CommunityState {
    let mut community = CommunityState::new(ConversationId::new(id));
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
        admin_title: Some("Invites".into()),
        admin_rights: AdminRights {
            invite_members: true,
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
    community.upsert_member(CommunityMember {
        actor_id: ActorId::new("human:restricted"),
        status: MemberStatus::Restricted,
        admin_title: None,
        admin_rights: AdminRights::default(),
        restrictions: MemberRestrictions {
            send_messages: true,
            send_media: true,
            send_polls: true,
            ..MemberRestrictions::default()
        },
        joined_at_ms: 1,
        invited_by: Some(ActorId::new("human:owner")),
    });
    community
}

fn seed_engine(group_id: &str) -> MessagingEngine {
    let mut engine = MessagingEngine::new();
    for actor_id in [
        "human:owner",
        "human:admin",
        "human:member",
        "human:restricted",
    ] {
        engine
            .execute(Command::UpsertActor {
                actor: Actor::human(actor_id, actor_id),
            })
            .unwrap();
    }
    engine
        .execute(Command::UpsertConversation {
            conversation: group_conversation(group_id),
        })
        .unwrap();
    engine
        .execute(Command::UpdateCommunity {
            actor_id: ActorId::new("human:owner"),
            community: community(group_id),
        })
        .unwrap();
    engine
}

#[test]
fn contacts_and_groups_are_searchable_through_the_messaging_protocol() {
    let mut service = MessagingService::load(MemoryStateStore::default()).unwrap();
    for (id, name) in [
        ("human:owner", "Alice Owner"),
        ("human:admin", "Bob Admin"),
        ("human:member", "Carol Member"),
        ("human:restricted", "Restricted Member"),
    ] {
        service
            .handle(
                ClientEnvelope::new(
                    context(id, &format!("profile:{id}")),
                    ClientCommand::UpsertProfile {
                        actor: Actor::human(id, name),
                    },
                ),
                1,
            )
            .unwrap();
    }
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "group"),
                ClientCommand::CreateConversation {
                    conversation: group_conversation("group:m5-search"),
                },
            ),
            2,
        )
        .unwrap();

    let contacts = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "search-contacts"),
                ClientCommand::Search {
                    query: SearchQuery {
                        text: "alice".into(),
                        scope: SearchScope::Contacts,
                        conversation_id: None,
                        sender_id: None,
                        from_ms: None,
                        to_ms: None,
                        limit: 10,
                    },
                },
            ),
            3,
        )
        .unwrap();
    assert!(matches!(
        &contacts[0].event,
        ServerEvent::SearchResults { results, .. }
            if results.iter().any(|result| result.id == "human:owner")
    ));

    let groups = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "search-groups"),
                ClientCommand::Search {
                    query: SearchQuery {
                        text: "dharma".into(),
                        scope: SearchScope::Groups,
                        conversation_id: None,
                        sender_id: None,
                        from_ms: None,
                        to_ms: None,
                        limit: 10,
                    },
                },
            ),
            4,
        )
        .unwrap();
    assert!(matches!(
        &groups[0].event,
        ServerEvent::SearchResults { results, .. }
            if results.iter().any(|result| result.id == "group:m5-search")
    ));

    let mut blocked = group_conversation("group:m5-search-blocked");
    blocked.title = "Dharma Blocked Group".into();
    blocked.permissions.can_send_messages = false;
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "group-blocked"),
                ClientCommand::CreateConversation {
                    conversation: blocked,
                },
            ),
            5,
        )
        .unwrap();

    let mut media_blocked = group_conversation("group:m5-search-media-blocked");
    media_blocked.title = "Dharma Media Blocked".into();
    media_blocked.permissions.can_send_media = false;
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "group-media-blocked"),
                ClientCommand::CreateConversation {
                    conversation: media_blocked,
                },
            ),
            6,
        )
        .unwrap();

    let recipients = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "search-recipients"),
                ClientCommand::SearchRecipients {
                    query: SearchQuery {
                        text: "dharma".into(),
                        scope: SearchScope::Global,
                        conversation_id: None,
                        sender_id: None,
                        from_ms: None,
                        to_ms: None,
                        limit: 10,
                    },
                    requirements: RecipientSearchRequirements::default(),
                },
            ),
            7,
        )
        .unwrap();
    assert!(matches!(
        &recipients[0].event,
        ServerEvent::SearchResults { results, .. }
            if results.iter().any(|result| result.id == "group:m5-search")
                && results.iter().any(|result| result.id == "group:m5-search-media-blocked")
                && results.iter().all(|result| result.id != "group:m5-search-blocked")
                && results.iter().all(|result| matches!(result.kind, SearchResultKind::Conversation))
    ));

    let media_recipients = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "search-media-recipients"),
                ClientCommand::SearchRecipients {
                    query: SearchQuery {
                        text: "dharma".into(),
                        scope: SearchScope::Global,
                        conversation_id: None,
                        sender_id: None,
                        from_ms: None,
                        to_ms: None,
                        limit: 10,
                    },
                    requirements: RecipientSearchRequirements {
                        require_media: true,
                        require_polls: false,
                        ..RecipientSearchRequirements::default()
                    },
                },
            ),
            8,
        )
        .unwrap();
    assert!(matches!(
        &media_recipients[0].event,
        ServerEvent::SearchResults { results, .. }
            if results.iter().any(|result| result.id == "group:m5-search")
                && results.iter().all(|result| result.id != "group:m5-search-blocked")
                && results.iter().all(|result| result.id != "group:m5-search-media-blocked")
    ));
}

#[test]
fn share_recipient_search_uses_canonical_typed_permission_axes_before_exposure() {
    let mut service = MessagingService::load(MemoryStateStore::default()).unwrap();
    for (id, name) in [
        ("human:owner", "Owner"),
        ("human:admin", "Admin"),
        ("human:member", "Member"),
        ("human:restricted", "Restricted"),
    ] {
        service
            .handle(
                ClientEnvelope::new(
                    context(id, &format!("profile:{id}")),
                    ClientCommand::UpsertProfile {
                        actor: Actor::human(id, name),
                    },
                ),
                1,
            )
            .unwrap();
    }

    let mut allowed = group_conversation("group:share-allowed");
    allowed.title = "Share Allowed".into();
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "create-share-allowed"),
                ClientCommand::CreateConversation {
                    conversation: allowed,
                },
            ),
            2,
        )
        .unwrap();

    let mut allowed_community = community("group:share-allowed");
    allowed_community.upsert_member(CommunityMember {
        actor_id: ActorId::new("human:restricted"),
        status: MemberStatus::Restricted,
        admin_title: None,
        admin_rights: AdminRights::default(),
        restrictions: MemberRestrictions {
            send_messages: false,
            send_other: true,
            send_inline: true,
            send_games: true,
            ..MemberRestrictions::default()
        },
        joined_at_ms: 1,
        invited_by: Some(ActorId::new("human:owner")),
    });
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "community-share-allowed"),
                ClientCommand::UpdateCommunity {
                    community: allowed_community,
                },
            ),
            3,
        )
        .unwrap();

    let mut other_blocked = group_conversation("group:share-other-blocked");
    other_blocked.title = "Share Other Blocked".into();
    other_blocked.permissions.can_send_other = false;
    let mut inline_blocked = group_conversation("group:share-inline-blocked");
    inline_blocked.title = "Share Inline Blocked".into();
    inline_blocked.permissions.can_send_inline = false;
    let mut game_blocked = group_conversation("group:share-game-blocked");
    game_blocked.title = "Share Game Blocked".into();
    game_blocked.permissions.can_send_games = false;

    for (index, conversation) in [other_blocked, inline_blocked, game_blocked]
        .into_iter()
        .enumerate()
    {
        service
            .handle(
                ClientEnvelope::new(
                    context("human:owner", &format!("create-share-blocked-{index}")),
                    ClientCommand::CreateConversation { conversation },
                ),
                4 + i64::try_from(index).unwrap(),
            )
            .unwrap();
    }

    let query = || SearchQuery {
        text: "share".into(),
        scope: SearchScope::Global,
        conversation_id: None,
        sender_id: None,
        from_ms: None,
        to_ms: None,
        limit: 20,
    };
    let mut search = |actor_id: &str, request_id: &str, requirements: RecipientSearchRequirements| {
        let response = service
            .handle(
                ClientEnvelope::new(
                    context(actor_id, request_id),
                    ClientCommand::SearchRecipients {
                        query: query(),
                        requirements,
                    },
                ),
                10,
            )
            .unwrap();
        match &response[0].event {
            ServerEvent::SearchResults { results, .. } => results.clone(),
            other => panic!("unexpected recipient search event: {other:?}"),
        }
    };

    let other = search(
        "human:owner",
        "share-other",
        RecipientSearchRequirements {
            require_send_other: true,
            ..RecipientSearchRequirements::default()
        },
    );
    assert!(other.iter().any(|result| result.id == "group:share-allowed"));
    assert!(other
        .iter()
        .all(|result| result.id != "group:share-other-blocked"));

    let inline = search(
        "human:owner",
        "share-inline",
        RecipientSearchRequirements {
            require_inline: true,
            ..RecipientSearchRequirements::default()
        },
    );
    assert!(inline.iter().any(|result| result.id == "group:share-allowed"));
    assert!(inline
        .iter()
        .all(|result| result.id != "group:share-inline-blocked"));

    let game = search(
        "human:owner",
        "share-game",
        RecipientSearchRequirements {
            require_game: true,
            ..RecipientSearchRequirements::default()
        },
    );
    assert!(game.iter().any(|result| result.id == "group:share-allowed"));
    assert!(game
        .iter()
        .all(|result| result.id != "group:share-game-blocked"));

    let restricted_default = search(
        "human:restricted",
        "share-restricted-default",
        RecipientSearchRequirements::default(),
    );
    assert!(restricted_default
        .iter()
        .any(|result| result.id == "group:share-allowed"));

    for (request_id, requirements) in [
        (
            "share-restricted-other",
            RecipientSearchRequirements {
                require_send_other: true,
                ..RecipientSearchRequirements::default()
            },
        ),
        (
            "share-restricted-inline",
            RecipientSearchRequirements {
                require_inline: true,
                ..RecipientSearchRequirements::default()
            },
        ),
        (
            "share-restricted-game",
            RecipientSearchRequirements {
                require_game: true,
                ..RecipientSearchRequirements::default()
            },
        ),
    ] {
        let restricted = search("human:restricted", request_id, requirements);
        assert!(restricted
            .iter()
            .all(|result| result.id != "group:share-allowed"));
    }
}

#[test]
fn typed_share_permission_fields_keep_legacy_persistence_fail_closed() {
    let legacy_permissions: ConversationPermissions = serde_json::from_str(
        r#"{"canSendMessages":true,"canSendMedia":true,"canSendPolls":true,"canAddMembers":true,"canPinMessages":true,"canManageTopics":true,"canManageCalls":true}"#,
    )
    .unwrap();
    assert!(!legacy_permissions.can_send_other);
    assert!(!legacy_permissions.can_send_inline);
    assert!(!legacy_permissions.can_send_games);

    let fresh_permissions = ConversationPermissions::default();
    assert!(fresh_permissions.can_send_other);
    assert!(fresh_permissions.can_send_inline);
    assert!(fresh_permissions.can_send_games);

    let legacy_restrictions: MemberRestrictions = serde_json::from_str(
        r#"{"sendMessages":false,"sendMedia":false,"sendPolls":false,"embedLinks":false,"addMembers":false,"pinMessages":false,"changeInfo":false,"untilMs":null}"#,
    )
    .unwrap();
    assert!(!legacy_restrictions.send_other);
    assert!(!legacy_restrictions.send_inline);
    assert!(!legacy_restrictions.send_games);

    let mut engine = seed_engine("group:share-send-other");
    let mut source_group = group_conversation("group:share-source-dice");
    source_group.title = "Share Dice Source".into();
    engine
        .execute(Command::UpsertConversation {
            conversation: source_group,
        })
        .unwrap();
    engine
        .execute(Command::QueueMessage {
            conversation_id: ConversationId::new("group:share-source-dice"),
            local_message_id: MessageId::new("msg:dice-source"),
            client_message_id: ClientMessageId("client:dice-source".into()),
            sender_id: ActorId::new("human:owner"),
            content: MessageContent::Dice {
                emoji: "🎲".into(),
                value: 6,
            },
            reply_to_message_id: None,
            thread_root_message_id: None,
            created_at_ms: 10,
            scheduled_at_ms: None,
            silent: false,
            protected_content: false,
        })
        .unwrap();

    let mut no_other_group = group_conversation("group:share-no-other");
    no_other_group.permissions.can_send_other = false;
    engine
        .execute(Command::UpsertConversation {
            conversation: no_other_group,
        })
        .unwrap();
    assert_eq!(
        engine.execute(Command::ForwardMessage {
            source_conversation_id: ConversationId::new("group:share-source-dice"),
            message_id: MessageId::new("msg:dice-source"),
            destination_conversation_id: ConversationId::new("group:share-no-other"),
            local_message_id: MessageId::new("msg:dice-forward-denied"),
            client_message_id: ClientMessageId("client:dice-forward-denied".into()),
            sender_id: ActorId::new("human:member"),
            thread_root_message_id: None,
            created_at_ms: 11,
            scheduled_at_ms: None,
            silent: false,
            privacy: ForwardPrivacy::default(),
        }),
        Err(EngineError::OtherSendPermissionDenied(ConversationId::new(
            "group:share-no-other"
        )))
    );

    let mut restricted_member = community_member("human:restricted", MemberStatus::Restricted);
    restricted_member.restrictions.send_other = true;
    engine
        .execute(Command::SetCommunityMember {
            actor_id: ActorId::new("human:owner"),
            conversation_id: ConversationId::new("group:share-send-other"),
            member: restricted_member,
        })
        .unwrap();
    assert_eq!(
        engine.execute(Command::ForwardMessage {
            source_conversation_id: ConversationId::new("group:share-source-dice"),
            message_id: MessageId::new("msg:dice-source"),
            destination_conversation_id: ConversationId::new("group:share-send-other"),
            local_message_id: MessageId::new("msg:dice-community-denied"),
            client_message_id: ClientMessageId("client:dice-community-denied".into()),
            sender_id: ActorId::new("human:restricted"),
            thread_root_message_id: None,
            created_at_ms: 12,
            scheduled_at_ms: None,
            silent: false,
            privacy: ForwardPrivacy::default(),
        }),
        Err(EngineError::CommunityOtherRestricted(ConversationId::new(
            "group:share-send-other"
        )))
    );
}

#[test]
fn admin_rights_are_operation_specific() {
    let mut engine = seed_engine("group:m5-admin");

    engine
        .execute(Command::CreateInviteLink {
            actor_id: ActorId::new("human:admin"),
            invite: InviteLink {
                id: "invite:m5".into(),
                conversation_id: ConversationId::new("group:m5-admin"),
                creator_id: ActorId::new("human:admin"),
                token: "m5-token".into(),
                name: Some("M5".into()),
                created_at_ms: 2,
                expires_at_ms: None,
                member_limit: None,
                join_request: false,
                revoked: false,
                joined_count: 0,
            },
        })
        .unwrap();

    let topic_denied = engine
        .execute(Command::UpsertForumTopic {
            actor_id: ActorId::new("human:admin"),
            topic: ForumTopicState {
                id: "topic:m5-denied".into(),
                conversation_id: ConversationId::new("group:m5-admin"),
                title: "Denied".into(),
                icon: None,
                creator_id: ActorId::new("human:admin"),
                created_at_ms: 3,
                pinned: false,
                closed: false,
                hidden: false,
                unread_count: 0,
                last_message_id: None,
            },
        })
        .unwrap_err();
    assert_eq!(topic_denied, EngineError::CommunityPermissionDenied);

    let member_denied = engine
        .execute(Command::CreateInviteLink {
            actor_id: ActorId::new("human:member"),
            invite: InviteLink {
                id: "invite:member".into(),
                conversation_id: ConversationId::new("group:m5-admin"),
                creator_id: ActorId::new("human:member"),
                token: "member-token".into(),
                name: None,
                created_at_ms: 4,
                expires_at_ms: None,
                member_limit: None,
                join_request: false,
                revoked: false,
                joined_count: 0,
            },
        })
        .unwrap_err();
    assert_eq!(member_denied, EngineError::CommunityPermissionDenied);

    let mut promoted = engine.state().communities[&ConversationId::new("group:m5-admin")].members
        [&ActorId::new("human:admin")]
        .clone();
    promoted.admin_rights.manage_topics = true;
    engine
        .execute(Command::SetCommunityMember {
            actor_id: ActorId::new("human:owner"),
            conversation_id: ConversationId::new("group:m5-admin"),
            member: promoted,
        })
        .unwrap();
    engine
        .execute(Command::UpsertForumTopic {
            actor_id: ActorId::new("human:admin"),
            topic: ForumTopicState {
                id: "topic:m5-allowed".into(),
                conversation_id: ConversationId::new("group:m5-admin"),
                title: "Allowed".into(),
                icon: None,
                creator_id: ActorId::new("human:admin"),
                created_at_ms: 5,
                pinned: false,
                closed: false,
                hidden: false,
                unread_count: 0,
                last_message_id: None,
            },
        })
        .unwrap();
}

#[test]
fn restricted_members_cannot_bypass_group_send_restrictions() {
    let mut engine = seed_engine("group:m5-restricted");
    let error = engine
        .execute(Command::QueueMessage {
            conversation_id: ConversationId::new("group:m5-restricted"),
            local_message_id: MessageId::new("message:restricted"),
            client_message_id: ClientMessageId("client:restricted".into()),
            sender_id: ActorId::new("human:restricted"),
            content: MessageContent::Text {
                text: FormattedText::plain("blocked"),
            },
            reply_to_message_id: None,
            thread_root_message_id: None,
            created_at_ms: 2,
            scheduled_at_ms: None,
            silent: false,
            protected_content: false,
        })
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::CommunitySendRestricted(id) if id == ConversationId::new("group:m5-restricted")
    ));
}

#[test]
fn group_unread_is_actor_scoped_and_clears_only_for_reader() {
    let mut service = MessagingService::load(MemoryStateStore::default()).unwrap();
    for (id, name) in [
        ("human:owner", "Owner"),
        ("human:admin", "Admin"),
        ("human:member", "Member"),
        ("human:restricted", "Restricted"),
    ] {
        service
            .handle(
                ClientEnvelope::new(
                    context(id, &format!("profile:{id}")),
                    ClientCommand::UpsertProfile {
                        actor: Actor::human(id, name),
                    },
                ),
                1,
            )
            .unwrap();
    }
    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "group"),
                ClientCommand::CreateConversation {
                    conversation: group_conversation("group:m5-unread"),
                },
            ),
            2,
        )
        .unwrap();

    let sent = service
        .handle(
            ClientEnvelope::new(
                context("human:admin", "send"),
                ClientCommand::SendMessage {
                    conversation_id: ConversationId::new("group:m5-unread"),
                    client_message_id: ClientMessageId("client:m5-unread".into()),
                    content: MessageContent::Text {
                        text: FormattedText::plain("group unread"),
                    },
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
        .find_map(|event| match &event.event {
            ServerEvent::MessageAdded { message } => Some(message.id.clone()),
            _ => None,
        })
        .unwrap();

    let owner_sync = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "sync-owner"),
                ClientCommand::Sync {
                    cursor: None,
                    limit: 100,
                },
            ),
            4,
        )
        .unwrap();
    assert!(owner_sync.iter().any(|event| matches!(
        &event.event,
        ServerEvent::SyncBatch { conversations, .. }
            if conversations.iter().any(|conversation| conversation.id == ConversationId::new("group:m5-unread") && conversation.unread_count == 1)
    )));

    service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "read-owner"),
                ClientCommand::MarkRead {
                    conversation_id: ConversationId::new("group:m5-unread"),
                    message_id,
                },
            ),
            5,
        )
        .unwrap();
    let owner_after = service
        .handle(
            ClientEnvelope::new(
                context("human:owner", "sync-owner-after"),
                ClientCommand::Sync {
                    cursor: None,
                    limit: 100,
                },
            ),
            6,
        )
        .unwrap();
    assert!(owner_after.iter().any(|event| matches!(
        &event.event,
        ServerEvent::SyncBatch { conversations, .. }
            if conversations.iter().any(|conversation| conversation.id == ConversationId::new("group:m5-unread") && conversation.unread_count == 0)
    )));

    let member_sync = service
        .handle(
            ClientEnvelope::new(
                context("human:member", "sync-member"),
                ClientCommand::Sync {
                    cursor: None,
                    limit: 100,
                },
            ),
            7,
        )
        .unwrap();
    assert!(member_sync.iter().any(|event| matches!(
        &event.event,
        ServerEvent::SyncBatch { conversations, .. }
            if conversations.iter().any(|conversation| conversation.id == ConversationId::new("group:m5-unread") && conversation.unread_count == 1)
    )));
}
