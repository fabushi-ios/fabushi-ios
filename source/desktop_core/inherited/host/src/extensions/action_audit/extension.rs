use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::experiments::HostExperimentsExtension;
use crate::extensions::telemetry::HostTelemetryProjection;
use crate::extensions::telemetry::host_telemetry_service::HostStructuredLogTelemetry;
use crate::storage::agent_paths::{get_sand_agents_root_dir, resolve_sand_agent_dir};

use super::action_audit_backend::{
    ActionAuditBackend, create_sand_audit_batch_sender,
};
use super::action_audit_service::{
    ACTION_AUDIT_FLUSH_INTERVAL_MS, AuditSendError, SandActionAuditor, SendBatch,
};

#[derive(Clone)]
pub struct ActionAuditExtension {
    backend: ActionAuditBackend,
}

impl ActionAuditExtension {
    pub fn new(service: Arc<SandActionAuditor>) -> Self {
        Self {
            backend: ActionAuditBackend::new(service),
        }
    }

    pub fn backend(&self) -> &ActionAuditBackend {
        &self.backend
    }

    pub fn service(&self) -> &Arc<SandActionAuditor> {
        self.backend.service()
    }

    pub fn stop(&self) {
        self.service().dispose();
    }
}

pub fn start_action_audit_extension(
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    experiments: Arc<HostExperimentsExtension>,
    telemetry: HostStructuredLogTelemetry,
) -> ActionAuditExtension {
    let sender = create_sand_audit_batch_sender(backend_url, auth);
    let telemetry_sender = telemetry.clone();
    let send_batch: SendBatch = Arc::new(move |events| {
        let result = sender(events);
        if let Err(error) = &result {
            report_send_failure(&telemetry_sender, error);
        }
        result
    });
    let agents_root = get_sand_agents_root_dir(None);
    let audit_root = agents_root.clone();
    let service = Arc::new(SandActionAuditor::new(
        agents_root.join("audit-outbox.json"),
        Arc::new(move |agent_id| {
            resolve_sand_agent_dir(agent_id, None)
                .unwrap_or_else(|_| audit_root.join(agent_id))
                .join("audit.jsonl")
        }),
        Arc::new(move || experiments.check_feature_gate("sand_action_audit_logs")),
        send_batch,
        Duration::from_millis(ACTION_AUDIT_FLUSH_INTERVAL_MS),
    ));
    ActionAuditExtension::new(service)
}

fn report_send_failure(telemetry: &HostStructuredLogTelemetry, error: &AuditSendError) {
    let mut metadata = BTreeMap::new();
    metadata.insert("extension".into(), "action_audit".into());
    metadata.insert("errorClass".into(), "backend_send_failed".into());
    metadata.insert(
        "retryAfterMs".into(),
        error
            .retry_after_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
    );
    let _ = telemetry.report_projection(&HostTelemetryProjection {
        level: Some("warn"),
        event: Some("sand.action_audit"),
        metadata,
    });
}

#[allow(dead_code)]
fn _path_anchor(path: &Path) -> bool {
    path.is_absolute() || !path.as_os_str().is_empty()
}
