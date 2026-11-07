//! The single Host-owned OpenClaw Gateway client transport.
//!
//! This owner is intentionally blocking and transport-local. Callers may run
//! it on the Host's dedicated network worker, but pairing, RPC projection, and
//! the inbound Fabushi messaging server never own this socket.

use std::collections::VecDeque;
use std::io;
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use tungstenite::http::{HeaderValue, Request};
use tungstenite::protocol::{Message, WebSocket, WebSocketConfig};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::client::connect_with_config;
use tungstenite::Error as WebSocketError;

use crate::sha256::sha256_hex;
use crate::host_secret_store::{
    HostNodeDeviceIdentity, HostNodeDeviceToken, load_or_create_host_node_identity,
    read_host_node_token,
};

use super::device_auth::{NodeHostDeviceAuthContext, build_device_auth_proof};
use super::candidate_connection::format_gateway_candidate_url;
use super::gateway_connection::{NodeHostGatewayConfig, gateway_origin_scope};
use super::gateway_frames::{
    NodeHostGatewayFrame, NodeHostGatewayRequestRegistry, ResponseSettlement,
    encode_gateway_request, parse_gateway_frame,
};
use super::hello_auth::persist_hello_ok_device_auth;
use super::invoke_protocol::{
    NodeInvokeCancel, NodeInvokeInput, NodeInvokeRequest, NodeInvokeResult,
    parse_node_invoke_cancel, parse_node_invoke_input, parse_node_invoke_request,
};
use super::invocation_registry::NodeInvocationAdmission;
use super::invocation_runtime::NodeHostInvocationRuntime;
use super::invocation_router::NodeHostInvocationRouter;
use super::optional_publication::{
    NodeHostOptionalPublicationHandle,
    NodeHostOptionalPublicationKind, NodeHostOptionalPublicationOutcome,
    NodeHostOptionalPublicationOwner, NodeHostOptionalPublicationRecord,
    NodeHostOptionalPublicationSettlement,
};
use super::runtime_state::NodeHostRuntimeState;
use super::runtime_manifest::NodeHostManifest;
use super::secret_materializer::{
    NodeHostCloudflareAccessCredentials, NodeHostCloudflareAccessInput,
    NodeHostSecretMaterializer, SharedNodeHostSecretMaterializer,
    validate_materializer_origin,
};
use super::session_registry::NodeHostSessionLease;

const CONNECT_REQUEST_ID_PREFIX: &str = "connect";
const DEFAULT_PROTOCOL: u16 = 3;
const MAX_PROTOCOL: u16 = 4;
const MAX_HELLO_BYTES: usize = 4 * 1024 * 1024;
const GATEWAY_IO_TIMEOUT_MS: u64 = 30_000;
const GATEWAY_SERVER_CAP_NODE_PERMISSION_ERRORS: &str = "node-permission-errors";

type GatewaySocket = WebSocket<MaybeTlsStream<TcpStream>>;

#[derive(Clone)]
pub struct NodeHostGatewayTransportConfig {
    pub gateway: NodeHostGatewayConfig,
    /// Opaque references only. Values are resolved for the WebSocket upgrade
    /// and never enter the live session state.
    pub access: Option<NodeHostCloudflareAccessInput>,
    pub identity_path: Option<PathBuf>,
    pub token_path: Option<PathBuf>,
    pub bootstrap_token: Option<String>,
    pub client_id: String,
    pub client_mode: String,
    pub role: String,
    pub scopes: Vec<String>,
    pub platform: String,
    pub device_family: String,
    pub client_version: String,
    pub display_name: Option<String>,
    pub commands: Vec<String>,
    pub capabilities: Vec<String>,
}

impl NodeHostGatewayTransportConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.gateway.validate()?;
        if self.access.is_some() && !self.gateway.tls {
            return Err("Cloudflare Access requires a TLS Gateway endpoint".into());
        }
        for (name, value) in [
            ("client id", self.client_id.as_str()),
            ("client mode", self.client_mode.as_str()),
            ("role", self.role.as_str()),
            ("platform", self.platform.as_str()),
            ("device family", self.device_family.as_str()),
            ("client version", self.client_version.as_str()),
        ] {
            let value = value.trim();
            if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
                return Err(format!("node Gateway {name} is invalid"));
            }
        }
        if self.scopes.iter().any(|scope| {
            let scope = scope.trim();
            scope.is_empty() || scope.len() > 256 || scope.chars().any(char::is_control)
        }) {
            return Err("node Gateway scope is invalid".into());
        }
        if self
            .bootstrap_token
            .as_deref()
            .is_some_and(|token| token.trim().is_empty() || token.len() > 256)
        {
            return Err("node Gateway bootstrap token is invalid".into());
        }
        Ok(())
    }

    fn origin_scope(&self) -> Result<String, String> {
        Ok(gateway_origin_scope(&format_gateway_candidate_url(
            &self.gateway,
            None,
        )?))
    }

    fn endpoint(&self) -> Result<String, String> {
        self.validate()?;
        let host = if self.gateway.host.contains(':')
            && !(self.gateway.host.starts_with('[') && self.gateway.host.ends_with(']'))
        {
            format!("[{}]", self.gateway.host)
        } else {
            self.gateway.host.clone()
        };
        let path = self
            .gateway
            .context_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty() && *path != "/")
            .map(|path| {
                if path.starts_with('/') {
                    path.to_string()
                } else {
                    format!("/{path}")
                }
            })
            .unwrap_or_default();
        Ok(format!(
            "{}://{host}:{}{path}",
            if self.gateway.tls { "wss" } else { "ws" },
            self.gateway.port
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHostGatewayHello {
    pub protocol: u16,
    pub capabilities: Vec<String>,
    pub methods: Vec<String>,
    pub events: Vec<String>,
}

#[derive(Debug)]
pub enum NodeHostGatewayTransportError {
    Config(String),
    Io(io::Error),
    WebSocket(WebSocketError),
    Protocol(String),
    Credential(io::Error),
}

impl std::fmt::Display for NodeHostGatewayTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(error) => write!(f, "Gateway transport configuration: {error}"),
            Self::Io(error) => write!(f, "Gateway transport I/O: {error}"),
            Self::WebSocket(error) => write!(f, "Gateway WebSocket: {error}"),
            Self::Protocol(error) => write!(f, "Gateway protocol: {error}"),
            Self::Credential(error) => write!(f, "Gateway credential store: {error}"),
        }
    }
}

impl std::error::Error for NodeHostGatewayTransportError {}

impl From<io::Error> for NodeHostGatewayTransportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<WebSocketError> for NodeHostGatewayTransportError {
    fn from(error: WebSocketError) -> Self {
        Self::WebSocket(error)
    }
}

pub struct NodeHostGatewayTransport {
    socket: GatewaySocket,
    config: NodeHostGatewayTransportConfig,
    identity: HostNodeDeviceIdentity,
    registry: NodeHostGatewayRequestRegistry,
    generation: u64,
    events: VecDeque<NodeHostGatewayFrame>,
    closed: bool,
}

pub struct NodeHostGatewaySession {
    transport: NodeHostGatewayTransport,
    runtime: std::sync::Arc<NodeHostRuntimeState>,
    lease: NodeHostSessionLease,
    hello: NodeHostGatewayHello,
    optional_publications: NodeHostOptionalPublicationOwner,
}

pub enum NodeHostGatewayInbound {
    InvokeRequest {
        request: NodeInvokeRequest,
        admission: NodeInvocationAdmission,
    },
    InvokeInput(NodeInvokeInput),
    InvokeCancel(NodeInvokeCancel),
}

pub enum NodeHostGatewayRoutedInbound {
    InvokeRequest(NodeInvokeRequest),
    InvokeInput { input: NodeInvokeInput, accepted: bool },
    InvokeCancel { cancel: NodeInvokeCancel, accepted: bool },
}

impl NodeHostGatewayTransport {
    pub fn connect_with_runtime(
        config: NodeHostGatewayTransportConfig,
        runtime: std::sync::Arc<NodeHostRuntimeState>,
    ) -> Result<NodeHostGatewaySession, NodeHostGatewayTransportError> {
        Self::connect_with_runtime_and_materializer(config, runtime, None)
    }

