use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread;
use std::time::Duration;

use mahayana_host_runtime::extensions::mcp::production::RealPluginSkillsPolling;

use mahayana_host_runtime::extensions::mcp::extension::{
    AuthRenewalEvent, AuthRenewalPort, MCP_EXTENSION_DEPENDENCIES, MCP_EXTENSION_ID,
    McpExtensionRuntime, McpHostServicePort, McpPluginSkillsService,
    PluginSkillsAuthenticatedStartup, PollingHandle, PollingPort,
};

#[derive(Default)]
struct AuthState {
    token: Option<String>,
    listener: Option<Arc<dyn Fn(AuthRenewalEvent) + Send + Sync>>,
    unsubscribed: usize,
}
struct Auth(Arc<Mutex<AuthState>>);
impl AuthRenewalPort for Auth {
    fn peek_access_token(&self) -> Option<String> {
        self.0.lock().unwrap().token.clone()
    }
    fn subscribe_to_renewal(
        &self,
        listener: Arc<dyn Fn(AuthRenewalEvent) + Send + Sync>,
    ) -> Box<dyn FnOnce() + Send> {
        self.0.lock().unwrap().listener = Some(listener);
        let state = self.0.clone();
        Box::new(move || state.lock().unwrap().unsubscribed += 1)
    }
}

#[derive(Default)]
struct SkillsState {
    syncs: Vec<String>,
    auth_changes: usize,
    disposals: usize,
}
struct Skills(Arc<Mutex<SkillsState>>);
impl McpPluginSkillsService for Skills {
    fn sync(&self, trigger: &str) -> Result<(), String> {
        self.0.lock().unwrap().syncs.push(trigger.into());
        Ok(())
    }
    fn handle_auth_change(&self) {
        self.0.lock().unwrap().auth_changes += 1;
    }
    fn dispose(&self) {
        self.0.lock().unwrap().disposals += 1;
    }
}

#[derive(Default)]
struct PollState {
    callback: Option<Arc<dyn Fn() + Send + Sync>>,
    disposed: usize,
}
struct Handle(Arc<Mutex<PollState>>);
impl PollingHandle for Handle {
    fn dispose(&mut self) {
        self.0.lock().unwrap().disposed += 1;
    }
}
struct Poll(Arc<Mutex<PollState>>);
impl PollingPort for Poll {
    fn start(&self, callback: Arc<dyn Fn() + Send + Sync>) -> Box<dyn PollingHandle> {
        self.0.lock().unwrap().callback = Some(callback);
        Box::new(Handle(self.0.clone()))
    }
}

#[derive(Default)]
struct Service(Arc<Mutex<usize>>);
impl McpHostServicePort for Service {
    fn dispose(&self) -> Result<(), String> {
        *self.0.lock().unwrap() += 1;
        Ok(())
    }
}

#[test]
fn frozen_identity_and_dependencies_are_preserved() {
    assert_eq!(MCP_EXTENSION_ID, "mcp");
    assert_eq!(
        MCP_EXTENSION_DEPENDENCIES,
        [
            "auth",
            "experiments",
            "forever-box",
            "settings",
            "telemetry"
        ]
    );
}

#[test]
fn authentication_gates_startup_and_first_success_runs_legacy_sweep() {
    let auth_state = Arc::new(Mutex::new(AuthState::default()));
    let skills_state = Arc::new(Mutex::new(SkillsState::default()));
    let poll_state = Arc::new(Mutex::new(PollState::default()));
    let swept = Arc::new(Mutex::new(0usize));
    let startup = PluginSkillsAuthenticatedStartup::start(
        Arc::new(Auth(auth_state.clone())),
        Arc::new(Skills(skills_state.clone())),
        Arc::new(Poll(poll_state.clone())),
        Some({
            let swept = swept.clone();
            Arc::new(move || *swept.lock().unwrap() += 1)
        }),
    );
    assert!(!startup.is_started());
    auth_state.lock().unwrap().token = Some("token".into());
    let renewal = auth_state.lock().unwrap().listener.clone().unwrap();
    renewal(AuthRenewalEvent {
        outcome: "renewed".into(),
        is_first_credential: true,
    });
    assert!(startup.is_started());

    let callback = poll_state.lock().unwrap().callback.clone().unwrap();
    callback();
    callback();
    assert_eq!(
        skills_state.lock().unwrap().syncs,
        vec!["startup", "refresh"]
    );
    assert_eq!(*swept.lock().unwrap(), 1);

    let renewal = auth_state.lock().unwrap().listener.clone().unwrap();
    renewal(AuthRenewalEvent {
        outcome: "renewed".into(),
        is_first_credential: true,
    });
    assert_eq!(skills_state.lock().unwrap().auth_changes, 1);
    startup.dispose();
    startup.dispose();
    assert_eq!(skills_state.lock().unwrap().disposals, 1);
    assert_eq!(poll_state.lock().unwrap().disposed, 1);
    assert_eq!(auth_state.lock().unwrap().unsubscribed, 1);
}

