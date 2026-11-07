use mahayana_host_runtime::extensions::transcript::client_side_tool_v2_producer::{
    CLIENT_SIDE_TOOL_V2_ACCOUNT_SLOT, CLIENT_SIDE_TOOL_V2_WIRE_VERSION,
    ClientSideToolV2ProducedValue, ClientSideToolV2Producer, ClientSideToolV2TransportKind,
};

#[test]
fn producer_preserves_open_call_fencing_and_per_agent_sequence() {
    let mut producer = ClientSideToolV2Producer::with_epoch("epoch-1");
    assert!(producer.publish("", ClientSideToolV2ProducedValue::call("c", [1u8])).is_none());
    assert!(producer.publish("agent-a", ClientSideToolV2ProducedValue::call("", [1u8])).is_none());
    assert!(producer.publish("agent-a", ClientSideToolV2ProducedValue::result("missing", [9u8])).is_none());

    let call = producer
        .publish("agent-a", ClientSideToolV2ProducedValue::call("call-1", [1u8, 2, 3]))
        .expect("call event");
    assert_eq!(call.version, CLIENT_SIDE_TOOL_V2_WIRE_VERSION);
    assert_eq!(call.account_slot, CLIENT_SIDE_TOOL_V2_ACCOUNT_SLOT);
    assert_eq!(call.epoch, "epoch-1");
    assert_eq!(call.sequence, 1);
    assert_eq!(call.kind, ClientSideToolV2TransportKind::Call);
    let message = call.message.expect("call message");
    assert_eq!(message.encoding, "protobuf-base64");
    assert_eq!(message.message_type, "aiserver.v1.ClientSideToolV2Call");
    assert_eq!(message.bytes, "AQID");

    assert!(producer
        .publish("agent-b", ClientSideToolV2ProducedValue::result("call-1", [4u8]))
        .is_none());

    let result = producer
        .publish("agent-a", ClientSideToolV2ProducedValue::result("call-1", [4u8, 5]))
        .expect("result event");
    assert_eq!(result.sequence, 2);
    assert_eq!(result.kind, ClientSideToolV2TransportKind::Result);
    assert_eq!(
        result.message.expect("result message").message_type,
        "aiserver.v1.ClientSideToolV2Result"
    );
    assert!(producer
        .publish("agent-a", ClientSideToolV2ProducedValue::result("call-1", [6u8]))
        .is_none());

    let other = producer
        .publish("agent-b", ClientSideToolV2ProducedValue::call("b-1", [7u8]))
        .expect("other agent");
    assert_eq!(other.sequence, 1);
}

#[test]
fn reset_clears_open_calls_and_keeps_monotonic_epoch_sequence() {
    let mut producer = ClientSideToolV2Producer::with_epoch("stable");
    producer
        .publish("agent", ClientSideToolV2ProducedValue::call("call", [1u8]))
        .expect("call");
    let reset = producer.reset("agent").expect("reset");
    assert_eq!(reset.sequence, 2);
    assert_eq!(reset.epoch, "stable");
    assert_eq!(reset.kind, ClientSideToolV2TransportKind::Reset);
    assert!(reset.message.is_none());
    assert!(producer
        .publish("agent", ClientSideToolV2ProducedValue::result("call", [2u8]))
        .is_none());
    let next = producer
        .publish("agent", ClientSideToolV2ProducedValue::call("next", [3u8]))
        .expect("next call");
    assert_eq!(next.sequence, 3);
    assert!(producer.reset("").is_none());
}

#[test]
fn serialized_event_matches_frozen_transport_field_names() {
    let mut producer = ClientSideToolV2Producer::with_epoch("e");
    let event = producer
        .publish("a", ClientSideToolV2ProducedValue::call("c", [0u8]))
        .expect("event");
    let value = serde_json::to_value(event).expect("json");
    assert_eq!(value["version"], 1);
    assert_eq!(value["kind"], "call");
    assert_eq!(value["accountSlot"], "host");
    assert_eq!(value["agentId"], "a");
    assert_eq!(value["epoch"], "e");
    assert_eq!(value["sequence"], 1);
    assert_eq!(value["message"]["encoding"], "protobuf-base64");
    assert_eq!(value["message"]["messageType"], "aiserver.v1.ClientSideToolV2Call");
    assert_eq!(value["message"]["bytes"], "AA==");
}
