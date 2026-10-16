#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    Write,
    Shell,
    Delete,
    Mcp,
}

impl OperationType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Shell => "shell",
            Self::Delete => "delete",
            Self::Mcp => "mcp",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_type_wire_values_match_grok() {
        assert_eq!(OperationType::Write.as_str(), "write");
        assert_eq!(OperationType::Shell.as_str(), "shell");
        assert_eq!(OperationType::Delete.as_str(), "delete");
        assert_eq!(OperationType::Mcp.as_str(), "mcp");
    }
}
