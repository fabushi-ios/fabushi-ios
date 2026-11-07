use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::automations::automation_store::FileAutomationStore;
use crate::extensions::session::agent_db::SandAgentDb;
use crate::extensions::session::agent_db_serde::SpendGuardState;
use crate::extensions::session::production::ProductionSessionWorkers;

use super::sand_automation_spend_guard::{
    SPEND_GUARD_SNOOZE_MS, SpendGuardAnswer, SpendGuardDecision, SpendGuardEvaluation,
    SpendGuardWidget, build_spend_guard_nudge_widget, build_spend_guard_paused_widget,
    count_automation_runs_since, evaluate_automation_spend_guard, interpret_spend_guard_answer,
    render_spend_guard_answer_ack, render_spend_guard_nudge_reminder,
};
use super::transcript_entry_ids::{TranscriptEntryIdKind, next_entry_id};

#[derive(Debug, Clone, PartialEq)]
pub struct SpendGuardEvaluationSnapshot {
    pub decision: SpendGuardDecision,
    pub evaluation: SpendGuardEvaluation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpendGuardApplyResult {
    pub paused: bool,
    pub reminder: Option<String>,
    pub issued_card_entry_id: Option<String>,
    pub paused_automation_ids: Vec<String>,
}

#[derive(Clone)]
pub struct AutomationSpendGuardRuntime {
    sessions: Arc<ProductionSessionWorkers>,
}

impl AutomationSpendGuardRuntime {
    pub fn new(sessions: Arc<ProductionSessionWorkers>) -> Self {
        Self { sessions }
    }

    pub fn evaluate_with_store(
        &self,
        agent_id: &str,
        store: &FileAutomationStore,
        now_ms: f64,
    ) -> Result<SpendGuardEvaluationSnapshot, String> {
        let owner = self.sessions.open_agent_db_owner(agent_id)?;
        Self::evaluate_with_owner(&owner, store, now_ms)
    }

    fn evaluate_with_owner(
        owner: &SandAgentDb,
        store: &FileAutomationStore,
        now_ms: f64,
    ) -> Result<SpendGuardEvaluationSnapshot, String> {
        let unread = owner.get_unread_state().map_err(|error| error.to_string())?;
        let state = owner
            .get_automation_spend_guard_state()
            .map_err(|error| error.to_string())?;
        let evaluation = SpendGuardEvaluation {
            now_ms,
            last_viewed_at_ms: unread.last_viewed_at,
            unread_count: unread.unread_count.max(0.0).floor() as usize,
            fires_since_viewed_count: count_automation_runs_since(
                &store.list_definitions(),
                unread.last_viewed_at,
            ),
            nudged_at_ms: state.nudged_at_ms,
            snoozed_until_ms: state.snoozed_until_ms,
            opted_out: state.opted_out,
        };
        Ok(SpendGuardEvaluationSnapshot {
            decision: evaluate_automation_spend_guard(evaluation),
            evaluation,
        })
    }

    pub fn apply_with_store(
        &self,
        agent_id: &str,
        store: &FileAutomationStore,
        firing_automation_id: &str,
        now_ms: f64,
    ) -> Result<SpendGuardApplyResult, String> {
        self.sessions.with_agent_admitted_db_mutation(agent_id, |owner| {
            self.apply_with_owner(agent_id, owner, store, firing_automation_id, now_ms)
        })
    }

