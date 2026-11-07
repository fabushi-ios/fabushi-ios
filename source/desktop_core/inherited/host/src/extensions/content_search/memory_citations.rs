//! Pure memory-citation policy for the existing content-search owner.
//!
//! This module requires explicit source-line provenance. It never derives line
//! numbers from transcript snippets, entry ordering, timestamps, or byte
//! offsets.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryCitationsMode {
    On,
    Off,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySourceProvenance {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// Formats only validated, explicit source-line provenance.
pub fn format_citation(provenance: &MemorySourceProvenance) -> Option<String> {
    let path = provenance.path.trim();
    if path.is_empty()
        || provenance.start_line == 0
        || provenance.end_line == 0
        || provenance.end_line < provenance.start_line
    {
        return None;
    }
    let range = if provenance.start_line == provenance.end_line {
        format!("#L{}", provenance.start_line)
    } else {
        format!("#L{}-L{}", provenance.start_line, provenance.end_line)
    };
    Some(format!("{path}{range}"))
}

/// Resolves whether automatic citation presentation is allowed for a session.
/// Explicit `on` and `off` always win; `auto` suppresses channel/group keys.
pub fn should_include_citations(mode: MemoryCitationsMode, session_key: Option<&str>) -> bool {
    match mode {
        MemoryCitationsMode::On => true,
        MemoryCitationsMode::Off => false,
        MemoryCitationsMode::Auto => session_key
            .map(|key| {
                !key.to_ascii_lowercase()
                    .split(':')
                    .any(|token| token == "channel" || token == "group")
            })
            .unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(path: &str, start_line: u32, end_line: u32) -> MemorySourceProvenance {
        MemorySourceProvenance {
            path: path.into(),
            start_line,
            end_line,
        }
    }

    #[test]
    fn formats_single_lines_and_ranges() {
        assert_eq!(
            format_citation(&source("MEMORY.md", 7, 7)).as_deref(),
            Some("MEMORY.md#L7")
        );
        assert_eq!(
            format_citation(&source("notes/today.md", 7, 12)).as_deref(),
            Some("notes/today.md#L7-L12")
        );
    }

    #[test]
    fn invalid_provenance_fails_closed() {
        assert!(format_citation(&source("", 1, 1)).is_none());
        assert!(format_citation(&source("MEMORY.md", 0, 1)).is_none());
        assert!(format_citation(&source("MEMORY.md", 4, 3)).is_none());
    }

    #[test]
    fn auto_mode_suppresses_channel_and_group_sessions() {
        assert!(!should_include_citations(
            MemoryCitationsMode::Auto,
            Some("agent:main:channel:telegram")
        ));
        assert!(!should_include_citations(
            MemoryCitationsMode::Auto,
            Some("agent:main:group:team")
        ));
        assert!(should_include_citations(
            MemoryCitationsMode::Auto,
            Some("agent:main:direct")
        ));
        assert!(should_include_citations(MemoryCitationsMode::Auto, None));
    }

    #[test]
    fn explicit_mode_wins() {
        assert!(should_include_citations(
            MemoryCitationsMode::On,
            Some("agent:main:group:team")
        ));
        assert!(!should_include_citations(
            MemoryCitationsMode::Off,
            Some("agent:main:direct")
        ));
    }
}
