use serde_json::Value;

use crate::host_request_context::HostRequestContext;
use crate::agents::agent_messaging::{
    AgentAddress, AgentGroupAddress,
    render_agent_directory_system_prompt as render_agent_directory_prompt,
};
use crate::automations::automation::{AutomationRecord, render_automations_system_prompt};
use crate::extensions::session::channel_store::ChannelConnection;
use crate::extensions::inference::provider_session::ProviderMessage;
use crate::extensions::memory::memory_service::{
    MemoryRecall, ProjectMemoryPromptRecall, UserMemoryRecall,
};
use crate::sand_multitask::SAND_MULTITASK_PROMPT_SECTION;
use crate::sand_activity::SAND_EXTERNAL_SHELL_TOOL_NAME;

use super::system_prompt::{
    SAND_CLOUD_AGENTS_DISABLED_PROMPT_SECTION, build_sand_base_system_prompt,
};
use super::sand_memory::{
    FrozenMemoryPrompt, FrozenMemorySnapshot, MEMORY_PROJECT_SYSTEM_PROMPT_HEADER,
    MEMORY_SYSTEM_PROMPT_HEADER, MEMORY_USER_SYSTEM_PROMPT_HEADER,
    render_memory_system_prompt, render_project_memory_system_prompt,
    render_user_memory_system_prompt, resolve_frozen_memory_prompt,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfileForPrompt {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub settings_file_path: String,
}

pub fn render_agent_profile_section(
    profile: &AgentProfileForPrompt,
    shared_room: bool,
) -> Option<String> {
    let configured_title = profile.name.trim();
    // Grok and Grok Bot were legacy product defaults, not user-authored agent names.
    // Keep those defaults aligned with the Fabushi product identity in assistant-facing prompts.
    let title = if configured_title.eq_ignore_ascii_case("grok")
        || configured_title.eq_ignore_ascii_case("grok bot")
    {
        "Fabushi"
    } else {
        configured_title
    };
    let description = profile.description.trim();
    let mut lines = Vec::new();
    if !title.is_empty() {
        lines.push(format!("Title: {title}"));
        if !shared_room {
            lines.push(format!(
                "Your agent name is \"{title}\". If the user asks for your name, answer with \"{title}\"."
            ));
        }
    }
    if !description.is_empty() {
        lines.push(format!("Description: {description}"));
    }
    if !shared_room && !profile.file_path.is_empty() {
        lines.push(format!(
            "Your profile is a JSON config file at {} with \"name\", \"description\", and \"title\" fields, which you can read with your shell tools. To rename yourself or rewrite your own description, use the update_state tool (target \"profile\", action \"set\"); it preserves every field you do not pass. Name and description edits are announced in a profile-update message for the current context and folded into this Agent profile section after the next conversation summary.",
            profile.file_path
        ));
        lines.push(format!(
            "Your profile picture is NOT part of that config — it is a conventional image file named \"avatar.png\" (or avatar.jpg/.jpeg/.webp/.gif/.svg) in the same directory, which you can read with your shell tools. To set it, put the image somewhere first (Shell under /workspace is fine — no CopyFromBox needed — or {SAND_EXTERNAL_SHELL_TOOL_NAME} on the user's computer), then call update_state (target \"avatar\", action \"set\", path=...); to go back to the default picture, update_state target \"avatar\", action \"clear\". Never change your picture unless the user asks."
        ));
    }
    if !shared_room && !profile.settings_file_path.is_empty() {
        lines.push(format!(
            "Your per-agent settings live in a separate JSON config file at {}, readable the same way and changed with update_state (target \"settings\", action \"set\"). \"hidden_from_sidebar\" (true/false) removes your own row from the user's sidebar: you stay fully functional — you keep your conversation, keep receiving messages, keep running your routines, and still accrue unread — and the user can still reach you through the Hidden chats manager and Cmd-K; the default is visible. Pass only the fields you mean to change; the rest are preserved.",
            profile.settings_file_path
        ));
    }
    (!lines.is_empty()).then(|| {
        let mut rendered = vec!["Agent profile:".to_string()];
        rendered.extend(lines);
        rendered.join("\n")
    })
}

pub fn append_agent_profile_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    profile_section: &str,
) {
    if profile_section.trim().is_empty() {
        return;
    }
    if messages.iter().any(|message| {
        message.role == "system"
            && (message.content.starts_with("Agent profile:\n")
                || message.content.contains("\n\nAgent profile:\n"))
    }) {
        return;
    }
    if let Some(system) = messages.iter_mut().find(|message| message.role == "system") {
        if !system.content.trim().is_empty() {
            system.content.push_str("\n\n");
        }
        system.content.push_str(profile_section);
    } else {
        messages.insert(0, ProviderMessage {
            role: "system".into(),
            content: profile_section.to_string(),
        });
    }
}

pub fn render_request_context_system_prompt(
    context: &HostRequestContext,
    rules: Option<&[Value]>,
) -> String {
    render_request_context_system_prompt_with_capabilities(context, rules, false, false)
}

