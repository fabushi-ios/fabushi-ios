//! OpenClaw Runtime Health Store, Process Liveness, and Health Records Deduplication
//!
//! Mirrors:
//! - `src/plugin-state/runtime-health-store.ts`
//! - `src/plugin-state/runtime-health-records.ts`
//! - `src/plugin-state/runtime-health-store.kernel.ts`

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayGroupPick {
    Earliest,
    Latest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHealthRecordEnvelope {
    pub process_token: String,
    pub pid: u32,
    pub display_key: String,
    pub failed_at_ms: u64,
    pub error_reason: String,
}

#[derive(Debug, Default)]
pub struct RuntimeHealthStore {
    records: Vec<RuntimeHealthRecordEnvelope>,
}

impl RuntimeHealthStore {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    pub fn record_health(&mut self, record: RuntimeHealthRecordEnvelope) {
        self.records.push(record);
    }

    pub fn sweep_stale_processes(&mut self, active_tokens: &[&str]) {
        self.records.retain(|r| active_tokens.contains(&r.process_token.as_str()));
    }

    pub fn get_deduplicated_records(&self, pick: DisplayGroupPick) -> Vec<RuntimeHealthRecordEnvelope> {
        let mut groups: HashMap<String, RuntimeHealthRecordEnvelope> = HashMap::new();

        for r in &self.records {
            groups
                .entry(r.display_key.clone())
                .and_modify(|existing| match pick {
                    DisplayGroupPick::Earliest => {
                        if r.failed_at_ms < existing.failed_at_ms {
                            *existing = r.clone();
                        }
                    }
                    DisplayGroupPick::Latest => {
                        if r.failed_at_ms > existing.failed_at_ms {
                            *existing = r.clone();
                        }
                    }
                })
                .or_insert_with(|| r.clone());
        }

        let mut out: Vec<RuntimeHealthRecordEnvelope> = groups.into_values().collect();
        out.sort_by_key(|r| r.failed_at_ms);
        out
    }
}
