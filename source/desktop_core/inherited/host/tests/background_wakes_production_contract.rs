use std::sync::{Arc, Mutex};

use mahayana_host_runtime::connectors::channel_delivery::{
    ChannelDeliveryPort, ChannelOutboundMessage, HostChannelDelivery,
};
use mahayana_host_runtime::extensions::transcript::background_wakes::{
    BackgroundWakes, CHANNEL_DELIVERY_FAILED_WAKE_CUE,
    build_channel_delivery_failure_wake_prompt, build_channel_outbound_message,
    humanize_channel_delivery_failure,
};
use mahayana_host_runtime::extensions::transcript::send_message_shaping::collect_inbound_images;
use serde_json::json;

#[derive(Default)]
struct RecordingPort {
    deliveries: Mutex<Vec<(String, String, ChannelOutboundMessage)>>,
}

impl ChannelDeliveryPort for RecordingPort {
    fn deliver(
        &self,
        agent_id: &str,
        address_token: &str,
        message: &ChannelOutboundMessage,
    ) -> Result<(), String> {
        self.deliveries.lock().expect("deliveries").push((
            agent_id.to_string(),
            address_token.to_string(),
            message.clone(),
        ));
        Ok(())
    }
}

#[test]
fn channel_outbound_projection_matches_frozen_text_attachment_and_widget_fences() {
    assert_eq!(
        build_channel_outbound_message(&json!({
            "type":"text",
            "content":"caption",
            "deliveryOperationId":"transcript-entry-1",
            "images":[
                {"url":"https://example.test/first.png"},
                {"url":"https://example.test/second.png"}
            ]
        })),
        Some(ChannelOutboundMessage::Attachment {
            url: "https://example.test/first.png".into(),
            caption: Some("caption".into()),
            operation_id: Some("transcript-entry-1".into()),
        })
    );
    assert_eq!(
        build_channel_outbound_message(&json!({
            "type":"attachment",
            "url":"file:///workspace/report.pdf",
            "alt":"report"
        })),
        Some(ChannelOutboundMessage::Attachment {
            url: "file:///workspace/report.pdf".into(),
            caption: Some("report".into()),
            operation_id: None,
        })
    );
    assert_eq!(
        build_channel_outbound_message(&json!({
            "type":"attachment",
            "url":"",
            "alt":"fallback text"
        })),
        Some(ChannelOutboundMessage::Text {
            text: "fallback text".into(),
            operation_id: None,
        })
    );
    assert!(build_channel_outbound_message(&json!({
        "type":"widget",
        "channel":"slack:C1",
        "widget":{"prompt":"Choose","options":[]}
    }))
    .is_none());
}

#[test]
fn host_channel_delivery_defaults_fail_closed_and_allows_one_explicit_provider_port() {
    let delivery = HostChannelDelivery::default();
    assert_eq!(
        delivery
            .deliver(
                "agent-a",
                "slack:C1",
                &ChannelOutboundMessage::Text { text: "hello".into(), operation_id: None },
            )
            .expect_err("unregistered delivery"),
        "No channel delivery mechanism is registered."
    );

    let port = Arc::new(RecordingPort::default());
    delivery.set_port(port.clone());
    delivery
        .deliver(
            "agent-a",
            "slack:C1",
            &ChannelOutboundMessage::Text { text: "hello".into(), operation_id: None },
        )
        .expect("registered delivery");
    let deliveries = port.deliveries.lock().expect("deliveries");
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].0, "agent-a");
    assert_eq!(deliveries[0].1, "slack:C1");
}

#[test]
fn delivery_failure_humanization_and_hidden_revival_prompt_match_frozen_contract() {
    assert_eq!(
        humanize_channel_delivery_failure(
            "slack:C1",
            "No channel delivery mechanism is registered."
        ),
        "Channel messaging isn't available on this computer, so that message wasn't delivered."
    );
    assert_eq!(
        humanize_channel_delivery_failure("invalid", "not a valid channel address"),
        "\"invalid\" isn't a valid channel address, so that message wasn't delivered."
    );
    let prompt = build_channel_delivery_failure_wake_prompt(&[json!({
        "addressToken":"slack:C1",
        "reason":"Slack isn't connected"
    })]);
    assert!(prompt.starts_with(CHANNEL_DELIVERY_FAILED_WAKE_CUE));
    assert!(prompt.contains("- To slack:C1: Slack isn't connected"));
    assert!(prompt.contains("SendMessage with no channel target"));
    assert!(prompt.contains("Don't silently retry the same channel"));
}

#[test]
fn channel_failure_queue_is_agent_scoped_and_revival_fenced() {
    let mut wakes = BackgroundWakes::<serde_json::Value>::default();
    BackgroundWakes::enqueue(
        &mut wakes.pending_channel_failures,
        "agent-a",
        json!({"addressToken":"slack:C1","reason":"failed"}),
    );
    assert!(BackgroundWakes::<serde_json::Value>::begin_revival(
        &mut wakes.reviving_channel_failure_agent_ids,
        "agent-a",
    ));
    assert!(!BackgroundWakes::<serde_json::Value>::begin_revival(
        &mut wakes.reviving_channel_failure_agent_ids,
        "agent-a",
    ));
    let failures =
        BackgroundWakes::take_pending(&mut wakes.pending_channel_failures, "agent-a");
    assert_eq!(failures.len(), 1);
    BackgroundWakes::<serde_json::Value>::end_revival(
        &mut wakes.reviving_channel_failure_agent_ids,
        "agent-a",
    );
}

