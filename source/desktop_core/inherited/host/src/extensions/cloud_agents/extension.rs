use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::extension_ids_generated::HostExtensionId;

use super::cloud_agents_service::{
    CloudConversationTraceConverter, SandCloudAgentManager,
};

pub const CLOUD_AGENTS_EXTENSION_ID: HostExtensionId = HostExtensionId::CloudAgents;
pub const CLOUD_AGENTS_DEPENDENCIES: &[HostExtensionId] = &[HostExtensionId::Auth];

struct CloudAgentsExtensionInner {
    service: Arc<SandCloudAgentManager>,
    stopped: AtomicBool,
}

impl CloudAgentsExtensionInner {
    fn stop(&self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            self.service.dispose();
        }
    }
}

impl Drop for CloudAgentsExtensionInner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
pub struct CloudAgentsExtension {
    inner: Arc<CloudAgentsExtensionInner>,
}

impl CloudAgentsExtension {
    pub fn new(service: Arc<SandCloudAgentManager>) -> Self {
        Self {
            inner: Arc::new(CloudAgentsExtensionInner {
                service,
                stopped: AtomicBool::new(false),
            }),
        }
    }

    pub fn service(&self) -> Arc<SandCloudAgentManager> {
        Arc::clone(&self.inner.service)
    }

    pub fn stop(&self) {
        self.inner.stop();
    }
}

/// Starts the Host-owned Cloud Agents extension.
///
/// Auth and the generated ConversationMessage -> trace adapter stay explicit
/// dependencies, matching the frozen Host extension boundary. The service is
/// started once, eagerly refreshes team-admin policy, and is disposed exactly
/// once when Host teardown runs or when startup unwinds after a later failure.
pub fn start_cloud_agents_extension(
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    convert_conversation: CloudConversationTraceConverter,
) -> CloudAgentsExtension {
    let service = SandCloudAgentManager::production(
        backend_url,
        auth,
        convert_conversation,
    );
    service.prefetch_team_admin_policy();
    CloudAgentsExtension::new(service)
}
