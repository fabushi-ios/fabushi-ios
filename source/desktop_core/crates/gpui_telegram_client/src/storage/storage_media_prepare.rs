//! Target Symbol: StorageMediaPrepare
//! Telegram Desktop 1:1 Submodule: `StorageMediaPrepare`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MimeDataState {
    None,
    Files,
    FilesArchive,
    FilesArchiveOnly,
    PhotoFiles,
    PhotoFilesArchive,
    MediaFiles,
    MediaFilesArchive,
    Image,
    Media,
    Folder,
    FolderArchiveOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumType {
    None,
    PhotoVideo,
    Music,
    File,
}

#[derive(Debug, Clone, Default)]
pub struct VideoDetails {
    pub original_width: u32,
    pub original_height: u32,
    pub duration_seconds: f64,
    pub preview_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct PreparedFile {
    pub path: PathBuf,
    pub name: String,
    pub mime_type: String,
    pub size: i64,
    pub is_image: bool,
    pub is_video: bool,
    pub video_details: Option<VideoDetails>,
}

#[derive(Debug, Clone, Default)]
pub struct PreparedList {
    pub files: Vec<PreparedFile>,
    pub album_type: Option<AlbumType>,
}

pub fn detect_mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("ogg") | Some("oga") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

pub fn compute_mime_data_state(paths: &[PathBuf]) -> MimeDataState {
    if paths.is_empty() {
        return MimeDataState::None;
    }
    let any_dir = paths.iter().any(|p| p.is_dir());
    if any_dir {
        return if paths.len() == 1 {
            MimeDataState::Folder
        } else {
            MimeDataState::FolderArchiveOnly
        };
    }

    let all_photos = paths.iter().all(|p| {
        let m = detect_mime_type(p);
        m.starts_with("image/") && m != "image/gif"
    });
    if all_photos {
        return MimeDataState::PhotoFiles;
    }

    let all_media = paths.iter().all(|p| {
        let m = detect_mime_type(p);
        m.starts_with("image/") || m.starts_with("video/") || m.starts_with("audio/")
    });
    if all_media {
        return MimeDataState::MediaFiles;
    }

    MimeDataState::Files
}

pub fn prepare_media_list(paths: &[PathBuf], max_dimension: u32) -> PreparedList {
    let mut files = Vec::new();
    let mut has_photo = false;
    let mut has_video = false;
    let mut has_other = false;

    for path in paths {
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let size = meta.len() as i64;
        let mime = detect_mime_type(path).to_string();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());

        let is_image = mime.starts_with("image/");
        let is_video = mime.starts_with("video/");

        if is_image {
            has_photo = true;
        } else if is_video {
            has_video = true;
        } else {
            has_other = true;
        }

        let video_details = if is_video {
            Some(VideoDetails {
                original_width: max_dimension,
                original_height: (max_dimension * 9) / 16,
                duration_seconds: 0.0,
                preview_bytes: Vec::new(),
            })
        } else {
            None
        };

        files.push(PreparedFile {
            path: path.clone(),
            name,
            mime_type: mime,
            size,
            is_image,
            is_video,
            video_details,
        });
    }

    let album_type = if !has_other && (has_photo || has_video) {
        Some(AlbumType::PhotoVideo)
    } else {
        Some(AlbumType::File)
    };

    PreparedList { files, album_type }
}

#[derive(Debug, Default)]
pub struct StorageMediaPrepare {
    pub prepared_count: usize,
}

impl StorageMediaPrepare {
    pub fn new() -> Self {
        Self { prepared_count: 0 }
    }

    pub fn prepare_files(&mut self, paths: &[PathBuf]) -> PreparedList {
        let list = prepare_media_list(paths, 1280);
        self.prepared_count += list.files.len();
        list
    }
}
