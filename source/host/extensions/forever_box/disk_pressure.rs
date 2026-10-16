use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::disk_pressure_guard::{
    DiskPressureChangeListener, DiskPressureGuard, DiskPressureGuardOptions,
    DiskPressureLevel, DiskPressureLogger, DiskPressureReporter,
    DiskPressureSampleListener, DiskVolumeRoot, read_disk_volume_snapshots,
};

pub const SAND_DISK_PRESSURE_REMINDERS_FILE_NAME: &str =
    "host-disk-pressure-reminders.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LedgerPending {
    agent_id: String,
    episode_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReminderLedger {
    version: u32,
    active_episode_id: Option<String>,
    handled_agent_ids: Vec<String>,
    pending: Vec<LedgerPending>,
}

#[derive(Debug, Clone)]
struct ReminderClaim {
    claim_id: String,
    episode_id: String,
}

#[derive(Debug, Default)]
struct ReminderState {
    active_episode_id: Option<String>,
    restored_episode_awaiting_pressure: bool,
    active_episode_persistence_pending: bool,
    handled_agent_ids: BTreeSet<String>,
    pending_episode_ids: BTreeMap<String, Vec<String>>,
    claims: BTreeMap<String, ReminderClaim>,
}

pub type DiskPressureLedgerErrorReporter =
    Arc<dyn Fn(&str) + Send + Sync + 'static>;
pub type EpisodeIdFactory = Arc<dyn Fn() -> String + Send + Sync + 'static>;

pub struct DiskPressureReminderEpisodes {
    file_path: PathBuf,
    create_episode_id: EpisodeIdFactory,
    on_ledger_error: Option<DiskPressureLedgerErrorReporter>,
    state: Mutex<ReminderState>,
}

