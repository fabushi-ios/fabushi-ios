use serde_json::Value;

use super::transcript_journal_codec::{
    DeferredTranscriptStep, TranscriptCheckpoint, bytes_equal, format_tool_line,
};
use super::transcript_mirror::{
    DerivedTranscriptOccurrences, TranscriptDeriver, TranscriptOccurrence,
};

const CONTEXT_TAGS_TO_STRIP: &[&str] = &[
    "user_info",
    "project_layout",
    "rules",
    "always_applied_workspace_rules",
    "agent_requestable_workspace_rules",
    "user_rules",
    "agent_skills",
    "available_skills",
    "cloud_instructions",
    "cloud_task_instructions",
    "open_and_recently_viewed_files",
    "system_reminder",
    "system-reminder",
    "mcp_instructions",
    "mcp_file_system",
    "mcp_file_system_servers",
    "git_status",
    "agent_transcripts",
    "cursor_rules_context",
    "attached_files",
    "system_notification",
    "task_notification",
    "agent_notification",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedTranscriptTurn {
    Agent {
        user_message: Vec<u8>,
        steps: Vec<Vec<u8>>,
    },
    Shell,
    Undefined,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodedTranscriptStep {
    Assistant { text: String },
    Thinking { text: String },
    Tool {
        name: String,
        input: Value,
        result: Option<Value>,
    },
    Undefined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedUserMessage {
    pub text: String,
    pub text_blob_id: Option<Vec<u8>>,
}

pub trait TranscriptOccurrenceCodec: Send + Sync {
    fn decode_turn(&self, bytes: &[u8]) -> Result<DecodedTranscriptTurn, String>;
    fn decode_user_message(&self, bytes: &[u8]) -> Result<DecodedUserMessage, String>;
    fn decode_step(&self, bytes: &[u8]) -> Result<DecodedTranscriptStep, String>;
}

pub trait TranscriptOccurrenceBlobStore: Send + Sync {
    fn get_blob(&self, id: &[u8]) -> Result<Option<Vec<u8>>, String>;
}

pub struct ArtifactTranscriptOccurrenceDeriver<Codec> {
    codec: Codec,
}

impl<Codec> ArtifactTranscriptOccurrenceDeriver<Codec> {
    pub fn new(codec: Codec) -> Self {
        Self { codec }
    }

    pub fn codec(&self) -> &Codec {
        &self.codec
    }
}

fn required_blob<Store>(
    store: &Store,
    id: &[u8],
    label: &str,
) -> Result<Vec<u8>, String>
where
    Store: TranscriptOccurrenceBlobStore,
{
    store
        .get_blob(id)?
        .ok_or_else(|| format!("missing {label} blob while deriving transcript checkpoint"))
}

fn collapse_blank_lines(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut newline_run = 0usize;
    for ch in value.chars() {
        if ch == '\n' {
            newline_run = newline_run.saturating_add(1);
            if newline_run <= 2 {
                output.push(ch);
            }
        } else {
            newline_run = 0;
            output.push(ch);
        }
    }
    output.trim().to_string()
}

fn strip_tag_case_insensitive(mut value: String, tag: &str) -> String {
    let open_prefix = format!("<{}", tag.to_ascii_lowercase());
    let close = format!("</{}>", tag.to_ascii_lowercase());
    let mut search_from = 0usize;

    loop {
        let lower = value.to_ascii_lowercase();
        let Some(relative_start) = lower[search_from..].find(&open_prefix) else {
            break;
        };
        let start = search_from + relative_start;
        let boundary = start + open_prefix.len();
        let Some(next) = lower.as_bytes().get(boundary).copied() else {
            break;
        };
        if next != b'>' && !next.is_ascii_whitespace() {
            search_from = boundary;
            continue;
        }
        let Some(open_end_relative) = lower[boundary..].find('>') else {
            break;
        };
        let content_start = boundary + open_end_relative + 1;
        let Some(close_relative) = lower[content_start..].find(&close) else {
            break;
        };
        let end = content_start + close_relative + close.len();
        value.replace_range(start..end, "");
        search_from = start;
    }
    value
}

pub fn strip_context_tags(text: &str) -> String {
    let mut value = text.to_string();
    for tag in CONTEXT_TAGS_TO_STRIP {
        value = strip_tag_case_insensitive(value, tag);
    }
    collapse_blank_lines(&value)
}

pub fn strip_hidden_thinking_tags(text: &str) -> String {
    let value = strip_tag_case_insensitive(text.to_string(), "think");
    let value = strip_tag_case_insensitive(value, "thinking");
    collapse_blank_lines(&value)
}

fn format_text_occurrence(role: &str, text: &str) -> Option<String> {
    let visible = if role == "user" {
        strip_context_tags(text)
    } else {
        strip_hidden_thinking_tags(text)
    };
    if visible.trim().is_empty() {
        return None;
    }
    Some(
        serde_json::json!({
            "role": role,
            "message": {
                "content": [{
                    "type": "text",
                    "text": visible,
                }]
            }
        })
        .to_string(),
    )
}

fn same_tool_input(left: &Value, right: &Value) -> bool {
    serde_json::to_string(left).ok() == serde_json::to_string(right).ok()
}

impl<Codec> ArtifactTranscriptOccurrenceDeriver<Codec>
where
    Codec: TranscriptOccurrenceCodec,
{
    fn derive_turn<Store>(
        &self,
        store: &Store,
        turn_index: usize,
        current_blob_id: &[u8],
        previous_blob_id: Option<&[u8]>,
        finalize_turn: bool,
        deferred_step: Option<DeferredTranscriptStep>,
    ) -> Result<DerivedTranscriptOccurrences, String>
    where
        Store: TranscriptOccurrenceBlobStore,
    {
        let current = self
            .codec
            .decode_turn(&required_blob(store, current_blob_id, "conversation-turn")?)?;
        if matches!(&current, DecodedTranscriptTurn::Shell) {
            return Err("Sand does not support shell conversation turns".into());
        }

        let previous = match previous_blob_id {
            Some(previous_blob_id) => Some(
                self.codec.decode_turn(&required_blob(
                    store,
                    previous_blob_id,
                    "previous conversation-turn",
                )?)?,
            ),
            None => None,
        };

        if previous.as_ref().is_some_and(|previous| {
            std::mem::discriminant(previous) != std::mem::discriminant(&current)
        }) {
            return Err("durable conversation turn changed kind".into());
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

        if let Some((previous_user_message, previous_steps)) = previous_agent.as_ref() {
            if !bytes_equal(previous_user_message, &user_message) {
                return Err("durable agent user message changed after checkpoint".into());
            }
            if steps.len() < previous_steps.len() {
                return Err("durable agent steps moved backwards".into());
            }
        }

        let mut first_changed_step = previous_agent
            .as_ref()
            .map(|(_, steps)| steps.len())
            .unwrap_or_default();
        if let Some((_, previous_steps)) = previous_agent.as_ref() {
            for (index, previous_step) in previous_steps.iter().enumerate() {
                let Some(current_step) = steps.get(index) else {
                    return Err("durable agent steps moved backwards".into());
                };
                if bytes_equal(previous_step, current_step) {
                    continue;
                }
                if index + 1 != previous_steps.len() {
                    return Err("durable agent step changed before the checkpoint tail".into());
                }
                first_changed_step = index;
                break;
            }
        }
        if let Some(deferred_step) =
            deferred_step.filter(|value| value.turn_index == turn_index)
        {
            first_changed_step = first_changed_step.min(deferred_step.step_index);
        }

        let mut occurrences = Vec::new();
        if previous_agent.is_none() {
            let decoded_user = self.codec.decode_user_message(&required_blob(
                store,
                &user_message,
                "user-message",
            )?)?;
            let text = if !decoded_user.text.is_empty()
                || decoded_user
                    .text_blob_id
                    .as_ref()
                    .is_none_or(|id| id.is_empty())
            {
                decoded_user.text
            } else {
                let text_blob_id = decoded_user.text_blob_id.unwrap_or_default();
                let bytes = required_blob(store, &text_blob_id, "user-message text")?;
                String::from_utf8_lossy(&bytes).into_owned()
            };
            if let Some(line) = format_text_occurrence("user", &text) {
                occurrences.push(TranscriptOccurrence {
                    id: format!("turn:{turn_index}:user"),
                    line,
                });
            }
        }

        for step_index in first_changed_step..steps.len() {
            let previous_step_blob = previous_agent
                .as_ref()
                .and_then(|(_, previous_steps)| previous_steps.get(step_index));
            let previous_step = match previous_step_blob {
                Some(previous_step_blob) => Some(self.codec.decode_step(&required_blob(
                    store,
                    previous_step_blob,
                    "previous conversation-step",
                )?)?),
                None => None,
            };
            let step = self.codec.decode_step(&required_blob(
                store,
                &steps[step_index],
                "conversation-step",
            )?)?;

            if previous_step.as_ref().is_some_and(|previous| {
                std::mem::discriminant(previous) != std::mem::discriminant(&step)
            }) {
                return Err("durable conversation step changed kind".into());
            }

            if !finalize_turn
                && step_index + 1 == steps.len()
                && matches!(
                    &step,
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
                    if let Some(previous_step) = previous_step {
                        let DecodedTranscriptStep::Tool {
                            name: previous_name,
                            input: previous_input,
                            result: previous_result,
                        } = previous_step
                        else {
                            return Err(
                                "durable conversation step changed into a tool call".into(),
                            );
                        };
                        if previous_name != name || !same_tool_input(&previous_input, &input) {
                            return Err("durable tool call changed after checkpoint".into());
                        }
                        if previous_result.is_some() {
                            return Err(
                                "completed durable tool call changed after checkpoint".into(),
                            );
                        }
                        if let Some(result) = result {
                            occurrences.push(TranscriptOccurrence {
                                id: format!(
                                    "turn:{turn_index}:step:{step_index}:tool-result"
                                ),
                                line: format_tool_line("tool", &name, result),
                            });
                        }
                    } else {
                        occurrences.push(TranscriptOccurrence {
                            id: format!("turn:{turn_index}:step:{step_index}:tool-use"),
                            line: format_tool_line("assistant", &name, input),
                        });
                        if let Some(result) = result {
                            occurrences.push(TranscriptOccurrence {
                                id: format!(
                                    "turn:{turn_index}:step:{step_index}:tool-result"
                                ),
                                line: format_tool_line("tool", &name, result),
                            });
                        }
                    }
                }
                DecodedTranscriptStep::Undefined => {}
            }
        }

        Ok(DerivedTranscriptOccurrences {
            occurrences,
            deferred_step: None,
        })
    }
}

impl<Codec, Store> TranscriptDeriver<Store> for ArtifactTranscriptOccurrenceDeriver<Codec>
where
    Codec: TranscriptOccurrenceCodec,
    Store: TranscriptOccurrenceBlobStore,
{
    fn derive(
        &self,
        store: &Store,
        previous: &TranscriptCheckpoint,
        checkpoint: &TranscriptCheckpoint,
        finalize_checkpoint: bool,
        deferred: Option<DeferredTranscriptStep>,
    ) -> Result<DerivedTranscriptOccurrences, String> {
        if checkpoint.turns.len() < previous.turns.len() {
            return Err("durable conversation turns moved backwards".into());
        }
        if previous.turns.len() > 1
            && !bytes_equal(
                &checkpoint.turns[previous.turns.len() - 2],
                &previous.turns[previous.turns.len() - 2],
            )
        {
            return Err("durable conversation history changed before the active turn".into());
        }

        let mut occurrences = Vec::new();
        let mut next_deferred = None;

        if !previous.turns.is_empty() {
            let turn_index = previous.turns.len() - 1;
            if !bytes_equal(
                &previous.turns[turn_index],
                &checkpoint.turns[turn_index],
            ) || deferred.is_some_and(|value| value.turn_index == turn_index)
            {
                let derived = self.derive_turn(
                    store,
                    turn_index,
                    &checkpoint.turns[turn_index],
                    Some(&previous.turns[turn_index]),
                    finalize_checkpoint || turn_index + 1 < checkpoint.turns.len(),
                    deferred,
                )?;
                if next_deferred.is_none() {
                    next_deferred = derived.deferred_step;
                }
                occurrences.extend(derived.occurrences);
            }
        }

        for turn_index in previous.turns.len()..checkpoint.turns.len() {
            let derived = self.derive_turn(
                store,
                turn_index,
                &checkpoint.turns[turn_index],
                None,
                finalize_checkpoint || turn_index + 1 < checkpoint.turns.len(),
                deferred,
            )?;
            if next_deferred.is_none() {
                next_deferred = derived.deferred_step;
            }
            occurrences.extend(derived.occurrences);
        }

        Ok(DerivedTranscriptOccurrences {
            occurrences,
            deferred_step: next_deferred,
        })
    }

    fn initial(
        &self,
        store: &Store,
        checkpoint: &TranscriptCheckpoint,
    ) -> Result<Vec<TranscriptOccurrence>, String> {
        let mut occurrences = Vec::new();
        for turn_index in 0..checkpoint.turns.len() {
            occurrences.extend(
                self.derive_turn(
                    store,
                    turn_index,
                    &checkpoint.turns[turn_index],
                    None,
                    true,
                    None,
                )?
                .occurrences,
            );
        }
        Ok(occurrences)
    }
}
