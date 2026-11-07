use std::collections::HashSet;
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::content_search::agent_content_search::AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT;
use mahayana_host_runtime::extensions::content_search::search_index_db::{
    FTS_QUERY_MAX_TERMS, build_fts_match_query, ensure_search_index_schema, search_messages,
};
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::extensions::memory::agent_state::{
    MemoryScope, MemoryTier, SandAgentState,
};
use mahayana_host_runtime::extensions::memory::memory_service::{
    FileMemoryStore, MemoryKind, SynthesisApplyResult, SynthesisChange, UserMemoryRecall,
    UserMemoryStore, get_user_memory_shards_dir, parse_facts,
};
use mahayana_host_runtime::extensions::memory::memory_synthesis_service::{
    MemoryChange, uses_known_evidence,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::sand_memory::render_user_memory_system_prompt;
use mahayana_host_runtime::runner::tools::sand_state_tool::{
    SAND_UPDATE_STATE_TOOL_NAME, SandStateToolBridge, SandStateWriter,
};
use rusqlite::params;
use serde_json::{Value, json};

struct DelegateBridge;

impl RoutedToolBridge for DelegateBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![RoutedToolDefinition {
            name: "delegate_tool".into(),
            provider_identifier: "delegate".into(),
            tool_name: "delegate_tool".into(),
            description: None,
            input_schema: json!({"type":"object"}),
        }])
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Ok(Value::String("delegated".into()))
    }
}

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-openclaw-memory-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn test_ocrm_mem_o01_whitespace_normalization_and_case_insensitive_deduplication() {
    let root = temp_root("mem-o01");
    let mem_dir = root.join("memory");
    let store = FileMemoryStore::new(&mem_dir);

    let res1 = store
        .add_memory("  User   prefers dark mode  ", 1_700_000_000_000, MemoryKind::Profile)
        .expect("first add");
    assert!(res1.is_some(), "first memory write should succeed");
    let record1 = res1.unwrap();
    assert_eq!(record1.content, "User prefers dark mode");

    let res2 = store
        .add_memory("user PREFERS dark MODE", 1_700_001_000_000, MemoryKind::Profile)
        .expect("second add");
    assert!(
        res2.is_none(),
        "case-insensitive normalized duplicate should be discarded"
    );

    let profile_path = mem_dir.join("profile.md");
    let content = fs::read_to_string(&profile_path).expect("read profile.md");
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| line.starts_with("- ("))
        .collect();
    assert_eq!(lines.len(), 1, "profile.md must contain exactly 1 fact line");
    assert!(
        lines[0].ends_with("User prefers dark mode"),
        "line must preserve normalized casing"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_ocrm_mem_o02_markdown_fact_parser_strict_conformance() {
    let raw = r#"# About the user

- (2026-09-24) User likes green tea
- (2026-02-30) Invalid leap day fact
- (bad-date) Missing proper year
- Just a regular bullet point
"#;

    let facts = parse_facts(raw, MemoryKind::Profile);
    assert_eq!(
        facts.len(),
        1,
        "only the conforming fact line must be accepted"
    );
    assert_eq!(facts[0].date, "2026-09-24");
    assert_eq!(facts[0].content, "User likes green tea");
    assert_eq!(facts[0].kind, MemoryKind::Profile);
}

#[test]
fn test_ocrm_mem_o03_multi_agent_shard_isolation_single_writer() {
    let root = temp_root("mem-o03");
    let state_alpha = SandAgentState::new(&root, "agent-alpha").expect("state-alpha");
    let _state_beta = SandAgentState::new(&root, "agent-beta").expect("state-beta");

    let write_res = state_alpha.write_memory(
        "User lives in Tokyo",
        MemoryTier::Log,
        MemoryScope::User,
        None,
    );
    assert!(write_res.ok, "user memory write from agent-alpha should succeed");

    let shards_dir = get_user_memory_shards_dir(&root);
    let alpha_shard = shards_dir.join("agent-alpha");
    let beta_shard = shards_dir.join("agent-beta");

    assert!(alpha_shard.is_dir(), "agent-alpha shard must exist");
    assert!(
        !beta_shard.exists(),
        "agent-beta shard must remain unmodified and not created by alpha"
    );

    let user_store_beta = UserMemoryStore::new(
        &root,
        "agent-beta",
        Arc::new(|id| format!("assistant-{id}")),
    );
    let recall = user_store_beta.recall(10, 10);
    assert_eq!(
        recall.recent.len(),
        1,
        "agent-beta must recall shared user fact"
    );
    assert_eq!(recall.recent[0].agent_id, "agent-alpha");
    assert_eq!(recall.recent[0].memory.content, "User lives in Tokyo");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_ocrm_mem_o04_precedence_resolution_conflicting_memories() {
    let recall = UserMemoryRecall::default();
    let prompt = render_user_memory_system_prompt(
        &recall,
        Some("/sand/user-memory"),
        Some("/sand/user-memory/agents/agent-charlie"),
    );

    assert!(
        prompt.contains(
            "Precedence: when a shared user fact conflicts with your OWN memory, prefer your own"
        ),
        "system prompt must explicitly instruct agent precedence over shared defaults"
    );
    assert!(
        prompt.contains("split into one shard folder per assistant so every file has a single writer"),
        "system prompt must declare single-writer invariant"
    );
}

#[test]
fn test_ocrm_mem_o05_synthesis_optimistic_concurrency_control_cas_rejection() {
    let root = temp_root("mem-o05");
    let mem_dir = root.join("memory");
    let store = FileMemoryStore::new(&mem_dir);

    store
        .add_memory("Fact 1", 1_700_000_000_000, MemoryKind::Log)
        .expect("fact 1");
    let snapshot = store.prepare_synthesis();

    store
        .add_memory("Fact 2", 1_700_001_000_000, MemoryKind::Log)
        .expect("fact 2 external write");

    let changes = vec![SynthesisChange::Create {
        content: "Fact 3".into(),
        kind: MemoryKind::Log,
    }];
    let apply_res = store
        .apply_synthesis(&snapshot, &changes, 1_700_002_000_000)
        .expect("apply synthesis");
    assert_eq!(
        apply_res,
        SynthesisApplyResult::Stale,
        "synthesis with stale fingerprint must be rejected via CAS"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_ocrm_mem_o06_ungrounded_synthesis_evidence_rejection() {
    let known = HashSet::from(["ev-101".to_string(), "ev-102".to_string()]);
    let changes = vec![MemoryChange::Create {
        content: "User owns a Tesla".into(),
        kind: MemoryKind::Profile,
        source_evidence_ids: vec!["ev-999".into()],
    }];

    assert!(
        !uses_known_evidence(&known, &changes, false),
        "change with ungrounded source evidence must be rejected"
    );
}

#[test]
fn test_ocrm_mem_o07_clock_only_hallucination_fact_rejection() {
    let known = HashSet::from(["ev-101".to_string()]);
    let changes = vec![MemoryChange::Create {
        content: "Time is passing quickly".into(),
        kind: MemoryKind::Log,
        source_evidence_ids: vec!["clock".into()],
    }];

    assert!(
        !uses_known_evidence(&known, &changes, true),
        "fact creation citing only clock must be rejected as ungrounded hallucination"
    );
}

#[test]
fn test_ocrm_mem_o08_tombstone_persistence_and_resurrection_prevention() {
    let root = temp_root("mem-o08");
    let mem_dir = root.join("memory");
    let store = FileMemoryStore::with_synthesis_metadata(&mem_dir, true);

    store
        .add_memory("User dislikes cilantro", 1_700_000_000_000, MemoryKind::Profile)
        .expect("add fact");
    assert_eq!(store.count_memories(), 1);

    let removed = store
        .remove_memory_by_content("User dislikes cilantro")
        .expect("remove fact");
    assert!(removed, "removal must succeed");
    assert_eq!(store.count_memories(), 0);

    let tombstones_dir = mem_dir.join(".dreaming").join("tombstones");
    assert!(
        tombstones_dir.is_dir(),
        ".dreaming/tombstones must be created"
    );

    let snapshot = store.prepare_synthesis();
    let changes = vec![SynthesisChange::Create {
        content: "User dislikes cilantro".into(),
        kind: MemoryKind::Profile,
    }];
    let apply_res = store
        .apply_synthesis(&snapshot, &changes, 1_700_002_000_000)
        .expect("apply synthesis");
    assert_eq!(apply_res, SynthesisApplyResult::Committed);
    assert_eq!(
        store.count_memories(),
        0,
        "tombstoned fact must NOT be resurrected by synthesis"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_ocrm_mem_o09_fts5_query_unicode_nfkc_normalization_and_term_truncation() {
    let input = "“Ｆｕｌｌｗｉｄｔｈ” term1 term2 term3 term4 term5 term6 term7 term8 term9 term10";
    let match_query = build_fts_match_query(input).expect("match_query");

    let terms = match_query.split_whitespace().collect::<Vec<_>>();
    assert_eq!(
        terms.len(),
        FTS_QUERY_MAX_TERMS,
        "query terms must be capped at FTS_QUERY_MAX_TERMS (8)"
    );
    assert!(
        terms[0].contains("Fullwidth"),
        "fullwidth characters must be normalized to ASCII via NFKC"
    );
    assert!(
        !match_query.contains("term9"),
        "terms beyond 8 must be discarded"
    );
    assert!(
        !match_query.contains("term10"),
        "terms beyond 8 must be discarded"
    );
}

#[test]
fn test_ocrm_mem_o10_content_search_per_agent_capping_and_recency_sorting() {
    let db = rusqlite::Connection::open_in_memory().expect("sqlite memory db");
    ensure_search_index_schema(&db).expect("schema");

    // Insert 10 messages for agent-a with timestamps 1000..1009
    for i in 0..10 {
        db.execute(
            "INSERT INTO messages (agent_id, entry_id, role, timestamp_ms, body)
             VALUES (?1, ?2, 'user', ?3, ?4)",
            params![
                "agent-a",
                format!("entry-a-{i}"),
                1000 + i as i64,
                format!("deploy service version {i}")
            ],
        )
        .expect("insert agent-a");
    }

    // Insert 2 messages for agent-b with more recent timestamps 2000, 2001
    for i in 0..2 {
        db.execute(
            "INSERT INTO messages (agent_id, entry_id, role, timestamp_ms, body)
             VALUES (?1, ?2, 'user', ?3, ?4)",
            params![
                "agent-b",
                format!("entry-b-{i}"),
                2000 + i as i64,
                format!("deploy hotfix {i}")
            ],
        )
        .expect("insert agent-b");
    }

    let results = search_messages(&db, "deploy", 50).expect("search_messages");
    // Agent-b: 2 messages, Agent-a: capped at AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT (5)
    assert_eq!(
        results.len(),
        7,
        "expected 2 from agent-b and 5 (capped) from agent-a"
    );
    assert_eq!(results[0].agent_id, "agent-b");
    assert_eq!(results[1].agent_id, "agent-b");
    for item in &results[2..7] {
        assert_eq!(item.agent_id, "agent-a");
    }
    assert_eq!(AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT, 5);
}

#[test]
fn test_ocrm_mem_o11_project_scope_path_traversal_and_unjoined_membership_gate() {
    let root = temp_root("mem-o11");
    let state = Arc::new(SandAgentState::new(&root, "agent-1").expect("state"));

    let res1 = state.write_memory(
        "Secret",
        MemoryTier::Log,
        MemoryScope::Project,
        Some("../escaped"),
    );
    assert!(!res1.ok);
    assert_eq!(res1.message, "\"../escaped\" is not a valid project slug.");

    // Create project directory so it exists, but agent-1 has not joined it
    let secret_dir = root.join("projects").join("secret-proj");
    fs::create_dir_all(&secret_dir).expect("create secret-proj dir");

    let res2 = state.write_memory(
        "Secret",
        MemoryTier::Log,
        MemoryScope::Project,
        Some("secret-proj"),
    );
    assert!(!res2.ok);
    assert_eq!(
        res2.message,
        "you haven't joined project \"secret-proj\" yet."
    );

    let join_res = state.join_project("secret-proj");
    assert!(join_res.ok, "joining project should succeed");

    let res3 = state.write_memory(
        "Secret",
        MemoryTier::Log,
        MemoryScope::Project,
        Some("secret-proj"),
    );
    assert!(res3.ok, "writing memory after joining project should succeed");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_ocrm_mem_o12_memory_update_state_tool_contract() {
    let root = temp_root("mem-o12");
    let state = Arc::new(SandAgentState::new(&root, "agent-memory-tool").expect("state"));
    let writer: Arc<dyn SandStateWriter> = state;
    let bridge = SandStateToolBridge::new(Arc::new(DelegateBridge), writer);

    let tools = bridge.list_tools().expect("tools");
    let update_tool = tools
        .iter()
        .find(|t| t.name == SAND_UPDATE_STATE_TOOL_NAME)
        .expect("update_state tool");

    // 1. Write fact via update_state
    let write_res = bridge
        .call_tool(
            update_tool,
            json!({
                "target": "memory",
                "action": "write",
                "fact": "User is allergic to peanuts",
                "tier": "profile",
                "scope": "agent"
            }),
            "call-write-1",
        )
        .expect("call write");
    assert!(
        write_res
            .as_str()
            .unwrap_or_default()
            .contains("Remembered in your memory (profile): User is allergic to peanuts")
    );

    // 2. Forget fact via update_state
    let forget_res = bridge
        .call_tool(
            update_tool,
            json!({
                "target": "memory",
                "action": "forget",
                "fact": "User is allergic to peanuts",
                "scope": "agent"
            }),
            "call-forget-1",
        )
        .expect("call forget");
    assert!(
        forget_res
            .as_str()
            .unwrap_or_default()
            .contains("Forgot from your memory: User is allergic to peanuts")
    );

    let _ = fs::remove_dir_all(root);
}
