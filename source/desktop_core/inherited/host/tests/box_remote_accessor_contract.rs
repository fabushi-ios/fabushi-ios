use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;
use std::io;

use mahayana_host_runtime::r#box::box_remote_accessor::{
    BoxEndpoint, BoxPingControlClient, BoxPingErrorMetadata, BoxRemoteExecClient,
    BoxRemoteExecControlMessage, BoxRemoteExecEnvelope, BoxRemoteExecError,
    BoxRemoteExecManager, BoxTransportOptions, ConnectCode, classify_ping_failure,
    create_box_authorization_interceptor, create_box_remote_resource_accessor,
    create_box_transport, ping_box_transport_classified_with_now,
};

#[derive(Debug)]
struct FakePingError {
    code: Option<ConnectCode>,
    errno: Option<&'static str>,
    message: &'static str,
}

impl fmt::Display for FakePingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for FakePingError {}

impl BoxPingErrorMetadata for FakePingError {
    fn connect_code(&self) -> Option<ConnectCode> {
        self.code.clone()
    }

    fn system_errno(&self) -> Option<&str> {
        self.errno
    }
}

struct FakePingClient {
    result: Result<(), FakePingError>,
    timeout_seen: Cell<u64>,
}

impl BoxPingControlClient<String> for FakePingClient {
    type Error = FakePingError;

    fn ping(&mut self, _ctx: &String, timeout_ms: u64) -> Result<(), Self::Error> {
        self.timeout_seen.set(timeout_ms);
        std::mem::replace(&mut self.result, Ok(()))
    }
}

#[test]
fn box_remote_accessor_builds_authenticated_binary_http11_transport() {
    let mut endpoint = BoxEndpoint::new("127.0.0.1", 1337, "secret");
    endpoint
        .headers
        .insert("x-sand-display".into(), "2".into());

    let interceptor = create_box_authorization_interceptor(&endpoint);
    let mut headers = BTreeMap::new();
    interceptor.apply(&mut headers);
    assert_eq!(headers["Authorization"], "Bearer secret");
    assert_eq!(headers["x-sand-display"], "2");

    let options = create_box_transport(&endpoint, |options| options);
    assert_eq!(
        options,
        BoxTransportOptions {
            http_version: "1.1",
            base_url: "http://127.0.0.1:1337".into(),
            use_binary_format: true,
            interceptors: vec![interceptor],
        }
    );
}

#[test]
fn box_remote_accessor_classifies_ping_failures_and_latency_like_grok() {
    let deadline = FakePingError {
        code: Some(ConnectCode::Number(4)),
        errno: None,
        message: "deadline exceeded",
    };
    let classified = classify_ping_failure(&deadline);
    assert_eq!(classified.outcome, "timeout");
    assert_eq!(classified.cause_summary, "DeadlineExceeded");

    let refused = FakePingError {
        code: Some(ConnectCode::Number(14)),
        errno: Some("ECONNREFUSED"),
        message: "connect failed",
    };
    let classified = classify_ping_failure(&refused);
    assert_eq!(classified.outcome, "refused");
    assert_eq!(classified.cause_summary, "Unavailable/ECONNREFUSED");

    let clock = Cell::new(100u64);
    let outcome = ping_box_transport_classified_with_now(
        &"ctx".into(),
        &(),
        |_| FakePingClient {
            result: Err(FakePingError {
                code: Some(ConnectCode::Text("Unavailable".into())),
                errno: None,
                message: "socket crashed",
            }),
            timeout_seen: Cell::new(0),
        },
        500,
        || {
            let value = clock.get();
            clock.set(value + 25);
            value
        },
    );
    assert_eq!(outcome.outcome, "crash");
    assert_eq!(outcome.latency_ms, 25);
    assert_eq!(outcome.cause_summary.as_deref(), Some("Unavailable"));
}

#[derive(Default)]
struct FakeExecClient {
    calls: Vec<u64>,
    throw: bool,
}

impl BoxRemoteExecClient<String, u64, String> for FakeExecClient {
    type Error = io::Error;

    fn exec(
        &mut self,
        _ctx: &String,
        args: u64,
    ) -> Result<Vec<BoxRemoteExecEnvelope<String>>, Self::Error> {
        self.calls.push(args);
        if self.throw {
            Ok(vec![
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::Heartbeat { id: args },
                ),
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::Throw {
                        id: Some(args),
                        error: "remote exploded".into(),
                        stack_trace: Some("stack".into()),
                        error_code: Some("E_REMOTE".into()),
                    },
                ),
            ])
        } else {
            Ok(vec![
                BoxRemoteExecEnvelope::ExecClientControlMessage(
                    BoxRemoteExecControlMessage::Heartbeat { id: args },
                ),
                BoxRemoteExecEnvelope::ExecClientMessage(format!("message-{args}")),
                BoxRemoteExecEnvelope::None,
            ])
        }
    }
}

#[test]
fn box_remote_exec_manager_allocates_ids_filters_control_and_surfaces_remote_throw() {
    let mut manager = BoxRemoteExecManager::new(FakeExecClient::default());
    let messages = manager
        .create_exec_instance(&"ctx".into(), |id| id)
        .expect("first remote exec");
    assert_eq!(messages, vec!["message-0"]);
    assert_eq!(manager.next_id(), 1);

    let messages = manager
        .create_exec_instance(&"ctx".into(), |id| id)
        .expect("second remote exec");
    assert_eq!(messages, vec!["message-1"]);
    assert_eq!(manager.next_id(), 2);

    let mut throwing = BoxRemoteExecManager::new(FakeExecClient {
        calls: Vec::new(),
        throw: true,
    });
    let error = throwing
        .create_exec_instance(&"ctx".into(), |id| id)
        .expect_err("throw control must surface");
    match error {
        BoxRemoteExecError::RemoteThrow {
            error,
            stack_trace,
            error_code,
        } => {
            assert_eq!(error, "remote exploded");
            assert_eq!(stack_trace.as_deref(), Some("stack"));
            assert_eq!(error_code.as_deref(), Some("E_REMOTE"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn box_remote_resource_accessor_composes_transport_client_and_manager() {
    let endpoint = BoxEndpoint::new("host", 1337, "token");
    let next_id = create_box_remote_resource_accessor(
        &endpoint,
        |options| options.base_url,
        |_transport| FakeExecClient::default(),
        |manager| manager.next_id(),
    );
    assert_eq!(next_id, 0);
}