#[test]
fn extension_stop_disposes_startup_before_service_and_is_idempotent() {
    let auth_state = Arc::new(Mutex::new(AuthState {
        token: Some("token".into()),
        ..Default::default()
    }));
    let skills_state = Arc::new(Mutex::new(SkillsState::default()));
    let poll_state = Arc::new(Mutex::new(PollState::default()));
    let startup = PluginSkillsAuthenticatedStartup::start(
        Arc::new(Auth(auth_state)),
        Arc::new(Skills(skills_state.clone())),
        Arc::new(Poll(poll_state)),
        None,
    );
    let service = Arc::new(Service::default());
    let runtime = McpExtensionRuntime::new(startup, service.clone(), Arc::new(()));
    runtime.stop().unwrap();
    runtime.stop().unwrap();
    assert_eq!(skills_state.lock().unwrap().disposals, 1);
    assert_eq!(*service.0.lock().unwrap(), 1);
}

// A regressed lock order must fail a contract, not hang the entire Host suite.
fn completes_without_deadlock(check: impl FnOnce() + Send + 'static) {
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        check();
        let _ = done_tx.send(());
    });
    done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("MCP lifecycle deadlocked or its worker failed");
    worker.join().expect("MCP lifecycle worker panicked");
}

fn authenticated_state() -> Arc<Mutex<AuthState>> {
    Arc::new(Mutex::new(AuthState {
        token: Some("test-token".into()),
        ..Default::default()
    }))
}

struct InlinePoll(Arc<Mutex<PollState>>);
impl PollingPort for InlinePoll {
    fn start(&self, callback: Arc<dyn Fn() + Send + Sync>) -> Box<dyn PollingHandle> {
        self.0.lock().unwrap().callback = Some(callback.clone());
        callback();
        Box::new(Handle(self.0.clone()))
    }
}

#[test]
fn inline_startup_callback_does_not_run_under_the_state_lock() {
    completes_without_deadlock(|| {
        let skills = Arc::new(Mutex::new(SkillsState::default()));
        let poll = Arc::new(Mutex::new(PollState::default()));
        let startup = PluginSkillsAuthenticatedStartup::start(
            Arc::new(Auth(authenticated_state())),
            Arc::new(Skills(skills.clone())),
            Arc::new(InlinePoll(poll.clone())),
            None,
        );
        assert_eq!(skills.lock().unwrap().syncs, ["startup"]);
        startup.dispose();
        assert_eq!(poll.lock().unwrap().disposed, 1);
        assert_eq!(skills.lock().unwrap().disposals, 1);
    });
}

struct CallbackOnDisposeHandle {
    callback: Arc<dyn Fn() + Send + Sync>,
    poll: Arc<Mutex<PollState>>,
}
impl PollingHandle for CallbackOnDisposeHandle {
    fn dispose(&mut self) {
        (self.callback)();
        self.poll.lock().unwrap().disposed += 1;
    }
}
struct CallbackOnDisposePoll(Arc<Mutex<PollState>>);
impl PollingPort for CallbackOnDisposePoll {
    fn start(&self, callback: Arc<dyn Fn() + Send + Sync>) -> Box<dyn PollingHandle> {
        Box::new(CallbackOnDisposeHandle {
            callback,
            poll: self.0.clone(),
        })
    }
}

#[test]
fn disposal_callback_observes_stopped_state_without_deadlock_or_new_sync() {
    completes_without_deadlock(|| {
        let skills = Arc::new(Mutex::new(SkillsState::default()));
        let poll = Arc::new(Mutex::new(PollState::default()));
        let startup = PluginSkillsAuthenticatedStartup::start(
            Arc::new(Auth(authenticated_state())),
            Arc::new(Skills(skills.clone())),
            Arc::new(CallbackOnDisposePoll(poll.clone())),
            None,
        );
        startup.dispose();
        startup.dispose();
        assert!(skills.lock().unwrap().syncs.is_empty());
        assert_eq!(skills.lock().unwrap().disposals, 1);
        assert_eq!(poll.lock().unwrap().disposed, 1);
    });
}

struct DelayedPoll {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    poll: Arc<Mutex<PollState>>,
}
impl PollingPort for DelayedPoll {
    fn start(&self, callback: Arc<dyn Fn() + Send + Sync>) -> Box<dyn PollingHandle> {
        self.entered.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .expect("test did not release delayed poll creation");
        callback();
        Box::new(Handle(self.poll.clone()))
    }
}

