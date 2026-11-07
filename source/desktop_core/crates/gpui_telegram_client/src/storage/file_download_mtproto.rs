//! Target Symbol: FileDownloadMtproto
//! Telegram Desktop 1:1 Submodule: `FileDownloadMtproto`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::path::PathBuf;
use super::file_download::FileLoader;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MtprotoFileLocation {
    pub dc_id: i32,
    pub volume_id: u64,
    pub local_id: i32,
    pub secret: u64,
    pub file_reference: Vec<u8>,
}

#[derive(Debug)]
pub struct FileDownloadMtproto {
    pub location: MtprotoFileLocation,
    pub loader: FileLoader,
    pub chunk_size: usize,
    pub next_offset: i64,
}

impl FileDownloadMtproto {
    pub fn new(
        location: MtprotoFileLocation,
        destination_path: impl Into<PathBuf>,
        full_size: i64,
    ) -> Self {
        Self {
            location,
            loader: FileLoader::new(destination_path, full_size, false, 0),
            chunk_size: 128 * 1024,
            next_offset: 0,
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        self.loader.start()
    }

    pub fn next_chunk_offset(&mut self) -> Option<i64> {
        if self.next_offset >= self.loader.full_size && self.loader.full_size > 0 {
            None
        } else {
            let offset = self.next_offset;
            self.next_offset += self.chunk_size as i64;
            Some(offset)
        }
    }

    pub fn feed_chunk(&mut self, offset: i64, data: &[u8]) -> Result<(), String> {
        self.loader.write_part(offset, data)
    }

    pub fn refresh_file_reference(&mut self, new_ref: Vec<u8>) {
        self.location.file_reference = new_ref;
    }

    pub fn cancel(&mut self) {
        self.loader.cancel();
    }

    pub fn is_finished(&self) -> bool {
        self.loader.finished
    }
}
