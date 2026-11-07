use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Weak;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(5);

enum LoadStep {
    Loaded(LoadedPlugins),
    Failed,
    Panicked,
}

struct GateLoader {
    entered: Sender<usize>,
    release: Mutex<Receiver<LoadStep>>,
    calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

struct ActiveLoad<'a>(&'a AtomicUsize);
impl Drop for ActiveLoad<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl PluginSkillsLoader for GateLoader {
    fn load(&self) -> Result<LoadedPlugins, String> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let _active = ActiveLoad(&self.active);
        self.max_active.fetch_max(active, Ordering::SeqCst);
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.entered.send(call).unwrap();
        // Release this fixture lock before deliberately unwinding.
        let step = self.release.lock().unwrap().recv_timeout(DEADLINE).unwrap();
        match step {
            LoadStep::Loaded(loaded) => Ok(loaded),
            LoadStep::Failed => Err("fixture loader failure".into()),
            LoadStep::Panicked => panic!("fixture loader panic"),
        }
    }
}

struct TempRoot(PathBuf);
impl TempRoot {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("plugin-sync-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("plugins/cache/market/plugin/skills/test")).unwrap();
        Self(root)
    }

    fn loaded(&self, version: &str) -> LoadedPlugins {
        LoadedPlugins {
            plugins: vec![InstalledPlugin {
                identifier: PluginIdentifier {
                    source: "cursor-third-party".into(),
                    name: "plugin".into(),
                    plugin_db_id: Some("9".into()),
                    version: Some(version.into()),
                },
                display_name: None,
                load_error: None,
                install_path: self.0.join("plugins/cache/market/plugin"),
                skills: vec![InstalledSkill {
                    name: Some("Test".into()),
                    description: None,
                    path: "skills/test/SKILL.md".into(),
                }],
            }],
            auth_blocked: vec![],
            listed_plugin_ids: vec!["9".into()],
            listed_cache_keys: vec![ListedCacheKey {
                marketplace_slug: "market".into(),
                plugin_id: "plugin".into(),
            }],
            publisher_facts: HashMap::new(),
            current_user_id: Some(12),
        }
    }
}
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Call {
    result: Receiver<Result<SyncResult, ()>>,
    thread: JoinHandle<()>,
}
impl Call {
    fn start(service: &Arc<SandPluginSkillsService>, trigger: &'static str) -> Self {
        let service = Arc::clone(service);
        let (tx, result) = mpsc::channel();
        let thread = thread::spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| service.sync(trigger))).map_err(|_| ());
            let _ = tx.send(outcome);
        });
        Self { result, thread }
    }

    fn assert_waiting(&self) {
        assert!(matches!(self.result.try_recv(), Err(TryRecvError::Empty)));
    }

    fn outcome(self) -> Result<SyncResult, ()> {
        let result = self
            .result
            .recv_timeout(DEADLINE)
            .expect("sync did not settle");
        self.thread.join().unwrap();
        result
    }

    fn finish(self) -> SyncResult {
        self.outcome().expect("unexpected sync panic")
    }
}

struct Harness {
    root: TempRoot,
    service: Arc<SandPluginSkillsService>,
    loader: Arc<GateLoader>,
    entered: Receiver<usize>,
    release: Sender<LoadStep>,
    events: Arc<Mutex<Vec<PluginSkillsSyncEvent>>>,
}
impl Harness {
    fn new() -> Self {
        let root = TempRoot::new();
        let (tx, entered) = mpsc::channel();
        let (release, rx) = mpsc::channel();
        let loader = Arc::new(GateLoader {
            entered: tx,
            release: Mutex::new(rx),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let service = Arc::new(
            SandPluginSkillsService::new(&root.0, loader.clone())
                .with_reporter(Arc::new(move |event| captured.lock().unwrap().push(event))),
        );
        Self {
            root,
            service,
            loader,
            entered,
            release,
            events,
        }
    }

    fn call(&self, trigger: &'static str) -> Call {
        Call::start(&self.service, trigger)
    }

    fn enter(&self, pass: usize) {
        assert_eq!(
            self.entered
                .recv_timeout(DEADLINE)
                .expect("loader did not start"),
            pass
        );
    }

    fn pending(&self, trigger: &str) {
        // Wait for actual enrollment, not an assumed OS scheduling delay.
        let until = Instant::now() + DEADLINE;
        let mut state = self.service.state.lock().unwrap();
        loop {
            if state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.trigger == trigger)
            {
                return;
            }
            let remaining = until
                .checked_duration_since(Instant::now())
                .expect("caller did not enroll");
            state = self
                .service
                .changed
                .wait_timeout(state, remaining)
                .unwrap()
                .0;
        }
    }

    fn loaded(&self, version: &str) {
        self.release
            .send(LoadStep::Loaded(self.root.loaded(version)))
            .unwrap();
    }

    fn assert_calls(&self, calls: usize) {
        assert_eq!(self.loader.calls.load(Ordering::SeqCst), calls);
        assert_eq!(self.loader.max_active.load(Ordering::SeqCst), 1);
    }
}

fn assert_version(result: SyncResult, version: &str) {
    let records = result.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].plugin_version, version);
}

