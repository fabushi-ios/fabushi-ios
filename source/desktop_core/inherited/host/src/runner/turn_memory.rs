use std::env;
use std::sync::Arc;

use crate::extensions::memory::memory_service::{
    FileMemoryStore, MemoryKind,
};
use crate::extensions::session::agent_db_serde::EpisodeTurn;
use crate::extensions::session::agent_db::SandAgentDb;
use crate::extensions::session::production::ProductionSessionWorkers;

use super::sand_memory::{
    MEMORY_EPISODE_PREFIX, MEMORY_EXTRACTION_ARCHIVE_SCAN_LIMIT,
    MEMORY_EXTRACTION_NONE_SENTINEL, MEMORY_RECENT_PROMPT_LIMIT,
    EpisodeTurn as MemoryEpisodeTurn, apply_extracted_memories,
    build_episode_system_prompt, build_episode_user_prompt,
    build_extraction_system_prompt, build_extraction_user_prompt,
    episode_interval, gather_extraction_memories, parse_extracted_memories,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnExchange {
    pub user: String,
    pub agent: String,
}

/// Build the frozen turn-memory exchange after terminal settlement.
///
/// Agent-to-agent messages emitted during the turn are part of the assistant
/// side of the remembered exchange and precede the final assistant text.
/// Empty auxiliary/final parts are omitted exactly like the Grok collector.
pub fn build_turn_memory_exchange<I, S>(
    user: impl Into<String>,
    agent_messages: I,
    assistant_text: impl Into<String>,
) -> TurnExchange
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parts = agent_messages
        .into_iter()
        .map(|message| message.as_ref().to_string())
        .filter(|message| !message.trim().is_empty())
        .collect::<Vec<_>>();
    let assistant_text = assistant_text.into();
    if !assistant_text.trim().is_empty() {
        parts.push(assistant_text);
    }
    TurnExchange {
        user: user.into(),
        agent: parts.join("\n"),
    }
}

pub enum TurnMemoryMode<'a> {
    Extract,
    RecordEvidence(&'a mut dyn FnMut(&TurnExchange)),
}

pub trait EpisodeProgress: Send + Sync {
    fn clear_pending_episode_turns(&self) -> Result<bool, String>;
    fn record_episode_turn(&self, turn: &EpisodeTurn) -> Result<bool, String>;
    fn get_pending_episode_turns(&self) -> Result<Vec<EpisodeTurn>, String>;
}

impl EpisodeProgress for SandAgentDb {
    fn clear_pending_episode_turns(&self) -> Result<bool, String> {
        self.clear_pending_episode_turns()
            .map_err(|error| error.to_string())
    }

    fn record_episode_turn(&self, turn: &EpisodeTurn) -> Result<bool, String> {
        self.record_episode_turn(turn)
            .map_err(|error| error.to_string())
    }

    fn get_pending_episode_turns(&self) -> Result<Vec<EpisodeTurn>, String> {
        self.get_pending_episode_turns()
            .map_err(|error| error.to_string())
    }
}

pub struct ProductionEpisodeProgress {
    sessions: Arc<ProductionSessionWorkers>,
    agent_id: String,
}

impl ProductionEpisodeProgress {
    pub fn new(sessions: Arc<ProductionSessionWorkers>, agent_id: impl Into<String>) -> Self {
        Self {
            sessions,
            agent_id: agent_id.into(),
        }
    }
}

impl EpisodeProgress for ProductionEpisodeProgress {
    fn clear_pending_episode_turns(&self) -> Result<bool, String> {
        self.sessions.clear_agent_pending_episode_turns(&self.agent_id)
    }

    fn record_episode_turn(&self, turn: &EpisodeTurn) -> Result<bool, String> {
        self.sessions.record_agent_episode_turn(&self.agent_id, turn)
    }

