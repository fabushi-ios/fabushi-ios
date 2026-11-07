use std::fmt;

pub const MEDIA_FILE_MODE: u32 = 0o644;

pub fn format_media_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        let kib = bytes as f64 / 1024.0;
        if kib.fract() == 0.0 {
            format!("{:.0}KB", kib)
        } else {
            format!("{:.2}KB", kib)
        }
    } else {
        let mib = bytes as f64 / (1024.0 * 1024.0);
        if mib.fract() == 0.0 {
            format!("{:.0}MB", mib)
        } else {
            format!("{:.2}MB", mib)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMediaSourceErrorCode {
    InvalidPath,
    NotFound,
    NotFile,
    PathMismatch,
    TooLarge,
}

impl SaveMediaSourceErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidPath => "invalid-path",
            Self::NotFound => "not-found",
            Self::NotFile => "not-file",
            Self::PathMismatch => "path-mismatch",
            Self::TooLarge => "too-large",
        }
    }
}

impl fmt::Display for SaveMediaSourceErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveMediaSourceError {
    pub code: SaveMediaSourceErrorCode,
    pub message: String,
}

impl SaveMediaSourceError {
    pub fn new(code: SaveMediaSourceErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn too_large(max_bytes: u64) -> Self {
        let limit = format_media_size(max_bytes);
        Self::new(
            SaveMediaSourceErrorCode::TooLarge,
            format!("Media exceeds {limit} limit"),
        )
    }
}

impl fmt::Display for SaveMediaSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for SaveMediaSourceError {}
