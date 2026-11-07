use std::fs;
use std::sync::{Arc,Mutex};
use std::sync::atomic::{AtomicUsize,Ordering};
use std::time::{SystemTime,UNIX_EPOCH};
use mahayana_host_runtime::agents::agent_messaging::AgentMessageImage;
use mahayana_host_runtime::agents::agent_profile::SandAgentProfile;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::session::agent_session::{
    SessionCommunicationMode, SessionCommunicationPolicyOverride,
};
use mahayana_host_runtime::extensions::telemetry::agent_error_telemetry::agent_error_telemetry;
use mahayana_host_runtime::extensions::transcript::agent_to_agent_messaging::{
    AgentWakeRequest, ProductionAgentToAgentMessaging, agent_inbound_failure_report,
    agent_inbound_failure_tray, persist_agent_inbound_message, should_interrupt_priority_peer,
};
use mahayana_host_runtime::extensions::transcript::run_scheduler::RunLane;

static NEXT_TEMP_ROOT: AtomicUsize = AtomicUsize::new(0);

fn temp_root()->std::path::PathBuf{
    let suffix=SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
    let ordinal=NEXT_TEMP_ROOT.fetch_add(1,Ordering::Relaxed);
    std::env::temp_dir().join(format!("fabushi-agent-to-agent-{}-{suffix}-{ordinal}",std::process::id()))
}
fn profile(name:&str)->SandAgentProfile{SandAgentProfile{name:name.into(),description:String::new(),title:String::new(),avatar_shape:String::new(),avatar_color:String::new()}}

#[test]
fn direct_delivery_defers_recipient_transcript_until_execution_admission(){
    let root=temp_root();let sessions=Arc::new(ProductionSessionWorkers::with_agents_root(&root,500));
    let alpha=sessions.materialize_new_session(Some(&profile("Alpha")),"user",None).expect("alpha");
    let beta=sessions.materialize_new_session(Some(&profile("Beta")),"user",None).expect("beta");
    let wakes=Arc::new(Mutex::new(Vec::<AgentWakeRequest>::new()));let interrupts=Arc::new(AtomicUsize::new(0));
    let service=ProductionAgentToAgentMessaging::new(Arc::clone(&sessions),{
        let wakes=Arc::clone(&wakes);Arc::new(move|wake|wakes.lock().expect("wakes").push(wake.clone()))
    },Some({let interrupts=Arc::clone(&interrupts);Arc::new(move|_,_|{interrupts.fetch_add(1,Ordering::SeqCst);1})}));
    let alpha_session_key = sessions
        .read_session_work_admission(&alpha.id)
        .expect("alpha admission")
        .expect("alpha admission record")
        .session_key;
    let ack=service.send_to_agent_with_session(&alpha.id,Some(&alpha_session_key),&beta.id,"  please review  ",&[AgentMessageImage{url:"https://example.com/chart.png".into(),alt:Some("chart".into())}],true).expect("send");
    assert!(ack.contains("priority"));
    let a=sessions.read_agent_transcript_entries(&alpha.id).expect("alpha transcript");
    let b=sessions.read_agent_transcript_entries(&beta.id).expect("beta transcript before admission");
    assert_eq!(a.len(),1);assert_eq!(a[0]["toAgent"]["id"],beta.id);assert_eq!(a[0]["content"],"please review");
    assert_eq!(a[0].get("sessionKey").and_then(serde_json::Value::as_str),Some(alpha_session_key.as_str()));
    assert!(b.is_empty(), "recipient entry is deferred until execution admission");
    assert_eq!(sessions.get_agent_conversation_partner_ids(&alpha.id).expect("a partners"),vec![beta.id.clone()]);
    assert!(sessions.get_agent_conversation_partner_ids(&beta.id).expect("b partners before admission").is_empty());
    assert_eq!(interrupts.load(Ordering::SeqCst),1);
    let wake=wakes.lock().expect("wakes").last().cloned().expect("wake");
    assert_eq!(wake.agent_id,beta.id);assert!(wake.priority);assert!(wake.prompt.starts_with("[agent]"));
    let inbound = wake.inbound.as_ref().expect("direct inbound envelope");
    assert_eq!(inbound["replyContext"]["targetId"], a[0]["id"]);
    assert_eq!(inbound["messageId"], a[0]["id"]);
    assert!(inbound.get("replyToId").is_none());
    assert!(persist_agent_inbound_message(&sessions, &beta.id, inbound).expect("persist inbound"));
    let b=sessions.read_agent_transcript_entries(&beta.id).expect("beta transcript after admission");
    assert_eq!(b.len(),1);assert_eq!(b[0]["fromAgent"]["id"],alpha.id);assert_eq!(b[0]["images"][0]["url"],"https://example.com/chart.png");
    let beta_session_key = sessions
        .read_session_work_admission(&beta.id)
        .expect("beta admission")
        .expect("beta admission record")
        .session_key;
    assert_eq!(
        b[0].get("sessionKey").and_then(serde_json::Value::as_str),
        Some(beta_session_key.as_str())
    );
    assert_eq!(sessions.get_agent_conversation_partner_ids(&beta.id).expect("b partners after admission"),vec![alpha.id.clone()]);
    assert!(!persist_agent_inbound_message(&sessions, &beta.id, inbound).expect("duplicate inbound is already admitted"));
    assert_eq!(sessions.read_agent_transcript_entries(&beta.id).expect("deduped beta transcript").len(), 1);
    let mut conflicting_inbound = inbound.clone();
    conflicting_inbound["text"] = serde_json::Value::String("tampered".into());
    assert!(persist_agent_inbound_message(&sessions, &beta.id, &conflicting_inbound).is_err());
    let beta_reply = service
        .send_to_agent_with_session_and_reply(
            &beta.id,
            Some(&beta_session_key),
            &alpha.id,
            "done",
            &[],
            false,
            a[0]["id"].as_str(),
        )
        .expect("reply");
    assert!(beta_reply.contains("Sent to"));
    let reply_wake = wakes.lock().expect("wakes").last().cloned().expect("reply wake");
    let reply_inbound = reply_wake.inbound.as_ref().expect("reply inbound envelope");
    assert_eq!(reply_inbound["replyToId"], a[0]["id"]);
    assert!(persist_agent_inbound_message(&sessions, &alpha.id, reply_inbound).expect("persist reply"));
    let a_after_reply = sessions.read_agent_transcript_entries(&alpha.id).expect("alpha reply transcript");
    assert_eq!(a_after_reply.len(), 2);
    sessions.shutdown();let _=fs::remove_dir_all(root);
}

