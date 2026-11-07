pub mod transport;
pub mod user_computer;
pub mod product_analytics;
pub mod sand_analytics_types;
pub mod r#box;

pub mod telemetry;

pub mod mcp_state_executor;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const REALTIME_VOICE_MAX_PENDING_AUDIO_CHUNKS: usize = 320;
pub const REALTIME_VOICE_MAX_PENDING_AUDIO_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioOutputPortError {
    Closed,
    QueueFull,
}

/// Bounded Host owner for realtime PCM output.
///
/// The close fence is a 32-bit atomic value so a playback worker can revoke
/// the port without racing a producer. Queue admission checks the fence both
/// before and after taking the queue lock; a frame racing with close is never
/// admitted after revocation becomes visible.
pub struct AudioOutputPort {
    close_fence: Arc<AtomicI32>,
    pending_frames: Mutex<VecDeque<Vec<u8>>>,
    pending_bytes: AtomicUsize,
    overflow_count: AtomicUsize,
}

impl Default for AudioOutputPort {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioOutputPort {
    pub fn new() -> Self {
        Self {
            close_fence: Arc::new(AtomicI32::new(0)),
            pending_frames: Mutex::new(VecDeque::with_capacity(
                REALTIME_VOICE_MAX_PENDING_AUDIO_CHUNKS,
            )),
            pending_bytes: AtomicUsize::new(0),
            overflow_count: AtomicUsize::new(0),
        }
    }

    pub fn close(&self) {
        self.close_fence.store(1, Ordering::SeqCst);
    }

    pub fn is_closed(&self) -> bool {
        self.close_fence.load(Ordering::SeqCst) == 1
    }

    pub fn fence(&self) -> Arc<AtomicI32> {
        Arc::clone(&self.close_fence)
    }

    pub fn send_audio(&self, frame: Vec<u8>) -> Result<(), AudioOutputPortError> {
        if self.is_closed() {
            return Err(AudioOutputPortError::Closed);
        }
        let mut pending = self
            .pending_frames
            .lock()
            .map_err(|_| AudioOutputPortError::Closed)?;
        if self.is_closed() {
            return Err(AudioOutputPortError::Closed);
        }
        let current_bytes = self.pending_bytes.load(Ordering::Acquire);
        if pending.len() >= REALTIME_VOICE_MAX_PENDING_AUDIO_CHUNKS
            || frame.len() > REALTIME_VOICE_MAX_PENDING_AUDIO_BYTES
            || current_bytes.saturating_add(frame.len())
                > REALTIME_VOICE_MAX_PENDING_AUDIO_BYTES
        {
            self.overflow_count.fetch_add(1, Ordering::Relaxed);
            return Err(AudioOutputPortError::QueueFull);
        }
        self.pending_bytes
            .store(current_bytes + frame.len(), Ordering::Release);
        pending.push_back(frame);
        Ok(())
    }

    pub fn pop_audio(&self) -> Option<Vec<u8>> {
        let mut pending = self.pending_frames.lock().ok()?;
        let frame = pending.pop_front()?;
        self.pending_bytes
            .fetch_sub(frame.len(), Ordering::Release);
        Some(frame)
    }

    pub fn pending_chunk_count(&self) -> usize {
        self.pending_frames.lock().map(|queue| queue.len()).unwrap_or(0)
    }

    pub fn pending_byte_count(&self) -> usize {
        self.pending_bytes.load(Ordering::Acquire)
    }

    pub fn overflow_count(&self) -> usize {
        self.overflow_count.load(Ordering::Acquire)
    }
}
