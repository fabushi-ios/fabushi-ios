use std::collections::BTreeMap;
use std::env;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const SAND_DATA_ROOT_ENV: &str = "SAND_DATA_ROOT";
pub const SAND_PRODUCTION_DATA_DIRNAME: &str = ".grokbot";
pub const SAND_USER_DATA_DIR_ENV: &str = "SAND_USER_DATA_DIR";
pub const SAND_DATA_DIRNAME: &str = "sand-data";
pub const USER_DATA_DIR_FLAG: &str = "--user-data-dir";
pub const SAND_BOX_HOME_DIR: &str = "/home/box";
pub const SAND_BOX_DATA_ROOT: &str = "/home/box/sand-data";
pub const SAND_BOX_MODEL_VISIBLE_DATA_ROOT: &str = "/home/box/agent-data";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandVariant {
    Dev,
    Lab,
    Production,
}

impl SandVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "sand-dev",
            Self::Lab => "sand-lab",
            Self::Production => "sand",
        }
    }
}

pub fn sand_variant_from_env(env: &BTreeMap<String, String>) -> SandVariant {
    if env.get("SAND_PACKAGED").map(String::as_str) != Some("1") {
        SandVariant::Dev
    } else if env.get("SAND_LAB").map(String::as_str) == Some("1") {
        SandVariant::Lab
    } else {
        SandVariant::Production
    }
}

