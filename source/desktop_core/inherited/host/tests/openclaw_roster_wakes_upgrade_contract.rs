//! OpenClaw Roster Search, Background Wakes & Upgrade Recreate Resume Native Rust Fusion & Contract Tests (OCRM-M20/M21/M22)
//!
//! Validates the 12 falsifiable test oracles defined for OCRM Roster Search, Background Wakes, and Upgrade Resume:
//! - OCRM-ROSTER-O01: AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT and AGENT_CONTENT_SEARCH_MAX_RESULTS constants.
//! - OCRM-ROSTER-O02: TranscriptSearchResult and RosterMediaSearchResult camelCase serialization and field integrity.
//! - OCRM-ROSTER-O03: build_content_snippet sliding window matching, whitespace flattening, and ellipsis boundary handling.
//! - OCRM-ROSTER-O04: find_agent_content_matches reverse order, match limit, and hidden peer message filtering.
//! - OCRM-WAKE-O01: Wake cue constants ([event], [inbound], [channel-delivery-failed]).
//! - OCRM-WAKE-O02: format_channel_address_value string and structured JSON object address normalization.
//! - OCRM-WAKE-O03: distinct_inbound_channel_addresses address deduplication and formatting.
//! - OCRM-WAKE-O04: build_channel_inbound_wake_prompt message/reaction pluralization, outside platform notice, and reply instructions.
//! - OCRM-WAKE-O05: humanize_channel_delivery_failure platform error translation and user-friendly copy.
//! - OCRM-WAKE-O06: build_channel_delivery_failure_wake_prompt failure cue and in-app disclosure instructions.
//! - OCRM-UPGRADE-O01: UpgradeRecreateResume atomic quiesce lifecycle (quiesce_for_upgrade, is_quiescing_for_upgrade, resume_after_recreate).
//! - OCRM-UPGRADE-O02: build_upgrade_resume_prompt source-specific silence semantics and prompt framing.