    pub fn connect_with_runtime_and_materializer(
        config: NodeHostGatewayTransportConfig,
        runtime: std::sync::Arc<NodeHostRuntimeState>,
        materializer: Option<&dyn NodeHostSecretMaterializer>,
    ) -> Result<NodeHostGatewaySession, NodeHostGatewayTransportError> {
        let node_id = load_or_create_host_node_identity(config.identity_path.as_deref())
            .map_err(NodeHostGatewayTransportError::Credential)?
            .device_id()
            .to_string();
        let display_name = config
            .display_name
            .clone()
            .unwrap_or_else(|| "OpenClaw Node Host".into());
        let metadata = super::session_registry::NodeHostSessionMetadata {
            node_id,
            display_name,
            platform: config.platform.clone(),
            device_family: config.device_family.clone(),
            commands: config.commands.clone(),
            capabilities: config.capabilities.clone(),
            paired: true,
            gateway_local: false,
        };
        let (transport, hello) = Self::connect_with_materializer(config, materializer)?;
        let lease = runtime.connect(metadata);
        if !runtime.set_active(&lease, true) {
            let _ = runtime.disconnect(&lease);
            return Err(NodeHostGatewayTransportError::Protocol(
                "Gateway session was retired during admission".into(),
            ));
        }
        let generation = transport.generation();
        Ok(NodeHostGatewaySession {
            transport,
            runtime,
            lease,
            hello,
            optional_publications: NodeHostOptionalPublicationOwner::new(generation)
                .map_err(|error| NodeHostGatewayTransportError::Protocol(error.to_string()))?,
        })
    }

    pub fn connect(
        config: NodeHostGatewayTransportConfig,
    ) -> Result<(Self, NodeHostGatewayHello), NodeHostGatewayTransportError> {
        Self::connect_with_materializer(config, None)
    }

    pub fn connect_with_shared_materializer(
        config: NodeHostGatewayTransportConfig,
        materializer: SharedNodeHostSecretMaterializer,
    ) -> Result<(Self, NodeHostGatewayHello), NodeHostGatewayTransportError> {
        Self::connect_with_materializer(config, Some(materializer.as_ref()))
    }

    fn connect_with_materializer(
        config: NodeHostGatewayTransportConfig,
        materializer: Option<&dyn NodeHostSecretMaterializer>,
    ) -> Result<(Self, NodeHostGatewayHello), NodeHostGatewayTransportError> {
        let endpoint = config.endpoint().map_err(NodeHostGatewayTransportError::Config)?;
        let identity = load_or_create_host_node_identity(config.identity_path.as_deref())
            .map_err(NodeHostGatewayTransportError::Credential)?;
        let mut websocket_config = WebSocketConfig::default();
        websocket_config.max_message_size = Some(MAX_HELLO_BYTES);
        websocket_config.max_frame_size = Some(MAX_HELLO_BYTES);
        let access = materialize_access(&config, materializer)?;
        let request = build_gateway_request(&endpoint, access.as_ref())?;
        let (mut socket, _) = connect_with_config(request, Some(websocket_config), 0)?;
        verify_gateway_tls_fingerprint(
            &socket,
            config.gateway.tls_fingerprint.as_deref(),
        )?;
        set_gateway_io_timeout(&mut socket, std::time::Duration::from_millis(GATEWAY_IO_TIMEOUT_MS))?;
        let mut transport = Self {
            socket,
            config,
            identity,
            registry: NodeHostGatewayRequestRegistry::new(1),
            generation: 1,
            events: VecDeque::new(),
            closed: false,
        };
        let hello = transport.handshake()?;
        Ok((transport, hello))
    }

