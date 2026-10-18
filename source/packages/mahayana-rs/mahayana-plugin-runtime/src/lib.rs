//! Cross-platform Mahayana plugin host primitives.
//!
//! The marketplace is metadata-only: release bytes stay on GitHub, npm, or an
//! arbitrary immutable HTTPS origin. This crate resolves, verifies, stages,
//! installs, and activates those artifacts locally. It also owns the runtime
//! lifecycle/service graph so desktop and mobile shells share one source of
//! truth.

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tar::Archive;
use url::Url;
use uuid::Uuid;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

const DEFAULT_MAX_ARTIFACT_BYTES: usize = 100 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactFormat {
    TarGz,
    Zip,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ArtifactSource {
    /// Immutable or versioned HTTPS artifact URL.
    Https { url: String },
    /// Public GitHub release asset. No marketplace binary proxy is required.
    GithubRelease {
        repository: String,
        tag: String,
        asset: String,
    },
    /// npm registry package tarball. The resolver reads registry metadata and
    /// downloads dist.tarball directly from the registry/CDN.
    Npm {
        package: String,
        version: String,
        #[serde(default = "default_npm_registry")]
        registry: String,
    },
}

fn default_npm_registry() -> String {
    "https://registry.npmjs.org".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseArtifact {
    pub id: String,
    pub runtime: String,
    pub platforms: Vec<String>,
    pub source: ArtifactSource,
    pub sha256: String,
    pub size: u64,
    pub format: ArtifactFormat,
    #[serde(default)]
    pub entry: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogFabappTrustBinding {
    pub publisher_id: String,
    pub app_id: String,
    pub artifact_id: String,
    pub key_id: String,
    pub raw_public_key_base64_url: String,
}

impl CatalogFabappTrustBinding {
    fn catalog_trust(&self) -> Result<CatalogFabappTrust, RuntimeError> {
        validate_identifier(&self.publisher_id, "fabappTrust.publisherId")?;
        validate_identifier(&self.app_id, "fabappTrust.appId")?;
        validate_identifier(&self.artifact_id, "fabappTrust.artifactId")?;
        validate_identifier(&self.key_id, "fabappTrust.keyId")?;
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(self.raw_public_key_base64_url.as_bytes())
            .map_err(|_| RuntimeError::InvalidRelease("fabapp trust public key is not base64url".into()))?;
        let public_key: [u8; 32] = decoded
            .try_into()
            .map_err(|_| RuntimeError::InvalidRelease("fabapp trust public key must be 32 bytes".into()))?;
        Ok(CatalogFabappTrust {
            publisher_id: self.publisher_id.clone(),
            app_id: self.app_id.clone(),
            key_id: self.key_id.clone(),
            public_key,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalReleaseManifest {
    pub schema_version: u32,
    pub protocol: String,
    pub plugin_id: String,
    pub version: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    pub artifacts: Vec<ReleaseArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fabapp_trust: Option<CatalogFabappTrustBinding>,
    /// Present for Marketplace releases. Generic external releases may omit
    /// this field and retain the broader runtime resolver contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<serde_json::Value>,
}

impl ExternalReleaseManifest {
    pub fn select_artifact(
        &self,
        platform: &str,
        preferred_runtimes: &[&str],
    ) -> Result<&ReleaseArtifact, RuntimeError> {
        let candidates = self
            .artifacts
            .iter()
            .filter(|artifact| artifact.platforms.iter().any(|p| p == platform))
            .collect::<Vec<_>>();
        for runtime in preferred_runtimes {
            if let Some(artifact) = candidates
                .iter()
                .copied()
                .find(|artifact| artifact.runtime == *runtime)
            {
                return Ok(artifact);
            }
        }
        candidates
            .first()
            .copied()
            .ok_or_else(|| RuntimeError::NoArtifactForPlatform(platform.to_string()))
    }

    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.schema_version != 1 || self.protocol != "mahayana.external-release.v1" {
            return Err(RuntimeError::UnsupportedReleaseProtocol);
        }
        validate_identifier(&self.plugin_id, "pluginId")?;
        if self.version.trim().is_empty() || self.artifacts.is_empty() {
            return Err(RuntimeError::InvalidRelease(
                "version/artifacts missing".into(),
            ));
        }
        validate_path_segment(&self.version, "release version")?;
        let mut ids = BTreeSet::new();
        for artifact in &self.artifacts {
            if !ids.insert(&artifact.id) {
                return Err(RuntimeError::InvalidRelease(format!(
                    "duplicate artifact id {}",
                    artifact.id
                )));
            }
            if artifact.size == 0 || artifact.sha256.len() != 64 {
                return Err(RuntimeError::InvalidRelease(format!(
                    "artifact {} has invalid digest/size",
                    artifact.id
                )));
            }
            if !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(RuntimeError::InvalidRelease(format!(
                    "artifact {} has invalid sha256",
                    artifact.id
                )));
            }
            if artifact.platforms.is_empty() || artifact.runtime.trim().is_empty() {
                return Err(RuntimeError::InvalidRelease(format!(
                    "artifact {} has no platform/runtime",
                    artifact.id
                )));
            }
            validate_source(&artifact.source)?;
        }
        if let Some(binding) = &self.fabapp_trust {
            if binding.app_id != self.plugin_id {
                return Err(RuntimeError::InvalidRelease(
                    "fabapp trust appId must match release pluginId".into(),
                ));
            }
            let _ = binding.catalog_trust()?;
            let artifact = self
                .artifacts
                .iter()
                .find(|artifact| artifact.id == binding.artifact_id)
                .ok_or_else(|| RuntimeError::InvalidRelease(
                    "fabapp trust artifactId was not found in release".into(),
                ))?;
            if artifact.format != ArtifactFormat::Zip || artifact.runtime != "local-web" {
                return Err(RuntimeError::InvalidRelease(
                    "catalog .fabapp artifact must use zip format and local-web runtime".into(),
                ));
            }
        }
        if let Some(install) = &self.install {
            validate_marketplace_install_contract(self, install)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArtifact {
    pub url: Url,
    pub expected_sha256: String,
    pub expected_size: u64,
    pub format: ArtifactFormat,
}

#[derive(Clone)]
pub struct ArtifactResolver {
    client: Client,
    max_bytes: usize,
}

impl ArtifactResolver {
    pub fn new() -> Result<Self, RuntimeError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::limited(8))
            .user_agent("Mahayana-Rust-Host/1")
            .build()
            .map_err(|error| RuntimeError::Transport(error.to_string()))?;
        Ok(Self {
            client,
            max_bytes: DEFAULT_MAX_ARTIFACT_BYTES,
        })
    }

    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    pub fn resolve(&self, artifact: &ReleaseArtifact) -> Result<ResolvedArtifact, RuntimeError> {
        let url = match &artifact.source {
            ArtifactSource::Https { url } => parse_public_https(url)?,
            ArtifactSource::GithubRelease {
                repository,
                tag,
                asset,
            } => {
                validate_github_repository(repository)?;
                validate_path_segment(tag, "GitHub release tag")?;
                validate_path_segment(asset, "GitHub release asset")?;
                parse_public_https(&format!(
                    "https://github.com/{repository}/releases/download/{tag}/{asset}"
                ))?
            }
            ArtifactSource::Npm {
                package,
                version,
                registry,
            } => self.resolve_npm_tarball(package, version, registry)?,
        };
        Ok(ResolvedArtifact {
            url,
            expected_sha256: artifact.sha256.to_ascii_lowercase(),
            expected_size: artifact.size,
            format: artifact.format.clone(),
        })
    }

    fn resolve_npm_tarball(
        &self,
        package: &str,
        version: &str,
        registry: &str,
    ) -> Result<Url, RuntimeError> {
        validate_npm_package(package)?;
        validate_path_segment(version, "npm version")?;
        let base = parse_public_https(registry)?;
        let encoded_package = package.replace('/', "%2f");
        let metadata_url = base
            .join(&format!("{encoded_package}/{version}"))
            .map_err(|error| RuntimeError::InvalidSource(error.to_string()))?;
        let response = self
            .client
            .get(metadata_url)
            .send()
            .map_err(|error| RuntimeError::Transport(error.to_string()))?;
        if !response.status().is_success() {
            return Err(RuntimeError::HttpStatus(response.status().as_u16()));
        }
        let metadata: serde_json::Value = response
            .json()
            .map_err(|error| RuntimeError::InvalidSource(error.to_string()))?;
        let tarball = metadata
            .pointer("/dist/tarball")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| RuntimeError::InvalidSource("npm dist.tarball missing".into()))?;
        parse_public_https(tarball)
    }

    pub fn download_verified(&self, artifact: &ReleaseArtifact) -> Result<Vec<u8>, RuntimeError> {
        let resolved = self.resolve(artifact)?;
        let expected_size =
            usize::try_from(resolved.expected_size).map_err(|_| RuntimeError::ArtifactTooLarge)?;
        if expected_size == 0 || expected_size > self.max_bytes {
            return Err(RuntimeError::ArtifactTooLarge);
        }
        let response = self
            .client
            .get(resolved.url)
            .send()
            .map_err(|error| RuntimeError::Transport(error.to_string()))?;
        if !response.status().is_success() {
            return Err(RuntimeError::HttpStatus(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length != resolved.expected_size)
        {
            return Err(RuntimeError::SizeMismatch);
        }
        let mut bytes = Vec::with_capacity(expected_size.min(4 * 1024 * 1024));
        response
            .take((self.max_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(RuntimeError::Io)?;
        if bytes.len() > self.max_bytes {
            return Err(RuntimeError::ArtifactTooLarge);
        }
        verify_bytes(&bytes, &resolved.expected_sha256, resolved.expected_size)?;
        Ok(bytes)
    }

    pub fn install_npm_dependencies(
        &self,
        package_root: &Path,
        registry: &str,
    ) -> Result<NpmInstallSummary, RuntimeError> {
        parse_public_https(registry)?;
        let mut state = NpmInstallState::default();
        self.install_npm_dependencies_recursive(package_root, registry, 0, &mut state)?;
        Ok(NpmInstallSummary {
            package_count: state.package_count,
            downloaded_bytes: state.downloaded_bytes,
        })
    }

    fn install_npm_dependencies_recursive(
        &self,
        package_root: &Path,
        registry: &str,
        depth: usize,
        state: &mut NpmInstallState,
    ) -> Result<(), RuntimeError> {
        if depth > 20 {
            return Err(RuntimeError::NpmDependencyLimit(
                "dependency depth exceeds 20".into(),
            ));
        }
        let manifest_path = package_root.join("package.json");
        if !manifest_path.is_file() {
            return Ok(());
        }
        let source = fs::read_to_string(&manifest_path)?;
        let manifest: NpmInstalledPackageJson =
            serde_json::from_str(&source).map_err(|error| RuntimeError::InvalidNpmPackage {
                path: manifest_path.clone(),
                message: error.to_string(),
            })?;
        let optional = manifest
            .optional_dependencies
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut dependencies = manifest.dependencies;
        dependencies.extend(manifest.optional_dependencies);
        for (requested_name, requirement) in dependencies {
            if npm_builtin_is_host_provided(&requested_name) {
                continue;
            }
            let result = self.install_one_npm_dependency(
                package_root,
                registry,
                &requested_name,
                &requirement,
                depth,
                state,
            );
            if optional.contains(&requested_name) && result.is_err() {
                continue;
            }
            result?;
        }
        Ok(())
    }

    fn install_one_npm_dependency(
        &self,
        package_root: &Path,
        registry: &str,
        requested_name: &str,
        requirement: &str,
        depth: usize,
        state: &mut NpmInstallState,
    ) -> Result<(), RuntimeError> {
        if state.package_count >= 512 || state.downloaded_bytes > 250 * 1024 * 1024 {
            return Err(RuntimeError::NpmDependencyLimit(
                "dependency graph exceeds local limits".into(),
            ));
        }
        let (actual_name, actual_requirement) = npm_dependency_target(requested_name, requirement)?;
        let resolved = self.resolve_npm_dependency(registry, &actual_name, &actual_requirement)?;
        let destination = npm_dependency_destination(package_root, requested_name)?;
        if destination.join("package.json").is_file() {
            let existing: NpmInstalledPackageJson =
                serde_json::from_str(&fs::read_to_string(destination.join("package.json"))?)
                    .map_err(|error| RuntimeError::InvalidNpmPackage {
                        path: destination.join("package.json"),
                        message: error.to_string(),
                    })?;
            if existing.version.as_deref() == Some(resolved.version.as_str()) {
                return self.install_npm_dependencies_recursive(
                    &destination,
                    registry,
                    depth + 1,
                    state,
                );
            }
        }
        if destination.exists() {
            fs::remove_dir_all(&destination)?;
        }
        fs::create_dir_all(&destination)?;
        let bytes = self.download_bounded(&resolved.tarball, 50 * 1024 * 1024)?;
        verify_npm_integrity(
            &bytes,
            resolved.integrity.as_deref(),
            resolved.shasum.as_deref(),
        )?;
        state.package_count += 1;
        state.downloaded_bytes = state.downloaded_bytes.saturating_add(bytes.len() as u64);
        extract_artifact(&bytes, &ArtifactFormat::TarGz, &destination)?;
        normalize_npm_package_root(&destination)?;
        self.install_npm_dependencies_recursive(&destination, registry, depth + 1, state)
    }

    fn resolve_npm_dependency(
        &self,
        registry: &str,
        package: &str,
        requirement: &str,
    ) -> Result<ResolvedNpmDependency, RuntimeError> {
        validate_npm_package(package)?;
        let registry = parse_public_https(registry)?;
        let encoded = package.replace('/', "%2f");
        let url = registry
            .join(&encoded)
            .map_err(|error| RuntimeError::InvalidSource(error.to_string()))?;
        let response = self
            .client
            .get(url)
            .header("Accept", "application/json")
            .send()
            .map_err(|error| RuntimeError::Transport(error.to_string()))?;
        if !response.status().is_success() {
            return Err(RuntimeError::HttpStatus(response.status().as_u16()));
        }
        let metadata: NpmFullPackageMetadata = response
            .json()
            .map_err(|error| RuntimeError::InvalidSource(error.to_string()))?;
        let version = select_npm_version(&metadata, requirement).ok_or_else(|| {
            RuntimeError::NpmVersionNotFound {
                package: package.to_string(),
                requirement: requirement.to_string(),
            }
        })?;
        let version_metadata =
            metadata
                .versions
                .get(&version)
                .ok_or_else(|| RuntimeError::NpmVersionNotFound {
                    package: package.to_string(),
                    requirement: requirement.to_string(),
                })?;
        parse_public_https(&version_metadata.dist.tarball)?;
        Ok(ResolvedNpmDependency {
            version,
            tarball: version_metadata.dist.tarball.clone(),
            integrity: version_metadata.dist.integrity.clone(),
            shasum: version_metadata.dist.shasum.clone(),
        })
    }

    fn download_bounded(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, RuntimeError> {
        let url = parse_public_https(url)?;
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| RuntimeError::Transport(error.to_string()))?;
        if !response.status().is_success() {
            return Err(RuntimeError::HttpStatus(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(RuntimeError::ArtifactTooLarge);
        }
        let mut bytes = Vec::new();
        response
            .take((max_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(RuntimeError::ArtifactTooLarge);
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmInstallSummary {
    pub package_count: usize,
    pub downloaded_bytes: u64,
}

#[derive(Default)]
struct NpmInstallState {
    package_count: usize,
    downloaded_bytes: u64,
}

#[derive(Debug)]
struct ResolvedNpmDependency {
    version: String,
    tarball: String,
    integrity: Option<String>,
    shasum: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NpmInstalledPackageJson {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    optional_dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct NpmFullPackageMetadata {
    #[serde(default, rename = "dist-tags")]
    dist_tags: BTreeMap<String, String>,
    #[serde(default)]
    versions: BTreeMap<String, NpmFullVersionMetadata>,
}

#[derive(Debug, Deserialize)]
struct NpmFullVersionMetadata {
    dist: NpmFullDistMetadata,
}

#[derive(Debug, Deserialize)]
struct NpmFullDistMetadata {
    tarball: String,
    #[serde(default)]
    integrity: Option<String>,
    #[serde(default)]
    shasum: Option<String>,
}


const FABAPP_MAX_COMPRESSED_BYTES: usize = 100 * 1024 * 1024;
const FABAPP_MAX_EXPANDED_BYTES: u64 = 250 * 1024 * 1024;
const FABAPP_MAX_FILES: usize = 10_000;
const FABAPP_MAX_INFLATION_RATIO: u64 = 100;
const FABAPP_HOST_API_V1: u32 = 1;
const FABAPP_SIGNATURE_DOMAIN: &[u8] = b"fabushi.fabapp.manifest.v1\0";
const FABAPP_DRAFT_DOMAIN: &[u8] = b"fabushi.fabapp.draft.v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FabappPackageClass {
    Catalog,
    LocalGenerated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FabappNetworkPolicy {
    pub mode: String,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FabappFileRecord {
    pub sha256: String,
    pub bytes: u64,
    pub mime: String,
}

fn deserialize_fabapp_files<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, FabappFileRecord>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct FileMapVisitor;

    impl<'de> serde::de::Visitor<'de> for FileMapVisitor {
        type Value = BTreeMap<String, FabappFileRecord>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a unique mini-app file table")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut files = BTreeMap::new();
            while let Some((path, record)) =
                map.next_entry::<String, FabappFileRecord>()?
            {
                if files.insert(path.clone(), record).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate manifest file path {path}"
                    )));
                }
            }
            Ok(files)
        }
    }

    deserializer.deserialize_map(FileMapVisitor)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FabappSignature {
    pub algorithm: String,
    pub key_id: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FabappLocalAuthorBinding {
    pub account_scope: String,
    pub draft_digest: String,
    pub user_approval_id: String,
    pub creation_transaction_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FabappManifest {
    pub manifest_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_class: Option<FabappPackageClass>,
    pub app_id: String,
    pub publisher_id: String,
    pub version: String,
    pub display_name: String,
    pub entrypoint: String,
    pub runtime: String,
    pub min_host_api: u32,
    pub offline_core: bool,
    #[serde(default)]
    pub requested_capabilities: Vec<String>,
    pub network_policy: FabappNetworkPolicy,
    #[serde(deserialize_with = "deserialize_fabapp_files")]
    pub files: BTreeMap<String, FabappFileRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_author: Option<FabappLocalAuthorBinding>,
    pub signature: FabappSignature,
}

impl FabappManifest {
    pub fn package_class(&self) -> FabappPackageClass {
        self.package_class.unwrap_or(FabappPackageClass::Catalog)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogFabappTrust {
    pub publisher_id: String,
    pub app_id: String,
    pub key_id: String,
    pub public_key: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalGeneratedFabappTrust {
    pub account_scope: String,
    pub publisher_id: String,
    pub reserved_app_id: String,
    pub approved_draft_digest: String,
    pub user_approval_id: String,
    pub creation_transaction_id: String,
    pub key_id: String,
    pub public_key: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FabappTrustContext {
    Catalog(CatalogFabappTrust),
    LocalGenerated(LocalGeneratedFabappTrust),
}

#[derive(Debug, Clone)]
pub struct VerifiedFabappPackage {
    pub manifest: FabappManifest,
    pub package_sha256: String,
    pub draft_digest: String,
    files: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FabappInstallJournal {
    schema_version: u32,
    transaction_id: String,
    package_sha256: String,
    state: String,
    #[serde(default)]
    staging_relative: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalApprovalRecord {
    schema_version: u32,
    account_scope: String,
    app_id: String,
    draft_digest: String,
    user_approval_id: String,
    creation_transaction_id: String,
    package_sha256: String,
    state: String,
}

pub fn compute_fabapp_draft_digest(manifest: &FabappManifest) -> Result<String, RuntimeError> {
    let binding = serde_json::json!({
        "manifestVersion": manifest.manifest_version,
        "packageClass": manifest.package_class,
        "appId": manifest.app_id,
        "publisherId": manifest.publisher_id,
        "version": manifest.version,
        "displayName": manifest.display_name,
        "entrypoint": manifest.entrypoint,
        "runtime": manifest.runtime,
        "minHostApi": manifest.min_host_api,
        "offlineCore": manifest.offline_core,
        "requestedCapabilities": manifest.requested_capabilities,
        "networkPolicy": manifest.network_policy,
        "files": manifest.files,
    });
    let canonical = jcs_canonicalize(&binding)?;
    let mut digest = Sha256::new();
    digest.update(FABAPP_DRAFT_DOMAIN);
    digest.update(canonical.as_bytes());
    Ok(format!("{:x}", digest.finalize()))
}

pub fn prepare_local_generated_fabapp_manifest(
    app_id: &str,
    publisher_id: &str,
    version: &str,
    display_name: &str,
    entrypoint: &str,
    requested_capabilities: Vec<String>,
    resources: &BTreeMap<String, (String, Vec<u8>)>,
) -> Result<FabappManifest, RuntimeError> {
    if resources.is_empty() || resources.len() > FABAPP_MAX_FILES {
        return Err(RuntimeError::FabappManifest(
            "local-generated draft resources are empty or too large".into(),
        ));
    }
    let mut files = BTreeMap::new();
    let mut expanded = 0_u64;
    for (path, (mime, data)) in resources {
        validate_fabapp_path(path)?;
        expanded = expanded
            .checked_add(data.len() as u64)
            .ok_or_else(|| RuntimeError::FabappPolicy("expanded size overflow".into()))?;
        if expanded > FABAPP_MAX_EXPANDED_BYTES {
            return Err(RuntimeError::FabappPolicy(
                "local-generated draft exceeds expanded size limit".into(),
            ));
        }
        let expected = fabapp_expected_mime(path).ok_or_else(|| {
            RuntimeError::FabappPolicy(format!("unsupported packaged resource type {path}"))
        })?;
        if mime != expected {
            return Err(RuntimeError::FabappPolicy(format!(
                "MIME mismatch for {path}: expected {expected}"
            )));
        }
        files.insert(
            path.clone(),
            FabappFileRecord {
                sha256: format!("{:x}", Sha256::digest(data)),
                bytes: data.len() as u64,
                mime: mime.clone(),
            },
        );
    }

    let mut manifest = FabappManifest {
        manifest_version: 1,
        package_class: Some(FabappPackageClass::LocalGenerated),
        app_id: app_id.to_string(),
        publisher_id: publisher_id.to_string(),
        version: version.to_string(),
        display_name: display_name.to_string(),
        entrypoint: entrypoint.to_string(),
        runtime: "web".into(),
        min_host_api: FABAPP_HOST_API_V1,
        offline_core: true,
        requested_capabilities,
        network_policy: FabappNetworkPolicy {
            mode: "deny".into(),
            allowed_origins: Vec::new(),
        },
        files,
        local_author: Some(FabappLocalAuthorBinding {
            account_scope: "0".repeat(64),
            draft_digest: "0".repeat(64),
            user_approval_id: "draft".into(),
            creation_transaction_id: "draft".into(),
        }),
        signature: FabappSignature {
            algorithm: "Ed25519".into(),
            key_id: "draft".into(),
            value: "AA".into(),
        },
    };
    validate_fabapp_manifest(&manifest)?;
    manifest.local_author = None;
    manifest.signature = FabappSignature {
        algorithm: "Ed25519".into(),
        key_id: String::new(),
        value: String::new(),
    };
    Ok(manifest)
}

pub fn fabapp_signing_payload(manifest: &FabappManifest) -> Result<Vec<u8>, RuntimeError> {
    let mut unsigned = serde_json::to_value(manifest)
        .map_err(|error| RuntimeError::FabappManifest(error.to_string()))?;
    let object = unsigned
        .as_object_mut()
        .ok_or_else(|| RuntimeError::FabappManifest("manifest must be object".into()))?;
    object.remove("signature");
    let canonical = jcs_canonicalize(&unsigned)?;
    let mut message = Vec::with_capacity(FABAPP_SIGNATURE_DOMAIN.len() + canonical.len());
    message.extend_from_slice(FABAPP_SIGNATURE_DOMAIN);
    message.extend_from_slice(canonical.as_bytes());
    Ok(message)
}

pub fn build_fabapp_package(
    manifest: &FabappManifest,
    resources: &BTreeMap<String, (String, Vec<u8>)>,
) -> Result<Vec<u8>, RuntimeError> {
    validate_fabapp_manifest(manifest)?;
    if resources.len() != manifest.files.len()
        || resources.keys().any(|path| !manifest.files.contains_key(path))
        || manifest.files.keys().any(|path| !resources.contains_key(path))
    {
        return Err(RuntimeError::FabappPolicy(
            "package resources must exactly match manifest file table".into(),
        ));
    }
    for (path, record) in &manifest.files {
        let (mime, data) = resources
            .get(path)
            .ok_or_else(|| RuntimeError::FabappPolicy(format!("missing manifest file {path}")))?;
        if mime != &record.mime
            || data.len() as u64 != record.bytes
            || format!("{:x}", Sha256::digest(data)) != record.sha256
        {
            return Err(RuntimeError::FabappPolicy(format!(
                "package resource changed after approval: {path}"
            )));
        }
    }

    let cursor = Cursor::new(Vec::<u8>::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    writer
        .start_file("manifest.json", options)
        .map_err(|error| RuntimeError::FabappArchive(error.to_string()))?;
    writer
        .write_all(
            &serde_json::to_vec(manifest)
                .map_err(|error| RuntimeError::FabappManifest(error.to_string()))?,
        )
        .map_err(RuntimeError::Io)?;
    for (path, (_, data)) in resources {
        writer
            .start_file(path, options)
            .map_err(|error| RuntimeError::FabappArchive(error.to_string()))?;
        writer.write_all(data).map_err(RuntimeError::Io)?;
    }
    let bytes = writer
        .finish()
        .map_err(|error| RuntimeError::FabappArchive(error.to_string()))?
        .into_inner();
    if bytes.len() > FABAPP_MAX_COMPRESSED_BYTES {
        return Err(RuntimeError::FabappPolicy(
            "package compressed size exceeds limit".into(),
        ));
    }
    Ok(bytes)
}

pub fn verify_fabapp_bytes(
    bytes: &[u8],
    trust: &FabappTrustContext,
) -> Result<VerifiedFabappPackage, RuntimeError> {
    if bytes.is_empty() || bytes.len() > FABAPP_MAX_COMPRESSED_BYTES {
        return Err(RuntimeError::FabappPolicy(
            "package compressed size exceeds limit".into(),
        ));
    }
    let package_sha256 = format!("{:x}", Sha256::digest(bytes));
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| RuntimeError::FabappArchive(error.to_string()))?;
    if archive.len() == 0 || archive.len() > FABAPP_MAX_FILES.saturating_add(1) {
        return Err(RuntimeError::FabappPolicy(
            "package file count exceeds limit".into(),
        ));
    }

    let mut expanded_total = 0_u64;
    let mut archive_files = BTreeMap::<String, Vec<u8>>::new();
    let mut normalized_names = BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| RuntimeError::FabappArchive(error.to_string()))?;
        let name = std::str::from_utf8(entry.name_raw())
            .map_err(|_| RuntimeError::FabappPolicy("package paths must be UTF-8".into()))?
            .to_string();
        validate_fabapp_path(&name)?;
        if entry.is_dir() {
            return Err(RuntimeError::FabappPolicy(
                "explicit directory entries are not allowed".into(),
            ));
        }
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != 0o100000 {
                return Err(RuntimeError::FabappPolicy(
                    "links and special files are not allowed".into(),
                ));
            }
            if mode & 0o111 != 0 {
                return Err(RuntimeError::FabappPolicy(
                    "executable permission bits are not allowed".into(),
                ));
            }
        }
        let normalized = name.to_ascii_lowercase();
        if !normalized_names.insert(normalized) {
            return Err(RuntimeError::FabappPolicy(
                "duplicate or case-colliding package path".into(),
            ));
        }
        expanded_total = expanded_total
            .checked_add(entry.size())
            .ok_or_else(|| RuntimeError::FabappPolicy("expanded size overflow".into()))?;
        if expanded_total > FABAPP_MAX_EXPANDED_BYTES {
            return Err(RuntimeError::FabappPolicy(
                "expanded package size exceeds limit".into(),
            ));
        }
        let compressed = entry.compressed_size();
        if entry.size() > 0
            && (compressed == 0
                || entry.size()
                    > compressed.saturating_mul(FABAPP_MAX_INFLATION_RATIO))
        {
            return Err(RuntimeError::FabappPolicy(
                "package entry inflation ratio exceeds limit".into(),
            ));
        }
        let entry_size = entry.size();
        let capacity = usize::try_from(entry_size)
            .map_err(|_| RuntimeError::FabappPolicy("entry size overflow".into()))?;
        let mut data = Vec::with_capacity(capacity.min(4 * 1024 * 1024));
        entry
            .take(entry_size.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(RuntimeError::Io)?;
        if data.len() as u64 != entry_size {
            return Err(RuntimeError::FabappPolicy(
                "package entry size changed while reading".into(),
            ));
        }
        archive_files.insert(name, data);
    }

    let manifest_bytes = archive_files
        .remove("manifest.json")
        .ok_or_else(|| RuntimeError::FabappManifest("root manifest.json is required".into()))?;
    let manifest: FabappManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| RuntimeError::FabappManifest(error.to_string()))?;
    validate_fabapp_manifest(&manifest)?;

    if archive_files.len() != manifest.files.len()
        || archive_files.keys().any(|path| !manifest.files.contains_key(path))
        || manifest.files.keys().any(|path| !archive_files.contains_key(path))
    {
        return Err(RuntimeError::FabappPolicy(
            "archive files must exactly match manifest file table".into(),
        ));
    }

    for (path, record) in &manifest.files {
        let data = archive_files
            .get(path)
            .ok_or_else(|| RuntimeError::FabappPolicy(format!("missing manifest file {path}")))?;
        if data.len() as u64 != record.bytes {
            return Err(RuntimeError::FabappPolicy(format!(
                "size mismatch for {path}"
            )));
        }
        let actual = format!("{:x}", Sha256::digest(data));
        if actual != record.sha256 {
            return Err(RuntimeError::FabappPolicy(format!(
                "digest mismatch for {path}"
            )));
        }
        let expected_mime = fabapp_expected_mime(path).ok_or_else(|| {
            RuntimeError::FabappPolicy(format!("unsupported packaged resource type {path}"))
        })?;
        if record.mime != expected_mime {
            return Err(RuntimeError::FabappPolicy(format!(
                "MIME mismatch for {path}: expected {expected_mime}"
            )));
        }
    }

    let draft_digest = compute_fabapp_draft_digest(&manifest)?;
    verify_fabapp_trust(&manifest, trust, &draft_digest)?;
    verify_fabapp_signature(&manifest, trust)?;

    Ok(VerifiedFabappPackage {
        manifest,
        package_sha256,
        draft_digest,
        files: archive_files,
    })
}

fn validate_fabapp_manifest(manifest: &FabappManifest) -> Result<(), RuntimeError> {
    if manifest.manifest_version != 1 {
        return Err(RuntimeError::FabappManifest(
            "unsupported manifestVersion".into(),
        ));
    }
    validate_fabapp_reverse_id(&manifest.app_id, "appId")?;
    validate_fabapp_reverse_id(&manifest.publisher_id, "publisherId")?;
    Version::parse(manifest.version.trim()).map_err(|_| {
        RuntimeError::FabappManifest("version must be strict semantic version".into())
    })?;
    if manifest.display_name.trim().is_empty() || manifest.display_name.chars().count() > 128 {
        return Err(RuntimeError::FabappManifest(
            "displayName is missing or too long".into(),
        ));
    }
    if manifest.runtime != "web" {
        return Err(RuntimeError::FabappManifest(
            "runtime must be web".into(),
        ));
    }
    if manifest.min_host_api == 0 || manifest.min_host_api > FABAPP_HOST_API_V1 {
        return Err(RuntimeError::FabappManifest(
            "minHostApi is unsupported".into(),
        ));
    }
    validate_fabapp_path(&manifest.entrypoint)?;
    if manifest.entrypoint == "manifest.json" || !manifest.files.contains_key(&manifest.entrypoint) {
        return Err(RuntimeError::FabappManifest(
            "entrypoint must name a listed package resource".into(),
        ));
    }
    if fabapp_expected_mime(&manifest.entrypoint) != Some("text/html") {
        return Err(RuntimeError::FabappManifest(
            "entrypoint must be an HTML resource".into(),
        ));
    }
    if manifest.files.is_empty() || manifest.files.len() > FABAPP_MAX_FILES {
        return Err(RuntimeError::FabappManifest(
            "files table is empty or too large".into(),
        ));
    }

    let mut normalized_paths = BTreeSet::new();
    for (path, record) in &manifest.files {
        validate_fabapp_path(path)?;
        if path == "manifest.json" {
            return Err(RuntimeError::FabappManifest(
                "manifest.json may not appear in files table".into(),
            ));
        }
        if !normalized_paths.insert(path.to_ascii_lowercase()) {
            return Err(RuntimeError::FabappManifest(
                "files table contains case-colliding paths".into(),
            ));
        }
        if record.sha256.len() != 64
            || !record.sha256.bytes().all(|byte| {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            })
        {
            return Err(RuntimeError::FabappManifest(format!(
                "invalid lowercase sha256 for {path}"
            )));
        }
        if record.bytes > FABAPP_MAX_EXPANDED_BYTES {
            return Err(RuntimeError::FabappManifest(format!(
                "declared file size too large for {path}"
            )));
        }
    }

    let mut capabilities = BTreeSet::new();
    for capability in &manifest.requested_capabilities {
        if capability.is_empty()
            || capability.len() > 128
            || !capability.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
            })
            || !capabilities.insert(capability)
        {
            return Err(RuntimeError::FabappManifest(
                "requestedCapabilities contains an invalid or duplicate value".into(),
            ));
        }
    }

    match manifest.network_policy.mode.as_str() {
        "deny" => {
            if !manifest.network_policy.allowed_origins.is_empty() {
                return Err(RuntimeError::FabappManifest(
                    "deny networkPolicy may not carry allowedOrigins".into(),
                ));
            }
        }
        "allowlist" => {
            let mut origins = BTreeSet::new();
            for origin in &manifest.network_policy.allowed_origins {
                let url = Url::parse(origin)
                    .map_err(|_| RuntimeError::FabappManifest("invalid allowedOrigin".into()))?;
                if url.scheme() != "https"
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                    || url.path() != "/"
                    || !origins.insert(origin)
                {
                    return Err(RuntimeError::FabappManifest(
                        "allowedOrigins must be unique HTTPS origins".into(),
                    ));
                }
            }
        }
        _ => {
            return Err(RuntimeError::FabappManifest(
                "networkPolicy.mode must be deny or allowlist".into(),
            ));
        }
    }

    if manifest.signature.algorithm != "Ed25519"
        || manifest.signature.key_id.trim().is_empty()
        || manifest.signature.value.trim().is_empty()
    {
        return Err(RuntimeError::FabappManifest(
            "signature must use Ed25519 with keyId/value".into(),
        ));
    }

    match manifest.package_class() {
        FabappPackageClass::Catalog => {
            if manifest.local_author.is_some() {
                return Err(RuntimeError::FabappManifest(
                    "catalog package may not carry localAuthor".into(),
                ));
            }
        }
        FabappPackageClass::LocalGenerated => {
            let binding = manifest.local_author.as_ref().ok_or_else(|| {
                RuntimeError::FabappManifest(
                    "local-generated package requires localAuthor binding".into(),
                )
            })?;
            validate_hex_digest(&binding.account_scope, "local author accountScope")?;
            validate_hex_digest(&binding.draft_digest, "local author draftDigest")?;
            validate_identifier(&binding.user_approval_id, "userApprovalId")?;
            validate_identifier(
                &binding.creation_transaction_id,
                "creationTransactionId",
            )?;
            for capability in &manifest.requested_capabilities {
                let lowered = capability.to_ascii_lowercase();
                if lowered.contains("shell")
                    || lowered.contains("host")
                    || lowered.contains("computer")
                    || lowered.contains("mcp")
                    || lowered.contains("native")
                    || lowered.contains("payment")
                    || lowered.contains("wallet")
                    || lowered.contains("network")
                {
                    return Err(RuntimeError::FabappPolicy(format!(
                        "local-generated package may not request privileged capability {capability}"
                    )));
                }
            }
            if manifest.network_policy.mode != "deny"
                || !manifest.network_policy.allowed_origins.is_empty()
            {
                return Err(RuntimeError::FabappPolicy(
                    "local-generated package network egress is denied in v1".into(),
                ));
            }
        }
    }
    Ok(())
}

fn verify_fabapp_trust(
    manifest: &FabappManifest,
    trust: &FabappTrustContext,
    computed_draft_digest: &str,
) -> Result<(), RuntimeError> {
    match (manifest.package_class(), trust) {
        (FabappPackageClass::Catalog, FabappTrustContext::Catalog(trust)) => {
            if manifest.publisher_id != trust.publisher_id
                || manifest.app_id != trust.app_id
                || manifest.signature.key_id != trust.key_id
            {
                return Err(RuntimeError::FabappTrust(
                    "catalog publisher/app/key binding mismatch".into(),
                ));
            }
        }
        (
            FabappPackageClass::LocalGenerated,
            FabappTrustContext::LocalGenerated(trust),
        ) => {
            let binding = manifest.local_author.as_ref().ok_or_else(|| {
                RuntimeError::FabappTrust("local author binding missing".into())
            })?;
            if manifest.publisher_id != trust.publisher_id
                || manifest.app_id != trust.reserved_app_id
                || manifest.signature.key_id != trust.key_id
                || binding.account_scope != trust.account_scope
                || binding.draft_digest != trust.approved_draft_digest
                || binding.draft_digest != computed_draft_digest
                || binding.user_approval_id != trust.user_approval_id
                || binding.creation_transaction_id != trust.creation_transaction_id
            {
                return Err(RuntimeError::FabappTrust(
                    "local author account/draft/approval/transaction binding mismatch".into(),
                ));
            }
        }
        (FabappPackageClass::Catalog, FabappTrustContext::LocalGenerated(_))
        | (FabappPackageClass::LocalGenerated, FabappTrustContext::Catalog(_)) => {
            return Err(RuntimeError::FabappTrust(
                "package class and trust root are disjoint".into(),
            ));
        }
    }
    Ok(())
}

fn verify_fabapp_signature(
    manifest: &FabappManifest,
    trust: &FabappTrustContext,
) -> Result<(), RuntimeError> {
    let key_bytes = match trust {
        FabappTrustContext::Catalog(trust) => trust.public_key,
        FabappTrustContext::LocalGenerated(trust) => trust.public_key,
    };
    let key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|_| RuntimeError::FabappTrust("invalid Ed25519 public key".into()))?;
    let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(manifest.signature.value.as_bytes())
        .map_err(|_| RuntimeError::FabappSignature)?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| RuntimeError::FabappSignature)?;
    let message = fabapp_signing_payload(manifest)?;
    key.verify(&message, &signature)
        .map_err(|_| RuntimeError::FabappSignature)
}

fn jcs_canonicalize(value: &serde_json::Value) -> Result<String, RuntimeError> {
    fn write_value(
        value: &serde_json::Value,
        output: &mut String,
    ) -> Result<(), RuntimeError> {
        match value {
            serde_json::Value::Null => output.push_str("null"),
            serde_json::Value::Bool(value) => {
                output.push_str(if *value { "true" } else { "false" })
            }
            serde_json::Value::Number(value) => {
                if value.as_i64().is_none() && value.as_u64().is_none() {
                    return Err(RuntimeError::FabappManifest(
                        "floating-point manifest numbers are not allowed".into(),
                    ));
                }
                output.push_str(&value.to_string());
            }
            serde_json::Value::String(value) => output.push_str(
                &serde_json::to_string(value)
                    .map_err(|error| RuntimeError::FabappManifest(error.to_string()))?,
            ),
            serde_json::Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    write_value(value, output)?;
                }
                output.push(']');
            }
            serde_json::Value::Object(values) => {
                output.push('{');
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort();
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    output.push_str(
                        &serde_json::to_string(key)
                            .map_err(|error| RuntimeError::FabappManifest(error.to_string()))?,
                    );
                    output.push(':');
                    write_value(&values[key], output)?;
                }
                output.push('}');
            }
        }
        Ok(())
    }

    let mut output = String::new();
    write_value(value, &mut output)?;
    Ok(output)
}

fn validate_fabapp_reverse_id(
    value: &str,
    kind: &'static str,
) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.len() > 128
        || value.starts_with('.')
        || value.ends_with('.')
        || value.split('.').count() < 2
        || value.split('.').any(|segment| {
            segment.is_empty()
                || !segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'-'
                })
                || segment.starts_with('-')
                || segment.ends_with('-')
        })
    {
        return Err(RuntimeError::InvalidIdentifier {
            kind,
            value: value.to_string(),
        });
    }
    Ok(())
}

fn validate_hex_digest(value: &str, label: &str) -> Result<(), RuntimeError> {
    if value.len() != 64
        || !value.bytes().all(|byte| {
            byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
        })
    {
        return Err(RuntimeError::FabappManifest(format!(
            "{label} must be lowercase SHA-256"
        )));
    }
    Ok(())
}

fn validate_fabapp_path(path: &str) -> Result<(), RuntimeError> {
    if path.is_empty()
        || path.len() > 1024
        || !path.is_ascii()
        || path.bytes().any(|byte| byte < 0x20 || byte == b'\\' || byte == b':')
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
        || path.split('/').any(|segment| {
            segment.is_empty() || segment == "." || segment == ".."
        })
    {
        return Err(RuntimeError::FabappPolicy(format!(
            "unsafe or ambiguous package path {path:?}"
        )));
    }
    Ok(())
}

fn fabapp_expected_mime(path: &str) -> Option<&'static str> {
    let extension = Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    match extension.as_str() {
        "html" | "htm" => Some("text/html"),
        "css" => Some("text/css"),
        "js" | "mjs" => Some("text/javascript"),
        "json" => Some("application/json"),
        "wasm" => Some("application/wasm"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "txt" => Some("text/plain"),
        "woff" => Some("font/woff"),
        "woff2" => Some("font/woff2"),
        "ico" => Some("image/x-icon"),
        _ => None,
    }
}

fn runtime_atomic_write_json<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), RuntimeError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| RuntimeError::FabappInstall(error.to_string()))?;
    let temp = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    fs::write(&temp, bytes)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temp, path)?;
    Ok(())
}

fn recover_fabapp_install_journal(plugin_root: &Path) -> Result<(), RuntimeError> {
    let path = plugin_root.join("install-journal.json");
    if !path.is_file() {
        return Ok(());
    }
    let mut journal: FabappInstallJournal = serde_json::from_slice(&fs::read(&path)?)
        .map_err(|error| RuntimeError::FabappInstall(error.to_string()))?;
    if journal.schema_version != 1 || journal.state == "completed" {
        return Ok(());
    }
    if let Some(relative) = journal.staging_relative.as_deref() {
        let relative = safe_relative_path(relative)?;
        let staging = plugin_root.join(relative);
        if staging.starts_with(plugin_root) && staging.is_dir() {
            fs::remove_dir_all(staging)?;
        }
    }
    journal.state = "recovered-aborted".into();
    journal.staging_relative = None;
    runtime_atomic_write_json(&path, &journal)
}

fn reserve_local_generated_approval(
    plugin_root: &Path,
    verified: &VerifiedFabappPackage,
    trust: &LocalGeneratedFabappTrust,
) -> Result<PathBuf, RuntimeError> {
    let approvals = plugin_root.join("approvals");
    fs::create_dir_all(&approvals)?;
    validate_identifier(&trust.user_approval_id, "userApprovalId")?;
    let path = approvals.join(format!("{}.json", trust.user_approval_id));
    let expected = LocalApprovalRecord {
        schema_version: 1,
        account_scope: trust.account_scope.clone(),
        app_id: trust.reserved_app_id.clone(),
        draft_digest: trust.approved_draft_digest.clone(),
        user_approval_id: trust.user_approval_id.clone(),
        creation_transaction_id: trust.creation_transaction_id.clone(),
        package_sha256: verified.package_sha256.clone(),
        state: "reserved".into(),
    };
    if path.is_file() {
        let existing: LocalApprovalRecord = serde_json::from_slice(&fs::read(&path)?)
            .map_err(|error| RuntimeError::FabappInstall(error.to_string()))?;
        let same_transaction = existing.schema_version == 1
            && existing.account_scope == expected.account_scope
            && existing.app_id == expected.app_id
            && existing.draft_digest == expected.draft_digest
            && existing.user_approval_id == expected.user_approval_id
            && existing.creation_transaction_id == expected.creation_transaction_id
            && existing.package_sha256 == expected.package_sha256;
        if same_transaction && existing.state == "reserved" {
            return Ok(path);
        }
        return Err(RuntimeError::FabappApprovalReplay);
    }
    let bytes = serde_json::to_vec_pretty(&expected)
        .map_err(|error| RuntimeError::FabappInstall(error.to_string()))?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                RuntimeError::FabappApprovalReplay
            } else {
                RuntimeError::Io(error)
            }
        })?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(path)
}

fn complete_local_generated_approval(
    path: &Path,
) -> Result<(), RuntimeError> {
    let mut record: LocalApprovalRecord = serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| RuntimeError::FabappInstall(error.to_string()))?;
    if record.state != "reserved" {
        return Err(RuntimeError::FabappApprovalReplay);
    }
    record.state = "completed".into();
    runtime_atomic_write_json(path, &record)
}

