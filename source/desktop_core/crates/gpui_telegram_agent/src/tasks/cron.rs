use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use mahayana_host_runtime::automations::automation_schedule::{
    compile_cron_matcher, compute_next_run_at, parse_every_interval_ms,
    summarize_schedule_next_7_days, AutomationScheduleSummary, CronMatcher,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronExecutionRecord {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,
    pub output_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronRoutine {
    pub routine_id: String,
    pub name: String,
    pub expression: String, // 5-field cron: minute hour dom month dow, alias, or @every interval
    pub time_zone: Option<String>,
    pub enabled: bool,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
    pub execution_records: Vec<CronExecutionRecord>,
}

impl CronRoutine {
    pub fn new(routine_id: String, expression: String) -> Self {
        Self {
            routine_id,
            name: "Default Routine".into(),
            expression,
            time_zone: None,
            enabled: true,
            last_run: None,
            next_run: None,
            execution_records: Vec::new(),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_time_zone(mut self, time_zone: Option<String>) -> Self {
        self.time_zone = time_zone;
        self
    }

    /// Validates the 5-field cron expression or @every interval
    pub fn validate_expression(&self) -> bool {
        compile_cron_matcher(&self.expression).is_some()
            || parse_every_interval_ms(&self.expression).is_some()
    }

    /// Compiles into the inherited CronMatcher (if a 5-field/alias cron schedule)
    pub fn compile(&self) -> Option<CronMatcher> {
        compile_cron_matcher(&self.expression)
    }

    /// Computes the next run time after a baseline timestamp
    pub fn compute_next_run(&mut self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let after_ms = after.timestamp_millis() as f64;
        let next_ms = compute_next_run_at(
            &self.expression,
            after_ms,
            self.time_zone.as_deref(),
        )?;
        let next_dt = DateTime::from_timestamp_millis(next_ms as i64)?;
        self.next_run = Some(next_dt);
        Some(next_dt)
    }

    /// Previews fires over the next 7 days using inherited summarize_schedule_next_7_days
    pub fn preview_next_7_days(&self, start: DateTime<Utc>) -> AutomationScheduleSummary {
        let start_ms = start.timestamp_millis() as f64;
        summarize_schedule_next_7_days(
            &self.expression,
            self.time_zone.as_deref(),
            start_ms,
        )
    }

    pub fn record_execution(&mut self, record: CronExecutionRecord) {
        self.last_run = Some(record.started_at);
        self.execution_records.push(record);
    }

    pub fn toggle_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}
