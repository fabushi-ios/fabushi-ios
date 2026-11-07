use serde::{Deserialize, Serialize};

pub const AGENT_INBOUND_WAKE_CUE: &str = "[agent]";
pub const ADMIN_BROADCAST_WAKE_CUE: &str = "[broadcast]";
pub const SAND_SEND_TO_AGENT_TOOL_NAME: &str = "SendToAgent";
pub const SAND_CREATE_AGENT_TOOL_NAME: &str = "CreateAgent";
pub const SAND_UPDATE_AGENT_TOOL_NAME: &str = "UpdateAgent";
pub const AGENT_MESSAGE_MAX_TEXT_LENGTH: usize = 8_000;
pub const AGENT_DIRECTORY_PROMPT_LIMIT: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageImage {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAddress {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub is_group: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGroupAddress {
    pub address: AgentAddress,
    pub members: Vec<AgentAddress>,
}

fn truncate_utf16(input: &str, limit: usize) -> String {
    let mut used = 0usize;
    input.chars().take_while(|ch| {
        let next = used.saturating_add(ch.len_utf16());
        if next > limit { false } else { used = next; true }
    }).collect()
}

fn clamp_line(raw: &str, max_length: usize) -> String {
    truncate_utf16(&raw.split_whitespace().collect::<Vec<_>>().join(" "), max_length)
}

pub fn clamp_agent_message(text: &str) -> String {
    truncate_utf16(text.trim(), AGENT_MESSAGE_MAX_TEXT_LENGTH)
}

pub fn describe_address(address: &AgentAddress) -> String {
    let description = address.description.as_deref().map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!(" — {}", clamp_line(value, 120))).unwrap_or_default();
    let group_tag = if address.is_group { " (group)" } else { "" };
    format!("- {} (id: {}){}{}", address.name, address.id, group_tag, description)
}

pub fn render_agent_directory_system_prompt(
    others: &[AgentAddress],
    groups: &[AgentGroupAddress],
    agents_root_dir: Option<&str>,
) -> String {
    let mut lines = vec![
        "Your teammates: the other agents this user runs. Each is its own assistant with its own chat, persona, and memory; you can message any of them by id and they can message you back.".to_string(),
        format!("Messaging is ASYNCHRONOUS, like texting a person: call {SAND_SEND_TO_AGENT_TOOL_NAME} with a target id and your message and it is delivered and returns right away. The target can be a single agent OR a group you belong to. You do NOT get a reply back in this turn and you must not wait or poll for one; a reply arrives later as its own message that wakes you on a fresh turn (the cue {AGENT_INBOUND_WAKE_CUE})."),
        "Use this deliberately and sparingly: waking another agent or a group is a real side effect. Never relay the user's private or unfiltered words verbatim; paraphrase the actionable substance when relay is warranted.".to_string(),
    ];
    if let Some(root) = agents_root_dir.map(str::trim).filter(|value| !value.is_empty()) {
        lines.push(format!("Discovering agents is file-based: every agent is a sibling folder under {root}. Read {root}/<agentId>/profile.json for identity and <agentId>/group.json for group membership."));
    }
    if others.is_empty() && groups.is_empty() {
        lines.push(format!("This user has no other agents yet. If a task would be better handled by a dedicated teammate, offer to {SAND_CREATE_AGENT_TOOL_NAME} one."));
        return lines.join("\n");
    }
    if !others.is_empty() {
        lines.push("Teammates you can message right now:".into());
        for address in others.iter().take(AGENT_DIRECTORY_PROMPT_LIMIT) { lines.push(describe_address(address)); }
        if others.len() > AGENT_DIRECTORY_PROMPT_LIMIT { lines.push("…and more (read the agent folders above for the full roster).".into()); }
    }
    if !groups.is_empty() {
        lines.push("Group chats you're in (post to one by its id to reach its members):".into());
        for group in groups.iter().take(AGENT_DIRECTORY_PROMPT_LIMIT) {
            let member_names = group.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", ");
            let with_clause = if member_names.is_empty() { String::new() } else { format!(" — with {member_names}") };
            lines.push(format!("- {} (id: {}){}", group.address.name, group.address.id, with_clause));
        }
    }
    lines.join("\n")
}

pub fn build_agent_inbound_wake_prompt(
    from: &AgentAddress,
    text: &str,
    images: &[AgentMessageImage],
    priority: bool,
) -> String {
    let mut lines = vec![
        format!("{AGENT_INBOUND_WAKE_CUE} A message just arrived from another of your user's agents: {} (id: {}).", from.name, from.id),
        if priority {
            "This is a PRIORITY instruction from another assistant — not the user typing here. It interrupted your previous non-user work. Drop conflicting in-flight work and follow it now. Your user can already see it in this chat.".to_string()
        } else {
            "This is another assistant reaching out — not the user typing here. It arrived asynchronously, and your user can already see it in this chat.".to_string()
        },
        String::new(),
        format!("{}: {}", from.name, text),
    ];
    if !images.is_empty() {
        lines.push(String::new());
        lines.push(if images.len() == 1 { format!("{} attached an image to this message:", from.name) } else { format!("{} attached {} images to this message:", from.name, images.len()) });
        for image in images {
            let alt = image.alt.as_deref().map(str::trim).filter(|v| !v.is_empty())
                .map(|v| format!(" — {}", clamp_line(v, 200))).unwrap_or_default();
            lines.push(format!("- {}{}", image.url, alt));
        }
        lines.push("Local image files are shown to you alongside this message. To pass one on, re-attach its url in your own SendMessage or SendToAgent.".into());
    }
    lines.push(String::new());
    lines.push(format!("If it needs a reply or an action, handle it: reply to {} with {SAND_SEND_TO_AGENT_TOOL_NAME} (their id: {}), which reaches them on a later turn. Use SendMessage to tell your user only when you have a real result to share. If it is just an FYI, it is fine to stay silent.", from.name, from.id));
    lines.join("\n")
}

pub fn build_admin_broadcast_wake_prompt(message: &str) -> String {
    [
        format!("{ADMIN_BROADCAST_WAKE_CUE} A direct message from your user — the owner who runs you — broadcast to their agents."),
        "This is the user speaking to you, not another agent and not a scheduled routine. Treat it as a directive or announcement from the person you work for.".into(),
        String::new(),
        format!("The user says: {message}"),
        String::new(),
        "Act on it as makes sense for you, then reply to the user with SendMessage so they know you received it and what you did. Keep your reply concise.".into(),
    ].join("\n")
}

pub fn build_mentioned_agents_context(mentioned: &[AgentAddress]) -> Option<String> {
    if mentioned.is_empty() { return None; }
    let mut lines = vec![format!("[Agents mentioned in this message — you can reach any of them with {SAND_SEND_TO_AGENT_TOOL_NAME} using their id:")];
    lines.extend(mentioned.iter().map(describe_address));
    lines.push("]".into());
    Some(lines.join("\n"))
}
