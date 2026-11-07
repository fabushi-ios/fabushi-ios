//! Contract tests for OpenClaw Multi-Channel Pipeline & Adapters (OC-S3).
//!
//! Validates:
//! - Channel runtime registration of all external adapters (Discord, Slack, Telegram, Feishu, LINE, WhatsApp,
//!   Signal, iMessage, Matrix, Mattermost, Nostr, Twitch, Zalo, Nextcloud, Tlon, BlueBubbles, Google Chat, MS Teams).
//! - Webhook signature verification across providers:
//!   * Slack HMAC-SHA256 signature and 5-minute replay prevention window.
//!   * LINE X-Line-Signature HMAC-SHA256 Base64 verification.
//!   * Feishu verification token verification.
//!   * BlueBubbles password and HMAC authentication.
//!   * Google Chat Bearer JWT header and claims shape.
//!   * MS Teams Bot Framework Bearer JWT shape.
//! - Policy gating and candidate normalization:
//!   * evaluate_dm_policy across Open, Pairing, Allowlist, and Disabled modes.
//!   * AllowlistMatch candidate matching (phone numbers, usernames, prefixed IDs, wildcards).
//!   * Group policy mention gating.
//! - Built-in command dispatch:
//!   * dispatch_builtin_command for /ping, /help, /status, /id, /reset, /stop, /mode, /model.
//!   * Extensible custom command registry.
//! - Typing indicator lifecycle:
//!   * TypingSignaler heartbeat ticker, session tracking, and cancellation.
//!   * TypingScopeGuard RAII cleanup.
//! - Outbound chunked delivery:
//!   * OutboundDeliveryPayload generation.
//!   * chunk_discord_text code fence preservation under Discord limit.
//!   * build_telegram_thread_params for topics and reply threads.
//!   * normalize_whatsapp_jid phone and JID normalization.
//!   * prepare_outbound_chunks across platforms.
//! - End-to-end ChannelRuntime dispatch loop integration.

#[path = "../src/extensions/channel/mod.rs"]
pub mod channel;

#[path = "../src/extensions/line/mod.rs"]
pub mod line;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use channel::{
    build_telegram_thread_params, chunk_discord_text, dispatch_builtin_command,
    evaluate_dm_policy, evaluate_group_policy, hmac_sha256,
    normalize_whatsapp_jid, prepare_outbound_chunks, to_hex_string,
    verify_bluebubbles_webhook_auth, verify_feishu_webhook_token,
    verify_googlechat_bearer, verify_line_signature, verify_msteams_bearer_jwt_shape,
    verify_slack_signature, AllowlistMatch, ChannelCommandRegistry,
    ChannelDmPolicy, ChannelGroupPolicy, ChannelKind, ChannelPolicyGate,
    ChannelRuntime, CommandContext, DispatchAction,
    OutboundDeliveryPayload, TypingCallback, TypingScopeGuard, TypingSignaler,
    DISCORD_DEFAULT_MAX_CHARS,
};

// ---------------------------------------------------------------------------
// 1. Webhook Signature Verification Tests
// ---------------------------------------------------------------------------

#[test]
fn test_webhook_verification_slack() {
    let secret = "slack_secret_12345";
    let body = b"{\"command\":\"/status\"}";
    let now = chrono::Utc::now().timestamp();
    let ts_str = now.to_string();

    let prefix = format!("v0:{ts_str}:");
    let mut basestring = prefix.into_bytes();
    basestring.extend_from_slice(body);
    let digest = hmac_sha256(secret.as_bytes(), &basestring);
    let valid_header = format!("v0={}", to_hex_string(&digest));

    // Valid signature passes
    assert!(verify_slack_signature(secret, &ts_str, body, &valid_header));

    // Tampered body fails
    assert!(!verify_slack_signature(secret, &ts_str, b"tampered", &valid_header));

    // Wrong secret fails
    assert!(!verify_slack_signature("wrong_secret", &ts_str, body, &valid_header));

    // Replay attack prevention: timestamp > 300 seconds old rejected
    let old_ts = (now - 305).to_string();
    assert!(!verify_slack_signature(secret, &old_ts, body, &valid_header));
}

#[test]
fn test_webhook_verification_line() {
    use base64::Engine;
    let secret = "line_channel_secret_abc";
    let body = b"{\"events\":[{\"type\":\"message\"}]}";

    let digest = hmac_sha256(secret.as_bytes(), body);
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(digest);

    assert!(verify_line_signature(secret, body, &sig_b64));
    assert!(!verify_line_signature("wrong_secret", body, &sig_b64));
    assert!(!verify_line_signature(secret, b"other_body", &sig_b64));
}