#[test]
fn direct_delivery_preflights_admitted_target_before_side_effects() {
    let source = include_str!("../src/extensions/transcript/agent_to_agent_messaging.rs");
    assert!(source.contains("self.sessions.assert_session_work_admission(\n                to_agent_id"));
    assert!(source.contains("Some(&target_admission.session_key)"));
    let sender_identity = source
        .split("pub fn send_to_agent_with_session_and_reply(")
        .nth(1)
        .expect("direct send owner exists")
        .split("pub fn persist_agent_inbound_message(")
        .next()
        .expect("direct send owner boundary exists");
    let sender_key = sender_identity
        .find("let sender_session_key =")
        .expect("sender admission key is resolved");
    let source_endpoint = sender_identity
        .find("let source_endpoint =")
        .expect("source communication endpoint exists");
    let planner = sender_identity
        .find("plan_session_communication(")
        .expect("communication planner exists");
    assert!(sender_key < source_endpoint);
    assert!(source_endpoint < planner);
}

#[test]
fn inbound_turn_claim_is_released_when_append_inserts_nothing() {
    let source = include_str!("../src/extensions/transcript/agent_to_agent_messaging.rs");
    let inbound = source
        .split("pub fn persist_agent_inbound_message(")
        .nth(1)
        .expect("inbound owner exists")
        .split("fn now_ms(")
        .next()
        .expect("inbound owner boundary exists");
    let no_insert = inbound
        .split("if inserted == 0 {")
        .nth(1)
        .expect("no-insert branch exists")
        .split("return Ok(false);")
        .next()
        .expect("no-insert branch boundary exists");
    assert!(no_insert.contains("pending_conversation_turns().release"));
    assert!(!no_insert.contains("pending_conversation_turns().complete"));
}