fn verify_installed_fabapp_dir(
    root: &Path,
    verified: &VerifiedFabappPackage,
) -> Result<(), RuntimeError> {
    for (path, record) in &verified.manifest.files {
        let relative = safe_relative_path(path)?;
        let file = root.join(relative);
        if !file.is_file() {
            return Err(RuntimeError::FabappInstall(format!(
                "immutable package file missing: {path}"
            )));
        }
        let bytes = fs::read(file)?;
        if bytes.len() as u64 != record.bytes
            || format!("{:x}", Sha256::digest(&bytes)) != record.sha256
        {
            return Err(RuntimeError::FabappInstall(format!(
                "immutable package file changed: {path}"
            )));
        }
    }
    Ok(())
}


#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPluginPointer {
    pub plugin_id: String,
    pub version: String,
    pub artifact_id: String,
    pub artifact_sha256: String,
    pub runtime: String,
    #[serde(default)]
    pub entry: Option<String>,
    #[serde(default)]
    pub requested_permissions: Vec<String>,
    pub installed_path: String,
}

pub struct PluginInstaller {
    root: PathBuf,
    resolver: ArtifactResolver,
}

impl PluginInstaller {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, RuntimeError> {
        Ok(Self {
            root: root.into(),
            resolver: ArtifactResolver::new()?,
        })
    }

