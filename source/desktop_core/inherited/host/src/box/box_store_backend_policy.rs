use std::collections::{BTreeMap, HashMap};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const SAND_BOX_STORE_LOCAL_DIR_ENV: &str = "SAND_BOX_STORE_LOCAL_DIR";
pub const SAND_BOX_STORE_BACKEND_ENV: &str = "SAND_BOX_STORE_BACKEND";
pub const FABUSHI_NATIVE_BOX_STORE_ENV: &str = "FABUSHI_NATIVE_BOX_STORE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxStoreBackendKind {
    LocalFs,
    Native,
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
    environment: &BTreeMap<String, String>,
) -> BoxStoreBackendKind {
    if local_dir.is_some() {
        return BoxStoreBackendKind::LocalFs;
    }
    if environment.get(FABUSHI_NATIVE_BOX_STORE_ENV).is_some_and(|v|v == "1") {
        return BoxStoreBackendKind::Native;
    }
    match environment
        .get(SAND_BOX_STORE_BACKEND_ENV)
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("v2") => BoxStoreBackendKind::SandBoxStoreV2,
        _ => BoxStoreBackendKind::AgentStore,
    }
}

pub fn resolve_box_store_backend_policy(
    environment: &BTreeMap<String, String>,
) -> BoxStoreBackendPolicy {
    let local_dir = environment
        .get(SAND_BOX_STORE_LOCAL_DIR_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    BoxStoreBackendPolicy {
        kind: resolve_backend_kind(local_dir.as_deref(), environment),
        local_dir,
    }
}

#[derive(Default)]
pub struct BoxStoreBackendPolicyCache {
    policies: HashMap<usize, BoxStoreBackendPolicy>,
}

impl BoxStoreBackendPolicyCache {
    pub fn get(&mut self, environment: &BTreeMap<String, String>) -> BoxStoreBackendPolicy {
        let identity = environment as *const BTreeMap<String, String> as usize;
        self.policies
            .entry(identity)
            .or_insert_with(|| resolve_box_store_backend_policy(environment))
            .clone()
    }
}

pub fn get_box_store_backend_policy() -> &'static BoxStoreBackendPolicy {
    static POLICY: OnceLock<BoxStoreBackendPolicy> = OnceLock::new();
    POLICY.get_or_init(|| {
        let environment = env::vars().collect::<BTreeMap<_, _>>();
        resolve_box_store_backend_policy(&environment)
    })
}

fn enabled(value: Option<&String>) -> bool {
    matches!(
        value.map(|value| value.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes")
    )
}

pub fn is_box_store_sync_enabled(environment: &BTreeMap<String, String>) -> bool {
    enabled(environment.get("SAND_BOX_STORE_SYNC"))
}

pub fn is_box_store_copy_in_enabled(environment: &BTreeMap<String, String>) -> bool {
    enabled(environment.get("SAND_BOX_STORE_COPY_IN"))
}
