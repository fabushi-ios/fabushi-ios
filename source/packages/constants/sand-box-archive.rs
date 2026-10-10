pub const SAND_BOX_PERSIST_ARCHIVE_EXCLUDES: [&str; 4] = [
    "home/box/chrome-profile/*/Cache",
    "home/box/chrome-profile/*/Code Cache",
    "home/box/chrome-profile/*/GPUCache",
    "home/box/chrome-profile/*/Service Worker/CacheStorage",
];
pub const SAND_WORKSPACE_IGNORE_FILE_NAME: &str = ".sandignore";
pub const SAND_BOX_WORKSPACE_DEFAULT_IGNORE_PATTERNS: [&str; 27] = [
    "node_modules/", ".next/", ".nuxt/", ".svelte-kit/", ".turbo/", ".parcel-cache/", ".cache/",
    "dist/", "build/", "out/", "coverage/", "__pycache__/", "*.pyc", "*.pyo", ".venv/", "venv/",
    ".pytest_cache/", ".mypy_cache/", ".ruff_cache/", ".tox/", ".ipynb_checkpoints/", "*.egg-info/",
    ".eggs/", "target/", ".gradle/", "core.[0-9]*", "*.core",
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_archive_and_workspace_exclusions() {
        assert_eq!(SAND_BOX_PERSIST_ARCHIVE_EXCLUDES.len(), 4);
        assert_eq!(SAND_WORKSPACE_IGNORE_FILE_NAME, ".sandignore");
        assert_eq!(SAND_BOX_WORKSPACE_DEFAULT_IGNORE_PATTERNS.len(), 27);
        assert!(SAND_BOX_WORKSPACE_DEFAULT_IGNORE_PATTERNS.contains(&"target/"));
    }
}