    pub fn with_resolver(root: impl Into<PathBuf>, resolver: ArtifactResolver) -> Self {
        Self {
            root: root.into(),
            resolver,
        }
    }

    pub fn install(
        &self,
        release: &ExternalReleaseManifest,
        platform: &str,
        preferred_runtimes: &[&str],
    ) -> Result<InstalledPluginPointer, RuntimeError> {
        self.install_with_commit_guard(release, platform, preferred_runtimes, || true)
    }

    pub fn install_with_commit_guard<F>(
        &self,
        release: &ExternalReleaseManifest,
        platform: &str,
        preferred_runtimes: &[&str],
        commit_guard: F,
    ) -> Result<InstalledPluginPointer, RuntimeError>
    where
        F: Fn() -> bool,
    {
        release.validate()?;
        let artifact = release.select_artifact(platform, preferred_runtimes)?;
        let bytes = self.resolver.download_verified(artifact)?;
        self.install_verified_bytes_with_commit_guard(
            release,
            artifact,
            &bytes,
            commit_guard,
        )
    }

    pub fn install_verified_bytes(
        &self,
        release: &ExternalReleaseManifest,
        artifact: &ReleaseArtifact,
        bytes: &[u8],
    ) -> Result<InstalledPluginPointer, RuntimeError> {
        self.install_verified_bytes_with_commit_guard(release, artifact, bytes, || true)
    }

    fn install_verified_bytes_with_commit_guard<F>(
        &self,
        release: &ExternalReleaseManifest,
        artifact: &ReleaseArtifact,
        bytes: &[u8],
        commit_guard: F,
    ) -> Result<InstalledPluginPointer, RuntimeError>
    where
        F: Fn() -> bool,
    {
        release.validate()?;
        if !release
            .artifacts
            .iter()
            .any(|candidate| candidate.id == artifact.id)
        {
            return Err(RuntimeError::InvalidRelease(
                "artifact does not belong to release".into(),
            ));
        }
        verify_bytes(bytes, &artifact.sha256, artifact.size)?;
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        if let Some(binding) = release
            .fabapp_trust
            .as_ref()
            .filter(|binding| binding.artifact_id == artifact.id)
        {
            let trust = FabappTrustContext::Catalog(binding.catalog_trust()?);
            // Verify and reconcile every release/package identity field before
            // the content-addressed installer is allowed to create or activate
            // anything. install_fabapp_bytes_with_commit_guard deliberately
            // verifies again immediately before writing.
            let verified = verify_fabapp_bytes(bytes, &trust)?;
            if verified.manifest.app_id != release.plugin_id
                || verified.manifest.version != release.version
                || !verified.package_sha256.eq_ignore_ascii_case(&artifact.sha256)
                || verified.manifest.requested_capabilities != release.permissions
            {
                return Err(RuntimeError::InvalidRelease(
                    "verified .fabapp identity/version/digest/capabilities do not match release".into(),
                ));
            }
            return self.install_fabapp_bytes_with_commit_guard(
                bytes,
                &trust,
                &commit_guard,
            );
        }
        let plugin_root = self.root.join(&release.plugin_id);
        if let Some(active) = self.active(&release.plugin_id)?
            && compare_install_versions(&release.version, &active.version) == Ordering::Less
        {
            return Err(RuntimeError::DowngradeBlocked {
                current: active.version,
                candidate: release.version.clone(),
            });
        }
        let versions_root = plugin_root.join("versions");
        fs::create_dir_all(&versions_root).map_err(RuntimeError::Io)?;
        let final_dir = versions_root.join(&release.version).join(&artifact.id);
        if final_dir.exists() {
            if !commit_guard() {
                return Err(RuntimeError::StaleAccountContext);
            }
            let pointer = InstalledPluginPointer {
                plugin_id: release.plugin_id.clone(),
                version: release.version.clone(),
                artifact_id: artifact.id.clone(),
                artifact_sha256: artifact.sha256.to_ascii_lowercase(),
                runtime: artifact.runtime.clone(),
                entry: artifact.entry.clone(),
                requested_permissions: release.permissions.clone(),
                installed_path: final_dir.to_string_lossy().into_owned(),
            };
            self.activate_with_commit_guard(&plugin_root, &pointer, &commit_guard)?;
            return Ok(pointer);
        }

        let staging = plugin_root
            .join(".staging")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&staging).map_err(RuntimeError::Io)?;
        let extraction = extract_artifact(bytes, &artifact.format, &staging);
        if let Err(error) = extraction {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        if let ArtifactSource::Npm { registry, .. } = &artifact.source
            && let Err(error) = normalize_npm_package_root(&staging).and_then(|_| {
                ArtifactResolver::new()?.install_npm_dependencies(&staging, registry)?;
                Ok(())
            })
        {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        if let Some(entry) = &artifact.entry {
            let entry = safe_relative_path(entry)?;
            if !staging.join(entry).exists() {
                let _ = fs::remove_dir_all(&staging);
                return Err(RuntimeError::MissingEntry(artifact.entry.clone().unwrap()));
            }
        }
        if !commit_guard() {
            let _ = fs::remove_dir_all(&staging);
            return Err(RuntimeError::StaleAccountContext);
        }
        if let Some(parent) = final_dir.parent() {
            fs::create_dir_all(parent).map_err(RuntimeError::Io)?;
        }
        fs::rename(&staging, &final_dir).map_err(RuntimeError::Io)?;
        let pointer = InstalledPluginPointer {
            plugin_id: release.plugin_id.clone(),
            version: release.version.clone(),
            artifact_id: artifact.id.clone(),
            artifact_sha256: artifact.sha256.to_ascii_lowercase(),
            runtime: artifact.runtime.clone(),
            entry: artifact.entry.clone(),
            requested_permissions: release.permissions.clone(),
            installed_path: final_dir.to_string_lossy().into_owned(),
        };
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        self.activate_with_commit_guard(&plugin_root, &pointer, &commit_guard)?;
        Ok(pointer)
    }

    pub fn install_fabapp_bytes(
        &self,
        bytes: &[u8],
        trust: &FabappTrustContext,
    ) -> Result<InstalledPluginPointer, RuntimeError> {
        self.install_fabapp_bytes_with_commit_guard(bytes, trust, || true)
    }

    pub fn install_fabapp_bytes_with_commit_guard<F>(
        &self,
        bytes: &[u8],
        trust: &FabappTrustContext,
        commit_guard: F,
    ) -> Result<InstalledPluginPointer, RuntimeError>
    where
        F: Fn() -> bool,
    {
        let verified = verify_fabapp_bytes(bytes, trust)?;
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        let plugin_root = self.root.join(&verified.manifest.app_id);
        fs::create_dir_all(&plugin_root)?;
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        recover_fabapp_install_journal(&plugin_root)?;

        if let Some(active) = self.active(&verified.manifest.app_id)?
            && compare_install_versions(&verified.manifest.version, &active.version) == Ordering::Less
        {
            return Err(RuntimeError::DowngradeBlocked {
                current: active.version,
                candidate: verified.manifest.version.clone(),
            });
        }

        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        let approval_path = match trust {
            FabappTrustContext::LocalGenerated(local) => {
                Some(reserve_local_generated_approval(&plugin_root, &verified, local)?)
            }
            FabappTrustContext::Catalog(_) => None,
        };

        let transaction_id = match trust {
            FabappTrustContext::LocalGenerated(local) => {
                local.creation_transaction_id.clone()
            }
            FabappTrustContext::Catalog(_) => {
                format!("catalog-{}", verified.package_sha256)
            }
        };
        let staging_relative = format!(".fabapp-staging/{}", Uuid::new_v4());
        let staging = plugin_root.join(&staging_relative);
        let journal_path = plugin_root.join("install-journal.json");
        let mut journal = FabappInstallJournal {
            schema_version: 1,
            transaction_id,
            package_sha256: verified.package_sha256.clone(),
            state: "verified".into(),
            staging_relative: Some(staging_relative.clone()),
        };
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        runtime_atomic_write_json(&journal_path, &journal)?;

        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        fs::create_dir_all(&staging)?;
        for (path, data) in &verified.files {
            let relative = safe_relative_path(path)?;
            let target = staging.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(target, data)?;
        }
        verify_installed_fabapp_dir(&staging, &verified)?;
        if !commit_guard() {
            let _ = fs::remove_dir_all(&staging);
            return Err(RuntimeError::StaleAccountContext);
        }
        journal.state = "staged".into();
        runtime_atomic_write_json(&journal_path, &journal)?;

        let final_dir = plugin_root
            .join("versions")
            .join(&verified.manifest.version)
            .join(&verified.package_sha256);
        let mut moved_new_content = false;
        if final_dir.is_dir() {
            verify_installed_fabapp_dir(&final_dir, &verified)?;
            fs::remove_dir_all(&staging)?;
        } else {
            if !commit_guard() {
                let _ = fs::remove_dir_all(&staging);
                return Err(RuntimeError::StaleAccountContext);
            }
            if let Some(parent) = final_dir.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&staging, &final_dir)?;
            moved_new_content = true;
        }
        if !commit_guard() {
            if moved_new_content {
                let _ = fs::remove_dir_all(&final_dir);
            }
            return Err(RuntimeError::StaleAccountContext);
        }
        journal.state = "content-moved".into();
        journal.staging_relative = None;
        runtime_atomic_write_json(&journal_path, &journal)?;

        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        let metadata_root = plugin_root.join(".fabapp-manifests");
        fs::create_dir_all(&metadata_root)?;
        runtime_atomic_write_json(
            &metadata_root.join(format!("{}.json", verified.package_sha256)),
            &verified.manifest,
        )?;

        let pointer = InstalledPluginPointer {
            plugin_id: verified.manifest.app_id.clone(),
            version: verified.manifest.version.clone(),
            artifact_id: format!("fabapp-{}", verified.package_sha256),
            artifact_sha256: verified.package_sha256.clone(),
            runtime: "local-web".into(),
            entry: Some(verified.manifest.entrypoint.clone()),
            requested_permissions: verified.manifest.requested_capabilities.clone(),
            installed_path: final_dir.to_string_lossy().into_owned(),
        };
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        self.activate_with_commit_guard(&plugin_root, &pointer, &commit_guard)?;
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        journal.state = "completed".into();
        runtime_atomic_write_json(&journal_path, &journal)?;
        if let Some(path) = approval_path {
            complete_local_generated_approval(&path)?;
        }
        Ok(pointer)
    }

