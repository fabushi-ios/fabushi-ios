use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::automation_runtime::AutomationRuntime;
use mahayana_host_runtime::extensions::transcript::production_runtime::ProductionTranscriptRuntime;
use mahayana_host_runtime::extensions::transcript::roster_emit::ProductionRosterEmit;
use mahayana_host_runtime::extensions::transcript::widget_responses::WidgetResponses;
use serde_json::{Value, json};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-widget-full-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn setup(label: &str) -> (
    std::path::PathBuf,
    Arc<ProductionSessionWorkers>,
    Arc<ProductionTranscriptRuntime>,
    WidgetResponses,
    Arc<Mutex<Vec<Value>>>,
    String,
) {
    let root = temp_root(label);
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let record = workers
        .materialize_new_session(None, "user", None)
        .expect("session");
    let runtime = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    runtime
        .switch_agent(&workers, &record.id, 10.0)
        .expect("switch active");
    let automation = Arc::new(AutomationRuntime::new(Arc::clone(&workers)));
    let widgets = WidgetResponses::with_runtime(
        Arc::clone(&workers),
        Arc::clone(&automation),
        Arc::clone(&runtime),
    );
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink_events = Arc::clone(&events);
    let roster = Arc::new(ProductionRosterEmit::new(
        Arc::clone(&workers),
        Arc::clone(&runtime),
        Arc::new(move |event| sink_events.lock().expect("events").push(event)),
    ));
    widgets.bind_roster(roster).expect("bind roster");
    (root, workers, runtime, widgets, events, record.id)
}