    fn apply_with_owner(
        &self,
        agent_id: &str,
        owner: &SandAgentDb,
        store: &FileAutomationStore,
        firing_automation_id: &str,
        now_ms: f64,
    ) -> Result<SpendGuardApplyResult, String> {
        let evaluated = Self::evaluate_with_owner(owner, store, now_ms)?;
        match evaluated.decision {
            SpendGuardDecision::UserActive
            | SpendGuardDecision::OptedOut
            | SpendGuardDecision::Snoozed
            | SpendGuardDecision::BelowThresholds => Ok(SpendGuardApplyResult {
                paused: firing_is_off(store, firing_automation_id),
                reminder: None,
                issued_card_entry_id: None,
                paused_automation_ids: Vec::new(),
            }),
            SpendGuardDecision::AwaitingAck => {
                let card = self.issue_card_if_none(
                    agent_id,
                    owner,
                    build_spend_guard_nudge_widget(),
                    now_ms,
                )?;
                Ok(SpendGuardApplyResult {
                    paused: firing_is_off(store, firing_automation_id),
                    reminder: None,
                    issued_card_entry_id: card,
                    paused_automation_ids: Vec::new(),
                })
            }
            SpendGuardDecision::Nudge => {
                let card = self.issue_guard_card_with_state(
                    agent_id,
                    owner,
                    &build_spend_guard_nudge_widget(),
                    &SpendGuardState {
                        nudged_at_ms: Some(now_ms),
                        ..SpendGuardState::default()
                    },
                    now_ms,
                )?;
                Ok(SpendGuardApplyResult {
                    paused: firing_is_off(store, firing_automation_id),
                    reminder: Some(render_spend_guard_nudge_reminder(
                        evaluated.evaluation,
                        self.sessions.resolve_user_time_zone().as_deref(),
                    )),
                    issued_card_entry_id: Some(card),
                    paused_automation_ids: Vec::new(),
                })
            }
            SpendGuardDecision::Pause => {
                let current = owner
                    .get_automation_spend_guard_state()
                    .map_err(|error| error.to_string())?;
                let recheck = Self::evaluate_with_owner(owner, store, now_ms)?;
                if recheck.decision != SpendGuardDecision::Pause {
                    return Ok(SpendGuardApplyResult {
                        paused: firing_is_off(store, firing_automation_id),
                        reminder: None,
                        issued_card_entry_id: None,
                        paused_automation_ids: Vec::new(),
                    });
                }
                let newly_paused = disable_every_enabled_routine(store)?;
                if newly_paused.is_empty() {
                    return Ok(SpendGuardApplyResult {
                        paused: firing_is_off(store, firing_automation_id),
                        reminder: None,
                        issued_card_entry_id: None,
                        paused_automation_ids: Vec::new(),
                    });
                }
                let mut paused = current
                    .paused_automation_ids
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>();
                paused.extend(newly_paused);
                let paused_automation_ids = paused.into_iter().collect::<Vec<_>>();
                let already_open = !current.paused_automation_ids.is_empty();
                let mut card_entry_ids = current.card_entry_ids;
                let (issued, built_card) = if already_open {
                    (None, None)
                } else {
                    let built = self.build_guard_card(
                        agent_id,
                        &build_spend_guard_paused_widget(),
                        now_ms,
                    )?;
                    card_entry_ids.push(built.0.clone());
                    (Some(built.0.clone()), Some(built))
                };
                let next_state = SpendGuardState {
                    nudged_at_ms: current.nudged_at_ms,
                    card_entry_ids,
                    paused_automation_ids: paused_automation_ids.clone(),
                    ..SpendGuardState::default()
                };
                if let Some((card_id, entry)) = built_card {
                    self.commit_guard_card(
                        agent_id,
                        owner,
                        card_id,
                        entry,
                        &next_state,
                        now_ms,
                    )?;
                } else {
                    owner
                        .set_automation_spend_guard_state(&next_state)
                        .map_err(|error| error.to_string())?;
                }
                Ok(SpendGuardApplyResult {
                    paused: true,
                    reminder: None,
                    issued_card_entry_id: issued,
                    paused_automation_ids,
                })
            }
        }
    }

    pub fn handle_widget_answer_with_store(
        &self,
        agent_id: &str,
        store: &FileAutomationStore,
        entry_id: &str,
        value: &str,
        now_ms: f64,
    ) -> Result<Option<String>, String> {
        let Some(answer) = interpret_spend_guard_answer(value) else {
            return Ok(None);
        };
        self.sessions.with_agent_admitted_db_mutation(agent_id, |owner| {
            self.handle_widget_answer_with_owner(
                agent_id, owner, store, entry_id, value, answer, now_ms,
            )
        })
    }

    fn handle_widget_answer_with_owner(
        &self,
        agent_id: &str,
        owner: &SandAgentDb,
        store: &FileAutomationStore,
        entry_id: &str,
        value: &str,
        answer: SpendGuardAnswer,
        now_ms: f64,
    ) -> Result<Option<String>, String> {
        let current = owner
            .get_automation_spend_guard_state()
            .map_err(|error| error.to_string())?;
        if !current.card_entry_ids.iter().any(|id| id == entry_id)
            || !self.is_host_issued_card(agent_id, entry_id, value)?
        {
            return Ok(None);
        }

        let mut next = SpendGuardState::default();
        match answer {
            SpendGuardAnswer::Keep | SpendGuardAnswer::Resume => {
                re_enable_guard_paused_routines(store, &current.paused_automation_ids)?;
                next.snoozed_until_ms = Some(now_ms + SPEND_GUARD_SNOOZE_MS);
            }
            SpendGuardAnswer::OptOut => {
                re_enable_guard_paused_routines(store, &current.paused_automation_ids)?;
                next.opted_out = true;
            }
            SpendGuardAnswer::Pause => {
                let mut paused = current
                    .paused_automation_ids
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>();
                paused.extend(disable_every_enabled_routine(store)?);
                next.paused_automation_ids = paused.into_iter().collect();
                if !next.paused_automation_ids.is_empty() {
                    next.card_entry_ids = current.card_entry_ids;
                }
            }
            SpendGuardAnswer::StayPaused => {}
        }
        owner
            .set_automation_spend_guard_state(&next)
            .map_err(|error| error.to_string())?;
        Ok(Some(render_spend_guard_answer_ack(answer)))
    }

    fn issue_card_if_none(
        &self,
        agent_id: &str,
        owner: &SandAgentDb,
        widget: SpendGuardWidget,
        now_ms: f64,
    ) -> Result<Option<String>, String> {
        let current = owner
            .get_automation_spend_guard_state()
            .map_err(|error| error.to_string())?;
        if !current.card_entry_ids.is_empty() {
            return Ok(None);
        }
        let (card, entry) = self.build_guard_card(agent_id, &widget, now_ms)?;
        let mut next = current;
        next.card_entry_ids = vec![card.clone()];
        self.commit_guard_card(agent_id, owner, card.clone(), entry, &next, now_ms)?;
        Ok(Some(card))
    }

