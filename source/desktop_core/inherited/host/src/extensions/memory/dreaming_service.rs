//! Memory Dreaming Configuration, Promotion Calibration & Recency Half-Life Evaluation
//!
//! Mirrors upstream OpenClaw `src/memory-host-sdk/dreaming.ts`:
//! Configures memory dreaming promotion crons, evaluates promotion eligibility
//! using deterministic calibration thresholds, and calculates recency decay.

use serde::{Deserialize, Serialize};

pub const DEFAULT_MEMORY_DREAMING_FREQUENCY: &str = "0 3 * * *";
pub const DEFAULT_MEMORY_DREAMING_PLUGIN_ID: &str = "memory-core";

pub const MANAGED_MEMORY_DREAMING_CRON_NAME: &str = "Memory Dreaming Promotion";
pub const MANAGED_MEMORY_DREAMING_CRON_TAG: &str =
    "[managed-by=memory-core.short-term-promotion]";
pub const MEMORY_DREAMING_SYSTEM_EVENT_TEXT: &str =
    "__openclaw_memory_core_short_term_promotion_dream__";

pub const LEGACY_MEMORY_LIGHT_DREAMING_CRON_NAME: &str = "Memory Light Dreaming";
pub const LEGACY_MEMORY_LIGHT_DREAMING_CRON_TAG: &str =
    "[managed-by=memory-core.dreaming.light]";
pub const LEGACY_MEMORY_LIGHT_DREAMING_EVENT_TEXT: &str =
    "__openclaw_memory_core_light_sleep__";

pub const LEGACY_MEMORY_REM_DREAMING_CRON_NAME: &str = "Memory REM Dreaming";
pub const LEGACY_MEMORY_REM_DREAMING_CRON_TAG: &str =
    "[managed-by=memory-core.dreaming.rem]";
pub const LEGACY_MEMORY_REM_DREAMING_EVENT_TEXT: &str =
    "__openclaw_memory_core_rem_sleep__";

pub const DEFAULT_MEMORY_DEEP_DREAMING_LIMIT: usize = 10;
pub const DEFAULT_MEMORY_DEEP_DREAMING_MIN_SCORE: f64 = 0.75;
pub const DEFAULT_MEMORY_DEEP_DREAMING_MIN_RECALL_COUNT: usize = 3;
pub const DEFAULT_MEMORY_DEEP_DREAMING_MIN_UNIQUE_QUERIES: usize = 3;
pub const DEFAULT_MEMORY_DEEP_DREAMING_RECENCY_HALF_LIFE_DAYS: f64 = 14.0;
pub const DEFAULT_MEMORY_DEEP_DREAMING_MAX_PROMOTED_SNIPPET_TOKENS: usize = 160;
pub const DEFAULT_MEMORY_DEEP_DREAMING_MAX_PRIOR_ENTRY_LOSS_FRACTION: f64 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryDreamingSpeed {
    Fast,
    Balanced,
    Slow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryDreamingThinking {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryDreamingBudget {
    Cheap,
    Medium,
    Expensive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDreamingConfig {
    pub enabled: bool,
    pub frequency: String,
    pub speed: MemoryDreamingSpeed,
    pub thinking: MemoryDreamingThinking,
    pub budget: MemoryDreamingBudget,
    pub min_score: f64,
    pub min_recall_count: usize,
    pub min_unique_queries: usize,
    pub recency_half_life_days: f64,
}

impl Default for MemoryDreamingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            frequency: DEFAULT_MEMORY_DREAMING_FREQUENCY.to_string(),
            speed: MemoryDreamingSpeed::Balanced,
            thinking: MemoryDreamingThinking::Medium,
            budget: MemoryDreamingBudget::Medium,
            min_score: DEFAULT_MEMORY_DEEP_DREAMING_MIN_SCORE,
            min_recall_count: DEFAULT_MEMORY_DEEP_DREAMING_MIN_RECALL_COUNT,
            min_unique_queries: DEFAULT_MEMORY_DEEP_DREAMING_MIN_UNIQUE_QUERIES,
            recency_half_life_days: DEFAULT_MEMORY_DEEP_DREAMING_RECENCY_HALF_LIFE_DAYS,
        }
    }
}

/// Candidate snippet evaluated for promotion from short-term to durable memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPromotionCandidate {
    pub key: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
    pub recall_count: usize,
    pub unique_queries: usize,
    pub age_days: f64,
}

/// Computes recency decay weight: 2^(-age_days / half_life_days).
pub fn compute_recency_decay(age_days: f64, half_life_days: f64) -> f64 {
    if half_life_days <= 0.0 || age_days <= 0.0 {
        return 1.0;
    }
    let exponent = -age_days / half_life_days;
    2.0_f64.powf(exponent).clamp(0.0, 1.0)
}

/// Evaluates whether a candidate qualifies for durable memory promotion.
pub fn is_candidate_eligible_for_promotion(
    candidate: &MemoryPromotionCandidate,
    config: &MemoryDreamingConfig,
) -> bool {
    let decayed_score = candidate.score * compute_recency_decay(candidate.age_days, config.recency_half_life_days);
    decayed_score >= config.min_score
        && candidate.recall_count >= config.min_recall_count
        && candidate.unique_queries >= config.min_unique_queries
}
