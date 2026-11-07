use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// ClawHub Install Error Codes & Failures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClawHubInstallErrorCode {
    InvalidSpec,
    PackageNotFound,
    VersionNotFound,
    NoInstallableVersion,
    SkillPackage,
    UnsupportedFamily,
    PrivatePackage,
    IncompatiblePluginApi,
    InvalidGatewayVersion,
    UnknownGatewayVersion,
    IncompatibleGateway,
    ArtifactUnavailable,
    MissingArchiveIntegrity,
    ArtifactDownloadUnavailable,
    ArchiveIntegrityMismatch,
    ClawhubSecurityUnavailable,
    ClawhubDownloadBlocked,
    PluginIdMismatch,
}

impl ClawHubInstallErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidSpec => "invalid_spec",
            Self::PackageNotFound => "package_not_found",
            Self::VersionNotFound => "version_not_found",
            Self::NoInstallableVersion => "no_installable_version",
            Self::SkillPackage => "skill_package",
            Self::UnsupportedFamily => "unsupported_family",
            Self::PrivatePackage => "private_package",
            Self::IncompatiblePluginApi => "incompatible_plugin_api",
            Self::InvalidGatewayVersion => "invalid_gateway_version",
            Self::UnknownGatewayVersion => "unknown_gateway_version",
            Self::IncompatibleGateway => "incompatible_gateway",
            Self::ArtifactUnavailable => "artifact_unavailable",
            Self::MissingArchiveIntegrity => "missing_archive_integrity",
            Self::ArtifactDownloadUnavailable => "artifact_download_unavailable",
            Self::ArchiveIntegrityMismatch => "archive_integrity_mismatch",
            Self::ClawhubSecurityUnavailable => "clawhub_security_unavailable",
            Self::ClawhubDownloadBlocked => "clawhub_download_blocked",
            Self::PluginIdMismatch => "plugin_id_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClawHubInstallFailure {
    pub ok: bool,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<ClawHubInstallErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl ClawHubInstallFailure {
    pub fn new(
        error: impl Into<String>,
        code: Option<ClawHubInstallErrorCode>,
        warning: Option<String>,
        version: Option<String>,
    ) -> Self {
        Self {
            ok: false,
            error: error.into(),
            code,
            warning,
            version,
        }
    }

    pub fn is_unavailable_target(&self) -> bool {
        matches!(
            self.code,
            Some(ClawHubInstallErrorCode::PackageNotFound)
                | Some(ClawHubInstallErrorCode::VersionNotFound)
        )
    }
}

// ---------------------------------------------------------------------------
// Official ClawHub Source & Base URL Resolution
// ---------------------------------------------------------------------------

pub const DEFAULT_CLAWHUB_BASE_URL: &str = "https://clawhub.ai";
pub const DEFAULT_CLAWHUB_API_BASE_URL: &str = "https://api.clawhub.ai";

pub fn is_default_clawhub_base_url(base_url: Option<&str>) -> bool {
    match base_url {
        None => true,
        Some(url) => {
            let trimmed = url.trim().trim_end_matches('/');
            trimmed.is_empty()
                || trimmed == DEFAULT_CLAWHUB_BASE_URL
                || trimmed == DEFAULT_CLAWHUB_API_BASE_URL
        }
    }
}

pub fn is_default_official_clawhub_package(
    base_url: Option<&str>,
    channel: Option<&str>,
    is_official: bool,
) -> bool {
    is_default_clawhub_base_url(base_url)
        && (channel == Some("official") || is_official)
}

// ---------------------------------------------------------------------------
// ClawHub Presentation & Link Formatting
// ---------------------------------------------------------------------------

pub fn format_clawhub_specifier(name: &str, version: Option<&str>) -> String {
    match version {
        Some(v) if !v.trim().is_empty() => format!("clawhub:{name}@{v}"),
        _ => format!("clawhub:{name}"),
    }
}

pub fn format_clawhub_release_label(name: &str, version: &str) -> String {
    format!("{name}@{version}")
}

pub fn resolve_clawhub_plugin_url(base_url: Option<&str>, package_name: &str) -> String {
    let base = if is_default_clawhub_base_url(base_url) {
        DEFAULT_CLAWHUB_BASE_URL
    } else {
        base_url.unwrap().trim().trim_end_matches('/')
    };
    format!("{base}/plugins/{package_name}")
}