pub fn render_request_context_system_prompt_with_capabilities(
    context: &HostRequestContext,
    rules: Option<&[Value]>,
    cloud_agents_enabled: bool,
    multitask_enabled: bool,
) -> String {
    let mut sections = vec![build_sand_base_system_prompt(cloud_agents_enabled).to_string()];
    if !cloud_agents_enabled {
        sections.push(SAND_CLOUD_AGENTS_DISABLED_PROMPT_SECTION.to_string());
    }
    if multitask_enabled {
        sections.push(SAND_MULTITASK_PROMPT_SECTION.to_string());
    }

    if let Some(time_zone) = context
        .time_zone
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sections.push(format!(
            "## Time\nYour box and tools run on a UTC clock, but the user lives in {time_zone}. Convert UTC timestamps you report to the user's time zone and label the converted time clearly."
        ));
    }

    if let Some(full_name) = context
        .user_full_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sections.push(format!(
            "Your user is {full_name}; when acting through their accounts and apps, such as Slack, speak as them and never refer to them in the third person."
        ));
    }

    if let Some(rendered_rules) = render_team_rules(rules) {
        sections.push(rendered_rules);
    }

    sections.join("\n\n")
}

fn render_team_rules(rules: Option<&[Value]>) -> Option<String> {
    let rules = rules?;
    let mut rendered = Vec::new();
    for rule in rules {
        let Some(content) = rule
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let name = rule
            .get("fullPath")
            .or_else(|| rule.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("team rule");
        let required = rule
            .get("isRequired")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        rendered.push(format!(
            "### {name}{}\n{content}",
            if required { " (required)" } else { "" }
        ));
    }
    (!rendered.is_empty()).then(|| {
        format!(
            "## Team rules\nApply the following organization rules to this turn.\n\n{}",
            rendered.join("\n\n")
        )
    })
}


pub fn append_automations_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    automations: &[AutomationRecord],
    location: Option<&str>,
    time_zone: Option<&str>,
) {
    let prompt = render_automations_system_prompt(automations, location, time_zone);
    if prompt.is_empty() {
        return;
    }
    if messages.iter().any(|message| {
        message.role == "system"
            && (message.content.starts_with("## Routines\n")
                || message.content.contains("\n\n## Routines\n"))
    }) {
        return;
    }
    if let Some(system) = messages.iter_mut().find(|message| message.role == "system") {
        if !system.content.trim().is_empty() {
            system.content.push_str("\n\n");
        }
        system.content.push_str(&prompt);
    } else {
        messages.insert(
            0,
            ProviderMessage {
                role: "system".into(),
                content: prompt,
            },
        );
    }
}

