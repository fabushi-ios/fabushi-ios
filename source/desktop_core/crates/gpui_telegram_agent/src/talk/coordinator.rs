use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub use fabushi_messaging_core::realtime::{CallKind, CallSession, CallSignal, CallState};

pub const MAX_AUDIO_BUFFER_CHUNKS: usize = 320; // 1 MiB boundary per OCRM-AUDIO-O05

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TalkSessionState {
    Idle,
    Listening,
    Thinking,
    Speaking,
    Interrupted,
    Reconnecting,
    Closed,
}

pub struct TalkCoordinator {
    pub state: TalkSessionState,
    pub session_id: String,
    pub generation: u64,
    pub signal_seq: u64,
    pub audio_buffer_queue: VecDeque<Vec<u8>>,
    pub total_flushed_chunks: usize,
}

impl Default for TalkCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl TalkCoordinator {
    pub fn new() -> Self {
        Self {
            state: TalkSessionState::Idle,
            session_id: uuid::Uuid::new_v4().to_string(),
            generation: 0,
            signal_seq: 0,
            audio_buffer_queue: VecDeque::new(),
            total_flushed_chunks: 0,
        }
    }

    /// <60ms VAD barge-in handler:
    /// Instantly clears the 320-chunk (1 MiB) client playback buffer and cancels TTS
    /// Transitions from Speaking to Interrupted
    pub fn handle_barge_in(&mut self) -> usize {
        if self.state == TalkSessionState::Speaking || self.state == TalkSessionState::Thinking {
            self.state = TalkSessionState::Interrupted;
            let flushed = self.audio_buffer_queue.len();
            self.audio_buffer_queue.clear();
            self.total_flushed_chunks += flushed;
            flushed
        } else {
            0
        }
    }

    /// Resumes listening phase after user interrupt is acknowledged
    pub fn resume_listening(&mut self) {
        if self.state == TalkSessionState::Interrupted {
            self.state = TalkSessionState::Listening;
        }
    }

    /// Enqueue an audio chunk into the playback queue.
    /// Strictly limits queue size to MAX_AUDIO_BUFFER_CHUNKS (320 chunks / 1 MiB boundary per OCRM-AUDIO-O05).
    pub fn enqueue_audio(&mut self, chunk: Vec<u8>) -> Result<(), &'static str> {
        if self.state == TalkSessionState::Closed {
            return Err("cannot enqueue audio to a closed session");
        }
        if self.audio_buffer_queue.len() >= MAX_AUDIO_BUFFER_CHUNKS {
            self.audio_buffer_queue.pop_front();
        }
        self.audio_buffer_queue.push_back(chunk);
        Ok(())
    }

    /// Dequeue next audio chunk for playback
    pub fn dequeue_audio(&mut self) -> Option<Vec<u8>> {
        self.audio_buffer_queue.pop_front()
    }

    pub fn buffered_chunks_count(&self) -> usize {
        self.audio_buffer_queue.len()
    }

    /// Reconnect lifecycle per OCRM-AUDIO-O01:
    /// Increments generation fence, resets signal sequence, transitions to Reconnecting
    pub fn reconnect(&mut self) -> u64 {
        self.state = TalkSessionState::Reconnecting;
        self.generation = self.generation.saturating_add(1);
        self.signal_seq = 0;
        self.generation
    }

    /// Advances and returns the next signal sequence per OCRM-AUDIO-O02
    pub fn next_signal_seq(&mut self) -> u64 {
        self.signal_seq = self.signal_seq.saturating_add(1);
        self.signal_seq
    }

    /// Validate FSM transitions enforcing terminal session invariant per OCRM-AUDIO-O04
    pub fn transition(&mut self, next: TalkSessionState) -> Result<(), &'static str> {
        if self.state == TalkSessionState::Closed {
            return Err("terminal session mutation invariant: closed session cannot transition");
        }
        self.state = next;
        Ok(())
    }

    /// Terminate session and flush any remaining audio buffers
    pub fn close(&mut self) {
        self.state = TalkSessionState::Closed;
        self.audio_buffer_queue.clear();
    }
}
