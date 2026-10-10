use crate::transcript_journal_codec::{
    DeferredTranscriptStep, TranscriptCheckpoint, TranscriptJournalCorruptionError, bytes_equal,
    format_tool_line,
};
use serde_json::{Value, json};

pub trait TranscriptOccurrenceBlobStore {
    fn get_blob(&self, id: &[u8]) -> Option<Vec<u8>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedTranscriptTurn {
    Agent {
        user_message: Vec<u8>,
        steps: Vec<Vec<u8>>,
    },
    Shell,
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodedTranscriptStep {
    Assistant {
        text: String,
    },
    Thinking {
        text: String,
    },
    Tool {
        name: String,
        input: Value,
        result: Option<Value>,
    },
    Empty,
}

pub trait TranscriptOccurrenceCodec {
    fn decode_turn(&self, bytes: &[u8]) -> Result<DecodedTranscriptTurn, String>;
    fn decode_user_message(&self, bytes: &[u8]) -> Result<DecodedUserMessage, String>;
    fn decode_step(&self, bytes: &[u8]) -> Result<DecodedTranscriptStep, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedUserMessage {
    pub text: String,
    pub text_blob_id: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptOccurrence {
    pub id: String,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DerivedTranscriptOccurrences {
    pub occurrences: Vec<TranscriptOccurrence>,
    pub deferred_step: Option<DeferredTranscriptStep>,
}

pub trait TranscriptDeriver<Store: TranscriptOccurrenceBlobStore> {
    fn initial(
        &self,
        store: &Store,
        checkpoint: &TranscriptCheckpoint,
    ) -> Result<Vec<TranscriptOccurrence>, TranscriptJournalCorruptionError>;

    fn derive(
        &self,
        store: &Store,
        previous: &TranscriptCheckpoint,
        checkpoint: &TranscriptCheckpoint,
        finalize_checkpoint: bool,
        deferred: Option<DeferredTranscriptStep>,
    ) -> Result<DerivedTranscriptOccurrences, TranscriptJournalCorruptionError>;
}

fn corruption(message: impl Into<String>) -> TranscriptJournalCorruptionError {
    TranscriptJournalCorruptionError(message.into())
}

fn required_blob<Store: TranscriptOccurrenceBlobStore>(
    store: &Store,
    id: &[u8],
    label: &str,
) -> Result<Vec<u8>, TranscriptJournalCorruptionError> {
    store
        .get_blob(id)
        .ok_or_else(|| corruption(format!("missing {label} blob while deriving transcript checkpoint")))
}

pub(crate) fn strip_context_tags(text: &str) -> String {
    crate::package_agent_transcript_context_stripping::strip_context_tags(text)
}

pub(crate) fn strip_hidden_thinking_tags(text: &str) -> String {
    crate::package_agent_transcript_context_stripping::strip_tags(text, &["think", "thinking"])
}

fn format_text_occurrence(role: &str, text: &str) -> Option<String> {
    let visible = match role {
        "user" => strip_context_tags(text),
        "assistant" => strip_hidden_thinking_tags(text),
        _ => return None,
    };
    if visible.trim().is_empty() {
        return None;
    }
    Some(
        json!({
            "role": role,
            "message": {"content": [{"type": "text", "text": visible}]}
        })
        .to_string(),
    )
}

fn same_tool_input(left: &Value, right: &Value) -> bool {
    left == right
}

pub struct ArtifactTranscriptOccurrenceDeriver<Codec> {
    codec: Codec,
}

impl<Codec: TranscriptOccurrenceCodec> ArtifactTranscriptOccurrenceDeriver<Codec> {
    pub fn new(codec: Codec) -> Self {
        Self { codec }
    }

    fn decode_turn(&self, bytes: &[u8]) -> Result<DecodedTranscriptTurn, TranscriptJournalCorruptionError> {
        self.codec
            .decode_turn(bytes)
            .map_err(|error| corruption(format!("generated turn decode failed: {error}")))
    }

    fn decode_user_message(
        &self,
        bytes: &[u8],
    ) -> Result<DecodedUserMessage, TranscriptJournalCorruptionError> {
        self.codec
            .decode_user_message(bytes)
            .map_err(|error| corruption(format!("generated user-message decode failed: {error}")))
    }

    fn decode_step(&self, bytes: &[u8]) -> Result<DecodedTranscriptStep, TranscriptJournalCorruptionError> {
        self.codec
            .decode_step(bytes)
            .map_err(|error| corruption(format!("generated step decode failed: {error}")))
    }

    fn derive_turn<Store: TranscriptOccurrenceBlobStore>(
        &self,
        store: &Store,
        turn_index: usize,
        current_blob_id: &[u8],
        previous_blob_id: Option<&[u8]>,
        finalize_turn: bool,
        deferred_step: Option<DeferredTranscriptStep>,
    ) -> Result<DerivedTranscriptOccurrences, TranscriptJournalCorruptionError> {
        let current = self.decode_turn(&required_blob(
            store,
            current_blob_id,
            "conversation-turn",
        )?)?;
        if current == DecodedTranscriptTurn::Shell {
            return Err(corruption("Sand does not support shell conversation turns"));
        }

        let previous = match previous_blob_id {
            Some(id) => Some(self.decode_turn(&required_blob(
                store,
                id,
                "previous conversation-turn",
            )?)?),
            None => None,
        };
        if previous.as_ref().is_some_and(|value| {
            std::mem::discriminant(value) != std::mem::discriminant(&current)
        }) {
            return Err(corruption("durable conversation turn changed kind"));
        }

        let DecodedTranscriptTurn::Agent {
            user_message,
            steps,
        } = current
        else {
            return Ok(DerivedTranscriptOccurrences::default());
        };
        let previous_agent = match previous {
            Some(DecodedTranscriptTurn::Agent {
                user_message,
                steps,
            }) => Some((user_message, steps)),
            _ => None,
        };

        if let Some((previous_user, previous_steps)) = &previous_agent {
            if !bytes_equal(previous_user, &user_message) {
                return Err(corruption(
                    "durable agent user message changed after checkpoint",
                ));
            }
            if steps.len() < previous_steps.len() {
                return Err(corruption("durable agent steps moved backwards"));
            }
        }

        let mut first_changed_step = previous_agent
            .as_ref()
            .map(|(_, steps)| steps.len())
            .unwrap_or(0);
        if let Some((_, previous_steps)) = &previous_agent {
            for (index, previous_step) in previous_steps.iter().enumerate() {
                if bytes_equal(previous_step, &steps[index]) {
                    continue;
                }
                if index + 1 != previous_steps.len() {
                    return Err(corruption(
                        "durable agent step changed before the checkpoint tail",
                    ));
                }
                first_changed_step = index;
                break;
            }
        }
        if deferred_step.is_some_and(|step| step.turn_index == turn_index) {
            first_changed_step = first_changed_step.min(deferred_step.unwrap().step_index);
        }

        let mut occurrences = Vec::new();
        if previous_agent.is_none() {
            let user = self.decode_user_message(&required_blob(
                store,
                &user_message,
                "user-message",
            )?)?;
            let text = if !user.text.is_empty()
                || user
                    .text_blob_id
                    .as_ref()
                    .map_or(true, |blob_id| blob_id.is_empty())
            {
                user.text
            } else {
                String::from_utf8_lossy(&required_blob(
                    store,
                    user.text_blob_id.as_deref().unwrap_or_default(),
                    "user-message text",
                )?)
                .into_owned()
            };
            if let Some(line) = format_text_occurrence("user", &text) {
                occurrences.push(TranscriptOccurrence {
                    id: format!("turn:{turn_index}:user"),
                    line,
                });
            }
        }

        for step_index in first_changed_step..steps.len() {
            let previous_step = match previous_agent
                .as_ref()
                .and_then(|(_, previous_steps)| previous_steps.get(step_index))
            {
                Some(blob_id) => Some(self.decode_step(&required_blob(
                    store,
                    blob_id,
                    "previous conversation-step",
                )?)?),
                None => None,
            };
            let step = self.decode_step(&required_blob(
                store,
                &steps[step_index],
                "conversation-step",
            )?)?;
            if previous_step.as_ref().is_some_and(|value| {
                std::mem::discriminant(value) != std::mem::discriminant(&step)
            }) {
                return Err(corruption("durable conversation step changed kind"));
            }

            if !finalize_turn
                && step_index + 1 == steps.len()
                && matches!(
                    step,
                    DecodedTranscriptStep::Assistant { .. }
                        | DecodedTranscriptStep::Thinking { .. }
                )
            {
                return Ok(DerivedTranscriptOccurrences {
                    occurrences,
                    deferred_step: Some(DeferredTranscriptStep {
                        turn_index,
                        step_index,
                    }),
                });
            }

            match step {
                DecodedTranscriptStep::Assistant { text }
                | DecodedTranscriptStep::Thinking { text } => {
                    if let Some(line) = format_text_occurrence("assistant", &text) {
                        occurrences.push(TranscriptOccurrence {
                            id: format!("turn:{turn_index}:step:{step_index}:text"),
                            line,
                        });
                    }
                }
                DecodedTranscriptStep::Tool {
                    name,
                    input,
                    result,
                } => {
                    match previous_step {
                        Some(DecodedTranscriptStep::Tool {
                            name: previous_name,
                            input: previous_input,
                            result: previous_result,
                        }) => {
                            if previous_name != name
                                || !same_tool_input(&previous_input, &input)
                            {
                                return Err(corruption(
                                    "durable tool call changed after checkpoint",
                                ));
                            }
                            if previous_result.is_some() {
                                return Err(corruption(
                                    "completed durable tool call changed after checkpoint",
                                ));
                            }
                            if let Some(result) = result {
                                occurrences.push(TranscriptOccurrence {
                                    id: format!(
                                        "turn:{turn_index}:step:{step_index}:tool-result"
                                    ),
                                    line: format_tool_line("tool", &name, result)
                                        .ok_or_else(|| corruption("invalid tool result role"))?,
                                });
                            }
                        }
                        Some(_) => {
                            return Err(corruption(
                                "durable conversation step changed into a tool call",
                            ));
                        }
                        None => {
                            occurrences.push(TranscriptOccurrence {
                                id: format!("turn:{turn_index}:step:{step_index}:tool-use"),
                                line: format_tool_line("assistant", &name, input)
                                    .ok_or_else(|| corruption("invalid tool use role"))?,
                            });
                            if let Some(result) = result {
                                occurrences.push(TranscriptOccurrence {
                                    id: format!(
                                        "turn:{turn_index}:step:{step_index}:tool-result"
                                    ),
                                    line: format_tool_line("tool", &name, result)
                                        .ok_or_else(|| corruption("invalid tool result role"))?,
                                });
                            }
                        }
                    }
                }
                DecodedTranscriptStep::Empty => {}
            }
        }

        Ok(DerivedTranscriptOccurrences {
            occurrences,
            deferred_step: None,
        })
    }

    pub fn initial<Store: TranscriptOccurrenceBlobStore>(
        &self,
        store: &Store,
        checkpoint: &TranscriptCheckpoint,
    ) -> Result<Vec<TranscriptOccurrence>, TranscriptJournalCorruptionError> {
        let mut occurrences = Vec::new();
        for (turn_index, turn) in checkpoint.turns.iter().enumerate() {
            occurrences.extend(
                self.derive_turn(store, turn_index, turn, None, true, None)?
                    .occurrences,
            );
        }
        Ok(occurrences)
    }

    pub fn derive<Store: TranscriptOccurrenceBlobStore>(
        &self,
        store: &Store,
        previous: &TranscriptCheckpoint,
        checkpoint: &TranscriptCheckpoint,
        finalize_checkpoint: bool,
        deferred: Option<DeferredTranscriptStep>,
    ) -> Result<DerivedTranscriptOccurrences, TranscriptJournalCorruptionError> {
        if checkpoint.turns.len() < previous.turns.len() {
            return Err(corruption("durable conversation turns moved backwards"));
        }
        if previous.turns.len() > 1
            && !bytes_equal(
                &checkpoint.turns[previous.turns.len() - 2],
                &previous.turns[previous.turns.len() - 2],
            )
        {
            return Err(corruption(
                "durable conversation history changed before the active turn",
            ));
        }

        let mut derived = Vec::new();
        if !previous.turns.is_empty() {
            let turn_index = previous.turns.len() - 1;
            if !bytes_equal(
                &previous.turns[turn_index],
                &checkpoint.turns[turn_index],
            ) || deferred.is_some_and(|step| step.turn_index == turn_index)
            {
                derived.push(self.derive_turn(
                    store,
                    turn_index,
                    &checkpoint.turns[turn_index],
                    Some(&previous.turns[turn_index]),
                    finalize_checkpoint || turn_index + 1 < checkpoint.turns.len(),
                    deferred,
                )?);
            }
        }
        for turn_index in previous.turns.len()..checkpoint.turns.len() {
            derived.push(self.derive_turn(
                store,
                turn_index,
                &checkpoint.turns[turn_index],
                None,
                finalize_checkpoint || turn_index + 1 < checkpoint.turns.len(),
                deferred,
            )?);
        }
        let next_deferred = derived.iter().find_map(|turn| turn.deferred_step);
        Ok(DerivedTranscriptOccurrences {
            occurrences: derived
                .into_iter()
                .flat_map(|turn| turn.occurrences)
                .collect(),
            deferred_step: next_deferred,
        })
    }
}

impl<Codec, Store> TranscriptDeriver<Store> for ArtifactTranscriptOccurrenceDeriver<Codec>
where
    Codec: TranscriptOccurrenceCodec,
    Store: TranscriptOccurrenceBlobStore,
{
    fn initial(
        &self,
        store: &Store,
        checkpoint: &TranscriptCheckpoint,
    ) -> Result<Vec<TranscriptOccurrence>, TranscriptJournalCorruptionError> {
        ArtifactTranscriptOccurrenceDeriver::initial(self, store, checkpoint)
    }

    fn derive(
        &self,
        store: &Store,
        previous: &TranscriptCheckpoint,
        checkpoint: &TranscriptCheckpoint,
        finalize_checkpoint: bool,
        deferred: Option<DeferredTranscriptStep>,
    ) -> Result<DerivedTranscriptOccurrences, TranscriptJournalCorruptionError> {
        ArtifactTranscriptOccurrenceDeriver::derive(
            self,
            store,
            previous,
            checkpoint,
            finalize_checkpoint,
            deferred,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Store(HashMap<Vec<u8>, Vec<u8>>);

    impl Store {
        fn insert(&mut self, id: u8, body: u8) {
            self.0.insert(vec![id], vec![body]);
        }
    }

    impl TranscriptOccurrenceBlobStore for Store {
        fn get_blob(&self, id: &[u8]) -> Option<Vec<u8>> {
            self.0.get(id).cloned()
        }
    }

    struct Codec;

    impl TranscriptOccurrenceCodec for Codec {
        fn decode_turn(&self, bytes: &[u8]) -> Result<DecodedTranscriptTurn, String> {
            match bytes.first().copied() {
                Some(1) => Ok(DecodedTranscriptTurn::Agent {
                    user_message: vec![10],
                    steps: vec![vec![20], vec![21]],
                }),
                Some(2) => Ok(DecodedTranscriptTurn::Agent {
                    user_message: vec![10],
                    steps: vec![vec![20], vec![22]],
                }),
                Some(3) => Ok(DecodedTranscriptTurn::Shell),
                _ => Ok(DecodedTranscriptTurn::Empty),
            }
        }

        fn decode_user_message(&self, _bytes: &[u8]) -> Result<DecodedUserMessage, String> {
            Ok(DecodedUserMessage {
                text: "<user_info>hidden</user_info>Hello".into(),
                text_blob_id: None,
            })
        }

        fn decode_step(&self, bytes: &[u8]) -> Result<DecodedTranscriptStep, String> {
            match bytes.first().copied() {
                Some(4) => Ok(DecodedTranscriptStep::Tool {
                    name: "search".into(),
                    input: json!({"q":"x"}),
                    result: Some(json!({"ok":true})),
                }),
                Some(5) => Ok(DecodedTranscriptStep::Assistant {
                    text: "<think>secret</think>Visible".into(),
                }),
                Some(6) => Ok(DecodedTranscriptStep::Assistant {
                    text: "Changed tail".into(),
                }),
                _ => Ok(DecodedTranscriptStep::Empty),
            }
        }
    }

    #[test]
    fn context_stripping_preserves_similar_non_tag_text_without_looping() {
        let text = strip_context_tags(
            "prefix <user_information>keep</user_information> <user_info>drop</user_info> suffix",
        );
        assert!(text.contains("<user_information>keep</user_information>"));
        assert!(!text.contains(">drop<"));
        assert!(text.ends_with("suffix"));
    }

    #[test]
    fn initial_emits_visible_user_tool_and_assistant_occurrences() {
        let mut store = Store::default();
        store.insert(1, 1);
        store.insert(10, 10);
        store.insert(20, 4);
        store.insert(21, 5);
        let deriver = ArtifactTranscriptOccurrenceDeriver::new(Codec);
        let rows = deriver
            .initial(
                &mut store,
                &TranscriptCheckpoint {
                    turns: vec![vec![1]],
                },
            )
            .unwrap();
        assert_eq!(rows.len(), 4);
        assert!(rows[0].line.contains("Hello"));
        assert!(!rows[0].line.contains("hidden"));
        assert!(rows[3].line.contains("Visible"));
        assert!(!rows[3].line.contains("secret"));
    }

    #[test]
    fn active_tail_text_is_deferred_until_finalize() {
        let mut store = Store::default();
        store.insert(1, 1);
        store.insert(2, 2);
        store.insert(10, 10);
        store.insert(20, 4);
        store.insert(21, 5);
        store.insert(22, 6);
        let deriver = ArtifactTranscriptOccurrenceDeriver::new(Codec);
        let pending = deriver
            .derive(
                &mut store,
                &TranscriptCheckpoint {
                    turns: vec![vec![1]],
                },
                &TranscriptCheckpoint {
                    turns: vec![vec![2]],
                },
                false,
                None,
            )
            .unwrap();
        assert_eq!(
            pending.deferred_step,
            Some(DeferredTranscriptStep {
                turn_index: 0,
                step_index: 1
            })
        );
        let finalized = deriver
            .derive(
                &mut store,
                &TranscriptCheckpoint {
                    turns: vec![vec![1]],
                },
                &TranscriptCheckpoint {
                    turns: vec![vec![2]],
                },
                true,
                pending.deferred_step,
            )
            .unwrap();
        assert!(finalized
            .occurrences
            .iter()
            .any(|entry| entry.line.contains("Changed tail")));
    }

    #[test]
    fn shell_turns_are_rejected() {
        let mut store = Store::default();
        store.insert(3, 3);
        let deriver = ArtifactTranscriptOccurrenceDeriver::new(Codec);
        let error = deriver
            .initial(
                &mut store,
                &TranscriptCheckpoint {
                    turns: vec![vec![3]],
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("shell conversation turns"));
    }
}