pub fn render_workflows_system_prompt(location: Option<&str>) -> String {
    let Some(location) = location.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };
    format!(
        "Workflows are a GLOBAL, shared library across all of the user's assistants. User-created skills live as files at {location}: one subfolder per workflow, each holding a SKILL.md. Prefer the update_state tool (target \"workflow\") to save, rewrite, and delete them. Reference workflows as [name](sand-workflow:<id>)."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSkillPromptItem {
    pub full_path: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSkillPromptBudgetResult {
    pub prompt: String,
    pub retained_count: usize,
    pub omitted_count: usize,
    pub strategy: &'static str,
}

const INITIAL_SKILL_CATALOG_CONTEXT_PERCENT: usize = 2;
const MIN_TRUNCATED_SKILL_DESCRIPTION_LENGTH: usize = 24;
const MAX_TRUNCATED_SKILL_DESCRIPTION_LENGTH: usize = 480;
const SHORT_SKILL_DESCRIPTION_PATH_ONLY_THRESHOLD: usize = 80;
const MAX_OMITTED_SKILL_DIRECTORY_COUNT: usize = 5;

fn estimate_prompt_tokens(value: &str) -> usize {
    // Frozen Grok 0.18: Math.round(value.length / 4).
    (value.chars().count() + 2) / 4
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn skill_name(full_path: &str) -> String {
    let normalized = full_path.replace('\\', "/");
    let parts = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.last().copied() == Some("SKILL.md") && parts.len() >= 2 {
        parts[parts.len() - 2].to_string()
    } else {
        parts.last().copied().unwrap_or("").to_string()
    }
}

fn is_protected_skill(item: &AgentSkillPromptItem) -> bool {
    matches!(skill_name(&item.full_path).as_str(), "canvas" | "env-setup")
}

fn skill_directory_hint(full_path: &str) -> String {
    let normalized = full_path.replace('\\', "/");
    for marker in [
        "/.cursor/skills/",
        "/.cursor/skills-cursor/",
        "/.agents/skills/",
        "/.claude/skills/",
        "/.codex/skills/",
        "/.claude/plugins/",
    ] {
        if let Some(index) = normalized.find(marker) {
            return normalized[..index + marker.len() - 1].to_string();
        }
    }
    if let Some(index) = normalized.find("/.cursor/plugins/cache/") {
        if let Some(relative) = normalized[index..].find("/skills/") {
            let skills_index = index + relative;
            return normalized[..skills_index + "/skills".len()].to_string();
        }
    }
    normalized
        .rsplit_once('/')
        .map(|(directory, _)| directory.to_string())
        .unwrap_or(normalized)
}

fn render_agent_skill_section(items: &[AgentSkillPromptItem], omitted: &[AgentSkillPromptItem]) -> String {
    let mut out = String::from(
        "<agent_skills>\nWhen the user names a skill, use it faithfully as part of the current task. Read the skill file using the read tool before following its instructions. The user's instructions take precedence over skill guidance.\n<available_skills>\n",
    );
    for item in items {
        out.push_str("<agent_skill fullPath=\"");
        out.push_str(&xml_escape(&item.full_path));
        out.push_str("\">");
        if let Some(description) = item.description.as_deref().filter(|value| !value.is_empty()) {
            out.push_str(&xml_escape(description));
        }
        out.push_str("</agent_skill>\n");
    }
    if !omitted.is_empty() {
        let mut directories = Vec::<String>::new();
        for item in omitted {
            let directory = skill_directory_hint(&item.full_path);
            if !directories.contains(&directory) {
                directories.push(directory);
                if directories.len() == MAX_OMITTED_SKILL_DIRECTORY_COUNT {
                    break;
                }
            }
        }
        out.push_str(&format!(
            "Additional skills omitted from this initial list ({}). Directories containing omitted skills: {}.\n",
            omitted.len(),
            directories.join(", ")
        ));
    }
    out.push_str("</available_skills>\n</agent_skills>");
    out
}

fn truncate_skill_description(description: Option<&str>, max_length: usize) -> Option<String> {
    let description = description?;
    if description.chars().count() <= max_length {
        return Some(description.to_string());
    }
    let content_length = max_length.saturating_sub(3);
    Some(format!(
        "{}...",
        description.chars().take(content_length).collect::<String>().trim_end()
    ))
}

pub fn render_budgeted_agent_skills_prompt(
    skills: &[AgentSkillPromptItem],
    agent_token_limit: usize,
) -> AgentSkillPromptBudgetResult {
    let uncapped = render_agent_skill_section(skills, &[]);
    let budget_tokens = agent_token_limit
        .saturating_mul(INITIAL_SKILL_CATALOG_CONTEXT_PERCENT)
        / 100;
    if estimate_prompt_tokens(&uncapped) <= budget_tokens {
        return AgentSkillPromptBudgetResult {
            prompt: uncapped,
            retained_count: skills.len(),
            omitted_count: 0,
            strategy: "under_budget",
        };
    }

    let max_description_length = skills
        .iter()
        .filter(|item| !is_protected_skill(item))
        .filter_map(|item| item.description.as_deref())
        .map(|description| description.chars().count())
        .max()
        .unwrap_or(0);
    if max_description_length > SHORT_SKILL_DESCRIPTION_PATH_ONLY_THRESHOLD {
        let mut lower = MIN_TRUNCATED_SKILL_DESCRIPTION_LENGTH;
        let mut upper = max_description_length
            .saturating_sub(1)
            .min(MAX_TRUNCATED_SKILL_DESCRIPTION_LENGTH);
        let mut best = None::<(String, Vec<AgentSkillPromptItem>)>;
        while lower <= upper {
            let midpoint = lower + (upper - lower) / 2;
            let candidate = skills
                .iter()
                .map(|item| AgentSkillPromptItem {
                    full_path: item.full_path.clone(),
                    description: if is_protected_skill(item) {
                        item.description.clone()
                    } else {
                        truncate_skill_description(item.description.as_deref(), midpoint)
                    },
                })
                .collect::<Vec<_>>();
            let prompt = render_agent_skill_section(&candidate, &[]);
            if estimate_prompt_tokens(&prompt) <= budget_tokens {
                best = Some((prompt, candidate));
                lower = midpoint.saturating_add(1);
            } else if midpoint == 0 {
                break;
            } else {
                upper = midpoint - 1;
            }
        }
        if let Some((prompt, candidate)) = best {
            return AgentSkillPromptBudgetResult {
                prompt,
                retained_count: candidate.len(),
                omitted_count: 0,
                strategy: "shortened_descriptions",
            };
        }
    }

    let path_only = skills
        .iter()
        .map(|item| AgentSkillPromptItem {
            full_path: item.full_path.clone(),
            description: is_protected_skill(item).then(|| item.description.clone()).flatten(),
        })
        .collect::<Vec<_>>();
    let path_only_prompt = render_agent_skill_section(&path_only, &[]);
    if estimate_prompt_tokens(&path_only_prompt) <= budget_tokens {
        return AgentSkillPromptBudgetResult {
            prompt: path_only_prompt,
            retained_count: path_only.len(),
            omitted_count: 0,
            strategy: "dropped_descriptions",
        };
    }

    let droppable = skills
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (!is_protected_skill(item)).then_some(index))
        .collect::<Vec<_>>();
    for retained_droppable_count in (0..=droppable.len()).rev() {
        let retained_indices = droppable[..retained_droppable_count]
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let mut retained = Vec::new();
        let mut omitted = Vec::new();
        for (index, original) in skills.iter().enumerate() {
            if is_protected_skill(original) || retained_indices.contains(&index) {
                retained.push(path_only[index].clone());
            } else {
                omitted.push(original.clone());
            }
        }
        let prompt = render_agent_skill_section(&retained, &omitted);
        if estimate_prompt_tokens(&prompt) <= budget_tokens || retained_droppable_count == 0 {
            return AgentSkillPromptBudgetResult {
                prompt,
                retained_count: retained.len(),
                omitted_count: omitted.len(),
                strategy: "omitted_skills",
            };
        }
    }
    unreachable!("skill omission loop must return")
}

pub fn append_budgeted_workflows_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    location: Option<&str>,
    skills: &[AgentSkillPromptItem],
    agent_token_limit: usize,
) {
    append_workflows_system_prompt(messages, location);
    if skills.is_empty() {
        return;
    }
    let budgeted = render_budgeted_agent_skills_prompt(skills, agent_token_limit);
    append_unique_system_section(messages, &budgeted.prompt, "<agent_skills>");
}

pub fn append_workflows_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    location: Option<&str>,
) {
    let prompt = render_workflows_system_prompt(location);
    if prompt.is_empty() {
        return;
    }
    if messages.iter().any(|message| {
        message.role == "system"
            && message.content.contains(
                "Workflows are a GLOBAL, shared library across all of the user's assistants.",
            )
    }) {
        return;
    }
    if let Some(system) = messages.iter_mut().find(|message| message.role == "system") {
        if !system.content.trim().is_empty() {
            system.content.push_str("\n\n");
        }
        system.content.push_str(&prompt);
    } else {
        messages.insert(
            0,
            ProviderMessage {
                role: "system".into(),
                content: prompt,
            },
        );
    }
}




