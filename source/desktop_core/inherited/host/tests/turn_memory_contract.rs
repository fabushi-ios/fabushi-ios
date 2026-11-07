use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::agent_db_serde::EpisodeTurn;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::runner::sand_memory::{
    MEMORY_EPISODE_PREFIX, MEMORY_EPISODE_PROMPT_MARKER,
    MEMORY_EXTRACTION_PROMPT_MARKER,
};
use mahayana_host_runtime::runner::turn_memory::{
    TurnExchange, TurnMemoryMode, build_turn_memory_exchange, run_turn_memory_with,
};

fn root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-turn-memory-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn completed_memorable_turns_extract_memory_and_roll_episode_summary() {
    let root = root("extract");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let session = workers
        .materialize_session_with_active(None, "user", None, None)
        .expect("session");
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));

    for index in 0..6 {
        let calls_for_turn = Arc::clone(&calls);
        let report = run_turn_memory_with(
            &session.memory,
            Some(session.db.as_ref()),
            1_900_000_000_000 + index,
            TurnExchange {
                user: format!("Continue the release plan with decision {index}."),
                agent: format!("Recorded decision {index}."),
            },
            TurnMemoryMode::Extract,
            move |system, _user| {
                calls_for_turn.lock().unwrap().push(system.to_string());
                if system.contains(MEMORY_EXTRACTION_PROMPT_MARKER) {
                    Ok("profile: User prefers concise release updates".into())
                } else if system.contains(MEMORY_EPISODE_PROMPT_MARKER) {
                    Ok("On 2030-03-17 the user and assistant advanced the release plan.".into())
                } else {
                    Err("unexpected prompt".into())
                }
            },
        );
        assert!(report.extraction_attempted);
        assert!(report.episode_recorded);
    }

    let memories = session.memory.list_memories(100);
    assert!(memories.iter().any(|memory| {
        memory.content == "User prefers concise release updates"
    }));
    assert!(memories.iter().any(|memory| {
        memory.content.starts_with(MEMORY_EPISODE_PREFIX)
    }));
    assert!(session.db.get_pending_episode_turns().unwrap().is_empty());
    assert_eq!(
        calls.lock().unwrap()
            .iter()
            .filter(|prompt| prompt.contains(MEMORY_EPISODE_PROMPT_MARKER))
            .count(),
        1
    );

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn evidence_mode_clears_pending_episode_state_without_running_extraction() {
    let root = root("evidence");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let session = workers
        .materialize_session_with_active(None, "user", None, None)
        .expect("session");
    session
        .db
        .record_episode_turn(&EpisodeTurn {
            ts: 1.0,
            user: "old user".into(),
            agent: "old agent".into(),
        })
        .unwrap();

    let seen = Arc::new(Mutex::new(Vec::<TurnExchange>::new()));
    let seen_recorder = Arc::clone(&seen);
    let mut recorder = move |exchange: &TurnExchange| {
        seen_recorder.lock().unwrap().push(exchange.clone());
    };
    let report = run_turn_memory_with(
        &session.memory,
        Some(session.db.as_ref()),
        2,
        TurnExchange {
            user: "new user".into(),
            agent: "new agent".into(),
        },
        TurnMemoryMode::RecordEvidence(&mut recorder),
        |_system, _user| -> Result<String, String> {
            panic!("evidence mode must not run extraction")
        },
    );

    assert!(report.evidence_recorded);
    assert!(!report.extraction_attempted);
    assert!(session.db.get_pending_episode_turns().unwrap().is_empty());
    assert_eq!(seen.lock().unwrap().len(), 1);

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn frozen_turn_memory_exchange_keeps_agent_messages_before_final_assistant_text() {
    let exchange = build_turn_memory_exchange(
        "user prompt",
        ["sent to peer", "   ", "second peer note"],
        "final assistant reply",
    );
    assert_eq!(exchange.user, "user prompt");
    assert_eq!(
        exchange.agent,
        "sent to peer\nsecond peer note\nfinal assistant reply"
    );

    let tool_only = build_turn_memory_exchange(
        "tool-only prompt",
        ["delegated work"],
        "   ",
    );
    assert_eq!(tool_only.agent, "delegated work");
}
