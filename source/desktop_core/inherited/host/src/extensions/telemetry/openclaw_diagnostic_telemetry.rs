//! OpenClaw Diagnostic Telemetry, Memory Pressure, and Watchdog Activity Projection
//!
//! Mirrors:
//! - `src/logging/diagnostic-controller-projection.ts`
//! - `src/logging/diagnostic-cpu-profile.ts`
//! - `src/logging/diagnostic-heap-profile.ts`
//! - `src/logging/diagnostic-gc.ts`
//! - `src/logging/diagnostic-heartbeat-context.ts`

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryPressureLevel {
    Normal,
    Moderate,
    Critical,
}

pub fn evaluate_memory_pressure(used_bytes: u64, total_bytes: u64) -> MemoryPressureLevel {
    if total_bytes == 0 {
        return MemoryPressureLevel::Normal;
    }
    let ratio = (used_bytes as f64) / (total_bytes as f64);
    if ratio >= 0.90 {
        MemoryPressureLevel::Critical
    } else if ratio >= 0.75 {
        MemoryPressureLevel::Moderate
    } else {
        MemoryPressureLevel::Normal
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticHeartbeat {
    pub session_id: String,
    pub active_operation: Option<String>,
    pub last_heartbeat_timestamp_ms: u64,
    pub is_alive: bool,
}

#[derive(Debug, Default)]
pub struct DiagnosticRunRegistry {
    sessions: HashMap<String, DiagnosticHeartbeat>,
}

impl DiagnosticRunRegistry {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn record_heartbeat(
        &mut self,
        session_id: impl Into<String>,
        operation: Option<String>,
        now_ms: u64,
    ) {
        let sid = session_id.into();
        self.sessions.insert(
            sid.clone(),
            DiagnosticHeartbeat {
                session_id: sid,
                active_operation: operation,
                last_heartbeat_timestamp_ms: now_ms,
                is_alive: true,
            },
        );
    }

    pub fn sweep_liveness(&mut self, now_ms: u64, timeout_ms: u64) {
        for hb in self.sessions.values_mut() {
            if now_ms.saturating_sub(hb.last_heartbeat_timestamp_ms) > timeout_ms {
                hb.is_alive = false;
            }
        }
    }

    pub fn get_session_heartbeat(&self, session_id: &str) -> Option<&DiagnosticHeartbeat> {
        self.sessions.get(session_id)
    }
}