#[test]
fn stop_during_auth_start_disposes_the_late_handle_exactly_once() {
    completes_without_deadlock(|| {
        let auth = Arc::new(Mutex::new(AuthState::default()));
        let skills = Arc::new(Mutex::new(SkillsState::default()));
        let poll = Arc::new(Mutex::new(PollState::default()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let startup = PluginSkillsAuthenticatedStartup::start(
            Arc::new(Auth(auth.clone())),
            Arc::new(Skills(skills.clone())),
            Arc::new(DelayedPoll {
                entered: entered_tx,
                release: Mutex::new(release_rx),
                poll: poll.clone(),
            }),
            None,
        );
        auth.lock().unwrap().token = Some("test-token".into());
        let listener = auth.lock().unwrap().listener.clone().unwrap();
        let (finished_tx, finished_rx) = mpsc::channel();
        let renewal = thread::spawn(move || {
            listener(AuthRenewalEvent {
                outcome: "renewed".into(),
                is_first_credential: true,
            });
            let _ = finished_tx.send(());
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        startup.dispose();
        assert_eq!(poll.lock().unwrap().disposed, 0);
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        renewal.join().unwrap();
        startup.dispose();
        assert_eq!(poll.lock().unwrap().disposed, 1);
        assert_eq!(skills.lock().unwrap().disposals, 1);
        assert_eq!(auth.lock().unwrap().unsubscribed, 1);
        assert!(skills.lock().unwrap().syncs.is_empty());
    });
}

struct InFlightSkills {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    state: Arc<Mutex<SkillsState>>,
}
impl McpPluginSkillsService for InFlightSkills {
    fn sync(&self, trigger: &str) -> Result<(), String> {
        self.state.lock().unwrap().syncs.push(trigger.into());
        self.entered.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .expect("test did not release in-flight sync");
        Ok(())
    }
    fn handle_auth_change(&self) {}
    fn dispose(&self) {
        self.state.lock().unwrap().disposals += 1;
    }
}
struct JoinObservedHandle {
    inner: Box<dyn PollingHandle>,
    joining: mpsc::Sender<()>,
}
impl PollingHandle for JoinObservedHandle {
    fn dispose(&mut self) {
        self.joining.send(()).unwrap();
        self.inner.dispose();
    }
}
struct JoinObservedPoll(mpsc::Sender<()>);
impl PollingPort for JoinObservedPoll {
    fn start(&self, callback: Arc<dyn Fn() + Send + Sync>) -> Box<dyn PollingHandle> {
        Box::new(JoinObservedHandle {
            inner: RealPluginSkillsPolling::daily().start(callback),
            joining: self.0.clone(),
        })
    }
}

#[test]
fn real_poll_shutdown_joins_in_flight_sync_without_running_legacy_sweep() {
    completes_without_deadlock(|| {
        let skills = Arc::new(Mutex::new(SkillsState::default()));
        let swept = Arc::new(Mutex::new(0usize));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (joining_tx, joining_rx) = mpsc::channel();
        let startup = PluginSkillsAuthenticatedStartup::start(
            Arc::new(Auth(authenticated_state())),
            Arc::new(InFlightSkills {
                entered: entered_tx,
                release: Mutex::new(release_rx),
                state: skills.clone(),
            }),
            Arc::new(JoinObservedPoll(joining_tx)),
            Some({
                let swept = swept.clone();
                Arc::new(move || *swept.lock().unwrap() += 1)
            }),
        );
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (stopped_tx, stopped_rx) = mpsc::channel();
        let stopping = thread::spawn(move || {
            startup.dispose();
            let _ = stopped_tx.send(());
        });
        joining_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        release_tx.send(()).unwrap();
        stopped_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        stopping.join().unwrap();
        assert_eq!(*swept.lock().unwrap(), 0);
        assert_eq!(skills.lock().unwrap().syncs, ["startup"]);
        assert_eq!(skills.lock().unwrap().disposals, 1);
    });
}

struct ReentrantService {
    owner: Arc<Mutex<Option<Weak<McpExtensionRuntime>>>>,
    disposals: Arc<Mutex<usize>>,
}
impl McpHostServicePort for ReentrantService {
    fn dispose(&self) -> Result<(), String> {
        let owner = self.owner.lock().unwrap().as_ref().unwrap().upgrade().unwrap();
        owner.stop()?;
        *self.disposals.lock().unwrap() += 1;
        Ok(())
    }
}

#[test]
fn runtime_stop_does_not_hold_its_lock_while_disposing_external_services() {
    completes_without_deadlock(|| {
        let owner = Arc::new(Mutex::new(None));
        let disposals = Arc::new(Mutex::new(0usize));
        let skills = Arc::new(Mutex::new(SkillsState::default()));
        let startup = PluginSkillsAuthenticatedStartup::start(
            Arc::new(Auth(authenticated_state())),
            Arc::new(Skills(skills.clone())),
            Arc::new(Poll(Arc::new(Mutex::new(PollState::default())))),
            None,
        );
        let runtime = Arc::new(McpExtensionRuntime::new(
            startup,
            Arc::new(ReentrantService {
                owner: owner.clone(),
                disposals: disposals.clone(),
            }),
            Arc::new(()),
        ));
        *owner.lock().unwrap() = Some(Arc::downgrade(&runtime));
        runtime.stop().unwrap();
        runtime.stop().unwrap();
        assert_eq!(*disposals.lock().unwrap(), 1);
        assert_eq!(skills.lock().unwrap().disposals, 1);
    });
}
