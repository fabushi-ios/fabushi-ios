use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginSdkApiExportKind {
    Function,
    Type,
    Class,
    Const,
    Interface,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSdkApiExport {
    pub name: String,
    pub kind: PluginSdkApiExportKind,
    pub declaration: String,
    pub closure_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSdkApiDeclarationChange {
    pub name: String,
    pub previous_hash: Option<String>,
    pub current_hash: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSdkApiDiff {
    pub breaking_changes: Vec<String>,
    pub added_exports: Vec<String>,
    pub removed_exports: Vec<String>,
    pub changed_exports: Vec<String>,
}

pub fn normalize_plugin_sdk_api_source_path(repo_root: &Path, file_path: &Path) -> String {
    let resolved = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        repo_root.join(file_path)
    };

    if let Ok(rel) = resolved.strip_prefix(repo_root) {
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !rel_str.starts_with("node_modules/") {
            return rel_str;
        }
    }

    let p_str = resolved.to_string_lossy().replace('\\', "/");
    if let Some(idx) = p_str.rfind("node_modules/") {
        return p_str[idx..].to_string();
    }

    p_str
}

pub fn normalize_plugin_sdk_api_declaration_text(repo_root: &Path, value: &str) -> String {
    let root_str = repo_root.to_string_lossy().replace('\\', "/");
    value.replace(&root_str, "")
}

pub fn diff_plugin_sdk_api(
    before: &[PluginSdkApiExport],
    after: &[PluginSdkApiExport],
) -> PluginSdkApiDiff {
    let before_map: HashMap<&str, &PluginSdkApiExport> =
        before.iter().map(|e| (e.name.as_str(), e)).collect();
    let after_map: HashMap<&str, &PluginSdkApiExport> =
        after.iter().map(|e| (e.name.as_str(), e)).collect();

    let mut breaking_changes = Vec::new();
    let mut added_exports = Vec::new();
    let mut removed_exports = Vec::new();
    let mut changed_exports = Vec::new();

    // Check for removed or changed exports
    for (name, prev) in &before_map {
        if let Some(curr) = after_map.get(name) {
            if prev.kind != curr.kind {
                breaking_changes.push(format!(
                    "Export '{name}' changed kind from {:?} to {:?}",
                    prev.kind, curr.kind
                ));
            } else if prev.closure_hash != curr.closure_hash {
                changed_exports.push(name.to_string());
            }
        } else {
            removed_exports.push(name.to_string());
            breaking_changes.push(format!("Export '{name}' was removed"));
        }
    }

    // Check for added exports
    for name in after_map.keys() {
        if !before_map.contains_key(name) {
            added_exports.push(name.to_string());
        }
    }

    breaking_changes.sort();
    added_exports.sort();
    removed_exports.sort();
    changed_exports.sort();

    PluginSdkApiDiff {
        breaking_changes,
        added_exports,
        removed_exports,
        changed_exports,
    }
}

pub fn has_plugin_sdk_api_changes(diff: &PluginSdkApiDiff) -> bool {
    !diff.breaking_changes.is_empty()
        || !diff.added_exports.is_empty()
        || !diff.removed_exports.is_empty()
        || !diff.changed_exports.is_empty()
}

pub fn format_plugin_sdk_api_diff_report(diff: &PluginSdkApiDiff) -> String {
    let mut report = String::new();
    if !diff.breaking_changes.is_empty() {
        report.push_str("### Breaking Changes\n");
        for b in &diff.breaking_changes {
            report.push_str(&format!("- {b}\n"));
        }
    }
    if !diff.added_exports.is_empty() {
        report.push_str("### Added Exports\n");
        for a in &diff.added_exports {
            report.push_str(&format!("- {a}\n"));
        }
    }
    if !diff.removed_exports.is_empty() {
        report.push_str("### Removed Exports\n");
        for r in &diff.removed_exports {
            report.push_str(&format!("- {r}\n"));
        }
    }
    if !diff.changed_exports.is_empty() {
        report.push_str("### Changed Exports\n");
        for c in &diff.changed_exports {
            report.push_str(&format!("- {c}\n"));
        }
    }
    report
}
