use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;
use std::time::Duration;

use mahayana_node_agent_coordinator::gateway::host_supervisor::{
    fetch_health, GatewayConnection, GatewayHealthDecision, GatewayHostSupervisor,
    HEALTH_PROBE_TTL_MS,
};

fn health_server(
    status: u16,
    body: &'static str,
    expected_authorization: Option<&'static str>,
) -> (GatewayConnection, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind health server");
    let address = listener.local_addr().expect("health server address");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept health request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let count = stream.read(&mut buffer).expect("read health request");
            if count == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..count]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8(request).expect("health request utf8");
        assert!(request.starts_with("GET /health HTTP/1.1\r\n"));
        if let Some(expected) = expected_authorization {
            assert!(
                request
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case(&format!("authorization: {expected}"))),
                "health request did not forward the gateway authorization header: {request}"
            );
        }

        let reason = if status == 200 { "OK" } else { "Service Unavailable" };
        write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("write health response");
        stream.flush().expect("flush health response");
    });

    let mut headers = BTreeMap::new();
    headers.insert("authorization".into(), "Bearer health-test".into());
    (
        GatewayConnection {
            base_url: format!("http://{address}"),
            headers,
        },
        handle,
    )
}

#[test]
fn health_probe_uses_real_http_and_preserves_gateway_auth() {
    let (connection, handle) = health_server(
        200,
        r#"{"ok":true,"generation":7}"#,
        Some("Bearer health-test"),
    );

    let health = fetch_health(&connection, Duration::from_millis(1_500))
        .expect("health probe succeeds")
        .expect("health is accepted");
    assert_eq!(health["generation"], 7);
    handle.join().expect("health server joins");
}

#[test]
fn failed_health_probe_moves_cached_connection_to_reconnect() {
    let (connection, handle) = health_server(
        503,
        r#"{"ok":false}"#,
        Some("Bearer health-test"),
    );

    let mut supervisor = GatewayHostSupervisor::new(HEALTH_PROBE_TTL_MS);
    let attempt = supervisor.begin_connection_attempt();
    supervisor
        .settle_connection_attempt(attempt, connection)
        .expect("install cached gateway");
    assert_eq!(supervisor.decision(10_000), GatewayHealthDecision::Probe);

    assert!(
        !supervisor
            .probe_cached_connection(10_000)
            .expect("unhealthy HTTP response is a health miss")
    );
    assert_eq!(
        supervisor.decision(10_001),
        GatewayHealthDecision::Reconnect
    );
    handle.join().expect("health server joins");
}


#[test]
fn feature_flags_disable_stream_liveness_and_health_ttl_shortcuts() {
    let mut supervisor = GatewayHostSupervisor::with_feature_flags(
        HEALTH_PROBE_TTL_MS,
        true,
        true,
    );
    let mut headers = BTreeMap::new();
    headers.insert("authorization".into(), "Bearer flag-test".into());
    let attempt = supervisor.begin_connection_attempt();
    supervisor
        .settle_connection_attempt(
            attempt,
            GatewayConnection {
                base_url: "http://127.0.0.1:9".into(),
                headers,
            },
        )
        .expect("install connection");

    supervisor.mark_transport_live(true);
    supervisor.record_health(10_000, true);

    assert_eq!(
        supervisor.decision(10_001),
        GatewayHealthDecision::Probe,
        "Grok debug flags must force a real health decision even when the stream is live and the cached health is fresh"
    );
}

#[test]
fn health_transport_failure_is_a_reconnect_decision_not_an_io_escape() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve refused health port");
    let address = listener.local_addr().expect("reserved health address");
    drop(listener);

    let mut supervisor = GatewayHostSupervisor::new(HEALTH_PROBE_TTL_MS);
    let attempt = supervisor.begin_connection_attempt();
    supervisor
        .settle_connection_attempt(
            attempt,
            GatewayConnection {
                base_url: format!("http://{address}"),
                headers: BTreeMap::new(),
            },
        )
        .expect("install cached gateway");

    assert_eq!(supervisor.decision(20_000), GatewayHealthDecision::Probe);
    assert_eq!(
        supervisor
            .probe_cached_connection(20_000)
            .expect("transport failure is normalized to an unhealthy probe"),
        false
    );
    assert_eq!(
        supervisor.decision(20_001),
        GatewayHealthDecision::Reconnect
    );
}
