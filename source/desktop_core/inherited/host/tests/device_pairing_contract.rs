use base64::Engine as _;
use mahayana_host_runtime::device_pairing_owner::DevicePairingOwner;
use serde_json::json;

#[test]
fn test_ocrm_pair_o01_request_success() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");
    let res = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({
                "deviceId": "dev-alpha-001",
                "publicKey": "base64-pubkey-sample",
                "displayName": "Alpha Device",
                "platform": "macos"
            }),
        )
        .expect("pair request call");

    assert_eq!(res["status"], "pending");
    assert!(res["requestId"].as_str().unwrap().starts_with("pair-"));
    assert_eq!(res["deviceId"], "dev-alpha-001");
    assert_eq!(res["created"], true);
    assert_eq!(res["isRepair"], false);
    assert!(res["expiresAtMs"].as_i64().unwrap() > 0);
}

#[test]
fn test_ocrm_pair_request_reuses_compatible_pending_identity() {
    let owner = DevicePairingOwner::in_memory().expect("pairing owner");
    let first = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({
                "deviceId": "dev-idempotent",
                "publicKey": "pubkey-idempotent",
                "browserOrigin": "https://browser-a.example",
                "roles": ["operator"],
                "scopes": ["operator.read", "operator.write"]
            }),
        )
        .unwrap();
    let first_request_id = first["requestId"].as_str().unwrap().to_string();

    let refreshed = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({
                "deviceId": "dev-idempotent",
                "publicKey": "pubkey-idempotent",
                "browserOrigin": "https://browser-a.example",
                "roles": ["operator"],
                "scopes": ["operator.read"]
            }),
        )
        .unwrap();
    assert_eq!(refreshed["requestId"], first_request_id);
    assert_eq!(refreshed["created"], false);
    assert!(refreshed["refreshedAtMs"].as_i64().unwrap() > 0);

    let status = owner
        .handle_pairing_command(
            "device.pair.status",
            &json!({ "requestId": first_request_id }),
        )
        .unwrap();
    assert_eq!(status["status"], "pending");

    let superseding = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({
                "deviceId": "dev-idempotent",
                "publicKey": "pubkey-idempotent",
                "browserOrigin": "https://browser-b.example",
                "roles": ["operator"],
                "scopes": ["operator.read"]
            }),
        )
        .unwrap();
    assert_ne!(superseding["requestId"], first_request_id);
    let superseded_status = owner
        .handle_pairing_command(
            "device.pair.status",
            &json!({ "requestId": first_request_id }),
        )
        .unwrap();
    assert_eq!(superseded_status["status"], "rejected");
}

#[test]
fn test_ocrm_pair_o02_request_requires_device_id_and_public_key() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");
    let err1 = owner
        .handle_pairing_command("device.pair.request", &json!({ "publicKey": "some-key" }))
        .unwrap_err();
    assert_eq!(err1.status(), 400);

    let err2 = owner
        .handle_pairing_command("device.pair.request", &json!({ "deviceId": "dev-01" }))
        .unwrap_err();
    assert_eq!(err2.status(), 400);
}

#[test]
fn test_ocrm_pair_o03_and_o04_pair_status() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");
    let req = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({
                "deviceId": "dev-status-test",
                "publicKey": "pubkey-status-test"
            }),
        )
        .expect("pair request");
    let request_id = req["requestId"].as_str().unwrap();

    // Valid pending request
    let status_pending = owner
        .handle_pairing_command("device.pair.status", &json!({ "requestId": request_id }))
        .expect("status call");
    assert_eq!(status_pending["status"], "pending");
    assert_eq!(status_pending["requestId"], request_id);

    // Unknown request -> rejected
    let status_unknown = owner
        .handle_pairing_command(
            "device.pair.status",
            &json!({ "requestId": "non-existent-req-id" }),
        )
        .expect("status call");
    assert_eq!(status_unknown["status"], "rejected");
}

