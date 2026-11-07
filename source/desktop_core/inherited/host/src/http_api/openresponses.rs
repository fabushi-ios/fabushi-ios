use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateResponseRequest {
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub messages: Option<Vec<Value>>,
    pub session_id: Option<String>,
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateResponseMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateResponseChoice {
    pub index: usize,
    pub message: CreateResponseMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateResponseUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateResponseBody {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<CreateResponseChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<CreateResponseUsage>,
}

impl CreateResponseBody {
    pub fn new(id: impl Into<String>, model: impl Into<String>, content: impl Into<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id: id.into(),
            object: "chat.completion".into(),
            created: now,
            model: model.into(),
            choices: vec![CreateResponseChoice {
                index: 0,
                message: CreateResponseMessage {
                    role: "assistant".into(),
                    content: content.into(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(CreateResponseUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            }),
        }
    }
}
