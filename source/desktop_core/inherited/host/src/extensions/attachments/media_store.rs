use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::extensions::attachments::media_store_shared::{
    MEDIA_FILE_MODE, SaveMediaSourceError, SaveMediaSourceErrorCode, format_media_size,
};
use crate::host_paths::get_sand_root_dir;

pub const MEDIA_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const PLAYBACK_TRANSCODE_SUBDIR: &str = "playback-transcode";
pub const MANAGED_OUTGOING_SUBDIR: &str = "outgoing";
pub const OUTBOUND_STAGING_SUBDIR: &str = "outbound";
pub const OUTBOUND_STAGING_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
pub const PLAYBACK_TRANSCODE_MAX_CACHE_BYTES: u64 = 512 * 1024 * 1024;
pub const PLAYBACK_TRANSCODE_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
pub const DEFAULT_TTL_MS: u64 = 2 * 60 * 1_000;
pub const MEDIA_STORE_LIFECYCLE_INTERVAL: Duration = Duration::from_secs(60 * 60);

static PLAYBACK_CACHE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub type MediaStoreLifecycleReporter = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// The sole periodic producer for media-store retention work.
/// The worker owns no media state; each pass delegates to the existing owner.
pub struct MediaStoreLifecycleWorker {
    stop: Arc<std::sync::atomic::AtomicBool>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl MediaStoreLifecycleWorker {
    pub fn start(reporter: Option<MediaStoreLifecycleReporter>) -> Result<Self, String> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_worker = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("fabushi-media-store-lifecycle".into())
            .spawn(move || {
                loop {
                    for (operation, result) in run_media_store_lifecycle_pass() {
                        if let Err(error) = result {
                            if let Some(report) = reporter.as_ref() {
                                report(operation, &error);
                            }
                        }
                    }
                    if stop_worker.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    thread::park_timeout(MEDIA_STORE_LIFECYCLE_INTERVAL);
                    if stop_worker.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                }
            })
            .map_err(|error| format!("could not start media-store lifecycle worker: {error}"))?;
        Ok(Self {
            stop,
            join: Mutex::new(Some(join)),
        })
    }

    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        let mut join = self.join.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(handle) = join.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

impl Drop for MediaStoreLifecycleWorker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(handle) = self
            .join
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

pub fn run_media_store_lifecycle_pass() -> Vec<(&'static str, Result<(), String>)> {
    vec![
        (
            "clean_old_media",
            clean_old_media(DEFAULT_TTL_MS, &CleanOldMediaOptions::default(), None),
        ),
        (
            "prune_playback_transcode_cache",
            prune_playback_transcode_cache(None),
        ),
        ("prune_outbound_media", prune_outbound_media(None)),
    ]
}

fn get_playback_cache_lock() -> &'static Mutex<()> {
    PLAYBACK_CACHE_LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedMedia {
    pub id: String,
    pub path: String,
    pub size: u64,
    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CleanOldMediaOptions {
    pub recursive: Option<bool>,
    pub prune_empty_dirs: Option<bool>,
}

pub fn get_media_dir() -> PathBuf {
    get_sand_root_dir().join("media")
}

pub fn ensure_media_dir() -> Result<PathBuf, io::Error> {
    let dir = get_media_dir();
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

pub fn resolve_media_subdir(subdir: &str, caller: &str) -> Result<String, String> {
    if subdir.is_empty() || subdir == "." {
        return Ok(String::new());
    }
    if subdir.contains('\0') {
        return Err(format!("{caller}: unsafe media subdir: {subdir:?}"));
    }
    let p = Path::new(subdir);
    if p.is_absolute() {
        return Err(format!("{caller}: unsafe media subdir: {subdir:?}"));
    }
    // Check for drive letters or UNC on Windows or backslash in posix
    if subdir.starts_with('/') || subdir.starts_with('\\') || (subdir.len() > 1 && subdir.as_bytes()[1] == b':') {
        return Err(format!("{caller}: unsafe media subdir: {subdir:?}"));
    }

    let mut segments = Vec::new();
    for seg in subdir.split(['/', '\\']) {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(format!("{caller}: unsafe media subdir: {subdir:?}"));
        }
        segments.push(seg);
    }
    if segments.is_empty() {
        Ok(String::new())
    } else {
        Ok(segments.join("/"))
    }
}

pub fn resolve_media_scoped_dir(base_dir: &Path, subdir: &str, caller: &str) -> Result<PathBuf, String> {
    let safe_subdir = resolve_media_subdir(subdir, caller)?;
    let target = if safe_subdir.is_empty() {
        base_dir.to_path_buf()
    } else {
        base_dir.join(&safe_subdir)
    };
    // Canonicalize the nearest existing ancestor so a symlinked intermediate
    // directory cannot bypass containment merely because the final directory
    // has not been created yet.
    let normalized_base = canonicalize_with_missing_tail(base_dir).map_err(|error| {
        format!("{caller}: could not resolve media directory identity: {error}")
    })?;
    let normalized_target = canonicalize_with_missing_tail(&target).map_err(|error| {
        format!("{caller}: could not resolve media target identity: {error}")
    })?;
    if !normalized_target.starts_with(&normalized_base) {
        return Err(format!("{caller}: media subdir escapes media directory: {subdir:?}"));
    }
    Ok(target)
}

fn canonicalize_with_missing_tail(path: &Path) -> io::Result<PathBuf> {
    let mut missing_tail = Vec::new();
    let mut cursor = path.to_path_buf();
    loop {
        match cursor.canonicalize() {
            Ok(mut canonical) => {
                for component in missing_tail.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(name) = cursor.file_name() else {
                    return Err(error);
                };
                missing_tail.push(name.to_os_string());
                let Some(parent) = cursor.parent() else {
                    return Err(error);
                };
                cursor = parent.to_path_buf();
            }
            Err(error) => return Err(error),
        }
    }
}

pub fn resolve_media_relative_path(id: &str, subdir: &str, caller: &str) -> Result<String, String> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains('\0') || id == ".." {
        return Err(format!("{caller}: unsafe media ID: {id:?}"));
    }
    let safe_subdir = resolve_media_subdir(subdir, caller)?;
    if safe_subdir.is_empty() {
        Ok(id.to_string())
    } else {
        Ok(format!("{safe_subdir}/{id}"))
    }
}

pub fn sanitize_filename(name: &str) -> String {
    let nfc: String = name.nfc().collect();
    let mut sanitized = String::with_capacity(nfc.len());
    for ch in nfc.chars() {
        if ch.is_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }
    // Collapse consecutive underscores
    let mut collapsed = String::new();
    let mut last_was_underscore = false;
    for ch in sanitized.chars() {
        if ch == '_' {
            if !last_was_underscore {
                collapsed.push(ch);
                last_was_underscore = true;
            }
        } else {
            collapsed.push(ch);
            last_was_underscore = false;
        }
    }
    let trimmed = collapsed.trim_matches('_');
    // Truncate UTF-16 code units to 60
    let mut utf16_count = 0;
    let mut byte_limit = 0;
    for (idx, ch) in trimmed.char_indices() {
        let u16_len = ch.len_utf16();
        if utf16_count + u16_len > 60 {
            break;
        }
        utf16_count += u16_len;
        byte_limit = idx + ch.len_utf8();
    }
    trimmed[..byte_limit].to_string()
}

pub fn extract_original_filename(file_path: &str) -> String {
    let basename = Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file.bin");
    
    let path_obj = Path::new(basename);
    let ext = path_obj.extension().and_then(|e| e.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    let stem = path_obj.file_stem().and_then(|s| s.to_str()).unwrap_or(basename);

    if let Some(idx) = stem.rfind("---") {
        let suffix = &stem[idx + 3..];
        if suffix.len() == 36 && Uuid::parse_str(suffix).is_ok() {
            let original_stem = &stem[..idx];
            if !original_stem.is_empty() {
                return format!("{original_stem}{ext}");
            }
        }
    }
    basename.to_string()
}

pub fn build_saved_media_id(base_id: &str, ext: &str, original_filename: Option<&str>) -> String {
    let ext = if ext.is_empty() || ext.starts_with('.') {
        ext.to_string()
    } else {
        format!(".{ext}")
    };

    if let Some(orig) = original_filename {
        let orig_name = Path::new(orig)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let orig_stem = Path::new(orig_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(orig_name);
        let sanitized = sanitize_filename(orig_stem);
        if !sanitized.is_empty() {
            return format!("{sanitized}---{base_id}{ext}");
        }
    }
    format!("{base_id}{ext}")
}

fn safe_original_filename_extension(original_filename: Option<&str>) -> Option<String> {
    let orig = original_filename?;
    let ext = Path::new(orig).extension().and_then(|e| e.to_str())?;
    if !ext.is_empty() && ext.len() <= 16 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(format!(".{ext}"))
    } else {
        None
    }
}

pub fn resolve_saved_media_extension(
    detected_mime: Option<&str>,
    content_type: Option<&str>,
    original_filename: Option<&str>,
    hint_path: Option<&str>,
) -> String {
    if let Some(det) = detected_mime {
        let is_container = det == "application/zip" || det == "application/octet-stream";
        let is_image_hdr = content_type.map(|c| c.starts_with("image/")).unwrap_or(false);
        if !(is_container && is_image_hdr) {
            if let Some(ext) = extension_for_mime(det) {
                return ext;
            }
        }
    }
    if let Some(ct) = content_type {
        if ct != "application/octet-stream" && ct != "binary/octet-stream" && ct != "application/zip" {
            if let Some(ext) = extension_for_mime(ct) {
                return ext;
            }
        }
    }
    if let Some(ext) = safe_original_filename_extension(original_filename) {
        return ext;
    }
    if let Some(hint) = hint_path {
        if let Some(ext) = Path::new(hint).extension().and_then(|e| e.to_str()) {
            return format!(".{ext}");
        }
    }
    String::new()
}

fn extension_for_mime(mime: &str) -> Option<String> {
    let clean = mime.split(';').next()?.trim().to_ascii_lowercase();
    match clean.as_str() {
        "image/png" => Some(".png".to_string()),
        "image/jpeg" | "image/jpg" => Some(".jpg".to_string()),
        "image/gif" => Some(".gif".to_string()),
        "image/webp" => Some(".webp".to_string()),
        "image/svg+xml" => Some(".svg".to_string()),
        "image/bmp" => Some(".bmp".to_string()),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some(".ico".to_string()),
        "audio/mpeg" | "audio/mp3" => Some(".mp3".to_string()),
        "audio/wav" | "audio/x-wav" => Some(".wav".to_string()),
        "audio/ogg" => Some(".ogg".to_string()),
        "audio/aac" => Some(".aac".to_string()),
        "audio/flac" => Some(".flac".to_string()),
        "video/mp4" => Some(".mp4".to_string()),
        "video/webm" => Some(".webm".to_string()),
        "application/pdf" => Some(".pdf".to_string()),
        "application/json" => Some(".json".to_string()),
        "text/plain" => Some(".txt".to_string()),
        "text/html" => Some(".html".to_string()),
        "text/markdown" => Some(".md".to_string()),
        _ => None,
    }
}

pub fn detect_mime_from_buffer(buf: &[u8], hint: Option<&str>) -> Option<String> {
    if buf.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png".to_string());
    }
    if buf.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg".to_string());
    }
    if buf.starts_with(b"GIF87a") || buf.starts_with(b"GIF89a") {
        return Some("image/gif".to_string());
    }
    if buf.starts_with(b"RIFF") && buf.len() >= 12 && &buf[8..12] == b"WEBP" {
        return Some("image/webp".to_string());
    }
    if buf.starts_with(b"BM") {
        return Some("image/bmp".to_string());
    }
    if buf.starts_with(b"%PDF-") {
        return Some("application/pdf".to_string());
    }
    if buf.starts_with(b"PK\x03\x04") {
        return Some("application/zip".to_string());
    }
    if let Some(h) = hint {
        if let Some(ext) = Path::new(h).extension().and_then(|e| e.to_str()) {
            match ext.to_ascii_lowercase().as_str() {
                "png" => return Some("image/png".to_string()),
                "jpg" | "jpeg" => return Some("image/jpeg".to_string()),
                "gif" => return Some("image/gif".to_string()),
                "webp" => return Some("image/webp".to_string()),
                "pdf" => return Some("application/pdf".to_string()),
                "txt" => return Some("text/plain".to_string()),
                "html" => return Some("text/html".to_string()),
                "json" => return Some("application/json".to_string()),
                _ => {}
            }
        }
    }
    None
}

