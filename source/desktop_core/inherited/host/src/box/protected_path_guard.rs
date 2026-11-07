use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct SandProtectedPathError(pub String);

pub fn refusal_message(path: &Path) -> String {
    format!(
        "Path is inside a protected host-only store and was refused: {}",
        path.display()
    )
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

fn realpath_nearest_existing(path: &Path) -> PathBuf {
    let mut cursor = path.to_path_buf();
    let mut suffix = Vec::<OsString>::new();
    loop {
        if let Ok(real) = fs::canonicalize(&cursor) {
            let mut result = real;
            for segment in suffix.iter().rev() {
                result.push(segment);
            }
            return lexical_normalize(&result);
        }
        let Some(name) = cursor.file_name().map(OsString::from) else {
            return lexical_normalize(path);
        };
        let Some(parent) = cursor.parent() else {
            return lexical_normalize(path);
        };
        suffix.push(name);
        cursor = parent.to_path_buf();
    }
}

fn is_path_within(root: &Path, candidate: &Path) -> bool {
    candidate == root || candidate.starts_with(root)
}

pub fn assert_path_outside_protected_roots(
    protected_roots: &[PathBuf],
    candidate_path: &Path,
    base_dir: &Path,
) -> Result<(), SandProtectedPathError> {
    if protected_roots.is_empty() {
        return Ok(());
    }

    let resolved = if candidate_path.is_absolute() {
        lexical_normalize(candidate_path)
    } else {
        lexical_normalize(&base_dir.join(candidate_path))
    };

    for root in protected_roots {
        let root = lexical_normalize(root);
        if is_path_within(&root, &resolved) {
            return Err(SandProtectedPathError(refusal_message(candidate_path)));
        }
    }

    let real_resolved = realpath_nearest_existing(&resolved);
    for root in protected_roots {
        let real_root = realpath_nearest_existing(root);
        if is_path_within(&real_root, &real_resolved) {
            return Err(SandProtectedPathError(refusal_message(candidate_path)));
        }
    }
    Ok(())
}
