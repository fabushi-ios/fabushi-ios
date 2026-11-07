use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::webauthn_proxy::webauthn_proxy_marker::{
    apply_web_authn_proxy_marker_at, BOX_CHROME_POLICY_COMMAND, SAND_WEBAUTHN_PROXY_MARKER_PATH,
};
use mahayana_host_runtime::ports::product_analytics::{
    SandProductAnalytics, create_noop_sand_product_analytics,
};
use mahayana_host_runtime::ports::sand_analytics_types::{
    SandMessageLengthBucket, sand_message_length_bucket,
};
use mahayana_host_runtime::selected_image_inputs::{
    image_mime_from_path, load_selected_image_inputs,
};
use mahayana_host_runtime::transcript_mutation_events::{
    publish_transcript_mutation, subscribe_transcript_mutations,
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

#[test]
fn product_analytics_noop_and_length_buckets_match_frozen_grok_contract() {
    let analytics = create_noop_sand_product_analytics();
    let properties = BTreeMap::from([("value".into(), json!(7))]);
    analytics.track_event("event", Some(&properties));

    let cases = [
        (-1.0, SandMessageLengthBucket::Empty, "empty"),
        (0.0, SandMessageLengthBucket::Empty, "empty"),
        (0.1, SandMessageLengthBucket::Xs, "xs"),
        (19.999, SandMessageLengthBucket::Xs, "xs"),
        (20.0, SandMessageLengthBucket::S, "s"),
        (99.999, SandMessageLengthBucket::S, "s"),
        (100.0, SandMessageLengthBucket::M, "m"),
        (499.999, SandMessageLengthBucket::M, "m"),
        (500.0, SandMessageLengthBucket::L, "l"),
        (1_999.999, SandMessageLengthBucket::L, "l"),
        (2_000.0, SandMessageLengthBucket::Xl, "xl"),
        (f64::INFINITY, SandMessageLengthBucket::Xl, "xl"),
        (f64::NAN, SandMessageLengthBucket::Xl, "xl"),
    ];
    for (value, expected, wire) in cases {
        let actual = sand_message_length_bucket(value);
        assert_eq!(actual, expected);
        assert_eq!(actual.as_str(), wire);
    }
}

#[test]
fn transcript_mutation_subscriptions_publish_and_unsubscribe() {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&seen);
    let subscription = subscribe_transcript_mutations(move |mutation| {
        if let Some(kind) = mutation.get("kind").and_then(Value::as_str) {
            sink.lock().expect("mutation sink").push(kind.to_string());
        }
    });

    let mut first = Map::new();
    first.insert("kind".into(), json!("entry-updated"));
    first.insert("id".into(), json!("entry-1"));
    publish_transcript_mutation(&first);
    assert_eq!(
        seen.lock().expect("mutation values").as_slice(),
        &["entry-updated".to_string()]
    );

    subscription.unsubscribe();
    let mut second = Map::new();
    second.insert("kind".into(), json!("ignored"));
    publish_transcript_mutation(&second);
    assert_eq!(seen.lock().expect("mutation values").len(), 1);
}

#[test]
fn selected_image_inputs_preserve_order_mime_and_read_failure_filtering() {
    let root = std::env::temp_dir().join(format!("fabushi-selected-images-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).expect("create selected image test root");
    let png = root.join("A.PNG");
    let webp = root.join("photo.webp");
    let missing = root.join("missing.jpg");
    fs::write(&png, [1_u8, 2, 3]).expect("write png");
    fs::write(&webp, [4_u8, 5]).expect("write webp");

    let loaded = load_selected_image_inputs([
        png.to_string_lossy().to_string(),
        missing.to_string_lossy().to_string(),
        webp.to_string_lossy().to_string(),
    ]);
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].data, vec![1, 2, 3]);
    assert_eq!(loaded[0].mime_type, Some("image/png"));
    assert_eq!(loaded[1].data, vec![4, 5]);
    assert_eq!(loaded[1].mime_type, Some("image/webp"));

    assert_eq!(image_mime_from_path(r"C:\\images\\photo.JPEG"), Some("image/jpeg"));
    assert_eq!(image_mime_from_path("/tmp/.hidden"), None);
    assert_eq!(image_mime_from_path("photo.heic"), None);

    fs::remove_dir_all(root).expect("remove selected image test root");
}

#[test]
fn webauthn_proxy_marker_matches_not_box_unchanged_and_apply_contract() {
    assert_eq!(SAND_WEBAUTHN_PROXY_MARKER_PATH, "/home/box/.sand-webauthn-proxy-enabled");
    assert_eq!(BOX_CHROME_POLICY_COMMAND, "/usr/local/bin/box-chrome-policy");

    let root = std::env::temp_dir().join(format!("fabushi-webauthn-marker-{}", Uuid::new_v4()));
    let missing_parent_marker = root.join("missing").join("marker");
    assert_eq!(
        apply_web_authn_proxy_marker_at(true, &missing_parent_marker, root.join("policy"))
            .expect("missing parent outcome"),
        "not-a-box"
    );

    let box_root = root.join("box");
    fs::create_dir_all(&box_root).expect("create box marker root");
    let marker = box_root.join("marker");
    let policy = root.join("missing-policy");
    assert_eq!(
        apply_web_authn_proxy_marker_at(true, &marker, &policy).expect("enable marker"),
        "applied"
    );
    assert!(marker.exists());
    assert_eq!(
        apply_web_authn_proxy_marker_at(true, &marker, &policy).expect("unchanged marker"),
        "unchanged"
    );
    assert_eq!(
        apply_web_authn_proxy_marker_at(false, &marker, &policy).expect("disable marker"),
        "applied"
    );
    assert!(!marker.exists());

    fs::remove_dir_all(root).expect("remove WebAuthn marker test root");
}