    fn get_pending_episode_turns(&self) -> Result<Vec<EpisodeTurn>, String> {
        self.sessions.get_agent_pending_episode_turns(&self.agent_id)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnMemoryReport {
    pub evidence_recorded: bool,
    pub extraction_attempted: bool,
    pub added_memories: usize,
    pub removed_memories: usize,
    pub episode_recorded: bool,
    pub episode_summary_attempted: bool,
    pub episode_memory_added: bool,
}

pub fn run_turn_memory_with<Execute>(
    memory_store: &FileMemoryStore,
    episode_progress: Option<&dyn EpisodeProgress>,
    turn_timestamp: i64,
    exchange: TurnExchange,
    mode: TurnMemoryMode<'_>,
    mut execute: Execute,
) -> TurnMemoryReport
where
    Execute: FnMut(&str, &str) -> Result<String, String>,
{
    let mut report = TurnMemoryReport::default();

    if let TurnMemoryMode::RecordEvidence(record) = mode {
        if let Some(progress) = episode_progress {
            let _ = progress.clear_pending_episode_turns();
        }
        record(&exchange);
        report.evidence_recorded = true;
        return report;
    }

    report.extraction_attempted = true;
    run_memory_extraction(
        memory_store,
        &exchange,
        &mut execute,
        &mut report,
    );

    let Some(progress) = episode_progress else {
        return report;
    };
    if progress
        .record_episode_turn(
            &EpisodeTurn {
                ts: turn_timestamp as f64,
                user: exchange.user.clone(),
                agent: exchange.agent.clone(),
            },
        )
        .is_err()
    {
        return report;
    }
    report.episode_recorded = true;

    let Ok(pending) = progress.get_pending_episode_turns() else {
        return report;
    };
    let interval_raw = env::var("SAND_MEMORY_EPISODE_INTERVAL").ok();
    if pending.len() < episode_interval(interval_raw.as_deref()) {
        return report;
    }

    report.episode_summary_attempted = true;
    let turns = pending
        .iter()
        .map(|turn| MemoryEpisodeTurn {
            ts: timestamp_ms_i64(turn.ts, turn_timestamp),
            user: turn.user.clone(),
            agent: turn.agent.clone(),
        })
        .collect::<Vec<_>>();
    let latest_timestamp = pending
        .last()
        .map(|turn| timestamp_ms_i64(turn.ts, turn_timestamp))
        .unwrap_or(turn_timestamp);

    let summary = execute(
        &build_episode_system_prompt(),
        &build_episode_user_prompt(&turns),
    )
    .ok()
    .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
    .filter(|value| {
        !value.is_empty()
            && !value.eq_ignore_ascii_case(MEMORY_EXTRACTION_NONE_SENTINEL)
    });

    if let Some(summary) = summary {
        if memory_store
            .add_memory(
                &format!("{MEMORY_EPISODE_PREFIX}{summary}"),
                latest_timestamp,
                MemoryKind::Log,
            )
            .ok()
            .flatten()
            .is_some()
        {
            report.episode_memory_added = true;
        }
    }

    let _ = progress.clear_pending_episode_turns();
    report
}

fn run_memory_extraction<Execute>(
    memory_store: &FileMemoryStore,
    exchange: &TurnExchange,
    execute: &mut Execute,
    report: &mut TurnMemoryReport,
) where
    Execute: FnMut(&str, &str) -> Result<String, String>,
{
    let recall = memory_store.recall(MEMORY_RECENT_PROMPT_LIMIT);
    let archive = memory_store.list_memories(MEMORY_EXTRACTION_ARCHIVE_SCAN_LIMIT);
    let existing = gather_extraction_memories(
        &recall,
        &archive,
        &format!("{}\n{}", exchange.user, exchange.agent),
    );
    let Ok(raw) = execute(
        &build_extraction_system_prompt(),
        &build_extraction_user_prompt(
            &exchange.user,
            &exchange.agent,
            &existing,
        ),
    ) else {
        return;
    };
    let extraction = parse_extracted_memories(&raw, &existing);
    let Ok(applied) = apply_extracted_memories(
        memory_store,
        &extraction,
        chrono::Utc::now().timestamp_millis(),
        &existing,
    ) else {
        return;
    };
    report.added_memories = applied.added.len();
    report.removed_memories = applied.removed.len();
}

fn timestamp_ms_i64(value: f64, fallback: i64) -> i64 {
    if value.is_finite()
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        value.trunc() as i64
    } else {
        fallback
    }
}
