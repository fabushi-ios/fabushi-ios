//! OpenClaw LLM Transport, Rate Limiting Gate, and Retry Diagnostics
//!
//! Mirrors:
//! - `src/llm/ai-transport-host.ts`
//! - `src/llm/provider-rate-limit.ts`
//! - `src/llm/provider-rate-limit-stream.ts`
//! - `src/llm/stream.ts`
//! - `src/llm/utils/retry.ts`
//! - `src/llm/model-runtime-binding.ts`
//! - `src/llm/model-transport-context.ts`

use std::collections::{HashMap, VecDeque};
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestPriority {
    Interactive,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitWaiter {
    pub order: u64,
    pub priority: RequestPriority,
    pub requested_at_ms: u64,
    pub max_retry_delay_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitGate {
    pub route_key: String,
    pub retry_at_ms: u64,
    pub next_at_ms: u64,
    pub queue: VecDeque<RateLimitWaiter>,
}

impl RateLimitGate {
    pub fn new(route_key: impl Into<String>) -> Self {
        Self {
            route_key: route_key.into(),
            retry_at_ms: 0,
            next_at_ms: 0,
            queue: VecDeque::new(),
        }
    }

    pub fn is_open(&self, now_ms: u64) -> bool {
        now_ms >= self.retry_at_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionResult {
    AdmittedImmediate,
    AdmittedFromQueue { wait_ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionError {
    RateLimited { retry_at_ms: u64, queue_depth: usize },
    TimeoutExceeded { wait_ms: u64, max_retry_delay_ms: u64 },
}

#[derive(Debug, Default)]
pub struct RateLimitAdmissionController {
    gates: HashMap<String, RateLimitGate>,
    waiter_order_counter: u64,
}

impl RateLimitAdmissionController {
    pub fn new() -> Self {
        Self {
            gates: HashMap::new(),
            waiter_order_counter: 0,
        }
    }

    pub fn record_rate_limit(&mut self, route: &str, retry_after_ms: u64, now_ms: u64) {
        let gate = self.gates.entry(route.to_string()).or_insert_with(|| RateLimitGate::new(route));
        gate.retry_at_ms = now_ms.saturating_add(retry_after_ms);
        gate.next_at_ms = gate.retry_at_ms;
    }

    pub fn record_success(&mut self, route: &str) {
        if let Some(gate) = self.gates.get_mut(route) {
            gate.retry_at_ms = 0;
            gate.next_at_ms = 0;
        }
    }

    pub fn admit_request(
        &mut self,
        route: &str,
        priority: RequestPriority,
        now_ms: u64,
        max_retry_delay_ms: u64,
    ) -> Result<AdmissionResult, AdmissionError> {
        let gate = self.gates.entry(route.to_string()).or_insert_with(|| RateLimitGate::new(route));

        if gate.is_open(now_ms) && gate.queue.is_empty() {
            return Ok(AdmissionResult::AdmittedImmediate);
        }

        let wait_ms = gate.retry_at_ms.saturating_sub(now_ms);
        if wait_ms > max_retry_delay_ms {
            return Err(AdmissionError::TimeoutExceeded {
                wait_ms,
                max_retry_delay_ms,
            });
        }

        self.waiter_order_counter += 1;
        let waiter = RateLimitWaiter {
            order: self.waiter_order_counter,
            priority,
            requested_at_ms: now_ms,
            max_retry_delay_ms,
        };

        // Interactive requests are placed before background requests
        if priority == RequestPriority::Interactive {
            let insert_idx = gate
                .queue
                .iter()
                .position(|w| w.priority == RequestPriority::Background)
                .unwrap_or(gate.queue.len());
            gate.queue.insert(insert_idx, waiter);
        } else {
            gate.queue.push_back(waiter);
        }

        Err(AdmissionError::RateLimited {
            retry_at_ms: gate.retry_at_ms,
            queue_depth: gate.queue.len(),
        })
    }

    pub fn pop_next_admitted(&mut self, route: &str, now_ms: u64) -> Option<(RateLimitWaiter, AdmissionResult)> {
        let gate = self.gates.get_mut(route)?;
        if !gate.is_open(now_ms) {
            return None;
        }
        let waiter = gate.queue.pop_front()?;
        let wait_ms = now_ms.saturating_sub(waiter.requested_at_ms);
        Some((waiter, AdmissionResult::AdmittedFromQueue { wait_ms }))
    }
}

pub const REPEATED_TOOL_ERROR_CODE: &str = "repeated_tool";
pub const REPETITIVE_OUTPUT_ERROR_CODE: &str = "repetitive_output";
pub const PROVIDER_FAILURE_WITH_OUTPUT_ERROR_CODE: &str = "provider_failure_with_output";
pub const PROVIDER_POST_DISPATCH_AMBIGUITY_ERROR_CODE: &str = "provider_post_dispatch_ambiguity";
pub const WEBSOCKET_NON_RETRYABLE_CLOSE_ERROR_CODE: &str = "websocket_non_retryable_close";

/// Mirrors `src/llm/utils/retry.ts:isTerminalAssistantError`
pub fn is_terminal_assistant_error(
    error_code: Option<&str>,
    http_status: Option<u16>,
    _error_message: Option<&str>,
) -> bool {
    if let Some(code) = error_code {
        match code {
            REPEATED_TOOL_ERROR_CODE
            | REPETITIVE_OUTPUT_ERROR_CODE
            | PROVIDER_FAILURE_WITH_OUTPUT_ERROR_CODE
            | PROVIDER_POST_DISPATCH_AMBIGUITY_ERROR_CODE
            | WEBSOCKET_NON_RETRYABLE_CLOSE_ERROR_CODE => return true,
            _ => {}
        }
    }

    if let Some(status) = http_status {
        match status {
            400 | 401 | 403 | 404 | 422 => return true,
            429 | 500 | 502 | 503 | 504 => return false,
            _ => {}
        }
    }

    false
}

/// Mirrors `src/llm/ai-transport-host.ts`: `unwrapModelHeaderSentinelsForProviderEgress` & secret redaction
pub fn redact_secret_sentinels_for_egress(text: &str) -> String {
    let re = Regex::new(r"__OPENCLAW_SECRET_[a-zA-Z0-9_\-]+__").unwrap();
    re.replace_all(text, "[REDACTED_SECRET]").into_owned()
}
