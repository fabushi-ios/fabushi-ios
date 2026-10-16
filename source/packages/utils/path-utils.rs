use std::io;
use std::path::{Component, Path, PathBuf};
use url::Url;

pub const WORKTREE_GUARD_ERROR: &str =
    "You cannot search other worktrees for this repository, stay within your workspace paths.";

fn untildify(value: &str) -> String {
    let expands = value == "~"
        || value.starts_with("~/")
        || value.starts_with("~\\");
    if !expands {
        return value.to_owned();
    }
    let Some(home) = std::env::var_os("HOME") else {
        return value.to_owned();
    };
    let suffix = value.strip_prefix('~').unwrap_or_default();
    let mut result = PathBuf::from(home);
    let trimmed = suffix.trim_start_matches(['/', '\\']);
    if !trimmed.is_empty() {
        result.push(trimmed);
    }
    result.to_string_lossy().into_owned()
}

pub fn file_path_from_file_url(value: &str) -> Option<PathBuf> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

pub fn posix_path_from_file_url(value: &str) -> Option<String> {
    file_path_from_file_url(value)
        .map(|path| normalize_to_unix_path(&path.to_string_lossy()))
}

fn strip_file_url_if_present(value: &str) -> String {
    if !value.starts_with("file://") {
        return value.to_owned();
    }
    file_path_from_file_url(value)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| value.to_owned())
}

