//! Target Symbol: DataDocumentResolver
//! Telegram Desktop 1:1 Submodule: `data/data_document_resolver`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewerType {
    InternalPhoto,
    InternalVideo,
    InternalAudio,
    InternalPdf,
    ExternalSystemDefault,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataDocumentResolver {
    extension_map: HashMap<String, String>,
}

impl Default for DataDocumentResolver {
    fn default() -> Self {
        let mut map = HashMap::new();
        // Common audio
        map.insert("mp3".to_string(), "audio/mpeg".to_string());
        map.insert("ogg".to_string(), "audio/ogg".to_string());
        map.insert("oga".to_string(), "audio/ogg".to_string());
        map.insert("opus".to_string(), "audio/opus".to_string());
        map.insert("m4a".to_string(), "audio/m4a".to_string());
        map.insert("flac".to_string(), "audio/flac".to_string());
        map.insert("wav".to_string(), "audio/wav".to_string());

        // Common video
        map.insert("mp4".to_string(), "video/mp4".to_string());
        map.insert("webm".to_string(), "video/webm".to_string());
        map.insert("mkv".to_string(), "video/x-matroska".to_string());
        map.insert("mov".to_string(), "video/quicktime".to_string());
        map.insert("avi".to_string(), "video/x-msvideo".to_string());

        // Common images
        map.insert("jpg".to_string(), "image/jpeg".to_string());
        map.insert("jpeg".to_string(), "image/jpeg".to_string());
        map.insert("png".to_string(), "image/png".to_string());
        map.insert("webp".to_string(), "image/webp".to_string());
        map.insert("gif".to_string(), "image/gif".to_string());
        map.insert("tgs".to_string(), "application/x-tgsticker".to_string());
        map.insert("svg".to_string(), "image/svg+xml".to_string());

        // Documents
        map.insert("pdf".to_string(), "application/pdf".to_string());
        map.insert("zip".to_string(), "application/zip".to_string());
        map.insert("tar".to_string(), "application/x-tar".to_string());
        map.insert("gz".to_string(), "application/gzip".to_string());
        map.insert("txt".to_string(), "text/plain".to_string());
        map.insert("json".to_string(), "application/json".to_string());

        Self { extension_map: map }
    }
}

impl DataDocumentResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve_mime(&self, filename: &str) -> String {
        if let Some(pos) = filename.rfind('.') {
            let ext = filename[pos + 1..].to_ascii_lowercase();
            if let Some(mime) = self.extension_map.get(&ext) {
                return mime.clone();
            }
        }
        "application/octet-stream".to_string()
    }

    pub fn resolve_viewer(&self, mime: &str) -> ViewerType {
        if mime.starts_with("image/") {
            ViewerType::InternalPhoto
        } else if mime.starts_with("video/") {
            ViewerType::InternalVideo
        } else if mime.starts_with("audio/") {
            ViewerType::InternalAudio
        } else if mime == "application/pdf" {
            ViewerType::InternalPdf
        } else {
            ViewerType::ExternalSystemDefault
        }
    }

    pub fn can_preview_inline(&self, mime: &str) -> bool {
        self.resolve_viewer(mime) != ViewerType::ExternalSystemDefault
    }
}
