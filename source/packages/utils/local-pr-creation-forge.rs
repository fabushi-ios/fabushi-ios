pub const LOCAL_PR_CREATION_FORGE_RULE_PATH: &str = "cursor://internal/local-pr-creation-forge";
pub const LOCAL_PR_CREATION_FORGE_GUIDANCE_HEADER: &str = "Preferred pull request host:";
pub const LEGACY_LOCAL_PR_CREATION_FORGE_GUIDANCE_HEADER: &str = "Pull request forge (Creation Provider):";
pub const FORGE_CLI_GLOSSARY: &str = "Cursor can open new pull requests on either:\n- GitHub, with the `gh` CLI (`gh pr create`)\n- Cursor Origin (Cursor's own pull-request host — not the git remote named `origin`), with the `origin` CLI (`origin pr create`)\nPrefer `gh` or `origin` over `gt`. If you use `gt`, you MUST pass `--github` or `--origin` for the intended host.";

pub fn should_rerender_user_info_for_local_pr_creation_forge(
    forge_rule_content: Option<&str>,
    user_info_content: &str,
) -> bool {
    match forge_rule_content {
        None | Some("") => [
            LOCAL_PR_CREATION_FORGE_GUIDANCE_HEADER,
            LEGACY_LOCAL_PR_CREATION_FORGE_GUIDANCE_HEADER,
        ]
        .iter()
        .any(|header| user_info_content.contains(&format!("<user_rule>{header}"))),
        Some(content) => !user_info_content.contains(&format!("<user_rule>{content}</user_rule>")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_rerender_for_stale_or_missing_forge_rule_projection() {
        assert!(should_rerender_user_info_for_local_pr_creation_forge(
            None,
            "<user_rule>Preferred pull request host: GitHub</user_rule>"
        ));
        assert!(should_rerender_user_info_for_local_pr_creation_forge(
            Some("Preferred pull request host: GitHub"),
            "<user_rule>other</user_rule>"
        ));
        assert!(!should_rerender_user_info_for_local_pr_creation_forge(
            Some("Preferred pull request host: GitHub"),
            "<user_rule>Preferred pull request host: GitHub</user_rule>"
        ));
    }

    #[test]
    fn preserves_reference_rule_path_and_cli_guidance() {
        assert_eq!(LOCAL_PR_CREATION_FORGE_RULE_PATH, "cursor://internal/local-pr-creation-forge");
        assert!(FORGE_CLI_GLOSSARY.contains("gh pr create"));
        assert!(FORGE_CLI_GLOSSARY.contains("origin pr create"));
    }
}