    fn activate(
        &self,
        plugin_root: &Path,
        pointer: &InstalledPluginPointer,
    ) -> Result<(), RuntimeError> {
        let commit_guard = || true;
        self.activate_with_commit_guard(plugin_root, pointer, &commit_guard)
    }

    fn activate_with_commit_guard<F>(
        &self,
        plugin_root: &Path,
        pointer: &InstalledPluginPointer,
        commit_guard: &F,
    ) -> Result<(), RuntimeError>
    where
        F: Fn() -> bool,
    {
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        fs::create_dir_all(plugin_root).map_err(RuntimeError::Io)?;
        let active = plugin_root.join("active.json");
        if active.is_file() {
            let current: InstalledPluginPointer = serde_json::from_slice(&fs::read(&active)?)
                .map_err(|error| RuntimeError::InvalidRelease(error.to_string()))?;
            if current != *pointer {
                let previous = plugin_root.join("previous-active.json");
                let bytes = serde_json::to_vec_pretty(&current)
                    .map_err(|error| RuntimeError::InvalidRelease(error.to_string()))?;
                let temporary =
                    plugin_root.join(format!("previous-active.{}.json", Uuid::new_v4()));
                if !commit_guard() {
                    return Err(RuntimeError::StaleAccountContext);
                }
                fs::write(&temporary, bytes).map_err(RuntimeError::Io)?;
                if !commit_guard() {
                    let _ = fs::remove_file(&temporary);
                    return Err(RuntimeError::StaleAccountContext);
                }
                #[cfg(windows)]
                if previous.exists() {
                    fs::remove_file(&previous).map_err(RuntimeError::Io)?;
                }
                fs::rename(&temporary, &previous).map_err(RuntimeError::Io)?;
            }
        }
        let bytes = serde_json::to_vec_pretty(pointer)
            .map_err(|error| RuntimeError::InvalidRelease(error.to_string()))?;
        let temp = plugin_root.join(format!("active.{}.json", Uuid::new_v4()));
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        fs::write(&temp, bytes).map_err(RuntimeError::Io)?;
        if !commit_guard() {
            let _ = fs::remove_file(&temp);
            return Err(RuntimeError::StaleAccountContext);
        }
        #[cfg(windows)]
        if active.exists() {
            fs::remove_file(&active).map_err(RuntimeError::Io)?;
        }
        fs::rename(&temp, &active).map_err(RuntimeError::Io)?;
        Ok(())
    }

    /// Switch back to the version that was active immediately before the
    /// latest successful activation. This is intentionally explicit: normal
    /// installs remain monotonic and cannot silently downgrade.
    pub fn rollback(
        &self,
        plugin_id: &str,
    ) -> Result<Option<InstalledPluginPointer>, RuntimeError> {
        self.rollback_with_commit_guard(plugin_id, || true)
    }

    pub fn rollback_with_commit_guard<F>(
        &self,
        plugin_id: &str,
        commit_guard: F,
    ) -> Result<Option<InstalledPluginPointer>, RuntimeError>
    where
        F: Fn() -> bool,
    {
        validate_identifier(plugin_id, "pluginId")?;
        let plugin_root = self.root.join(plugin_id);
        let previous_path = plugin_root.join("previous-active.json");
        let source = match fs::read(&previous_path) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(RuntimeError::Io(error)),
        };
        let previous: InstalledPluginPointer = serde_json::from_slice(&source)
            .map_err(|error| RuntimeError::InvalidRelease(error.to_string()))?;
        if !Path::new(&previous.installed_path).is_dir() {
            return Err(RuntimeError::InvalidRelease(
                "previous active plugin files are missing".into(),
            ));
        }
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        self.activate_with_commit_guard(&plugin_root, &previous, &commit_guard)?;
        Ok(Some(previous))
    }

    pub fn active(&self, plugin_id: &str) -> Result<Option<InstalledPluginPointer>, RuntimeError> {
        validate_identifier(plugin_id, "pluginId")?;
        let path = self.root.join(plugin_id).join("active.json");
        let source = match fs::read(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(RuntimeError::Io(error)),
        };
        serde_json::from_slice(&source)
            .map(Some)
            .map_err(|error| RuntimeError::InvalidRelease(error.to_string()))
    }

    /// Remove only installed plugin code and installer-owned ledgers.
    ///
    /// Account-scoped permission grants and platform app/WebKit data live outside
    /// the plugin install root and are deliberately not touched by this owner.
    pub fn remove_code(&self, plugin_id: &str) -> Result<bool, RuntimeError> {
        self.remove_code_with_commit_guard(plugin_id, || true)
    }

    pub fn remove_code_with_commit_guard<F>(
        &self,
        plugin_id: &str,
        commit_guard: F,
    ) -> Result<bool, RuntimeError>
    where
        F: Fn() -> bool,
    {
        validate_identifier(plugin_id, "pluginId")?;
        let plugin_root = self.root.join(plugin_id);
        if !plugin_root.exists() {
            return Ok(false);
        }

        let mut installer_entries = Vec::new();
        for entry in fs::read_dir(&plugin_root).map_err(RuntimeError::Io)? {
            let entry = entry.map_err(RuntimeError::Io)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let installer_owned = matches!(
                name.as_ref(),
                "versions"
                    | "active.json"
                    | "previous-active.json"
                    | "install-journal.json"
                    | ".fabapp-manifests"
                    | ".staging"
                    | ".fabapp-staging"
                    | "approvals"
            ) || (name.starts_with("active.") && name.ends_with(".json"))
                || (name.starts_with("previous-active.") && name.ends_with(".json"));
            if installer_owned {
                installer_entries.push(entry.path());
            }
        }
        if installer_entries.is_empty() {
            return Ok(false);
        }
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }

        let tombstone = self
            .root
            .join(format!(".code-removal-{plugin_id}-{}", Uuid::new_v4()));
        fs::create_dir(&tombstone).map_err(RuntimeError::Io)?;
        let mut moved = Vec::<(PathBuf, PathBuf)>::new();

        let rollback = |moved: &[(PathBuf, PathBuf)]| {
            for (source, staged) in moved.iter().rev() {
                if staged.exists() {
                    let _ = fs::rename(staged, source);
                }
            }
            let _ = fs::remove_dir(&tombstone);
        };

        for source in installer_entries {
            if !commit_guard() {
                rollback(&moved);
                return Err(RuntimeError::StaleAccountContext);
            }
            let Some(file_name) = source.file_name() else {
                rollback(&moved);
                return Err(RuntimeError::InvalidRelease(
                    "plugin installer entry has no file name".into(),
                ));
            };
            let staged = tombstone.join(file_name);
            if let Err(error) = fs::rename(&source, &staged) {
                rollback(&moved);
                return Err(RuntimeError::Io(error));
            }
            moved.push((source, staged));
        }

        if !commit_guard() {
            rollback(&moved);
            return Err(RuntimeError::StaleAccountContext);
        }

        fs::remove_dir_all(&tombstone).map_err(RuntimeError::Io)?;
        if plugin_root
            .read_dir()
            .map_err(RuntimeError::Io)?
            .next()
            .is_none()
        {
            fs::remove_dir(&plugin_root).map_err(RuntimeError::Io)?;
        }
        Ok(true)
    }

    pub fn uninstall(&self, plugin_id: &str) -> Result<bool, RuntimeError> {
        self.uninstall_with_commit_guard(plugin_id, || true)
    }

    pub fn uninstall_with_commit_guard<F>(
        &self,
        plugin_id: &str,
        commit_guard: F,
    ) -> Result<bool, RuntimeError>
    where
        F: Fn() -> bool,
    {
        validate_identifier(plugin_id, "pluginId")?;
        let plugin_root = self.root.join(plugin_id);
        if !plugin_root.exists() {
            return Ok(false);
        }
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        fs::remove_dir_all(plugin_root).map_err(RuntimeError::Io)?;
        Ok(true)
    }
}

/// Owns cleanup callbacks for one plugin instance. Every runtime adapter
/// (JS/WASM/MCP/native) registers timers, service registrations, workers,
/// subscriptions, child handles, and other resources here so unload is
/// deterministic and idempotent.
#[derive(Default)]
pub struct EffectScope {
    disposers: Vec<Box<dyn FnOnce() + Send + 'static>>,
    disposed: bool,
}

impl EffectScope {
    pub fn register(
        &mut self,
        disposer: impl FnOnce() + Send + 'static,
    ) -> Result<(), RuntimeError> {
        if self.disposed {
            return Err(RuntimeError::EffectScopeDisposed);
        }
        self.disposers.push(Box::new(disposer));
        Ok(())
    }

    pub fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        while let Some(disposer) = self.disposers.pop() {
            disposer();
        }
    }

    pub fn is_disposed(&self) -> bool {
        self.disposed
    }
}

impl Drop for EffectScope {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PluginState {
    Installed,
    Pending,
    Loading,
    Active,
    Failed,
    Unloading,
    Disposed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRequirement {
    pub service: String,
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceProvider {
    pub plugin_id: String,
    pub generation: u64,
}

#[derive(Debug, Default)]
pub struct ServiceRegistry {
    providers: HashMap<String, ServiceProvider>,
    next_generation: u64,
}

impl ServiceRegistry {
    pub fn provide(&mut self, service: impl Into<String>, plugin_id: impl Into<String>) -> u64 {
        self.next_generation = self.next_generation.saturating_add(1).max(1);
        let generation = self.next_generation;
        self.providers.insert(
            service.into(),
            ServiceProvider {
                plugin_id: plugin_id.into(),
                generation,
            },
        );
        generation
    }

    pub fn remove_provider(&mut self, plugin_id: &str) -> Vec<String> {
        let removed = self
            .providers
            .iter()
            .filter(|(_, provider)| provider.plugin_id == plugin_id)
            .map(|(service, _)| service.clone())
            .collect::<Vec<_>>();
        for service in &removed {
            self.providers.remove(service);
        }
        removed
    }

    pub fn remove_service(&mut self, service: &str, plugin_id: &str) -> bool {
        if self
            .providers
            .get(service)
            .is_some_and(|provider| provider.plugin_id == plugin_id)
        {
            self.providers.remove(service);
            true
        } else {
            false
        }
    }

    pub fn get(&self, service: &str) -> Option<&ServiceProvider> {
        self.providers.get(service)
    }
}

#[derive(Debug, Clone)]
pub struct PluginEntry {
    pub plugin_id: String,
    pub enabled: bool,
    pub state: PluginState,
    pub provides: Vec<String>,
    pub injects: Vec<ServiceRequirement>,
    observed_generations: BTreeMap<String, u64>,
}

impl PluginEntry {
    pub fn new(
        plugin_id: impl Into<String>,
        provides: Vec<String>,
        injects: Vec<ServiceRequirement>,
    ) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            enabled: true,
            state: PluginState::Installed,
            provides,
            injects,
            observed_generations: BTreeMap::new(),
        }
    }

    fn dependencies_ready(&self, services: &ServiceRegistry) -> bool {
        self.injects
            .iter()
            .filter(|requirement| requirement.required)
            .all(|requirement| services.get(&requirement.service).is_some())
    }

    fn dependency_epoch_changed(&self, services: &ServiceRegistry) -> bool {
        self.injects.iter().any(|requirement| {
            let current = services
                .get(&requirement.service)
                .map(|provider| provider.generation)
                .unwrap_or(0);
            self.observed_generations
                .get(&requirement.service)
                .copied()
                .unwrap_or(0)
                != current
        })
    }

    fn snapshot_dependencies(&mut self, services: &ServiceRegistry) {
        self.observed_generations = self
            .injects
            .iter()
            .map(|requirement| {
                (
                    requirement.service.clone(),
                    services
                        .get(&requirement.service)
                        .map(|provider| provider.generation)
                        .unwrap_or(0),
                )
            })
            .collect();
    }
}

#[derive(Debug, Default)]
pub struct PluginSupervisor {
    pub services: ServiceRegistry,
    pub entries: HashMap<String, PluginEntry>,
}

impl PluginSupervisor {
    pub fn insert(&mut self, entry: PluginEntry) {
        self.entries.insert(entry.plugin_id.clone(), entry);
    }

    /// Reconciles the graph to a stable state. Runtime-specific load/unload
    /// hooks intentionally live above this primitive; this method owns the
    /// deterministic state/service semantics shared by JS, WASM, MCP, and
    /// native runtimes.
    pub fn reconcile(&mut self) {
        // First unload entries whose provider epoch changed or disappeared.
        let to_unload = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.state == PluginState::Active
                    && (!entry.enabled
                        || !entry.dependencies_ready(&self.services)
                        || entry.dependency_epoch_changed(&self.services))
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in to_unload {
            self.services.remove_provider(&id);
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.state = if entry.enabled {
                    PluginState::Pending
                } else {
                    PluginState::Disposed
                };
            }
        }

