use std::sync::{Arc, Mutex};

pub const MCP_EXTENSION_ID: &str = "mcp";
pub const MCP_EXTENSION_DEPENDENCIES: &[&str] = &[
    "auth",
    "experiments",
    "forever-box",
    "settings",
    "telemetry",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRenewalEvent {
    pub outcome: String,
    pub is_first_credential: bool,
}

pub trait AuthRenewalPort: Send + Sync {
    fn peek_access_token(&self) -> Option<String>;
    fn subscribe_to_renewal(
        &self,
        listener: Arc<dyn Fn(AuthRenewalEvent) + Send + Sync>,
    ) -> Box<dyn FnOnce() + Send>;
}

pub trait McpPluginSkillsService: Send + Sync {
    fn sync(&self, trigger: &str) -> Result<(), String>;
    fn handle_auth_change(&self);
    fn dispose(&self);
}

pub trait PollingHandle: Send {
    fn dispose(&mut self);
}

pub trait PollingPort: Send + Sync {
    fn start(&self, callback: Arc<dyn Fn() + Send + Sync>) -> Box<dyn PollingHandle>;
}

pub trait McpHostServicePort: Send + Sync {
    fn dispose(&self) -> Result<(), String>;
}

pub trait McpSkillPublishPort: Send + Sync {}

impl<T: Send + Sync> McpSkillPublishPort for T {}

struct StartupState {
    started: bool,
    disposed: bool,
    first_poll: bool,
    polling: Option<Box<dyn PollingHandle>>,
}

pub struct PluginSkillsAuthenticatedStartup {
    state: Arc<Mutex<StartupState>>,
    unsubscribe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    plugin_skills: Arc<dyn McpPluginSkillsService>,
}

impl PluginSkillsAuthenticatedStartup {
    pub fn start(
        auth: Arc<dyn AuthRenewalPort>,
        plugin_skills: Arc<dyn McpPluginSkillsService>,
        polling: Arc<dyn PollingPort>,
        on_startup_sync_succeeded: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let state = Arc::new(Mutex::new(StartupState {
            started: false,
            disposed: false,
            first_poll: true,
            polling: None,
        }));

        let start = {
            let state = Arc::clone(&state);
            let plugin_skills = Arc::clone(&plugin_skills);
            let polling = Arc::clone(&polling);
            let on_startup_sync_succeeded = on_startup_sync_succeeded.clone();
            Arc::new(move || {
                {
                    let mut guard = state.lock().expect("mcp startup state poisoned");
                    if guard.started || guard.disposed {
                        return;
                    }
                    guard.started = true;
                }
                let state_for_poll = Arc::clone(&state);
                let plugin_for_poll = Arc::clone(&plugin_skills);
                let on_startup = on_startup_sync_succeeded.clone();
                let callback = Arc::new(move || {
                    let trigger = {
                        let mut state = state_for_poll.lock().expect("mcp startup state poisoned");
                        if state.disposed {
                            return;
                        }
                        let trigger = if state.first_poll {
                            state.first_poll = false;
                            "startup"
                        } else {
                            "refresh"
                        };
                        trigger
                    };
                    let succeeded = plugin_for_poll.sync(trigger).is_ok();
                    if trigger == "startup" && succeeded {
                        if let Some(on_startup) = &on_startup {
                            let disposed = state_for_poll
                                .lock()
                                .expect("mcp startup state poisoned")
                                .disposed;
                            if !disposed {
                                on_startup();
                            }
                        }
                    }
                });
                // A polling adapter may invoke its callback before returning.
                // Do not hold the state lock across that external call.
                let mut handle = Some(polling.start(callback));
                {
                    let mut guard = state.lock().expect("mcp startup state poisoned");
                    if !guard.disposed {
                        guard.polling = handle.take();
                    }
                }
                // Stop may have won while the polling adapter was starting.
                if let Some(mut handle) = handle {
                    handle.dispose();
                }
            })
        };

        let renewal_listener = {
            let state = Arc::clone(&state);
            let auth = Arc::clone(&auth);
            let start = Arc::clone(&start);
            let plugin_skills = Arc::clone(&plugin_skills);
            Arc::new(move |event: AuthRenewalEvent| {
                if event.outcome != "renewed" {
                    return;
                }
                let (disposed, started) = {
                    let guard = state.lock().expect("mcp startup state poisoned");
                    (guard.disposed, guard.started)
                };
                if disposed {
                    return;
                }
                if !started {
                    if auth.peek_access_token().is_some() {
                        start();
                    }
                    return;
                }
                if event.is_first_credential {
                    plugin_skills.handle_auth_change();
                }
            })
        };

        let unsubscribe = auth.subscribe_to_renewal(renewal_listener);
        if auth.peek_access_token().is_some() {
            start();
        }

        Self {
            state,
            unsubscribe: Mutex::new(Some(unsubscribe)),
            plugin_skills,
        }
    }

    pub fn dispose(&self) {
        let polling = {
            let mut state = self.state.lock().expect("mcp startup state poisoned");
            if state.disposed {
                return;
            }
            state.disposed = true;
            state.polling.take()
        };
        // Retire queued syncs before joining: the poll may be waiting behind
        // a refresh owned by another thread. It must not start a new load just
        // to let shutdown complete. An active load observes disposal on return.
        self.plugin_skills.dispose();
        // The polling thread may need the state lock to finish its callback.
        // Joining it while holding that lock would deadlock Host shutdown.
        if let Some(mut polling) = polling {
            polling.dispose();
        }
        let unsubscribe = self
            .unsubscribe
            .lock()
            .expect("mcp startup unsubscribe poisoned")
            .take();
        if let Some(unsubscribe) = unsubscribe {
            unsubscribe();
        }
    }

    pub fn is_started(&self) -> bool {
        self.state
            .lock()
            .expect("mcp startup state poisoned")
            .started
    }
}

pub struct McpExtensionRuntime {
    startup: PluginSkillsAuthenticatedStartup,
    service: Arc<dyn McpHostServicePort>,
    #[allow(dead_code)]
    skill_publish: Arc<dyn McpSkillPublishPort>,
    stopped: Mutex<bool>,
}

impl McpExtensionRuntime {
    pub fn new(
        startup: PluginSkillsAuthenticatedStartup,
        service: Arc<dyn McpHostServicePort>,
        skill_publish: Arc<dyn McpSkillPublishPort>,
    ) -> Self {
        Self {
            startup,
            service,
            skill_publish,
            stopped: Mutex::new(false),
        }
    }

    pub fn stop(&self) -> Result<(), String> {
        {
            let mut stopped = self.stopped.lock().expect("mcp extension state poisoned");
            if *stopped {
                return Ok(());
            }
            *stopped = true;
        }
        self.startup.dispose();
        self.service.dispose()
    }
}

impl Drop for McpExtensionRuntime {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