#[test]
fn generic_widget_answer_and_dismissal_are_durable_and_project_updated_entries() {
    let (root, workers, runtime, widgets, events, agent_id) = setup("generic");
    workers
        .append_agent_transcript_entries(
            &agent_id,
            &[
                json!({
                    "id":"question",
                    "kind":"send-message",
                    "replyTo":"parent",
                    "message":{"type":"widget","widget":{"prompt":"Pick","options":[]}}
                }),
                json!({
                    "id":"dismiss",
                    "kind":"send-message",
                    "widgetSkipped":true,
                    "message":{"type":"widget","widget":{"prompt":"Later","options":[]}}
                }),
            ],
        )
        .expect("append");

    let sent = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sent_capture = Arc::clone(&sent);
    assert!(widgets
        .respond_to_widget_with("question", "  yes  ", &agent_id, 20.0, move |args| {
            sent_capture.lock().expect("sent").push(args);
            Ok(())
        })
        .expect("answer"));
    assert_eq!(sent.lock().expect("sent")[0]["prompt"], "yes");
    assert_eq!(sent.lock().expect("sent")[0]["replyToId"], "parent");
    assert_eq!(sent.lock().expect("sent")[0]["appendUserMessage"], false);
    assert_eq!(sent.lock().expect("sent")[0]["awaitTurn"], false);

    assert!(widgets
        .dismiss_widget("dismiss", &agent_id)
        .expect("dismiss"));
    let entries = workers
        .read_agent_transcript_entries(&agent_id)
        .expect("read");
    let answered = entries.iter().find(|entry| entry["id"] == "question").unwrap();
    let dismissed = entries.iter().find(|entry| entry["id"] == "dismiss").unwrap();
    assert_eq!(answered["respondedValue"], "yes");
    assert_eq!(dismissed["widgetDismissed"], true);
    assert!(dismissed.get("widgetSkipped").is_none());

    let projected = events.lock().expect("events");
    assert!(projected.iter().any(|event| {
        event["channel"] == "transcript"
            && event["payload"]["type"] == "updated"
            && event["payload"]["entry"]["id"] == "question"
    }));
    assert!(projected.iter().any(|event| {
        event["channel"] == "transcript"
            && event["payload"]["type"] == "updated"
            && event["payload"]["entry"]["id"] == "dismiss"
    }));

    runtime.dispose();
    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_generic_send_rolls_back_but_spend_guard_rejection_never_leaves_a_response_stamp() {
    let (root, workers, runtime, widgets, _events, agent_id) = setup("rollback");
    workers
        .append_agent_transcript_entries(
            &agent_id,
            &[
                json!({
                    "id":"normal",
                    "kind":"send-message",
                    "message":{"type":"widget","widget":{"prompt":"Normal","options":[]}}
                }),
                json!({
                    "id":"guard",
                    "kind":"send-message",
                    "message":{"type":"widget","widget":{"prompt":"Guard","options":[]}}
                }),
            ],
        )
        .expect("append");

    let error = widgets
        .respond_to_widget_with("normal", "answer", &agent_id, 30.0, |_| Err("send failed".into()))
        .expect_err("send failure");
    assert_eq!(error, "send failed");

    let accepted = widgets
        .respond_to_widget_with("guard", "spend-guard:not-a-real-choice", &agent_id, 30.0, |_| {
            panic!("rejected spend guard must not dispatch")
        })
        .expect("guard");
    assert!(!accepted);

    let entries = workers
        .read_agent_transcript_entries(&agent_id)
        .expect("read");
    assert!(entries
        .iter()
        .filter(|entry| entry["id"] == "normal" || entry["id"] == "guard")
        .all(|entry| entry.get("respondedValue").is_none()));

    runtime.dispose();
    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn auto_review_secret_and_reaction_delegate_to_existing_session_and_projection_owners() {
    let (root, workers, runtime, widgets, _events, agent_id) = setup("domains");
    workers
        .append_agent_transcript_entries(
            &agent_id,
            &[
                json!({
                    "id":"review",
                    "kind":"send-message",
                    "message":{
                        "type":"auto-review-approval",
                        "approval":{"requestId":"review-1","status":"pending"}
                    }
                }),
                json!({
                    "id":"secret",
                    "kind":"send-message",
                    "message":{
                        "type":"secret-request",
                        "secretRequest":{
                            "label":"Slack token",
                            "target":{"kind":"channel-credential","platform":"slack","field":"token"}
                        }
                    }
                }),
                json!({
                    "id":"assistant",
                    "kind":"message",
                    "role":"assistant",
                    "content":"Done with the task."
                }),
            ],
        )
        .expect("append");

    assert!(widgets
        .settle_stale_auto_review_card(&agent_id, "review", "review-1")
        .expect("expire review"));
    let review = workers
        .read_agent_transcript_entries(&agent_id)
        .expect("read review")
        .into_iter()
        .find(|entry| entry["id"] == "review")
        .unwrap();
    assert_eq!(review["message"]["approval"]["status"], "expired");

    let ack = widgets
        .submit_secret("secret", "  top-secret  ", &agent_id)
        .expect("secret")
        .expect("ack");
    assert!(ack.contains("Slack token"));
    assert_eq!(
        workers
            .get_connector_secret(&agent_id, "slack", "token")
            .expect("read secret")
            .as_deref(),
        Some("top-secret")
    );
    let secret = workers
        .read_agent_transcript_entries(&agent_id)
        .expect("read secret card")
        .into_iter()
        .find(|entry| entry["id"] == "secret")
        .unwrap();
    assert_eq!(secret["secretProvided"], true);

    let resume = widgets
        .react_to_message("assistant", " 👍 ", &agent_id)
        .expect("react")
        .expect("resume prompt");
    assert!(resume.contains("Done with the task."));
    let reacted = workers
        .read_agent_transcript_entries(&agent_id)
        .expect("read reacted")
        .into_iter()
        .find(|entry| entry["id"] == "assistant")
        .unwrap();
    assert_eq!(reacted["reactions"][0]["emoji"], "👍");
    assert_eq!(reacted["reactions"][0]["by"], "me");
    assert!(widgets
        .react_to_message("assistant", "👍", &agent_id)
        .expect("unreact")
        .is_none());
    let unreacted = workers
        .read_agent_transcript_entries(&agent_id)
        .expect("read unreacted")
        .into_iter()
        .find(|entry| entry["id"] == "assistant")
        .unwrap();
    assert!(unreacted.get("reactions").is_none());

    runtime.dispose();
    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn dismiss_on_move_on_blocks_stale_main_surface_response_and_secret_emits_channel_config_change() {
    let (root, workers, runtime, widgets, _events, agent_id) = setup("move-on");
    workers
        .append_agent_transcript_entries(
            &agent_id,
            &[
                json!({
                    "id":"move-on",
                    "kind":"send-message",
                    "message":{
                        "type":"widget",
                        "widget":{"prompt":"Old question","options":[],"dismissOnMoveOn":true}
                    }
                }),
                json!({
                    "id":"later-user",
                    "kind":"message",
                    "role":"user",
                    "content":"I moved on."
                }),
                json!({
                    "id":"secret-signal",
                    "kind":"send-message",
                    "message":{
                        "type":"secret-request",
                        "secretRequest":{
                            "label":"Discord token",
                            "target":{"kind":"channel-credential","platform":"discord","field":"token"}
                        }
                    }
                }),
            ],
        )
        .expect("append");

    assert!(!widgets
        .respond_to_widget_with("move-on", "late answer", &agent_id, 40.0, |_| {
            panic!("stale dismissOnMoveOn widget must not dispatch")
        })
        .expect("move-on"));
    let move_on = workers
        .read_agent_transcript_entries(&agent_id)
        .expect("read")
        .into_iter()
        .find(|entry| entry["id"] == "move-on")
        .unwrap();
    assert!(move_on.get("respondedValue").is_none());

    let changed = Arc::new(Mutex::new(0usize));
    let changed_capture = Arc::clone(&changed);
    widgets
        .bind_channel_config_changed(Arc::new(move || {
            *changed_capture.lock().expect("changed") += 1;
        }))
        .expect("bind channel config");
    assert!(widgets
        .submit_secret("secret-signal", "discord-secret", &agent_id)
        .expect("secret")
        .is_some());
    assert_eq!(*changed.lock().expect("changed"), 1);

    runtime.dispose();
    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}
