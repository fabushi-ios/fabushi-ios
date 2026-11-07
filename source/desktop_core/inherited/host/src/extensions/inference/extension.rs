use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::extensions::browser_ua::extension::StopSubscription;

use crate::extensions::extension_ids_generated::HostExtensionId;

pub const INFERENCE_DEPENDENCIES: &[HostExtensionId] = &[
    HostExtensionId::Auth,
    HostExtensionId::Experiments,
    HostExtensionId::Settings,
];

pub fn inference_extension_id() -> HostExtensionId {
    HostExtensionId::Inference
}

pub trait AgentInferenceOwner: Send + Sync {}

pub type ModelExperimentListener = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct ModelExperimentListeners {
    next_id: u64,
    listeners: BTreeMap<u64, ModelExperimentListener>,
}

pub struct InferenceExtensionRuntime<P> {
    port: P,
    peek_access_token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    listeners: Arc<Mutex<ModelExperimentListeners>>,
}

impl<P> InferenceExtensionRuntime<P> {
    pub fn new(
        port: P,
        peek_access_token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    ) -> Self {
        Self {
            port,
            peek_access_token,
            listeners: Arc::new(Mutex::new(ModelExperimentListeners::default())),
        }
    }

    pub fn is_ready(&self, mock_response_configured: bool) -> bool {
        mock_response_configured || (self.peek_access_token)().is_some()
    }

    pub fn port(&self) -> &P {
        &self.port
    }

    pub fn on_model_experiment_applied(
        &self,
        listener: ModelExperimentListener,
    ) -> StopSubscription {
        let id = {
            let mut state = self
                .listeners
                .lock()
                .expect("inference experiment listeners");
            state.next_id = state.next_id.saturating_add(1);
            let id = state.next_id;
            state.listeners.insert(id, listener);
            id
        };
        let listeners = Arc::downgrade(&self.listeners);
        Box::new(move || {
            if let Some(listeners) = listeners.upgrade() {
                listeners
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .listeners
                    .remove(&id);
            }
        })
    }

    pub fn notify_model_experiment_applied(&self) {
        let listeners = self
            .listeners
            .lock()
            .expect("inference experiment listeners")
            .listeners
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for listener in listeners {
            listener();
        }
    }
}
