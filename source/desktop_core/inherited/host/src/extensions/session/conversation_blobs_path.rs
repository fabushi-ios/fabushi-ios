use std::path::{Path, PathBuf};
use super::session_paths::CONVERSATION_BLOBS_FILENAME;

pub fn conversation_blobs_path(db_path: impl AsRef<Path>) -> PathBuf {
    db_path.as_ref().parent().unwrap_or_else(|| Path::new("")).join(CONVERSATION_BLOBS_FILENAME)
}
