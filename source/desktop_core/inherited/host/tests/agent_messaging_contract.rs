use mahayana_host_runtime::agents::agent_messaging::{
    AGENT_MESSAGE_MAX_TEXT_LENGTH, AgentAddress, AgentGroupAddress, AgentMessageImage,
    build_agent_inbound_wake_prompt, build_admin_broadcast_wake_prompt,
    build_mentioned_agents_context, clamp_agent_message, describe_address,
    render_agent_directory_system_prompt,
};

#[test]
fn agent_message_text_and_directory_contract_match_frozen_semantics() {
    assert_eq!(clamp_agent_message("  hello  "), "hello");
    assert_eq!(
        clamp_agent_message(&"x".repeat(AGENT_MESSAGE_MAX_TEXT_LENGTH + 10)).len(),
        AGENT_MESSAGE_MAX_TEXT_LENGTH
    );
    let teammate = AgentAddress {
        id: "agent-b".into(), name: "Research".into(),
        description: Some("  Deep   research  ".into()), is_group: false,
    };
    assert_eq!(describe_address(&teammate), "- Research (id: agent-b) — Deep research");
    let group = AgentGroupAddress {
        address: AgentAddress { id:"group-1".into(), name:"Launch".into(), description:None, is_group:true },
        members: vec![teammate.clone()],
    };
    let prompt=render_agent_directory_system_prompt(std::slice::from_ref(&teammate),&[group],Some("/agents"));
    assert!(prompt.contains("ASYNCHRONOUS"));
    assert!(prompt.contains("Research (id: agent-b)"));
    assert!(prompt.contains("/agents/<agentId>/profile.json"));
}

#[test]
fn wake_and_broadcast_prompts_keep_agent_vs_user_identity_explicit() {
    let from=AgentAddress{id:"agent-a".into(),name:"Planner".into(),description:None,is_group:false};
    let wake=build_agent_inbound_wake_prompt(&from,"please check",&[AgentMessageImage{url:"file:///tmp/a.png".into(),alt:Some(" screenshot ".into())}],true);
    assert!(wake.starts_with("[agent]"));assert!(wake.contains("PRIORITY"));assert!(wake.contains("file:///tmp/a.png — screenshot"));assert!(wake.contains("SendToAgent"));
    let broadcast=build_admin_broadcast_wake_prompt("ship it");assert!(broadcast.starts_with("[broadcast]"));assert!(broadcast.contains("The user says: ship it"));
    assert!(build_mentioned_agents_context(&[]).is_none());
    assert!(build_mentioned_agents_context(&[from]).expect("mentioned context").contains("SendToAgent"));
}
