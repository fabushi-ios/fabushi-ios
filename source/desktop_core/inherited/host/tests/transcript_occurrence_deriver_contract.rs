use std::collections::HashMap;

use mahayana_host_runtime::transcript_mirror::transcript_journal_codec::{
    DeferredTranscriptStep, TranscriptCheckpoint,
};
use mahayana_host_runtime::transcript_mirror::transcript_mirror::TranscriptDeriver;
use mahayana_host_runtime::transcript_mirror::transcript_occurrence_deriver::{
    ArtifactTranscriptOccurrenceDeriver, DecodedTranscriptStep, DecodedTranscriptTurn,
    DecodedUserMessage, TranscriptOccurrenceBlobStore, TranscriptOccurrenceCodec,
    strip_context_tags, strip_hidden_thinking_tags,
};
use serde_json::{Value, json};

#[derive(Default)]
struct FakeStore {
    blobs: HashMap<Vec<u8>, Vec<u8>>,
}

impl FakeStore {
    fn with(mut self, id: u8, value: Value) -> Self {
        self.blobs
            .insert(vec![id], serde_json::to_vec(&value).expect("blob"));
        self
    }

    fn with_bytes(mut self, id: u8, value: &[u8]) -> Self {
        self.blobs.insert(vec![id], value.to_vec());
        self
    }
}

impl TranscriptOccurrenceBlobStore for FakeStore {
    fn get_blob(&self, id: &[u8]) -> Result<Option<Vec<u8>>, String> {
        Ok(self.blobs.get(id).cloned())
    }
}

struct FakeCodec;

impl TranscriptOccurrenceCodec for FakeCodec {
    fn decode_turn(&self, bytes: &[u8]) -> Result<DecodedTranscriptTurn, String> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        match value.get("case").and_then(Value::as_str) {
            Some("agent") => Ok(DecodedTranscriptTurn::Agent {
                user_message: vec![
                    value
                        .get("user")
                        .and_then(Value::as_u64)
                        .unwrap_or_default() as u8,
                ],
                steps: value
                    .get("steps")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .map(|value| vec![value as u8])
                    .collect(),
            }),
            Some("shell") => Ok(DecodedTranscriptTurn::Shell),
            _ => Ok(DecodedTranscriptTurn::Undefined),
        }
    }

    fn decode_user_message(&self, bytes: &[u8]) -> Result<DecodedUserMessage, String> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        Ok(DecodedUserMessage {
            text: value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            text_blob_id: value
                .get("textBlob")
                .and_then(Value::as_u64)
                .map(|value| vec![value as u8]),
        })
    }

    fn decode_step(&self, bytes: &[u8]) -> Result<DecodedTranscriptStep, String> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        match value.get("case").and_then(Value::as_str) {
            Some("assistant") => Ok(DecodedTranscriptStep::Assistant {
                text: value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            Some("thinking") => Ok(DecodedTranscriptStep::Thinking {
                text: value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            Some("tool") => Ok(DecodedTranscriptStep::Tool {
                name: value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: value.get("input").cloned().unwrap_or_else(|| json!({})),
                result: value.get("result").cloned(),
            }),
            _ => Ok(DecodedTranscriptStep::Undefined),
        }
    }
}

fn parsed_lines(
    entries: &[mahayana_host_runtime::transcript_mirror::transcript_mirror::TranscriptOccurrence],
) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| serde_json::from_str(&entry.line).expect("line json"))
        .collect()
}

#[test]
fn strips_frozen_context_and_hidden_thinking_tags() {
    assert_eq!(
        strip_context_tags(
            "hello\n<user_info id=\"1\">secret</user_info>\n\n\n<git_status>x</git_status>\nworld"
        ),
        "hello\n\nworld"
    );
    assert_eq!(
        strip_hidden_thinking_tags(
            "<think>private</think>visible\n\n\n<thinking>hidden</thinking>end"
        ),
        "visible\n\nend"
    );
}

#[test]
fn initial_projection_emits_user_assistant_and_tool_occurrences() {
    let store = FakeStore::default()
        .with(1, json!({"case":"agent","user":2,"steps":[3,4,5]}))
        .with(2, json!({"text":"question\n<rules>hidden</rules>\nvisible"}))
        .with(3, json!({"case":"assistant","text":"<think>hidden</think>answer"}))
        .with(4, json!({"case":"tool","name":"search","input":{"q":"grok"}}))
        .with(
            5,
            json!({"case":"tool","name":"read","input":{"path":"a"},"result":{"ok":true}}),
        );
    let deriver = ArtifactTranscriptOccurrenceDeriver::new(FakeCodec);
    let entries = deriver
        .initial(&store, &TranscriptCheckpoint { turns: vec![vec![1]] })
        .expect("initial");
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "turn:0:user",
            "turn:0:step:0:text",
            "turn:0:step:1:tool-use",
            "turn:0:step:2:tool-use",
            "turn:0:step:2:tool-result",
        ]
    );
    let lines = parsed_lines(&entries);
    assert_eq!(
        lines[0]["message"]["content"][0]["text"],
        "question\n\nvisible"
    );
    assert_eq!(lines[1]["message"]["content"][0]["text"], "answer");
    assert_eq!(lines[2]["message"]["content"][0]["name"], "search");
    assert_eq!(lines[4]["message"]["content"][0]["result"]["ok"], true);
}

