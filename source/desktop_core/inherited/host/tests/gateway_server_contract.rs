use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use mahayana_host_runtime::device_pairing_owner::{DevicePairingOwner, DeviceTokenAuthorization};
use mahayana_host_runtime::gateway_config::{GatewayServerConfig, GatewayTlsConfig};
use mahayana_host_runtime::gateway_server::{
    GatewayApi, GatewayBridgeHub, GatewayCommandContext, GatewayCommandError,
    GatewayCommandReport, GatewayEventHub, GatewayHealth, GatewayServerDeps,
    start_gateway_server,
};
use rcgen::generate_simple_self_signed;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde_json::{Value, json};

#[derive(Debug)]
struct TestApi;

impl GatewayApi for TestApi {
    fn redeem_device_pairing_join_code(&self, shortcode: &str) -> Result<Option<Value>, GatewayCommandError> {
        Ok((shortcode == "A234567890123456789012").then(|| json!({
            "deviceId": "device-join-1",
            "token": "single-use-token",
        })))
    }

    fn call(&self, method: &str, args: Value) -> Result<Value, GatewayCommandError> {
        match method {
            "listAgents" => Ok(args.get("agents").cloned().unwrap_or_else(|| json!([
                {"id":"agent-1","name":"One","avatarDataUrl":"data:image/png;base64,aGVsbG8="}
            ]))),
            "getAgentAvatar" => Ok(json!({
                "dataUrl": "data:image/png;base64,aGVsbG8=",
                "version": "avatar-v1"
            })),
            "getTranscript" => Ok(args),
            "searchHumanRecipients" | "forwardHumanMessage" | "retryHumanMessage" => Ok(json!({"method":method,"args":args})),
            method if method.starts_with("feature.") => Ok(json!({
                "method": method,
                "args": args,
            })),
            method if matches!(
                method,
                "runner.acceptRoutedPrompt"
                    | "runner.startRoutedProvider"
                    | "runner.cancelRoutedProvider"
                    | "runner.resolveRoutedToolRequest"
                    | "mcp.resolveLifecycleRequest"
            ) => Ok(json!({
                "method": method,
                "args": args,
            })),
            method if method.starts_with("device.pair.")
                || method.starts_with("node.")
                || method == "getHostSettings"
                || method == "setHostSettings" => Ok(json!({
                "method": method,
                "args": args,
            })),
            "conflict" => Err(GatewayCommandError::Conflict("conflict".into())),
            other => Err(GatewayCommandError::UnknownMethod(other.to_string())),
        }
    }

    fn health(&self) -> GatewayHealth {
        GatewayHealth {
            is_busy: true,
            busy_only_awaiting_approval: Some(false),
            active_agent_id: Some("agent-1".into()),
            last_busy_at_ms: Some(123),
        }
    }

    fn prepare_for_upgrade(&self) -> Result<Value, GatewayCommandError> {
        Ok(json!({ "quiescing": true, "runningTurns": 2 }))
    }
}

struct NodeTokenApi;

impl GatewayApi for NodeTokenApi {
    fn authenticate_device_token(
        &self,
        token: &str,
    ) -> Result<Option<DeviceTokenAuthorization>, GatewayCommandError> {
        Ok((token == "node-token").then(|| DeviceTokenAuthorization {
            device_id: "node-1".into(),
            role: "node".into(),
            scopes: vec![],
        }))
    }

    fn authorize_device_token_request(
        &self,
        authorization: &DeviceTokenAuthorization,
        method: &str,
        args: &Value,
    ) -> Result<(), GatewayCommandError> {
        if authorization.role == "node"
            && method == "node.invoke"
            && args.get("nodeId").and_then(Value::as_str) != Some(authorization.device_id.as_str())
        {
            return Err(GatewayCommandError::Forbidden("DEVICE_SCOPE_MISMATCH".into()));
        }
        Ok(())
    }

    fn call(&self, method: &str, args: Value) -> Result<Value, GatewayCommandError> {
        Ok(json!({"method": method, "args": args}))
    }
}

#[test]
fn device_pairing_owner_authenticates_issued_token_with_role_and_scopes() {
    let owner = DevicePairingOwner::in_memory().expect("pairing owner");
    let pending = owner
        .handle_pair_request(&json!({
            "deviceId": "device-auth-1",
            "publicKey": "public-key-auth-1",
            "platform": "macos",
            "deviceFamily": "desktop",
            "roles": ["operator"],
            "scopes": ["operator.read", "operator.write"]
        }))
        .expect("pair request");
    let request_id = pending
        .get("requestId")
        .and_then(Value::as_str)
        .expect("request id");
    owner
        .handle_pair_approve(&json!({ "requestId": request_id }))
        .expect("pair approval");
    let status = owner
        .handle_pair_status(&json!({ "deviceId": "device-auth-1" }))
        .expect("pair status");
    assert_eq!(status["device"]["platform"], "macos");
    assert_eq!(status["device"]["deviceFamily"], "desktop");
    let rotated = owner
        .handle_pair_rotate_token(&json!({ "deviceId": "device-auth-1", "role": "operator" }))
        .expect("token rotation");
    let token = rotated
        .get("token")
        .and_then(Value::as_str)
        .expect("rotated token");

    let authorization = owner
        .authenticate_device_token(token)
        .expect("token lookup")
        .expect("issued token authorization");
    assert_eq!(authorization.device_id, "device-auth-1");
    assert_eq!(authorization.role, "operator");
    assert_eq!(
        authorization.scopes,
        vec!["operator.read".to_string(), "operator.write".to_string()]
    );
    assert!(owner
        .authenticate_device_token("sand-device-tok-invalid")
        .expect("invalid token lookup")
        .is_none());
}

