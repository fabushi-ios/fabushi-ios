use std::env;
use std::fs;
use std::path::Path;

use crate::sand_user_identity::normalize_sand_user_full_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRequestContext {
    pub os_version: String,
    pub shell: Option<String>,
    pub time_zone: Option<String>,
    pub transcripts_folder: String,
    pub user_full_name: Option<String>,
}

pub struct HostRequestContextProvider<ResolveTimeZone, ResolveRules, ResolveUserFullName> {
    transcripts_folder: String,
    resolve_user_time_zone: ResolveTimeZone,
    resolve_rules: ResolveRules,
    resolve_user_full_name: ResolveUserFullName,
}

impl<ResolveTimeZone, ResolveRules, ResolveUserFullName>
    HostRequestContextProvider<ResolveTimeZone, ResolveRules, ResolveUserFullName>
where
    ResolveTimeZone: Fn() -> Option<String>,
    ResolveUserFullName: Fn() -> Option<String>,
{
    pub fn resolve(&self) -> HostRequestContext {
        let user_full_name = normalize_sand_user_full_name((self.resolve_user_full_name)().as_deref());
        HostRequestContext {
            os_version: resolve_os_version(),
            shell: env::var("SHELL").ok().filter(|value| !value.trim().is_empty()),
            time_zone: (self.resolve_user_time_zone)().or_else(resolve_time_zone),
            transcripts_folder: self.transcripts_folder.clone(),
            user_full_name,
        }
    }
}

impl<ResolveTimeZone, ResolveRules, ResolveUserFullName>
    HostRequestContextProvider<ResolveTimeZone, ResolveRules, ResolveUserFullName>
{
    pub fn resolve_rules<Rules>(&self) -> Rules
    where
        ResolveRules: Fn() -> Rules,
    {
        (self.resolve_rules)()
    }
}

pub fn create_host_request_context<ResolveTimeZone, ResolveRules, ResolveUserFullName>(
    transcripts_folder: impl Into<String>,
    resolve_user_time_zone: ResolveTimeZone,
    resolve_rules: ResolveRules,
    resolve_user_full_name: ResolveUserFullName,
) -> HostRequestContextProvider<ResolveTimeZone, ResolveRules, ResolveUserFullName>
where
    ResolveTimeZone: Fn() -> Option<String>,
    ResolveUserFullName: Fn() -> Option<String>,
{
    HostRequestContextProvider {
        transcripts_folder: transcripts_folder.into(),
        resolve_user_time_zone,
        resolve_rules,
        resolve_user_full_name,
    }
}

pub fn resolve_time_zone() -> Option<String> {
    if let Ok(value) = env::var("TZ") {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    if let Ok(value) = fs::read_to_string("/etc/timezone") {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    fs::read_link("/etc/localtime")
        .ok()
        .and_then(|path| zoneinfo_suffix(&path))
}

fn zoneinfo_suffix(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let marker = "/zoneinfo/";
    let (_, suffix) = text.split_once(marker)?;
    (!suffix.trim().is_empty()).then(|| suffix.to_string())
}

fn resolve_os_version() -> String {
    let os_type = if cfg!(target_os = "macos") {
        "Darwin"
    } else if cfg!(target_os = "windows") {
        "Windows_NT"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        env::consts::OS
    };

    let release = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "ver"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| value.trim().to_string())
    } else {
        std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| value.trim().to_string())
    }
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| env::consts::ARCH.to_string());

    format!("{os_type} {release}")
}