#[test]
fn test_ocrm_pair_o05_and_o06_pair_approve() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");
    let initial_rev = owner.current_revision();

    let req = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({
                "deviceId": "dev-approve-test",
                "publicKey": "pubkey-approve-test",
                "displayName": "Approval Test Device"
            }),
        )
        .expect("pair request");
    let request_id = req["requestId"].as_str().unwrap();

    // Unknown requestId rejection (O06)
    let err_unknown = owner
        .handle_pairing_command(
            "device.pair.approve",
            &json!({ "requestId": "pair-unknown-999" }),
        )
        .unwrap_err();
    assert_eq!(err_unknown.status(), 400);

    // Successful approval (O05)
    let approve_res = owner
        .handle_pairing_command("device.pair.approve", &json!({ "requestId": request_id }))
        .expect("approve call");

    assert_eq!(approve_res["decision"], "approved");
    assert_eq!(approve_res["deviceId"], "dev-approve-test");
    let new_rev = approve_res["revision"].as_u64().unwrap();
    assert!(new_rev > initial_rev);
    assert_eq!(owner.current_revision(), new_rev);

    // Verify status transitions to approved
    let status = owner
        .handle_pairing_command(
            "device.pair.status",
            &json!({ "deviceId": "dev-approve-test" }),
        )
        .expect("status check");
    assert_eq!(status["status"], "approved");
    assert_eq!(status["device"]["deviceId"], "dev-approve-test");
}

#[test]
fn test_ocrm_pair_o07_pair_reject() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");
    let req = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({
                "deviceId": "dev-reject-test",
                "publicKey": "pubkey-reject-test"
            }),
        )
        .expect("pair request");
    let request_id = req["requestId"].as_str().unwrap();

    let rej_res = owner
        .handle_pairing_command("device.pair.reject", &json!({ "requestId": request_id }))
        .expect("reject call");
    assert_eq!(rej_res["decision"], "rejected");
    assert_eq!(rej_res["deviceId"], "dev-reject-test");

    // Status after rejection returns rejected
    let status = owner
        .handle_pairing_command("device.pair.status", &json!({ "requestId": request_id }))
        .expect("status check");
    assert_eq!(status["status"], "rejected");
}

#[test]
fn test_ocrm_pair_o08_pair_list() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");

    let _req1 = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({ "deviceId": "dev-list-pending", "publicKey": "pk-1" }),
        )
        .unwrap();
    let req2 = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({ "deviceId": "dev-list-paired", "publicKey": "pk-2" }),
        )
        .unwrap();

    owner
        .handle_pairing_command(
            "device.pair.approve",
            &json!({ "requestId": req2["requestId"].as_str().unwrap() }),
        )
        .unwrap();

    let list_res = owner
        .handle_pairing_command("device.pair.list", &json!({}))
        .unwrap();
    let pending = list_res["pending"].as_array().unwrap();
    let paired = list_res["paired"].as_array().unwrap();

    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["deviceId"], "dev-list-pending");
    assert_eq!(paired.len(), 1);
    assert_eq!(paired[0]["deviceId"], "dev-list-paired");
}

#[test]
fn test_ocrm_pair_o09_and_o10_pair_revoke() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");

    // Unknown device revoke rejects (O10)
    let err_unknown = owner
        .handle_pairing_command("device.pair.revoke", &json!({ "deviceId": "dev-ghost" }))
        .unwrap_err();
    assert_eq!(err_unknown.status(), 400);

    // Create & approve device
    let req = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({ "deviceId": "dev-to-revoke", "publicKey": "pk-rev" }),
        )
        .unwrap();
    owner
        .handle_pairing_command(
            "device.pair.approve",
            &json!({ "requestId": req["requestId"].as_str().unwrap() }),
        )
        .unwrap();
    let rotated = owner
        .handle_pairing_command(
            "device.pair.rotateToken",
            &json!({ "deviceId": "dev-to-revoke" }),
        )
        .unwrap();
    let token = rotated["token"].as_str().unwrap().to_string();
    assert!(owner.authenticate_device_token(&token).unwrap().is_some());
    let rev_before = owner.current_revision();

    // Revoke device (O09)
    let revoke_res = owner
        .handle_pairing_command(
            "device.pair.revoke",
            &json!({ "deviceId": "dev-to-revoke" }),
        )
        .unwrap();
    assert_eq!(revoke_res["deviceId"], "dev-to-revoke");
    let new_rev = revoke_res["revision"].as_u64().unwrap();
    assert!(new_rev > rev_before);

    // Subsequent status check returns rejected
    let status = owner
        .handle_pairing_command(
            "device.pair.status",
            &json!({ "deviceId": "dev-to-revoke" }),
        )
        .unwrap();
    assert_eq!(status["status"], "rejected");
    assert!(owner.authenticate_device_token(&token).unwrap().is_none());
}