fn append_unique_system_section(
    messages: &mut Vec<ProviderMessage>,
    prompt: &str,
    marker: &str,
) {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return;
    }
    if messages
        .iter()
        .any(|message| message.role == "system" && message.content.contains(marker))
    {
        return;
    }
    if let Some(system) = messages.iter_mut().find(|message| message.role == "system") {
        if !system.content.trim().is_empty() {
            system.content.push_str("\n\n");
        }
        system.content.push_str(prompt);
    } else {
        messages.insert(
            0,
            ProviderMessage {
                role: "system".into(),
                content: prompt.to_string(),
            },
        );
    }
}

fn channel_display_name(platform: &str) -> &str {
    match platform {
        "discord" => "Discord",
        "slack" => "Slack",
        other => other,
    }
}

pub fn render_channels_system_prompt(
    connections: &[ChannelConnection],
    location: Option<&str>,
) -> String {
    let Some(location) = location.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };

    // Frozen Grok 0.18 exposes Discord and Slack manifests, both coming-soon.
    // A real persisted+secret-backed connection still makes the section visible.
    if connections.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "Channels: outside messaging surfaces you can talk on, beyond this Fabushi chat.".to_string(),
        format!(
            "Each connected channel lives in a subfolder at {location} holding a connection.json. That file holds only a label, never a credential; the secret is kept in a separate store you cannot read. To disconnect one, prefer the update_state tool (target \"channel\", action \"disconnect\", the platform); a background connector notices and closes the live connection within a few seconds."
        ),
        "Never ask the user to paste a token, API key, or password into the chat, and never write one into a file: that would persist it in the transcript or somewhere you can read it back. To collect any credential, send a SendMessage of type secret-request (connector + field + a clear label). The user types it into a masked field and the value goes straight to the secret store; you only learn that it was provided, never the value. You do not need the credential to check status; never cat the connection file expecting one.".to_string(),
        "Every conversation on a channel has an address shaped like platform:chat (e.g. slack:C12345). An address names one chat; that is all routing needs.".to_string(),
        "INBOUND: when someone messages you on a connected channel, you are woken with a hidden message that opens with the cue [inbound] and names the source address and sender. That is a real person reaching out on that platform, not the user typing in this app. Reply to them on that same channel by calling SendMessage with a channel target set to their address; if you instead omit the channel, your message goes to this in-app Fabushi chat (the user at their desk), not to them.".to_string(),
        "REACTIONS: the same [inbound] cue also wakes you when someone reacts to one of your messages (e.g. ❤️). A reaction is a lightweight acknowledgement, not a question: you usually do not need to reply, only act on it if it is useful.".to_string(),
        "OUTBOUND: SendMessage takes an optional channel target. Set it to an address (e.g. slack:C12345) to deliver there; leave it off and the message lands in this in-app chat exactly as before. You choose where each message goes, so be deliberate: by default answer an inbound message on the channel it came from.".to_string(),
        "Pace a channel reply exactly like the in-app chat: open with a quick one-line acknowledgement, then send each progress beat and the final result as its own SendMessage as it happens. Each SendMessage is delivered to the platform immediately as a separate message, so the person sees you respond in real time; never hold it all back for one long message at the end, the worst way to reply on a channel. Keep every one of those messages extra concise: a channel is a messaging app, so write the short, to-the-point messages a person texts, terser than your in-app replies. Lead with the answer, prefer one or two short sentences, and skip long multi-paragraph messages, exhaustive detail, and unprompted caveats; expand only if they ask.".to_string(),
        "A channel only carries text and attachments, never the in-app widget or cursor-agent cards (those render only in this app), so degrade them to text when the conversation is on a channel: ask a multiple-choice question as plain text with the options as a numbered list and tell them to reply with their choice; reference a Cursor cloud agent as a plain https://cursor.com/agents/<bcId> link instead of a card; and for an attachment pass either a local file:// path or an https URL: the file is uploaded to the platform so they receive the real image or file, never a path.".to_string(),
        "Platforms you can connect:".to_string(),
        "Coming soon (not connectable yet): Discord, Slack.".to_string(),
        "Currently connected:".to_string(),
    ];
    for connection in connections {
        lines.push(format!(
            "- {} \"{}\" [{}]. Address people on it as {}:<chat id>",
            channel_display_name(&connection.platform),
            connection.label,
            connection.status,
            connection.platform
        ));
    }
    lines.join("\n")
}

