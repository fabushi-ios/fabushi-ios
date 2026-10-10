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
        match self.conflict_rel_path.as_deref() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_canonical_conflict_fields_and_reference_messages() {
        let canonical =
            BoxStoreCanonicalWriteConflictError::new("agent/one", None, None, None);
        assert_eq!(
            canonical.to_string(),
            "canonical write for agent/one lost a concurrent-write race"
        );

        let preserved = BoxStoreCanonicalWriteConflictError::new(
            "agent/one",
            Some("conflicts/one.db".into()),
            Some("etag-1".into()),
            Some("baseline".into()),
        );
        assert_eq!(
            preserved.to_string(),
            "agent-store write for agent/one lost a concurrent-write race; content preserved at conflicts/one.db"
        );
        assert_eq!(preserved.base_etag.as_deref(), Some("etag-1"));
        assert_eq!(preserved.baseline_source.as_deref(), Some("baseline"));
    }
}
