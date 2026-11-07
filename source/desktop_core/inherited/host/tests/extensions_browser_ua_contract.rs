use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::browser_ua::{
    BROWSER_UA_DEPENDENCIES, BrowserUaAuthApi, BrowserUaAuthRenewalEvent,
    BrowserUaExperimentsApi, BrowserUaHostLog, browser_ua_extension_id,
    start_browser_ua_extension,
};
use mahayana_host_runtime::extensions::browser_ua::ua_owner_stamp_service::{
    UA_OWNER_STAMP_LENGTH, ua_owner_stamp_for_access_token,
};
use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;

struct FakeAuth {
    token: Mutex<Option<String>>,
    listener: Mutex<Option<Arc<dyn Fn(BrowserUaAuthRenewalEvent) + Send + Sync>>>,
    stops: Arc<AtomicUsize>,
}

impl BrowserUaAuthApi for FakeAuth {
    fn peek_access_token(&self) -> Option<String> {
        self.token.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    fn subscribe_to_renewal(
        &self,
        listener: Arc<dyn Fn(BrowserUaAuthRenewalEvent) + Send + Sync>,
    ) -> Box<dyn FnOnce() + Send> {
        self.listener.lock().unwrap_or_else(|p| p.into_inner()).replace(listener);
        let stops = Arc::clone(&self.stops);
        Box::new(move || { stops.fetch_add(1, Ordering::SeqCst); })
    }
}
impl FakeAuth {
    fn renew(&self, token: &str) {
        self.token.lock().unwrap_or_else(|p| p.into_inner()).replace(token.to_string());
        let listener = self.listener.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(listener) = listener {
            listener(BrowserUaAuthRenewalEvent { outcome: "renewed".into() });
        }
    }
}

struct FakeExperiments {
    enabled: AtomicBool,
    listener: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    stops: Arc<AtomicUsize>,
}
impl BrowserUaExperimentsApi for FakeExperiments {
    fn is_ua_token_kill_switch_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
    fn subscribe(&self, listener: Arc<dyn Fn() + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.listener.lock().unwrap_or_else(|p| p.into_inner()).replace(listener);
        let stops = Arc::clone(&self.stops);
        Box::new(move || { stops.fetch_add(1, Ordering::SeqCst); })
    }
}
impl FakeExperiments {
    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        let listener = self.listener.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(listener) = listener { listener(); }
    }
}

#[derive(Default)]
struct FakeLog { messages: Mutex<Vec<String>> }
impl BrowserUaHostLog for FakeLog {
    fn log(&self, message: &str) {
        self.messages.lock().unwrap_or_else(|p| p.into_inner()).push(message.to_string());
    }
}

#[test]
fn browser_ua_extension_wires_auth_and_experiments_to_marker_services() {
    assert_eq!(browser_ua_extension_id(), HostExtensionId::BrowserUa);
    assert_eq!(BROWSER_UA_DEPENDENCIES, &[HostExtensionId::Auth, HostExtensionId::Experiments]);

    let suffix = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
    let root = std::env::temp_dir().join(format!("fabushi-browser-ua-{suffix}"));
    fs::create_dir_all(&root).expect("create test root");
    let owner_path = root.join("owner");
    let disabled_path = root.join("disabled");

    let initial_token = "e30.eyJzdWIiOiJ1c2VyLTEyMyJ9.sig";
    let renewed_token = "e30.eyJzdWIiOiJ1c2VyLTQ1NiJ9.sig";
    let auth_stops = Arc::new(AtomicUsize::new(0));
    let experiment_stops = Arc::new(AtomicUsize::new(0));
    let auth = Arc::new(FakeAuth {
        token: Mutex::new(Some(initial_token.to_string())),
        listener: Mutex::new(None),
        stops: Arc::clone(&auth_stops),
    });
    let experiments = Arc::new(FakeExperiments {
        enabled: AtomicBool::new(false),
        listener: Mutex::new(None),
        stops: Arc::clone(&experiment_stops),
    });
    let log = Arc::new(FakeLog::default());

    let runtime = start_browser_ua_extension(
        Arc::clone(&auth), Arc::clone(&experiments), Arc::clone(&log),
        Some(owner_path.clone()), Some(disabled_path.clone()),
    );

    let initial_stamp = ua_owner_stamp_for_access_token(initial_token).expect("initial stamp");
    assert_eq!(initial_stamp.len(), UA_OWNER_STAMP_LENGTH);
    assert_eq!(fs::read_to_string(&owner_path).expect("owner stamp"), format!("{initial_stamp}\n"));
    assert!(!disabled_path.exists());

    experiments.set_enabled(true);
    assert_eq!(fs::read_to_string(&disabled_path).expect("disabled marker"), "1\n");

    auth.renew(renewed_token);
    let renewed_stamp = ua_owner_stamp_for_access_token(renewed_token).expect("renewed stamp");
    assert_eq!(fs::read_to_string(&owner_path).expect("renewed owner stamp"), format!("{renewed_stamp}\n"));

    drop(runtime);
    assert_eq!(auth_stops.load(Ordering::SeqCst), 1);
    assert_eq!(experiment_stops.load(Ordering::SeqCst), 1);
    assert!(log.messages.lock().unwrap_or_else(|p| p.into_inner()).is_empty());
    fs::remove_dir_all(root).expect("remove test root");
}
