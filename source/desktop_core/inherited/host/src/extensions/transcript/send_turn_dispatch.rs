use std::sync::Arc;

use serde_json::{Map, Value};

use crate::agents::agent_messaging::{AgentAddress, build_mentioned_agents_context};
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::groups::group_chat::{GroupMember, parse_group_mentions};

use super::run_scheduler::{
    QueueAccepted, QueueDequeued, QueuedRun, RunLane, RunScheduler, RunSettlement,
    WatchdogEvent, WatchdogStage,
};
use super::send_message_shaping::build_composed_offline_note;
use super::send_pipeline::PersistedSendContext;
use super::workflow_commands::expand_workflow_references;

pub fn prepare_direct_turn_runner_args(
    workers: Arc<ProductionSessionWorkers>,
    agent_id: &str,
    args: &Value,
    persisted: &PersistedSendContext,
) -> Result<Value, String> {
    let mut shaped = args.as_object().cloned().unwrap_or_else(Map::new);
    let raw_prompt = shaped
        .get("prompt")
        .or_else(|| shaped.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let rich_text = shaped.get("richText").and_then(Value::as_str);

    let expanded = expand_workflow_references(Arc::clone(&workers), agent_id, &raw_prompt, rich_text)
        .map_err(|error| error.to_string())?;
    let expanded = with_mentioned_agents_context(workers.as_ref(), agent_id, &raw_prompt, &expanded)?;
    let composed_note = shaped
        .get("composedAtMs")
        .and_then(Value::as_f64)
        .map(build_composed_offline_note)
        .unwrap_or_default();
    let prompt_for_run = if composed_note.is_empty() {
        expanded
    } else if expanded.is_empty() {
        composed_note
    } else {
        format!("{composed_note}\n{expanded}")
    };
    shaped.insert("prompt".into(), Value::String(prompt_for_run));

    if let Some(message_id) = persisted
        .user_message_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        shaped.insert("messageId".into(), Value::String(message_id.to_string()));
        let recent = workers
            .read_agent_transcript_entries(agent_id)?
            .into_iter()
            .filter(|entry| {
                entry.get("kind").and_then(Value::as_str) == Some("message")
                    && entry.get("role").and_then(Value::as_str) == Some("user")
                    && entry.get("fromAgent").is_none_or(Value::is_null)
                    && entry.get("channel").is_none_or(Value::is_null)
            })
            .filter_map(|entry| {
                let id = entry.get("id").and_then(Value::as_str)?.trim().to_string();
                if id.is_empty() {
                    return None;
                }
                let mut message = Map::new();
                message.insert("id".into(), Value::String(id));
                message.insert(
                    "text".into(),
                    Value::String(
                        entry
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ),
                );
                if let Some(rich_text) = entry.get("richText").filter(|value| !value.is_null()) {
                    message.insert("richText".into(), rich_text.clone());
                }
                Some(Value::Object(message))
            })
            .collect::<Vec<_>>();
        shaped.insert("recentUserMessages".into(), Value::Array(recent));
    }

    Ok(Value::Object(shaped))
}

