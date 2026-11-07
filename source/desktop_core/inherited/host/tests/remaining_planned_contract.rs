use mahayana_host_runtime::extensions::telemetry::box_log_shipper::{
    classify_offset_save_errno, is_box_log_shipping_enabled, saturating_add, to_source_name,
};
use mahayana_host_runtime::extensions::transcript::background_wakes::{
    BackgroundWakes, build_channel_inbound_wake_prompt, build_timeline_event_wake_prompt,
    distinct_channel_addresses, distinct_inbound_channel_addresses, redrivable_inbound_envelopes,
};
use mahayana_host_runtime::extensions::transcript::client_side_tool_v2_projection::{
    ProjectedClientSideToolV2, ToolProjectionPhase, project_basic_tool_call,
};
use mahayana_host_runtime::extensions::transcript::roster_projection::{
    OUTLINE_STREAM_COALESCE_MS, OutlineStreamCoalescingPolicy, RosterProjection,
};
use mahayana_host_runtime::extensions::transcript::shared_rooms::{
    RoomEntryKind, can_post_to_group, decode_avatar_data_url, is_room_content_entry,
    normalize_group_message,
};
use mahayana_host_runtime::host_gateway_api::{
    CREATE_AGENT_NONCE_LEDGER_CAP, CreateAgentNonceLedger, FROZEN_HOST_GATEWAY_METHODS,
    HOST_CAPABILITIES, HostGatewayOwner, host_gateway_owner, is_sand_agent_purpose,
    sanitize_template_id,
};
use mahayana_host_runtime::sand_host::{
    BOX_READY_REPORT_ATTEMPTS, compute_host_health, should_report_box_ready,
};

#[test]
fn box_log_shipper_core_policy_matches_frozen_contract() {
    assert!(!is_box_log_shipping_enabled(Some("0"), None));
    assert!(is_box_log_shipping_enabled(Some("1"), None));
    assert!(!is_box_log_shipping_enabled(Some("1"), Some(" yes ")));
    assert_eq!(to_source_name("sand-host.log"), "sand-host");
    assert_eq!(to_source_name("sand-host.txt"), "sand-host.txt");
    assert_eq!(classify_offset_save_errno(Some("ENOSPC")), "no_space");
    assert_eq!(
        classify_offset_save_errno(Some("EACCES")),
        "permission_denied"
    );
    assert_eq!(classify_offset_save_errno(None), "unknown");
    assert_eq!(saturating_add(u64::MAX - 1, 3), u64::MAX);
}

#[test]
fn background_wakes_dedupe_addresses_and_fence_parallel_revival() {
    assert_eq!(
        distinct_channel_addresses(["slack:a", "slack:a", "teams:b"]),
        vec!["slack:a", "teams:b"]
    );
    let mut wakes = BackgroundWakes::<u32>::default();
    BackgroundWakes::enqueue(&mut wakes.pending_inbound, "agent", 1);
    BackgroundWakes::enqueue(&mut wakes.pending_inbound, "agent", 2);
    assert!(BackgroundWakes::<u32>::begin_revival(
        &mut wakes.reviving_inbound_agent_ids,
        "agent"
    ));
    assert!(!BackgroundWakes::<u32>::begin_revival(
        &mut wakes.reviving_inbound_agent_ids,
        "agent"
    ));
    assert_eq!(
        BackgroundWakes::take_pending(&mut wakes.pending_inbound, "agent"),
        vec![1, 2]
    );
}

#[test]
fn channel_inbound_wake_prompt_and_redrive_match_frozen_contract() {
    let envelopes = vec![
        serde_json::json!({
            "address": {"platform": "slack", "chat": "C123"},
            "sender": "Ada",
            "text": "Status?",
            "timestampMs": 10,
        }),
        serde_json::json!({
            "address": {"platform": "slack", "chat": "C123"},
            "sender": "Lin",
            "text": "",
            "reaction": {"emoji": "👍", "messageQuote": "Shipped"},
            "timestampMs": 11,
        }),
    ];
    assert_eq!(
        distinct_inbound_channel_addresses(&envelopes),
        vec!["slack:C123"]
    );
    let prompt = build_channel_inbound_wake_prompt(&envelopes);
    assert!(prompt.starts_with("[inbound] New messages on a channel you are connected to."));
    assert!(prompt.contains(
        "On Slack, from slack:C123:\n  Ada: Status?\n  Lin reacted 👍 to your message: \"Shipped\""
    ));
    assert!(prompt.contains("Reply to them by calling SendMessage with the channel target"));

    let redriven = redrivable_inbound_envelopes(&envelopes);
    assert_eq!(redriven.len(), 2);
    assert_eq!(redriven[0]["isDisplayed"], true);
    assert_eq!(redriven[0]["isRedriven"], true);
    assert!(redrivable_inbound_envelopes(&redriven).is_empty());
}