#[test]
fn test_ocrm_pair_o11_rotate_token_in_band_and_cross_device() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");

    let req = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({ "deviceId": "dev-rotate-target", "publicKey": "pk-rot" }),
        )
        .unwrap();
    owner
        .handle_pairing_command(
            "device.pair.approve",
            &json!({ "requestId": req["requestId"].as_str().unwrap() }),
        )
        .unwrap();

    // In-band rotation: returns token directly
    let rot_in_band = owner
        .handle_pairing_command(
            "device.pair.rotateToken",
            &json!({ "deviceId": "dev-rotate-target", "crossDevice": false }),
        )
        .unwrap();
    assert_eq!(rot_in_band["tokenDelivery"], "in-band");
    let first_token = rot_in_band["token"].as_str().unwrap().to_string();
    assert!(owner
        .authenticate_device_token(&first_token)
        .unwrap()
        .is_some());
    assert!(
        rot_in_band["token"]
            .as_str()
            .unwrap()
            .starts_with("sand-device-tok-")
    );
    let token_body = rot_in_band["token"]
        .as_str()
        .unwrap()
        .strip_prefix("sand-device-tok-")
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token_body)
            .unwrap()
            .len(),
        32
    );

    // Cross-device rotation: withholds token in-band
    let rot_cross = owner
        .handle_pairing_command(
            "device.pair.rotateToken",
            &json!({ "deviceId": "dev-rotate-target", "crossDevice": true }),
        )
        .unwrap();
    assert_eq!(rot_cross["tokenDelivery"], "withheld-cross-device");
    assert!(rot_cross.get("token").is_none());
    assert!(owner
        .authenticate_device_token(&first_token)
        .unwrap()
        .is_none());

    let rejected_role = owner.handle_pairing_command(
        "device.pair.rotateToken",
        &json!({ "deviceId": "dev-rotate-target", "role": "node" }),
    );
    assert!(rejected_role.is_err());

    let multi_role_request = owner
        .handle_pairing_command(
            "device.pair.request",
            &json!({
                "deviceId": "dev-multi-role",
                "publicKey": "pk-multi-role",
                "roles": ["node", "operator"]
            }),
        )
        .unwrap();
    owner
        .handle_pairing_command(
            "device.pair.approve",
            &json!({ "requestId": multi_role_request["requestId"] }),
        )
        .unwrap();
    let operator_rotation = owner
        .handle_pairing_command(
            "device.pair.rotateToken",
            &json!({ "deviceId": "dev-multi-role", "role": "operator" }),
        )
        .unwrap();
    let operator_token = operator_rotation["token"].as_str().unwrap().to_string();
    let node_rotation = owner
        .handle_pairing_command(
            "device.pair.rotateToken",
            &json!({ "deviceId": "dev-multi-role", "role": "node" }),
        )
        .unwrap();
    assert_eq!(node_rotation["role"], "node");
    assert!(owner.authenticate_device_token(&operator_token).unwrap().is_some());
}