pub fn save_media_buffer(
    buffer: &[u8],
    content_type: Option<&str>,
    subdir: &str,
    max_bytes: u64,
    original_filename: Option<&str>,
    detection_hint: Option<&str>,
    base_dir: Option<&Path>,
) -> Result<SavedMedia, SaveMediaSourceError> {
    if buffer.len() as u64 > max_bytes {
        return Err(SaveMediaSourceError::too_large(max_bytes));
    }
    let media_root = base_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_media_dir);
    let target_dir = resolve_media_scoped_dir(&media_root, subdir, "save_media_buffer")
        .map_err(|e| SaveMediaSourceError::new(SaveMediaSourceErrorCode::InvalidPath, e))?;
    fs::create_dir_all(&target_dir).map_err(|e| {
        SaveMediaSourceError::new(
            SaveMediaSourceErrorCode::InvalidPath,
            format!("Failed to create media directory: {e}"),
        )
    })?;

    let uuid = Uuid::new_v4().to_string();
    let detected = detect_mime_from_buffer(buffer, original_filename.or(detection_hint));
    let ext = resolve_saved_media_extension(
        detected.as_deref(),
        content_type,
        original_filename,
        detection_hint,
    );
    let id = build_saved_media_id(&uuid, &ext, original_filename);

    let temp_name = format!(".{id}.tmp-{}", Uuid::new_v4());
    let temp_path = target_dir.join(&temp_name);
    let final_path = target_dir.join(&id);

    {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|e| {
                SaveMediaSourceError::new(
                    SaveMediaSourceErrorCode::InvalidPath,
                    format!("Failed to open temp file: {e}"),
                )
            })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = f.set_permissions(fs::Permissions::from_mode(MEDIA_FILE_MODE));
        }
        f.write_all(buffer).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            SaveMediaSourceError::new(
                SaveMediaSourceErrorCode::InvalidPath,
                format!("Failed to write media: {e}"),
            )
        })?;
        f.sync_all().map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            SaveMediaSourceError::new(
                SaveMediaSourceErrorCode::InvalidPath,
                format!("Failed to sync media: {e}"),
            )
        })?;
    }

    fs::rename(&temp_path, &final_path).map_err(|e| {
        let _ = fs::remove_file(&temp_path);
        SaveMediaSourceError::new(
            SaveMediaSourceErrorCode::PathMismatch,
            format!("Atomic rename failed: {e}"),
        )
    })?;

    let canonical_final = final_path.canonicalize().map_err(|e| {
        SaveMediaSourceError::new(
            SaveMediaSourceErrorCode::PathMismatch,
            format!("Saved media path identity could not be verified: {e}"),
        )
    })?;

    Ok(SavedMedia {
        id,
        path: canonical_final.to_string_lossy().to_string(),
        size: buffer.len() as u64,
        content_type: detected.or_else(|| content_type.map(|s| s.to_string())),
    })
}