#[test]
fn inbound_turn_completion_precedes_partner_bookkeeping() {
    let source = include_str!("../src/extensions/transcript/agent_to_agent_messaging.rs");
    let inbound = source
        .split("pub fn persist_agent_inbound_message(")
        .nth(1)
        .expect("inbound owner exists")
        .split("fn now_ms(")
        .next()
        .expect("inbound owner boundary exists");
    let completion = inbound
        .find("pending_conversation_turns().complete")
        .expect("successful append completes the pending turn");
    let partner = inbound
        .find("add_agent_conversation_partner")
        .expect("partner bookkeeping exists");
    assert!(completion < partner);
}

#[test]
fn sender_partner_bookkeeping_cannot_block_wake_delivery() {
    let source = include_str!("../src/extensions/transcript/agent_to_agent_messaging.rs");
    let sender = source
        .split("pub fn send_to_agent_with_session_and_reply(")
        .nth(1)
        .expect("direct sender owner exists")
        .split("pub fn persist_agent_inbound_message(")
        .next()
        .expect("direct sender owner boundary exists");
    let partner = sender
        .find("add_agent_conversation_partner")
        .expect("sender partner bookkeeping exists");
    let wake = sender
        .find("(self.wake_sink)(")
        .expect("sender wake dispatch exists");
    assert!(partner < wake);
    assert!(sender.contains(
        "let _ = self\n            .sessions\n            .add_agent_conversation_partner(from_agent_id, to_agent_id);"
    ));
}

#[test]
fn persisted_never_policy_rejects_before_transcript_or_wake(){
    let root=temp_root();let sessions=Arc::new(ProductionSessionWorkers::with_agents_root(&root,500));
    let alpha=sessions.materialize_new_session(Some(&profile("Alpha")),"user",None).expect("alpha");
    let beta=sessions.materialize_new_session(Some(&profile("Beta")),"user",None).expect("beta");
    let admission=sessions.read_session_work_admission(&alpha.id).expect("admission").expect("alpha admission");
    sessions.write_session_communication_policy(
        &alpha.id,&admission.session_id,&admission.lifecycle_revision,
        &SessionCommunicationPolicyOverride{send:Some(SessionCommunicationMode::Never),receive:None},
    ).expect("policy write");
    let wakes=Arc::new(Mutex::new(Vec::<AgentWakeRequest>::new()));
    let service=ProductionAgentToAgentMessaging::new(Arc::clone(&sessions),{
        let wakes=Arc::clone(&wakes);Arc::new(move|wake|wakes.lock().expect("wakes").push(wake.clone()))
    },None);
    let error=service.send_to_agent_with_session(&alpha.id,None,&beta.id,"blocked",&[],false)
        .expect_err("never policy must reject");
    assert!(error.contains("send is disabled"));
    assert!(sessions.read_agent_transcript_entries(&alpha.id).expect("sender").is_empty());
    assert!(wakes.lock().expect("wakes").is_empty());
    sessions.shutdown();let _=fs::remove_dir_all(root);
}

#[test]
fn persisted_ask_policy_rejects_without_approval_consumer(){
    let root=temp_root();let sessions=Arc::new(ProductionSessionWorkers::with_agents_root(&root,500));
    let alpha=sessions.materialize_new_session(Some(&profile("Alpha")),"user",None).expect("alpha");
    let beta=sessions.materialize_new_session(Some(&profile("Beta")),"user",None).expect("beta");
    let admission=sessions.read_session_work_admission(&alpha.id).expect("admission").expect("alpha admission");
    sessions.write_session_communication_policy(
        &alpha.id,&admission.session_id,&admission.lifecycle_revision,
        &SessionCommunicationPolicyOverride{send:Some(SessionCommunicationMode::Ask),receive:None},
    ).expect("policy write");
    let wakes=Arc::new(Mutex::new(Vec::<AgentWakeRequest>::new()));
    let service=ProductionAgentToAgentMessaging::new(Arc::clone(&sessions),{
        let wakes=Arc::clone(&wakes);Arc::new(move|wake|wakes.lock().expect("wakes").push(wake.clone()))
    },None);
    let error=service.send_to_agent_with_session(&alpha.id,None,&beta.id,"approval",&[],false)
        .expect_err("ask policy must fail closed without approval consumer");
    assert!(error.contains("requires approval"));
    assert!(sessions.read_agent_transcript_entries(&alpha.id).expect("sender").is_empty());
    assert!(wakes.lock().expect("wakes").is_empty());
    sessions.shutdown();let _=fs::remove_dir_all(root);
}