        // Repeatedly activate entries made ready by providers from this round.
        loop {
            let ready = self
                .entries
                .iter()
                .filter(|(_, entry)| {
                    entry.enabled
                        && matches!(entry.state, PluginState::Installed | PluginState::Pending)
                        && entry.dependencies_ready(&self.services)
                })
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            if ready.is_empty() {
                break;
            }
            let mut changed = false;
            for id in ready {
                let provides = self
                    .entries
                    .get(&id)
                    .map(|entry| entry.provides.clone())
                    .unwrap_or_default();
                for service in provides {
                    self.services.provide(service, id.clone());
                }
                if let Some(entry) = self.entries.get_mut(&id) {
                    entry.snapshot_dependencies(&self.services);
                    entry.state = PluginState::Active;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        for entry in self.entries.values_mut() {
            if entry.enabled
                && !entry.dependencies_ready(&self.services)
                && entry.state != PluginState::Active
            {
                entry.state = PluginState::Pending;
            }
        }
    }

    pub fn replace_provider_generation(&mut self, service: &str, plugin_id: &str) {
        self.services
            .provide(service.to_string(), plugin_id.to_string());
        self.reconcile();
    }
}

fn npm_builtin_is_host_provided(package: &str) -> bool {
    matches!(
        package,
        "@deepseek-ai/cordis" | "@deepseek-ai/dsh-tools" | "@deepseek-ai/dsh-llm"
    )
}

fn npm_dependency_target(
    requested_name: &str,
    requirement: &str,
) -> Result<(String, String), RuntimeError> {
    validate_npm_package(requested_name)?;
    let requirement = requirement.trim();
    if let Some(alias) = requirement.strip_prefix("npm:") {
        let split = if alias.starts_with('@') {
            let slash = alias
                .find('/')
                .ok_or_else(|| RuntimeError::InvalidSource("invalid npm alias".into()))?;
            alias[slash + 1..].rfind('@').map(|index| slash + 1 + index)
        } else {
            alias.rfind('@')
        };
        let (package, version) = match split {
            Some(index) if index > 0 => (&alias[..index], &alias[index + 1..]),
            _ => (alias, "latest"),
        };
        validate_npm_package(package)?;
        if version.trim().is_empty() {
            return Err(RuntimeError::InvalidSource(
                "npm alias has empty version".into(),
            ));
        }
        return Ok((package.to_string(), version.to_string()));
    }
    if requirement.starts_with("file:")
        || requirement.starts_with("git:")
        || requirement.starts_with("git+")
        || requirement.starts_with("workspace:")
        || requirement.starts_with("http:")
        || requirement.starts_with("https:")
    {
        return Err(RuntimeError::InvalidSource(format!(
            "npm dependency source is not portable: {requirement}"
        )));
    }
    Ok((requested_name.to_string(), requirement.to_string()))
}

fn npm_dependency_destination(root: &Path, package: &str) -> Result<PathBuf, RuntimeError> {
    validate_npm_package(package)?;
    let mut destination = root.join("node_modules");
    for segment in package.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") {
            return Err(RuntimeError::InvalidSource(format!(
                "invalid npm package path: {package}"
            )));
        }
        destination.push(segment);
    }
    Ok(destination)
}

fn select_npm_version(metadata: &NpmFullPackageMetadata, requirement: &str) -> Option<String> {
    let requirement = requirement.trim();
    if (requirement.is_empty() || requirement == "*" || requirement == "latest")
        && let Some(version) = metadata.dist_tags.get("latest")
    {
        return Some(version.clone());
    }
    if let Some(version) = metadata.dist_tags.get(requirement) {
        return Some(version.clone());
    }
    if metadata.versions.contains_key(requirement) {
        return Some(requirement.to_string());
    }
    let normalized = requirement
        .split("||")
        .map(str::trim)
        .map(|branch| branch.split_whitespace().collect::<Vec<_>>().join(", "))
        .collect::<Vec<_>>();
    let requirements = normalized
        .iter()
        .filter_map(|branch| VersionReq::parse(branch).ok())
        .collect::<Vec<_>>();
    metadata
        .versions
        .keys()
        .filter_map(|raw| Version::parse(raw).ok().map(|version| (raw, version)))
        .filter(|(_, version)| requirements.iter().any(|req| req.matches(version)))
        .max_by(|(_, left), (_, right)| left.cmp(right))
        .map(|(raw, _)| raw.clone())
}

fn verify_npm_integrity(
    bytes: &[u8],
    integrity: Option<&str>,
    shasum: Option<&str>,
) -> Result<(), RuntimeError> {
    if let Some(integrity) = integrity {
        let mut supported = false;
        for token in integrity.split_whitespace() {
            if let Some(value) = token.strip_prefix("sha512-") {
                supported = true;
                let expected = base64::engine::general_purpose::STANDARD
                    .decode(value)
                    .map_err(|error| RuntimeError::InvalidSource(error.to_string()))?;
                if expected.as_slice() == Sha512::digest(bytes).as_slice() {
                    return Ok(());
                }
            } else if let Some(value) = token.strip_prefix("sha256-") {
                supported = true;
                let expected = base64::engine::general_purpose::STANDARD
                    .decode(value)
                    .map_err(|error| RuntimeError::InvalidSource(error.to_string()))?;
                if expected.as_slice() == Sha256::digest(bytes).as_slice() {
                    return Ok(());
                }
            }
        }
        if supported {
            return Err(RuntimeError::NpmIntegrityMismatch);
        }
    }
    if let Some(shasum) = shasum {
        let actual = format!("{:x}", Sha1::digest(bytes));
        if actual.eq_ignore_ascii_case(shasum.trim()) {
            return Ok(());
        }
        return Err(RuntimeError::NpmIntegrityMismatch);
    }
    Err(RuntimeError::InvalidSource(
        "npm package metadata has no supported integrity digest".into(),
    ))
}

fn normalize_npm_package_root(root: &Path) -> Result<(), RuntimeError> {
    let package = root.join("package");
    if !package.is_dir() || root.join("package.json").is_file() {
        return Ok(());
    }
    let entries = fs::read_dir(&package)?.collect::<Result<Vec<_>, _>>()?;
    for entry in entries {
        let target = root.join(entry.file_name());
        if target.exists() {
            return Err(RuntimeError::Archive(format!(
                "npm package root collision: {}",
                target.display()
            )));
        }
        fs::rename(entry.path(), target)?;
    }
    fs::remove_dir(&package)?;
    Ok(())
}

fn extract_artifact(
    bytes: &[u8],
    format: &ArtifactFormat,
    destination: &Path,
) -> Result<(), RuntimeError> {
    match format {
        ArtifactFormat::TarGz => {
            let decoder = GzDecoder::new(Cursor::new(bytes));
            let mut archive = Archive::new(decoder);
            for entry in archive.entries().map_err(RuntimeError::Io)? {
                let mut entry = entry.map_err(RuntimeError::Io)?;
                let entry_type = entry.header().entry_type();
                if !entry_type.is_file() && !entry_type.is_dir() {
                    return Err(RuntimeError::Archive(
                        "archive links and special files are not allowed".into(),
                    ));
                }
                let path = entry.path().map_err(RuntimeError::Io)?.into_owned();
                let safe = safe_archive_path(&path)?;
                let target = destination.join(safe);
                if entry_type.is_dir() {
                    fs::create_dir_all(&target).map_err(RuntimeError::Io)?;
                } else {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent).map_err(RuntimeError::Io)?;
                    }
                    entry.unpack(&target).map_err(RuntimeError::Io)?;
                }
            }
        }
        ArtifactFormat::Zip => {
            let mut archive = ZipArchive::new(Cursor::new(bytes))
                .map_err(|error| RuntimeError::Archive(error.to_string()))?;
            for index in 0..archive.len() {
                let mut entry = archive
                    .by_index(index)
                    .map_err(|error| RuntimeError::Archive(error.to_string()))?;
                let path = entry
                    .enclosed_name()
                    .ok_or_else(|| RuntimeError::Archive("unsafe zip path".into()))?
                    .to_path_buf();
                let target = destination.join(path);
                if entry
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000)
                {
                    return Err(RuntimeError::Archive("zip symlinks are not allowed".into()));
                }
                if entry.is_dir() {
                    fs::create_dir_all(&target).map_err(RuntimeError::Io)?;
                } else {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent).map_err(RuntimeError::Io)?;
                    }
                    let mut output = File::create(&target).map_err(RuntimeError::Io)?;
                    io::copy(&mut entry, &mut output).map_err(RuntimeError::Io)?;
                }
            }
        }
        ArtifactFormat::Directory => {
            return Err(RuntimeError::Archive(
                "directory artifacts cannot be installed from network bytes".into(),
            ));
        }
    }
    Ok(())
}

fn verify_bytes(
    bytes: &[u8],
    expected_sha256: &str,
    expected_size: u64,
) -> Result<(), RuntimeError> {
    if bytes.len() as u64 != expected_size {
        return Err(RuntimeError::SizeMismatch);
    }
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(RuntimeError::DigestMismatch);
    }
    Ok(())
}

fn compare_install_versions(candidate: &str, current: &str) -> Ordering {
    fn normalize(value: &str) -> &str {
        let trimmed = value.trim();
        trimmed.strip_prefix('v').unwrap_or(trimmed)
    }
    let candidate = normalize(candidate);
    let current = normalize(current);
    match (Version::parse(candidate), Version::parse(current)) {
        (Ok(candidate), Ok(current)) => candidate.cmp(&current),
        _ => {
            let tokenize = |value: &str| {
                value
                    .split(['.', '+', '-'])
                    .map(|part| part.parse::<u64>().unwrap_or(0))
                    .collect::<Vec<_>>()
            };
            let candidate_parts = tokenize(candidate);
            let current_parts = tokenize(current);
            for index in 0..candidate_parts.len().max(current_parts.len()) {
                let left = candidate_parts.get(index).copied().unwrap_or_default();
                let right = current_parts.get(index).copied().unwrap_or_default();
                if left != right {
                    return left.cmp(&right);
                }
            }
            match (candidate.contains('-'), current.contains('-')) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => Ordering::Equal,
            }
        }
    }
}

fn validate_source(source: &ArtifactSource) -> Result<(), RuntimeError> {
    match source {
        ArtifactSource::Https { url } => {
            parse_public_https(url)?;
        }
        ArtifactSource::GithubRelease {
            repository,
            tag,
            asset,
        } => {
            validate_github_repository(repository)?;
            validate_path_segment(tag, "GitHub release tag")?;
            validate_path_segment(asset, "GitHub release asset")?;
        }
        ArtifactSource::Npm {
            package,
            version,
            registry,
        } => {
            validate_npm_package(package)?;
            validate_path_segment(version, "npm version")?;
            parse_public_https(registry)?;
        }
    }
    Ok(())
}

fn validate_marketplace_install_contract(
    release: &ExternalReleaseManifest,
    install: &serde_json::Value,
) -> Result<(), RuntimeError> {
    let object = install.as_object().ok_or_else(|| {
        RuntimeError::InvalidRelease("marketplace install contract must be an object".into())
    })?;
    if object.get("protocol").and_then(serde_json::Value::as_str)
        != Some("fabushi.marketplace.install.v1")
        || object.get("strategy").and_then(serde_json::Value::as_str) != Some("github-immutable")
        || object.get("pluginId").and_then(serde_json::Value::as_str)
            != Some(release.plugin_id.as_str())
        || object.get("version").and_then(serde_json::Value::as_str)
            != Some(release.version.as_str())
    {
        return Err(RuntimeError::InvalidRelease(
            "marketplace install contract identity or strategy is invalid".into(),
        ));
    }

    let source = object
        .get("source")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            RuntimeError::InvalidRelease("marketplace install contract has no source".into())
        })?;
    let repository = source
        .get("repository")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            RuntimeError::InvalidRelease("marketplace source has no GitHub repository".into())
        })?;
    validate_github_repository_url(repository)?;
    let _source_ref = source
        .get("sourceRef")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| {
            RuntimeError::InvalidRelease(
                "marketplace sourceRef must be a 40-character Git commit".into(),
            )
        })?;
    if source
        .get("marketplaceHostsPackage")
        .and_then(serde_json::Value::as_bool)
        != Some(false)
    {
        return Err(RuntimeError::InvalidRelease(
            "marketplace must not host executable package bytes".into(),
        ));
    }

    let contract_artifacts = object
        .get("artifacts")
        .and_then(serde_json::Value::as_array)
        .filter(|artifacts| !artifacts.is_empty())
        .ok_or_else(|| {
            RuntimeError::InvalidRelease("marketplace install contract has no artifacts".into())
        })?;
    if contract_artifacts.len() != release.artifacts.len()
        || release
            .artifacts
            .iter()
            .any(|artifact| !is_github_artifact_source(&artifact.source))
    {
        return Err(RuntimeError::InvalidRelease(
            "marketplace artifacts must be GitHub-hosted and match the release".into(),
        ));
    }
    for (artifact, contract) in release.artifacts.iter().zip(contract_artifacts) {
        let contract_object = contract.as_object().ok_or_else(|| {
            RuntimeError::InvalidRelease("marketplace artifact contract is invalid".into())
        })?;
        let same_id = contract_object
            .get("id")
            .and_then(serde_json::Value::as_str)
            == Some(artifact.id.as_str());
        let same_digest = contract_object
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|digest| digest.eq_ignore_ascii_case(&artifact.sha256));
        let same_size = contract_object
            .get("size")
            .and_then(serde_json::Value::as_u64)
            == Some(artifact.size);
        if !(same_id && same_digest && same_size) {
            return Err(RuntimeError::InvalidRelease(
                "marketplace install artifact does not match release artifact".into(),
            ));
        }
    }
    let allow_downgrade = object
        .get("update")
        .and_then(serde_json::Value::as_object)
        .and_then(|update| update.get("allowDowngrade"))
        .and_then(serde_json::Value::as_bool);
    if allow_downgrade != Some(false) {
        return Err(RuntimeError::InvalidRelease(
            "marketplace updates must reject downgrade".into(),
        ));
    }
    Ok(())
}

fn validate_github_repository_url(value: &str) -> Result<(), RuntimeError> {
    let url =
        Url::parse(value.trim()).map_err(|error| RuntimeError::InvalidSource(error.to_string()))?;
    if url.scheme() != "https"
        || !url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RuntimeError::InvalidSource(
            "marketplace repository must be a public GitHub URL".into(),
        ));
    }
    let path = url.path().trim_matches('/');
    let mut parts = path.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if parts.next().is_some() || owner.is_empty() || repo.is_empty() {
        return Err(RuntimeError::InvalidSource(
            "marketplace repository must be https://github.com/owner/repo".into(),
        ));
    }
    validate_github_repository(&format!("{owner}/{repo}"))
}

fn is_github_artifact_source(source: &ArtifactSource) -> bool {
    match source {
        ArtifactSource::Https { url } => Url::parse(url.trim()).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some_and(|host| {
                    host.eq_ignore_ascii_case("github.com")
                        || host.eq_ignore_ascii_case("raw.githubusercontent.com")
                })
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        }),
        ArtifactSource::GithubRelease { .. } => true,
        ArtifactSource::Npm { .. } => false,
    }
}

fn validate_identifier(value: &str, kind: &'static str) -> Result<(), RuntimeError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(RuntimeError::InvalidIdentifier {
            kind,
            value: value.to_string(),
        })
    }
}

fn validate_npm_package(value: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.len() > 214
        || value.contains(char::is_whitespace)
        || value.contains("..")
        || value.contains('\\')
    {
        return Err(RuntimeError::InvalidSource(
            "invalid npm package name".into(),
        ));
    }
    if value.starts_with('@') {
        let Some((scope, name)) = value.split_once('/') else {
            return Err(RuntimeError::InvalidSource(
                "invalid scoped npm package".into(),
            ));
        };
        if scope.len() < 2 || name.is_empty() || name.contains('/') {
            return Err(RuntimeError::InvalidSource(
                "invalid scoped npm package".into(),
            ));
        }
    } else if value.contains('/') {
        return Err(RuntimeError::InvalidSource(
            "invalid npm package name".into(),
        ));
    }
    Ok(())
}

fn validate_github_repository(value: &str) -> Result<(), RuntimeError> {
    let Some((owner, repo)) = value.split_once('/') else {
        return Err(RuntimeError::InvalidSource(
            "GitHub repository must be owner/repo".into(),
        ));
    };
    if owner.is_empty()
        || repo.is_empty()
        || repo.contains('/')
        || !owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || !repo
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(RuntimeError::InvalidSource(
            "invalid GitHub repository".into(),
        ));
    }
    Ok(())
}

fn validate_path_segment(value: &str, kind: &'static str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.len() > 255
        || value.contains(['/', '\\', '\r', '\n'])
        || value == "."
        || value == ".."
    {
        Err(RuntimeError::InvalidSource(format!("invalid {kind}")))
    } else {
        Ok(())
    }
}

fn parse_public_https(value: &str) -> Result<Url, RuntimeError> {
    let url = Url::parse(value).map_err(|error| RuntimeError::InvalidSource(error.to_string()))?;
    if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty() {
        return Err(RuntimeError::InvalidSource(
            "artifact URLs must be public HTTPS URLs".into(),
        ));
    }
    Ok(url)
}

fn safe_relative_path(value: &str) -> Result<PathBuf, RuntimeError> {
    let path = Path::new(value.trim_start_matches("./"));
    safe_archive_path(path)
}

fn safe_archive_path(path: &Path) -> Result<PathBuf, RuntimeError> {
    if path.as_os_str().is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(RuntimeError::Archive("unsafe archive path".into()));
    }
    Ok(path.to_path_buf())
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionStore {
    #[serde(default)]
    grants: BTreeMap<String, BTreeSet<String>>,
}

/// Persistent least-privilege grants for installed plugins.
///
/// Granting is allowed only for permissions declared by the plugin release;
/// callers can therefore expose this API to platform shells without allowing
/// those shells to invent new plugin privileges.
pub struct PermissionManager {
    path: PathBuf,
    store: PermissionStore,
}

impl PermissionManager {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, RuntimeError> {
        let path = path.into();
        let store = if path.is_file() {
            let source = fs::read_to_string(&path)?;
            serde_json::from_str(&source).map_err(|error| {
                RuntimeError::Io(io::Error::new(io::ErrorKind::InvalidData, error))
            })?
        } else {
            PermissionStore::default()
        };
        Ok(Self { path, store })
    }