pub fn save_media_source(
    source: &str,
    headers: Option<&BTreeMap<String, String>>,
    subdir: &str,
    max_bytes: u64,
    base_dir: Option<&Path>,
    workspace_root: Option<&Path>,
) -> Result<SavedMedia, SaveMediaSourceError> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return save_remote_media_for_store(source, headers, subdir, max_bytes, base_dir);
    }

    let source_path = Path::new(source);
    let meta = match fs::symlink_metadata(source_path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(SaveMediaSourceError::new(
                SaveMediaSourceErrorCode::NotFound,
                "Media path does not exist",
            ));
        }
        Err(e) => {
            return Err(SaveMediaSourceError::new(
                SaveMediaSourceErrorCode::InvalidPath,
                format!("Media path access error: {e}"),
            ));
        }
    };

    if meta.file_type().is_symlink() {
        return Err(SaveMediaSourceError::new(
            SaveMediaSourceErrorCode::InvalidPath,
            "Media path must not be a symlink",
        ));
    }

    if meta.is_dir() {
        return Err(SaveMediaSourceError::new(
            SaveMediaSourceErrorCode::NotFile,
            "Media path is not a file",
        ));
    }

    if meta.len() > max_bytes {
        return Err(SaveMediaSourceError::too_large(max_bytes));
    }

    if let Some(ws) = workspace_root {
        let canonical_src = source_path.canonicalize().map_err(|e| {
            SaveMediaSourceError::new(
                SaveMediaSourceErrorCode::InvalidPath,
                format!("Failed to canonicalize path: {e}"),
            )
        })?;
        let canonical_ws = ws.canonicalize().unwrap_or_else(|_| ws.to_path_buf());
        if !canonical_src.starts_with(&canonical_ws) {
            return Err(SaveMediaSourceError::new(
                SaveMediaSourceErrorCode::InvalidPath,
                "Media path is outside workspace root",
            ));
        }
    }

    let bytes = read_media_source_bytes(source_path, max_bytes).map_err(|e| {
        SaveMediaSourceError::new(
            SaveMediaSourceErrorCode::InvalidPath,
            format!("Failed to read source file: {e}"),
        )
    })?;

    let original_filename = source_path.file_name().and_then(|n| n.to_str());
    save_media_buffer(
        &bytes,
        None,
        subdir,
        max_bytes,
        original_filename,
        Some(source),
        base_dir,
    )
}