#[test]
fn user_message_blob_fallback_uses_text_decoder_replacement_semantics() {
    let store = FakeStore::default()
        .with(1, json!({"case":"agent","user":2,"steps":[]}))
        .with(2, json!({"text":"","textBlob":9}))
        .with_bytes(9, &[b'a', 0xff, b'b']);
    let deriver = ArtifactTranscriptOccurrenceDeriver::new(FakeCodec);
    let entries = deriver
        .initial(&store, &TranscriptCheckpoint { turns: vec![vec![1]] })
        .expect("initial");
    let line: Value = serde_json::from_str(&entries[0].line).expect("line");
    assert_eq!(line["message"]["content"][0]["text"], "a\u{fffd}b");
}

#[test]
fn final_assistant_step_is_deferred_until_checkpoint_finalization() {
    let store = FakeStore::default()
        .with(1, json!({"case":"agent","user":2,"steps":[3]}))
        .with(2, json!({"text":"question"}))
        .with(3, json!({"case":"assistant","text":"streaming answer"}));
    let deriver = ArtifactTranscriptOccurrenceDeriver::new(FakeCodec);
    let previous = TranscriptCheckpoint { turns: vec![] };
    let current = TranscriptCheckpoint { turns: vec![vec![1]] };

    let streaming = deriver
        .derive(&store, &previous, &current, false, None)
        .expect("streaming");
    assert_eq!(streaming.occurrences.len(), 1);
    assert_eq!(
        streaming.deferred_step,
        Some(DeferredTranscriptStep {
            turn_index: 0,
            step_index: 0,
        })
    );

    let finalized = deriver
        .derive(
            &store,
            &current,
            &current,
            true,
            streaming.deferred_step,
        )
        .expect("finalized");
    assert_eq!(
        finalized
            .occurrences
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["turn:0:step:0:text"]
    );
    assert_eq!(finalized.deferred_step, None);
}

#[test]
fn completed_tool_only_emits_new_result_and_rejects_mutated_input() {
    let store = FakeStore::default()
        .with(1, json!({"case":"agent","user":2,"steps":[3]}))
        .with(4, json!({"case":"agent","user":2,"steps":[5]}))
        .with(2, json!({"text":"question"}))
        .with(3, json!({"case":"tool","name":"search","input":{"q":"grok"}}))
        .with(
            5,
            json!({"case":"tool","name":"search","input":{"q":"grok"},"result":{"count":3}}),
        );
    let deriver = ArtifactTranscriptOccurrenceDeriver::new(FakeCodec);
    let previous = TranscriptCheckpoint { turns: vec![vec![1]] };
    let current = TranscriptCheckpoint { turns: vec![vec![4]] };

    let derived = deriver
        .derive(&store, &previous, &current, true, None)
        .expect("tool result");
    assert_eq!(
        derived
            .occurrences
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["turn:0:step:0:tool-result"]
    );

    let mutated = FakeStore::default()
        .with(1, json!({"case":"agent","user":2,"steps":[3]}))
        .with(6, json!({"case":"agent","user":2,"steps":[7]}))
        .with(2, json!({"text":"question"}))
        .with(3, json!({"case":"tool","name":"search","input":{"q":"grok"}}))
        .with(
            7,
            json!({"case":"tool","name":"search","input":{"q":"changed"},"result":{"count":3}}),
        );
    let error = deriver
        .derive(
            &mutated,
            &previous,
            &TranscriptCheckpoint { turns: vec![vec![6]] },
            true,
            None,
        )
        .expect_err("mutated input");
    assert!(error.contains("durable tool call changed"));
}

#[test]
fn non_tail_history_mutation_and_missing_blob_fail_closed() {
    let store = FakeStore::default()
        .with(1, json!({"case":"agent","user":2,"steps":[3,4]}))
        .with(5, json!({"case":"agent","user":2,"steps":[6,4]}))
        .with(2, json!({"text":"question"}))
        .with(3, json!({"case":"assistant","text":"a"}))
        .with(4, json!({"case":"assistant","text":"b"}))
        .with(6, json!({"case":"assistant","text":"changed"}));
    let deriver = ArtifactTranscriptOccurrenceDeriver::new(FakeCodec);
    let error = deriver
        .derive(
            &store,
            &TranscriptCheckpoint { turns: vec![vec![1]] },
            &TranscriptCheckpoint { turns: vec![vec![5]] },
            true,
            None,
        )
        .expect_err("history mutation");
    assert!(error.contains("before the checkpoint tail"));

    let missing = deriver
        .initial(
            &FakeStore::default(),
            &TranscriptCheckpoint { turns: vec![vec![99]] },
        )
        .expect_err("missing blob");
    assert!(missing.contains("missing conversation-turn blob"));
}