#[test]
fn gateway_binds_node_token_to_the_addressed_node_before_dispatch() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(NodeTokenApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(Some("gateway-secret")),
        started_at: 100,
    })
    .expect("gateway server");
    let port = server.port();

    let rejected = request(
        port,
        "POST /api/node.invoke HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer node-token\r\nContent-Length: 19\r\nConnection: close\r\n\r\n{\"nodeId\":\"node-2\"}",
    );
    assert!(rejected.starts_with("HTTP/1.1 403 Forbidden"), "{rejected}");
    assert!(rejected.contains("DEVICE_SCOPE_MISMATCH"), "{rejected}");

    server.close();
}

struct PairingOwnerApi {
    owner: Arc<DevicePairingOwner>,
}

impl GatewayApi for PairingOwnerApi {
    fn authenticate_device_token(
        &self,
        token: &str,
    ) -> Result<Option<mahayana_host_runtime::device_pairing_owner::DeviceTokenAuthorization>, GatewayCommandError> {
        self.owner.authenticate_device_token(token)
    }

    fn call(&self, method: &str, _args: Value) -> Result<Value, GatewayCommandError> {
        if method == "device.pair.list" {
            return Ok(json!({"paired": [], "pending": []}));
        }
        Err(GatewayCommandError::UnknownMethod(method.to_string()))
    }

    fn redeem_device_pairing_join_code(
        &self,
        shortcode: &str,
    ) -> Result<Option<Value>, GatewayCommandError> {
        self.owner.redeem_device_pairing_join_code(shortcode)
    }
}

