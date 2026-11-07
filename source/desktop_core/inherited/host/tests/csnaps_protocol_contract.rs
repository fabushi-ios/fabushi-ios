use serde_json::json;

use mahayana_host_runtime::extensions::codebase_telemetry::csnaps_protocol::{
    CsnapsErrorCode, CsnapsOperation, CsnapsOperationResult, CsnapsProtocolError,
    CsnapsResponse, CsnapsResponseDecoder, MAX_FRAME_BYTES,
    decode_frame_payloads, encode_csnaps_request, encode_frame,
    parse_csnaps_operation_result, parse_csnaps_response,
};

#[test]
fn frame_codec_is_big_endian_bounded_and_stream_safe() {
    let one = encode_frame(br#"{"id":1,"ok":true,"result":{}}"#).expect("frame");
    assert_eq!(&one[..4], &(one.len() as u32 - 4).to_be_bytes());

    let two = encode_frame(br#"{"id":2,"ok":false,"error":"server_busy"}"#)
        .expect("frame");
    let mut joined = one.clone();
    joined.extend_from_slice(&two);
    let split = one.len() + 3;
    let (payloads, remainder) =
        decode_frame_payloads(&joined[..split]).expect("decode partial");
    assert_eq!(payloads.len(), 1);
    assert_eq!(remainder.len(), 3);

    let mut decoder = CsnapsResponseDecoder::new();
    let first = decoder.decode(&joined[..split]).expect("decoder first");
    assert_eq!(first, vec![CsnapsResponse::Success { id: 1, result: json!({}) }]);
    assert!(decoder.assert_complete().is_err());
    let second = decoder.decode(&joined[split..]).expect("decoder second");
    assert_eq!(
        second,
        vec![CsnapsResponse::Error {
            id: 2,
            error: CsnapsErrorCode::ServerBusy,
        }]
    );
    decoder.assert_complete().expect("complete");

    let oversized = vec![0_u8; MAX_FRAME_BYTES + 1];
    assert!(encode_frame(&oversized)
        .expect_err("oversized request")
        .to_string()
        .contains("request frame length"));
    let mut bad_header = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec();
    bad_header.extend_from_slice(&[0; 4]);
    assert!(decode_frame_payloads(&bad_header)
        .expect_err("oversized response")
        .to_string()
        .contains("response frame length"));
}

#[test]
fn request_and_response_envelopes_match_frozen_protocol() {
    let frame = encode_csnaps_request(
        7,
        &CsnapsOperation::Initialize(
            serde_json::Map::from_iter([("root".into(), json!("/repo"))]),
        ),
    )
    .expect("request");
    let length = u32::from_be_bytes(frame[..4].try_into().expect("header")) as usize;
    let request: serde_json::Value =
        serde_json::from_slice(&frame[4..4 + length]).expect("request json");
    assert_eq!(
        request,
        json!({"id":7,"method":"initialize","params":{"root":"/repo"}})
    );

    assert_eq!(
        parse_csnaps_response(br#"{"id":9,"ok":true,"result":{"x":1}}"#)
            .expect("success"),
        CsnapsResponse::Success {
            id: 9,
            result: json!({"x":1}),
        }
    );
    assert_eq!(
        parse_csnaps_response(br#"{"id":10,"ok":false,"error":"not_initialized"}"#)
            .expect("error"),
        CsnapsResponse::Error {
            id: 10,
            error: CsnapsErrorCode::NotInitialized,
        }
    );
    for invalid in [
        br#"{"id":1,"ok":true,"result":{},"extra":1}"#.as_slice(),
        br#"{"id":1,"ok":false,"error":"made_up"}"#.as_slice(),
        br#"{"id":-1,"ok":true,"result":{}}"#.as_slice(),
        br#"[]"#.as_slice(),
    ] {
        assert!(parse_csnaps_response(invalid).is_err());
    }
}

#[test]
fn operation_results_are_strict_and_validate_uuid_v4() {
    let state = json!({
        "tracked": [{
            "codebaseUuid": "123e4567-e89b-42d3-a456-426614174000",
            "path": "/repo",
            "kind": 2,
            "environment": 3
        }]
    });
    let initialized = parse_csnaps_operation_result(
        &CsnapsOperation::Initialize(Default::default()),
        &json!({"state": state.clone()}),
    )
    .expect("initialize result");
    match initialized {
        CsnapsOperationResult::Initialize { state } => {
            assert_eq!(state.tracked.len(), 1);
            assert_eq!(state.tracked[0].path, "/repo");
        }
        other => panic!("unexpected result: {other:?}"),
    }

    let applied = parse_csnaps_operation_result(
        &CsnapsOperation::ApplyCodebaseSpecs(Default::default()),
        &json!({
            "state": state,
            "rejected": [{"path":"/bad","kind":4}]
        }),
    )
    .expect("apply result");
    match applied {
        CsnapsOperationResult::ApplyCodebaseSpecs { state, rejected } => {
            assert_eq!(state.tracked.len(), 1);
            assert_eq!(rejected[0].path, "/bad");
        }
        other => panic!("unexpected result: {other:?}"),
    }

    parse_csnaps_operation_result(
        &CsnapsOperation::Ping,
        &json!({}),
    )
    .expect("empty result");
    assert!(parse_csnaps_operation_result(
        &CsnapsOperation::Ping,
        &json!({"unexpected": true}),
    )
    .is_err());
    assert!(parse_csnaps_operation_result(
        &CsnapsOperation::Initialize(Default::default()),
        &json!({"state":{"tracked":[{
            "codebaseUuid":"123e4567-e89b-12d3-a456-426614174000",
            "path":"/repo","kind":1,"environment":1
        }]}})
    )
    .is_err());
}

#[test]
fn protocol_error_is_stable() {
    let error = CsnapsProtocolError::new("boom");
    assert_eq!(error.to_string(), "boom");
}