impl DiskPressureReminderEpisodes {
    pub fn new(
        root_dir: &Path,
        create_episode_id: Option<EpisodeIdFactory>,
        on_ledger_error: Option<DiskPressureLedgerErrorReporter>,
    ) -> Self {
        let file_path = root_dir.join(SAND_DISK_PRESSURE_REMINDERS_FILE_NAME);
        let mut state = ReminderState::default();
        match fs::read(&file_path) {
            Ok(bytes) => match serde_json::from_slice::<ReminderLedger>(&bytes) {
                Ok(ledger) if ledger.version == 1 => {
                    state.active_episode_id = ledger.active_episode_id;
                    state.restored_episode_awaiting_pressure =
                        state.active_episode_id.is_some();
                    if state.active_episode_id.is_some() {
                        state.handled_agent_ids.extend(ledger.handled_agent_ids);
                    }
                    for item in ledger.pending {
                        Self::enqueue_locked(&mut state, &item.agent_id, &item.episode_id);
                    }
                }
                Ok(_) | Err(_) => {
                    if let Some(report) = &on_ledger_error {
                        report("invalid disk-pressure reminder ledger");
                    }
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                if let Some(report) = &on_ledger_error {
                    report(&error.to_string());
                }
            }
        }
        Self {
            file_path,
            create_episode_id: create_episode_id
                .unwrap_or_else(|| Arc::new(|| uuid::Uuid::new_v4().to_string())),
            on_ledger_error,
            state: Mutex::new(state),
        }
    }

    fn enqueue_locked(state: &mut ReminderState, agent_id: &str, episode_id: &str) -> bool {
        let pending = state.pending_episode_ids.entry(agent_id.to_string()).or_default();
        if pending.iter().any(|value| value == episode_id) {
            return false;
        }
        pending.push(episode_id.to_string());
        true
    }

    fn report_ledger_error(&self, error: impl ToString) {
        if let Some(report) = &self.on_ledger_error {
            report(&error.to_string());
        }
    }

    fn persist_locked(&self, state: &ReminderState) -> bool {
        if state.active_episode_id.is_none() && state.pending_episode_ids.is_empty() {
            match fs::remove_file(&self.file_path) {
                Ok(()) => return true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
                Err(error) => {
                    self.report_ledger_error(error);
                    return false;
                }
            }
        }
        let ledger = ReminderLedger {
            version: 1,
            active_episode_id: state.active_episode_id.clone(),
            handled_agent_ids: if state.active_episode_id.is_some() {
                state.handled_agent_ids.iter().cloned().collect()
            } else {
                Vec::new()
            },
            pending: state.pending_episode_ids.iter().flat_map(|(agent_id, ids)| {
                ids.iter().map(move |episode_id| LedgerPending {
                    agent_id: agent_id.clone(),
                    episode_id: episode_id.clone(),
                })
            }).collect(),
        };
        let bytes = match serde_json::to_vec(&ledger) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.report_ledger_error(error);
                return false;
            }
        };
        let Some(parent) = self.file_path.parent() else {
            self.report_ledger_error("disk-pressure reminder ledger has no parent directory");
            return false;
        };
        if let Err(error) = fs::create_dir_all(parent) {
            self.report_ledger_error(error);
            return false;
        }
        let part = PathBuf::from(format!("{}.part", self.file_path.display()));
        let write_result = (|| -> std::io::Result<()> {
            let mut file = File::create(&part)?;
            file.write_all(&bytes)?;
            file.flush()?;
            file.sync_all()?;
            fs::rename(&part, &self.file_path)
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&part);
            self.report_ledger_error(error);
            return false;
        }
        true
    }

    pub fn claim(&self, agent_id: &str, claim_id: &str) -> Option<String> {
        let mut state = self.state.lock().ok()?;
        if let Some(existing) = state.claims.get(agent_id) {
            return (existing.claim_id == claim_id)
                .then(|| existing.episode_id.clone());
        }
        if let Some(active_episode_id) = state.active_episode_id.clone() {
            if !state.handled_agent_ids.contains(agent_id)
                && Self::enqueue_locked(&mut state, agent_id, &active_episode_id)
            {
                self.persist_locked(&state);
            }
        }
        let episode_id = state.pending_episode_ids.get(agent_id)
            .and_then(|pending| pending.first()).cloned()?;
        state.claims.insert(agent_id.to_string(), ReminderClaim {
            claim_id: claim_id.to_string(),
            episode_id: episode_id.clone(),
        });
        Some(episode_id)
    }

    pub fn release(&self, agent_id: &str, claim_id: &str) {
        if let Ok(mut state) = self.state.lock() {
            if state.claims.get(agent_id)
                .is_some_and(|claim| claim.claim_id == claim_id)
            {
                state.claims.remove(agent_id);
            }
        }
    }

    pub fn enroll<'a>(&self, agent_ids: impl IntoIterator<Item = &'a str>) {
        let Ok(mut state) = self.state.lock() else { return };
        let Some(active_episode_id) = state.active_episode_id.clone() else { return };
        let mut changed = false;
        for agent_id in agent_ids {
            if !state.handled_agent_ids.contains(agent_id) {
                changed = Self::enqueue_locked(&mut state, agent_id, &active_episode_id) || changed;
            }
        }
        if changed {
            self.persist_locked(&state);
        }
    }

    pub fn commit(&self, agent_id: &str, claim_id: &str) -> bool {
        let Ok(mut state) = self.state.lock() else { return false };
        let Some(claim) = state.claims.get(agent_id).cloned() else { return false };
        let Some(pending) = state.pending_episode_ids.get(agent_id).cloned() else {
            return false;
        };
        if claim.claim_id != claim_id
            || pending.first().is_none_or(|episode| episode != &claim.episode_id)
        {
            return false;
        }
        let remaining = pending.iter().skip(1).cloned().collect::<Vec<_>>();
        if remaining.is_empty() {
            state.pending_episode_ids.remove(agent_id);
        } else {
            state.pending_episode_ids.insert(agent_id.to_string(), remaining);
        }
        let already_handled = state.handled_agent_ids.contains(agent_id);
        if state.active_episode_id.as_deref() == Some(claim.episode_id.as_str()) {
            state.handled_agent_ids.insert(agent_id.to_string());
        }
        if !self.persist_locked(&state) {
            state.pending_episode_ids.insert(agent_id.to_string(), pending);
            if !already_handled {
                state.handled_agent_ids.remove(agent_id);
            }
            return false;
        }
        state.claims.remove(agent_id);
        true
    }

    pub fn forget_agent(&self, agent_id: &str) {
        let Ok(mut state) = self.state.lock() else { return };
        state.claims.remove(agent_id);
        let changed = state.pending_episode_ids.remove(agent_id).is_some()
            || state.handled_agent_ids.remove(agent_id);
        if changed {
            self.persist_locked(&state);
        }
    }

    pub fn observe_pressure(&self, level: DiskPressureLevel, complete: bool) {
        let Ok(mut state) = self.state.lock() else { return };
        if level == DiskPressureLevel::Healthy {
            if state.active_episode_id.is_none()
                || (!complete && state.restored_episode_awaiting_pressure)
            {
                return;
            }
        } else if state.active_episode_id.is_some() {
            state.restored_episode_awaiting_pressure = false;
            if state.active_episode_persistence_pending && self.persist_locked(&state) {
                state.active_episode_persistence_pending = false;
            }
            return;
        }
        let old_active = state.active_episode_id.clone();
        let old_restored = state.restored_episode_awaiting_pressure;
        let old_handled = state.handled_agent_ids.clone();
        state.active_episode_id = if level == DiskPressureLevel::Healthy {
            None
        } else {
            Some((self.create_episode_id)())
        };
        state.restored_episode_awaiting_pressure = false;
        state.handled_agent_ids.clear();
        if self.persist_locked(&state) {
            state.active_episode_persistence_pending = false;
            return;
        }
        if level != DiskPressureLevel::Healthy {
            state.active_episode_persistence_pending = true;
            return;
        }
        state.active_episode_id = old_active;
        state.restored_episode_awaiting_pressure = old_restored;
        state.handled_agent_ids = old_handled;
    }
}

