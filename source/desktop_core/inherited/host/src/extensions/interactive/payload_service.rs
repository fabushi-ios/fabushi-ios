//! Interactive Message Presentation, Portable Action Schemas & Fallback Text Rendering
//!
//! Mirrors upstream OpenClaw `src/interactive/payload.ts`:
//! Defines portable presentation schemas for interactive message cards, buttons,
//! select menus, question actions, and model-picker envelopes with safe fallback
//! rendering that prevents leaking private callback payloads.

use serde::{Deserialize, Serialize};

/// Visual tone for a portable message presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessagePresentationTone {
    Info,
    Success,
    Warning,
    Danger,
    Neutral,
}

/// Button style hint for renderers that support styled actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InteractiveButtonStyle {
    Primary,
    Secondary,
    Success,
    Danger,
}

/// Question presentation action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "kebab-case")]
pub enum QuestionPresentationAction {
    #[serde(rename = "custom-input")]
    CustomInput { question_id: String },
    #[serde(untagged)]
    OptionValue {
        question_id: String,
        option_value: String,
    },
}

/// Intent behind core-owned model-picker actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "kebab-case")]
pub enum ModelPickerIntent {
    ShowProviders {
        cursor: Option<String>,
    },
    ShowModels {
        provider_token: String,
        cursor: Option<String>,
    },
    ShowRecents {
        cursor: Option<String>,
    },
    ChooseModel {
        provider_token: String,
        model_token: String,
    },
    ChooseRuntime {
        provider_token: String,
        model_token: String,
        runtime_token: String,
    },
    Reset,
    Cancel,
}

/// Core-owned model-picker action envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPickerAction {
    pub version: u32,
    pub snapshot_token: String,
    #[serde(flatten)]
    pub intent: ModelPickerIntent,
}

/// Portable typed action behind a button or select option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum MessagePresentationAction {
    Command {
        command: String,
    },
    Callback {
        value: String,
    },
    Url {
        url: String,
    },
    Approval {
        approval_id: String,
        decision: String,
    },
    Question(QuestionPresentationAction),
    ModelPicker(ModelPickerAction),
}

/// Button component within an interactive presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationButton {
    pub label: String,
    pub action: MessagePresentationAction,
    pub style: Option<InteractiveButtonStyle>,
}

/// Select component option within an interactive presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationSelectOption {
    pub label: String,
    pub action: MessagePresentationAction,
}

/// Structured content block within an interactive presentation card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PresentationBlock {
    Buttons {
        buttons: Vec<PresentationButton>,
    },
    Select {
        placeholder: Option<String>,
        options: Vec<PresentationSelectOption>,
    },
    Text {
        text: String,
    },
}

/// Top-level portable message presentation container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MessagePresentation {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tone: Option<MessagePresentationTone>,
    #[serde(default)]
    pub blocks: Vec<PresentationBlock>,
}

/// Normalizes and validates a message presentation.
/// Returns None if any action fails validation (e.g., unknown question intent).
pub fn normalize_message_presentation(pres: &MessagePresentation) -> Option<MessagePresentation> {
    let mut normalized_blocks = Vec::with_capacity(pres.blocks.len());

    for block in &pres.blocks {
        match block {
            PresentationBlock::Buttons { buttons } => {
                let mut valid_buttons = Vec::with_capacity(buttons.len());
                for btn in buttons {
                    if let Some(norm_action) = normalize_action(&btn.action) {
                        valid_buttons.push(PresentationButton {
                            label: btn.label.clone(),
                            action: norm_action,
                            style: btn.style,
                        });
                    } else {
                        return None;
                    }
                }
                normalized_blocks.push(PresentationBlock::Buttons {
                    buttons: valid_buttons,
                });
            }
            PresentationBlock::Select {
                placeholder,
                options,
            } => {
                let mut valid_options = Vec::with_capacity(options.len());
                for opt in options {
                    if let Some(norm_action) = normalize_action(&opt.action) {
                        valid_options.push(PresentationSelectOption {
                            label: opt.label.clone(),
                            action: norm_action,
                        });
                    } else {
                        return None;
                    }
                }
                normalized_blocks.push(PresentationBlock::Select {
                    placeholder: placeholder.clone(),
                    options: valid_options,
                });
            }
            PresentationBlock::Text { text } => {
                normalized_blocks.push(PresentationBlock::Text { text: text.clone() });
            }
        }
    }

    Some(MessagePresentation {
        title: pres.title.clone(),
        description: pres.description.clone(),
        tone: pres.tone,
        blocks: normalized_blocks,
    })
}