    fn handshake(&mut self) -> Result<NodeHostGatewayHello, NodeHostGatewayTransportError> {
        let challenge = loop {
            match self.read_frame()? {
                NodeHostGatewayFrame::Event { event, payload, .. }
                    if event == "connect.challenge" =>
                {
                    break payload.ok_or_else(|| {
                        NodeHostGatewayTransportError::Protocol(
                            "connect.challenge payload is missing".into(),
                        )
                    })?;
                }
                NodeHostGatewayFrame::Event { .. } => continue,
                other => {
                    return Err(NodeHostGatewayTransportError::Protocol(format!(
                        "expected connect.challenge, received {}",
                        frame_kind(&other)
                    )))
                }
            }
        };
        let challenge = challenge.as_object().ok_or_else(|| {
            NodeHostGatewayTransportError::Protocol("connect.challenge is not an object".into())
        })?;
        let nonce = challenge
            .get("nonce")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|nonce| !nonce.is_empty())
            .ok_or_else(|| {
                NodeHostGatewayTransportError::Protocol(
                    "connect.challenge nonce is missing".into(),
                )
            })?;
        let signed_at_ms = challenge
            .get("ts")
            .and_then(Value::as_u64)
            .unwrap_or_else(now_millis);
        let origin = self
            .config
            .origin_scope()
            .map_err(NodeHostGatewayTransportError::Config)?;
        let cached_token = read_host_node_token(
            self.config.token_path.as_deref(),
            self.identity.device_id(),
            &self.config.role,
            Some(&origin),
        )
        .map_err(NodeHostGatewayTransportError::Credential)?;
        let (scopes, token) = match cached_token.as_ref() {
            Some(token) => (token.scopes().to_vec(), Some(token.secret().to_string())),
            None => (self.config.scopes.clone(), self.config.bootstrap_token.clone()),
        };
        let auth_context = NodeHostDeviceAuthContext {
            client_id: &self.config.client_id,
            client_mode: &self.config.client_mode,
            role: &self.config.role,
            scopes: &scopes,
            signed_at_ms,
            token: token.as_deref(),
            nonce,
            platform: Some(&self.config.platform),
            device_family: Some(&self.config.device_family),
        };
        let proof = build_device_auth_proof(&self.identity, &auth_context)
            .map_err(NodeHostGatewayTransportError::Protocol)?;
        let mut client = Map::new();
        client.insert("id".into(), json!(self.config.client_id.trim()));
        client.insert("version".into(), json!(self.config.client_version.trim()));
        client.insert("platform".into(), json!(self.config.platform.trim()));
        client.insert("deviceFamily".into(), json!(self.config.device_family.trim()));
        client.insert("mode".into(), json!(self.config.client_mode.trim()));
        if let Some(display_name) = self.config.display_name.as_deref() {
            if !display_name.trim().is_empty() {
                client.insert("displayName".into(), json!(display_name.trim()));
            }
        }
        let mut params = json!({
            "minProtocol": DEFAULT_PROTOCOL,
            "maxProtocol": MAX_PROTOCOL,
            "client": client,
            "caps": self.config.capabilities,
            "commands": self.config.commands,
            "role": self.config.role,
            "scopes": scopes,
            "device": proof,
        });
        if let Some(token) = token {
            params["auth"] = json!({ "deviceToken": token });
        }
        let (request_id, generation) = self
            .registry
            .admit(CONNECT_REQUEST_ID_PREFIX)
            .map_err(NodeHostGatewayTransportError::Protocol)?;
        self.send_request(&request_id, "connect", Some(params))?;
        let payload = match self.await_response(&request_id, generation)? {
            NodeHostGatewayFrame::Response { payload, .. } => payload.unwrap_or(Value::Null),
            other => {
                return Err(NodeHostGatewayTransportError::Protocol(format!(
                    "connect did not produce a response: {}",
                    frame_kind(&other)
                )))
            }
        };
        let hello = parse_hello(&payload)?;
        persist_hello_ok_device_auth(
            self.config.token_path.as_deref(),
            &self.identity,
            &payload,
            Some(&origin),
            cached_token.as_ref().map(HostNodeDeviceToken::secret),
        )
        .map_err(NodeHostGatewayTransportError::Credential)?;
        Ok(hello)
    }

    pub fn request(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<NodeHostGatewayFrame, NodeHostGatewayTransportError> {
        if self.closed {
            return Err(NodeHostGatewayTransportError::Protocol("Gateway transport is closed".into()));
        }
        let (id, generation) = self
            .registry
            .admit("request")
            .map_err(NodeHostGatewayTransportError::Protocol)?;
        self.send_request(&id, method, params)?;
        self.await_response(&id, generation)
    }

    /// Deliver one already-settled node invocation result. The caller owns
    /// the exactly-once lifecycle decision; this method only performs the
    /// single Gateway request and never retries it after transport loss.
    fn send_invoke_result(
        &mut self,
        result: NodeInvokeResult,
    ) -> Result<(), NodeHostGatewayTransportError> {
        let payload = serde_json::to_value(result).map_err(|error| {
            NodeHostGatewayTransportError::Protocol(format!(
                "serialize node.invoke.result: {error}"
            ))
        })?;
        let _ = self.request("node.invoke.result", Some(payload))?;
        Ok(())
    }

    pub fn next_event(&mut self) -> Option<NodeHostGatewayFrame> {
        self.events.pop_front()
    }

    /// Read one frame for the owning session loop. The caller must route event
    /// frames through `NodeHostGatewaySession::admit_event`; this method does
    /// not project presence or execute side effects.
    pub fn read_next_frame(&mut self) -> Result<NodeHostGatewayFrame, NodeHostGatewayTransportError> {
        if self.closed {
            return Err(NodeHostGatewayTransportError::Protocol("Gateway transport is closed".into()));
        }
        self.read_frame()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn close(&mut self) -> Result<(), NodeHostGatewayTransportError> {
        self.closed = true;
        self.socket.close(None)?;
        Ok(())
    }

    fn send_request(
        &mut self,
        id: &str,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), NodeHostGatewayTransportError> {
        let bytes = encode_gateway_request(id, method, params)
            .map_err(NodeHostGatewayTransportError::Protocol)?;
        let text = String::from_utf8(bytes)
            .map_err(|error| NodeHostGatewayTransportError::Protocol(error.to_string()))?;
        self.socket.send(Message::text(text))?;
        Ok(())
    }

    fn await_response(
        &mut self,
        id: &str,
        generation: u64,
    ) -> Result<NodeHostGatewayFrame, NodeHostGatewayTransportError> {
        loop {
            let frame = self.read_frame()?;
            match frame {
                NodeHostGatewayFrame::Response {
                    id: response_id,
                    ok,
                    payload,
                    error,
                } if response_id == id => {
                    match self.registry.settle(id, generation) {
                        ResponseSettlement::Current => {}
                        ResponseSettlement::StaleGeneration => {
                            return Err(NodeHostGatewayTransportError::Protocol(
                                "stale Gateway response generation".into(),
                            ))
                        }
                        ResponseSettlement::UnknownRequest => {
                            return Err(NodeHostGatewayTransportError::Protocol(
                                "unknown Gateway response id".into(),
                            ))
                        }
                    }
                    if ok {
                        return Ok(NodeHostGatewayFrame::Response {
                            id: response_id,
                            ok,
                            payload,
                            error,
                        });
                    }
                    let _ = error;
                    return Err(NodeHostGatewayTransportError::Protocol(
                        "Gateway request failed".into(),
                    ));
                }
                frame @ NodeHostGatewayFrame::Event { .. } => self.events.push_back(frame),
                NodeHostGatewayFrame::Response { id: other, .. } => {
                    return Err(NodeHostGatewayTransportError::Protocol(format!(
                        "unexpected Gateway response id: {other}"
                    )))
                }
                NodeHostGatewayFrame::Request { .. } => {
                    return Err(NodeHostGatewayTransportError::Protocol(
                        "unexpected server request during Gateway response".into(),
                    ))
                }
            }
        }
    }

    fn read_frame(&mut self) -> Result<NodeHostGatewayFrame, NodeHostGatewayTransportError> {
        let message = self.socket.read()?;
        match message {
            Message::Text(text) => parse_gateway_frame(text.as_str().as_bytes())
                .map_err(NodeHostGatewayTransportError::Protocol),
            Message::Binary(_) => Err(NodeHostGatewayTransportError::Protocol(
                "Gateway application frames must be text JSON".into(),
            )),
            Message::Ping(payload) => {
                self.socket.send(Message::Pong(payload))?;
                self.read_frame()
            }
            Message::Pong(_) => self.read_frame(),
            Message::Close(_) => {
                self.closed = true;
                Err(NodeHostGatewayTransportError::Protocol(
                    "Gateway closed the transport".into(),
                ))
            }
            Message::Frame(_) => self.read_frame(),
        }
    }

}

fn materialize_access(
    config: &NodeHostGatewayTransportConfig,
    materializer: Option<&dyn NodeHostSecretMaterializer>,
) -> Result<Option<NodeHostCloudflareAccessCredentials>, NodeHostGatewayTransportError> {
    let Some(input) = config.access.as_ref() else {
        return Ok(None);
    };
    let Some(materializer) = materializer else {
        return Err(NodeHostGatewayTransportError::Config(
            "Cloudflare Access requires an injected NodeHost SecretInput materializer".into(),
        ));
    };
    let origin = config
        .origin_scope()
        .map_err(NodeHostGatewayTransportError::Config)?;
    let credentials = materializer
        .materialize_cloudflare_access(&origin, input)
        .map_err(NodeHostGatewayTransportError::Config)?;
    validate_materializer_origin(&origin, credentials.origin_scope())
        .map_err(NodeHostGatewayTransportError::Config)?;
    Ok(Some(credentials))
}

fn build_gateway_request(
    endpoint: &str,
    access: Option<&NodeHostCloudflareAccessCredentials>,
) -> Result<Request<()>, NodeHostGatewayTransportError> {
    let mut builder = Request::builder().uri(endpoint);
    if let Some(access) = access {
        let client_id = HeaderValue::try_from(access.client_id()).map_err(|_| {
            NodeHostGatewayTransportError::Config(
                "Cloudflare Access client id cannot be encoded as a header".into(),
            )
        })?;
        let client_secret = HeaderValue::try_from(access.client_secret()).map_err(|_| {
            NodeHostGatewayTransportError::Config(
                "Cloudflare Access client secret cannot be encoded as a header".into(),
            )
        })?;
        builder = builder
            .header("CF-Access-Client-Id", client_id)
            .header("CF-Access-Client-Secret", client_secret);
    }
    builder.body(()).map_err(|error| {
        NodeHostGatewayTransportError::Config(format!(
            "failed to build Gateway WebSocket request: {error}"
        ))
    })
}

fn set_gateway_io_timeout(
    socket: &mut GatewaySocket,
    timeout: std::time::Duration,
) -> Result<(), NodeHostGatewayTransportError> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
        }
        MaybeTlsStream::Rustls(stream) => {
            stream.sock.set_read_timeout(Some(timeout))?;
            stream.sock.set_write_timeout(Some(timeout))?;
        }
        _ => {
            return Err(NodeHostGatewayTransportError::Config(
                "unsupported Gateway TLS stream implementation".into(),
            ));
        }
    }
    Ok(())
}

