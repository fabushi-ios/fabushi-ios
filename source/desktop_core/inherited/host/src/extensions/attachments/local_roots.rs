//! Local media roots management and sandbox isolation filtering.
//!
//! Maps upstream OpenClaw module:
//! - `src/media/local-roots.ts`

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// Resolves a path, normalizing components and resolving current/parent dirs.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => output.push(prefix.as_os_str()),
            Component::RootDir => output.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            Component::Normal(val) => output.push(val),
        }
    }
    output
}

/// Resolves canonical root via nearest existing ancestor or normalized absolute path.
pub fn resolve_canonical_root(root: &Path) -> PathBuf {
    if let Ok(canon) = std::fs::canonicalize(root) {
        return canon;
    }
    // Walk ancestors to find nearest existing
    let mut current = root.to_path_buf();
    let mut missing = Vec::new();
    while !current.as_os_str().is_empty() {
        if let Ok(canon) = std::fs::canonicalize(&current) {
            let mut resolved = canon;
            for part in missing.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        if let Some(parent) = current.parent() {
            if let Some(file_name) = current.file_name() {
                missing.push(file_name.to_os_string());
            }
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
    normalize_path(root)
}

/// Checks if `child` is inside or equal to `parent`.
pub fn is_path_inside_or_equal(parent: &Path, child: &Path) -> bool {
    let p_norm = normalize_path(parent);
    let c_norm = normalize_path(child);
    c_norm.starts_with(&p_norm)
}

/// Builds process default local roots from state_dir and config_dir.
pub fn build_media_local_roots(state_dir: &Path, config_dir: &Path) -> Vec<PathBuf> {
    let resolved_state = resolve_canonical_root(state_dir);
    let resolved_config = resolve_canonical_root(config_dir);
    let tmp = resolve_canonical_root(&std::env::temp_dir());

    let candidates = vec![
        tmp,
        resolved_config.join("media"),
        resolved_state.join("media"),
        resolved_state.join("delivery-queue").join("media"),
        resolved_state.join("canvas"),
        resolved_state.join("workspace"),
        resolved_state.join("sandboxes"),
    ];

    let mut seen = HashSet::new();
    let mut roots = Vec::new();
    for c in candidates {
        let norm = normalize_path(&c);
        if seen.insert(norm.clone()) {
            roots.push(norm);
        }
    }
    roots
}

/// Returns process default local media roots.
pub fn get_default_media_local_roots(state_dir: &Path, config_dir: &Path) -> Vec<PathBuf> {
    build_media_local_roots(state_dir, config_dir)
}

fn overlaps(shared_dir: &Path, root: &Path) -> bool {
    is_path_inside_or_equal(shared_dir, root) || is_path_inside_or_equal(root, shared_dir)
}

/// Drops shared isolation parents from a media root list.
///
/// Roots overlapping the shared `<state>/sandboxes` parent are removed unless they live inside the
/// supplied session workspace. The shared `<state>/workspace` parent is removed only when the caller
/// supplies a session workspace outside it (a sandboxed session).
pub fn filter_shared_media_local_roots(
    roots: &[PathBuf],
    state_dir: &Path,
    session_workspace_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let resolved_state = resolve_canonical_root(state_dir);
    let sandboxes_dir = resolve_canonical_root(&resolved_state.join("sandboxes"));
    let workspace_dir = resolve_canonical_root(&resolved_state.join("workspace"));

    let session_ws = session_workspace_dir.map(resolve_canonical_root);
    // The shared sandboxes parent itself (or any ancestor of it) is never a valid session workspace
    let valid_session_ws = session_ws.as_ref().and_then(|sw| {
        if is_path_inside_or_equal(sw, &sandboxes_dir) {
            None
        } else {
            Some(sw)
        }
    });

    let mut filtered = Vec::new();
    for root in roots {
        let resolved_root = resolve_canonical_root(root);
        let within_session_ws = valid_session_ws
            .map(|sw| is_path_inside_or_equal(sw, &resolved_root))
            .unwrap_or(false);

        if overlaps(&sandboxes_dir, &resolved_root) && !within_session_ws {
            continue;
        }
        if session_ws.is_some()
            && overlaps(&workspace_dir, &resolved_root)
            && !within_session_ws
        {
            continue;
        }
        filtered.push(resolved_root);
    }
    filtered
}

/// Default local media roots with shared isolation parents removed.
pub fn get_session_safe_default_media_local_roots(
    state_dir: &Path,
    config_dir: &Path,
    session_workspace_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let defaults = get_default_media_local_roots(state_dir, config_dir);
    filter_shared_media_local_roots(&defaults, state_dir, session_workspace_dir)
}

/// Adds exact agent/session workspaces without exposing shared agent or sandbox roots.
pub fn get_agent_scoped_media_local_roots(
    state_dir: &Path,
    config_dir: &Path,
    agent_id: Option<&str>,
    agent_workspace_dir: Option<&Path>,
    session_workspace_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let defaults = build_media_local_roots(state_dir, config_dir);
    let mut roots = filter_shared_media_local_roots(&defaults, state_dir, session_workspace_dir);

    let default_agent_ws = agent_id.map(|id| state_dir.join(format!("workspace-{id}")));
    let effective_ws = session_workspace_dir
        .or(agent_workspace_dir)
        .or(default_agent_ws.as_deref());

    if let Some(ws) = effective_ws {
        let resolved_state = resolve_canonical_root(state_dir);
        let sandboxes_dir = resolve_canonical_root(&resolved_state.join("sandboxes"));
        let norm_ws = resolve_canonical_root(ws);

        if norm_ws != sandboxes_dir && !roots.contains(&norm_ws) {
            roots.push(norm_ws);
        }
    }
    roots
}

/// Adds only concrete local source parent directories to an existing root allowlist.
pub fn append_local_media_parent_roots(roots: &[PathBuf], media_sources: &[String]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for r in roots {
        let norm = resolve_canonical_root(r);
        if seen.insert(norm.clone()) {
            result.push(norm);
        }
    }

    for source in media_sources {
        let path = Path::new(source);
        if let Some(parent) = path.parent() {
            // Skip root directory
            if parent.as_os_str().is_empty() || parent == Path::new("/") {
                continue;
            }
            let norm_parent = resolve_canonical_root(parent);
            if seen.insert(norm_parent.clone()) {
                result.push(norm_parent);
            }
        }
    }
    result
}

/// Resolves outbound media roots, expanding for local sources only when filesystem policy allows it.
pub fn get_agent_scoped_media_local_roots_for_sources(
    state_dir: &Path,
    config_dir: &Path,
    agent_id: Option<&str>,
    agent_workspace_dir: Option<&Path>,
    session_workspace_dir: Option<&Path>,
    media_sources: &[String],
    tool_fs_expansion_allowed: bool,
) -> Vec<PathBuf> {
    let roots = get_agent_scoped_media_local_roots(
        state_dir,
        config_dir,
        agent_id,
        agent_workspace_dir,
        session_workspace_dir,
    );
    if !tool_fs_expansion_allowed {
        return roots;
    }
    let expanded = append_local_media_parent_roots(&roots, media_sources);
    let added_parents: Vec<PathBuf> = expanded
        .iter()
        .filter(|r| !roots.contains(r))
        .cloned()
        .collect();

    let confined_parents = filter_shared_media_local_roots(&added_parents, state_dir, session_workspace_dir);
    if confined_parents.len() == added_parents.len() {
        return expanded;
    }

    let mut result = roots;
    for cp in confined_parents {
        if !result.contains(&cp) {
            result.push(cp);
        }
    }
    result
}
