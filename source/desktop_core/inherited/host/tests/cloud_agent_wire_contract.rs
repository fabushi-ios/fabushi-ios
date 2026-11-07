use prost::Message;

use mahayana_host_runtime::extensions::cloud_agents::cloud_agent_wire::{
    AddAsyncFollowupBackgroundComposerRequestWire, ConversationActionWire,
    GetBackgroundComposerConversationResponseWire, SelectedContextWire, SelectedImageWire,
    StartBackgroundComposerRequestWire, UserMessageActionWire, UserMessageWire,
    AGENT_MODE_AGENT, BACKGROUND_COMPOSER_SOURCE_GROK_BOT, STARTING_MESSAGE_TYPE_USER_MESSAGE,
};

#[test]
fn launch_wire_uses_frozen_cloud_agent_field_numbers() {
    let request = StartBackgroundComposerRequestWire {
        snapshot_name_or_id: "github.com/acme/repo".into(),
        repository_info: None,
        snapshot_workspace_root_path: "/workspace".into(),
        auto_branch: true,
        return_immediately: true,
        devcontainer_starting_point: None,
        repo_url: Some("https://github.com/acme/repo".into()),
        bc_id: "bc-1".into(),
        source: Some(BACKGROUND_COMPOSER_SOURCE_GROK_BOT),
        add_initial_message_to_responses: Some(true),
        base_branch: None,
        auto_create_pr: Some(true),
        starting_message_type: Some(STARTING_MESSAGE_TYPE_USER_MESSAGE),
        conversation_action: Some(ConversationActionWire {
            user_message_action: Some(UserMessageActionWire {
                user_message: Some(UserMessageWire {
                    text: "hello".into(),
                    message_id: "m-1".into(),
                    selected_context: Some(SelectedContextWire {
                        selected_images: vec![SelectedImageWire {
                            data: vec![1, 2, 3],
                            uuid: String::new(),
                            path: "/workspace/shot.png".into(),
                            mime_type: "image/png".into(),
                        }],
                    }),
                    mode: AGENT_MODE_AGENT,
                }),
                send_to_interaction_listener: Some(true),
            }),
        }),
        name: None,
        use_private_worker: None,
        requested_models: Vec::new(),
        labels: Vec::new(),
        team_id: None,
    };
    let bytes = request.encode_to_vec();
    let decoded = StartBackgroundComposerRequestWire::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded.bc_id, "bc-1");
    assert_eq!(decoded.source, Some(33));
    assert_eq!(decoded.starting_message_type, Some(1));
    let action = decoded.conversation_action.unwrap().user_message_action.unwrap();
    assert_eq!(action.send_to_interaction_listener, Some(true));
    let message = action.user_message.unwrap();
    assert_eq!(message.mode, 1);
    assert_eq!(message.selected_context.unwrap().selected_images[0].data, vec![1, 2, 3]);
}

#[test]
fn followup_wire_uses_conversation_action_and_requested_model_slots() {
    let request = AddAsyncFollowupBackgroundComposerRequestWire {
        bc_id: "bc-1".into(),
        synchronous: true,
        followup_source: Some(BACKGROUND_COMPOSER_SOURCE_GROK_BOT),
        followup_conversation_action: Some(ConversationActionWire {
            user_message_action: Some(UserMessageActionWire {
                user_message: Some(UserMessageWire {
                    text: "continue".into(),
                    message_id: "m-2".into(),
                    selected_context: None,
                    mode: AGENT_MODE_AGENT,
                }),
                send_to_interaction_listener: Some(true),
            }),
        }),
        requested_model: None,
    };
    let bytes = request.encode_to_vec();
    assert!(bytes.contains(&0x52)); // field 10, length-delimited
}

#[test]
fn raw_conversation_messages_are_preserved_as_nested_protobuf_bytes() {
    let wire = vec![0x0a, 0x03, 0x08, 0x01, 0x10, 0x0a, 0x02, 0x18, 0x01];
    let response = GetBackgroundComposerConversationResponseWire::decode(wire.as_slice()).unwrap();
    assert_eq!(response.conversation, vec![vec![0x08, 0x01, 0x10], vec![0x18, 0x01]]);
}