#[test]
fn timeline_event_wake_prompt_matches_frozen_system_event_contract() {
    let prompt = build_timeline_event_wake_prompt(&[serde_json::json!({
        "type": "name-changed",
        "to": "Researcher"
    })]);
    assert_eq!(
        prompt,
        "[event] Something about this conversation just changed.\nThis is a system event recorded in your timeline, not the user typing in this app, and possibly something you did yourself.\n- Renamed to Researcher\nIf it is worth acknowledging to the user, reply with SendMessage; otherwise it is fine to stay silent."
    );
}

#[test]
fn client_projection_requires_identity_and_preserves_phase() {
    assert!(
        project_basic_tool_call(ToolProjectionPhase::Started, "", "shell", "{}", "", None)
            .is_none()
    );
    let projected = project_basic_tool_call(
        ToolProjectionPhase::Partial,
        "tc-1",
        "shell",
        "{\"command\":\"pwd\"}",
        "model-1",
        None,
    )
    .unwrap();
    assert!(matches!(
        projected,
        ProjectedClientSideToolV2::Call {
            is_streaming: true,
            ..
        }
    ));
    let completed = project_basic_tool_call(
        ToolProjectionPhase::Completed,
        "tc-1",
        "shell",
        "{}",
        "",
        Some(Err("denied")),
    )
    .unwrap();
    assert!(matches!(
        completed,
        ProjectedClientSideToolV2::Result { error: Some(_), .. }
    ));
}

#[test]
fn roster_coalescing_keeps_only_latest_pending_outline() {
    assert_eq!(OUTLINE_STREAM_COALESCE_MS, 250);
    let mut roster = RosterProjection::default();
    roster.queue_stream_outline_update("a", "first");
    roster.queue_stream_outline_update("a", "second");
    let update = roster.flush_stream_outline_update().unwrap();
    assert_eq!(update.item, "second");
    assert!(roster.flush_stream_outline_update().is_none());
}

#[test]
fn roster_projection_owns_stream_item_identity_and_lifecycle() {
    let mut roster = RosterProjection::<String>::default();
    let (first, inserted) =
        roster.stream_outline_item_id_or_insert_with("stream-a", || "item-a".into());
    assert!(inserted);
    assert_eq!(first, "item-a");
    let (same, inserted) =
        roster.stream_outline_item_id_or_insert_with("stream-a", || "unused".into());
    assert!(!inserted);
    assert_eq!(same, "item-a");
    assert!(roster.has_outline_stream("stream-a"));
    assert!(roster.finish_outline_stream("stream-a"));
    assert!(!roster.has_outline_stream("stream-a"));
    assert!(!roster.finish_outline_stream("stream-a"));

    roster.stream_outline_item_id_or_insert_with("stream-b", || "item-b".into());
    roster.clear_outline_streams();
    assert!(!roster.has_outline_stream("stream-b"));
}

#[test]
fn outline_stream_policy_enforces_frozen_deadline_merge_flush_and_stop_semantics() {
    let mut policy = OutlineStreamCoalescingPolicy::new(OUTLINE_STREAM_COALESCE_MS, 1_000);

    let first = policy.queue(1_100, "agent-a", "item-a", "first");
    assert!(first.flushed.is_empty());
    assert_eq!(first.deadline.expect("first deadline").at_ms, 1_350);

    let second = policy.queue(1_200, "agent-a", "item-a", "second");
    let second_deadline = second.deadline.expect("debounced deadline");
    assert!(second.flushed.is_empty());
    assert_eq!(second_deadline.at_ms, 1_450);
    assert!(
        policy
            .flush_deadline(1_449, second_deadline.generation)
            .is_none()
    );
    assert_eq!(
        policy
            .flush_deadline(1_450, second_deadline.generation)
            .expect("deadline flush")
            .item,
        "second"
    );

    let same_window = policy.queue(1_500, "agent-a", "item-a", "third");
    assert!(same_window.flushed.is_empty());
    let different = policy.queue(1_510, "agent-a", "item-b", "other");
    assert_eq!(different.flushed.len(), 1);
    assert_eq!(different.flushed[0].item, "third");
    assert_eq!(different.deadline.expect("new item deadline").at_ms, 1_760);

    assert_eq!(policy.flush(1_520).expect("explicit flush").item, "other");
    let immediate = policy.queue(1_800, "agent-a", "item-c", "late");
    assert_eq!(immediate.flushed.len(), 1);
    assert_eq!(immediate.flushed[0].item, "late");
    assert!(immediate.deadline.is_none());

    let pending = policy.queue(1_810, "agent-a", "item-d", "pending");
    assert!(pending.deadline.is_some());
    assert!(policy.has_pending());
    policy.stop();
    assert!(!policy.has_pending());
    assert!(policy.next_deadline().is_none());
    assert!(!policy.is_enabled());

    let disabled = policy.queue(1_820, "agent-a", "item-e", "direct");
    assert_eq!(disabled.flushed.len(), 1);
    assert_eq!(disabled.flushed[0].item, "direct");
    assert!(disabled.deadline.is_none());
}