pub fn append_channels_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    connections: &[ChannelConnection],
    location: Option<&str>,
) {
    let prompt = render_channels_system_prompt(connections, location);
    append_unique_system_section(messages, &prompt, "Channels: outside messaging surfaces");
}

pub fn append_agent_directory_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    others: &[AgentAddress],
    groups: &[AgentGroupAddress],
    agents_root_dir: Option<&str>,
    enabled: bool,
) {
    if !enabled {
        return;
    }
    let prompt = render_agent_directory_prompt(others, groups, agents_root_dir);
    append_unique_system_section(messages, &prompt, "Your teammates: the other agents");
}

fn default_mcp_custom_instruction(server_name: &str) -> &'static str {
    if server_name.trim().eq_ignore_ascii_case("hex") {
        "When using Hex, get the underlying numbers as data: download/export the results as CSV or use the data the connector returns, and analyze those raw values directly. Don't read rendered charts or graphs from screenshots (computer-use chart reading is unreliable) — work from the actual data."
    } else {
        ""
    }
}

pub fn render_mcp_custom_instructions_system_prompt(installed: &[Value]) -> String {
    let mut entries = installed
        .iter()
        .filter(|server| {
            server
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status.eq_ignore_ascii_case("connected"))
        })
        .filter_map(|server| {
            let name = server.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            // Frozen resolveMcpCustomInstruction falls back only when no stored
            // instruction exists; an explicitly stored empty string suppresses the default.
            let instructions = match server.get("customInstructions") {
                Some(value) => value.as_str().unwrap_or_default().trim(),
                None => default_mcp_custom_instruction(name),
            };
            (!instructions.is_empty()).then(|| (name.to_string(), instructions.to_string()))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries.dedup_by(|left, right| left.0 == right.0);
    if entries.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "## Connector custom instructions".to_string(),
        "Custom instructions are configured for some connected tools (MCP connectors). Always follow the matching instruction whenever you use that connector's tools, even before your first call to it:".to_string(),
    ];
    lines.extend(
        entries
            .into_iter()
            .map(|(name, instructions)| format!("- {name}: {instructions}")),
    );
    lines.join("\n")
}

pub fn render_mcp_discovery_status_system_prompt(discovery_unavailable: bool) -> String {
    if !discovery_unavailable {
        return String::new();
    }
    "<mcp_status>\nYour MCP tools are temporarily unavailable: discovering the user's MCP connectors from the backend failed this turn. This does NOT mean the user has no MCP connectors. Do not claim they have none or that a connector is missing; if the user needs an MCP tool, tell them MCP is temporarily unavailable and to retry shortly.\n</mcp_status>".to_string()
}

