use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::cursor_backend::create_cursor_checksum;
use crate::extensions::auth::credential_renewer::{
    SAND_CLIENT_TYPE, sand_box_namespace, sand_client_version,
};
use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::experiments::HostExperimentsExtension;
use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::host_event_bus::SandHostEventBus;

use super::codebase_telemetry_adapter::{
    CsnapsAdapterError, CsnapsCodebaseTelemetryAdapter, UploadCredentialsFactory,
};
use super::codebase_telemetry_host::{
    AdapterFactory, SandCodebaseTelemetryHost,
};
use super::codebase_telemetry_service::{
    CodebaseTelemetryHostApi, CodebaseTelemetryService,
    ServiceLog, create_sand_codebase_telemetry_logger,
};
use super::csnaps_capability::{
    CsnapsCapability, CsnapsUnavailableReason, resolve_csnaps_capability,
};
use super::privacy_mode::{
    CodebaseTelemetryPrivacyMode, create_sand_privacy_mode_loader,
};

pub const CODEBASE_TELEMETRY_DEPENDENCIES: &[HostExtensionId] =
    &[HostExtensionId::Auth, HostExtensionId::Experiments];
pub const CODEBASE_TELEMETRY_UPLOAD_INTERVAL: Duration =
    Duration::from_secs(5 * 60);
pub const CODEBASE_TELEMETRY_RESTART_DELAY: Duration =
    Duration::from_secs(30);
pub const CODEBASE_TELEMETRY_SHUTDOWN_DEADLINE: Duration =
    Duration::from_secs(30);
pub const CODEBASE_TELEMETRY_SNAPSHOTS_BASE_DIR: &str =
    "/var/lib/sand/telemetry/codebase";

