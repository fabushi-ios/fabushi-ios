use mahayana_host_runtime::extensions::cross_user_sharing::xuser_remote_turns::*;
use mahayana_host_runtime::extensions::cross_user_sharing::xuser_state_reconcile::{
    RoomMember, XuserRoom,
};
use mahayana_host_runtime::extensions::cross_user_sharing::xuser_turn_dedupe_store::{
    InMemoryXuserTurnDedupe, XUSER_TURN_DEDUPE_TTL_MS,
};
use mahayana_host_runtime::groups::group_chat::{
    GroupDescription, GroupMember, GroupMessage, GroupSpeaker,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

struct FakeRemoteHost {
    now_ms: AtomicU64,
    enabled: bool,
    self_auth_id: Option<String>,
    rooms: Vec<XuserRoom>,
    run_count: AtomicUsize,
    sent: Mutex<Vec<Value>>,
    outgoing_turn_requests: Mutex<Option<mpsc::Sender<Value>>>,
}

impl FakeRemoteHost {
    fn fixture(outgoing_turn_requests: Option<mpsc::Sender<Value>>) -> Arc<Self> {
        Arc::new(Self {
            now_ms: AtomicU64::new(1_000),
            enabled: true,
            self_auth_id: Some("self-auth".into()),
            rooms: vec![XuserRoom {
                room_id: "room-a".into(),
                name: "Shared".into(),
                host_auth_id: "host-auth".into(),
                members: vec![
                    RoomMember {
                        auth_id: "self-auth".into(),
                        agent_id: None,
                        name: Some("Local user".into()),
                    },
                    RoomMember {
                        auth_id: "self-auth".into(),
                        agent_id: Some("agent-local".into()),
                        name: Some("Local Agent".into()),
                    },
                    RoomMember {
                        auth_id: "host-auth".into(),
                        agent_id: None,
                        name: Some("Remote Host".into()),
                    },
                ],
                avatar_data_url: None,
            }],
            run_count: AtomicUsize::new(0),
            sent: Mutex::new(Vec::new()),
            outgoing_turn_requests: Mutex::new(outgoing_turn_requests),
        })
    }
}

impl RemoteTurnsHost for FakeRemoteHost {
    fn relay_send(&self, payload: &Value) -> Result<Value, String> {
        self.sent.lock().unwrap().push(payload.clone());
        if payload.get("kind").and_then(Value::as_str) == Some("turn-request") {
            if let Some(tx) = self.outgoing_turn_requests.lock().unwrap().as_ref() {
                let _ = tx.send(payload.clone());
            }
        }
        Ok(json!({}))
    }

    fn relay_request_drain(&self) {}

    fn relay_remove_deleted_agent(&self, _agent_id: &str) -> Result<Value, String> {
        Ok(json!({}))
    }

    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::Acquire)
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn self_auth_id(&self) -> Option<String> {
        self.self_auth_id.clone()
    }

    fn rooms(&self) -> Vec<XuserRoom> {
        self.rooms.clone()
    }

    fn get_agent_display_profile(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentDisplayProfile>, String> {
        Ok((agent_id == "agent-local").then(|| AgentDisplayProfile {
            name: "Local Agent".into(),
            description: "Helpful".into(),
        }))
    }

    fn append_shared_room_activity_notice(
        &self,
        _room_id: &str,
        _text: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    fn run_remote_requested_member_turn(
        &self,
        _agent_id: &str,
        system_prompt: &str,
        prompt: &str,
    ) -> Result<Vec<String>, String> {
        assert!(system_prompt.contains("SHARED ROOM"));
        assert!(system_prompt.contains("Remote Host"));
        assert!(prompt.contains("Guest"));
        self.run_count.fetch_add(1, Ordering::AcqRel);
        Ok(vec!["first".into(), "second".into(), "must-be-clamped".into()])
    }

    fn reconcile_share_state(&self) -> Result<(), String> {
        Ok(())
    }
}

fn remote_turns(host: Arc<FakeRemoteHost>) -> Arc<SandXuserRemoteTurns> {
    let dedupe = Arc::new(InMemoryXuserTurnDedupe::new(
        XUSER_TURN_DEDUPE_TTL_MS,
        Box::new(|| 1_000),
    ));
    let host_trait: Arc<dyn RemoteTurnsHost> = host;
    let dedupe_trait: Arc<
        dyn mahayana_host_runtime::extensions::cross_user_sharing::xuser_turn_dedupe_store::XuserTurnDedupe,
    > = dedupe;
    Arc::new(SandXuserRemoteTurns::new(host_trait, dedupe_trait))
}

#[test]
fn incoming_remote_turn_enforces_room_membership_dedupe_and_two_message_settlement() {
    let host = FakeRemoteHost::fixture(None);
    let turns = remote_turns(Arc::clone(&host));
    let event = json!({
        "kind": "turn-request",
        "roomId": "room-a",
        "turnNonce": "nonce-a",
        "agentId": "agent-local",
        "hostAuthId": "host-auth",
        "groupName": "Shared",
        "groupDescription": "Cross-user room",
        "peers": [{"name":"Peer","description":"Other agent"}],
        "newMessages": [{
            "speakerKind":"human",
            "speakerName":"Guest",
            "text":"@Local Agent hello",
            "isSelf":false
        }]
    });
    assert!(turns.handle_turn_request(&event));

    for _ in 0..100 {
        let settled = host
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|value| value.get("kind").and_then(Value::as_str) == Some("turn-result"));
        if settled {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host.run_count.load(Ordering::Acquire), 1);
    let sent = host.sent.lock().unwrap();
    let settled = sent
        .iter()
        .find(|value| value.get("kind").and_then(Value::as_str) == Some("turn-result"))
        .expect("turn result");
    assert_eq!(settled["turnNonce"], "nonce-a");
    assert_eq!(settled["messages"], json!(["first", "second"]));
    drop(sent);

    assert!(turns.handle_turn_request(&event));
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(host.run_count.load(Ordering::Acquire), 1);
}

#[test]
fn outgoing_remote_turn_uses_frozen_wire_shape_and_result_nonce_settlement() {
    let (request_tx, request_rx) = mpsc::channel();
    let host = FakeRemoteHost::fixture(Some(request_tx));
    let turns = remote_turns(Arc::clone(&host));
    let runner = Arc::clone(&turns);

    let worker = std::thread::spawn(move || {
        runner.run_remote_member_turn(
            &GroupMember {
                id: "sand-remote:owner-auth/agent-remote".into(),
                name: "Remote Agent".into(),
                description: "Remote".into(),
            },
            "room-a",
            &GroupDescription {
                name: "Shared".into(),
                description: "Cross-user room".into(),
            },
            &[GroupMember {
                id: "peer".into(),
                name: "Peer".into(),
                description: "Peer".into(),
            }],
            &[GroupMessage {
                speaker: GroupSpeaker::User {
                    name: Some("Guest".into()),
                },
                content: "hello".into(),
            }],
        )
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("turn request");
    assert_eq!(request["ownerAuthId"], "owner-auth");
    assert_eq!(request["agentId"], "agent-remote");
    assert_eq!(request["roomId"], "room-a");
    let nonce = request["turnNonce"].as_str().unwrap().to_string();
    assert_eq!(turns.pending_remote_turn_count(), 1);
    assert!(turns.handle_turn_result(&json!({
        "kind":"turn-result",
        "turnNonce":nonce,
        "messages":["remote-one","remote-two","ignored"]
    })));

    assert_eq!(worker.join().unwrap(), vec!["remote-one", "remote-two"]);
    assert_eq!(turns.pending_remote_turn_count(), 0);
}

#[test]
fn remote_turn_budget_is_windowed_and_stop_releases_pending_waiters() {
    let budget = RemoteTurnBudget::new(2, 1_000);
    assert!(budget.accept(0));
    assert!(budget.accept(10));
    assert!(!budget.accept(999));
    assert!(budget.accept(1_000));
    budget.clear();
    assert!(budget.accept(1_001));

    let host = FakeRemoteHost::fixture(None);
    let turns = remote_turns(host);
    turns.stop();
    assert_eq!(turns.pending_remote_turn_count(), 0);
}
