use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const COMMAND_FAMILIES: &[&str] = &[
    "node", "bun", "git", "ps", "pgrep", "lsof", "sh", "bash", "zsh", "cmd",
    "powershell", "pwsh", "npm", "pnpm", "python", "python3", "uv", "ssh", "openclaw",
];

pub fn classify_command_family(command: &str) -> &'static str {
    let name = Path::new(command)
        .file_name()
        .and_then(|v| v.to_str())
        .map(|v| v.trim_end_matches(".exe").trim_end_matches(".cmd"))
        .unwrap_or(command)
        .to_ascii_lowercase();

    for &family in COMMAND_FAMILIES {
        if name == family {
            return family;
        }
    }
    "other"
}

pub fn classify_unscoped_operation(family: &str, name: &str, args: &[&str]) -> String {
    if let Some(&first) = args.first() {
        if first == "--version" || (first == "-v" && matches!(family, "node" | "bun" | "gh")) {
            return "unscoped.version".to_string();
        }
    }
    match family {
        "powershell" | "pwsh" => {
            if args.iter().any(|arg| arg.eq_ignore_ascii_case("-command") || arg.eq_ignore_ascii_case("-c")) {
                "unscoped.shell-command".to_string()
            } else {
                "unscoped.shell".to_string()
            }
        }
        "sh" | "bash" | "zsh" => {
            if args.iter().any(|arg| *arg == "-c" || arg.starts_with("-c")) {
                "unscoped.shell-command".to_string()
            } else {
                "unscoped.shell".to_string()
            }
        }
        "cmd" => {
            if args.iter().any(|arg| arg.eq_ignore_ascii_case("/c") || arg.eq_ignore_ascii_case("-c")) {
                "unscoped.shell-command".to_string()
            } else {
                "unscoped.shell".to_string()
            }
        }
        "node" | "bun" => "unscoped.runtime".to_string(),
        "git" => classify_unscoped_git_operation(args),
        "other" if matches!(name, "gh" | "docker" | "podman" | "which" | "uname" | "curl" | "tar") => {
            format!("unscoped.{name}")
        }
        _ => "unscoped.command".to_string(),
    }
}

pub fn classify_unscoped_git_operation(args: &[&str]) -> String {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if matches!(arg, "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env") {
            i += 2;
        } else if arg.starts_with("--git-dir=") || arg.starts_with("--work-tree=") {
            i += 1;
        } else if matches!(arg, "--no-pager" | "--no-optional-locks" | "--bare") {
            i += 1;
        } else {
            break;
        }
    }
    if let Some(&cmd) = args.get(i) {
        if cmd == "--version" || cmd == "version" {
            "unscoped.version".to_string()
        } else if matches!(
            cmd,
            "status" | "diff" | "clone" | "fetch" | "commit" | "push" | "checkout" | "branch"
                | "rev-parse" | "log" | "show" | "worktree" | "merge" | "reset" | "clean"
        ) {
            format!("unscoped.{cmd}")
        } else {
            "unscoped.other".to_string()
        }
    } else {
        "unscoped.other".to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCountRecord {
    pub family: String,
    pub operation: String,
    pub count: u64,
}

#[derive(Clone)]
pub struct ChildProcessSpawnTracker {
    counts: Arc<Mutex<HashMap<String, SpawnCountRecord>>>,
    sampled_at: Arc<Mutex<Instant>>,
}

impl Default for ChildProcessSpawnTracker {
    fn default() -> Self {
        Self {
            counts: Arc::new(Mutex::new(HashMap::new())),
            sampled_at: Arc::new(Mutex::new(Instant::now())),
        }
    }
}

impl ChildProcessSpawnTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_spawn(&self, command: &str, args: &[&str], operation_override: Option<&str>) {
        let family = classify_command_family(command);
        let name = Path::new(command)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or(command);
        let operation = match operation_override {
            Some(op) => op.to_string(),
            None => classify_unscoped_operation(family, name, args),
        };
        let key = format!("{family}:{operation}");
        if let Ok(mut map) = self.counts.lock() {
            let entry = map.entry(key).or_insert_with(|| SpawnCountRecord {
                family: family.to_string(),
                operation,
                count: 0,
            });
            entry.count += 1;
        }
    }

    pub fn sample_counts(&self, force: bool) -> Vec<SpawnCountRecord> {
        let mut sampled_at = match self.sampled_at.lock() {
            Ok(guard) => guard,
            Err(_) => return Vec::new(),
        };
        let now = Instant::now();
        if !force && now.duration_since(*sampled_at).as_secs() < 60 {
            return Vec::new();
        }
        *sampled_at = now;
        if let Ok(mut map) = self.counts.lock() {
            map.drain().map(|(_, record)| record).collect()
        } else {
            Vec::new()
        }
    }
}

static GLOBAL_SPAWN_TRACKER: std::sync::OnceLock<ChildProcessSpawnTracker> = std::sync::OnceLock::new();

pub fn global_child_process_spawn_tracker() -> &'static ChildProcessSpawnTracker {
    GLOBAL_SPAWN_TRACKER.get_or_init(ChildProcessSpawnTracker::new)
}