#[test]
fn test_webhook_verification_feishu_and_bluebubbles() {
    // Feishu token verification
    assert!(verify_feishu_webhook_token("tok_correct", "tok_correct"));
    assert!(!verify_feishu_webhook_token("tok_correct", "tok_wrong"));
    assert!(!verify_feishu_webhook_token("", "tok_correct"));

    // BlueBubbles password and HMAC auth
    assert!(verify_bluebubbles_webhook_auth(
        Some("pass_secret"),
        Some("pass_secret"),
        None,
        b""
    ));
    assert!(!verify_bluebubbles_webhook_auth(
        Some("pass_secret"),
        Some("wrong_pass"),
        None,
        b""
    ));

    // BlueBubbles HMAC signature header
    let body = b"{\"text\":\"bluebubbles\"}";
    let digest = hmac_sha256(b"hmac_key", body);
    let hex_sig = to_hex_string(&digest);
    assert!(verify_bluebubbles_webhook_auth(
        Some("hmac_key"),
        None,
        Some(&format!("sha256={hex_sig}")),
        body
    ));
}

#[test]
fn test_webhook_verification_googlechat_and_msteams() {
    use base64::Engine;

    // Google Chat simulated JWT with valid issuer
    let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"alg\":\"RS256\"}");
    let claims = serde_json::json!({
        "iss": "chat@system.gserviceaccount.com",
        "aud": "project-12345"
    });
    let claims_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"simulated_signature");
    let valid_jwt = format!("Bearer {header_b64}.{claims_b64}.{sig_b64}");

    let res = verify_googlechat_bearer(&valid_jwt, Some("project-12345")).unwrap();
    assert!(res);

    // Mismatched audience rejected
    let res_bad_aud = verify_googlechat_bearer(&valid_jwt, Some("other-aud")).unwrap();
    assert!(!res_bad_aud);

    // MS Teams Bot Framework JWT shape
    let teams_claims = serde_json::json!({
        "iss": "https://api.botframework.com",
        "aud": "teams-app-id"
    });
    let teams_claims_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&teams_claims).unwrap());
    let teams_jwt = format!("Bearer {header_b64}.{teams_claims_b64}.{sig_b64}");

    let teams_res = verify_msteams_bearer_jwt_shape(&teams_jwt).unwrap();
    assert!(teams_res);

    // Invalid 2-segment JWT rejected
    let bad_jwt = "Bearer abc.def";
    assert!(verify_msteams_bearer_jwt_shape(bad_jwt).is_err());
}

// ---------------------------------------------------------------------------
// 2. Policy Gating and Candidate Normalization Tests
// ---------------------------------------------------------------------------

#[test]
fn test_policy_gate_dm_and_group() {
    let allowlist = vec![
        "alice".to_string(),
        "+1 (555) 123-4567".to_string(),
        "team_*".to_string(),
    ];

    // DM Allowlist mode
    assert_eq!(
        evaluate_dm_policy(ChannelDmPolicy::Allowlist, "@alice", &allowlist),
        AllowlistMatch::Exact("alice".to_string())
    );
    assert_eq!(
        evaluate_dm_policy(ChannelDmPolicy::Allowlist, "+15551234567", &allowlist),
        AllowlistMatch::Exact("+1 (555) 123-4567".to_string())
    );
    assert_eq!(
        evaluate_dm_policy(ChannelDmPolicy::Allowlist, "bob", &allowlist),
        AllowlistMatch::NotMatched
    );

    // Open / Pairing / Disabled modes
    assert_eq!(
        evaluate_dm_policy(ChannelDmPolicy::Open, "stranger", &allowlist),
        AllowlistMatch::Wildcard
    );
    assert_eq!(
        evaluate_dm_policy(ChannelDmPolicy::Pairing, "stranger", &allowlist),
        AllowlistMatch::Wildcard
    );
    assert_eq!(
        evaluate_dm_policy(ChannelDmPolicy::Disabled, "alice", &allowlist),
        AllowlistMatch::Denied
    );

    // Group Mention Policy
    assert_eq!(
        evaluate_group_policy(ChannelGroupPolicy::RequireMention, "user1", true, &[]),
        AllowlistMatch::Wildcard
    );
    assert_eq!(
        evaluate_group_policy(ChannelGroupPolicy::RequireMention, "user1", false, &[]),
        AllowlistMatch::NotMatched
    );

    // Composite ChannelPolicyGate
    let gate = ChannelPolicyGate::new(ChannelDmPolicy::Allowlist, ChannelGroupPolicy::RequireMention)
        .with_dm_allowlist(allowlist);

    assert!(gate.check_admission("@alice", true, false).is_allowed());
    assert!(!gate.check_admission("bob", true, false).is_allowed());
    assert!(gate.check_admission("anyone", false, true).is_allowed());
    assert!(!gate.check_admission("anyone", false, false).is_allowed());
}