pub type DiskPressureWatchListener =
    Arc<dyn Fn(Option<DiskPressureLevel>) + Send + Sync + 'static>;

pub struct DiskPressureWatchDeps {
    pub root_dir: PathBuf,
    pub polling_interval: Duration,
    pub report: DiskPressureReporter,
    pub log: DiskPressureLogger,
}

pub struct DiskPressureWatch {
    level: Arc<Mutex<Option<DiskPressureLevel>>>,
    reminder_episodes: Arc<DiskPressureReminderEpisodes>,
    listeners: Arc<Mutex<BTreeMap<u64, DiskPressureWatchListener>>>,
    next_listener_id: Mutex<u64>,
    guard: Arc<DiskPressureGuard>,
    stop_tx: mpsc::Sender<()>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl DiskPressureWatch {
    pub fn level(&self) -> Option<DiskPressureLevel> {
        self.level.lock().ok().and_then(|level| *level)
    }

    pub fn reminder_episodes(&self) -> Arc<DiskPressureReminderEpisodes> {
        Arc::clone(&self.reminder_episodes)
    }

    pub fn subscribe(&self, listener: DiskPressureWatchListener) -> u64 {
        let mut next = self.next_listener_id.lock().expect("disk-pressure listener id");
        *next = next.saturating_add(1);
        let id = *next;
        self.listeners.lock().expect("disk-pressure listeners").insert(id, listener);
        id
    }

    pub fn unsubscribe(&self, subscription: u64) {
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.remove(&subscription);
        }
    }

    pub fn dispose(&self) {
        self.guard.dispose();
        let _ = self.stop_tx.send(());
        if let Some(worker) = self.worker.lock().ok().and_then(|mut worker| worker.take()) {
            let _ = worker.join();
        }
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.clear();
        }
    }
}

