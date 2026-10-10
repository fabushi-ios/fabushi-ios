use std::path::{Path, PathBuf};

pub const CONVERSATION_BLOBS_FILENAME: &str = "conversation-blobs.db";

pub fn conversation_blobs_path(db_path: impl AsRef<Path>) -> PathBuf {
    db_path
        .as_ref()
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(CONVERSATION_BLOBS_FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_blob_database_next_to_session_store() {
        assert_eq!(
            conversation_blobs_path("/data/agents/a/store.db"),
            PathBuf::from("/data/agents/a/conversation-blobs.db")
        );
    }
}