fn read_media_source_bytes(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "media source is not a regular file",
        ));
    }
    let read_limit = max_bytes.saturating_add(1);
    let mut bytes = Vec::new();
    file.take(read_limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn save_remote_media_for_store(
    url: &str,
    _headers: Option<&BTreeMap<String, String>>,
    subdir: &str,
    max_bytes: u64,
    base_dir: Option<&Path>,
) -> Result<SavedMedia, SaveMediaSourceError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(SaveMediaSourceError::new(
            SaveMediaSourceErrorCode::InvalidPath,
            "URL must use http or https protocol",
        ));
    }

    // In native runtime without active outbound networking, we guard or download boundedly
    // For local test URLs or mock servers:
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| {
            SaveMediaSourceError::new(
                SaveMediaSourceErrorCode::InvalidPath,
                format!("HTTP client error: {e}"),
            )
        })?
        .get(url)
        .send()
        .map_err(|e| {
            SaveMediaSourceError::new(
                SaveMediaSourceErrorCode::InvalidPath,
                format!("Remote fetch error: {e}"),
            )
        })?;

    if !response.status().is_success() {
        return Err(SaveMediaSourceError::new(
            SaveMediaSourceErrorCode::InvalidPath,
            format!("Remote server responded with {}", response.status()),
        ));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let mut bytes = Vec::new();
    response
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| {
            SaveMediaSourceError::new(
                SaveMediaSourceErrorCode::InvalidPath,
                format!("Failed to read remote payload: {e}"),
            )
        })?;

    if bytes.len() as u64 > max_bytes {
        return Err(SaveMediaSourceError::too_large(max_bytes));
    }

    let synthetic_ext = Path::new(url)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!("_{e}"));

    save_media_buffer(
        &bytes,
        content_type.as_deref(),
        subdir,
        max_bytes,
        synthetic_ext.as_deref(),
        Some(url),
        base_dir,
    )
}