#[test]
fn gateway_accepts_pairing_owner_device_token_without_gateway_secret() {
    let owner = Arc::new(DevicePairingOwner::in_memory().expect("pairing owner"));
    let pending = owner
        .handle_pair_request(&json!({
            "deviceId": "device-gateway-auth-1",
            "publicKey": "public-key-gateway-auth-1"
        }))
        .expect("pair request");
    let request_id = pending
        .get("requestId")
        .and_then(Value::as_str)
        .expect("request id");
    owner
        .handle_pair_approve(&json!({ "requestId": request_id }))
        .expect("pair approval");
    let rotated = owner
        .handle_pair_rotate_token(&json!({ "deviceId": "device-gateway-auth-1" }))
        .expect("token rotation");
    let token = rotated
        .get("token")
        .and_then(Value::as_str)
        .expect("rotated token");

    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(PairingOwnerApi { owner }),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(Some("gateway-token")),
        started_at: 3,
    })
    .expect("gateway server");
    let port = server.port();
    let accepted = request(
        port,
        &format!(
            "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    );
    assert!(accepted.starts_with("HTTP/1.1 200 OK"), "{accepted}");
    let rejected = request(
        port,
        "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer invalid-device-token\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(rejected.starts_with("HTTP/1.1 401 Unauthorized"), "{rejected}");
    server.close();
}

#[test]
fn gateway_rejects_node_token_on_operator_pairing_surface() {
    let owner = Arc::new(DevicePairingOwner::in_memory().expect("pairing owner"));
    let pending = owner
        .handle_pair_request(&json!({
            "deviceId": "device-node-only",
            "publicKey": "public-key-node-only",
            "roles": ["node"]
        }))
        .expect("pair request");
    let request_id = pending
        .get("requestId")
        .and_then(Value::as_str)
        .expect("request id");
    owner
        .handle_pair_approve(&json!({ "requestId": request_id }))
        .expect("pair approval");
    let rotated = owner
        .handle_pair_rotate_token(&json!({
            "deviceId": "device-node-only",
            "role": "node"
        }))
        .expect("node token rotation");
    let token = rotated
        .get("token")
        .and_then(Value::as_str)
        .expect("node token");

    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(PairingOwnerApi { owner }),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 100,
    })
    .expect("gateway server");
    let response = request(
        server.port(),
        &format!(
            "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    );
    assert!(response.starts_with("HTTP/1.1 403 Forbidden"), "{response}");
    assert!(response.contains("ROLE_MISMATCH"), "{response}");
    server.close();
}

#[test]
fn gateway_pairing_join_route_reaches_sqlite_owner_and_burns_code() {
    let owner = Arc::new(DevicePairingOwner::in_memory().expect("pairing owner"));
    let expires_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_millis() as i64
        + 60_000;
    let code = owner
        .register_device_pairing_join_code(&json!({
            "deviceId": "device-owner-backed",
            "setupCode": "secret-payload",
        }), expires_at_ms)
        .expect("register owner-backed join code");
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(PairingOwnerApi { owner }),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(Some("gateway-token")),
        started_at: 2,
    })
    .expect("gateway server");
    let port = server.port();
    let first = request(
        port,
        &format!(
            "GET /j/{code} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(first.starts_with("HTTP/1.1 200 OK"), "{first}");
    assert_eq!(json_body(&first)["deviceId"], "device-owner-backed");
    let second = request(
        port,
        &format!(
            "GET /j/{code} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(second.starts_with("HTTP/1.1 404 Not Found"), "{second}");
    server.close();
}

#[test]
fn gateway_public_pairing_join_route_redeems_once_and_rejects_query_or_empty_code() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(Some("secret")),
        started_at: 1,
    })
    .expect("gateway server");
    let port = server.port();

    let redeemed = request(
        port,
        "GET /j/A234567890123456789012 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(redeemed.starts_with("HTTP/1.1 200 OK"), "{redeemed}");
    assert_eq!(json_body(&redeemed)["deviceId"], "device-join-1");
    assert!(redeemed.to_ascii_lowercase().contains("cache-control: no-store"));

    let queried = request(
        port,
        "GET /j/A234567890123456789012?x=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(queried.starts_with("HTTP/1.1 404 Not Found"), "{queried}");
    assert!(queried.to_ascii_lowercase().contains("cache-control: no-store"));

    let empty = request(
        port,
        "GET /j HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(empty.starts_with("HTTP/1.1 404 Not Found"), "{empty}");
    assert!(empty.to_ascii_lowercase().contains("cache-control: no-store"));
    server.close();
}


#[derive(Clone, Default)]
struct ReportingApi {
    reports: Arc<Mutex<Vec<(String, GatewayCommandReport)>>>,
    contexts: Arc<Mutex<Vec<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
    )>>>,
}

impl GatewayApi for ReportingApi {
    fn call(&self, method: &str, args: Value) -> Result<Value, GatewayCommandError> {
        if method == "getTranscript" && args.get("fail").and_then(Value::as_bool) == Some(true) {
            return Err(GatewayCommandError::Internal("synthetic internal failure".into()));
        }
        if method == "getTranscript" {
            return Ok(args);
        }
        Err(GatewayCommandError::UnknownMethod(method.to_string()))
    }

    fn call_with_context(
        &self,
        method: &str,
        args: Value,
        context: &GatewayCommandContext,
    ) -> Result<Value, GatewayCommandError> {
        self.contexts
            .lock()
            .expect("context lock")
            .push((
                context.request_id.clone(),
                context.traceparent.clone(),
                context.trace_id.clone(),
                context.span_id.clone(),
                context.authenticated,
            ));
        self.call(method, args)
    }

    fn on_command_complete(&self, report: GatewayCommandReport) {
        self.reports
            .lock()
            .expect("report lock")
            .push(("complete".into(), report));
    }

    fn on_command_error(&self, report: GatewayCommandReport) {
        self.reports
            .lock()
            .expect("report lock")
            .push(("error".into(), report));
    }
}

fn config(token: Option<&str>) -> GatewayServerConfig {
    GatewayServerConfig {
        host: "127.0.0.1".into(),
        port: None,
        auth_token: token.map(str::to_string),
        tls: None,
    }
}

fn request_bytes(port: u16, request: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect gateway");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read gateway response");
    response
}

fn request(port: u16, request: &str) -> String {
    String::from_utf8(request_bytes(port, request)).expect("utf8 gateway response")
}

fn json_body(response: &str) -> Value {
    let (_, body) = response
        .split_once("\r\n\r\n")
        .expect("HTTP response body");
    serde_json::from_str(body).expect("JSON body")
}

#[test]
fn gateway_health_command_security_and_upgrade_routes_match_grok_contract() {
    let events = GatewayEventHub::default();
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events,
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 777,
    })
    .expect("gateway server");
    let port = server.port();

    let health = request(
        port,
        "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(health.starts_with("HTTP/1.1 200 OK"), "{health}");
    let health_json = json_body(&health);
    assert_eq!(health_json["ok"], true);
    assert_eq!(health_json["isBusy"], true);
    assert_eq!(health_json["busyOnlyAwaitingApproval"], false);
    assert_eq!(health_json["activeAgentId"], "agent-1");
    assert_eq!(health_json["startedAt"], 777);

    let body = r#"{"value":"hello"}"#;
    let transcript = request(
        port,
        &format!(
            "POST /api/getTranscript HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(transcript.starts_with("HTTP/1.1 200 OK"), "{transcript}");
    assert!(
        transcript.to_ascii_lowercase()
            .contains("x-sand-mint-dedupe: 1"),
        "{transcript}"
    );
    assert_eq!(json_body(&transcript), json!({ "value": "hello" }));

    let unknown = request(
        port,
        "POST /api/noSuchMethod HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(unknown.starts_with("HTTP/1.1 404 Not Found"), "{unknown}");
    assert!(
        !unknown
            .to_ascii_lowercase()
            .contains("x-sand-mint-dedupe:"),
        "error responses must not carry the successful command dedupe marker: {unknown}"
    );

    let upgrade = request(
        port,
        "POST /prepare-upgrade HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(upgrade.starts_with("HTTP/1.1 200 OK"), "{upgrade}");
    assert_eq!(
        json_body(&upgrade),
        json!({ "quiescing": true, "runningTurns": 2 })
    );

    let origin = request(
        port,
        "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://example.com\r\nConnection: close\r\n\r\n",
    );
    assert!(origin.starts_with("HTTP/1.1 403 Forbidden"), "{origin}");

    let untrusted_host = request(
        port,
        "GET /health HTTP/1.1\r\nHost: attacker.example\r\nConnection: close\r\n\r\n",
    );
    assert!(
        untrusted_host.starts_with("HTTP/1.1 403 Forbidden"),
        "{untrusted_host}"
    );

    server.close();
}


#[test]
fn gateway_serves_real_tls_when_cert_and_key_are_configured() {
    let certified = generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("generate TLS certificate");
    let cert_pem = certified.cert.pem();
    let key_pem = certified.key_pair.serialize_pem();

    let mut roots = RootCertStore::empty();
    roots
        .add(certified.cert.der().clone())
        .expect("trust generated TLS certificate");
    let client = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: GatewayServerConfig {
            host: "127.0.0.1".into(),
            port: None,
            auth_token: None,
            tls: Some(GatewayTlsConfig {
                cert: cert_pem.into_bytes(),
                key: key_pem.into_bytes(),
            }),
        },
        started_at: 321,
    })
    .expect("TLS gateway server");

    let socket = TcpStream::connect(("127.0.0.1", server.port())).expect("connect TLS gateway");
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("TLS read timeout");
    let name = ServerName::try_from("localhost")
        .expect("valid TLS server name")
        .to_owned();
    let connection =
        ClientConnection::new(Arc::new(client), name).expect("create TLS client connection");
    let mut stream = StreamOwned::new(connection, socket);
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .expect("write TLS health request");
    stream.flush().expect("flush TLS health request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read TLS health response");
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert_eq!(json_body(&response)["startedAt"], 321);

    server.close();
}

#[test]
fn gateway_forward_recipient_methods_are_authenticated_and_unknown_methods_stay_closed() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi), events: GatewayEventHub::default(),
        local_exec: None, webauthn: None,
        config: config(Some("forward-secret")), started_at: 1,
    }).expect("gateway");
    for method in ["searchHumanRecipients", "forwardHumanMessage", "retryHumanMessage"] {
        let body = if method == "retryHumanMessage" {
            r#"{"conversationId":"a","clientNonce":"c"}"#
        } else {
            r#"{"sourceConversationId":"a","sourceEntryId":"b","clientNonce":"c"}"#
        };
        let send = |token: &str| request(server.port(), &format!(
            "POST /api/{method} HTTP/1.1\r\nHost: 127.0.0.1\r\n{token}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()));
        let denied = send("");
        assert!(denied.starts_with("HTTP/1.1 401 Unauthorized"), "{denied}");
        let allowed = send("Authorization: Bearer forward-secret\r\n");
        assert!(allowed.starts_with("HTTP/1.1 200 OK"), "{allowed}");
        assert_eq!(json_body(&allowed)["method"], method);
        assert_eq!(json_body(&allowed)["args"]["clientNonce"], "c");
        if method != "retryHumanMessage" {
            assert_eq!(json_body(&allowed)["args"]["sourceEntryId"], "b");
        } else {
            assert_eq!(json_body(&allowed)["args"]["conversationId"], "a");
        }
    }
    for method in ["telegram.forwardMessage", "forwardHumanMessages", "telegram.retryMessage", "retryHumanMessages"] {
        let response = request(server.port(), &format!(
            "POST /api/{method} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer forward-secret\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"));
        assert!(response.starts_with("HTTP/1.1 404 Not Found"), "{response}");
    }
    server.close();
}

#[test]
fn gateway_bearer_auth_is_required_when_configured() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(Some("secret")),
        started_at: 1,
    })
    .expect("gateway server");
    let port = server.port();

    let missing = request(
        port,
        "POST /api/getTranscript HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(missing.starts_with("HTTP/1.1 401 Unauthorized"), "{missing}");
    assert!(
        !missing
            .to_ascii_lowercase()
            .contains("x-sand-mint-dedupe:"),
        "authentication errors must not look like successful minted responses: {missing}"
    );

    let authorized = request(
        port,
        "POST /api/getTranscript HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(
        authorized.starts_with("HTTP/1.1 200 OK"),
        "{authorized}"
    );
    assert_eq!(json_body(&authorized), json!({}));

    server.close();
}

