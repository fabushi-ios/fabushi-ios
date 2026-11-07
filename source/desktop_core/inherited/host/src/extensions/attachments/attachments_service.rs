use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::attachment_paths::{
    ATTACHMENTS_DIRNAME, ASSETS_DIRNAME, file_url_for_path, get_agent_assets_dir,
    get_agent_attachments_dir,
};
use crate::connectors::channel_attachment::{
    ResolvedChannelAttachment, resolve_channel_attachment,
};
use crate::extensions::attachments::box_staging::{
    BoxStagedUpload, BoxStagingBox, stage_attachments_into_box,
};
use crate::extensions::attachments::generate_image_resource_accessor::SandGenerateImageResourceAccessor;
use crate::extensions::attachments::generate_image_service::{
    CursorGenerateImageOptions, GenerateImageAuth, GenerateImageRequestIdObserver,
    PersistGeneratedImage, SandGenerateImageService, create_cursor_generate_image_backend,
};
use crate::extensions::attachments::link_preview_image_bounds::{
    ImageSize, read_encoded_image_size,
};
use crate::extensions::attachments::safe_link_preview_fetch::{
    SandLinkPreviewError, fetch_safe_link_preview_resource, parse_safe_link_preview_url,
};
use crate::extensions::attachments::video_playback_rendition::with_video_playback_source;
use crate::extensions::forever_box::ForeverBoxService;
use crate::host_paths::{get_sand_root_dir, reanchor_sand_path};
use crate::media_mime::{
    VIDEO_BYTE_LIMIT, attachment_byte_limit_for_name, audio_mime_from_path,
    extension_from_image_mime, servable_image_mime_from_path, video_mime_from_path,
};
use crate::storage::folder_id::is_safe_folder_id;

pub const LINK_CACHE_DIRNAME: &str = "link-cache";
pub const LINK_CACHE_VERSION: u32 = 3;
pub const LINK_CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
pub const LINK_HTML_BYTE_LIMIT: usize = 512 * 1024;
pub const LINK_IMAGE_BYTE_LIMIT: usize = 2 * 1024 * 1024;
pub const LINK_TITLE_CHAR_LIMIT: usize = 512;
pub const LINK_DESCRIPTION_CHAR_LIMIT: usize = 2_048;
pub const LINK_SITE_NAME_CHAR_LIMIT: usize = 256;
pub const LINK_IMAGE_ACCEPT: &str =
    "image/png,image/jpeg,image/webp,image/gif,image/avif,image/x-icon,image/vnd.microsoft.icon";
pub const ATTACHMENT_CHUNK_MAX_BYTES: usize = 8 * 1024 * 1024;
pub const ATTACHMENT_TEXT_PREVIEW_BYTE_CAP: usize = 64 * 1024;
pub const VIDEO_DIMENSIONS_HEAD_BYTES: usize = 1024 * 1024;
pub const VIDEO_DIMENSIONS_TAIL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandAttachmentError {
    #[error("Attachment file path is empty.")]
    EmptyPath,
    #[error("Attachment path must be absolute: {0}")]
    RelativePath(String),
    #[error("Attachment source is not a file: {0}")]
    NotFile(String),
    #[error("Attachment filename is empty.")]
    EmptyFilename,
    #[error("Attachment is empty.")]
    EmptyAttachment,
    #[error("Attachment exceeds its byte limit of {0} bytes.")]
    TooLarge(u64),
    #[error("No active agent to attach to.")]
    NoActiveAgent,
    #[error("Invalid agent id: {0}")]
    InvalidAgentId(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    InvalidBase64(String),
}

impl From<std::io::Error> for SandAttachmentError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestedAttachment {
    pub absolute_path: PathBuf,
    pub hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAttachmentImage {
    pub data_url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAttachmentChunk {
    pub bytes_base64: String,
    pub total_size: u64,
    pub mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentTextPreview {
    Text {
        text: String,
        truncated: bool,
        bytes: u64,
    },
    Binary {
        bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedImageResult {
    pub absolute_path: PathBuf,
    pub file_url: String,
    pub bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkMetadata {
    pub url: String,
    pub canonical_url: String,
    pub title: String,
    pub description: String,
    pub site_name: String,
    pub hostname: String,
    pub image_data_url: Option<String>,
    pub favicon_data_url: Option<String>,
    pub fetched_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedLinkMetadata {
    cache_version: u32,
    #[serde(flatten)]
    metadata: LinkMetadata,
}

pub type AttachmentDiagnosticReporter = Arc<dyn Fn(Value) + Send + Sync>;

pub trait AttachmentsBox: Send + Sync {
    fn run_state(&self, agent_id: &str) -> Result<String, String>;
    fn upload_file(&self, agent_id: &str, path: &str, data: &[u8]) -> Result<(), String>;
}

impl AttachmentsBox for ForeverBoxService {
    fn run_state(&self, _agent_id: &str) -> Result<String, String> {
        Ok(self.box_().inner().run_state().to_string())
    }

    fn upload_file(&self, agent_id: &str, path: &str, data: &[u8]) -> Result<(), String> {
        self.box_()
            .upload_file(agent_id, path, data)
            .map_err(|error| error.to_string())
    }
}

struct BoxStateAdapter<'a>(&'a dyn AttachmentsBox);

impl BoxStagingBox for BoxStateAdapter<'_> {
    fn run_state(&self, agent_id: &str) -> Result<String, String> {
        self.0.run_state(agent_id)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_text(value: &str) -> String {
    sha256_hex(value.as_bytes())
}

fn normalize_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => output.push(prefix.as_os_str()),
            Component::RootDir => output.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                if !output.pop() {
                    return None;
                }
            }
            Component::Normal(value) => output.push(value),
        }
    }
    Some(output)
}

fn path_is_within(parent: &Path, child: &Path, inclusive: bool) -> bool {
    let normalized_parent = normalize_absolute(parent).unwrap_or_else(|| parent.to_path_buf());
    let normalized_child = normalize_absolute(child).unwrap_or_else(|| child.to_path_buf());
    let parent_real = fs::canonicalize(&normalized_parent).unwrap_or(normalized_parent);
    let child_real = fs::canonicalize(&normalized_child).unwrap_or(normalized_child);
    child_real.strip_prefix(parent_real).ok().is_some_and(|relative| {
        inclusive || !relative.as_os_str().is_empty()
    })
}

fn write_content_addressed_file(
    dir: &Path,
    target_path: &Path,
    bytes: &[u8],
) -> Result<(), SandAttachmentError> {
    fs::create_dir_all(dir)?;
    if !target_path.exists() {
        fs::write(target_path, bytes)?;
    }
    Ok(())
}

pub fn ingest_attachment(
    agent_dir: &Path,
    source_path: &Path,
) -> Result<IngestedAttachment, SandAttachmentError> {
    if source_path.as_os_str().is_empty() {
        return Err(SandAttachmentError::EmptyPath);
    }
    if !source_path.is_absolute() {
        return Err(SandAttachmentError::RelativePath(
            source_path.display().to_string(),
        ));
    }
    let attachments_dir = get_agent_attachments_dir(agent_dir);
    if path_is_within(&attachments_dir, source_path, true) {
        let info = fs::metadata(source_path)?;
        return Ok(IngestedAttachment {
            absolute_path: source_path.to_path_buf(),
            hash: "preserved".into(),
            bytes: info.len(),
        });
    }
    let info = fs::metadata(source_path)?;
    if !info.is_file() {
        return Err(SandAttachmentError::NotFile(
            source_path.display().to_string(),
        ));
    }
    let byte_limit = attachment_byte_limit_for_name(source_path);
    if info.len() > byte_limit {
        return Err(SandAttachmentError::TooLarge(byte_limit));
    }
    let bytes = crate::extensions::attachments::media_reference::read_outbound_media_file_bounded(
        source_path,
        usize::try_from(byte_limit).unwrap_or(usize::MAX),
    )
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidData {
            SandAttachmentError::TooLarge(byte_limit)
        } else {
            SandAttachmentError::Io(error.to_string())
        }
    })?;
    let filename = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment.bin");
    ingest_attachment_bytes(agent_dir, filename, &bytes)
}

pub fn ingest_attachment_bytes(
    agent_dir: &Path,
    filename: &str,
    data: &[u8],
) -> Result<IngestedAttachment, SandAttachmentError> {
    if filename.trim().is_empty() {
        return Err(SandAttachmentError::EmptyFilename);
    }
    if data.is_empty() {
        return Err(SandAttachmentError::EmptyAttachment);
    }
    let byte_limit = attachment_byte_limit_for_name(Path::new(filename));
    let bytes = u64::try_from(data.len()).unwrap_or(u64::MAX);
    if bytes > byte_limit {
        return Err(SandAttachmentError::TooLarge(byte_limit));
    }
    let hash = sha256_hex(data);
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_else(|| ".bin".into());
    let dir = get_agent_attachments_dir(agent_dir);
    let target = dir.join(format!("{hash}{extension}"));
    write_content_addressed_file(&dir, &target, data)?;
    Ok(IngestedAttachment {
        absolute_path: target,
        hash,
        bytes,
    })
}

fn image_size(bytes: &[u8], mime: &str) -> Option<ImageSize> {
    let data_url = format!("data:{mime};base64,{}", STANDARD.encode(bytes));
    read_encoded_image_size(&data_url)
}

pub fn read_host_attachment_image_with_root(
    sand_root: &Path,
    file_path: &Path,
) -> Option<HostAttachmentImage> {
    if file_path.as_os_str().is_empty() {
        return None;
    }
    let resolved = reanchor_path_with_root(sand_root, file_path);
    if !path_is_within(sand_root, &resolved, true) {
        return None;
    }
    let mime = servable_image_mime_from_path(&resolved)?;
    let limit = attachment_byte_limit_for_name(&resolved) as usize;
    let data = crate::extensions::attachments::media_reference::read_outbound_media_file_bounded(
        &resolved,
        limit,
    )
    .ok()?;
    let size = image_size(&data, mime);
    Some(HostAttachmentImage {
        data_url: format!("data:{mime};base64,{}", STANDARD.encode(data)),
        width: size.map(|value| value.width),
        height: size.map(|value| value.height),
    })
}

pub fn read_host_attachment_image(file_path: &Path) -> Option<HostAttachmentImage> {
    read_host_attachment_image_with_root(&get_sand_root_dir(), file_path)
}

pub fn read_host_attachment_video_bytes_with_root(
    sand_root: &Path,
    file_path: &Path,
) -> Option<Vec<u8>> {
    if file_path.as_os_str().is_empty() {
        return None;
    }
    let resolved = reanchor_path_with_root(sand_root, file_path);
    if !path_is_within(sand_root, &resolved, true) || video_mime_from_path(&resolved).is_none() {
        return None;
    }
    crate::extensions::attachments::media_reference::read_outbound_media_file_bounded(
        &resolved,
        VIDEO_BYTE_LIMIT as usize,
    )
    .ok()
}

pub fn read_host_attachment_video_bytes(file_path: &Path) -> Option<Vec<u8>> {
    read_host_attachment_video_bytes_with_root(&get_sand_root_dir(), file_path)
}

fn read_chunk_file(
    path: &Path,
    offset: usize,
    length: usize,
) -> std::io::Result<Option<HostAttachmentChunk>> {
    let info = fs::metadata(path)?;
    if !info.is_file() {
        return Ok(None);
    }
    let total_size = info.len();
    let start = u64::try_from(offset).unwrap_or(u64::MAX).min(total_size);
    let requested = length.min(ATTACHMENT_CHUNK_MAX_BYTES);
    let remaining = total_size.saturating_sub(start);
    let read_len = usize::try_from(
        remaining.min(u64::try_from(requested).unwrap_or(u64::MAX)),
    )
    .unwrap_or(0);
    let mime = servable_image_mime_from_path(path)
        .or_else(|| video_mime_from_path(path))
        .or_else(|| audio_mime_from_path(path))
        .map(str::to_string);
    if read_len == 0 {
        return Ok(Some(HostAttachmentChunk {
            bytes_base64: String::new(),
            total_size,
            mime,
        }));
    }
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = vec![0_u8; read_len];
    let bytes_read = file.read(&mut buffer)?;
    buffer.truncate(bytes_read);
    Ok(Some(HostAttachmentChunk {
        bytes_base64: STANDARD.encode(buffer),
        total_size,
        mime,
    }))
}

pub fn read_host_attachment_chunk_with_root(
    sand_root: &Path,
    agent_dir: &Path,
    file_path: &Path,
    offset: usize,
    length: usize,
    video_playback: bool,
) -> Option<HostAttachmentChunk> {
    if file_path.as_os_str().is_empty() {
        return None;
    }
    let resolved = reanchor_path_with_root(sand_root, file_path);
    let attachments = get_agent_attachments_dir(agent_dir);
    let assets = get_agent_assets_dir(agent_dir);
    if !path_is_within(&attachments, &resolved, true)
        && !path_is_within(&assets, &resolved, true)
    {
        return None;
    }
    if video_playback && video_mime_from_path(&resolved).is_none() {
        return None;
    }
    if video_playback {
        return with_video_playback_source(&resolved, |path| {
            read_chunk_file(path, offset, length)
        })
        .ok()
        .flatten();
    }
    read_chunk_file(&resolved, offset, length).ok().flatten()
}

pub fn resolve_attachment_owner_dir_with_root(
    sand_root: &Path,
    file_path: &Path,
) -> Option<PathBuf> {
    if file_path.as_os_str().is_empty() {
        return None;
    }
    let resolved = reanchor_path_with_root(sand_root, file_path);
    let agents_root = sand_root.join("agents");
    if !path_is_within(&agents_root, &resolved, false) {
        return None;
    }
    let relative = resolved.strip_prefix(&agents_root).ok()?;
    let segments = relative.components().collect::<Vec<_>>();
    if segments.len() < 3 {
        return None;
    }
    let Component::Normal(agent_id) = segments[0] else {
        return None;
    };
    let Component::Normal(bucket) = segments[1] else {
        return None;
    };
    let agent_id = agent_id.to_str()?;
    let bucket = bucket.to_str()?;
    if !is_safe_folder_id(agent_id)
        || !matches!(bucket, ATTACHMENTS_DIRNAME | ASSETS_DIRNAME)
    {
        return None;
    }
    Some(agents_root.join(agent_id))
}

pub fn resolve_attachment_owner_dir(file_path: &Path) -> Option<PathBuf> {
    resolve_attachment_owner_dir_with_root(&get_sand_root_dir(), file_path)
}

fn is_text_previewable_name(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "txt"
            | "text"
            | "log"
            | "md"
            | "markdown"
            | "mdx"
            | "rst"
            | "adoc"
            | "tex"
            | "json"
            | "jsonc"
            | "json5"
            | "ndjson"
            | "csv"
            | "tsv"
            | "xml"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "conf"
            | "env"
            | "properties"
            | "plist"
            | "gradle"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "svg"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "mts"
            | "cts"
            | "py"
            | "pyi"
            | "rb"
            | "go"
            | "rs"
            | "java"
            | "kt"
            | "kts"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "cxx"
            | "hpp"
            | "hh"
            | "cs"
            | "php"
            | "swift"
            | "scala"
            | "dart"
            | "lua"
            | "pl"
            | "pm"
            | "r"
            | "sql"
            | "graphql"
            | "gql"
            | "proto"
            | "vue"
            | "svelte"
            | "astro"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "bat"
            | "ps1"
            | "tf"
            | "tfvars"
            | "dockerfile"
            | "diff"
            | "patch"
    )
}

fn looks_like_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8 * 1024)];
    if sample.is_empty() {
        return false;
    }
    let mut controls = 0usize;
    for byte in sample {
        if *byte == 0 {
            return true;
        }
        if *byte < 32 && !(9..=13).contains(byte) {
            controls += 1;
        }
    }
    controls as f64 / sample.len() as f64 > 0.3
}

