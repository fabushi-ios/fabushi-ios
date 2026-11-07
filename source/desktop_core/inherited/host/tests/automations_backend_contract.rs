use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::automations::backend_relay_source::{
    BackendRelayRuntime, map_relay_wire_event, relay_subscription_request,
};
use mahayana_host_runtime::extensions::automations::backend_transport::{
    AutomationsBackendError, AutomationsBackendTransport,
};
use mahayana_host_runtime::extensions::automations::sand_automation_fire_consumer::{
    AutomationFireBackendRuntime, FireCompletion, NOTIFY_DRAIN_FLOOR_MS, NOTIFY_SAFETY_POLL_MS,
};
use serde_json::{Value, json};

#[derive(Default)]
struct MockTransport {
    responses: Mutex<VecDeque<Result<Value, AutomationsBackendError>>>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl MockTransport {
    fn with_responses(responses: Vec<Result<Value, AutomationsBackendError>>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("calls mutex").clone()
    }
}

impl AutomationsBackendTransport for MockTransport {
    fn post_json(&self, path: &str, body: &Value) -> Result<Value, AutomationsBackendError> {
        self.calls
            .lock()
            .expect("calls mutex")
            .push((path.to_string(), body.clone()));
        self.responses
            .lock()
            .expect("responses mutex")
            .pop_front()
            .expect("mock response")
    }
}

#[test]
fn relay_subscription_request_is_stable_and_deduplicated() {
    let request = relay_subscription_request(
        &[
            json!({"type":"slack","channel":"#b"}),
            json!({"type":"slack","channel":"#a"}),
            json!({"type":"slack","channel":"#b"}),
        ],
        &[
            json!({"type":"github","repo":"OpenAI/Repo","events":["pr-opened","pr-merged"]}),
            json!({"type":"github","repo":"openai/repo","events":["pr-opened"]}),
        ],
    );
    assert_eq!(
        request,
        json!({
            "slackChannels":["#a","#b"],
            "githubRepos":["openai/repo"],
            "githubKinds":["pr-merged","pr-opened"],
        })
    );
}

#[test]
fn relay_wire_mapping_matches_grok_slack_and_github_shapes() {
    assert_eq!(
        map_relay_wire_event(
            &json!({
                "source":"slack",
                "kind":"reaction",
                "channelName":"alerts",
                "channelId":"C1",
                "senderSlackUserId":"U1",
                "reactionEmoji":":eyes:",
                "text":"ship it"
            }),
            42,
        ),
        Some(json!({
            "source":"slack",
            "channel":"#alerts",
            "sender":"@U1",
            "text":"ship it",
            "isMention":false,
            "isSelf":false,
            "reactionEmoji":":eyes:",
            "timestampMs":42
        }))
    );
    assert_eq!(
        map_relay_wire_event(
            &json!({
                "source":"github",
                "repo":"openai/repo",
                "kind":"pr-opened",
                "actor":"octocat",
                "prOwner":"owner"
            }),
            43,
        ),
        Some(json!({
            "source":"github",
            "repo":"openai/repo",
            "kind":"pr-opened",
            "title":"",
            "actor":"octocat",
            "prOwner":"owner",
            "timestampMs":43
        }))
    );
}

#[test]
fn relay_runtime_registers_polls_and_acks_only_accepted_or_unmappable_events() {
    let transport = MockTransport::with_responses(vec![
        Ok(json!({"slack":{"status":"ok"},"github":{"status":"ok"}})),
        Ok(json!({"events":[
            {"id":"accepted","source":"slack","channelName":"alerts","text":"one"},
            {"id":"retry","source":"slack","channelName":"alerts","text":"two"},
            {"id":"bad","source":"unknown"}
        ]})),
        Ok(json!({"events":[
            {"id":"accepted","source":"slack","channelName":"alerts","text":"one"},
            {"id":"bad","source":"unknown"}
        ]})),
    ]);
    let mut runtime = BackendRelayRuntime::new(transport.clone());
    runtime.set_listeners(vec![json!({"type":"slack","channel":"#alerts","match":{"kind":"message"}})], vec![]);

    let delivered = runtime
        .tick(1_000, true, true, |event| event.get("text").and_then(Value::as_str) != Some("two"))
        .expect("first relay tick");
    assert_eq!(delivered, 1);
    assert_eq!(runtime.pending_ack_ids(), &["accepted".to_string(), "bad".to_string()]);

    runtime.tick(1_001, true, true, |_| true).expect("ack relay tick");
    let calls = transport.calls();
    assert_eq!(calls[0].0, "/sand/listener-subscriptions");
    assert_eq!(calls[1].0, "/sand/listener-events/poll");
    assert_eq!(calls[2].1, json!({"ackIds":["accepted","bad"]}));
}

#[test]
fn relay_runtime_backoff_suppresses_immediate_retry() {
    let transport = MockTransport::with_responses(vec![
        Err(AutomationsBackendError::Status { path:"/sand/listener-subscriptions".into(), status:503 }),
    ]);
    let mut runtime = BackendRelayRuntime::new(transport.clone());
    runtime.set_listeners(vec![json!({"type":"slack","channel":"#alerts"})], vec![]);

    assert!(runtime.tick(100, true, true, |_| true).is_err());
    assert_eq!(runtime.tick(101, true, true, |_| true).expect("backoff tick"), 0);
    assert_eq!(transport.calls().len(), 1);
}

#[test]
fn fire_runtime_polls_delivers_reports_and_acks_on_next_poll() {
    let transport = MockTransport::with_responses(vec![
        Ok(json!({"events":[{
            "id":"run-1",
            "sandAgentId":"agent-1",
            "automationId":"cloud-1",
            "timestampMs":10,
            "scheduledForMs":9
        }]})),
        Ok(json!({})),
        Ok(json!({"events":[]})),
    ]);
    let mut runtime = AutomationFireBackendRuntime::new(transport.clone());

    let count = runtime
        .tick(100, true, true, |event| {
            assert_eq!(event.id, "run-1");
            assert_eq!(event.sand_agent_id, "agent-1");
            true
        })
        .expect("fire tick");
    assert_eq!(count, 1);
    assert!(runtime.is_running("run-1"));
    runtime
        .complete("run-1", FireCompletion::succeeded())
        .expect("completion report");
    assert_eq!(runtime.pending_len(), 1);

    runtime.tick(101, true, true, |_| false).expect("ack fire tick");
    assert_eq!(runtime.pending_len(), 0);

    let calls = transport.calls();
    assert_eq!(calls[0], ("/sand/automation-events/poll".into(), json!({"ackRunUuids":[]})));
    assert_eq!(
        calls[1],
        (
            "/sand/automation-runs/complete".into(),
            json!({"runUuid":"run-1","status":"succeeded","errorMessage":null})
        )
    );
    assert_eq!(
        calls[2],
        ("/sand/automation-events/poll".into(), json!({"ackRunUuids":["run-1"]}))
    );
}

#[test]
fn fire_runtime_does_not_poll_until_run_execution_is_ready() {
    let transport = MockTransport::with_responses(vec![
        Ok(json!({"events":[]})),
    ]);
    let mut runtime = AutomationFireBackendRuntime::new(transport.clone());

    let count = runtime
        .tick_with_state(100, true, true, false, true, |_| true)
        .expect("not-ready tick");
    assert_eq!(count, 0);
    assert!(transport.calls().is_empty());

    runtime
        .tick_with_state(101, true, true, true, true, |_| true)
        .expect("ready tick");
    assert_eq!(transport.calls().len(), 1);
}

#[test]
fn fire_runtime_drains_once_while_no_server_schedulable_routines_exist() {
    let transport = MockTransport::with_responses(vec![
        Ok(json!({"events":[]})),
        Ok(json!({"events":[]})),
    ]);
    let mut runtime = AutomationFireBackendRuntime::new(transport.clone());

    runtime
        .tick_with_state(100, true, true, true, false, |_| true)
        .expect("initial drain");
    assert_eq!(transport.calls().len(), 1);

    runtime
        .tick_with_state(101, true, true, true, false, |_| true)
        .expect("suppressed empty drain");
    assert_eq!(transport.calls().len(), 1);

    runtime.start();
    runtime
        .tick_with_state(102, true, true, true, false, |_| true)
        .expect("config-reset drain");
    assert_eq!(transport.calls().len(), 2);
}

#[test]
fn fire_runtime_matches_frozen_notify_floor_and_safety_poll_gate() {
    let transport = MockTransport::with_responses(vec![
        Ok(json!({"events":[]})),
        Ok(json!({"events":[]})),
        Ok(json!({"events":[]})),
    ]);
    let mut runtime = AutomationFireBackendRuntime::new(transport.clone());

    runtime
        .tick_with_state(100, true, true, true, true, |_| true)
        .expect("initial connected poll");
    assert_eq!(transport.calls().len(), 1);

    runtime.request_drain();
    runtime
        .tick_with_state(
            100 + NOTIFY_DRAIN_FLOOR_MS - 1,
            true,
            true,
            true,
            true,
            |_| true,
        )
        .expect("notify floor");
    assert_eq!(transport.calls().len(), 1);

    runtime
        .tick_with_state(
            100 + NOTIFY_DRAIN_FLOOR_MS,
            true,
            true,
            true,
            true,
            |_| true,
        )
        .expect("notify drain");
    assert_eq!(transport.calls().len(), 2);

    runtime
        .tick_with_state(
            100 + NOTIFY_DRAIN_FLOOR_MS + NOTIFY_SAFETY_POLL_MS - 1,
            true,
            true,
            true,
            true,
            |_| true,
        )
        .expect("before safety poll");
    assert_eq!(transport.calls().len(), 2);

    runtime
        .tick_with_state(
            100 + NOTIFY_DRAIN_FLOOR_MS + NOTIFY_SAFETY_POLL_MS,
            true,
            true,
            true,
            true,
            |_| true,
        )
        .expect("safety poll");
    assert_eq!(transport.calls().len(), 3);
}

#[test]
fn fire_runtime_disconnected_and_owed_work_bypass_notify_gate() {
    let transport = MockTransport::with_responses(vec![
        Ok(json!({"events":[]})),
        Ok(json!({"events":[{
            "id":"run-1",
            "sandAgentId":"agent-1",
            "automationId":"cloud-1",
            "timestampMs":10
        }]})),
        Ok(json!({"events":[{"id":"run-1","sandAgentId":"agent-1","automationId":"cloud-1","timestampMs":10}]})),
    ]);
    let mut runtime = AutomationFireBackendRuntime::new(transport.clone());

    runtime
        .tick_with_state(100, true, false, true, true, |_| true)
        .expect("initial poll");
    runtime
        .tick_with_state(101, false, false, true, true, |_| true)
        .expect("disconnected poll");
    assert_eq!(transport.calls().len(), 2);
    assert!(runtime.is_running("run-1"));

    runtime
        .tick_with_state(102, true, false, true, true, |_| true)
        .expect("owed work poll");
    assert_eq!(transport.calls().len(), 3);
}

#[test]
fn fire_runtime_config_reset_does_not_impersonate_notify() {
    let transport = MockTransport::with_responses(vec![
        Ok(json!({"events":[],"nextPollAfterMs":60000})),
        Ok(json!({"events":[]})),
    ]);
    let mut runtime = AutomationFireBackendRuntime::new(transport.clone());

    runtime
        .tick_with_state(100, true, true, true, true, |_| true)
        .expect("initial poll");
    runtime.reset_poll_delay();
    runtime
        .tick_with_state(101, true, true, true, true, |_| true)
        .expect("config reset remains notify-gated");
    assert_eq!(transport.calls().len(), 1);

    runtime
        .tick_with_state(
            100 + NOTIFY_SAFETY_POLL_MS,
            true,
            true,
            true,
            true,
            |_| true,
        )
        .expect("safety poll after config reset");
    assert_eq!(transport.calls().len(), 2);
}

#[test]
fn fire_runtime_successful_completion_honors_next_poll_delay() {
    let transport = MockTransport::with_responses(vec![
        Ok(json!({"events":[{
            "id":"run-1",
            "sandAgentId":"agent-1",
            "automationId":"cloud-1",
            "timestampMs":10
        }]})),
        Ok(json!({})),
        Ok(json!({"events":[]})),
        Ok(json!({"events":[],"nextPollAfterMs":5000})),
    ]);
    let mut runtime = AutomationFireBackendRuntime::new(transport.clone());
    runtime
        .tick(100, true, true, |_| true)
        .expect("delivery tick");
    runtime
        .complete("run-1", FireCompletion::failed("boom"))
        .expect("successful completion");
    runtime.tick(101, true, true, |_| false).expect("ack tick");
    runtime.tick(102, true, true, |_| false).expect("empty tick");
    let before = transport.calls().len();
    runtime.tick(103, true, true, |_| false).expect("delayed tick");
    assert_eq!(transport.calls().len(), before);
}


#[test]
fn relay_runtime_preserves_scope_and_repo_degradation_status() {
    let transport = MockTransport::with_responses(vec![
        Ok(json!({
            "slack": {
                "status": "ok",
                "teams": [{
                    "channels": [{"input":"#private","isBotMember":false}],
                    "unresolvedChannels": ["#missing"]
                }]
            },
            "github": {
                "status": "ok",
                "repos": [
                    {"repo":"org/good","isSubscribed":true},
                    {"repo":"org/bad","isSubscribed":false,"detail":"Missing repository access."}
                ]
            }
        })),
        Ok(json!({"events":[]}))
    ]);
    let mut runtime = BackendRelayRuntime::new(transport);
    runtime.set_listeners(
        vec![json!({"type":"slack","channel":"#private","match":{"kind":"message"}})],
        vec![json!({"type":"github","repo":"org/bad","events":["pr-opened"]})],
    );
    runtime.tick(1_000, true, true, |_| true).expect("relay status tick");

    assert_eq!(runtime.slack_status.state, "listening");
    assert_eq!(runtime.slack_status.scope_issues.len(), 2);
    assert!(runtime.slack_status.detail.as_deref().unwrap_or_default().contains("Invite @Cursor"));
    assert!(runtime.slack_status.detail.as_deref().unwrap_or_default().contains("#missing"));
    assert_eq!(runtime.github_status.state, "listening");
    assert_eq!(
        runtime.github_status.detail.as_deref(),
        Some("Missing repository access.")
    );
}


#[test]
fn fire_runtime_never_acks_unconfirmed_completion_or_redispatches_redelivery() {
    for error in [
        AutomationsBackendError::Status { path: "/sand/automation-runs/complete".into(), status: 401 },
        AutomationsBackendError::Status { path: "/sand/automation-runs/complete".into(), status: 404 },
        AutomationsBackendError::Status { path: "/sand/automation-runs/complete".into(), status: 409 },
        AutomationsBackendError::Transport("completion response lost".into()),
    ] {
        let delivery = json!({"events":[{
            "id":"run-1", "sandAgentId":"agent-1", "automationId":"cloud-1", "timestampMs":10
        }]});
        let transport = MockTransport::with_responses(vec![
            Ok(delivery.clone()), Err(error.clone()),
            Ok(delivery.clone()), Err(error),
            Ok(delivery), Ok(json!({})),
            Ok(json!({"events":[]})),
        ]);
        let mut runtime = AutomationFireBackendRuntime::new(transport.clone());
        let mut dispatched = 0;
        runtime.tick(100, false, false, |_| { dispatched += 1; true }).unwrap();
        let completion = FireCompletion::succeeded();
        assert!(runtime.complete("run-1", completion).is_err());
        assert_eq!(runtime.pending_len(), 1);
        runtime.tick(101, false, false, |_| { dispatched += 1; true }).unwrap();
        assert_eq!(runtime.pending_len(), 1);
        runtime.tick(102, false, false, |_| { dispatched += 1; true }).unwrap();
        runtime.tick(103, false, false, |_| { dispatched += 1; true }).unwrap();
        assert_eq!(dispatched, 1);
        assert_eq!(runtime.pending_len(), 0);
        let calls = transport.calls();
        let polls = calls.iter().filter(|(path, _)| path == "/sand/automation-events/poll")
            .map(|(_, body)| body.clone()).collect::<Vec<_>>();
        assert_eq!(polls, vec![
            json!({"ackRunUuids":[]}), json!({"ackRunUuids":[]}),
            json!({"ackRunUuids":[]}), json!({"ackRunUuids":["run-1"]}),
        ]);
        let reports = calls.iter().filter(|(path, _)| path == "/sand/automation-runs/complete")
            .map(|(_, body)| body.clone()).collect::<Vec<_>>();
        assert_eq!(reports, vec![json!({"runUuid":"run-1", "status":"succeeded", "errorMessage":null}); 3]);
    }
}
