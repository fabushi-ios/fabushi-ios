use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SAND_BOX_STORE_LOCAL_DIR_ENV: &str = "SAND_BOX_STORE_LOCAL_DIR";
pub const SAND_BOX_STORE_BACKEND_ENV: &str = "SAND_BOX_STORE_BACKEND";
pub const SAND_BOX_STORE_SYNC_ENV: &str = "SAND_BOX_STORE_SYNC";
pub const SAND_BOX_STORE_COPY_IN_ENV: &str = "SAND_BOX_STORE_COPY_IN";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxStoreBackendKind {
    LocalFs,
    SandBoxStoreV2,
    AgentStore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreBackendPolicy {
    pub kind: BoxStoreBackendKind,
    pub local_dir: Option<PathBuf>,
}

pub fn resolve_backend_kind(
    local_dir: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> BoxStoreBackendKind {
    if local_dir.is_some() {
        return BoxStoreBackendKind::LocalFs;
    }
    if env
        .get(SAND_BOX_STORE_BACKEND_ENV)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("v2"))
    {
        BoxStoreBackendKind::SandBoxStoreV2
    } else {
        BoxStoreBackendKind::AgentStore
    }
}

pub fn get_box_store_backend_policy(
    env: &BTreeMap<String, String>,
) -> BoxStoreBackendPolicy {
    let local_dir = env
        .get(SAND_BOX_STORE_LOCAL_DIR_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());

    BoxStoreBackendPolicy {
        kind: resolve_backend_kind(local_dir.as_deref(), env),
        local_dir,
    }
}

fn enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

pub fn is_box_store_sync_enabled(env: &BTreeMap<String, String>) -> bool {
    enabled(env.get(SAND_BOX_STORE_SYNC_ENV).map(String::as_str))
}

pub fn is_box_store_copy_in_enabled(env: &BTreeMap<String, String>) -> bool {
    enabled(env.get(SAND_BOX_STORE_COPY_IN_ENV).map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_local_directory_has_priority() {
        let env = BTreeMap::from([
            (SAND_BOX_STORE_LOCAL_DIR_ENV.into(), "/tmp/box-store".into()),
            (SAND_BOX_STORE_BACKEND_ENV.into(), "v2".into()),
        ]);
        let policy = get_box_store_backend_policy(&env);
        assert_eq!(policy.kind, BoxStoreBackendKind::LocalFs);
        assert_eq!(
            policy.local_dir.as_deref(),
            Some(Path::new("/tmp/box-store"))
        );
    }

    #[test]
    fn relative_local_directory_is_ignored() {
        let env = BTreeMap::from([
            (SAND_BOX_STORE_LOCAL_DIR_ENV.into(), "relative/store".into()),
            (SAND_BOX_STORE_BACKEND_ENV.into(), " V2 ".into()),
        ]);
        let policy = get_box_store_backend_policy(&env);
        assert_eq!(policy.kind, BoxStoreBackendKind::SandBoxStoreV2);
        assert!(policy.local_dir.is_none());
    }

    #[test]
    fn boolean_flags_match_reference_truthy_values() {
        let env = BTreeMap::from([
            (SAND_BOX_STORE_SYNC_ENV.into(), "YES".into()),
            (SAND_BOX_STORE_COPY_IN_ENV.into(), "0".into()),
        ]);
        assert!(is_box_store_sync_enabled(&env));
        assert!(!is_box_store_copy_in_enabled(&env));
    }
}