pub fn read_attachment_text_with_root(
    sand_root: &Path,
    agent_dir: &Path,
    file_path: &Path,
) -> Option<AttachmentTextPreview> {
    let resolved = reanchor_path_with_root(sand_root, file_path);
    if file_path.as_os_str().is_empty()
        || !path_is_within(&get_agent_attachments_dir(agent_dir), &resolved, true)
    {
        return None;
    }
    let info = fs::metadata(&resolved).ok()?;
    if !info.is_file() {
        return None;
    }
    if !is_text_previewable_name(&resolved) {
        return Some(AttachmentTextPreview::Binary { bytes: info.len() });
    }
    let mut file = fs::File::open(resolved).ok()?;
    let mut head = vec![0_u8; ATTACHMENT_TEXT_PREVIEW_BYTE_CAP];
    let read = file.read(&mut head).ok()?;
    head.truncate(read);
    if looks_like_binary(&head) {
        return Some(AttachmentTextPreview::Binary { bytes: info.len() });
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    if super::media_understanding_service::is_transcript_artifact_text(&text) {
        return None;
    }
    Some(AttachmentTextPreview::Text {
        text,
        truncated: info.len()
            > u64::try_from(ATTACHMENT_TEXT_PREVIEW_BYTE_CAP).unwrap_or(u64::MAX),
        bytes: info.len(),
    })
}

pub fn read_image_dimensions_with_root(
    sand_root: &Path,
    file_path: &Path,
) -> Option<ImageSize> {
    let resolved = reanchor_path_with_root(sand_root, file_path);
    if !path_is_within(sand_root, &resolved, true) {
        return None;
    }
    let mime = servable_image_mime_from_path(&resolved)?;
    let limit = attachment_byte_limit_for_name(&resolved) as usize;
    let data = crate::extensions::attachments::media_reference::read_outbound_media_file_bounded(
        &resolved,
        limit,
    )
    .ok()?;
    image_size(&data, mime)
}

#[derive(Debug, Clone, Copy)]
struct ByteRange {
    start: usize,
    end: usize,
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    let value = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([value[0], value[1], value[2], value[3]]))
}

fn four_char_tag(bytes: &[u8], offset: usize) -> Option<[u8; 4]> {
    let value = bytes.get(offset..offset.checked_add(4)?)?;
    Some([value[0], value[1], value[2], value[3]])
}

fn clamped_box_at(bytes: &[u8], at: usize, end: usize) -> Option<(ByteRange, usize)> {
    if at.checked_add(8)? > end {
        return None;
    }
    let mut size = usize::try_from(read_u32_be(bytes, at)?).ok()?;
    let mut header = 8usize;
    if size == 1 {
        if at.checked_add(16)? > end {
            return None;
        }
        size = usize::try_from(read_u32_be(bytes, at + 12)?).ok()?;
        header = 16;
    } else if size == 0 {
        size = end.checked_sub(at)?;
    }
    if size < header {
        return None;
    }
    let next = at.checked_add(size)?;
    Some((
        ByteRange {
            start: at.checked_add(header)?,
            end: next.min(end),
        },
        next,
    ))
}

