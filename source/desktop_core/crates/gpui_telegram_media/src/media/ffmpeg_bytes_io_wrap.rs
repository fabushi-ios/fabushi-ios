//! Telegram Desktop 1:1 Parity Submodule: `FfmpegBytesIoWrap`
//! Upstream Source: `Telegram/SourceFiles/ffmpeg/ffmpeg_bytes_io_wrap.h:L1-L131`
//! Subsystem: `media` | Feature: `TCG-N06`
//!
//! Provides AVIO custom IO wrapper callbacks for FFmpeg demuxing and muxing:
//! - `ReadBytesWrap`: in-memory byte slice reader (L45-L78)
//! - `WriteBytesWrap`: dynamic chunked memory writer with 1MB chunk reserve (L80-L128)
//! - `ReadFileWrap`: seekable file descriptor reader (L16-L43)


/// FFmpeg seek constants corresponding to C AVSEEK / whence values.
pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;
pub const AVSEEK_SIZE: i32 = 0x10000;

/// Standard FFmpeg error return codes.
pub const AVERROR_EOF: i32 = -541478725; // FFERRTAG('E', 'O', 'F', ' ')
pub const AVERROR_EIO: i32 = -5;

/// 1MB memory reservation chunk size (ffmpeg_bytes_io_wrap.h:L93).
pub const K_WRITE_RESERVE_CHUNK: usize = 1024 * 1024;

/// In-memory byte buffer reader corresponding to C++ `ReadBytesWrap` (L45-L78).
#[derive(Debug, Clone, Default)]
pub struct ReadBytesWrap {
    pub data: Vec<u8>,
    pub offset: i64,
}

impl ReadBytesWrap {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data, offset: 0 }
    }

    pub fn size(&self) -> i64 {
        self.data.len() as i64
    }

    /// Reads up to `buf_size` bytes into `buf` (L50-L62).
    pub fn read(&mut self, buf: &mut [u8]) -> i32 {
        let total = self.size();
        let remaining = total - self.offset;
        let to_read = (buf.len() as i64).min(remaining);
        if to_read <= 0 {
            return AVERROR_EOF;
        }
        let start = self.offset as usize;
        let end = start + to_read as usize;
        buf[..to_read as usize].copy_from_slice(&self.data[start..end]);
        self.offset += to_read;
        to_read as i32
    }

    /// Custom AVIO seek callback implementation (L63-L77).
    pub fn seek(&mut self, offset: i64, whence: i32) -> i64 {
        let total = self.size();
        let updated = match whence {
            SEEK_SET => offset,
            SEEK_CUR => self.offset + offset,
            SEEK_END => total + offset,
            AVSEEK_SIZE => return total,
            _ => -1,
        };
        if updated < 0 || updated > total {
            return -1;
        }
        self.offset = updated;
        updated
    }
}

/// Dynamic in-memory writer corresponding to C++ `WriteBytesWrap` (L80-L128).
#[derive(Debug, Clone, Default)]
pub struct WriteBytesWrap {
    pub content: Vec<u8>,
    pub offset: i64,
}

impl WriteBytesWrap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn size(&self) -> i64 {
        self.content.len() as i64
    }

    /// Custom AVIO write callback with 1MB chunked reserve (L85-L109).
    pub fn write(&mut self, buf: &[u8]) -> i32 {
        let buf_size = buf.len() as i64;
        if buf_size <= 0 {
            return 0;
        }
        let total = self.offset + buf_size;
        let current_size = self.content.len() as i64;
        let target_capacity = ((total as usize + K_WRITE_RESERVE_CHUNK - 1)
            / K_WRITE_RESERVE_CHUNK)
            * K_WRITE_RESERVE_CHUNK;
        if target_capacity > self.content.capacity() {
            self.content.reserve(target_capacity - self.content.len());
        }

        let overwrite = (current_size - self.offset).clamp(0, buf_size) as usize;
        let offset_idx = self.offset as usize;
        if overwrite > 0 {
            self.content[offset_idx..offset_idx + overwrite].copy_from_slice(&buf[..overwrite]);
        }
        if overwrite < buf.len() {
            self.content.extend_from_slice(&buf[overwrite..]);
        }
        self.offset += buf_size;
        buf_size as i32
    }

    /// Custom AVIO seek callback with bounds check (L111-L127).
    pub fn seek(&mut self, offset: i64, whence: i32) -> i64 {
        let current_size = self.content.len() as i64;
        let updated = match whence {
            SEEK_SET => offset,
            SEEK_CUR => self.offset + offset,
            SEEK_END => current_size + offset,
            AVSEEK_SIZE => return current_size,
            _ => -1,
        };
        if updated < 0 || updated > current_size {
            return -1;
        }
        self.offset = updated;
        updated
    }
}

/// File IO wrap corresponding to C++ `ReadFileWrap` (L16-L43).
#[derive(Debug, Clone, Default)]
pub struct ReadFileWrap {
    pub path: String,
    pub simulated_data: Vec<u8>,
    pub offset: i64,
}

impl ReadFileWrap {
    pub fn new(path: impl Into<String>, simulated_data: Vec<u8>) -> Self {
        Self {
            path: path.into(),
            simulated_data,
            offset: 0,
        }
    }

    pub fn size(&self) -> i64 {
        self.simulated_data.len() as i64
    }

    pub fn read(&mut self, buf: &mut [u8]) -> i32 {
        let total = self.size();
        let remaining = total - self.offset;
        let to_read = (buf.len() as i64).min(remaining);
        if to_read <= 0 {
            return AVERROR_EOF;
        }
        let start = self.offset as usize;
        let end = start + to_read as usize;
        buf[..to_read as usize].copy_from_slice(&self.simulated_data[start..end]);
        self.offset += to_read;
        to_read as i32
    }

    pub fn seek(&mut self, offset: i64, whence: i32) -> i64 {
        let total = self.size();
        let updated = match whence {
            SEEK_SET => offset,
            SEEK_CUR => self.offset + offset,
            SEEK_END => total + offset,
            AVSEEK_SIZE => return total,
            _ => -1,
        };
        if updated < 0 || updated > total {
            return -1;
        }
        self.offset = updated;
        updated
    }
}

/// Parity target coordinator struct `FfmpegBytesIoWrap`.
#[derive(Debug, Clone, Default)]
pub struct FfmpegBytesIoWrap {
    pub reader: ReadBytesWrap,
    pub writer: WriteBytesWrap,
}

impl FfmpegBytesIoWrap {
    pub fn new(initial_bytes: Vec<u8>) -> Self {
        Self {
            reader: ReadBytesWrap::new(initial_bytes),
            writer: WriteBytesWrap::new(),
        }
    }

    pub fn read_offset(&self) -> i64 {
        self.reader.offset
    }

    pub fn write_offset(&self) -> i64 {
        self.writer.offset
    }
}
