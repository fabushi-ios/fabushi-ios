use std::collections::BTreeMap;
use std::env;
use std::fs;
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
    SandDev,
    SandLab,
    Sand,
}

impl SandVariant {
    pub fn directory_name(self) -> &'static str {
        match self {
            Self::SandDev => "sand-dev",
            Self::SandLab => "sand-lab",
            Self::Sand => "sand",
        }
    }
}

pub fn sand_variant_from_env(env: &BTreeMap<String, String>) -> SandVariant {
    if env.get("SAND_PACKAGED").is_none_or(|value| value != "1") {
        SandVariant::SandDev
    } else if env.get("SAND_LAB").is_some_and(|value| value == "1") {
        SandVariant::SandLab
    } else {
        SandVariant::Sand
    }
}

pub fn to_model_visible_path(path: &Path) -> PathBuf {
    let root = Path::new(SAND_BOX_DATA_ROOT);
    if !path.starts_with(root) {
        return path.to_path_buf();
    }
    let relative = path.strip_prefix(root).unwrap_or(Path::new(""));
    Path::new(SAND_BOX_MODEL_VISIBLE_DATA_ROOT).join(relative)
}

fn symlink_dir(target: &Path, alias: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, alias)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, alias)
    }
}

pub fn ensure_data_root_alias(data_root: &Path, alias_path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(alias_path) {
        Ok(metadata) => {
            if !metadata.file_type().is_symlink() {
                return Ok(());
            }
            if fs::read_link(alias_path).ok().as_deref() == Some(data_root) {
                return Ok(());
            }
            fs::remove_file(alias_path)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if let Some(parent) = alias_path.parent() {
        fs::create_dir_all(parent)?;
    }
    symlink_dir(data_root, alias_path)
}

pub fn read_user_data_dir_arg(argv: &[String]) -> Option<String> {
    let mut index = 0;
    while index < argv.len() {
        let argument = &argv[index];
        if argument == USER_DATA_DIR_FLAG {
            return argv
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .cloned();
        }
        let prefix = format!("{USER_DATA_DIR_FLAG}=");
        if let Some(value) = argument.strip_prefix(&prefix) {
            return Some(value.to_string());
        }
        index += 1;
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
    let path = PathBuf::from(trimmed);
    Some(if path.is_absolute() { path } else { cwd.join(path) })
}

pub fn get_sand_production_root_dir(home_dir: &Path) -> PathBuf {
    home_dir.join(SAND_PRODUCTION_DATA_DIRNAME)
}

pub fn resolve_sand_data_root_override(
    env: &BTreeMap<String, String>,
) -> Option<PathBuf> {
    let raw = env.get(SAND_DATA_ROOT_ENV)?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    path.is_absolute().then_some(path)
}

pub fn get_sand_root_dir_with(
    home_dir: &Path,
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> PathBuf {
    if let Some(override_root) = resolve_sand_data_root_override(env) {
        return override_root;
    }
    if let Some(user_data_dir) = resolve_sand_user_data_dir(argv, env, cwd) {
        return user_data_dir.join(SAND_DATA_DIRNAME);
    }
    match sand_variant_from_env(env) {
        SandVariant::Sand => get_sand_production_root_dir(home_dir),
        variant => home_dir.join(".cursor").join(variant.directory_name()),
    }
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_sand_root_dir() -> PathBuf {
    let env_map = env::vars().collect::<BTreeMap<_, _>>();
    let argv = env::args().collect::<Vec<_>>();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    get_sand_root_dir_with(&home_dir(), &argv, &env_map, &cwd)
}

pub fn reanchor_sand_path(stored_path: &Path) -> PathBuf {
    let root = get_sand_root_dir();
    if stored_path.starts_with(&root) {
        return stored_path.to_path_buf();
    }
    let components = stored_path.components().collect::<Vec<_>>();
    let mut suffix_start = None;
    for index in 0..components.len() {
        let name = match components[index] {
            Component::Normal(value) => value.to_string_lossy(),
            _ => continue,
        };
        if name == ".grokbot" {
            suffix_start = Some(index + 1);
            break;
        }
        if name == ".cursor" {
            if let Some(Component::Normal(variant)) = components.get(index + 1) {
                if variant.to_string_lossy().starts_with("sand") {
                    suffix_start = Some(index + 2);
                    break;
                }
            }
        }
    }
    let Some(start) = suffix_start else {
        return stored_path.to_path_buf();
    };
    let mut output = root;
    for component in &components[start..] {
        match component {
            Component::Normal(segment) => output.push(segment),
            Component::CurDir | Component::ParentDir => return stored_path.to_path_buf(),
            Component::RootDir | Component::Prefix(_) => return stored_path.to_path_buf(),
        }
    }
    output
}

pub fn get_gateway_discovery_path() -> PathBuf {
    get_sand_root_dir().join("gateway.json")
}

pub fn get_host_lock_path() -> PathBuf {
    get_sand_root_dir().join("host.lock")
}

pub fn get_host_secrets_path() -> PathBuf {
    get_sand_root_dir().join("host-secrets.json")
}

pub fn get_host_node_identity_path() -> PathBuf {
    get_sand_root_dir().join("host-node-identity.json")
}

pub fn get_host_node_auth_path() -> PathBuf {
    get_sand_root_dir().join("host-node-auth.json")
}

pub fn get_host_upgrade_marker_path() -> PathBuf {
    get_sand_root_dir().join(".sand-host-upgrade.json")
}

pub fn get_host_crash_marker_path() -> PathBuf {
    get_sand_root_dir().join(".sand-host-crash.json")
}