fn child_boxes(bytes: &[u8], range: ByteRange, kind: [u8; 4]) -> Vec<ByteRange> {
    let mut output = Vec::new();
    let mut offset = range.start;
    while offset.saturating_add(8) <= range.end {
        let Some((body, next)) = clamped_box_at(bytes, offset, range.end) else {
            break;
        };
        if four_char_tag(bytes, offset + 4) == Some(kind) {
            output.push(body);
        }
        if next > range.end || next <= offset {
            break;
        }
        offset = next;
    }
    output
}

fn read_mp4_tkhd(bytes: &[u8], range: ByteRange) -> Option<ImageSize> {
    if range.start >= range.end {
        return None;
    }
    let version = *bytes.get(range.start)?;
    let widened = if version == 1 { 12 } else { 0 };
    let matrix_at = range.start.checked_add(40 + widened)?;
    let width_at = matrix_at.checked_add(36)?;
    if width_at.checked_add(8)? > range.end {
        return None;
    }
    let width = read_u32_be(bytes, width_at)? >> 16;
    let height = read_u32_be(bytes, width_at + 4)? >> 16;
    if width == 0 || height == 0 {
        return None;
    }
    let a = read_u32_be(bytes, matrix_at)?;
    let b = read_u32_be(bytes, matrix_at + 4)?;
    let c = read_u32_be(bytes, matrix_at + 12)?;
    let d = read_u32_be(bytes, matrix_at + 16)?;
    if a == 0 && d == 0 && b != 0 && c != 0 {
        Some(ImageSize {
            width: height,
            height: width,
        })
    } else {
        Some(ImageSize { width, height })
    }
}

fn read_mp4_dimensions(window: &[u8]) -> Option<ImageSize> {
    let mut search_from = 4usize;
    while search_from.saturating_add(4) <= window.len() {
        let position = window[search_from..]
            .windows(4)
            .position(|value| value == b"moov")?
            + search_from;
        let at = position.checked_sub(4)?;
        if let Some((moov, _)) = clamped_box_at(window, at, window.len()) {
            for track in child_boxes(window, moov, *b"trak") {
                for tkhd in child_boxes(window, track, *b"tkhd") {
                    if let Some(dimensions) = read_mp4_tkhd(window, tkhd) {
                        return Some(dimensions);
                    }
                }
            }
        }
        search_from = position.saturating_add(4);
    }
    None
}

pub fn read_video_dimensions_with_root(
    sand_root: &Path,
    file_path: &Path,
) -> Option<ImageSize> {
    let resolved = reanchor_path_with_root(sand_root, file_path);
    if !path_is_within(sand_root, &resolved, true)
        || video_mime_from_path(&resolved).is_none()
    {
        return None;
    }
    let mut file = fs::File::open(resolved).ok()?;
    let size = file.metadata().ok()?.len();
    let head_len = usize::try_from(
        size.min(u64::try_from(VIDEO_DIMENSIONS_HEAD_BYTES).unwrap_or(u64::MAX)),
    )
    .ok()?;
    let mut head = vec![0_u8; head_len];
    file.read_exact(&mut head).ok()?;
    if let Some(dimensions) = read_mp4_dimensions(&head) {
        return Some(dimensions);
    }
    if size <= u64::try_from(head_len).ok()? {
        return None;
    }
    let tail_len = usize::try_from(
        size.min(u64::try_from(VIDEO_DIMENSIONS_TAIL_BYTES).unwrap_or(u64::MAX)),
    )
    .ok()?;
    file.seek(SeekFrom::Start(size.saturating_sub(
        u64::try_from(tail_len).unwrap_or_default(),
    )))
    .ok()?;
    let mut tail = vec![0_u8; tail_len];
    file.read_exact(&mut tail).ok()?;
    read_mp4_dimensions(&tail)
}

pub fn read_media_dimensions_with_root(
    sand_root: &Path,
    file_path: &Path,
) -> Option<ImageSize> {
    let resolved = reanchor_path_with_root(sand_root, file_path);
    if !path_is_within(sand_root, &resolved, true) {
        return None;
    }
    if servable_image_mime_from_path(&resolved).is_some() {
        return read_image_dimensions_with_root(sand_root, &resolved);
    }
    if video_mime_from_path(&resolved).is_some() {
        return read_video_dimensions_with_root(sand_root, &resolved);
    }
    None
}

pub fn persist_image_bytes(
    target_dir: &Path,
    data: &[u8],
    mime_type: &str,
) -> Result<PersistedImageResult, SandAttachmentError> {
    let hash = sha256_hex(data);
    let extension = extension_from_image_mime(mime_type).unwrap_or(".png");
    let target = target_dir.join(format!("{hash}{extension}"));
    write_content_addressed_file(target_dir, &target, data)?;
    let size = image_size(data, mime_type);
    Ok(PersistedImageResult {
        absolute_path: target.clone(),
        file_url: file_url_for_path(&target).unwrap_or_default(),
        bytes: u64::try_from(data.len()).unwrap_or(u64::MAX),
        width: size.map(|value| value.width),
        height: size.map(|value| value.height),
    })
}

fn get_link_cache_path(agent_dir: &Path, url: &str) -> PathBuf {
    agent_dir
        .join(LINK_CACHE_DIRNAME)
        .join(format!("{}.json", sha256_text(url)))
}

fn read_cached_link_metadata(agent_dir: &Path, url: &str) -> Option<LinkMetadata> {
    let raw = fs::read_to_string(get_link_cache_path(agent_dir, url)).ok()?;
    let cached: CachedLinkMetadata = serde_json::from_str(&raw).ok()?;
    if cached.cache_version != LINK_CACHE_VERSION
        || now_ms().saturating_sub(cached.metadata.fetched_at) > LINK_CACHE_TTL_MS
    {
        return None;
    }
    Some(cached.metadata)
}

fn write_cached_link_metadata(
    agent_dir: &Path,
    url: &str,
    metadata: &LinkMetadata,
) -> std::io::Result<()> {
    let path = get_link_cache_path(agent_dir, url);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&CachedLinkMetadata {
            cache_version: LINK_CACHE_VERSION,
            metadata: metadata.clone(),
        })
        .map_err(std::io::Error::other)?,
    )
}

fn content_type(headers: &BTreeMap<String, String>) -> String {
    headers
        .get("content-type")
        .cloned()
        .unwrap_or_default()
}

fn fetch_text(
    url: &str,
    byte_limit: usize,
) -> Result<(String, Url, Vec<Url>, String), SandLinkPreviewError> {
    let response = fetch_safe_link_preview_resource(
        url,
        byte_limit,
        "text/html,application/xhtml+xml,application/xml;q=0.9",
        true,
    )?;
    if !(200..300).contains(&response.status_code) {
        return Err(SandLinkPreviewError::Request(format!(
            "HTTP {} while fetching link preview.",
            response.status_code
        )));
    }
    let final_url = parse_safe_link_preview_url(&response.final_url)?;
    let redirects = response
        .redirect_urls
        .iter()
        .map(|value| parse_safe_link_preview_url(value))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        String::from_utf8_lossy(&response.body).into_owned(),
        final_url,
        redirects,
        content_type(&response.headers),
    ))
}

fn image_mime_allowed(mime: &str) -> bool {
    matches!(
        mime,
        "image/png"
            | "image/jpeg"
            | "image/webp"
            | "image/gif"
            | "image/avif"
            | "image/x-icon"
            | "image/vnd.microsoft.icon"
    )
}

fn fetch_image_as_data_url(url: &str) -> Option<String> {
    let response = fetch_safe_link_preview_resource(
        url,
        LINK_IMAGE_BYTE_LIMIT,
        LINK_IMAGE_ACCEPT,
        false,
    )
    .ok()?;
    if !(200..300).contains(&response.status_code) {
        return None;
    }
    let mime = content_type(&response.headers)
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !image_mime_allowed(&mime)
        || response.body.is_empty()
        || response.body.len() > LINK_IMAGE_BYTE_LIMIT
    {
        return None;
    }
    Some(format!(
        "data:{mime};base64,{}",
        STANDARD.encode(response.body)
    ))
}

fn decode_numeric_entities(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while cursor < input.len() {
        let remaining = &input[cursor..];
        let Some(offset) = remaining.find("&#") else {
            output.push_str(remaining);
            break;
        };
        output.push_str(&remaining[..offset]);
        let start = cursor + offset;
        let after = &input[start + 2..];
        let Some(semi) = after.find(';') else {
            output.push_str(&input[start..]);
            break;
        };
        let token = &after[..semi];
        let parsed = if let Some(hex) = token.strip_prefix('x').or_else(|| token.strip_prefix('X')) {
            u32::from_str_radix(hex, 16).ok()
        } else {
            token.parse::<u32>().ok()
        };
        if let Some(ch) = parsed.and_then(char::from_u32) {
            output.push(ch);
        } else {
            output.push_str(&input[start..start + 2 + semi + 1]);
        }
        cursor = start + 2 + semi + 1;
    }
    output
}