fn verify_gateway_tls_fingerprint(
    socket: &GatewaySocket,
    expected: Option<&str>,
) -> Result<(), NodeHostGatewayTransportError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let expected = expected.trim();
    let actual = match socket.get_ref() {
        MaybeTlsStream::Rustls(stream) => {
            let certificate = stream
                .conn
                .peer_certificates()
                .and_then(|certificates| certificates.first())
                .ok_or_else(|| {
                    NodeHostGatewayTransportError::Protocol(
                        "Gateway TLS peer certificate is missing".into(),
                    )
                })?;
            sha256_hex(certificate.as_ref())
        }
        MaybeTlsStream::Plain(_) => {
            return Err(NodeHostGatewayTransportError::Config(
                "Gateway TLS fingerprint was configured for a plaintext connection".into(),
            ));
        }
        _ => {
            return Err(NodeHostGatewayTransportError::Config(
                "Gateway TLS fingerprint is unsupported by this stream".into(),
            ));
        }
    };
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(NodeHostGatewayTransportError::Protocol(
            "Gateway TLS peer certificate fingerprint does not match".into(),
        ));
    }
    Ok(())
}

impl NodeHostGatewaySession {
    pub fn hello(&self) -> &NodeHostGatewayHello {
        &self.hello
    }

    /// Publish the current manifest owner snapshot into the live session
    /// projection. The caller must supply a freshly composed manifest; this
    /// method only applies the generation fence and stores its public fields.
    pub fn publish_manifest(&self, manifest: &NodeHostManifest) -> Result<(), String> {
        if self.runtime.update_manifest(
            &self.lease,
            manifest.advertised_commands.clone(),
            manifest.capabilities.clone(),
        ) {
            Ok(())
        } else {
            Err("Gateway session is no longer current for manifest publication".into())
        }
    }