#[test]
fn test_ocrm_pair_o12_setup_code() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");
    let res = owner
        .handle_pairing_command("device.pair.setupCode", &json!({}))
        .unwrap();

    assert!(res["setupId"].as_str().unwrap().starts_with("setup-"));
    assert!(
        res["setupCode"]
            .as_str()
            .unwrap()
            .starts_with("sand-setup-")
    );
    assert_eq!(res["auth"], "token");
    assert_eq!(res["access"], "full");
    assert!(res["expiresAtMs"].as_i64().unwrap() > 0);
    assert!(res.get("gatewayUrl").is_none());

    let configured = owner
        .handle_pair_setup_code(&json!({ "gatewayUrl": "wss://gateway.example.test" }))
        .unwrap();
    assert_eq!(configured["gatewayUrl"], "wss://gateway.example.test");

    let rejected = owner.handle_pair_setup_code(
        &json!({ "gatewayUrl": "ws://gateway.example.test" }),
    );
    assert!(rejected.is_err());
}

#[test]
fn test_ocrm_pair_o12_setup_code_is_expiring_and_single_use() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");
    let issued = owner
        .handle_pairing_command("device.pair.setupCode", &json!({}))
        .expect("issue setup code");
    let setup_code = issued["setupCode"].as_str().expect("setup code");

    let consumed = owner
        .consume_bootstrap_token(setup_code)
        .expect("consume setup code")
        .expect("live setup code");
    assert_eq!(consumed["setupId"], issued["setupId"]);
    assert_eq!(consumed["expiresAtMs"], issued["expiresAtMs"]);
    assert!(consumed["expiresAtMs"].as_i64().unwrap() > consumed["issuedAtMs"].as_i64().unwrap());

    assert!(owner
        .consume_bootstrap_token(setup_code)
        .expect("second consume")
        .is_none());
}

#[test]
fn test_ocrm_pair_o13_join_code_validation_and_path_parsing() {
    use mahayana_host_runtime::device_pairing_owner::*;

    // Valid 22-char base64url codes
    assert!(is_device_pairing_join_code("A1b2C3d4E5f6G7h8I9j0_-"));
    assert!(is_device_pairing_join_code("abcdefghijklmnopqrstuv"));

    // Invalid length or chars
    assert!(!is_device_pairing_join_code("too-short"));
    assert!(!is_device_pairing_join_code("A1b2C3d4E5f6G7h8I9j0_+=")); // + and = are standard base64, not url
    assert!(!is_device_pairing_join_code(
        "this-code-is-longer-than-22-chars"
    ));

    // Path parsing
    assert_eq!(
        parse_device_pairing_join_request_path("/gateway/j"),
        Some("")
    );
    assert_eq!(parse_device_pairing_join_request_path("/j"), Some(""));
    assert_eq!(
        parse_device_pairing_join_request_path("/proxy/j/app/j/code123"),
        Some("code123")
    );
    assert_eq!(
        parse_device_pairing_join_request_path("/gateway/j/shortcode"),
        Some("shortcode")
    );
    assert_eq!(parse_device_pairing_join_request_path("/api/status"), None);
}

#[test]
fn test_ocrm_pair_o14_safe_keys_and_labels() {
    use mahayana_host_runtime::device_pairing_owner::*;

    // Key sanitization
    assert_eq!(safe_channel_key("Telegram").unwrap(), "telegram");
    assert_eq!(
        safe_channel_key("slack/team:work*channel").unwrap(),
        "slack_team_work_channel"
    );
    assert_eq!(safe_account_key("user..profile").unwrap(), "user_profile");

    // Empty and sanitized-empty keys rejected
    assert!(safe_channel_key("   ").is_err());
    assert!(safe_channel_key("..").is_err());
    assert!(safe_account_key("/").is_err());

    // ID labels
    assert_eq!(resolve_pairing_id_label(None), "userId");
    assert_eq!(resolve_pairing_id_label(Some("telegramId")), "telegramId");
}

