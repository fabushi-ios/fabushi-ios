use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_BOX_PING_TIMEOUT_MS: u64 = 1_500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxEndpoint {
    pub host: String,
    pub port: u16,
    pub auth_token: String,
    pub headers: BTreeMap<String, String>,
}

impl BoxEndpoint {
    pub fn new(host: impl Into<String>, port: u16, auth_token: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port,
            auth_token: auth_token.into(),
            headers: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxAuthorizationInterceptor {
    headers: BTreeMap<String, String>,
}

impl BoxAuthorizationInterceptor {
    pub fn apply(&self, request_headers: &mut BTreeMap<String, String>) {
        for (name, value) in &self.headers {
            request_headers.insert(name.clone(), value.clone());
        }
    }
}

pub fn create_box_authorization_interceptor(
    endpoint: &BoxEndpoint,
) -> BoxAuthorizationInterceptor {
    let mut headers = BTreeMap::from([(
        "Authorization".into(),
        format!("Bearer {}", endpoint.auth_token),
    )]);
    for (name, value) in &endpoint.headers {
        headers.insert(name.clone(), value.clone());
    }
    BoxAuthorizationInterceptor { headers }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxTransportOptions {
    pub http_version: &'static str,
    pub base_url: String,
    pub use_binary_format: bool,
    pub interceptors: Vec<BoxAuthorizationInterceptor>,
}

pub fn create_box_transport<Transport, Factory>(
    endpoint: &BoxEndpoint,
    factory: Factory,
) -> Transport
where
    Factory: FnOnce(BoxTransportOptions) -> Transport,
{
    factory(BoxTransportOptions {
        http_version: "1.1",
        base_url: format!("http://{}:{}", endpoint.host, endpoint.port),
        use_binary_format: true,
        interceptors: vec![create_box_authorization_interceptor(endpoint)],
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectCode {
    Number(i64),
    Text(String),
}

pub trait BoxPingErrorMetadata: Error {
    fn connect_code(&self) -> Option<ConnectCode> {
        None
    }

    fn system_errno(&self) -> Option<&str> {
        None
    }
}

fn connect_code_name(code: Option<&ConnectCode>) -> String {
    match code {
        Some(ConnectCode::Number(code)) => match *code {
            1 => "Canceled",
            2 => "Unknown",
            3 => "InvalidArgument",
            4 => "DeadlineExceeded",
            5 => "NotFound",
            6 => "AlreadyExists",
            7 => "PermissionDenied",
            8 => "ResourceExhausted",
            9 => "FailedPrecondition",
            10 => "Aborted",
            11 => "OutOfRange",
            12 => "Unimplemented",
            13 => "Internal",
            14 => "Unavailable",
            15 => "DataLoss",
            16 => "Unauthenticated",
            _ => "unknown",
        }
        .into(),
        Some(ConnectCode::Text(code)) if !code.is_empty() => code.clone(),
        _ => "unknown".into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedPingFailure {
    pub outcome: &'static str,
    pub cause_summary: String,
}

pub fn classify_ping_failure<ErrorType>(error: &ErrorType) -> ClassifiedPingFailure
where
    ErrorType: BoxPingErrorMetadata,
{
    let code = error.connect_code();
    let code_name = connect_code_name(code.as_ref());
    let cause_summary = match error.system_errno() {
        Some(errno) => format!("{code_name}/{errno}"),
        None => code_name,
    };
    let message = error.to_string();
    let deadline = matches!(code, Some(ConnectCode::Number(4)))
        || message.to_ascii_lowercase().contains("deadline");
    if deadline {
        return ClassifiedPingFailure {
            outcome: "timeout",
            cause_summary,
        };
    }
    let refused = error.system_errno() == Some("ECONNREFUSED")
        || message
            .to_ascii_uppercase()
            .contains("ECONNREFUSED");
    if refused {
        return ClassifiedPingFailure {
            outcome: "refused",
            cause_summary,
        };
    }
    ClassifiedPingFailure {
        outcome: "crash",
        cause_summary,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxPingOutcome {
    pub outcome: &'static str,
    pub latency_ms: u64,
    pub cause_summary: Option<String>,
}

pub trait BoxPingControlClient<Ctx> {
    type Error: BoxPingErrorMetadata;

    fn ping(&mut self, ctx: &Ctx, timeout_ms: u64) -> Result<(), Self::Error>;
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn ping_box_transport_classified_with_now<Ctx, Transport, Client, Factory, Now>(
    ctx: &Ctx,
    transport: &Transport,
    create_control_client: Factory,
    timeout_ms: u64,
    mut now: Now,
) -> BoxPingOutcome
where
    Client: BoxPingControlClient<Ctx>,
    Factory: FnOnce(&Transport) -> Client,
    Now: FnMut() -> u64,
{
    let mut control = create_control_client(transport);
    let start = now();
    match control.ping(ctx, timeout_ms) {
        Ok(()) => BoxPingOutcome {
            outcome: "ok",
            latency_ms: now().saturating_sub(start),
            cause_summary: None,
        },
        Err(error) => {
            let classified = classify_ping_failure(&error);
            BoxPingOutcome {
                outcome: classified.outcome,
                latency_ms: now().saturating_sub(start),
                cause_summary: Some(classified.cause_summary),
            }
        }
    }
}

pub fn ping_box_transport_classified<Ctx, Transport, Client, Factory>(
    ctx: &Ctx,
    transport: &Transport,
    create_control_client: Factory,
    timeout_ms: u64,
) -> BoxPingOutcome
where
    Client: BoxPingControlClient<Ctx>,
    Factory: FnOnce(&Transport) -> Client,
{
    ping_box_transport_classified_with_now(
        ctx,
        transport,
        create_control_client,
        timeout_ms,
        system_now_ms,
    )
}

pub fn ping_box_classified<Ctx, Transport, Client, TransportFactory, ClientFactory>(
    ctx: &Ctx,
    endpoint: &BoxEndpoint,
    create_transport: TransportFactory,
    create_control_client: ClientFactory,
    timeout_ms: u64,
) -> BoxPingOutcome
where
    Client: BoxPingControlClient<Ctx>,
    TransportFactory: FnOnce(BoxTransportOptions) -> Transport,
    ClientFactory: FnOnce(&Transport) -> Client,
{
    let transport = create_box_transport(endpoint, create_transport);
    ping_box_transport_classified(ctx, &transport, create_control_client, timeout_ms)
}

pub fn ping_box_transport<Ctx, Transport, Client, Factory>(
    ctx: &Ctx,
    transport: &Transport,
    create_control_client: Factory,
    timeout_ms: u64,
) -> bool
where
    Client: BoxPingControlClient<Ctx>,
    Factory: FnOnce(&Transport) -> Client,
{
    ping_box_transport_classified(ctx, transport, create_control_client, timeout_ms).outcome == "ok"
}

pub fn ping_box<Ctx, Transport, Client, TransportFactory, ClientFactory>(
    ctx: &Ctx,
    endpoint: &BoxEndpoint,
    create_transport: TransportFactory,
    create_control_client: ClientFactory,
    timeout_ms: u64,
) -> bool
where
    Client: BoxPingControlClient<Ctx>,
    TransportFactory: FnOnce(BoxTransportOptions) -> Transport,
    ClientFactory: FnOnce(&Transport) -> Client,
{
    ping_box_classified(
        ctx,
        endpoint,
        create_transport,
        create_control_client,
        timeout_ms,
    )
    .outcome
        == "ok"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxRemoteExecControlMessage {
    StreamClose { id: u64 },
    Throw {
        id: Option<u64>,
        error: String,
        stack_trace: Option<String>,
        error_code: Option<String>,
    },
    Heartbeat { id: u64 },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxRemoteExecEnvelope<ClientMessage> {
    ExecClientMessage(ClientMessage),
    ExecClientControlMessage(BoxRemoteExecControlMessage),
    None,
}

pub trait BoxRemoteExecClient<Ctx, ServerMessage, ClientMessage> {
    type Error;

    fn exec(
        &mut self,
        ctx: &Ctx,
        args: ServerMessage,
    ) -> Result<Vec<BoxRemoteExecEnvelope<ClientMessage>>, Self::Error>;
}

#[derive(Debug)]
pub enum BoxRemoteExecError<E> {
    Transport(E),
    RemoteThrow {
        error: String,
        stack_trace: Option<String>,
        error_code: Option<String>,
    },
}

impl<E: fmt::Display> fmt::Display for BoxRemoteExecError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(formatter),
            Self::RemoteThrow { error, .. } => formatter.write_str(error),
        }
    }
}

impl<E: Error + 'static> Error for BoxRemoteExecError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::RemoteThrow { .. } => None,
        }
    }
}

pub struct BoxRemoteExecManager<Client> {
    client: Client,
    next_id: u64,
}

impl<Client> BoxRemoteExecManager<Client> {
    pub fn new(client: Client) -> Self {
        Self { client, next_id: 0 }
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    pub fn create_exec_instance<Ctx, ServerMessage, ClientMessage, Serialize>(
        &mut self,
        ctx: &Ctx,
        serialize: Serialize,
    ) -> Result<Vec<ClientMessage>, BoxRemoteExecError<Client::Error>>
    where
        Client: BoxRemoteExecClient<Ctx, ServerMessage, ClientMessage>,
        Serialize: FnOnce(u64) -> ServerMessage,
    {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let messages = self
            .client
            .exec(ctx, serialize(id))
            .map_err(BoxRemoteExecError::Transport)?;
        let mut output = Vec::new();
        for message in messages {
            match message {
                BoxRemoteExecEnvelope::ExecClientMessage(message) => output.push(message),
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::Throw {
                        error,
                        stack_trace,
                        error_code,
                        ..
                    },
                ) => {
                    return Err(BoxRemoteExecError::RemoteThrow {
                        error,
                        stack_trace,
                        error_code,
                    });
                }
                BoxRemoteExecEnvelope::ExecClientControlMessage(_)
                | BoxRemoteExecEnvelope::None => {}
            }
        }
        Ok(output)
    }

    pub fn into_client(self) -> Client {
        self.client
    }
}

pub fn create_box_remote_resource_accessor_from_transport<
    Transport,
    Client,
    Accessor,
    CreateClient,
    CreateAccessor,
>(
    transport: Transport,
    create_exec_client: CreateClient,
    create_resource_accessor: CreateAccessor,
) -> Accessor
where
    CreateClient: FnOnce(Transport) -> Client,
    CreateAccessor: FnOnce(BoxRemoteExecManager<Client>) -> Accessor,
{
    create_resource_accessor(BoxRemoteExecManager::new(create_exec_client(transport)))
}

pub fn create_box_remote_resource_accessor<
    Transport,
    Client,
    Accessor,
    TransportFactory,
    CreateClient,
    CreateAccessor,
>(
    endpoint: &BoxEndpoint,
    create_transport: TransportFactory,
    create_exec_client: CreateClient,
    create_resource_accessor: CreateAccessor,
) -> Accessor
where
    TransportFactory: FnOnce(BoxTransportOptions) -> Transport,
    CreateClient: FnOnce(Transport) -> Client,
    CreateAccessor: FnOnce(BoxRemoteExecManager<Client>) -> Accessor,
{
    create_box_remote_resource_accessor_from_transport(
        create_box_transport(endpoint, create_transport),
        create_exec_client,
        create_resource_accessor,
    )
}
