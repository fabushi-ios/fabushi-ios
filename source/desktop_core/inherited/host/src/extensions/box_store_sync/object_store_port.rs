use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreCanonicalWriteConflictError {
    pub key: String,
    pub conflict_rel_path: Option<String>,
    pub base_etag: Option<String>,
    pub baseline_source: Option<String>,
}

impl BoxStoreCanonicalWriteConflictError {
    pub fn new(
        key: impl Into<String>,
        conflict_rel_path: Option<String>,
        base_etag: Option<String>,
        baseline_source: Option<String>,
    ) -> Self {
        Self {
            key: key.into(),
            conflict_rel_path,
            base_etag,
            baseline_source,
        }
    }
}

impl fmt::Display for BoxStoreCanonicalWriteConflictError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.conflict_rel_path {
            Some(path) => write!(
                formatter,
                "agent-store write for {} lost a concurrent-write race; content preserved at {}",
                self.key, path
            ),
            None => write!(
                formatter,
                "canonical write for {} lost a concurrent-write race",
                self.key
            ),
        }
    }
}

impl std::error::Error for BoxStoreCanonicalWriteConflictError {}


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxStoreConditionalWriteOutcome {
    Written,
    Conflict(BoxStoreCanonicalWriteConflictError),
}
