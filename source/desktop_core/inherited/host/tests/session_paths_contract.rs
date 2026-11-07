use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::session_paths::{
    ACTIVE_AGENT_FILENAME, CONNECTOR_SECRETS_DIRNAME, CONVERSATION_BLOBS_FILENAME,
    HIDDEN_ENTRY_REPAIR_VERSION, LEGACY_GROUP_MEMBERS_DIRNAME,
    SAND_CONVERSATION_ROOT_SLOT_ID, STALE_ROOT_CLEANUP_VERSION, STORE_FILENAME,
    get_agent_db_path, get_connector_secrets_root, get_sand_transcripts_dir_with,
    is_stale_root_gc_enabled_from_env, pin_stale_root_gc, stat_if_exists,
    is_root_main_session_key, reject_public_agent_harness_session_key,
    resolve_public_session_route,
    PublicSessionRouteError, SessionKind,
};

#[test]
fn session_paths_match_frozen_names_root_slot_and_gc_override_contract() {
    assert_eq!(STORE_FILENAME, "store.db");
    assert_eq!(CONVERSATION_BLOBS_FILENAME, "conversation-blobs.db");
    assert_eq!(
        SAND_CONVERSATION_ROOT_SLOT_ID,
        b"sand-live-conversation-root-v1__"
    );
    assert_eq!(STALE_ROOT_CLEANUP_VERSION, 1);
    assert_eq!(ACTIVE_AGENT_FILENAME, "active-agent.json");
    assert_eq!(HIDDEN_ENTRY_REPAIR_VERSION, 1);
    assert_eq!(LEGACY_GROUP_MEMBERS_DIRNAME, "members");
    assert_eq!(CONNECTOR_SECRETS_DIRNAME, "connector-secrets");

    let mut environment = BTreeMap::new();
    pin_stale_root_gc(false);
    assert!(!is_stale_root_gc_enabled_from_env(&environment));
    pin_stale_root_gc(true);
    assert!(is_stale_root_gc_enabled_from_env(&environment));

    environment.insert("SAND_STALE_ROOT_GC".into(), " off ".into());
    assert!(!is_stale_root_gc_enabled_from_env(&environment));
    environment.insert("SAND_STALE_ROOT_GC".into(), "TRUE".into());
    assert!(is_stale_root_gc_enabled_from_env(&environment));
    pin_stale_root_gc(false);
}

#[test]
fn session_paths_validate_agent_ids_and_preserve_connector_secret_sibling_layout() {
    let agents_root = Path::new("/tmp/fabushi-session-root/agents");
    assert_eq!(
        get_agent_db_path(agents_root, "agent-a").expect("valid agent path"),
        agents_root.join("agent-a").join(STORE_FILENAME)
    );
    assert!(get_agent_db_path(agents_root, "../escape").is_err());
    assert_eq!(
        get_connector_secrets_root(Some(agents_root)),
        Path::new("/tmp/fabushi-session-root").join(CONNECTOR_SECRETS_DIRNAME)
    );
}

#[test]
fn transcript_path_uses_the_same_sand_root_resolution_as_the_host() {
    let home = Path::new("/home/tester");
    let cwd = Path::new("/workspace");
    let argv = vec!["mahayana-app-host".to_string()];
    let environment = BTreeMap::from([(
        "SAND_DATA_ROOT".to_string(),
        "/var/lib/fabushi".to_string(),
    )]);
    assert_eq!(
        get_sand_transcripts_dir_with(home, &argv, &environment, cwd),
        Path::new("/var/lib/fabushi").join("agent-transcripts")
    );
}

#[test]
fn stat_if_exists_matches_the_frozen_missing_file_contract() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-session-paths-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("fixture root");
    let present = root.join("store.db");
    fs::write(&present, b"db").expect("fixture file");

    assert!(stat_if_exists(&present).is_some());
    assert!(stat_if_exists(&root.join("missing.db")).is_none());

    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn public_session_commands_reject_harness_owned_namespace() {
    assert!(reject_public_agent_harness_session_key("agent:main:main").is_ok());
    assert_eq!(
        reject_public_agent_harness_session_key("agent:main:harness:codex:run"),
        Err("Session key namespace is reserved for agent harness-owned sessions.")
    );
}

#[test]
fn public_session_route_resolves_one_canonical_boundary() {
    let route = resolve_public_session_route(
        " agent:agent-a:group:main ",
        Some("group"),
        None,
    )
    .expect("canonical route");
    assert_eq!(route.raw_key, "agent:agent-a:group:main");
    assert_eq!(route.agent_id, "agent-a");
    assert_eq!(route.kind, SessionKind::Group);
    let mixed_case_route = resolve_public_session_route("AGENT:Agent-A:Main", None, None)
        .expect("mixed-case canonical route");
    assert_eq!(mixed_case_route.agent_id, "agent-a");
    assert_eq!(
        resolve_public_session_route("agent:agent-a:harness:child", None, None),
        Err(PublicSessionRouteError::HarnessNamespaceReserved)
    );
    assert_eq!(
        resolve_public_session_route("agent::main", None, None),
        Err(PublicSessionRouteError::EmptyAgentIdentity)
    );
    assert_eq!(
        resolve_public_session_route("   ", None, None),
        Err(PublicSessionRouteError::EmptySelector)
    );
    let unicode_selector = resolve_public_session_route("用户会话", None, None)
        .expect("non-agent UTF-8 selectors remain safe legacy identities");
    assert_eq!(unicode_selector.agent_id, "用户会话");
}

#[test]
fn root_main_session_guard_is_case_insensitive_only_for_structural_root_keys() {
    assert!(is_root_main_session_key("global"));
    assert!(is_root_main_session_key(" GLOBAL "));
    assert!(is_root_main_session_key("AGENT:GLOBAL:MAIN"));
    assert!(!is_root_main_session_key("agent:global:main:child"));
    assert!(!is_root_main_session_key("global-user"));
    assert!(!is_root_main_session_key("legacy-session"));
}
