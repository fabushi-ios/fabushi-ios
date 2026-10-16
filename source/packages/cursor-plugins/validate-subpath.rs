use std::fmt;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSubpathError {
    sub_path: String,
}

impl fmt::Display for InvalidSubpathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid subPath: path traversal not allowed ({:?})", self.sub_path)
    }
}

impl std::error::Error for InvalidSubpathError {}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            _ => out.push(component.as_os_str()),
        }
    }
    out
}

pub fn validate_and_resolve_subpath(base_dir: &Path, sub_path: &Path) -> Result<PathBuf, InvalidSubpathError> {
    let base = if base_dir.is_absolute() {
        lexical_normalize(base_dir)
    } else {
        lexical_normalize(&std::env::current_dir().unwrap_or_default().join(base_dir))
    };
    let resolved = if sub_path.is_absolute() {
        lexical_normalize(sub_path)
    } else {
        lexical_normalize(&base.join(sub_path))
    };
    if resolved.strip_prefix(&base).is_err() {
        return Err(InvalidSubpathError {
            sub_path: sub_path.to_string_lossy().into_owned(),
        });
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_descendants_and_base_itself() {
        let base = Path::new("/tmp/plugin");
        assert_eq!(validate_and_resolve_subpath(base, Path::new("a/../b/file.json")).unwrap(), Path::new("/tmp/plugin/b/file.json"));
        assert_eq!(validate_and_resolve_subpath(base, Path::new("")).unwrap(), base);
    }

    #[test]
    fn rejects_parent_and_absolute_escape() {
        let base = Path::new("/tmp/plugin");
        assert!(validate_and_resolve_subpath(base, Path::new("../secret")).is_err());
        assert!(validate_and_resolve_subpath(base, Path::new("/etc/passwd")).is_err());
        assert!(validate_and_resolve_subpath(base, Path::new("../plugin-evil/file")).is_err());
    }
}
