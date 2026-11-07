use super::xuser_sharing_environment::resolve_xuser_sharing_environment;
use super::xuser_sharing_service::{SandSharingState, SandXuserSharingService};
use crate::extensions::browser_ua::extension::StopSubscription;
use crate::extensions::experiments::HostExperimentsExtension;
use crate::extensions::notify_bus::extension::HostNotifyBusExtension;
use crate::extensions::notify_bus::notify_bus_client::SandNotifyTopic;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub const SHARING_DISABLED_MESSAGE: &str = "Sharing isn't enabled for your account.";

pub struct CrossUserSharingExtension {
    service: Arc<SandXuserSharingService>,
    allowed: bool,
    reason: Option<String>,
    experiment_stop: Mutex<Option<StopSubscription>>,
    notify_stop: Mutex<Option<StopSubscription>>,
}
impl CrossUserSharingExtension {
    pub fn new(
        service: Arc<SandXuserSharingService>,
        backend_url: &str,
        env: &BTreeMap<String, String>,
    ) -> Self {
        let gate = resolve_xuser_sharing_environment(backend_url, env);
        if !gate.is_allowed {
            service.set_enabled(false)
        }
        Self {
            service,
            allowed: gate.is_allowed,
            reason: gate.reason,
            experiment_stop: Mutex::new(None),
            notify_stop: Mutex::new(None),
        }
    }
    pub fn set_feature_gate(&self, on: bool) {
        self.service.set_enabled(on && self.allowed)
    }

    fn reconcile_feature_gate(
        &self,
        experiments: &HostExperimentsExtension,
    ) -> Result<(), String> {
        let enabled = self.allowed && experiments.check_feature_gate("sand_multiplayer");
        self.service.set_enabled(enabled);
        if enabled {
            self.service.start().map(|_| ())
        } else {
            Ok(())
        }
    }

    pub fn start_background_work(
        self: &Arc<Self>,
        experiments: Arc<HostExperimentsExtension>,
        notify_bus: HostNotifyBusExtension,
    ) -> Result<(), String> {
        {
            let mut notify_stop = self.notify_stop.lock().unwrap_or_else(|p| p.into_inner());
            if notify_stop.is_none() {
                let weak = Arc::downgrade(self);
                *notify_stop = Some(notify_bus.on_notify(
                    SandNotifyTopic::XuserEvents,
                    Arc::new(move || {
                        if let Some(extension) = weak.upgrade() {
                            extension.service.request_relay_drain();
                        }
                    }),
                ));
            }
        }

        {
            let mut experiment_stop = self
                .experiment_stop
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if experiment_stop.is_none() {
                let weak = Arc::downgrade(self);
                let experiments_for_listener = Arc::clone(&experiments);
                *experiment_stop = Some(experiments.subscribe(Arc::new(move || {
                    let Some(extension) = weak.upgrade() else {
                        return;
                    };
                    if let Err(error) =
                        extension.reconcile_feature_gate(&experiments_for_listener)
                    {
                        eprintln!("[sand:sharing] feature-gate reconcile failed: {error}");
                    }
                })));
            }
        }

        self.reconcile_feature_gate(&experiments)
    }

    pub fn stop(&self) {
        if let Some(stop) = self
            .notify_stop
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            stop();
        }
        if let Some(stop) = self
            .experiment_stop
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            stop();
        }
        self.service.stop();
    }
    pub fn is_enabled(&self) -> bool {
        self.service.get_state().is_enabled
    }
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
    pub fn get_sharing_state(&self) -> SandSharingState {
        self.service.get_state()
    }
    pub fn prepare_for_upgrade(&self) {
        self.service.stop()
    }
}
