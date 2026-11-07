use std::fs;
use std::path::{Path, PathBuf};

use url::Url;

use crate::host_paths::reanchor_sand_path;
use crate::media_mime::{image_mime_from_path, video_mime_from_path};

pub const CHANNEL_ATTACHMENT_MAX_UPLOAD_BYTES: u64 = 50 * 1024 * 1024;
pub const GENERIC_BINARY_MIME: &str = "application/octet-stream";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedChannelAttachment {
    Url { is_image: bool, url: String },
    Upload {
        is_image: bool,
        bytes: Vec<u8>,
        filename: String,
        mime: String,
    },
}

pub fn to_local_channel_attachment_path(raw: &str) -> Option<PathBuf> {
    let source_info = crate::extensions::attachments::media_reference::classify_media_reference_source(raw, false);
    if source_info.is_media_store_url {
        if let Ok(Some(uri)) = crate::extensions::attachments::media_reference::parse_inbound_media_uri(raw) {
            return Some(crate::host_paths::get_sand_root_dir().join("media").join("inbound").join(uri.id));
        }
        return None;
    }

    crate::extensions::attachments::media_file_context::resolve_local_media_path(raw)
}

pub fn is_voice_message_attachment(raw_url: &str) -> bool {
    let filename = Path::new(raw_url).file_name().and_then(|s| s.to_str());
    let mime = crate::media_mime::audio_mime_from_path(Path::new(raw_url));
    crate::extensions::attachments::media_audio_transcode::is_voice_message_compatible_audio(mime, filename)
}

pub fn url_looks_like_image(raw: &str) -> bool {
    let Ok(parsed) = Url::parse(raw) else { return false; };
    image_mime_from_path(Path::new(parsed.path())).is_some()
}

pub fn resolve_channel_attachment(raw_url: Option<&str>) -> Option<ResolvedChannelAttachment> {
    let raw_url = raw_url?;
    if raw_url.is_empty() {
        return None;
    }
    if let Ok(parsed) = Url::parse(raw_url) {
        if matches!(parsed.scheme(), "http" | "https") {
            return Some(ResolvedChannelAttachment::Url {
                is_image: image_mime_from_path(Path::new(parsed.path())).is_some(),
                url: raw_url.to_string(),
            });
        }
    }

    let local_path = to_local_channel_attachment_path(raw_url)?;
    let resolved = reanchor_sand_path(&local_path);
    let metadata = fs::metadata(&resolved).ok()?;
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > CHANNEL_ATTACHMENT_MAX_UPLOAD_BYTES
    {
        return None;
    }

    let bytes = crate::extensions::attachments::media_reference::read_outbound_media_file_bounded(
        &resolved,
        CHANNEL_ATTACHMENT_MAX_UPLOAD_BYTES as usize,
    )
    .ok()?;
    let image_mime = image_mime_from_path(&resolved);
    let mime = image_mime
        .or_else(|| video_mime_from_path(&resolved))
        .unwrap_or(GENERIC_BINARY_MIME)
        .to_string();
    let filename = resolved
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment")
        .to_string();

    Some(ResolvedChannelAttachment::Upload {
        is_image: image_mime.is_some(),
        bytes,
        filename,
        mime,
    })
}