pub fn append_mcp_system_prompt_sections(
    messages: &mut Vec<ProviderMessage>,
    installed: &[Value],
    discovery_unavailable: bool,
    enabled: bool,
) {
    if !enabled {
        return;
    }
    let custom = render_mcp_custom_instructions_system_prompt(installed);
    append_unique_system_section(messages, &custom, "## Connector custom instructions");
    let discovery = render_mcp_discovery_status_system_prompt(discovery_unavailable);
    append_unique_system_section(messages, &discovery, "<mcp_status>");
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerPromptRole {
    Main,
    ComputerUseSubagent,
    BrowserUseSubagent,
    OtherSubagent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBoxPromptState {
    pub role: RunnerPromptRole,
    pub available: bool,
    pub runtime_state: String,
    pub desktop_capable: bool,
    pub desktop_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerPromptState {
    pub role: RunnerPromptRole,
    pub box_available: bool,
    pub desktop_capable: bool,
    pub desktop_ready: bool,
    pub control_lease_active: bool,
    pub human_takeover_pending: bool,
    pub browser_use_offered: bool,
    pub window_index: Option<u32>,
}

pub fn render_remote_box_system_prompt(state: &RemoteBoxPromptState) -> String {
    if !state.available {
        return format!(
            "## Your box\nThe shipping box runtime is unavailable this turn (runtime state: {}). Do not claim that Read, Shell, browser, or box desktop actions succeeded. If a box tool fails, report the unavailable state and retry only after the runtime becomes available.",
            state.runtime_state.trim()
        );
    }

    match state.role {
        RunnerPromptRole::ComputerUseSubagent => {
            if !state.desktop_capable {
                return "## Your box\nThe box filesystem and Shell are available, but this shipping runtime has no desktop monitor capability. Computer input is unavailable; do not invent a screen or browser window.".into();
            }
            let readiness = if state.desktop_ready {
                "This agent's desktop is assigned and ready."
            } else {
                "This agent's desktop is supported but is not assigned yet; the first real Computer/browser operation may initialize it."
            };
            format!(
                "## Your box\nYou drive this agent's own desktop on the box: a persistent Linux machine shared by all of this user's agents, while each agent gets its own desktop. Read, Shell, and the desktop share the box filesystem; files, installed tools, and browser logins persist across turns. The user's computer is a separate machine. {readiness}"
            )
        }
        RunnerPromptRole::BrowserUseSubagent => {
            if !state.desktop_capable {
                return "## Your box\nThe box filesystem and Shell are available, but this shipping runtime has no desktop/browser monitor capability. Browser UI work is unavailable this turn.".into();
            }
            let readiness = if state.desktop_ready { "The assigned desktop/browser is ready." } else { "The desktop/browser is supported and will be initialized by the real box runtime when needed." };
            format!(
                "## Your box\nYou drive this agent's box browser on the persistent shared Linux box. Read, Shell, and the browser share one filesystem; files and browser logins persist across turns. Each agent has its own desktop/browser window on the shared machine. {readiness}"
            )
        }
        RunnerPromptRole::Main | RunnerPromptRole::OtherSubagent => {
            if !state.desktop_capable {
                return "## Your box\nYou have the persistent shared Linux box with Read and Shell. This shipping runtime does not currently provide a desktop monitor, so do not claim that Computer, Screenshot, or browser-desktop interaction is available. The box and the user's computer are separate filesystems.".into();
            }
            let readiness = if state.desktop_ready {
                "Your desktop assignment is currently ready."
            } else {
                "A desktop is supported, but no live desktop assignment exists yet; the shipping box runtime will establish one when a desktop-capable operation actually needs it."
            };
            format!(
                "## Your box\nAlongside the user's computer you have one persistent Linux box shared by all of this user's agents. Read and Shell use the shared filesystem; each agent gets its own desktop/window on that shared machine, so agents share the computer but not each other's screen. Files, installed tools, and browser logins persist across turns. The box and the user's computer are separate machines and filesystems. {readiness}"
            )
        }
    }
}

pub fn append_remote_box_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    state: &RemoteBoxPromptState,
) {
    let prompt = render_remote_box_system_prompt(state);
    append_unique_system_section(messages, &prompt, "## Your box");
}

pub fn render_computer_system_prompt(state: &ComputerPromptState) -> String {
    if state.role == RunnerPromptRole::OtherSubagent {
        return String::new();
    }
    if !state.box_available || !state.desktop_capable {
        return "## Computer\nComputer control is unavailable in the current shipping box runtime. Do not claim to see or control a desktop, and do not substitute Shell-driven GUI automation for unavailable Computer capability.".into();
    }
    if state.role == RunnerPromptRole::ComputerUseSubagent && !state.control_lease_active {
        return "## Computer\nInteractive Computer control is unavailable because this Runner does not hold the live ComputerControlLease for the box desktop. Do not send desktop input or compete with the current controller; report the unavailable control state and wait for a new computer-use turn.".into();
    }
    if state.human_takeover_pending {
        return "## Computer\nThe user currently has control of this box desktop through the shipping handoff owner. You may use the read-only Screenshot surface if it is available, but do not send clicks, typing, keys, scrolling, drag, move, or wait actions until the user hands control back.".into();
    }

    match state.role {
        RunnerPromptRole::ComputerUseSubagent => {
            let display = state.window_index
                .map(|index| format!(" Your shipping desktop window index is {index}."))
                .unwrap_or_default();
            format!(
                "## Computer\nYou drive this box desktop with the Computer tool: screenshot, click, move, drag, type, key, scroll, and wait. Work in a see-act-verify loop and rely on the fresh screenshot returned by Computer instead of remembered coordinates. Stop and report when a human-only login, 2FA, captcha, payment, or other handoff is required.{display}"
            )
        }
        RunnerPromptRole::BrowserUseSubagent => {
            "## Browser\nYou drive this box browser with the browser tool surface. Computer/Screenshot are not exposed to this Runner role; use browser state and element references rather than pixel coordinates.".into()
        }
        RunnerPromptRole::Main => {
            let delegate = if state.browser_use_offered {
                "Delegate browser interaction to browserUse first and desktop-only interaction to computerUse."
            } else {
                "Delegate desktop interaction to computerUse."
            };
            let readiness = if state.desktop_ready {
                "The desktop assignment is ready."
            } else {
                "The desktop is supported but will be initialized by the shipping runtime when a delegated desktop operation needs it."
            };
            format!(
                "## The box desktop\nYou have the read-only Screenshot tool for this agent's own box desktop, but you do not have interactive Computer input in the main Runner. {delegate} While a computerUse subagent is operating the shared screen, limit yourself to Screenshot and do not compete for input. {readiness}"
            )
        }
        RunnerPromptRole::OtherSubagent => String::new(),
    }
}

pub fn append_computer_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    state: &ComputerPromptState,
) {
    let prompt = render_computer_system_prompt(state);
    if prompt.starts_with("## The box desktop") {
        append_unique_system_section(messages, &prompt, "## The box desktop");
    } else {
        append_unique_system_section(messages, &prompt, "## Computer");
        if prompt.starts_with("## Browser") {
            append_unique_system_section(messages, &prompt, "## Browser");
        }
    }
}


pub fn resolve_combined_memory_system_prompt(
    user_recall: &UserMemoryRecall,
    user_memory_dir: Option<&str>,
    user_own_shard_dir: Option<&str>,
    project_recall: &ProjectMemoryPromptRecall,
    projects_root_dir: Option<&str>,
    agent_recall: &MemoryRecall,
    agent_memory_dir: Option<&str>,
    snapshot: Option<&FrozenMemorySnapshot>,
    compaction_epoch: u64,
    freeze_enabled: bool,
) -> FrozenMemoryPrompt {
    let render_live = || {
        let mut parts = Vec::new();
        let mut has_facts = !user_recall.profile.is_empty() || !user_recall.recent.is_empty();

        let user = render_user_memory_system_prompt(
            user_recall,
            user_memory_dir,
            user_own_shard_dir,
        );
        if !user.is_empty() {
            parts.push(user);
        }

        let project = render_project_memory_system_prompt(project_recall, projects_root_dir);
        if !project.is_empty() {
            parts.push(project);
        }
        has_facts |= project_recall.injected.iter().any(|block| {
            !block.recall.profile.is_empty() || !block.recall.recent.is_empty()
        });

        let agent = render_memory_system_prompt(agent_recall, agent_memory_dir);
        if !agent.is_empty() {
            parts.push(agent);
        }
        has_facts |= !agent_recall.profile.is_empty() || !agent_recall.recent.is_empty();

        (parts.join("\n\n"), has_facts)
    };

    if !freeze_enabled {
        let (render, _) = render_live();
        return FrozenMemoryPrompt {
            render,
            snapshot_to_persist: None,
        };
    }
    resolve_frozen_memory_prompt(snapshot, compaction_epoch, render_live)
}

pub fn append_combined_memory_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    memory: &str,
) {
    if memory.is_empty() {
        return;
    }
    if messages.iter().any(|message| {
        message.role == "system"
            && (message.content.contains(MEMORY_USER_SYSTEM_PROMPT_HEADER)
                || message.content.contains(MEMORY_PROJECT_SYSTEM_PROMPT_HEADER)
                || message.content.contains(MEMORY_SYSTEM_PROMPT_HEADER))
    }) {
        return;
    }
    if let Some(system) = messages.iter_mut().find(|message| message.role == "system") {
        if !system.content.trim().is_empty() {
            system.content.push_str("\n\n");
        }
        system.content.push_str(memory);
    } else {
        messages.insert(
            0,
            ProviderMessage {
                role: "system".into(),
                content: memory.to_string(),
            },
        );
    }
}

