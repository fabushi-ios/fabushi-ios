//! Upstream Sources:
//! - `Telegram/SourceFiles/media/clip/media_clip_check_streaming.h:L1-L22`
//! - `Telegram/SourceFiles/media/clip/media_clip_check_streaming.cpp:L1-L97`
//! Parity Module: `media_clip_check_streaming` | Feature: `TCG-N52` (MP4 Fast-Start Moov Atom Validator)

use serde::{Deserialize, Serialize};

/// `media_clip_check_streaming.cpp:L22-L23`
pub const K_HEADER_SIZE: usize = 8;
pub const K_FIND_MOOV_BEFORE: usize = 128 * 1024;

/// `media_clip_check_streaming.cpp:L39-L94`
pub fn check_streaming_support(data: &[u8]) -> bool {
    let size = data.len();
    if size < K_HEADER_SIZE {
        return false;
    }

    let mut pos = 0;
    while pos + K_HEADER_SIZE <= size {
        if pos >= K_FIND_MOOV_BEFORE {
            return false;
        }

        let atom_size_32 = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let atom_name = &data[pos + 4..pos + 8];

        if atom_name == b"moov" {
            return true;
        }

        let length = if atom_size_32 == 1 {
            // 64-bit atom length
            if pos + 16 > size {
                break;
            }
            u64::from_be_bytes([
                data[pos + 8],
                data[pos + 9],
                data[pos + 10],
                data[pos + 11],
                data[pos + 12],
                data[pos + 13],
                data[pos + 14],
                data[pos + 15],
            ]) as usize
        } else if atom_size_32 == 0 {
            // Extends to EOF
            size - pos
        } else {
            atom_size_32 as usize
        };

        if length < K_HEADER_SIZE || pos + length > size {
            break;
        }
        pos += length;
    }

    false
}

/// Ledger Target Symbol: `MediaClipCheckStreamingClip`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaClipCheckStreamingClip {
    pub max_search_offset: usize,
    pub is_supported: bool,
    pub inspected_bytes: usize,
}

impl Default for MediaClipCheckStreamingClip {
    fn default() -> Self {
        Self {
            max_search_offset: K_FIND_MOOV_BEFORE,
            is_supported: false,
            inspected_bytes: 0,
        }
    }
}

impl MediaClipCheckStreamingClip {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn check(&mut self, data: &[u8]) -> bool {
        self.inspected_bytes = data.len();
        self.is_supported = check_streaming_support(data);
        self.is_supported
    }
}
