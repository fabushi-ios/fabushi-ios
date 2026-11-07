use std::sync::{Arc, Mutex};

use futures::FutureExt;
use mahayana_host_runtime::extensions::transcript::group_chat_orchestrator::{
    GroupChatOrchestrator, GroupMemberTurnRequest, GroupOrchestratorDeps,
};
use mahayana_host_runtime::groups::group_chat::{
    GROUP_MAX_MEMBER_TURNS, GroupDescription, GroupMember, GroupMessage, GroupSpeaker,
};

#[derive(Clone)]
struct TestDeps {
    members: Vec<GroupMember>,
    history: Arc<Mutex<Vec<GroupMessage>>>,
    posted: Arc<Mutex<Vec<(String, String)>>>,
    turns: Arc<Mutex<Vec<GroupMemberTurnRequest>>>,
    current: Arc<Mutex<bool>>,
    shared: bool,
}

impl GroupOrchestratorDeps for TestDeps {
    fn resolve_members<'a>(&'a self, _ids: &'a [String]) -> futures::future::BoxFuture<'a, Vec<GroupMember>> {
        futures::future::ready(self.members.clone()).boxed()
    }

    fn read_history(&self) -> Vec<GroupMessage> {
        self.history.lock().expect("history").clone()
    }

    fn is_current(&self) -> bool {
        *self.current.lock().expect("current")
    }

    fn run_member_turn<'a>(
        &'a self,
        request: GroupMemberTurnRequest,
    ) -> futures::future::BoxFuture<'a, Vec<String>> {
        self.turns.lock().expect("turns").push(request.clone());
        let content = if request.member.id == "a" {
            vec!["(pass)".into(), "  A says hi  ".into(), "ignored third".into()]
        } else {
            vec!["B replies".into()]
        };
        futures::future::ready(content).boxed()
    }

    fn post_member_message(&self, member: &GroupMember, content: &str) {
        self.posted.lock().expect("posted").push((member.id.clone(), content.into()));
        self.history.lock().expect("history").push(GroupMessage {
            speaker: GroupSpeaker::Member { id: member.id.clone(), name: member.name.clone() },
            content: content.into(),
        });
    }

    fn is_shared_room(&self) -> bool {
        self.shared
    }
}

fn member(id: &str, name: &str) -> GroupMember {
    GroupMember { id: id.into(), name: name.into(), description: String::new() }
}

#[test]
fn bounded_orchestrator_filters_passes_trims_and_stops_at_frozen_caps() {
    let deps = TestDeps {
        members: vec![member("a", "Alice"), member("b", "Bob")],
        history: Arc::new(Mutex::new(vec![GroupMessage {
            speaker: GroupSpeaker::User { name: None },
            content: "hello".into(),
        }])),
        posted: Arc::new(Mutex::new(Vec::new())),
        turns: Arc::new(Mutex::new(Vec::new())),
        current: Arc::new(Mutex::new(true)),
        shared: false,
    };
    let posted = Arc::clone(&deps.posted);
    let turns = Arc::clone(&deps.turns);
    let orchestrator = GroupChatOrchestrator::new(deps);
    futures::executor::block_on(orchestrator.run(
        &GroupDescription { name: "Team".into(), description: String::new() },
        &["a".into(), "b".into()],
    ));
    let posted = posted.lock().expect("posted");
    assert!(!posted.is_empty());
    assert!(posted.len() <= GROUP_MAX_MEMBER_TURNS);
    assert!(posted.iter().all(|(_, content)| content != "(pass)"));
    assert!(posted.iter().any(|(id, content)| id == "a" && content == "A says hi"));
    let turns = turns.lock().expect("turns");
    assert!(!turns.is_empty());
    assert!(turns[0].system_prompt.contains("SendMessage"));
}

#[test]
fn orchestrator_respects_epoch_cancellation_before_turns() {
    let deps = TestDeps {
        members: vec![member("a", "Alice")],
        history: Arc::new(Mutex::new(Vec::new())),
        posted: Arc::new(Mutex::new(Vec::new())),
        turns: Arc::new(Mutex::new(Vec::new())),
        current: Arc::new(Mutex::new(false)),
        shared: false,
    };
    let posted = Arc::clone(&deps.posted);
    let turns = Arc::clone(&deps.turns);
    let orchestrator = GroupChatOrchestrator::new(deps);
    futures::executor::block_on(orchestrator.run(
        &GroupDescription { name: "Team".into(), description: String::new() },
        &["a".into()],
    ));
    assert!(posted.lock().expect("posted").is_empty());
    assert!(turns.lock().expect("turns").is_empty());
}
