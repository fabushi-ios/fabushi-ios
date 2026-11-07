use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use super::sand_prompt_markers::SAND_HIDDEN_PROMPT_MARKER;

pub const SAND_AGENT_PROFILE_UPDATE_MARKER: &str = "<<SAND_AGENT_PROFILE_UPDATE:v1:";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfileIdentity {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfilePromptSnapshot {
    pub version: u8,
    pub profile_section: String,
    pub system_identity: AgentProfileIdentity,
    pub announced_identity: AgentProfileIdentity,
    pub compaction_epoch: i64,
}

pub fn normalize_agent_profile_identity(profile: AgentProfileIdentity) -> AgentProfileIdentity {
    AgentProfileIdentity {
        name: profile.name.trim().to_string(),
        description: profile.description.trim().to_string(),
    }
}

pub fn agent_profile_identities_equal(
    left: &AgentProfileIdentity,
    right: &AgentProfileIdentity,
) -> bool {
    left == right
}

pub fn resolve_agent_profile_prompt_snapshot(
    snapshot: Option<&AgentProfilePromptSnapshot>,
    compaction_epoch: i64,
    profile_section: impl Into<String>,
    identity: AgentProfileIdentity,
) -> AgentProfilePromptSnapshot {
    if let Some(snapshot) = snapshot {
        if snapshot.compaction_epoch == compaction_epoch {
            return snapshot.clone();
        }
    }
    AgentProfilePromptSnapshot {
        version: 1,
        profile_section: profile_section.into(),
        system_identity: identity.clone(),
        announced_identity: identity,
        compaction_epoch,
    }
}

pub fn persist_announced_agent_profile_snapshot(
    current: Option<&AgentProfilePromptSnapshot>,
    turn_snapshot: &AgentProfilePromptSnapshot,
    identity: &AgentProfileIdentity,
) -> Option<AgentProfilePromptSnapshot> {
    let current = current?;
    if current.compaction_epoch != turn_snapshot.compaction_epoch
        || current.profile_section != turn_snapshot.profile_section
        || !agent_profile_identities_equal(&current.system_identity, &turn_snapshot.system_identity)
        || agent_profile_identities_equal(&current.announced_identity, identity)
    {
        return None;
    }
    let mut next = current.clone();
    next.announced_identity = normalize_agent_profile_identity(identity.clone());
    Some(next)
}

pub fn render_agent_profile_update(identity: &AgentProfileIdentity) -> String {
    let encoded = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(identity).expect("agent profile identity serialization is infallible"),
    );
    [
        format!(
            "{SAND_HIDDEN_PROMPT_MARKER}{SAND_AGENT_PROFILE_UPDATE_MARKER}{encoded}>>"
        ),
        "<agent_profile_update>".to_string(),
        "Your agent profile changed. This full update is authoritative and supersedes the Agent profile section in the system prompt and every earlier profile update in this conversation.".to_string(),
        format!(
            "Current name: {}",
            if identity.name.is_empty() {
                "(no name)"
            } else {
                identity.name.as_str()
            }
        ),
        format!(
            "Current description: {}",
            if identity.description.is_empty() {
                "(no description)"
            } else {
                identity.description.as_str()
            }
        ),
        "Use this identity until a future conversation summary folds it into the Agent profile section.".to_string(),
        "</agent_profile_update>".to_string(),
    ]
    .join("\n")
}

pub fn parse_latest_agent_profile_update(text: &str) -> Option<AgentProfileIdentity> {
    let mut latest = None;
    let mut from = 0usize;
    loop {
        let Some(relative_at) = text[from..].find(SAND_AGENT_PROFILE_UPDATE_MARKER) else {
            return latest;
        };
        let start = from + relative_at + SAND_AGENT_PROFILE_UPDATE_MARKER.len();
        let Some(relative_end) = text[start..].find(">>") else {
            return latest;
        };
        let end = start + relative_end;
        if let Ok(decoded) = URL_SAFE_NO_PAD.decode(&text[start..end]) {
            if let Ok(identity) = serde_json::from_slice::<AgentProfileIdentity>(&decoded) {
                latest = Some(normalize_agent_profile_identity(identity));
            }
        }
        from = end + 2;
    }
}
