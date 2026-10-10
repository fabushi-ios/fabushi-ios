#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskToolModelInfo<'a> {
    pub is_composer1: bool,
    pub is_composer15: bool,
    pub prompt_version: &'a str,
}

fn is_codex_prompt_version(version: &str) -> bool {
    matches!(version, "gpt5-codex" | "codex-cloud")
}

pub fn get_task_tool_name(parent_model_info: &TaskToolModelInfo<'_>) -> &'static str {
    if parent_model_info.is_composer1 || parent_model_info.is_composer15 {
        return "mcp_task";
    }
    if is_codex_prompt_version(parent_model_info.prompt_version) {
        return "Subagent";
    }
    "Task"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_models_take_mcp_task_name() {
        for (is_composer1, is_composer15) in [(true, false), (false, true)] {
            assert_eq!(
                get_task_tool_name(&TaskToolModelInfo {
                    is_composer1,
                    is_composer15,
                    prompt_version: "gpt5-codex",
                }),
                "mcp_task"
            );
        }
    }

    #[test]
    fn codex_prompt_versions_take_subagent_name() {
        for version in ["gpt5-codex", "codex-cloud"] {
            assert_eq!(
                get_task_tool_name(&TaskToolModelInfo {
                    is_composer1: false,
                    is_composer15: false,
                    prompt_version: version,
                }),
                "Subagent"
            );
        }
    }

    #[test]
    fn ordinary_models_keep_task_name() {
        assert_eq!(
            get_task_tool_name(&TaskToolModelInfo {
                is_composer1: false,
                is_composer15: false,
                prompt_version: "gpt-5",
            }),
            "Task"
        );
    }
}
