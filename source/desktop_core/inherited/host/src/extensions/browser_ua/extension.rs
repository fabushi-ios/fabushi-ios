use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::extensions::extension_ids_generated::HostExtensionId;

use super::ua_owner_stamp_service::create_ua_owner_stamp_writer;
use super::ua_token_kill_switch_service::create_ua_token_kill_switch_reconciler;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserUaAuthRenewalEvent {
    pub outcome: String,
}

pub type StopSubscription = Box<dyn FnOnce() + Send>;

pub trait BrowserUaAuthApi: Send + Sync {
    fn peek_access_token(&self) -> Option<String>;
    fn subscribe_to_renewal(
        &self,
        listener: Arc<dyn Fn(BrowserUaAuthRenewalEvent) + Send + Sync>,
    ) -> StopSubscription;
}

pub trait BrowserUaExperimentsApi: Send + Sync {
    fn is_ua_token_kill_switch_enabled(&self) -> bool;
    fn subscribe(&self, listener: Arc<dyn Fn() + Send + Sync>) -> StopSubscription;
}

pub trait BrowserUaHostLog: Send + Sync {
    fn log(&self, message: &str);
}

pub const BROWSER_UA_DEPENDENCIES: &[HostExtensionId] =
    &[HostExtensionId::Auth, HostExtensionId::Experiments];

pub fn browser_ua_extension_id() -> HostExtensionId {
    HostExtensionId::BrowserUa
}

pub struct BrowserUaExtensionRuntime {
    stops: Vec<StopSubscription>,
}

impl BrowserUaExtensionRuntime {
    pub fn stop(mut self) {
        self.run_stops();
    }

    fn run_stops(&mut self) {
        while let Some(stop) = self.stops.pop() {
            stop();
        }
    }
}

impl Drop for BrowserUaExtensionRuntime {
    fn drop(&mut self) {
        self.run_stops();
    }
}

pub fn start_browser_ua_extension<Auth, Experiments, Log>(
    auth: Arc<Auth>,
    experiments: Arc<Experiments>,
    log: Arc<Log>,
    owner_stamp_path: Option<PathBuf>,
    kill_switch_path: Option<PathBuf>,
) -> BrowserUaExtensionRuntime
where
    Auth: BrowserUaAuthApi + 'static,
    Experiments: BrowserUaExperimentsApi + 'static,
    Log: BrowserUaHostLog + 'static,
{
    let owner_log = Arc::clone(&log);
    let owner_writer = Arc::new(Mutex::new(create_ua_owner_stamp_writer(
        owner_stamp_path,
        move |message| owner_log.log(message),
    )));

    let auth_for_renewal = Arc::clone(&auth);
    let writer_for_renewal = Arc::clone(&owner_writer);
    let auth_stop = auth.subscribe_to_renewal(Arc::new(move |event| {
        if event.outcome == "renewed" {
            let token = auth_for_renewal.peek_access_token();
            writer_for_renewal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .write(token.as_deref());
        }
    }));

    let initial_token = auth.peek_access_token();
    if initial_token.is_some() {
        owner_writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write(initial_token.as_deref());
    }

    let experiments_for_reconcile = Arc::clone(&experiments);
    let kill_switch_log = Arc::clone(&log);
    let kill_switch = Arc::new(Mutex::new(create_ua_token_kill_switch_reconciler(
        kill_switch_path,
        move || experiments_for_reconcile.is_ua_token_kill_switch_enabled(),
        move |message| kill_switch_log.log(message),
    )));

    let kill_switch_for_update = Arc::clone(&kill_switch);
    let experiments_stop = experiments.subscribe(Arc::new(move || {
        kill_switch_for_update
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reconcile();
    }));
    kill_switch
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .reconcile();

    BrowserUaExtensionRuntime {
        stops: vec![auth_stop, experiments_stop],
    }
}