use mahayana_host_runtime::extensions::transcript::background_wakes::{
    build_channel_delivery_failure_wake_prompt, build_channel_inbound_wake_prompt,
    distinct_inbound_channel_addresses, format_channel_address_value,
    humanize_channel_delivery_failure, CHANNEL_DELIVERY_FAILED_WAKE_CUE, CHANNEL_INBOUND_WAKE_CUE,
    TIMELINE_EVENT_WAKE_CUE,
};
use mahayana_host_runtime::extensions::transcript::roster_search::{
    build_content_snippet, find_agent_content_matches,
    RosterMediaSearchResult, TranscriptSearchResult,
    AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT, AGENT_CONTENT_SEARCH_MAX_RESULTS,
};
use mahayana_host_runtime::extensions::transcript::upgrade_recreate_resume::{
    build_upgrade_resume_prompt, UpgradeRecreateResume,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// OCRM-ROSTER-O01: Roster Search Constant Bounds
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_roster_o01_search_constant_bounds() {
    assert_eq!(
        AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT, 5,
        "Per-agent content search match limit must be fixed at 5"
    );
    assert_eq!(
        AGENT_CONTENT_SEARCH_MAX_RESULTS, 50,
        "Total roster search results limit must be capped at 50"
    );
}

// ---------------------------------------------------------------------------
// OCRM-ROSTER-O02: Transcript & Media Search Result Serialization
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_roster_o02_search_results_serialization() {
    let transcript_result = TranscriptSearchResult {
        agent_id: "agent-alpha-42".to_string(),
        entry_id: "entry-9988".to_string(),
        role: "assistant".to_string(),
        timestamp_ms: 1730000000123.0,
        snippet: "Found relevant information here…".to_string(),
    };

    let serialized_transcript = serde_json::to_value(&transcript_result).expect("serialize transcript result");
    assert_eq!(
        serialized_transcript.get("agentId").and_then(|v| v.as_str()),
        Some("agent-alpha-42")
    );
    assert_eq!(
        serialized_transcript.get("entryId").and_then(|v| v.as_str()),
        Some("entry-9988")
    );
    assert_eq!(
        serialized_transcript.get("role").and_then(|v| v.as_str()),
        Some("assistant")
    );
    assert_eq!(
        serialized_transcript.get("snippet").and_then(|v| v.as_str()),
        Some("Found relevant information here…")
    );

    let media_result = RosterMediaSearchResult {
        agent_id: "agent-beta-12".to_string(),
        entry_id: "entry-7766".to_string(),
        file_name: "architecture_diagram.png".to_string(),
        ext: "png".to_string(),
        mime: Some("image/png".to_string()),
        kind: "image".to_string(),
        timestamp_ms: 1730000005000,
        width: Some(1920),
        height: Some(1080),
    };

    let serialized_media = serde_json::to_value(&media_result).expect("serialize media result");
    assert_eq!(
        serialized_media.get("agentId").and_then(|v| v.as_str()),
        Some("agent-beta-12")
    );
    assert_eq!(
        serialized_media.get("fileName").and_then(|v| v.as_str()),
        Some("architecture_diagram.png")
    );
    assert_eq!(
        serialized_media.get("mime").and_then(|v| v.as_str()),
        Some("image/png")
    );
    assert_eq!(
        serialized_media.get("width").and_then(|v| v.as_i64()),
        Some(1920)
    );
}

// ---------------------------------------------------------------------------
// OCRM-ROSTER-O03: Content Snippet Window Matching & Whitespace Flattening
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_roster_o03_content_snippet_builder() {
    let long_text = "This is a lengthy document containing various sections. \
                     Deep inside the text we find the unique target keyword that should be centered. \
                     Following the match there are multiple additional trailing words.";

    let snippet = build_content_snippet(long_text, "target keyword");
    assert!(snippet.is_some(), "Snippet should be generated for existing keyword");
    let unwrapped = snippet.unwrap();
    assert!(unwrapped.contains("target keyword"));
    assert!(unwrapped.contains('…'), "Snippet should contain boundary ellipsis");

    // Whitespace flattening
    let messy_text = "Multiple   spaces  \n\n and tabs \t target   word  \n here.";
    let flattened_snippet = build_content_snippet(messy_text, "target word");
    assert!(flattened_snippet.is_some());
    assert!(flattened_snippet.unwrap().contains("target word"));

    // Empty query or text
    assert!(build_content_snippet("", "query").is_none());
    assert!(build_content_snippet("some text", "").is_none());
}

// ---------------------------------------------------------------------------
// OCRM-ROSTER-O04: Find Agent Content Matches & Hidden Peer Filter
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_roster_o04_find_agent_content_matches_and_filter() {
    let entries = vec![
        json!({
            "id": "entry-oldest",
            "kind": "message",
            "role": "user",
            "content": "Initial query about Rust migration",
            "timestampMs": 1000.0
        }),
        json!({
            "id": "entry-hidden",
            "kind": "message",
            "role": "assistant",
            "content": "Hidden internal peer coordination about Rust",
            "timestampMs": 2000.0,
            "hiddenOutboundAgentPeerMessage": true
        }),
        json!({
            "id": "entry-notice",
            "kind": "notice",
            "text": "System notice regarding Rust toolchain status",
            "timestampMs": 3000.0
        }),
        json!({
            "id": "entry-newest",
            "kind": "send-message",
            "message": {
                "type": "text",
                "content": "Latest update on Rust migration status"
            },
            "timestampMs": 4000.0
        }),
    ];

    let matches = find_agent_content_matches(&entries, "rust", 10);

    // Verified reverse chronological order (newest first)
    assert_eq!(matches.len(), 3, "Hidden peer message must be excluded");
    assert_eq!(matches[0].entry_id, "entry-newest");
    assert_eq!(matches[1].entry_id, "entry-notice");
    assert_eq!(matches[2].entry_id, "entry-oldest");

    // Verified role resolution
    assert_eq!(matches[0].role, "assistant");
    assert_eq!(matches[1].role, "assistant"); // Notices default to assistant role
    assert_eq!(matches[2].role, "user");
}

// ---------------------------------------------------------------------------
// OCRM-WAKE-O01: Wake Cue String Constants
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_wake_o01_cue_constants() {
    assert_eq!(TIMELINE_EVENT_WAKE_CUE, "[event]");
    assert_eq!(CHANNEL_INBOUND_WAKE_CUE, "[inbound]");
    assert_eq!(CHANNEL_DELIVERY_FAILED_WAKE_CUE, "[channel-delivery-failed]");
}

// ---------------------------------------------------------------------------
// OCRM-WAKE-O02: Channel Address Normalization
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_wake_o02_channel_address_formatting() {
    // String address
    let string_addr = json!("slack:general");
    assert_eq!(
        format_channel_address_value(&string_addr),
        Some("slack:general".to_string())
    );

    // Whitespace string address
    let trimmed_addr = json!("   discord:community   ");
    assert_eq!(
        format_channel_address_value(&trimmed_addr),
        Some("discord:community".to_string())
    );

    // Empty string address
    let empty_addr = json!("   ");
    assert_eq!(format_channel_address_value(&empty_addr), None);

    // Structured platform/chat object
    let obj_addr = json!({
        "platform": "telegram",
        "chat": "eng-channel"
    });
    assert_eq!(
        format_channel_address_value(&obj_addr),
        Some("telegram:eng-channel".to_string())
    );

    // Missing chat field
    let missing_chat = json!({
        "platform": "telegram"
    });
    assert_eq!(format_channel_address_value(&missing_chat), None);
}

// ---------------------------------------------------------------------------
// OCRM-WAKE-O03: Distinct Inbound Channel Address Deduplication
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_wake_o03_distinct_inbound_addresses() {
    let envelopes = vec![
        json!({"address": "slack:general", "text": "hello"}),
        json!({"address": {"platform": "discord", "chat": "general"}, "text": "ping"}),
        json!({"address": "slack:general", "text": "duplicate address"}),
        json!({"address": "   slack:general   ", "text": "padded address"}),
        json!({"address": "telegram:alerts", "text": "alert"}),
    ];

    let distinct = distinct_inbound_channel_addresses(&envelopes);
    assert_eq!(distinct.len(), 3);
    assert_eq!(distinct[0], "slack:general");
    assert_eq!(distinct[1], "discord:general");
    assert_eq!(distinct[2], "telegram:alerts");
}

// ---------------------------------------------------------------------------
// OCRM-WAKE-O04: Channel Inbound Wake Prompt Composition
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_wake_o04_channel_inbound_wake_prompt() {
    // Single message envelope
    let single_msg = vec![
        json!({
            "address": "slack:support",
            "sender": "Alice",
            "text": "How do I install Fabushi?"
        })
    ];
    let prompt_single = build_channel_inbound_wake_prompt(&single_msg);
    assert!(prompt_single.starts_with(CHANNEL_INBOUND_WAKE_CUE));
    assert!(prompt_single.contains("New message on a channel you are connected to."));
    assert!(prompt_single.contains("On Slack, from slack:support:"));
    assert!(prompt_single.contains("Alice: How do I install Fabushi?"));
    assert!(prompt_single.contains("outside platform, not the user typing in this app"));
    assert!(prompt_single.contains("Reply to them by calling SendMessage"));

    // Multiple message envelopes
    let multi_msgs = vec![
        json!({"address": "slack:support", "sender": "Alice", "text": "Question 1"}),
        json!({"address": "slack:support", "sender": "Bob", "text": "Question 2"}),
    ];
    let prompt_multi = build_channel_inbound_wake_prompt(&multi_msgs);
    assert!(prompt_multi.contains("New messages on a channel you are connected to."));

    // Reaction-only envelopes
    let reaction_envelopes = vec![
        json!({
            "address": {
                "platform": "discord",
                "chat": "chat"
            },
            "sender": "Charlie",
            "reaction": {
                "emoji": "👍",
                "messageQuote": "Deployment complete"
            }
        })
    ];
    let prompt_reaction = build_channel_inbound_wake_prompt(&reaction_envelopes);
    assert!(prompt_reaction.contains("New reaction on a channel you are connected to."));
    assert!(prompt_reaction.contains("On Discord, from discord:chat:"));
    assert!(prompt_reaction.contains("Charlie reacted 👍 to your message: \"Deployment complete\""));
    assert!(prompt_reaction.contains("You don't need to reply; act on a reaction only if it's useful"));
}

// ---------------------------------------------------------------------------
// OCRM-WAKE-O05: Humanized Channel Delivery Failure Copy
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_wake_o05_humanize_delivery_failure() {
    let unavail = humanize_channel_delivery_failure(
        "slack:general",
        "No channel delivery mechanism is registered.",
    );
    assert_eq!(
        unavail,
        "Channel messaging isn't available on this computer, so that message wasn't delivered."
    );

    let not_connected = humanize_channel_delivery_failure(
        "discord:random",
        "No live discord connection active",
    );
    assert!(not_connected.contains("Discord isn't connected on this computer"));
    assert!(not_connected.contains("Connect Discord (add its token)"));

    let invalid_addr = humanize_channel_delivery_failure(
        "malformed-token",
        "invalid channel format",
    );
    assert_eq!(
        invalid_addr,
        "\"malformed-token\" isn't a valid channel address, so that message wasn't delivered."
    );
}

// ---------------------------------------------------------------------------
// OCRM-WAKE-O06: Channel Delivery Failure Wake Prompt
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_wake_o06_delivery_failure_wake_prompt() {
    let failures = vec![
        json!({
            "addressToken": "slack:ops",
            "reason": "Token expired"
        }),
        json!({
            "addressToken": "discord:alerts",
            "reason": "Channel not found"
        })
    ];

    let prompt = build_channel_delivery_failure_wake_prompt(&failures);
    assert!(prompt.starts_with(CHANNEL_DELIVERY_FAILED_WAKE_CUE));
    assert!(prompt.contains("A message you tried to send to a channel did not go through."));
    assert!(prompt.contains("- To slack:ops: Token expired"));
    assert!(prompt.contains("- To discord:alerts: Channel not found"));
    assert!(prompt.contains("Tell the user plainly here, in this in-app chat (a SendMessage with no channel target)"));
    assert!(prompt.contains("Don't silently retry the same channel"));
}

// ---------------------------------------------------------------------------
// OCRM-UPGRADE-O01: UpgradeRecreateResume Atomic Quiesce Lifecycle
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_upgrade_o01_quiesce_lifecycle() {
    let resume = UpgradeRecreateResume::default();
    assert!(
        !resume.is_quiescing_for_upgrade(),
        "Initially should not be quiescing"
    );

    let summary = resume.quiesce_for_upgrade(7);
    assert!(summary.quiescing, "Summary should indicate quiescing");
    assert_eq!(summary.running_turns, 7, "Running turns count must match");
    assert!(
        resume.is_quiescing_for_upgrade(),
        "is_quiescing_for_upgrade must return true after quiesce call"
    );

    resume.resume_after_recreate();
    assert!(
        !resume.is_quiescing_for_upgrade(),
        "is_quiescing_for_upgrade must return false after resume"
    );
}

// ---------------------------------------------------------------------------
// OCRM-UPGRADE-O02: Upgrade Resume Prompts & Source Silence Semantics
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_upgrade_o02_resume_prompts() {
    let standard = build_upgrade_resume_prompt("turn");
    assert!(standard.contains("A background system update restarted your environment and interrupted you mid-task."));
    assert!(standard.contains("Continue exactly where you left off and finish what you were doing."));
    assert!(standard.contains("nothing reaches the user unless it's inside a SendMessage."));

    let automation = build_upgrade_resume_prompt("automation");
    assert!(automation.contains("interrupted a scheduled routine run mid-task."));
    assert!(automation.contains("ending with no SendMessage remains a valid outcome."));

    let revival = build_upgrade_resume_prompt("background-revival");
    assert!(revival.contains("interrupted one of your background-work follow-ups mid-delivery."));
    assert!(revival.contains("ending with no SendMessage remains a valid outcome."));
}
