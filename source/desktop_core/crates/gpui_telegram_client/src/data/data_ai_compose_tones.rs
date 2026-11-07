//! Target Symbol: DataAiComposeTones
//! Telegram Desktop 1:1 Submodule: `data/data_ai_compose_tones`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AiTone {
    #[default]
    Neutral,
    Formal,
    Casual,
    Concise,
    Persuasive,
    Poetic,
    Humorous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToneOption {
    pub tone: AiTone,
    pub name: String,
    pub description: String,
    pub icon_emoji: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataAiComposeTones {
    pub available_tones: Vec<ToneOption>,
    pub selected_tone: AiTone,
}

impl Default for DataAiComposeTones {
    fn default() -> Self {
        Self {
            available_tones: vec![
                ToneOption {
                    tone: AiTone::Neutral,
                    name: "Neutral".to_string(),
                    description: "Standard balanced rewrite".to_string(),
                    icon_emoji: "✨".to_string(),
                },
                ToneOption {
                    tone: AiTone::Formal,
                    name: "Formal".to_string(),
                    description: "Professional and polite".to_string(),
                    icon_emoji: "💼".to_string(),
                },
                ToneOption {
                    tone: AiTone::Casual,
                    name: "Casual".to_string(),
                    description: "Friendly and relaxed".to_string(),
                    icon_emoji: "☕".to_string(),
                },
                ToneOption {
                    tone: AiTone::Concise,
                    name: "Concise".to_string(),
                    description: "Short and to the point".to_string(),
                    icon_emoji: "⚡".to_string(),
                },
                ToneOption {
                    tone: AiTone::Persuasive,
                    name: "Persuasive".to_string(),
                    description: "Compelling and convincing".to_string(),
                    icon_emoji: "🎯".to_string(),
                },
                ToneOption {
                    tone: AiTone::Humorous,
                    name: "Humorous".to_string(),
                    description: "Witty with playful tone".to_string(),
                    icon_emoji: "😄".to_string(),
                },
            ],
            selected_tone: AiTone::Neutral,
        }
    }
}

impl DataAiComposeTones {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn select_tone(&mut self, tone: AiTone) {
        self.selected_tone = tone;
    }

    pub fn get_tone_info(&self, tone: AiTone) -> Option<&ToneOption> {
        self.available_tones.iter().find(|t| t.tone == tone)
    }
}
