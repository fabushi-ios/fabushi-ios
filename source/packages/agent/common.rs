use std::collections::BTreeSet;

const BROWSER_MCP_PROVIDER_IDS: [&str; 2] = ["cursor-dev-control", "cursor-ide-browser"];

pub fn is_equal<T: PartialEq>(left: &[T], right: &[T]) -> bool {
    left == right
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolDescriptor {
    pub provider_identifier: String,
    pub name: String,
}

pub fn get_browser_tool_names(tools: &[McpToolDescriptor]) -> Vec<String> {
    let providers = BROWSER_MCP_PROVIDER_IDS.into_iter().collect::<BTreeSet<_>>();
    tools
        .iter()
        .filter(|tool| providers.contains(tool.provider_identifier.as_str()))
        .map(|tool| tool.name.clone())
        .collect()
}

pub fn get_browser_mcp_provider_name(browser_tools: Option<&[String]>) -> Option<String> {
    let tools = browser_tools?;
    if tools.is_empty() {
        return None;
    }

    for tool in tools {
        if let Some(index) = tool.find("-browser_") {
            if index > 0 {
                return Some(tool[..index].to_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_is_order_and_length_sensitive() {
        assert!(is_equal(&[1, 2], &[1, 2]));
        assert!(!is_equal(&[1, 2], &[2, 1]));
        assert!(!is_equal(&[1], &[1, 2]));
    }

    #[test]
    fn browser_tool_filter_preserves_input_order() {
        let tools = vec![
            McpToolDescriptor {
                provider_identifier: "other".into(),
                name: "ignored".into(),
            },
            McpToolDescriptor {
                provider_identifier: "cursor-dev-control".into(),
                name: "alpha-browser_open".into(),
            },
            McpToolDescriptor {
                provider_identifier: "cursor-ide-browser".into(),
                name: "beta-browser_click".into(),
            },
        ];
        assert_eq!(
            get_browser_tool_names(&tools),
            vec!["alpha-browser_open".to_owned(), "beta-browser_click".to_owned()]
        );
    }

    #[test]
    fn extracts_first_browser_prefix_only_when_nonempty() {
        assert_eq!(
            get_browser_mcp_provider_name(Some(&[
                "plain-tool".to_owned(),
                "provider-browser_open".to_owned(),
            ])),
            Some("provider".to_owned())
        );
        assert_eq!(get_browser_mcp_provider_name(Some(&[])), None);
        assert_eq!(
            get_browser_mcp_provider_name(Some(&["-browser_open".to_owned()])),
            None
        );
        assert_eq!(get_browser_mcp_provider_name(None), None);
    }
}