#[test]
fn gateway_carries_explicit_fabushi_extensions_without_widening_unknown_methods() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 777,
    })
    .expect("gateway server");
    let port = server.port();

    let extensions = [
        "feature.info",
        "feature.execute",
        "feature.marketplace.browse",
        "feature.marketplace.release",
        "feature.plugin.install",
        "feature.plugin.uninstall",
        "feature.plugin.rollback",
        "feature.plugin.active",
        "feature.plugin.listInstalled",
        "feature.plugin.uiDocument",
        "feature.auth.status",
        "feature.auth.providers",
        "feature.auth.browserStart",
        "feature.auth.browserPoll",
        "feature.auth.browserCancel",
        "feature.auth.browserReopen",
        "feature.auth.passwordLogin",
        "feature.auth.oauthStart",
        "feature.auth.oauthPoll",
        "feature.auth.logout",
        "feature.interrupt",
        "feature.approval.resolve",
    ];

    for method in extensions {
        let response = request(
            port,
            &format!(
                "POST /api/{method} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            ),
        );
        assert!(
            response.starts_with("HTTP/1.1 200 OK"),
            "{method} was not admitted through the explicit Fabushi extension boundary: {response}"
        );
        assert_eq!(json_body(&response)["method"], method);
    }
}

#[test]
fn gateway_admits_only_explicit_internal_mcp_lifecycle_settlement_method() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 1,
    })
    .expect("gateway server");

    let method = "mcp.resolveLifecycleRequest";
    let response = request(
        server.port(),
        &format!(
            "POST /api/{method} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    );
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "{method} was not admitted through the explicit Host MCP lifecycle settlement boundary: {response}"
    );
    assert_eq!(json_body(&response)["method"], method);

    let denied = request(
        server.port(),
        "POST /api/mcp.anythingElse HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(
        denied.starts_with("HTTP/1.1 404 Not Found"),
        "the mcp namespace must stay fail-closed: {denied}"
    );
    assert_eq!(
        json_body(&denied)["error"],
        "unknown gateway method: mcp.anythingElse"
    );

    server.close();
}

