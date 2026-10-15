use async_trait::async_trait;
use mahayana_kernel::{
    ApprovalResolution, BackendDescriptor, EngineBackend, KernelError, KernelEvent,
    KernelEventSink, OpenSessionRequest, OperationId, ResumeOperationRequest, RunRequest,
    SessionId, SessionSnapshot, SharedKernelEventSink, SuspendOperationRequest,
};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const PROVIDER_FABUSHI: &str = "fabushi";
pub const PROVIDER_OPENROUTER: &str = "openrouter";
pub const PROVIDER_CLAUDE_CODE: &str = "claude-code";
pub const PROVIDER_CODEX: &str = "codex";

fn normalize_provider(value: &str) -> Option<&'static str> {
    match value.trim() {
        PROVIDER_FABUSHI => Some(PROVIDER_FABUSHI),
        PROVIDER_OPENROUTER => Some(PROVIDER_OPENROUTER),
        PROVIDER_CLAUDE_CODE => Some(PROVIDER_CLAUDE_CODE),
        PROVIDER_CODEX => Some(PROVIDER_CODEX),
        _ => None,
    }
}

fn lock<T>(value: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, KernelError> {
    value
        .lock()
        .map_err(|_| KernelError::Backend("provider router mutex poisoned".into()))
}

fn metadata_provider(metadata: &Value, default_provider: &str) -> String {
    metadata
        .get("inferenceProvider")
        .and_then(Value::as_str)
        .and_then(normalize_provider)
        .unwrap_or(default_provider)
        .to_string()
}

fn with_provider_metadata(metadata: &mut Value, provider: &str) {
    if !metadata.is_object() {
        *metadata = Value::Object(Map::new());
    }
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "inferenceProvider".into(),
            Value::String(provider.to_string()),
        );
    }
}

/// Routes one provider-neutral Mahayana conversation backend to a concrete
/// model/Agent engine per Agent session. The route is selected only at
/// open_session, then remains stable for the lifetime of that session.
///
/// This is deliberately below the renderer and conversation provider. Two
/// Agents can therefore run concurrently on different inference providers
/// without restarting the Host or sharing operation/approval state.
pub struct ProviderRoutingEngineBackend {
    default_provider: String,
    backends: HashMap<String, Arc<dyn EngineBackend>>,
    descriptor: BackendDescriptor,
    sessions: Mutex<HashMap<String, String>>,
    operations: Arc<Mutex<HashMap<String, String>>>,
    approvals: Arc<Mutex<HashMap<String, String>>>,
}

