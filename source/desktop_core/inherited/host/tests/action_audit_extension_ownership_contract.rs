use std::sync::Arc;

use mahayana_host_runtime::extensions::action_audit::action_audit_backend::ActionAuditBackend;
use mahayana_host_runtime::extensions::action_audit::action_audit_service::SandActionAuditor;
use mahayana_host_runtime::extensions::action_audit::extension::ActionAuditExtension;

fn backend_constructor(
    service: Arc<SandActionAuditor>,
) -> ActionAuditBackend {
    ActionAuditBackend::new(service)
}

fn extension_constructor(
    service: Arc<SandActionAuditor>,
) -> ActionAuditExtension {
    ActionAuditExtension::new(service)
}

#[test]
fn action_audit_backend_and_extension_reuse_the_single_service_owner() {
    let _backend: fn(Arc<SandActionAuditor>) -> ActionAuditBackend = backend_constructor;
    let _extension: fn(Arc<SandActionAuditor>) -> ActionAuditExtension = extension_constructor;
}