// ---------------------------------------------------------------------------
// ClawHub Archive & File List Verification
// ---------------------------------------------------------------------------

pub const CLAWHUB_GENERATED_ARCHIVE_METADATA_FILE: &str = "_meta.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClawHubFileVerificationEntry {
    pub path: String,
    pub sha256: String,
}

pub struct ClawHubArchiveVerification;
impl ClawHubArchiveVerification {
    pub fn validate_meta_json(
        package_name: &str,
        version: &str,
        bytes: &[u8],
    ) -> Result<(), ClawHubInstallFailure> {
        let parsed: serde_json::Value = match serde_json::from_slice(bytes) {
            Ok(val) => val,
            Err(_) => {
                return Err(ClawHubInstallFailure::new(
                    format!(
                        "ClawHub archive contents do not match files[] metadata for \"{package_name}@{version}\": _meta.json is not valid JSON."
                    ),
                    Some(ClawHubInstallErrorCode::ArchiveIntegrityMismatch),
                    None,
                    None,
                ));
            }
        };

        let obj = match parsed.as_object() {
            Some(o) => o,
            None => {
                return Err(ClawHubInstallFailure::new(
                    format!(
                        "ClawHub archive contents do not match files[] metadata for \"{package_name}@{version}\": _meta.json is not a JSON object."
                    ),
                    Some(ClawHubInstallErrorCode::ArchiveIntegrityMismatch),
                    None,
                    None,
                ));
            }
        };

        if obj.get("slug").and_then(|s| s.as_str()) != Some(package_name) {
            return Err(ClawHubInstallFailure::new(
                format!(
                    "ClawHub archive contents do not match files[] metadata for \"{package_name}@{version}\": _meta.json slug does not match the package name."
                ),
                Some(ClawHubInstallErrorCode::ArchiveIntegrityMismatch),
                None,
                None,
            ));
        }

        if obj.get("version").and_then(|v| v.as_str()) != Some(version) {
            return Err(ClawHubInstallFailure::new(
                format!(
                    "ClawHub archive contents do not match files[] metadata for \"{package_name}@{version}\": _meta.json version does not match the package version."
                ),
                Some(ClawHubInstallErrorCode::ArchiveIntegrityMismatch),
                None,
                None,
            ));
        }

        Ok(())
    }

    pub fn verify_extracted_files(
        actual_files: &HashMap<String, (Vec<u8>, String)>, // relative_path -> (bytes, sha256)
        expected_files: &[ClawHubFileVerificationEntry],
        package_name: &str,
        package_version: &str,
    ) -> Result<Vec<String>, ClawHubInstallFailure> {
        let mut validated_generated_paths = Vec::new();
        let mut remaining_actual: HashMap<String, String> = HashMap::new();

        for (path, (bytes, digest)) in actual_files {
            if path == CLAWHUB_GENERATED_ARCHIVE_METADATA_FILE {
                Self::validate_meta_json(package_name, package_version, bytes)?;
                validated_generated_paths.push(path.clone());
            } else {
                remaining_actual.insert(path.clone(), digest.clone());
            }
        }

        for exp in expected_files {
            match remaining_actual.get(&exp.path) {
                None => {
                    return Err(ClawHubInstallFailure::new(
                        format!(
                            "ClawHub archive contents do not match files[] metadata for \"{package_name}@{package_version}\": missing \"{}\".",
                            exp.path
                        ),
                        Some(ClawHubInstallErrorCode::ArchiveIntegrityMismatch),
                        None,
                        None,
                    ));
                }
                Some(actual_sha) if actual_sha != &exp.sha256 => {
                    return Err(ClawHubInstallFailure::new(
                        format!(
                            "ClawHub archive contents do not match files[] metadata for \"{package_name}@{package_version}\": expected {} to hash to {}, got {}.",
                            exp.path, exp.sha256, actual_sha
                        ),
                        Some(ClawHubInstallErrorCode::ArchiveIntegrityMismatch),
                        None,
                        None,
                    ));
                }
                Some(_) => {
                    remaining_actual.remove(&exp.path);
                }
            }
        }

        if !remaining_actual.is_empty() {
            let mut unexpected: Vec<String> = remaining_actual.into_keys().collect();
            unexpected.sort();
            return Err(ClawHubInstallFailure::new(
                format!(
                    "ClawHub archive contents do not match files[] metadata for \"{package_name}@{package_version}\": unexpected file \"{}\".",
                    unexpected[0]
                ),
                Some(ClawHubInstallErrorCode::ArchiveIntegrityMismatch),
                None,
                None,
            ));
        }

        Ok(validated_generated_paths)
    }
}

