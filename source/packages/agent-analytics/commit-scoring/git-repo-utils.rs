use url::Url;

fn parse_git_url(url_string: &str) -> Option<Url> {
    let mut normalized = url_string.to_owned();

    if url_string.contains('@') && !url_string.starts_with("http") {
        if !url_string.starts_with("ssh://") {
            if let Some(at) = url_string.find('@') {
                let user = &url_string[..at];
                let rest = &url_string[at + 1..];
                if !user.is_empty() {
                    if let Some(colon) = rest.find(':') {
                        let host = &rest[..colon];
                        let path = &rest[colon + 1..];
                        if !host.is_empty() {
                            normalized = format!("ssh://{user}@{host}/{path}");
                        }
                    }
                }
            }
        }
    }

    Url::parse(&normalized).ok()
}

pub fn extract_repo_name_from_git_url(upstream_url: Option<&str>) -> Option<String> {
    let upstream = upstream_url.filter(|value| !value.is_empty())?;
    let normalized = upstream
        .replacen("https///", "https://", 1)
        .replacen("http///", "http://", 1);

    if normalized.starts_with("gitlab-remote://") {
        if let Ok(url) = Url::parse(&normalized) {
            if let Some((_, project_id)) =
                url.query_pairs().find(|(key, _)| key == "project")
            {
                let project_id = project_id.into_owned();
                if project_id.contains('/') {
                    return Some(project_id);
                }
                return None;
            }
        }
    }

    let url = parse_git_url(&normalized)?;
    let hostname = url.host_str()?.to_ascii_lowercase();
    let mut path_parts = url
        .path()
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    if let Some(last) = path_parts.last_mut() {
        if last.ends_with(".git") {
            last.truncate(last.len().saturating_sub(4));
        }
    }

    let is_azure = hostname == "dev.azure.com"
        || hostname == "ssh.dev.azure.com"
        || hostname.ends_with(".visualstudio.com");
    if is_azure {
        let mut azure = path_parts.clone();
        if hostname == "ssh.dev.azure.com"
            && azure
                .first()
                .is_some_and(|part| part.eq_ignore_ascii_case("v3"))
        {
            azure.remove(0);
        }
        azure.retain(|part| !part.eq_ignore_ascii_case("_git"));
        return (!azure.is_empty()).then(|| azure.join("/"));
    }

    if hostname.contains("github") || hostname.ends_with(".ghe.com") {
        return (path_parts.len() >= 2).then(|| path_parts[..2].join("/"));
    }

    if hostname.contains("gitlab") {
        return (!path_parts.is_empty()).then(|| path_parts.join("/"));
    }

    if hostname.contains("bitbucket") || hostname.contains("stash") {
        if path_parts.len() >= 3 && path_parts[0].eq_ignore_ascii_case("scm") {
            return Some(path_parts[1..3].join("/"));
        }
        return (path_parts.len() >= 2).then(|| path_parts[..2].join("/"));
    }

    let is_gerrit_host = hostname.contains("gerrit");
    let has_gerrit_path = path_parts
        .first()
        .is_some_and(|part| part.eq_ignore_ascii_case("gerrit"));
    if is_gerrit_host || has_gerrit_path {
        let mut gerrit = path_parts.clone();
        if has_gerrit_path {
            gerrit.remove(0);
        }
        if gerrit
            .first()
            .is_some_and(|part| part.eq_ignore_ascii_case("a"))
        {
            gerrit.remove(0);
        }
        return (!gerrit.is_empty()).then(|| gerrit.join("/"));
    }

    (path_parts.len() >= 2).then(|| path_parts[..2].join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_https_and_scp_style_remotes() {
        assert_eq!(
            extract_repo_name_from_git_url(Some("https://github.com/OpenAI/codex.git")),
            Some("OpenAI/codex".to_owned())
        );
        assert_eq!(
            extract_repo_name_from_git_url(Some("git@github.com:bhrumom/fabushi-ios.git")),
            Some("bhrumom/fabushi-ios".to_owned())
        );
        assert_eq!(
            extract_repo_name_from_git_url(Some("https///github.com/a/b.git")),
            Some("a/b".to_owned())
        );
    }

    #[test]
    fn preserves_host_specific_repository_shapes() {
        assert_eq!(
            extract_repo_name_from_git_url(Some(
                "ssh://git@ssh.dev.azure.com/v3/org/project/repo"
            )),
            Some("org/project/repo".to_owned())
        );
        assert_eq!(
            extract_repo_name_from_git_url(Some(
                "https://dev.azure.com/org/project/_git/repo.git"
            )),
            Some("org/project/repo".to_owned())
        );
        assert_eq!(
            extract_repo_name_from_git_url(Some(
                "https://gitlab.com/group/subgroup/repo.git"
            )),
            Some("group/subgroup/repo".to_owned())
        );
        assert_eq!(
            extract_repo_name_from_git_url(Some(
                "https://stash.example.com/scm/team/repo.git"
            )),
            Some("team/repo".to_owned())
        );
        assert_eq!(
            extract_repo_name_from_git_url(Some(
                "https://gerrit.example.com/a/platform/project.git"
            )),
            Some("platform/project".to_owned())
        );
    }

    #[test]
    fn parses_gitlab_remote_project_and_rejects_unusable_urls() {
        assert_eq!(
            extract_repo_name_from_git_url(Some(
                "gitlab-remote://ignored?project=group%2Frepo"
            )),
            Some("group/repo".to_owned())
        );
        assert_eq!(
            extract_repo_name_from_git_url(Some(
                "gitlab-remote://ignored?project=123"
            )),
            None
        );
        assert_eq!(extract_repo_name_from_git_url(None), None);
        assert_eq!(extract_repo_name_from_git_url(Some("not a url")), None);
    }
}
