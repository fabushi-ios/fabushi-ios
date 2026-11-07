pub mod agent_v1_wire;
pub mod heartbeat;
pub mod tools;
pub mod video_container;
pub mod site_visit_tracking;
pub mod sand_prompt_markers;
pub mod conversation_outline;
pub mod clock_skew_guard;
pub mod agent_state;
mod checkpoint;
mod turn_usage;
mod tool_call_identity;
mod conversation_state;
pub mod compaction_planning;
mod stream_attempt;
mod transient_stream_error;
mod turn_settle;
mod turn_run_shell;

pub const EXACT_SPEECH_PREFIX: &str = "Internal OpenClaw voice playback result.";
pub const EXACT_SPEECH_DIRECTIVE: &str =
    "Do not call openclaw_agent_consult or any other tool for this message.";
pub const EXACT_SPEECH_INSTRUCTION: &str = "Speak this exact OpenClaw answer to ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactSpeechClassification {
    ExactSpeechEcho(String),
    PeerConsult(String),
    Malformed(&'static str),
}

/// Classify a voice-consult payload without owning dispatch or retained state.
pub fn classify_realtime_voice_consult_tool_call(
    text: &str,
    retained_exact_speech: &[String],
) -> ExactSpeechClassification {
    if !text.contains(EXACT_SPEECH_PREFIX) {
        return ExactSpeechClassification::PeerConsult(text.to_string());
    }
    if !text.contains(EXACT_SPEECH_DIRECTIVE) || !text.contains(EXACT_SPEECH_INSTRUCTION) {
        return ExactSpeechClassification::Malformed("missing required exact speech sentinels");
    }

    let Some(answer_pos) = text.find("Answer: \"") else {
        return ExactSpeechClassification::Malformed("missing Answer: delimiter");
    };
    let rest = &text[answer_pos + "Answer: \"".len()..];
    let Some(end_quote) = rest.rfind('"') else {
        return ExactSpeechClassification::Malformed("unterminated Answer string");
    };
    let answer = &rest[..end_quote];
    if retained_exact_speech.iter().any(|retained| retained == answer) {
        ExactSpeechClassification::ExactSpeechEcho(answer.to_string())
    } else {
        ExactSpeechClassification::PeerConsult(answer.to_string())
    }
}

pub const REALTIME_VOICE_MAX_TURN_CONTEXTS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeVoiceTurnContext {
    pub turn_id: String,
    pub has_audio: bool,
    pub closed: bool,
    pub response_active: bool,
    pub cancel_sent: bool,
}

/// Bounded state owner for future realtime voice turn producers.
#[derive(Debug, Default)]
pub struct TurnContextTracker {
    turns: std::collections::VecDeque<RealtimeVoiceTurnContext>,
}

impl TurnContextTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_turn(&mut self, turn_id: impl Into<String>) -> bool {
        let turn_id = turn_id.into();
        if turn_id.trim().is_empty() {
            return false;
        }
        if self.turns.len() >= REALTIME_VOICE_MAX_TURN_CONTEXTS {
            self.turns.pop_front();
        }
        self.turns.push_back(RealtimeVoiceTurnContext {
            turn_id,
            has_audio: false,
            closed: false,
            response_active: true,
            cancel_sent: false,
        });
        true
    }

    pub fn mark_audio(&mut self, turn_id: &str) -> bool {
        let Some(turn) = self.turns.iter_mut().find(|turn| turn.turn_id == turn_id) else {
            return false;
        };
        turn.has_audio = true;
        true
    }

    pub fn close_turn(&mut self, turn_id: &str) -> bool {
        let Some(turn) = self.turns.iter_mut().find(|turn| turn.turn_id == turn_id) else {
            return false;
        };
        turn.closed = true;
        turn.response_active = false;
        true
    }

    pub fn handle_barge_in(&mut self) -> bool {
        let mut cancelled = false;
        for turn in &mut self.turns {
            if turn.response_active {
                turn.response_active = false;
                turn.cancel_sent = true;
                cancelled = true;
            }
        }
        cancelled
    }

    pub fn consume_audio_context(&mut self) -> Vec<RealtimeVoiceTurnContext> {
        self.turns.retain(|turn| !(turn.closed && !turn.has_audio));
        self.turns.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.turns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }
}