pub(crate) fn normalize_lexically(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() && !path.is_absolute() {
                    result.push("..");
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn make_absolute(path: PathBuf, base_path: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        return normalize_lexically(&path);
    }
    let base = base_path
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"));
    normalize_lexically(&base.join(path))
}

pub fn resolve_path(value: &str, base_path: Option<&Path>) -> PathBuf {
    let stripped = strip_file_url_if_present(value);
    let untildified = untildify(&stripped);
    make_absolute(PathBuf::from(untildified), base_path)
}

pub(crate) fn canonicalize_nearest_existing(path: &Path) -> io::Result<PathBuf> {
    let normalized = normalize_lexically(path);
    let mut current = normalized.clone();
    let mut tail = Vec::new();

    loop {
        match current.canonicalize() {
            Ok(mut resolved) => {
                for component in tail.into_iter().rev() {
                    resolved.push(component);
                }
                return Ok(normalize_lexically(&resolved));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(name) = current.file_name().map(|value| value.to_os_string()) else {
                    return Ok(normalized);
                };
                tail.push(name);
                if !current.pop() {
                    return Ok(normalized);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

pub fn resolve_real_path_strict(value: &str, base_path: Option<&Path>) -> Option<PathBuf> {
    let normalized = resolve_path(value, base_path);
    canonicalize_nearest_existing(&normalized).ok()
}

pub(crate) fn is_path_within_path(base_path: &Path, target_path: &Path) -> bool {
    let base = normalize_lexically(base_path);
    let target = normalize_lexically(target_path);
    target == base || target.starts_with(&base)
}

pub fn is_path_within(base_path: &Path, target_path: &Path) -> bool {
    let base = make_absolute(base_path.to_path_buf(), None);
    let target = if target_path.is_absolute() {
        make_absolute(target_path.to_path_buf(), None)
    } else {
        make_absolute(target_path.to_path_buf(), Some(&base))
    };
    is_path_within_path(&base, &target)
}

pub fn is_path_strictly_within(base_path: &Path, target_path: &Path) -> bool {
    let base = make_absolute(base_path.to_path_buf(), None);
    let target = if target_path.is_absolute() {
        make_absolute(target_path.to_path_buf(), None)
    } else {
        make_absolute(target_path.to_path_buf(), Some(&base))
    };
    target != base && is_path_within_path(&base, &target)
}

pub fn contain_within_roots(
    roots: &[PathBuf],
    candidate_path: &Path,
) -> io::Result<Option<PathBuf>> {
    if roots.is_empty() || !candidate_path.is_absolute() {
        return Ok(None);
    }

    let resolved = normalize_lexically(candidate_path);
    if !roots
        .iter()
        .any(|root| is_path_strictly_within(root, &resolved))
    {
        return Ok(None);
    }

    let real_resolved = canonicalize_nearest_existing(&resolved)?;
    for root in roots {
        let real_root = canonicalize_nearest_existing(root)?;
        if is_path_strictly_within(&real_root, &real_resolved) {
            return Ok(Some(resolved));
        }
    }
    Ok(None)
}

pub fn normalize_to_unix_path(value: &str) -> String {
    value.replace('\\', "/")
}

pub fn is_worktrees_path(value: &Path) -> bool {
    normalize_to_unix_path(&value.to_string_lossy()).contains(".cursor/worktrees")
}

fn get_worktrees_repo_root(worktree_path: &Path) -> Option<PathBuf> {
    let normalized = normalize_to_unix_path(&worktree_path.to_string_lossy());
    let marker = "/.cursor/worktrees/";
    let marker_index = normalized.find(marker)?;
    let after_marker = &normalized[marker_index + marker.len()..];
    let repo_name = after_marker.split('/').next().unwrap_or_default();
    if repo_name.is_empty() {
        return None;
    }
    Some(PathBuf::from(
        &normalized[..marker_index + marker.len() + repo_name.len()],
    ))
}

fn is_path_within_any_root(target_path: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .any(|root| is_path_within(root.as_path(), target_path))
}

pub fn should_block_worktree_path(
    target_path: Option<&Path>,
    workspace_paths: &[PathBuf],
    main_worktree_path: Option<&Path>,
) -> bool {
    let Some(target_path) = target_path else {
        return false;
    };
    if workspace_paths.is_empty()
        || !workspace_paths.iter().all(|path| is_worktrees_path(path))
    {
        return false;
    }
    if is_path_within_any_root(target_path, workspace_paths) {
        return false;
    }

    let resolved_targets: Vec<PathBuf> = if target_path.is_absolute() {
        vec![normalize_lexically(target_path)]
    } else {
        workspace_paths
            .iter()
            .map(|workspace| normalize_lexically(&workspace.join(target_path)))
            .collect()
    };
    let repo_roots: Vec<PathBuf> = workspace_paths
        .iter()
        .filter_map(|root| get_worktrees_repo_root(root))
        .collect();

    if !repo_roots.is_empty()
        && resolved_targets
            .iter()
            .any(|target| is_path_within_any_root(target, &repo_roots))
    {
        return true;
    }
    if let Some(main) = main_worktree_path {
        let roots = vec![main.to_path_buf()];
        if resolved_targets
            .iter()
            .any(|target| is_path_within_any_root(target, &roots))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_tilde_and_file_urls() {
        let base = Path::new("/tmp/fabushi-base");
        assert_eq!(
            resolve_path("../child", Some(base)),
            PathBuf::from("/tmp/child")
        );
        assert_eq!(
            resolve_path("file:///tmp/a%20b", None),
            PathBuf::from("/tmp/a b")
        );
        let tilde = resolve_path("~/fabushi", None);
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(tilde, PathBuf::from(home).join("fabushi"));
        }
    }

    #[test]
    fn file_url_helpers_reject_non_file_urls_and_decode_file_paths() {
        assert_eq!(
            file_path_from_file_url("file:///tmp/a%20b"),
            Some(PathBuf::from("/tmp/a b"))
        );
        assert_eq!(
            posix_path_from_file_url("file:///tmp/a%20b"),
            Some("/tmp/a b".to_string())
        );
        assert_eq!(file_path_from_file_url("https://example.com/a"), None);
        assert_eq!(file_path_from_file_url("not a url"), None);
    }

    #[test]
    fn path_within_is_component_aware() {
        assert!(is_path_within(
            Path::new("/tmp/work"),
            Path::new("/tmp/work/a")
        ));
        assert!(is_path_within(
            Path::new("/tmp/work"),
            Path::new(".")
        ));
        assert!(!is_path_within(
            Path::new("/tmp/work"),
            Path::new("/tmp/work-other")
        ));

        assert!(is_path_strictly_within(
            Path::new("/tmp/work"),
            Path::new("/tmp/work/a")
        ));
        assert!(!is_path_strictly_within(
            Path::new("/tmp/work"),
            Path::new("/tmp/work")
        ));
    }

    #[test]
    fn contain_within_roots_rejects_relative_root_and_sibling_paths() {
        let roots = vec![PathBuf::from("/tmp/work")];
        assert_eq!(
            contain_within_roots(&roots, Path::new("relative/file")).unwrap(),
            None
        );
        assert_eq!(
            contain_within_roots(&roots, Path::new("/tmp/work")).unwrap(),
            None
        );
        assert_eq!(
            contain_within_roots(&roots, Path::new("/tmp/work-other/file")).unwrap(),
            None
        );
    }

    #[test]
    fn recognizes_and_blocks_sibling_worktrees() {
        let workspace = PathBuf::from("/Users/alice/.cursor/worktrees/repo/current");
        let workspaces = vec![workspace.clone()];
        assert!(is_worktrees_path(&workspace));
        assert!(!should_block_worktree_path(
            Some(Path::new("src/main.rs")),
            &workspaces,
            None,
        ));
        assert!(should_block_worktree_path(
            Some(Path::new("/Users/alice/.cursor/worktrees/repo/other/src/main.rs")),
            &workspaces,
            None,
        ));
        assert_eq!(
            WORKTREE_GUARD_ERROR,
            "You cannot search other worktrees for this repository, stay within your workspace paths."
        );
    }

    #[test]
    fn worktree_guard_is_disabled_for_normal_workspace_roots() {
        let workspaces = vec![PathBuf::from("/Users/alice/project")];
        assert!(!should_block_worktree_path(
            Some(Path::new("/Users/alice/other")),
            &workspaces,
            Some(Path::new("/Users/alice/project")),
        ));
    }

    #[cfg(unix)]
    #[test]
    fn strict_realpath_resolves_nearest_existing_symlink_parent() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "fabushi-path-utils-{}",
            std::process::id()
        ));
        let real = root.join("real");
        let link = root.join("link");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&real).unwrap();
        symlink(&real, &link).unwrap();

        let resolved = resolve_real_path_strict(
            &link.join("missing/file.txt").to_string_lossy(),
            None,
        )
        .unwrap();
        let canonical_real = real.canonicalize().unwrap();
        assert_eq!(resolved, canonical_real.join("missing/file.txt"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn contain_within_roots_rejects_symlink_escape_for_missing_leaf() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "fabushi-containment-root-{}",
            std::process::id()
        ));
        let allowed = root.join("allowed");
        let outside = root.join("outside");
        let link = allowed.join("escape");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, &link).unwrap();

        let escaped = link.join("missing/file.txt");
        assert_eq!(
            contain_within_roots(std::slice::from_ref(&allowed), &escaped).unwrap(),
            None
        );

        let safe = allowed.join("nested/missing.txt");
        assert_eq!(
            contain_within_roots(std::slice::from_ref(&allowed), &safe).unwrap(),
            Some(normalize_lexically(&safe))
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