fn normalize_action(action: &MessagePresentationAction) -> Option<MessagePresentationAction> {
    match action {
        MessagePresentationAction::Command { command } => {
            let trimmed = command.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(MessagePresentationAction::Command {
                    command: trimmed.to_string(),
                })
            }
        }
        MessagePresentationAction::Callback { value } => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(MessagePresentationAction::Callback {
                    value: trimmed.to_string(),
                })
            }
        }
        MessagePresentationAction::Url { url } => {
            let trimmed = url.trim();
            if trimmed.starts_with("http://")
                || trimmed.starts_with("https://")
                || trimmed.starts_with("mailto:")
            {
                Some(MessagePresentationAction::Url {
                    url: trimmed.to_string(),
                })
            } else {
                None
            }
        }
        MessagePresentationAction::Approval {
            approval_id,
            decision,
        } => {
            if approval_id.trim().is_empty() || decision.trim().is_empty() {
                None
            } else {
                Some(action.clone())
            }
        }
        MessagePresentationAction::Question(q) => match q {
            QuestionPresentationAction::CustomInput { question_id } => {
                if question_id.trim().is_empty() {
                    None
                } else {
                    Some(MessagePresentationAction::Question(q.clone()))
                }
            }
            QuestionPresentationAction::OptionValue {
                question_id,
                option_value,
            } => {
                if question_id.trim().is_empty() || option_value.trim().is_empty() {
                    None
                } else {
                    Some(MessagePresentationAction::Question(q.clone()))
                }
            }
        },
        MessagePresentationAction::ModelPicker(m) => {
            if m.version == 1 && !m.snapshot_token.trim().is_empty() {
                Some(action.clone())
            } else {
                None
            }
        }
    }
}

/// Renders message presentation into plaintext/markdown fallback text.
///
/// Ensures callback tokens are never leaked to user-facing transcripts.
pub fn render_message_presentation_fallback_text(pres: &MessagePresentation) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(title) = &pres.title {
        parts.push(format!("**{title}**"));
    }
    if let Some(desc) = &pres.description {
        parts.push(desc.clone());
    }

    for block in &pres.blocks {
        match block {
            PresentationBlock::Text { text } => {
                parts.push(text.clone());
            }
            PresentationBlock::Select {
                placeholder,
                options,
            } => {
                let header = placeholder.as_deref().unwrap_or("Select an option");
                let mut lines = vec![format!("{header}:")];
                for opt in options {
                    match &opt.action {
                        MessagePresentationAction::Command { command } => {
                            lines.push(format!("- {}: `{command}`", opt.label));
                        }
                        _ => {
                            lines.push(format!("- {}", opt.label));
                        }
                    }
                }
                parts.push(lines.join("\n"));
            }
            PresentationBlock::Buttons { buttons } => {
                let btn_strs: Vec<String> = buttons
                    .iter()
                    .map(|b| match &b.action {
                        MessagePresentationAction::Command { command } => {
                            format!("[ {}: `{command}` ]", b.label)
                        }
                        MessagePresentationAction::Url { url } => {
                            format!("[ {} ({url}) ]", b.label)
                        }
                        _ => format!("[ {} ]", b.label),
                    })
                    .collect();
                parts.push(btn_strs.join(" "));
            }
        }
    }

    parts.join("\n\n")
}
