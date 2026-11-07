//! Telegram Desktop 1:1 Submodule: `chart_lines_filter_controller`
//! Subsystem: `statistics`, Feature ID: `TCG-N37`

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Upstream 1:1 struct: `LinesFilterController::Entry`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub enabled: bool,
    pub started_at: i64,
    pub alpha: f64,
    pub from_alpha: f64,
    pub to_alpha: f64,
    pub dt_current: f64,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            enabled: true,
            started_at: 0,
            alpha: 1.0,
            from_alpha: 1.0,
            to_alpha: 1.0,
            dt_current: 1.0,
        }
    }
}

/// Ease-in-cubic animation curve: f(t) = t * t * t
#[inline]
pub fn ease_in_cubic(t: f64) -> f64 {
    t * t * t
}

/// Upstream 1:1 class: `LinesFilterController`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinesFilterController {
    entries: HashMap<i32, Entry>,
    is_finished: bool,
}

impl Default for LinesFilterController {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            is_finished: true,
        }
    }
}

impl LinesFilterController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_enabled(&mut self, id: i32, enabled: bool, now: i64) {
        if let Some(entry) = self.entries.get_mut(&id) {
            if entry.enabled != enabled {
                entry.enabled = enabled;
                entry.started_at = now;
                entry.from_alpha = entry.alpha;
                entry.to_alpha = if enabled { 1.0 } else { 0.0 };
                entry.dt_current = 0.0;
            }
        } else {
            let from_alpha = if enabled { 0.0 } else { 1.0 };
            let to_alpha = if enabled { 1.0 } else { 0.0 };
            self.entries.insert(
                id,
                Entry {
                    enabled,
                    started_at: now,
                    alpha: from_alpha,
                    from_alpha,
                    to_alpha,
                    dt_current: 0.0,
                },
            );
        }
        self.is_finished = false;
    }

    pub fn is_enabled(&self, id: i32) -> bool {
        self.entries.get(&id).map_or(true, |e| e.enabled)
    }

    pub fn is_finished(&self) -> bool {
        self.is_finished
    }

    pub fn alpha(&self, id: i32) -> f64 {
        self.entries.get(&id).map_or(1.0, |e| e.alpha)
    }

    pub fn tick(&mut self, dt_speed: f64) {
        let mut finished_count = 0;
        let mut ids_to_remove = Vec::new();

        for (&id, entry) in self.entries.iter_mut() {
            if entry.started_at == 0 {
                finished_count += 1;
                continue;
            }
            entry.dt_current = (entry.dt_current + dt_speed).min(1.0);
            let progress = ease_in_cubic(entry.dt_current);
            entry.alpha = (entry.from_alpha + (entry.to_alpha - entry.from_alpha) * progress).clamp(0.0, 1.0);

            if entry.alpha >= 1.0 && entry.enabled {
                ids_to_remove.push(id);
            }
            if entry.dt_current >= 1.0 {
                finished_count += 1;
            }
        }

        self.is_finished = finished_count >= self.entries.len();
        for id in ids_to_remove {
            self.entries.remove(&id);
        }
    }
}

/// Target symbol alias / container
pub type ChartLinesFilterController = LinesFilterController;
