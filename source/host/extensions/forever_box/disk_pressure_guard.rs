use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

pub const GIB: u64 = 1024 * 1024 * 1024;
pub const DISK_PRESSURE_HEARTBEAT_MS: u64 = 5 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiskPressureThresholds {
    pub soft_available_bytes: u64,
    pub soft_available_ratio: f64,
    pub hard_available_bytes: u64,
    pub hard_available_ratio: f64,
    pub soft_recovery_bytes: u64,
    pub soft_recovery_ratio: f64,
    pub hard_recovery_bytes: u64,
    pub hard_recovery_ratio: f64,
}

pub const DISK_PRESSURE_THRESHOLDS: DiskPressureThresholds = DiskPressureThresholds {
    soft_available_bytes: 8 * GIB,
    soft_available_ratio: 0.15,
    hard_available_bytes: 2 * GIB,
    hard_available_ratio: 0.05,
    soft_recovery_bytes: 10 * GIB,
    soft_recovery_ratio: 0.20,
    hard_recovery_bytes: 3 * GIB,
    hard_recovery_ratio: 0.08,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskPressureLevel {
    Healthy,
    Soft,
    Hard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskVolumeRoot {
    pub volume: String,
    pub path: PathBuf,
}

impl DiskVolumeRoot {
    pub fn new(volume: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self { volume: volume.into(), path: path.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskVolumeSnapshot {
    pub volume: String,
    pub device_id: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskVolumeSample {
    pub snapshots: Vec<DiskVolumeSnapshot>,
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiskPressureState {
    level: DiskPressureLevel,
    last_reported_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiskPressureTrigger {
    Transition,
    Heartbeat,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiskPressureReport {
    pub volume: String,
    pub device_id: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub level: DiskPressureLevel,
    pub trigger: DiskPressureTrigger,
    pub used_percent: f64,
}

pub fn aggregate_disk_pressure_level(
    states: &HashMap<String, DiskPressureLevel>,
) -> DiskPressureLevel {
    if states.values().any(|level| *level == DiskPressureLevel::Hard) {
        DiskPressureLevel::Hard
    } else if states.values().any(|level| *level == DiskPressureLevel::Soft) {
        DiskPressureLevel::Soft
    } else {
        DiskPressureLevel::Healthy
    }
}

pub fn classify_disk_pressure(
    total_bytes: u64,
    available_bytes: u64,
    previous: DiskPressureLevel,
) -> DiskPressureLevel {
    if total_bytes == 0 {
        return DiskPressureLevel::Healthy;
    }
    let ratio = available_bytes as f64 / total_bytes as f64;
    if available_bytes <= DISK_PRESSURE_THRESHOLDS.hard_available_bytes
        || ratio <= DISK_PRESSURE_THRESHOLDS.hard_available_ratio
    {
        return DiskPressureLevel::Hard;
    }
    if previous == DiskPressureLevel::Hard
        && (available_bytes <= DISK_PRESSURE_THRESHOLDS.hard_recovery_bytes
            || ratio <= DISK_PRESSURE_THRESHOLDS.hard_recovery_ratio)
    {
        return DiskPressureLevel::Hard;
    }
    if available_bytes <= DISK_PRESSURE_THRESHOLDS.soft_available_bytes
        || ratio <= DISK_PRESSURE_THRESHOLDS.soft_available_ratio
    {
        return DiskPressureLevel::Soft;
    }
    if previous == DiskPressureLevel::Soft
        && (available_bytes <= DISK_PRESSURE_THRESHOLDS.soft_recovery_bytes
            || ratio <= DISK_PRESSURE_THRESHOLDS.soft_recovery_ratio)
    {
        return DiskPressureLevel::Soft;
    }
    DiskPressureLevel::Healthy
}

#[cfg(unix)]
fn volume_device_id(path: &Path, metadata: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    let _ = path;
    metadata.dev().to_string()
}

#[cfg(not(unix))]
fn volume_device_id(path: &Path, _metadata: &fs::Metadata) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

#[cfg(unix)]
fn statvfs_snapshot(root: &DiskVolumeRoot) -> std::io::Result<DiskVolumeSnapshot> {
    use std::os::unix::ffi::OsStrExt;
    let metadata = fs::metadata(&root.path)?;
    let c_path = CString::new(root.path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "volume path contains NUL"))?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let block_size = if stats.f_frsize > 0 { stats.f_frsize } else { stats.f_bsize } as u64;
    Ok(DiskVolumeSnapshot {
        volume: root.volume.clone(),
        device_id: volume_device_id(&root.path, &metadata),
        total_bytes: (stats.f_blocks as u64).saturating_mul(block_size),
        available_bytes: (stats.f_bavail as u64).saturating_mul(block_size),
    })
}

#[cfg(not(unix))]
fn statvfs_snapshot(_root: &DiskVolumeRoot) -> std::io::Result<DiskVolumeSnapshot> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "filesystem capacity sampling is unsupported on this platform",
    ))
}

pub fn read_disk_volume_snapshots(roots: &[DiskVolumeRoot]) -> DiskVolumeSample {
    let mut snapshots = Vec::new();
    let mut seen_devices = HashSet::new();
    let mut complete = true;
    for root in roots {
        match statvfs_snapshot(root) {
            Ok(snapshot) => {
                if seen_devices.insert(snapshot.device_id.clone()) {
                    snapshots.push(snapshot);
                }
            }
            Err(_) => complete = false,
        }
    }
    DiskVolumeSample { snapshots, complete }
}

fn monotonic_now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub type DiskVolumeReader =
    Arc<dyn Fn() -> Result<DiskVolumeSample, String> + Send + Sync + 'static>;
pub type DiskPressureReporter =
    Arc<dyn Fn(&DiskPressureReport) + Send + Sync + 'static>;
pub type DiskPressureChangeListener =
    Arc<dyn Fn(Option<DiskPressureLevel>) + Send + Sync + 'static>;
pub type DiskPressureSampleListener =
    Arc<dyn Fn(DiskPressureLevel, bool) + Send + Sync + 'static>;
pub type DiskPressureLogger = Arc<dyn Fn(&str) + Send + Sync + 'static>;
pub type DiskPressureClock = Arc<dyn Fn() -> u64 + Send + Sync + 'static>;

pub struct DiskPressureGuardOptions {
    pub read_volumes: DiskVolumeReader,
    pub report: DiskPressureReporter,
    pub on_pressure_change: DiskPressureChangeListener,
    pub on_successful_sample: Option<DiskPressureSampleListener>,
    pub log: DiskPressureLogger,
    pub clock: Option<DiskPressureClock>,
}

pub struct DiskPressureGuard {
    options: DiskPressureGuardOptions,
    states: Mutex<HashMap<String, DiskPressureState>>,
    aggregate: Mutex<DiskPressureLevel>,
    in_flight: AtomicBool,
    disposed: AtomicBool,
}

impl DiskPressureGuard {
    pub fn new(options: DiskPressureGuardOptions) -> Self {
        Self {
            options,
            states: Mutex::new(HashMap::new()),
            aggregate: Mutex::new(DiskPressureLevel::Healthy),
            in_flight: AtomicBool::new(false),
            disposed: AtomicBool::new(false),
        }
    }

    pub fn on_tick(&self) {
        if self.disposed.load(Ordering::Acquire)
            || self.in_flight.swap(true, Ordering::AcqRel)
        {
            return;
        }
        self.check();
        self.in_flight.store(false, Ordering::Release);
    }

    fn check(&self) {
        let checked_at_ms = self.options.clock.as_ref()
            .map(|clock| clock())
            .unwrap_or_else(monotonic_now_ms);
        let sample = match (self.options.read_volumes)() {
            Ok(sample) => sample,
            Err(error) => {
                (self.options.log)(&format!("disk-pressure sample failed: {error}"));
                return;
            }
        };
        let mut states = match self.states.lock() {
            Ok(states) => states,
            Err(_) => {
                (self.options.log)("disk-pressure state mutex poisoned");
                return;
            }
        };
        let mut sampled = HashSet::new();
        for snapshot in &sample.snapshots {
            if self.disposed.load(Ordering::Acquire) {
                return;
            }
            sampled.insert(snapshot.device_id.clone());
            let previous = states.get(&snapshot.device_id).copied();
            let previous_level = previous.map(|state| state.level)
                .unwrap_or(DiskPressureLevel::Healthy);
            let level = classify_disk_pressure(
                snapshot.total_bytes,
                snapshot.available_bytes,
                previous_level,
            );
            let transitioned = level != previous_level;
            let heartbeat_due = level != DiskPressureLevel::Healthy
                && checked_at_ms.saturating_sub(
                    previous.map(|state| state.last_reported_at_ms).unwrap_or(0),
                ) >= DISK_PRESSURE_HEARTBEAT_MS;
            let should_report = transitioned || heartbeat_due;
            states.insert(
                snapshot.device_id.clone(),
                DiskPressureState {
                    level,
                    last_reported_at_ms: if should_report {
                        checked_at_ms
                    } else {
                        previous.map(|state| state.last_reported_at_ms).unwrap_or(checked_at_ms)
                    },
                },
            );
            if should_report {
                let used_percent = if snapshot.total_bytes > 0 {
                    snapshot.total_bytes.saturating_sub(snapshot.available_bytes) as f64
                        / snapshot.total_bytes as f64 * 100.0
                } else {
                    0.0
                };
                (self.options.report)(&DiskPressureReport {
                    volume: snapshot.volume.clone(),
                    device_id: snapshot.device_id.clone(),
                    total_bytes: snapshot.total_bytes,
                    available_bytes: snapshot.available_bytes,
                    level,
                    trigger: if transitioned {
                        DiskPressureTrigger::Transition
                    } else {
                        DiskPressureTrigger::Heartbeat
                    },
                    used_percent,
                });
            }
        }
        if sample.complete {
            states.retain(|device_id, _| sampled.contains(device_id));
        }
        let levels = states.iter()
            .map(|(id, state)| (id.clone(), state.level))
            .collect::<HashMap<_, _>>();
        let next = aggregate_disk_pressure_level(&levels);
        drop(states);
        if sample.complete || !sample.snapshots.is_empty() {
            if let Some(listener) = &self.options.on_successful_sample {
                listener(next, sample.complete);
            }
        }
        let changed = match self.aggregate.lock() {
            Ok(mut aggregate) if *aggregate != next => {
                *aggregate = next;
                true
            }
            _ => false,
        };
        if changed {
            (self.options.on_pressure_change)(match next {
                DiskPressureLevel::Healthy => None,
                level => Some(level),
            });
        }
    }

    pub fn dispose(&self) {
        self.disposed.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn gib(value: u64) -> u64 { value * GIB }

    #[test]
    fn thresholds_preserve_desktop_hysteresis() {
        assert_eq!(
            classify_disk_pressure(gib(100), gib(8), DiskPressureLevel::Healthy),
            DiskPressureLevel::Soft
        );
        assert_eq!(
            classify_disk_pressure(gib(100), gib(2), DiskPressureLevel::Soft),
            DiskPressureLevel::Hard
        );
        assert_eq!(
            classify_disk_pressure(gib(100), gib(3), DiskPressureLevel::Hard),
            DiskPressureLevel::Hard
        );
        assert_eq!(
            classify_disk_pressure(gib(100), gib(9), DiskPressureLevel::Soft),
            DiskPressureLevel::Soft
        );
        assert_eq!(
            classify_disk_pressure(gib(100), gib(21), DiskPressureLevel::Soft),
            DiskPressureLevel::Healthy
        );
    }

    #[test]
    fn guard_reports_transition_then_heartbeat_without_spinning() {
        let samples = Arc::new(Mutex::new(VecDeque::from([
            DiskVolumeSample {
                snapshots: vec![DiskVolumeSnapshot {
                    volume: "app_data".into(),
                    device_id: "disk".into(),
                    total_bytes: gib(100),
                    available_bytes: gib(7),
                }],
                complete: true,
            },
            DiskVolumeSample {
                snapshots: vec![DiskVolumeSnapshot {
                    volume: "app_data".into(),
                    device_id: "disk".into(),
                    total_bytes: gib(100),
                    available_bytes: gib(7),
                }],
                complete: true,
            },
        ])));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let clock = Arc::new(AtomicU64::new(1));
        let sample_source = Arc::clone(&samples);
        let report_sink = Arc::clone(&reports);
        let clock_source = Arc::clone(&clock);
        let guard = DiskPressureGuard::new(DiskPressureGuardOptions {
            read_volumes: Arc::new(move || {
                sample_source.lock().unwrap().pop_front()
                    .ok_or_else(|| "no sample".to_string())
            }),
            report: Arc::new(move |report| {
                report_sink.lock().unwrap().push(report.trigger);
            }),
            on_pressure_change: Arc::new(|_| {}),
            on_successful_sample: None,
            log: Arc::new(|_| {}),
            clock: Some(Arc::new(move || clock_source.load(Ordering::SeqCst))),
        });
        guard.on_tick();
        clock.store(DISK_PRESSURE_HEARTBEAT_MS + 2, Ordering::SeqCst);
        guard.on_tick();
        assert_eq!(
            reports.lock().unwrap().as_slice(),
            &[DiskPressureTrigger::Transition, DiskPressureTrigger::Heartbeat]
        );
    }
}
