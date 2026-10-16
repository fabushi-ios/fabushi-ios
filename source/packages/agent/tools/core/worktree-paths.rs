pub fn maybe_redirect_worktries_path(original_path: &str) -> String {
    for root in ["/Users/", "/home/"] {
        let Some(rest) = original_path.strip_prefix(root) else {
            continue;
        };
        let Some((user, tail)) = rest.split_once('/') else {
            continue;
        };
        if user.is_empty() {
            continue;
        }
        const LEGACY: &str = ".cursor/worktries";
        if tail == LEGACY || tail.starts_with(".cursor/worktries/") {
            let suffix = &tail[LEGACY.len()..];
            return format!("{root}{user}/.cursor/worktrees{suffix}");
        }
    }
    original_path.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_only_anchored_cursor_worktries_paths() {
        assert_eq!(
            maybe_redirect_worktries_path("/Users/alice/.cursor/worktries/repo"),
            "/Users/alice/.cursor/worktrees/repo"
        );
        assert_eq!(
            maybe_redirect_worktries_path("/home/bob/.cursor/worktries"),
            "/home/bob/.cursor/worktrees"
        );
        assert_eq!(
            maybe_redirect_worktries_path("/tmp/Users/alice/.cursor/worktries/repo"),
            "/tmp/Users/alice/.cursor/worktries/repo"
        );
        assert_eq!(
            maybe_redirect_worktries_path("/Users/alice/.cursor/worktries-old/repo"),
            "/Users/alice/.cursor/worktries-old/repo"
        );
    }
}
