use mahayana_node_agent_coordinator::webauthn::WebAuthnCeremony;
use serde_json::json;

#[test]
fn ceremony_preserves_frozen_grok_top_level_wire_shape() {
    let frozen = json!({
        "kind": "get",
        "origin": "https://example.test",
        "optionsJson": "{\"challenge\":\"abc\"}",
        "rpId": "example.test",
        "futureField": { "nested": true }
    });
    let ceremony: WebAuthnCeremony =
        serde_json::from_value(frozen.clone()).expect("parse frozen ceremony");
    assert_eq!(ceremony.kind, "get");
    assert_eq!(ceremony.origin, "https://example.test");
    assert_eq!(ceremony.payload["rpId"], "example.test");
    assert_eq!(ceremony.payload["futureField"]["nested"], true);

    let encoded = serde_json::to_value(&ceremony).expect("serialize frozen ceremony");
    assert_eq!(encoded, frozen);
    assert!(encoded.get("payload").is_none(), "wire format must stay flat");
}

#[test]
fn legacy_nested_payload_is_flattened_on_write() {
    let legacy = json!({
        "kind": "create",
        "origin": "https://login.example.test",
        "payload": {
            "optionsJson": "{}",
            "rpId": "example.test"
        }
    });
    let ceremony: WebAuthnCeremony =
        serde_json::from_value(legacy).expect("parse legacy ceremony");
    let encoded = serde_json::to_value(ceremony).expect("serialize ceremony");
    assert_eq!(encoded["kind"], "create");
    assert_eq!(encoded["origin"], "https://login.example.test");
    assert_eq!(encoded["optionsJson"], "{}");
    assert_eq!(encoded["rpId"], "example.test");
    assert!(encoded.get("payload").is_none());
}

#[test]
fn ceremony_authoritative_kind_and_origin_cannot_be_shadowed_by_payload() {
    let ceremony = WebAuthnCeremony {
        kind: "create".into(),
        origin: "https://login.example.test".into(),
        payload: json!({
            "kind": "wrong",
            "origin": "https://attacker.invalid",
            "optionsJson": "{}"
        }),
    };
    let encoded = serde_json::to_value(ceremony).expect("serialize ceremony");
    assert_eq!(encoded["kind"], "create");
    assert_eq!(encoded["origin"], "https://login.example.test");
    assert_eq!(encoded["optionsJson"], "{}");
}