fn normalize_absolute(path: &Path, cwd: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

pub fn is_path_within(
    parent: &Path,
    child: &Path,
    inclusive: bool,
    cwd: &Path,
) -> bool {
    let parent = normalize_absolute(parent, cwd);
    let child = normalize_absolute(child, cwd);
    if parent == child {
        return inclusive;
    }
    child.strip_prefix(&parent).is_ok()
}

pub fn to_model_visible_path(path: &Path) -> PathBuf {
    let root = Path::new(SAND_BOX_DATA_ROOT);
    let cwd = Path::new("/");
    if !is_path_within(root, path, true, cwd) {
        return path.to_path_buf();
    }
    let relative = normalize_absolute(path, cwd)
        .strip_prefix(root)
        .unwrap_or_else(|_| Path::new(""))
        .to_path_buf();
    Path::new(SAND_BOX_MODEL_VISIBLE_DATA_ROOT).join(relative)
}

pub async fn ensure_data_root_alias(
    data_root: &Path,
    alias_path: &Path,
) -> io::Result<()> {
    match tokio::fs::symlink_metadata(alias_path).await {
        Ok(metadata) => {
            if !metadata.file_type().is_symlink() {
                return Ok(());
            }
            if tokio::fs::read_link(alias_path).await? == data_root {
                return Ok(());
            }
            tokio::fs::remove_file(alias_path).await?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    if let Some(parent) = alias_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::symlink(data_root, alias_path).await
}

pub fn read_user_data_dir_arg(argv: &[String]) -> Option<String> {
    let prefix = format!("{USER_DATA_DIR_FLAG}=");
    for (index, arg) in argv.iter().enumerate() {
        if arg == USER_DATA_DIR_FLAG {
            return argv
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .cloned();
        }
        if let Some(value) = arg.strip_prefix(&prefix) {
            return Some(value.to_owned());
        }
    }
    None
}

pub fn resolve_sand_user_data_dir(
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Option<PathBuf> {
    let raw = read_user_data_dir_arg(argv)
        .or_else(|| env.get(SAND_USER_DATA_DIR_ENV).cloned())?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(normalize_absolute(Path::new(trimmed), cwd))
}

pub fn get_sand_production_root_dir(home_dir: &Path) -> PathBuf {
    home_dir.join(SAND_PRODUCTION_DATA_DIRNAME)
}

pub fn resolve_sand_data_root_override(
    env: &BTreeMap<String, String>,
) -> Option<PathBuf> {
    let value = env.get(SAND_DATA_ROOT_ENV)?.trim();
    let path = PathBuf::from(value);
    (!value.is_empty() && path.is_absolute()).then_some(path)
}

pub fn get_sand_root_dir_with(
    home_dir: &Path,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> PathBuf {
    if let Some(override_path) = resolve_sand_data_root_override(env) {
        return override_path;
    }
    if let Some(user_data_dir) = resolve_sand_user_data_dir(&[], env, cwd) {
        return user_data_dir.join(SAND_DATA_DIRNAME);
    }
    match sand_variant_from_env(env) {
        SandVariant::Production => get_sand_production_root_dir(home_dir),
        variant => home_dir.join(".cursor").join(variant.as_str()),
    }
}

fn process_env() -> BTreeMap<String, String> {
    env::vars().collect()
}

pub fn get_sand_root_dir() -> PathBuf {
    let env = process_env();
    let home = env
        .get("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let cwd = env::current_dir().unwrap_or_else(|_| home.clone());
    get_sand_root_dir_with(&home, &env, &cwd)
}

pub fn reanchor_sand_path_with(
    stored_path: &str,
    root: &Path,
    cwd: &Path,
) -> PathBuf {
    let stored = PathBuf::from(stored_path);
    if is_path_within(root, &stored, true, cwd) {
        return stored;
    }

    let normalized = stored_path.replace('\\', "/");
    let parts = normalized.split('/').collect::<Vec<_>>();
    let mut suffix_start = None;
    for index in 0..parts.len() {
        if parts[index] == ".grokbot" {
            suffix_start = Some(index + 1);
            break;
        }
        if parts[index] == ".cursor"
            && parts
                .get(index + 1)
                .is_some_and(|value| *value == "sand" || value.starts_with("sand-"))
        {
            suffix_start = Some(index + 2);
            break;
        }
    }
    let Some(start) = suffix_start else {
        return PathBuf::from(stored_path);
    };
    if start >= parts.len() {
        return PathBuf::from(stored_path);
    }
    let suffix = &parts[start..];
    if suffix.iter().any(|segment| matches!(*segment, "." | "..")) {
        return PathBuf::from(stored_path);
    }
    suffix.iter().fold(root.to_path_buf(), |path, segment| {
        path.join(segment)
    })
}

pub fn reanchor_sand_path(stored_path: &str) -> PathBuf {
    let root = get_sand_root_dir();
    let cwd = env::current_dir().unwrap_or_else(|_| root.clone());
    reanchor_sand_path_with(stored_path, &root, &cwd)
}

pub fn get_gateway_discovery_path(home_dir: &Path) -> PathBuf {
    let env = process_env();
    let cwd = env::current_dir().unwrap_or_else(|_| home_dir.to_path_buf());
    get_sand_root_dir_with(home_dir, &env, &cwd).join("gateway.json")
}

pub fn get_host_lock_path(home_dir: &Path) -> PathBuf {
    let env = process_env();
    let cwd = env::current_dir().unwrap_or_else(|_| home_dir.to_path_buf());
    get_sand_root_dir_with(home_dir, &env, &cwd).join("host.lock")
}

pub fn get_host_secrets_path(home_dir: &Path) -> PathBuf {
    let env = process_env();
    let cwd = env::current_dir().unwrap_or_else(|_| home_dir.to_path_buf());
    get_sand_root_dir_with(home_dir, &env, &cwd).join("host-secrets.json")
}

pub fn get_host_upgrade_marker_path(home_dir: &Path) -> PathBuf {
    let env = process_env();
    let cwd = env::current_dir().unwrap_or_else(|_| home_dir.to_path_buf());
    get_sand_root_dir_with(home_dir, &env, &cwd).join(".sand-host-upgrade.json")
}

pub fn get_host_crash_marker_path(home_dir: &Path) -> PathBuf {
    let env = process_env();
    let cwd = env::current_dir().unwrap_or_else(|_| home_dir.to_path_buf());
    get_sand_root_dir_with(home_dir, &env, &cwd).join(".sand-host-crash.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect()
    }

    #[test]
    fn resolves_user_data_args_over_environment_and_relative_to_cwd() {
        let argv = vec![
            "app".into(),
            "--user-data-dir".into(),
            "profile".into(),
        ];
        let env = env_map(&[(SAND_USER_DATA_DIR_ENV, "/env/profile")]);
        assert_eq!(
            resolve_sand_user_data_dir(&argv, &env, Path::new("/work")),
            Some(PathBuf::from("/work/profile"))
        );
        assert_eq!(
            read_user_data_dir_arg(&["--user-data-dir=/exact".into()]),
            Some("/exact".into())
        );
        assert_eq!(
            read_user_data_dir_arg(&[
                "--user-data-dir".into(),
                "--other".into(),
            ]),
            None
        );
    }

    #[test]
    fn root_resolution_preserves_override_user_data_and_variant_order() {
        let home = Path::new("/home/me");
        let cwd = Path::new("/work");
        assert_eq!(
            get_sand_root_dir_with(
                home,
                &env_map(&[(SAND_DATA_ROOT_ENV, "/override")]),
                cwd,
            ),
            PathBuf::from("/override")
        );
        assert_eq!(
            get_sand_root_dir_with(
                home,
                &env_map(&[(SAND_USER_DATA_DIR_ENV, "profile")]),
                cwd,
            ),
            PathBuf::from("/work/profile/sand-data")
        );
        assert_eq!(
            get_sand_root_dir_with(home, &BTreeMap::new(), cwd),
            PathBuf::from("/home/me/.cursor/sand-dev")
        );
        assert_eq!(
            get_sand_root_dir_with(
                home,
                &env_map(&[("SAND_PACKAGED", "1")]),
                cwd,
            ),
            PathBuf::from("/home/me/.grokbot")
        );
        assert_eq!(
            get_sand_root_dir_with(
                home,
                &env_map(&[("SAND_PACKAGED", "1"), ("SAND_LAB", "1")]),
                cwd,
            ),
            PathBuf::from("/home/me/.cursor/sand-lab")
        );
    }

    #[test]
    fn reanchors_legacy_roots_without_accepting_parent_segments() {
        let root = Path::new("/new/root");
        assert_eq!(
            reanchor_sand_path_with(
                "/old/.cursor/sand-dev/agents/a/store.db",
                root,
                Path::new("/"),
            ),
            PathBuf::from("/new/root/agents/a/store.db")
        );
        assert_eq!(
            reanchor_sand_path_with(
                "/old/.grokbot/agents/a",
                root,
                Path::new("/"),
            ),
            PathBuf::from("/new/root/agents/a")
        );
        assert_eq!(
            reanchor_sand_path_with(
                "/old/.grokbot/../escape",
                root,
                Path::new("/"),
            ),
            PathBuf::from("/old/.grokbot/../escape")
        );
    }

    #[test]
    fn maps_only_box_data_paths_to_model_visible_root() {
        assert_eq!(
            to_model_visible_path(Path::new("/home/box/sand-data/a/b")),
            PathBuf::from("/home/box/agent-data/a/b")
        );
        assert_eq!(
            to_model_visible_path(Path::new("/tmp/a")),
            PathBuf::from("/tmp/a")
        );
    }

    #[tokio::test]
    async fn data_root_alias_is_idempotent_and_preserves_non_symlink() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-host-paths-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(root.join("data")).await.unwrap();
        let alias = root.join("alias");
        ensure_data_root_alias(&root.join("data"), &alias)
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_link(&alias).await.unwrap(),
            root.join("data")
        );
        ensure_data_root_alias(&root.join("data"), &alias)
            .await
            .unwrap();

        tokio::fs::remove_file(&alias).await.unwrap();
        tokio::fs::write(&alias, b"keep").await.unwrap();
        ensure_data_root_alias(&root.join("data"), &alias)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&alias).await.unwrap(), b"keep");
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