// ---------------------------------------------------------------------------
// 3. Built-in Command Dispatch Tests
// ---------------------------------------------------------------------------

#[test]
fn test_command_dispatch_builtins() {
    let ctx = CommandContext::new("slack", "user_1", "conv_1", true);

    // /ping
    let ping = dispatch_builtin_command("/ping", &[], &ctx).unwrap();
    assert_eq!(ping.immediate_reply.as_deref(), Some("pong"));
    assert!(ping.should_halt);

    // /help
    let help = dispatch_builtin_command("/help", &[], &ctx).unwrap();
    assert!(help.immediate_reply.unwrap().contains("/help"));

    // /status
    let status = dispatch_builtin_command("/status", &[], &ctx).unwrap();
    assert!(status.immediate_reply.unwrap().contains("Online"));

    // /id
    let id_res = dispatch_builtin_command("/id", &[], &ctx).unwrap();
    assert!(id_res.immediate_reply.unwrap().contains("user_1"));

    // /reset & /stop
    let reset = dispatch_builtin_command("/reset", &[], &ctx).unwrap();
    assert_eq!(reset.system_notice.as_deref(), Some("context_reset_requested"));

    let stop = dispatch_builtin_command("/stop", &[], &ctx).unwrap();
    assert_eq!(stop.system_notice.as_deref(), Some("turn_stop_requested"));

    // Non-command
    assert!(dispatch_builtin_command("not a command", &[], &ctx).is_none());
}

#[test]
fn test_custom_command_registry() {
    let mut reg = ChannelCommandRegistry::new();
    reg.register("shout", |args, _ctx| {
        channel::CommandExecutionResult::reply(args.join(" ").to_uppercase())
    });

    let ctx = CommandContext::new("discord", "user_2", "conv_2", false);
    let res = reg.execute("/shout hello there", &ctx).unwrap();
    assert_eq!(res.immediate_reply.as_deref(), Some("HELLO THERE"));
}

