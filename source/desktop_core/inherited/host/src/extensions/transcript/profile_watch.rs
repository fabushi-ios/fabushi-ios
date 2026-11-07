use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, RecvTimeoutError},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{Value, json};

use crate::agents::agent_avatar::{
    invalidate_avatar_data_url_cache, is_conventional_avatar_filename,
};
use crate::agents::agent_profile::{
    SAND_PROFILE_FILENAME, get_sand_profile_path, read_sand_profile_file,
};
use crate::agents::settings_file::{
    SAND_SETTINGS_FILENAME, get_sand_settings_path,
};
use crate::extensions::session::agent_session::SandAgentSessionStore;
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::extensions::session::session_paths::ACTIVE_AGENT_FILENAME;

use super::roster_emit::ProductionRosterEmit;

pub const PROFILE_WATCH_DEBOUNCE_MS: u64 = 50;
pub const SAND_DEFAULT_AGENT_NAME: &str = "Grok";

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileWatchEvent {
    Timeline {
        agent_id: String,
        event: Value,
    },
    ProfileChanged {
        agent_id: String,
    },
}

pub type ProfileWatchEventSink = Arc<dyn Fn(ProfileWatchEvent) + Send + Sync + 'static>;

#[derive(Debug, Default)]
pub struct ProfileWatchCoalescer {
    pending_agent_id: Option<String>,
    deadline: Option<Instant>,
}

impl ProfileWatchCoalescer {
    pub fn schedule(&mut self, agent_id: &str, now: Instant) {
        self.pending_agent_id = Some(agent_id.to_string());
        if self.deadline.is_none() {
            self.deadline =
                Some(now + Duration::from_millis(PROFILE_WATCH_DEBOUNCE_MS));
        }
    }

    pub fn clear(&mut self) {
        self.pending_agent_id = None;
        self.deadline = None;
    }

    pub fn switch_agent(
        &mut self,
        current: &mut Option<String>,
        next: Option<String>,
    ) -> bool {
        if *current == next {
            return false;
        }
        self.clear();
        *current = next;
        true
    }

