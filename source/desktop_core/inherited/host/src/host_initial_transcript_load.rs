use std::collections::BTreeSet;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::extensions::session::agent_session::SandAgentSessionStore;
use crate::extensions::session::production::{FallbackSession, ProductionSessionWorkers};
use crate::extensions::transcript::session_runtime::SessionRuntime;

pub const INITIAL_TRANSCRIPT_BUSY_ATTEMPTS: usize = 5;
pub const INITIAL_TRANSCRIPT_BUSY_BASE_DELAY_MS: u64 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialTranscriptDegradedReason {
    SqliteBusy(String),
    AgentLimit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitialTranscriptLoadOutcome {
    pub entry_count: usize,
    pub degraded: Option<InitialTranscriptDegradedReason>,
}

impl InitialTranscriptLoadOutcome {
    pub fn loaded(entry_count: usize) -> Self {
        Self {
            entry_count,
            degraded: None,
        }
    }

    fn degraded(reason: InitialTranscriptDegradedReason) -> Self {
        Self {
            entry_count: 0,
            degraded: Some(reason),
        }
    }
}

pub fn is_sqlite_busy_message(error: &str) -> bool {
    let message = error.to_ascii_lowercase();
    message.contains("database is locked")
        || message.contains("database table is locked")
        || message.contains("database schema is locked")
        || message.contains("sqlite_busy")
        || message.contains("sqlite busy")
}

pub fn is_agent_limit_message(error: &str) -> bool {
    let message = error.trim();
    message.starts_with("Agent limit of ") && message.ends_with(" reached")
}

pub fn load_initial_transcript_resiliently<F>(
    ensure_loaded: F,
) -> Result<InitialTranscriptLoadOutcome, String>
where
    F: FnMut() -> Result<usize, String>,
{
    load_initial_transcript_resiliently_with_delay(ensure_loaded, |delay_ms| {
        thread::sleep(Duration::from_millis(delay_ms));
    })
}

pub fn load_initial_transcript_resiliently_with_delay<F, D>(
    mut ensure_loaded: F,
    mut delay: D,
) -> Result<InitialTranscriptLoadOutcome, String>
where
    F: FnMut() -> Result<usize, String>,
    D: FnMut(u64),
{
    let mut last_busy = None;
    for attempt in 0..INITIAL_TRANSCRIPT_BUSY_ATTEMPTS {
        match ensure_loaded() {
            Ok(entry_count) => return Ok(InitialTranscriptLoadOutcome::loaded(entry_count)),
            Err(error) if is_sqlite_busy_message(&error) => {
                last_busy = Some(error);
                if attempt + 1 < INITIAL_TRANSCRIPT_BUSY_ATTEMPTS {
                    let factor = 1_u64
                        .checked_shl(attempt.min(63) as u32)
                        .unwrap_or(u64::MAX);
                    delay(INITIAL_TRANSCRIPT_BUSY_BASE_DELAY_MS.saturating_mul(factor));
                }
            }
            Err(error) if is_agent_limit_message(&error) => {
                return Ok(InitialTranscriptLoadOutcome::degraded(
                    InitialTranscriptDegradedReason::AgentLimit,
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(InitialTranscriptLoadOutcome::degraded(
        InitialTranscriptDegradedReason::SqliteBusy(
            last_busy.unwrap_or_else(|| "SQLite busy".to_string()),
        ),
    ))
}

pub fn ensure_initial_transcript_loaded(
    sessions: &Arc<ProductionSessionWorkers>,
    runtime: &SessionRuntime,
) -> Result<usize, String> {
    let store = SandAgentSessionStore::new(Arc::clone(sessions));
    let persisted = store.read_active_agent_id();
    let visible = store.list_agents()?;
    let record_ids = store.list_agent_record_ids()?;

    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(active) = persisted.as_ref() {
        if visible.iter().any(|summary| summary.id == *active)
            || record_ids.iter().any(|candidate| candidate == active)
        {
            seen.insert(active.clone());
            ordered.push(active.clone());
        }
    }
    for agent_id in visible
        .iter()
        .map(|summary| summary.id.clone())
        .chain(record_ids.into_iter())
    {
        if seen.insert(agent_id.clone()) {
            ordered.push(agent_id);
        }
    }

    for agent_id in ordered {
        match runtime.switch_agent(sessions, &agent_id, now_ms()) {
            Ok(entries) => return Ok(entries.len()),
            Err(error) => {
                eprintln!(
                    "[sand] skipping unopenable agent {agent_id} on boot: {error}"
                );
            }
        }
    }

    let fallback = store.create_fallback_session()?;
    let fallback_id = match fallback {
        FallbackSession::Existing(prepared) => prepared.agent_id,
        FallbackSession::Created(record) => record.id,
    };
    runtime
        .switch_agent(sessions, &fallback_id, now_ms())
        .map(|entries| entries.len())
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}