#[test]
fn gateway_admits_only_explicit_internal_runner_methods() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 1,
    })
    .expect("gateway server");

    for method in [
        "runner.acceptRoutedPrompt",
        "runner.startRoutedProvider",
        "runner.cancelRoutedProvider",
        "runner.resolveRoutedToolRequest",
    ] {
        let response = request(
            server.port(),
            &format!(
                "POST /api/{method} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            ),
        );
        assert!(
            response.starts_with("HTTP/1.1 200 OK"),
            "{method} was not admitted through the explicit internal Runner boundary: {response}"
        );
        assert_eq!(json_body(&response)["method"], method);
    }

    let denied = request(
        server.port(),
        "POST /api/runner.anythingElse HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(
        denied.starts_with("HTTP/1.1 404 Not Found"),
        "the runner namespace must stay fail-closed: {denied}"
    );
    assert_eq!(
        json_body(&denied)["error"],
        "unknown gateway method: runner.anythingElse"
    );

    server.close();
}

#[test]
fn gateway_rejects_unknown_commands_before_host_dispatch() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 1,
    })
    .expect("gateway server");
    let response = request(
        server.port(),
        "POST /api/notARealGrokMethod HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 404 Not Found"), "{response}");
    assert_eq!(json_body(&response)["error"], "unknown gateway method: notARealGrokMethod");
    server.close();
}

#[test]
fn gateway_slim_avatar_projection_matches_grok_contract() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 1,
    })
    .expect("gateway server");
    let response = request(
        server.port(),
        "POST /api/listAgents HTTP/1.1\r\nHost: 127.0.0.1\r\nx-sand-slim-avatars: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    let body = json_body(&response);
    assert_eq!(body[0]["id"], "agent-1");
    assert_eq!(body[0]["avatarDataUrl"], Value::Null);
    server.close();
}

#[test]
fn gateway_avatar_endpoint_serves_versioned_bytes_and_security_headers() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 1,
    })
    .expect("gateway server");
    let response = request(
        server.port(),
        "GET /avatars/agent-1?v=avatar-v1 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    let lower = response.to_ascii_lowercase();
    assert!(lower.contains("content-type: image/png"), "{response}");
    assert!(lower.contains("content-disposition: attachment"), "{response}");
    assert!(lower.contains("x-content-type-options: nosniff"), "{response}");
    assert!(lower.contains("cache-control: private, max-age=31536000, immutable"), "{response}");
    assert!(response.contains("ETag: \"avatar-v1\""), "{response}");
    assert!(response.ends_with("hello"), "{response}");

    let not_modified = request(
        server.port(),
        "GET /avatars/agent-1?v=avatar-v1 HTTP/1.1\r\nHost: 127.0.0.1\r\nIf-None-Match: \"avatar-v1\"\r\nConnection: close\r\n\r\n",
    );
    assert!(not_modified.starts_with("HTTP/1.1 304 Not Modified"), "{not_modified}");

    let cross_site = request(
        server.port(),
        "GET /avatars/agent-1 HTTP/1.1\r\nHost: 127.0.0.1\r\nSec-Fetch-Site: cross-site\r\nConnection: close\r\n\r\n",
    );
    assert!(cross_site.starts_with("HTTP/1.1 403 Forbidden"), "{cross_site}");
    server.close();
}

