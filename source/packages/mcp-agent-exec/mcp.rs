use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolNotFoundError {
    pub tool_name: String,
    pub available_tools: Vec<String>,
}

impl McpToolNotFoundError {
    pub fn new(tool_name: impl Into<String>, available_tools: Vec<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            available_tools,
        }
    }
}

impl Display for McpToolNotFoundError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Tool {} not found, available tools: {}",
            self.tool_name,
            self.available_tools.join(", ")
        )
    }
}

impl Error for McpToolNotFoundError {}

pub fn is_mcp_tool_not_found_error(error: &(dyn Error + 'static)) -> bool {
    error.downcast_ref::<McpToolNotFoundError>().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct OtherError;
    impl Display for OtherError {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("other")
        }
    }
    impl Error for OtherError {}

    #[test]
    fn preserves_message_and_payload() {
        let error = McpToolNotFoundError::new(
            "search",
            vec!["read".to_owned(), "write".to_owned()],
        );
        assert_eq!(error.tool_name, "search");
        assert_eq!(error.available_tools, vec!["read", "write"]);
        assert_eq!(
            error.to_string(),
            "Tool search not found, available tools: read, write"
        );
        assert!(is_mcp_tool_not_found_error(&error));
        assert!(!is_mcp_tool_not_found_error(&OtherError));
    }
}