#[test]
fn inbound_consumer_binds_source_identity_and_rejects_self_or_unknown(){
    let root=temp_root();let sessions=Arc::new(ProductionSessionWorkers::with_agents_root(&root,500));
    let target=sessions.materialize_new_session(Some(&profile("Target")),"user",None).expect("target");
    let source=sessions.materialize_new_session(Some(&profile("Canonical Source")),"user",None).expect("source");
    let inbound=|source:&str| serde_json::json!({
        "from":{"id":source,"name":"spoofed source name"},
        "text":"must not be accepted",
        "timestampMs":100,
    });
    assert!(!persist_agent_inbound_message(&sessions,&target.id,&inbound(&target.id)).expect("self inbound"));
    assert!(!persist_agent_inbound_message(&sessions,&target.id,&inbound("missing-source")).expect("unknown inbound"));
    let accepted = persist_agent_inbound_message(&sessions,&target.id,&inbound(&source.id)).expect("known inbound");
    assert!(accepted);
    let entries=sessions.read_agent_transcript_entries(&target.id).expect("target transcript");
    assert_eq!(entries[0]["fromAgent"]["name"],"Canonical Source");
    assert_eq!(sessions.get_agent_conversation_partner_ids(&target.id).expect("target partners"),vec![source.id]);
    sessions.shutdown();let _=fs::remove_dir_all(root);
}

#[test]
fn inbound_consumer_rechecks_target_receive_policy_before_persistence(){
    let root=temp_root();let sessions=Arc::new(ProductionSessionWorkers::with_agents_root(&root,500));
    let source=sessions.materialize_new_session(Some(&profile("Source")),"user",None).expect("source");
    let target=sessions.materialize_new_session(Some(&profile("Target")),"user",None).expect("target");
    let admission=sessions.read_session_work_admission(&target.id).expect("admission").expect("target admission");
    sessions.write_session_communication_policy(
        &target.id,&admission.session_id,&admission.lifecycle_revision,
        &SessionCommunicationPolicyOverride{send:None,receive:Some(SessionCommunicationMode::Never)},
    ).expect("policy write");
    let inbound=serde_json::json!({
        "from":{"id":source.id,"name":"spoofed"},
        "text":"blocked by target receive policy",
        "timestampMs":100,
    });
    let error=persist_agent_inbound_message(&sessions,&target.id,&inbound).expect_err("receive policy must reject");
    assert!(error.contains("receive is disabled"));
    assert!(sessions.read_agent_transcript_entries(&target.id).expect("target transcript").is_empty());
    assert!(sessions.get_agent_conversation_partner_ids(&target.id).expect("target partners").is_empty());
    sessions.shutdown();let _=fs::remove_dir_all(root);
}

#[test]
fn inbound_policy_recheck_is_target_admission_scoped() {
    let source = include_str!("../src/extensions/transcript/agent_to_agent_messaging.rs");
    assert!(source.contains("if let Some(source_admission) = source_admission.as_ref()"));
    assert!(source.contains("from_id,\n            Some(&source_admission.session_key)"));
    assert!(source.contains("if source_admission.is_some() || target_admission.is_some()"));
    assert!(source.contains(".map(|admission| admission.session_key.clone())"));
    assert!(source.contains(".unwrap_or_default()"));
    assert!(source.contains("sessions.assert_session_work_admission(\n                agent_id"));
}

#[test]
fn session_bound_sender_append_rejects_an_archived_source_without_writing(){
    let root=temp_root();let sessions=Arc::new(ProductionSessionWorkers::with_agents_root(&root,500));
    let alpha=sessions.materialize_new_session(Some(&profile("Alpha")),"user",None).expect("alpha");
    let beta=sessions.materialize_new_session(Some(&profile("Beta")),"user",None).expect("beta");
    let session_key=format!("agent:{}:main",alpha.id);
    let current=sessions.read_session_work_admission(&alpha.id).expect("admission").expect("main admission");
    sessions.transition_session_work_archive(
        &alpha.id,&session_key,&current.session_id,Some(&current.lifecycle_revision),true,
        Some("human-1"),Some("review"),100.0,
    ).expect("archive source session");
    let service=ProductionAgentToAgentMessaging::new(Arc::clone(&sessions),Arc::new(|_|{}),None);
    let error=service.send_to_agent_with_session(&alpha.id,Some(&session_key),&beta.id,"hello",&[],false)
        .expect_err("archived source must reject the session-bound append");
    assert!(error.contains("is archived"));
    assert!(sessions.read_agent_transcript_entries(&alpha.id).expect("source transcript").is_empty());
    sessions.shutdown();let _=fs::remove_dir_all(root);
}

