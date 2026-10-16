use crate::package_utils_path_utils::{
    canonicalize_nearest_existing, is_path_within_path, normalize_lexically,
};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum ProtectedPathError {
    Protected(String),
    Io(io::Error),
}

impl std::fmt::Display for ProtectedPathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protected(message) => formatter.write_str(message),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ProtectedPathError {}

pub fn refusal_message(path: &Path) -> String {
    format!(
        "Path is inside a protected host-only store and was refused: {}",
        path.display()
    )
}

pub fn assert_path_outside_protected_roots(
    protected_roots: &[PathBuf],
    candidate_path: &Path,
    base_dir: &Path,
) -> Result<(), ProtectedPathError> {
    if protected_roots.is_empty() {
        return Ok(());
    }

    let absolute_candidate = if candidate_path.is_absolute() {
        candidate_path.to_path_buf()
    } else {
        base_dir.join(candidate_path)
    };
    let resolved = normalize_lexically(&absolute_candidate);

    for root in protected_roots {
        let root = normalize_lexically(root);
        if is_path_within_path(&root, &resolved) {
            return Err(ProtectedPathError::Protected(refusal_message(
                candidate_path,
            )));
        }
    }

    let real_resolved =
        canonicalize_nearest_existing(&resolved).map_err(ProtectedPathError::Io)?;
    for root in protected_roots {
        let real_root =
            canonicalize_nearest_existing(root).map_err(ProtectedPathError::Io)?;
        if is_path_within_path(&real_root, &real_resolved) {
            return Err(ProtectedPathError::Protected(refusal_message(
                candidate_path,
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_direct_and_parent_traversal_into_protected_root() {
        let root = std::env::temp_dir().join("fabushi-protected-root");
        assert!(matches!(
            assert_path_outside_protected_roots(
                std::slice::from_ref(&root),
                &root.join("secret.db"),
                Path::new("/")
            ),
            Err(ProtectedPathError::Protected(_))
        ));
        assert!(matches!(
            assert_path_outside_protected_roots(
                std::slice::from_ref(&root),
                Path::new("../fabushi-protected-root/secret.db"),
                &root.join("public")
            ),
            Err(ProtectedPathError::Protected(_))
        ));
    }

    #[test]
    fn allows_unrelated_paths() {
        let root = std::env::temp_dir().join("fabushi-protected-root");
        let other = std::env::temp_dir().join("fabushi-public-root/file.txt");
        assert!(
            assert_path_outside_protected_roots(
                &[root],
                &other,
                Path::new("/")
            )
            .is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_into_protected_root() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "fabushi-path-guard-{}",
            std::process::id()
        ));
        let protected = base.join("protected");
        let public = base.join("public");
        let link = public.join("alias");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&protected).unwrap();
        std::fs::create_dir_all(&public).unwrap();
        symlink(&protected, &link).unwrap();

        let result = assert_path_outside_protected_roots(
            std::slice::from_ref(&protected),
            &link.join("secret.db"),
            &public,
        );
        let _ = std::fs::remove_dir_all(&base);
        assert!(matches!(result, Err(ProtectedPathError::Protected(_))));
    }
}