#[test]
fn gateway_bridge_channels_require_auth_and_round_trip_frames() {
    let unauthenticated_local = GatewayBridgeHub::default();
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: Some(unauthenticated_local),
        webauthn: None,
        config: config(None),
        started_at: 1,
    })
    .expect("gateway server");
    let denied = request(
        server.port(),
        "GET /local-exec/requests HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(denied.starts_with("HTTP/1.1 401 Unauthorized"), "{denied}");
    server.close();

    let local_exec = GatewayBridgeHub::default();
    let webauthn = GatewayBridgeHub::default();
    let local_responses = local_exec.subscribe_responses();
    let webauthn_responses = webauthn.subscribe_responses();
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: Some(local_exec.clone()),
        webauthn: Some(webauthn.clone()),
        config: config(Some("secret")),
        started_at: 1,
    })
    .expect("authenticated gateway server");
    let port = server.port();

    let local_client = thread::spawn(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect local-exec SSE");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("local-exec read timeout");
        stream
            .write_all(
                b"GET /local-exec/requests HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\nConnection: keep-alive\r\n\r\n",
            )
            .expect("write local-exec SSE request");
        stream.flush().expect("flush local-exec request");
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let count = stream.read(&mut chunk).expect("read local-exec SSE");
            assert!(count > 0, "local-exec SSE ended before request frame");
            bytes.extend_from_slice(&chunk[..count]);
            let text = String::from_utf8_lossy(&bytes);
            if text.contains("\"kind\":\"execute\"") {
                return text.into_owned();
            }
        }
    });
    for _ in 0..100 {
        if local_exec.request_subscriber_count() > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(local_exec.request_subscriber_count(), 1);
    local_exec.publish_request(json!({"kind":"execute","requestId":"local-1"}));
    let local_stream = local_client.join().expect("local-exec client");
    assert!(local_stream.contains("retry: 1000"), "{local_stream}");

    let local_body = r#"{"kind":"result","requestId":"local-1","value":7}"#;
    let local_post = request(
        port,
        &format!(
            "POST /local-exec/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{local_body}",
            local_body.len()
        ),
    );
    assert!(local_post.starts_with("HTTP/1.1 200 OK"), "{local_post}");
    assert_eq!(
        local_responses.recv_timeout(Duration::from_secs(1)).expect("local response"),
        json!({"kind":"result","requestId":"local-1","value":7})
    );

    let web_body = r#"{"kind":"hello","computerId":"computer-1"}"#;
    let web_post = request(
        port,
        &format!(
            "POST /webauthn/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{web_body}",
            web_body.len()
        ),
    );
    assert!(web_post.starts_with("HTTP/1.1 200 OK"), "{web_post}");
    assert_eq!(
        webauthn_responses.recv_timeout(Duration::from_secs(1)).expect("webauthn response"),
        json!({"kind":"hello","computerId":"computer-1"})
    );

    server.close();
}

#[test]
fn gateway_events_stream_retries_filters_and_delivers_runtime_events() {
    let events = GatewayEventHub::default();
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: events.clone(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 1,
    })
    .expect("gateway server");
    let port = server.port();

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect SSE");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    stream
        .write_all(
            b"GET /events?channels=runtime HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n\r\n",
        )
        .expect("write SSE request");
    stream.flush().expect("flush SSE request");

    for _ in 0..100 {
        if events.subscriber_count() > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(events.subscriber_count(), 1, "SSE subscriber registered");

    events.publish(json!({
        "channel": "ignored",
        "payload": { "value": 0 }
    }));
    events.publish(json!({
        "channel": "runtime",
        "payload": { "value": 7 }
    }));

    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let count = stream.read(&mut chunk).expect("read SSE frame");
        assert!(count > 0, "SSE stream closed before event");
        bytes.extend_from_slice(&chunk[..count]);
        let text = String::from_utf8_lossy(&bytes);
        if text.contains("\"value\":7") {
            assert!(text.contains("HTTP/1.1 200 OK"), "{text}");
            assert!(text.contains("retry: 1000"), "{text}");
            assert!(!text.contains("\"value\":0"), "{text}");
            break;
        }
        assert!(bytes.len() < 32 * 1024, "unexpectedly large SSE response");
    }

    drop(stream);
    server.close();
}


#[test]
fn gateway_gzips_only_large_successful_command_json_when_requested() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 1,
    })
    .expect("gateway server");
    let port = server.port();

    let payload = "x".repeat(2_000);
    let body = serde_json::to_string(&json!({ "payload": payload })).expect("serialize large request");
    let response = request_bytes(
        port,
        &format!(
            "POST /api/getTranscript HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept-Encoding: br, gzip\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .expect("gzip response header");
    let headers = String::from_utf8(response[..header_end].to_vec()).expect("gzip response headers utf8");
    let lower = headers.to_ascii_lowercase();
    assert!(headers.starts_with("HTTP/1.1 200 OK"), "{headers}");
    assert!(lower.contains("content-encoding: gzip"), "{headers}");
    assert!(lower.contains("vary: accept-encoding"), "{headers}");
    assert!(lower.contains("x-sand-mint-dedupe: 1"), "{headers}");

    let mut decoder = GzDecoder::new(&response[header_end..]);
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).expect("decode gateway gzip");
    assert_eq!(serde_json::from_str::<Value>(&decoded).expect("decoded JSON"), serde_json::from_str::<Value>(&body).unwrap());

    let small = request(
        port,
        "POST /api/getTranscript HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept-Encoding: gzip\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(
        !small.to_ascii_lowercase().contains("content-encoding: gzip"),
        "small command responses must stay uncompressed: {small}"
    );

    server.close();
}