pub fn resolve_media_buffer_path(
    id: &str,
    subdir: &str,
    base_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let media_root = base_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_media_dir);
    let target_dir = resolve_media_scoped_dir(&media_root, subdir, "resolve_media_buffer_path")?;
    let safe_id = resolve_media_relative_path(id, "", "resolve_media_buffer_path")?;
    let target_file = target_dir.join(safe_id);

    let meta = fs::symlink_metadata(&target_file).map_err(|e| {
        format!("resolveMediaBufferPath: media ID does not resolve to a file: {id:?} ({e})")
    })?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "resolveMediaBufferPath: media ID must not resolve through a symlink: {id:?}"
        ));
    }
    if !meta.is_file() {
        return Err(format!("resolveMediaBufferPath: media ID does not resolve to a file: {id:?}"));
    }
    let canonical_dir = target_dir.canonicalize().map_err(|e| {
        format!("resolveMediaBufferPath: media directory cannot be resolved: {e}")
    })?;
    let canonical_file = target_file.canonicalize().map_err(|e| {
        format!("resolveMediaBufferPath: media file cannot be resolved: {e}")
    })?;
    if !canonical_file.starts_with(&canonical_dir) {
        return Err(format!(
            "resolveMediaBufferPath: media ID resolves outside its scoped directory: {id:?}"
        ));
    }
    Ok(canonical_file)
}

pub fn read_media_buffer(
    id: &str,
    subdir: &str,
    max_bytes: u64,
    base_dir: Option<&Path>,
) -> Result<(Vec<u8>, PathBuf), SaveMediaSourceError> {
    let path = resolve_media_buffer_path(id, subdir, base_dir)
        .map_err(|e| SaveMediaSourceError::new(SaveMediaSourceErrorCode::NotFound, e))?;
    let meta = fs::metadata(&path).map_err(|e| {
        SaveMediaSourceError::new(
            SaveMediaSourceErrorCode::NotFound,
            format!("Failed to stat media buffer: {e}"),
        )
    })?;
    if meta.len() > max_bytes {
        return Err(SaveMediaSourceError::too_large(max_bytes));
    }
    let data = read_media_source_bytes(&path, max_bytes).map_err(|e| {
        SaveMediaSourceError::new(
            SaveMediaSourceErrorCode::InvalidPath,
            format!("Failed to read media buffer: {e}"),
        )
    })?;
    if data.len() as u64 > max_bytes {
        return Err(SaveMediaSourceError::too_large(max_bytes));
    }
    Ok((data, path))
}

