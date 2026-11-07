use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use regex::Regex;
use std::sync::LazyLock;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum MediaReferenceError {
    #[error("invalid-path: {0}")]
    InvalidPath(String),
    #[error("path-not-allowed: {0}")]
    PathNotAllowed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMediaUri {
    pub id: String,
    pub normalized_source: String,
}

static MEDIA_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*MEDIA\s*:\s*").expect("valid regex"));
static WIN_DRIVE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z]:[\\/]").expect("valid regex"));
static SCHEME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*:").expect("valid regex"));

/// Strips legacy MEDIA: prefixes while preserving canonical media:// references.
pub fn normalize_media_reference_source(source: &str) -> String {
    let trimmed = source.trim();
    if trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("media://") {
        return trimmed.to_string();
    }
    MEDIA_PREFIX_RE.replace(trimmed, "").trim().to_string()
}

/// Parses canonical inbound media-store URIs and rejects nested or cross-bucket references.
pub fn parse_inbound_media_uri(source: &str) -> Result<Option<InboundMediaUri>, MediaReferenceError> {
    let normalized = normalize_media_reference_source(source);
    if normalized.len() < 8 || !normalized[..8].eq_ignore_ascii_case("media://") {
        return Ok(None);
    }

    let parsed = Url::parse(&normalized).map_err(|e| {
        MediaReferenceError::InvalidPath(format!("Invalid media URI: {} ({})", normalized, e))
    })?;

    let host = parsed.host_str().unwrap_or("");
    if host != "inbound" {
        return Err(MediaReferenceError::PathNotAllowed(format!(
            "Unsupported media URI location: {}",
            if host.is_empty() { "(missing)" } else { host }
        )));
    }

    // Check raw path after host to prevent URL parser normalization from masking traversal
    let after_scheme = &normalized[8..];
    if let Some((_h, rest)) = after_scheme.split_once('/') {
        if rest.starts_with("../")
            || rest.contains("/../")
            || rest.ends_with("/..")
            || rest == ".."
            || rest == "."
            || rest.contains('\\')
        {
            return Err(MediaReferenceError::InvalidPath(format!(
                "Invalid media URI: {}",
                normalized
            )));
        }
    }

    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(MediaReferenceError::InvalidPath(format!(
            "Invalid media URI: {}",
            normalized
        )));
    }

    let raw_path = parsed.path().trim_start_matches('/');
    let decoded_id = percent_encoding::percent_decode_str(raw_path)
        .decode_utf8()
        .map_err(|e| {
            MediaReferenceError::InvalidPath(format!("Invalid media URI: {} ({})", normalized, e))
        })?
        .to_string();

    let invalid_id = decoded_id.is_empty()
        || decoded_id == "."
        || decoded_id == ".."
        || decoded_id.contains('/')
        || decoded_id.contains('\\')
        || decoded_id.contains('\0');

    if invalid_id {
        return Err(MediaReferenceError::InvalidPath(format!(
            "Invalid media URI: {}",
            normalized
        )));
    }

    Ok(Some(InboundMediaUri {
        id: decoded_id,
        normalized_source: normalized,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaReferenceSourceInfo {
    pub has_unsupported_scheme: bool,
    pub is_data_url: bool,
    pub is_file_url: bool,
    pub is_http_url: bool,
    pub is_media_store_url: bool,
    pub looks_like_windows_drive_path: bool,
}

/// Classifies media reference schemes before local resolution or sandbox rewriting.
pub fn classify_media_reference_source(
    source: &str,
    allow_data_url: bool,
) -> MediaReferenceSourceInfo {
    let looks_like_windows_drive_path = WIN_DRIVE_RE.is_match(source);
    let has_scheme = SCHEME_RE.is_match(source);
    let is_file_url = source.len() >= 5 && source[..5].eq_ignore_ascii_case("file:");
    let is_http_url = (source.len() >= 7 && source[..7].eq_ignore_ascii_case("http://"))
        || (source.len() >= 8 && source[..8].eq_ignore_ascii_case("https://"));
    let is_data_url = source.len() >= 5 && source[..5].eq_ignore_ascii_case("data:");
    let is_media_store_url = source.len() >= 8 && source[..8].eq_ignore_ascii_case("media://");

    let has_unsupported_scheme = has_scheme
        && !looks_like_windows_drive_path
        && !is_file_url
        && !is_http_url
        && !is_media_store_url
        && !(allow_data_url && is_data_url);

    MediaReferenceSourceInfo {
        has_unsupported_scheme,
        is_data_url,
        is_file_url,
        is_http_url,
        is_media_store_url,
        looks_like_windows_drive_path,
    }
}

/// Converts a managed inbound path to a URI without exposing paths outside its store.
pub fn build_inbound_media_uri_from_path(source: &Path, media_dir: &Path) -> Option<String> {
    let inbound_dir = media_dir.join("inbound");
    let norm_source = if source.is_absolute() {
        source.to_path_buf()
    } else {
        return None;
    };

    let rel = match norm_source.strip_prefix(&inbound_dir) {
        Ok(p) => p,
        Err(_) => return None,
    };

    let mut components = rel.components();
    let first = components.next()?;
    if components.next().is_some() {
        // Multi-segment subdirectories are rejected
        return None;
    }

    let segment_name = match first {
        Component::Normal(s) => s.to_str()?,
        _ => return None,
    };

    if segment_name.is_empty()
        || segment_name == "."
        || segment_name == ".."
        || segment_name.contains('/')
        || segment_name.contains('\\')
    {
        return None;
    }

    let candidate = format!("media://inbound/{}", segment_name);
    parse_inbound_media_uri(&candidate).ok()?.map(|u| u.normalized_source)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSandboxPathRewrite {
    pub resolved: String,
    pub rewritten_from: Option<String>,
}

/// Rewrites inbound media-store URIs to sandbox-relative paths for staged agent inputs.
pub fn resolve_media_reference_sandbox_path(
    source: &str,
    inbound_dir: Option<&str>,
) -> MediaSandboxPathRewrite {
    let normalized = normalize_media_reference_source(source);
    match parse_inbound_media_uri(&normalized) {
        Ok(Some(uri)) => {
            let base = inbound_dir.unwrap_or("media/inbound").replace('\\', "/");
            let clean_base = base.trim_end_matches('/');
            MediaSandboxPathRewrite {
                resolved: format!("{}/{}", clean_base, uri.id),
                rewritten_from: Some(uri.normalized_source),
            }
        }
        _ => MediaSandboxPathRewrite {
            resolved: normalized,
            rewritten_from: None,
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMediaReference {
    pub id: String,
    pub physical_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaReferenceLocalPathInfo {
    Inbound { id: String, path: PathBuf },
    Local { path: PathBuf },
}

/// Resolves inbound media:// URIs or first-level inbound file paths to concrete store files.
pub fn resolve_inbound_media_reference(
    source: &str,
    media_dir: &Path,
) -> Result<Option<InboundMediaReference>, MediaReferenceError> {
    let normalized = normalize_media_reference_source(source);
    if normalized.is_empty() {
        return Ok(None);
    }

    if let Some(uri) = parse_inbound_media_uri(&normalized)? {
        let physical = media_dir.join("inbound").join(&uri.id);
        return Ok(Some(InboundMediaReference {
            id: uri.id,
            physical_path: physical,
        }));
    }

    let path = Path::new(&normalized);
    if path.is_absolute() {
        let inbound_dir = media_dir.join("inbound");
        if let Ok(rel) = path.strip_prefix(&inbound_dir) {
            let mut components = rel.components();
            if let Some(Component::Normal(s)) = components.next() {
                if components.next().is_none() {
                    let id_str = s.to_string_lossy().into_owned();
                    return Ok(Some(InboundMediaReference {
                        id: id_str,
                        physical_path: path.to_path_buf(),
                    }));
                }
            }
        }
    }

    Ok(None)
}

/// Resolves a media reference while preserving whether it belongs to the inbound store.
pub fn resolve_media_reference_local_path_info(
    source: &str,
    media_dir: &Path,
) -> Result<MediaReferenceLocalPathInfo, MediaReferenceError> {
    if let Some(inbound) = resolve_inbound_media_reference(source, media_dir)? {
        Ok(MediaReferenceLocalPathInfo::Inbound {
            id: inbound.id,
            path: inbound.physical_path,
        })
    } else {
        let normalized = normalize_media_reference_source(source);
        Ok(MediaReferenceLocalPathInfo::Local {
            path: PathBuf::from(normalized),
        })
    }
}

/// Passes source limits only to owned readers; rejects oversized files before buffering.
pub fn read_outbound_media_file_bounded(
    file_path: &Path,
    max_bytes: usize,
) -> std::io::Result<Vec<u8>> {
    let meta = fs::metadata(file_path)?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "target path is not a file",
        ));
    }
    if meta.len() > max_bytes as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "file size ({} bytes) exceeds maximum limit ({} bytes)",
                meta.len(),
                max_bytes
            ),
        ));
    }

    let mut file = fs::File::open(file_path)?;
    let mut buffer = Vec::with_capacity(meta.len() as usize);
    file.by_ref().take((max_bytes + 1) as u64).read_to_end(&mut buffer)?;
    if buffer.len() > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file grew beyond maximum limit during read",
        ));
    }

    Ok(buffer)
}