pub const REALTIME_TRANSCRIPTION_MAX_BUFFERED_BYTES: usize = 1_048_576;
pub const REALTIME_TRANSCRIPTION_MAX_SDP_LINES: usize = 4_096;
pub const REALTIME_TRANSCRIPTION_MAX_SDP_LINE_BYTES: usize = 4_096;
pub const REALTIME_TRANSCRIPTION_MAX_MEDIA_SECTIONS: usize = 8;

#[derive(Debug, Default)]
pub struct RealtimeVoiceAudioQueue {
    chunks: std::collections::VecDeque<Vec<u8>>,
    buffered_bytes: usize,
    overflow_count: usize,
}

impl RealtimeVoiceAudioQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: Vec<u8>) -> bool {
        if self.buffered_bytes.saturating_add(chunk.len())
            > REALTIME_TRANSCRIPTION_MAX_BUFFERED_BYTES
        {
            self.overflow_count = self.overflow_count.saturating_add(1);
            return false;
        }
        self.buffered_bytes += chunk.len();
        self.chunks.push_back(chunk);
        true
    }

    pub fn pop(&mut self) -> Option<Vec<u8>> {
        let chunk = self.chunks.pop_front()?;
        self.buffered_bytes = self.buffered_bytes.saturating_sub(chunk.len());
        Some(chunk)
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn buffered_bytes(&self) -> usize {
        self.buffered_bytes
    }

    pub fn overflow_count(&self) -> usize {
        self.overflow_count
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

pub fn validate_realtime_transcription_sdp(sdp: &str) -> Result<(), &'static str> {
    let lines: Vec<&str> = sdp.lines().collect();
    if lines.len() > REALTIME_TRANSCRIPTION_MAX_SDP_LINES {
        return Err("SDP offer exceeds maximum line count of 4096");
    }
    let mut media_count = 0;
    let mut has_active_audio = false;
    for line in lines {
        if line.len() > REALTIME_TRANSCRIPTION_MAX_SDP_LINE_BYTES {
            return Err("SDP line exceeds 4096 bytes");
        }
        let Some(rest) = line.strip_prefix("m=") else {
            continue;
        };
        media_count += 1;
        if media_count > REALTIME_TRANSCRIPTION_MAX_MEDIA_SECTIONS {
            return Err("SDP offer exceeds maximum of 8 media sections");
        }
        if rest.starts_with("video") {
            return Err("video media sections are rejected by audio gateway");
        }
        if rest.starts_with("audio") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.get(1).is_some_and(|port| *port != "0") {
                has_active_audio = true;
            }
        }
    }
    if has_active_audio {
        Ok(())
    } else {
        Err("SDP offer requires at least one active audio media stream")
    }
}

pub fn validate_transcription_request(
    audio: &[u8],
    mime_type: &str,
) -> Result<(), &'static str> {
    if audio.is_empty() {
        return Err("empty audio buffer: SandTranscribeEmptyAudioError");
    }
    if !mime_type.starts_with("audio/") {
        return Err("invalid audio MIME type");
    }
    Ok(())
}

pub fn is_cjk_dominant(text: &str) -> bool {
    let total_chars = text.chars().count();
    if total_chars == 0 {
        return false;
    }
    let cjk_count = text
        .chars()
        .filter(|&character| {
            matches!(
                character,
                '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}' | '\u{20000}'..='\u{2a6df}'
            )
        })
        .count();
    (cjk_count as f64 / total_chars as f64) > 0.30
}

pub fn resolve_edge_tts_voice(text: &str, default_voice: &str) -> (&'static str, &'static str) {
    if is_cjk_dominant(text) {
        ("zh-CN-XiaoxiaoNeural", "zh-CN")
    } else {
        let voice = match default_voice {
            "en-US-MichelleNeural" => "en-US-MichelleNeural",
            _ => "en-US-MichelleNeural",
        };
        (voice, "en-US")
    }
}

