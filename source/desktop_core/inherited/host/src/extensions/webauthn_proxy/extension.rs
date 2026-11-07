use std::sync::{Arc, mpsc::Sender};

use serde_json::Value;

use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::gateway_server::{GatewayBridgeClose, GatewayBridgeHub};

use super::webauthn_proxy_bridge::{
    SandWebAuthnBridge, SandWebAuthnBridgeError, WebAuthnProviderRegistration,
    WebAuthnProxyReportSink,
};
use super::webauthn_proxy_marker::apply_web_authn_proxy_marker;

pub const WEBAUTHN_PROXY_DEPENDENCIES: &[HostExtensionId] = &[HostExtensionId::Telemetry];

pub fn webauthn_proxy_extension_id() -> HostExtensionId {
    HostExtensionId::WebauthnProxy
}

#[derive(Clone)]
pub struct HostWebAuthnProxyExtension {
    bridge: SandWebAuthnBridge,
}

impl HostWebAuthnProxyExtension {
    pub fn request_ceremony(&self, ceremony: Value) -> Result<Value, SandWebAuthnBridgeError> {
        self.bridge.request_ceremony(ceremony)
    }

    pub fn register_provider(&self, send: Sender<Value>) -> WebAuthnProviderRegistration {
        self.bridge.register_provider(send)
    }

    pub fn submit_responses(&self, batch: Value) {
        self.bridge.submit_responses(batch);
    }

    pub fn apply_enablement(&self, enabled: bool) -> std::io::Result<&'static str> {
        apply_web_authn_proxy_marker(enabled)
    }

    pub fn provider_counts(&self) -> (usize, usize) {
        self.bridge.provider_counts()
    }

    pub fn gateway_bridge(self: &Arc<Self>) -> GatewayBridgeHub {
        let registration_owner = Arc::clone(self);
        let response_owner = Arc::clone(self);
        GatewayBridgeHub::with_handlers(
            move |send| {
                let registration = registration_owner.register_provider(send);
                Box::new(move || drop(registration)) as GatewayBridgeClose
            },
            move |batch| response_owner.submit_responses(batch),
        )
    }
}

pub fn start_webauthn_proxy_extension(
    report: WebAuthnProxyReportSink,
) -> HostWebAuthnProxyExtension {
    HostWebAuthnProxyExtension {
        bridge: SandWebAuthnBridge::production(report),
    }
}
