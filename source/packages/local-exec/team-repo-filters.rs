pub fn is_wildcard_repo_url(url: &str) -> bool {
    url.trim() == "*"
}

pub fn is_path_pattern_repo_url(url: &str) -> bool {
    !url.contains("://")
        && !url.contains("github.com")
        && !url.contains("gitlab.com")
        && !url.contains("bitbucket.org")
        && !url.contains('@')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_filter_semantics_match_grok_reference() {
        assert!(is_wildcard_repo_url("  *  "));
        assert!(is_path_pattern_repo_url("org/repo"));
        assert!(!is_path_pattern_repo_url("https://github.com/org/repo"));
        assert!(!is_path_pattern_repo_url("git@github.com:org/repo.git"));
    }
}