// ---------------------------------------------------------------------------
// ClawHub Install Records Fields
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClawHubPluginInstallRecordFields {
    pub source: String, // "clawhub"
    pub clawhub_url: String,
    pub clawhub_package: String,
    pub clawhub_family: String, // "code-plugin" | "bundle-plugin"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clawhub_channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clawhub_trust_disposition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clawhub_trust_scan_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clawhub_trust_reasons: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm_integrity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm_shasum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm_tarball_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clawpack_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clawpack_size: Option<u64>,
}

impl Default for ClawHubPluginInstallRecordFields {
    fn default() -> Self {
        Self {
            source: "clawhub".to_string(),
            clawhub_url: DEFAULT_CLAWHUB_BASE_URL.to_string(),
            clawhub_package: String::new(),
            clawhub_family: "code-plugin".to_string(),
            clawhub_channel: None,
            clawhub_trust_disposition: None,
            clawhub_trust_scan_status: None,
            clawhub_trust_reasons: None,
            version: None,
            integrity: None,
            resolved_at: None,
            installed_at: None,
            artifact_kind: None,
            artifact_format: None,
            npm_integrity: None,
            npm_shasum: None,
            npm_tarball_name: None,
            clawpack_sha256: None,
            clawpack_size: None,
        }
    }
}

// ---------------------------------------------------------------------------
// ClawHub Trust & Security Assessment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClawHubSecurityAssessment {
    pub package_name: String,
    pub release_version: String,
    pub verdict: String, // "clean" | "review-recommended" | "review-required" | "blocked"
    pub scan_status: String,
    pub blocked_from_download: bool,
    pub reasons: Vec<String>,
    pub overview: String,
}

pub struct ClawHubInstallPipeline;
impl ClawHubInstallPipeline {
    pub fn assess_package_eligibility(
        family: &str,
        channel: Option<&str>,
        package_name: &str,
    ) -> Result<(), ClawHubInstallFailure> {
        if family == "skill" {
            return Err(ClawHubInstallFailure::new(
                format!("\"{package_name}\" is a skill. Use \"openclaw skills install {package_name}\" instead."),
                Some(ClawHubInstallErrorCode::SkillPackage),
                None,
                None,
            ));
        }
        if family != "code-plugin" && family != "bundle-plugin" {
            return Err(ClawHubInstallFailure::new(
                format!("Unsupported ClawHub package family: {family}"),
                Some(ClawHubInstallErrorCode::UnsupportedFamily),
                None,
                None,
            ));
        }
        if channel == Some("private") {
            return Err(ClawHubInstallFailure::new(
                format!("\"{package_name}\" is private on ClawHub and cannot be installed anonymously."),
                Some(ClawHubInstallErrorCode::PrivatePackage),
                None,
                None,
            ));
        }
        Ok(())
    }

    pub fn assess_security_verdict(
        security: &ClawHubSecurityAssessment,
    ) -> Result<Option<String>, ClawHubInstallFailure> {
        if security.blocked_from_download || security.verdict == "blocked" {
            return Err(ClawHubInstallFailure::new(
                format!(
                    "ClawHub blocked artifact download for \"{}@{}\"; install was not started. Security scan verdict: {}.",
                    security.package_name, security.release_version, security.verdict
                ),
                Some(ClawHubInstallErrorCode::ClawhubDownloadBlocked),
                None,
                Some(security.release_version.clone()),
            ));
        }

        if security.verdict == "review-recommended" || security.verdict == "review-required" {
            Ok(Some(format!(
                "Security review recommended for {}@{}: {}",
                security.package_name, security.release_version, security.overview
            )))
        } else {
            Ok(None)
        }
    }
}