fn decode_html_entities(text: &str) -> String {
    decode_numeric_entities(
        &text
            .replace("&amp;", "&")
            .replace("&#38;", "&")
            .replace("&lt;", "<")
            .replace("&#60;", "<")
            .replace("&gt;", ">")
            .replace("&#62;", ">")
            .replace("&quot;", "\"")
            .replace("&#34;", "\"")
            .replace("&apos;", "'")
            .replace("&#39;", "'")
            .replace("&nbsp;", " "),
    )
}

fn clean_link_metadata_text(text: &str, char_limit: usize) -> String {
    let replaced = text
        .chars()
        .map(|ch| {
            if ch <= '\u{001f}' || ('\u{007f}'..='\u{009f}').contains(&ch) {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>();
    replaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(char_limit)
        .collect()
}

fn tag_slices<'a>(html: &'a str, tag_name: &str) -> Vec<&'a str> {
    let lower = html.to_ascii_lowercase();
    let needle = format!("<{tag_name}");
    let mut output = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = lower[cursor..].find(&needle) {
        let start = cursor + offset;
        let Some(end_offset) = lower[start..].find('>') else {
            break;
        };
        let end = start + end_offset + 1;
        output.push(&html[start..end]);
        cursor = end;
    }
    output
}

fn match_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let attr = attr.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut cursor = 0usize;
    while let Some(offset) = lower[cursor..].find(&attr) {
        let start = cursor + offset;
        let before_ok = start == 0
            || bytes
                .get(start.saturating_sub(1))
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'<');
        let after_index = start + attr.len();
        let after_ok = bytes
            .get(after_index)
            .is_none_or(|byte| byte.is_ascii_whitespace() || *byte == b'=');
        if !before_ok || !after_ok {
            cursor = after_index;
            continue;
        }
        let mut at = after_index;
        while bytes.get(at).is_some_and(|byte| byte.is_ascii_whitespace()) {
            at += 1;
        }
        if bytes.get(at) != Some(&b'=') {
            cursor = after_index;
            continue;
        }
        at += 1;
        while bytes.get(at).is_some_and(|byte| byte.is_ascii_whitespace()) {
            at += 1;
        }
        let original = tag.as_bytes();
        let quote = original.get(at).copied();
        if matches!(quote, Some(b'"') | Some(b'\'')) {
            let quote = quote?;
            at += 1;
            let tail = &tag[at..];
            let end = tail.as_bytes().iter().position(|byte| *byte == quote)?;
            return Some(tail[..end].to_string());
        }
        let end = tag[at..]
            .bytes()
            .position(|byte| byte.is_ascii_whitespace() || byte == b'>')
            .unwrap_or(tag.len().saturating_sub(at));
        return Some(tag[at..at + end].to_string());
    }
    None
}

fn find_meta(html: &str, attr: &str, value: &str) -> Option<String> {
    for tag in tag_slices(html, "meta") {
        let Some(candidate) = match_attr(tag, attr) else {
            continue;
        };
        if !candidate.eq_ignore_ascii_case(value) {
            continue;
        }
        if let Some(content) = match_attr(tag, "content") {
            return Some(decode_html_entities(&content).trim().to_string());
        }
    }
    None
}

fn find_first_link_href(html: &str, rel_values: &[&str]) -> Option<String> {
    for tag in tag_slices(html, "link") {
        let Some(rel) = match_attr(tag, "rel") else {
            continue;
        };
        let tokens = rel
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        if !rel_values
            .iter()
            .any(|value| tokens.iter().any(|token| token == value))
        {
            continue;
        }
        if let Some(href) = match_attr(tag, "href") {
            return Some(decode_html_entities(&href).trim().to_string());
        }
    }
    None
}

fn find_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title>")? + "<title>".len();
    let end = lower[start..].find("</title>")? + start;
    Some(decode_html_entities(&html[start..end]).trim().to_string())
}

fn to_absolute_url(href: &str, base: &str) -> Option<String> {
    let base = Url::parse(base).ok()?;
    let joined = base.join(href).ok()?;
    parse_safe_link_preview_url(joined.as_str())
        .ok()
        .map(|url| url.to_string())
}

struct ScrapedOpenGraph {
    title: String,
    description: String,
    site_name: String,
    canonical_url: String,
    image_url: Option<String>,
    favicon_url: Option<String>,
}

fn scrape_open_graph(html: &str, final_url: &str) -> ScrapedOpenGraph {
    let title = find_meta(html, "property", "og:title")
        .or_else(|| find_meta(html, "name", "twitter:title"))
        .or_else(|| find_title(html))
        .unwrap_or_default();
    let description = find_meta(html, "property", "og:description")
        .or_else(|| find_meta(html, "name", "description"))
        .or_else(|| find_meta(html, "name", "twitter:description"))
        .unwrap_or_default();
    let site_name = find_meta(html, "property", "og:site_name")
        .or_else(|| find_meta(html, "name", "application-name"))
        .unwrap_or_default();
    let canonical = find_first_link_href(html, &["canonical"])
        .or_else(|| find_meta(html, "property", "og:url"))
        .unwrap_or_else(|| final_url.to_string());
    let image = find_meta(html, "property", "og:image")
        .or_else(|| find_meta(html, "property", "og:image:url"))
        .or_else(|| find_meta(html, "name", "twitter:image"))
        .or_else(|| find_meta(html, "name", "twitter:image:src"));
    let favicon = find_first_link_href(
        html,
        &[
            "icon",
            "shortcut",
            "apple-touch-icon",
            "apple-touch-icon-precomposed",
            "mask-icon",
        ],
    );
    ScrapedOpenGraph {
        title: clean_link_metadata_text(&title, LINK_TITLE_CHAR_LIMIT),
        description: clean_link_metadata_text(&description, LINK_DESCRIPTION_CHAR_LIMIT),
        site_name: clean_link_metadata_text(&site_name, LINK_SITE_NAME_CHAR_LIMIT),
        canonical_url: to_absolute_url(&canonical, final_url)
            .unwrap_or_else(|| final_url.to_string()),
        image_url: image.and_then(|value| to_absolute_url(&value, final_url)),
        favicon_url: favicon.and_then(|value| to_absolute_url(&value, final_url)),
    }
}

fn google_favicon_url(hostname: &str) -> String {
    let mut url = Url::parse("https://www.google.com/s2/favicons").expect("static URL");
    url.query_pairs_mut()
        .append_pair("domain", hostname)
        .append_pair("sz", "64");
    url.to_string()
}

fn percent_decode_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).ok();
            if let Some(decoded) = hex.and_then(|value| u8::from_str_radix(value, 16).ok()) {
                output.push(decoded);
                at += 3;
                continue;
            }
        }
        output.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn is_authentication_destination(url: &Url) -> bool {
    const HOSTNAMES: &[&str] = &["accounts.google.com"];
    const LABELS: &[&str] = &[
        "auth",
        "authenticate",
        "authentication",
        "authenticator",
        "idp",
        "identity",
        "login",
        "oauth",
        "signin",
        "sso",
    ];
    const SEGMENTS: &[&str] = &[
        "auth",
        "authenticate",
        "authorize",
        "login",
        "oauth",
        "saml",
        "sign-in",
        "sign_in",
        "signin",
        "sso",
    ];
    let hostname = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if HOSTNAMES.contains(&hostname.as_str())
        || hostname
            .split('.')
            .any(|label| LABELS.contains(&label))
    {
        return true;
    }
    let segments = url
        .path()
        .split('/')
        .filter(|value| !value.is_empty())
        .map(|value| percent_decode_component(value).to_ascii_lowercase())
        .collect::<Vec<_>>();
    if segments
        .last()
        .is_some_and(|value| SEGMENTS.contains(&value.as_str()))
    {
        return true;
    }
    segments.windows(2).any(|pair| {
        matches!(
            format!("{}/{}", pair[0], pair[1]).as_str(),
            "sign-in/identifier" | "sign_in/identifier" | "signin/identifier"
        )
    })
}

pub fn fetch_link_metadata(agent_dir: &Path, raw_url: &str) -> Option<LinkMetadata> {
    let url = parse_safe_link_preview_url(raw_url.trim()).ok()?;
    if let Some(cached) = read_cached_link_metadata(agent_dir, url.as_str()) {
        return Some(cached);
    }
    let (html, final_url, redirects, content_type) =
        fetch_text(url.as_str(), LINK_HTML_BYTE_LIMIT).ok()?;
    if redirects.iter().any(is_authentication_destination) {
        return None;
    }
    let lower_type = content_type.to_ascii_lowercase();
    if !content_type.is_empty()
        && !lower_type.contains("html")
        && !lower_type.contains("xml")
    {
        return None;
    }
    let scraped = scrape_open_graph(&html, final_url.as_str());
    let hostname = final_url.host_str().unwrap_or_default().to_string();
    let image_data_url = scraped
        .image_url
        .as_deref()
        .and_then(fetch_image_as_data_url);
    let favicon_data_url = scraped
        .favicon_url
        .as_deref()
        .and_then(fetch_image_as_data_url)
        .or_else(|| fetch_image_as_data_url(&google_favicon_url(&hostname)));
    let metadata = LinkMetadata {
        url: url.to_string(),
        canonical_url: scraped.canonical_url,
        title: scraped.title,
        description: scraped.description,
        site_name: scraped.site_name,
        hostname,
        image_data_url,
        favicon_data_url,
        fetched_at: now_ms(),
    };
    let _ = write_cached_link_metadata(agent_dir, url.as_str(), &metadata);
    Some(metadata)
}