#[test]
fn gateway_events_negotiate_gzip_for_sse_streams() {
    let events = GatewayEventHub::default();
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: events.clone(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 1,
    })
    .expect("gateway server");
    let port = server.port();

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect gzip SSE");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("gzip SSE read timeout");
    stream
        .write_all(
            b"GET /events?channels=runtime HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept-Encoding: gzip\r\nConnection: keep-alive\r\n\r\n",
        )
        .expect("write gzip SSE request");
    stream.flush().expect("flush gzip SSE request");

    for _ in 0..100 {
        if events.subscriber_count() > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(events.subscriber_count(), 1, "gzip SSE subscriber registered");
    events.publish(json!({
        "channel": "runtime",
        "payload": { "value": 9 }
    }));

    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    let header_end = loop {
        let count = stream.read(&mut chunk).expect("read gzip SSE frame");
        assert!(count > 0, "gzip SSE closed before compressed body");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let end = index + 4;
            if bytes.len() >= end + 10 {
                break end;
            }
        }
        assert!(bytes.len() < 32 * 1024, "unexpectedly large gzip SSE response");
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("gzip SSE headers utf8");
    let lower = headers.to_ascii_lowercase();
    assert!(headers.starts_with("HTTP/1.1 200 OK"), "{headers}");
    assert!(lower.contains("content-encoding: gzip"), "{headers}");
    assert!(lower.contains("vary: accept-encoding"), "{headers}");
    assert_eq!(&bytes[header_end..header_end + 2], &[0x1f, 0x8b]);

    drop(stream);
    server.close();
}


#[test]
fn gateway_command_telemetry_preserves_request_trace_and_server_error_semantics() {
    let api = ReportingApi::default();
    let reports = Arc::clone(&api.reports);
    let contexts = Arc::clone(&api.contexts);
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(api),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 1,
    })
    .expect("gateway server");
    let port = server.port();
    let valid_traceparent =
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    let success = request(
        port,
        &format!(
            "POST /api/getTranscript HTTP/1.1\r\nHost: 127.0.0.1\r\nx-sand-request-id: request-ok\r\ntraceparent: {valid_traceparent}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    );
    assert!(success.starts_with("HTTP/1.1 200 OK"), "{success}");

    let failed_body = r#"{"fail":true}"#;
    let failed = request(
        port,
        &format!(
            "POST /api/getTranscript HTTP/1.1\r\nHost: 127.0.0.1\r\nx-sand-request-id: request-fail\r\ntraceparent: invalid\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{failed_body}",
            failed_body.len()
        ),
    );
    assert!(failed.starts_with("HTTP/1.1 500 Internal Server Error"), "{failed}");

    let contexts = contexts.lock().expect("read contexts");
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0].0.as_deref(), Some("request-ok"));
    assert_eq!(contexts[0].1.as_deref(), Some(valid_traceparent));
    assert_eq!(
        contexts[0].2.as_deref(),
        Some("4bf92f3577b34da6a3ce929d0e0e4736")
    );
    assert_eq!(contexts[0].3.as_deref(), Some("00f067aa0ba902b7"));
    assert!(contexts[0].4);
    assert_eq!(contexts[1].0.as_deref(), Some("request-fail"));
    assert_eq!(contexts[1].1, None);
    assert_eq!(contexts[1].2, None);
    assert_eq!(contexts[1].3, None);
    assert!(contexts[1].4);
    drop(contexts);

    let reports = reports.lock().expect("read reports");
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].0, "complete");
    assert_eq!(reports[0].1.method, "getTranscript");
    assert_eq!(reports[0].1.request_id.as_deref(), Some("request-ok"));
    assert_eq!(reports[0].1.traceparent.as_deref(), Some(valid_traceparent));
    assert_eq!(reports[0].1.status, 200);
    assert_eq!(reports[0].1.error, None);

    assert_eq!(reports[1].0, "error");
    assert_eq!(reports[1].1.method, "getTranscript");
    assert_eq!(reports[1].1.request_id.as_deref(), Some("request-fail"));
    assert_eq!(reports[1].1.traceparent, None);
    assert_eq!(reports[1].1.status, 500);
    assert_eq!(
        reports[1].1.error.as_deref(),
        Some("synthetic internal failure")
    );
    assert_eq!(reports[1].1.reason.as_deref(), Some("application"));
    assert_eq!(reports[1].1.error_class.as_deref(), Some("GatewayCommandError"));
    assert_eq!(reports[1].1.errno, None);

    server.close();
}

