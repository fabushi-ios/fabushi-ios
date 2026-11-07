//! Signal Bars Visual Indicator & Network Quality Evaluator
//! Upstream: Telegram/SourceFiles/calls/calls_signal_bars.h:L20-L36, calls_signal_bars.cpp:L16-L65
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalBarMetrics {
    pub width: f32,
    pub radius: f32,
    pub skip: f32,
    pub min_height: f32,
    pub max_height: f32,
    pub inactive_opacity: f32,
}

impl Default for SignalBarMetrics {
    fn default() -> Self {
        Self {
            width: 2.0,
            radius: 1.0,
            skip: 2.0,
            min_height: 4.0,
            max_height: 10.0,
            inactive_opacity: 0.5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsSignalBars {
    pub count: i32,
    pub metrics: SignalBarMetrics,
    pub rtt_ms: u32,
    pub packet_loss_rate: f32,
    pub jitter_ms: u32,
}

impl CallsSignalBars {
    pub const K_SIGNAL_BAR_STARTING: i32 = -1;
    pub const K_SIGNAL_BAR_FINISHED: i32 = -2;
    pub const K_SIGNAL_BAR_COUNT: i32 = 4;

    pub fn new() -> Self {
        Self {
            count: Self::K_SIGNAL_BAR_STARTING,
            metrics: SignalBarMetrics::default(),
            rtt_ms: 0,
            packet_loss_rate: 0.0,
            jitter_ms: 0,
        }
    }

    pub fn compute_bars_from_metrics(rtt_ms: u32, packet_loss: f32, jitter_ms: u32) -> i32 {
        if packet_loss > 0.15 || rtt_ms > 800 || jitter_ms > 100 {
            1
        } else if packet_loss > 0.08 || rtt_ms > 450 || jitter_ms > 60 {
            2
        } else if packet_loss > 0.02 || rtt_ms > 200 || jitter_ms > 30 {
            3
        } else {
            4
        }
    }

    pub fn update_network_stats(&mut self, rtt_ms: u32, packet_loss: f32, jitter_ms: u32) {
        self.rtt_ms = rtt_ms;
        self.packet_loss_rate = packet_loss;
        self.jitter_ms = jitter_ms;
        if self.count != Self::K_SIGNAL_BAR_FINISHED {
            self.count = Self::compute_bars_from_metrics(rtt_ms, packet_loss, jitter_ms);
        }
    }

    pub fn calculate_bar_height(&self, bar_index: usize) -> f32 {
        if bar_index >= Self::K_SIGNAL_BAR_COUNT as usize {
            return self.metrics.min_height;
        }
        let step = (self.metrics.max_height - self.metrics.min_height) / 3.0;
        self.metrics.min_height + step * (bar_index as f32)
    }

    pub fn bar_opacity(&self, bar_index: usize) -> f32 {
        if self.count == Self::K_SIGNAL_BAR_FINISHED || (bar_index as i32) >= self.count {
            self.metrics.inactive_opacity
        } else {
            1.0
        }
    }

    pub fn total_width(&self) -> f32 {
        let n = Self::K_SIGNAL_BAR_COUNT as f32;
        self.metrics.width * n + self.metrics.skip * (n - 1.0)
    }
}
