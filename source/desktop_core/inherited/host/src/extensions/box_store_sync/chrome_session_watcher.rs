use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub const DEFAULT_CHROME_SESSION_WATCH_DEBOUNCE_MS: u64 = 250;

type ChangeListener = Arc<dyn Fn() + Send + Sync + 'static>;
type LogListener = Arc<dyn Fn(String) + Send + Sync + 'static>;

struct ChromeSessionWatcherInner {
    watch_dir: PathBuf,
    session_db_names: Vec<String>,
    debounce_ms: u64,
    on_session_change: ChangeListener,
    log: LogListener,
    watcher: Mutex<Option<RecommendedWatcher>>,
    stopped: AtomicBool,
    generation: AtomicU64,
    last_mtimes: Mutex<HashMap<String, u128>>,
}

#[derive(Clone)]
pub struct ChromeSessionWatcher {
    inner: Arc<ChromeSessionWatcherInner>,
}

impl ChromeSessionWatcher {
    pub fn new(
        watch_dir: impl Into<PathBuf>,
        session_db_names: impl IntoIterator<Item = impl Into<String>>,
        debounce_ms: u64,
        on_session_change: ChangeListener,
    ) -> Self {
        Self::with_logger(
            watch_dir,
            session_db_names,
            debounce_ms,
            on_session_change,
            Arc::new(|message| eprintln!("[chrome-session-watcher] {message}")),
        )
    }

    pub fn with_logger(
        watch_dir: impl Into<PathBuf>,
        session_db_names: impl IntoIterator<Item = impl Into<String>>,
        debounce_ms: u64,
        on_session_change: ChangeListener,
        log: LogListener,
    ) -> Self {
        Self {
            inner: Arc::new(ChromeSessionWatcherInner {
                watch_dir: watch_dir.into(),
                session_db_names: session_db_names.into_iter().map(Into::into).collect(),
                debounce_ms,
                on_session_change,
                log,
                watcher: Mutex::new(None),
                stopped: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                last_mtimes: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn start(&self) {
        if self.inner.stopped.load(Ordering::Acquire) {
            return;
        }
        if self.inner.watcher.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
            return;
        }
        self.refresh_session_db_mtimes();

        let weak = Arc::downgrade(&self.inner);
        let log = Arc::clone(&self.inner.log);
        let watcher_result = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => handle_notify_event(&weak, &event.paths),
                Ok(_) => {}
                Err(error) => log(format!("watch event failed (periodic cycle is the backstop): {error}")),
            },
            Config::default(),
        );

        let Ok(mut watcher) = watcher_result else {
            (self.inner.log)("watch could not arm (periodic cycle is the backstop)".into());
            return;
        };
        if let Err(error) = watcher.watch(&self.inner.watch_dir, RecursiveMode::NonRecursive) {
            (self.inner.log)(format!("watch could not arm (periodic cycle is the backstop): {error}"));
            return;
        }

        (self.inner.log)(format!("watching {} for session-db changes", self.inner.watch_dir.display()));
        *self.inner.watcher.lock().unwrap_or_else(|p| p.into_inner()) = Some(watcher);
    }

    pub fn handle_fs_event(&self, filename: Option<&str>) {
        handle_fs_event(&self.inner, filename);
    }

    pub fn is_session_db_file(&self, filename: &str) -> bool {
        self.inner.session_db_names.iter().any(|name| filename.starts_with(name))
    }

    pub fn session_db_changed_since_last_check(&self) -> bool {
        session_db_changed_since_last_check(&self.inner)
    }

    pub fn refresh_session_db_mtimes(&self) {
        let mut last = self.inner.last_mtimes.lock().unwrap_or_else(|p| p.into_inner());
        for name in &self.inner.session_db_names {
            if let Some(mtime) = stat_mtime(&self.inner.watch_dir.join(name)) {
                last.insert(name.clone(), mtime);
            }
        }
    }

    pub fn stop(&self) {
        if self.inner.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.inner.watcher.lock().unwrap_or_else(|p| p.into_inner()).take();
    }

    pub fn is_stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::Acquire)
    }
}

fn handle_notify_event(inner: &Weak<ChromeSessionWatcherInner>, paths: &[PathBuf]) {
    let Some(inner) = inner.upgrade() else { return; };
    if inner.stopped.load(Ordering::Acquire) { return; }
    let mut saw_name = false;
    for path in paths {
        if let Some(name) = path.file_name().and_then(|v| v.to_str()) {
            saw_name = true;
            if inner.session_db_names.iter().any(|db| name.starts_with(db)) {
                schedule_trigger(&inner);
                return;
            }
        }
    }
    if !saw_name && session_db_changed_since_last_check(&inner) {
        schedule_trigger(&inner);
    }
}

fn handle_fs_event(inner: &Arc<ChromeSessionWatcherInner>, filename: Option<&str>) {
    if inner.stopped.load(Ordering::Acquire) { return; }
    match filename {
        Some(name) if inner.session_db_names.iter().any(|db| name.starts_with(db)) => schedule_trigger(inner),
        Some(_) => {}
        None if session_db_changed_since_last_check(inner) => schedule_trigger(inner),
        None => {}
    }
}

fn session_db_changed_since_last_check(inner: &ChromeSessionWatcherInner) -> bool {
    let mut changed = false;
    let mut last = inner.last_mtimes.lock().unwrap_or_else(|p| p.into_inner());
    for name in &inner.session_db_names {
        let Some(mtime) = stat_mtime(&inner.watch_dir.join(name)) else { continue; };
        match last.get(name).copied() {
            Some(previous) if mtime <= previous => {}
            _ => {
                last.insert(name.clone(), mtime);
                changed = true;
            }
        }
    }
    changed
}

fn stat_mtime(path: &Path) -> Option<u128> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    modified.duration_since(UNIX_EPOCH).ok().map(|d| d.as_nanos()).or_else(|| {
        SystemTime::now().duration_since(modified).ok().map(|d| u128::MAX.saturating_sub(d.as_nanos()))
    })
}

fn schedule_trigger(inner: &Arc<ChromeSessionWatcherInner>) {
    if inner.stopped.load(Ordering::Acquire) { return; }
    let generation = inner.generation.fetch_add(1, Ordering::AcqRel) + 1;
    let weak = Arc::downgrade(inner);
    let debounce_ms = inner.debounce_ms;
    thread::spawn(move || {
        if debounce_ms > 0 {
            thread::sleep(Duration::from_millis(debounce_ms));
        }
        let Some(inner) = weak.upgrade() else { return; };
        if inner.stopped.load(Ordering::Acquire) || inner.generation.load(Ordering::Acquire) != generation {
            return;
        }
        let callback = Arc::clone(&inner.on_session_change);
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback())).is_err() {
            (inner.log)("onSessionChange panicked (ignored)".into());
        }
    });
}