#[test]
fn group_delivery_delegates_persistence_to_shared_rooms_owner() {
    use mahayana_host_runtime::extensions::transcript::group_chat_glue::GroupChatGlue;

    let root=temp_root();let sessions=Arc::new(ProductionSessionWorkers::with_agents_root(&root,500));
    let alpha=sessions.materialize_new_session(Some(&profile("Alpha")),"user",None).expect("alpha");
    let beta=sessions.materialize_new_session(Some(&profile("Beta")),"user",None).expect("beta");
    let group=GroupChatGlue::new(Arc::clone(&sessions))
        .create_group("Review Room","",&[alpha.id.clone(),beta.id.clone()])
        .expect("group");
    let wakes=Arc::new(Mutex::new(Vec::<AgentWakeRequest>::new()));
    let analytics=Arc::new(Mutex::new(Vec::<(String,String,bool,bool)>::new()));
    let captured=Arc::clone(&analytics);
    let group_sessions=Arc::clone(&sessions);
    let service=ProductionAgentToAgentMessaging::new(Arc::clone(&sessions),{
        let wakes=Arc::clone(&wakes);Arc::new(move|wake|wakes.lock().expect("wakes").push(wake.clone()))
    },None).with_analytics(Arc::new(move|from,to,is_group,priority|{
        captured.lock().expect("analytics").push((from.into(),to.into(),is_group,priority));
    })).with_group_post({
        let analytics=Arc::clone(&analytics);
        Arc::new(move|from,group_id,message,priority|{
            if message == "(pass)" {
                return Ok("Nothing was posted: \"(pass)\" means staying silent in a group chat.".into());
            }
            let posted=mahayana_host_runtime::extensions::transcript::shared_rooms::SharedRooms::new(Arc::clone(&group_sessions))
                .post_local_agent_message(from,group_id,message,123.0)?;
            analytics.lock().expect("analytics").push((from.into(),group_id.into(),true,priority));
            Ok(format!("Posted to \"{}\". Its members will see it and reply on their own turns.",posted.group_name))
        })
    });

    let ack=service.send_to_agent(&alpha.id,&group.agent.id,"group hello",&[],false).expect("group post");
    assert_eq!(ack,"Posted to \"Review Room\". Its members will see it and reply on their own turns.");
    let sender=sessions.read_agent_transcript_entries(&alpha.id).expect("sender");
    assert_eq!(sender.len(),1);
    assert_eq!(sender[0]["toAgent"]["kind"],"group");
    let room=sessions.read_agent_transcript_entries(&group.agent.id).expect("room");
    assert_eq!(room.len(),1);
    assert_eq!(room[0]["kind"],"send-message");
    assert_eq!(room[0]["author"]["id"],alpha.id);
    assert_eq!(room[0]["message"]["content"],"group hello");
    assert_eq!(analytics.lock().expect("analytics").as_slice(),&[(alpha.id.clone(),group.agent.id.clone(),true,false)]);
    assert!(wakes.lock().expect("wakes").is_empty(), "group posting does not emit direct-agent wake requests");

    let before=room.len();
    let pass=service.send_to_agent(&alpha.id,&group.agent.id,"(pass)",&[],false).expect("pass");
    assert_eq!(pass,"Nothing was posted: \"(pass)\" means staying silent in a group chat.");
    assert_eq!(sessions.read_agent_transcript_entries(&group.agent.id).expect("room after pass").len(),before);

    sessions.shutdown();let _=fs::remove_dir_all(root);
}

#[test]
fn direct_peer_ack_preserves_frozen_async_non_waiting_guidance() {
    let root=temp_root();let sessions=Arc::new(ProductionSessionWorkers::with_agents_root(&root,500));
    let alpha=sessions.materialize_new_session(Some(&profile("Alpha")),"user",None).expect("alpha");
    let beta=sessions.materialize_new_session(Some(&profile("Beta")),"user",None).expect("beta");
    let service=ProductionAgentToAgentMessaging::new(Arc::clone(&sessions),Arc::new(|_|{}),None);
    let normal=service.send_to_agent(&alpha.id,&beta.id,"hello",&[],false).expect("normal");
    assert_eq!(
        normal,
        "Sent to Beta. This is asynchronous — if they reply, it'll arrive later as a new message that wakes you; don't wait on it now."
    );
    let priority=service.send_to_agent(&alpha.id,&beta.id,"urgent",&[],true).expect("priority");
    assert_eq!(
        priority,
        "Sent to Beta as a priority message — it will interrupt their current non-user work and wake them now. This is asynchronous — if they reply, it'll arrive later as a new message that wakes you; don't wait on it now."
    );
    sessions.shutdown();let _=fs::remove_dir_all(root);
}

