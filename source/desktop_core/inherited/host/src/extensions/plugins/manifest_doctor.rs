use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDoctorDiagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub plugin_id: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginDoctorReport {
    pub diagnostics: Vec<PluginDoctorDiagnostic>,
    pub scanned_plugins: usize,
}

impl PluginDoctorReport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_diagnostic(&mut self, diag: PluginDoctorDiagnostic) {
        self.diagnostics.push(diag);
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == DiagnosticSeverity::Error)
    }

    pub fn errors(&self) -> Vec<&PluginDoctorDiagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == DiagnosticSeverity::Error)
            .collect()
    }
}

pub fn check_plugin_manifest_health(
    plugin_id: &str,
    has_manifest_file: bool,
    entry_path: Option<&str>,
    has_deprecated_channel_config: bool,
) -> Vec<PluginDoctorDiagnostic> {
    let mut diags = Vec::new();

    if !has_manifest_file {
        diags.push(PluginDoctorDiagnostic {
            code: "MISSING_MANIFEST".to_string(),
            severity: DiagnosticSeverity::Error,
            message: format!("Plugin '{plugin_id}' lacks a valid manifest file"),
            plugin_id: plugin_id.to_string(),
            remediation: Some("Ensure plugin.json or .codex-plugin/plugin.json is present".to_string()),
        });
    }

    if let Some(entry) = entry_path {
        if entry.trim().is_empty() {
            diags.push(PluginDoctorDiagnostic {
                code: "EMPTY_ENTRY_POINT".to_string(),
                severity: DiagnosticSeverity::Error,
                message: format!("Plugin '{plugin_id}' entry point is empty"),
                plugin_id: plugin_id.to_string(),
                remediation: Some("Specify a valid entry file path (e.g. 'index.js')".to_string()),
            });
        }
    } else {
        diags.push(PluginDoctorDiagnostic {
            code: "NO_ENTRY_POINT".to_string(),
            severity: DiagnosticSeverity::Warning,
            message: format!("Plugin '{plugin_id}' specifies no entry point"),
            plugin_id: plugin_id.to_string(),
            remediation: Some("Declare an entry candidate or index file in the manifest".to_string()),
        });
    }

    if has_deprecated_channel_config {
        diags.push(PluginDoctorDiagnostic {
            code: "DEPRECATED_CHANNEL_CONFIG".to_string(),
            severity: DiagnosticSeverity::Warning,
            message: format!("Plugin '{plugin_id}' uses legacy channel config shape"),
            plugin_id: plugin_id.to_string(),
            remediation: Some("Migrate to openclaw/plugin-sdk/channel-config-schema".to_string()),
        });
    }

    diags
}