#[test]
fn overlapping_callers_wait_for_one_followup_and_last_trigger_wins() {
    let h = Harness::new();
    let first = h.call("startup");
    h.enter(1);
    let second = h.call("install");
    h.pending("install");
    let third = h.call("auth_change");
    h.pending("auth_change");
    second.assert_waiting();
    third.assert_waiting();
    h.loaded("v1");
    assert_version(first.finish(), "v1");
    h.enter(2);
    second.assert_waiting();
    third.assert_waiting();
    h.loaded("v2");
    assert_version(second.finish(), "v2");
    assert_version(third.finish(), "v2");
    h.assert_calls(2);
    let events = h.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.trigger.as_str())
            .collect::<Vec<_>>(),
        vec!["startup", "auth_change"]
    );
}

#[test]
fn followup_failure_reaches_every_waiter_without_replacing_active_success() {
    let h = Harness::new();
    let first = h.call("startup");
    h.enter(1);
    let second = h.call("install");
    h.pending("install");
    let third = h.call("refresh");
    h.pending("refresh");
    h.loaded("v1");
    assert_version(first.finish(), "v1");
    h.enter(2);
    h.release.send(LoadStep::Failed).unwrap();
    assert_eq!(second.finish().unwrap_err(), "fixture loader failure");
    assert_eq!(third.finish().unwrap_err(), "fixture loader failure");
    assert_version(Ok(h.service.current()), "v1");
    h.assert_calls(2);
}

#[test]
fn active_failure_does_not_cancel_a_pending_success_or_borrow_its_result() {
    let h = Harness::new();
    let first = h.call("startup");
    h.enter(1);
    let second = h.call("install");
    h.pending("install");
    h.release.send(LoadStep::Failed).unwrap();
    assert_eq!(first.finish().unwrap_err(), "fixture loader failure");
    h.enter(2);
    h.loaded("v2");
    assert_version(second.finish(), "v2");
    h.assert_calls(2);
}

#[test]
fn disposal_wakes_queued_callers_without_loading_or_overwriting_retained_cache() {
    let h = Harness::new();
    let seed = h.call("seed");
    h.enter(1);
    h.loaded("retained");
    assert_version(seed.finish(), "retained");
    let active = h.call("refresh");
    h.enter(2);
    let queued = h.call("auth_change");
    h.pending("auth_change");
    h.service.dispose();
    assert_version(queued.finish(), "retained");
    assert_version(h.call("after-dispose").finish(), "retained");
    active.assert_waiting();
    h.loaded("must-not-write");
    assert_version(active.finish(), "retained");
    assert_version(Ok(h.service.current()), "retained");
    h.assert_calls(2);
    assert_eq!(h.events.lock().unwrap().len(), 1);
}

#[test]
fn requests_during_a_followup_have_their_own_next_pass() {
    let h = Harness::new();
    let first = h.call("startup");
    h.enter(1);
    let second = h.call("install");
    h.pending("install");
    h.loaded("v1");
    assert_version(first.finish(), "v1");
    h.enter(2);
    let third = h.call("refresh");
    h.pending("refresh");
    h.loaded("v2");
    assert_version(second.finish(), "v2");
    h.enter(3);
    third.assert_waiting();
    h.loaded("v3");
    assert_version(third.finish(), "v3");
    h.assert_calls(3);
}

#[test]
fn panicking_followup_settles_shared_waiters_and_releases_future_sync() {
    let h = Harness::new();
    let first = h.call("startup");
    h.enter(1);
    let second = h.call("install");
    h.pending("install");
    let third = h.call("refresh");
    h.pending("refresh");
    h.loaded("v1");
    assert_version(first.finish(), "v1");
    h.enter(2);
    h.release.send(LoadStep::Panicked).unwrap();
    let outcomes = [second.outcome(), third.outcome()];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_err()).count(),
        1
    );
    let settled = outcomes.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(
        settled.unwrap_err(),
        "plugin skills sync aborted before completion"
    );
    let recovery = h.call("recovery");
    h.enter(3);
    h.loaded("v3");
    assert_version(recovery.finish(), "v3");
    h.assert_calls(3);
}