pub fn append_user_memory_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    recall: &UserMemoryRecall,
    user_memory_dir: Option<&str>,
    own_shard_dir: Option<&str>,
) {
    let memory = render_user_memory_system_prompt(recall, user_memory_dir, own_shard_dir);
    if memory.is_empty() { return; }
    if messages.iter().any(|message| {
        message.role == "system" && message.content.contains(MEMORY_USER_SYSTEM_PROMPT_HEADER)
    }) { return; }
    if let Some(system) = messages.iter_mut().find(|message| message.role == "system") {
        if !system.content.trim().is_empty() { system.content.push_str("\n\n"); }
        system.content.push_str(&memory);
    } else {
        messages.insert(0, ProviderMessage { role: "system".into(), content: memory });
    }
}

pub fn append_project_memory_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    recall: &ProjectMemoryPromptRecall,
    projects_root_dir: Option<&str>,
) {
    let memory = render_project_memory_system_prompt(recall, projects_root_dir);
    if memory.is_empty() {
        return;
    }
    if messages.iter().any(|message| {
        message.role == "system" && message.content.contains(MEMORY_PROJECT_SYSTEM_PROMPT_HEADER)
    }) {
        return;
    }
    if let Some(system) = messages.iter_mut().find(|message| message.role == "system") {
        if !system.content.trim().is_empty() {
            system.content.push_str("\n\n");
        }
        system.content.push_str(&memory);
    } else {
        messages.insert(
            0,
            ProviderMessage {
                role: "system".into(),
                content: memory,
            },
        );
    }
}