    /// Admit a generation-bound optional publication payload. Payload
    /// selection remains with the future availability/plugin producer.
    pub fn begin_optional_publication(
        &mut self,
        kind: NodeHostOptionalPublicationKind,
        payload_json: String,
    ) -> Result<NodeHostOptionalPublicationHandle, String> {
        if !self.runtime.is_current(&self.lease) {
            return Err("Gateway session is no longer current".into());
        }
        self.optional_publications
            .begin(kind, payload_json)
            .map_err(|error| error.to_string())
    }

    /// Apply a producer/transport outcome only to the current publication
    /// attempt. This method does not retry or send anything itself.
    pub fn settle_optional_publication(
        &mut self,
        handle: &NodeHostOptionalPublicationHandle,
        outcome: NodeHostOptionalPublicationOutcome,
    ) -> NodeHostOptionalPublicationSettlement {
        if !self.runtime.is_current(&self.lease) {
            return NodeHostOptionalPublicationSettlement::Stale;
        }
        self.optional_publications.settle(handle, outcome)
    }

    pub fn current_optional_publication(
        &self,
        kind: NodeHostOptionalPublicationKind,
    ) -> Option<&NodeHostOptionalPublicationRecord> {
        if !self.runtime.is_current(&self.lease) {
            return None;
        }
        self.optional_publications.current(kind)
    }

