#[derive(Debug, Clone, Copy)]
pub struct GithubPrInvocationContext<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub comments: &'a str,
    pub ci_failures: Option<&'a str>,
}

pub fn render_github_pr_invocation_context(pr: GithubPrInvocationContext<'_>) -> String {
    let mut rendered = format!(
        "<github_pr_context>\nHere is the context of the Pull Request you are working on:\nPR Title: {}",
        pr.title
    );

    if !pr.description.is_empty() {
        rendered.push_str("\nPR Description:\n");
        rendered.push_str(pr.description);
    }
    if !pr.comments.is_empty() {
        rendered.push_str("\nRecent PR Comments/Reviews:\n");
        rendered.push_str(pr.comments);
    }
    if let Some(ci_failures) = pr.ci_failures.filter(|value| !value.is_empty()) {
        rendered.push_str("\nPossibly Relevant CI Failures:\n");
        rendered.push_str(ci_failures);
    }

    rendered.push_str("\n</github_pr_context>");
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_only_non_empty_optional_pr_sections() {
        assert_eq!(
            render_github_pr_invocation_context(GithubPrInvocationContext {
                title: "Fix race",
                description: "",
                comments: "review comment",
                ci_failures: Some("unit failed"),
            }),
            "<github_pr_context>\nHere is the context of the Pull Request you are working on:\nPR Title: Fix race\nRecent PR Comments/Reviews:\nreview comment\nPossibly Relevant CI Failures:\nunit failed\n</github_pr_context>"
        );

        assert_eq!(
            render_github_pr_invocation_context(GithubPrInvocationContext {
                title: "Only title",
                description: "",
                comments: "",
                ci_failures: Some(""),
            }),
            "<github_pr_context>\nHere is the context of the Pull Request you are working on:\nPR Title: Only title\n</github_pr_context>"
        );
    }
}
