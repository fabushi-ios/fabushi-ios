use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::extensions::box_store_sync::box_store_diagnostics::pin_box_store_diagnostics_reporter;
use crate::extensions::box_store_sync::chrome_session_stage::{
    CHROME_SESSION_STAGE_MAX_ATTEMPTS, CHROME_SESSION_STAGE_RETRY_DELAY_MS,
};
use crate::extensions::extension_ids_generated::HostExtensionId;

pub const BOX_STORE_SYNC_EXTENSION_ID: HostExtensionId = HostExtensionId::BoxStoreSync;
pub const BOX_STORE_SYNC_DEPENDENCIES: &[HostExtensionId] = &[
    HostExtensionId::Auth,
    HostExtensionId::Mcp,
    HostExtensionId::SourceMap,
    HostExtensionId::Telemetry,
];

pub const BOX_STORE_SYNC_INTERVAL_MS: u64 = 2 * 60_000;
pub const BOX_STORE_DB_DEBOUNCE_MS: u64 = 5_000;
pub const CHROME_SESSION_CHANGE_DEBOUNCE_MS: u64 = 5_000;
pub const BOX_STORE_MANIFEST_RETRY_ATTEMPTS: usize = 3;
pub const BOX_STORE_MANIFEST_RETRY_DELAY_MS: u64 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollingPolicyConfig {
    pub name: &'static str,
    pub interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebouncePolicyConfig {
    pub name: &'static str,
    pub delay_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryPredicate {
    CanonicalWriteConflict,
    ChromeSessionStageRetryable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicyConfig {
    pub name: &'static str,
    pub max_attempts: usize,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub predicate: RetryPredicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreScheduling {
    pub polling: PollingPolicyConfig,
    pub store_db_debounce: DebouncePolicyConfig,
    pub chrome_session_debounce: DebouncePolicyConfig,
    pub manifest_retry: RetryPolicyConfig,
    pub chrome_stage_retry: RetryPolicyConfig,
}

impl Default for BoxStoreScheduling {
    fn default() -> Self {
        Self {
            polling: PollingPolicyConfig {
                name: "box-store-sync-cycle",
                interval_ms: BOX_STORE_SYNC_INTERVAL_MS,
            },
            store_db_debounce: DebouncePolicyConfig {
                name: "box-store-sync-store-db",
                delay_ms: BOX_STORE_DB_DEBOUNCE_MS,
            },
            chrome_session_debounce: DebouncePolicyConfig {
                name: "box-store-sync-chrome-session",
                delay_ms: CHROME_SESSION_CHANGE_DEBOUNCE_MS,
            },
            manifest_retry: RetryPolicyConfig {
                name: "box-store-sync-manifest-cas",
                max_attempts: BOX_STORE_MANIFEST_RETRY_ATTEMPTS,
                initial_delay_ms: BOX_STORE_MANIFEST_RETRY_DELAY_MS,
                max_delay_ms: BOX_STORE_MANIFEST_RETRY_DELAY_MS,
                predicate: RetryPredicate::CanonicalWriteConflict,
            },
            chrome_stage_retry: RetryPolicyConfig {
                name: "box-store-sync-chrome-stage",
                max_attempts: CHROME_SESSION_STAGE_MAX_ATTEMPTS,
                initial_delay_ms: CHROME_SESSION_STAGE_RETRY_DELAY_MS,
                max_delay_ms: CHROME_SESSION_STAGE_RETRY_DELAY_MS,
                predicate: RetryPredicate::ChromeSessionStageRetryable,
            },
        }
    }
}

pub type DiagnosticReporter =
    Arc<dyn Fn(&Map<String, Value>) + Send + Sync + 'static>;
pub type BoxStoreSyncTelemetryReporter =
    Arc<dyn Fn(&str, &BTreeMap<String, String>) + Send + Sync + 'static>;
pub type StoreIdResolver =
    Arc<dyn Fn() -> Result<String, String> + Send + Sync + 'static>;
pub type AccessTokenResolver =
    Arc<dyn Fn() -> Result<String, String> + Send + Sync + 'static>;
pub type MachineIdResolver =
    Arc<dyn Fn() -> Result<String, String> + Send + Sync + 'static>;

#[derive(Clone)]
pub struct BoxStoreSyncExtensionDeps {
    pub native_credentials: Option<Arc<dyn crate::extensions::auth::auth_service::FabushiCoreCredentialProvider>>,
    pub is_idle: Arc<dyn Fn() -> bool + Send + Sync>,
    pub resolve_store_id: StoreIdResolver,
    pub backend_url: Option<String>,
    pub get_access_token: Option<AccessTokenResolver>,
    pub get_machine_id: Option<MachineIdResolver>,
    pub log: Arc<dyn Fn(&str) + Send + Sync>,
    pub report_host_extension_diagnostic: DiagnosticReporter,
    pub report_box_store_sync_cycle: BoxStoreSyncTelemetryReporter,
    pub report_box_store_db_capture: BoxStoreSyncTelemetryReporter,
    pub report_box_store_manifest_conflict: BoxStoreSyncTelemetryReporter,
    pub report_chrome_session_stage: BoxStoreSyncTelemetryReporter,
    pub scheduling: BoxStoreScheduling,
}

pub trait BoxStoreSyncService: Send + Sync + 'static {
    type Api: Clone + Send + Sync + 'static;

    fn api(&self) -> Self::Api;
    fn start(&self);
    fn dispose(&self);
}

pub trait BoxStoreSyncServiceFactory: Send + Sync {
    type Service: BoxStoreSyncService;

    fn create(&self, deps: BoxStoreSyncExtensionDeps) -> Self::Service;
}

pub struct BoxStoreSyncExtension<S: BoxStoreSyncService> {
    service: Arc<S>,
    diagnostics_pinned: bool,
}

impl<S: BoxStoreSyncService> BoxStoreSyncExtension<S> {
    pub fn api(&self) -> S::Api {
        self.service.api()
    }

    pub fn service(&self) -> &Arc<S> {
        &self.service
    }
}

impl<S: BoxStoreSyncService> Drop for BoxStoreSyncExtension<S> {
    fn drop(&mut self) {
        self.service.dispose();
        if self.diagnostics_pinned {
            pin_box_store_diagnostics_reporter(None);
            self.diagnostics_pinned = false;
        }
    }
}

pub fn start_box_store_sync_extension<F>(
    factory: &F,
    deps: BoxStoreSyncExtensionDeps,
) -> BoxStoreSyncExtension<F::Service>
where
    F: BoxStoreSyncServiceFactory,
{
    pin_box_store_diagnostics_reporter(Some(Arc::clone(
        &deps.report_host_extension_diagnostic,
    )));
    let service = Arc::new(factory.create(deps));
    service.start();
    BoxStoreSyncExtension {
        service,
        diagnostics_pinned: true,
    }
}