#[test]
fn shared_rooms_avatar_content_and_membership_are_fail_closed() {
    assert_eq!(
        decode_avatar_data_url("data:image/png;base64,YQ=="),
        Some(b"a".to_vec())
    );
    assert!(decode_avatar_data_url("data:text/plain;base64,YQ==").is_none());
    assert!(is_room_content_entry(RoomEntryKind::Message));
    assert!(is_room_content_entry(RoomEntryKind::SendMessage));
    assert!(!is_room_content_entry(RoomEntryKind::Other));
    assert!(normalize_group_message("(pass)", 100).is_none());
    assert_eq!(
        normalize_group_message(" hello ", 100).as_deref(),
        Some("hello")
    );
    assert!(can_post_to_group(&["a".into(), "b".into()], "b"));
    assert!(!can_post_to_group(&["a".into()], "b"));
}

#[test]
fn host_gateway_nonce_and_template_fences_match_reference() {
    assert_eq!(HOST_CAPABILITIES, ["orderedReplicasV1", "sendAcceptanceV1"]);
    assert!(is_sand_agent_purpose("disk-saver"));
    assert!(!is_sand_agent_purpose("other"));
    assert_eq!(sanitize_template_id("abc-123"), Some("abc-123"));
    assert!(sanitize_template_id("ABC").is_none());

    let mut ledger = CreateAgentNonceLedger::default();
    for index in 0..(CREATE_AGENT_NONCE_LEDGER_CAP + 5) {
        ledger.insert(format!("n-{index}"), index);
    }
    assert_eq!(ledger.len(), CREATE_AGENT_NONCE_LEDGER_CAP);
    assert!(ledger.get("n-0").is_none());
    assert_eq!(ledger.get("n-68"), Some(&68));
}

#[test]
fn sand_host_health_does_not_refresh_busy_clock_for_approval_only_wait() {
    assert_eq!(BOX_READY_REPORT_ATTEMPTS, 3);
    let health = compute_host_health(
        ["a".to_string()],
        ["a".to_string()],
        false,
        Some("a".into()),
        200,
        100,
    );
    assert!(health.is_busy);
    assert!(health.busy_only_awaiting_approval);
    assert_eq!(health.last_busy_at_ms, 100);

    let active = compute_host_health(
        ["a".to_string()],
        Vec::<String>::new(),
        false,
        Some("a".into()),
        200,
        100,
    );
    assert_eq!(active.last_busy_at_ms, 200);
    assert!(should_report_box_ready(
        Some("boot-1"),
        Some(10),
        Some("boot-0")
    ));
    assert!(!should_report_box_ready(
        Some("boot-1"),
        Some(10),
        Some("boot-1")
    ));
}