    pub fn lease(&self) -> &NodeHostSessionLease {
        &self.lease
    }

    pub fn transport_mut(&mut self) -> &mut NodeHostGatewayTransport {
        &mut self.transport
    }

    /// Consume the next queued/live Gateway event and admit it through the
    /// session owner. Non-event frames are not silently ignored: a response
    /// belongs to a synchronous request owner, while a server request has no
    /// admitted NodeHost handler in this boundary.
    pub fn next_inbound(&mut self) -> Result<Option<NodeHostGatewayInbound>, String> {
        loop {
            let frame = match self.transport.next_event() {
                Some(frame) => frame,
                None => self
                    .transport
                    .read_next_frame()
                    .map_err(|error| error.to_string())?,
            };
            match frame {
                NodeHostGatewayFrame::Event { .. } => {
                    if let Some(inbound) = self.admit_event(&frame)? {
                        return Ok(Some(inbound));
                    }
                }
                NodeHostGatewayFrame::Response { .. } => {
                    return Err("Gateway response reached the event pump".into());
                }
                NodeHostGatewayFrame::Request { .. } => {
                    return Err("Gateway server request is not admitted by NodeHost".into());
                }
            }
        }
    }

    /// Route one admitted inbound item through the single invocation runtime
    /// owner. The request admission created by `admit_event` is reused; no
    /// second registry admission is performed.
    pub fn route_inbound(
        &self,
        router: &mut NodeHostInvocationRouter,
        inbound: NodeHostGatewayInbound,
    ) -> Result<NodeHostGatewayRoutedInbound, String> {
        match inbound {
            NodeHostGatewayInbound::InvokeRequest { request, admission } => {
                router.admit_request(
                    std::sync::Arc::clone(&self.runtime),
                    self.lease.clone(),
                    &request,
                    admission,
                )?;
                Ok(NodeHostGatewayRoutedInbound::InvokeRequest(request))
            }
            NodeHostGatewayInbound::InvokeInput(input) => {
                let accepted = router.dispatch_input(&input)?;
                Ok(NodeHostGatewayRoutedInbound::InvokeInput { input, accepted })
            }
            NodeHostGatewayInbound::InvokeCancel(cancel) => {
                let accepted = router.cancel(&cancel)?;
                Ok(NodeHostGatewayRoutedInbound::InvokeCancel { cancel, accepted })
            }
        }
    }

    /// Read and route the next event in one owner-bound operation.
    pub fn next_routed_inbound(
        &mut self,
        router: &mut NodeHostInvocationRouter,
    ) -> Result<Option<NodeHostGatewayRoutedInbound>, String> {
        self.next_inbound()?
            .map(|inbound| self.route_inbound(router, inbound))
            .transpose()
    }