    fn issue_guard_card_with_state(
        &self,
        agent_id: &str,
        owner: &SandAgentDb,
        widget: &SpendGuardWidget,
        state: &SpendGuardState,
        now_ms: f64,
    ) -> Result<String, String> {
        let (id, entry) = self.build_guard_card(agent_id, widget, now_ms)?;
        let mut next = state.clone();
        if !next.card_entry_ids.iter().any(|card| card == &id) {
            next.card_entry_ids.push(id.clone());
        }
        self.commit_guard_card(agent_id, owner, id.clone(), entry, &next, now_ms)?;
        Ok(id)
    }

    fn build_guard_card(
        &self,
        agent_id: &str,
        widget: &SpendGuardWidget,
        now_ms: f64,
    ) -> Result<(String, Value), String> {
        let entries = self.sessions.read_agent_transcript_entries_locked(agent_id)?;
        let id = next_entry_id(&entries, TranscriptEntryIdKind::SendMessage);
        let entry = json!({
            "kind": "send-message",
            "id": id.clone(),
            "message": {
                "type": "widget",
                "widget": widget_value(widget),
            },
            "timestampMs": now_ms,
        });
        Ok((id, entry))
    }

    fn commit_guard_card(
        &self,
        agent_id: &str,
        owner: &SandAgentDb,
        _id: String,
        mut entry: Value,
        state: &SpendGuardState,
        now_ms: f64,
    ) -> Result<(), String> {
        if let Some(session_key) = owner
            .read_session_work_admission()
            .map_err(|error| error.to_string())?
            .map(|record| record.session_key)
            .filter(|value| !value.trim().is_empty())
        {
            entry["sessionKey"] = Value::String(session_key.clone());
            let inserted = owner
                .append_spend_guard_card_if_session_admitted(
                    &session_key,
                    &entry,
                    state,
                    now_ms,
                )
                .map_err(|error| error.to_string())?;
            if !inserted {
                return Err("spend-guard card was not inserted".into());
            }
        } else {
            let inserted = self.sessions.append_agent_transcript_entries_locked(
                agent_id,
                None,
                &[entry],
            )?;
            if inserted != 1 {
                return Err("spend-guard card was not inserted".into());
            }
            let _ = owner.mark_activity(now_ms).map_err(|error| error.to_string())?;
            owner
                .set_automation_spend_guard_state(state)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn is_host_issued_card(
        &self,
        agent_id: &str,
        entry_id: &str,
        value: &str,
    ) -> Result<bool, String> {
        let entries = self.sessions.read_agent_transcript_entries_locked(agent_id)?;
        let Some(widget) = entries
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(entry_id))
            .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("send-message"))
            .and_then(|entry| entry.get("message"))
            .filter(|message| message.get("type").and_then(Value::as_str) == Some("widget"))
            .and_then(|message| message.get("widget"))
        else {
            return Ok(false);
        };
        let prompt = widget.get("prompt").and_then(Value::as_str).unwrap_or_default();
        let known_prompt = prompt == build_spend_guard_nudge_widget().prompt
            || prompt == build_spend_guard_paused_widget().prompt;
        let has_value = widget
            .get("options")
            .and_then(Value::as_array)
            .is_some_and(|options| {
                options.iter().any(|option| {
                    option.get("value").and_then(Value::as_str) == Some(value)
                })
            });
        Ok(known_prompt && has_value)
    }
}

pub fn disable_every_enabled_routine(store: &FileAutomationStore) -> Result<Vec<String>, String> {
    let mut disabled = Vec::new();
    for automation in store.list_definitions() {
        if automation.is_enabled {
            store
                .set_enabled(&automation.id, false)
                .map_err(|error| error.to_string())?;
            disabled.push(automation.id);
        }
    }
    Ok(disabled)
}

pub fn re_enable_guard_paused_routines(
    store: &FileAutomationStore,
    ids: &[String],
) -> Result<(), String> {
    let resume = ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
    for automation in store.list_definitions() {
        if resume.contains(automation.id.as_str()) && !automation.is_enabled {
            store
                .set_enabled(&automation.id, true)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn firing_is_off(store: &FileAutomationStore, automation_id: &str) -> bool {
    store
        .get(automation_id)
        .is_none_or(|automation| !automation.is_enabled)
}

fn widget_value(widget: &SpendGuardWidget) -> Value {
    let options = widget
        .options
        .iter()
        .map(|option| {
            let mut value = Map::new();
            value.insert("label".into(), Value::String(option.label.clone()));
            value.insert("value".into(), Value::String(option.value.clone()));
            if let Some(style) = option.style.as_ref() {
                value.insert("style".into(), Value::String(style.clone()));
            }
            Value::Object(value)
        })
        .collect::<Vec<_>>();
    json!({
        "prompt": widget.prompt,
        "options": options,
    })
}
