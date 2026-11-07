use std::sync::Arc;

use serde_json::Value;

use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::extensions::forever_box::ForeverBoxService;

use super::attachments_service::{
    AttachmentDiagnosticReporter, AttachmentsBox, AttachmentsService,
};
use super::generate_image_service::GenerateImageAuth;
use super::media_store::{MediaStoreLifecycleReporter, MediaStoreLifecycleWorker};

pub const ATTACHMENTS_EXTENSION_ID: HostExtensionId = HostExtensionId::Attachments;
pub const ATTACHMENTS_DEPENDENCIES: &[HostExtensionId] = &[
    HostExtensionId::Auth,
    HostExtensionId::ForeverBox,
    HostExtensionId::Telemetry,
];

pub struct HostAttachmentsExtension {
    service: Arc<AttachmentsService>,
    lifecycle_worker: Option<MediaStoreLifecycleWorker>,
}

impl HostAttachmentsExtension {
    pub fn new(service: Arc<AttachmentsService>) -> Self {
        Self {
            service,
            lifecycle_worker: None,
        }
    }

    pub fn start_lifecycle_worker(
        &mut self,
        report: Option<AttachmentDiagnosticReporter>,
    ) -> Result<(), String> {
        let reporter: Option<MediaStoreLifecycleReporter> = report.map(|report| {
            Arc::new(move |operation: &str, error: &str| {
                report(serde_json::json!({
                    "extension": "attachments",
                    "kind": "media_store_lifecycle_failed",
                    "operation": operation,
                    "error": error,
                }));
            }) as MediaStoreLifecycleReporter
        });
        self.lifecycle_worker = Some(MediaStoreLifecycleWorker::start(reporter)?);
        Ok(())
    }

    pub fn stop_lifecycle_worker(&mut self) {
        if let Some(worker) = self.lifecycle_worker.take() {
            worker.stop();
        }
    }

    pub fn service(&self) -> Arc<AttachmentsService> {
        Arc::clone(&self.service)
    }

    pub fn dispatch_gateway(
        &self,
        method: &str,
        args: &Value,
    ) -> Option<Result<Value, String>> {
        self.service.dispatch_gateway(method, args)
    }
}

pub fn start_attachments_extension(
    auth: Arc<HostAuthExtension>,
    box_: Arc<ForeverBoxService>,
    report: Option<AttachmentDiagnosticReporter>,
) -> Result<HostAttachmentsExtension, String> {
    let auth: Arc<dyn GenerateImageAuth> = auth;
    let box_: Arc<dyn AttachmentsBox> = box_;
    let lifecycle_report = report.clone();
    let mut extension = HostAttachmentsExtension::new(Arc::new(AttachmentsService::production(
        auth, box_, report,
    )));
    extension.start_lifecycle_worker(lifecycle_report)?;
    Ok(extension)
}