pub fn delete_media_buffer(
    id: &str,
    subdir: &str,
    base_dir: Option<&Path>,
) -> Result<(), String> {
    let media_root = base_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_media_dir);
    let target_dir = resolve_media_scoped_dir(&media_root, subdir, "delete_media_buffer")?;
    let safe_id = resolve_media_relative_path(id, "", "delete_media_buffer")?;
    let target_file = target_dir.join(safe_id);
    if target_file.exists() {
        fs::remove_file(&target_file).map_err(|e| format!("Failed to delete media buffer: {e}"))?;
    }
    Ok(())
}

pub fn write_playback_transcode_cache(
    file_name: &str,
    buffer: &[u8],
    max_bytes: u64,
    base_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let _guard = get_playback_cache_lock().lock().unwrap();
    if buffer.len() as u64 > max_bytes {
        return Err(format!("Playback buffer exceeds max_bytes {max_bytes}"));
    }
    let media_root = base_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_media_dir);
    let cache_dir = resolve_media_scoped_dir(&media_root, PLAYBACK_TRANSCODE_SUBDIR, "write_playback_transcode_cache")?;
    fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {e}"))?;

    let safe_file_name = resolve_media_relative_path(
        file_name,
        "",
        "write_playback_transcode_cache",
    )?;
    let target_path = cache_dir.join(&safe_file_name);
    let temp_name = format!(".{safe_file_name}.tmp-{}", Uuid::new_v4());
    let temp_path = cache_dir.join(&temp_name);

    {
        let mut f = File::create(&temp_path).map_err(|e| format!("Failed to create temp cache file: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = f.set_permissions(fs::Permissions::from_mode(MEDIA_FILE_MODE));
        }
        f.write_all(buffer).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            format!("Failed to write cache file: {e}")
        })?;
    }
    fs::rename(&temp_path, &target_path).map_err(|e| {
        let _ = fs::remove_file(&temp_path);
        format!("Failed to rename cache file: {e}")
    })?;

    prune_playback_transcode_cache_to_size(&cache_dir, PLAYBACK_TRANSCODE_MAX_CACHE_BYTES)?;
    Ok(target_path)
}

pub fn prune_playback_transcode_cache_to_size(cache_dir: &Path, max_bytes: u64) -> Result<(), String> {
    if !cache_dir.exists() {
        return Ok(());
    }
    let entries = fs::read_dir(cache_dir).map_err(|e| format!("Failed to read cache dir: {e}"))?;
    let mut files = Vec::new();
    let mut total_bytes: u64 = 0;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
                let size = meta.len();
                total_bytes += size;
                files.push((mtime, name, size, entry.path()));
            }
        }
    }

    // Sort by mtime ascending (oldest first)
    files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    for (_, _, size, path) in files {
        if total_bytes <= max_bytes {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total_bytes = total_bytes.saturating_sub(size);
        }
    }
    Ok(())
}

