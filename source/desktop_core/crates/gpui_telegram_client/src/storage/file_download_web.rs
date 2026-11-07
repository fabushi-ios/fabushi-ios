//! Target Symbol: FileDownloadWeb
//! Telegram Desktop 1:1 Submodule: `FileDownloadWeb`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::path::PathBuf;
use super::file_download::FileLoader;

#[derive(Debug)]
pub struct FileDownloadWeb {
    pub url: String,
    pub loader: FileLoader,
    pub max_redirects: usize,
    pub redirected_count: usize,
}

impl FileDownloadWeb {
    pub fn new(url: impl Into<String>, destination_path: impl Into<PathBuf>, full_size: i64) -> Self {
        Self {
            url: url.into(),
            loader: FileLoader::new(destination_path, full_size, false, 0),
            max_redirects: 5,
            redirected_count: 0,
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        self.loader.start()
    }

    pub fn handle_redirect(&mut self, new_url: String) -> Result<(), String> {
        if self.redirected_count >= self.max_redirects {
            return Err("Too many HTTP redirects".to_string());
        }
        self.url = new_url;
        self.redirected_count += 1;
        Ok(())
    }

    pub fn feed_bytes(&mut self, offset: i64, data: &[u8]) -> Result<(), String> {
        self.loader.write_part(offset, data)
    }

    pub fn finish(&mut self) -> Result<(), String> {
        self.loader.finalize()
    }

    pub fn cancel(&mut self) {
        self.loader.cancel();
    }
}
