use crate::box_mcp::{ConnectErrorClassification, ConnectErrorCode};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxEndpoint {
    pub host: String,
    pub port: u16,
    pub auth_token: String,
    pub headers: BTreeMap<String, String>,
}

impl BoxEndpoint {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxTransportOptions {
    pub http_version: &'static str,
    pub base_url: String,
    pub use_binary_format: bool,
    pub headers: BTreeMap<String, String>,
}

pub fn create_box_authorization_headers(
    endpoint: &BoxEndpoint,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::from([(
        "Authorization".to_owned(),
        format!("Bearer {}", endpoint.auth_token),
    )]);
    headers.extend(endpoint.headers.clone());
    headers
}

pub fn create_box_transport<Transport>(
    endpoint: &BoxEndpoint,
    factory: impl FnOnce(BoxTransportOptions) -> Transport,
) -> Transport {
    factory(BoxTransportOptions {
        http_version: "1.1",
        base_url: endpoint.base_url(),
        use_binary_format: true,
        headers: create_box_authorization_headers(endpoint),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxConnectError {
    pub code: Option<ConnectErrorCode>,
    pub errno: Option<String>,
    pub message: String,
}

impl std::fmt::Display for BoxConnectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BoxConnectError {}

impl ConnectErrorClassification for BoxConnectError {
    fn connect_error_code(&self) -> Option<ConnectErrorCode> {
        self.code.clone()
    }
}

fn connect_code_name(code: Option<&ConnectErrorCode>) -> String {
    match code {
        Some(ConnectErrorCode::Numeric(1)) => "Canceled".into(),
        Some(ConnectErrorCode::Numeric(2)) => "Unknown".into(),
        Some(ConnectErrorCode::Numeric(3)) => "InvalidArgument".into(),
        Some(ConnectErrorCode::Numeric(4)) => "DeadlineExceeded".into(),
        Some(ConnectErrorCode::Numeric(5)) => "NotFound".into(),
        Some(ConnectErrorCode::Numeric(6)) => "AlreadyExists".into(),
        Some(ConnectErrorCode::Numeric(7)) => "PermissionDenied".into(),
        Some(ConnectErrorCode::Numeric(8)) => "ResourceExhausted".into(),
        Some(ConnectErrorCode::Numeric(9)) => "FailedPrecondition".into(),
        Some(ConnectErrorCode::Numeric(10)) => "Aborted".into(),
        Some(ConnectErrorCode::Numeric(11)) => "OutOfRange".into(),
        Some(ConnectErrorCode::Numeric(12)) => "Unimplemented".into(),
        Some(ConnectErrorCode::Numeric(13)) => "Internal".into(),
        Some(ConnectErrorCode::Numeric(14)) => "Unavailable".into(),
        Some(ConnectErrorCode::Numeric(15)) => "DataLoss".into(),
        Some(ConnectErrorCode::Numeric(16)) => "Unauthenticated".into(),
        Some(ConnectErrorCode::Numeric(_)) | None => "unknown".into(),
        Some(ConnectErrorCode::Text(value)) if value.is_empty() => "unknown".into(),
        Some(ConnectErrorCode::Text(value)) => value.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxPingFailureOutcome {
    Timeout,
    Refused,
    Crash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedPingFailure {
    pub outcome: BoxPingFailureOutcome,
    pub cause_summary: String,
}

pub fn classify_ping_failure(error: &BoxConnectError) -> ClassifiedPingFailure {
    let code_name = connect_code_name(error.code.as_ref());
    let cause_summary = match error.errno.as_deref() {
        Some(errno) => format!("{code_name}/{errno}"),
        None => code_name,
    };
    let deadline = matches!(
        error.code,
        Some(ConnectErrorCode::Numeric(4))
    ) || error.message.to_ascii_lowercase().contains("deadline");
    if deadline {
        return ClassifiedPingFailure {
            outcome: BoxPingFailureOutcome::Timeout,
            cause_summary,
        };
    }
    let refused = error.errno.as_deref() == Some("ECONNREFUSED")
        || error.message.to_ascii_uppercase().contains("ECONNREFUSED");
    ClassifiedPingFailure {
        outcome: if refused {
            BoxPingFailureOutcome::Refused
        } else {
            BoxPingFailureOutcome::Crash
        },
        cause_summary,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxPingOutcome {
    Ok { latency_ms: u64 },
    Timeout { latency_ms: u64, cause_summary: String },
    Refused { latency_ms: u64, cause_summary: String },
    Crash { latency_ms: u64, cause_summary: String },
}

pub trait BoxPingControlClient<Context> {
    async fn ping(
        &mut self,
        context: &Context,
        timeout_ms: u64,
    ) -> Result<(), BoxConnectError>;
}

pub async fn ping_box_transport_classified<Context, Client>(
    context: &Context,
    mut client: Client,
    timeout_ms: u64,
    mut now: impl FnMut() -> u64,
) -> BoxPingOutcome
where
    Client: BoxPingControlClient<Context>,
{
    let started = now();
    match client.ping(context, timeout_ms).await {
        Ok(()) => BoxPingOutcome::Ok {
            latency_ms: now().saturating_sub(started),
        },
        Err(error) => {
            let failure = classify_ping_failure(&error);
            let latency_ms = now().saturating_sub(started);
            match failure.outcome {
                BoxPingFailureOutcome::Timeout => BoxPingOutcome::Timeout {
                    latency_ms,
                    cause_summary: failure.cause_summary,
                },
                BoxPingFailureOutcome::Refused => BoxPingOutcome::Refused {
                    latency_ms,
                    cause_summary: failure.cause_summary,
                },
                BoxPingFailureOutcome::Crash => BoxPingOutcome::Crash {
                    latency_ms,
                    cause_summary: failure.cause_summary,
                },
            }
        }
    }
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
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxRemoteExecEnvelope<ClientMessage> {
    ExecClientMessage(ClientMessage),
    ExecClientControlMessage(BoxRemoteExecControlMessage),
    Empty,
}

pub trait BoxRemoteExecClient<Context, ServerMessage, ClientMessage> {
    type Error;

    async fn exec(
        &self,
        context: &Context,
        request: ServerMessage,
    ) -> Result<Vec<BoxRemoteExecEnvelope<ClientMessage>>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxRemoteExecError<ClientError> {
    Client(ClientError),
    RemoteThrow {
        message: String,
        stack_trace: Option<String>,
        error_code: Option<String>,
    },
    Protocol {
        message: String,
    },
}

pub struct BoxRemoteExecManager<Client> {
    client: Client,
    next_id: Mutex<u64>,
}

impl<Client> BoxRemoteExecManager<Client> {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            next_id: Mutex::new(0),
        }
    }

    fn take_id(&self) -> u64 {
        let mut next = self.next_id.lock().expect("remote exec id lock");
        let id = *next;
        *next = next.saturating_add(1);
        id
    }

    pub async fn create_exec_instance<
        Context,
        ServerMessage,
        ClientMessage,
    >(
        &self,
        context: &Context,
        serialize: impl FnOnce(u64) -> ServerMessage,
    ) -> Result<Vec<ClientMessage>, BoxRemoteExecError<Client::Error>>
    where
        Client: BoxRemoteExecClient<Context, ServerMessage, ClientMessage>,
    {
        let request_id = self.take_id();
        let request = serialize(request_id);
        let envelopes = self
            .client
            .exec(context, request)
            .await
            .map_err(BoxRemoteExecError::Client)?;
        let mut messages = Vec::new();
        let mut stream_closed = false;
        for envelope in envelopes {
            if stream_closed {
                return Err(BoxRemoteExecError::Protocol {
                    message: format!(
                        "remote exec {request_id} produced an envelope after stream close"
                    ),
                });
            }
            match envelope {
                BoxRemoteExecEnvelope::ExecClientMessage(message) => {
                    messages.push(message);
                }
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::Throw {
                        id,
                        error,
                        stack_trace,
                        error_code,
                    },
                ) => {
                    if id.is_some_and(|id| id != request_id) {
                        return Err(BoxRemoteExecError::Protocol {
                            message: format!(
                                "remote exec throw id did not match request {request_id}"
                            ),
                        });
                    }
                    return Err(BoxRemoteExecError::RemoteThrow {
                        message: error,
                        stack_trace,
                        error_code,
                    });
                }
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::Heartbeat { id },
                ) => {
                    if id != request_id {
                        return Err(BoxRemoteExecError::Protocol {
                            message: format!(
                                "remote exec heartbeat id {id} did not match request {request_id}"
                            ),
                        });
                    }
                }
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::StreamClose { id },
                ) => {
                    if id != request_id {
                        return Err(BoxRemoteExecError::Protocol {
                            message: format!(
                                "remote exec close id {id} did not match request {request_id}"
                            ),
                        });
                    }
                    stream_closed = true;
                }
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::Empty,
                )
                | BoxRemoteExecEnvelope::Empty => {}
            }
        }
        Ok(messages)
    }
}

pub fn create_box_remote_resource_accessor_from_client<
    Client,
    Accessor,
>(
    client: Client,
    create_accessor: impl FnOnce(BoxRemoteExecManager<Client>) -> Accessor,
) -> Accessor {
    create_accessor(BoxRemoteExecManager::new(client))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_options_include_bearer_and_endpoint_headers() {
        let endpoint = BoxEndpoint {
            host: "runner.test".into(),
            port: 1337,
            auth_token: "secret".into(),
            headers: BTreeMap::from([(
                "x-sand-display".into(),
                "2".into(),
            )]),
        };
        let options = create_box_transport(&endpoint, |options| options);
        assert_eq!(options.http_version, "1.1");
        assert!(options.use_binary_format);
        assert_eq!(options.base_url, "http://runner.test:1337");
        assert_eq!(
            options.headers.get("Authorization").map(String::as_str),
            Some("Bearer secret")
        );
        assert_eq!(
            options.headers.get("x-sand-display").map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn ping_failure_classification_matches_connect_and_errno_semantics() {
        let timeout = classify_ping_failure(&BoxConnectError {
            code: Some(ConnectErrorCode::Numeric(4)),
            errno: None,
            message: "deadline exceeded".into(),
        });
        assert_eq!(timeout.outcome, BoxPingFailureOutcome::Timeout);

        let refused = classify_ping_failure(&BoxConnectError {
            code: Some(ConnectErrorCode::Numeric(14)),
            errno: Some("ECONNREFUSED".into()),
            message: "unavailable".into(),
        });
        assert_eq!(refused.outcome, BoxPingFailureOutcome::Refused);
        assert_eq!(refused.cause_summary, "Unavailable/ECONNREFUSED");
    }

    struct ExecClient;

    impl BoxRemoteExecClient<(), u64, &'static str> for ExecClient {
        type Error = ();

        async fn exec(
            &self,
            _context: &(),
            request: u64,
        ) -> Result<Vec<BoxRemoteExecEnvelope<&'static str>>, Self::Error> {
            Ok(vec![
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::Heartbeat { id: request },
                ),
                BoxRemoteExecEnvelope::ExecClientMessage("ok"),
            ])
        }
    }

    #[tokio::test]
    async fn remote_exec_manager_correlates_incrementing_ids() {
        let manager = BoxRemoteExecManager::new(ExecClient);
        assert_eq!(
            manager
                .create_exec_instance(&(), |id| id)
                .await
                .unwrap(),
            vec!["ok"]
        );
        assert_eq!(
            manager
                .create_exec_instance(&(), |id| id)
                .await
                .unwrap(),
            vec!["ok"]
        );
        assert_eq!(*manager.next_id.lock().unwrap(), 2);
    }

    struct InvalidControlClient {
        envelopes: Vec<BoxRemoteExecEnvelope<&'static str>>,
    }

    impl BoxRemoteExecClient<(), u64, &'static str> for InvalidControlClient {
        type Error = ();

        async fn exec(
            &self,
            _context: &(),
            _request: u64,
        ) -> Result<Vec<BoxRemoteExecEnvelope<&'static str>>, Self::Error> {
            Ok(self.envelopes.clone())
        }
    }

    #[tokio::test]
    async fn remote_exec_rejects_control_identity_mismatch() {
        let manager = BoxRemoteExecManager::new(InvalidControlClient {
            envelopes: vec![BoxRemoteExecEnvelope::ExecClientControlMessage(
                BoxRemoteExecControlMessage::Heartbeat { id: 7 },
            )],
        });
        assert!(matches!(
            manager.create_exec_instance(&(), |id| id).await,
            Err(BoxRemoteExecError::Protocol { message })
                if message.contains("heartbeat id 7")
        ));
    }

    #[tokio::test]
    async fn remote_exec_rejects_messages_after_stream_close() {
        let manager = BoxRemoteExecManager::new(InvalidControlClient {
            envelopes: vec![
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::StreamClose { id: 0 },
                ),
                BoxRemoteExecEnvelope::ExecClientMessage("late"),
            ],
        });
        assert!(matches!(
            manager.create_exec_instance(&(), |id| id).await,
            Err(BoxRemoteExecError::Protocol { message })
                if message.contains("after stream close")
        ));
    }

    struct ThrowingClient;

    impl BoxRemoteExecClient<(), (), ()> for ThrowingClient {
        type Error = ();

        async fn exec(
            &self,
            _context: &(),
            _request: (),
        ) -> Result<Vec<BoxRemoteExecEnvelope<()>>, Self::Error> {
            Ok(vec![
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::Throw {
                        id: Some(0),
                        error: "remote boom".into(),
                        stack_trace: Some("stack".into()),
                        error_code: Some("REMOTE".into()),
                    },
                ),
            ])
        }
    }

    #[tokio::test]
    async fn remote_throw_is_not_silently_dropped() {
        let manager = BoxRemoteExecManager::new(ThrowingClient);
        assert!(matches!(
            manager.create_exec_instance(&(), |_| ()).await,
            Err(BoxRemoteExecError::RemoteThrow {
                message,
                error_code: Some(code),
                ..
            }) if message == "remote boom" && code == "REMOTE"
        ));
    }
}