pub fn prune_playback_transcode_cache(base_dir: Option<&Path>) -> Result<(), String> {
    let _guard = get_playback_cache_lock().lock().unwrap();
    let media_root = base_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_media_dir);
    let cache_dir = resolve_media_scoped_dir(&media_root, PLAYBACK_TRANSCODE_SUBDIR, "prune_playback_transcode_cache")?;
    if !cache_dir.exists() {
        return Ok(());
    }

    let now = SystemTime::now();
    let ttl = Duration::from_millis(PLAYBACK_TRANSCODE_TTL_MS);

    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    if let Ok(mtime) = meta.modified() {
                        if let Ok(age) = now.duration_since(mtime) {
                            if age > ttl {
                                let _ = fs::remove_file(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }
    prune_playback_transcode_cache_to_size(&cache_dir, PLAYBACK_TRANSCODE_MAX_CACHE_BYTES)
}

pub fn prune_outbound_media(base_dir: Option<&Path>) -> Result<(), String> {
    let media_root = base_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_media_dir);
    let outbound_dir = resolve_media_scoped_dir(&media_root, OUTBOUND_STAGING_SUBDIR, "prune_outbound_media")?;
    if !outbound_dir.exists() {
        return Ok(());
    }

    let now = SystemTime::now();
    let ttl = Duration::from_millis(OUTBOUND_STAGING_TTL_MS);

    if let Ok(entries) = fs::read_dir(&outbound_dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    if let Ok(mtime) = meta.modified() {
                        if let Ok(age) = now.duration_since(mtime) {
                            if age > ttl {
                                let _ = fs::remove_file(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn clean_old_media(
    ttl_ms: u64,
    options: &CleanOldMediaOptions,
    base_dir: Option<&Path>,
) -> Result<(), String> {
    let media_root = base_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_media_dir);
    if !media_root.exists() {
        return Ok(());
    }

    let now = SystemTime::now();
    let ttl = Duration::from_millis(ttl_ms);

    let entries = fs::read_dir(&media_root).map_err(|e| format!("Failed to read media root: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Never clean playback transcode cache or sqlite-managed outgoing directory with mtime sweep
        if name == PLAYBACK_TRANSCODE_SUBDIR || name == MANAGED_OUTGOING_SUBDIR {
            continue;
        }

        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                if let Ok(mtime) = meta.modified() {
                    if let Ok(age) = now.duration_since(mtime) {
                        if age > ttl {
                            let _ = fs::remove_file(entry.path());
                        }
                    }
                }
            } else if meta.is_dir() && options.recursive.unwrap_or(true) {
                let sub_entries = fs::read_dir(entry.path()).ok();
                let mut empty = true;
                if let Some(sub) = sub_entries {
                    for s in sub.flatten() {
                        if let Ok(sm) = s.metadata() {
                            if sm.is_file() {
                                if let Ok(mtime) = sm.modified() {
                                    if let Ok(age) = now.duration_since(mtime) {
                                        if age > ttl {
                                            let _ = fs::remove_file(s.path());
                                        } else {
                                            empty = false;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if empty && options.prune_empty_dirs.unwrap_or(false) {
                    let _ = fs::remove_dir(entry.path());
                }
            }
        }
    }
    Ok(())
}

use std::sync::atomic::{AtomicUsize, Ordering};

static ACTIVE_REMOTE_FIXTURE_SCOPES: AtomicUsize = AtomicUsize::new(0);

/// Scoped remote fixture simulating network interceptors for media store tests.
#[derive(Debug, Clone)]
pub struct StoreRemoteFixtureScope {
    pub url: String,
    pub active: bool,
    pub matched_count: usize,
}

impl StoreRemoteFixtureScope {
    pub fn new(url: &str) -> Self {
        ACTIVE_REMOTE_FIXTURE_SCOPES.fetch_add(1, Ordering::SeqCst);
        Self {
            url: url.to_string(),
            active: true,
            matched_count: 0,
        }
    }

    pub fn matches(&mut self, request_url: &str) -> bool {
        if self.active && self.url == request_url {
            self.matched_count += 1;
            true
        } else {
            false
        }
    }

    pub fn deactivate(&mut self) {
        if self.active {
            self.active = false;
            ACTIVE_REMOTE_FIXTURE_SCOPES.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Drop for StoreRemoteFixtureScope {
    fn drop(&mut self) {
        self.deactivate();
    }
}

/// Verifies that all active test fixtures have joined and completed.
pub fn check_active_remote_fixtures_empty() -> Result<(), String> {
    let count = ACTIVE_REMOTE_FIXTURE_SCOPES.load(Ordering::SeqCst);
    if count != 0 {
        Err(format!("Store remote fixtures are still active: {count}"))
    } else {
        Ok(())
    }
}

/// Helper asserting original filename patterns from `store-filename.test-support.ts`.
pub fn expect_saved_original_filename_case(
    saved: &SavedMedia,
    expected_original_filename: Option<&str>,
    expect_uuid_only: bool,
) -> Result<(), String> {
    if expect_uuid_only {
        if saved.id.contains("---") {
            return Err(format!("Expected UUID-only ID, got: {}", saved.id));
        }
    }
    if let Some(expected) = expected_original_filename {
        let extracted = extract_original_filename(&saved.path);
        if extracted != expected {
            return Err(format!(
                "Original filename mismatch: expected '{expected}', extracted '{extracted}'"
            ));
        }
    }
    Ok(())
}