#[test]
fn priority_peer_preemption_never_interrupts_an_active_user_lane() {
    assert!(!should_interrupt_priority_peer(Some(RunLane::User)));
    assert!(should_interrupt_priority_peer(Some(RunLane::Agent)));
    assert!(should_interrupt_priority_peer(Some(RunLane::Background)));
    assert!(should_interrupt_priority_peer(None));
}



#[test]
fn direct_peer_wake_materializes_file_url_images_for_runner_selected_input() {
    let root = temp_root();
    fs::create_dir_all(&root).expect("root");
    let image_path = root.join("peer.png");
    fs::write(&image_path, [1_u8, 2, 3, 4]).expect("image");
    let image_url = url::Url::from_file_path(&image_path)
        .expect("file url")
        .to_string();

    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let alpha = sessions
        .materialize_new_session(Some(&profile("Alpha")), "user", None)
        .expect("alpha");
    let beta = sessions
        .materialize_new_session(Some(&profile("Beta")), "user", None)
        .expect("beta");
    let wakes = Arc::new(Mutex::new(Vec::<AgentWakeRequest>::new()));
    let service = ProductionAgentToAgentMessaging::new(
        Arc::clone(&sessions),
        {
            let wakes = Arc::clone(&wakes);
            Arc::new(move |wake| wakes.lock().expect("wakes").push(wake.clone()))
        },
        None,
    );

    service
        .send_to_agent(
            &alpha.id,
            &beta.id,
            "inspect this",
            &[AgentMessageImage {
                url: image_url,
                alt: Some("peer image".into()),
            }],
            false,
        )
        .expect("send");

    let wake = wakes.lock().expect("wakes").last().cloned().expect("wake");
    assert_eq!(wake.selected_images.len(), 1);
    assert_eq!(
        wake.selected_images[0]["path"],
        image_path.to_string_lossy().as_ref()
    );
    assert_eq!(wake.selected_images[0]["mimeType"], "image/png");
    assert_eq!(wake.selected_images[0]["data"], serde_json::json!([1, 2, 3, 4]));

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn priority_wake_is_enqueued_before_peer_steering_interrupt() {
    let root = temp_root();
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let alpha = sessions
        .materialize_new_session(Some(&profile("Alpha")), "user", None)
        .expect("alpha");
    let beta = sessions
        .materialize_new_session(Some(&profile("Beta")), "user", None)
        .expect("beta");
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

    let service = ProductionAgentToAgentMessaging::new(
        Arc::clone(&sessions),
        {
            let order = Arc::clone(&order);
            Arc::new(move |_| order.lock().expect("order").push("wake"))
        },
        Some({
            let order = Arc::clone(&order);
            Arc::new(move |_, _| {
                order.lock().expect("order").push("interrupt");
                1
            })
        }),
    );

    service
        .send_to_agent(&alpha.id, &beta.id, "urgent", &[], true)
        .expect("send");
    assert_eq!(*order.lock().expect("order"), vec!["wake", "interrupt"]);

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_priority_peer_steering_cancels_direct_and_group_member_runners() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest_dir.join("app/src/main.rs")).expect("shipping main");
    assert!(main.contains("priority_registry.cancel_agent(target_agent_id, reason)"));
    assert!(main.contains(
        "priority_registry.preempt_group_member_agent(target_agent_id, reason)"
    ));
    assert!(main.contains("priority_runtime.is_agent_running(target_agent_id)"));
    assert!(main.contains("reason: \"agent_steer\".into()"));
    assert!(main.contains("priority_telemetry.report_turn_interrupt(&TurnInterruptFields"));
}