    pub fn recv_timeout(&self, now: Instant) -> Duration {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(now))
            .unwrap_or_else(|| Duration::from_millis(PROFILE_WATCH_DEBOUNCE_MS))
    }

    pub fn take_due(&mut self, now: Instant) -> Option<String> {
        let deadline = self.deadline?;
        if now < deadline {
            return None;
        }
        self.deadline = None;
        self.pending_agent_id.take()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDisplayProfile {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAgentProfile {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    pub settings_file_path: PathBuf,
}

pub struct ProductionProfileWatch {
    _watcher: RecommendedWatcher,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    watched_agent_id: Arc<Mutex<Option<String>>>,
}

impl ProductionProfileWatch {
    pub fn start(
        sessions: Arc<ProductionSessionWorkers>,
        roster_emit: Arc<ProductionRosterEmit>,
        event_sink: ProfileWatchEventSink,
    ) -> notify::Result<Self> {
        let agents_root = sessions.agents_root().to_path_buf();
        let _ = fs::create_dir_all(&agents_root);
        let (event_tx, event_rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = event_tx.send(event);
        })?;
        watcher.watch(&agents_root, RecursiveMode::Recursive)?;

        let watched_agent_id = Arc::new(Mutex::new(
            SandAgentSessionStore::new(Arc::clone(&sessions)).read_active_agent_id(),
        ));
        let last_known_names = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        if let Some(agent_id) = watched_agent_id
            .lock()
            .ok()
            .and_then(|value| value.clone())
        {
            seed_known_agent_name(&sessions, &last_known_names, &agent_id);
        }

        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_watched = Arc::clone(&watched_agent_id);
        let worker_names = Arc::clone(&last_known_names);
        let worker_sessions = Arc::clone(&sessions);
        let worker_roster = Arc::clone(&roster_emit);
        let worker_events = Arc::clone(&event_sink);
        let worker_root = agents_root.clone();

        let worker = thread::spawn(move || {
            let mut coalescer = ProfileWatchCoalescer::default();
            while !worker_stop.load(Ordering::Acquire) {
                if let Some(agent_id) = coalescer.take_due(Instant::now()) {
                    publish_profile_watch_update(
                        &worker_sessions,
                        &worker_names,
                        &worker_roster,
                        &worker_events,
                        &worker_root,
                        &agent_id,
                    );
                    continue;
                }
                match event_rx.recv_timeout(coalescer.recv_timeout(Instant::now())) {
                    Ok(Ok(event)) => {
                        for path in event.paths {
                            if path == worker_root.join(ACTIVE_AGENT_FILENAME) {
                                let active = SandAgentSessionStore::new(Arc::clone(&worker_sessions))
                                    .read_active_agent_id();
                                let changed = worker_watched
                                    .lock()
                                    .map(|mut watched| {
                                        coalescer.switch_agent(&mut watched, active.clone())
                                    })
                                    .unwrap_or(false);
                                if changed {
                                    if let Some(agent_id) = active {
                                        seed_known_agent_name(
                                            &worker_sessions,
                                            &worker_names,
                                            &agent_id,
                                        );
                                    }
                                }
                                continue;
                            }
                            let watched = worker_watched
                                .lock()
                                .ok()
                                .and_then(|value| value.clone());
                            let Some(watched) = watched else {
                                continue;
                            };
                            if watched_profile_path_agent_id(&worker_root, &path).as_deref()
                                == Some(watched.as_str())
                            {
                                coalescer.schedule(&watched, Instant::now());
                            }
                        }
                    }
                    Ok(Err(_)) => {}
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        Ok(Self {
            _watcher: watcher,
            stop,
            worker: Some(worker),
            watched_agent_id,
        })
    }

    pub fn watched_agent_id(&self) -> Option<String> {
        self.watched_agent_id
            .lock()
            .ok()
            .and_then(|value| value.clone())
    }
}

impl Drop for ProductionProfileWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn is_profile_watch_filename(name: &str) -> bool {
    name == SAND_PROFILE_FILENAME
        || name == SAND_SETTINGS_FILENAME
        || is_conventional_avatar_filename(name)
}

pub fn watched_profile_path_agent_id(agents_root: &Path, path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if !is_profile_watch_filename(name) {
        return None;
    }
    let agent_dir = path.parent()?;
    if agent_dir.parent()? != agents_root {
        return None;
    }
    agent_dir.file_name()?.to_str().map(ToOwned::to_owned)
}

pub fn get_agent_display_profile(
    sessions: &Arc<ProductionSessionWorkers>,
    agent_id: &str,
) -> Option<AgentDisplayProfile> {
    let dir = sessions.agents_root().join(agent_id);
    if !dir.is_dir() {
        return None;
    }
    let profile = read_sand_profile_file(get_sand_profile_path(&dir));
    Some(AgentDisplayProfile {
        name: profile
            .as_ref()
            .map(|profile| profile.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(SAND_DEFAULT_AGENT_NAME)
            .to_string(),
        description: profile
            .map(|profile| profile.description)
            .unwrap_or_default(),
    })
}

pub fn resolve_agent_profile(db_path: &Path) -> ResolvedAgentProfile {
    let dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let file_path = get_sand_profile_path(dir);
    let profile = read_sand_profile_file(&file_path);
    ResolvedAgentProfile {
        name: profile
            .as_ref()
            .map(|profile| profile.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(SAND_DEFAULT_AGENT_NAME)
            .to_string(),
        description: profile
            .map(|profile| profile.description)
            .unwrap_or_default(),
        file_path,
        settings_file_path: get_sand_settings_path(dir),
    }
}

pub fn read_profile_watch_name(
    sessions: &Arc<ProductionSessionWorkers>,
    agent_id: &str,
) -> Option<String> {
    SandAgentSessionStore::new(Arc::clone(sessions))
        .get_agent_profile_text(agent_id)
        .ok()
        .flatten()
        .map(|profile| profile.name.trim().to_string())
        .filter(|name| !name.is_empty())
}

pub fn profile_watch_events(
    agent_id: &str,
    name_change: Option<(&str, &str)>,
) -> Vec<ProfileWatchEvent> {
    let mut events = Vec::with_capacity(if name_change.is_some() { 2 } else { 1 });
    if let Some((from, to)) = name_change {
        if from != to {
            events.push(ProfileWatchEvent::Timeline {
                agent_id: agent_id.to_string(),
                event: json!({
                    "type": "name-changed",
                    "from": from,
                    "to": to,
                }),
            });
        }
    }
    events.push(ProfileWatchEvent::ProfileChanged {
        agent_id: agent_id.to_string(),
    });
    events
}

fn seed_known_agent_name(
    sessions: &Arc<ProductionSessionWorkers>,
    names: &Arc<Mutex<HashMap<String, String>>>,
    agent_id: &str,
) {
    if names
        .lock()
        .map(|names| names.contains_key(agent_id))
        .unwrap_or(false)
    {
        return;
    }
    let Some(name) = read_profile_watch_name(sessions, agent_id) else {
        return;
    };
    if let Ok(mut names) = names.lock() {
        names.entry(agent_id.to_string()).or_insert(name);
    }
}

fn record_name_change(
    sessions: &Arc<ProductionSessionWorkers>,
    names: &Arc<Mutex<HashMap<String, String>>>,
    agent_id: &str,
) -> Option<(String, String)> {
    let current = read_profile_watch_name(sessions, agent_id)?;
    let previous = names
        .lock()
        .ok()
        .and_then(|mut names| names.insert(agent_id.to_string(), current.clone()));
    match previous {
        Some(previous) if previous != current => Some((previous, current)),
        _ => None,
    }
}

fn publish_profile_watch_update(
    sessions: &Arc<ProductionSessionWorkers>,
    names: &Arc<Mutex<HashMap<String, String>>>,
    roster_emit: &Arc<ProductionRosterEmit>,
    event_sink: &ProfileWatchEventSink,
    agents_root: &Path,
    agent_id: &str,
) {
    invalidate_avatar_data_url_cache(&agents_root.join(agent_id));
    let _ = roster_emit.emit_agent_update(agent_id);
    let name_change = record_name_change(sessions, names, agent_id);
    let name_change = name_change
        .as_ref()
        .map(|(from, to)| (from.as_str(), to.as_str()));
    for event in profile_watch_events(agent_id, name_change) {
        (event_sink)(event);
    }
}