pub fn append_memory_system_prompt(
    messages: &mut Vec<ProviderMessage>,
    recall: &MemoryRecall,
    location: Option<&str>,
) {
    let memory = render_memory_system_prompt(recall, location);
    if memory.is_empty() {
        return;
    }
    if messages.iter().any(|message| {
        message.role == "system" && message.content.contains(MEMORY_SYSTEM_PROMPT_HEADER)
    }) {
        return;
    }
    if let Some(system) = messages.iter_mut().find(|message| message.role == "system") {
        if !system.content.trim().is_empty() {
            system.content.push_str("\n\n");
        }
        system.content.push_str(&memory);
    } else {
        messages.insert(
            0,
            ProviderMessage {
                role: "system".into(),
                content: memory,
            },
        );
    }
}

#[cfg(test)]
mod frozen_dynamic_section_tests {
    use super::*;
    use serde_json::json;

    fn system_content(messages: &[ProviderMessage]) -> &str {
        messages
            .iter()
            .find(|message| message.role == "system")
            .map(|message| message.content.as_str())
            .expect("system message")
    }

    #[test]
    fn dynamic_sections_preserve_frozen_order_and_dedupe() {
        let mut messages = vec![ProviderMessage {
            role: "system".into(),
            content: "base".into(),
        }];
        append_workflows_system_prompt(&mut messages, Some("/agents/a/workflows"));
        let channels = vec![ChannelConnection {
            platform: "slack".into(),
            label: "Ops".into(),
            status: "configured",
        }];
        append_channels_system_prompt(&mut messages, &channels, Some("/agents/a/channels"));
        let others = vec![AgentAddress {
            id: "peer".into(),
            name: "Peer".into(),
            description: Some("Research".into()),
            is_group: false,
        }];
        let groups = vec![AgentGroupAddress {
            address: AgentAddress {
                id: "group".into(),
                name: "Team".into(),
                description: None,
                is_group: true,
            },
            members: others.clone(),
        }];
        append_agent_directory_system_prompt(&mut messages, &others, &groups, Some("/agents"), true);
        let installed = vec![
            json!({"name":"Acme","status":"connected","customInstructions":"Use raw rows."}),
            json!({"name":"Ignored","status":"disconnected","customInstructions":"Do not render."}),
        ];
        append_mcp_system_prompt_sections(&mut messages, &installed, true, true);

        append_channels_system_prompt(&mut messages, &channels, Some("/agents/a/channels"));
        append_agent_directory_system_prompt(&mut messages, &others, &groups, Some("/agents"), true);
        append_mcp_system_prompt_sections(&mut messages, &installed, true, true);

        let content = system_content(&messages);
        let workflows = content.find("Workflows are a GLOBAL").expect("workflows");
        let channels = content.find("Channels: outside messaging surfaces").expect("channels");
        let directory = content.find("Your teammates: the other agents").expect("agent directory");
        let custom = content.find("## Connector custom instructions").expect("mcp custom");
        let discovery = content.find("<mcp_status>").expect("mcp discovery");
        assert!(workflows < channels && channels < directory && directory < custom && custom < discovery);
        assert_eq!(content.matches("Channels: outside messaging surfaces").count(), 1);
        assert_eq!(content.matches("Your teammates: the other agents").count(), 1);
        assert_eq!(content.matches("## Connector custom instructions").count(), 1);
        assert_eq!(content.matches("<mcp_status>").count(), 1);
        assert!(!content.contains("Do not render."));
    }

    #[test]
    fn feature_and_failure_gates_match_frozen_semantics() {
        let mut messages = vec![ProviderMessage {
            role: "system".into(),
            content: "base".into(),
        }];
        append_agent_directory_system_prompt(
            &mut messages,
            &[AgentAddress {
                id: "peer".into(),
                name: "Peer".into(),
                description: None,
                is_group: false,
            }],
            &[],
            Some("/agents"),
            false,
        );
        assert!(!system_content(&messages).contains("Your teammates: the other agents"));

        append_mcp_system_prompt_sections(&mut messages, &[], true, true);
        assert!(system_content(&messages).contains("<mcp_status>"));
        assert!(system_content(&messages).contains("temporarily unavailable"));
    }
}

