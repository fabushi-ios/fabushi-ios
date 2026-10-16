use crate::transcript_occurrence_deriver::{
    DecodedTranscriptStep, DecodedTranscriptTurn, DecodedUserMessage, TranscriptOccurrenceCodec,
};
use serde_json::{Value, json};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratedConversationTurn {
    Agent {
        user_message: Vec<u8>,
        steps: Vec<Vec<u8>>,
    },
    Shell,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedUserMessage {
    pub text: String,
    pub text_blob_id: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GeneratedConversationStep {
    Assistant { text: String },
    Thinking { text: String },
    ToolCall(GeneratedToolCall),
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedToolCall {
    /// Exact generated oneof case, for example `mcpToolCall` or
    /// `shellToolCall`.
    pub tool_case: String,
    /// Exact generated `args.toJson()` projection. Bindings must provide the
    /// generated JSON value; the adapter never substitutes its own protobuf
    /// decoder or field mapping.
    pub args_json: Option<Value>,
    /// Exact generated `result.toJson()` projection.
    pub result_json: Option<Value>,
    /// Generated MCP args.toolName when present.
    pub mcp_tool_name: Option<String>,
    /// Generated MCP args.name compatibility field when present.
    pub mcp_name: Option<String>,
}

pub trait GeneratedAgentV1Bindings {
    type Error: fmt::Display;

    fn decode_conversation_turn_structure(
        &self,
        bytes: &[u8],
    ) -> Result<GeneratedConversationTurn, Self::Error>;

    fn decode_user_message(&self, bytes: &[u8]) -> Result<GeneratedUserMessage, Self::Error>;

    fn decode_conversation_step(
        &self,
        bytes: &[u8],
    ) -> Result<GeneratedConversationStep, Self::Error>;
}

pub struct GeneratedTranscriptOccurrenceCodec<Bindings> {
    bindings: Bindings,
}

impl<Bindings> GeneratedTranscriptOccurrenceCodec<Bindings>
where
    Bindings: GeneratedAgentV1Bindings,
{
    pub fn new(bindings: Bindings) -> Self {
        Self { bindings }
    }

    pub fn bindings(&self) -> &Bindings {
        &self.bindings
    }
}

fn tool_name(call: &GeneratedToolCall) -> String {
    if call.tool_case == "mcpToolCall" {
        return call
            .mcp_tool_name
            .as_deref()
            .or(call.mcp_name.as_deref())
            .unwrap_or("mcp")
            .to_owned();
    }

    let base = call
        .tool_case
        .strip_suffix("ToolCall")
        .unwrap_or(&call.tool_case);
    let mut out = String::with_capacity(base.len() + 4);
    let mut previous_was_lower_or_digit = false;
    for ch in base.chars() {
        if ch.is_ascii_uppercase() && previous_was_lower_or_digit {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
        previous_was_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    out
}

impl<Bindings> TranscriptOccurrenceCodec for GeneratedTranscriptOccurrenceCodec<Bindings>
where
    Bindings: GeneratedAgentV1Bindings,
{
    fn decode_turn(&self, bytes: &[u8]) -> Result<DecodedTranscriptTurn, String> {
        match self
            .bindings
            .decode_conversation_turn_structure(bytes)
            .map_err(|error| error.to_string())?
        {
            GeneratedConversationTurn::Agent {
                user_message,
                steps,
            } => Ok(DecodedTranscriptTurn::Agent {
                user_message,
                steps,
            }),
            GeneratedConversationTurn::Shell => Ok(DecodedTranscriptTurn::Shell),
            GeneratedConversationTurn::Empty => Ok(DecodedTranscriptTurn::Empty),
        }
    }

    fn decode_user_message(&self, bytes: &[u8]) -> Result<DecodedUserMessage, String> {
        let decoded = self
            .bindings
            .decode_user_message(bytes)
            .map_err(|error| error.to_string())?;
        Ok(DecodedUserMessage {
            text: decoded.text,
            text_blob_id: decoded.text_blob_id,
        })
    }

    fn decode_step(&self, bytes: &[u8]) -> Result<DecodedTranscriptStep, String> {
        match self
            .bindings
            .decode_conversation_step(bytes)
            .map_err(|error| error.to_string())?
        {
            GeneratedConversationStep::Assistant { text } => {
                Ok(DecodedTranscriptStep::Assistant { text })
            }
            GeneratedConversationStep::Thinking { text } => {
                Ok(DecodedTranscriptStep::Thinking { text })
            }
            GeneratedConversationStep::ToolCall(call) => Ok(DecodedTranscriptStep::Tool {
                name: tool_name(&call),
                input: call.args_json.unwrap_or_else(|| json!({})),
                result: call.result_json,
            }),
            GeneratedConversationStep::Empty => Ok(DecodedTranscriptStep::Empty),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct DecodeError(&'static str);

    impl fmt::Display for DecodeError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    #[derive(Clone, Copy)]
    struct Bindings;

    impl GeneratedAgentV1Bindings for Bindings {
        type Error = DecodeError;

        fn decode_conversation_turn_structure(
            &self,
            bytes: &[u8],
        ) -> Result<GeneratedConversationTurn, Self::Error> {
            match bytes.first().copied() {
                Some(1) => Ok(GeneratedConversationTurn::Agent {
                    user_message: vec![7],
                    steps: vec![vec![8], vec![9]],
                }),
                Some(2) => Ok(GeneratedConversationTurn::Shell),
                Some(3) => Err(DecodeError("generated turn decode failed")),
                _ => Ok(GeneratedConversationTurn::Empty),
            }
        }

        fn decode_user_message(
            &self,
            bytes: &[u8],
        ) -> Result<GeneratedUserMessage, Self::Error> {
            if bytes.first() == Some(&3) {
                return Err(DecodeError("generated user decode failed"));
            }
            Ok(GeneratedUserMessage {
                text: "hello".into(),
                text_blob_id: Some(vec![4, 5]),
            })
        }

        fn decode_conversation_step(
            &self,
            bytes: &[u8],
        ) -> Result<GeneratedConversationStep, Self::Error> {
            match bytes.first().copied() {
                Some(1) => Ok(GeneratedConversationStep::Assistant {
                    text: "answer".into(),
                }),
                Some(2) => Ok(GeneratedConversationStep::Thinking {
                    text: "reason".into(),
                }),
                Some(3) => Ok(GeneratedConversationStep::ToolCall(GeneratedToolCall {
                    tool_case: "shellToolCall".into(),
                    args_json: Some(json!({"command":"pwd"})),
                    result_json: Some(json!({"exitCode":0})),
                    mcp_tool_name: None,
                    mcp_name: None,
                })),
                Some(4) => Ok(GeneratedConversationStep::ToolCall(GeneratedToolCall {
                    tool_case: "mcpToolCall".into(),
                    args_json: Some(json!({"toolName":"files.search"})),
                    result_json: None,
                    mcp_tool_name: Some("files.search".into()),
                    mcp_name: Some("fallback".into()),
                })),
                Some(5) => Ok(GeneratedConversationStep::ToolCall(GeneratedToolCall {
                    tool_case: "mcpToolCall".into(),
                    args_json: None,
                    result_json: None,
                    mcp_tool_name: None,
                    mcp_name: None,
                })),
                Some(6) => Err(DecodeError("generated step decode failed")),
                _ => Ok(GeneratedConversationStep::Empty),
            }
        }
    }

    #[test]
    fn maps_generated_turn_and_user_message_shapes_without_redecoding_wire_bytes() {
        let codec = GeneratedTranscriptOccurrenceCodec::new(Bindings);
        assert_eq!(
            codec.decode_turn(&[1]).unwrap(),
            DecodedTranscriptTurn::Agent {
                user_message: vec![7],
                steps: vec![vec![8], vec![9]],
            }
        );
        assert_eq!(codec.decode_turn(&[2]).unwrap(), DecodedTranscriptTurn::Shell);
        assert_eq!(
            codec.decode_user_message(&[1]).unwrap(),
            DecodedUserMessage {
                text: "hello".into(),
                text_blob_id: Some(vec![4, 5]),
            }
        );
    }

    #[test]
    fn maps_generated_tool_cases_and_preserves_generated_json_projection() {
        let codec = GeneratedTranscriptOccurrenceCodec::new(Bindings);
        assert_eq!(
            codec.decode_step(&[3]).unwrap(),
            DecodedTranscriptStep::Tool {
                name: "shell".into(),
                input: json!({"command":"pwd"}),
                result: Some(json!({"exitCode":0})),
            }
        );
        assert_eq!(
            codec.decode_step(&[4]).unwrap(),
            DecodedTranscriptStep::Tool {
                name: "files.search".into(),
                input: json!({"toolName":"files.search"}),
                result: None,
            }
        );
        assert_eq!(
            codec.decode_step(&[5]).unwrap(),
            DecodedTranscriptStep::Tool {
                name: "mcp".into(),
                input: json!({}),
                result: None,
            }
        );
    }

    #[test]
    fn generated_decode_errors_are_propagated_instead_of_falling_back() {
        let codec = GeneratedTranscriptOccurrenceCodec::new(Bindings);
        assert_eq!(
            codec.decode_turn(&[3]).unwrap_err(),
            "generated turn decode failed"
        );
        assert_eq!(
            codec.decode_step(&[6]).unwrap_err(),
            "generated step decode failed"
        );
    }
}
