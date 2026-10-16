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


/// A platform-neutral representation of a Cursor rule used by the iOS-owned
/// agent runtime. The source ordering is significant for non-team rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorRule {
    pub source: CursorRuleSource,
    pub full_path: String,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorRuleSource {
    User,
    Team,
    Other,
}

/// Merge request-local and server-fetched rules using the Desktop-main policy.
pub fn get_all_rules(
    request_rules: &[CursorRule],
    server_rules: &[CursorRule],
    drop_custom_prompt_context: bool,
    disabled_team_rule_names: &[String],
) -> Vec<CursorRule> {
    if drop_custom_prompt_context {
        return Vec::new();
    }

    let disabled = disabled_team_rule_names.iter().collect::<BTreeSet<_>>();
    let mut result = Vec::new();
    let mut seen_team_rules = BTreeSet::new();

    for rule in request_rules.iter().chain(server_rules.iter()) {
        if rule.source == CursorRuleSource::Team {
            let name = rule
                .full_path
                .rsplit('/')
                .next()
                .unwrap_or(rule.full_path.as_str())
                .to_owned();
            if !rule.required && disabled.contains(&name) {
                continue;
            }
            if !seen_team_rules.insert(name) {
                continue;
            }
        }
        result.push(rule.clone());
    }
    result
}

/// Return the first user message's safe text when it contains the explicit
/// user-info marker. A non-user leading message never qualifies.
pub fn first_user_info_message<'a>(
    messages: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<String> {
    let (role, content) = messages.into_iter().next()?;
    if role != "user" || !content.contains("<user_info>") {
        return None;
    }
    Some(content.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerCloudTestingPlacement {
    UserInfo,
    Other,
}

/// Resolve Composer cloud-testing placement with an explicit metadata-first
/// policy and deterministic user-info fallback.
pub fn composer_cloud_testing_placement(
    first_message_role: &str,
    first_message_has_user_info: bool,
    metadata: Option<ComposerCloudTestingPlacement>,
) -> Option<ComposerCloudTestingPlacement> {
    if first_message_role != "user" {
        return None;
    }
    metadata.or_else(|| {
        first_message_has_user_info.then_some(ComposerCloudTestingPlacement::UserInfo)
    })
}

/// Extract the first complete automation-trigger XML block from user messages.
/// Open-only/malformed blocks are ignored so a later valid user message can
/// still produce the trigger context.
pub fn extract_automation_trigger_context(
    messages: &[(String, String)],
) -> Option<String> {
    const OPEN: &str = "<automation_trigger_info>";
    const CLOSE: &str = "</automation_trigger_info>";

    for (role, content) in messages {
        if role != "user" {
            continue;
        }
        let Some(open) = content.find(OPEN) else {
            continue;
        };
        let Some(relative_close) = content[open..].find(CLOSE) else {
            continue;
        };
        let close = open + relative_close + CLOSE.len();
        return Some(content[open..close].trim().to_owned());
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


#[cfg(test)]
mod common_semantics_tests {
    use super::*;

    #[test]
    fn rule_merge_preserves_non_team_rules_and_deduplicates_team_rules() {
        let request = vec![
            CursorRule { source: CursorRuleSource::User, full_path: "user/a".into(), required: false },
            CursorRule { source: CursorRuleSource::Team, full_path: "team/required.md".into(), required: true },
            CursorRule { source: CursorRuleSource::Team, full_path: "team/disabled.md".into(), required: false },
        ];
        let server = vec![
            CursorRule { source: CursorRuleSource::Team, full_path: "other/required.md".into(), required: true },
            CursorRule { source: CursorRuleSource::Team, full_path: "server/disabled.md".into(), required: false },
            CursorRule { source: CursorRuleSource::Other, full_path: "other/a".into(), required: false },
        ];
        let out = get_all_rules(&request, &server, false, &["disabled.md".into()]);
        assert_eq!(
            out,
            vec![
                request[0].clone(),
                request[1].clone(),
                CursorRule { source: CursorRuleSource::Other, full_path: "other/a".into(), required: false },
            ]
        );
    }

    #[test]
    fn dropping_custom_prompt_context_is_fail_closed() {
        let rules = vec![CursorRule { source: CursorRuleSource::User, full_path: "user/a".into(), required: false }];
        assert!(get_all_rules(&rules, &rules, true, &[]).is_empty());
    }

    #[test]
    fn first_user_info_requires_user_as_the_first_message() {
        assert_eq!(
            first_user_info_message([
                ("assistant", "<user_info>secret</user_info>"),
                ("user", "<user_info>later</user_info>"),
            ]),
            None
        );
        assert_eq!(
            first_user_info_message([("user", "<user_info>first</user_info>")]),
            Some("<user_info>first</user_info>".into())
        );
    }

    #[test]
    fn composer_metadata_has_deterministic_fallback() {
        assert_eq!(
            composer_cloud_testing_placement("user", true, None),
            Some(ComposerCloudTestingPlacement::UserInfo)
        );
        assert_eq!(
            composer_cloud_testing_placement(
                "user",
                true,
                Some(ComposerCloudTestingPlacement::Other),
            ),
            Some(ComposerCloudTestingPlacement::Other)
        );
        assert_eq!(composer_cloud_testing_placement("assistant", true, None), None);
    }

    #[test]
    fn automation_trigger_scans_user_messages_and_requires_a_closing_tag() {
        let messages = vec![
            ("assistant".into(), "<automation_trigger_info>ignored</automation_trigger_info>".into()),
            ("user".into(), "<automation_trigger_info>open-only".into()),
            ("user".into(), "prefix <automation_trigger_info>valid</automation_trigger_info> suffix".into()),
        ];
        assert_eq!(
            extract_automation_trigger_context(&messages),
            Some("<automation_trigger_info>valid</automation_trigger_info>".into())
        );
    }

    #[test]
    fn automation_trigger_returns_first_complete_block_when_multiple_exist() {
        let messages = vec![
            (
                "user".into(),
                "<automation_trigger_info>first</automation_trigger_info><automation_trigger_info>second</automation_trigger_info>".into(),
            ),
        ];
        assert_eq!(
            extract_automation_trigger_context(&messages),
            Some("<automation_trigger_info>first</automation_trigger_info>".into())
        );
    }
}