    pub fn grants_for(&self, plugin_id: &str) -> Vec<String> {
        self.store
            .grants
            .get(plugin_id)
            .map(|grants| grants.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn is_granted(&self, plugin_id: &str, permission: &str) -> bool {
        self.store
            .grants
            .get(plugin_id)
            .is_some_and(|grants| grants.contains(permission))
    }

    pub fn grant(
        &mut self,
        plugin_id: &str,
        requested: &[String],
        permission: &str,
    ) -> Result<(), RuntimeError> {
        self.grant_with_commit_guard(plugin_id, requested, permission, || true)
    }

    pub fn grant_with_commit_guard<F>(
        &mut self,
        plugin_id: &str,
        requested: &[String],
        permission: &str,
        commit_guard: F,
    ) -> Result<(), RuntimeError>
    where
        F: Fn() -> bool,
    {
        if !requested.iter().any(|candidate| candidate == permission) {
            return Err(RuntimeError::PermissionNotRequested {
                plugin_id: plugin_id.to_string(),
                permission: permission.to_string(),
            });
        }
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        self.store
            .grants
            .entry(plugin_id.to_string())
            .or_default()
            .insert(permission.to_string());
        self.persist_with_commit_guard(commit_guard)
    }

    pub fn revoke(&mut self, plugin_id: &str, permission: &str) -> Result<(), RuntimeError> {
        self.revoke_with_commit_guard(plugin_id, permission, || true)
    }

    pub fn revoke_with_commit_guard<F>(
        &mut self,
        plugin_id: &str,
        permission: &str,
        commit_guard: F,
    ) -> Result<(), RuntimeError>
    where
        F: Fn() -> bool,
    {
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        let remove_plugin = if let Some(grants) = self.store.grants.get_mut(plugin_id) {
            grants.remove(permission);
            grants.is_empty()
        } else {
            false
        };
        if remove_plugin {
            self.store.grants.remove(plugin_id);
        }
        self.persist_with_commit_guard(commit_guard)
    }

    pub fn remove_plugin(&mut self, plugin_id: &str) -> Result<bool, RuntimeError> {
        self.remove_plugin_with_commit_guard(plugin_id, || true)
    }

    pub fn remove_plugin_with_commit_guard<F>(
        &mut self,
        plugin_id: &str,
        commit_guard: F,
    ) -> Result<bool, RuntimeError>
    where
        F: Fn() -> bool,
    {
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        let removed = self.store.grants.remove(plugin_id).is_some();
        self.persist_with_commit_guard(commit_guard)?;
        Ok(removed)
    }

    pub fn retain_requested(
        &mut self,
        plugin_id: &str,
        requested: &[String],
    ) -> Result<(), RuntimeError> {
        self.retain_requested_with_commit_guard(plugin_id, requested, || true)
    }

    pub fn retain_requested_with_commit_guard<F>(
        &mut self,
        plugin_id: &str,
        requested: &[String],
        commit_guard: F,
    ) -> Result<(), RuntimeError>
    where
        F: Fn() -> bool,
    {
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        let requested = requested.iter().cloned().collect::<BTreeSet<_>>();
        let remove_plugin = if let Some(grants) = self.store.grants.get_mut(plugin_id) {
            grants.retain(|permission| requested.contains(permission));
            grants.is_empty()
        } else {
            false
        };
        if remove_plugin {
            self.store.grants.remove(plugin_id);
        }
        self.persist_with_commit_guard(commit_guard)
    }

    fn persist(&self) -> Result<(), RuntimeError> {
        self.persist_with_commit_guard(|| true)
    }

    fn persist_with_commit_guard<F>(&self, commit_guard: F) -> Result<(), RuntimeError>
    where
        F: Fn() -> bool,
    {
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(&self.store)
            .map_err(|error| RuntimeError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
        let temporary = self.path.with_extension(format!("tmp-{}", Uuid::new_v4()));
        if !commit_guard() {
            return Err(RuntimeError::StaleAccountContext);
        }
        fs::write(&temporary, bytes)?;
        if !commit_guard() {
            let _ = fs::remove_file(&temporary);
            return Err(RuntimeError::StaleAccountContext);
        }
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("unsupported marketplace release protocol")]
    UnsupportedReleaseProtocol,
    #[error("invalid release: {0}")]
    InvalidRelease(String),
    #[error("invalid {kind}: {value}")]
    InvalidIdentifier { kind: &'static str, value: String },
    #[error("invalid artifact source: {0}")]
    InvalidSource(String),
    #[error("no compatible artifact for platform {0}")]
    NoArtifactForPlatform(String),
    #[error("artifact exceeds configured size limit")]
    ArtifactTooLarge,
    #[error("artifact size mismatch")]
    SizeMismatch,
    #[error("artifact SHA-256 mismatch")]
    DigestMismatch,
    #[error("plugin operation belongs to a stale account context")]
    StaleAccountContext,
    #[error("invalid .fabapp manifest: {0}")]
    FabappManifest(String),
    #[error(".fabapp archive rejected: {0}")]
    FabappArchive(String),
    #[error(".fabapp policy rejected package: {0}")]
    FabappPolicy(String),
    #[error(".fabapp trust binding rejected package: {0}")]
    FabappTrust(String),
    #[error(".fabapp Ed25519 signature verification failed")]
    FabappSignature,
    #[error(".fabapp installation journal failed: {0}")]
    FabappInstall(String),
    #[error("local-generated approval was replayed or already consumed")]
    FabappApprovalReplay,
    #[error("refusing to downgrade plugin from {current} to {candidate}")]
    DowngradeBlocked { current: String, candidate: String },
    #[error("npm package integrity mismatch")]
    NpmIntegrityMismatch,
    #[error("npm dependency limit exceeded: {0}")]
    NpmDependencyLimit(String),
    #[error("invalid npm package manifest at {path}: {message}")]
    InvalidNpmPackage { path: PathBuf, message: String },
    #[error("no npm version satisfies {package}@{requirement}")]
    NpmVersionNotFound {
        package: String,
        requirement: String,
    },
    #[error("plugin {plugin_id} did not request permission {permission}")]
    PermissionNotRequested {
        plugin_id: String,
        permission: String,
    },
    #[error("artifact entry is missing: {0}")]
    MissingEntry(String),
    #[error("plugin effect scope is already disposed")]
    EffectScopeDisposed,
    #[error("HTTP status {0}")]
    HttpStatus(u16),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("archive error: {0}")]
    Archive(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;
    use flate2::{Compression, write::GzEncoder};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, contents) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *contents).unwrap();
        }
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap()
    }


    fn local_fabapp_manifest(
        signing: &ed25519_dalek::SigningKey,
        account_scope: &str,
        app_id: &str,
        approval_id: &str,
        transaction_id: &str,
        files: &[(&str, &[u8], &str)],
    ) -> FabappManifest {
        let mut file_table = BTreeMap::new();
        for (path, bytes, mime) in files {
            file_table.insert(
                (*path).to_string(),
                FabappFileRecord {
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                    bytes: bytes.len() as u64,
                    mime: (*mime).to_string(),
                },
            );
        }
        let mut manifest = FabappManifest {
            manifest_version: 1,
            package_class: Some(FabappPackageClass::LocalGenerated),
            app_id: app_id.to_string(),
            publisher_id: "local.fabushi".into(),
            version: "1.0.0".into(),
            display_name: "Private App".into(),
            entrypoint: "index.html".into(),
            runtime: "web".into(),
            min_host_api: 1,
            offline_core: true,
            requested_capabilities: Vec::new(),
            network_policy: FabappNetworkPolicy {
                mode: "deny".into(),
                allowed_origins: Vec::new(),
            },
            files: file_table,
            local_author: Some(FabappLocalAuthorBinding {
                account_scope: account_scope.into(),
                draft_digest: "0".repeat(64),
                user_approval_id: approval_id.into(),
                creation_transaction_id: transaction_id.into(),
            }),
            signature: FabappSignature {
                algorithm: "Ed25519".into(),
                key_id: "device-local-key-1".into(),
                value: String::new(),
            },
        };
        sign_local_fabapp_manifest(signing, &mut manifest);
        manifest
    }

