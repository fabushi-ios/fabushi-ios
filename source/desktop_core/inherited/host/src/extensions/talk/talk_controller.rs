use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSessionPhase {
    Inactive,
    Listening,
    Thinking,
    Speaking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TalkbackMode {
    AgentOnly,
    Bidirectional,
}

#[derive(Clone)]
pub struct TalkSessionController {
    phase: VoiceSessionPhase,
    mode: TalkbackMode,
    silence_threshold_rms: f32,
}

impl Default for TalkSessionController {
    fn default() -> Self {
        Self {
            phase: VoiceSessionPhase::Inactive,
            mode: TalkbackMode::Bidirectional,
            silence_threshold_rms: 0.015,
        }
    }
}

impl TalkSessionController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn phase(&self) -> VoiceSessionPhase {
        self.phase
    }

    pub fn set_phase(&mut self, phase: VoiceSessionPhase) {
        self.phase = phase;
    }

    pub fn mode(&self) -> TalkbackMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: TalkbackMode) {
        self.mode = mode;
    }

    pub fn compute_audio_rms(pcm_bytes: &[u8]) -> f32 {
        if pcm_bytes.len() < 2 {
            return 0.0;
        }

        let mut sum_sq = 0.0;
        let mut sample_count = 0;

        for chunk in pcm_bytes.chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0;
            sum_sq += sample * sample;
            sample_count += 1;
        }

        if sample_count == 0 {
            0.0
        } else {
            (sum_sq / sample_count as f32).sqrt()
        }
    }

    pub fn is_speech_detected(&self, pcm_bytes: &[u8]) -> bool {
        Self::compute_audio_rms(pcm_bytes) > self.silence_threshold_rms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TtsAutoApplyMode {
    Off,
    Inbound,
    Tagged,
    Always,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ParsedTtsDirectives {
    pub cleaned_text: String,
    pub tts_text: Option<String>,
    pub provider: Option<String>,
    pub voice_id: Option<String>,
    pub speed: Option<f32>,
    pub stability: Option<f32>,
}

pub struct TtsDirectiveParser;

impl TtsDirectiveParser {
    pub fn parse(input: &str, allow_provider_override: bool) -> ParsedTtsDirectives {
        let mut result = ParsedTtsDirectives::default();
        let mut work = input.to_string();

        // Check for [[tts:text]]...[[/tts:text]]
        if let Some(start_tag) = work.find("[[tts:text]]") {
            if let Some(end_tag) = work.find("[[/tts:text]]") {
                if start_tag + 12 <= end_tag {
                    result.tts_text = Some(work[start_tag + 12..end_tag].trim().to_string());
                }
            }
        }

        // Parse directive tags like [[tts:provider=... voice=...]]
        while let Some(start) = work.find("[[tts:") {
            if let Some(end) = work[start..].find("]]") {
                let directive = &work[start + 6..start + end];
                for token in directive.split_whitespace() {
                    if let Some((k, v)) = token.split_once('=') {
                        let k = k.to_lowercase();
                        match k.as_str() {
                            "provider" => {
                                if allow_provider_override {
                                    let prov = if v == "edge" { "microsoft" } else { v };
                                    result.provider = Some(prov.to_string());
                                }
                            }
                            "voice" | "voiceid" => {
                                result.voice_id = Some(v.to_string());
                            }
                            "speed" => {
                                if let Ok(s) = v.parse::<f32>() {
                                    result.speed = Some(s);
                                }
                            }
                            "stability" => {
                                if let Ok(s) = v.parse::<f32>() {
                                    result.stability = Some(s);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                work.replace_range(start..start + end + 2, "");
            } else {
                break;
            }
        }

        // Clean out any leftover [[tts:text]] or [[/tts:text]]
        work = work.replace("[[tts:text]]", "").replace("[[/tts:text]]", "");
        result.cleaned_text = work.trim().to_string();
        result
    }

    pub fn evaluate_auto_tts(
        mode: TtsAutoApplyMode,
        raw_text: &str,
        inbound_audio: bool,
    ) -> bool {
        match mode {
            TtsAutoApplyMode::Off => false,
            TtsAutoApplyMode::Always => !raw_text.trim().is_empty(),
            TtsAutoApplyMode::Inbound => {
                if !inbound_audio {
                    return false;
                }
                // Strip markdown styling to check for meaningful text
                let cleaned = raw_text
                    .replace('#', "")
                    .replace('*', "")
                    .replace('_', "")
                    .replace('`', "")
                    .trim()
                    .to_string();
                !cleaned.is_empty()
            }
            TtsAutoApplyMode::Tagged => raw_text.contains("[[tts:text]]"),
        }
    }

    pub fn format_tts_provider_error(provider: &str, raw_error: &str) -> String {
        let redacted = Self::redact_sensitive_tokens(raw_error);
        format!("{}: {}", provider, redacted)
    }

    pub fn sanitize_tts_error_for_log(raw_error: &str) -> String {
        let redacted = Self::redact_sensitive_tokens(raw_error);
        redacted.replace('\n', "\\n").replace('\t', "\\t")
    }

    fn redact_sensitive_tokens(msg: &str) -> String {
        let mut res = msg.to_string();
        for prefix in &["sk-", "Bearer "] {
            let mut search_idx = 0;
            while let Some(idx) = res[search_idx..].find(prefix) {
                let start = search_idx + idx + prefix.len();
                let end = res[start..]
                    .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '\n')
                    .map(|e| start + e)
                    .unwrap_or(res.len());
                if end > start {
                    res.replace_range(start..end, "[REDACTED]");
                    search_idx = start + 10;
                } else {
                    search_idx = start;
                }
                if search_idx >= res.len() {
                    break;
                }
            }
        }
        res
    }
}