    /// Complete the invocation under the current lease, then attempt exactly
    /// one result delivery. Completion happens first so a lost result cannot
    /// be replayed on a replacement connection. The transport error is
    /// returned for transport lifecycle evidence; callers must not retry it.
    pub fn complete_and_send_result(
        &mut self,
        invocation: &mut NodeHostInvocationRuntime,
        result: NodeInvokeResult,
    ) -> Result<(), String> {
        if !self.runtime.is_current(&self.lease) {
            return Err("Gateway session is no longer current".into());
        }
        let result = invocation.complete(result)?;
        let allow_permission_details = self
            .hello
            .capabilities
            .iter()
            .any(|capability| capability == GATEWAY_SERVER_CAP_NODE_PERMISSION_ERRORS);
        self.transport
            .send_invoke_result(result.without_permission_details(allow_permission_details))
            .map_err(|error| error.to_string())
    }

    pub fn admit_event(
        &self,
        frame: &NodeHostGatewayFrame,
    ) -> Result<Option<NodeHostGatewayInbound>, String> {
        if !self.runtime.is_current(&self.lease) {
            return Err("Gateway event belongs to a retired session".into());
        }
        let NodeHostGatewayFrame::Event { event, payload, .. } = frame else {
            return Ok(None);
        };
        let Some(payload) = payload else {
            return Err(format!("{event} event payload is missing"));
        };
        match event.as_str() {
            "node.invoke.request" => {
                let request = parse_node_invoke_request(payload)?;
                let admission = self.runtime.admit_request(&self.lease, &request)?;
                Ok(Some(NodeHostGatewayInbound::InvokeRequest {
                    request,
                    admission,
                }))
            }
            "node.invoke.input" => {
                let input = parse_node_invoke_input(payload)?;
                if input.node_id != self.lease.node_id {
                    return Err("node.invoke.input belongs to another node".into());
                }
                Ok(Some(NodeHostGatewayInbound::InvokeInput(input)))
            }
            "node.invoke.cancel" => {
                let cancel = parse_node_invoke_cancel(payload)?;
                if cancel.node_id != self.lease.node_id {
                    return Err("node.invoke.cancel belongs to another node".into());
                }
                Ok(Some(NodeHostGatewayInbound::InvokeCancel(cancel)))
            }
            _ => Ok(None),
        }
    }

    pub fn close(mut self) -> Result<(), NodeHostGatewayTransportError> {
        let retired_generation = self
            .optional_publications
            .generation()
            .saturating_add(1);
        let _ = self
            .optional_publications
            .retire_to(retired_generation);
        let _ = self.runtime.disconnect(&self.lease);
        self.transport.close()
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn parse_hello(payload: &Value) -> Result<NodeHostGatewayHello, NodeHostGatewayTransportError> {
    let object = payload.as_object().ok_or_else(|| {
        NodeHostGatewayTransportError::Protocol("hello-ok payload is not an object".into())
    })?;
    if object.get("type").and_then(Value::as_str) != Some("hello-ok") {
        return Err(NodeHostGatewayTransportError::Protocol(
            "Gateway connect response is not hello-ok".into(),
        ));
    }
    let protocol = object
        .get("protocol")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| NodeHostGatewayTransportError::Protocol("hello protocol is invalid".into()))?;
    if !(DEFAULT_PROTOCOL..=MAX_PROTOCOL).contains(&protocol) {
        return Err(NodeHostGatewayTransportError::Protocol(
            "hello protocol is outside the supported NodeHost window".into(),
        ));
    }
    let features = object
        .get("features")
        .and_then(Value::as_object)
        .ok_or_else(|| NodeHostGatewayTransportError::Protocol("hello features are missing".into()))?;
    let strings = |field: &str| -> Result<Vec<String>, NodeHostGatewayTransportError> {
        features
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| NodeHostGatewayTransportError::Protocol(format!("hello {field} are missing")))?
            .iter()
            .map(|value| {
                value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    NodeHostGatewayTransportError::Protocol(format!("hello {field} contains a non-string"))
                })
            })
            .collect()
    };
    Ok(NodeHostGatewayHello {
        protocol,
        capabilities: features
            .get("capabilities")
            .map(|_| strings("capabilities"))
            .transpose()?
            .unwrap_or_default(),
        methods: strings("methods")?,
        events: strings("events")?,
    })
}

fn frame_kind(frame: &NodeHostGatewayFrame) -> &'static str {
    match frame {
        NodeHostGatewayFrame::Request { .. } => "request",
        NodeHostGatewayFrame::Response { .. } => "response",
        NodeHostGatewayFrame::Event { .. } => "event",
    }
}
