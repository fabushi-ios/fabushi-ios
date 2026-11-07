use std::path::{Path, PathBuf};

use super::workspace_ignore::{WorkspaceIgnore, compile_workspace_ignore};

pub const SAND_STORE_BETTER_CLI_ENV: &str = "SAND_STORE_BETTER_CLI";
pub const SAND_USER_NON_ROOT_ENV: &str = "SAND_USER_NON_ROOT";
pub const BOX_HOME_CATEGORY_NAME: &str = "home";
pub const BOX_HOME_DIR: &str = "/home/box";
pub const BOX_HOME_REL_PREFIX: &str = "home/box";
pub const BOX_STORE_SYNC_INTERVAL_MS: u64 = 2 * 60_000;
pub const BOX_STORE_CHROME_INTERVAL_MS: u64 = 15 * 60_000;
pub const BOX_STORE_DB_DEBOUNCE_MS: u64 = 5_000;
pub const CHROME_SESSION_CHANGE_DEBOUNCE_MS: u64 = 5_000;
pub const BOX_STORE_MANIFEST_RETRY_ATTEMPTS: usize = 3;
pub const BOX_STORE_MANIFEST_RETRY_DELAY_MS: u64 = 100;

pub const BOX_HOME_PRUNE_GUARDED_FOREIGN_TREES: [&str; 5] = [
    "home/box/cli-config",
    "home/box/chrome-profile*",
    "home/box/sand-data",
    "home/box/sand-host",
    "home/box/deps",
];

pub const BOX_HOME_REGENERABLE_JUNK_IGNORE_PATTERNS: [&str; 23] = [
    "/cli-config/",
    "/chrome-profile*/",
    "/sand-data/",
    "/sand-host/",
    "/deps/",
    ".cache/",
    "/.npm/",
    "/.nvm/",
    "/.rustup/",
    "/.cargo/registry/",
    "/.cargo/git/",
    "/.local/share/pnpm/",
    "/.local/share/Trash/",
    "/.local/state/",
    "/.docker/buildx/",
    "/.gradle/caches/",
    "/.gradle/daemon/",
    "/.gradle/wrapper/",
    "/.m2/repository/",
    "/.vnc/",
    "/.dbus/",
    "/.Xauthority",
    "*.sock",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreCategory {
    pub name: String,
    pub abs_root: PathBuf,
    pub rel_prefix: String,
    pub excludes: Vec<String>,
    pub idle_only: bool,
    pub stage_only: bool,
    pub contains_agent_store_dbs: bool,
}

pub fn build_box_home_category(abs_root: impl AsRef<Path>) -> BoxStoreCategory {
    BoxStoreCategory {
        name: BOX_HOME_CATEGORY_NAME.into(),
        abs_root: abs_root.as_ref().to_path_buf(),
        rel_prefix: BOX_HOME_REL_PREFIX.into(),
        excludes: BOX_HOME_PRUNE_GUARDED_FOREIGN_TREES
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        idle_only: false,
        stage_only: false,
        contains_agent_store_dbs: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxStorePeriodicCyclePlan {
    pub seal_hydration_handoff: bool,
    pub include_idle_only: bool,
    pub include_store_dbs: bool,
    pub skip_live_handle_store_dbs: bool,
    pub include_packs: bool,
}

pub fn plan_periodic_cycle(
    idle: bool,
    hydration_handoff_pending: bool,
    elapsed_since_idle_only_ms: u64,
) -> BoxStorePeriodicCyclePlan {
    let seal_hydration_handoff = hydration_handoff_pending && idle;
    let include_idle_only = seal_hydration_handoff
        || (idle && elapsed_since_idle_only_ms >= BOX_STORE_CHROME_INTERVAL_MS);
    BoxStorePeriodicCyclePlan {
        seal_hydration_handoff,
        include_idle_only,
        include_store_dbs: idle,
        skip_live_handle_store_dbs: !seal_hydration_handoff,
        include_packs: include_idle_only,
    }
}

pub fn build_box_home_ignore() -> WorkspaceIgnore {
    compile_workspace_ignore(BOX_HOME_REGENERABLE_JUNK_IGNORE_PATTERNS)
}

pub fn is_better_cli_home_enabled(better_cli: Option<&str>, user_non_root: Option<&str>) -> bool {
    better_cli == Some("1") && user_non_root == Some("1")
}
