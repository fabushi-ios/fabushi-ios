//! OpenClaw Plugin SDK Agent Harness Runtime
//!
//! Mirrors:
//! - `src/plugin-sdk/agent-harness-runtime.ts`
//! - `src/plugin-sdk/agent-harness.ts`
//! - `src/plugin-sdk/agent-harness-completion.ts`
//! - `src/plugin-sdk/agent-harness-exec-review-runtime.ts`
//! - `src/plugin-sdk/agent-core.ts`
//! - `src/plugin-sdk/agent-runtime.ts`
//! - `src/plugin-sdk/agent-sessions.ts`
//! - `src/plugin-sdk/agent-scope-runtime.ts`
//! - `src/plugin-sdk/agent-media-payload.ts`

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttemptTerminalStatus {
    Completed,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunAttempt {
    pub attempt_id: String,
    pub session_id: String,
    pub terminal_status: Option<AttemptTerminalStatus>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredInputField {
    pub id: String,
    pub question: String,
    pub answer: Option<String>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredInputForm {
    pub form_id: String,
    pub fields: Vec<StructuredInputField>,
}

impl StructuredInputForm {
    pub fn new(form_id: impl Into<String>) -> Self {
        Self {
            form_id: form_id.into(),
            fields: Vec::new(),
        }
    }

    pub fn add_field(mut self, id: impl Into<String>, question: impl Into<String>, required: bool) -> Self {
        self.fields.push(StructuredInputField {
            id: id.into(),
            question: question.into(),
            answer: None,
            required,
        });
        self
    }

    pub fn set_answer(&mut self, field_id: &str, answer: impl Into<String>) -> bool {
        if let Some(f) = self.fields.iter_mut().find(|f| f.id == field_id) {
            f.answer = Some(answer.into());
            true
        } else {
            false
        }
    }

    pub fn validate_answers(&self) -> Result<(), String> {
        for f in &self.fields {
            if f.required && f.answer.as_ref().map_or(true, |a| a.trim().is_empty()) {
                return Err(format!("Required field '{}' is missing an answer", f.id));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct AgentHarnessManager {
    attempts: HashMap<String, AgentRunAttempt>,
}

impl AgentHarnessManager {
    pub fn new() -> Self {
        Self {
            attempts: HashMap::new(),
        }
    }

    pub fn register_attempt(&mut self, attempt_id: impl Into<String>, session_id: impl Into<String>) {
        let aid = attempt_id.into();
        self.attempts.insert(
            aid.clone(),
            AgentRunAttempt {
                attempt_id: aid,
                session_id: session_id.into(),
                terminal_status: None,
                error_message: None,
            },
        );
    }

    pub fn abort_and_drain(&mut self, attempt_id: &str, reason: &str) -> bool {
        if let Some(attempt) = self.attempts.get_mut(attempt_id) {
            attempt.terminal_status = Some(AttemptTerminalStatus::Aborted);
            attempt.error_message = Some(reason.to_string());
            true
        } else {
            false
        }
    }

    pub fn get_attempt(&self, attempt_id: &str) -> Option<&AgentRunAttempt> {
        self.attempts.get(attempt_id)
    }
}
