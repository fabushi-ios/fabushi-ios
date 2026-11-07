use mahayana_host_runtime::runner::bot_block_detection::{
    BotBlockConfidence, classify_bot_block_page,
};
use mahayana_host_runtime::runner::sand_agent_profile_prompt::{
    AgentProfileIdentity, AgentProfilePromptSnapshot, agent_profile_identities_equal,
    normalize_agent_profile_identity, parse_latest_agent_profile_update,
    render_agent_profile_update, resolve_agent_profile_prompt_snapshot,
};
use mahayana_host_runtime::runner::sand_prompt_markers::SAND_HIDDEN_PROMPT_MARKER;
use mahayana_host_runtime::runner::tools::sand_browser_use_subagent::{
    BROWSER_USE_SUBAGENT_DESCRIPTION, BROWSER_USE_SUBAGENT_TYPE,
    create_sand_browser_use_subagent_config, is_browser_use_subagent_type,
};

#[test]
fn browser_use_subagent_type_and_config_match_grok() {
    assert_eq!(BROWSER_USE_SUBAGENT_TYPE, "browserUse");
    assert!(is_browser_use_subagent_type(Some("browserUse")));
    assert!(is_browser_use_subagent_type(Some("Browser-Use")));
    assert!(is_browser_use_subagent_type(Some("browser_use")));
    assert!(is_browser_use_subagent_type(Some("browser use")));
    assert!(!is_browser_use_subagent_type(Some("computerUse")));
    assert!(!is_browser_use_subagent_type(None));

    let config = create_sand_browser_use_subagent_config();
    assert_eq!(config.subagent_type.r#type.case, "custom");
    assert_eq!(config.subagent_type.r#type.value.name, "browserUse");
    assert_eq!(config.description, BROWSER_USE_SUBAGENT_DESCRIPTION);
    assert!(!config.preserve_task_tool);
    assert_eq!(config.subagent_source, "builtin");
    assert!(config.description.contains("without touching the desktop's mouse or keyboard"));
    assert!(config.description.contains("request_box_help"));
}

#[test]
fn agent_profile_prompt_round_trips_and_respects_compaction_epoch() {
    let raw = AgentProfileIdentity {
        name: "  Dharma Researcher  ".into(),
        description: "  Finds sources precisely.  ".into(),
    };
    let normalized = normalize_agent_profile_identity(raw);
    assert_eq!(normalized.name, "Dharma Researcher");
    assert_eq!(normalized.description, "Finds sources precisely.");
    assert!(agent_profile_identities_equal(&normalized, &normalized.clone()));

    let first = resolve_agent_profile_prompt_snapshot(
        None,
        3,
        "profile-v1",
        normalized.clone(),
    );
    assert_eq!(first.version, 1);
    assert_eq!(first.profile_section, "profile-v1");
    assert_eq!(first.system_identity, normalized);
    assert_eq!(first.announced_identity, normalized);
    assert_eq!(first.compaction_epoch, 3);

    let reused = resolve_agent_profile_prompt_snapshot(
        Some(&first),
        3,
        "ignored-new-profile",
        AgentProfileIdentity {
            name: "Ignored".into(),
            description: "Ignored".into(),
        },
    );
    assert_eq!(reused, first);

    let refreshed = resolve_agent_profile_prompt_snapshot(
        Some(&first),
        4,
        "profile-v2",
        AgentProfileIdentity {
            name: "New Name".into(),
            description: "New Description".into(),
        },
    );
    assert_eq!(refreshed.compaction_epoch, 4);
    assert_eq!(refreshed.profile_section, "profile-v2");
    assert_eq!(refreshed.system_identity.name, "New Name");

    let update = render_agent_profile_update(&AgentProfileIdentity {
        name: "New Name".into(),
        description: "New Description".into(),
    });
    assert!(update.starts_with(&format!(
        "{SAND_HIDDEN_PROMPT_MARKER}<<SAND_AGENT_PROFILE_UPDATE:v1:"
    )));
    assert!(update.contains("<agent_profile_update>"));
    assert!(update.contains("Current name: New Name"));
    assert!(update.contains("Current description: New Description"));
    assert_eq!(
        parse_latest_agent_profile_update(&update),
        Some(AgentProfileIdentity {
            name: "New Name".into(),
            description: "New Description".into(),
        })
    );

    let empty = render_agent_profile_update(&AgentProfileIdentity::default());
    assert!(empty.contains("Current name: (no name)"));
    assert!(empty.contains("Current description: (no description)"));

    let latest = format!(
        "{update}\nnoise\n{}",
        render_agent_profile_update(&AgentProfileIdentity {
            name: "  Latest  ".into(),
            description: "  Identity  ".into(),
        })
    );
    assert_eq!(
        parse_latest_agent_profile_update(&latest),
        Some(AgentProfileIdentity {
            name: "Latest".into(),
            description: "Identity".into(),
        })
    );

    let malformed = format!(
        "{update}\n<<SAND_AGENT_PROFILE_UPDATE:v1:not-base64>>"
    );
    assert_eq!(
        parse_latest_agent_profile_update(&malformed),
        Some(AgentProfileIdentity {
            name: "New Name".into(),
            description: "New Description".into(),
        })
    );
}

#[test]
fn bot_block_classifier_matches_grok_signatures_and_strips_query_fragments() {
    let sorry = classify_bot_block_page(
        "https://www.google.com/sorry/index?continue=secret#fragment",
        "anything",
    )
    .expect("google sorry signature");
    assert_eq!(sorry.family, "google_sorry");
    assert_eq!(sorry.confidence, BotBlockConfidence::High);
    assert_eq!(sorry.blocked_host, "google.com");
    assert_eq!(sorry.blocked_url, "https://www.google.com/sorry/index");

    let cloudflare = classify_bot_block_page(
        "https://example.com/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1",
        "Welcome",
    )
    .expect("cloudflare path signature");
    assert_eq!(cloudflare.family, "cloudflare_challenge");
    assert_eq!(cloudflare.confidence, BotBlockConfidence::High);

    let cloudflare_title = classify_bot_block_page(
        "https://example.com/home",
        "Just a moment...",
    )
    .expect("cloudflare title signature");
    assert_eq!(cloudflare_title.family, "cloudflare_challenge");

    let recaptcha = classify_bot_block_page(
        "https://example.com/recaptcha/api2/anchor?k=private",
        "",
    )
    .expect("recaptcha signature");
    assert_eq!(recaptcha.family, "recaptcha");
    assert_eq!(recaptcha.confidence, BotBlockConfidence::Low);
    assert!(!recaptcha.blocked_url.contains('?'));

    let linkedin = classify_bot_block_page(
        "https://www.linkedin.com/checkpoint/challenge/123?token=private",
        "",
    )
    .expect("linkedin signature");
    assert_eq!(linkedin.family, "linkedin_checkpoint");
    assert_eq!(linkedin.blocked_host, "linkedin.com");

    let generic = classify_bot_block_page(
        "https://safe.example/path",
        " Access Denied ",
    )
    .expect("generic access denied signature");
    assert_eq!(generic.family, "generic_access_denied");
    assert_eq!(generic.confidence, BotBlockConfidence::Low);

    assert!(classify_bot_block_page("not a URL", "Access Denied").is_none());
    assert!(classify_bot_block_page("https://example.com/", "Normal page").is_none());
}

#[test]
fn agent_profile_snapshot_struct_remains_cloneable_for_runner_state() {
    let snapshot = AgentProfilePromptSnapshot {
        version: 1,
        profile_section: "section".into(),
        system_identity: AgentProfileIdentity {
            name: "A".into(),
            description: "B".into(),
        },
        announced_identity: AgentProfileIdentity {
            name: "A".into(),
            description: "B".into(),
        },
        compaction_epoch: 9,
    };
    assert_eq!(snapshot.clone(), snapshot);
}