struct ReentrantLoader {
    service: Mutex<Weak<SandPluginSkillsService>>,
    loaded: LoadedPlugins,
    observed: Mutex<Option<String>>,
}
impl PluginSkillsLoader for ReentrantLoader {
    fn load(&self) -> Result<LoadedPlugins, String> {
        let service = self.service.lock().unwrap().upgrade().unwrap();
        let error = service.sync("recursive").unwrap_err();
        *self.observed.lock().unwrap() = Some(error);
        Ok(self.loaded.clone())
    }
}

#[test]
fn same_thread_recursive_sync_fails_explicitly_instead_of_waiting_on_itself() {
    let root = TempRoot::new();
    let loader = Arc::new(ReentrantLoader {
        service: Mutex::new(Weak::new()),
        loaded: root.loaded("v1"),
        observed: Mutex::new(None),
    });
    let service = Arc::new(SandPluginSkillsService::new(&root.0, loader.clone()));
    *loader.service.lock().unwrap() = Arc::downgrade(&service);
    assert_version(Call::start(&service, "startup").finish(), "v1");
    assert_eq!(
        loader.observed.lock().unwrap().as_deref(),
        Some("recursive plugin skills sync cannot wait on its own pass")
    );
    assert!(service.state.lock().unwrap().pending.is_none());
}

struct ReadyAuth;
impl super::super::extension::AuthRenewalPort for ReadyAuth {
    fn peek_access_token(&self) -> Option<String> {
        Some("fixture-ready".into())
    }

    fn subscribe_to_renewal(
        &self,
        _listener: Arc<dyn Fn(super::super::extension::AuthRenewalEvent) + Send + Sync>,
    ) -> Box<dyn FnOnce() + Send> {
        Box::new(|| {})
    }
}

fn start_real_poll(
    service: &Arc<SandPluginSkillsService>,
    sweeps: &Arc<AtomicUsize>,
) -> super::super::extension::PluginSkillsAuthenticatedStartup {
    let sweeps = Arc::clone(sweeps);
    super::super::extension::PluginSkillsAuthenticatedStartup::start(
        Arc::new(ReadyAuth),
        service.clone(),
        Arc::new(
            super::super::production::RealPluginSkillsPolling::with_interval(Duration::from_secs(
                60,
            )),
        ),
        Some(Arc::new(move || {
            sweeps.fetch_add(1, Ordering::SeqCst);
        })),
    )
}

#[test]
fn real_poll_shutdown_retires_a_queued_sync_before_joining_its_thread() {
    let h = Harness::new();
    let active = h.call("manual-refresh");
    h.enter(1);
    let sweeps = Arc::new(AtomicUsize::new(0));
    let startup = start_real_poll(&h.service, &sweeps);
    h.pending("startup");
    let (tx, stopped) = mpsc::channel();
    let stopping = thread::spawn(move || {
        startup.dispose();
        tx.send(()).unwrap();
    });
    // The queued poll must retire without waiting for the unrelated loader.
    stopped
        .recv_timeout(DEADLINE)
        .expect("queued poll blocked shutdown");
    stopping.join().unwrap();
    active.assert_waiting();
    h.assert_calls(1);
    h.loaded("must-not-write");
    assert!(active.finish().unwrap().is_empty());
    assert!(h.service.current().is_empty());
    assert_eq!(sweeps.load(Ordering::SeqCst), 0);
    h.assert_calls(1);
}

#[test]
fn real_poll_shutdown_fences_an_active_load_before_join_finishes() {
    let h = Harness::new();
    let sweeps = Arc::new(AtomicUsize::new(0));
    let startup = start_real_poll(&h.service, &sweeps);
    h.enter(1);
    let (tx, stopped) = mpsc::channel();
    let stopping = thread::spawn(move || {
        startup.dispose();
        tx.send(()).unwrap();
    });
    let until = Instant::now() + DEADLINE;
    let mut state = h.service.state.lock().unwrap();
    while !state.disposed {
        let remaining = until
            .checked_duration_since(Instant::now())
            .expect("skills not disposed");
        state = h.service.changed.wait_timeout(state, remaining).unwrap().0;
    }
    drop(state);
    assert!(matches!(stopped.try_recv(), Err(TryRecvError::Empty)));
    h.loaded("must-not-write");
    stopped
        .recv_timeout(DEADLINE)
        .expect("active poll blocked shutdown");
    stopping.join().unwrap();
    assert!(h.service.current().is_empty());
    assert_eq!(sweeps.load(Ordering::SeqCst), 0);
    h.assert_calls(1);
}