pub fn codebase_telemetry_extension_id() -> HostExtensionId {
    HostExtensionId::CodebaseTelemetry
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodebaseTelemetryPaths {
    pub codebase_uuid_state_path: PathBuf,
    pub snapshots_base_dir: PathBuf,
}

pub fn codebase_telemetry_paths(sand_root: &Path) -> CodebaseTelemetryPaths {
    CodebaseTelemetryPaths {
        codebase_uuid_state_path: sand_root
            .join("telemetry")
            .join("codebase-uuids.json"),
        snapshots_base_dir: PathBuf::from(CODEBASE_TELEMETRY_SNAPSHOTS_BASE_DIR),
    }
}

pub fn build_upload_credentials(
    auth_token: &str,
    machine_id: &str,
    now_ms: u64,
) -> Map<String, Value> {
    let headers = Map::from_iter([
        (
            "x-cursor-checksum".to_string(),
            json!(create_cursor_checksum(machine_id, now_ms)),
        ),
        (
            "x-cursor-client-type".to_string(),
            json!(SAND_CLIENT_TYPE),
        ),
        (
            "x-cursor-client-version".to_string(),
            json!(sand_client_version()),
        ),
        (
            "x-sand-box-namespace".to_string(),
            json!(sand_box_namespace()),
        ),
        ("x-ghost-mode".to_string(), json!("false")),
    ]);
    Map::from_iter([
        ("authToken".to_string(), json!(auth_token)),
        ("requestHeaders".to_string(), Value::Object(headers)),
    ])
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub struct CodebaseTelemetryExtension {
    service: Option<Arc<CodebaseTelemetryService>>,
    unavailable: Option<(PathBuf, CsnapsUnavailableReason)>,
}

impl CodebaseTelemetryExtension {
    pub fn disabled(
        executable_path: PathBuf,
        reason: CsnapsUnavailableReason,
    ) -> Self {
        Self {
            service: None,
            unavailable: Some((executable_path, reason)),
        }
    }

    pub fn available(&self) -> bool {
        self.service.is_some()
    }

    pub fn unavailable_reason(
        &self,
    ) -> Option<(&Path, CsnapsUnavailableReason)> {
        self.unavailable
            .as_ref()
            .map(|(path, reason)| (path.as_path(), *reason))
    }

    pub fn flush_pending_uploads(&self) -> Result<(), CsnapsAdapterError> {
        match &self.service {
            Some(service) => service.flush_pending_uploads(),
            None => Ok(()),
        }
    }

    pub fn stop(&self) {
        if let Some(service) = &self.service {
            service.dispose();
        }
    }
}

impl Drop for CodebaseTelemetryExtension {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_codebase_telemetry_extension(
    auth: Arc<HostAuthExtension>,
    experiments: Arc<HostExperimentsExtension>,
    events: SandHostEventBus,
    sand_root: &Path,
    backend_url: String,
) -> Result<CodebaseTelemetryExtension, String> {
    start_codebase_telemetry_extension_with_capability(
        auth,
        experiments,
        events,
        sand_root,
        backend_url,
        resolve_csnaps_capability(),
    )
}

pub fn start_codebase_telemetry_extension_with_capability(
    auth: Arc<HostAuthExtension>,
    experiments: Arc<HostExperimentsExtension>,
    events: SandHostEventBus,
    sand_root: &Path,
    backend_url: String,
    capability: CsnapsCapability,
) -> Result<CodebaseTelemetryExtension, String> {
    let executable_path = match capability {
        CsnapsCapability::Available { executable_path } => executable_path,
        CsnapsCapability::Unavailable {
            executable_path,
            reason,
        } => {
            eprintln!(
                "mahayana-host-codebase-telemetry unavailable: csnaps {} at {}",
                reason.as_str(),
                executable_path.display()
            );
            return Ok(CodebaseTelemetryExtension::disabled(
                executable_path,
                reason,
            ));
        }
    };

    let paths = codebase_telemetry_paths(sand_root);
    let auth_for_privacy = Arc::clone(&auth);
    let privacy = Arc::new(CodebaseTelemetryPrivacyMode::new(
        create_sand_privacy_mode_loader(
            backend_url.clone(),
            Arc::new(move || {
                auth_for_privacy
                    .get_machine_id()
                    .map_err(|error| error.to_string())
            }),
        ),
    ));

    let adapter_auth = Arc::clone(&auth);
    let adapter_backend = backend_url.clone();
    let adapter_bin = executable_path.clone();
    let adapter_paths = paths.clone();
    let adapter_factory: AdapterFactory = Arc::new(move |credentials| {
        let upload_auth = Arc::clone(&adapter_auth);
        let upload_token = credentials.auth_token.clone();
        let upload_credentials: UploadCredentialsFactory = Arc::new(move || {
            let machine_id = upload_auth
                .get_machine_id()
                .map_err(|error| CsnapsAdapterError::new(error.to_string()))?;
            Ok(build_upload_credentials(
                &upload_token,
                &machine_id,
                system_now_ms(),
            ))
        });
        CsnapsCodebaseTelemetryAdapter::create(
            credentials.auth_id,
            &adapter_paths.codebase_uuid_state_path,
            &adapter_paths.snapshots_base_dir,
            &adapter_backend,
            &adapter_bin,
            upload_credentials,
            CODEBASE_TELEMETRY_UPLOAD_INTERVAL,
        )
    });

    let host = SandCodebaseTelemetryHost::new(
        auth,
        experiments,
        privacy,
        adapter_factory,
    );
    let host_api: Arc<dyn CodebaseTelemetryHostApi> = host;
    let raw_log: ServiceLog = Arc::new(|message| {
        eprintln!("mahayana-host-codebase-telemetry {message}");
    });
    let logger = create_sand_codebase_telemetry_logger(raw_log);
    let service = CodebaseTelemetryService::start(
        host_api,
        events,
        CODEBASE_TELEMETRY_RESTART_DELAY,
        CODEBASE_TELEMETRY_SHUTDOWN_DEADLINE,
        logger,
    );

    Ok(CodebaseTelemetryExtension {
        service: Some(service),
        unavailable: None,
    })
}