pub use checkpoint::AttemptCheckpoint;
pub use stream_attempt::{
    AttemptProgress, OuterCheckpointDisposition, OuterStreamFuture,
    OuterStreamPersistence, RetryDecision, StreamAttemptGeneration,
    StreamAttemptPolicy, StreamAttemptRuntime,
    StreamCancelReason, StreamWatchdog, persist_outer_stream_checkpoint,
    persist_outer_stream_final_state, release_outer_stream_persistence,
};
pub use transient_stream_error::{StreamFailureKind, TransientStreamError};
pub use turn_settle::{
    DurableTurnCheckpointStore, SettledTokenDetails, TerminalOutcome,
    TokenDetailsPersistenceTracker, TranscriptCheckpointMirror, TurnCheckpointFuture,
    TurnCheckpointPersistenceError, TurnSettlement, persist_checkpoint_with_mirror,
    should_run_completed_turn_side_effects, should_run_turn_memory,
};
pub use turn_run_shell::{
    CheckpointBoundary, TurnCancellation, TurnOwnerToken, TurnRunFinished,
    ToolLoopDecision, ToolLoopGate, TurnRunOptions, TurnRunShell, TurnRunShellError, TurnRunStarted,
    TOOL_LOOP_HISTORY_CAPACITY, TOOL_LOOP_REPEAT_THRESHOLD, is_recovery_shaped_turn,
};
pub use conversation_state::{
    CompletedAwaitOutcome, ContextWindowTracker, FullStreamSanitizer, HIDDEN_PROMPT_MARKER,
    ModelResolutionTicket, RecentUserMessage, RecoveryUserMessage, ResolvedModelTracker, SUMMARIZATION_MAX_OUTPUT_TOKENS,
    SUMMARIZATION_MAX_PROMPT_CHARS, SanitizedExtendedUsage, SanitizedUsage, StreamSanitizerItem,
    SummarizationPolicy, await_block_until_ms, build_unanswered_questions_note,
    classify_completed_await_outcome, sanitize_usage, select_unconfirmed_user_messages,
    should_use_self_summary, summarization_policy, to_safe_usage_count,
    would_recover_via_prepend,
};
pub use tool_call_identity::{ToolCallIdentity, ToolSurfaceUpdate, TOOL_CALL_IDENTITY_CAP};
pub use turn_usage::{
    MAX_SAFE_TOKEN_COUNT, TurnEndedUsage, TurnUsage, add_token_counts, merge_turn_usage,
    to_safe_token_count, total_input_tokens, turn_usage_from_turn_ended,
};

pub mod sand_agent_profile_prompt;
pub mod sand_action_audit;
pub mod sand_auto_review;
pub mod sand_auto_review_classifier_run;
pub mod sand_auto_review_summaries;
pub mod bot_block_detection;
pub mod background_work;
pub mod auto_review_gate;
pub mod sand_auto_review_tool_escalations;
pub mod sand_automation_auto_review;
pub mod sand_browser_auto_review;
pub mod sand_cloud_agent_auto_review;
pub mod sand_computer_auto_review;
pub mod sand_shell_auto_review_enrichment;
pub mod sand_subagent_auto_review;

pub mod send_message_reminder_middleware;
pub mod start_of_turn_ack_reminder_middleware;
pub mod turn_tool_session_reminders;
pub mod turn_shape;
pub mod routed_provider_runtime;
pub mod coordinator_tool_relay;
pub mod large_output_spill;
pub mod box_tool_access;
pub mod box_reference_docs;
pub mod production_turn_run_shell_adapter;
pub mod process_tree;
pub mod bounded_line_framer;
pub mod inactive_turn_agent_stream;
pub mod turn_agent_composition;
pub mod production_turn_agent_owner;
pub mod production_agent_checkpoint;
pub mod generated_agent_turn_stream;
pub mod production_turn_input_projection;
pub mod prompt_collector_glue;
pub mod sand_agent_runner;
pub mod system_prompt;
pub mod system_prompt_assembly;
pub mod sand_memory;
pub mod turn_observation;
pub mod turn_memory;

pub mod subagent_runtime;
pub mod agent_adapters;

pub mod computer_use;

pub mod runner_prompt_glue;

pub mod shell_terminal_watch;

pub mod remote_box_resources;

pub mod host_computer_tool_dependencies;
pub mod host_file_transfer_dependencies;
pub mod host_external_machine_dependencies;

pub mod host_web_dependencies;
pub mod host_generate_image_dependencies;