#[test]
fn frozen_host_gateway_registry_is_complete_unique_and_owner_resolved() {
    const EXPECTED_FROZEN_METHODS: &[&str] = &[
        "getTranscript", "getAgentTranscript", "getAgentTranscriptPage",
        "getAgentTranscriptWindow", "getAgentTranscriptTail", "getAgentThread",
        "sendPrompt", "promptAcceptanceStatus", "respondToWidget",
        "resolveAutoReviewApproval", "resolveLocalToolPermission", "dismissWidget",
        "submitSecret", "reactToMessage", "appendConnectorCard", "listAgents",
        "countAgents", "searchAgents", "searchMedia", "createAgent", "kickstartAgent",
        "requestDiskSaverAudit", "createGroup", "setGroupMembers", "updateAgent",
        "deleteAgent", "deleteAgents", "duplicateAgent", "setAgentUnread",
        "setAgentNotificationsEnabled", "setAgentNotifyOnUpdates",
        "setAgentHiddenFromSidebar", "openAgent", "openAgentWindowed", "openAgentTail",
        "setWindowFocused", "getAgentMemories", "deleteAgentMemory", "clearAgentMemories",
        "getAgentAutomations", "listAllAutomations", "isAgentNetworkEnabled",
        "isGlobalSearchEnabled", "isEgressTunnelAvailable", "getSharingState",
        "createRoomFromAgent", "createRoomInvite", "joinSharedRoom",
        "respondToRoomJoinRequest", "createSharedRoom", "addOwnAgentToSharedRoom",
        "removeOwnAgentFromSharedRoom", "setSharedRoomTyping", "leaveSharedRoom",
        "setAgentAutomationEnabled", "createAgentAutomation", "updateAgentAutomation",
        "deleteAgentAutomation", "runAgentAutomationNow", "broadcastToAgents",
        "getAgentWorkflows", "createAgentWorkflow", "updateAgentWorkflow",
        "setAgentWorkflowEnabled", "deleteAgentWorkflow", "runAgentWorkflowNow",
        "importAgentWorkflowText", "importAgentWorkflowUrl", "portAgentLocalSkills",
        "getConversationOutline", "skillsCatalog", "syncPluginSkills",
        "getPluginSyncStatus", "getSkillPublishTargets", "publishSkill",
        "resyncPublishedSkill", "unpublishSkill", "getAgentChannels", "connectChannel",
        "disconnectChannel", "refreshChannel", "getListenerIntegrations",
        "getListenerConnectUrl", "getSubagents", "getAsyncTasks", "setAgentAvatarBytes",
        "getAgentAvatar", "getForeverBoxStatus", "getCloudAgentInfo", "ensureForeverBox",
        "resetForeverBox", "updateForeverBox", "autoUpdateBoxNow", "snapshotBoxStoreNow",
        "getBoxStoreStatus", "clearBoxStoreNow", "updateHostNow", "getHostStatus",
        "setBoxMigrating", "prepareBoxForRecreate", "resumeBoxAfterRecreate",
        "handBackForeverBox", "startTeachRecording", "stopTeachRecording",
        "getTeachRecordingStatus", "getTrays", "dismissTray", "clearTrays",
        "uploadAttachment", "readAttachmentImage", "readAttachmentText",
        "readAttachmentChunk", "getHostSettings", "setHostSettings", "refreshMcp",
        "listRoutedMcpTools", "executeRoutedMcpTool", "listBoxMcpServers",
        "completeMcpOAuth", "requestWebAuthnCeremony", "setBoxSecrets",
        "getBoxSecretsStatus",
    ];

    assert_eq!(FROZEN_HOST_GATEWAY_METHODS, EXPECTED_FROZEN_METHODS);
    assert_eq!(FROZEN_HOST_GATEWAY_METHODS.len(), 122);

    let mut unique = std::collections::BTreeSet::new();
    for method in FROZEN_HOST_GATEWAY_METHODS {
        assert!(unique.insert(*method), "duplicate frozen gateway method: {method}");
        assert!(
            host_gateway_owner(method).is_some(),
            "frozen gateway method must resolve to exactly one owner: {method}"
        );
    }

    assert_eq!(
        host_gateway_owner("sendPrompt"),
        Some(HostGatewayOwner::TranscriptManager)
    );
    assert_eq!(
        host_gateway_owner("resolveAutoReviewApproval"),
        Some(HostGatewayOwner::AutoReview)
    );
    assert_eq!(
        host_gateway_owner("createAgent"),
        Some(HostGatewayOwner::AgentLifecycle)
    );
    assert_eq!(
        host_gateway_owner("createAgentAutomation"),
        Some(HostGatewayOwner::Automations)
    );
    assert_eq!(
        host_gateway_owner("runAgentWorkflowNow"),
        Some(HostGatewayOwner::Workflow)
    );
    assert_eq!(
        host_gateway_owner("refreshMcp"),
        Some(HostGatewayOwner::Mcp)
    );
    assert_eq!(
        host_gateway_owner("listRoutedMcpTools"),
        Some(HostGatewayOwner::Mcp)
    );
    assert_eq!(
        host_gateway_owner("executeRoutedMcpTool"),
        Some(HostGatewayOwner::Mcp)
    );
    assert_eq!(
        host_gateway_owner("resumeBoxAfterRecreate"),
        Some(HostGatewayOwner::HostControl)
    );
    assert_eq!(
        host_gateway_owner("uploadAttachment"),
        Some(HostGatewayOwner::Attachments)
    );
    assert_eq!(
        host_gateway_owner("setBoxSecrets"),
        Some(HostGatewayOwner::Secrets)
    );
    assert_eq!(host_gateway_owner("resumeAfterRecreate"), None);
}
