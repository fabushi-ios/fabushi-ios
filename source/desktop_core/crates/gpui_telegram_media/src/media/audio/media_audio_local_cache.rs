//! Telegram Desktop 1:1 Parity Submodule: `media_audio_local_cache.rs`
//! Subsystem: `media` | Feature: `TCG-N06` (Local Sound WAV Cache for UI/Notifications)
//! Upstream: `Telegram/SourceFiles/media/audio/media_audio_local_cache.h:L12-L45`
//!           `Telegram/SourceFiles/media/audio/media_audio_local_cache.cpp:L16-L18, L37-L60`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Upstream: `media_audio_local_cache.cpp:L16-L17`
pub const LOCAL_CACHE_MAX_DURATION_MS: i64 = 3000;
pub const LOCAL_CACHE_FRAME_SIZE: usize = 4096;

/// Upstream: `media_audio_local_cache.h:L12-L19`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LocalSound {
    pub id: u64,
    pub wav_bytes: Vec<u8>,
}

impl LocalSound {
    pub fn is_valid(&self) -> bool {
        !self.wav_bytes.is_empty()
    }
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MediaAudioLocalCacheAudio {
    pub memory_cache: HashMap<u64, Vec<u8>>,
    pub base_disk_path: String,
}

impl MediaAudioLocalCacheAudio {
    pub fn new() -> Self {
        Self {
            memory_cache: HashMap::new(),
            base_disk_path: String::new(),
        }
    }

    pub fn with_disk_path(path: String) -> Self {
        Self {
            memory_cache: HashMap::new(),
            base_disk_path: path,
        }
    }

    pub fn get(&self, id: u64) -> Option<&[u8]> {
        self.memory_cache.get(&id).map(|v| v.as_slice())
    }

    pub fn insert(&mut self, id: u64, bytes: Vec<u8>) {
        self.memory_cache.insert(id, bytes);
    }

    pub fn disk_path_for_id(&self, id: u64) -> String {
        format!("{}/{}.wav", self.base_disk_path.trim_end_matches('/'), id)
    }

    pub fn clear(&mut self) {
        self.memory_cache.clear();
    }
}