fn reanchor_path_with_root(sand_root: &Path, stored_path: &Path) -> PathBuf {
    let current_root = get_sand_root_dir();
    let reanchored = reanchor_sand_path(stored_path);
    if current_root == sand_root {
        return reanchored;
    }
    if stored_path.starts_with(&current_root) {
        return sand_root.join(stored_path.strip_prefix(current_root).unwrap_or(stored_path));
    }
    if reanchored.starts_with(&current_root) {
        return sand_root.join(reanchored.strip_prefix(current_root).unwrap_or(&reanchored));
    }
    stored_path.to_path_buf()
}

pub struct AttachmentsService {
    sand_root: PathBuf,
    auth: Arc<dyn GenerateImageAuth>,
    box_: Arc<dyn AttachmentsBox>,
    report: Option<AttachmentDiagnosticReporter>,
    fallback_agent_id: Mutex<Option<String>>,
}

impl AttachmentsService {
    pub fn new(
        sand_root: impl Into<PathBuf>,
        auth: Arc<dyn GenerateImageAuth>,
        box_: Arc<dyn AttachmentsBox>,
        report: Option<AttachmentDiagnosticReporter>,
    ) -> Self {
        Self {
            sand_root: sand_root.into(),
            auth,
            box_,
            report,
            fallback_agent_id: Mutex::new(None),
        }
    }

    pub fn production(
        auth: Arc<dyn GenerateImageAuth>,
        box_: Arc<dyn AttachmentsBox>,
        report: Option<AttachmentDiagnosticReporter>,
    ) -> Self {
        Self::new(get_sand_root_dir(), auth, box_, report)
    }

    pub fn sand_root(&self) -> &Path {
        &self.sand_root
    }

    pub fn set_fallback_agent_id(&self, agent_id: Option<String>) {
        *self
            .fallback_agent_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = agent_id;
    }

    fn resolve_dir(&self, agent_id: Option<&str>) -> Result<PathBuf, SandAttachmentError> {
        let fallback = self
            .fallback_agent_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let id = agent_id
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or(fallback)
            .ok_or(SandAttachmentError::NoActiveAgent)?;
        if !is_safe_folder_id(&id) || id.trim() != id {
            return Err(SandAttachmentError::InvalidAgentId(id));
        }
        Ok(self.sand_root.join("agents").join(id))
    }

    fn read_dir(&self, path: &Path, agent_id: Option<&str>) -> Option<PathBuf> {
        resolve_attachment_owner_dir_with_root(&self.sand_root, path)
            .or_else(|| self.resolve_dir(agent_id).ok())
    }

    fn report_miss(&self, kind: &str, has_active: bool) {
        if let Some(report) = self.report.as_ref() {
            report(json!({
                "extension": "attachments",
                "kind": kind,
                "hasActive": has_active,
            }));
        }
    }

    pub fn upload(
        &self,
        filename: &str,
        bytes_base64: Option<&str>,
        agent_id: Option<&str>,
    ) -> Result<PathBuf, SandAttachmentError> {
        let b64 = bytes_base64.unwrap_or_default();
        if !b64.is_empty() {
            crate::media_mime::inspect_canonical_base64(b64)
                .ok_or_else(|| SandAttachmentError::InvalidBase64("non-canonical base64 payload".into()))?;
        }
        let bytes = STANDARD
            .decode(b64)
            .map_err(|error| SandAttachmentError::InvalidBase64(error.to_string()))?;
        Ok(ingest_attachment_bytes(
            &self.resolve_dir(agent_id)?,
            filename,
            &bytes,
        )?
        .absolute_path)
    }

    pub fn read_image(&self, path: &Path) -> Option<HostAttachmentImage> {
        read_host_attachment_image_with_root(&self.sand_root, path)
    }

    pub fn read_text(
        &self,
        path: &Path,
        agent_id: Option<&str>,
    ) -> Option<AttachmentTextPreview> {
        let Some(dir) = self.read_dir(path, agent_id) else {
            self.report_miss("read_text_miss", agent_id.is_some());
            return None;
        };
        read_attachment_text_with_root(&self.sand_root, &dir, path)
    }

    /// Reads text and, when the caller supplies extraction facts, prefixes the
    /// same bounded truncation notice used by the document-context owner.
    /// Keeping this opt-in preserves the legacy raw preview contract while
    /// giving gateway callers a real producer-to-owner path for partial files.
    pub fn read_text_with_document_metadata(
        &self,
        path: &Path,
        agent_id: Option<&str>,
        metadata: Option<&crate::extensions::attachments::media_file_context::DocumentExtractionMetadata>,
        explicit_selection_limit: Option<usize>,
    ) -> Option<AttachmentTextPreview> {
        let preview = self.read_text(path, agent_id)?;
        let AttachmentTextPreview::Text {
            text,
            truncated,
            bytes,
        } = preview
        else {
            return Some(preview);
        };
        let notice = crate::extensions::attachments::media_file_context::render_document_truncation_notice(
            metadata,
            explicit_selection_limit,
        );
        let text = match notice {
            Some(notice) => format!("{notice}\n{text}"),
            None => text,
        };
        Some(AttachmentTextPreview::Text {
            text,
            truncated,
            bytes,
        })
    }

    pub fn read_chunk(
        &self,
        path: &Path,
        agent_id: Option<&str>,
        offset: usize,
        length: usize,
        video_playback: bool,
    ) -> Option<HostAttachmentChunk> {
        let Some(dir) = self.read_dir(path, agent_id) else {
            self.report_miss("read_chunk_miss", agent_id.is_some());
            return None;
        };
        read_host_attachment_chunk_with_root(
            &self.sand_root,
            &dir,
            path,
            offset,
            length,
            video_playback,
        )
    }

    pub fn ingest(
        &self,
        agent_dir: &Path,
        source_path: &Path,
    ) -> Result<IngestedAttachment, SandAttachmentError> {
        ingest_attachment(agent_dir, source_path)
    }

    pub fn ingest_bytes(
        &self,
        agent_dir: &Path,
        filename: &str,
        data: &[u8],
    ) -> Result<IngestedAttachment, SandAttachmentError> {
        ingest_attachment_bytes(agent_dir, filename, data)
    }

    pub fn read_video_bytes(&self, path: &Path) -> Option<Vec<u8>> {
        read_host_attachment_video_bytes_with_root(&self.sand_root, path)
    }

    pub fn resolve_channel_attachment(
        &self,
        raw_url: Option<&str>,
    ) -> Option<ResolvedChannelAttachment> {
        resolve_channel_attachment(raw_url)
    }

    pub fn resolve_owner_dir(&self, path: &Path) -> Option<PathBuf> {
        resolve_attachment_owner_dir_with_root(&self.sand_root, path)
    }

    pub fn create_generate_image_resource_accessor(
        &self,
        agent_dir: &Path,
    ) -> SandGenerateImageResourceAccessor {
        SandGenerateImageResourceAccessor::new(agent_dir)
    }

    pub fn create_generate_image_service(
        &self,
        persist: PersistGeneratedImage,
        on_request_id: Option<GenerateImageRequestIdObserver>,
    ) -> Result<SandGenerateImageService, String> {
        let mut options = CursorGenerateImageOptions::production(Arc::clone(&self.auth))?;
        options.on_request_id = on_request_id;
        Ok(SandGenerateImageService::new(
            create_cursor_generate_image_backend(options),
            persist,
        ))
    }

    pub fn stage_into_box(
        &self,
        agent_id: &str,
        paths: &[PathBuf],
    ) -> BTreeMap<PathBuf, String> {
        let adapter = BoxStateAdapter(self.box_.as_ref());
        stage_attachments_into_box(
            &adapter,
            &|path| self.resolve_owner_dir(path),
            &|id, files: &[BoxStagedUpload]| {
                for file in files {
                    self.box_.upload_file(id, &file.box_path, &file.data)?;
                }
                Ok(())
            },
            agent_id,
            paths,
        )
    }

    pub fn fetch_link_metadata(&self, agent_id: &str, raw_url: &str) -> Option<LinkMetadata> {
        self.resolve_dir(Some(agent_id))
            .ok()
            .and_then(|dir| fetch_link_metadata(&dir, raw_url))
    }

    pub fn read_image_dimensions(&self, path: &Path) -> Option<ImageSize> {
        read_image_dimensions_with_root(&self.sand_root, path)
    }

    pub fn read_media_dimensions(&self, path: &Path) -> Option<ImageSize> {
        read_media_dimensions_with_root(&self.sand_root, path)
    }

    pub fn resolve_media_reference(
        &self,
        source: &str,
    ) -> Result<PathBuf, crate::extensions::attachments::media_reference::MediaReferenceError> {
        let media_dir = self.sand_root.join("media");
        let info = crate::extensions::attachments::media_reference::resolve_media_reference_local_path_info(
            source,
            &media_dir,
        )?;
        match info {
            crate::extensions::attachments::media_reference::MediaReferenceLocalPathInfo::Inbound { path, .. } => {
                Ok(path)
            }
            crate::extensions::attachments::media_reference::MediaReferenceLocalPathInfo::Local { path } => {
                Ok(path)
            }
        }
    }

    pub fn parse_inbound_media_uri(
        &self,
        source: &str,
    ) -> Result<Option<crate::extensions::attachments::media_reference::InboundMediaUri>, crate::extensions::attachments::media_reference::MediaReferenceError> {
        crate::extensions::attachments::media_reference::parse_inbound_media_uri(source)
    }