#[test]
fn openclaw_gateway_device_pairing_handshake_allows_unauthenticated() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(Some("configured-secret-token")),
        started_at: 100,
    })
    .expect("gateway server");
    let port = server.port();

    let pair_req = request(
        port,
        "POST /api/device.pair.request HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(pair_req.starts_with("HTTP/1.1 200 OK"), "{pair_req}");

    let pair_status = request(
        port,
        "POST /api/device.pair.status HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(pair_status.starts_with("HTTP/1.1 200 OK"), "{pair_status}");

    let pair_list_unauth = request(
        port,
        "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(pair_list_unauth.starts_with("HTTP/1.1 401 Unauthorized"), "{pair_list_unauth}");

    let pair_list_auth = request(
        port,
        "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer configured-secret-token\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(pair_list_auth.starts_with("HTTP/1.1 200 OK"), "{pair_list_auth}");

    server.close();
}

#[test]
fn openclaw_gateway_device_timestamp_skew_and_nonce_require_authentication() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(Some("configured-secret-token")),
        started_at: 100,
    })
    .expect("gateway server");
    let port = server.port();

    let skew_req = request(
        port,
        "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nx-sand-device-timestamp: 1000\r\nx-sand-device-nonce: nonce-skew-test\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(skew_req.starts_with("HTTP/1.1 401 Unauthorized"), "{skew_req}");
    assert!(skew_req.contains("TIMESTAMP_SKEW"), "{skew_req}");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let valid_req = request(
        port,
        &format!(
            "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nx-sand-device-timestamp: {now}\r\nx-sand-device-nonce: fresh-nonce-999\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    );
    assert!(valid_req.starts_with("HTTP/1.1 401 Unauthorized"), "{valid_req}");
    assert!(valid_req.contains("unauthorized"), "{valid_req}");

    let authenticated_req = request(
        port,
        &format!(
            "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer configured-secret-token\r\nx-sand-device-timestamp: {now}\r\nx-sand-device-nonce: fresh-nonce-authenticated\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    );
    assert!(authenticated_req.starts_with("HTTP/1.1 200 OK"), "{authenticated_req}");

    let replay_req = request(
        port,
        &format!(
            "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer configured-secret-token\r\nx-sand-device-timestamp: {now}\r\nx-sand-device-nonce: fresh-nonce-authenticated\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    );
    assert!(replay_req.starts_with("HTTP/1.1 401 Unauthorized"), "{replay_req}");
    assert!(replay_req.contains("NONCE_REPLAY"), "{replay_req}");

    let stale_req = request(
        port,
        &format!(
            "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nx-sand-device-timestamp: {now}\r\nx-sand-device-nonce: nonce-stale-rev\r\nx-sand-pairing-revision: stale-1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    );
    assert!(stale_req.starts_with("HTTP/1.1 401 Unauthorized"), "{stale_req}");
    assert!(stale_req.contains("PAIRING_CHANGED"), "{stale_req}");

    server.close();
}

#[test]
fn openclaw_gateway_role_and_scope_enforcement() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(None),
        started_at: 100,
    })
    .expect("gateway server");
    let port = server.port();

    let role_mismatch = request(
        port,
        "POST /api/device.pair.revoke HTTP/1.1\r\nHost: 127.0.0.1\r\nx-sand-role: node\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(role_mismatch.starts_with("HTTP/1.1 403 Forbidden"), "{role_mismatch}");
    assert!(role_mismatch.contains("ROLE_MISMATCH"), "{role_mismatch}");

    let scope_insufficient = request(
        port,
        "POST /api/device.pair.revoke HTTP/1.1\r\nHost: 127.0.0.1\r\nx-sand-scope: operator.read\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(scope_insufficient.starts_with("HTTP/1.1 403 Forbidden"), "{scope_insufficient}");
    assert!(scope_insufficient.contains("INSUFFICIENT_SCOPE"), "{scope_insufficient}");

    let origin_req = request(
        port,
        "POST /api/device.pair.list HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.com\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(origin_req.starts_with("HTTP/1.1 403 Forbidden"), "{origin_req}");

    server.close();
}

#[test]
fn openclaw_gateway_websocket_handshake_with_connect_params() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(Some("ws-secret")),
        started_at: 100,
    })
    .expect("gateway server");
    let port = server.port();

    let unauth_ws = request(
        port,
        "GET /ws HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
    );
    assert!(unauth_ws.starts_with("HTTP/1.1 401 Unauthorized"), "{unauth_ws}");

    let auth_ws = request(
        port,
        "GET /ws?token=ws-secret&role=operator&clientId=test-client HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
    );
    assert!(auth_ws.starts_with("HTTP/1.1 101 Switching Protocols"), "{auth_ws}");
    assert!(auth_ws.contains("Upgrade: websocket"), "{auth_ws}");
    assert!(auth_ws.contains("\"role\":\"operator\""), "{auth_ws}");

    server.close();
}

#[test]
fn openclaw_gateway_openresponses_http_endpoint_wires_to_host() {
    let server = start_gateway_server(GatewayServerDeps {
        api: Arc::new(TestApi),
        events: GatewayEventHub::default(),
        local_exec: None,
        webauthn: None,
        config: config(Some("open-secret")),
        started_at: 100,
    })
    .expect("gateway server");
    let port = server.port();

    let body = json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "user", "content": "Hello open responses"}
        ]
    }).to_string();

    let res = request(
        port,
        &format!(
            "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer open-secret\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );
    assert!(res.starts_with("HTTP/1.1 200 OK"), "{res}");
    assert!(res.contains("\"object\":\"chat.completion\""), "{res}");

    server.close();
}

#[test]
fn openclaw_gateway_webhook_rate_limiter_enforces_budget() {
    use mahayana_host_runtime::gateway_server::WebhookRateLimiter;
    use std::time::Duration;

    let limiter = WebhookRateLimiter::new(2, Duration::from_secs(10));
    assert!(limiter.check_admission("client-1").is_ok());
    assert!(limiter.check_admission("client-1").is_ok());
    assert!(limiter.check_admission("client-1").is_err());
    assert!(limiter.check_admission("client-2").is_ok());
}

#[test]
fn openclaw_gateway_session_reset_service_seam_retires_placement() {
    use mahayana_host_runtime::gateway_server::gateway_session_reset_service_seam;

    let empty = gateway_session_reset_service_seam("", None);
    assert!(empty.is_err());

    let retired = gateway_session_reset_service_seam("sess-42", Some(3)).expect("retired");
    assert_eq!(retired["sessionId"], "sess-42");
    assert_eq!(retired["retired"], true);
    assert_eq!(retired["expectedRevision"], 3);
    assert_eq!(retired["status"], "placement_retired_before_mutation");
}