#[test]
fn inbound_image_projection_preserves_all_frozen_image_payloads() {
    let images = collect_inbound_images(&[
        json!({"images":[{"data":"aGVsbG8=","mimeType":"image/png"}]}),
        json!({"images":[{"data":"d29ybGQ=","mimeType":"image/webp"}]}),
    ]);
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].data, "aGVsbG8=");
    assert_eq!(images[0].mime_type, "image/png");
    assert_eq!(images[1].data, "d29ybGQ=");
}

#[test]
fn shipping_send_message_reaches_host_owned_delivery_failure_broadcast_and_context_paths() {
    const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");

    let send_sink = SHIPPING_HOST
        .find("struct ProductionSendMessageSink")
        .expect("shipping SendMessage sink");
    let dispatch = SHIPPING_HOST[send_sink..]
        .find("\"deliverToChannel\"")
        .map(|offset| send_sink + offset)
        .expect("channel delivery dispatch");
    let gateway = SHIPPING_HOST[dispatch..]
        .find("if method == \"deliverToChannel\"")
        .map(|offset| dispatch + offset)
        .expect("Host channel owner");
    assert!(send_sink < dispatch && dispatch < gateway);
    let persisted_entry = SHIPPING_HOST[send_sink..]
        .find("append_generated_send_message_entry(")
        .map(|offset| send_sink + offset)
        .expect("transcript entry persistence");
    assert!(persisted_entry < dispatch, "delivery dispatch must follow transcript persistence");
    assert!(SHIPPING_HOST.contains("deliveryOperationId"));
    assert!(SHIPPING_HOST.contains("delivery_operation_id: Option<String>"));
    assert!(SHIPPING_HOST.contains(
        "for cleanup_error in custody.rollback()"
    ));
    assert!(SHIPPING_HOST.contains(
        "outgoing_media_cleanup_failed agent={} tool={} error={cleanup_error}"
    ));

    assert!(SHIPPING_HOST.contains("queue_channel_delivery_failure("));
    assert!(SHIPPING_HOST.contains("\"markedAtMs\": marked_at_ms"));
    assert!(SHIPPING_HOST.contains("fn channel_failure_marker_from_failure("));
    assert!(SHIPPING_HOST.contains("run_channel_failure_revival_worker"));
    assert!(SHIPPING_HOST.contains("clear_durable_channel_failure_markers"));
    assert!(SHIPPING_HOST.contains("store.clear_if_matches(&marker)"));
    assert!(SHIPPING_HOST.contains("channel_failure_marker_identity_incomplete"));
    assert!(SHIPPING_HOST.contains(
        "Some(_) => {\n                stale.push(failure.clone());\n                false\n            }\n            None => true"
    ));
    assert!(SHIPPING_HOST.contains(
        "return Err(\"channel failure session provenance is unavailable\".into());"
    ));
    assert!(SHIPPING_HOST.contains(
        "Ok(None) => {\n                clear_durable_channel_failure_markers(&deps, &agent_id, &failures);\n                continue;\n            }"
    ));
    assert!(SHIPPING_HOST.contains(
        "!execution.aborted\n                    && !execution.quiesced_for_upgrade\n                    && execution.sent_message_count > 0"
    ));
    assert!(SHIPPING_HOST.contains("requeue_channel_failures(&wakes, &agent_id, failures)"));
    assert!(SHIPPING_HOST.contains("tray.title = \"Delivery-failure follow-up failed\".into();\n                deps.trays.push_error(tray);\n                requeue_channel_failures(&wakes, &agent_id, failures);\n                break;"));
    assert!(SHIPPING_HOST.contains("tray.title = \"Message not delivered\""));
    assert!(SHIPPING_HOST.contains("tray.title = \"Delivery-failure follow-up failed\""));
    assert!(SHIPPING_HOST.contains("summary.is_group || summary.remote_room.is_some()"));
    assert!(SHIPPING_HOST.contains("collect_inbound_images(&envelopes)"));
    assert!(SHIPPING_HOST.contains("collect_unanswered_question_prompts(&agent_id)"));
    assert!(SHIPPING_HOST.contains("\"selectedImages\""));
    assert!(SHIPPING_HOST.contains("\"skippedQuestionPrompts\""));
    assert!(SHIPPING_HOST.contains("\"dismissedQuestionPrompts\""));
    assert!(SHIPPING_HOST.contains("if method == \"broadcastToAgents\""));
    assert!(SHIPPING_HOST.contains("build_admin_broadcast_wake_prompt(&text)"));
    assert!(SHIPPING_HOST.contains("REPLY_NUDGE_PROMPT"));
    assert!(SHIPPING_HOST.contains("\"channel-connected\""));
    assert!(SHIPPING_HOST.contains("\"channel-disconnected\""));
    assert!(SHIPPING_HOST.contains("\"automation-changed\""));
}
