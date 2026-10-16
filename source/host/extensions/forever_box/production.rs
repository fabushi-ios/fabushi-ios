use super::disk_pressure::{
    DiskPressureReminderEpisodes, DiskPressureWatch, DiskPressureWatchDeps,
    start_disk_pressure_watch,
};
use super::disk_pressure_guard::{DiskPressureLevel, DiskPressureReport, DiskPressureTrigger};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub const MOBILE_DISK_PRESSURE_POLL_INTERVAL_MS: u64 = 60_000;

pub struct MobileForeverBoxPressureOwner {
    watch: DiskPressureWatch,
}

impl MobileForeverBoxPressureOwner {
    pub fn start(root: &Path) -> Self {
        let report = Arc::new(|report: &DiskPressureReport| {
            let level = match report.level {
                DiskPressureLevel::Healthy => "healthy",
                DiskPressureLevel::Soft => "soft",
                DiskPressureLevel::Hard => "hard",
            };
            let trigger = match report.trigger {
                DiskPressureTrigger::Transition => "transition",
                DiskPressureTrigger::Heartbeat => "heartbeat",
            };
            eprintln!(
                "[fabushi:disk-pressure] volume={} level={} trigger={} available={} total={} used={:.1}%",
                report.volume,
                level,
                trigger,
                report.available_bytes,
                report.total_bytes,
                report.used_percent,
            );
        });
        let log = Arc::new(|message: &str| {
            eprintln!("[fabushi:disk-pressure] {message}");
        });
        Self {
            watch: start_disk_pressure_watch(DiskPressureWatchDeps {
                root_dir: root.to_path_buf(),
                polling_interval: Duration::from_millis(MOBILE_DISK_PRESSURE_POLL_INTERVAL_MS),
                report,
                log,
            }),
        }
    }

    pub fn level(&self) -> Option<DiskPressureLevel> {
        self.watch.level()
    }

    pub fn reminder_episodes(&self) -> Arc<DiskPressureReminderEpisodes> {
        self.watch.reminder_episodes()
    }

    pub fn stop(&self) {
        self.watch.dispose();
    }
}

impl Drop for MobileForeverBoxPressureOwner {
    fn drop(&mut self) {
        self.watch.dispose();
    }
}
