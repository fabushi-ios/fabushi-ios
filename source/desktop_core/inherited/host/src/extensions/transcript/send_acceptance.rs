use serde_json::{Value, json};

use crate::extensions::session::production::ProductionSessionWorkers;
use crate::extensions::session::session_profile_files::AgentProfileUpdate;
use crate::host_event_bus::SandHostEventBus;

use super::roster_emit::ProductionRosterEmit;
use super::send_pipeline::PersistedSendContext;

pub const SAND_DEFAULT_AGENT_NAME: &str = "New Bot";
pub const LEGACY_SAND_DEFAULT_AGENT_NAME: &str = "New Agent";
pub const NEW_CONVERSATION_FALLBACK_NAME: &str = "New conversation";
pub const MAX_SEEDED_AGENT_NAME_UTF16: usize = 72;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SendAcceptancePreparation {
    pub seeded_name: Option<String>,
    pub awaiting_cleared: bool,
    pub introduction_cleared: bool,
}

pub fn is_sand_default_agent_name(name: &str) -> bool {
    matches!(
        name.trim(),
        SAND_DEFAULT_AGENT_NAME | LEGACY_SAND_DEFAULT_AGENT_NAME
    )
}

pub fn build_seeded_agent_name(prompt: &str) -> String {
    let collapsed = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let seed = if collapsed.is_empty() {
        NEW_CONVERSATION_FALLBACK_NAME
    } else {
        collapsed.as_str()
    };
    let utf16 = seed
        .encode_utf16()
        .take(MAX_SEEDED_AGENT_NAME_UTF16)
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&utf16)
}

pub fn prepare_send_acceptance(
    session: &ProductionSessionWorkers,
    agent_id: &str,
    transcript_len_before: usize,
    trimmed_prompt: &str,
) -> Result<SendAcceptancePreparation, String> {
    let current_profile = session.get_agent_profile_text(agent_id)?;
    let effective_name = current_profile
        .as_ref()
        .map(|profile| profile.name.as_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(SAND_DEFAULT_AGENT_NAME);

    let seeded_name = if transcript_len_before == 0 && is_sand_default_agent_name(effective_name) {
        let seeded_name = build_seeded_agent_name(trimmed_prompt);
        let current = current_profile.as_ref();
        session.update_agent_profile(
            agent_id,
            &AgentProfileUpdate {
                name: seeded_name.clone(),
                description: current
                    .map(|profile| profile.description.clone())
                    .unwrap_or_default(),
                title: Some(
                    current
                        .map(|profile| profile.title.clone())
                        .unwrap_or_default(),
                ),
                avatar_shape: Some(
                    current
                        .map(|profile| profile.avatar_shape.clone())
                        .unwrap_or_default(),
                ),
                avatar_color: Some(
                    current
                        .map(|profile| profile.avatar_color.clone())
                        .unwrap_or_default(),
                ),
            },
            None,
        )?;
        Some(seeded_name)
    } else {
        None
    };

    let awaiting_cleared = session.set_agent_awaiting_user_response(agent_id, None)?;
    let introduction_cleared = session.set_agent_introduction_pending(agent_id, false)?;

    Ok(SendAcceptancePreparation {
        seeded_name,
        awaiting_cleared,
        introduction_cleared,
    })
}

pub fn mark_accepted_send_activity(
    session: &ProductionSessionWorkers,
    agent_id: &str,
    at_ms: f64,
) -> Result<bool, String> {
    session.mark_agent_activity(agent_id, at_ms)
}


#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AcceptedSendEchoPlan {
    pub events: Vec<Value>,
    pub has_offscreen_entries: bool,
    pub needs_roster_refresh: bool,
}

/// Reconstructs the frozen send-acceptance echo race semantics without adding a
/// second event owner. The persistence layer records whether each echo was
/// appended while the addressed transcript was active; this function rechecks
/// which chat is active at emit time so a switch-away race is handled exactly
/// like Grok: active echoes broadcast, offscreen echoes emit only for direct
/// addressed acceptance, and any offscreen persistence refreshes roster state.
pub fn plan_accepted_send_echoes(
    agent_id: &str,
    addressed_chat_is_on_screen: bool,
    direct_addressed_acceptance: bool,
    context: &PersistedSendContext,
) -> AcceptedSendEchoPlan {
    if !context.acceptance_effects_applied {
        return AcceptedSendEchoPlan::default();
    }

    let mut events = Vec::new();
    let mut has_offscreen_entries = false;
    for echo in &context.accepted_echoes {
        if echo.is_on_active_transcript && addressed_chat_is_on_screen {
            events.push(json!({
                "channel": "transcript",
                "payload": {
                    "type": "appended",
                    "agentId": agent_id,
                    "entry": echo.entry,
                }
            }));
        } else {
            has_offscreen_entries = true;
            if direct_addressed_acceptance {
                events.push(json!({
                    "channel": "transcript",
                    "payload": {
                        "type": "appended",
                        "agentId": agent_id,
                        "entry": echo.entry,
                    }
                }));
            }
        }
    }

    AcceptedSendEchoPlan {
        events,
        has_offscreen_entries,
        needs_roster_refresh: context.needs_roster_refresh || has_offscreen_entries,
    }
}

pub fn emit_accepted_send_echoes(
    events: &SandHostEventBus,
    roster: &ProductionRosterEmit,
    active_agent_id: Option<&str>,
    agent_id: &str,
    direct_addressed_acceptance: bool,
    context: &PersistedSendContext,
) -> Result<AcceptedSendEchoPlan, String> {
    let plan = plan_accepted_send_echoes(
        agent_id,
        active_agent_id == Some(agent_id),
        direct_addressed_acceptance,
        context,
    );
    for event in &plan.events {
        events.publish(event.clone());
    }
    if plan.needs_roster_refresh {
        roster.emit_agent_update(agent_id)?;
    }
    if plan.has_offscreen_entries {
        eprintln!(
            "[sand] send raced a chat switch away from {agent_id}: entries persisted to the addressed store off screen"
        );
    }
    Ok(plan)
}