#[test]
fn test_ocrm_pair_o15_pairing_reply_formatting() {
    use mahayana_host_runtime::device_pairing_owner::*;

    let reply = build_pairing_reply("discord", "Discord User: @alice#1234", "PAIR-9876");
    assert!(reply.contains("OpenClaw: access not configured."));
    assert!(reply.contains("Discord User: @alice#1234"));
    assert!(reply.contains("PAIR-9876"));
    assert!(reply.contains("openclaw pairing approve discord PAIR-9876"));
}

#[test]
fn test_ocrm_pair_o16_mobile_pairing_security_policy() {
    use mahayana_host_runtime::device_pairing_owner::*;

    // Secure WSS and HTTPS always allowed
    assert!(validate_mobile_pairing_url("wss://gateway.example.com/ws").is_ok());
    assert!(validate_mobile_pairing_url("https://gateway.example.com/ws").is_ok());

    // Loopback and LAN cleartext allowed
    assert!(validate_mobile_pairing_url("ws://localhost:8080").is_ok());
    assert!(validate_mobile_pairing_url("ws://127.0.0.1:8080").is_ok());
    assert!(validate_mobile_pairing_url("ws://[::1]:8080").is_ok());
    assert!(validate_mobile_pairing_url("ws://192.168.1.100:8080").is_ok());
    assert!(validate_mobile_pairing_url("ws://10.0.1.20:8080").is_ok());
    assert!(validate_mobile_pairing_url("ws://172.20.0.5:8080").is_ok());
    assert!(validate_mobile_pairing_url("ws://169.254.10.20:8080").is_ok());
    assert!(validate_mobile_pairing_url("ws://macbook.local:8080").is_ok());
    assert!(validate_mobile_pairing_url("ws://10.0.2.2:8080").is_ok()); // Android emulator

    // Public cleartext internet rejected
    let public_err = validate_mobile_pairing_url("ws://8.8.8.8:8080").unwrap_err();
    assert!(public_err.contains("require a secure gateway URL"));
    let domain_err = validate_mobile_pairing_url("ws://public.openclaw.ai:8080").unwrap_err();
    assert!(domain_err.contains("require a secure gateway URL"));
}

#[test]
fn test_ocrm_pair_o17_join_code_registration_and_single_use_redeem() {
    use mahayana_host_runtime::device_pairing_owner::DEVICE_PAIRING_JOIN_CODE_BYTES;

    let owner = DevicePairingOwner::in_memory().expect("in-memory db");
    let expires_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 60_000;
    let payload = json!({
        "url": "wss://gateway.example.test/ws",
        "bootstrapToken": "bootstrap-token",
        "expiresAtMs": expires_at_ms
    });

    let shortcode = owner
        .register_device_pairing_join_code(&payload, expires_at_ms)
        .expect("register join code");
    assert_eq!(shortcode.len(), 22);
    assert!(shortcode.len() >= (DEVICE_PAIRING_JOIN_CODE_BYTES * 4 / 3));
    assert!(
        shortcode
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    );

    let redeemed = owner
        .redeem_device_pairing_join_code(&shortcode)
        .expect("redeem join code")
        .expect("live payload");
    assert_eq!(redeemed, payload);
    assert!(
        owner
            .redeem_device_pairing_join_code(&shortcode)
            .expect("second redeem")
            .is_none()
    );
}

#[test]
fn test_ocrm_pair_o18_join_code_expiry_and_payload_bounds_fail_closed() {
    let owner = DevicePairingOwner::in_memory().expect("in-memory db");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        owner
            .register_device_pairing_join_code(
                &json!({"url": "wss://gateway.example.test"}),
                now_ms
            )
            .is_err()
    );
    assert!(
        owner
            .redeem_device_pairing_join_code("invalid-shortcode")
            .expect("invalid shortcode is a safe miss")
            .is_none()
    );
    let oversized = json!("x".repeat(64 * 1024));
    assert!(
        owner
            .register_device_pairing_join_code(&oversized, now_ms + 60_000)
            .is_err()
    );
}
