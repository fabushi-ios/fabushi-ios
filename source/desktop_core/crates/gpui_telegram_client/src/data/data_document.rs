//! Target Symbol: DataDocument
//! Telegram Desktop 1:1 Submodule: `data/data_document`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DocumentType {
    #[default]
    File,
    Sticker,
    Song,
    Voice,
    Video,
    RoundVideo,
    Animated,
    WallPaper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StickerType {
    #[default]
    Webp,
    Tgs,
    Webm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StickerInfo {
    pub alt: String,
    pub set_id: u64,
    pub sticker_type: StickerType,
    pub is_premium: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SongInfo {
    pub title: String,
    pub performer: String,
    pub duration_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VoiceInfo {
    pub waveform: Vec<u8>,
    pub duration_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VideoQuality {
    pub width: u32,
    pub height: u32,
    pub codec: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VideoInfo {
    pub duration_seconds: u32,
    pub codec: String,
    pub qualities: Vec<VideoQuality>,
    pub supports_streaming: bool,
    pub is_silent: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FileLoadingStatus {
    Idle,
    Loading { progress: f64, offset: i64 },
    Ready,
    Failed { reason: String },
}

impl Default for FileLoadingStatus {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataDocument {
    pub id: u64,
    pub size: i64,
    pub dc_id: i32,
    pub access_hash: u64,
    pub file_reference: Vec<u8>,
    pub filename: String,
    pub mime_type: String,
    pub dimensions: (u32, u32),
    pub date: i64,
    pub doc_type: DocumentType,
    pub status: FileLoadingStatus,
    pub sticker: Option<StickerInfo>,
    pub song: Option<SongInfo>,
    pub voice: Option<VoiceInfo>,
    pub video: Option<VideoInfo>,
    pub has_thumbnail: bool,
    pub thumbnail_bytes: Option<Vec<u8>>,
}

impl Default for DataDocument {
    fn default() -> Self {
        Self {
            id: 0,
            size: 0,
            dc_id: 1,
            access_hash: 0,
            file_reference: Vec::new(),
            filename: String::new(),
            mime_type: "application/octet-stream".to_string(),
            dimensions: (0, 0),
            date: 0,
            doc_type: DocumentType::File,
            status: FileLoadingStatus::Idle,
            sticker: None,
            song: None,
            voice: None,
            video: None,
            has_thumbnail: false,
            thumbnail_bytes: None,
        }
    }
}

impl DataDocument {
    pub fn new(id: u64, size: i64, mime_type: impl Into<String>) -> Self {
        let mime = mime_type.into();
        let doc_type = Self::detect_type(&mime);
        Self {
            id,
            size,
            mime_type: mime,
            doc_type,
            ..Default::default()
        }
    }

    pub fn detect_type(mime: &str) -> DocumentType {
        if mime.starts_with("audio/ogg") {
            DocumentType::Voice
        } else if mime.starts_with("audio/") {
            DocumentType::Song
        } else if mime.starts_with("video/mp4") || mime.starts_with("video/webm") {
            DocumentType::Video
        } else if mime == "image/webp" || mime == "application/x-tgsticker" {
            DocumentType::Sticker
        } else if mime == "image/gif" {
            DocumentType::Animated
        } else {
            DocumentType::File
        }
    }

    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = filename.into();
        self
    }

    pub fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.dimensions = (width, height);
        self
    }

    pub fn with_sticker(mut self, sticker: StickerInfo) -> Self {
        self.doc_type = DocumentType::Sticker;
        self.sticker = Some(sticker);
        self
    }

    pub fn with_song(mut self, song: SongInfo) -> Self {
        self.doc_type = DocumentType::Song;
        self.song = Some(song);
        self
    }

    pub fn with_voice(mut self, voice: VoiceInfo) -> Self {
        self.doc_type = DocumentType::Voice;
        self.voice = Some(voice);
        self
    }

    pub fn with_video(mut self, video: VideoInfo) -> Self {
        self.doc_type = DocumentType::Video;
        self.video = Some(video);
        self
    }

    pub fn is_voice_message(&self) -> bool {
        self.doc_type == DocumentType::Voice || self.voice.is_some()
    }

    pub fn is_video_message(&self) -> bool {
        self.doc_type == DocumentType::RoundVideo
    }

    pub fn is_song(&self) -> bool {
        self.doc_type == DocumentType::Song || self.song.is_some()
    }

    pub fn is_sticker(&self) -> bool {
        self.doc_type == DocumentType::Sticker || self.sticker.is_some()
    }

    pub fn is_animation(&self) -> bool {
        self.doc_type == DocumentType::Animated
    }

    pub fn supports_streaming(&self) -> bool {
        if let Some(video) = &self.video {
            video.supports_streaming
        } else {
            self.mime_type.starts_with("audio/") || self.mime_type.starts_with("video/")
        }
    }

    pub fn has_thumbnail(&self) -> bool {
        self.has_thumbnail || self.thumbnail_bytes.is_some()
    }

    pub fn has_mime_type(&self, mime: &str) -> bool {
        self.mime_type.eq_ignore_ascii_case(mime)
    }

    pub fn progress(&self) -> f64 {
        match &self.status {
            FileLoadingStatus::Loading { progress, .. } => *progress,
            FileLoadingStatus::Ready => 1.0,
            _ => 0.0,
        }
    }

    pub fn set_loading(&mut self, progress: f64, offset: i64) {
        self.status = FileLoadingStatus::Loading {
            progress: progress.clamp(0.0, 1.0),
            offset,
        };
    }

    pub fn set_ready(&mut self) {
        self.status = FileLoadingStatus::Ready;
    }

    pub fn set_failed(&mut self, reason: impl Into<String>) {
        self.status = FileLoadingStatus::Failed {
            reason: reason.into(),
        };
    }
}