pub fn start_disk_pressure_watch(deps: DiskPressureWatchDeps) -> DiskPressureWatch {
    let reminder_episodes = Arc::new(DiskPressureReminderEpisodes::new(
        &deps.root_dir,
        None,
        Some({
            let log = Arc::clone(&deps.log);
            Arc::new(move |error| log(&format!("disk-pressure reminder ledger failed: {error}")))
        }),
    ));
    let level = Arc::new(Mutex::new(None));
    let listeners = Arc::new(Mutex::new(BTreeMap::<u64, DiskPressureWatchListener>::new()));
    let roots = vec![
        DiskVolumeRoot::new("app_data", deps.root_dir.clone()),
        DiskVolumeRoot::new("temp", std::env::temp_dir()),
    ];
    let level_for_change = Arc::clone(&level);
    let listeners_for_change = Arc::clone(&listeners);
    let on_pressure_change: DiskPressureChangeListener = Arc::new(move |next| {
        if let Ok(mut slot) = level_for_change.lock() {
            *slot = next;
        }
        let listeners = listeners_for_change.lock()
            .map(|listeners| listeners.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for listener in listeners {
            listener(next);
        }
    });
    let reminders_for_sample = Arc::clone(&reminder_episodes);
    let on_successful_sample: DiskPressureSampleListener = Arc::new(move |next, complete| {
        reminders_for_sample.observe_pressure(next, complete);
    });
    let guard = Arc::new(DiskPressureGuard::new(DiskPressureGuardOptions {
        read_volumes: Arc::new(move || Ok(read_disk_volume_snapshots(&roots))),
        report: deps.report,
        on_pressure_change,
        on_successful_sample: Some(on_successful_sample),
        log: Arc::clone(&deps.log),
        clock: None,
    }));
    guard.on_tick();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let worker_guard = Arc::clone(&guard);
    let interval = deps.polling_interval;
    let worker_log = Arc::clone(&deps.log);
    let worker = thread::Builder::new()
        .name("fabushi-ios-disk-pressure".into())
        .spawn(move || loop {
            match stop_rx.recv_timeout(interval) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => worker_guard.on_tick(),
            }
        })
        .map_err(|error| {
            worker_log(&format!("disk-pressure worker failed to start: {error}"));
            error
        })
        .ok();
    DiskPressureWatch {
        level,
        reminder_episodes,
        listeners,
        next_listener_id: Mutex::new(0),
        guard,
        stop_tx,
        worker: Mutex::new(worker),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-ios-disk-pressure-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn reminder_episode_survives_restart_and_commits_exactly_once() {
        let root = root();
        fs::create_dir_all(&root).unwrap();
        let ids = Arc::new(AtomicUsize::new(0));
        {
            let ids = Arc::clone(&ids);
            let episodes = DiskPressureReminderEpisodes::new(
                &root,
                Some(Arc::new(move || format!("episode-{}", ids.fetch_add(1, Ordering::SeqCst)))),
                None,
            );
            episodes.observe_pressure(DiskPressureLevel::Soft, true);
            episodes.enroll(["agent-a"].iter().copied());
            assert_eq!(episodes.claim("agent-a", "claim-a").as_deref(), Some("episode-0"));
        }
        {
            let episodes = DiskPressureReminderEpisodes::new(&root, None, None);
            assert_eq!(episodes.claim("agent-a", "claim-b").as_deref(), Some("episode-0"));
            assert!(episodes.commit("agent-a", "claim-b"));
            assert!(episodes.claim("agent-a", "claim-c").is_none());
            episodes.observe_pressure(DiskPressureLevel::Healthy, true);
        }
        assert!(!root.join(SAND_DISK_PRESSURE_REMINDERS_FILE_NAME).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restored_episode_does_not_clear_on_incomplete_healthy_sample() {
        let root = root();
        fs::create_dir_all(&root).unwrap();
        {
            let episodes = DiskPressureReminderEpisodes::new(
                &root,
                Some(Arc::new(|| "episode-restored".into())),
                None,
            );
            episodes.observe_pressure(DiskPressureLevel::Hard, true);
        }
        let episodes = DiskPressureReminderEpisodes::new(&root, None, None);
        episodes.observe_pressure(DiskPressureLevel::Healthy, false);
        assert_eq!(
            episodes.claim("agent-a", "claim-a").as_deref(),
            Some("episode-restored")
        );
        let _ = fs::remove_dir_all(root);
    }
}
