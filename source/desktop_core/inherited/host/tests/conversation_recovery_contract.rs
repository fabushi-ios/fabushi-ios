use std::collections::{HashMap, HashSet};

use mahayana_host_runtime::extensions::session::conversation_recovery::{
    ConversationStructureRefs, OutlineItem, RootScore, conversation_structure_fully_resolves,
    find_latest_root_blob_id_in_database, is_better_root, parse_conversation_state_structure,
    rebuild_transcript_entries_from_state, score_root_candidate,
    select_hidden_artifact_entry_ids,
};
use rusqlite::params;
use serde_json::json;

fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn push_length_delimited(field: u64, value: &[u8], output: &mut Vec<u8>) {
    encode_varint((field << 3) | 2, output);
    encode_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn root_bytes(turns: &[Vec<u8>], prompts: usize) -> Vec<u8> {
    let mut output = Vec::new();
    for index in 0..prompts {
        push_length_delimited(1, format!("prompt-{index}").as_bytes(), &mut output);
    }
    for turn in turns {
        push_length_delimited(8, turn, &mut output);
    }
    output
}

#[test]
fn rebuild_and_hidden_artifact_selection_match_frozen_recovery_contract() {
    let turns = vec![vec![
        OutlineItem::User {
            id: "u1".into(),
            hidden: false,
            text: "hello".into(),
            timestamp_ms: Some(10.0),
        },
        OutlineItem::User {
            id: "hidden".into(),
            hidden: true,
            text: "secret".into(),
            timestamp_ms: None,
        },
        OutlineItem::SendMessage {
            id: "s1".into(),
            message: json!({"type":"text","content":"send"}),
            timestamp_ms: None,
        },
        OutlineItem::ToolCall {
            id: "t1".into(),
            name: "search".into(),
            status: "done".into(),
            summary: Some("ok".into()),
            timestamp_ms: Some(20.0),
        },
    ]];

    let entries = rebuild_transcript_entries_from_state(&turns);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["id"], "recovered-u1");
    assert_eq!(entries[0]["role"], "user");
    assert_eq!(entries[1]["kind"], "send-message");
    assert_eq!(entries[2]["summary"], "ok");

    let persisted = vec![
        json!({"kind":"message","id":"recovered-hidden","role":"user","content":"secret"}),
        json!({"kind":"message","id":"recovered-u1","role":"user","content":"hello"}),
        json!({"kind":"notice","id":"recovered-hidden","text":"secret"}),
    ];
    assert_eq!(
        select_hidden_artifact_entry_ids(&persisted, &turns[0]),
        vec!["recovered-hidden".to_string()]
    );
}

#[test]
fn structure_resolution_requires_every_referenced_blob_and_rejects_oversize_data() {
    let structure = ConversationStructureRefs {
        turns: vec![vec![1], vec![2]],
        todos: vec![vec![3]],
        summary: Some(vec![4]),
    };
    let blobs = HashMap::from([
        (vec![1], vec![1]),
        (vec![2], vec![2]),
        (vec![3], vec![3]),
        (vec![4], vec![4]),
    ]);
    assert!(futures::executor::block_on(conversation_structure_fully_resolves(
        &structure,
        |id| {
            let value = blobs.get(&id).cloned();
            async move { Ok::<_, ()>(value) }
        },
    )));

    assert!(!futures::executor::block_on(conversation_structure_fully_resolves(
        &ConversationStructureRefs {
            turns: vec![vec![9]],
            todos: vec![],
            summary: None,
        },
        |_id| async { Ok::<_, ()>(None) },
    )));
    assert!(!futures::executor::block_on(conversation_structure_fully_resolves(
        &ConversationStructureRefs::default(),
        |_id| async { Ok::<_, ()>(Some(vec![])) },
    )));
}

#[test]
fn root_candidate_scoring_matches_turn_prompt_and_size_ordering() {
    let turn_a = vec![0x11; 32];
    let turn_b = vec![0x22; 32];
    let present = HashSet::from([
        turn_a.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        turn_b.iter().map(|b| format!("{b:02x}")).collect::<String>(),
    ]);
    let one_turn = root_bytes(std::slice::from_ref(&turn_a), 3);
    let two_turns = root_bytes(&[turn_a.clone(), turn_b.clone()], 1);
    let parsed = parse_conversation_state_structure(&two_turns).expect("parse root");
    assert_eq!(parsed.turns, vec![turn_a.clone(), turn_b.clone()]);
    assert_eq!(parsed.root_prompts, 1);

    let first = score_root_candidate(&one_turn, &present).expect("one-turn score");
    let second = score_root_candidate(&two_turns, &present).expect("two-turn score");
    assert!(is_better_root(second, Some(first)));
    assert!(!is_better_root(
        RootScore { turns: 1, root_prompts: 1, bytes: 1 },
        Some(RootScore { turns: 1, root_prompts: 2, bytes: 1 }),
    ));

    let unresolved = root_bytes(&[vec![0x33; 32]], 9);
    assert!(score_root_candidate(&unresolved, &present).is_none());
}

#[test]
fn database_root_scan_is_the_production_scoring_boundary() {
    let db = rusqlite::Connection::open_in_memory().expect("db");
    db.execute_batch("CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB NOT NULL) STRICT;")
        .expect("schema");

    let turn_a = vec![0x11; 32];
    let turn_b = vec![0x22; 32];
    for (id, data) in [
        (
            turn_a.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            b"turn-a".to_vec(),
        ),
        (
            turn_b.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            b"turn-b".to_vec(),
        ),
    ] {
        db.execute("INSERT INTO blobs (id, data) VALUES (?1, ?2)", params![id, data])
            .expect("turn");
    }

    let weak = root_bytes(std::slice::from_ref(&turn_a), 5);
    let strong = root_bytes(&[turn_a, turn_b], 1);
    let weak_id = "aa".repeat(32);
    let strong_id = "bb".repeat(32);
    db.execute("INSERT INTO blobs (id, data) VALUES (?1, ?2)", params![weak_id, weak])
        .expect("weak root");
    db.execute("INSERT INTO blobs (id, data) VALUES (?1, ?2)", params![strong_id, strong])
        .expect("strong root");

    assert_eq!(
        find_latest_root_blob_id_in_database(&db).expect("scan"),
        Some(vec![0xbb; 32])
    );
}
