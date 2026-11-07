use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnExecutionPhase {
    Admitted,
    Running,
    Completed,
    Cancelled,
    TimedOut,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub turn_index: u32,
    pub user_input: String,
    pub agent_output: Option<String>,
    pub model_override: Option<String>,
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveTurnExecution {
    pub turn_index: u32,
    pub turn_id: String,
    pub session_id: String,
    pub prompt: String,
    pub model_override: Option<String>,
    pub phase: TurnExecutionPhase,
    pub started_at_ms: u64,
    pub deadline_ms: Option<u64>,
    pub cancellation_reason: Option<String>,
    pub transcript_entry_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowUpTurn {
    pub follow_up_id: String,
    pub session_id: String,
    pub prompt: String,
    pub model_override: Option<String>,
    pub enqueued_at_ms: u64,
    pub priority: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnAdmissionResult {
    Admitted(u32),
    Queued(u32),
    RejectedConflict(String),
    RejectedFenced(String),
}

#[derive(Default)]
struct SessionTurnState {
    history: Vec<ConversationTurn>,
    active_turn: Option<ActiveTurnExecution>,
    follow_up_queue: VecDeque<FollowUpTurn>,
    cancellation_flags: HashMap<String, String>,
}

#[derive(Clone, Default)]
pub struct SessionTurnsController {
    sessions: Arc<Mutex<HashMap<String, SessionTurnState>>>,
    max_turn_lease: Duration,
}

impl SessionTurnsController {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            max_turn_lease: Duration::from_secs(300),
        }
    }

    pub fn with_turn_lease(max_turn_lease: Duration) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            max_turn_lease,
        }
    }

    /// Backwards-compatible turn recording start.
    pub fn record_turn_start(
        &self,
        session_id: &str,
        user_input: &str,
        model_override: Option<String>,
    ) -> u32 {
        let now_ms = current_time_ms();
        let mut sessions = self.sessions.lock().unwrap();
        let state = sessions.entry(session_id.to_string()).or_default();
        let turn_index = state.history.len() as u32;

        state.history.push(ConversationTurn {
            turn_index,
            user_input: user_input.to_string(),
            agent_output: None,
            model_override: model_override.clone(),
            started_at_ms: now_ms,
            completed_at_ms: None,
        });

        state.active_turn = Some(ActiveTurnExecution {
            turn_index,
            turn_id: format!("{session_id}:turn:{turn_index}"),
            session_id: session_id.to_string(),
            prompt: user_input.to_string(),
            model_override,
            phase: TurnExecutionPhase::Running,
            started_at_ms: now_ms,
            deadline_ms: Some(now_ms + self.max_turn_lease.as_millis() as u64),
            cancellation_reason: None,
            transcript_entry_id: None,
        });

        turn_index
    }

    /// Backwards-compatible turn completion.
    pub fn record_turn_complete(&self, session_id: &str, turn_index: u32, output: &str) -> bool {
        let now_ms = current_time_ms();
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(state) = sessions.get_mut(session_id) {
            if let Some(turn) = state.history.get_mut(turn_index as usize) {
                turn.agent_output = Some(output.to_string());
                turn.completed_at_ms = Some(now_ms);

                if let Some(active) = state.active_turn.as_mut() {
                    if active.turn_index == turn_index {
                        active.phase = TurnExecutionPhase::Completed;
                    }
                }
                state.active_turn = None;
                return true;
            }
        }
        false
    }

    /// Retrieve all completed / recorded turns for a session.
    pub fn get_turns(&self, session_id: &str) -> Vec<ConversationTurn> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(session_id)
            .map(|s| s.history.clone())
            .unwrap_or_default()
    }

    /// Production turn admission checking active execution fences and concurrency barriers.
    pub fn admit_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        prompt: &str,
        model_override: Option<String>,
        allow_queue: bool,
    ) -> TurnAdmissionResult {
        let now_ms = current_time_ms();
        let mut sessions = self.sessions.lock().unwrap();
        let state = sessions.entry(session_id.to_string()).or_default();

        if let Some(active) = &state.active_turn {
            if active.phase == TurnExecutionPhase::Running {
                if allow_queue {
                    let queue_idx = state.follow_up_queue.len() as u32;
                    state.follow_up_queue.push_back(FollowUpTurn {
                        follow_up_id: turn_id.to_string(),
                        session_id: session_id.to_string(),
                        prompt: prompt.to_string(),
                        model_override,
                        enqueued_at_ms: now_ms,
                        priority: 0,
                    });
                    return TurnAdmissionResult::Queued(queue_idx);
                }
                return TurnAdmissionResult::RejectedConflict(format!(
                    "Active turn {} in progress",
                    active.turn_id
                ));
            }
        }

        let turn_index = state.history.len() as u32;
        state.history.push(ConversationTurn {
            turn_index,
            user_input: prompt.to_string(),
            agent_output: None,
            model_override: model_override.clone(),
            started_at_ms: now_ms,
            completed_at_ms: None,
        });

        state.active_turn = Some(ActiveTurnExecution {
            turn_index,
            turn_id: turn_id.to_string(),
            session_id: session_id.to_string(),
            prompt: prompt.to_string(),
            model_override,
            phase: TurnExecutionPhase::Running,
            started_at_ms: now_ms,
            deadline_ms: Some(now_ms + self.max_turn_lease.as_millis() as u64),
            cancellation_reason: None,
            transcript_entry_id: None,
        });

        TurnAdmissionResult::Admitted(turn_index)
    }

    /// Check if a session has an active running turn.
    pub fn has_active_turn(&self, session_id: &str) -> bool {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(session_id)
            .and_then(|s| s.active_turn.as_ref())
            .map(|a| a.phase == TurnExecutionPhase::Running)
            .unwrap_or(false)
    }

    /// Retrieve active turn custody details if running.
    pub fn active_turn_details(&self, session_id: &str) -> Option<ActiveTurnExecution> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(session_id)
            .and_then(|s| s.active_turn.clone())
            .filter(|a| a.phase == TurnExecutionPhase::Running)
    }

    /// Request cancellation/abort of the currently active turn.
    pub fn cancel_active_turn(&self, session_id: &str, reason: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(state) = sessions.get_mut(session_id) {
            if let Some(active) = state.active_turn.as_mut() {
                if active.phase == TurnExecutionPhase::Running {
                    active.phase = TurnExecutionPhase::Cancelled;
                    active.cancellation_reason = Some(reason.to_string());
                    state
                        .cancellation_flags
                        .insert(active.turn_id.clone(), reason.to_string());
                    return true;
                }
            }
        }
        false
    }

    /// Query whether a specific turn ID has a pending cancellation request.
    pub fn is_turn_cancelled(&self, session_id: &str, turn_id: &str) -> bool {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(session_id)
            .map(|s| s.cancellation_flags.contains_key(turn_id))
            .unwrap_or(false)
    }

    /// Watchdog: recover any turns that have exceeded their execution deadline.
    pub fn recover_timed_out_turns(&self, now_ms: u64) -> Vec<(String, String)> {
        let mut timed_out = Vec::new();
        let mut sessions = self.sessions.lock().unwrap();
        for (session_id, state) in sessions.iter_mut() {
            if let Some(active) = state.active_turn.as_mut() {
                if active.phase == TurnExecutionPhase::Running {
                    if let Some(deadline) = active.deadline_ms {
                        if now_ms >= deadline {
                            active.phase = TurnExecutionPhase::TimedOut;
                            timed_out.push((session_id.clone(), active.turn_id.clone()));
                        }
                    }
                }
            }
        }
        timed_out
    }

    /// Enqueue a follow-up turn into the session barrier queue.
    pub fn enqueue_follow_up(&self, session_id: &str, follow_up: FollowUpTurn) {
        let mut sessions = self.sessions.lock().unwrap();
        let state = sessions.entry(session_id.to_string()).or_default();
        state.follow_up_queue.push_back(follow_up);
    }

    /// Drain pending follow-up turns once an active turn finishes or yields.
    pub fn drain_follow_ups(&self, session_id: &str) -> Vec<FollowUpTurn> {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(state) = sessions.get_mut(session_id) {
            state.follow_up_queue.drain(..).collect()
        } else {
            Vec::new()
        }
    }

    /// Commit hook binding turn execution settlement to a durable transcript entry ID.
    pub fn bind_turn_transcript_entry(
        &self,
        session_id: &str,
        turn_index: u32,
        transcript_entry_id: &str,
    ) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(state) = sessions.get_mut(session_id) {
            if let Some(active) = state.active_turn.as_mut() {
                if active.turn_index == turn_index {
                    active.transcript_entry_id = Some(transcript_entry_id.to_string());
                    return true;
                }
            }
        }
        false
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