    pub fn canonicalize_user_message_media(&self, message: &mut serde_json::Value) -> Result<bool, String> {
        crate::extensions::attachments::media_facts::canonicalize_persisted_user_message_media(message)
    }

    pub fn persist_image_bytes(
        &self,
        target_dir: &Path,
        data: &[u8],
        mime_type: &str,
    ) -> Result<PersistedImageResult, SandAttachmentError> {
        persist_image_bytes(target_dir, data, mime_type)
    }

    pub fn dispatch_gateway(
        &self,
        method: &str,
        args: &Value,
    ) -> Option<Result<Value, String>> {
        let result = match method {
            "uploadAttachment" => {
                let filename = args
                    .get("filename")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "uploadAttachment.filename must be a string".to_string());
                filename.and_then(|filename| {
                    self.upload(
                        filename,
                        args.get("bytesBase64").and_then(Value::as_str),
                        args.get("agentId").and_then(Value::as_str),
                    )
                    .map(|path| json!({ "path": path.to_string_lossy() }))
                    .map_err(|error| error.to_string())
                })
            }
            "readAttachmentImage" => args
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| "readAttachmentImage.path must be a string".to_string())
                .map(|path| {
                    self.read_image(Path::new(path))
                        .map(|image| {
                            json!({
                                "dataUrl": image.data_url,
                                "width": image.width,
                                "height": image.height,
                            })
                        })
                        .unwrap_or(Value::Null)
                }),
            "readAttachmentText" => args
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| "readAttachmentText.path must be a string".to_string())
                .map(|path| {
                    let metadata = args
                        .get("metadata")
                        .and_then(|value| serde_json::from_value(value.clone()).ok());
                    let explicit_selection_limit = args
                        .get("explicitSelectionLimit")
                        .and_then(Value::as_u64)
                        .and_then(|value| usize::try_from(value).ok());
                    match self.read_text_with_document_metadata(
                        Path::new(path),
                        args.get("agentId").and_then(Value::as_str),
                        metadata.as_ref(),
                        explicit_selection_limit,
                    ) {
                        Some(AttachmentTextPreview::Text {
                            text,
                            truncated,
                            bytes,
                        }) => json!({
                            "kind": "text",
                            "text": text,
                            "truncated": truncated,
                            "bytes": bytes,
                        }),
                        Some(AttachmentTextPreview::Binary { bytes }) => {
                            json!({ "kind": "binary", "bytes": bytes })
                        }
                        None => Value::Null,
                    }
                }),
            "fetchLinkMetadata" => {
                let cache_dir = args
                    .get("cacheDir")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "fetchLinkMetadata.cacheDir must be a string".to_string());
                let url = args
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "fetchLinkMetadata.url must be a string".to_string());
                cache_dir.and_then(|cache_dir| {
                    url.map(|url| {
                        fetch_link_metadata(Path::new(cache_dir), url)
                            .and_then(|metadata| serde_json::to_value(metadata).ok())
                            .unwrap_or(Value::Null)
                    })
                })
            }
            "readAttachmentChunk" => args
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| "readAttachmentChunk.path must be a string".to_string())
                .map(|path| {
                    let number = |name: &str| {
                        args.get(name)
                            .and_then(Value::as_f64)
                            .filter(|value| value.is_finite())
                            .map(|value| value.max(0.0).floor() as usize)
                            .unwrap_or(0)
                    };
                    self.read_chunk(
                        Path::new(path),
                        args.get("agentId").and_then(Value::as_str),
                        number("offset"),
                        number("length"),
                        args.get("videoPlayback")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    )
                    .map(|chunk| {
                        json!({
                            "bytesBase64": chunk.bytes_base64,
                            "totalSize": chunk.total_size,
                            "mime": chunk.mime,
                        })
                    })
                    .unwrap_or(Value::Null)
                }),
            "renderFileContext" => {
                let content = args
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let filename = args.get("filename").and_then(Value::as_str);
                let fallback_name = args.get("fallbackName").and_then(Value::as_str);
                let mime_type = args.get("mimeType").and_then(Value::as_str);
                let surround = args
                    .get("surroundContentWithNewlines")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let rendered = crate::extensions::attachments::media_file_context::render_file_context_block(
                    crate::extensions::attachments::media_file_context::RenderFileContextParams {
                        filename,
                        fallback_name,
                        mime_type,
                        content,
                        surround_content_with_newlines: surround,
                    },
                );
                Ok(Value::String(rendered))
            }
            "resolveLocalMediaPath" => {
                let source = args
                    .get("source")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "resolveLocalMediaPath.source must be a string".to_string());
                source.map(|src| {
                    crate::extensions::attachments::media_file_context::resolve_local_media_path(src)
                        .map(|p| Value::String(p.to_string_lossy().into_owned()))
                        .unwrap_or(Value::Null)
                })
            }
            "renderDocumentTruncation" => {
                let explicit_limit = args
                    .get("explicitSelectionLimit")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize);
                let meta = args
                    .get("metadata")
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                let rendered = crate::extensions::attachments::media_file_context::render_document_truncation_notice(
                    meta.as_ref(),
                    explicit_limit,
                );
                Ok(rendered.map(Value::String).unwrap_or(Value::Null))
            }
            "renderQrTerminal" => (|| {
                let input = args
                    .get("input")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "renderQrTerminal.input must be a string".to_string())?;
                let small = args
                    .get("small")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                crate::extensions::attachments::media_qr_png::render_qr_terminal(input, small)
                    .map(Value::String)
            })(),
            "renderQrPngDataUrl" => (|| {
                let input = args
                    .get("input")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "renderQrPngDataUrl.input must be a string".to_string())?;
                let scale = args.get("scale").and_then(Value::as_u64).map(|n| n as u32);
                let margin_modules = args.get("marginModules").and_then(Value::as_u64).map(|n| n as u32);
                let opts = crate::extensions::attachments::media_qr_png::QrPngRenderOptions {
                    scale,
                    margin_modules,
                };
                crate::extensions::attachments::media_qr_png::render_qr_png_data_url(input, &opts)
                    .map(Value::String)
            })(),
            "convertBmpToPng" => (|| {
                let b64 = args
                    .get("bytesBase64")
                    .or_else(|| args.get("base64"))
                    .or_else(|| args.get("data"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "convertBmpToPng.base64 must be a string".to_string())?;
                use base64::Engine;
                let raw = base64::prelude::BASE64_STANDARD
                    .decode(b64)
                    .map_err(|e| format!("Invalid base64 payload: {e}"))?;
                let png = crate::extensions::attachments::image_ops::convert_bmp_to_png(&raw)?;
                let encoded = base64::prelude::BASE64_STANDARD.encode(&png);
                if args.get("base64").is_some() || args.get("data").is_some() {
                    Ok(serde_json::json!({
                        "data": encoded,
                        "mimeType": "image/png"
                    }))
                } else {
                    Ok(Value::String(encoded))
                }
            })(),
            "formatMediaSize" => {
                let bytes = args
                    .get("bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                Ok(Value::String(crate::extensions::attachments::media_store_shared::format_media_size(bytes)))
            }
            "saveMediaBuffer" => (|| {
                let b64 = args
                    .get("bufferBase64")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "saveMediaBuffer.bufferBase64 must be a string".to_string())?;
                let content_type = args.get("contentType").and_then(Value::as_str);
                let subdir = args.get("subdir").and_then(Value::as_str).unwrap_or("inbound");
                let max_bytes = args.get("maxBytes").and_then(Value::as_u64).unwrap_or(crate::extensions::attachments::media_store::MEDIA_MAX_BYTES);
                let original_filename = args.get("originalFilename").and_then(Value::as_str);

                use base64::Engine;
                let raw = base64::prelude::BASE64_STANDARD
                    .decode(b64)
                    .map_err(|e| format!("Invalid base64 buffer: {e}"))?;

                let saved = crate::extensions::attachments::media_store::save_media_buffer(
                    &raw,
                    content_type,
                    subdir,
                    max_bytes,
                    original_filename,
                    None,
                    None,
                ).map_err(|e| e.to_string())?;

                serde_json::to_value(&saved).map_err(|e| e.to_string())
            })(),
            "readMediaBuffer" => (|| {
                let id = args
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "readMediaBuffer.id must be a string".to_string())?;
                let subdir = args.get("subdir").and_then(Value::as_str).unwrap_or("inbound");
                let max_bytes = args.get("maxBytes").and_then(Value::as_u64).unwrap_or(crate::extensions::attachments::media_store::MEDIA_MAX_BYTES);

                let (bytes, path) = crate::extensions::attachments::media_store::read_media_buffer(
                    id,
                    subdir,
                    max_bytes,
                    None,
                ).map_err(|e| e.to_string())?;

                use base64::Engine;
                let b64 = base64::prelude::BASE64_STANDARD.encode(&bytes);

                Ok(json!({
                    "id": id,
                    "path": path.to_string_lossy(),
                    "size": bytes.len(),
                    "bufferBase64": b64
                }))
            })(),
            "deleteMediaBuffer" => (|| {
                let id = args
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "deleteMediaBuffer.id must be a string".to_string())?;
                let subdir = args.get("subdir").and_then(Value::as_str).unwrap_or("inbound");
                crate::extensions::attachments::media_store::delete_media_buffer(id, subdir, None)
                    .map(|_| Value::Bool(true))
            })(),
            "cleanOldMedia" => (|| {
                let ttl_ms = args.get("ttlMs").and_then(Value::as_u64).unwrap_or(crate::extensions::attachments::media_store::DEFAULT_TTL_MS);
                let recursive = args.get("recursive").and_then(Value::as_bool);
                let prune_empty_dirs = args.get("pruneEmptyDirs").and_then(Value::as_bool);
                let opts = crate::extensions::attachments::media_store::CleanOldMediaOptions {
                    recursive,
                    prune_empty_dirs,
                };
                crate::extensions::attachments::media_store::clean_old_media(ttl_ms, &opts, None)
                    .map(|_| Value::Bool(true))
            })(),
            "extractOriginalFilename" => {
                let path = args
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Ok(Value::String(crate::extensions::attachments::media_store::extract_original_filename(path)))
            }
            "readImageMetadata" => (|| {
                let base64_str = args
                    .get("base64")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "readImageMetadata.base64 must be a string".to_string())?;
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(base64_str)
                    .map_err(|e| format!("Invalid base64: {e}"))?;
                let meta = crate::extensions::attachments::image_ops::read_image_metadata_from_header(&bytes);
                Ok(serde_json::to_value(meta).unwrap_or(Value::Null))
            })(),
            "isAnimatedWebp" => (|| {
                let base64_str = args
                    .get("base64")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "isAnimatedWebp.base64 must be a string".to_string())?;
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(base64_str)
                    .map_err(|e| format!("Invalid base64: {e}"))?;
                Ok(Value::Bool(crate::extensions::attachments::image_ops::is_animated_webp_buffer(&bytes)))
            })(),
            "isStillPng" => (|| {
                let base64_str = args
                    .get("base64")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "isStillPng.base64 must be a string".to_string())?;
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(base64_str)
                    .map_err(|e| format!("Invalid base64: {e}"))?;
                Ok(Value::Bool(crate::extensions::attachments::image_ops::is_still_png_buffer(&bytes)))
            })(),
            "buildImageResizeSideGrid" => (|| {
                let max_side = args.get("maxSide").and_then(Value::as_u64).unwrap_or(0) as u32;
                let side_start = args.get("sideStart").and_then(Value::as_u64).unwrap_or(0) as u32;
                let grid = crate::extensions::attachments::image_ops::build_image_resize_side_grid(max_side, side_start);
                Ok(serde_json::to_value(grid).unwrap_or(Value::Array(Vec::new())))
            })(),
            "prepareAttachment" => (|| {
                let input: crate::extensions::attachments::image_ops::AttachmentInput = serde_json::from_value(args.clone())
                    .map_err(|e| format!("Invalid AttachmentInput: {e}"))?;
                let prepared = crate::extensions::attachments::image_ops::prepare_attachment(&input)?;
                Ok(serde_json::to_value(prepared).unwrap_or(Value::Null))
            })(),
            "normalizeAnthropicInlineImage" => (|| {
                let data = args
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "normalizeAnthropicInlineImage.data must be a string".to_string())?;
                let mime = args
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("image/jpeg");
                let (norm_data, norm_mime) = crate::extensions::attachments::image_ops::normalize_anthropic_inline_image(data, mime)?;
                Ok(serde_json::json!({
                    "data": norm_data,
                    "mimeType": norm_mime
                }))
            })(),
            "normalizeAnthropicInlineContentBlocks" => (|| {
                let blocks_val = args
                    .get("blocks")
                    .ok_or_else(|| "normalizeAnthropicInlineContentBlocks.blocks is required".to_string())?;
                let blocks: Vec<crate::extensions::attachments::anthropic_inline_images::AnthropicContentBlock> =
                    serde_json::from_value(blocks_val.clone())
                        .map_err(|e| format!("Invalid AnthropicContentBlock array: {e}"))?;
                let normalized = crate::extensions::attachments::anthropic_inline_images::normalize_anthropic_inline_content_blocks(&blocks)?;
                Ok(serde_json::to_value(normalized).unwrap_or(Value::Array(Vec::new())))
            })(),
            "loadWebMedia" => (|| {
                let url = args
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "loadWebMedia.url must be a string".to_string())?;
                let max_bytes = args.get("maxBytes").and_then(Value::as_u64).map(|v| v as usize);
                let optimize = args.get("optimizeImages").and_then(Value::as_bool).unwrap_or(true);
                let opts = crate::extensions::attachments::web_media::WebMediaOptions {
                    max_bytes,
                    optimize_images: optimize,
                    ..Default::default()
                };
                let res = crate::extensions::attachments::web_media::load_web_media(url, &opts)
                    .map_err(|e| format!("{e}"))?;
                Ok(serde_json::to_value(res).unwrap_or(Value::Null))
            })(),
            "resolveChannelInboundRoots" => (|| {
                let channel_id = args.get("channelId").and_then(Value::as_str);
                let account_id = args.get("accountId").and_then(Value::as_str);
                let cfg = crate::extensions::attachments::channel_inbound_roots::ChannelInboundRootsConfig::default();
                let roots = crate::extensions::attachments::channel_inbound_roots::resolve_channel_inbound_attachment_roots_for_channel(
                    &cfg,
                    &[],
                    channel_id,
                    account_id,
                )?;
                Ok(serde_json::to_value(roots).unwrap_or(Value::Null))
            })(),
            "resolveLocalRoots" => (|| {
                let state_dir_str = args.get("stateDir").and_then(Value::as_str).unwrap_or("/tmp");
                let config_dir_str = args.get("configDir").and_then(Value::as_str).unwrap_or("/tmp");
                let agent_id = args.get("agentId").and_then(Value::as_str);
                let session_ws_str = args.get("sessionWorkspaceDir").and_then(Value::as_str);
                let state_dir = Path::new(state_dir_str);
                let config_dir = Path::new(config_dir_str);
                let session_ws = session_ws_str.map(Path::new);
                let roots = crate::extensions::attachments::local_roots::get_agent_scoped_media_local_roots(
                    state_dir,
                    config_dir,
                    agent_id,
                    None,
                    session_ws,
                );
                let str_roots: Vec<String> = roots.into_iter().map(|p| p.to_string_lossy().to_string()).collect();
                Ok(serde_json::to_value(str_roots).unwrap_or(Value::Null))
            })(),
            "assertHostReadMediaAllowed" => (|| {
                let file_path_str = args.get("filePath").and_then(Value::as_str);
                let file_path = file_path_str.map(Path::new);
                let sniffed = args.get("sniffedContentType").and_then(Value::as_str);
                let content_type = args.get("contentType").and_then(Value::as_str);
                let kind = args.get("kind").and_then(Value::as_str);
                let data_base64 = args.get("dataBase64").and_then(Value::as_str);
                let decoded = data_base64.and_then(|b64| STANDARD.decode(b64).ok());
                let params = crate::extensions::attachments::host_read_media_policy::AssertHostReadParams {
                    sniffed_content_type: sniffed,
                    content_type,
                    file_path,
                    kind,
                    buffer: decoded.as_deref(),
                    trusted_generated_html_path: None,
                    explicit_html_publication: false,
                };
                crate::extensions::attachments::host_read_media_policy::assert_host_read_media_allowed(&params)
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::json!({ "allowed": true }))
            })(),
            "readRemoteMediaBuffer" => (|| {
                let options: crate::extensions::attachments::media_fetch::FetchMediaOptions =
                    serde_json::from_value(args.clone())
                        .map_err(|e| format!("Invalid FetchMediaOptions: {e}"))?;
                let res = crate::extensions::attachments::media_fetch::read_remote_media_buffer(&options)
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::to_value(res).unwrap_or(Value::Null))
            })(),
            "saveRemoteMedia" => (|| {
                let options: crate::extensions::attachments::media_fetch::SaveRemoteMediaOptions =
                    serde_json::from_value(args.clone())
                        .map_err(|e| format!("Invalid SaveRemoteMediaOptions: {e}"))?;
                let res = crate::extensions::attachments::media_fetch::save_remote_media(&options)
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::to_value(res).unwrap_or(Value::Null))
            })(),
            "buildOutboundMediaLoadOptions" => (|| {
                let params: crate::extensions::attachments::load_options::OutboundMediaLoadParams =
                    serde_json::from_value(args.clone())
                        .map_err(|e| format!("Invalid OutboundMediaLoadParams: {e}"))?;
                let opts = crate::extensions::attachments::load_options::build_outbound_media_load_options(&params)?;
                Ok(serde_json::to_value(opts).unwrap_or(Value::Null))
            })(),
            "resolveAgentScopedOutboundMediaAccess" => (|| {
                let params: crate::extensions::attachments::read_capability::AgentScopedOutboundMediaAccessParams =
                    serde_json::from_value(args.clone())
                        .map_err(|e| format!("Invalid AgentScopedOutboundMediaAccessParams: {e}"))?;
                let access = crate::extensions::attachments::read_capability::resolve_agent_scoped_outbound_media_access(&params);
                Ok(serde_json::to_value(access).unwrap_or(Value::Null))
            })(),
            "resolveInputFileLimits" => (|| {
                let config: Option<crate::extensions::attachments::input_files::InputFileLimitsConfig> =
                    if args.is_null() || (args.is_object() && args.as_object().unwrap().is_empty()) {
                        None
                    } else {
                        Some(serde_json::from_value(args.clone()).map_err(|e| format!("Invalid InputFileLimitsConfig: {e}"))?)
                    };
                let limits = crate::extensions::attachments::input_files::resolve_input_file_limits(config.as_ref());
                Ok(serde_json::to_value(limits).unwrap_or(Value::Null))
            })(),
            "extractFileContentFromBuffer" => (|| {
                let b64 = args.get("bufferBase64").and_then(Value::as_str)
                    .ok_or_else(|| "extractFileContentFromBuffer requires bufferBase64".to_string())?;
                let buf = STANDARD.decode(b64).map_err(|e| format!("Invalid base64: {e}"))?;
                let filename = args.get("filename").and_then(Value::as_str);
                let mime_type = args.get("mimeType").and_then(Value::as_str);
                let charset = args.get("charset").and_then(Value::as_str);
                let limits_val = args.get("limits").cloned().unwrap_or(Value::Null);
                let limits: crate::extensions::attachments::input_files::InputFileLimits =
                    serde_json::from_value(limits_val).unwrap_or_else(|_| {
                        crate::extensions::attachments::input_files::resolve_input_file_limits(None)
                    });
                let result = crate::extensions::attachments::input_files::extract_file_content_from_buffer(
                    &buf,
                    filename,
                    mime_type,
                    charset,
                    &limits,
                    None,
                )?;
                Ok(serde_json::to_value(result).unwrap_or(Value::Null))
            })(),
            "extractFileContentFromSource" => (|| {
                let source_val = args.get("source").cloned().ok_or_else(|| "extractFileContentFromSource requires source".to_string())?;
                let source: crate::extensions::attachments::input_files::InputFileSource =
                    serde_json::from_value(source_val).map_err(|e| format!("Invalid InputFileSource: {e}"))?;
                let limits_val = args.get("limits").cloned().unwrap_or(Value::Null);
                let limits: crate::extensions::attachments::input_files::InputFileLimits =
                    serde_json::from_value(limits_val).unwrap_or_else(|_| {
                        crate::extensions::attachments::input_files::resolve_input_file_limits(None)
                    });
                let result = crate::extensions::attachments::input_files::extract_file_content_from_source(&source, &limits)?;
                Ok(serde_json::to_value(result).unwrap_or(Value::Null))
            })(),
            "extractImageContentFromSource" => (|| {
                let source_val = args.get("source").cloned().ok_or_else(|| "extractImageContentFromSource requires source".to_string())?;
                let source: crate::extensions::attachments::input_files::InputImageSource =
                    serde_json::from_value(source_val).map_err(|e| format!("Invalid InputImageSource: {e}"))?;
                let limits_val = args.get("limits").cloned().unwrap_or(Value::Null);
                let limits: crate::extensions::attachments::input_files::InputImageLimits =
                    serde_json::from_value(limits_val).map_err(|e| format!("Invalid InputImageLimits: {e}"))?;
                let result = crate::extensions::attachments::input_files::extract_image_content_from_source(&source, &limits)?;
                Ok(serde_json::to_value(result).unwrap_or(Value::Null))
            })(),
            "resolveStagedInputMediaPaths" => (|| {
                let media: Option<Vec<crate::extensions::attachments::media_facts::MediaFact>> =
                    if let Some(m) = args.get("media") {
                        Some(serde_json::from_value(m.clone()).map_err(|e| format!("Invalid MediaFact list: {e}"))?)
                    } else if args.is_array() {
                        Some(serde_json::from_value(args.clone()).map_err(|e| format!("Invalid MediaFact array: {e}"))?)
                    } else {
                        None
                    };
                let resolved = crate::extensions::attachments::staged_inputs::resolve_staged_input_media_paths(media.as_deref());
                Ok(serde_json::to_value(resolved).unwrap_or(Value::Null))
            })(),
            "stagedInputPathDirectory" => (|| {
                let path = args.get("relativePath").and_then(Value::as_str)
                    .or_else(|| args.as_str())
                    .ok_or_else(|| "stagedInputPathDirectory requires relativePath".to_string())?;
                let dir = crate::extensions::attachments::staged_inputs::staged_input_path_directory(path);
                Ok(serde_json::to_value(dir).unwrap_or(Value::Null))
            })(),
            "isStagedInputPath" => (|| {
                let path = args.get("relativePath").and_then(Value::as_str)
                    .ok_or_else(|| "isStagedInputPath requires relativePath".to_string())?;
                let dirs: std::collections::HashSet<String> = if let Some(d) = args.get("directories") {
                    serde_json::from_value(d.clone()).unwrap_or_default()
                } else {
                    std::collections::HashSet::new()
                };
                let matched = crate::extensions::attachments::staged_inputs::is_staged_input_path(path, &dirs);
                Ok(Value::Bool(matched))
            })(),
            "ensureStagedInputDirectory" => (|| {
                let root_str = args.get("rootDir").and_then(Value::as_str)
                    .ok_or_else(|| "ensureStagedInputDirectory requires rootDir".to_string())?;
                let dir_str = args.get("directory").and_then(Value::as_str)
                    .ok_or_else(|| "ensureStagedInputDirectory requires directory".to_string())?;
                crate::extensions::attachments::staged_inputs::ensure_staged_input_directory(Path::new(root_str), dir_str)?;
                Ok(serde_json::json!({ "success": true }))
            })(),
            "extractPdfContent" => (|| {
                let req: crate::extensions::attachments::pdf_extract::PdfExtractionRequest =
                    serde_json::from_value(args.clone()).map_err(|e| format!("Invalid PdfExtractionRequest: {e}"))?;
                let res = crate::extensions::attachments::pdf_extract::extract_pdf_content(&req, &[])?;
                Ok(serde_json::to_value(res).unwrap_or(Value::Null))
            })(),
            "validatePdfExtractionMetadata" => (|| {
                let meta_val = args.get("metadata").ok_or_else(|| "metadata required".to_string())?;
                let req_val = args.get("request").ok_or_else(|| "request required".to_string())?;
                let meta: crate::extensions::attachments::pdf_extract::DocumentExtractionMetadata =
                    serde_json::from_value(meta_val.clone()).map_err(|e| format!("Invalid metadata: {e}"))?;
                let req: crate::extensions::attachments::pdf_extract::PdfExtractionRequest =
                    serde_json::from_value(req_val.clone()).map_err(|e| format!("Invalid request: {e}"))?;
                let trusted = crate::extensions::attachments::pdf_extract::validate_extraction_metadata(&meta, &req);
                Ok(serde_json::to_value(trusted).unwrap_or(Value::Null))
            })(),
            "splitMediaFromOutput" => (|| {
                let raw = args.get("raw").and_then(Value::as_str)
                    .or_else(|| args.as_str())
                    .ok_or_else(|| "splitMediaFromOutput requires raw text".to_string())?;
                let opts: crate::extensions::attachments::parse_output::SplitMediaFromOutputOptions =
                    if let Some(o) = args.get("options") {
                        serde_json::from_value(o.clone()).unwrap_or_default()
                    } else {
                        crate::extensions::attachments::parse_output::SplitMediaFromOutputOptions {
                            extract_media_directives: true,
                            extract_audio_directives: true,
                            ..Default::default()
                        }
                    };
                let parsed = crate::extensions::attachments::parse_output::split_media_from_output(raw, &opts);
                Ok(serde_json::to_value(parsed).unwrap_or(Value::Null))
            })(),
            "splitMediaOutput" => (|| {
                let raw = args.get("raw").and_then(Value::as_str)
                    .or_else(|| args.as_str())
                    .ok_or_else(|| "splitMediaOutput requires raw text".to_string())?;
                let opts: crate::extensions::attachments::parse_output::SplitMediaOutputOptions =
                    if let Some(o) = args.get("options") {
                        serde_json::from_value(o.clone()).unwrap_or_default()
                    } else {
                        crate::extensions::attachments::parse_output::SplitMediaOutputOptions {
                            extract_media_directives: true,
                            extract_audio_directives: true,
                            ..Default::default()
                        }
                    };
                let parsed = crate::extensions::attachments::parse_output::split_media_output(raw, &opts);
                Ok(serde_json::to_value(parsed).unwrap_or(Value::Null))
            })(),
            _ => return None,
        };
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_metadata_parser_preserves_frozen_precedence_and_limits() {
        let html = r#"
          <html><head>
          <title>fallback</title>
          <meta property="og:title" content="  &amp; Lotus   Title ">
          <meta name="description" content="description">
          <meta property="og:site_name" content="Example">
          <meta property="og:image" content="/image.png">
          <link rel="canonical" href="/canonical">
          <link rel="icon shortcut" href="/favicon.ico">
          </head></html>
        "#;
        let scraped = scrape_open_graph(html, "https://example.com/page");
        assert_eq!(scraped.title, "& Lotus Title");
        assert_eq!(scraped.description, "description");
        assert_eq!(scraped.site_name, "Example");
        assert_eq!(scraped.canonical_url, "https://example.com/canonical");
        assert_eq!(
            scraped.image_url.as_deref(),
            Some("https://example.com/image.png")
        );
        assert_eq!(
            scraped.favicon_url.as_deref(),
            Some("https://example.com/favicon.ico")
        );
    }

    #[test]
    fn authentication_destination_matches_frozen_hosts_labels_and_paths() {
        for url in [
            "https://accounts.google.com/",
            "https://login.example.com/",
            "https://example.com/oauth",
            "https://example.com/sign-in/identifier",
        ] {
            assert!(is_authentication_destination(&Url::parse(url).unwrap()));
        }
        assert!(!is_authentication_destination(
            &Url::parse("https://example.com/article").unwrap()
        ));
    }
}