impl ProviderRoutingEngineBackend {
    pub fn new(
        default_provider: impl Into<String>,
        backends: HashMap<String, Arc<dyn EngineBackend>>,
    ) -> Result<Self, KernelError> {
        let requested_default = normalize_provider(&default_provider.into())
            .unwrap_or(PROVIDER_FABUSHI)
            .to_string();
        let default_provider = if backends.contains_key(&requested_default) {
            requested_default
        } else {
            PROVIDER_FABUSHI.to_string()
        };
        let default_backend = backends.get(&default_provider)
            .ok_or_else(|| KernelError::BackendUnavailable(
                "provider router requires a Fabushi/default backend".into(),
            ))?;
        let mut descriptor = default_backend.descriptor();
        descriptor.id = "mahayana-provider-router".into();
        descriptor.display_name = "Mahayana per-Agent inference router".into();
        descriptor.native = true;
        Ok(Self {
            default_provider,
            backends,
            descriptor,
            sessions: Mutex::new(HashMap::new()),
            operations: Arc::new(Mutex::new(HashMap::new())),
            approvals: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn backend(&self, provider: &str) -> Result<Arc<dyn EngineBackend>, KernelError> {
        self.backends.get(provider).cloned().ok_or_else(|| {
            KernelError::BackendUnavailable(format!(
                "inference provider '{provider}' is not configured on this device"
            ))
        })
    }

    fn provider_for_session(&self, session_id: &SessionId) -> Result<String, KernelError> {
        lock(&self.sessions)?
            .get(session_id.as_str())
            .cloned()
            .ok_or_else(|| KernelError::SessionNotFound(session_id.as_str().to_string()))
    }

    fn provider_for_operation(&self, operation_id: &OperationId) -> Result<String, KernelError> {
        lock(&self.operations)?
            .get(operation_id.as_str())
            .cloned()
            .ok_or_else(|| KernelError::OperationNotFound(operation_id.as_str().to_string()))
    }
}

struct RoutedEventSink {
    provider: String,
    inner: SharedKernelEventSink,
    approvals: Arc<Mutex<HashMap<String, String>>>,
}

impl KernelEventSink for RoutedEventSink {
    fn emit(&self, mut event: KernelEvent) -> Result<(), KernelError> {
        if let KernelEvent::ApprovalRequested {
            approval_id,
            details,
            ..
        } = &mut event
        {
            lock(&self.approvals)?.insert(approval_id.clone(), self.provider.clone());
            if !details.is_object() {
                *details = Value::Object(Map::new());
            }
            if let Some(object) = details.as_object_mut() {
                object.insert(
                    "inferenceProvider".into(),
                    Value::String(self.provider.clone()),
                );
            }
        }
        if let KernelEvent::Activity { metadata, .. } = &mut event {
            if !metadata.is_object() {
                *metadata = Value::Object(Map::new());
            }
            if let Some(object) = metadata.as_object_mut() {
                object.insert(
                    "inferenceProvider".into(),
                    Value::String(self.provider.clone()),
                );
            }
        }
        self.inner.emit(event)
    }
}

#[async_trait]
impl EngineBackend for ProviderRoutingEngineBackend {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    async fn open_session(
        &self,
        mut request: OpenSessionRequest,
    ) -> Result<SessionId, KernelError> {
        let provider = metadata_provider(&request.metadata, &self.default_provider);
        let backend = self.backend(&provider)?;
        with_provider_metadata(&mut request.metadata, &provider);
        let session_id = backend.open_session(request).await?;
        lock(&self.sessions)?.insert(session_id.as_str().to_string(), provider);
        Ok(session_id)
    }

    async fn run(
        &self,
        mut request: RunRequest,
        events: SharedKernelEventSink,
    ) -> Result<(), KernelError> {
        let provider = self.provider_for_session(&request.session_id)?;
        let backend = self.backend(&provider)?;
        with_provider_metadata(&mut request.metadata, &provider);
        lock(&self.operations)?
            .insert(request.operation_id.as_str().to_string(), provider.clone());
        let sink: SharedKernelEventSink = Arc::new(RoutedEventSink {
            provider: provider.clone(),
            inner: events,
            approvals: Arc::clone(&self.approvals),
        });
        let operation_id = request.operation_id.clone();
        let result = backend.run(request, sink).await;
        lock(&self.operations)?.remove(operation_id.as_str());
        result
    }

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), KernelError> {
        let provider = self.provider_for_operation(operation_id)?;
        self.backend(&provider)?.interrupt(operation_id).await
    }

    async fn resolve_approval(&self, resolution: ApprovalResolution) -> Result<(), KernelError> {
        let provider = lock(&self.approvals)?
            .get(&resolution.approval_id)
            .cloned()
            .ok_or_else(|| KernelError::ApprovalNotFound(resolution.approval_id.clone()))?;
        let result = self.backend(&provider)?.resolve_approval(resolution.clone()).await;
        if result.is_ok() {
            lock(&self.approvals)?.remove(&resolution.approval_id);
        }
        result
    }

    fn reset_session(&self) -> Result<(), KernelError> {
        for backend in self.backends.values() {
            backend.reset_session()?;
        }
        lock(&self.sessions)?.clear();
        lock(&self.operations)?.clear();
        lock(&self.approvals)?.clear();
        Ok(())
    }

    async fn snapshot_session(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionSnapshot, KernelError> {
        let provider = self.provider_for_session(session_id)?;
        let mut snapshot = self.backend(&provider)?.snapshot_session(session_id).await?;
        with_provider_metadata(&mut snapshot.metadata, &provider);
        Ok(snapshot)
    }

    async fn restore_session(
        &self,
        mut snapshot: SessionSnapshot,
    ) -> Result<SessionId, KernelError> {
        let provider = metadata_provider(&snapshot.metadata, &self.default_provider);
        let backend = self.backend(&provider)?;
        with_provider_metadata(&mut snapshot.metadata, &provider);
        let session_id = backend.restore_session(snapshot).await?;
        lock(&self.sessions)?.insert(session_id.as_str().to_string(), provider);
        Ok(session_id)
    }

    async fn suspend_operation(
        &self,
        request: SuspendOperationRequest,
    ) -> Result<(), KernelError> {
        let provider = self.provider_for_operation(&request.operation_id)?;
        self.backend(&provider)?.suspend_operation(request).await
    }

    async fn resume_operation(
        &self,
        mut request: ResumeOperationRequest,
        events: SharedKernelEventSink,
    ) -> Result<(), KernelError> {
        let provider = self.provider_for_session(&request.session_id)?;
        let backend = self.backend(&provider)?;
        with_provider_metadata(&mut request.metadata, &provider);
        lock(&self.operations)?
            .insert(request.operation_id.as_str().to_string(), provider.clone());
        let sink: SharedKernelEventSink = Arc::new(RoutedEventSink {
            provider,
            inner: events,
            approvals: Arc::clone(&self.approvals),
        });
        let operation_id = request.operation_id.clone();
        let result = backend.resume_operation(request, sink).await;
        lock(&self.operations)?.remove(operation_id.as_str());
        result
    }
}

#[cfg(feature = "codex-compat")]
mod codex {
    use super::*;
    use mahayana_agent::AgentBackend;
    use mahayana_agent_codex::{CodexAgentBackend, CodexAgentConfig};
    use mahayana_agent_kernel_bridge::LegacyAgentKernelBridge;
    use mahayana_kernel::{Capability, CapabilitySet};
    use tokio::sync::OnceCell;

    pub struct LazyCodexEngineBackend {
        config: CodexAgentConfig,
        backend: OnceCell<Arc<dyn EngineBackend>>,
        descriptor: BackendDescriptor,
    }

    impl LazyCodexEngineBackend {
        pub fn new(config: CodexAgentConfig) -> Self {
            Self {
                config,
                backend: OnceCell::new(),
                descriptor: BackendDescriptor {
                    id: "compat:codex".into(),
                    display_name: "Codex local account compatibility backend".into(),
                    native: false,
                    capabilities: CapabilitySet::new([
                        Capability::Model,
                        Capability::FilesystemRead,
                        Capability::FilesystemWrite,
                        Capability::Process,
                        Capability::Git,
                        Capability::Network,
                        Capability::WebSearch,
                        Capability::ComputerUse,
                        Capability::ToolProtocol,
                        Capability::Mcp,
                        Capability::Skills,
                        Capability::Plugins,
                    ]),
                },
            }
        }

        async fn backend(&self) -> Result<Arc<dyn EngineBackend>, KernelError> {
            let config = self.config.clone();
            let descriptor = self.descriptor.clone();
            self.backend
                .get_or_try_init(|| async move {
                    let agent: Arc<dyn AgentBackend> = Arc::new(
                        CodexAgentBackend::start(config)
                            .await
                            .map_err(|error| KernelError::BackendUnavailable(error.to_string()))?,
                    );
                    Ok(Arc::new(LegacyAgentKernelBridge::new(agent, descriptor))
                        as Arc<dyn EngineBackend>)
                })
                .await
                .cloned()
        }
    }

    #[async_trait]
    impl EngineBackend for LazyCodexEngineBackend {
        fn descriptor(&self) -> BackendDescriptor {
            self.descriptor.clone()
        }

        async fn open_session(
            &self,
            request: OpenSessionRequest,
        ) -> Result<SessionId, KernelError> {
            self.backend().await?.open_session(request).await
        }

        async fn run(
            &self,
            request: RunRequest,
            events: SharedKernelEventSink,
        ) -> Result<(), KernelError> {
            self.backend().await?.run(request, events).await
        }

        async fn interrupt(&self, operation_id: &OperationId) -> Result<(), KernelError> {
            self.backend().await?.interrupt(operation_id).await
        }

        async fn resolve_approval(
            &self,
            resolution: ApprovalResolution,
        ) -> Result<(), KernelError> {
            self.backend().await?.resolve_approval(resolution).await
        }

        fn reset_session(&self) -> Result<(), KernelError> {
            match self.backend.get() {
                Some(backend) => backend.reset_session(),
                None => Ok(()),
            }
        }

        async fn snapshot_session(
            &self,
            session_id: &SessionId,
        ) -> Result<SessionSnapshot, KernelError> {
            self.backend().await?.snapshot_session(session_id).await
        }

        async fn restore_session(
            &self,
            snapshot: SessionSnapshot,
        ) -> Result<SessionId, KernelError> {
            self.backend().await?.restore_session(snapshot).await
        }

        async fn suspend_operation(
            &self,
            request: SuspendOperationRequest,
        ) -> Result<(), KernelError> {
            self.backend().await?.suspend_operation(request).await
        }

        async fn resume_operation(
            &self,
            request: ResumeOperationRequest,
            events: SharedKernelEventSink,
        ) -> Result<(), KernelError> {
            self.backend().await?.resume_operation(request, events).await
        }
    }

    pub use LazyCodexEngineBackend as PublicLazyCodexEngineBackend;
}

#[cfg(feature = "codex-compat")]
pub use codex::PublicLazyCodexEngineBackend as LazyCodexEngineBackend;

#[cfg(test)]
mod tests {
    use super::*;
    use mahayana_kernel::{Capability, CapabilitySet, ExecutionPolicy};

    struct EchoBackend {
        id: &'static str,
    }

    #[async_trait]
    impl EngineBackend for EchoBackend {
        fn descriptor(&self) -> BackendDescriptor {
            BackendDescriptor {
                id: self.id.into(),
                display_name: self.id.into(),
                native: true,
                capabilities: CapabilitySet::new([Capability::Model]),
            }
        }

        async fn open_session(
            &self,
            _request: OpenSessionRequest,
        ) -> Result<SessionId, KernelError> {
            Ok(SessionId::new())
        }

        async fn run(
            &self,
            request: RunRequest,
            events: SharedKernelEventSink,
        ) -> Result<(), KernelError> {
            events.emit(KernelEvent::Activity {
                operation_id: request.operation_id.clone(),
                kind: "test".into(),
                title: self.id.into(),
                detail: None,
                metadata: Value::Null,
            })?;
            events.emit(KernelEvent::OperationCompleted {
                operation_id: request.operation_id,
            })
        }

        async fn interrupt(&self, _operation_id: &OperationId) -> Result<(), KernelError> {
            Ok(())
        }

        async fn resolve_approval(
            &self,
            _resolution: ApprovalResolution,
        ) -> Result<(), KernelError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CollectSink(Mutex<Vec<KernelEvent>>);
    impl KernelEventSink for CollectSink {
        fn emit(&self, event: KernelEvent) -> Result<(), KernelError> {
            lock(&self.0)?.push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn routes_sessions_to_their_agent_provider_without_global_state() {
        let mut backends: HashMap<String, Arc<dyn EngineBackend>> = HashMap::new();
        backends.insert(PROVIDER_FABUSHI.into(), Arc::new(EchoBackend { id: "fabushi-test" }));
        backends.insert(PROVIDER_OPENROUTER.into(), Arc::new(EchoBackend { id: "openrouter-test" }));
        let router = ProviderRoutingEngineBackend::new(PROVIDER_FABUSHI, backends).expect("router");

        let left = router.open_session(OpenSessionRequest {
            profile: mahayana_kernel::RuntimeProfile::DesktopFull,
            workspace_root: None,
            model: None,
            metadata: serde_json::json!({"inferenceProvider": "fabushi"}),
        }).await.expect("left session");
        let right = router.open_session(OpenSessionRequest {
            profile: mahayana_kernel::RuntimeProfile::DesktopFull,
            workspace_root: None,
            model: None,
            metadata: serde_json::json!({"inferenceProvider": "openrouter"}),
        }).await.expect("right session");

        let left_sink = Arc::new(CollectSink::default());
        let right_sink = Arc::new(CollectSink::default());
        router.run(RunRequest {
            session_id: left,
            operation_id: OperationId::from_string("operation:left"),
            input: "left".into(),
            policy: ExecutionPolicy::default(),
            required_capabilities: CapabilitySet::new([Capability::Model]),
            metadata: Value::Null,
        }, left_sink.clone()).await.expect("left run");
        router.run(RunRequest {
            session_id: right,
            operation_id: OperationId::from_string("operation:right"),
            input: "right".into(),
            policy: ExecutionPolicy::default(),
            required_capabilities: CapabilitySet::new([Capability::Model]),
            metadata: Value::Null,
        }, right_sink.clone()).await.expect("right run");

        let left_events = lock(&left_sink.0).expect("left events");
        let right_events = lock(&right_sink.0).expect("right events");
        assert!(left_events.iter().any(|event| matches!(
            event,
            KernelEvent::Activity { title, metadata, .. }
                if title == "fabushi-test"
                    && metadata.get("inferenceProvider").and_then(Value::as_str) == Some("fabushi")
        )));
        assert!(right_events.iter().any(|event| matches!(
            event,
            KernelEvent::Activity { title, metadata, .. }
                if title == "openrouter-test"
                    && metadata.get("inferenceProvider").and_then(Value::as_str) == Some("openrouter")
        )));
    }
}