#[test]
fn direct_delivery_records_frozen_product_analytics_fields() {
    let root = temp_root();
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let alpha = sessions
        .materialize_new_session(Some(&profile("Alpha")), "user", None)
        .expect("alpha");
    let beta = sessions
        .materialize_new_session(Some(&profile("Beta")), "user", None)
        .expect("beta");
    let events = Arc::new(Mutex::new(Vec::<(String, String, bool, bool)>::new()));
    let captured = Arc::clone(&events);
    let service = ProductionAgentToAgentMessaging::new(
        Arc::clone(&sessions),
        Arc::new(|_| {}),
        None,
    )
    .with_analytics(Arc::new(move |from, to, is_group_target, priority| {
        captured
            .lock()
            .expect("analytics")
            .push((from.to_string(), to.to_string(), is_group_target, priority));
    }));

    service
        .send_to_agent(&alpha.id, &beta.id, "hello", &[], true)
        .expect("send");
    assert_eq!(
        *events.lock().expect("analytics"),
        vec![(alpha.id.clone(), beta.id.clone(), false, true)]
    );

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_agent_message_analytics_uses_canonical_product_analytics_owner() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest_dir.join("app/src/main.rs")).expect("shipping main");
    assert!(main.contains(".with_analytics(Arc::new(move |from_agent_id, to_agent_id, is_group_target, is_priority|"));
    assert!(main.contains("\"sand.agent_message.sent\""));
    assert!(main.contains("\"from_agent_id\": from_agent_id"));
    assert!(main.contains("\"to_agent_id\": to_agent_id"));
    assert!(main.contains("\"is_group_target\": is_group_target"));
    assert!(main.contains("\"is_priority\": is_priority"));
}


#[test]
fn agent_inbound_failure_projects_frozen_telemetry_and_tray() {
    let report = agent_inbound_failure_report(
        "agent-b",
        Some("request-1"),
        "INFERENCE_PROVIDER_FAILED",
        "provider exploded",
    );
    assert_eq!(report.source, "agent");
    assert_eq!(report.conversation_id, "agent-b");
    assert_eq!(report.request_id.as_deref(), Some("request-1"));
    assert_eq!(report.error.code, "SAND-E0406");
    assert_eq!(
        report.detail.as_ref().map(|detail| detail.message.as_str()),
        Some("provider exploded")
    );
    let telemetry = agent_error_telemetry(&report);
    assert_eq!(telemetry.metadata["error_code"], "SAND-E0406");
    assert_eq!(telemetry.metadata["connect_code"], "INFERENCE_PROVIDER_FAILED");

    let tray = agent_inbound_failure_tray(
        "agent-b",
        Some("request-1"),
        "INFERENCE_PROVIDER_FAILED",
        "provider exploded",
    );
    assert_eq!(tray.agent_id.as_deref(), Some("agent-b"));
    assert_eq!(tray.title, "Message from another agent failed");
    assert_eq!(tray.detail, "provider exploded");
    assert_eq!(tray.request_id.as_deref(), Some("request-1"));
    assert_eq!(tray.error_kind.as_deref(), Some("INFERENCE_PROVIDER_FAILED"));
}

#[test]
fn shipping_host_owns_agent_inbound_failure_telemetry_and_tray() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest_dir.join("app/src/main.rs")).expect("shipping main");
    assert!(main.contains("method == \"reportAgentInboundFailure\""));
    assert!(main.contains("self.telemetry_logs.report_agent_error(&agent_inbound_failure_report("));
    assert!(main.contains("self.trays.push_error(agent_inbound_failure_tray("));
}


#[test]
fn deletion_fenced_target_uses_frozen_agent_gone_response() {
    let root = temp_root();
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let alpha = sessions
        .materialize_new_session(Some(&profile("Alpha")), "user", None)
        .expect("alpha");
    let beta = sessions
        .materialize_new_session(Some(&profile("Beta")), "user", None)
        .expect("beta");
    sessions
        .begin_agent_delete(&beta.id)
        .expect("publish deletion fence");

    let service = ProductionAgentToAgentMessaging::new(
        Arc::clone(&sessions),
        Arc::new(|_| panic!("deleting target must not wake")),
        Some(Arc::new(|_, _| panic!("deleting target must not interrupt"))),
    );
    let ack = service
        .send_to_agent(&alpha.id, &beta.id, "hello", &[], false)
        .expect("send result");
    assert_eq!(ack, "That agent no longer exists.");

    sessions
        .end_agent_delete(&beta.id)
        .expect("clear deletion fence");
    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}
