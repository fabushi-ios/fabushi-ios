use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub type ChangeListener = Arc<dyn Fn() + Send + Sync + 'static>;
type FailureReporter = Arc<dyn Fn(String) + Send + Sync + 'static>;

struct WatchedDirectoryInner {
    root: PathBuf,
    debounce_ms: u64,
    listener: Mutex<Option<ChangeListener>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    generation: AtomicU64,
    report_failure: FailureReporter,
}

#[derive(Clone)]
pub struct WatchedDirectory {
    inner: Arc<WatchedDirectoryInner>,
}

impl std::fmt::Debug for WatchedDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WatchedDirectory")
            .field("root", &self.inner.root)
            .field("debounce_ms", &self.inner.debounce_ms)
            .finish_non_exhaustive()
    }
}

impl WatchedDirectory {
    pub fn new(root: impl Into<PathBuf>, debounce_ms: u64) -> Self {
        Self::with_reporter(root, debounce_ms, Arc::new(|message| {
            eprintln!("[watched-directory] {message}");
        }))
    }

    pub fn with_reporter(
        root: impl Into<PathBuf>,
        debounce_ms: u64,
        report_failure: FailureReporter,
    ) -> Self {
        Self {
            inner: Arc::new(WatchedDirectoryInner {
                root: root.into(),
                debounce_ms,
                listener: Mutex::new(None),
                watcher: Mutex::new(None),
                generation: AtomicU64::new(0),
                report_failure,
            }),
        }
    }

    pub fn get_location(&self) -> &Path {
        &self.inner.root
    }

    pub fn list_subdirectory_names(&self) -> Vec<String> {
        let mut names = fs::read_dir(&self.inner.root)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    pub fn set_on_change(&self, listener: Option<ChangeListener>) -> Result<(), String> {
        if listener.is_none() {
            *self
                .inner
                .listener
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            self.stop_watching();
            return Ok(());
        }

        *self
            .inner
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = listener;
        if let Err(error) = self.start_watching() {
            *self
                .inner
                .listener
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            return Err(error);
        }
        Ok(())
    }

    pub fn start_watching(&self) -> Result<(), String> {
        let mut slot = self
            .inner
            .watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return Ok(());
        }

        fs::create_dir_all(&self.inner.root).map_err(|error| error.to_string())?;
        let weak = Arc::downgrade(&self.inner);
        let reporter = Arc::clone(&self.inner.report_failure);
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    schedule_notify(&weak);
                }
                Ok(_) => {}
                Err(error) => reporter(error.to_string()),
            },
            Config::default(),
        )
        .map_err(|error| error.to_string())?;
        watcher
            .watch(&self.inner.root, RecursiveMode::Recursive)
            .map_err(|error| error.to_string())?;
        *slot = Some(watcher);
        Ok(())
    }

    pub fn stop_watching(&self) {
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self
            .inner
            .watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }

    pub fn schedule_notify(&self) {
        schedule_notify(&Arc::downgrade(&self.inner));
    }

    pub fn write_file_atomic(&self, path: impl AsRef<Path>, contents: &[u8]) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = PathBuf::from(format!(
            "{}.{}.tmp",
            path.display(),
            std::process::id()
        ));
        fs::write(&temporary, contents)?;
        match fs::rename(&temporary, path) {
            Ok(()) => {}
            Err(first) if path.exists() => {
                fs::remove_file(path)?;
                fs::rename(&temporary, path).map_err(|_| first)?;
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        }
        self.schedule_notify();
        Ok(())
    }
}

impl Drop for WatchedDirectoryInner {
    fn drop(&mut self) {
        let _ = self
            .watcher
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

fn schedule_notify(weak: &Weak<WatchedDirectoryInner>) {
    let Some(inner) = weak.upgrade() else {
        return;
    };
    let generation = inner.generation.fetch_add(1, Ordering::AcqRel) + 1;
    let debounce_ms = inner.debounce_ms;
    let weak = Arc::downgrade(&inner);
    thread::spawn(move || {
        if debounce_ms > 0 {
            thread::sleep(Duration::from_millis(debounce_ms));
        }
        let Some(inner) = weak.upgrade() else {
            return;
        };
        if inner.generation.load(Ordering::Acquire) != generation {
            return;
        }
        let listener = inner
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(listener) = listener {
            listener();
        }
    });
}