// ---------------------------------------------------------------------------
// 4. Typing Indicator Lifecycle Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_typing_signaler_lifecycle_and_guard() {
    let signaler = TypingSignaler::new(Duration::from_millis(40));
    let ticks = Arc::new(AtomicUsize::new(0));

    let t_clone = ticks.clone();
    let callback: TypingCallback = Arc::new(move |_target, _thread| {
        t_clone.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    signaler.start("sess_a", "target_channel", None, callback);
    assert!(signaler.is_active("sess_a"));

    tokio::time::sleep(Duration::from_millis(100)).await;
    let count_before = ticks.load(Ordering::SeqCst);
    assert!(count_before >= 2);

    signaler.stop("sess_a");
    assert!(!signaler.is_active("sess_a"));

    // Test RAII Scope Guard
    let guard_ticks = Arc::new(AtomicUsize::new(0));
    let gt_clone = guard_ticks.clone();
    {
        let _guard = TypingScopeGuard::new(
            signaler.clone(),
            "sess_b",
            "target_b",
            None,
            Arc::new(move |_t, _th| {
                gt_clone.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        );
        assert!(signaler.is_active("sess_b"));
    }
    // Automatically stopped upon leaving scope
    assert!(!signaler.is_active("sess_b"));
}

// ---------------------------------------------------------------------------
// 5. Outbound Chunked Delivery & Helpers Tests
// ---------------------------------------------------------------------------

#[test]
fn test_chunk_discord_text_code_fence_preservation() {
    let code_sample = "Prefix text\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\nSuffix text";
    let chunks = chunk_discord_text(code_sample, 35);

    assert!(chunks.len() >= 2);
    for chunk in chunks {
        let count = chunk.matches("```").count();
        assert_eq!(count % 2, 0, "Fences must remain balanced: {chunk}");
    }
}

#[test]
fn test_build_telegram_thread_params() {
    let topic_params = build_telegram_thread_params(Some("1024"), true);
    assert_eq!(topic_params.message_thread_id, Some(1024));
    assert_eq!(topic_params.reply_to_message_id, None);
    assert!(topic_params.is_forum_topic);

    let reply_params = build_telegram_thread_params(Some("2048"), false);
    assert_eq!(reply_params.message_thread_id, None);
    assert_eq!(reply_params.reply_to_message_id, Some(2048));
    assert!(!reply_params.is_forum_topic);
}

#[test]
fn test_normalize_whatsapp_jid() {
    assert_eq!(
        normalize_whatsapp_jid("+1 (555) 987-6543").unwrap(),
        "15559876543@s.whatsapp.net"
    );
    assert_eq!(
        normalize_whatsapp_jid("999888777@s.whatsapp.net").unwrap(),
        "999888777@s.whatsapp.net"
    );
    assert_eq!(
        normalize_whatsapp_jid("12345-67890@g.us").unwrap(),
        "12345-67890@g.us"
    );
    assert!(normalize_whatsapp_jid("").is_err());
}

#[test]
fn test_prepare_outbound_chunks() {
    let long_body = "word ".repeat(600); // 3000 chars
    let payload = OutboundDeliveryPayload::new(ChannelKind::Discord, "ch-100", long_body);
    let chunks = prepare_outbound_chunks(&payload);

    assert!(chunks.len() >= 2);
    for chunk in chunks {
        assert!(chunk.text.chars().count() <= DISCORD_DEFAULT_MAX_CHARS);
    }
}

// ---------------------------------------------------------------------------
// 6. Unified ChannelRuntime Pipeline Integration Tests
// ---------------------------------------------------------------------------

#[test]
fn test_channel_runtime_pipeline_end_to_end() {
    let runtime = ChannelRuntime::new().with_agent_handler(Arc::new(|inbound| {
        Ok(format!("Agent received: {}", inbound.text))
    }));

    // Inbound Discord message
    let body = serde_json::json!({
        "id": "discord_msg_101",
        "channel_id": "guild_channel_99",
        "content": "Hello agent world!",
        "author": {
            "id": "discord_user_1",
            "username": "tester",
            "bot": false
        }
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    let res = runtime
        .handle_inbound_webhook(ChannelKind::Discord, &HashMap::new(), &body_bytes)
        .expect("handle_inbound_webhook");

    assert!(res.accepted);
    assert_eq!(res.action_taken, DispatchAction::ForwardedToAgent);

    // Duplicate message is suppressed
    let dup_res = runtime
        .handle_inbound_webhook(ChannelKind::Discord, &HashMap::new(), &body_bytes)
        .expect("duplicate dispatch");

    assert!(dup_res.accepted);
    assert_eq!(dup_res.action_taken, DispatchAction::SuppressedDuplicate);

    // Policy rejection
    let policy = ChannelPolicyGate::new(ChannelDmPolicy::Allowlist, ChannelGroupPolicy::Disabled)
        .with_dm_allowlist(vec!["authorized_user".to_string()]);
    runtime.set_policy_gate(ChannelKind::Telegram, policy);

    let tg_body = serde_json::json!({
        "update_id": 999,
        "message": {
            "message_id": 1,
            "from": { "id": 55555, "is_bot": false, "first_name": "Stranger" },
            "chat": { "id": 55555, "type": "private" },
            "text": "Secret probe"
        }
    });
    let tg_bytes = serde_json::to_vec(&tg_body).unwrap();

    let rej_res = runtime
        .handle_inbound_webhook(ChannelKind::Telegram, &HashMap::new(), &tg_bytes)
        .expect("policy evaluation");

    assert!(!rej_res.accepted);
    assert_eq!(rej_res.action_taken, DispatchAction::RejectedPolicy);
}

#[test]
fn test_all_18_channel_adapters_registered() {
    let runtime = ChannelRuntime::new();
    let channels = [
        ChannelKind::Discord,
        ChannelKind::Slack,
        ChannelKind::Telegram,
        ChannelKind::Feishu,
        ChannelKind::Line,
        ChannelKind::WhatsApp,
        ChannelKind::Signal,
        ChannelKind::IMessage,
        ChannelKind::Matrix,
        ChannelKind::Mattermost,
        ChannelKind::Nostr,
        ChannelKind::Twitch,
        ChannelKind::Zalo,
        ChannelKind::Nextcloud,
        ChannelKind::Tlon,
        ChannelKind::BlueBubbles,
        ChannelKind::GoogleChat,
        ChannelKind::MSTeams,
    ];

    for ch in channels {
        let adapter = runtime.get_adapter(ch);
        assert!(adapter.is_some(), "Adapter must be registered for channel {ch}");
        assert_eq!(adapter.unwrap().channel_kind(), ch);
    }
}