fn with_mentioned_agents_context(
    workers: &ProductionSessionWorkers,
    agent_id: &str,
    raw_prompt: &str,
    prompt_for_run: &str,
) -> Result<String, String> {
    if raw_prompt.is_empty() {
        return Ok(prompt_for_run.to_string());
    }
    if workers
        .summarize_agent_by_id(agent_id, None)?
        .is_some_and(|summary| summary.is_group)
    {
        return Ok(prompt_for_run.to_string());
    }

    let roster = workers
        .list_agent_summaries(None)?
        .into_iter()
        .filter(|summary| {
            summary.id != agent_id
                && (!summary.is_group || summary.member_ids.iter().any(|id| id == agent_id))
        })
        .collect::<Vec<_>>();
    let mention_members = roster
        .iter()
        .map(|summary| GroupMember {
            id: summary.id.clone(),
            name: summary.name.clone(),
            description: summary.description.clone(),
        })
        .collect::<Vec<_>>();
    let mentions = parse_group_mentions(raw_prompt, &mention_members);
    let mentioned = mentions
        .member_ids
        .iter()
        .filter_map(|id| roster.iter().find(|summary| &summary.id == id))
        .map(|summary| AgentAddress {
            id: summary.id.clone(),
            name: summary.name.clone(),
            description: (!summary.description.trim().is_empty())
                .then(|| summary.description.clone()),
            is_group: summary.is_group,
        })
        .collect::<Vec<_>>();
    let Some(context) = build_mentioned_agents_context(&mentioned) else {
        return Ok(prompt_for_run.to_string());
    };
    if prompt_for_run.is_empty() {
        Ok(context)
    } else {
        Ok(format!("{context}\n\n{prompt_for_run}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTurnTicket {
    pub agent_id: String,
    pub task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnWatchdogTick {
    pub event: WatchdogEvent,
    pub started_next: Option<QueueDequeued>,
}

#[derive(Debug)]
pub struct ProductionTurnDispatch {
    scheduler: RunScheduler,
    next_task_seq: u64,
}

impl Default for ProductionTurnDispatch {
    fn default() -> Self {
        Self {
            scheduler: RunScheduler::default(),
            next_task_seq: 0,
        }
    }
}

impl ProductionTurnDispatch {
    pub fn with_watchdog(watchdog_ms: u64, watchdog_grace_ms: u64) -> Self {
        Self {
            scheduler: RunScheduler::new(watchdog_ms, watchdog_grace_ms),
            next_task_seq: 0,
        }
    }
}

impl ProductionTurnDispatch {
    pub fn watchdog_wait_ms(&self, agent_id: &str, now_ms: u64) -> Option<u64> {
        self.scheduler.next_watchdog_delay_ms(agent_id, now_ms)
    }

    pub fn watchdog_tick(
        &mut self,
        agent_id: &str,
        now_ms: u64,
    ) -> Option<TurnWatchdogTick> {
        let event = self.scheduler.watchdog_tick(agent_id, now_ms)?;
        let started_next = if event.stage == WatchdogStage::Escape {
            self.scheduler.start_next(agent_id, now_ms)
        } else {
            None
        };
        Some(TurnWatchdogTick {
            event,
            started_next,
        })
    }

    pub fn enqueue_turn(
        &mut self,
        agent_id: &str,
        client_nonce: Option<&str>,
        accepted_at_ms: u64,
        now_ms: u64,
        lane: RunLane,
        source: &str,
        ack_token: Option<&str>,
    ) -> Result<(UserTurnTicket, QueueAccepted), &'static str> {
        let (ticket, accepted, _) = self.enqueue_turn_with_start(
            agent_id, client_nonce, accepted_at_ms, now_ms, lane, source, ack_token,
        )?;
        Ok((ticket, accepted))
    }

    pub fn enqueue_turn_with_start(
        &mut self,
        agent_id: &str,
        client_nonce: Option<&str>,
        accepted_at_ms: u64,
        now_ms: u64,
        lane: RunLane,
        source: &str,
        ack_token: Option<&str>,
    ) -> Result<(UserTurnTicket, QueueAccepted, Option<QueueDequeued>), &'static str> {
        self.next_task_seq = self.next_task_seq.saturating_add(1);
        let nonce = client_nonce
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("no-nonce");
        let task_id = format!("send:{agent_id}:{}:{nonce}", self.next_task_seq);
        let accepted = self.scheduler.enqueue(
            agent_id.to_string(),
            QueuedRun {
                task_id: task_id.clone(),
                lane,
                source: source.to_string(),
                enqueued_at_ms: now_ms,
                accepted_at_ms: Some(accepted_at_ms),
                ack_token: ack_token.map(ToOwned::to_owned),
            },
        )?;
        let started = if self.scheduler.active(agent_id).is_none() {
            self.scheduler.start_next(agent_id, now_ms)
        } else {
            None
        };
        Ok((
            UserTurnTicket {
                agent_id: agent_id.to_string(),
                task_id,
            },
            accepted,
            started,
        ))
    }

    pub fn enqueue_user_turn(
        &mut self,
        agent_id: &str,
        client_nonce: Option<&str>,
        accepted_at_ms: u64,
        now_ms: u64,
    ) -> Result<(UserTurnTicket, QueueAccepted), &'static str> {
        self.enqueue_turn(
            agent_id,
            client_nonce,
            accepted_at_ms,
            now_ms,
            RunLane::User,
            "turn",
            None,
        )
    }

    pub fn active_generation_for(&self, ticket: &UserTurnTicket) -> Option<u64> {
        let active = self.scheduler.active(&ticket.agent_id)?;
        (active.item.task_id == ticket.task_id).then_some(active.generation)
    }

    pub fn settle_and_start_next(
        &mut self,
        ticket: &UserTurnTicket,
        generation: u64,
        now_ms: u64,
    ) -> (RunSettlement, Option<QueueDequeued>) {
        let settlement = self
            .scheduler
            .settle(&ticket.agent_id, generation, now_ms);
        let next = self.scheduler.start_next(&ticket.agent_id, now_ms);
        (settlement, next)
    }

    pub fn queued_task_ids(&self, agent_id: &str) -> Vec<String> {
        self.scheduler.queued_task_ids(agent_id)
    }

    pub fn active_lane(&self, agent_id: &str) -> Option<RunLane> {
        self.scheduler.get_active_lane(agent_id)
    }

    pub fn active_source(&self, agent_id: &str) -> Option<&str> {
        self.scheduler.active(agent_id).map(|active| active.item.source.as_str())
    }

    pub fn is_idle(&self, agent_id: &str) -> bool {
        self.scheduler.is_idle(agent_id)
    }

    pub fn dispose(&mut self) {
        self.scheduler.dispose();
    }
}
