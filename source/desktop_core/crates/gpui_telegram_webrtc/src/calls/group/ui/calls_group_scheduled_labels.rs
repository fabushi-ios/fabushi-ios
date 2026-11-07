//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupScheduledLabelsGroupUi`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/ui/calls_group_scheduled_labels.h`
//! - `Telegram/SourceFiles/calls/group/ui/calls_group_scheduled_labels.cpp:L22-L137`

use serde::{Deserialize, Serialize};

/// Upstream seconds in a day (24 * 60 * 60)
/// calls_group_scheduled_labels.cpp:L32
pub const K_DAY_SECONDS: i64 = 86400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduledStage {
    ExactDate(i64),
    Tomorrow(i64),
    Today(i64),
    LiveNow,
}

/// Compute relative scheduled call presentation stage
/// calls_group_scheduled_labels.cpp:L31-L35: StartsWhenText
pub fn compute_scheduled_stage(scheduled_unix: i64, now_unix: i64) -> ScheduledStage {
    let diff = scheduled_unix - now_unix;
    if diff <= 0 {
        ScheduledStage::LiveNow
    } else if diff < K_DAY_SECONDS {
        ScheduledStage::Today(diff)
    } else if diff < 2 * K_DAY_SECONDS {
        ScheduledStage::Tomorrow(diff)
    } else {
        ScheduledStage::ExactDate(scheduled_unix)
    }
}

/// Upstream 3-stop text gradient stops
/// calls_group_scheduled_labels.cpp:L109-L113
pub const K_GRADIENT_STOPS: [(f32, u32); 3] = [
    (0.0, 0xFF7A87F5), // groupCallForceMutedBar1
    (0.7, 0xFFBA68C8), // groupCallForceMutedBar2
    (1.0, 0xFFFF7043), // groupCallForceMutedBar3
];

/// Scheduled Call Countdown & Gradient Label Controller
/// Target Symbol: `CallsGroupScheduledLabelsGroupUi`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallsGroupScheduledLabelsGroupUi {
    pub scheduled_timestamp: i64,
    pub current_stage: ScheduledStage,
    pub countdown_font_size: u32,
}

impl Default for CallsGroupScheduledLabelsGroupUi {
    fn default() -> Self {
        Self {
            scheduled_timestamp: 0,
            current_stage: ScheduledStage::LiveNow,
            countdown_font_size: 28,
        }
    }
}

impl CallsGroupScheduledLabelsGroupUi {
    pub fn new(scheduled_timestamp: i64, now_unix: i64) -> Self {
        let stage = compute_scheduled_stage(scheduled_timestamp, now_unix);
        Self {
            scheduled_timestamp,
            current_stage: stage,
            countdown_font_size: 28,
        }
    }

    pub fn update_now(&mut self, now_unix: i64) -> &ScheduledStage {
        self.current_stage = compute_scheduled_stage(self.scheduled_timestamp, now_unix);
        &self.current_stage
    }

    pub fn formatted_stage_text(&self) -> String {
        match &self.current_stage {
            ScheduledStage::LiveNow => "lng_group_call_starts_now".to_string(),
            ScheduledStage::Today(sec) => {
                let m = (sec / 60) % 60;
                let h = sec / 3600;
                format!("Today at {:02}:{:02}", h, m)
            }
            ScheduledStage::Tomorrow(_) => "Tomorrow".to_string(),
            ScheduledStage::ExactDate(ts) => format!("Scheduled on {}", ts),
        }
    }

    pub fn gradient_stops() -> &'static [(f32, u32); 3] {
        &K_GRADIENT_STOPS
    }
}