    fn sign_local_fabapp_manifest(
        signing: &ed25519_dalek::SigningKey,
        manifest: &mut FabappManifest,
    ) {
        let draft = compute_fabapp_draft_digest(manifest).unwrap();
        manifest.local_author.as_mut().unwrap().draft_digest = draft;
        let mut unsigned = serde_json::to_value(&*manifest).unwrap();
        unsigned.as_object_mut().unwrap().remove("signature");
        let canonical = jcs_canonicalize(&unsigned).unwrap();
        let mut message = FABAPP_SIGNATURE_DOMAIN.to_vec();
        message.extend_from_slice(canonical.as_bytes());
        use ed25519_dalek::Signer as _;
        let signature = signing.sign(&message);
        manifest.signature.value =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes());
    }

    fn fabapp_zip(manifest: &FabappManifest, files: &[(&str, &[u8])]) -> Vec<u8> {
        use zip::write::SimpleFileOptions;
        let cursor = Cursor::new(Vec::<u8>::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        writer.start_file("manifest.json", options).unwrap();
        writer
            .write_all(&serde_json::to_vec(manifest).unwrap())
            .unwrap();
        for (path, bytes) in files {
            writer.start_file(*path, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn local_trust(
        signing: &ed25519_dalek::SigningKey,
        manifest: &FabappManifest,
    ) -> LocalGeneratedFabappTrust {
        let binding = manifest.local_author.as_ref().unwrap();
        LocalGeneratedFabappTrust {
            account_scope: binding.account_scope.clone(),
            publisher_id: manifest.publisher_id.clone(),
            reserved_app_id: manifest.app_id.clone(),
            approved_draft_digest: binding.draft_digest.clone(),
            user_approval_id: binding.user_approval_id.clone(),
            creation_transaction_id: binding.creation_transaction_id.clone(),
            key_id: manifest.signature.key_id.clone(),
            public_key: signing.verifying_key().to_bytes(),
        }
    }

    #[test]
    fn fabapp_local_generated_verifier_binds_account_draft_and_approval() {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[7_u8; 32]);
        let scope = format!("{:x}", Sha256::digest(b"account-a"));
        let html = b"<!doctype html><title>Offline</title>";
        let manifest = local_fabapp_manifest(
            &signing,
            &scope,
            "private.fabushi.app-1",
            "approval-1",
            "transaction-1",
            &[("index.html", html, "text/html")],
        );
        let package = fabapp_zip(&manifest, &[("index.html", html)]);
        let trust = local_trust(&signing, &manifest);
        let verified =
            verify_fabapp_bytes(&package, &FabappTrustContext::LocalGenerated(trust.clone()))
                .unwrap();
        assert_eq!(verified.manifest.app_id, "private.fabushi.app-1");

        let mut wrong_account = trust.clone();
        wrong_account.account_scope = format!("{:x}", Sha256::digest(b"account-b"));
        assert!(matches!(
            verify_fabapp_bytes(
                &package,
                &FabappTrustContext::LocalGenerated(wrong_account)
            ),
            Err(RuntimeError::FabappTrust(_))
        ));

        let mut wrong_approval = trust.clone();
        wrong_approval.user_approval_id = "approval-2".into();
        assert!(matches!(
            verify_fabapp_bytes(
                &package,
                &FabappTrustContext::LocalGenerated(wrong_approval)
            ),
            Err(RuntimeError::FabappTrust(_))
        ));

        let catalog = CatalogFabappTrust {
            publisher_id: manifest.publisher_id.clone(),
            app_id: manifest.app_id.clone(),
            key_id: manifest.signature.key_id.clone(),
            public_key: signing.verifying_key().to_bytes(),
        };
        assert!(matches!(
            verify_fabapp_bytes(&package, &FabappTrustContext::Catalog(catalog)),
            Err(RuntimeError::FabappTrust(_))
        ));
    }

    #[test]
    fn fabapp_local_generated_rejects_post_approval_mutation_and_privileged_egress() {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[9_u8; 32]);
        let scope = format!("{:x}", Sha256::digest(b"account-a"));
        let html = b"<h1>approved</h1>";
        let manifest = local_fabapp_manifest(
            &signing,
            &scope,
            "private.fabushi.app-2",
            "approval-2",
            "transaction-2",
            &[("index.html", html, "text/html")],
        );
        let trust = local_trust(&signing, &manifest);
        let mutated = fabapp_zip(&manifest, &[("index.html", b"<h1>mutated</h1>")]);
        assert!(matches!(
            verify_fabapp_bytes(
                &mutated,
                &FabappTrustContext::LocalGenerated(trust.clone())
            ),
            Err(RuntimeError::FabappPolicy(_))
        ));

        let mut privileged = manifest.clone();
        privileged.requested_capabilities = vec!["host.shell".into()];
        let draft = compute_fabapp_draft_digest(&privileged).unwrap();
        privileged.local_author.as_mut().unwrap().draft_digest = draft.clone();
        let mut privileged_trust = trust.clone();
        privileged_trust.approved_draft_digest = draft;
        let mut unsigned = serde_json::to_value(&privileged).unwrap();
        unsigned.as_object_mut().unwrap().remove("signature");
        let canonical = jcs_canonicalize(&unsigned).unwrap();
        let mut message = FABAPP_SIGNATURE_DOMAIN.to_vec();
        message.extend_from_slice(canonical.as_bytes());
        use ed25519_dalek::Signer as _;
        privileged.signature.value =
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(signing.sign(&message).to_bytes());
        let privileged_package = fabapp_zip(&privileged, &[("index.html", html)]);
        assert!(matches!(
            verify_fabapp_bytes(
                &privileged_package,
                &FabappTrustContext::LocalGenerated(privileged_trust)
            ),
            Err(RuntimeError::FabappPolicy(_))
        ));
    }

    #[test]
    fn fabapp_install_is_content_addressed_atomic_and_consumes_approval_once() {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[11_u8; 32]);
        let scope = format!("{:x}", Sha256::digest(b"account-a"));
        let html = b"<h1>offline</h1>";
        let manifest = local_fabapp_manifest(
            &signing,
            &scope,
            "private.fabushi.app-3",
            "approval-3",
            "transaction-3",
            &[("index.html", html, "text/html")],
        );
        let trust = local_trust(&signing, &manifest);
        let package = fabapp_zip(&manifest, &[("index.html", html)]);
        let root = tempfile::tempdir().unwrap();
        let installer = PluginInstaller::new(root.path()).unwrap();
        let pointer = installer
            .install_fabapp_bytes(
                &package,
                &FabappTrustContext::LocalGenerated(trust.clone()),
            )
            .unwrap();
        assert!(Path::new(&pointer.installed_path).join("index.html").is_file());
        assert_eq!(installer.active(&manifest.app_id).unwrap(), Some(pointer));
        assert!(matches!(
            installer.install_fabapp_bytes(
                &package,
                &FabappTrustContext::LocalGenerated(trust)
            ),
            Err(RuntimeError::FabappApprovalReplay)
        ));
    }

    #[test]
    fn fabapp_install_survives_cold_installer_recreation() {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[13_u8; 32]);
        let scope = format!("{:x}", Sha256::digest(b"account-cold-relaunch"));
        let html = b"<h1>cold relaunch</h1>";
        let manifest = local_fabapp_manifest(
            &signing,
            &scope,
            "private.fabushi.cold-relaunch",
            "approval-cold",
            "transaction-cold",
            &[("index.html", html, "text/html")],
        );
        let trust = local_trust(&signing, &manifest);
        let package = fabapp_zip(&manifest, &[("index.html", html)]);
        let root = tempfile::tempdir().unwrap();

        let pointer = PluginInstaller::new(root.path())
            .unwrap()
            .install_fabapp_bytes(
                &package,
                &FabappTrustContext::LocalGenerated(trust),
            )
            .unwrap();

        let relaunched = PluginInstaller::new(root.path()).unwrap();
        assert_eq!(
            relaunched.active(&manifest.app_id).unwrap(),
            Some(pointer.clone())
        );
        assert_eq!(
            fs::read(Path::new(&pointer.installed_path).join("index.html")).unwrap(),
            html
        );
    }

    #[test]
    fn fabapp_install_recovers_interrupted_staging_before_next_install() {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[14_u8; 32]);
        let scope = format!("{:x}", Sha256::digest(b"account-recovery"));
        let html = b"<h1>recovered</h1>";
        let manifest = local_fabapp_manifest(
            &signing,
            &scope,
            "private.fabushi.recovery",
            "approval-recovery",
            "transaction-recovery",
            &[("index.html", html, "text/html")],
        );
        let trust = local_trust(&signing, &manifest);
        let package = fabapp_zip(&manifest, &[("index.html", html)]);
        let root = tempfile::tempdir().unwrap();
        let plugin_root = root.path().join(&manifest.app_id);
        let stale_relative = ".fabapp-staging/interrupted";
        let stale_staging = plugin_root.join(stale_relative);
        fs::create_dir_all(&stale_staging).unwrap();
        fs::write(stale_staging.join("partial.bin"), b"partial").unwrap();
        runtime_atomic_write_json(
            &plugin_root.join("install-journal.json"),
            &FabappInstallJournal {
                schema_version: 1,
                transaction_id: "interrupted-transaction".into(),
                package_sha256: "0".repeat(64),
                state: "staged".into(),
                staging_relative: Some(stale_relative.into()),
            },
        )
        .unwrap();

        let installer = PluginInstaller::new(root.path()).unwrap();
        let pointer = installer
            .install_fabapp_bytes(
                &package,
                &FabappTrustContext::LocalGenerated(trust),
            )
            .unwrap();

        assert!(!stale_staging.exists());
        assert_eq!(installer.active(&manifest.app_id).unwrap(), Some(pointer));
        let journal: FabappInstallJournal = serde_json::from_slice(
            &fs::read(plugin_root.join("install-journal.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(journal.state, "completed");
        assert!(journal.staging_relative.is_none());
    }

    #[test]
    fn fabapp_local_generated_upgrade_keeps_previous_and_rolls_back() {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[15_u8; 32]);
        let scope = format!("{:x}", Sha256::digest(b"account-upgrade"));
        let app_id = "private.fabushi.upgrade";
        let html_v1 = b"<h1>v1</h1>";
        let manifest_v1 = local_fabapp_manifest(
            &signing,
            &scope,
            app_id,
            "approval-v1",
            "transaction-v1",
            &[("index.html", html_v1, "text/html")],
        );
        let package_v1 = fabapp_zip(&manifest_v1, &[("index.html", html_v1)]);
        let trust_v1 = local_trust(&signing, &manifest_v1);

        let html_v2 = b"<h1>v2</h1>";
        let mut manifest_v2 = local_fabapp_manifest(
            &signing,
            &scope,
            app_id,
            "approval-v2",
            "transaction-v2",
            &[("index.html", html_v2, "text/html")],
        );
        manifest_v2.version = "2.0.0".into();
        sign_local_fabapp_manifest(&signing, &mut manifest_v2);
        let package_v2 = fabapp_zip(&manifest_v2, &[("index.html", html_v2)]);
        let trust_v2 = local_trust(&signing, &manifest_v2);

        let root = tempfile::tempdir().unwrap();
        let installer = PluginInstaller::new(root.path()).unwrap();
        let pointer_v1 = installer
            .install_fabapp_bytes(
                &package_v1,
                &FabappTrustContext::LocalGenerated(trust_v1),
            )
            .unwrap();
        let pointer_v2 = installer
            .install_fabapp_bytes(
                &package_v2,
                &FabappTrustContext::LocalGenerated(trust_v2),
            )
            .unwrap();
        assert_eq!(pointer_v2.version, "2.0.0");
        assert_eq!(installer.active(app_id).unwrap(), Some(pointer_v2));

        let rolled_back = installer.rollback(app_id).unwrap().unwrap();
        assert_eq!(rolled_back, pointer_v1);
        assert_eq!(installer.active(app_id).unwrap(), Some(pointer_v1));
    }

    #[test]
    fn external_installer_commit_guard_prevents_stale_account_write() {
        let archive = tar_gz(&[("plugin.json", br#"{"ok":true}"#)]);
        let digest = format!("{:x}", Sha256::digest(&archive));
        let artifact = ReleaseArtifact {
            id: "universal".into(),
            runtime: "local-web".into(),
            platforms: vec!["ios".into()],
            source: ArtifactSource::Https {
                url: "https://example.com/plugin.tar.gz".into(),
            },
            sha256: digest,
            size: archive.len() as u64,
            format: ArtifactFormat::TarGz,
            entry: Some("plugin.json".into()),
        };
        let release = ExternalReleaseManifest {
            schema_version: 1,
            protocol: "mahayana.external-release.v1".into(),
            plugin_id: "stale-account-plugin".into(),
            version: "1.0.0".into(),
            permissions: Vec::new(),
            artifacts: vec![artifact.clone()],
            fabapp_trust: None,
            install: None,
        };
        let root = tempfile::tempdir().unwrap();
        let installer = PluginInstaller::new(root.path()).unwrap();
        assert!(matches!(
            installer.install_verified_bytes_with_commit_guard(
                &release,
                &artifact,
                &archive,
                || false
            ),
            Err(RuntimeError::StaleAccountContext)
        ));
        assert!(!root.path().join("stale-account-plugin").exists());
    }

    #[test]
    fn uninstall_commit_guard_prevents_stale_account_write() {
        let root = tempfile::tempdir().unwrap();
        let plugin_root = root.path().join("stale-uninstall");
        fs::create_dir_all(plugin_root.join("versions/1.0.0/test")).unwrap();
        let installer = PluginInstaller::new(root.path()).unwrap();

        assert!(matches!(
            installer.uninstall_with_commit_guard("stale-uninstall", || false),
            Err(RuntimeError::StaleAccountContext)
        ));
        assert!(plugin_root.exists());
    }

    #[test]
    fn remove_code_removes_install_ledger_and_retains_app_data_and_permissions() {
        let root = tempfile::tempdir().unwrap();
        let plugin_root = root.path().join("code-only-plugin");
        fs::create_dir_all(plugin_root.join("versions/1.0.0/digest")).unwrap();
        fs::create_dir_all(plugin_root.join(".staging/interrupted")).unwrap();
        fs::create_dir_all(plugin_root.join(".fabapp-manifests")).unwrap();
        fs::create_dir_all(plugin_root.join("approvals")).unwrap();
        fs::write(plugin_root.join("active.json"), b"{}").unwrap();
        fs::write(plugin_root.join("previous-active.json"), b"{}").unwrap();
        fs::write(plugin_root.join("install-journal.json"), b"{}").unwrap();
        fs::write(plugin_root.join(".fabapp-manifests/digest.json"), b"{}").unwrap();
        fs::write(plugin_root.join("approvals/approval.json"), b"{}").unwrap();

        let storage_path = plugin_root.join("storage/state.json");
        fs::create_dir_all(storage_path.parent().unwrap()).unwrap();
        fs::write(&storage_path, b"durable-account-data").unwrap();
        let app_data_path = plugin_root.join("app-data.json");
        fs::write(&app_data_path, b"preserve-me").unwrap();

        let permission_path = root.path().join("permissions.json");
        let mut permissions = PermissionManager::load(&permission_path).unwrap();
        permissions
            .grant(
                "code-only-plugin",
                &["storage.local".to_string()],
                "storage.local",
            )
            .unwrap();
        drop(permissions);

        let installer = PluginInstaller::new(root.path()).unwrap();
        assert!(installer.remove_code("code-only-plugin").unwrap());
        assert!(plugin_root.is_dir());
        assert!(!plugin_root.join("versions").exists());
        assert!(!plugin_root.join("active.json").exists());
        assert!(!plugin_root.join("previous-active.json").exists());
        assert!(!plugin_root.join("install-journal.json").exists());
        assert!(!plugin_root.join(".staging").exists());
        assert!(!plugin_root.join(".fabapp-manifests").exists());
        assert!(!plugin_root.join("approvals").exists());
        assert_eq!(fs::read(&storage_path).unwrap(), b"durable-account-data");
        assert_eq!(fs::read(&app_data_path).unwrap(), b"preserve-me");
        assert!(permission_path.is_file());

        let permissions = PermissionManager::load(&permission_path).unwrap();
        assert_eq!(
            permissions.grants_for("code-only-plugin"),
            vec!["storage.local".to_string()]
        );
        assert!(!installer.remove_code("code-only-plugin").unwrap());
    }

    #[test]
    fn remove_code_is_distinct_from_full_uninstall() {
        let root = tempfile::tempdir().unwrap();
        let plugin_root = root.path().join("code-only-vs-uninstall");
        fs::create_dir_all(plugin_root.join("versions/1.0.0/digest")).unwrap();
        let storage_path = plugin_root.join("storage/state.json");
        fs::create_dir_all(storage_path.parent().unwrap()).unwrap();
        fs::write(&storage_path, b"preserve-on-code-removal").unwrap();

        let installer = PluginInstaller::new(root.path()).unwrap();
        assert!(installer.remove_code("code-only-vs-uninstall").unwrap());
        assert_eq!(
            fs::read(&storage_path).unwrap(),
            b"preserve-on-code-removal"
        );

        assert!(installer.uninstall("code-only-vs-uninstall").unwrap());
        assert!(!plugin_root.exists());
    }

    #[test]
    fn remove_code_commit_guard_prevents_stale_account_write() {
        let root = tempfile::tempdir().unwrap();
        let plugin_root = root.path().join("stale-code-removal");
        let code_path = plugin_root.join("versions/1.0.0/test");
        let storage_path = plugin_root.join("storage/state.json");
        fs::create_dir_all(&code_path).unwrap();
        fs::create_dir_all(storage_path.parent().unwrap()).unwrap();
        fs::write(&storage_path, b"durable").unwrap();
        let installer = PluginInstaller::new(root.path()).unwrap();

        assert!(matches!(
            installer.remove_code_with_commit_guard("stale-code-removal", || false),
            Err(RuntimeError::StaleAccountContext)
        ));
        assert!(code_path.is_dir());
        assert_eq!(fs::read(&storage_path).unwrap(), b"durable");
    }

    #[test]
    fn rollback_commit_guard_prevents_stale_account_pointer_switch() {
        let root = tempfile::tempdir().unwrap();
        let plugin_root = root.path().join("stale-rollback");
        let current_dir = plugin_root.join("versions/2.0.0/current");
        let previous_dir = plugin_root.join("versions/1.0.0/previous");
        fs::create_dir_all(&current_dir).unwrap();
        fs::create_dir_all(&previous_dir).unwrap();
        let current = InstalledPluginPointer {
            plugin_id: "stale-rollback".into(),
            version: "2.0.0".into(),
            artifact_id: "current".into(),
            artifact_sha256: "a".repeat(64),
            runtime: "local-web".into(),
            entry: Some("index.html".into()),
            requested_permissions: vec![],
            installed_path: current_dir.to_string_lossy().into_owned(),
        };
        let previous = InstalledPluginPointer {
            plugin_id: "stale-rollback".into(),
            version: "1.0.0".into(),
            artifact_id: "previous".into(),
            artifact_sha256: "b".repeat(64),
            runtime: "local-web".into(),
            entry: Some("index.html".into()),
            requested_permissions: vec![],
            installed_path: previous_dir.to_string_lossy().into_owned(),
        };
        runtime_atomic_write_json(&plugin_root.join("active.json"), &current).unwrap();
        runtime_atomic_write_json(&plugin_root.join("previous-active.json"), &previous).unwrap();
        let installer = PluginInstaller::new(root.path()).unwrap();

        assert!(matches!(
            installer.rollback_with_commit_guard("stale-rollback", || false),
            Err(RuntimeError::StaleAccountContext)
        ));
        assert_eq!(installer.active("stale-rollback").unwrap(), Some(current));
    }

    #[test]
    fn service_dependency_waits_then_activates() {
        let mut supervisor = PluginSupervisor::default();
        supervisor.insert(PluginEntry::new(
            "consumer",
            vec![],
            vec![ServiceRequirement {
                service: "storage".into(),
                required: true,
            }],
        ));
        supervisor.reconcile();
        assert_eq!(supervisor.entries["consumer"].state, PluginState::Pending);

        supervisor.insert(PluginEntry::new("provider", vec!["storage".into()], vec![]));
        supervisor.reconcile();
        assert_eq!(supervisor.entries["provider"].state, PluginState::Active);
        assert_eq!(supervisor.entries["consumer"].state, PluginState::Active);
    }

    #[test]
    fn provider_generation_change_reloads_dependency_epoch() {
        let mut supervisor = PluginSupervisor::default();
        supervisor.insert(PluginEntry::new("provider", vec!["storage".into()], vec![]));
        supervisor.insert(PluginEntry::new(
            "consumer",
            vec![],
            vec![ServiceRequirement {
                service: "storage".into(),
                required: true,
            }],
        ));
        supervisor.reconcile();
        let before = supervisor.entries["consumer"].observed_generations["storage"];
        supervisor.replace_provider_generation("storage", "provider");
        let after = supervisor.entries["consumer"].observed_generations["storage"];
        assert!(after > before);
        assert_eq!(supervisor.entries["consumer"].state, PluginState::Active);
    }

    #[test]
    fn installs_verified_archive_and_switches_active_pointer() {
        let archive = tar_gz(&[("plugin.json", br#"{"ok":true}"#)]);
        let digest = format!("{:x}", Sha256::digest(&archive));
        let artifact = ReleaseArtifact {
            id: "mobile-web".into(),
            runtime: "local-web".into(),
            platforms: vec!["ios".into(), "android".into()],
            source: ArtifactSource::Https {
                url: "https://example.com/plugin.tar.gz".into(),
            },
            sha256: digest.clone(),
            size: archive.len() as u64,
            format: ArtifactFormat::TarGz,
            entry: Some("./plugin.json".into()),
        };
        let release = ExternalReleaseManifest {
            schema_version: 1,
            protocol: "mahayana.external-release.v1".into(),
            plugin_id: "global-dharma".into(),
            version: "1.0.0".into(),
            permissions: vec!["network".into()],
            artifacts: vec![artifact.clone()],
            fabapp_trust: None,
            install: None,
        };
        let temp = tempfile::tempdir().unwrap();
        let installer = PluginInstaller::new(temp.path()).unwrap();
        let pointer = installer
            .install_verified_bytes(&release, &artifact, &archive)
            .unwrap();
        assert_eq!(pointer.runtime, "local-web");
        assert!(
            Path::new(&pointer.installed_path)
                .join("plugin.json")
                .is_file()
        );
        assert_eq!(installer.active("global-dharma").unwrap(), Some(pointer));
    }

    #[test]
    fn updates_are_monotonic_and_keep_an_explicit_previous_active_pointer() {
        let archive = tar_gz(&[("plugin.json", br#"{"ok":true}"#)]);
        let digest = format!("{:x}", Sha256::digest(&archive));
        let artifact = ReleaseArtifact {
            id: "universal".into(),
            runtime: "local-web".into(),
            platforms: vec!["desktop".into(), "chrome-extension".into()],
            source: ArtifactSource::Https {
                url: "https://example.com/plugin.tar.gz".into(),
            },
            sha256: digest,
            size: archive.len() as u64,
            format: ArtifactFormat::TarGz,
            entry: Some("plugin.json".into()),
        };
        let release_v1 = ExternalReleaseManifest {
            schema_version: 1,
            protocol: "mahayana.external-release.v1".into(),
            plugin_id: "monotonic-plugin".into(),
            version: "1.0.0".into(),
            permissions: Vec::new(),
            artifacts: vec![artifact.clone()],
            fabapp_trust: None,
            install: None,
        };
        let release_v2 = ExternalReleaseManifest {
            version: "2.0.0".into(),
            ..release_v1.clone()
        };
        let release_v15 = ExternalReleaseManifest {
            version: "1.5.0".into(),
            ..release_v1.clone()
        };
        let temp = tempfile::tempdir().unwrap();
        let installer = PluginInstaller::new(temp.path()).unwrap();
        installer
            .install_verified_bytes(&release_v1, &artifact, &archive)
            .unwrap();
        installer
            .install_verified_bytes(&release_v2, &artifact, &archive)
            .unwrap();
        assert_eq!(
            installer
                .active("monotonic-plugin")
                .unwrap()
                .unwrap()
                .version,
            "2.0.0"
        );
        assert!(matches!(
            installer.install_verified_bytes(&release_v15, &artifact, &archive),
            Err(RuntimeError::DowngradeBlocked { .. })
        ));
        assert_eq!(
            installer
                .rollback("monotonic-plugin")
                .unwrap()
                .unwrap()
                .version,
            "1.0.0"
        );
        assert_eq!(
            installer
                .active("monotonic-plugin")
                .unwrap()
                .unwrap()
                .version,
            "1.0.0"
        );
    }

    #[test]
    fn marketplace_install_contract_is_github_pinned_and_matches_artifacts() {
        let release: ExternalReleaseManifest = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "protocol": "mahayana.external-release.v1",
            "pluginId": "github-plugin",
            "version": "1.0.0",
            "permissions": [],
            "artifacts": [{
                "id": "universal",
                "runtime": "local-web",
                "platforms": ["desktop", "cli"],
                "source": {
                    "type": "https",
                    "url": "https://raw.githubusercontent.com/example/repository/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/extension.tar.gz"
                },
                "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "size": 4,
                "format": "tar-gz",
                "entry": "index.html"
            }],
            "install": {
                "protocol": "fabushi.marketplace.install.v1",
                "strategy": "github-immutable",
                "pluginId": "github-plugin",
                "version": "1.0.0",
                "source": {
                    "repository": "https://github.com/example/repository",
                    "sourceRef": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "marketplaceHostsPackage": false
                },
                "artifacts": [{
                    "id": "universal",
                    "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "size": 4
                }],
                "update": {"allowDowngrade": false}
            }
        }))
        .unwrap();
        assert!(release.validate().is_ok());
    }

    #[test]
    fn marketplace_install_contract_rejects_non_github_artifacts() {
        let mut release: ExternalReleaseManifest = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "protocol": "mahayana.external-release.v1",
            "pluginId": "github-plugin",
            "version": "1.0.0",
            "permissions": [],
            "artifacts": [{
                "id": "universal",
                "runtime": "local-web",
                "platforms": ["desktop"],
                "source": {"type": "https", "url": "https://raw.githubusercontent.com/example/repository/a/archive.tar.gz"},
                "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "size": 4,
                "format": "tar-gz",
                "entry": "index.html"
            }],
            "install": {
                "protocol": "fabushi.marketplace.install.v1",
                "strategy": "github-immutable",
                "pluginId": "github-plugin",
                "version": "1.0.0",
                "source": {
                    "repository": "https://github.com/example/repository",
                    "sourceRef": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "marketplaceHostsPackage": false
                },
                "artifacts": [{
                    "id": "universal",
                    "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "size": 4
                }],
                "update": {"allowDowngrade": false}
            }
        }))
        .unwrap();
        release.artifacts[0].source = ArtifactSource::Https {
            url: "https://example.com/archive.tar.gz".into(),
        };
        assert!(release.validate().is_err());
    }

    #[test]
    fn uninstall_removes_plugin_root_and_is_idempotent() {
        let archive = tar_gz(&[("plugin.json", br#"{"ok":true}"#)]);
        let digest = format!("{:x}", Sha256::digest(&archive));
        let artifact = ReleaseArtifact {
            id: "desktop-web".into(),
            runtime: "local-web".into(),
            platforms: vec!["linux".into()],
            source: ArtifactSource::Https {
                url: "https://example.com/plugin.tar.gz".into(),
            },
            sha256: digest,
            size: archive.len() as u64,
            format: ArtifactFormat::TarGz,
            entry: Some("./plugin.json".into()),
        };
        let release = ExternalReleaseManifest {
            schema_version: 1,
            protocol: "mahayana.external-release.v1".into(),
            plugin_id: "cleanup-plugin".into(),
            version: "1.0.0".into(),
            permissions: vec!["network".into()],
            artifacts: vec![artifact.clone()],
            fabapp_trust: None,
            install: None,
        };
        let temp = tempfile::tempdir().unwrap();
        let installer = PluginInstaller::new(temp.path()).unwrap();
        installer
            .install_verified_bytes(&release, &artifact, &archive)
            .unwrap();
        assert!(temp.path().join("cleanup-plugin/active.json").is_file());
        assert!(installer.uninstall("cleanup-plugin").unwrap());
        assert!(!temp.path().join("cleanup-plugin").exists());
        assert!(!installer.uninstall("cleanup-plugin").unwrap());
    }

    #[test]
    fn local_generated_builder_binds_exact_resources_and_signing_payload() {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[31_u8; 32]);
        let mut resources = BTreeMap::from([
            ("index.html".to_string(), ("text/html".to_string(), b"<html>safe</html>".to_vec())),
            ("app.js".to_string(), ("text/javascript".to_string(), b"window.ok=true".to_vec())),
        ]);
        let mut manifest = prepare_local_generated_fabapp_manifest(
            "local.fabushi.a1234.app",
            "local.fabushi.a1234",
            "1.0.0",
            "Generated App",
            "index.html",
            vec!["storage.local".into()],
            &resources,
        )
        .unwrap();
        let draft = compute_fabapp_draft_digest(&manifest).unwrap();
        manifest.local_author = Some(FabappLocalAuthorBinding {
            account_scope: "a".repeat(64),
            draft_digest: draft.clone(),
            user_approval_id: "approval-1".into(),
            creation_transaction_id: "transaction-1".into(),
        });
        manifest.signature.key_id = "local-key".into();
        let payload = fabapp_signing_payload(&manifest).unwrap();
        manifest.signature.value = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing.sign(&payload).to_bytes());
        let package = build_fabapp_package(&manifest, &resources).unwrap();
        let verified = verify_fabapp_bytes(
            &package,
            &FabappTrustContext::LocalGenerated(LocalGeneratedFabappTrust {
                account_scope: "a".repeat(64),
                publisher_id: "local.fabushi.a1234".into(),
                reserved_app_id: "local.fabushi.a1234.app".into(),
                approved_draft_digest: draft,
                user_approval_id: "approval-1".into(),
                creation_transaction_id: "transaction-1".into(),
                key_id: "local-key".into(),
                public_key: signing.verifying_key().to_bytes(),
            }),
        )
        .unwrap();
        assert_eq!(verified.manifest.app_id, "local.fabushi.a1234.app");

        resources.get_mut("app.js").unwrap().1.push(b'!');
        assert!(matches!(
            build_fabapp_package(&manifest, &resources),
            Err(RuntimeError::FabappPolicy(_))
        ));
    }

    #[test]
    fn fabapp_install_commit_guard_rejects_stale_account_before_writes() {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[29_u8; 32]);
        let html = b"<html><body>guard</body></html>";
        let mut manifest = FabappManifest {
            manifest_version: 1,
            package_class: Some(FabappPackageClass::Catalog),
            app_id: "com.example.guard".into(),
            publisher_id: "com.example".into(),
            version: "1.0.0".into(),
            display_name: "Guard".into(),
            entrypoint: "index.html".into(),
            runtime: "web".into(),
            min_host_api: FABAPP_HOST_API_V1,
            offline_core: true,
            requested_capabilities: Vec::new(),
            network_policy: FabappNetworkPolicy {
                mode: "deny".into(),
                allowed_origins: Vec::new(),
            },
            files: BTreeMap::from([(
                "index.html".into(),
                FabappFileRecord {
                    sha256: format!("{:x}", Sha256::digest(html)),
                    bytes: html.len() as u64,
                    mime: "text/html".into(),
                },
            )]),
            local_author: None,
            signature: FabappSignature {
                algorithm: "Ed25519".into(),
                key_id: "catalog-key".into(),
                value: String::new(),
            },
        };
        manifest.signature.value = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing.sign(&fabapp_signing_payload(&manifest).unwrap()).to_bytes());
        let package = fabapp_zip(&manifest, &[("index.html", html)]);
        let trust = FabappTrustContext::Catalog(CatalogFabappTrust {
            publisher_id: "com.example".into(),
            app_id: "com.example.guard".into(),
            key_id: "catalog-key".into(),
            public_key: signing.verifying_key().to_bytes(),
        });
        let temp = tempfile::tempdir().unwrap();
        let installer = PluginInstaller::new(temp.path()).unwrap();

        let result = installer.install_fabapp_bytes_with_commit_guard(
            &package,
            &trust,
            || false,
        );
        assert!(matches!(result, Err(RuntimeError::StaleAccountContext)));
        assert!(!temp.path().join("com.example.guard").exists());
    }

    #[test]
    fn catalog_fabapp_release_uses_strict_verifier_before_activation() {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[41_u8; 32]);
        let html = b"<html><body>Global Dharma offline</body></html>";
        let capabilities = vec!["offline.state.read".to_string()];
        let mut manifest = FabappManifest {
            manifest_version: 1,
            package_class: Some(FabappPackageClass::Catalog),
            app_id: "global-dharma".into(),
            publisher_id: "fabushi-official".into(),
            version: "1.1.0".into(),
            display_name: "Global Dharma".into(),
            entrypoint: "index.html".into(),
            runtime: "web".into(),
            min_host_api: FABAPP_HOST_API_V1,
            offline_core: true,
            requested_capabilities: capabilities.clone(),
            network_policy: FabappNetworkPolicy {
                mode: "deny".into(),
                allowed_origins: Vec::new(),
            },
            files: BTreeMap::from([(
                "index.html".into(),
                FabappFileRecord {
                    sha256: format!("{:x}", Sha256::digest(html)),
                    bytes: html.len() as u64,
                    mime: "text/html".into(),
                },
            )]),
            local_author: None,
            signature: FabappSignature {
                algorithm: "Ed25519".into(),
                key_id: "catalog-key".into(),
                value: String::new(),
            },
        };
        manifest.signature.value = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing.sign(&fabapp_signing_payload(&manifest).unwrap()).to_bytes());
        let package = fabapp_zip(&manifest, &[("index.html", html)]);
        let digest = format!("{:x}", Sha256::digest(&package));
        let artifact = ReleaseArtifact {
            id: "global-dharma-fabapp".into(),
            runtime: "local-web".into(),
            platforms: vec!["ios".into(), "mobile".into()],
            source: ArtifactSource::Https {
                url: "https://github.com/example/global-dharma/releases/download/v1.1.0/global-dharma-1.1.0.fabapp".into(),
            },
            sha256: digest.clone(),
            size: package.len() as u64,
            format: ArtifactFormat::Zip,
            entry: Some("index.html".into()),
        };
        let binding = CatalogFabappTrustBinding {
            publisher_id: "fabushi-official".into(),
            app_id: "global-dharma".into(),
            artifact_id: artifact.id.clone(),
            key_id: "catalog-key".into(),
            raw_public_key_base64_url: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(signing.verifying_key().to_bytes()),
        };
        let release = ExternalReleaseManifest {
            schema_version: 1,
            protocol: "mahayana.external-release.v1".into(),
            plugin_id: "global-dharma".into(),
            version: "1.1.0".into(),
            permissions: capabilities,
            artifacts: vec![artifact.clone()],
            fabapp_trust: Some(binding.clone()),
            install: None,
        };
        let root = tempfile::tempdir().unwrap();
        let installer = PluginInstaller::new(root.path()).unwrap();
        let pointer = installer
            .install_verified_bytes(&release, &artifact, &package)
            .unwrap();
        assert_eq!(pointer.plugin_id, "global-dharma");
        assert_eq!(pointer.version, "1.1.0");
        assert_eq!(pointer.artifact_sha256, digest);
        assert!(pointer.artifact_id.starts_with("fabapp-"));
        assert!(Path::new(&pointer.installed_path).join("index.html").is_file());

        let mismatch_root = tempfile::tempdir().unwrap();
        let mismatch_installer = PluginInstaller::new(mismatch_root.path()).unwrap();
        let mut mismatched_release = release.clone();
        mismatched_release.version = "1.1.1".into();
        assert!(matches!(
            mismatch_installer.install_verified_bytes(
                &mismatched_release,
                &artifact,
                &package
            ),
            Err(RuntimeError::InvalidRelease(_))
        ));
        assert!(!mismatch_root.path().join("global-dharma").exists());

        let wrong_key_root = tempfile::tempdir().unwrap();
        let wrong_key_installer = PluginInstaller::new(wrong_key_root.path()).unwrap();
        let wrong_signing = ed25519_dalek::SigningKey::from_bytes(&[42_u8; 32]);
        let mut wrong_key_release = release.clone();
        wrong_key_release.fabapp_trust = Some(CatalogFabappTrustBinding {
            raw_public_key_base64_url: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(wrong_signing.verifying_key().to_bytes()),
            ..binding
        });
        assert!(matches!(
            wrong_key_installer.install_verified_bytes(
                &wrong_key_release,
                &artifact,
                &package
            ),
            Err(RuntimeError::FabappSignature)
        ));
        assert!(!wrong_key_root.path().join("global-dharma").exists());
    }

    #[test]
    fn identical_plugin_ids_are_fully_isolated_across_account_roots() {
        fn release(version: &str, artifact: &ReleaseArtifact) -> ExternalReleaseManifest {
            ExternalReleaseManifest {
                schema_version: 1,
                protocol: "mahayana.external-release.v1".into(),
                plugin_id: "same-plugin".into(),
                version: version.into(),
                permissions: vec!["storage.local".into()],
                artifacts: vec![artifact.clone()],
                fabapp_trust: None,
                install: None,
            }
        }

        let archive = tar_gz(&[("index.html", b"<h1>offline</h1>")]);
        let artifact = ReleaseArtifact {
            id: "ios-web".into(),
            runtime: "local-web".into(),
            platforms: vec!["ios".into()],
            source: ArtifactSource::Https {
                url: "https://example.com/plugin.tar.gz".into(),
            },
            sha256: format!("{:x}", Sha256::digest(&archive)),
            size: archive.len() as u64,
            format: ArtifactFormat::TarGz,
            entry: Some("index.html".into()),
        };

        let temp = tempfile::tempdir().unwrap();
        let account_a = temp.path().join("accounts/account-a");
        let account_b = temp.path().join("accounts/account-b");
        let installer_a = PluginInstaller::new(&account_a).unwrap();
        let installer_b = PluginInstaller::new(&account_b).unwrap();

        let a_v1 = installer_a
            .install_verified_bytes(&release("1.0.0", &artifact), &artifact, &archive)
            .unwrap();
        let b_v1 = installer_b
            .install_verified_bytes(&release("1.0.0", &artifact), &artifact, &archive)
            .unwrap();
        assert_ne!(a_v1.installed_path, b_v1.installed_path);
        assert_eq!(installer_a.active("same-plugin").unwrap(), Some(a_v1.clone()));
        assert_eq!(installer_b.active("same-plugin").unwrap(), Some(b_v1.clone()));

        let mut permissions_a =
            PermissionManager::load(account_a.join("permissions.json")).unwrap();
        permissions_a
            .grant("same-plugin", &["storage.local".into()], "storage.local")
            .unwrap();
        let permissions_b =
            PermissionManager::load(account_b.join("permissions.json")).unwrap();
        assert!(permissions_b.grants_for("same-plugin").is_empty());

        let storage_a = account_a.join("same-plugin/storage/state.json");
        let storage_b = account_b.join("same-plugin/storage/state.json");
        fs::create_dir_all(storage_a.parent().unwrap()).unwrap();
        fs::create_dir_all(storage_b.parent().unwrap()).unwrap();
        fs::write(&storage_a, b"account-a").unwrap();
        fs::write(&storage_b, b"account-b").unwrap();
        assert_eq!(fs::read(&storage_a).unwrap(), b"account-a");
        assert_eq!(fs::read(&storage_b).unwrap(), b"account-b");

        let b_v2 = installer_b
            .install_verified_bytes(&release("2.0.0", &artifact), &artifact, &archive)
            .unwrap();
        assert_eq!(installer_b.active("same-plugin").unwrap(), Some(b_v2));
        let rolled_back_b = installer_b.rollback("same-plugin").unwrap().unwrap();
        assert_eq!(rolled_back_b.version, "1.0.0");
        assert_eq!(installer_a.active("same-plugin").unwrap(), Some(a_v1));

        assert!(installer_a.uninstall("same-plugin").unwrap());
        assert!(installer_a.active("same-plugin").unwrap().is_none());
        assert_eq!(installer_b.active("same-plugin").unwrap(), Some(b_v1));
        assert_eq!(fs::read(&storage_b).unwrap(), b"account-b");
    }

    #[test]
    fn permission_commit_guard_rejects_stale_account_before_file_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("permissions.json");
        let requested = vec!["storage.local".to_string()];
        let mut manager = PermissionManager::load(&path).unwrap();

        assert!(matches!(
            manager.grant_with_commit_guard(
                "sample-plugin",
                &requested,
                "storage.local",
                || false
            ),
            Err(RuntimeError::StaleAccountContext)
        ));
        assert!(!path.exists());
        assert!(!manager.is_granted("sample-plugin", "storage.local"));
    }

    #[test]
    fn permission_manager_persists_only_declared_grants() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("permissions.json");
        let requested = vec!["storage.local".to_string(), "network.request".to_string()];
        let mut manager = PermissionManager::load(&path).unwrap();
        manager
            .grant("sample-plugin", &requested, "network.request")
            .unwrap();
        assert!(manager.is_granted("sample-plugin", "network.request"));
        assert!(matches!(
            manager.grant("sample-plugin", &requested, "process.spawn"),
            Err(RuntimeError::PermissionNotRequested { .. })
        ));
        drop(manager);

        let mut manager = PermissionManager::load(&path).unwrap();
        assert_eq!(
            manager.grants_for("sample-plugin"),
            vec!["network.request".to_string()]
        );
        manager
            .retain_requested("sample-plugin", &["storage.local".to_string()])
            .unwrap();
        assert!(manager.grants_for("sample-plugin").is_empty());
    }

    #[test]
    fn permission_manager_removes_all_grants_for_uninstalled_plugin() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("permissions.json");
        let mut manager = PermissionManager::load(&path).unwrap();
        manager
            .grant("cleanup-plugin", &["network".into()], "network")
            .unwrap();
        assert_eq!(
            manager.grants_for("cleanup-plugin"),
            vec!["network".to_string()]
        );
        assert!(manager.remove_plugin("cleanup-plugin").unwrap());
        assert!(manager.grants_for("cleanup-plugin").is_empty());
        assert!(!manager.remove_plugin("cleanup-plugin").unwrap());
    }

    #[test]
    fn npm_semver_alias_and_integrity_helpers_are_deterministic() {
        let metadata = NpmFullPackageMetadata {
            dist_tags: BTreeMap::from([("latest".into(), "2.0.0".into())]),
            versions: BTreeMap::from([
                (
                    "1.2.0".into(),
                    NpmFullVersionMetadata {
                        dist: NpmFullDistMetadata {
                            tarball: "https://registry.npmjs.org/a/-/a-1.2.0.tgz".into(),
                            integrity: None,
                            shasum: None,
                        },
                    },
                ),
                (
                    "1.9.0".into(),
                    NpmFullVersionMetadata {
                        dist: NpmFullDistMetadata {
                            tarball: "https://registry.npmjs.org/a/-/a-1.9.0.tgz".into(),
                            integrity: None,
                            shasum: None,
                        },
                    },
                ),
                (
                    "2.0.0".into(),
                    NpmFullVersionMetadata {
                        dist: NpmFullDistMetadata {
                            tarball: "https://registry.npmjs.org/a/-/a-2.0.0.tgz".into(),
                            integrity: None,
                            shasum: None,
                        },
                    },
                ),
            ]),
        };
        assert_eq!(
            select_npm_version(&metadata, "^1.2.0").as_deref(),
            Some("1.9.0")
        );
        assert_eq!(
            select_npm_version(&metadata, "latest").as_deref(),
            Some("2.0.0")
        );
        assert_eq!(
            npm_dependency_target("alias", "npm:@scope/real@^1.0.0").unwrap(),
            ("@scope/real".into(), "^1.0.0".into())
        );

        let payload = b"verified npm tarball";
        let integrity = format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(payload))
        );
        verify_npm_integrity(payload, Some(&integrity), None).unwrap();
        assert!(matches!(
            verify_npm_integrity(b"tampered", Some(&integrity), None),
            Err(RuntimeError::NpmIntegrityMismatch)
        ));
    }

    #[test]
    fn npm_package_root_is_flattened_without_lifecycle_execution() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("package/lib")).unwrap();
        fs::write(
            root.path().join("package/package.json"),
            r#"{"name":"sample","version":"1.0.0","scripts":{"postinstall":"exit 99"}}"#,
        )
        .unwrap();
        fs::write(root.path().join("package/lib/index.js"), "export default 1").unwrap();
        normalize_npm_package_root(root.path()).unwrap();
        assert!(root.path().join("package.json").is_file());
        assert!(root.path().join("lib/index.js").is_file());
        assert!(!root.path().join("package").exists());
    }

    #[test]
    fn rejects_tar_symlinks_even_when_archive_path_is_relative() {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_path("safe-link").unwrap();
        header.set_link_name("/etc/passwd").unwrap();
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();
        let encoder = builder.into_inner().unwrap();
        let archive_bytes = encoder.finish().unwrap();
        let destination = tempfile::tempdir().unwrap();
        assert!(matches!(
            extract_artifact(&archive_bytes, &ArtifactFormat::TarGz, destination.path()),
            Err(RuntimeError::Archive(_))
        ));
    }

    #[test]
    fn rejects_archive_traversal() {
        let path = Path::new("../escape");
        assert!(safe_archive_path(path).is_err());
    }

    #[test]
    fn effect_scope_disposes_once_in_reverse_registration_order() {
        let count = Arc::new(AtomicUsize::new(0));
        let first = count.clone();
        let second = count.clone();
        let mut scope = EffectScope::default();
        scope
            .register(move || {
                assert_eq!(first.fetch_add(1, Ordering::SeqCst), 1);
            })
            .unwrap();
        scope
            .register(move || {
                assert_eq!(second.fetch_add(1, Ordering::SeqCst), 0);
            })
            .unwrap();
        scope.dispose();
        scope.dispose();
        assert!(scope.is_disposed());
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert!(scope.register(|| {}).is_err());
    }
}
